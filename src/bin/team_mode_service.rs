use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use agent_teams::team_mode::data_dir;
use agent_teams::team_mode::mcp::http_transport::{HttpMcpState, router as http_mcp_router};
use agent_teams::team_mode::mcp::{TeamModeMcpRuntime, TeamModeToolset};
use agent_teams::team_mode::runtime_workers::RuntimeWorkerStore;
use axum::Router;
use clap::{Parser, Subcommand};
use include_dir::{Dir, include_dir};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

mod team_mode_service_shell;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8786;
const LEAD_WATCH_INTERVAL: Duration = Duration::from_secs(5);
const WORKER_LIVENESS_INTERVAL: Duration = Duration::from_secs(5);
const MCP_HEADERS_HELPER: &str = ".agent-teams/scripts/mcp-http-headers.js";
const STOP_HOOK_SCRIPT: &str = ".agent-teams/scripts/hooks/lead-pending-async-wake.js";
const POST_TOOL_USE_HOOK_SCRIPT: &str = ".agent-teams/scripts/hooks/lead-pending-mid-turn.js";
const EMBEDDED_HEADERS_HELPER: &str = include_str!("../../scripts/mcp-http-headers.js");
const EMBEDDED_STOP_HOOK: &str = include_str!("../../scripts/hooks/lead-pending-async-wake.js");
const EMBEDDED_POST_TOOL_USE_HOOK: &str =
    include_str!("../../scripts/hooks/lead-pending-mid-turn.js");

const SKILL_INSTALL_NAME: &str = "agent-teams-mcp-setup";
static EMBEDDED_SKILL_DIR: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/.claude/skills/agent-teams-mcp-setup");

#[derive(Debug, Parser)]
#[command(name = "team_mode_service")]
#[command(about = "Local HTTP Team Mode MCP service")]
struct Cli {
    #[command(subcommand)]
    command: Option<ServiceCommand>,

    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long)]
    project_root: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_HOST)]
    host: String,
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    #[arg(long)]
    token_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Install global Team Mode config into user scope.
    InstallGlobal,
    /// Remove global Team Mode config from user scope.
    UninstallGlobal,
    /// Scaffold Team Mode config into a project directory.
    Init {
        /// Target project directory. Defaults to the current directory.
        target_project_dir: Option<PathBuf>,
    },
    /// Proxy stdio JSON-RPC to the HTTP service.
    Relay,
    /// Rewrite of the Stop hook.
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
    /// Rewrite of the headers helper.
    Headers,
}

#[derive(Debug, Subcommand)]
enum HookCommand {
    AsyncWake,
    MidTurn,
}

#[derive(Debug)]
struct ServiceArgs {
    data_dir: PathBuf,
    data_dir_explicit: bool,
    project_root: PathBuf,
    host: String,
    port: u16,
    token_file: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct RuntimeInfo {
    pid: u32,
    host: String,
    port: u16,
    url: String,
    token_file: PathBuf,
    started_at: String,
    binary_commit: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Some(ServiceCommand::InstallGlobal) => {
            install_global()?;
            return Ok(());
        }
        Some(ServiceCommand::UninstallGlobal) => {
            uninstall_global()?;
            return Ok(());
        }
        Some(ServiceCommand::Init { target_project_dir }) => {
            let target = target_project_dir.unwrap_or(env::current_dir()?);
            init_project(&target)?;
            return Ok(());
        }
        Some(ServiceCommand::Relay) => {
            return team_mode_service_shell::relay_stdio(
                cli.data_dir.clone(),
                cli.project_root.clone(),
            );
        }
        Some(ServiceCommand::Hook { command }) => match command {
            HookCommand::AsyncWake => team_mode_service_shell::run_async_wake_hook(),
            HookCommand::MidTurn => team_mode_service_shell::run_mid_turn_hook(),
        },
        Some(ServiceCommand::Headers) => {
            return team_mode_service_shell::headers_stdout(cli.data_dir.clone());
        }
        None => {}
    }

    init_tracing();
    let args = service_args_from_cli(cli)?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_service(args))
}

async fn run_service(args: ServiceArgs) -> Result<(), Box<dyn std::error::Error>> {
    if args.host != DEFAULT_HOST {
        return Err("team_mode_service only binds 127.0.0.1".into());
    }
    data_dir::ensure_scaffold(&args.data_dir)?;
    let runtime_dir = runtime_dir_for_service(&args)?;
    std::fs::create_dir_all(&runtime_dir)?;
    let _service_lock = match team_mode_service_shell::try_acquire_service_lock(&runtime_dir)? {
        Some(lock) => lock,
        None => {
            tracing::info!(
                lock_path = %team_mode_service_shell::service_lock_path(&runtime_dir).display(),
                "team_mode_service already holds runtime lock; exiting idempotently"
            );
            return Ok(());
        }
    };

    let token_file = args
        .token_file
        .clone()
        .unwrap_or_else(|| runtime_dir.join("http-mcp.token"));
    let token = read_or_create_token(&token_file)?;
    let url = format!("http://{}:{}/mcp", args.host, args.port);
    write_runtime_info(&runtime_dir, &args, &token_file, &url)?;
    set_child_env(&url, &token);

    let marked_dead = RuntimeWorkerStore::new(args.data_dir.clone())
        .mark_daemon_restart_dead(std::process::id())?;
    tracing::info!(
        pid = std::process::id(),
        %url,
        marked_dead,
        data_dir = %args.data_dir.display(),
        project_root = %args.project_root.display(),
        "Team Mode service starting"
    );

    let toolset = Arc::new(TeamModeToolset::new_with_project_root(
        args.data_dir.clone(),
        Some(args.project_root.clone()),
    ));
    pre_spawn_web(&args.data_dir);
    spawn_watchdogs(Arc::clone(&toolset))?;

    let runtime = TeamModeMcpRuntime::with_tool_executor(
        args.data_dir.clone(),
        Box::new(Arc::clone(&toolset)),
    );
    let lock_holder_pid = std::process::id();
    let app = Router::new().merge(http_mcp_router(HttpMcpState::new(
        runtime,
        token,
        args.data_dir.clone(),
        runtime_dir.clone(),
        lock_holder_pid,
        Instant::now(),
    )));
    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "Team Mode HTTP MCP service listening");
    axum::serve(listener, app).await?;
    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| EnvFilter::new("info,agent_teams=debug")),
        )
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_thread_ids(false)
        .init();
}

