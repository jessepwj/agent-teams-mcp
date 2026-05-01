//! Mixed parallel review: Claude Code + parallel Codex agents.
//!
//! Demonstrates the full power of `agent-teams` by combining:
//!   - Multiple Codex agents reviewing source files in parallel (fast, targeted)
//!   - One Claude Code agent synthesizing a high-level architecture review
//!
//! This is "方案 C" — using the agent-teams library to orchestrate mixed backends.
//!
//! Requirements:
//!   - `claude` CLI in PATH
//!   - `codex` CLI in PATH (or /opt/homebrew/bin/codex)
//!   - Valid API keys for both
//!
//! Run with:
//!   cargo run --example mixed_parallel_review

use agent_teams::backend::claude_code::ClaudeCodeBackend;
use agent_teams::backend::codex::CodexBackend;
use agent_teams::backend::{AgentOutput, BackendType, SpawnConfig};
use agent_teams::orchestrator::TeamOrchestrator;
use std::time::{Duration, Instant};

/// Codex agents: each reviews one file in parallel.
const CODEX_TARGETS: &[(&str, &str)] = &[
    ("codex-1", "src/backend/codex.rs"),
    ("codex-2", "src/orchestrator/mod.rs"),
    ("codex-3", "src/backend/claude_code.rs"),
    ("codex-4", "src/team/mod.rs"),
    ("codex-5", "src/task/mod.rs"),
];

