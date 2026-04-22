//! Mixed-backend example: Claude Code lead + Codex teammates.
//!
//! This demonstrates agent-teams' core value proposition: a single orchestrator
//! managing teammates on different backends, communicating through a shared
//! file-based task list and inbox system.
//!
//! Architecture:
//! ```text
//!   ┌──────────────────────────────────────────────────┐
//!   │              TeamOrchestrator                     │
//!   │                                                   │
//!   │  ┌─────────────┐   shared    ┌─────────────────┐ │
//!   │  │ ClaudeCode  │◄──tasks───►│     Codex        │ │
//!   │  │  Backend    │   inbox     │    Backend       │ │
//!   │  │             │             │                  │ │
//!   │  │ ┌─────────┐ │            │ ┌──────────────┐ │ │
//!   │  │ │ planner │ │            │ │ coder        │ │ │
//!   │  │ │(claude) │ │            │ │(codex)       │ │ │
//!   │  │ └─────────┘ │            │ └──────────────┘ │ │
//!   │  │ ┌─────────┐ │            │ ┌──────────────┐ │ │
//!   │  │ │reviewer │ │            │ │ test-runner  │ │ │
//!   │  │ │(claude) │ │            │ │(codex)       │ │ │
//!   │  │ └─────────┘ │            │ └──────────────┘ │ │
//!   │  └─────────────┘            └─────────────────┘ │
//!   │                                                   │
//!   │  ~/.claude/teams/mixed/config.json    ← shared   │
//!   │  ~/.claude/tasks/mixed/{id}.json      ← shared   │
//!   │  ~/.claude/teams/mixed/inboxes/*.json ← shared   │
//!   └──────────────────────────────────────────────────┘
//! ```
//!
//! Run with:
//!   cargo run --example mixed_backends
//!
//! NOTE: This example uses mock backends for demonstration.
//! For real usage, replace MockClaudeBackend/MockCodexBackend with
//! ClaudeCodeBackend::new() and CodexBackend::new().

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use agent_teams::backend::{AgentBackend, AgentOutput, AgentSession, BackendType, SpawnConfig};
use agent_teams::models::{CreateTaskRequest, TaskStatus, TaskUpdate};
use agent_teams::orchestrator::TeamOrchestrator;
use agent_teams::Result;

// ---------------------------------------------------------------------------
// Mock backends (replace with real ones in production)
// ---------------------------------------------------------------------------

/// Simulates a Claude Code backend.
struct MockClaudeBackend;

#[async_trait]
impl AgentBackend for MockClaudeBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::ClaudeCode
    }

    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>> {
        println!("    [cc-sdk] Spawning '{}' on Claude Code...", config.name);
        let (tx, rx) = mpsc::channel(16);
        let _ = tx
            .send(AgentOutput::Message(format!(
                "Claude agent '{}' ready",
                config.name
            )))
            .await;
        Ok(Box::new(MockSession::new(config.name, rx)))
    }
}

/// Simulates a Codex backend.
struct MockCodexBackend;

#[async_trait]
impl AgentBackend for MockCodexBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::Codex
    }

    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>> {
        println!(
            "    [codex] Spawning '{}' on Codex (JSON-RPC)...",
            config.name
        );
        let (tx, rx) = mpsc::channel(16);
        let _ = tx
            .send(AgentOutput::Message(format!(
                "Codex agent '{}' ready",
                config.name
            )))
            .await;
        Ok(Box::new(MockSession::new(config.name, rx)))
    }
}

struct MockSession {
    name: String,
    output_rx: Option<mpsc::Receiver<AgentOutput>>,
    alive: Arc<AtomicBool>,
}

impl MockSession {
    fn new(name: String, rx: mpsc::Receiver<AgentOutput>) -> Self {
        Self {
            name,
            output_rx: Some(rx),
            alive: Arc::new(AtomicBool::new(true)),
        }
    }
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
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let tmp = tempfile::tempdir().expect("tempdir");

