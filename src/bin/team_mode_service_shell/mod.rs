use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use agent_teams::util::file_lock::FileLock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8786;
const RELAY_SPAWN_TIMEOUT_SECS: u64 = 30;
const RELAY_SPAWN_POLL_MS: u64 = 250;
const POLL_INTERVAL_MS: u64 = 500;
const BATCH_GRACE_MS: u64 = 2000;
const LONG_IDLE_SLEEP_SECS: u64 = 60;
const MAX_OWNER_WALK_DEPTH: usize = 40;
const SHELL_WRAPPER_NAMES: &[&str] = &["cmd", "sh", "bash", "zsh", "pwsh", "powershell", "conhost"];

#[derive(Debug, Deserialize)]
struct RuntimeInfo {
    pid: u32,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    url: Option<String>,
    #[serde(alias = "tokenFile")]
    token_file: PathBuf,
}

#[derive(Debug, Clone)]
struct HttpHeaders {
    authorization: String,
    owner_cc_pid: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct TeamEntry {
    id: String,
    pending_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct MyTeamsResponse {
    #[serde(default)]
    cc_pid: Option<u32>,
    #[serde(default)]
    teams: Vec<TeamEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct IdentityCache {
    session_id: String,
    cached_at: String,
    cc_pid: Option<u32>,
    teams: Vec<TeamEntry>,
}

#[derive(Debug)]
struct ResolvedRuntime {
    info: RuntimeInfo,
}

#[derive(Debug)]
enum HealthProbeError {
    Transient(String),
    IdentityMismatch(String),
}

impl fmt::Display for HealthProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HealthProbeError::Transient(message) | HealthProbeError::IdentityMismatch(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for HealthProbeError {}

#[derive(Debug, Clone)]
struct RelaySpawnSpec {
    service_exe: PathBuf,
    project_root: PathBuf,
    data_dir_override: Option<PathBuf>,
    runtime_dir: PathBuf,
    port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessRow {
    ppid: u32,
    name: String,
}

pub(crate) fn relay_stdio(
    data_dir_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let project_root = project_root()?;
    let client = http_client()?;
    let runtime = ensure_runtime_for_relay(&project_root, data_dir_override.as_deref(), &client)?;
    // Token is read from disk once: it doesn't change for the lifetime of
    // a service instance and the relay can recover from a service restart
    // by exiting cleanly when the service rejects a stale token.
    let static_headers = build_http_headers(&project_root, &runtime)?;
    let service_url = runtime_url(&runtime);

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = io::BufReader::new(stdin.lock());
    let mut writer = io::BufWriter::new(stdout.lock());

    loop {
        let Some(message) = read_json_rpc_message(&mut reader)? else {
            writer.flush()?;
            return Ok(());
        };
        // Re-walk the parent chain on every forward instead of caching the
        // startup result. Windows stdin EOF is unreliable, so a relay can
        // outlive the CC that spawned it; if the user restarts CC, our
        // parent chain points at a fresh node.exe with a different PID.
        // Re-walking keeps `X-Team-Mode-Owner-CC-Pid` aligned with the
        // currently-alive CC, which `team_create` (ADR-023) and
        // `/lead-pending/my-teams` both rely on for owner matching.
        let headers = HttpHeaders {
            authorization: static_headers.authorization.clone(),
            owner_cc_pid: owner_cc_pid(),
        };
        let response = forward_json_rpc_message(&client, &service_url, &headers, message)?;
        if let Some(value) = response {
            write_json_rpc_message(&mut writer, &value)?;
        }
    }
}

pub(crate) fn headers_stdout(
    data_dir_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let project_root = project_root()?;
    let client = http_client()?;
    let runtime = match find_healthy_runtime(&client, &project_root, data_dir_override.as_deref())?
    {
        Some(runtime) => runtime.info,
        None => {
            return Err(format!(
                "no healthy Team Mode runtime found for project '{}'",
                project_root.display()
            )
            .into());
        }
    };
    let headers = build_http_headers(&project_root, &runtime)?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    write!(
        out,
        "{}",
        serde_json::to_string(&headers_to_json(&headers))?
    )?;
    out.flush()?;
    Ok(())
}

pub(crate) fn run_async_wake_hook() -> ! {
    let project_root = match project_root() {
        Ok(root) => root,
        Err(err) => exit_with_error(err),
    };
    if !project_has_team_registration(&project_root).unwrap_or(false) {
        std::process::exit(0);
    }
    if env::var_os("TEAM_MODE_WORKER").is_some() {
        std::process::exit(0);
    }

    let event = read_hook_event();
    if event
        .as_ref()
        .and_then(|v| v.get("hook_event_name"))
        .and_then(Value::as_str)
        != Some("Stop")
    {
        std::process::exit(0);
    }
    let session_id = event
        .as_ref()
        .and_then(|v| v.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let client = match http_client() {
        Ok(client) => client,
        Err(err) => exit_with_hook_error(err),
    };
    let runtime = match find_healthy_runtime(&client, &project_root, None) {
        Ok(Some(runtime)) => runtime.info,
        Ok(None) => exit_hook_error(
            1,
            &format!(
                "lead-pending-async-wake: no healthy Team Mode runtime found in {} or legacy fallback",
                project_root.display()
            ),
        ),
        Err(err) => exit_hook_error(1, &format!("lead-pending-async-wake: {err}")),
    };
    let headers = match build_http_headers(&project_root, &runtime) {
        Ok(headers) => headers,
        Err(err) => exit_with_hook_error(err),
    };
    let service_url = runtime_url(&runtime);
    let my_teams = fetch_my_teams_checked(
        &client,
        &service_url,
        &headers,
        session_id.as_deref(),
        "lead-pending-async-wake",
    )
    .unwrap_or_else(|err| exit_hook_error(err.code, &err.message));

    if my_teams.teams.is_empty() {
        loop {
            thread::sleep(Duration::from_secs(LONG_IDLE_SLEEP_SECS));
        }
    }

    loop {
        let mut found = Vec::new();
        for team in &my_teams.teams {
            found.extend(drain_pending_file(&team.pending_path));
        }
        if !found.is_empty() {
            thread::sleep(Duration::from_millis(BATCH_GRACE_MS));
            for team in &my_teams.teams {
                found.extend(drain_pending_file(&team.pending_path));
            }
            eprintln!("{}", render_async_wake_reminder(&found));
            std::process::exit(2);
        }
        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
    }
}

pub(crate) fn run_mid_turn_hook() -> ! {
    let project_root = match project_root() {
        Ok(root) => root,
        Err(err) => exit_with_error(err),
    };
    if !project_has_team_registration(&project_root).unwrap_or(false) {
        std::process::exit(0);
    }
    if env::var_os("TEAM_MODE_WORKER").is_some() {
        std::process::exit(0);
    }

    let event = read_hook_event();
    let Some(session_id) = event
        .as_ref()
        .and_then(|v| v.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        std::process::exit(0);
    };

    let client = match http_client() {
        Ok(client) => client,
        Err(err) => exit_with_hook_error(err),
    };
    let runtime = match find_healthy_runtime(&client, &project_root, None) {
        Ok(Some(runtime)) => runtime.info,
        Ok(None) => exit_hook_error(
            1,
            &format!(
                "lead-pending-mid-turn: no healthy Team Mode runtime found in {} or legacy fallback",
                project_root.display()
            ),
        ),
        Err(err) => exit_hook_error(1, &format!("lead-pending-mid-turn: {err}")),
    };
    let headers = match build_http_headers(&project_root, &runtime) {
        Ok(headers) => headers,
        Err(err) => exit_with_hook_error(err),
    };
    let service_url = runtime_url(&runtime);
    let identity =
        resolve_mid_turn_identity(&project_root, &session_id, &client, &service_url, &headers)
            .unwrap_or_else(|err| exit_hook_error(err.code, &err.message));

    if identity.teams.is_empty() {
        std::process::exit(0);
    }

    let mut found = Vec::new();
    for team in &identity.teams {
        found.extend(drain_pending_file(&team.pending_path));
    }
    if found.is_empty() {
        std::process::exit(0);
    }

    let payload = json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": render_mid_turn_reminder(&found),
        },
    });
    let mut out = io::BufWriter::new(io::stdout().lock());
    let _ = write!(
        out,
        "{}",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into())
    );
    let _ = out.flush();
    std::process::exit(0);
}

pub(crate) fn service_lock_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("service.lock")
}

pub(crate) fn try_acquire_service_lock(
    runtime_dir: &Path,
) -> Result<Option<FileLock>, Box<dyn std::error::Error>> {
    let lock_path = service_lock_path(runtime_dir);
    Ok(FileLock::try_acquire(&lock_path)?)
}

pub(crate) fn project_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(root) = env::var_os("CLAUDE_PROJECT_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    Ok(env::current_dir()?)
}

fn legacy_runtime_info_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".agent-teams")
        .join("runtime")
        .join("http-mcp.json")
}

