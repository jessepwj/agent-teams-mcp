//! Interop with Claude Code Agent Teams.
//!
//! This example demonstrates how `agent-teams` can work alongside
//! Claude Code's native Agent Teams system by operating on the same
//! `~/.claude/teams/` and `~/.claude/tasks/` directories.
//!
//! ## Three Usage Modes
//!
//! 1. **Pre-stage**: Create teams & tasks in Rust, then let Claude Code work on them
//! 2. **Monitor**: Read teams/tasks/inboxes that Claude Code created
//! 3. **Full control**: Spawn Claude Code agents via cc-sdk and orchestrate them
//!
//! Run with:
//!   cargo run --example claude_code_interop

use agent_teams::backend::claude_code::ClaudeCodeBackend;
use agent_teams::backend::{BackendType, SpawnConfig};
use agent_teams::messaging::{FileInboxManager, InboxManager};
use agent_teams::models::{CreateTaskRequest, TaskUpdate};
use agent_teams::orchestrator::TeamOrchestrator;
use agent_teams::task::{FileTaskManager, TaskManager};
use agent_teams::team::{FileTeamManager, TeamManager};

#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_default();

    match mode.as_str() {
        "prestage" => demo_prestage().await,
        "monitor" => demo_monitor().await,
        "spawn" => demo_spawn_real_agent().await,
        _ => {
            println!("Claude Code Agent Teams Interop Examples");
            println!("=========================================\n");
            println!("Usage: cargo run --example claude_code_interop -- <mode>\n");
            println!("Modes:");
            println!("  prestage  — Create team & tasks for Claude Code to pick up");
            println!("  monitor   — Read existing Claude Code teams/tasks/inboxes");
            println!("  spawn     — Spawn a real Claude Code agent via cc-sdk\n");
            println!("Example:");
            println!("  cargo run --example claude_code_interop -- prestage");
            println!("  cargo run --example claude_code_interop -- monitor");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Mode 1: Pre-stage teams and tasks for Claude Code
// ---------------------------------------------------------------------------
async fn demo_prestage() -> agent_teams::Result<()> {
    println!("=== Mode: Pre-stage for Claude Code ===\n");

    // Use the REAL ~/.claude directories so Claude Code can see them
    let home = dirs::home_dir().expect("no home dir");
    let teams_dir = home.join(".claude/teams");
    let tasks_dir = home.join(".claude/tasks");

    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .build()?;

    let team_name = "rust-refactor";

    // Create team (or skip if exists)
    match orch
        .create_team(team_name, Some("Rust codebase refactoring project"))
        .await
    {
        Ok(config) => println!("[Created] Team: {}", config.team_name),
        Err(agent_teams::Error::TeamAlreadyExists { .. }) => {
            println!("[Exists] Team already exists, skipping creation");
        }
        Err(e) => return Err(e),
    }

    // Create tasks with dependency chain
    let t1 = orch
        .create_task(
            team_name,
            CreateTaskRequest {
                subject: "Audit error handling across codebase".into(),
                description: Some(
                    "Find all .unwrap() calls and replace with proper error handling using thiserror"
                        .into(),
                ),
                active_form: Some("Auditing error handling".into()),
                metadata: None,
            },
        )
        .await?;
    println!("[Task #{}] {}", t1.id, t1.subject);

    let t2 = orch
        .create_task(
            team_name,
            CreateTaskRequest {
                subject: "Define custom error types".into(),
                description: Some("Create error.rs with thiserror-derived enums".into()),
                active_form: Some("Defining error types".into()),
                metadata: None,
            },
        )
        .await?;

    let t3 = orch
        .create_task(
            team_name,
            CreateTaskRequest {
                subject: "Migrate functions to Result returns".into(),
                description: Some("Update function signatures to return Result<T, Error>".into()),
                active_form: Some("Migrating to Result".into()),
                metadata: None,
            },
        )
        .await?;

    let t4 = orch
        .create_task(
            team_name,
            CreateTaskRequest {
                subject: "Add integration tests for error paths".into(),
                description: Some("Test all error conditions are handled correctly".into()),
                active_form: Some("Writing error path tests".into()),
                metadata: None,
            },
        )
        .await?;

    // t2 depends on t1, t3 depends on t2, t4 depends on t3
    orch.update_task(
        team_name,
        &t2.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t1.id.clone()]),
            ..Default::default()
        },
    )
    .await?;
    orch.update_task(
        team_name,
        &t3.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t2.id.clone()]),
            ..Default::default()
        },
    )
    .await?;
    orch.update_task(
        team_name,
        &t4.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t3.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    println!(
        "\n[Deps] #{} -> #{} -> #{} -> #{}",
        t1.id, t2.id, t3.id, t4.id
    );

    println!("\n--- Files created ---");
    println!("  Team config:  ~/.claude/teams/{team_name}/config.json");
    println!("  Tasks:        ~/.claude/tasks/{team_name}/{{1..4}}.json");
    println!("\nClaude Code can now see this team via TeamCreate/TaskList tools.");
    println!("Or run: cargo run --example claude_code_interop -- monitor");

    Ok(())
}

