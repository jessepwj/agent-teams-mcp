//! Article writing pipeline: CC writes, Codex + Gemini review, CC finalizes.
//!
//! Diamond DAG:
//!   T1(CC): Write draft  →  T2(Codex): Tech review  →  T4(CC): Final article
//!                        →  T3(Gemini): Writing review →
//!
//! Run with:
//!   cargo run --example article_writer

use std::collections::HashMap;
use std::time::{Duration, Instant};

use agent_teams::backend::claude_code::ClaudeCodeBackend;
use agent_teams::backend::codex::CodexBackend;
use agent_teams::backend::gemini::GeminiCliBackend;
use agent_teams::backend::{AgentOutput, BackendType, SpawnConfig};
use agent_teams::models::{CreateTaskRequest, TaskStatus, TaskUpdate};
use agent_teams::orchestrator::TeamOrchestrator;

const TEAM: &str = "article";
const TIMEOUT: u64 = 180;

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
    println!("║  Article Writer: agent-teams 介绍文章                  ║");
    println!("║  CC 写稿 · Codex+Gemini 审查 · CC 定稿                ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");

    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .with_claude_code(ClaudeCodeBackend::new().expect("claude CLI not found"))
        .with_codex(CodexBackend::with_path("/opt/homebrew/bin/codex"))
        .with_gemini_cli(GeminiCliBackend::with_path("/opt/homebrew/bin/gemini"))
        .build()?;

    orch.create_team(TEAM, Some("Article writing pipeline"))
        .await?;
    let start = Instant::now();

    // =====================================================================
    // BUILD DAG
    // =====================================================================
    let t1 = mktask(&orch, "CC: Write article draft").await?;
    let t2 = mktask(&orch, "Codex: Technical accuracy review").await?;
    let t3 = mktask(&orch, "Gemini: Writing quality review").await?;
    let t4 = mktask(&orch, "CC: Finalize article").await?;

    orch.update_task(
        TEAM,
        &t2.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t1.id.clone()]),
            ..Default::default()
        },
    )
    .await?;
    orch.update_task(
        TEAM,
        &t3.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t1.id.clone()]),
            ..Default::default()
        },
    )
    .await?;
    orch.update_task(
        TEAM,
        &t4.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t2.id.clone(), t3.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    let dag = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag}");

    // =====================================================================
    // PHASE 1: CC writes the article draft
    // =====================================================================
    println!("\n[Phase 1] CC 研读源码并撰写初稿...\n");
    mark_started(&orch, &t1.id, "cc-writer").await?;

    // Gather source material
    let lib_rs = readf(project, "src/lib.rs");
    let backend_mod = readn(project, "src/backend/mod.rs", 120);
    let orch_mod = readn(project, "src/orchestrator/mod.rs", 100);
    let consensus = readn(project, "src/consensus/mod.rs", 80);
    let memory = readn(project, "src/memory/mod.rs", 80);
    let graph = readn(project, "src/task/graph.rs", 60);
    let cargo = readf(project, "Cargo.toml");
    let example = readn(project, "examples/new_features_demo.rs", 80);

    spawn_agent(&orch, "cc-writer", BackendType::ClaudeCode, SpawnConfig {
        name: "cc-writer".into(),
        prompt: format!(
r#"你是一位技术博客作者。根据下面提供的 agent-teams Rust 库源码，撰写一篇完整的中文介绍文章。

## 文章要求
- 标题：agent-teams：用 Rust 编排多 AI Agent 协作
- 字数：1500-2000 字
- 语言：中文，技术术语保留英文
- 结构：
  1. 引言：为什么需要多 Agent 协作框架
  2. 架构概览：AgentBackend + AgentSession trait split，文件协议
  3. 三大后端：Claude Code (cc-sdk)、Codex (JSON-RPC)、Gemini CLI (one-shot pipe)
  4. 任务 DAG：DependencyGraph，拓扑排序，关键路径，终端可视化
  5. 共识协议：Majority/Weighted/Unanimous/HumanInTheLoop 四种策略
  6. Agent 记忆：ConversationMemory，MemoryManager 文件持久化
  7. TeamOrchestrator：一站式 API，spawn/send_input/shutdown
  8. 实战示例：简短代码展示如何用 3 行创建团队并分发任务
  9. 总结与展望

- 风格：专业但易读，像 Rust Blog 或知乎专栏的技术文章
- 不要使用任何工具，直接根据源码写文章

## 源码参考

=== Cargo.toml ===
{cargo}

=== src/lib.rs ===
{lib_rs}

=== src/backend/mod.rs (前120行) ===
{backend_mod}

=== src/orchestrator/mod.rs (前100行) ===
{orch_mod}

=== src/consensus/mod.rs (前80行) ===
{consensus}

=== src/memory/mod.rs (前80行) ===
{memory}

=== src/task/graph.rs (前60行) ===
{graph}

=== examples/new_features_demo.rs (前80行) ===
{example}"#),
        model: Some("sonnet".into()),
        cwd: Some(project.to_path_buf()),
        max_turns: Some(3),
        allowed_tools: vec![],
        permission_mode: None,
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    }).await;

    let r1 = orch.take_output_receiver(TEAM, "cc-writer").await?;
    let draft = collect(r1, TIMEOUT).await;

    orch.update_task(
        TEAM,
        &t1.id,
        TaskUpdate {
            status: Some(TaskStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    let _ = orch.shutdown_teammate(TEAM, "cc-writer").await;

    println!(
        "─────────── [Phase 1] CC Draft ({} chars) ───────────",
        draft.len()
    );
    print_truncated(&draft, 40);
    println!(
        "\n  Phase 1 done in {:.0}s\n",
        start.elapsed().as_secs_f64()
    );

    let dag = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag}");

    // =====================================================================
    // PHASE 2: Codex + Gemini review in parallel
    // =====================================================================
    println!("\n[Phase 2] Codex 技术审查 + Gemini 写作审查 (并行)...\n");
    mark_started(&orch, &t2.id, "codex-reviewer").await?;
    mark_started(&orch, &t3.id, "gemini-reviewer").await?;

    // T2: Codex technical review
    spawn_agent(
        &orch,
        "codex-reviewer",
        BackendType::Codex,
        SpawnConfig {
            name: "codex-reviewer".into(),
            prompt: format!(
                r#"You are a Rust expert reviewing a technical article about the agent-teams crate.

Review the article below for TECHNICAL ACCURACY ONLY:
1. Are API names correct? (struct/trait/method names matching actual code)
2. Are code examples compilable and correct?
3. Are architectural descriptions accurate?
4. Any factual errors about how the backends work?
5. Any important features missing from the article?

For each issue found, format as:
- [ERROR] description (with correction)
- [MISSING] important feature not mentioned
- [SUGGESTION] improvement idea

Be concise. Max 500 words.

=== ARTICLE DRAFT ===
{draft}

=== ACTUAL src/lib.rs (for verification) ===
{lib_rs}"#
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

    // T3: Gemini writing quality review
    spawn_agent(
        &orch,
        "gemini-reviewer",
        BackendType::GeminiCli,
        SpawnConfig {
            name: "gemini-reviewer".into(),
            prompt: format!(
                r#"你是一位技术写作编辑。审查以下关于 agent-teams Rust 库的中文技术文章。

仅关注写作质量：
1. 结构是否清晰？段落过渡是否自然？
2. 是否有冗余或啰嗦的段落？
3. 技术术语使用是否一致？（中英文混排是否恰当）
4. 开头是否吸引读者？结尾是否有力？
5. 是否适合知乎/Rust Blog 的读者群体？

每个问题格式：
- [结构] 具体建议
- [语言] 具体建议
- [建议] 改进建议

简洁输出，最多 400 字。

=== 文章初稿 ===
{draft}"#
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

    let r2 = orch.take_output_receiver(TEAM, "codex-reviewer").await?;
    let r3 = orch.take_output_receiver(TEAM, "gemini-reviewer").await?;

    let mut results: HashMap<String, String> = HashMap::new();
    let h2 = tokio::spawn(async { ("codex".into(), collect(r2, TIMEOUT).await) });
    let h3 = tokio::spawn(async { ("gemini".into(), collect(r3, TIMEOUT).await) });
    for h in [h2, h3] {
        let (k, v) = h.await.unwrap();
        results.insert(k, v);
    }

    let codex_review = results.get("codex").cloned().unwrap_or_default();
    let gemini_review = results.get("gemini").cloned().unwrap_or_default();

    println!("─────────── [Phase 2] Codex Review ───────────");
    print_truncated(&codex_review, 30);
    println!("\n─────────── [Phase 2] Gemini Review ───────────");
    print_truncated(&gemini_review, 30);

    for id in [&t2.id, &t3.id] {
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
    for n in ["codex-reviewer", "gemini-reviewer"] {
        let _ = orch.shutdown_teammate(TEAM, n).await;
    }

    println!(
        "\n  Phase 2 done in {:.0}s\n",
        start.elapsed().as_secs_f64()
    );
    let dag = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag}");

    // =====================================================================
    // PHASE 3: CC incorporates feedback and outputs final article
    // =====================================================================
    println!("\n[Phase 3] CC 整合反馈，输出终稿...\n");
    mark_started(&orch, &t4.id, "cc-editor").await?;

    spawn_agent(
        &orch,
        "cc-editor",
        BackendType::ClaudeCode,
        SpawnConfig {
            name: "cc-editor".into(),
            prompt: format!(
                r#"你是技术文章的终审编辑。根据初稿和两位审查者的反馈，输出最终定稿。

## 任务
1. 修正 Codex 指出的所有技术错误
2. 采纳 Gemini 的写作改进建议
3. 保持原文结构，除非审查建议调整
4. 确保所有 API 名称与实际代码一致
5. 输出完整的最终文章（不要输出修改说明，直接输出文章全文）

不要使用任何工具。直接输出最终文章。

=== 初稿 ===
{draft}

=== Codex 技术审查 ===
{codex_review}

=== Gemini 写作审查 ===
{gemini_review}"#
            ),
            model: Some("sonnet".into()),
            cwd: Some(project.to_path_buf()),
            max_turns: Some(3),
            allowed_tools: vec![],
            permission_mode: None,
            reasoning_effort: None,
            env: Default::default(),
            memory_config: None,
            delegations: Vec::new(),
        },
    )
    .await;

    let r4 = orch.take_output_receiver(TEAM, "cc-editor").await?;
    let final_article = collect(r4, TIMEOUT).await;

    orch.update_task(
        TEAM,
        &t4.id,
        TaskUpdate {
            status: Some(TaskStatus::Completed),
            ..Default::default()
        },
    )
    .await?;

    // =====================================================================
    // OUTPUT
    // =====================================================================
    let sep = "═".repeat(60);
    println!("\n{sep}");
    println!("  FINAL ARTICLE");
    println!("{sep}\n");

    if final_article.is_empty() {
        println!("  (no output — using draft as fallback)\n");
        println!("{draft}");
    } else {
        println!("{final_article}");
    }

    println!("{}\n", "═".repeat(60));

    // Save to file
    let out_path = project.join("doc").join("article_agent_teams.md");
    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = if final_article.is_empty() {
        &draft
    } else {
        &final_article
    };
    std::fs::write(&out_path, content).unwrap_or_else(|e| eprintln!("Failed to save: {e}"));
    println!("  Article saved to: {}", out_path.display());

    // Final DAG
    let dag = orch.export_task_graph_terminal(TEAM).await?;
    print!("{dag}");

    // Cleanup
    let _ = orch.shutdown_teammate(TEAM, "cc-editor").await;
    orch.delete_team(TEAM).await?;

    println!(
        "\n  Total: {:.0}s · 4 tasks · 3 phases · 3 backends",
        start.elapsed().as_secs_f64()
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

fn print_truncated(text: &str, max_lines: usize) {
    if text.is_empty() {
        println!("  (no output)");
        return;
    }
    let total = text.lines().count();
    for (i, line) in text.lines().enumerate() {
        if i >= max_lines {
            println!("  ... ({} more lines)", total - max_lines);
            break;
        }
        println!("{line}");
    }
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