fn service_args_from_cli(cli: Cli) -> Result<ServiceArgs, Box<dyn std::error::Error>> {
    let project_root = cli.project_root.unwrap_or(env::current_dir()?);
    let data_dir_explicit = cli.data_dir.is_some();
    let data_dir = cli
        .data_dir
        .unwrap_or_else(|| data_dir::resolve_default_base_dir(&project_root));
    Ok(ServiceArgs {
        data_dir,
        data_dir_explicit,
        project_root,
        host: cli.host,
        port: cli.port,
        token_file: cli.token_file,
    })
}

fn global_home_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    dirs::home_dir().ok_or_else(|| {
        std::io::Error::other("could not resolve home directory for global Team Mode install")
            .into()
    })
}

fn global_claude_json_path(home: &Path) -> PathBuf {
    home.join(".claude.json")
}

fn global_claude_settings_path(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

fn install_global() -> Result<(), Box<dyn std::error::Error>> {
    let home = global_home_dir()?;
    let warning = install_global_at(&home)?;
    println!("Team Mode global config installed at {}", home.display());
    if let Some(warning) = warning {
        println!();
        print!("{warning}");
    }
    println!("Next steps:");
    println!("  team_mode_service");
    println!("  claude");
    Ok(())
}

fn uninstall_global() -> Result<(), Box<dyn std::error::Error>> {
    let home = global_home_dir()?;
    uninstall_global_at(&home)?;
    println!("Team Mode global config removed from {}", home.display());
    Ok(())
}

fn install_global_at(home: &Path) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mcp_path = global_claude_json_path(home);
    let settings_path = global_claude_settings_path(home);
    if let Some(mcp_json) = merged_global_mcp_json(&mcp_path)? {
        write_json_pretty(&mcp_path, &mcp_json)?;
    }
    if let Some(settings_json) = merged_global_claude_settings_json(&settings_path)? {
        write_json_pretty(&settings_path, &settings_json)?;
    }
    install_skill_global_at(home)?;
    legacy_v2_hook_warning_section(home)
}

fn uninstall_global_at(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mcp_path = global_claude_json_path(home);
    if mcp_path.exists() {
        let mut mcp_json = read_json_file(&mcp_path)?;
        remove_global_mcp_server(&mut mcp_json, &mcp_path)?;
        if global_mcp_json_is_empty_shell(&mcp_json, &mcp_path)? {
            fs::remove_file(&mcp_path)?;
        } else {
            write_json_pretty(&mcp_path, &mcp_json)?;
        }
    }

    let settings_path = global_claude_settings_path(home);
    if settings_path.exists() {
        let mut settings_json = read_json_file(&settings_path)?;
        remove_global_hooks(&mut settings_json, &settings_path)?;
        if global_settings_json_is_empty_shell(&settings_json, &settings_path)? {
            fs::remove_file(&settings_path)?;
        } else {
            write_json_pretty(&settings_path, &settings_json)?;
        }
    }

    uninstall_skill_global_at(home)?;
    remove_empty_directory(&home.join(".claude").join("skills"))?;
    remove_empty_directory(&home.join(".claude"))?;
    Ok(())
}

fn skill_install_root(home: &Path) -> PathBuf {
    home.join(".claude")
        .join("skills")
        .join(SKILL_INSTALL_NAME)
}

/// Recursively yield every embedded file under `dir` (relative paths kept).
fn walk_embedded_files<'a>(dir: &'a Dir<'a>, out: &mut Vec<&'a include_dir::File<'a>>) {
    for f in dir.files() {
        out.push(f);
    }
    for sub in dir.dirs() {
        walk_embedded_files(sub, out);
    }
}

/// Write the embedded `agent-teams-mcp-setup` skill into `<home>/.claude/skills/`.
///
/// Idempotent: files whose on-disk bytes already match the embedded source are
/// left untouched. Fail-closed: if a target file exists with **different**
/// contents we refuse and surface the path, mirroring the
/// `mcpServers.team-mode` conflict policy. This keeps user-authored skill
/// edits safe from silent clobbering across reinstalls.
fn install_skill_global_at(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = skill_install_root(home);
    let mut files = Vec::new();
    walk_embedded_files(&EMBEDDED_SKILL_DIR, &mut files);
    for file in files {
        let rel = file.path();
        let target = root.join(rel);
        let bytes = file.contents();
        if target.exists() {
            let existing = fs::read(&target)?;
            if existing == bytes {
                continue;
            }
            return Err(format!(
                "Refusing to overwrite {} — file content differs from the embedded \
                 agent-teams-mcp-setup skill source. Save your local edits elsewhere, \
                 delete the file, then re-run install-global.",
                target.display()
            )
            .into());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, bytes)?;
    }
    Ok(())
}

/// Symmetric removal: delete files whose on-disk bytes still match the
/// embedded source; preserve user-modified files (and report nothing — the
/// remaining file itself is the trace). Empty subdirectories are pruned.
fn uninstall_skill_global_at(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = skill_install_root(home);
    if !root.exists() {
        return Ok(());
    }
    let mut files = Vec::new();
    walk_embedded_files(&EMBEDDED_SKILL_DIR, &mut files);
    for file in files {
        let target = root.join(file.path());
        if !target.exists() {
            continue;
        }
        let existing = fs::read(&target)?;
        if existing == file.contents() {
            fs::remove_file(&target)?;
        }
    }
    remove_empty_dir_tree(&root)?;
    Ok(())
}