// ---------------------------------------------------------------------------
// Mode 2: Monitor existing Claude Code teams
// ---------------------------------------------------------------------------
async fn demo_monitor() -> agent_teams::Result<()> {
    println!("=== Mode: Monitor Claude Code Teams ===\n");

    let home = dirs::home_dir().expect("no home dir");

    let team_mgr = FileTeamManager::new(home.join(".claude/teams"));
    let task_mgr = FileTaskManager::new(home.join(".claude/tasks"));
    let inbox_mgr = FileInboxManager::new(home.join(".claude/teams"));

    // List all teams
    let teams = team_mgr.list_teams().await?;
    if teams.is_empty() {
        println!("No teams found in ~/.claude/teams/");
        println!("Run 'prestage' mode first, or create a team in Claude Code.");
        return Ok(());
    }

    println!("Found {} team(s):\n", teams.len());

    for team_name in &teams {
        println!("--- Team: {team_name} ---");

        // Read config
        match team_mgr.read_config(team_name).await {
            Ok(config) => {
                println!(
                    "  Description: {}",
                    config.description.as_deref().unwrap_or("-")
                );
                println!("  Members ({}):", config.members.len());
                for member in &config.members {
                    let role = if member.is_teammate() {
                        "teammate"
                    } else {
                        "lead"
                    };
                    println!(
                        "    - {} ({}, {})",
                        member.name(),
                        role,
                        member.agent_type()
                    );
                }
            }
            Err(e) => println!("  Error reading config: {e}"),
        }

        // List tasks
        match task_mgr.list_tasks(team_name, None).await {
            Ok(tasks) if !tasks.is_empty() => {
                println!("  Tasks ({}):", tasks.len());
                for task in &tasks {
                    let owner = task.owner.as_deref().unwrap_or("-");
                    let blocked: String = if task.blocked_by.is_empty() {
                        String::new()
                    } else {
                        format!(" [blocked by: {}]", task.blocked_by.join(", "))
                    };
                    println!(
                        "    #{} [{}] {} (owner: {}){blocked}",
                        task.id, task.status, task.subject, owner
                    );
                }
            }
            Ok(_) => println!("  Tasks: (none)"),
            Err(_) => println!("  Tasks: (no task directory)"),
        }

        // Check inboxes
        let inbox_dir = home.join(format!(".claude/teams/{team_name}/inboxes"));
        if inbox_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&inbox_dir) {
                let agents: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                    .collect();

                if !agents.is_empty() {
                    println!("  Inboxes:");
                    for entry in agents {
                        let agent = entry
                            .path()
                            .file_stem()
                            .unwrap()
                            .to_string_lossy()
                            .to_string();
                        match inbox_mgr.read_unread(team_name, &agent).await {
                            Ok(msgs) => {
                                println!("    - {agent}: {} unread message(s)", msgs.len());
                                for msg in msgs.iter().take(3) {
                                    let preview = if msg.content.len() > 60 {
                                        format!("{}...", &msg.content[..60])
                                    } else {
                                        msg.content.clone()
                                    };
                                    // Check if it's a structured message
                                    if let Some(structured) = msg.try_as_structured() {
                                        println!(
                                            "      [{} -> {}] {} (structured: {})",
                                            msg.from,
                                            msg.to,
                                            structured.summary(),
                                            msg.timestamp.format("%H:%M:%S")
                                        );
                                    } else {
                                        println!(
                                            "      [{} -> {}] \"{}\" ({})",
                                            msg.from,
                                            msg.to,
                                            preview,
                                            msg.timestamp.format("%H:%M:%S")
                                        );
                                    }
                                }
                            }
                            Err(_) => println!("    - {agent}: (error reading)"),
                        }
                    }
                }
            }
        }

        println!();
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Mode 3: Spawn a real Claude Code agent
// ---------------------------------------------------------------------------
async fn demo_spawn_real_agent() -> agent_teams::Result<()> {
    println!("=== Mode: Spawn Real Claude Code Agent ===\n");

    let home = dirs::home_dir().expect("no home dir");
    let teams_dir = home.join(".claude/teams");
    let tasks_dir = home.join(".claude/tasks");

    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .with_claude_code(ClaudeCodeBackend::new().expect("claude CLI not found"))
        .build()?;

    let team_name = "live-demo";

    // Create team
    match orch
        .create_team(team_name, Some("Live Claude Code demo"))
        .await
    {
        Ok(_) => println!("[Created] Team: {team_name}"),
        Err(agent_teams::Error::TeamAlreadyExists { .. }) => {
            println!("[Exists] Team: {team_name}");
        }
        Err(e) => return Err(e),
    }

    // Spawn a real Claude Code agent
    // NOTE: This requires `claude` CLI to be installed and in PATH
    println!("\n[Spawning] Claude Code agent 'researcher'...");
    println!("  (requires 'claude' CLI in PATH)\n");

    let config = SpawnConfig {
        name: "researcher".into(),
        prompt: "You are a research assistant. When you receive a task, investigate it and report back with a concise summary. Keep responses under 200 words.".into(),
        model: Some("sonnet".into()),
        cwd: Some(std::env::current_dir().unwrap_or_default()),
        max_turns: Some(3),
        allowed_tools: vec![],
        permission_mode: Some("plan".into()),
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    match orch
        .spawn_teammate(team_name, config, BackendType::ClaudeCode)
        .await
    {
        Ok(()) => {
            println!(
                "[Spawned] researcher is alive: {}",
                orch.is_alive(team_name, "researcher").await
            );

            // Assign a task
            let task = orch
                .create_task(
                    team_name,
                    CreateTaskRequest {
                        subject: "Research Rust edition 2024 changes".into(),
                        description: Some("List the top 5 changes in Rust edition 2024".into()),
                        active_form: Some("Researching Rust 2024".into()),
                        metadata: None,
                    },
                )
                .await?;

            orch.assign_task(team_name, &task.id, "researcher").await?;
            println!("[Assigned] Task #{} to researcher", task.id);

            // Send additional context
            orch.send_message(
                team_name,
                "lead",
                "researcher",
                "Focus on language changes, not tooling updates.",
            )
            .await?;

            println!("\n[Info] Agent is running. Check status with:");
            println!("  cargo run --example claude_code_interop -- monitor");

            // In a real app, you'd poll for results or read the output receiver
            // For this demo, we just show the setup worked
            println!("\n[Cleanup] Shutting down researcher...");
            orch.shutdown_teammate(team_name, "researcher").await?;
            orch.delete_team(team_name).await?;
            println!("[Done]");
        }
        Err(e) => {
            println!("[Error] Failed to spawn agent: {e}");
            println!("  Make sure 'claude' CLI is installed:");
            println!("  npm install -g @anthropic-ai/claude-code");
            // Cleanup
            let _ = orch.delete_team(team_name).await;
        }
    }

    Ok(())
}