fn global_runtime_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home = dirs::home_dir().ok_or_else(|| {
        "could not resolve home directory for global Team Mode runtime".to_string()
    })?;
    Ok(home.join(".team-mode").join("runtime"))
}

fn runtime_info_path_candidates(
    project_root: &Path,
    data_dir_override: Option<&Path>,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut candidates = Vec::new();
    if let Some(data_dir) = data_dir_override.filter(|path| !path.as_os_str().is_empty()) {
        candidates.push(data_dir.join("runtime").join("http-mcp.json"));
    }
    if let Ok(global_runtime_dir) = global_runtime_dir() {
        candidates.push(global_runtime_dir.join("http-mcp.json"));
    }
    candidates.push(legacy_runtime_info_path(project_root));
    Ok(candidates)
}

fn service_exe_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = env::var_os("TEAM_MODE_SERVICE_EXE").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    Ok(env::current_exe()?)
}

fn project_has_team_registration(project_root: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let mcp = project_root.join(".mcp.json");
    if mcp.exists() {
        if let Ok(value) = read_json_file(&mcp) {
            if mcp_json_has_team_mode(&value) {
                return Ok(true);
            }
        }
    }

    for path in [
        project_root.join(".claude").join("settings.json"),
        project_root.join(".claude").join("settings.local.json"),
    ] {
        if path.exists() {
            if let Ok(value) = read_json_file(&path) {
                if settings_has_team_mode_hooks(&value) {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

fn mcp_json_has_team_mode(value: &Value) -> bool {
    value
        .get("mcpServers")
        .and_then(Value::as_object)
        .map(|servers| servers.contains_key("team-mode"))
        .unwrap_or(false)
}

fn settings_has_team_mode_hooks(value: &Value) -> bool {
    json_contains_string(value, "lead-pending-async-wake.js")
        || json_contains_string(value, "lead-pending-mid-turn.js")
        || json_contains_string(value, "team_mode_service hook async-wake")
        || json_contains_string(value, "team_mode_service hook mid-turn")
}

fn read_runtime_info_from_path(path: &Path) -> Result<RuntimeInfo, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let info: RuntimeInfo = serde_json::from_str(&content).map_err(|err| {
        io::Error::other(format!(
            "failed to parse runtime info '{}': {err}",
            path.display()
        ))
    })?;
    Ok(info)
}

fn wait_for_runtime_info(path: &Path) -> Result<RuntimeInfo, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(RELAY_SPAWN_TIMEOUT_SECS);
    let mut last_error = None;
    while Instant::now() < deadline {
        match read_runtime_info_from_path(path) {
            Ok(info) => return Ok(info),
            Err(err) => last_error = Some(err.to_string()),
        }
        thread::sleep(Duration::from_millis(RELAY_SPAWN_POLL_MS));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "service spawn failed: {}",
            last_error.unwrap_or_else(|| "runtime info file timed out".into())
        ),
    )
    .into())
}

fn discover_runtime_candidate(
    project_root: &Path,
    data_dir_override: Option<&Path>,
) -> Result<Option<ResolvedRuntime>, Box<dyn std::error::Error>> {
    for path in runtime_info_path_candidates(project_root, data_dir_override)? {
        if !path.exists() {
            continue;
        }
        if let Ok(info) = read_runtime_info_from_path(&path) {
            return Ok(Some(ResolvedRuntime { info }));
        }
    }
    Ok(None)
}

fn runtime_base_url(info: &RuntimeInfo) -> String {
    runtime_url(info)
        .trim_end_matches("/mcp")
        .trim_end_matches('/')
        .to_string()
}

fn probe_healthz(
    client: &reqwest::blocking::Client,
    info: &RuntimeInfo,
    expected_runtime_dir: &Path,
) -> Result<(), HealthProbeError> {
    let url = format!("{}/healthz", runtime_base_url(info));
    let response = client
        .get(url)
        .send()
        .map_err(|err| HealthProbeError::Transient(err.to_string()))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|err| HealthProbeError::Transient(err.to_string()))?;
    if !status.is_success() {
        return Err(HealthProbeError::Transient(format!(
            "HTTP {}: {text}",
            status
        )));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|err| HealthProbeError::Transient(err.to_string()))?;
    if value.get("status").and_then(Value::as_str) != Some("ok") {
        return Err(HealthProbeError::Transient(
            "healthz response missing ok status".into(),
        ));
    }
    let actual_runtime_dir = value
        .get("runtime_dir")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            HealthProbeError::IdentityMismatch(
                "service identity mismatch: healthz response missing runtime_dir".into(),
            )
        })?;
    if Path::new(actual_runtime_dir) != expected_runtime_dir {
        return Err(HealthProbeError::IdentityMismatch(format!(
            "service identity mismatch: expected runtime_dir '{}' but healthz reported '{}'",
            expected_runtime_dir.display(),
            actual_runtime_dir
        )));
    }
    let actual_lock_holder_pid = value
        .get("lock_holder_pid")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            HealthProbeError::IdentityMismatch(
                "service identity mismatch: healthz response missing lock_holder_pid".into(),
            )
        })?;
    if actual_lock_holder_pid != u64::from(info.pid) {
        return Err(HealthProbeError::IdentityMismatch(format!(
            "service identity mismatch: expected lock_holder_pid {} but healthz reported {}",
            info.pid, actual_lock_holder_pid
        )));
    }
    Ok(())
}