    println!("=== Mixed-Backend Agent Teams ===\n");

    // 1. Build orchestrator with BOTH backends
    let orch = TeamOrchestrator::builder()
        .teams_base(tmp.path().join("teams"))
        .tasks_base(tmp.path().join("tasks"))
        .with_claude_code(MockClaudeBackend)  // In production: ClaudeCodeBackend::new()
        .with_codex(MockCodexBackend)          // In production: CodexBackend::new()?
        .build()?;

    // 2. Create team
    orch.create_team("mixed", Some("Claude Code + Codex hybrid team"))
        .await?;
    println!("[Team] Created 'mixed'\n");

    // 3. Spawn teammates on DIFFERENT backends
    println!("[Spawning teammates]");

    // Claude Code agents: good at planning and code review
    orch.spawn_teammate(
        "mixed",
        SpawnConfig {
            name: "planner".into(),
            prompt: "You are an architect. Design the implementation plan.".into(),
            model: Some("opus".into()),
            memory_config: None,
            ..SpawnConfig::new("", "")
        },
        BackendType::ClaudeCode,  // ← Claude Code backend
    )
    .await?;

    orch.spawn_teammate(
        "mixed",
        SpawnConfig {
            name: "reviewer".into(),
            prompt: "You are a code reviewer. Check for bugs and style issues.".into(),
            model: Some("sonnet".into()),
            memory_config: None,
            ..SpawnConfig::new("", "")
        },
        BackendType::ClaudeCode,  // ← Claude Code backend
    )
    .await?;

    // Codex agents: good at writing and testing code
    orch.spawn_teammate(
        "mixed",
        SpawnConfig {
            name: "coder".into(),
            prompt: "You are a Rust developer. Write clean, idiomatic code.".into(),
            memory_config: None,
            ..SpawnConfig::new("", "")
        },
        BackendType::Codex,  // ← Codex backend
    )
    .await?;

    orch.spawn_teammate(
        "mixed",
        SpawnConfig {
            name: "test-runner".into(),
            prompt: "You run tests and report failures with diagnostics.".into(),
            memory_config: None,
            ..SpawnConfig::new("", "")
        },
        BackendType::Codex,  // ← Codex backend
    )
    .await?;

    // 4. Show team config
    let config = orch.read_team("mixed").await?;
    println!("\n[Team config]");
    for m in &config.members {
        println!("  {} ({})", m.name(), m.agent_type());
    }

    // 5. Create tasks with a realistic workflow
    println!("\n[Creating task pipeline]");

    let t_plan = orch
        .create_task("mixed", CreateTaskRequest {
            subject: "Design API for user service".into(),
            description: Some("Define routes, data models, and error types".into()),
            active_form: Some("Designing API".into()),
            metadata: None,
        })
        .await?;

    let t_impl = orch
        .create_task("mixed", CreateTaskRequest {
            subject: "Implement user service endpoints".into(),
            description: Some("Write axum handlers per the design doc".into()),
            active_form: Some("Implementing endpoints".into()),
            metadata: None,
        })
        .await?;

    let t_test = orch
        .create_task("mixed", CreateTaskRequest {
            subject: "Write integration tests".into(),
            description: Some("Test all endpoints with mock DB".into()),
            active_form: Some("Writing tests".into()),
            metadata: None,
        })
        .await?;

    let t_review = orch
        .create_task("mixed", CreateTaskRequest {
            subject: "Code review implementation".into(),
            description: Some("Review code quality, error handling, and style".into()),
            active_form: Some("Reviewing code".into()),
            metadata: None,
        })
        .await?;

    // Dependencies: plan -> impl -> (test, review)
    orch.update_task("mixed", &t_impl.id, TaskUpdate {
        add_blocked_by: Some(vec![t_plan.id.clone()]),
        ..Default::default()
    }).await?;
    orch.update_task("mixed", &t_test.id, TaskUpdate {
        add_blocked_by: Some(vec![t_impl.id.clone()]),
        ..Default::default()
    }).await?;
    orch.update_task("mixed", &t_review.id, TaskUpdate {
        add_blocked_by: Some(vec![t_impl.id.clone()]),
        ..Default::default()
    }).await?;

