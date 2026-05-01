use std::env;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use agent_teams::team_mode::data_dir;
use agent_teams::team_mode::mcp::StdioExitReason;
use agent_teams::team_mode_daemon::{DaemonToolClient, prune_stale_endpoint};
use agent_teams::util::{current_cc_pid, resolve_cc_pid_from};
use agent_teams::{TeamModeMcpRuntime, TeamModeToolset};
use tracing_subscriber::EnvFilter;

/// How often the parent-CC liveness watchdog re-checks. 5s matches the
/// daemon's LEAD_WATCH_INTERVAL so an MCP and its daemon notice the lead
/// is gone on roughly the same cycle. MCP exits immediately on death
/// (no grace counter) — the daemon already absorbs `/mcp reconnect`
/// blips with its own grace window, and MCP stdio is meant to be
/// auto-respawned by CC anyway.
const PARENT_LIVENESS_INTERVAL: Duration = Duration::from_secs(5);

/// Process stem we recognise as a peer MCP server (case-insensitive,
/// `.exe` stripped). Kept here rather than in `util` because only this
/// bin's startup sweep needs it.
const MCP_BIN_STEM: &str = "team_mode_mcp";

/// Process stems we accept as a real Claude Code parent (case-insensitive,
/// `.exe` stripped). Used by the watchdog to validate that the recorded
/// owner_cc_pid actually still belongs to a CC process rather than a
/// recycled PID held by some unrelated binary. Without this check,
/// Windows PID recycling on machines with hundreds of node.exe processes
/// will trick `sys.process(p).is_some()` into reporting false-alive — see
/// ADR-018.
const CC_BIN_STEMS: &[&str] = &["node", "claude"];

/// Process stems we treat as legitimate ancestors when deciding whether
/// a peer team_mode_mcp.exe is still owned by something living. The
/// startup sweep walks each peer's parent chain looking for any of
/// these; if found, the peer is spared. Includes the daemon and codex
/// stems so that worker MCP relays (whose ancestor chain is
/// `codex → daemon → ...`) are not mistaken for zombies even though
/// they have no path to a real CC. See ADR-018.
const TRUSTED_OWNER_STEMS: &[&str] = &["node", "claude", "team_mode_daemon", "codex"];

/// Walk `mcp_pid`'s parent chain (depth-bounded) looking for a real
/// living trusted ancestor. Two layered defences against Windows PID
/// recycling:
///
/// 1. **Per-hop chronology check**: every walked process must have been
///    born no later than its child. If we ever see
///    `parent.start_time > child.start_time`, the OS has reassigned
///    that PID to an unrelated newer process — the chain is broken
///    and we abort.
/// 2. **Trusted stem check**: only return Some when the surviving link
///    has a stem in `TRUSTED_OWNER_STEMS`.
///
/// Without (1), spared decisions look correct (`is_some()` and stem
/// match) yet point at processes that have nothing to do with the
/// zombie peer — exactly what we observed in the wild on machines with
/// rapid codex.exe churn (ADR-018).
fn peer_trusted_ancestor(mcp_pid: u32, sys: &sysinfo::System) -> Option<(u32, String)> {
    use sysinfo::Pid;
    const MAX_DEPTH: u8 = 12;
    let me = Pid::from_u32(mcp_pid);
    let me_proc = sys.process(me)?;
    let mut child_start = me_proc.start_time();
    let mut current = me_proc.parent()?;
    for _ in 0..MAX_DEPTH {
        let proc = sys.process(current)?;
        let parent_start = proc.start_time();
        // Causality: a parent process must have been born no later than
        // its child. Equality is fine (same-second spawn). Strictly
        // greater means PID recycling.
        if parent_start > child_start {
            return None;
        }
        let name_lc = proc.name().to_string_lossy().to_lowercase();
        let stem = name_lc.trim_end_matches(".exe").to_string();
        if TRUSTED_OWNER_STEMS.contains(&stem.as_str()) {
            return Some((current.as_u32(), stem));
        }
        // Non-trusted stem — keep walking, but treat this hop as the
        // new "child" for the next chronology check.
        child_start = parent_start;
        current = proc.parent()?;
    }
    None
}

