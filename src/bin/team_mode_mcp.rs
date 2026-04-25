use std::env;
use std::path::PathBuf;

use agent_teams::team_mode::data_dir;
use agent_teams::team_mode_daemon::DaemonToolClient;
use agent_teams::{TeamModeMcpRuntime, TeamModeToolset};
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut explicit_data_dir: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => {
                let Some(value) = args.next() else {
                    return Err("--data-dir requires a value".into());
                };
                explicit_data_dir = Some(PathBuf::from(value));
            }
            other => {
                return Err(format!("unknown argument: {other}").into());
            }
        }
    }

    let data_dir = match explicit_data_dir {
        Some(p) => p,
        None => data_dir::resolve_default_base_dir(&env::current_dir()?),
    };
    data_dir::ensure_scaffold(&data_dir)?;

    // Enable persistent file log (in addition to stderr) unless the caller
    // explicitly disabled it by setting TEAM_MODE_LOG_FILE="". Critical for
    // diagnosing failures when the MCP is spawned by a host (Claude Code)
    // that silently captures stderr.
    if env::var_os("TEAM_MODE_LOG_FILE").is_none() {
        unsafe {
            env::set_var("TEAM_MODE_LOG_FILE", data_dir.join("mcp.log"));
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| EnvFilter::new("info,agent_teams=debug")),
        )
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_thread_ids(false)
        .init();

    ensure_git_bash_path();

    // Absorb console control events (Ctrl+C, Ctrl+Break, close) on Windows.
    //
    // CC's ESC on Windows broadcasts CTRL_C_EVENT via GenerateConsoleCtrlEvent
    // to the entire console process group. Without a custom handler, the
    // default Windows behavior terminates the process, so CC marks the MCP
    // disconnected (stdio MCP servers don't auto-reconnect).
    //
    // Implementation note: we went back and forth on two approaches here —
    //   (a) tokio::signal::ctrl_c() in a background task (USER-OBSERVED
    //       behavior: MCP usually survives ESC, occasionally dies)
    //   (b) Raw Win32 FFI SetConsoleCtrlHandler returning TRUE (USER-OBSERVED
    //       behavior: MCP dies more reliably on ESC, plus an unexplained
    //       regression where MCP also died mid-worker-interaction)
    // The user's empirical feedback favored (a), so we reverted. Tokio's
    // handler should also return TRUE on CTRL_C, but its async plumbing
    // somehow interacts better with whatever CC actually does on ESC in
    // practice. We don't have a complete mechanistic explanation yet —
    // the stdin-EOF diagnostic logging in runtime.rs will help narrow it
    // down next time MCP dies unexpectedly.
    //
    // This thread intentionally runs an infinite recv loop. We don't care
    // what happens on signal — the SIDE EFFECT of tokio installing its
    // SetConsoleCtrlHandler is what protects us. The body of the loop is
    // just to keep the future polled so tokio's handler stays registered.
    std::thread::Builder::new()
        .name("mcp-ctrl-c-absorber".into())
        .spawn(|| {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async {
                loop {
                    if tokio::signal::ctrl_c().await.is_err() {
                        break;
                    }
                    tracing::info!("MCP: Ctrl+C absorbed (likely CC ESC broadcast)");
                }
            });
        })
        .ok();

    tracing::info!("Team Mode MCP server starting");
    tracing::info!(data_dir = %data_dir.display(), "using data directory");

    let project_root = env::current_dir()?;
    let mut runtime = if daemon_relay_disabled() {
        tracing::warn!("TEAM_MODE_DAEMON disabled; running workers in MCP process");
        TeamModeMcpRuntime::with_tool_executor(
            data_dir.clone(),
            Box::new(TeamModeToolset::new(data_dir)),
        )
    } else {
        let client = DaemonToolClient::new(data_dir.clone(), project_root);
        TeamModeMcpRuntime::with_tool_executor(data_dir, Box::new(client))
    };
    runtime.run_stdio()?;
    Ok(())
}

fn daemon_relay_disabled() -> bool {
    matches!(
        env::var("TEAM_MODE_DAEMON").as_deref(),
        Ok("0" | "false" | "FALSE" | "off" | "OFF")
    )
}

#[cfg(target_os = "windows")]
fn ensure_git_bash_path() {
    use std::path::Path;

    if std::env::var("CLAUDE_CODE_GIT_BASH_PATH").is_ok() {
        return;
    }

    let candidates = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        r"D:\Git\bin\bash.exe",
    ];

    for candidate in candidates.iter() {
        if Path::new(candidate).exists() {
            // SAFETY: called once during startup, before any threads are spawned.
            unsafe {
                std::env::set_var("CLAUDE_CODE_GIT_BASH_PATH", candidate);
            }
            tracing::info!("CLAUDE_CODE_GIT_BASH_PATH auto-detected: {}", candidate);
            return;
        }
    }

    if let Ok(path) = which::which("bash.exe").or_else(|_| which::which("bash")) {
        let path_str = path.to_string_lossy().to_string();
        // SAFETY: called once during startup, before any threads are spawned.
        unsafe {
            std::env::set_var("CLAUDE_CODE_GIT_BASH_PATH", &path_str);
        }
        tracing::info!(
            "CLAUDE_CODE_GIT_BASH_PATH auto-detected via PATH: {}",
            path_str
        );
        return;
    }

    tracing::warn!(
        "CLAUDE_CODE_GIT_BASH_PATH not found — claude-code workers on Windows may fail to spawn"
    );
}

#[cfg(not(target_os = "windows"))]
fn ensure_git_bash_path() {}
