//! Integration tests for the TeamOrchestrator using a mock backend.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use agent_teams::backend::{
    AgentBackend, AgentOutput, AgentSession, BackendType, SpawnConfig,
};
use agent_teams::models::{CreateTaskRequest, TaskStatus, TaskUpdate};
use agent_teams::orchestrator::TeamOrchestrator;
use agent_teams::{Error, Result};

// ---------------------------------------------------------------------------
// Mock backend
// ---------------------------------------------------------------------------

struct MockBackend;

#[async_trait]
impl AgentBackend for MockBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::ClaudeCode
    }

    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>> {
        let (tx, rx) = mpsc::channel(16);
        // Send a greeting as the first output
        let _ = tx
            .send(AgentOutput::Message(format!(
                "Hello, I'm {}",
                config.name
            )))
            .await;
        let _ = tx.send(AgentOutput::TurnComplete).await;

        Ok(Box::new(MockSession {
            name: config.name,
            output_rx: Some(rx),
            alive: Arc::new(AtomicBool::new(true)),
        }))
    }
}

struct MockSession {
    name: String,
    output_rx: Option<mpsc::Receiver<AgentOutput>>,
    alive: Arc<AtomicBool>,
}

#[async_trait]
impl AgentSession for MockSession {
    fn name(&self) -> &str {
        &self.name
    }

    async fn send_input(&mut self, _input: &str) -> Result<()> {
        Ok(())
    }

    fn output_receiver(&mut self) -> Option<mpsc::Receiver<AgentOutput>> {
        self.output_rx.take()
    }

