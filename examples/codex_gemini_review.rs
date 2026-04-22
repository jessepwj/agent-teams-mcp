//! Codex + Gemini parallel code review with DAG task management.
//!
//! Demonstrates a two-phase review pipeline:
//!   Phase 1 (parallel): Codex reviews low-level code quality,
//!                        Gemini reviews architecture/design
//!   Phase 2 (depends on Phase 1): Synthesize findings into a summary
//!
//! The DAG ensures Phase 2 doesn't start until both Phase 1 tasks complete.
//!
//! Run with:
//!   cargo run --example codex_gemini_review

use std::time::{Duration, Instant};

use agent_teams::backend::codex::CodexBackend;
use agent_teams::backend::gemini::GeminiCliBackend;
use agent_teams::backend::{AgentOutput, BackendType, SpawnConfig};
use agent_teams::consensus::{AgentResponse, ConsensusStrategy};
use agent_teams::models::{CreateTaskRequest, TaskStatus, TaskUpdate};
use agent_teams::orchestrator::TeamOrchestrator;


#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("agent_teams=info,warn")
        .init();

    let tmp = tempfile::tempdir().expect("tempdir");
    let teams_dir = tmp.path().join("teams");
    let tasks_dir = tmp.path().join("tasks");
    let project_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    println!("╔═══════════════════════════════════════════════════╗");
    println!("║  Codex + Gemini Code Review (with DAG)            ║");
    println!("║  Phase 1: Parallel review  →  Phase 2: Synthesis  ║");
    println!("╚═══════════════════════════════════════════════════╝\n");

    // --- Build orchestrator with Codex + Gemini backends ---
    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .with_codex(CodexBackend::with_path("/opt/homebrew/bin/codex"))
        .with_gemini_cli(GeminiCliBackend::with_path("/opt/homebrew/bin/gemini"))
        .build()?;

    orch.create_team("review", Some("Codex+Gemini code review"))
        .await?;

    let start = Instant::now();

    // =========================================================================
    // Step 1: Create task DAG
    // =========================================================================
    println!("[1/5] Creating task DAG...\n");

    let t_codex = orch
        .create_task(
            "review",
            CreateTaskRequest {
                subject: "Codex: Low-level code review".into(),
                description: Some(
                    "Review backend/mod.rs for correctness, safety, and concurrency issues"
                        .into(),
                ),
                active_form: Some("Codex reviewing code".into()),
                metadata: None,
            },
        )
        .await?;

    let t_gemini = orch
        .create_task(
            "review",
            CreateTaskRequest {
                subject: "Gemini: Architecture review".into(),
                description: Some(
                    "Review overall architecture of lib.rs + error.rs for design issues".into(),
                ),
                active_form: Some("Gemini reviewing architecture".into()),
                metadata: None,
            },
        )
        .await?;

    let t_synthesis = orch
        .create_task(
            "review",
            CreateTaskRequest {
                subject: "Synthesize review findings".into(),
                description: Some(
                    "Combine Codex + Gemini findings into actionable items".into(),
                ),
                active_form: Some("Synthesizing findings".into()),
                metadata: None,
            },
        )
        .await?;

    // Phase 2 blocked by BOTH Phase 1 tasks
    orch.update_task(
        "review",
        &t_synthesis.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t_codex.id.clone(), t_gemini.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    // --- Visualize the DAG ---
    let mermaid = orch.export_task_graph_mermaid("review").await?;
    println!("  Task DAG (Mermaid):");
    for line in mermaid.lines() {
        println!("    {line}");
    }

    let cp = orch.get_critical_path("review").await?;
    println!(
        "  Critical path: {}",
        cp.iter()
            .map(|t| format!("#{}", t.id))
            .collect::<Vec<_>>()
            .join(" → ")
    );

    // =========================================================================
    // Step 2: Spawn agents (Phase 1 — parallel)
    // =========================================================================
    println!("\n[2/5] Spawning Phase 1 agents (parallel)...\n");

    // Mark Phase 1 tasks as in-progress
    orch.update_task(
        "review",
        &t_codex.id,
        TaskUpdate {
            status: Some(TaskStatus::InProgress),
            owner: Some("codex-reviewer".into()),
            ..Default::default()
        },
    )
    .await?;

    orch.update_task(
        "review",
        &t_gemini.id,
        TaskUpdate {
            status: Some(TaskStatus::InProgress),
            owner: Some("gemini-reviewer".into()),
            ..Default::default()
        },
    )
    .await?;

    // Read target files for review
    let backend_mod =
        std::fs::read_to_string(project_dir.join("src/backend/mod.rs")).unwrap_or_default();
    let lib_rs = std::fs::read_to_string(project_dir.join("src/lib.rs")).unwrap_or_default();
    let error_rs = std::fs::read_to_string(project_dir.join("src/error.rs")).unwrap_or_default();

    // Spawn Codex agent
    let codex_prompt = format!(
        "You are a senior Rust engineer doing code review. \
         Review this file for correctness, safety, concurrency, and Rust idiom issues. \
         List only CRITICAL and WARNING findings with line numbers. Be concise.\n\n\
         File: src/backend/mod.rs\n```rust\n{backend_mod}\n```"
    );

    let codex_config = SpawnConfig {
        name: "codex-reviewer".into(),
        prompt: codex_prompt,
        model: None,
        cwd: Some(project_dir.to_path_buf()),
        max_turns: Some(1),
        allowed_tools: vec![],
        permission_mode: None,
        reasoning_effort: Some("high".into()),
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    orch.spawn_teammate("review", codex_config, BackendType::Codex)
        .await?;
    println!("  ✓ codex-reviewer spawned → src/backend/mod.rs");

    // Spawn Gemini agent
    let gemini_prompt = format!(
        "You are a senior Rust architect. Review this library's overall design. Focus on:\n\
         1. Module organization and public API surface\n\
         2. Error handling strategy\n\
         3. Trait design patterns\n\
         List CRITICAL, WARNING, and SUGGESTION items. Be concise (max 500 words).\n\n\
         === src/lib.rs ===\n```rust\n{lib_rs}\n```\n\n\
         === src/error.rs ===\n```rust\n{error_rs}\n```"
    );

    let gemini_config = SpawnConfig {
        name: "gemini-reviewer".into(),
        prompt: gemini_prompt,
        model: Some("gemini-2.5-flash".into()),
        cwd: Some(project_dir.to_path_buf()),
        max_turns: Some(1),
        allowed_tools: vec![],
        permission_mode: None,
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    orch.spawn_teammate("review", gemini_config, BackendType::GeminiCli)
        .await?;
    println!("  ✓ gemini-reviewer spawned → lib.rs + error.rs");

    let spawn_time = start.elapsed();
    println!(
        "\n  Both agents spawned in {:.1}s\n",
        spawn_time.as_secs_f64()
    );

    // =========================================================================
    // Step 3: Collect outputs (parallel)
    // =========================================================================
    println!("[3/5] Collecting reviews (both agents working in parallel)...\n");

    let codex_rx = orch
        .take_output_receiver("review", "codex-reviewer")
        .await?;
    let gemini_rx = orch
        .take_output_receiver("review", "gemini-reviewer")
        .await?;

    let codex_handle = tokio::spawn(async move { collect_output(codex_rx, 120).await });
    let gemini_handle = tokio::spawn(async move { collect_output(gemini_rx, 120).await });

    let codex_output = codex_handle.await.unwrap();
    let gemini_output = gemini_handle.await.unwrap();

    // Mark Phase 1 complete
    orch.update_task(
        "review",
        &t_codex.id,
        TaskUpdate {
            status: Some(TaskStatus::Completed),
            ..Default::default()
        },
    )
    .await?;

    orch.update_task(
        "review",
        &t_gemini.id,
        TaskUpdate {
            status: Some(TaskStatus::Completed),
            ..Default::default()
        },
    )
    .await?;

    let review_time = start.elapsed();

    // =========================================================================
    // Step 4: Display results
    // =========================================================================
    println!("[4/5] Review results ({:.1}s wall time):\n", review_time.as_secs_f64());

    println!("─────────── [Codex] Low-level Code Review ───────────");
    if codex_output.is_empty() {
        println!("  (no output — Codex may need API key or app-server)");
    } else {
        println!("{codex_output}");
    }

    println!("\n─────────── [Gemini] Architecture Review ───────────");
    if gemini_output.is_empty() {
        println!("  (no output — Gemini may need API key)");
    } else {
        println!("{gemini_output}");
    }

    // =========================================================================
    // Step 5: Consensus + DAG synthesis
    // =========================================================================
    println!("\n[5/5] Phase 2: Synthesis + Consensus\n");

    // Now the synthesis task is unblocked
    let next = orch.get_next_available_task("review").await?;
    if let Some(task) = &next {
        println!(
            "  DAG check: #{} \"{}\" is now unblocked!",
            task.id, task.subject
        );
    }

    // Use consensus to compare the two reviews
    let responses = vec![
        AgentResponse {
            agent: "codex-reviewer".into(),
            content: if codex_output.is_empty() {
                "no output".into()
            } else {
                summarize_first_line(&codex_output)
            },
            weight: 1.0,
            timed_out: codex_output.is_empty(),
        },
        AgentResponse {
            agent: "gemini-reviewer".into(),
            content: if gemini_output.is_empty() {
                "no output".into()
            } else {
                summarize_first_line(&gemini_output)
            },
            weight: 1.0,
            timed_out: gemini_output.is_empty(),
        },
    ];

    let result = orch.resolve_consensus(&responses, ConsensusStrategy::HumanInTheLoop);
    println!("\n  Consensus (HumanInTheLoop):");
    println!("  Decision: {:?} (human reviews all findings)", result.decision);
    for r in &result.responses {
        let status = if r.timed_out { "TIMEOUT" } else { "OK" };
        println!("    [{status}] {}: {}", r.agent, r.content);
    }

    // Final DAG visualization with updated statuses
    println!("\n  Final DAG state:");
    let mermaid = orch.export_task_graph_mermaid("review").await?;
    for line in mermaid.lines() {
        println!("    {line}");
    }

    // Cleanup
    println!("\n--- Cleanup ---");
    for name in ["codex-reviewer", "gemini-reviewer"] {
        match orch.shutdown_teammate("review", name).await {
            Ok(()) => println!("  ✓ {name} shut down"),
            Err(_) => println!("  {name} already stopped"),
        }
    }
    orch.delete_team("review").await?;

    let total = start.elapsed();
    println!("\n═══════════════════════════════════════════");
    println!("  Total wall time: {:.1}s", total.as_secs_f64());
    println!("  Codex + Gemini reviewed in parallel");
    println!("═══════════════════════════════════════════");

    Ok(())
}

/// Collect all output from an agent until turn completion or timeout.
async fn collect_output(
    rx: Option<tokio::sync::mpsc::Receiver<AgentOutput>>,
    timeout_secs: u64,
) -> String {
    let Some(mut rx) = rx else {
        return String::new();
    };

    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(AgentOutput::Delta(d))) => text.push_str(&d),
            Ok(Some(AgentOutput::Message(m))) => {
                if !text.contains(&m) {
                    text.push_str(&m);
                }
            }
            Ok(Some(AgentOutput::TurnComplete | AgentOutput::Idle)) => break,
            Ok(Some(AgentOutput::Error(e))) => {
                text.push_str(&format!("\n[error] {e}"));
                break;
            }
            Ok(None) => break,
            Err(_) => {
                text.push_str(&format!("\n[timeout after {timeout_secs}s]"));
                break;
            }
        }
    }

    text
}

/// Extract first non-empty line as a summary.
fn summarize_first_line(text: &str) -> String {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("(empty)")
        .chars()
        .take(120)
        .collect()
}
