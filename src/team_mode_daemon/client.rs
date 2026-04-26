use std::fs::OpenOptions;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::team_mode::data_dir;
use crate::team_mode::mcp::executor::TeamModeToolExecutor;
use crate::team_mode::mcp::schemas::ToolDescriptor;
use crate::team_mode::mcp::tools::ToolExecution;
use crate::team_mode_daemon::ipc::{
    DaemonInfo, DaemonRequest, DaemonResponse, info_path, read_frame, read_info, write_frame,
};

pub struct DaemonToolClient {
    base_dir: PathBuf,
    project_root: PathBuf,
    owner_cc_pid: Option<u32>,
    /// Caller identity for daemon RPCs — populated from the relay process's
    /// own env. When this relay was spawned by a worker subprocess (via the
    /// worker's auto-loaded .mcp.json or injected codex config), the daemon
    /// passes `TEAM_MODE_TEAM` and `TEAM_MODE_MEMBER` env vars; we read them
    /// here and attach them to every tool call so the daemon knows the
    /// real sender. When unset (the lead's own MCP relay started by Claude
    /// Code from the project's .mcp.json), we default to lead — that's the
    /// historical behavior and matches the implicit assumption everywhere.
    caller_team: Option<String>,
    caller_member: String,
    cached_info: Mutex<Option<DaemonInfo>>,
    next_id: AtomicU64,
}

impl DaemonToolClient {
    pub fn new(base_dir: impl Into<PathBuf>, project_root: impl Into<PathBuf>) -> Self {
        let caller_team = std::env::var("TEAM_MODE_TEAM").ok().filter(|s| !s.is_empty());
        let caller_member = std::env::var("TEAM_MODE_MEMBER")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "lead".to_string());
        Self {
            base_dir: base_dir.into(),
            project_root: project_root.into(),
            owner_cc_pid: current_parent_pid(),
            caller_team,
            caller_member,
            cached_info: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }

    fn call_method(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let mut last_error = None;
        for _ in 0..2 {
            let info = self.ensure_daemon()?;
            match self.call_existing(&info, method, params.clone()) {
                Ok(value) => return Ok(value),
                Err(err) => {
                    last_error = Some(err.to_string());
                    *self.cached_info.lock().unwrap() = None;
                }
            }
        }
        Err(Error::Other(format!(
            "daemon request failed: {}",
            last_error.unwrap_or_else(|| "unknown error".into())
        )))
    }

    fn ensure_daemon(&self) -> Result<DaemonInfo> {
        if let Some(info) = self.cached_info.lock().unwrap().clone() {
            if is_pid_alive(info.pid) && self.ping_info(&info).is_ok() {
                return Ok(info);
            }
        }

        // Fast path: prune endpoint file if its pid is already dead. Avoids
        // the 2s TCP timeout in ping_info and ensures stale endpoints from a
        // self-killed daemon (lead-watchdog grace expiry) don't confuse the
        // next request.
        prune_stale_endpoint(&self.base_dir);

        if let Ok(info) = read_info(&self.base_dir) {
            if is_pid_alive(info.pid) && self.ping_info(&info).is_ok() {
                *self.cached_info.lock().unwrap() = Some(info.clone());
                return Ok(info);
            }
        }

        self.start_daemon_locked()
    }

    fn start_daemon_locked(&self) -> Result<DaemonInfo> {
        std::fs::create_dir_all(data_dir::locks_dir(&self.base_dir))?;
        let lock_path = data_dir::lock_path(&self.base_dir, "daemon-start");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        lock.lock_exclusive().map_err(|err| Error::LockFailed {
            path: lock_path.clone(),
            reason: err.to_string(),
        })?;

        let result = (|| {
            // Re-prune inside the lock — between ensure_daemon and now, another
            // MCP could have noticed the same death and (a) pruned the file
            // already, or (b) started a fresh daemon that wrote a new file.
            // Either way is fine: read_info will reflect current truth.
            prune_stale_endpoint(&self.base_dir);

            if let Ok(info) = read_info(&self.base_dir) {
                if is_pid_alive(info.pid) && self.ping_info(&info).is_ok() {
                    *self.cached_info.lock().unwrap() = Some(info.clone());
                    return Ok(info);
                }
            }

            let token = Uuid::new_v4().to_string();
            let daemon_exe = daemon_exe_path()?;
            let daemon_log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.base_dir.join("daemon.log"))?;

            let mut command = Command::new(daemon_exe);
            command
                .arg("--data-dir")
                .arg(&self.base_dir)
                .arg("--project-root")
                .arg(&self.project_root)
                .arg("--token")
                .arg(&token)
                .current_dir(&self.project_root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(daemon_log));

            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                const DETACHED_PROCESS: u32 = 0x0000_0008;
                const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
                const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
                // Try breaking away from any parent Job Object so that when
                // CC terminates its job (e.g. on ESC or session end), the
                // daemon survives. If CC's job forbids breakaway, the spawn
                // fails with ERROR_ACCESS_DENIED — we retry without the flag
                // below so the refactor still degrades gracefully (daemon
                // dies with CC, but at least every non-ESC code path works).
                command.creation_flags(
                    DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
                );
            }

            let spawn_result = command.spawn();
            let _child = match spawn_result {
                Ok(child) => child,
                #[cfg(target_os = "windows")]
                Err(err) if err.raw_os_error() == Some(5) => {
                    // ERROR_ACCESS_DENIED (5) — CC's job object forbids
                    // breakaway. Retry without the breakaway flag so daemon
                    // at least starts (it'll share CC's job and die with it,
                    // but that's strictly better than failing to start).
                    use std::os::windows::process::CommandExt;
                    const DETACHED_PROCESS: u32 = 0x0000_0008;
                    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
                    tracing::warn!(
                        "daemon spawn with CREATE_BREAKAWAY_FROM_JOB denied (parent job forbids \
                         breakaway); retrying without — daemon will share CC's job object"
                    );
                    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
                    command.spawn().map_err(|err| {
                        Error::Other(format!("failed to spawn team_mode_daemon: {err}"))
                    })?
                }
                Err(err) => {
                    return Err(Error::Other(format!(
                        "failed to spawn team_mode_daemon: {err}"
                    )));
                }
            };

            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Ok(info) = read_info(&self.base_dir) {
                    if info.token == token && self.ping_info(&info).is_ok() {
                        *self.cached_info.lock().unwrap() = Some(info.clone());
                        return Ok(info);
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }

            Err(Error::Timeout { seconds: 5 })
        })();

