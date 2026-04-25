use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot;

use crate::backend::AgentOutput;
use crate::runtime::orchestrator::RuntimeOrchestrator;
use crate::team_mode::domain::{InboxStatus, MessageKind};
use crate::team_mode::service::{InboxNotifier, InboxService, MessageService, SendMessageRequest};
use crate::team_mode::storage::{MemberStore, MessageStore};

/// Why the worker's output stream stopped accepting events for the
/// current inbox turn. Drives the step-5 branch that decides whether to
/// post a real `Reply` or a synthesized `[SYSTEM]` Status to keep the
/// lead informed.
#[derive(Debug, Clone, Copy)]
enum TurnEndCause {
    /// `AgentOutput::TurnComplete` or `Idle` — clean end-of-turn signal.
    TurnComplete,
    /// `AgentOutput::Error` — backend reported a turn-level error.
    AgentError,
    /// `output_rx.recv()` returned None — the child's stdout pipe closed
    /// (process died, was killed, or stdin pair was dropped). Caller
    /// must shut down the loop after the terminal message is posted.
    OutputClosed,
}

/// Handle returned by `AgentLoop::start`. Drop or call `shutdown()` to stop the loop.
pub struct AgentLoopHandle {
    pub join_handle: std::thread::JoinHandle<()>,
    pub shutdown_tx: oneshot::Sender<()>,
}

impl std::fmt::Debug for AgentLoopHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentLoopHandle").finish_non_exhaustive()
    }
}

impl AgentLoopHandle {
    /// Signal the loop to stop and wait for it to exit.
    pub fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.join_handle.join();
    }
}

/// Drives a managed member's autonomous work loop:
///   poll inbox → feed to Claude Code → collect reply → post to room → ack
pub struct AgentLoop {
    /// The worker's handle within its team (e.g. "alice"). Used for inbox,
    /// message sender, and ack operations — anything team-scoped.
    pub member_id: String,
    /// The orchestrator's session key (e.g. "diag__alice"). Used when calling
    /// orchestrator APIs like `send_input` that key sessions by spawn_key,
    /// not by team-scoped member name.
    pub session_key: String,
    pub team_id: String,
    pub room_id: String,
    pub orchestrator: Arc<Mutex<RuntimeOrchestrator>>,
    pub inbox_service: InboxService,
    pub message_store: MessageStore,
    pub message_service: MessageService,
    pub poll_interval: Duration,
    pub inbox_notifier: Option<InboxNotifier>,
    /// Used to persist the backend-assigned `session_id` once the worker
    /// emits its first stream-json event (which only happens after the
    /// worker processes its first message — so worker_add can't capture
    /// it at spawn time). Optional to keep backwards-compat with tests
    /// that didn't supply it.
    pub member_store: Option<MemberStore>,
}