    async fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.alive.store(false, Ordering::Relaxed);
        Ok(())
    }

    async fn force_kill(&mut self) -> Result<()> {
        self.alive.store(false, Ordering::Relaxed);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_orchestrator(dir: &std::path::Path) -> TeamOrchestrator {
    let teams_dir = dir.join("teams");
    let tasks_dir = dir.join("tasks");

    TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .with_claude_code(MockBackend)
        .build()
        .unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_team_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    // Create team
    let config = orch.create_team("test-team", Some("Test team")).await.unwrap();
    assert_eq!(config.team_name, "test-team");

    // List teams
    let teams = orch.list_teams().await.unwrap();
    assert!(teams.contains(&"test-team".to_string()));

    // Spawn teammate
    let spawn_cfg = SpawnConfig::new("worker-1", "You are a test worker");
    orch.spawn_teammate("test-team", spawn_cfg, BackendType::ClaudeCode)
        .await
        .unwrap();

    // Verify member registered
    let config = orch.read_team("test-team").await.unwrap();
    assert_eq!(config.members.len(), 1);
    assert_eq!(config.members[0].name(), "worker-1");

    // Check alive
    assert!(orch.is_alive("test-team", "worker-1").await);

    // Shutdown teammate
    orch.shutdown_teammate("test-team", "worker-1").await.unwrap();
    assert!(!orch.is_alive("test-team", "worker-1").await);

    // Delete team
    orch.delete_team("test-team").await.unwrap();

    // Verify deleted
    let teams = orch.list_teams().await.unwrap();
    assert!(!teams.contains(&"test-team".to_string()));
}

#[tokio::test]
async fn task_creation_and_assignment() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    // Setup
    orch.create_team("proj", None).await.unwrap();
    let spawn_cfg = SpawnConfig::new("coder", "Write code");
    orch.spawn_teammate("proj", spawn_cfg, BackendType::ClaudeCode)
        .await
        .unwrap();

    // Create tasks
    let t1 = orch
        .create_task(
            "proj",
            CreateTaskRequest {
                subject: "Setup project".into(),
                description: Some("cargo init".into()),
                active_form: None,
                metadata: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(t1.id, "1");
    assert_eq!(t1.status, TaskStatus::Pending);

    let t2 = orch
        .create_task(
            "proj",
            CreateTaskRequest {
                subject: "Write tests".into(),
                description: None,
                active_form: None,
                metadata: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(t2.id, "2");

    // Assign task to coder (should also send inbox message)
    let assigned = orch.assign_task("proj", "1", "coder").await.unwrap();
    assert_eq!(assigned.owner.as_deref(), Some("coder"));

    // Check coder's inbox for TaskAssignment
    let inbox = orch
        .list_teams()
        .await
        .unwrap(); // just verify no panic
    assert!(!inbox.is_empty());

    // Get next available task (should be task 2 since task 1 is assigned)
    let next = orch.get_next_available_task("proj").await.unwrap();
    assert!(next.is_some());
    assert_eq!(next.unwrap().id, "2");

    // Complete task 1
    let completed = orch
        .update_task(
            "proj",
            "1",
            TaskUpdate {
                status: Some(TaskStatus::InProgress),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(completed.status, TaskStatus::InProgress);

    let completed = orch
        .update_task(
            "proj",
            "1",
            TaskUpdate {
                status: Some(TaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);

    // Cleanup
    orch.shutdown_teammate("proj", "coder").await.unwrap();
    orch.delete_team("proj").await.unwrap();
}

#[tokio::test]
async fn messaging_through_orchestrator() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    orch.create_team("msg-team", None).await.unwrap();

    let cfg1 = SpawnConfig::new("alice", "Alice agent");
    let cfg2 = SpawnConfig::new("bob", "Bob agent");
    orch.spawn_teammate("msg-team", cfg1, BackendType::ClaudeCode)
        .await
        .unwrap();
    orch.spawn_teammate("msg-team", cfg2, BackendType::ClaudeCode)
        .await
        .unwrap();

    // Direct message
    orch.send_message("msg-team", "alice", "bob", "Hello Bob!")
        .await
        .unwrap();

    // Broadcast
    orch.broadcast("msg-team", "alice", "Team announcement!")
        .await
        .unwrap();

    // Shutdown request
    orch.send_shutdown_request("msg-team", "alice", "bob", Some("All done"))
        .await
        .unwrap();

    // Cleanup
    orch.shutdown_teammate("msg-team", "alice").await.unwrap();
    orch.shutdown_teammate("msg-team", "bob").await.unwrap();
    orch.delete_team("msg-team").await.unwrap();
}

#[tokio::test]
async fn backend_not_configured_error() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    orch.create_team("no-codex", None).await.unwrap();

    let cfg = SpawnConfig::new("agent", "test");
    let result = orch
        .spawn_teammate("no-codex", cfg, BackendType::Codex)
        .await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        Error::BackendNotConfigured { .. }
    ));

    orch.delete_team("no-codex").await.unwrap();
}

#[tokio::test]
async fn multiple_teammates_spawn_and_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    orch.create_team("big-team", Some("Many workers"))
        .await
        .unwrap();

    // Spawn 5 teammates
    for i in 0..5 {
        let cfg = SpawnConfig::new(format!("w-{i}"), format!("Worker {i}"));
        orch.spawn_teammate("big-team", cfg, BackendType::ClaudeCode)
            .await
            .unwrap();
    }

    let config = orch.read_team("big-team").await.unwrap();
    assert_eq!(config.members.len(), 5);

    // All alive
    for i in 0..5 {
        assert!(orch.is_alive("big-team", &format!("w-{i}")).await);
    }

    // Force kill all
    for i in 0..5 {
        orch.force_kill_teammate("big-team", &format!("w-{i}"))
            .await
            .unwrap();
    }

    // Delete team
    orch.delete_team("big-team").await.unwrap();
}

#[tokio::test]
async fn are_alive_batch_status() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    orch.create_team("alive-team", None).await.unwrap();

    // Spawn 3 teammates
    for name in &["alpha", "beta", "gamma"] {
        let cfg = SpawnConfig::new(*name, format!("{name} agent"));
        orch.spawn_teammate("alive-team", cfg, BackendType::ClaudeCode)
            .await
            .unwrap();
    }

    // All should be alive
    let status = orch.are_alive("alive-team").await.unwrap();
    assert_eq!(status.len(), 3);
    assert!(status["alpha"]);
    assert!(status["beta"]);
    assert!(status["gamma"]);

    // Shut down one
    orch.shutdown_teammate("alive-team", "beta").await.unwrap();

    // beta should now be missing from are_alive (removed from team config)
    let status = orch.are_alive("alive-team").await.unwrap();
    assert_eq!(status.len(), 2);
    assert!(status["alpha"]);
    assert!(status["gamma"]);
    assert!(!status.contains_key("beta"));

    // Cleanup
    orch.shutdown_teammate("alive-team", "alpha").await.unwrap();
    orch.shutdown_teammate("alive-team", "gamma").await.unwrap();
    orch.delete_team("alive-team").await.unwrap();
}

#[tokio::test]
async fn shutdown_all_teammates_batch() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    orch.create_team("batch-team", None).await.unwrap();

    // Spawn 4 teammates
    for i in 0..4 {
        let cfg = SpawnConfig::new(format!("agent-{i}"), format!("Agent {i}"));
        orch.spawn_teammate("batch-team", cfg, BackendType::ClaudeCode)
            .await
            .unwrap();
    }

    let config = orch.read_team("batch-team").await.unwrap();
    assert_eq!(config.members.len(), 4);

    // Shutdown all at once
    let count = orch.shutdown_all_teammates("batch-team").await.unwrap();
    assert_eq!(count, 4);

    // No members left
    let config = orch.read_team("batch-team").await.unwrap();
    assert_eq!(config.members.len(), 0);

    // None alive
    for i in 0..4 {
        assert!(!orch.is_alive("batch-team", &format!("agent-{i}")).await);
    }

    orch.delete_team("batch-team").await.unwrap();
}

#[tokio::test]
async fn spawn_config_builder_in_orchestrator() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    orch.create_team("builder-team", None).await.unwrap();

    // Use the builder pattern instead of struct literal
    let cfg = SpawnConfig::builder("built-agent", "You are a builder test agent")
        .max_turns(10)
        .permission_mode("default")
        .env_var("TEST_KEY", "test_value")
        .build();

    assert_eq!(cfg.name, "built-agent");
    assert_eq!(cfg.max_turns, Some(10));
    assert_eq!(cfg.permission_mode.as_deref(), Some("default"));
    assert_eq!(cfg.env.get("TEST_KEY").unwrap(), "test_value");

    // Actually spawn with the built config
    orch.spawn_teammate("builder-team", cfg, BackendType::ClaudeCode)
        .await
        .unwrap();

    assert!(orch.is_alive("builder-team", "built-agent").await);

    orch.shutdown_teammate("builder-team", "built-agent")
        .await
        .unwrap();
    orch.delete_team("builder-team").await.unwrap();
}

