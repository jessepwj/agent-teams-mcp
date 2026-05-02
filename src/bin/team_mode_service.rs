use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_teams::team_mode::data_dir;
use agent_teams::team_mode::mcp::http_transport::{HttpMcpState, router as http_mcp_router};
use agent_teams::team_mode::mcp::{TeamModeMcpRuntime, TeamModeToolset};
use agent_teams::team_mode::runtime_workers::RuntimeWorkerStore;
use agent_teams::team_mode::storage::TeamStore;
use axum::Router;
use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8786;
const LEAD_WATCH_INTERVAL: Duration = Duration::from_secs(5);
const LEAD_WATCH_GRACE_CHECKS: u32 = 3;
const WORKER_LIVENESS_INTERVAL: Duration = Duration::from_secs(5);
const MCP_HEADERS_HELPER: &str = ".agent-teams/scripts/mcp-http-headers.js";
const STOP_HOOK_SCRIPT: &str = ".agent-teams/scripts/hooks/lead-pending-async-wake.js";
const POST_TOOL_USE_HOOK_SCRIPT: &str = ".agent-teams/scripts/hooks/lead-pending-mid-turn.js";
const EMBEDDED_HEADERS_HELPER: &str = include_str!("../../scripts/mcp-http-headers.js");
const EMBEDDED_STOP_HOOK: &str = include_str!("../../scripts/hooks/lead-pending-async-wake.js");
const EMBEDDED_POST_TOOL_USE_HOOK: &str =
    include_str!("../../scripts/hooks/lead-pending-mid-turn.js");

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
    /// Scaffold Team Mode config into a project directory.
    Init {
        /// Target project directory. Defaults to the current directory.
        target_project_dir: Option<PathBuf>,
    },
}

#[derive(Debug)]
struct ServiceArgs {
    data_dir: PathBuf,
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    if let Some(ServiceCommand::Init { target_project_dir }) = cli.command {
        let target = target_project_dir.unwrap_or(env::current_dir()?);
        init_project(&target)?;
        return Ok(());
    }

    init_tracing();
    let args = service_args_from_cli(cli)?;
    run_service(args).await
}

async fn run_service(args: ServiceArgs) -> Result<(), Box<dyn std::error::Error>> {
    if args.host != DEFAULT_HOST {
        return Err("team_mode_service only binds 127.0.0.1".into());
    }
    data_dir::ensure_scaffold(&args.data_dir)?;
    let runtime_dir = args.data_dir.join("runtime");
    std::fs::create_dir_all(&runtime_dir)?;

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
    spawn_watchdogs(args.data_dir.clone(), Arc::clone(&toolset))?;

    let runtime = TeamModeMcpRuntime::with_tool_executor(
        args.data_dir.clone(),
        Box::new(Arc::clone(&toolset)),
    );
    let app = Router::new().merge(http_mcp_router(HttpMcpState::new(
        runtime,
        token,
        args.data_dir.clone(),
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
    let data_dir = cli
        .data_dir
        .unwrap_or_else(|| data_dir::resolve_default_base_dir(&project_root));
    Ok(ServiceArgs {
        data_dir,
        project_root,
        host: cli.host,
        port: cli.port,
        token_file: cli.token_file,
    })
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

fn spawn_watchdogs(
    base_dir: PathBuf,
    toolset: Arc<TeamModeToolset>,
) -> Result<(), Box<dyn std::error::Error>> {
    let team_store = TeamStore::new(base_dir);
    let lead_toolset = Arc::clone(&toolset);
    std::thread::Builder::new()
        .name("lead-watchdog".into())
        .spawn(move || run_lead_watchdog(team_store, lead_toolset))?;
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

fn run_lead_watchdog(team_store: TeamStore, _toolset: Arc<TeamModeToolset>) {
    // HTTP Team Mode service is a durable process: it must outlive any
    // particular Claude Code instance (CC restarts, switches projects, or
    // crashes shouldn't bring the service down). The old daemon assumed the
    // owning CC was a strict parent and exited on its death; that contract
    // does not apply here — CC connects via HTTP, and its PID can change
    // arbitrarily across reconnects. Watchdog is downgraded to pure
    // observability: log "lead apparently gone" but never exit. Service
    // lifecycle is now governed only by `team-mode-service.ps1 stop` /
    // explicit shutdown.
    //
    // Keeping the loop (instead of removing the watchdog entirely) preserves
    // a periodic visibility signal in `team-mode-service.log` and leaves a
    // single place to rewire to e.g. inactivity-based shutdown later.
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut sys = System::new();
    let mut consecutive_dead = 0;
    loop {
        std::thread::sleep(LEAD_WATCH_INTERVAL);
        let teams = match team_store.list() {
            Ok(teams) => teams,
            Err(err) => {
                tracing::warn!(error = %err, "lead-watchdog list failed");
                continue;
            }
        };
        sys.refresh_processes(ProcessesToUpdate::All, true);
        let any_live = teams.iter().any(|team| {
            team.owner_cc_pid
                .map(|pid| sys.process(Pid::from_u32(pid)).is_some())
                .unwrap_or(true)
        });
        consecutive_dead = if teams.is_empty() || !any_live {
            consecutive_dead + 1
        } else {
            0
        };
        if consecutive_dead == LEAD_WATCH_GRACE_CHECKS {
            // Log once at the threshold; do NOT exit. Reset is implicit
            // (next live tick zeros the counter).
            tracing::info!(
                event = "lead_watchdog.observation",
                teams_total = teams.len(),
                "lead-watchdog: no live owner_cc_pid across all teams; service stays up (HTTP service is durable)"
            );
        }
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
}
