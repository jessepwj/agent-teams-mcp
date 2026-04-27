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
    /// Count messages this worker sent into the team room with a
    /// `created_at` strictly after `since`. Used by step 5 to detect
    /// whether the worker called `send_message` during the turn that
    /// just ended (delta > 0 = explicit reply, delta == 0 = silent).
    ///
    /// Cheap: a single full read of `messages.jsonl` then a linear
    /// filter. Per-turn cost is acceptable for any realistic project
    /// size (a million-message team is hypothetical; in practice we're
    /// in the hundreds-to-thousands range).
    ///
    /// Returns `None` on storage I/O error; callers should treat that
    /// as "unknown" and pick a conservative default rather than
    /// panicking.
    fn count_self_messages_after(
        &self,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Option<usize> {
        let messages = self.message_store.list(&self.team_id).ok()?;
        Some(
            messages
                .into_iter()
                .filter(|m| m.sender == self.member_id && m.created_at > since)
                .count(),
        )
    }

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

        while let Ok(inbox) = self
            .inbox_service
            .peek(&self.team_id, &self.member_id, None)
        {
            // 1. Poll inbox for unread messages directed at this member.
            let unread = inbox
                .items
                .into_iter()
                .filter(|item| matches!(item.status, InboxStatus::Unread))
                .collect::<Vec<_>>();
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
                            std::slice::from_ref(&message.id),
                        );
                        return;
                    }
                }

                // 4. Drain worker output events for this turn. We DELIBERATELY
                // do not accumulate stdout into a reply body — the worker's
                // job is to call `mcp__team-mode__send_message` explicitly.
                // Anything written to stdout (LLM narration, codex shell
                // command output, intermediate token streams) is treated as
                // private working notes — visible in the web UI's session
                // transcript but never copied into messages.jsonl.
                //
                // We still drain events because we need:
                //   * the terminal signal (TurnComplete / Error / pipe close)
                //     to know the turn ended,
                //   * tracing for debugging.
                //
                // History: this loop used to JOIN every Message + Delta event
                // into a body and post it as a Reply (the "auto-capture stdout
                // as reply" path). That design caused Bug 26 (codex command
                // output and double-writes bloating the reply to 50K+ tokens)
                // and was removed in Bug 29. The contract is now strict:
                // explicit MCP call = message; anything else = noise.
                let baseline_sent = self
                    .count_self_messages_after(message.created_at)
                    .unwrap_or(0);
                let mut error_text: Option<String> = None;
                // Active liveness probe — independent background task.
                //
                // Why a separate task and not a recv-coupled timeout:
                // codex on Windows keeps streaming buffered stdout events
                // for up to ~20s after a hard kill. While that buffer is
                // draining, every `recv()` returns Some(...) within
                // milliseconds, so any timeout-on-recv approach never
                // fires. We need a probe that runs in parallel with
                // recv, on its own clock.
                //
                // Implementation: spawn a tokio task that polls
                // `is_alive` every 3s. The first time it sees the
                // process gone, it signals the recv loop via a oneshot
                // channel. The recv loop selects between recv and the
                // probe-dead signal — whichever fires first wins. The
                // task is aborted at end of the turn whether by clean
                // termination or probe.
                let (probe_dead_tx, probe_dead_rx) = tokio::sync::oneshot::channel::<()>();
                let probe_orch = Arc::clone(&self.orchestrator);
                let probe_session_key = self.session_key.clone();
                let probe_member = self.member_id.clone();
                let probe_handle = tokio::spawn(async move {
                    let mut tx = Some(probe_dead_tx);
                    let mut interval = tokio::time::interval(Duration::from_secs(3));
                    interval.set_missed_tick_behavior(
                        tokio::time::MissedTickBehavior::Delay,
                    );
                    // Skip the immediate first tick at t=0; give the
                    // worker at least one probe interval to bring up.
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        let alive = {
                            let orch = probe_orch.lock().await;
                            orch.is_alive(&probe_session_key).await.unwrap_or(true)
                        };
                        if !alive {
                            tracing::warn!(
                                member = %probe_member,
                                session_key = %probe_session_key,
                                "liveness probe: worker process gone, signaling OutputClosed"
                            );
                            if let Some(tx) = tx.take() {
                                let _ = tx.send(());
                            }
                            return;
                        }
                    }
                });

                tokio::pin!(probe_dead_rx);
                let end_cause: TurnEndCause = loop {
                    tokio::select! {
                        result = output_rx.recv() => match result {
                            Some(AgentOutput::Message(_))
                            | Some(AgentOutput::Delta(_))
                            | Some(AgentOutput::ToolOutput(_)) => {
                                // Discarded for body purposes. Web UI
                                // still shows the worker's transcript via
                                // the session JSONL file.
                            }
                            Some(AgentOutput::TurnComplete) | Some(AgentOutput::Idle) => {
                                break TurnEndCause::TurnComplete;
                            }
                            Some(AgentOutput::Error(e)) => {
                                error_text = Some(e);
                                break TurnEndCause::AgentError;
                            }
                            None => {
                                // Pipe closed cleanly — child died and
                                // stdout drained.
                                break TurnEndCause::OutputClosed;
                            }
                        },
                        _ = &mut probe_dead_rx => {
                            // Probe task detected the process is gone.
                            // Stop draining buffered events — they will
                            // be EOF anyway.
                            break TurnEndCause::OutputClosed;
                        }
                    }
                };
                probe_handle.abort();

                // 5. Post a terminal `[SYSTEM]` notice ONLY for failure
                // modes the worker can't surface itself. A clean
                // TurnComplete where the worker explicitly called
                // send_message at least once → no noise from us; the
                // worker has spoken for itself. A clean TurnComplete
                // where the worker said nothing → one Status notice so
                // the lead doesn't wait forever for a silent worker.
                let final_sent = self
                    .count_self_messages_after(message.created_at)
                    .unwrap_or(baseline_sent);
                let worker_spoke_explicitly = final_sent > baseline_sent;
                let pipe_closed = matches!(end_cause, TurnEndCause::OutputClosed);

                let notice: Option<String> = match end_cause {
                    TurnEndCause::OutputClosed => Some(format!(
                        "[SYSTEM] worker '{member}' output channel closed \
                         while answering msg {mid}. The child process \
                         died. Use `worker_add name={member} on_existing=reuse` \
                         to revive (worker loses prior conversation context).",
                        member = self.member_id,
                        mid = message.id
                    )),
                    TurnEndCause::AgentError => Some(format!(
                        "[SYSTEM] worker '{member}' raised an agent error \
                         while processing msg {mid}: {err}. See daemon log \
                         for details.",
                        member = self.member_id,
                        mid = message.id,
                        err = error_text.as_deref().unwrap_or("(no detail)")
                    )),
                    TurnEndCause::TurnComplete if !worker_spoke_explicitly => Some(format!(
                        "[SYSTEM] worker '{member}' completed its turn for \
                         msg {mid} without calling send_message. Either the \
                         worker chose to stay silent or its onboarding \
                         didn't teach the explicit-send protocol. Send a \
                         follow-up to nudge, or check the worker's system \
                         prompt.",
                        member = self.member_id,
                        mid = message.id
                    )),
                    TurnEndCause::TurnComplete => None, // worker spoke — silent success
                };

                if let Some(body) = notice {
                    tracing::info!(
                        member = %self.member_id,
                        msg_id = %message.id,
                        end_cause = ?end_cause,
                        "posting [SYSTEM] terminal notice for inbox turn"
                    );
                    let _ = self.message_service.send(SendMessageRequest {
                        team_id: self.team_id.clone(),
                        room_id: self.room_id.clone(),
                        sender: self.member_id.clone(),
                        kind: MessageKind::Status,
                        subject: None,
                        body,
                        mentions: Vec::new(),
                        visibility: Vec::new(),
                        audience_policy: None,
                        reply_to: Some(message.id.clone()),
                        thread_id: message.thread_id.clone(),
                        expires_at: None,
                    });
                } else {
                    tracing::info!(
                        member = %self.member_id,
                        msg_id = %message.id,
                        sent = final_sent - baseline_sent,
                        "turn complete; worker spoke explicitly via send_message"
                    );
                }

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
                    let sid = self
                        .orchestrator
                        .lock()
                        .await
                        .session_id_of(&self.session_key);
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

        // Bug 29 contract: stdout `Message`/`Delta` events are NOT
        // auto-published as a Reply. Workers must call send_message
        // explicitly to communicate. The driver here only emits a
        // TurnComplete with a Message event but never calls
        // send_message → expect a [SYSTEM] silent-turn notice, not a
        // Reply with the Message body.
        let messages = message_store.list_by_room("team-1", "main").unwrap();
        let auto_reply = messages
            .iter()
            .find(|m| m.sender == "worker" && matches!(m.kind, MessageKind::Reply));
        assert!(
            auto_reply.is_none(),
            "stdout `Message` should NOT be auto-captured as a Reply; \
             worker must call send_message explicitly. Found: {:?}",
            auto_reply
        );
        let silent_notice = messages
            .iter()
            .find(|m| m.sender == "worker" && matches!(m.kind, MessageKind::Status));
        assert!(
            silent_notice.is_some(),
            "expected [SYSTEM] silent-turn notice when worker doesn't call send_message"
        );
        assert!(
            silent_notice.unwrap().body.contains("without calling send_message"),
            "silent-turn notice should mention send_message; got: {}",
            silent_notice.unwrap().body
        );
    }

    /// Helper to wire up an AgentLoop + scripted output channel.
    /// Returns the message_store and inbox_service so the test can assert
    /// what the agent_loop posted.
    fn run_loop_with_script<F>(script: F) -> (MessageStore, InboxService)
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
                    .contains("without calling send_message"),
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
        // Bug 29: partial stdout output is no longer included in pipe-close
        // notices — the worker's "partial answer" via stdout is private
        // working notes, not a message. The notice only states the death
        // and points the lead at how to revive.
        assert!(
            !from_worker[0].body.contains("partial..."),
            "partial stdout should NOT leak into pipe-close notice: {}",
            from_worker[0].body
        );
        assert!(
            from_worker[0].body.contains("worker_add"),
            "notice should suggest revive command: {}",
            from_worker[0].body
        );
    }
}