/// Return true iff `cc_pid` refers to the same process as recorded at
/// startup: PID exists AND stem ∈ `CC_BIN_STEMS` AND start_time matches
/// `expected_start_time`. The third check protects against Windows PID
/// recycling — the dead CC's PID may be reused for a brand-new node.exe
/// (e.g. another Electron app), and stem alone would let the watchdog
/// believe the CC is still alive forever.
fn cc_pid_alive(cc_pid: u32, expected_start_time: u64, sys: &sysinfo::System) -> bool {
    use sysinfo::Pid;
    let Some(proc) = sys.process(Pid::from_u32(cc_pid)) else {
        return false;
    };
    let name_lc = proc.name().to_string_lossy().to_lowercase();
    let stem = name_lc.trim_end_matches(".exe");
    if !CC_BIN_STEMS.contains(&stem) {
        return false;
    }
    proc.start_time() == expected_start_time
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let started_at = Instant::now();
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
    let ctrl_c_absorbed = Arc::new(AtomicBool::new(false));
    std::thread::Builder::new()
        .name("mcp-ctrl-c-absorber".into())
        .spawn({
            let ctrl_c_absorbed = Arc::clone(&ctrl_c_absorbed);
            move || {
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
                        ctrl_c_absorbed.store(true, Ordering::SeqCst);
                        log_mcp_exit(
                            "mcp.exit.ctrl_c_absorbed",
                            started_at,
                            "ctrl_c_absorbed_without_process_exit",
                        );
                    }
                });
            }
        })
        .ok();

    tracing::info!("Team Mode MCP server starting");
    tracing::info!(data_dir = %data_dir.display(), "using data directory");

    // Resolve our owning CC PID once at startup. The watchdog below polls
    // this PID and self-exits when it's gone; the startup sweep below
    // uses the same algorithm via `resolve_cc_pid_from` to decide which
    // peer MCPs are zombies.
    //
    // Why: stdio MCP processes only notice CC dying via stdin EOF, which
    // is unreliable on Windows when CC is force-killed or its console
    // handle isn't torn down cleanly. We accumulated 11+ orphan
    // `team_mode_mcp.exe` processes in the wild before adding this
    // defence — see ADR-017.
    let owner_cc_pid = current_cc_pid();
    tracing::info!(
        owner_cc_pid = ?owner_cc_pid,
        "MCP: resolved owning CC PID at startup"
    );

    // Worker MCP relays (spawned by codex workers under the daemon) have
    // a process tree that does NOT lead back to the lead CC — their
    // ancestor chain is `worker codex.exe → daemon → ...`. Running sweep
    // or watchdog on them would (a) make sweep want to kill them because
    // their resolved `owner_cc_pid` is a fallback wrapper PID, and (b)
    // make watchdog self-exit prematurely. Worker relays already follow
    // their codex parent's lifecycle via stdin EOF + daemon kill_on_drop.
    // Recognise them by the `TEAM_MODE_TEAM` env var (set by the daemon
    // when it spawns codex). Lead MCP processes started by CC do not get
    // this var and remain protected by sweep + watchdog.
    let is_worker_relay = env::var_os("TEAM_MODE_TEAM").is_some();
    if is_worker_relay {
        tracing::info!(
            "MCP: detected worker relay (TEAM_MODE_TEAM set); \
             skipping startup sweep and parent-liveness watchdog"
        );
    } else {
        sweep_zombie_mcp_peers();
        spawn_parent_liveness_watchdog(owner_cc_pid, started_at);
    }

    let project_root = env::current_dir()?;

    // Startup prune: if a previous daemon self-killed (lead-watchdog grace
    // expiry on empty teams) but left runtime/daemon.json behind, the next
    // tool call would otherwise burn a 2s TCP timeout pinging a dead pid
    // before falling through to spawn a fresh daemon. Prune up-front so
    // /mcp reconnect after daemon death is fast and clean.
    prune_stale_endpoint(&data_dir);

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
    let exit_reason = runtime.run_stdio_with_exit_reason();
    match &exit_reason {
        Ok(StdioExitReason::StdinEof) => {
            log_mcp_exit("mcp.exit.stdin_eof", started_at, "stdin closed");
        }
        Ok(StdioExitReason::StdinReadError(error)) => {
            log_mcp_exit("mcp.exit.signal", started_at, error);
        }
        Err(error) => {
            log_mcp_exit("mcp.exit.signal", started_at, &error.to_string());
        }
    }
    if ctrl_c_absorbed.load(Ordering::SeqCst) {
        log_mcp_exit(
            "mcp.exit.ctrl_c_absorbed",
            started_at,
            "run_stdio_exit_after_ctrl_c_absorbed",
        );
    }
    exit_reason?;
    Ok(())
}

fn log_mcp_exit(event: &'static str, started_at: Instant, reason: &str) {
    tracing::warn!(
        event,
        pid = std::process::id(),
        parent_pid = ?current_parent_pid(),
        uptime_ms = started_at.elapsed().as_millis() as u64,
        reason,
        "{event}"
    );
}

fn current_parent_pid() -> Option<u32> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.process(Pid::from_u32(std::process::id()))
        .and_then(|proc| proc.parent())
        .map(|pid| pid.as_u32())
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