fn wait_for_healthz(
    client: &reqwest::blocking::Client,
    info: &RuntimeInfo,
    expected_runtime_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(RELAY_SPAWN_TIMEOUT_SECS);
    let mut last_error = None;
    while Instant::now() < deadline {
        match probe_healthz(client, info, expected_runtime_dir) {
            Ok(()) => return Ok(()),
            Err(HealthProbeError::Transient(err)) => last_error = Some(err),
            Err(HealthProbeError::IdentityMismatch(err)) => {
                return Err(io::Error::other(err).into());
            }
        }
        thread::sleep(Duration::from_millis(RELAY_SPAWN_POLL_MS));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "service spawn failed: {}",
            last_error.unwrap_or_else(|| "healthz probe timed out".into())
        ),
    )
    .into())
}

fn find_healthy_runtime(
    client: &reqwest::blocking::Client,
    project_root: &Path,
    data_dir_override: Option<&Path>,
) -> Result<Option<ResolvedRuntime>, Box<dyn std::error::Error>> {
    for path in runtime_info_path_candidates(project_root, data_dir_override)? {
        if !path.exists() {
            continue;
        }
        let Ok(info) = read_runtime_info_from_path(&path) else {
            continue;
        };
        let Some(runtime_dir) = path.parent() else {
            continue;
        };
        match probe_healthz(client, &info, runtime_dir) {
            Ok(()) => return Ok(Some(ResolvedRuntime { info })),
            Err(HealthProbeError::Transient(_)) => continue,
            Err(HealthProbeError::IdentityMismatch(err)) => {
                return Err(io::Error::other(err).into());
            }
        }
    }
    Ok(None)
}

