use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::team_mode::data_dir;
use crate::team_mode::mcp::tools::TeamModeToolset;
use crate::team_mode::runtime_workers::RuntimeWorkerStore;
use crate::team_mode::storage::TeamStore;
use crate::team_mode_daemon::ipc::{
    DaemonInfo, DaemonRequest, DaemonResponse, read_frame, write_frame, write_info,
};

/// How often the lead-watchdog re-checks for live owner CC processes.
const LEAD_WATCH_INTERVAL: Duration = Duration::from_secs(5);
/// How many consecutive "no lead alive" checks we need before shutting down.
/// With a 5s interval this means 3×5=15s grace — enough to ride out a single
/// `/mcp reconnect` round-trip without tearing the team down, but quick enough
/// that a genuinely-closed CC session reclaims workers promptly.
const LEAD_WATCH_GRACE_CHECKS: u32 = 3;

pub fn serve_daemon(base_dir: PathBuf, project_root: PathBuf, token: String) -> Result<()> {
    data_dir::ensure_scaffold(&base_dir)?;
    std::fs::create_dir_all(crate::team_mode_daemon::runtime_dir(&base_dir))?;

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    let info = DaemonInfo {
        pid: std::process::id(),
        host: "127.0.0.1".into(),
        port: addr.port(),
        token: token.clone(),
        base_dir: base_dir.clone(),
        project_root: project_root.clone(),
    };
    write_info(&base_dir, &info)?;
    let marked_dead =
        RuntimeWorkerStore::new(base_dir.clone()).mark_daemon_restart_dead(info.pid)?;

    tracing::info!(
        pid = info.pid,
        port = info.port,
        marked_dead,
        base_dir = %base_dir.display(),
        project_root = %project_root.display(),
        "Team Mode daemon listening"
    );

    let toolset = Arc::new(TeamModeToolset::new_with_project_root(
        base_dir.clone(),
        Some(project_root),
    ));

    // Pre-spawn the read-only web UI so browser tabs still load after a
    // daemon restart. Without this, the web server only starts inside the
    // first `team_create` call, leaving existing bookmarks / open tabs
    // returning ERR_CONNECTION_REFUSED until the next team is created.
    // Best-effort: a startup failure (e.g. TEAM_MODE_WEB_AUTO_OPEN=0)
    // just logs and continues; the daemon is still usable via MCP.
    match crate::team_mode::mcp::tools::ensure_team_web_server_public(&base_dir) {
        Ok(url) => tracing::info!(url = %url, "team_mode_web pre-spawned at daemon startup"),
        Err(err) => tracing::warn!(error = %err, "team_mode_web not started at daemon startup"),
    }

    // Lead-watchdog: when every team's owner_cc_pid is dead (i.e. the CC
    // that launched this daemon has closed), tear everything down. Workers
    // and their tasks have no reason to outlive the lead — if the user
    // closed CC, they closed the team. We can't do this synchronously on
    // MCP stdin EOF because ESC also closes MCP briefly and we want
    // /mcp reconnect to keep working; hence the grace counter.
    {
        let team_store = TeamStore::new(base_dir.clone());
        let toolset = Arc::clone(&toolset);
        std::thread::Builder::new()
            .name("lead-watchdog".into())
            .spawn(move || run_lead_watchdog(team_store, toolset))
            .map_err(|err| Error::Other(format!("failed to spawn lead-watchdog thread: {err}")))?;
    }

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            Err(err) => {
                tracing::warn!(error = %err, "daemon accept failed");
                continue;
            }
        };
        let token = token.clone();
        let toolset = Arc::clone(&toolset);
        std::thread::Builder::new()
            .name("team-mode-daemon-client".into())
            .spawn(move || {
                if let Err(err) = handle_client(stream, &token, &toolset) {
                    tracing::warn!(error = %err, "daemon client request failed");
                }
            })
            .map_err(|err| Error::Other(format!("failed to spawn daemon client thread: {err}")))?;
    }

    Ok(())
}

fn handle_client(mut stream: TcpStream, token: &str, toolset: &TeamModeToolset) -> Result<()> {
    let request: DaemonRequest = read_frame(&mut stream)?;
    let response = match handle_request(request, token, toolset) {
        Ok(response) => response,
        Err((id, error)) => DaemonResponse {
            id,
            result: None,
            error: Some(error),
        },
    };
    write_frame(&mut stream, &response)
}

fn handle_request(
    request: DaemonRequest,
    token: &str,
    toolset: &TeamModeToolset,
) -> std::result::Result<DaemonResponse, (u64, String)> {
    if request.token != token {
        return Err((request.id, "daemon token mismatch".into()));
    }

    let result = match request.method.as_str() {
        "daemon/ping" => json!({
            "ok": true,
            "pid": std::process::id(),
        }),
        "tools/list" => serde_json::to_value(toolset.list_tools())
            .map_err(|err| (request.id, err.to_string()))?,
        "tools/call" => {
            let params = request.params.unwrap_or(Value::Null);
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| (request.id, "tools/call requires name".into()))?;
            let mut arguments = params.get("arguments").cloned();
            inject_call_context(&mut arguments, &params);
            let execution = toolset
                .call_tool(name, arguments)
                .map_err(|err| (request.id, err.to_string()))?;
            serde_json::to_value(execution).map_err(|err| (request.id, err.to_string()))?
        }
        other => {
            return Err((request.id, format!("unknown daemon method '{other}'")));
        }
    };

    Ok(DaemonResponse {
        id: request.id,
        result: Some(result),
        error: None,
    })
}