fn remove_empty_dir_tree(dir: &Path) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            remove_empty_dir_tree(&path)?;
        }
    }
    let _ = fs::remove_dir(dir);
    Ok(())
}

fn runtime_dir_for_service(args: &ServiceArgs) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if args.data_dir_explicit {
        return Ok(args.data_dir.join("runtime"));
    }
    let home = dirs::home_dir().ok_or_else(|| {
        "could not resolve home directory for global Team Mode runtime".to_string()
    })?;
    Ok(home.join(".team-mode").join("runtime"))
}

fn read_or_create_token(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    if path.exists() {
        return Ok(std::fs::read_to_string(path)?.trim().to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let token = Uuid::new_v4().to_string();
    std::fs::write(path, &token)?;
    Ok(token)
}

fn write_runtime_info(
    runtime_dir: &Path,
    args: &ServiceArgs,
    token_file: &Path,
    url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let info = RuntimeInfo {
        pid: std::process::id(),
        host: args.host.clone(),
        port: args.port,
        url: url.to_string(),
        token_file: token_file.to_path_buf(),
        started_at: chrono::Utc::now().to_rfc3339(),
        binary_commit: env!("TEAM_MODE_GIT_REV").to_string(),
    };
    let path = runtime_dir.join("http-mcp.json");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&info)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn set_child_env(url: &str, token: &str) {
    // SAFETY: called during single-threaded startup before constructing
    // TeamModeToolset or spawning worker/runtime threads.
    unsafe {
        env::set_var("TEAM_MODE_HTTP_MCP_URL", url);
        env::set_var("TEAM_MODE_HTTP_MCP_TOKEN", token);
    }
}

fn init_project(target: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let target = target.canonicalize().map_err(|err| {
        format!(
            "target project directory '{}' is not accessible: {err}",
            target.display()
        )
    })?;
    if !target.is_dir() {
        return Err(format!("target '{}' is not a directory", target.display()).into());
    }

    let scripts = embedded_script_targets(&target);
    for (path, _) in &scripts {
        if path.exists() {
            return Err(format!(
                "refusing to overwrite existing script '{}'; remove it or merge manually",
                path.display()
            )
            .into());
        }
    }

    let mcp_path = target.join(".mcp.json");
    let settings_path = target.join(".claude").join("settings.json");
    let mcp_json = merged_mcp_json(&mcp_path)?;
    let settings_json = merged_claude_settings_json(&settings_path)?;

    for (path, content) in scripts {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
    }

    write_json_pretty(&mcp_path, &mcp_json)?;
    write_json_pretty(&settings_path, &settings_json)?;
    ensure_gitignore_entry(&target.join(".gitignore"), ".agent-teams/")?;

    println!(
        "Team Mode project config initialized at {}",
        target.display()
    );
    println!("Next steps:");
    println!("  team_mode_service --project-root . --data-dir .agent-teams &");
    println!("  claude");
    Ok(())
}

fn merged_global_mcp_json(path: &Path) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    let mut value = if path.exists() {
        read_json_file(path)?
    } else {
        json!({
            "mcpServers": {}
        })
    };

    let root = value
        .as_object_mut()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    let servers = ensure_object_field(root, "mcpServers", path)?;
    let expected = global_team_mode_mcp_server_json();
    if let Some(existing) = servers.get("team-mode") {
        if *existing == expected {
            return Ok(None);
        }
        return Err(format!(
            "{} already contains mcpServers.team-mode with different config; merge manually",
            path.display()
        )
        .into());
    }
    servers.insert("team-mode".into(), expected);
    Ok(Some(value))
}

fn merged_global_claude_settings_json(
    path: &Path,
) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    let mut value = if path.exists() {
        read_json_file(path)?
    } else {
        json!({})
    };

    let root = value
        .as_object_mut()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    let hooks = ensure_object_field(root, "hooks", path)?;
    let mut changed = false;
    if !hook_event_has_command(hooks, "Stop", "team_mode_service hook async-wake") {
        append_hook_entry(hooks, "Stop", global_stop_hook_entry(), path)?;
        changed = true;
    }
    if !hook_event_has_command(hooks, "PostToolUse", "team_mode_service hook mid-turn") {
        append_hook_entry(
            hooks,
            "PostToolUse",
            global_post_tool_use_hook_entry(),
            path,
        )?;
        changed = true;
    }
    if changed { Ok(Some(value)) } else { Ok(None) }
}