fn runtime_port_hint(candidate: Option<&ResolvedRuntime>, default_port: u16) -> u16 {
    candidate
        .and_then(|runtime| runtime.info.port)
        .unwrap_or(default_port)
}

fn build_relay_spawn_spec(
    project_root: &Path,
    data_dir_override: Option<&Path>,
    port: u16,
) -> Result<RelaySpawnSpec, Box<dyn std::error::Error>> {
    let runtime_dir = if let Some(data_dir) = data_dir_override {
        data_dir.join("runtime")
    } else {
        global_runtime_dir()?
    };
    Ok(RelaySpawnSpec {
        service_exe: service_exe_path()?,
        project_root: project_root.to_path_buf(),
        data_dir_override: data_dir_override.map(Path::to_path_buf),
        runtime_dir,
        port,
    })
}

fn ensure_runtime_for_relay(
    project_root: &Path,
    data_dir_override: Option<&Path>,
    client: &reqwest::blocking::Client,
) -> Result<RuntimeInfo, Box<dyn std::error::Error>> {
    ensure_runtime_for_relay_with_spawn(
        project_root,
        data_dir_override,
        client,
        spawn_service_detached,
    )
}

fn ensure_runtime_for_relay_with_spawn<F>(
    project_root: &Path,
    data_dir_override: Option<&Path>,
    client: &reqwest::blocking::Client,
    spawn_service: F,
) -> Result<RuntimeInfo, Box<dyn std::error::Error>>
where
    F: Fn(&RelaySpawnSpec) -> Result<(), Box<dyn std::error::Error>>,
{
    if let Some(runtime) = find_healthy_runtime(client, project_root, data_dir_override)? {
        return Ok(runtime.info);
    }

    let discovered = discover_runtime_candidate(project_root, data_dir_override)?;
    let port = runtime_port_hint(discovered.as_ref(), DEFAULT_PORT);
    let spawn_spec = build_relay_spawn_spec(project_root, data_dir_override, port)?;
    spawn_service(&spawn_spec)?;
    let spawned_runtime = wait_for_runtime_info(&spawn_spec.runtime_dir.join("http-mcp.json"))?;
    wait_for_healthz(client, &spawned_runtime, &spawn_spec.runtime_dir)?;
    Ok(spawned_runtime)
}