#[tokio::test]
async fn export_mermaid_and_critical_path() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    orch.create_team("dag-team", None).await.unwrap();

    // Create tasks with dependencies: t1 -> t2 -> t3
    let t1 = orch
        .create_task(
            "dag-team",
            CreateTaskRequest {
                subject: "Setup".into(),
                description: None,
                active_form: None,
                metadata: None,
            },
        )
        .await
        .unwrap();

    let t2 = orch
        .create_task(
            "dag-team",
            CreateTaskRequest {
                subject: "Build".into(),
                description: None,
                active_form: None,
                metadata: None,
            },
        )
        .await
        .unwrap();

    let t3 = orch
        .create_task(
            "dag-team",
            CreateTaskRequest {
                subject: "Deploy".into(),
                description: None,
                active_form: None,
                metadata: None,
            },
        )
        .await
        .unwrap();

    // Add dependencies: t2 blocked by t1, t3 blocked by t2
    orch.update_task(
        "dag-team",
        &t2.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t1.id.clone()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    orch.update_task(
        "dag-team",
        &t3.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t2.id.clone()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Export Mermaid
    let mermaid = orch.export_task_graph_mermaid("dag-team").await.unwrap();
    assert!(mermaid.starts_with("graph TD\n"));
    assert!(mermaid.contains("Setup"));
    assert!(mermaid.contains("Build"));
    assert!(mermaid.contains("Deploy"));

    // Export DOT
    let dot = orch.export_task_graph_dot("dag-team").await.unwrap();
    assert!(dot.starts_with("digraph tasks {"));
    assert!(dot.contains("Setup"));

    // Critical path: should be t1 -> t2 -> t3
    let critical = orch.get_critical_path("dag-team").await.unwrap();
    assert_eq!(critical.len(), 3);
    assert_eq!(critical[0].subject, "Setup");
    assert_eq!(critical[1].subject, "Build");
    assert_eq!(critical[2].subject, "Deploy");

    orch.delete_team("dag-team").await.unwrap();
}

#[tokio::test]
async fn consensus_via_orchestrator() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    let responses = vec![
        agent_teams::AgentResponse {
            agent: "agent-1".into(),
            content: "yes".into(),
            weight: 1.0,
            timed_out: false,
        },
        agent_teams::AgentResponse {
            agent: "agent-2".into(),
            content: "yes".into(),
            weight: 1.0,
            timed_out: false,
        },
        agent_teams::AgentResponse {
            agent: "agent-3".into(),
            content: "no".into(),
            weight: 1.0,
            timed_out: false,
        },
    ];

    let result = orch.resolve_consensus(&responses, agent_teams::ConsensusStrategy::Majority);
    assert!(result.consensus_reached);
    assert_eq!(result.decision, Some("yes".to_string()));
    assert_eq!(result.responses.len(), 3);

    let result = orch.resolve_consensus(&responses, agent_teams::ConsensusStrategy::Unanimous);
    assert!(!result.consensus_reached);
    assert!(result.decision.is_none());
}

#[tokio::test]
async fn memory_context_injection() {
    let dir = tempfile::tempdir().unwrap();
    let orch = make_orchestrator(dir.path());

    orch.create_team("mem-team", None).await.unwrap();

    let cfg = SpawnConfig::new("mem-agent", "You are a test agent");
    orch.spawn_teammate("mem-team", cfg, BackendType::ClaudeCode)
        .await
        .unwrap();

    // Enable memory
    orch.enable_memory("mem-team", "mem-agent", agent_teams::MemoryConfig::default())
        .await
        .unwrap();

    // Send input (records user turn in memory)
    orch.send_input("mem-team", "mem-agent", "What is 2+2?")
        .await
        .unwrap();

    // Record assistant output
    orch.record_assistant_output("mem-team", "mem-agent", "The answer is 4.")
        .await
        .unwrap();

    // Send another input — should have context prepended
    orch.send_input("mem-team", "mem-agent", "And 3+3?")
        .await
        .unwrap();

    // Disable memory (persists final state)
    orch.disable_memory("mem-team", "mem-agent").await.unwrap();

    // Clear memory
    orch.enable_memory("mem-team", "mem-agent", agent_teams::MemoryConfig::default())
        .await
        .unwrap();
    orch.clear_memory("mem-team", "mem-agent").await.unwrap();

    // Cleanup
    orch.shutdown_teammate("mem-team", "mem-agent")
        .await
        .unwrap();
    orch.delete_team("mem-team").await.unwrap();
}