fn hook_event_has_command(hooks: &Map<String, Value>, event: &str, needle: &str) -> bool {
    hooks
        .get(event)
        .map(|v| json_contains_command(v, needle))
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyV2HookWarning {
    project_settings_path: PathBuf,
    stop_commands: Vec<String>,
    post_tool_use_commands: Vec<String>,
}

fn legacy_v2_hook_warning_section(
    home: &Path,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let warnings = collect_legacy_v2_hook_warnings(home)?;
    if warnings.is_empty() {
        return Ok(None);
    }
    Ok(Some(format_legacy_v2_hook_warning(&warnings)))
}

fn collect_legacy_v2_hook_warnings(
    home: &Path,
) -> Result<Vec<LegacyV2HookWarning>, Box<dyn std::error::Error>> {
    let mcp_path = global_claude_json_path(home);
    if !mcp_path.exists() {
        return Ok(Vec::new());
    }
    let mcp_json = read_json_file(&mcp_path)?;
    let Some(projects) = mcp_json.get("projects") else {
        return Ok(Vec::new());
    };

    let mut project_paths = collect_project_paths(projects, home);
    if project_paths.is_empty() {
        return Ok(Vec::new());
    }

    let mut warnings = Vec::new();
    for project_path in project_paths.drain(..) {
        let settings_path = project_path.join(".claude").join("settings.json");
        if !settings_path.exists() {
            continue;
        }
        let Ok(settings_json) = read_json_file(&settings_path) else {
            continue;
        };
        let stop_commands =
            legacy_v2_hook_commands(&settings_json, "Stop", "lead-pending-async-wake");
        let post_tool_use_commands =
            legacy_v2_hook_commands(&settings_json, "PostToolUse", "lead-pending-mid-turn");
        if stop_commands.is_empty() && post_tool_use_commands.is_empty() {
            continue;
        }
        warnings.push(LegacyV2HookWarning {
            project_settings_path: settings_path,
            stop_commands,
            post_tool_use_commands,
        });
    }

    Ok(warnings)
}

fn collect_project_paths(projects: &Value, home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    let mut push_path = |path: PathBuf| {
        let resolved = if path.is_absolute() {
            path
        } else {
            home.join(path)
        };
        let key = resolved.display().to_string();
        if seen.insert(key) {
            paths.push(resolved);
        }
    };

    match projects {
        Value::Array(entries) => {
            for entry in entries {
                collect_project_paths_from_value(entry, &mut push_path);
            }
        }
        Value::Object(map) => {
            if let Some(Value::Array(entries)) = map.get("items") {
                for entry in entries {
                    collect_project_paths_from_value(entry, &mut push_path);
                }
            } else {
                for (key, value) in map {
                    if let Some(path) = project_path_from_value(value) {
                        push_path(path);
                    } else if !key.trim().is_empty() {
                        push_path(PathBuf::from(key));
                    }
                }
            }
        }
        Value::String(path) => push_path(PathBuf::from(path)),
        _ => {}
    }

    paths
}

fn collect_project_paths_from_value(value: &Value, push_path: &mut impl FnMut(PathBuf)) {
    match value {
        Value::String(path) => push_path(PathBuf::from(path)),
        Value::Object(map) => {
            if let Some(path) = project_path_from_value(value) {
                push_path(path);
                return;
            }
            for (key, nested) in map {
                if key == "path" || key == "project_path" || key == "projectPath" {
                    continue;
                }
                if let Some(path) = project_path_from_value(nested) {
                    push_path(path);
                }
            }
        }
        Value::Array(entries) => {
            for entry in entries {
                collect_project_paths_from_value(entry, push_path);
            }
        }
        _ => {}
    }
}

fn project_path_from_value(value: &Value) -> Option<PathBuf> {
    let Value::Object(map) = value else {
        return None;
    };
    for key in ["path", "project_path", "projectPath"] {
        if let Some(Value::String(path)) = map.get(key) {
            return Some(PathBuf::from(path));
        }
    }
    None
}

fn legacy_v2_hook_commands(settings_json: &Value, event: &str, needle: &str) -> Vec<String> {
    let Some(hooks) = settings_json.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    let Some(entries) = hooks.get(event).and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut commands = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        collect_command_matches(entry, needle, &mut seen, &mut commands);
    }
    commands
}

fn collect_command_matches(
    value: &Value,
    needle: &str,
    seen: &mut HashSet<String>,
    commands: &mut Vec<String>,
) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(command)) = map.get("command") {
                if command.contains(needle) && seen.insert(command.clone()) {
                    commands.push(command.clone());
                }
            }
            for (key, nested) in map {
                if key == "command" {
                    continue;
                }
                collect_command_matches(nested, needle, seen, commands);
            }
        }
        Value::Array(entries) => {
            for entry in entries {
                collect_command_matches(entry, needle, seen, commands);
            }
        }
        _ => {}
    }
}

fn format_legacy_v2_hook_warning(warnings: &[LegacyV2HookWarning]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "⚠ Found legacy v2 hooks in {} project(s) — these will conflict with v3 user-scope hooks:",
        warnings.len()
    );
    for warning in warnings {
        let _ = writeln!(out, "  - {}", warning.project_settings_path.display());
        for command in &warning.stop_commands {
            let _ = writeln!(out, "      Stop hook: {command}");
        }
        for command in &warning.post_tool_use_commands {
            let _ = writeln!(out, "      PostToolUse hook: {command}");
        }
    }
    let _ = writeln!(out, "  Replace those `command` strings with:");
    let _ = writeln!(out, "    - Stop:        team_mode_service hook async-wake");
    let _ = writeln!(out, "    - PostToolUse: team_mode_service hook mid-turn");
    let _ = writeln!(
        out,
        "  Keep `asyncRewake: true` and `timeout: 7200` on the Stop entry."
    );
    out
}

fn global_mcp_json_is_empty_shell(
    value: &Value,
    path: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    let root = value
        .as_object()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    Ok(root.is_empty()
        || (root.len() == 1
            && root
                .get("mcpServers")
                .and_then(Value::as_object)
                .is_some_and(|servers| servers.is_empty())))
}

fn global_settings_json_is_empty_shell(
    value: &Value,
    path: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    let root = value
        .as_object()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    Ok(root.is_empty()
        || (root.len() == 1
            && root
                .get("hooks")
                .and_then(Value::as_object)
                .is_some_and(|hooks| hooks.is_empty())))
}

fn embedded_script_targets(target: &Path) -> Vec<(PathBuf, &'static str)> {
    vec![
        (target.join(MCP_HEADERS_HELPER), EMBEDDED_HEADERS_HELPER),
        (target.join(STOP_HOOK_SCRIPT), EMBEDDED_STOP_HOOK),
        (
            target.join(POST_TOOL_USE_HOOK_SCRIPT),
            EMBEDDED_POST_TOOL_USE_HOOK,
        ),
    ]
}

fn merged_mcp_json(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let mut value = if path.exists() {
        read_json_file(path)?
    } else {
        json!({
            "mcpServers": {}
        })
    };

    let root = value
        .as_object_mut()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    let servers = ensure_object_field(root, "mcpServers", path)?;
    if servers.contains_key("team-mode") {
        return Err(format!(
            "{} already contains mcpServers.team-mode; merge manually",
            path.display()
        )
        .into());
    }
    servers.insert("team-mode".into(), team_mode_mcp_server_json());
    Ok(value)
}