fn runtime_url(info: &RuntimeInfo) -> String {
    if let Some(url) = &info.url {
        return url.clone();
    }
    let host = info.host.as_deref().unwrap_or(DEFAULT_HOST);
    let port = info.port.unwrap_or(DEFAULT_PORT);
    format!("http://{host}:{port}/mcp")
}

fn runtime_token_path(project_root: &Path, info: &RuntimeInfo) -> PathBuf {
    if info.token_file.is_absolute() {
        info.token_file.clone()
    } else {
        project_root.join(&info.token_file)
    }
}

fn build_http_headers(
    project_root: &Path,
    info: &RuntimeInfo,
) -> Result<HttpHeaders, Box<dyn std::error::Error>> {
    let token_path = runtime_token_path(project_root, info);
    let token = fs::read_to_string(&token_path)?.trim().to_string();
    if token.is_empty() {
        return Err(
            io::Error::other(format!("token file '{}' is empty", token_path.display())).into(),
        );
    }

    Ok(HttpHeaders {
        authorization: format!("Bearer {token}"),
        owner_cc_pid: owner_cc_pid(),
    })
}

fn headers_to_json(headers: &HttpHeaders) -> Value {
    let mut map = Map::new();
    map.insert("Authorization".into(), json!(headers.authorization));
    if let Some(pid) = headers.owner_cc_pid {
        map.insert("X-Team-Mode-Owner-CC-Pid".into(), json!(pid));
    }
    Value::Object(map)
}

fn http_client() -> Result<reqwest::blocking::Client, Box<dyn std::error::Error>> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?)
}

