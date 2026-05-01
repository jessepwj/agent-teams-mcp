//! Release preparation audit: CC + Codex + Gemini with real DAG dependencies.
//!
//! Diamond DAG where Phase 2 genuinely requires Phase 1 findings:
//!
//!   T1(CC): API surface inventory        ─┬─→ T4(Codex): Write missing doc examples
//!   T2(Codex): Dependency health check    │     (needs T1 API + T3 doc gaps)
//!   T3(Gemini): Doc coverage audit       ─┤
//!                                         └─→ T5(Gemini): Draft CHANGELOG
//!                                               (needs T1 API + T2 dep changes)
//!   T4 + T5 ──→ T6(CC): Release readiness verdict
//!
//! Run with:
//!   cargo run --example release_audit

use std::collections::HashMap;
use std::time::{Duration, Instant};

use agent_teams::backend::claude_code::ClaudeCodeBackend;
use agent_teams::backend::codex::CodexBackend;
use agent_teams::backend::gemini::GeminiCliBackend;
use agent_teams::backend::{AgentOutput, BackendType, SpawnConfig};
use agent_teams::models::{CreateTaskRequest, TaskStatus, TaskUpdate};
use agent_teams::orchestrator::TeamOrchestrator;

const TEAM: &str = "release-audit";
const TIMEOUT: u64 = 120;

