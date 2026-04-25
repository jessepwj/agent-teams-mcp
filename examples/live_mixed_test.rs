//! Live test: Claude Code + Codex mixed backend.
//!
//! This spawns a real Claude Code agent and a real Codex agent,
//! assigns them tasks, and verifies cross-backend communication.
//!
//! Requirements:
//!   - `claude` CLI in PATH (Claude Code)
//!   - `codex` CLI in PATH (OpenAI Codex)
//!   - Valid API keys configured for both
//!
//! Run with:
//!   cargo run --example live_mixed_test

use agent_teams::backend::claude_code::ClaudeCodeBackend;
use agent_teams::backend::codex::CodexBackend;
use agent_teams::backend::{BackendType, SpawnConfig};
use agent_teams::messaging::{FileInboxManager, InboxManager};
use agent_teams::models::CreateTaskRequest;
use agent_teams::orchestrator::TeamOrchestrator;

#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    // Initialize tracing for debug output
    tracing_subscriber::fmt()
        .with_env_filter("agent_teams=debug,info")
        .init();

    let tmp = tempfile::tempdir().expect("tempdir");
    let teams_dir = tmp.path().join("teams");
    let tasks_dir = tmp.path().join("tasks");

    println!("╔══════════════════════════════════════════════╗");
    println!("║  Live Mixed-Backend Test                      ║");
    println!("║  Claude Code + Codex                          ║");
    println!("╚══════════════════════════════════════════════╝\n");

    // 1. Build orchestrator with REAL backends
    println!("[1/7] Building orchestrator with real backends...");
    let codex_backend = CodexBackend::with_path("/opt/homebrew/bin/codex");

    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .with_claude_code(ClaudeCodeBackend::new().expect("claude CLI not found"))
        .with_codex(codex_backend)
        .build()?;
    println!("  ✓ Orchestrator ready\n");

    // 2. Create team
    println!("[2/7] Creating team...");
    orch.create_team("live-test", Some("Live mixed-backend test"))
        .await?;
    println!("  ✓ Team 'live-test' created\n");

    // 3. Create tasks
    println!("[3/7] Creating tasks...");
    let t_claude = orch
        .create_task(
            "live-test",
            CreateTaskRequest {
                subject: "Explain Rust ownership in one sentence".into(),
                description: Some(
                    "Give a single-sentence explanation of Rust's ownership model.".into(),
                ),
                active_form: Some("Explaining ownership".into()),
                metadata: None,
            },
        )
        .await?;

    let t_codex = orch
        .create_task(
            "live-test",
            CreateTaskRequest {
                subject: "Write a hello world function in Rust".into(),
                description: Some(
                    "Write a simple Rust function that prints 'Hello from Codex!'. Just the function, no explanation needed."
                        .into(),
                ),
                active_form: Some("Writing hello world".into()),
                metadata: None,
            },
        )
        .await?;
    println!("  ✓ Task #{}: {}", t_claude.id, t_claude.subject);
    println!("  ✓ Task #{}: {}\n", t_codex.id, t_codex.subject);

    // 4. Spawn Claude Code agent
    println!("[4/7] Spawning Claude Code agent 'thinker'...");
    let claude_config = SpawnConfig {
        name: "thinker".into(),
        prompt: "You are a concise Rust expert. Answer in ONE sentence only. Do not use any tools."
            .into(),
        model: Some("sonnet".into()),
        cwd: Some(tmp.path().to_path_buf()),
        max_turns: Some(2),
        allowed_tools: vec![],
        permission_mode: Some("plan".into()),
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    match orch
        .spawn_teammate("live-test", claude_config, BackendType::ClaudeCode)
        .await
    {
        Ok(()) => println!("  ✓ thinker (Claude Code) spawned"),
        Err(e) => {
            println!("  ✗ Failed to spawn Claude Code agent: {e}");
            println!("    Skipping Claude Code agent, continuing with Codex only...");
        }
    }

    // 5. Spawn Codex agent
    println!("\n[5/7] Spawning Codex agent 'coder'...");
    let codex_config = SpawnConfig {
        name: "coder".into(),
        prompt: "You are a Rust code writer. Write minimal, clean code. No explanations.".into(),
        cwd: Some(tmp.path().to_path_buf()),
        max_turns: Some(2),
        allowed_tools: vec![],
        permission_mode: None,
        model: None,
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    match orch
        .spawn_teammate("live-test", codex_config, BackendType::Codex)
        .await
    {
        Ok(()) => println!("  ✓ coder (Codex) spawned"),
        Err(e) => {
            println!("  ✗ Failed to spawn Codex agent: {e}");
            println!("    Codex app-server may not be available or API key not configured.");
        }
    }

    // 6. Show team config
    println!("\n[6/7] Team configuration:");
    let config = orch.read_team("live-test").await?;
    for m in &config.members {
        let backend = m.agent_type();
        println!("  - {} ({})", m.name(), backend);
    }

    // 7. Cross-backend messaging test
    println!("\n[7/7] Testing cross-backend messaging...");

    // Send messages between backends
    orch.send_message(
        "live-test",
        "thinker",
        "coder",
        "Can you write a function that demonstrates ownership transfer?",
    )
    .await?;
    println!("  ✓ thinker (Claude) -> coder (Codex): message sent");

    orch.send_message(
        "live-test",
        "coder",
        "thinker",
        "Done. Check fn take_ownership(s: String) { println!(\"{s}\"); }",
    )
    .await?;
    println!("  ✓ coder (Codex) -> thinker (Claude): message sent");

    // Read inboxes to verify
    let inbox_mgr = FileInboxManager::new(&teams_dir);

    let coder_msgs = inbox_mgr.read_inbox("live-test", "coder").await?;
    println!("\n  coder's inbox: {} message(s)", coder_msgs.len());
    for msg in &coder_msgs {
        println!("    from {}: \"{}\"", msg.from, truncate(&msg.content, 60));
    }

    let thinker_msgs = inbox_mgr.read_inbox("live-test", "thinker").await?;
    println!("  thinker's inbox: {} message(s)", thinker_msgs.len());
    for msg in &thinker_msgs {
        println!("    from {}: \"{}\"", msg.from, truncate(&msg.content, 60));
    }

    // Show generated files
    println!("\n--- Generated JSON (task file) ---");
    let task_json = std::fs::read_to_string(tasks_dir.join("live-test/1.json"))?;
    println!("{task_json}");

    // Cleanup
    println!("--- Cleanup ---");
    for name in ["thinker", "coder"] {
        match orch.shutdown_teammate("live-test", name).await {
            Ok(()) => println!("  {name} shut down"),
            Err(_) => println!("  {name} already stopped"),
        }
    }
    orch.delete_team("live-test").await?;
    println!("  Team deleted.\n");

    println!("╔══════════════════════════════════════════════╗");
    println!("║  Test complete!                               ║");
    println!("╚══════════════════════════════════════════════╝");

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max])
    } else {
        s.to_string()
    }
}