fn spawn_service_detached(spec: &RelaySpawnSpec) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new(&spec.service_exe);
    command
        .arg("--project-root")
        .arg(&spec.project_root)
        .arg("--port")
        .arg(spec.port.to_string())
        .current_dir(&spec.project_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(data_dir) = &spec.data_dir_override {
        command.arg("--data-dir").arg(data_dir);
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        command.creation_flags(
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
        );
    }

    match command.spawn() {
        Ok(_child) => Ok(()),
        #[cfg(target_os = "windows")]
        Err(err) if err.raw_os_error() == Some(5) => {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            tracing::warn!(
                "relay spawn with CREATE_BREAKAWAY_FROM_JOB denied; retrying without it"
            );
            let mut retry = Command::new(&spec.service_exe);
            retry
                .arg("--project-root")
                .arg(&spec.project_root)
                .arg("--port")
                .arg(spec.port.to_string())
                .current_dir(&spec.project_root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(data_dir) = &spec.data_dir_override {
                retry.arg("--data-dir").arg(data_dir);
            }
            retry.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
            retry.spawn().map(|_child| ()).map_err(|err| {
                io::Error::other(format!("failed to spawn team_mode_service: {err}")).into()
            })
        }
        Err(err) => {
            Err(io::Error::other(format!("failed to spawn team_mode_service: {err}")).into())
        }
    }
}

fn forward_json_rpc_message(
    client: &reqwest::blocking::Client,
    service_url: &str,
    headers: &HttpHeaders,
    payload: Value,
) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    let mut request = client.post(service_url).json(&payload);
    request = request.header("Authorization", &headers.authorization);
    if let Some(pid) = headers.owner_cc_pid {
        request = request.header("X-Team-Mode-Owner-CC-Pid", pid.to_string());
    }
    let response = request.send()?;
    let status = response.status();
    if status.as_u16() == 202 {
        return Ok(None);
    }
    let text = response.text()?;
    if !status.is_success() {
        return Err(io::Error::other(format!("HTTP {}: {text}", status)).into());
    }
    if text.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&text)?))
}

fn read_json_file(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content).map_err(|err| {
        io::Error::other(format!(
            "failed to parse JSON file '{}': {err}",
            path.display()
        ))
    })?)
}

fn json_contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(s) => s.contains(needle),
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains_string(value, needle)),
        Value::Object(map) => map
            .values()
            .any(|value| json_contains_string(value, needle)),
        _ => false,
    }
}