/// Scan all running `team_mode_mcp` processes (peers, not us) and kill
/// any whose ancestor chain contains no living trusted owner (CC,
/// daemon, or codex worker). Best-effort — failures are logged but
/// never abort startup.
///
/// This eliminates the historical zombie-accumulation problem: if a CC
/// session crashes without closing the MCP stdio pipe, the orphan MCP
/// can hang in `read()` forever. The next CC session sweeps it up here.
///
/// Selection rule (ADR-018): walk each peer's parent chain looking for
/// any living process whose name stem is in `TRUSTED_OWNER_STEMS`.
/// Found → spared (live MCP of another CC session, or worker MCP relay
/// of a live codex/daemon). Not found → killed. The previous PID-only
/// alive check was unreliable on Windows because high-density node.exe
/// environments recycle dead CC PIDs almost immediately.
fn sweep_zombie_mcp_peers() {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());

    let my_pid = std::process::id();
    let mut killed = 0u32;
    let mut spared = 0u32;

    for (pid, proc) in sys.processes() {
        let pid_u32 = pid.as_u32();
        if pid_u32 == my_pid {
            continue;
        }
        let name_lc = proc.name().to_string_lossy().to_lowercase();
        let stem = name_lc.trim_end_matches(".exe");
        if stem != MCP_BIN_STEM {
            continue;
        }

        let cc_pid = resolve_cc_pid_from(pid_u32, &sys);
        let trusted = peer_trusted_ancestor(pid_u32, &sys);

        if let Some((ancestor_pid, ancestor_stem)) = trusted {
            spared += 1;
            tracing::info!(
                peer_mcp_pid = pid_u32,
                cc_pid = ?cc_pid,
                ancestor_pid,
                ancestor_stem,
                "MCP startup sweep: peer has trusted living ancestor, sparing"
            );
            continue;
        }

        tracing::warn!(
            peer_mcp_pid = pid_u32,
            cc_pid = ?cc_pid,
            "MCP startup sweep: no trusted ancestor in chain, killing zombie team_mode_mcp peer"
        );
        if proc.kill() {
            killed += 1;
        } else {
            tracing::warn!(
                peer_mcp_pid = pid_u32,
                "MCP startup sweep: kill() failed (insufficient permissions or already gone)"
            );
        }
    }

    tracing::info!(killed, spared, "MCP startup sweep complete");
}

/// Spawn a background thread that polls the owning CC PID every
/// `PARENT_LIVENESS_INTERVAL` and exits the MCP process the moment that
/// PID disappears.
///
/// This is the second layer of defence (the first being stdin EOF
/// detection in `runtime.rs::run_stdio`). Stdin EOF is best-effort on
/// Windows; this watchdog guarantees the MCP follows its CC within ~5s.
///
/// If `initial_cc_pid` is `None` (process tree query failed at startup)
/// we skip the watchdog entirely rather than risk killing ourselves on
/// every tick — the daemon's own lead-watchdog still bounds team
/// lifetime, and stdin EOF still works in the common case.
fn spawn_parent_liveness_watchdog(initial_cc_pid: Option<u32>, started_at: Instant) {
    let cc_pid = match initial_cc_pid {
        Some(p) => p,
        None => {
            tracing::warn!(
                "MCP: parent CC PID not resolvable at startup; \
                 parent-liveness watchdog disabled (relying on stdin EOF only)"
            );
            return;
        }
    };

    std::thread::Builder::new()
        .name("mcp-parent-liveness".into())
        .spawn(move || {
            use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

            let mut sys = System::new_with_specifics(
                RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
            );
            sys.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing(),
            );

            // Capture the parent CC's start_time at watchdog start. Subsequent
            // ticks compare against this exact value so PID recycling (the
            // CC dies and Windows hands its PID to an unrelated node.exe)
            // is detected as an identity mismatch rather than treated as
            // "still alive". See ADR-018.
            let Some(initial_proc) = sys.process(Pid::from_u32(cc_pid)) else {
                tracing::warn!(
                    cc_pid,
                    "MCP: parent CC PID gone before watchdog could record start_time, exiting"
                );
                log_mcp_exit(
                    "mcp.exit.watchdog_parent_dead",
                    started_at,
                    "parent_gone_before_start_time_capture",
                );
                std::process::exit(0);
            };
            let cc_start_time = initial_proc.start_time();
            tracing::info!(
                cc_pid,
                cc_start_time,
                "MCP: parent-liveness watchdog started"
            );

            loop {
                std::thread::sleep(PARENT_LIVENESS_INTERVAL);
                sys.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    ProcessRefreshKind::nothing(),
                );
                if !cc_pid_alive(cc_pid, cc_start_time, &sys) {
                    tracing::warn!(
                        cc_pid,
                        cc_start_time,
                        "MCP: parent CC died (or PID recycled to a different node), exiting (watchdog)"
                    );
                    log_mcp_exit(
                        "mcp.exit.watchdog_parent_dead",
                        started_at,
                        "parent_dead_or_pid_recycled",
                    );
                    std::process::exit(0);
                }
            }
        })
        .ok();
}