/// Background watcher: periodically check every team's `owner_cc_pid`. If
/// all of them point at dead processes for `LEAD_WATCH_GRACE_CHECKS`
/// consecutive rounds, shut down the daemon so workers don't outlive the
/// lead CC that created them.
///
/// Behavior:
/// - No teams yet → idle (not "all dead"); daemon keeps running waiting for
///   the first `team_create` call.
/// - At least one team has `owner_cc_pid` unset → treat as alive (can't say
///   lead is dead if we never knew who it was).
/// - Every team has `owner_cc_pid` set AND sysinfo says that PID is gone →
///   one strike. After 3 strikes in a row, exit.
/// - Any team regains a live lead (rare, e.g. PID reuse within a check
///   window) → reset the strike counter.
fn run_lead_watchdog(team_store: TeamStore, toolset: Arc<TeamModeToolset>) {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let mut sys = System::new();
    let mut consecutive_dead: u32 = 0;

    loop {
        std::thread::sleep(LEAD_WATCH_INTERVAL);

        let teams = match team_store.list() {
            Ok(teams) => teams,
            Err(err) => {
                tracing::warn!(error = %err, "lead-watchdog: team_store.list failed, skipping");
                continue;
            }
        };

        // Empty team list now COUNTS as a "no live lead" round. Previously
        // `team_delete` removing the last team froze the watchdog forever
        // (counter reset every tick), so the daemon stayed up indefinitely
        // holding a TCP port + a few MB of RAM. Treating empty as dead lets
        // the daemon self-clean after the same 15s grace as the
        // "all-owners-dead" path. A subsequent team_create still spawns a
        // fresh daemon in ~1s.
        if teams.is_empty() {
            consecutive_dead += 1;
            tracing::debug!(
                consecutive_dead,
                grace = LEAD_WATCH_GRACE_CHECKS,
                "lead-watchdog: no teams, counting toward grace"
            );
            if consecutive_dead >= LEAD_WATCH_GRACE_CHECKS {
                tracing::info!(
                    "lead-watchdog: no teams left for grace period, shutting down daemon"
                );
                drop(toolset);
                std::process::exit(0);
            }
            continue;
        }

        sys.refresh_processes(ProcessesToUpdate::All, true);

        let mut any_lead_alive = false;
        let mut any_unbound = false;
        for team in &teams {
            match team.owner_cc_pid {
                Some(pid) => {
                    if sys.process(Pid::from_u32(pid)).is_some() {
                        any_lead_alive = true;
                        break;
                    }
                }
                None => {
                    // Team has no recorded owner — older/manual entries.
                    // Treat conservatively: don't kill the daemon on account
                    // of a team we can't verify.
                    any_unbound = true;
                }
            }
        }

        if any_lead_alive || any_unbound {
            consecutive_dead = 0;
            continue;
        }

        consecutive_dead += 1;
        tracing::info!(
            teams = teams.len(),
            consecutive_dead,
            grace = LEAD_WATCH_GRACE_CHECKS,
            "lead-watchdog: all owner CCs dead"
        );
        if consecutive_dead >= LEAD_WATCH_GRACE_CHECKS {
            tracing::warn!(
                "lead-watchdog: no live lead CC across all teams for grace period, shutting down daemon"
            );
            // Best-effort worker cleanup. TeamModeToolset::shutdown_all is
            // not exposed; relying on kill_on_drop when the process exits.
            drop(toolset);
            // Exit the daemon process. Child workers have kill_on_drop(true),
            // so the OS reaps them once we go away.
            std::process::exit(0);
        }
    }
}

fn inject_call_context(arguments: &mut Option<Value>, params: &Value) {
    let context = params.get("context");

    // Materialize an empty object if the caller didn't pass one but we
    // have context fields to inject — otherwise tools that depend on
    // _caller_member would silently miss it just because the caller
    // omitted other arguments.
    if context.is_some() && arguments.is_none() {
        *arguments = Some(json!({}));
    }
    let Some(Value::Object(args)) = arguments else {
        return;
    };

    if let Some(owner_cc_pid) = context
        .and_then(|c| c.get("owner_cc_pid"))
        .and_then(Value::as_u64)
    {
        args.insert("_owner_cc_pid".into(), json!(owner_cc_pid));
    }
    // Bug 29: identity passthrough. We always inject `_caller_member`
    // (defaults to "lead" when absent — preserves historical behavior
    // for any client that doesn't set the env). `_caller_team` is only
    // injected when present so tools can distinguish "this caller has
    // no team affiliation" from "this caller is in team X".
    let caller_member = context
        .and_then(|c| c.get("caller_member"))
        .and_then(Value::as_str)
        .unwrap_or("lead")
        .to_string();
    args.insert("_caller_member".into(), json!(caller_member));
    if let Some(caller_team) = context
        .and_then(|c| c.get("caller_team"))
        .and_then(Value::as_str)
    {
        args.insert("_caller_team".into(), json!(caller_team));
    }
}
