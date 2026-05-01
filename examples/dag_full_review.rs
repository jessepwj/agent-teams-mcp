//! Full 3-phase DAG code review with CC + Codex + Gemini.
//!
//! Diamond DAG:
//!   Phase 1 (parallel):  T1(CC), T2(Codex), T3(Gemini)
//!   Phase 2 (partial):   T4(Codex, blocked by T1+T2), T5(Gemini, blocked by T2+T3)
//!   Phase 3 (final):     T6(CC, blocked by T4+T5)
//!
//! Run with:
//!   cargo run --example dag_full_review

use std::collections::HashMap;
use std::time::{Duration, Instant};

use agent_teams::backend::claude_code::ClaudeCodeBackend;
use agent_teams::backend::codex::CodexBackend;
use agent_teams::backend::gemini::GeminiCliBackend;
use agent_teams::backend::{AgentOutput, BackendType, SpawnConfig};
use agent_teams::models::{CreateTaskRequest, TaskStatus, TaskUpdate};
use agent_teams::orchestrator::TeamOrchestrator;

const TEAM: &str = "dag-review";
const TIMEOUT_SECS: u64 = 120;

#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("agent_teams=info,warn")
        .init();

    let tmp = tempfile::tempdir().expect("tempdir");
    let teams_dir = tmp.path().join("teams");
    let tasks_dir = tmp.path().join("tasks");
    let project_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║  Diamond DAG Review: CC + Codex + Gemini              ║");
    println!("║  3 Phases · 6 Tasks · 3 Backends                     ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");

    // --- Build orchestrator with ALL THREE backends ---
    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .with_claude_code(ClaudeCodeBackend::new().expect("claude CLI not found"))
        .with_codex(CodexBackend::with_path("/opt/homebrew/bin/codex"))
        .with_gemini_cli(GeminiCliBackend::with_path("/opt/homebrew/bin/gemini"))
        .build()?;

    orch.create_team(TEAM, Some("Diamond DAG full review"))
        .await?;

    let start = Instant::now();

    // =========================================================================
    // Step 1: Create the full DAG
    // =========================================================================
    println!("[1/6] Creating diamond DAG (6 tasks, 3 phases)...\n");

    let t1 = orch
        .create_task(
            TEAM,
            CreateTaskRequest {
                subject: "CC: Concurrency analysis".into(),
                description: Some("Analyze async patterns in orchestrator and backend".into()),
                active_form: Some("CC analyzing concurrency".into()),
                metadata: None,
            },
        )
        .await?;

    let t2 = orch
        .create_task(
            TEAM,
            CreateTaskRequest {
                subject: "Codex: Security audit".into(),
                description: Some("Audit for env leaks, input validation, unsafe patterns".into()),
                active_form: Some("Codex auditing security".into()),
                metadata: None,
            },
        )
        .await?;

    let t3 = orch
        .create_task(
            TEAM,
            CreateTaskRequest {
                subject: "Gemini: API design review".into(),
                description: Some("Review trait hierarchy and public API surface".into()),
                active_form: Some("Gemini reviewing API".into()),
                metadata: None,
            },
        )
        .await?;

    let t4 = orch
        .create_task(
            TEAM,
            CreateTaskRequest {
                subject: "Codex: Fix proposals".into(),
                description: Some("Propose fixes for concurrency + security findings".into()),
                active_form: Some("Codex proposing fixes".into()),
                metadata: None,
            },
        )
        .await?;

    let t5 = orch
        .create_task(
            TEAM,
            CreateTaskRequest {
                subject: "Gemini: Refactoring roadmap".into(),
                description: Some(
                    "Create refactoring priorities from security + API findings".into(),
                ),
                active_form: Some("Gemini planning refactoring".into()),
                metadata: None,
            },
        )
        .await?;

    let t6 = orch
        .create_task(
            TEAM,
            CreateTaskRequest {
                subject: "CC: Final synthesis report".into(),
                description: Some("Combine all findings into actionable report".into()),
                active_form: Some("CC synthesizing report".into()),
                metadata: None,
            },
        )
        .await?;

    // Wire up the diamond DAG
    // T4 blocked by T1 + T2
    orch.update_task(
        TEAM,
        &t4.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t1.id.clone(), t2.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    // T5 blocked by T2 + T3
    orch.update_task(
        TEAM,
        &t5.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t2.id.clone(), t3.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    // T6 blocked by T4 + T5
    orch.update_task(
        TEAM,
        &t6.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t4.id.clone(), t5.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    // Show the initial DAG (terminal rendering with ANSI colors)
    let dag_view = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag_view}");

    // =========================================================================
    // Step 2: Phase 1 — Three parallel reviews (CC + Codex + Gemini)
    // =========================================================================
    println!("[2/6] Phase 1: Spawning 3 agents in parallel...\n");

    // Mark Phase 1 tasks in-progress
    for (tid, owner) in [
        (&t1.id, "cc-analyst"),
        (&t2.id, "codex-auditor"),
        (&t3.id, "gemini-reviewer"),
    ] {
        orch.update_task(
            TEAM,
            tid,
            TaskUpdate {
                status: Some(TaskStatus::InProgress),
                owner: Some(owner.into()),
                ..Default::default()
            },
        )
        .await?;
    }

    // Read source files
    let orch_mod = read_file(project_dir, "src/orchestrator/mod.rs", 200);
    let backend_mod = read_file(project_dir, "src/backend/mod.rs", 0);
    let codex_rs = read_file(project_dir, "src/backend/codex.rs", 120);
    let gemini_rs = read_file(project_dir, "src/backend/gemini.rs", 0);
    let lib_rs = read_file(project_dir, "src/lib.rs", 0);
    let error_rs = read_file(project_dir, "src/error.rs", 0);

    // T1: CC — Concurrency analysis
    let cc_prompt = format!(
        "You are a Rust concurrency expert. Analyze these async patterns for:\n\
         1. Deadlock risks (Mutex held across .await)\n\
         2. AtomicBool ordering correctness (Relaxed on ARM)\n\
         3. Channel backpressure / liveness issues\n\
         4. Race conditions in session lifecycle\n\n\
         List findings as CRITICAL/WARNING with line numbers. Max 400 words. \
         Do NOT use any tools — analyze only the code below.\n\n\
         === src/orchestrator/mod.rs (first 200 lines) ===\n```rust\n{orch_mod}\n```\n\n\
         === src/backend/mod.rs ===\n```rust\n{backend_mod}\n```"
    );

    let cc_config = SpawnConfig {
        name: "cc-analyst".into(),
        prompt: cc_prompt,
        model: Some("sonnet".into()),
        cwd: Some(project_dir.to_path_buf()),
        max_turns: Some(2),
        allowed_tools: vec![],
        permission_mode: None,
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    match orch
        .spawn_teammate(TEAM, cc_config, BackendType::ClaudeCode)
        .await
    {
        Ok(()) => println!("  ✓ cc-analyst (Claude Code/Sonnet) → concurrency analysis"),
        Err(e) => println!("  ✗ cc-analyst failed: {e}"),
    }

    // T2: Codex — Security audit
    let codex_prompt = format!(
        "You are a security auditor for Rust code. Focus on:\n\
         1. Secrets in Debug output (SpawnConfig with env HashMap)\n\
         2. Input validation gaps (names, paths, config values)\n\
         3. Process spawning safety (command injection via env/args)\n\
         4. Error messages leaking internal state\n\n\
         List findings as CRITICAL/WARNING with file + line. Max 400 words.\n\n\
         === src/backend/mod.rs ===\n```rust\n{backend_mod}\n```\n\n\
         === src/backend/codex.rs (first 120 lines) ===\n```rust\n{codex_rs}\n```\n\n\
         === src/backend/gemini.rs ===\n```rust\n{gemini_rs}\n```"
    );

    let codex_config = SpawnConfig {
        name: "codex-auditor".into(),
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

    match orch
        .spawn_teammate(TEAM, codex_config, BackendType::Codex)
        .await
    {
        Ok(()) => println!("  ✓ codex-auditor (Codex/high) → security audit"),
        Err(e) => println!("  ✗ codex-auditor failed: {e}"),
    }

    // T3: Gemini — API design review
    let gemini_prompt = format!(
        "You are a senior Rust API designer. Review the public API surface for:\n\
         1. Trait design: Is AgentBackend + AgentSession split well-designed?\n\
         2. Error enum completeness and ergonomics\n\
         3. Module organization and re-export clarity\n\
         4. Builder pattern correctness\n\n\
         List findings as CRITICAL/WARNING/SUGGESTION. Max 400 words.\n\n\
         === src/lib.rs ===\n```rust\n{lib_rs}\n```\n\n\
         === src/error.rs ===\n```rust\n{error_rs}\n```\n\n\
         === src/backend/mod.rs ===\n```rust\n{backend_mod}\n```"
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

    match orch
        .spawn_teammate(TEAM, gemini_config, BackendType::GeminiCli)
        .await
    {
        Ok(()) => println!("  ✓ gemini-reviewer (Gemini/2.5-flash) → API design review"),
        Err(e) => println!("  ✗ gemini-reviewer failed: {e}"),
    }

    println!(
        "\n  Phase 1: 3 agents spawned in {:.1}s\n",
        start.elapsed().as_secs_f64()
    );

    // --- Collect Phase 1 outputs in parallel ---
    println!("[3/6] Collecting Phase 1 reviews (3 agents in parallel)...\n");

    let mut phase1_outputs: HashMap<String, String> = HashMap::new();
    {
        let cc_rx = orch.take_output_receiver(TEAM, "cc-analyst").await?;
        let codex_rx = orch.take_output_receiver(TEAM, "codex-auditor").await?;
        let gemini_rx = orch.take_output_receiver(TEAM, "gemini-reviewer").await?;

        let h1 = tokio::spawn(async {
            (
                "cc-analyst".to_string(),
                collect_output(cc_rx, TIMEOUT_SECS).await,
            )
        });
        let h2 = tokio::spawn(async {
            (
                "codex-auditor".to_string(),
                collect_output(codex_rx, TIMEOUT_SECS).await,
            )
        });
        let h3 = tokio::spawn(async {
            (
                "gemini-reviewer".to_string(),
                collect_output(gemini_rx, TIMEOUT_SECS).await,
            )
        });

        for handle in [h1, h2, h3] {
            let (name, output) = handle.await.unwrap();
            phase1_outputs.insert(name, output);
        }
    }

    // Print Phase 1 results
    for (name, output) in &phase1_outputs {
        let backend = match name.as_str() {
            "cc-analyst" => "Claude Code",
            "codex-auditor" => "Codex",
            "gemini-reviewer" => "Gemini",
            _ => "Unknown",
        };
        println!("─────────── [P1][{backend}] {name} ───────────");
        if output.is_empty() {
            println!("  (no output)");
        } else {
            // Print max 30 lines
            for (i, line) in output.lines().enumerate() {
                if i >= 30 {
                    println!("  ... ({} more lines)", output.lines().count() - 30);
                    break;
                }
                println!("{line}");
            }
        }
        println!();
    }

    // Mark Phase 1 tasks complete
    for tid in [&t1.id, &t2.id, &t3.id] {
        orch.update_task(
            TEAM,
            tid,
            TaskUpdate {
                status: Some(TaskStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
    }

    // Shutdown Phase 1 agents (free resources before Phase 2)
    for name in ["cc-analyst", "codex-auditor", "gemini-reviewer"] {
        let _ = orch.shutdown_teammate(TEAM, name).await;
    }

    let phase1_time = start.elapsed();
    println!("  Phase 1 complete in {:.1}s\n", phase1_time.as_secs_f64());

    // Show updated DAG
    let dag_view = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag_view}");

    // =========================================================================
    // Step 3: Phase 2 — Deeper analysis with Phase 1 context
    // =========================================================================
    println!("\n[4/6] Phase 2: Spawning 2 agents with Phase 1 context...\n");

    // Check which Phase 2 tasks are now unblocked
    let available = orch.get_next_available_task(TEAM).await?;
    if let Some(t) = &available {
        println!("  Next available: #{} \"{}\"", t.id, t.subject);
    }

    // Prepare Phase 1 summaries for context injection
    let cc_findings = phase1_outputs
        .get("cc-analyst")
        .cloned()
        .unwrap_or_default();
    let codex_findings = phase1_outputs
        .get("codex-auditor")
        .cloned()
        .unwrap_or_default();
    let gemini_findings = phase1_outputs
        .get("gemini-reviewer")
        .cloned()
        .unwrap_or_default();

    // Mark Phase 2 tasks in-progress
    orch.update_task(
        TEAM,
        &t4.id,
        TaskUpdate {
            status: Some(TaskStatus::InProgress),
            owner: Some("codex-fixer".into()),
            ..Default::default()
        },
    )
    .await?;

    orch.update_task(
        TEAM,
        &t5.id,
        TaskUpdate {
            status: Some(TaskStatus::InProgress),
            owner: Some("gemini-planner".into()),
            ..Default::default()
        },
    )
    .await?;

    // T4: Codex — Fix proposals (uses CC concurrency + Codex security findings)
    let codex2_prompt = format!(
        "You are a senior Rust engineer. Based on these review findings, propose concrete code fixes.\n\
         For each finding, provide a specific fix with a code snippet.\n\
         Prioritize by severity (CRITICAL first). Max 500 words.\n\n\
         === Concurrency Analysis (from CC) ===\n{cc_findings}\n\n\
         === Security Audit (from Codex) ===\n{codex_findings}"
    );

    let codex2_config = SpawnConfig {
        name: "codex-fixer".into(),
        prompt: codex2_prompt,
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

    match orch
        .spawn_teammate(TEAM, codex2_config, BackendType::Codex)
        .await
    {
        Ok(()) => println!("  ✓ codex-fixer (Phase 2) → fix proposals"),
        Err(e) => println!("  ✗ codex-fixer failed: {e}"),
    }

    // T5: Gemini — Refactoring roadmap (uses Codex security + Gemini API findings)
    let gemini2_prompt = format!(
        "You are a Rust architect creating a refactoring roadmap.\n\
         Based on these review findings, create a prioritized list of refactoring tasks.\n\
         Group by: P0 (must fix), P1 (should fix), P2 (nice to have). Max 400 words.\n\n\
         === Security Audit ===\n{codex_findings}\n\n\
         === API Design Review ===\n{gemini_findings}"
    );

    let gemini2_config = SpawnConfig {
        name: "gemini-planner".into(),
        prompt: gemini2_prompt,
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

    match orch
        .spawn_teammate(TEAM, gemini2_config, BackendType::GeminiCli)
        .await
    {
        Ok(()) => println!("  ✓ gemini-planner (Phase 2) → refactoring roadmap"),
        Err(e) => println!("  ✗ gemini-planner failed: {e}"),
    }

    // --- Collect Phase 2 outputs ---
    println!("\n  Collecting Phase 2 outputs...\n");

    let mut phase2_outputs: HashMap<String, String> = HashMap::new();
    {
        let codex_rx = orch.take_output_receiver(TEAM, "codex-fixer").await?;
        let gemini_rx = orch.take_output_receiver(TEAM, "gemini-planner").await?;

        let h1 = tokio::spawn(async {
            (
                "codex-fixer".to_string(),
                collect_output(codex_rx, TIMEOUT_SECS).await,
            )
        });
        let h2 = tokio::spawn(async {
            (
                "gemini-planner".to_string(),
                collect_output(gemini_rx, TIMEOUT_SECS).await,
            )
        });

        for handle in [h1, h2] {
            let (name, output) = handle.await.unwrap();
            phase2_outputs.insert(name, output);
        }
    }

    // Print Phase 2 results
    for (name, output) in &phase2_outputs {
        let backend = if name.contains("codex") {
            "Codex"
        } else {
            "Gemini"
        };
        println!("─────────── [P2][{backend}] {name} ───────────");
        if output.is_empty() {
            println!("  (no output)");
        } else {
            for (i, line) in output.lines().enumerate() {
                if i >= 25 {
                    println!("  ... ({} more lines)", output.lines().count() - 25);
                    break;
                }
                println!("{line}");
            }
        }
        println!();
    }

    // Mark Phase 2 complete
    for tid in [&t4.id, &t5.id] {
        orch.update_task(
            TEAM,
            tid,
            TaskUpdate {
                status: Some(TaskStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
    }

    // Shutdown Phase 2 agents
    for name in ["codex-fixer", "gemini-planner"] {
        let _ = orch.shutdown_teammate(TEAM, name).await;
    }

    let phase2_time = start.elapsed();
    println!("  Phase 2 complete in {:.1}s\n", phase2_time.as_secs_f64());

    // =========================================================================
    // Step 4: Phase 3 — CC synthesizes final report
    // =========================================================================
    println!("[5/6] Phase 3: CC synthesizing final report...\n");

    // Mark T6 in-progress
    orch.update_task(
        TEAM,
        &t6.id,
        TaskUpdate {
            status: Some(TaskStatus::InProgress),
            owner: Some("cc-synthesizer".into()),
            ..Default::default()
        },
    )
    .await?;

    let fix_proposals = phase2_outputs
        .get("codex-fixer")
        .cloned()
        .unwrap_or_default();
    let refactor_roadmap = phase2_outputs
        .get("gemini-planner")
        .cloned()
        .unwrap_or_default();

    let cc3_prompt = format!(
        "You are the team lead synthesizing a final code review report for the `agent-teams` Rust crate.\n\
         Combine ALL findings below into a single prioritized action plan.\n\
         Format: numbered list with Priority (P0/P1/P2), Source, Issue, Fix.\n\
         Do NOT use any tools. Max 600 words.\n\n\
         === Phase 1: Concurrency Analysis (CC) ===\n{cc_findings}\n\n\
         === Phase 1: Security Audit (Codex) ===\n{codex_findings}\n\n\
         === Phase 1: API Design (Gemini) ===\n{gemini_findings}\n\n\
         === Phase 2: Fix Proposals (Codex) ===\n{fix_proposals}\n\n\
         === Phase 2: Refactoring Roadmap (Gemini) ===\n{refactor_roadmap}"
    );

    let cc3_config = SpawnConfig {
        name: "cc-synthesizer".into(),
        prompt: cc3_prompt,
        model: Some("sonnet".into()),
        cwd: Some(project_dir.to_path_buf()),
        max_turns: Some(2),
        allowed_tools: vec![],
        permission_mode: None,
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    match orch
        .spawn_teammate(TEAM, cc3_config, BackendType::ClaudeCode)
        .await
    {
        Ok(()) => println!("  ✓ cc-synthesizer (Claude Code/Sonnet) → final report"),
        Err(e) => println!("  ✗ cc-synthesizer failed: {e}"),
    }

    // Collect Phase 3
    let cc3_rx = orch.take_output_receiver(TEAM, "cc-synthesizer").await?;
    let final_report = collect_output(cc3_rx, TIMEOUT_SECS).await;

    println!("═══════════════════════════════════════════════════════");
    println!("  FINAL SYNTHESIS REPORT (by CC)");
    println!("═══════════════════════════════════════════════════════");
    if final_report.is_empty() {
        println!("  (no output from synthesizer)");
    } else {
        println!("{final_report}");
    }
    println!("═══════════════════════════════════════════════════════\n");

    // Mark T6 complete
    orch.update_task(
        TEAM,
        &t6.id,
        TaskUpdate {
            status: Some(TaskStatus::Completed),
            ..Default::default()
        },
    )
    .await?;

    // =========================================================================
    // Step 5: Final DAG state + cleanup
    // =========================================================================
    println!("[6/6] Final DAG state + cleanup\n");

    let dag_view = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag_view}");

    // Cleanup
    println!("\n--- Cleanup ---");
    for name in ["cc-synthesizer"] {
        let _ = orch.shutdown_teammate(TEAM, name).await;
    }
    orch.delete_team(TEAM).await?;

    let total = start.elapsed();
    println!("\n╔═══════════════════════════════════════════════════════╗");
    println!("║  Review complete!                                     ║");
    println!(
        "║  3 phases · 6 tasks · 3 backends · {:.0}s total         ║",
        total.as_secs_f64()
    );
    println!("╚═══════════════════════════════════════════════════════╝");

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a source file, optionally limited to first N lines (0 = all).
fn read_file(project_dir: &std::path::Path, path: &str, max_lines: usize) -> String {
    let content = std::fs::read_to_string(project_dir.join(path))
        .unwrap_or_else(|_| format!("(could not read {path})"));
    if max_lines > 0 {
        content
            .lines()
            .take(max_lines)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        content
    }
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
            Ok(Some(AgentOutput::Delta(d))) => {
                text.push_str(&d);
                text.push('\n');
            }
            Ok(Some(AgentOutput::Message(m))) => {
                if !text.contains(&m) {
                    text.push_str(&m);
                }
            }
            Ok(Some(AgentOutput::ToolOutput(_))) => {}
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