        let _ = fs2::FileExt::unlock(&lock);
        result
    }

    fn ping_info(&self, info: &DaemonInfo) -> Result<()> {
        self.call_existing(info, "daemon/ping", None).map(|_| ())
    }

    fn call_existing(
        &self,
        info: &DaemonInfo,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value> {
        let addr = (info.host.as_str(), info.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| Error::Other("daemon address did not resolve".into()))?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = DaemonRequest {
            id,
            token: info.token.clone(),
            method: method.into(),
            params,
        };
        write_frame(&mut stream, &request)?;
        let response: DaemonResponse = read_frame(&mut stream)?;
        if response.id != id {
            return Err(Error::Other(format!(
                "daemon response id mismatch: expected {id}, got {}",
                response.id
            )));
        }
        if let Some(error) = response.error {
            return Err(Error::Other(error));
        }
        Ok(response.result.unwrap_or(Value::Null))
    }
}

impl TeamModeToolExecutor for DaemonToolClient {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        let value = self.call_method("tools/list", None)?;
        Ok(serde_json::from_value(value)?)
    }

    fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<ToolExecution> {
        let value = self.call_method(
            "tools/call",
            Some(json!({
                "name": name,
                "arguments": arguments,
                "context": {
                    "owner_cc_pid": self.owner_cc_pid,
                    "project_root": self.project_root,
                    // Bug 29: caller identity for sender attribution. The
                    // daemon's `inject_call_context` lifts these into
                    // `_caller_team` / `_caller_member` arguments so
                    // identity-aware tools (send_message, etc.) can
                    // attribute the action to the correct member instead
                    // of forging "lead".
                    "caller_team": self.caller_team,
                    "caller_member": self.caller_member,
                }
            })),
        )?;
        Ok(serde_json::from_value(value)?)
    }
}

fn daemon_exe_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("TEAM_MODE_DAEMON_EXE") {
        return Ok(PathBuf::from(path));
    }

    let mut exe = std::env::current_exe()?;
    exe.set_file_name(exe_name("team_mode_daemon"));
    if exe.exists() {
        return Ok(exe);
    }

    Err(Error::Other(format!(
        "team_mode_daemon executable not found next to current executable; set TEAM_MODE_DAEMON_EXE (looked for {})",
        exe.display()
    )))
}

fn exe_name(stem: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        format!("{stem}.exe")
    }
    #[cfg(not(target_os = "windows"))]
    {
        stem.to_string()
    }
}

/// Best-effort liveness check for a PID. Returns `false` if the process has
/// exited. Treats refresh failures as `true` (fail-open) so a sysinfo glitch
/// doesn't accidentally prune a healthy daemon.
fn is_pid_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let target = Pid::from_u32(pid);
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[target]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(target).is_some()
}

/// If `<base>/runtime/daemon.json` references a dead pid, delete it. Idempotent
/// and silent on missing file / read errors — this is a best-effort cleanup.
pub fn prune_stale_endpoint(base_dir: &std::path::Path) {
    let path = info_path(base_dir);
    let Ok(info) = read_info(base_dir) else {
        return;
    };
    if is_pid_alive(info.pid) {
        return;
    }
    match std::fs::remove_file(&path) {
        Ok(()) => tracing::info!(
            stale_pid = info.pid,
            path = %path.display(),
            "pruned stale daemon endpoint (pid no longer alive)",
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!(
            stale_pid = info.pid,
            path = %path.display(),
            error = %err,
            "failed to prune stale daemon endpoint",
        ),
    }
}

fn current_parent_pid() -> Option<u32> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    let me = Pid::from_u32(std::process::id());
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[me]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(me)
        .and_then(|p| p.parent())
        .map(|ppid| ppid.as_u32())
}
