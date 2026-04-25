//! Demo of three new features: DAG Visualization, Consensus Protocol, Agent Memory.
//!
//! This example uses NO real CLI backends — it demonstrates pure-function features
//! that work standalone without Claude Code, Codex, or Gemini CLI installed.
//!
//! Run with:
//!   cargo run --example new_features_demo

use std::time::Duration;

use agent_teams::consensus::{AgentResponse, ConsensusRequest, ConsensusStrategy};
use agent_teams::memory::{ConversationMemory, MemoryConfig, Role};
use agent_teams::models::{CreateTaskRequest, TaskStatus, TaskUpdate};
use agent_teams::orchestrator::TeamOrchestrator;
use agent_teams::task::DependencyGraph;

#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    let tmp = tempfile::tempdir().expect("failed to create temp dir");
    let teams_dir = tmp.path().join("teams");
    let tasks_dir = tmp.path().join("tasks");

    println!("╔══════════════════════════════════════════╗");
    println!("║  agent-teams — New Features Demo         ║");
    println!("╚══════════════════════════════════════════╝\n");

    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .build()?;

    orch.create_team("demo", Some("Feature demo team")).await?;

    // =========================================================================
    // Feature 1: Task DAG Visualization
    // =========================================================================
    println!("━━━ Feature 1: Task DAG Visualization ━━━\n");

    // Create a realistic task pipeline:
    //   design ──→ implement ──→ test
    //                  ↑
    //   setup-ci ──────┘

    let design = orch
        .create_task(
            "demo",
            CreateTaskRequest {
                subject: "Design API schema".into(),
                description: Some("Define REST endpoints".into()),
                active_form: Some("Designing".into()),
                metadata: None,
            },
        )
        .await?;

    let setup_ci = orch
        .create_task(
            "demo",
            CreateTaskRequest {
                subject: "Setup CI pipeline".into(),
                description: Some("GitHub Actions config".into()),
                active_form: Some("Setting up CI".into()),
                metadata: None,
            },
        )
        .await?;

    let implement = orch
        .create_task(
            "demo",
            CreateTaskRequest {
                subject: "Implement endpoints".into(),
                description: Some("Build all handlers".into()),
                active_form: Some("Implementing".into()),
                metadata: None,
            },
        )
        .await?;

    let test = orch
        .create_task(
            "demo",
            CreateTaskRequest {
                subject: "Write integration tests".into(),
                description: Some("Test all endpoints".into()),
                active_form: Some("Testing".into()),
                metadata: None,
            },
        )
        .await?;

    // Set up dependencies
    orch.update_task(
        "demo",
        &implement.id,
        TaskUpdate {
            add_blocked_by: Some(vec![design.id.clone(), setup_ci.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    orch.update_task(
        "demo",
        &test.id,
        TaskUpdate {
            add_blocked_by: Some(vec![implement.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    // Mark some tasks with different statuses for colorful output
    // State machine: pending → in_progress → completed (can't skip steps)
    orch.update_task(
        "demo",
        &design.id,
        TaskUpdate {
            status: Some(TaskStatus::InProgress),
            owner: Some("alice".into()),
            ..Default::default()
        },
    )
    .await?;
    orch.update_task(
        "demo",
        &design.id,
        TaskUpdate {
            status: Some(TaskStatus::Completed),
            ..Default::default()
        },
    )
    .await?;

    orch.update_task(
        "demo",
        &setup_ci.id,
        TaskUpdate {
            status: Some(TaskStatus::InProgress),
            owner: Some("bob".into()),
            ..Default::default()
        },
    )
    .await?;

    // --- Terminal rendering (ANSI colors!) ---
    let dag_view = orch.export_task_graph_terminal("demo").await?;
    print!("{dag_view}");

    // --- Also show Mermaid + DOT for export ---
    println!("\n[Mermaid export]");
    let mermaid = orch.export_task_graph_mermaid("demo").await?;
    println!("{mermaid}");

    // --- Topological sort ---
    let all_tasks = orch.list_tasks("demo", None).await?;
    let graph = DependencyGraph::from_tasks(&all_tasks);
    let order = graph.topological_sort()?;
    println!("[Topological Sort] {}", order.join(" → "));

    // =========================================================================
    // Feature 2: Consensus Protocol
    // =========================================================================
    println!("\n━━━ Feature 2: Consensus Protocol ━━━\n");

    // Simulate: 3 agents review code, 2 say "approve", 1 says "request changes"
    let responses = vec![
        AgentResponse {
            agent: "reviewer-1".into(),
            content: "approve".into(),
            weight: 1.0,
            timed_out: false,
        },
        AgentResponse {
            agent: "reviewer-2".into(),
            content: "approve".into(),
            weight: 2.0, // senior reviewer, higher weight
            timed_out: false,
        },
        AgentResponse {
            agent: "reviewer-3".into(),
            content: "request changes".into(),
            weight: 1.0,
            timed_out: false,
        },
    ];

    // --- Majority vote ---
    let result = orch.resolve_consensus(&responses, ConsensusStrategy::Majority);
    println!("[Majority Vote]");
    println!("  Decision: {:?}", result.decision);
    println!("  Consensus reached: {}", result.consensus_reached);
    println!("  Votes: {} responses", result.responses.len());

    // --- Weighted vote ---
    let result = orch.resolve_consensus(&responses, ConsensusStrategy::Weighted);
    println!("\n[Weighted Vote]");
    println!("  Decision: {:?}", result.decision);
    println!("  Weight breakdown: approve={:.0}, changes={:.0}", 3.0, 1.0);

    // --- Unanimous (will fail since they disagree) ---
    let result = orch.resolve_consensus(&responses, ConsensusStrategy::Unanimous);
    println!("\n[Unanimous Vote]");
    println!("  Decision: {:?}", result.decision);
    println!(
        "  Consensus reached: {} (expected: false)",
        result.consensus_reached
    );

    // --- Human-in-the-loop ---
    let result = orch.resolve_consensus(&responses, ConsensusStrategy::HumanInTheLoop);
    println!("\n[Human-in-the-Loop]");
    println!(
        "  Decision: {:?} (always None — human decides)",
        result.decision
    );
    for r in &result.responses {
        println!("    {} (weight={:.1}): {}", r.agent, r.weight, r.content);
    }

    // Show that ConsensusRequest is also available for structured workflows
    let _request = ConsensusRequest {
        prompt: "Should we merge PR #42?".into(),
        agents: vec!["reviewer-1".into(), "reviewer-2".into()],
        strategy: ConsensusStrategy::Majority,
        timeout: Duration::from_secs(30),
        weights: None,
    };
    println!("\n  (ConsensusRequest struct also available for structured workflows)");

    // =========================================================================
    // Feature 3: Agent Memory
    // =========================================================================
    println!("\n━━━ Feature 3: Agent Memory ━━━\n");

    // --- Pure in-memory usage (no backend needed) ---
    let config = MemoryConfig {
        max_turns: 3,
        budget_chars: 500,
    };
    let mut memory = ConversationMemory::new(config);

    println!("[Recording conversation turns]");
    memory.record_turn(Role::User, "Review the auth module for security issues.");
    println!("  User: Review the auth module for security issues.");

    memory.record_turn(
        Role::Assistant,
        "Found 2 issues: 1) JWT tokens not rotated, 2) No rate limiting on login endpoint.",
    );
    println!("  Assistant: Found 2 issues: JWT rotation + rate limiting.");

    memory.record_turn(Role::User, "Fix the JWT rotation issue first.");
    println!("  User: Fix the JWT rotation issue first.");

    memory.record_turn(
        Role::Assistant,
        "Implemented token rotation with 24h expiry and refresh token flow.",
    );
    println!("  Assistant: Implemented token rotation.");

    println!("\n[Memory state]");
    println!(
        "  Turns stored: {} (max_turns=3, oldest evicted)",
        memory.len()
    );

    let context = memory.format_context();
    println!("\n[Formatted context for injection]\n{context}");

    // --- Orchestrator-level memory (persistent, per-agent) ---
    println!("[Orchestrator memory management]");

    orch.enable_memory(
        "demo",
        "gemini-reviewer",
        MemoryConfig {
            max_turns: 5,
            budget_chars: 4000,
        },
    )
    .await?;
    println!("  Enabled memory for 'gemini-reviewer' (5 turns, 4000 chars)");

    // Simulate recording turns via orchestrator
    orch.record_assistant_output(
        "demo",
        "gemini-reviewer",
        "I've reviewed the codebase. The main concern is error handling in the API layer.",
    )
    .await?;
    println!("  Recorded assistant output for 'gemini-reviewer'");

    orch.disable_memory("demo", "gemini-reviewer").await?;
    println!("  Disabled memory for 'gemini-reviewer'");

    // --- Memory with file persistence ---
    println!("\n[File persistence]");
    let mgr = agent_teams::memory::MemoryManager::new(teams_dir.clone());
    let mut mem = ConversationMemory::with_defaults();
    mem.record_turn(Role::User, "Hello from persistent memory!");
    mgr.save("demo", "test-agent", &mem)?;
    println!(
        "  Saved memory to {}/demo/memory/test-agent.json",
        teams_dir.display()
    );

    let loaded = mgr.load("demo", "test-agent")?;
    println!(
        "  Loaded back: {} turn(s), content matches: {}",
        loaded.as_ref().map_or(0, |m| m.len()),
        loaded.is_some()
    );

    mgr.delete("demo", "test-agent")?;
    println!("  Deleted persisted memory");

    // =========================================================================
    // Cleanup
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    orch.delete_team("demo").await?;
    println!("Done! All features demonstrated without any real AI backends.");

    Ok(())
}