fn merged_claude_settings_json(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let mut value = if path.exists() {
        read_json_file(path)?
    } else {
        json!({})
    };

    if settings_has_lead_pending_hook(&value) {
        return Err(format!(
            "{} already contains Team Mode lead-pending hooks; merge manually",
            path.display()
        )
        .into());
    }

    let root = value
        .as_object_mut()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    let hooks = ensure_object_field(root, "hooks", path)?;
    append_hook_entry(hooks, "Stop", stop_hook_entry(), path)?;
    append_hook_entry(hooks, "PostToolUse", post_tool_use_hook_entry(), path)?;
    Ok(value)
}

fn read_json_file(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)
        .map_err(|err| format!("failed to parse JSON file '{}': {err}", path.display()))?)
}

fn write_json_pretty(path: &Path, value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(value)?))?;
    Ok(())
}

fn ensure_object_field<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
    path: &Path,
) -> Result<&'a mut Map<String, Value>, Box<dyn std::error::Error>> {
    if !root.contains_key(key) {
        root.insert(key.to_string(), json!({}));
    }
    root.get_mut(key)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("{}.{} must be a JSON object", path.display(), key).into())
}

fn append_hook_entry(
    hooks: &mut Map<String, Value>,
    event: &str,
    entry: Value,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if !hooks.contains_key(event) {
        hooks.insert(event.to_string(), json!([]));
    }
    let entries = hooks
        .get_mut(event)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("{}.hooks.{event} must be a JSON array", path.display()))?;
    entries.push(entry);
    Ok(())
}

fn settings_has_lead_pending_hook(value: &Value) -> bool {
    json_contains_command(value, "lead-pending-async-wake.js")
        || json_contains_command(value, "lead-pending-mid-turn.js")
}

fn remove_global_mcp_server(
    value: &mut Value,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    if let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.remove("team-mode");
    }
    Ok(())
}

fn remove_global_hooks(value: &mut Value, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    let Some(hooks_value) = root.get_mut("hooks") else {
        return Ok(());
    };
    let hooks = hooks_value
        .as_object_mut()
        .ok_or_else(|| format!("{}.hooks must be a JSON object", path.display()))?;
    remove_hook_entry_command(hooks, "Stop", "team_mode_service hook async-wake", path)?;
    remove_hook_entry_command(
        hooks,
        "PostToolUse",
        "team_mode_service hook mid-turn",
        path,
    )?;
    Ok(())
}

fn remove_hook_entry_command(
    hooks: &mut Map<String, Value>,
    event: &str,
    command: &str,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(entries_value) = hooks.get_mut(event) else {
        return Ok(());
    };
    let entries = entries_value
        .as_array_mut()
        .ok_or_else(|| format!("{}.hooks.{event} must be a JSON array", path.display()))?;
    entries.retain(|entry| !json_contains_command(entry, command));
    if entries.is_empty() {
        hooks.remove(event);
    }
    Ok(())
}

fn remove_empty_directory(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !path.is_dir() {
        return Ok(());
    }
    if fs::read_dir(path)?.next().is_none() {
        fs::remove_dir(path)?;
    }
    Ok(())
}

fn json_contains_command(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(s) => s.contains(needle),
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains_command(value, needle)),
        Value::Object(map) => map
            .values()
            .any(|value| json_contains_command(value, needle)),
        _ => false,
    }
}

fn team_mode_mcp_server_json() -> Value {
    json!({
        "type": "http",
        "url": format!("http://{DEFAULT_HOST}:{DEFAULT_PORT}/mcp"),
        "headersHelper": format!("node {MCP_HEADERS_HELPER}")
    })
}

fn global_team_mode_mcp_server_json() -> Value {
    json!({
        "command": "team_mode_service",
        "args": ["relay"],
        "env": {}
    })
}

fn stop_hook_entry() -> Value {
    json!({
        "hooks": [
            {
                "type": "command",
                "command": format!("node {STOP_HOOK_SCRIPT}"),
                "asyncRewake": true,
                "timeout": 7200
            }
        ]
    })
}

fn post_tool_use_hook_entry() -> Value {
    json!({
        "hooks": [
            {
                "type": "command",
                "command": format!("node {POST_TOOL_USE_HOOK_SCRIPT}")
            }
        ]
    })
}

fn global_stop_hook_entry() -> Value {
    json!({
        "hooks": [
            {
                "type": "command",
                "command": "team_mode_service hook async-wake",
                "asyncRewake": true,
                "timeout": 7200
            }
        ]
    })
}

fn global_post_tool_use_hook_entry() -> Value {
    json!({
        "hooks": [
            {
                "type": "command",
                "command": "team_mode_service hook mid-turn"
            }
        ]
    })
}

fn ensure_gitignore_entry(path: &Path, entry: &str) -> Result<(), Box<dyn std::error::Error>> {
    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    if existing.lines().any(|line| line.trim() == entry) {
        return Ok(());
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(entry);
    next.push('\n');
    fs::write(path, next)?;
    Ok(())
}

fn pre_spawn_web(base_dir: &Path) {
    match agent_teams::team_mode::mcp::tools::ensure_team_web_server_public(base_dir) {
        Ok(url) => tracing::info!(url = %url, "team_mode_web pre-spawned"),
        Err(err) => tracing::warn!(error = %err, "team_mode_web not started"),
    }
}

fn spawn_watchdogs(toolset: Arc<TeamModeToolset>) -> Result<(), Box<dyn std::error::Error>> {
    let lead_toolset = Arc::clone(&toolset);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(LEAD_WATCH_INTERVAL);
        let mut dead_strikes: HashMap<String, u32> = HashMap::new();
        loop {
            interval.tick().await;
            let archived =
                tokio::task::block_in_place(|| lead_toolset.lead_watchdog_tick(&mut dead_strikes));
            if archived > 0 {
                tracing::info!(archived, "lead-watchdog: auto-archived dead-owner team(s)");
            }
        }
    });
    std::thread::Builder::new()
        .name("worker-liveness".into())
        .spawn(move || run_worker_liveness_watchdog(toolset))?;
    Ok(())
}