impl AgentLoop {
    /// Spawn the loop on a dedicated thread with its own Tokio runtime.
    pub fn start(self, output_rx: Receiver<AgentOutput>) -> AgentLoopHandle {
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let join_handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("agent loop tokio runtime");
            rt.block_on(self.run(output_rx, shutdown_rx));
        });
        AgentLoopHandle {
            join_handle,
            shutdown_tx,
        }
    }

    async fn run(
        self,
        mut output_rx: Receiver<AgentOutput>,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) {
        // Best-effort drain of any pending synthetic ready signal (TurnComplete/Idle
        // emitted by the backend on spawn). The caller's ready-check may have already
        // consumed it — in that case the 100ms timeout fires and we proceed normally.
        let drain_deadline = Duration::from_millis(100);
        match tokio::time::timeout(drain_deadline, output_rx.recv()).await {
            Ok(Some(AgentOutput::Error(_))) | Ok(None) => return,
            Ok(Some(_)) | Err(_) => {}
        }
        tracing::info!(member = %self.member_id, "agent loop ready, polling inbox");

        loop {
            // 1. Poll inbox for unread messages directed at this member.
            let unread = match self
                .inbox_service
                .peek(&self.team_id, &self.member_id, None)
            {
                Ok(inbox) => inbox
                    .items
                    .into_iter()
                    .filter(|item| matches!(item.status, InboxStatus::Unread))
                    .collect::<Vec<_>>(),
                Err(_) => break,
            };
            tracing::debug!(member = %self.member_id, count = unread.len(), "inbox polled");

            for item in unread {
                // 2. Load the full message.
                let message = match self.message_store.get(&self.team_id, &item.message_id) {
                    Ok(Some(m)) => m,
                    _ => continue,
                };

                // Skip messages sent by self — prevents echo loops.
                if message.sender == self.member_id {
                    tracing::debug!(member = %self.member_id, msg_id = %message.id, "skipping self-sent message");
                    let _ = self
                        .inbox_service
                        .ack(&self.team_id, &self.member_id, &[message.id]);
                    continue;
                }

                tracing::info!(member = %self.member_id, msg_id = %message.id, sender = %message.sender, "processing inbox message");

                // 3. Feed message to Claude Code with sender context.
                let input = format!("[Message from {}]: {}", message.sender, message.body);
                {
                    tracing::debug!(
                        member = %self.member_id,
                        session_key = %self.session_key,
                        "sending input to session"
                    );
                    let mut orch = self.orchestrator.lock().await;
                    if let Err(err) = orch.send_input(&self.session_key, &input).await {
                        tracing::error!(
                            member = %self.member_id,
                            session_key = %self.session_key,
                            error = %err,
                            "send_input failed, shutting down agent loop"
                        );
                        // Drop the orchestrator lock before doing anything
                        // that reaches other services, to avoid deadlock.
                        drop(orch);

                        // Notify lead that this worker died mid-message so
                        // the coordinator doesn't wait forever for a reply
                        // that will never come. Without this the inbox
                        // message stays unread and the lead has no signal
                        // that anything went wrong.
                        let notice = format!(
                            "[SYSTEM] worker '{member}' died while processing message \
                             from '{sender}' (msg_id={mid}). Error: {err}. \
                             The message will not be answered. Use worker_add with \
                             on_existing=reuse to restart this worker if you want to \
                             retry.",
                            member = self.member_id,
                            sender = message.sender,
                            mid = message.id,
                        );
                        let _ = self.message_service.send(SendMessageRequest {
                            team_id: self.team_id.clone(),
                            room_id: self.room_id.clone(),
                            sender: self.member_id.clone(),
                            kind: MessageKind::Status,
                            subject: None,
                            body: notice,
                            mentions: Vec::new(),
                            visibility: Vec::new(),
                            audience_policy: None,
                            reply_to: Some(message.id.clone()),
                            thread_id: message.thread_id.clone(),
                            expires_at: None,
                        });
                        // Ack the message so the dead worker's inbox doesn't
                        // keep this entry as unread forever (UX noise in
                        // inbox_read and zombie visibility in projections).
                        let _ = self.inbox_service.ack(
                            &self.team_id,
                            &self.member_id,
                            &[message.id.clone()],
                        );
                        return;
                    }
                }

                // 4. Collect Claude Code's response until the turn ends.
                //
                // We track the END-CAUSE so step 5 can synthesize a [SYSTEM]
                // status whenever the worker produced no text. The whole point
                // is the lead must always get exactly ONE terminal message
                // back per inbox dispatch — Reply when there's content, Status
                // otherwise. Without this guarantee, a worker that finishes a
                // turn silently (LLM ignored the request, content was filtered,
                // process died mid-turn) leaves the lead waiting forever.
                let mut parts: Vec<String> = Vec::new();
                let end_cause: TurnEndCause = loop {
                    match output_rx.recv().await {
                        Some(AgentOutput::Message(text)) | Some(AgentOutput::Delta(text)) => {
                            parts.push(text);
                        }
                        Some(AgentOutput::TurnComplete) | Some(AgentOutput::Idle) => {
                            break TurnEndCause::TurnComplete;
                        }
                        Some(AgentOutput::Error(e)) => {
                            parts.push(format!("[agent error: {e}]"));
                            break TurnEndCause::AgentError;
                        }
                        None => {
                            // stdout pipe closed mid-turn — the child process
                            // is gone (crashed, killed, or stdin/stdout pair
                            // dropped). We still must surface this to the
                            // lead. Fall through to step 5; after sending the
                            // [SYSTEM] notice we will exit the loop because
                            // there's nothing left to listen on.
                            break TurnEndCause::OutputClosed;
                        }
                    }
                };

                // 5. Always post EXACTLY ONE terminal message back to the room.
                //
                // Branching:
                //   - body has visible content (after trim) → `Reply`
                //   - empty body + TurnComplete             → `[SYSTEM]` Status: silent turn
                //   - empty body + AgentError               → `[SYSTEM]` Status: agent error
                //                                              (parts already includes
                //                                              the error string above,
                //                                              so this branch usually
                //                                              has content; included for
                //                                              completeness)
                //   - any cause + OutputClosed              → `[SYSTEM]` Status: pipe closed
                //
                // Rationale: lead coordination relies on every dispatch
                // producing a single observable terminal event. Two events
                // (a Reply AND a "completed" status) would be redundant noise;
                // zero events strands the lead. So we pick exactly one.
                let raw_body = parts.join("");
                let trimmed = raw_body.trim();
                let has_content = !trimmed.is_empty();
                let pipe_closed = matches!(end_cause, TurnEndCause::OutputClosed);

                let (kind, body) = if has_content && !pipe_closed {
                    (MessageKind::Reply, raw_body)
                } else {
                    let notice = match (has_content, end_cause) {
                        (false, TurnEndCause::TurnComplete) => format!(
                            "[SYSTEM] worker '{member}' completed its turn without \
                             producing any reply text for msg {mid}. The worker may \
                             have silently ignored the request or finished without \
                             output. Check the worker's prompt or send a follow-up.",
                            member = self.member_id,
                            mid = message.id
                        ),
                        (true, TurnEndCause::OutputClosed) => format!(
                            "[SYSTEM] worker '{member}' output channel closed \
                             mid-turn while answering msg {mid} (partial output: {n} \
                             chars). The child process likely died. Use \
                             `worker_add name={member} on_existing=reuse` to revive.\n\n\
                             --- partial output ---\n{partial}",
                            member = self.member_id,
                            mid = message.id,
                            n = trimmed.len(),
                            partial = trimmed
                        ),
                        (false, TurnEndCause::OutputClosed) => format!(
                            "[SYSTEM] worker '{member}' output channel closed \
                             before producing any reply for msg {mid}. The child \
                             process died at the start of its turn. Use \
                             `worker_add name={member} on_existing=reuse` to revive.",
                            member = self.member_id,
                            mid = message.id
                        ),
                        (false, TurnEndCause::AgentError) => format!(
                            "[SYSTEM] worker '{member}' raised an agent error while \
                             processing msg {mid} but emitted no message body. See \
                             the worker's stderr / daemon log for details.",
                            member = self.member_id,
                            mid = message.id
                        ),
                        // (true, TurnComplete) or (true, AgentError) hit the
                        // Reply branch above; this match is exhaustive.
                        _ => unreachable!(
                            "terminal-message branch: has_content={has_content} cause={cause:?}",
                            cause = end_cause
                        ),
                    };
                    (MessageKind::Status, notice)
                };

                tracing::info!(
                    member = %self.member_id,
                    msg_id = %message.id,
                    kind = ?kind,
                    body_len = body.len(),
                    end_cause = ?end_cause,
                    "posting terminal message for inbox turn"
                );
                let _ = self.message_service.send(SendMessageRequest {
                    team_id: self.team_id.clone(),
                    room_id: self.room_id.clone(),
                    sender: self.member_id.clone(),
                    kind,
                    subject: None,
                    body,
                    mentions: Vec::new(),
                    visibility: Vec::new(),
                    audience_policy: None,
                    reply_to: Some(message.id.clone()),
                    thread_id: message.thread_id.clone(),
                    expires_at: None,
                });

                // 6. Ack the processed inbox item.
                tracing::debug!(member = %self.member_id, msg_id = %message.id, "ack'd inbox item");
                let _ = self
                    .inbox_service
                    .ack(&self.team_id, &self.member_id, &[message.id]);

                // 6b. If the child's stdout pipe closed, the loop cannot
                // process any further messages — its source of TurnComplete
                // events is gone. Exit cleanly now (the [SYSTEM] notice for
                // this turn was already delivered in step 5).
                if pipe_closed {
                    tracing::info!(
                        member = %self.member_id,
                        "exiting agent loop after output pipe closed"
                    );
                    return;
                }

                // 7. Backfill backend session_id into the member record once
                // the worker has actually started its conversation. The
                // session_id is only emitted after the first user message
                // (claude CLI in stream-json mode stays silent at spawn),
                // so worker_add couldn't persist it — we do it here, lazily,
                // after each turn. Idempotent: only writes if value changed.
                if let Some(store) = &self.member_store {
                    let sid = self.orchestrator.lock().await.session_id_of(&self.session_key);
                    if let Some(sid) = sid {
                        let _ = store.update(&self.team_id, &self.member_id, |m| {
                            if let Some(exec) = m.execution.as_mut() {
                                if exec.session_id.as_deref() != Some(sid.as_str()) {
                                    exec.session_id = Some(sid.clone());
                                }
                            }
                        });
                    }
                }
            }

            // Wait for a push notification or the 30-second fallback timeout.
            tokio::select! {
                // Immediately wake when a new inbox message is available.
                _ = async {
                    if let Some(ref n) = self.inbox_notifier {
                        n.notified().await
                    } else {
                        // No notifier: never completes; fall through to timeout.
                        std::future::pending::<()>().await
                    }
                } => {}
                // 30-second fallback to guard against lost notifications.
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                _ = &mut shutdown_rx => {
                    tracing::info!(member_id = %self.member_id, "agent loop shutting down");
                    return;
                }
            }
        }
        tracing::info!(member_id = %self.member_id, "agent loop shutting down");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::Utc;
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    use super::*;
    use crate::backend::{AgentBackend, AgentOutput, AgentSession, BackendType, SpawnConfig};
    use crate::runtime::orchestrator::RuntimeOrchestrator;
    use crate::team_mode::domain::{
        MemberKind, MemberProfile, MemberStatus, MessageKind, Room, RoomKind, RoomStatus, Team,
        TeamStatus,
    };
    use crate::team_mode::service::{MessageService, SendMessageRequest};
    use crate::team_mode::storage::{
        MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore,
    };
    use crate::{Error, Result};

    // ------------------------------------------------------------------
    // Mock session: records inputs, emits scripted outputs.
    // ------------------------------------------------------------------

    struct MockSession {
        name: String,
        output_rx: Option<mpsc::Receiver<AgentOutput>>,
        input_tx: mpsc::Sender<String>,
    }

    #[async_trait]
    impl AgentSession for MockSession {
        fn name(&self) -> &str {
            &self.name
        }

        async fn send_input(&mut self, input: &str) -> Result<()> {
            let _ = self.input_tx.send(input.to_string()).await;
            Ok(())
        }

        fn output_receiver(&mut self) -> Option<mpsc::Receiver<AgentOutput>> {
            self.output_rx.take()
        }

        async fn is_alive(&self) -> bool {
            true
        }

        async fn shutdown(&mut self) -> Result<()> {
            Ok(())
        }

        async fn force_kill(&mut self) -> Result<()> {
            Ok(())
        }
    }

    // ------------------------------------------------------------------
    // Mock backend: wraps a pre-built MockSession.
    // ------------------------------------------------------------------

    struct MockBackend {
        session: std::sync::Mutex<Option<Box<dyn AgentSession>>>,
    }

    impl MockBackend {
        fn new(session: MockSession) -> Self {
            Self {
                session: std::sync::Mutex::new(Some(Box::new(session))),
            }
        }
    }

    #[async_trait]
    impl AgentBackend for MockBackend {
        fn backend_type(&self) -> BackendType {
            BackendType::ClaudeCode
        }

        async fn spawn(&self, _config: SpawnConfig) -> Result<Box<dyn AgentSession>> {
            self.session
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| Error::Other("MockBackend already consumed".into()))
        }
    }

    // ------------------------------------------------------------------
    // Test helpers
    // ------------------------------------------------------------------

    fn seed_env(base: &std::path::Path) {
        TeamStore::new(base)
            .save(&Team {
                id: "team-1".into(),
                name: "team-1".into(),
                description: None,
                cwd: None,
                status: TeamStatus::Active,
                lead_member_id: Some("lead".into()),
                owner_cc_pid: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .unwrap();

        RoomStore::new(base)
            .save(
                "team-1",
                &Room {
                    id: "main".into(),
                    team_id: Some("team-1".into()),
                    kind: RoomKind::Main,
                    status: RoomStatus::Active,
                },
            )
            .unwrap();

        let store = MemberStore::new(base);
        for (name, kind) in [("lead", MemberKind::Lead), ("worker", MemberKind::Member)] {
            store
                .add(crate::team_mode::storage::MemberRecord {
                    profile: MemberProfile {
                        team_id: "team-1".into(),
                        name: name.into(),
                        kind,
                        role_label: name.into(),
                        role_description: None,
                        status: MemberStatus::Active,
                        joined_at: Utc::now(),
                    },
                    execution: None,
                })
                .unwrap();
        }
    }

    fn build_services(base: &std::path::Path) -> (MessageService, InboxService, MessageStore) {
        let ms = MessageStore::new(base);
        let member = MemberStore::new(base);
        let room = RoomStore::new(base);
        let team = TeamStore::new(base);
        let proj = ProjectionStore::new(base);
        let svc = MessageService::new(ms.clone(), member, room, team);
        let inbox = InboxService::new(proj, ms.clone());
        (svc, inbox, ms)
    }

    // ------------------------------------------------------------------
    // Test: full loop iteration with mock session
    // ------------------------------------------------------------------

    #[test]
    fn agent_loop_processes_inbox_and_posts_reply() {
        let dir = tempdir().unwrap();
        let base = dir.path();
        seed_env(base);

        let (message_service, inbox_service, message_store) = build_services(base);

        // Lead dispatches to worker.
        let dispatch = message_service
            .send(SendMessageRequest {
                team_id: "team-1".into(),
                room_id: "main".into(),
                sender: "lead".into(),
                kind: MessageKind::Dispatch,
                subject: Some("Task".into()),
                body: "Hey @worker please do the thing".into(),
                mentions: Vec::new(),
                visibility: Vec::new(),
                audience_policy: None,
                reply_to: None,
                thread_id: None,
                expires_at: None,
            })
            .unwrap();

        assert_eq!(dispatch.effective_recipients, vec!["worker"]);

        // Verify worker has 1 unread inbox item.
        let before = inbox_service.peek("team-1", "worker", None).unwrap();
        assert_eq!(before.items.len(), 1);
        assert!(matches!(before.items[0].status, InboxStatus::Unread));

        // Build mock session and orchestrator.
        let (output_tx, output_rx) = mpsc::channel::<AgentOutput>(32);
        let (input_tx, mut input_rx) = mpsc::channel::<String>(32);

        let mock = MockSession {
            name: "worker".into(),
            output_rx: None, // we drive output_tx directly; orchestrator gets None
            input_tx: input_tx.clone(),
        };
        let mut orch = RuntimeOrchestrator::new();
        orch.register_backend(MockBackend::new(mock));

        let orch = Arc::new(Mutex::new(orch));

        // Pre-spawn via orchestrator so send_input("worker-1") is routable.
        {
            let orch2 = Arc::clone(&orch);
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                orch2
                    .lock()
                    .await
                    .spawn_managed_member(
                        "worker",
                        "worker",
                        SpawnConfig::new("worker", "test prompt"),
                        BackendType::ClaudeCode,
                    )
                    .await
                    .unwrap();
            });
        }

        // Start the agent loop with our scripted output_rx.
        let agent_loop = AgentLoop {
            member_id: "worker".into(),
            session_key: "worker".into(),
            team_id: "team-1".into(),
            room_id: "main".into(),
            orchestrator: Arc::clone(&orch),
            inbox_service: inbox_service.clone(),
            message_store: message_store.clone(),
            message_service: message_service.clone(),
            poll_interval: Duration::from_millis(50),
            inbox_notifier: None,
            member_store: None,
        };

        // Script: emit TurnComplete (initial prompt drain), then after send_input
        // emit "Done!" + TurnComplete. Driver thread drops output_tx on exit.
        let driver = std::thread::spawn(move || {
            output_tx.blocking_send(AgentOutput::TurnComplete).unwrap();
            let _ = input_rx.blocking_recv().unwrap();
            output_tx
                .blocking_send(AgentOutput::Message("Done!".into()))
                .unwrap();
            output_tx.blocking_send(AgentOutput::TurnComplete).unwrap();
            // output_tx dropped here; further recv() in loop returns None
        });

        let loop_handle = agent_loop.start(output_rx);
        driver.join().unwrap();

        // Give the loop time to finish the iteration and ack the message.
        std::thread::sleep(Duration::from_millis(200));

        // Signal shutdown and wait for the loop thread to exit cleanly.
        loop_handle.shutdown();

        // Worker inbox should now be acked.
        let after = inbox_service.peek("team-1", "worker", None).unwrap();
        assert_eq!(after.items.len(), 1);
        assert!(matches!(after.items[0].status, InboxStatus::Acked));

        // A reply message from worker should exist in the room.
        let messages = message_store.list_by_room("team-1", "main").unwrap();
        let reply = messages
            .iter()
            .find(|m| m.sender == "worker" && matches!(m.kind, MessageKind::Reply));
        assert!(reply.is_some(), "expected a reply from worker in the room");
        assert_eq!(reply.unwrap().body, "Done!");
    }

    /// Helper to wire up an AgentLoop + scripted output channel.
    /// Returns the message_store and inbox_service so the test can assert
    /// what the agent_loop posted.
    fn run_loop_with_script<F>(
        script: F,
    ) -> (MessageStore, InboxService)
    where
        F: FnOnce(mpsc::Sender<AgentOutput>, mpsc::Receiver<String>) + Send + 'static,
    {
        let dir = tempdir().unwrap();
        let base = dir.path().to_path_buf();
        seed_env(&base);

        let (message_service, inbox_service, message_store) = build_services(&base);

        // Lead → worker dispatch.
        message_service
            .send(SendMessageRequest {
                team_id: "team-1".into(),
                room_id: "main".into(),
                sender: "lead".into(),
                kind: MessageKind::Dispatch,
                subject: None,
                body: "@worker do something".into(),
                mentions: Vec::new(),
                visibility: Vec::new(),
                audience_policy: None,
                reply_to: None,
                thread_id: None,
                expires_at: None,
            })
            .unwrap();

        let (output_tx, output_rx) = mpsc::channel::<AgentOutput>(32);
        let (input_tx, input_rx) = mpsc::channel::<String>(32);
        let mock = MockSession {
            name: "worker".into(),
            output_rx: None,
            input_tx,
        };
        let mut orch = RuntimeOrchestrator::new();
        orch.register_backend(MockBackend::new(mock));
        let orch = Arc::new(Mutex::new(orch));

        // Pre-spawn so send_input is routable.
        {
            let orch2 = Arc::clone(&orch);
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                orch2
                    .lock()
                    .await
                    .spawn_managed_member(
                        "worker",
                        "worker",
                        SpawnConfig::new("worker", "test prompt"),
                        BackendType::ClaudeCode,
                    )
                    .await
                    .unwrap();
            });
        }

        let agent_loop = AgentLoop {
            member_id: "worker".into(),
            session_key: "worker".into(),
            team_id: "team-1".into(),
            room_id: "main".into(),
            orchestrator: orch,
            inbox_service: inbox_service.clone(),
            message_store: message_store.clone(),
            message_service,
            poll_interval: Duration::from_millis(50),
            inbox_notifier: None,
            member_store: None,
        };

        let driver = std::thread::spawn(move || script(output_tx, input_rx));
        let loop_handle = agent_loop.start(output_rx);
        driver.join().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        loop_handle.shutdown();

        // Keep dir alive for caller's assertions by leaking — only OK because
        // tempdir auto-deletes when the binding drops. We move the dir into
        // a long-lived handle the caller still holds (the stores).
        std::mem::forget(dir);

        (message_store, inbox_service)
    }

    /// Bug 25 regression: a turn that completes without any output text MUST
    /// still produce a terminal message, otherwise the lead waits forever
    /// for a reply that never comes.
    #[test]
    fn agent_loop_emits_system_status_on_silent_turn() {
        let (message_store, inbox_service) = run_loop_with_script(|tx, mut rx| {
            tx.blocking_send(AgentOutput::TurnComplete).unwrap(); // ready signal
            let _ = rx.blocking_recv().unwrap();
            // Worker produced ZERO output, then signalled TurnComplete.
            tx.blocking_send(AgentOutput::TurnComplete).unwrap();
        });

        let messages = message_store.list_by_room("team-1", "main").unwrap();
        let from_worker: Vec<_> = messages.iter().filter(|m| m.sender == "worker").collect();
        assert_eq!(
            from_worker.len(),
            1,
            "expected exactly one terminal message from worker"
        );
        assert!(
            matches!(from_worker[0].kind, MessageKind::Status),
            "silent turn should produce Status, not Reply: kind={:?}",
            from_worker[0].kind
        );
        assert!(
            from_worker[0].body.contains("[SYSTEM]")
                && from_worker[0]
                    .body
                    .contains("completed its turn without producing any reply text"),
            "unexpected body: {}",
            from_worker[0].body
        );

        // Inbox should still be acked.
        let inbox = inbox_service.peek("team-1", "worker", None).unwrap();
        assert!(matches!(
            inbox.items[0].status,
            crate::team_mode::domain::InboxStatus::Acked
        ));
    }

    /// Bug 25 regression: stdout pipe closing mid-turn (child crashed) MUST
    /// surface a [SYSTEM] notice and stop the loop cleanly.
    #[test]
    fn agent_loop_emits_system_status_on_output_pipe_close() {
        let (message_store, _inbox) = run_loop_with_script(|tx, mut rx| {
            tx.blocking_send(AgentOutput::TurnComplete).unwrap(); // ready signal
            let _ = rx.blocking_recv().unwrap();
            // Worker emitted partial output, then the pipe died.
            tx.blocking_send(AgentOutput::Delta("partial...".into()))
                .unwrap();
            drop(tx); // simulate child stdout pipe close
        });

        let messages = message_store.list_by_room("team-1", "main").unwrap();
        let from_worker: Vec<_> = messages.iter().filter(|m| m.sender == "worker").collect();
        assert_eq!(
            from_worker.len(),
            1,
            "expected exactly one terminal message"
        );
        assert!(matches!(from_worker[0].kind, MessageKind::Status));
        assert!(
            from_worker[0].body.contains("output channel closed"),
            "unexpected body: {}",
            from_worker[0].body
        );
        assert!(
            from_worker[0].body.contains("partial..."),
            "should include the partial output: {}",
            from_worker[0].body
        );
    }
}