#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("agent_teams=info,warn")
        .init();

    let tmp = tempfile::tempdir().expect("tempdir");
    let teams_dir = tmp.path().join("teams");
    let tasks_dir = tmp.path().join("tasks");
    let project_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    let codex_count = CODEX_TARGETS.len();
    let total_agents = codex_count + 1; // +1 for Claude Code

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║  Mixed Parallel Review                               ║");
    println!("║  1× Claude Code (architecture) + {codex_count}× Codex (per-file)  ║");
    println!("║  Total: {total_agents} agents                                    ║");
    println!("╚══════════════════════════════════════════════════════╝\n");

    // --- Build orchestrator with BOTH backends ---
    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .with_claude_code(ClaudeCodeBackend::new().expect("claude CLI not found"))
        .with_codex(CodexBackend::with_path("/opt/homebrew/bin/codex"))
        .build()?;

    orch.create_team("mixed-review", Some("Mixed Claude Code + Codex review"))
        .await?;

    let start = Instant::now();

    // --- Phase 1: Spawn all agents concurrently ---
    println!("[1] Spawning {total_agents} agents...\n");

    // 1a. Spawn Claude Code agent for architecture review
    let arch_prompt = build_architecture_prompt(project_dir);
    let claude_config = SpawnConfig {
        name: "architect".into(),
        prompt: arch_prompt,
        model: Some("sonnet".into()),
        cwd: Some(project_dir.to_path_buf()),
        max_turns: Some(2),
        allowed_tools: vec![],
        permission_mode: Some("plan".into()),
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    match orch
        .spawn_teammate("mixed-review", claude_config, BackendType::ClaudeCode)
        .await
    {
        Ok(()) => println!("  ✓ architect (Claude Code/Sonnet) spawned → architecture review"),
        Err(e) => {
            println!("  ✗ architect failed: {e}");
            println!("    (continuing with Codex agents only)");
        }
    }

    // 1b. Spawn Codex agents for per-file review
    for &(name, file) in CODEX_TARGETS {
        let code = std::fs::read_to_string(project_dir.join(file))
            .unwrap_or_else(|_| format!("(could not read {file})"));

        let prompt = format!(
            "You are a senior Rust engineer. Review this file for correctness, safety, \
             and concurrency issues. List only CRITICAL and WARNING findings with line numbers.\n\n\
             File: {file}\n```rust\n{code}\n```"
        );

        let config = SpawnConfig {
            name: name.into(),
            prompt,
            model: None,                             // default gpt-5.3-codex
            reasoning_effort: Some("medium".into()), // override global config
            cwd: Some(project_dir.to_path_buf()),
            max_turns: Some(1),
            allowed_tools: vec![],
            permission_mode: None,
            env: Default::default(),
            memory_config: None,
            delegations: Vec::new(),
        };

        orch.spawn_teammate("mixed-review", config, BackendType::Codex)
            .await?;
        println!("  ✓ {name} (Codex/medium) spawned → {file}");
    }

    let spawn_time = start.elapsed();
    println!(
        "\n  All {total_agents} agents spawned in {:.1}s\n",
        spawn_time.as_secs_f64()
    );

    // --- Phase 2: Collect all outputs concurrently ---
    println!("[2] Collecting reviews (all agents working in parallel)...\n");

    let mut handles = Vec::new();

    // Collect architect output
    {
        let rx = orch
            .take_output_receiver("mixed-review", "architect")
            .await?;
        handles.push(tokio::spawn(async move {
            let result = collect_output(rx, 120).await;
            ("architect".to_string(), "architecture".to_string(), result)
        }));
    }

    // Collect all Codex outputs
    for &(name, file) in CODEX_TARGETS {
        let rx = orch.take_output_receiver("mixed-review", name).await?;
        let file = file.to_string();
        let name = name.to_string();
        handles.push(tokio::spawn(async move {
            let result = collect_output(rx, 120).await;
            (name, file, result)
        }));
    }

    // Wait for all
    for handle in handles {
        let (name, target, text) = handle.await.unwrap();
        let backend = if name == "architect" {
            "Claude Code"
        } else {
            "Codex"
        };
        println!("─────────── [{backend}] {name}: {target} ───────────");
        if text.is_empty() {
            println!("  (no output received)");
        } else {
            println!("{text}");
        }
        println!();
    }

    let total_time = start.elapsed();
    println!("═══════════════════════════════════════════════════════");
    println!("  Total wall-clock time: {:.1}s", total_time.as_secs_f64());
    println!(
        "  (1 Claude Code + {} Codex agents in parallel)",
        codex_count
    );
    println!("═══════════════════════════════════════════════════════\n");

    // --- Phase 3: Cleanup ---
    println!("[3] Cleanup...");
    for name in std::iter::once("architect").chain(CODEX_TARGETS.iter().map(|(n, _)| *n)) {
        match orch.shutdown_teammate("mixed-review", name).await {
            Ok(()) => println!("  ✓ {name} shut down"),
            Err(_) => println!("  {name} already stopped"),
        }
    }
    orch.delete_team("mixed-review").await?;
    println!("  ✓ Team deleted\n");

    println!("Done!");
    Ok(())
}

/// Build the architecture-level review prompt for the Claude Code agent.
fn build_architecture_prompt(project_dir: &std::path::Path) -> String {
    // Read key files for context
    let lib_rs = std::fs::read_to_string(project_dir.join("src/lib.rs")).unwrap_or_default();
    let backend_mod =
        std::fs::read_to_string(project_dir.join("src/backend/mod.rs")).unwrap_or_default();
    let error_rs = std::fs::read_to_string(project_dir.join("src/error.rs")).unwrap_or_default();

    format!(
        "You are a senior Rust architect. Review the overall architecture of this agent-teams library. \
         Focus on:\n\
         1. Trait design (AgentBackend + AgentSession separation)\n\
         2. Error handling strategy\n\
         3. Module organization and public API surface\n\
         4. Cross-cutting concerns (async safety, platform portability)\n\n\
         Be concise — list CRITICAL, WARNING, and key SUGGESTION items only.\n\
         Do not use any tools. Just analyze the code provided.\n\n\
         === src/lib.rs ===\n```rust\n{lib_rs}\n```\n\n\
         === src/error.rs ===\n```rust\n{error_rs}\n```\n\n\
         === src/backend/mod.rs ===\n```rust\n{backend_mod}\n```"
    )
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
            Ok(Some(AgentOutput::ToolOutput(_))) => {}
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