fn run_worker_liveness_watchdog(toolset: Arc<TeamModeToolset>) {
    loop {
        std::thread::sleep(WORKER_LIVENESS_INTERVAL);
        let _ = toolset.worker_liveness_tick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn init_empty_project_writes_scripts_and_config() {
        let dir = tempdir().unwrap();

        init_project(dir.path()).unwrap();

        assert!(dir.path().join(MCP_HEADERS_HELPER).is_file());
        assert!(dir.path().join(STOP_HOOK_SCRIPT).is_file());
        assert!(dir.path().join(POST_TOOL_USE_HOOK_SCRIPT).is_file());
        let mcp = read_json_file(&dir.path().join(".mcp.json")).unwrap();
        assert_eq!(
            mcp["mcpServers"]["team-mode"]["headersHelper"],
            format!("node {MCP_HEADERS_HELPER}")
        );
        let settings = read_json_file(&dir.path().join(".claude/settings.json")).unwrap();
        assert!(json_contains_command(&settings, STOP_HOOK_SCRIPT));
        assert!(json_contains_command(&settings, POST_TOOL_USE_HOOK_SCRIPT));
        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.lines().any(|line| line == ".agent-teams/"));
    }

    #[test]
    fn init_merges_mcp_json_without_team_mode_server() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"other":{"type":"stdio","command":"x"}}}"#,
        )
        .unwrap();

        init_project(dir.path()).unwrap();

        let mcp = read_json_file(&dir.path().join(".mcp.json")).unwrap();
        assert_eq!(mcp["mcpServers"]["other"]["command"], "x");
        assert_eq!(
            mcp["mcpServers"]["team-mode"]["url"],
            format!("http://{DEFAULT_HOST}:{DEFAULT_PORT}/mcp")
        );
    }

    #[test]
    fn init_errors_when_mcp_json_already_has_team_mode_server() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"team-mode":{"type":"http","url":"old"}}}"#,
        )
        .unwrap();

        let err = init_project(dir.path()).unwrap_err().to_string();

        assert!(err.contains("mcpServers.team-mode"), "got: {err}");
        assert!(!dir.path().join(MCP_HEADERS_HELPER).exists());
    }

    #[test]
    fn init_merges_claude_settings_without_lead_pending_hook() {
        let dir = tempdir().unwrap();
        let settings_path = dir.path().join(".claude/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        fs::write(
            &settings_path,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo existing"}]}]}}"#,
        )
        .unwrap();

        init_project(dir.path()).unwrap();

        let settings = read_json_file(&settings_path).unwrap();
        let stop_entries = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop_entries.len(), 2);
        assert!(json_contains_command(&settings, STOP_HOOK_SCRIPT));
        assert!(json_contains_command(&settings, POST_TOOL_USE_HOOK_SCRIPT));
    }

    #[test]
    fn merged_claude_settings_errors_when_existing_lead_pending_hook_conflicts() {
        let dir = tempdir().unwrap();
        let settings_path = dir.path().join(".claude/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        fs::write(
            &settings_path,
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"node .agent-teams/scripts/hooks/lead-pending-mid-turn.js"}]}]}}"#,
        )
        .unwrap();

        let err = merged_claude_settings_json(&settings_path)
            .unwrap_err()
            .to_string();

        assert!(err.contains(".claude/settings.json"), "got: {err}");
        assert!(err.contains("Team Mode lead-pending hooks"), "got: {err}");
        assert!(err.contains("merge manually"), "got: {err}");
    }

    #[test]
    fn init_errors_when_claude_settings_already_has_lead_pending_hook() {
        let dir = tempdir().unwrap();
        let settings_path = dir.path().join(".claude/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        fs::write(
            &settings_path,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"node .agent-teams/scripts/hooks/lead-pending-async-wake.js"}]}]}}"#,
        )
        .unwrap();

        let err = merged_claude_settings_json(&settings_path)
            .unwrap_err()
            .to_string();

        assert!(err.contains(".claude/settings.json"), "got: {err}");
        assert!(err.contains("Team Mode lead-pending hooks"), "got: {err}");
        assert!(err.contains("merge manually"), "got: {err}");
    }

    #[test]
    fn install_global_empty_home_writes_expected_config() {
        let home = tempdir().unwrap();

        let warning = install_global_at(home.path()).unwrap();
        assert!(warning.is_none());

        let mcp = read_json_file(&home.path().join(".claude.json")).unwrap();
        assert_eq!(
            mcp["mcpServers"]["team-mode"]["command"],
            "team_mode_service"
        );
        assert_eq!(mcp["mcpServers"]["team-mode"]["args"], json!(["relay"]));

        let settings = read_json_file(&home.path().join(".claude/settings.json")).unwrap();
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "team_mode_service hook async-wake"
        );
        assert_eq!(
            settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
            "team_mode_service hook mid-turn"
        );
    }

    #[test]
    fn install_global_merges_existing_user_config_and_uninstall_restores_it() {
        let home = tempdir().unwrap();
        let mcp_path = home.path().join(".claude.json");
        let settings_path = home.path().join(".claude/settings.json");
        fs::write(
            &mcp_path,
            r#"{"mcpServers":{"other":{"command":"x"}},"extra":"keep-me"}"#,
        )
        .unwrap();
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        fs::write(
            &settings_path,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo existing"}]}]},"extra":"keep-me"}"#,
        )
        .unwrap();
        let original_mcp = read_json_file(&mcp_path).unwrap();
        let original_settings = read_json_file(&settings_path).unwrap();

        let warning = install_global_at(home.path()).unwrap();
        assert!(warning.is_none());
        let installed_mcp = read_json_file(&mcp_path).unwrap();
        assert_eq!(installed_mcp["mcpServers"]["other"]["command"], "x");
        assert_eq!(installed_mcp["extra"], json!("keep-me"));
        assert_eq!(
            installed_mcp["mcpServers"]["team-mode"]["command"],
            "team_mode_service"
        );
        assert_eq!(
            installed_mcp["mcpServers"]["team-mode"]["args"],
            json!(["relay"])
        );
        let installed_settings = read_json_file(&settings_path).unwrap();
        assert!(json_contains_command(&installed_settings, "echo existing"));
        assert!(json_contains_command(
            &installed_settings,
            "team_mode_service hook async-wake"
        ));
        assert!(json_contains_command(
            &installed_settings,
            "team_mode_service hook mid-turn"
        ));
        assert_eq!(installed_settings["extra"], json!("keep-me"));

        uninstall_global_at(home.path()).unwrap();
        let restored_mcp = read_json_file(&mcp_path).unwrap();
        let restored_settings = read_json_file(&settings_path).unwrap();
        assert_eq!(restored_mcp, original_mcp);
        assert_eq!(restored_settings, original_settings);
    }

    #[test]
    fn install_global_clean_home_round_trip_leaves_no_shells() {
        let home = tempdir().unwrap();

        let warning = install_global_at(home.path()).unwrap();
        assert!(warning.is_none());
        uninstall_global_at(home.path()).unwrap();

        assert!(!home.path().join(".claude.json").exists());
        assert!(!home.path().join(".claude/settings.json").exists());
        assert!(!home.path().join(".claude").exists());
    }

    #[test]
    fn install_global_errors_when_user_mcp_team_mode_entry_differs() {
        // If user's ~/.claude.json has a `team-mode` entry with a different value
        // than what install-global would write, refuse to clobber.
        let home = tempdir().unwrap();
        let mcp_path = home.path().join(".claude.json");
        fs::write(
            &mcp_path,
            r#"{"mcpServers":{"team-mode":{"command":"some_other_binary","args":["relay"]}}}"#,
        )
        .unwrap();
        let settings_path = home.path().join(".claude/settings.json");
        fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        fs::write(&settings_path, "{}").unwrap();

        let err = install_global_at(home.path()).unwrap_err().to_string();

        assert!(err.contains(".claude.json"), "got: {err}");
        assert!(err.contains("mcpServers.team-mode"), "got: {err}");
        assert!(err.contains("different config"), "got: {err}");
    }

    #[test]
    fn install_global_is_idempotent_when_already_installed() {
        // Re-running install-global on an already-installed home succeeds as a no-op
        // and leaves the files byte-identical (no spurious rewrites).
        let home = tempdir().unwrap();

        install_global_at(home.path()).unwrap();
        let mcp_after_first = fs::read_to_string(home.path().join(".claude.json")).unwrap();
        let settings_after_first =
            fs::read_to_string(home.path().join(".claude/settings.json")).unwrap();

        // Second invocation must not error and must not change file contents.
        let warning = install_global_at(home.path()).unwrap();
        assert!(warning.is_none());
        let mcp_after_second = fs::read_to_string(home.path().join(".claude.json")).unwrap();
        let settings_after_second =
            fs::read_to_string(home.path().join(".claude/settings.json")).unwrap();
        assert_eq!(mcp_after_first, mcp_after_second);
        assert_eq!(settings_after_first, settings_after_second);
    }

    #[test]
    fn install_global_repairs_partial_install_when_hook_was_removed() {
        // Real-world scenario: user ran install-global once (mcp + hooks both written).
        // Later something cleared `~/.claude/settings.json` hooks. Re-running
        // install-global must add the missing hook without erroring on the
        // already-present mcp entry.
        let home = tempdir().unwrap();
        install_global_at(home.path()).unwrap();
        // Simulate hook loss while mcp entry remains.
        fs::write(
            home.path().join(".claude/settings.json"),
            r#"{"effortLevel":"high"}"#,
        )
        .unwrap();

        let warning = install_global_at(home.path()).unwrap();
        assert!(warning.is_none());

        let settings = read_json_file(&home.path().join(".claude/settings.json")).unwrap();
        // Pre-existing user fields must survive.
        assert_eq!(settings["effortLevel"], "high");
        // Hooks must be re-added.
        assert!(json_contains_command(
            &settings,
            "team_mode_service hook async-wake"
        ));
        assert!(json_contains_command(
            &settings,
            "team_mode_service hook mid-turn"
        ));
    }

    #[test]
    fn install_global_warns_about_legacy_v2_project_hooks_without_touching_other_projects() {
        let home = tempdir().unwrap();
        let legacy_project = home.path().join("legacy-project");
        let empty_project = home.path().join("empty-project");
        let v3_project = home.path().join("v3-project");
        let mut projects = Map::new();
        projects.insert(legacy_project.display().to_string(), json!({}));
        projects.insert(empty_project.display().to_string(), json!({}));
        projects.insert(v3_project.display().to_string(), json!({}));

        fs::create_dir_all(legacy_project.join(".claude")).unwrap();
        fs::write(
            legacy_project.join(".claude/settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"node scripts/hooks/lead-pending-async-wake.js"}]}],"PostToolUse":[{"hooks":[{"type":"command","command":"node scripts/hooks/lead-pending-mid-turn.js"}]}]}}"#,
        )
        .unwrap();
        fs::create_dir_all(empty_project.join(".claude")).unwrap();
        fs::write(
            empty_project.join(".claude/settings.json"),
            r#"{"theme":"dark"}"#,
        )
        .unwrap();
        fs::create_dir_all(v3_project.join(".claude")).unwrap();
        fs::write(
            v3_project.join(".claude/settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"team_mode_service hook async-wake","asyncRewake":true,"timeout":7200}]}],"PostToolUse":[{"hooks":[{"type":"command","command":"team_mode_service hook mid-turn"}]}]}}"#,
        )
        .unwrap();
        fs::write(
            home.path().join(".claude.json"),
            serde_json::to_string_pretty(&json!({
                "projects": projects,
                "mcpServers": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let warnings = collect_legacy_v2_hook_warnings(home.path()).unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            warnings[0].project_settings_path,
            legacy_project.join(".claude/settings.json")
        );
        assert_eq!(
            warnings[0].stop_commands,
            vec!["node scripts/hooks/lead-pending-async-wake.js".to_string()]
        );
        assert_eq!(
            warnings[0].post_tool_use_commands,
            vec!["node scripts/hooks/lead-pending-mid-turn.js".to_string()]
        );

        let warning = legacy_v2_hook_warning_section(home.path())
            .unwrap()
            .unwrap();
        let warning = warning.replace('\\', "/");
        assert!(warning.contains("⚠ Found legacy v2 hooks in 1 project(s)"));
        assert!(warning.contains("legacy-project/.claude/settings.json"));
        assert!(warning.contains("Stop hook: node scripts/hooks/lead-pending-async-wake.js"));
        assert!(warning.contains("PostToolUse hook: node scripts/hooks/lead-pending-mid-turn.js"));
        assert!(warning.contains("Replace those `command` strings with:"));
        assert!(warning.contains("team_mode_service hook async-wake"));
        assert!(warning.contains("team_mode_service hook mid-turn"));
        assert!(!warning.contains("empty-project/.claude/settings.json"));
        assert!(!warning.contains("v3-project/.claude/settings.json"));
    }

    #[test]
    fn install_global_warns_only_when_claude_projects_are_present() {
        let home = tempdir().unwrap();

        let warning = install_global_at(home.path()).unwrap();

        assert!(warning.is_none());
        let mcp = read_json_file(&home.path().join(".claude.json")).unwrap();
        assert!(mcp.get("projects").is_none());
    }

    #[test]
    fn install_global_writes_skill_to_user_skills_dir() {
        let home = tempdir().unwrap();

        install_global_at(home.path()).unwrap();

        let skill_root = home
            .path()
            .join(".claude")
            .join("skills")
            .join("agent-teams-mcp-setup");
        let skill_md = skill_root.join("SKILL.md");
        assert!(skill_md.exists(), "SKILL.md not written: {skill_md:?}");

        let on_disk = fs::read_to_string(&skill_md).unwrap();
        assert!(
            on_disk.contains("name: agent-teams-mcp-setup"),
            "SKILL.md frontmatter missing"
        );
        assert!(
            on_disk.contains("硬前置依赖"),
            "SKILL.md missing hard-dependency block — embedded skill bytes appear stale"
        );

        // references/ subtree must also land
        let onboarding = skill_root.join("references").join("onboarding.md");
        assert!(
            onboarding.exists(),
            "references/onboarding.md not written: {onboarding:?}"
        );
    }

    #[test]
    fn install_global_skill_install_is_idempotent() {
        let home = tempdir().unwrap();
        install_global_at(home.path()).unwrap();
        let skill_md = home
            .path()
            .join(".claude/skills/agent-teams-mcp-setup/SKILL.md");
        let bytes_first = fs::read(&skill_md).unwrap();

        install_global_at(home.path()).unwrap();
        let bytes_second = fs::read(&skill_md).unwrap();

        assert_eq!(
            bytes_first, bytes_second,
            "skill file rewritten on second install"
        );
    }

    #[test]
    fn install_global_skill_errors_when_user_modified_file_conflicts() {
        let home = tempdir().unwrap();
        let skill_md = home
            .path()
            .join(".claude/skills/agent-teams-mcp-setup/SKILL.md");
        fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        fs::write(&skill_md, "user-modified skill body, do not clobber").unwrap();

        let err = install_global_at(home.path()).unwrap_err().to_string();

        assert!(
            err.contains("agent-teams-mcp-setup"),
            "error should name the skill: {err}"
        );
        assert!(
            err.contains("Refusing to overwrite"),
            "error should refuse overwrite: {err}"
        );
        let after = fs::read_to_string(&skill_md).unwrap();
        assert_eq!(after, "user-modified skill body, do not clobber");
    }

    #[test]
    fn uninstall_global_removes_skill_when_unmodified() {
        let home = tempdir().unwrap();
        install_global_at(home.path()).unwrap();
        let skill_root = home
            .path()
            .join(".claude/skills/agent-teams-mcp-setup");
        assert!(skill_root.exists());

        uninstall_global_at(home.path()).unwrap();

        assert!(
            !skill_root.exists(),
            "skill dir should be gone after uninstall when files are unchanged"
        );
        // .claude/skills/ should also be pruned (no other skills installed in this temp home)
        assert!(
            !home.path().join(".claude/skills").exists(),
            "empty .claude/skills/ should be pruned"
        );
    }

    #[test]
    fn uninstall_global_preserves_user_modified_skill_files() {
        let home = tempdir().unwrap();
        install_global_at(home.path()).unwrap();
        let skill_md = home
            .path()
            .join(".claude/skills/agent-teams-mcp-setup/SKILL.md");
        fs::write(&skill_md, "i edited this after install").unwrap();

        uninstall_global_at(home.path()).unwrap();

        // Modified SKILL.md must still be on disk; its dir must therefore survive.
        assert_eq!(
            fs::read_to_string(&skill_md).unwrap(),
            "i edited this after install"
        );
        // Sibling untouched files (references/onboarding.md etc.) should be cleaned
        // because they still match the embedded source.
        assert!(
            !home
                .path()
                .join(".claude/skills/agent-teams-mcp-setup/references/onboarding.md")
                .exists(),
            "unmodified sibling files should be removed even when SKILL.md kept"
        );
    }
}
