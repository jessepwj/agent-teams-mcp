//! CLI for agent-teams: DAG analysis and checkpoint management.
//!
//! Reads Claude Code native task files from `~/.claude/tasks/{team}/`
//! and provides graph analysis that Claude Code doesn't have natively:
//! cycle detection, topological sort, critical path, and visualization.
//!
//! With the `checkpoint` feature, also provides Git notes-based
//! agent session context attached to commits.
//!
//! With the `tui` feature, provides a terminal dashboard for
//! browsing checkpoints, team status, and token costs.

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

use agent_teams::models::task::TaskFile;
use agent_teams::task::DependencyGraph;

#[derive(Parser)]
#[command(
    name = "agent-teams",
    version,
    about = "Agent Teams CLI: DAG analysis & checkpoint management"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// DAG analysis for team task dependencies
    Dag {
        #[command(subcommand)]
        action: DagAction,
    },

    /// Checkpoint: attach agent session context to Git commits via notes
    #[cfg(feature = "checkpoint")]
    Checkpoint {
        #[command(subcommand)]
        action: CheckpointAction,
    },

    /// Launch the TUI dashboard
    #[cfg(feature = "tui")]
    Tui {
        /// Team name to focus on
        #[arg(short, long)]
        team: Option<String>,

        /// Repository path (default: current directory)
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum DagAction {
    /// Validate the DAG (detect cycles)
    Validate {
        /// Team name
        #[arg(short, long)]
        team: String,

        /// Custom tasks directory (default: ~/.claude/tasks)
        #[arg(long)]
        tasks_dir: Option<PathBuf>,
    },

    /// Show the DAG visualization
    Show {
        /// Team name
        #[arg(short, long)]
        team: String,

        /// Output format: terminal (default), mermaid, dot
        #[arg(short, long, default_value = "terminal")]
        format: OutputFormat,

        /// Custom tasks directory (default: ~/.claude/tasks)
        #[arg(long)]
        tasks_dir: Option<PathBuf>,
    },

    /// Show tasks that are ready to run (unblocked)
    Next {
        /// Team name
        #[arg(short, long)]
        team: String,

        /// Custom tasks directory (default: ~/.claude/tasks)
        #[arg(long)]
        tasks_dir: Option<PathBuf>,
    },

    /// Show the critical path (longest dependency chain)
    CriticalPath {
        /// Team name
        #[arg(short, long)]
        team: String,

        /// Custom tasks directory (default: ~/.claude/tasks)
        #[arg(long)]
        tasks_dir: Option<PathBuf>,
    },
}

#[cfg(feature = "checkpoint")]
#[derive(Subcommand)]
enum CheckpointAction {
    /// Create a checkpoint for the current HEAD commit
    Create {
        /// Agent name to record in the checkpoint
        #[arg(short, long)]
        agent: String,

        /// Team name (reads team config and tasks if provided)
        #[arg(short, long)]
        team: Option<String>,

        /// Include extended data (tool calls, token usage) from JSONL session
        #[arg(long)]
        extended: bool,

        /// Path to a Claude Code JSONL session file for extended data.
        /// When --extended is set without this flag, auto-discovery is used.
        #[arg(long)]
        session_jsonl: Option<PathBuf>,

        /// Model name to record
        #[arg(long)]
        model: Option<String>,

        /// Repository path (default: current directory)
        #[arg(long)]
        repo: Option<PathBuf>,

        /// Custom teams directory (default: ~/.claude/teams)
        #[arg(long)]
        teams_dir: Option<PathBuf>,

        /// Custom tasks directory (default: ~/.claude/tasks)
        #[arg(long)]
        tasks_dir: Option<PathBuf>,
    },

    /// Show the checkpoint for a commit
    Show {
        /// Commit reference (SHA, HEAD, branch name). Default: HEAD
        #[arg(default_value = "HEAD")]
        commit: String,

        /// Output format
        #[arg(short, long, default_value = "summary")]
        format: CheckpointOutputFormat,

        /// Repository path (default: current directory)
        #[arg(long)]
        repo: Option<PathBuf>,
    },

    /// List checkpoints in the repository
    List {
        /// Filter by branch name
        #[arg(short, long)]
        branch: Option<String>,

        /// Filter by agent name
        #[arg(short, long)]
        agent: Option<String>,

        /// Filter by team name
        #[arg(long)]
        team: Option<String>,

        /// Maximum number of results
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,

        /// Output format
        #[arg(short, long, default_value = "table")]
        format: CheckpointListFormat,

        /// Repository path (default: current directory)
        #[arg(long)]
        repo: Option<PathBuf>,
    },

    /// Diff two checkpoints
    Diff {
        /// From commit reference
        from: String,

        /// To commit reference
        to: String,

        /// Repository path (default: current directory)
        #[arg(long)]
        repo: Option<PathBuf>,
    },

    /// Show token cost summary for a repository
    Cost {
        /// Filter by branch name
        #[arg(short, long)]
        branch: Option<String>,

        /// Filter by agent name
        #[arg(short, long)]
        agent: Option<String>,

        /// Repository path (default: current directory)
        #[arg(long)]
        repo: Option<PathBuf>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Manage post-commit hook for auto-checkpointing
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
}

#[cfg(feature = "checkpoint")]
#[derive(Subcommand)]
enum HookAction {
    /// Install the post-commit hook
    Install {
        /// Repository path (default: current directory)
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Uninstall the post-commit hook
    Uninstall {
        /// Repository path (default: current directory)
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

#[derive(Clone, clap::ValueEnum)]
enum OutputFormat {
    Terminal,
    Mermaid,
    Dot,
    Plain,
}

#[cfg(feature = "checkpoint")]
#[derive(Clone, clap::ValueEnum)]
enum CheckpointOutputFormat {
    Summary,
    Json,
}

#[cfg(feature = "checkpoint")]
#[derive(Clone, clap::ValueEnum)]
enum CheckpointListFormat {
    Table,
    Json,
}

fn tasks_dir(custom: Option<PathBuf>) -> PathBuf {
    custom.unwrap_or_else(|| {
        dirs::home_dir()
            .expect("Could not determine home directory")
            .join(".claude")
            .join("tasks")
    })
}

fn load_tasks(base: &PathBuf, team: &str) -> Vec<TaskFile> {
    let dir = base.join(team);

    if !dir.exists() {
        eprintln!("Error: team directory not found: {}", dir.display());
        process::exit(1);
    }

    let mut tasks = Vec::new();

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error reading directory {}: {e}", dir.display());
            process::exit(1);
        }
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();

        // Only parse numeric .json files (matches FileTaskManager behavior)
        if let Some(stem) = name.strip_suffix(".json") {
            if stem.parse::<u64>().is_ok() {
                match std::fs::read_to_string(entry.path()) {
                    Ok(data) => match serde_json::from_str::<TaskFile>(&data) {
                        Ok(task) => tasks.push(task),
                        Err(e) => {
                            eprintln!("Warning: failed to parse {}: {e}", entry.path().display());
                        }
                    },
                    Err(e) => {
                        eprintln!("Warning: failed to read {}: {e}", entry.path().display());
                    }
                }
            }
        }
    }

    // Sort by numeric ID for consistent output
    tasks.sort_by(|a, b| {
        let a_num = a.id.parse::<u64>().unwrap_or(u64::MAX);
        let b_num = b.id.parse::<u64>().unwrap_or(u64::MAX);
        a_num.cmp(&b_num)
    });

    tasks
}

#[cfg(feature = "checkpoint")]
fn repo_path(custom: Option<PathBuf>) -> PathBuf {
    custom
        .unwrap_or_else(|| std::env::current_dir().expect("Could not determine current directory"))
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Dag { action } => handle_dag(action),
        #[cfg(feature = "checkpoint")]
        Commands::Checkpoint { action } => handle_checkpoint(action),
        #[cfg(feature = "tui")]
        Commands::Tui { team, repo } => handle_tui(team, repo),
    }
}

fn handle_dag(action: DagAction) {
    match action {
        DagAction::Validate {
            team,
            tasks_dir: custom,
        } => {
            let base = tasks_dir(custom);
            let tasks = load_tasks(&base, &team);
            let graph = DependencyGraph::from_tasks(&tasks);

            match graph.topological_sort() {
                Ok(order) => {
                    println!("OK: DAG is valid ({} tasks, no cycles)", order.len());
                    println!("Topological order: {}", order.join(" -> "));
                }
                Err(e) => {
                    eprintln!("FAIL: {e}");
                    process::exit(1);
                }
            }
        }

        DagAction::Show {
            team,
            format,
            tasks_dir: custom,
        } => {
            let base = tasks_dir(custom);
            let tasks = load_tasks(&base, &team);

            if tasks.is_empty() {
                println!("No tasks found for team \"{team}\"");
                return;
            }

            let graph = DependencyGraph::from_tasks(&tasks);

            match format {
                OutputFormat::Terminal => print!("{}", graph.to_terminal(&tasks)),
                OutputFormat::Plain => print!("{}", graph.to_terminal_plain(&tasks)),
                OutputFormat::Mermaid => print!("{}", graph.to_mermaid(&tasks)),
                OutputFormat::Dot => print!("{}", graph.to_dot(&tasks)),
            }
        }

        DagAction::Next {
            team,
            tasks_dir: custom,
        } => {
            let base = tasks_dir(custom);
            let tasks = load_tasks(&base, &team);
            let graph = DependencyGraph::from_tasks(&tasks);

            let ready: Vec<&TaskFile> = tasks
                .iter()
                .filter(|t| {
                    t.status == agent_teams::TaskStatus::Pending && graph.is_ready(&t.id, &tasks)
                })
                .collect();

            if ready.is_empty() {
                println!("No ready tasks (all pending tasks are blocked or none remain)");
            } else {
                println!("Ready tasks ({} unblocked):", ready.len());
                for t in &ready {
                    let owner = t.owner.as_deref().unwrap_or("unassigned");
                    println!("  #{:<4} {} [{}]", t.id, t.subject, owner);
                }
            }
        }

        DagAction::CriticalPath {
            team,
            tasks_dir: custom,
        } => {
            let base = tasks_dir(custom);
            let tasks = load_tasks(&base, &team);
            let graph = DependencyGraph::from_tasks(&tasks);

            let path = graph.critical_path(&tasks);

            if path.is_empty() {
                println!("No critical path (empty or fully independent DAG)");
                return;
            }

            let task_map: std::collections::HashMap<&str, &TaskFile> =
                tasks.iter().map(|t| (t.id.as_str(), t)).collect();

            println!("Critical path ({} tasks):", path.len());
            for (i, id) in path.iter().enumerate() {
                let arrow = if i < path.len() - 1 { " ->" } else { "" };
                if let Some(task) = task_map.get(id.as_str()) {
                    println!("  #{:<4} {}{arrow}", id, task.subject);
                } else {
                    println!("  #{id}{arrow}");
                }
            }
        }
    }
}

#[cfg(feature = "checkpoint")]
fn handle_checkpoint(action: CheckpointAction) {
    use agent_teams::checkpoint::{
        CheckpointCollector, CheckpointFilter, CheckpointQuery, CheckpointStore,
    };
    use agent_teams::models::checkpoint::CheckpointSession;
    use agent_teams::util::session_discovery;

    match action {
        CheckpointAction::Create {
            agent,
            team,
            extended,
            session_jsonl,
            model,
            repo,
            teams_dir,
            tasks_dir: custom_tasks_dir,
        } => {
            let path = repo_path(repo);
            let mut collector = CheckpointCollector::new(&path);

            if let Some(ref team_name) = team {
                collector = collector.with_team(team_name);
            }
            if let Some(ref dir) = teams_dir {
                collector = collector.with_teams_base(dir);
            }
            if let Some(ref dir) = custom_tasks_dir {
                collector = collector.with_tasks_base(dir);
            }

            let mut session = CheckpointSession::new(&agent);
            session.model = model;

            let mut checkpoint = match collector.collect(session) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error creating checkpoint: {e}");
                    process::exit(1);
                }
            };

            // Extended data from JSONL session
            if extended {
                if let Some(ref jsonl_path) = session_jsonl {
                    // Explicit path provided
                    match CheckpointCollector::parse_jsonl_session(jsonl_path) {
                        Ok((tool_calls, token_usage)) => {
                            checkpoint.tool_calls = tool_calls;
                            checkpoint.token_usage = token_usage;
                        }
                        Err(e) => {
                            eprintln!("Warning: failed to parse JSONL session: {e}");
                        }
                    }
                } else {
                    // Auto-discover session files
                    let sessions = session_discovery::discover_sessions(&path);
                    if sessions.is_empty() {
                        eprintln!(
                            "Warning: --extended but no session files found for auto-discovery"
                        );
                        eprintln!("  Use --session-jsonl <PATH> to specify explicitly");
                    } else {
                        eprintln!(
                            "Auto-discovered {} session file(s), using most recent",
                            sessions.len()
                        );
                        match session_discovery::parse_session_file(&sessions[0].path) {
                            Ok((tool_calls, token_usage)) => {
                                checkpoint.tool_calls = tool_calls;
                                checkpoint.token_usage = token_usage;
                            }
                            Err(e) => {
                                eprintln!("Warning: failed to parse auto-discovered session: {e}");
                            }
                        }
                    }
                }
            }

            let store = match CheckpointStore::open(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error opening repository: {e}");
                    process::exit(1);
                }
            };

            match store.save(&checkpoint) {
                Ok(()) => {
                    println!("Checkpoint created: {}", checkpoint.id);
                    println!(
                        "  Commit: {} ({})",
                        &checkpoint.commit_sha[..7.min(checkpoint.commit_sha.len())],
                        checkpoint.branch
                    );
                    println!("  Agent:  {}", checkpoint.session.agent_name);
                    println!("  Files:  {}", checkpoint.files.len());
                    if let Some(ref team_state) = checkpoint.team {
                        println!(
                            "  Team:   {} ({} members)",
                            team_state.team_name,
                            team_state.members.len()
                        );
                    }
                    if !checkpoint.tasks.is_empty() {
                        println!("  Tasks:  {}", checkpoint.tasks.len());
                    }
                    if checkpoint.has_extended_data() {
                        println!(
                            "  Extended: yes ({} tool calls)",
                            checkpoint.tool_calls.len()
                        );
                    }
                }
                Err(e) => {
                    eprintln!("Error saving checkpoint: {e}");
                    process::exit(1);
                }
            }
        }

        CheckpointAction::Show {
            commit,
            format,
            repo,
        } => {
            let path = repo_path(repo);
            let store = match CheckpointStore::open(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error opening repository: {e}");
                    process::exit(1);
                }
            };

            let checkpoint = match store.load(&commit) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            };

            match format {
                CheckpointOutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&checkpoint).unwrap());
                }
                CheckpointOutputFormat::Summary => {
                    print_checkpoint_summary(&checkpoint);
                }
            }
        }

        CheckpointAction::List {
            branch,
            agent,
            team,
            limit,
            format,
            repo,
        } => {
            let path = repo_path(repo);
            let query = match CheckpointQuery::open(&path) {
                Ok(q) => q,
                Err(e) => {
                    eprintln!("Error opening repository: {e}");
                    process::exit(1);
                }
            };

            let filter = CheckpointFilter {
                branch,
                agent,
                team,
                limit: Some(limit),
                ..Default::default()
            };

            let checkpoints = match query.list(&filter) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error listing checkpoints: {e}");
                    process::exit(1);
                }
            };

            if checkpoints.is_empty() {
                println!("No checkpoints found.");
                return;
            }

            match format {
                CheckpointListFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&checkpoints).unwrap());
                }
                CheckpointListFormat::Table => {
                    println!(
                        "{:<12} {:<9} {:<16} {:<16} {:<6} {:<20}",
                        "COMMIT", "BRANCH", "AGENT", "TEAM", "FILES", "CREATED"
                    );
                    println!("{}", "-".repeat(82));
                    for ckpt in &checkpoints {
                        let sha_short = &ckpt.commit_sha[..7.min(ckpt.commit_sha.len())];
                        let team_name = ckpt
                            .team
                            .as_ref()
                            .map(|t| t.team_name.as_str())
                            .unwrap_or("-");
                        let created = ckpt.created_at.format("%Y-%m-%d %H:%M");
                        println!(
                            "{:<12} {:<9} {:<16} {:<16} {:<6} {:<20}",
                            sha_short,
                            truncate_display(&ckpt.branch, 9),
                            truncate_display(&ckpt.session.agent_name, 16),
                            truncate_display(team_name, 16),
                            ckpt.files.len(),
                            created,
                        );
                    }
                    println!("\n{} checkpoint(s) found", checkpoints.len());
                }
            }
        }

        CheckpointAction::Diff { from, to, repo } => {
            let path = repo_path(repo);
            let query = match CheckpointQuery::open(&path) {
                Ok(q) => q,
                Err(e) => {
                    eprintln!("Error opening repository: {e}");
                    process::exit(1);
                }
            };

            let diff = match query.diff(&from, &to) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Error computing diff: {e}");
                    process::exit(1);
                }
            };

            println!(
                "Checkpoint diff: {}..{}",
                &diff.from_sha[..7.min(diff.from_sha.len())],
                &diff.to_sha[..7.min(diff.to_sha.len())]
            );
            println!();

            if !diff.files_added.is_empty() {
                println!("Files added:");
                for f in &diff.files_added {
                    println!("  + {f}");
                }
            }
            if !diff.files_removed.is_empty() {
                println!("Files removed:");
                for f in &diff.files_removed {
                    println!("  - {f}");
                }
            }
            if !diff.files_modified.is_empty() {
                println!("Files modified:");
                for f in &diff.files_modified {
                    println!("  ~ {f}");
                }
            }

            if !diff.tasks_changed.is_empty() {
                println!("\nTask changes:");
                for t in &diff.tasks_changed {
                    let symbol = match t.change {
                        agent_teams::checkpoint::query::DeltaKind::Added => "+",
                        agent_teams::checkpoint::query::DeltaKind::Removed => "-",
                        agent_teams::checkpoint::query::DeltaKind::Changed => "~",
                    };
                    let status_info = match (&t.old_status, &t.new_status) {
                        (Some(old), Some(new)) => format!(" ({old} -> {new})"),
                        (None, Some(new)) => format!(" ({new})"),
                        (Some(old), None) => format!(" (was {old})"),
                        (None, None) => String::new(),
                    };
                    println!("  {symbol} #{} {}{status_info}", t.task_id, t.subject);
                }
            }

            if !diff.members_changed.is_empty() {
                println!("\nMember changes:");
                for m in &diff.members_changed {
                    let symbol = match m.change {
                        agent_teams::checkpoint::query::DeltaKind::Added => "+",
                        agent_teams::checkpoint::query::DeltaKind::Removed => "-",
                        agent_teams::checkpoint::query::DeltaKind::Changed => "~",
                    };
                    println!("  {symbol} {}", m.name);
                }
            }

            if diff.files_added.is_empty()
                && diff.files_removed.is_empty()
                && diff.files_modified.is_empty()
                && diff.tasks_changed.is_empty()
                && diff.members_changed.is_empty()
            {
                println!("No differences found.");
            }
        }

        CheckpointAction::Cost {
            branch,
            agent,
            repo,
            json,
        } => {
            let path = repo_path(repo);

            // Gather token usage from checkpoints
            let query = match CheckpointQuery::open(&path) {
                Ok(q) => q,
                Err(e) => {
                    eprintln!("Error opening repository: {e}");
                    process::exit(1);
                }
            };

            let filter = CheckpointFilter {
                branch,
                agent: agent.clone(),
                ..Default::default()
            };

            let checkpoints = match query.list(&filter) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error listing checkpoints: {e}");
                    process::exit(1);
                }
            };

            // Aggregate from checkpoints that have token data
            let mut total = agent_teams::models::token::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: None,
                cache_write_tokens: None,
            };
            let mut per_agent: std::collections::HashMap<
                String,
                agent_teams::models::token::TokenUsage,
            > = std::collections::HashMap::new();
            let mut count = 0;

            for ckpt in &checkpoints {
                if let Some(ref usage) = ckpt.token_usage {
                    total.merge(usage);
                    count += 1;

                    let agent_name = &ckpt.session.agent_name;
                    per_agent
                        .entry(agent_name.clone())
                        .and_modify(|u| u.merge(usage))
                        .or_insert_with(|| usage.clone());
                }
            }

            // Also try auto-discovered sessions
            let discovered = session_discovery::discover_sessions(&path);
            let session_cost = session_discovery::aggregate_cost(&discovered, agent.as_deref())
                .unwrap_or_else(|e| {
                    eprintln!("Warning: session discovery failed: {e}");
                    agent_teams::models::token::CostSummary {
                        total_usage: agent_teams::models::token::TokenUsage {
                            input_tokens: 0,
                            output_tokens: 0,
                            cache_read_tokens: None,
                            cache_write_tokens: None,
                        },
                        session_count: 0,
                        per_agent: vec![],
                        estimated_cost_usd: 0.0,
                    }
                });

            if json {
                let summary = serde_json::json!({
                    "checkpoint_data": {
                        "checkpoints_with_tokens": count,
                        "total_checkpoints": checkpoints.len(),
                        "usage": total,
                        "estimated_cost_usd": agent_teams::models::token::estimate_cost(&total),
                    },
                    "session_data": session_cost,
                });
                println!("{}", serde_json::to_string_pretty(&summary).unwrap());
            } else {
                println!("Token Cost Summary");
                println!("{}", "=".repeat(50));
                println!();

                if count > 0 {
                    println!(
                        "From checkpoints ({count}/{} have token data):",
                        checkpoints.len()
                    );
                    println!("  Input tokens:  {:>12}", total.input_tokens);
                    println!("  Output tokens: {:>12}", total.output_tokens);
                    if let Some(cr) = total.cache_read_tokens {
                        println!("  Cache read:    {:>12}", cr);
                    }
                    if let Some(cw) = total.cache_write_tokens {
                        println!("  Cache write:   {:>12}", cw);
                    }
                    println!(
                        "  Estimated cost: ${:.4}",
                        agent_teams::models::token::estimate_cost(&total)
                    );
                    println!();

                    if per_agent.len() > 1 {
                        println!("  Per agent:");
                        let mut agents: Vec<_> = per_agent.iter().collect();
                        agents.sort_by(|a, b| b.1.total().cmp(&a.1.total()));
                        for (name, usage) in &agents {
                            println!(
                                "    {:<20} {:>8} in / {:>8} out  (${:.4})",
                                name,
                                usage.input_tokens,
                                usage.output_tokens,
                                agent_teams::models::token::estimate_cost(usage),
                            );
                        }
                        println!();
                    }
                } else {
                    println!("No checkpoint token data found.");
                    println!();
                }

                if session_cost.session_count > 0 {
                    println!(
                        "From auto-discovered sessions ({} files):",
                        session_cost.session_count
                    );
                    println!(
                        "  Input tokens:  {:>12}",
                        session_cost.total_usage.input_tokens
                    );
                    println!(
                        "  Output tokens: {:>12}",
                        session_cost.total_usage.output_tokens
                    );
                    println!("  Estimated cost: ${:.4}", session_cost.estimated_cost_usd);
                } else {
                    println!("No auto-discovered session data found.");
                }
            }
        }

        CheckpointAction::Hook { action } => {
            handle_hook(action);
        }
    }
}