    println!("  #{} Design API (planner/claude)", t_plan.id);
    println!("    └─► #{} Implement (coder/codex)", t_impl.id);
    println!("        ├─► #{} Test (test-runner/codex)", t_test.id);
    println!("        └─► #{} Review (reviewer/claude)", t_review.id);

    // 6. Assign tasks to agents on their respective backends
    println!("\n[Assigning tasks to cross-backend agents]");

    // planner (Claude) designs the API
    orch.assign_task("mixed", &t_plan.id, "planner").await?;
    println!("  #{} -> planner (Claude Code)", t_plan.id);

    // Simulate planner completing
    orch.update_task("mixed", &t_plan.id, TaskUpdate {
        status: Some(TaskStatus::InProgress),
        ..Default::default()
    }).await?;
    orch.update_task("mixed", &t_plan.id, TaskUpdate {
        status: Some(TaskStatus::Completed),
        ..Default::default()
    }).await?;
    println!("  #{} ✓ planner completed design", t_plan.id);

    // coder (Codex) implements
    orch.assign_task("mixed", &t_impl.id, "coder").await?;
    println!("  #{} -> coder (Codex)", t_impl.id);

    orch.update_task("mixed", &t_impl.id, TaskUpdate {
        status: Some(TaskStatus::InProgress),
        ..Default::default()
    }).await?;
    orch.update_task("mixed", &t_impl.id, TaskUpdate {
        status: Some(TaskStatus::Completed),
        ..Default::default()
    }).await?;
    println!("  #{} ✓ coder completed implementation", t_impl.id);

    // Now test and review can run IN PARALLEL on different backends
    orch.assign_task("mixed", &t_test.id, "test-runner").await?;
    orch.assign_task("mixed", &t_review.id, "reviewer").await?;
    println!("  #{} -> test-runner (Codex)  ┐ parallel!", t_test.id);
    println!("  #{} -> reviewer (Claude)    ┘", t_review.id);

    // 7. Cross-backend messaging works seamlessly
    println!("\n[Cross-backend messaging]");

    // Codex agent sends message to Claude agent
    orch.send_message("mixed", "coder", "reviewer", "Implementation done, please review src/handlers.rs")
        .await?;
    println!("  coder (Codex) -> reviewer (Claude): review request");

    // Claude agent replies to Codex agent
    orch.send_message("mixed", "reviewer", "coder", "Found 2 issues, please fix error handling in create_user()")
        .await?;
    println!("  reviewer (Claude) -> coder (Codex): review feedback");

    // Broadcast from lead to all
    orch.broadcast("mixed", "lead", "Great work team! Wrapping up.")
        .await?;
    println!("  lead -> ALL: broadcast");

    // 8. Final status
    println!("\n[Final task status]");
    let tasks = orch.list_tasks("mixed", None).await?;
    for t in &tasks {
        let backend = match t.owner.as_deref() {
            Some("planner" | "reviewer") => "claude",
            Some("coder" | "test-runner") => "codex",
            _ => "-",
        };
        println!(
            "  #{} [{}] {} (owner: {}, backend: {})",
            t.id,
            t.status,
            t.subject,
            t.owner.as_deref().unwrap_or("-"),
            backend
        );
    }

    // 9. Cleanup
    println!("\n[Shutdown]");
    for name in ["planner", "reviewer", "coder", "test-runner"] {
        orch.shutdown_teammate("mixed", name).await?;
        println!("  {name} shut down");
    }
    orch.delete_team("mixed").await?;
    println!("  Team deleted.\n");

    println!("=== Key takeaway ===");
    println!("All agents share the SAME task list and inbox system,");
    println!("regardless of whether they run on Claude Code or Codex.");
    println!("The file-based protocol is the universal coordination layer.");

    Ok(())
}
