use std::env;
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
use serde::Serialize;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8786;
const LEAD_WATCH_INTERVAL: Duration = Duration::from_secs(5);
const LEAD_WATCH_GRACE_CHECKS: u32 = 3;
const WORKER_LIVENESS_INTERVAL: Duration = Duration::from_secs(5);

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
    init_tracing();
    let args = parse_args()?;
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

fn parse_args() -> Result<ServiceArgs, Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut data_dir = None;
    let mut project_root = None;
    let mut host = DEFAULT_HOST.to_string();
    let mut port = DEFAULT_PORT;
    let mut token_file = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = Some(PathBuf::from(next_arg(&mut args, &arg)?)),
            "--project-root" => project_root = Some(PathBuf::from(next_arg(&mut args, &arg)?)),
            "--host" => host = next_arg(&mut args, &arg)?,
            "--port" => port = next_arg(&mut args, &arg)?.parse()?,
            "--token-file" => token_file = Some(PathBuf::from(next_arg(&mut args, &arg)?)),
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let project_root = project_root.unwrap_or(env::current_dir()?);
    let data_dir = data_dir.unwrap_or_else(|| data_dir::resolve_default_base_dir(&project_root));
    Ok(ServiceArgs {
        data_dir,
        project_root,
        host,
        port,
        token_file,
    })
}

fn next_arg(
    args: &mut impl Iterator<Item = String>,
    name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    args.next()
        .ok_or_else(|| format!("{name} requires a value").into())
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