#[cfg(feature = "checkpoint")]
fn handle_hook(action: HookAction) {
    const HOOK_MARKER: &str = "# agent-teams auto-checkpoint hook";
    const HOOK_SCRIPT: &str = r#"#!/bin/sh
# agent-teams auto-checkpoint hook
# Installed by: agent-teams checkpoint hook install
# Remove with:  agent-teams checkpoint hook uninstall

if command -v agent-teams >/dev/null 2>&1; then
    agent-teams checkpoint create --agent auto --extended 2>/dev/null || true
fi
"#;

    match action {
        HookAction::Install { repo } => {
            let path = repo_path(repo);
            let hooks_dir = path.join(".git").join("hooks");

            if !hooks_dir.exists() {
                eprintln!(
                    "Error: not a git repository (no .git/hooks): {}",
                    path.display()
                );
                process::exit(1);
            }

            let hook_path = hooks_dir.join("post-commit");

            if hook_path.exists() {
                // Check if our hook is already installed
                let content = std::fs::read_to_string(&hook_path).unwrap_or_default();
                if content.contains(HOOK_MARKER) {
                    println!("Hook already installed at {}", hook_path.display());
                    return;
                }
                // Append to existing hook
                let appended = format!("{content}\n{HOOK_SCRIPT}");
                std::fs::write(&hook_path, appended).unwrap_or_else(|e| {
                    eprintln!("Error writing hook: {e}");
                    process::exit(1);
                });
                println!("Hook appended to existing {}", hook_path.display());
            } else {
                std::fs::write(&hook_path, HOOK_SCRIPT).unwrap_or_else(|e| {
                    eprintln!("Error writing hook: {e}");
                    process::exit(1);
                });
                // Make executable
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(0o755);
                    std::fs::set_permissions(&hook_path, perms).unwrap_or_else(|e| {
                        eprintln!("Warning: could not set executable permission: {e}");
                    });
                }
                println!("Hook installed at {}", hook_path.display());
            }
        }

        HookAction::Uninstall { repo } => {
            let path = repo_path(repo);
            let hook_path = path.join(".git").join("hooks").join("post-commit");

            if !hook_path.exists() {
                println!("No post-commit hook found.");
                return;
            }

            let content = std::fs::read_to_string(&hook_path).unwrap_or_default();
            if !content.contains(HOOK_MARKER) {
                println!("No agent-teams hook found in post-commit.");
                return;
            }

            // Remove our section from the hook
            let lines: Vec<&str> = content.lines().collect();
            let mut new_lines = Vec::new();
            let mut skip = false;

            for line in &lines {
                if line.contains(HOOK_MARKER) {
                    skip = true;
                    continue;
                }
                if skip {
                    // Skip until we find a line that's not part of our script
                    if line.starts_with('#')
                        || line.starts_with("if ")
                        || line.starts_with("    ")
                        || line.starts_with("fi")
                        || line.is_empty()
                    {
                        continue;
                    }
                    skip = false;
                }
                if !skip {
                    new_lines.push(*line);
                }
            }

            let new_content = new_lines.join("\n");
            if new_content.trim().is_empty() || new_content.trim() == "#!/bin/sh" {
                std::fs::remove_file(&hook_path).unwrap_or_else(|e| {
                    eprintln!("Error removing hook: {e}");
                    process::exit(1);
                });
                println!("Hook removed: {}", hook_path.display());
            } else {
                std::fs::write(&hook_path, new_content).unwrap_or_else(|e| {
                    eprintln!("Error writing hook: {e}");
                    process::exit(1);
                });
                println!("Agent-teams section removed from post-commit hook.");
            }
        }
    }
}

