//! Live test: Drive a real Codex agent through a full conversation.
//!
//! This spawns a Codex agent, sends it a prompt, and captures the streaming output.
//! It also spawns a Claude Code agent for comparison.
//!
//! Run with:
//!   cargo run --example live_codex_conversation

use agent_teams::backend::claude_code::ClaudeCodeBackend;
use agent_teams::backend::codex::CodexBackend;
use agent_teams::backend::{AgentOutput, BackendType, SpawnConfig};
use agent_teams::orchestrator::TeamOrchestrator;
use std::time::Duration;

#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("agent_teams=debug,info")
        .init();

    let tmp = tempfile::tempdir().expect("tempdir");
    let teams_dir = tmp.path().join("teams");
    let tasks_dir = tmp.path().join("tasks");

    println!("╔═══════════════════════════════════════════════╗");
    println!("║  Live Codex + Claude Code Conversation Test   ║");
    println!("╚═══════════════════════════════════════════════╝\n");

    // Build orchestrator
    let codex_backend = CodexBackend::with_path("/opt/homebrew/bin/codex");
    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .with_claude_code(ClaudeCodeBackend::new().expect("claude CLI not found"))
        .with_codex(codex_backend)
        .build()?;

    orch.create_team("conv-test", Some("Conversation test"))
        .await?;

    // --- Spawn Codex agent ---
    println!("[1] Spawning Codex agent 'codex-coder'...");
    let codex_config = SpawnConfig {
        name: "codex-coder".into(),
        prompt: "Write a Rust function called `greet` that takes a name (&str) and returns a greeting String. Only output the code, no explanation.".into(),
        cwd: Some(tmp.path().to_path_buf()),
        max_turns: Some(1),
        allowed_tools: vec![],
        permission_mode: None,
        model: None,
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    orch.spawn_teammate("conv-test", codex_config, BackendType::Codex)
        .await?;
    println!("  ✓ codex-coder spawned\n");

    // --- Spawn Claude Code agent ---
    println!("[2] Spawning Claude Code agent 'claude-thinker'...");
    let claude_config = SpawnConfig {
        name: "claude-thinker".into(),
        prompt:
            "What is Rust's ownership model? Answer in exactly ONE sentence. Do not use any tools."
                .into(),
        model: Some("haiku".into()),
        cwd: Some(tmp.path().to_path_buf()),
        max_turns: Some(1),
        allowed_tools: vec![],
        permission_mode: Some("plan".into()),
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    orch.spawn_teammate("conv-test", claude_config, BackendType::ClaudeCode)
        .await?;
    println!("  ✓ claude-thinker spawned\n");

    // --- Collect Codex output ---
    println!("[3] Collecting Codex output (streaming)...");
    println!("─────────────── Codex Response ───────────────");

    let codex_output = collect_output(&orch, "conv-test", "codex-coder", 30).await;
    if codex_output.is_empty() {
        println!("  (no text output received - agent may still be processing)");
    } else {
        println!("{codex_output}");
    }
    println!("──────────────────────────────────────────────\n");

    // --- Collect Claude Code output ---
    println!("[4] Collecting Claude Code output (streaming)...");
    println!("──────────── Claude Code Response ────────────");

    let claude_output = collect_output(&orch, "conv-test", "claude-thinker", 30).await;
    if claude_output.is_empty() {
        println!("  (no text output received)");
    } else {
        println!("{claude_output}");
    }
    println!("──────────────────────────────────────────────\n");

    // --- Cleanup ---
    println!("[5] Cleanup...");
    for name in ["codex-coder", "claude-thinker"] {
        match orch.shutdown_teammate("conv-test", name).await {
            Ok(()) => println!("  ✓ {name} shut down"),
            Err(_) => println!("  {name} already stopped"),
        }
    }
    orch.delete_team("conv-test").await?;
    println!("  ✓ Team deleted\n");

    println!("╔═══════════════════════════════════════════════╗");
    println!("║  Done!                                        ║");
    println!("╚═══════════════════════════════════════════════╝");

    Ok(())
}

/// Collect all output from an agent until turn completes or timeout.
async fn collect_output(
    orch: &TeamOrchestrator,
    team: &str,
    agent: &str,
    timeout_secs: u64,
) -> String {
    let mut rx = match orch.take_output_receiver(team, agent).await {
        Ok(Some(rx)) => rx,
        Ok(None) => {
            eprintln!("  [warn] No output receiver for {agent}");
            return String::new();
        }
        Err(e) => {
            eprintln!("  [warn] Failed to get output receiver for {agent}: {e}");
            return String::new();
        }
    };

    let mut full_text = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(AgentOutput::Delta(text))) => {
                print!("{text}");
                full_text.push_str(&text);
            }
            Ok(Some(AgentOutput::Message(text))) => {
                if !full_text.contains(&text) {
                    println!("{text}");
                    full_text.push_str(&text);
                }
            }
            Ok(Some(AgentOutput::ToolOutput(text))) => {
                eprint!("{text}");
            }
            Ok(Some(AgentOutput::TurnComplete)) => {
                if !full_text.ends_with('\n') {
                    println!();
                }
                break;
            }
            Ok(Some(AgentOutput::Idle)) => {
                break;
            }
            Ok(Some(AgentOutput::Error(e))) => {
                eprintln!("\n  [error] {e}");
                break;
            }
            Ok(None) => {
                break;
            }
            Err(_) => {
                eprintln!("\n  [timeout after {timeout_secs}s]");
                break;
            }
        }
    }

    full_text
}
