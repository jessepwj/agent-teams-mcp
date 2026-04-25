//! Parallel Codex review: spawn multiple Codex agents to review files concurrently.
//!
//! Demonstrates how to use `agent-teams` to split a code review across N parallel
//! Codex agents with configurable model and reasoning effort, dramatically reducing
//! total wall-clock time compared to a single heavy agent.
//!
//! Run with:
//!   cargo run --example parallel_codex_review

use agent_teams::backend::codex::CodexBackend;
use agent_teams::backend::{AgentOutput, BackendType, SpawnConfig};
use agent_teams::orchestrator::TeamOrchestrator;
use std::time::{Duration, Instant};

/// Files to review (relative to the project root).
const REVIEW_TARGETS: &[(&str, &str)] = &[
    ("reviewer-1", "src/backend/mod.rs"),
    ("reviewer-2", "src/backend/codex.rs"),
    ("reviewer-3", "src/orchestrator/mod.rs"),
    ("reviewer-4", "src/team/mod.rs"),
];

#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("agent_teams=debug,info")
        .init();

    let tmp = tempfile::tempdir().expect("tempdir");
    let teams_dir = tmp.path().join("teams");
    let tasks_dir = tmp.path().join("tasks");

    let project_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    println!("╔═══════════════════════════════════════════════════╗");
    println!(
        "║  Parallel Codex Review ({} agents)               ║",
        REVIEW_TARGETS.len()
    );
    println!("╚═══════════════════════════════════════════════════╝\n");

    // Build orchestrator with Codex backend
    let codex_backend = CodexBackend::with_path("/opt/homebrew/bin/codex");
    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .with_codex(codex_backend)
        .build()?;

    orch.create_team("par-review", Some("Parallel code review"))
        .await?;

    // --- Spawn all reviewers in parallel ---
    let start = Instant::now();
    println!(
        "[1] Spawning {} Codex reviewers (effort: medium)...",
        REVIEW_TARGETS.len()
    );

    for &(name, file) in REVIEW_TARGETS {
        let code = std::fs::read_to_string(project_dir.join(file))
            .unwrap_or_else(|_| format!("(could not read {file})"));

        let prompt = format!(
            "You are a senior Rust engineer. Review this code for correctness, safety, \
             and best practices. Be concise — list only CRITICAL and WARNING issues.\n\n\
             File: {file}\n```rust\n{code}\n```"
        );

        let config = SpawnConfig {
            name: name.into(),
            prompt,
            model: None,                             // Use default (gpt-5.2-codex)
            reasoning_effort: Some("medium".into()), // Lower effort = faster (vs xhigh default)
            cwd: Some(project_dir.to_path_buf()),
            max_turns: Some(1),
            allowed_tools: vec![],
            permission_mode: None,
            env: Default::default(),
            memory_config: None,
            delegations: Vec::new(),
        };

        orch.spawn_teammate("par-review", config, BackendType::Codex)
            .await?;
        println!("  ✓ {name} spawned → {file}");
    }

    let spawn_time = start.elapsed();
    println!(
        "\n  All agents spawned in {:.1}s\n",
        spawn_time.as_secs_f64()
    );

    // --- Collect all outputs concurrently ---
    println!("[2] Collecting reviews...\n");

    let mut handles = Vec::new();

    for &(name, file) in REVIEW_TARGETS {
        let rx = orch.take_output_receiver("par-review", name).await?;
        let file = file.to_string();
        let name = name.to_string();

        handles.push(tokio::spawn(async move {
            let result = collect_output(rx, 120).await;
            (name, file, result)
        }));
    }

    // Wait for all reviews
    for handle in handles {
        let (name, file, text) = handle.await.unwrap();
        println!("─────────── {name}: {file} ───────────");
        if text.is_empty() {
            println!("  (no output received)");
        } else {
            println!("{text}");
        }
        println!();
    }

    let total_time = start.elapsed();
    println!("═══════════════════════════════════════════════════");
    println!("  Total wall-clock time: {:.1}s", total_time.as_secs_f64());
    println!("  ({} files reviewed in parallel)", REVIEW_TARGETS.len());
    println!("═══════════════════════════════════════════════════\n");

    // --- Cleanup ---
    println!("[3] Cleanup...");
    for &(name, _) in REVIEW_TARGETS {
        match orch.shutdown_teammate("par-review", name).await {
            Ok(()) => println!("  ✓ {name} shut down"),
            Err(_) => println!("  {name} already stopped"),
        }
    }
    orch.delete_team("par-review").await?;
    println!("  ✓ Team deleted\n");

    println!("Done!");
    Ok(())
}

/// Collect all output from an agent's receiver until turn completes or timeout.
async fn collect_output(
    rx: Option<tokio::sync::mpsc::Receiver<AgentOutput>>,
    timeout_secs: u64,
) -> String {
    let Some(mut rx) = rx else {
        return String::new();
    };

    let mut full_text = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(AgentOutput::Delta(text))) => {
                full_text.push_str(&text);
            }
            Ok(Some(AgentOutput::Message(text))) => {
                if !full_text.contains(&text) {
                    full_text.push_str(&text);
                }
            }
            Ok(Some(AgentOutput::TurnComplete | AgentOutput::Idle)) => break,
            Ok(Some(AgentOutput::Error(e))) => {
                full_text.push_str(&format!("\n[error] {e}"));
                break;
            }
            Ok(None) => break,
            Err(_) => {
                full_text.push_str(&format!("\n[timeout after {timeout_secs}s]"));
                break;
            }
        }
    }

    full_text
}