#[cfg(feature = "tui")]
fn handle_tui(team: Option<String>, repo: Option<PathBuf>) {
    let repo_path = repo
        .unwrap_or_else(|| std::env::current_dir().expect("Could not determine current directory"));

    match agent_teams::tui::run(team, Some(repo_path)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("TUI error: {e}");
            process::exit(1);
        }
    }
}

#[cfg(feature = "checkpoint")]
fn print_checkpoint_summary(ckpt: &agent_teams::models::checkpoint::Checkpoint) {
    println!("Checkpoint: {}", ckpt.id);
    println!("  Commit:  {} ({})", ckpt.commit_sha, ckpt.branch);
    println!(
        "  Created: {}",
        ckpt.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!();

    println!("Session:");
    println!("  Agent:   {}", ckpt.session.agent_name);
    if let Some(ref backend) = ckpt.session.backend_type {
        println!("  Backend: {backend}");
    }
    if let Some(ref model) = ckpt.session.model {
        println!("  Model:   {model}");
    }
    if let Some(ref cwd) = ckpt.session.cwd {
        println!("  CWD:     {}", cwd.display());
    }
    if let Some(ref prompt) = ckpt.session.prompt_summary {
        println!(
            "  Prompt:  {}",
            if prompt.len() > 80 {
                format!("{}...", &prompt[..77])
            } else {
                prompt.clone()
            }
        );
    }

    if let Some(ref team) = ckpt.team {
        println!();
        println!("Team: {} ({} members)", team.team_name, team.members.len());
        if let Some(ref desc) = team.description {
            println!("  {desc}");
        }
        for m in &team.members {
            println!("  - {} ({})", m.name, m.agent_type);
        }
    }

    if !ckpt.tasks.is_empty() {
        println!();
        println!("Tasks ({}):", ckpt.tasks.len());
        for t in &ckpt.tasks {
            let owner = t.owner.as_deref().unwrap_or("unassigned");
            println!("  #{:<4} {} [{}] @{}", t.id, t.subject, t.status, owner);
        }
    }

    if !ckpt.files.is_empty() {
        println!();
        println!("Files ({}):", ckpt.files.len());
        for f in &ckpt.files {
            let symbol = match f.role {
                agent_teams::models::checkpoint::FileRole::Created => "+",
                agent_teams::models::checkpoint::FileRole::Modified => "~",
                agent_teams::models::checkpoint::FileRole::Deleted => "-",
                agent_teams::models::checkpoint::FileRole::Referenced => "?",
            };
            println!("  {symbol} {}", f.path);
        }
    }

    if let Some(ref usage) = ckpt.token_usage {
        println!();
        println!("Token usage:");
        println!("  Input:  {}", usage.input_tokens);
        println!("  Output: {}", usage.output_tokens);
        if let Some(cache_read) = usage.cache_read_tokens {
            println!("  Cache read:  {cache_read}");
        }
        if let Some(cache_write) = usage.cache_write_tokens {
            println!("  Cache write: {cache_write}");
        }
        println!(
            "  Estimated cost: ${:.4}",
            agent_teams::models::token::estimate_cost(usage)
        );
    }

    if !ckpt.tool_calls.is_empty() {
        println!();
        println!("Tool calls ({}):", ckpt.tool_calls.len());
        for tc in &ckpt.tool_calls {
            let input = tc
                .input_summary
                .as_deref()
                .map(|s| {
                    if s.len() > 60 {
                        format!("{}...", &s[..57])
                    } else {
                        s.to_string()
                    }
                })
                .unwrap_or_default();
            println!("  {} {}", tc.tool_name, input);
        }
    }
}

#[cfg(feature = "checkpoint")]
fn truncate_display(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}..", &s[..max.saturating_sub(2)])
    }
}