fn read_hook_event() -> Option<Value> {
    let mut raw = String::new();
    if io::stdin().read_to_string(&mut raw).is_err() {
        return None;
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn fetch_my_teams(
    client: &reqwest::blocking::Client,
    service_url: &str,
    headers: &HttpHeaders,
    session_id: Option<&str>,
) -> Result<MyTeamsResponse, Box<dyn std::error::Error>> {
    // `service_url` is the MCP endpoint (e.g. `http://127.0.0.1:8786/mcp`)
    // stored in runtime JSON for stdio relay forwarding. The /lead-pending
    // routes hang off the service base, NOT under /mcp, so strip any /mcp
    // suffix before appending. Without this the request 404s and async-wake
    // exits before draining any pending replies.
    let base = service_url
        .trim_end_matches('/')
        .strip_suffix("/mcp")
        .unwrap_or_else(|| service_url.trim_end_matches('/'));
    let url = format!("{base}/lead-pending/my-teams");
    let mut request = client
        .get(url)
        .query(&[("pid", std::process::id().to_string())]);
    if let Some(session_id) = session_id {
        request = request.query(&[("session_id", session_id)]);
    }
    request = request.header("Authorization", &headers.authorization);
    let response = request.send()?;
    let status = response.status();
    let text = response.text()?;
    if !status.is_success() {
        return Err(io::Error::other(format!("HTTP {}: {text}", status)).into());
    }
    Ok(serde_json::from_str(&text)?)
}

fn fetch_my_teams_checked(
    client: &reqwest::blocking::Client,
    service_url: &str,
    headers: &HttpHeaders,
    session_id: Option<&str>,
    hook_name: &str,
) -> Result<MyTeamsResponse, HookExit> {
    fetch_my_teams(client, service_url, headers, session_id).map_err(|err| HookExit {
        code: 1,
        message: format!("{hook_name}: /my-teams query failed: {err}"),
    })
}

fn identity_cache_path(project_root: &Path, session_id: &str) -> PathBuf {
    let safe = session_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .take(80)
        .collect::<String>();
    project_root
        .join(".agent-teams")
        .join(format!(".cc-identity.{safe}.json"))
}

fn read_identity_cache(
    project_root: &Path,
    session_id: &str,
) -> Result<Option<IdentityCache>, Box<dyn std::error::Error>> {
    let path = identity_cache_path(project_root, session_id);
    if !path.exists() {
        return Ok(None);
    }
    let value: IdentityCache =
        serde_json::from_str(&fs::read_to_string(&path)?).map_err(|err| {
            io::Error::other(format!(
                "failed to parse identity cache '{}': {err}",
                path.display()
            ))
        })?;
    if value.session_id == session_id {
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

fn resolve_mid_turn_identity(
    project_root: &Path,
    session_id: &str,
    client: &reqwest::blocking::Client,
    service_url: &str,
    headers: &HttpHeaders,
) -> Result<IdentityCache, HookExit> {
    if let Ok(Some(cache)) = read_identity_cache(project_root, session_id) {
        return Ok(cache);
    }

    let result = fetch_my_teams_checked(
        client,
        service_url,
        headers,
        Some(session_id),
        "lead-pending-mid-turn",
    )?;
    let cache = IdentityCache {
        session_id: session_id.to_string(),
        cached_at: chrono::Utc::now().to_rfc3339(),
        cc_pid: result.cc_pid,
        teams: result.teams,
    };
    let _ = write_identity_cache(project_root, &cache);
    Ok(cache)
}

fn write_identity_cache(
    project_root: &Path,
    cache: &IdentityCache,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = identity_cache_path(project_root, &cache.session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(cache)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn drain_pending_file(pending_path: &Path) -> Vec<Value> {
    if !pending_path.exists() {
        return Vec::new();
    }
    let tmp = pending_path.with_extension(format!(
        "draining-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    ));
    if fs::rename(pending_path, &tmp).is_err() {
        return Vec::new();
    }
    let raw = fs::read_to_string(&tmp).unwrap_or_default();
    let _ = fs::remove_file(&tmp);
    raw.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else {
                serde_json::from_str::<Value>(trimmed).ok()
            }
        })
        .collect()
}

fn render_async_wake_reminder(entries: &[Value]) -> String {
    let banner = "─── [TEAM-MODE] 新团队消息 ───";
    let footer = "─── 你可以继续手头的任务 ───";
    if entries.len() == 1 {
        return render_entry(banner, footer, &entries[0], None);
    }
    let blocks = entries
        .iter()
        .map(|entry| render_entry("", "", entry, Some(false)))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    format!(
        "{banner}\n\n共 {} 条新消息：\n\n{blocks}\n\n{footer}\n",
        entries.len()
    )
}

fn render_mid_turn_reminder(entries: &[Value]) -> String {
    let banner = "─── [TEAM-MODE] mid-turn 团队消息（worker 主动推送，可稍后响应）───";
    let footer = "─── 你可以继续手头的任务；turn 结束时如果未回应会再次提醒 ───";
    if entries.len() == 1 {
        return render_entry(banner, footer, &entries[0], None);
    }
    let blocks = entries
        .iter()
        .map(|entry| render_entry("", "", entry, Some(false)))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    format!(
        "{banner}\n\n共 {} 条新消息：\n\n{blocks}\n\n{footer}\n",
        entries.len()
    )
}

fn render_entry(
    banner: &str,
    footer: &str,
    entry: &Value,
    suppress_banner_footer: Option<bool>,
) -> String {
    let team = entry.get("team").and_then(Value::as_str).unwrap_or("?");
    let from = entry
        .get("from")
        .or_else(|| entry.get("from_id"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    let kind = entry
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("message");
    let text = entry
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let body = format!("[team={team}] {from} ({kind}):\n{text}");
    if suppress_banner_footer == Some(false) {
        return body;
    }
    format!("{banner}\n\n{body}\n\n{footer}\n")
}

fn owner_cc_pid() -> Option<u32> {
    snapshot_process_tree()
        .ok()
        .and_then(|tree| owner_cc_pid_from_tree(&tree, std::process::id()))
}

fn snapshot_process_tree() -> Result<HashMap<u32, ProcessRow>, Box<dyn std::error::Error>> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut map = HashMap::new();
    for (pid, proc) in sys.processes() {
        let name = proc.name().to_string_lossy().to_string();
        let ppid = proc.parent().map(|value| value.as_u32()).unwrap_or(0);
        map.insert(pid.as_u32(), ProcessRow { ppid, name });
    }
    Ok(map)
}

fn owner_cc_pid_from_tree(tree: &HashMap<u32, ProcessRow>, start_pid: u32) -> Option<u32> {
    let mut seen = HashSet::new();
    let mut pid = start_pid;
    for _ in 0..MAX_OWNER_WALK_DEPTH {
        if !seen.insert(pid) {
            return None;
        }
        let current = tree.get(&pid)?;
        if current.ppid == 0 {
            return None;
        }
        let parent = tree.get(&current.ppid)?;
        let stem = parent
            .name
            .to_lowercase()
            .trim_end_matches(".exe")
            .to_string();
        if !SHELL_WRAPPER_NAMES.contains(&stem.as_str()) {
            return Some(current.ppid);
        }
        pid = current.ppid;
    }
    None
}

fn write_json_rpc_message<W, T>(
    writer: &mut W,
    payload: &T,
) -> Result<(), Box<dyn std::error::Error>>
where
    W: Write,
    T: Serialize,
{
    let body = serde_json::to_vec(payload)?;
    writer.write_all(&body)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn read_json_rpc_message<R>(reader: &mut R) -> Result<Option<Value>, Box<dyn std::error::Error>>
where
    R: BufRead + Read,
{
    let mut first_line = String::new();
    loop {
        first_line.clear();
        let bytes = reader.read_line(&mut first_line)?;
        if bytes == 0 {
            return Ok(None);
        }
        if first_line.trim().is_empty() {
            continue;
        }
        break;
    }

    if first_line.trim_start().starts_with('{') || first_line.trim_start().starts_with('[') {
        return Ok(Some(serde_json::from_str(first_line.trim())?));
    }

    let mut content_length = parse_content_length_header(&first_line)?;
    let mut header = String::new();
    loop {
        header.clear();
        let bytes = reader.read_line(&mut header)?;
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected EOF while reading JSON-RPC headers",
            )
            .into());
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if header.to_ascii_lowercase().starts_with("content-length:") {
            content_length = Some(
                header
                    .split_once(':')
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length header")
                    })?,
            );
        }
    }

    let length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

fn parse_content_length_header(line: &str) -> Result<Option<usize>, Box<dyn std::error::Error>> {
    if line.to_ascii_lowercase().starts_with("content-length:") {
        Ok(Some(
            line.split_once(':')
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length header")
                })?,
        ))
    } else if line.contains(':') {
        Ok(None)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid JSON-RPC transport header or body",
        )
        .into())
    }
}

fn exit_with_error(err: Box<dyn std::error::Error>) -> ! {
    eprintln!("team_mode_service: {err}");
    std::process::exit(1)
}

fn exit_with_hook_error(err: Box<dyn std::error::Error>) -> ! {
    eprintln!("team_mode_service hook: {err}");
    std::process::exit(1)
}

fn exit_hook_error(code: i32, message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(code)
}

#[derive(Debug)]
struct HookExit {
    code: i32,
    message: String,
}

#[cfg(test)]
mod tests;