#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("agent_teams=info,warn")
        .init();

    let tmp = tempfile::tempdir().expect("tempdir");
    let teams_dir = tmp.path().join("teams");
    let tasks_dir = tmp.path().join("tasks");
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║  agent-teams v0.2.0 Release Preparation Audit        ║");
    println!("║  CC + Codex + Gemini · 3 Phases · 6 Tasks            ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");

    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .with_claude_code(ClaudeCodeBackend::new().expect("claude CLI not found"))
        .with_codex(CodexBackend::with_path("/opt/homebrew/bin/codex"))
        .with_gemini_cli(GeminiCliBackend::with_path("/opt/homebrew/bin/gemini"))
        .build()?;

    orch.create_team(TEAM, Some("v0.2.0 release audit")).await?;

    let start = Instant::now();

    // =====================================================================
    // BUILD THE DAG
    // =====================================================================
    println!("[DAG] Building task graph...\n");

    let t1 = mktask(&orch, "CC: API surface inventory").await?;
    let t2 = mktask(&orch, "Codex: Dependency health check").await?;
    let t3 = mktask(&orch, "Gemini: Doc coverage audit").await?;
    let t4 = mktask(&orch, "Codex: Write missing doc examples").await?;
    let t5 = mktask(&orch, "Gemini: Draft CHANGELOG").await?;
    let t6 = mktask(&orch, "CC: Release readiness verdict").await?;

    // T4 needs T1 (API inventory) + T3 (doc gaps)
    orch.update_task(
        TEAM,
        &t4.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t1.id.clone(), t3.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    // T5 needs T1 (API changes) + T2 (dep health)
    orch.update_task(
        TEAM,
        &t5.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t1.id.clone(), t2.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    // T6 needs T4 + T5
    orch.update_task(
        TEAM,
        &t6.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t4.id.clone(), t5.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    let dag = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag}");

    // =====================================================================
    // PHASE 1: Parallel analysis (CC + Codex + Gemini)
    // =====================================================================
    println!("\n[Phase 1] Spawning 3 analysis agents in parallel...\n");

    mark_started(&orch, &t1.id, "cc-api").await?;
    mark_started(&orch, &t2.id, "codex-deps").await?;
    mark_started(&orch, &t3.id, "gemini-docs").await?;

    // Read source files
    let lib_rs = readf(project, "src/lib.rs");
    let cargo = readf(project, "Cargo.toml");
    let backend_mod = readf(project, "src/backend/mod.rs");
    let error_rs = readf(project, "src/error.rs");
    let orch_excerpt = readn(project, "src/orchestrator/mod.rs", 80);
    let consensus = readn(project, "src/consensus/mod.rs", 60);
    let memory = readn(project, "src/memory/mod.rs", 60);

    // T1: CC — API surface inventory
    spawn_agent(
        &orch,
        "cc-api",
        BackendType::ClaudeCode,
        SpawnConfig {
            name: "cc-api".into(),
            prompt: format!(
                "You are a Rust library maintainer preparing v0.2.0 release.\n\
             List ALL public types, traits, functions, and methods exported by this crate.\n\
             Group by module. Mark items that are NEW since v0.1.0 with [NEW].\n\
             (Hint: consensus, memory, and terminal DAG rendering are new modules)\n\
             Do NOT use any tools. Max 500 words.\n\n\
             === src/lib.rs ===\n{lib_rs}\n\n\
             === src/backend/mod.rs (public types) ===\n{backend_mod}\n\n\
             === src/consensus/mod.rs (first 60 lines) ===\n{consensus}\n\n\
             === src/memory/mod.rs (first 60 lines) ===\n{memory}"
            ),
            model: Some("sonnet".into()),
            cwd: Some(project.to_path_buf()),
            max_turns: Some(2),
            allowed_tools: vec![],
            permission_mode: None,
            reasoning_effort: None,
            env: Default::default(),
            memory_config: None,
            delegations: Vec::new(),
        },
    )
    .await;

    // T2: Codex — Dependency health check
    spawn_agent(
        &orch,
        "codex-deps",
        BackendType::Codex,
        SpawnConfig {
            name: "codex-deps".into(),
            prompt: format!(
                "You are a Rust dependency auditor. Analyze this Cargo.toml:\n\
             1. Check each dependency version — is it latest stable? List any outdated.\n\
             2. Flag any dependency with known security issues.\n\
             3. Check feature flags — any unnecessary features enabled?\n\
             4. Suggest any dependencies that could be removed or replaced.\n\
             Be concise, max 400 words.\n\n\
             === Cargo.toml ===\n{cargo}"
            ),
            model: None,
            cwd: Some(project.to_path_buf()),
            max_turns: Some(1),
            allowed_tools: vec![],
            permission_mode: None,
            reasoning_effort: Some("high".into()),
            env: Default::default(),
            memory_config: None,
            delegations: Vec::new(),
        },
    )
    .await;

    // T3: Gemini — Doc coverage audit
    spawn_agent(
        &orch,
        "gemini-docs",
        BackendType::GeminiCli,
        SpawnConfig {
            name: "gemini-docs".into(),
            prompt: format!(
                "You are a Rust documentation auditor. Analyze these files and list:\n\
             1. Public items MISSING doc comments (///) — list each with its location\n\
             2. Public items with docs but MISSING examples (```rust blocks)\n\
             3. Module-level docs (//!) that are missing or incomplete\n\
             Rate overall doc coverage as A/B/C/D/F. Max 400 words.\n\n\
             === src/lib.rs ===\n{lib_rs}\n\n\
             === src/error.rs ===\n{error_rs}\n\n\
             === src/orchestrator/mod.rs (first 80 lines) ===\n{orch_excerpt}"
            ),
            model: Some("gemini-2.5-flash".into()),
            cwd: Some(project.to_path_buf()),
            max_turns: Some(1),
            allowed_tools: vec![],
            permission_mode: None,
            reasoning_effort: None,
            env: Default::default(),
            memory_config: None,
            delegations: Vec::new(),
        },
    )
    .await;

    // Show live DAG
    let dag = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag}");

    // Collect Phase 1
    println!("\n  Waiting for Phase 1 results...\n");
    let mut results: HashMap<String, String> = HashMap::new();
    {
        let r1 = orch.take_output_receiver(TEAM, "cc-api").await?;
        let r2 = orch.take_output_receiver(TEAM, "codex-deps").await?;
        let r3 = orch.take_output_receiver(TEAM, "gemini-docs").await?;

        let h1 = tokio::spawn(async { ("cc-api".into(), collect(r1, TIMEOUT).await) });
        let h2 = tokio::spawn(async { ("codex-deps".into(), collect(r2, TIMEOUT).await) });
        let h3 = tokio::spawn(async { ("gemini-docs".into(), collect(r3, TIMEOUT).await) });

        for h in [h1, h2, h3] {
            let (k, v) = h.await.unwrap();
            results.insert(k, v);
        }
    }

    // Print Phase 1 results
    print_result("P1", "Claude Code", "cc-api", &results);
    print_result("P1", "Codex", "codex-deps", &results);
    print_result("P1", "Gemini", "gemini-docs", &results);

    // Complete Phase 1
    for id in [&t1.id, &t2.id, &t3.id] {
        orch.update_task(
            TEAM,
            id,
            TaskUpdate {
                status: Some(TaskStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
    }
    for n in ["cc-api", "codex-deps", "gemini-docs"] {
        let _ = orch.shutdown_teammate(TEAM, n).await;
    }

    println!("  Phase 1 done in {:.0}s\n", start.elapsed().as_secs_f64());
    let dag = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag}");

    // =====================================================================
    // PHASE 2: Cross-cutting work (needs Phase 1 context)
    // =====================================================================
    println!("\n[Phase 2] Spawning 2 agents with Phase 1 context...\n");

    let api_inventory = results.get("cc-api").cloned().unwrap_or_default();
    let dep_health = results.get("codex-deps").cloned().unwrap_or_default();
    let doc_audit = results.get("gemini-docs").cloned().unwrap_or_default();

    mark_started(&orch, &t4.id, "codex-examples").await?;
    mark_started(&orch, &t5.id, "gemini-changelog").await?;

    // T4: Codex — Write missing doc examples
    spawn_agent(
        &orch,
        "codex-examples",
        BackendType::Codex,
        SpawnConfig {
            name: "codex-examples".into(),
            prompt: format!(
                "You are a Rust documentation writer.\n\
             Based on the API inventory and doc coverage audit below, write\n\
             concrete `/// # Examples` blocks for the TOP 5 most important\n\
             undocumented public items.\n\
             Use real types from the crate. Each example should compile.\n\
             Output as: `/// # Examples` blocks ready to paste. Max 600 words.\n\n\
             === API Surface (from CC) ===\n{api_inventory}\n\n\
             === Doc Gaps (from Gemini) ===\n{doc_audit}"
            ),
            model: None,
            cwd: Some(project.to_path_buf()),
            max_turns: Some(1),
            allowed_tools: vec![],
            permission_mode: None,
            reasoning_effort: Some("high".into()),
            env: Default::default(),
            memory_config: None,
            delegations: Vec::new(),
        },
    )
    .await;

    // T5: Gemini — Draft CHANGELOG
    spawn_agent(
        &orch,
        "gemini-changelog",
        BackendType::GeminiCli,
        SpawnConfig {
            name: "gemini-changelog".into(),
            prompt: format!(
                "You are writing the CHANGELOG for agent-teams v0.2.0.\n\
             Based on the API inventory and dependency analysis below,\n\
             write a CHANGELOG.md entry in Keep a Changelog format.\n\
             Sections: Added, Changed, Fixed, Dependencies.\n\
             Mark breaking changes with [BREAKING]. Max 400 words.\n\n\
             === API Surface (new items marked [NEW]) ===\n{api_inventory}\n\n\
             === Dependency Analysis ===\n{dep_health}"
            ),
            model: Some("gemini-2.5-flash".into()),
            cwd: Some(project.to_path_buf()),
            max_turns: Some(1),
            allowed_tools: vec![],
            permission_mode: None,
            reasoning_effort: None,
            env: Default::default(),
            memory_config: None,
            delegations: Vec::new(),
        },
    )
    .await;

    // Collect Phase 2
    println!("  Waiting for Phase 2 results...\n");
    {
        let r4 = orch.take_output_receiver(TEAM, "codex-examples").await?;
        let r5 = orch.take_output_receiver(TEAM, "gemini-changelog").await?;

        let h4 = tokio::spawn(async { ("codex-examples".into(), collect(r4, TIMEOUT).await) });
        let h5 = tokio::spawn(async { ("gemini-changelog".into(), collect(r5, TIMEOUT).await) });

        for h in [h4, h5] {
            let (k, v) = h.await.unwrap();
            results.insert(k, v);
        }
    }

    print_result("P2", "Codex", "codex-examples", &results);
    print_result("P2", "Gemini", "gemini-changelog", &results);

    for id in [&t4.id, &t5.id] {
        orch.update_task(
            TEAM,
            id,
            TaskUpdate {
                status: Some(TaskStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
    }
    for n in ["codex-examples", "gemini-changelog"] {
        let _ = orch.shutdown_teammate(TEAM, n).await;
    }

    println!("  Phase 2 done in {:.0}s\n", start.elapsed().as_secs_f64());
    let dag = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag}");

    // =====================================================================
    // PHASE 3: Final verdict (CC synthesizes everything)
    // =====================================================================
    println!("\n[Phase 3] CC generating release readiness verdict...\n");

    mark_started(&orch, &t6.id, "cc-verdict").await?;

    let doc_examples = results.get("codex-examples").cloned().unwrap_or_default();
    let changelog = results.get("gemini-changelog").cloned().unwrap_or_default();

    spawn_agent(
        &orch,
        "cc-verdict",
        BackendType::ClaudeCode,
        SpawnConfig {
            name: "cc-verdict".into(),
            prompt: format!(
                "You are the release manager for `agent-teams` v0.2.0.\n\
             Based on ALL audit findings below, give a release readiness verdict:\n\n\
             ## Verdict: READY / NEEDS WORK / BLOCKED\n\n\
             Include:\n\
             1. Release readiness score (0-100)\n\
             2. Blockers (must fix before release)\n\
             3. Recommended improvements (nice to have)\n\
             4. Summary of what's new in v0.2.0\n\n\
             Do NOT use any tools. Max 500 words.\n\n\
             === API Surface ===\n{api_inventory}\n\n\
             === Dependency Health ===\n{dep_health}\n\n\
             === Doc Coverage ===\n{doc_audit}\n\n\
             === Generated Examples ===\n{doc_examples}\n\n\
             === CHANGELOG Draft ===\n{changelog}"
            ),
            model: Some("sonnet".into()),
            cwd: Some(project.to_path_buf()),
            max_turns: Some(2),
            allowed_tools: vec![],
            permission_mode: None,
            reasoning_effort: None,
            env: Default::default(),
            memory_config: None,
            delegations: Vec::new(),
        },
    )
    .await;

    let r6 = orch.take_output_receiver(TEAM, "cc-verdict").await?;
    let verdict = collect(r6, TIMEOUT).await;

    orch.update_task(
        TEAM,
        &t6.id,
        TaskUpdate {
            status: Some(TaskStatus::Completed),
            ..Default::default()
        },
    )
    .await?;

    // =====================================================================
    // FINAL OUTPUT
    // =====================================================================
    println!("═══════════════════════════════════════════════════════");
    println!("  RELEASE READINESS VERDICT");
    println!("═══════════════════════════════════════════════════════");
    if verdict.is_empty() {
        println!("  (no verdict output)");
    } else {
        println!("{verdict}");
    }
    println!("═══════════════════════════════════════════════════════\n");

    // Final DAG
    let dag = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag}");

    // Cleanup
    let _ = orch.shutdown_teammate(TEAM, "cc-verdict").await;
    orch.delete_team(TEAM).await?;

    let total = start.elapsed();
    println!(
        "\n  Total: {:.0}s · 6 tasks · 3 phases · 3 backends",
        total.as_secs_f64()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn mktask(
    orch: &TeamOrchestrator,
    subject: &str,
) -> agent_teams::Result<agent_teams::models::TaskFile> {
    orch.create_task(
        TEAM,
        CreateTaskRequest {
            subject: subject.into(),
            description: None,
            active_form: None,
            metadata: None,
        },
    )
    .await
}

async fn mark_started(orch: &TeamOrchestrator, tid: &str, owner: &str) -> agent_teams::Result<()> {
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
    Ok(())
}

async fn spawn_agent(
    orch: &TeamOrchestrator,
    name: &str,
    backend: BackendType,
    config: SpawnConfig,
) {
    match orch.spawn_teammate(TEAM, config, backend).await {
        Ok(()) => println!("  + {name} ({backend}) spawned"),
        Err(e) => println!("  ! {name} failed: {e}"),
    }
}

fn readf(base: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(base.join(path)).unwrap_or_else(|_| format!("(missing: {path})"))
}

fn readn(base: &std::path::Path, path: &str, lines: usize) -> String {
    readf(base, path)
        .lines()
        .take(lines)
        .collect::<Vec<_>>()
        .join("\n")
}

fn print_result(phase: &str, backend: &str, key: &str, results: &HashMap<String, String>) {
    let output = results.get(key).cloned().unwrap_or_default();
    println!("─────────── [{phase}][{backend}] {key} ───────────");
    if output.is_empty() {
        println!("  (no output)");
    } else {
        for (i, line) in output.lines().enumerate() {
            if i >= 35 {
                println!("  ... ({} more lines)", output.lines().count() - 35);
                break;
            }
            println!("{line}");
        }
    }
    println!();
}

async fn collect(
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
                text.push_str(&format!("\n[timeout {timeout_secs}s]"));
                break;
            }
        }
    }
    text
}
