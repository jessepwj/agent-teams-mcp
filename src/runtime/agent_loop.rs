use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot;

use crate::backend::AgentOutput;
use crate::runtime::orchestrator::RuntimeOrchestrator;
use crate::team_mode::domain::{InboxStatus, MessageKind};
use crate::team_mode::service::{InboxNotifier, InboxService, MessageService, SendMessageRequest};
use crate::team_mode::storage::MessageStore;

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
    pub member_id: String,
    pub team_id: String,
    pub room_id: String,
    pub orchestrator: Arc<Mutex<RuntimeOrchestrator>>,
    pub inbox_service: InboxService,
    pub message_store: MessageStore,
    pub message_service: MessageService,
    pub poll_interval: Duration,
    pub inbox_notifier: Option<InboxNotifier>,
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

    async fn run(self, mut output_rx: Receiver<AgentOutput>, mut shutdown_rx: oneshot::Receiver<()>) {
        // Drain the initial Claude Code response to the system prompt before polling.
        loop {
            match output_rx.recv().await {
                Some(AgentOutput::TurnComplete) => break,
                Some(AgentOutput::Error(_)) | None => return,
                Some(AgentOutput::Message(_))
                | Some(AgentOutput::Delta(_))
                | Some(AgentOutput::Idle) => {}
            }
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
                let input = format!(
                    "[Message from {}]: {}",
                    message.sender, message.body
                );
                {
                    tracing::debug!(member = %self.member_id, "sending input to session");
                    let mut orch = self.orchestrator.lock().await;
                    if orch
                        .send_input(&self.member_id, &input)
                        .await
                        .is_err()
                    {
                        tracing::error!(member = %self.member_id, "send_input failed, shutting down agent loop");
                        return;
                    }
                }

                // 4. Collect Claude Code's response until TurnComplete.
                let mut parts: Vec<String> = Vec::new();
                loop {
                    match output_rx.recv().await {
                        Some(AgentOutput::Message(text)) | Some(AgentOutput::Delta(text)) => {
                            parts.push(text);
                        }
                        Some(AgentOutput::TurnComplete) | Some(AgentOutput::Idle) => break,
                        Some(AgentOutput::Error(e)) => {
                            parts.push(format!("[agent error: {e}]"));
                            break;
                        }
                        None => return,
                    }
                }

                // 5. Post the reply back to the room.
                let body = parts.join("");
                if !body.is_empty() {
                    tracing::info!(member = %self.member_id, reply_len = body.len(), "posting reply to room");
                    let _ = self.message_service.send(SendMessageRequest {
                        team_id: self.team_id.clone(),
                        room_id: self.room_id.clone(),
                        sender: self.member_id.clone(),
                        kind: MessageKind::Reply,
                        subject: None,
                        body,
                        mentions: Vec::new(),
                        visibility: Vec::new(),
                        audience_policy: None,
                        reply_to: Some(message.id.clone()),
                        thread_id: message.thread_id.clone(),
                        expires_at: None,
                    });
                }

                // 6. Ack the processed inbox item.
                tracing::debug!(member = %self.member_id, msg_id = %message.id, "ack'd inbox item");
                let _ = self
                    .inbox_service
                    .ack(&self.team_id, &self.member_id, &[message.id]);
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
        for (name, kind) in [
            ("lead", MemberKind::Lead),
            ("worker", MemberKind::Member),
        ] {
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

    fn build_services(
        base: &std::path::Path,
    ) -> (MessageService, InboxService, MessageStore) {
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
        let before = inbox_service
            .peek("team-1", "worker", None)
            .unwrap();
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
            team_id: "team-1".into(),
            room_id: "main".into(),
            orchestrator: Arc::clone(&orch),
            inbox_service: inbox_service.clone(),
            message_store: message_store.clone(),
            message_service: message_service.clone(),
            poll_interval: Duration::from_millis(50),
            inbox_notifier: None,
        };

        // Script: emit TurnComplete (initial prompt drain), then after send_input
        // emit "Done!" + TurnComplete. Driver thread drops output_tx on exit.
        let driver = std::thread::spawn(move || {
            output_tx.blocking_send(AgentOutput::TurnComplete).unwrap();
            let _ = input_rx.blocking_recv().unwrap();
            output_tx
                .blocking_send(AgentOutput::Message("Done!".into()))
                .unwrap();
            output_tx
                .blocking_send(AgentOutput::TurnComplete)
                .unwrap();
            // output_tx dropped here; further recv() in loop returns None
        });

        let loop_handle = agent_loop.start(output_rx);
        driver.join().unwrap();

        // Give the loop time to finish the iteration and ack the message.
        std::thread::sleep(Duration::from_millis(200));

        // Signal shutdown and wait for the loop thread to exit cleanly.
        loop_handle.shutdown();

        // Worker inbox should now be acked.
        let after = inbox_service
            .peek("team-1", "worker", None)
            .unwrap();
        assert_eq!(after.items.len(), 1);
        assert!(matches!(after.items[0].status, InboxStatus::Acked));

        // A reply message from worker should exist in the room.
        let messages = message_store
            .list_by_room("team-1", "main")
            .unwrap();
        let reply = messages
            .iter()
            .find(|m| m.sender == "worker" && matches!(m.kind, MessageKind::Reply));
        assert!(reply.is_some(), "expected a reply from worker in the room");
        assert_eq!(reply.unwrap().body, "Done!");
    }
}
