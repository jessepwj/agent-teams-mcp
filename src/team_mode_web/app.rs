use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::routes;
use super::sse::{self, SseConfig};
use super::state::{StaticBundleMode, TeamModeWebState};

/// Suffix appended to a project_root to get its `.agent-teams/` data dir.
const AGENT_TEAMS_DIR_NAME: &str = ".agent-teams";

/// Cap incoming POST bodies. Messages are short text + a small mentions
/// array, so 256 KiB is generous and prevents a misbehaving client from
/// holding the worker thread on a multi-MB POST.
const MAX_REQUEST_BODY_BYTES: usize = 256 * 1024;

/// Cap how long we wait reading subsequent body chunks. The first read is
/// blocking on the kernel buffer; if a later chunk doesn't arrive within
/// this window we abort instead of stalling the request thread.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct TeamModeWebApp {
    default_state: Arc<TeamModeWebState>,
    /// Pinned base_dir captured at startup. Becomes the parent of every
    /// per-project state when `?project=` resolves to its project_root.
    /// Also the fallback when the request omits the project query.
    default_base_dir: PathBuf,
    project_states: Arc<Mutex<HashMap<PathBuf, Arc<TeamModeWebState>>>>,
    config: TeamModeWebServerConfig,
}

impl TeamModeWebApp {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self::with_config(base_dir, TeamModeWebServerConfig::default())
    }

    pub fn with_config(base_dir: impl Into<PathBuf>, config: TeamModeWebServerConfig) -> Self {
        let base_dir: PathBuf = base_dir.into();
        let default_state = Arc::new(TeamModeWebState::with_session_home_and_static_bundle(
            base_dir.clone(),
            config.session_home.clone(),
            config.static_bundle.clone(),
        ));
        Self {
            default_state,
            default_base_dir: base_dir,
            project_states: Arc::new(Mutex::new(HashMap::new())),
            config,
        }
    }

    /// Look up the per-project `TeamModeWebState`. When `project_root` is
    /// `None`, returns the startup default (matches legacy behavior). When
    /// `Some(path)`, returns a cached state pinned to
    /// `<path>/.agent-teams/`, constructing it lazily on first hit.
    ///
    /// BUG-7 fix: the web service used to be hard-pinned to the project that
    /// lazy-spawned it (whichever CC reconnected first). A second CC in a
    /// different project would open the team_create-emitted URL and see
    /// `{teams: []}` even though its data was on disk. Per-request project
    /// resolution lets both CCs share one web port and still see their own
    /// data.
    pub(crate) fn resolve_state(
        &self,
        project_root: Option<&Path>,
    ) -> Arc<TeamModeWebState> {
        let Some(project_root) = project_root else {
            return Arc::clone(&self.default_state);
        };
        // If the requested project_root would resolve to the same base_dir
        // we were started with, just hand back the pinned default state.
        // Avoids constructing a duplicate TeamModeWebState (and a duplicate
        // SHARED_MESSAGE_SERVICE-aware MessageService) for the common case.
        let target_base = project_root.join(AGENT_TEAMS_DIR_NAME);
        if same_path(&target_base, &self.default_base_dir) {
            return Arc::clone(&self.default_state);
        }
        let mut cache = self.project_states.lock().expect("project_states mutex");
        if let Some(state) = cache.get(&target_base) {
            return Arc::clone(state);
        }
        // BUG-12: must use `for_project` (not `with_session_home_and_static_bundle`)
        // here. The latter consults `SHARED_MESSAGE_SERVICE`, which is pinned to
        // the daemon's startup base_dir and therefore reads/writes the wrong
        // project's `messages.jsonl` for any non-default project. `for_project`
        // forces a fresh MessageService rooted at the per-project base_dir.
        let state = Arc::new(TeamModeWebState::for_project(
            target_base.clone(),
            self.config.session_home.clone(),
            self.config.static_bundle.clone(),
        ));
        cache.insert(target_base, Arc::clone(&state));
        state
    }

    pub fn handle_request(&self, method: &str, target: &str) -> routes::WebResponse {
        let project_root = project_root_from_query(target);
        let state = self.resolve_state(project_root.as_deref());
        routes::handle_request(&state, method, target)
    }

    pub fn handle_request_with_body(
        &self,
        method: &str,
        target: &str,
        body: &[u8],
    ) -> routes::WebResponse {
        let project_root = project_root_from_query(target);
        let state = self.resolve_state(project_root.as_deref());
        routes::handle_request_with_body(&state, method, target, body)
    }

    pub fn handle_request_with_project(
        &self,
        method: &str,
        target: &str,
        body: &[u8],
        project_root_header: Option<&str>,
    ) -> routes::WebResponse {
        let project_root = project_root_from_query(target)
            .or_else(|| project_root_header.map(PathBuf::from));
        let state = self.resolve_state(project_root.as_deref());
        routes::handle_request_with_body(&state, method, target, body)
    }
}

/// Extract `?project=<urlencoded path>` from a request target like
/// `/api/teams?project=E%3A%5Caigc%5Copencode`. Returns the decoded path
/// (using percent-encoding round-trip) or `None` when absent.
fn project_root_from_query(target: &str) -> Option<PathBuf> {
    let (_, query) = target.split_once('?')?;
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("project=") {
            let decoded = url_decode(value);
            if !decoded.is_empty() {
                return Some(PathBuf::from(decoded));
            }
        }
    }
    None
}

fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = decode_hex(bytes[i + 1]);
                let lo = decode_hex(bytes[i + 2]);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn decode_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        p.to_string_lossy().replace('/', "\\").to_lowercase()
    }
    norm(a) == norm(b)
}

#[derive(Debug, Clone)]
pub struct TeamModeWebServerConfig {
    pub sse: SseConfig,
    pub max_connections: Option<usize>,
    pub session_home: Option<PathBuf>,
    /// Internal extension point for tests/embedding/dev tooling. Default
    /// production behavior is `Baked`; dev mode must be explicitly selected.
    pub static_bundle: StaticBundleMode,
}

impl Default for TeamModeWebServerConfig {
    fn default() -> Self {
        Self {
            sse: SseConfig::default(),
            max_connections: None,
            session_home: None,
            static_bundle: StaticBundleMode::from_env(),
        }
    }
}

/// Construct the read-only Team Mode Web application.
///
/// Kept as `router` for the public API name used by the initial plan, but this
/// is intentionally not tied to a web framework. The implementation uses a
/// tiny std-only HTTP server so the feature can compile without downloading
/// optional dashboard dependencies.
pub fn router(base_dir: impl Into<PathBuf>) -> TeamModeWebApp {
    TeamModeWebApp::new(base_dir)
}

pub fn serve(base_dir: impl Into<PathBuf>, addr: SocketAddr) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    serve_listener(base_dir, listener)
}

pub fn serve_listener(base_dir: impl Into<PathBuf>, listener: TcpListener) -> std::io::Result<()> {
    serve_listener_with_config(base_dir, listener, TeamModeWebServerConfig::default())
}

pub fn serve_listener_with_config(
    base_dir: impl Into<PathBuf>,
    listener: TcpListener,
    config: TeamModeWebServerConfig,
) -> std::io::Result<()> {
    let app = TeamModeWebApp::with_config(base_dir, config.clone());
    let mut accepted = 0_usize;
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                accepted += 1;
                let app = app.clone();
                std::thread::spawn(move || {
                    let _ = handle_stream(app, stream);
                });
                if config
                    .max_connections
                    .map(|max| accepted >= max)
                    .unwrap_or(false)
                {
                    break;
                }
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn handle_stream(app: TeamModeWebApp, mut stream: TcpStream) -> std::io::Result<()> {
    let started = Instant::now();

    // Read the first chunk; this typically captures the entire request for
    // GETs and small POSTs in one syscall. We then parse the headers, find
    // Content-Length, and (for POSTs whose body wasn't fully delivered yet)
    // pull in the rest with a read timeout so a slow client can't tie up
    // the thread indefinitely.
    let mut raw = Vec::with_capacity(8 * 1024);
    let mut buf = [0_u8; 16 * 1024];
    let n = stream.read(&mut buf)?;
    if n == 0 {
        return Ok(());
    }
    raw.extend_from_slice(&buf[..n]);

    // Find end-of-headers marker; if it isn't here yet (large header set?
    // very partial POST?) keep reading until either we find it or we
    // exceed the body cap.
    let mut headers_end = find_headers_end(&raw);
    while headers_end.is_none() && raw.len() < MAX_REQUEST_BODY_BYTES {
        stream.set_read_timeout(Some(REQUEST_READ_TIMEOUT))?;
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        headers_end = find_headers_end(&raw);
    }
    let headers_end = headers_end.unwrap_or(raw.len());

    let header_text = String::from_utf8_lossy(&raw[..headers_end]);
    let mut lines = header_text.lines();
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let headers = lines
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key.trim().to_string(), value.trim().to_string()))
        })
        .collect::<Vec<_>>();

    // Parse Content-Length to know how many body bytes to expect (after the
    // \r\n\r\n boundary). 0 / missing → no body.
    let content_length = headers
        .iter()
        .filter_map(|(key, value)| {
            if key.eq_ignore_ascii_case("content-length") {
                value.parse::<usize>().ok()
            } else {
                None
            }
        })
        .next()
        .unwrap_or(0)
        .min(MAX_REQUEST_BODY_BYTES);

    // Slurp body. headers_end is where \r\n\r\n ENDS; bytes after that are
    // body. The first read often already includes some/all of it.
    let body_start = headers_end;
    let already = raw.len().saturating_sub(body_start);
    while raw.len().saturating_sub(body_start) < content_length
        && raw.len() < MAX_REQUEST_BODY_BYTES
    {
        stream.set_read_timeout(Some(REQUEST_READ_TIMEOUT))?;
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    let body_end = (body_start + content_length).min(raw.len());
    let body_slice = &raw[body_start..body_end];

    if content_length > MAX_REQUEST_BODY_BYTES {
        let response = routes::WebResponse::json(
            super::error::StatusCode::BadRequest,
            &super::error::ErrorBody {
                error: format!(
                    "request body too large ({content_length} bytes; cap is {MAX_REQUEST_BODY_BYTES})"
                ),
            },
        );
        write_response(&mut stream, &response)?;
        eprintln!(
            "[team_mode_web] {method} {target} -> 400 (body cap) read={already}",
            method = method,
            target = target,
            already = already
        );
        return Ok(());
    }

    // Pull `X-Team-Mode-Project-Root` header (case-insensitive) for the
    // multi-project router. The query string is preferred (matches the URL
    // the team_create response embeds), but the header is honored as a
    // backup so tools that prefer headers (curl, programmatic clients)
    // still route correctly. BUG-7.
    let project_root_header = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("x-team-mode-project-root"))
        .map(|(_, value)| value.clone());

    if method == "GET" {
        if let Some(stream_request) = parse_sse_request(&target, &headers) {
            // SSE also picks the per-project state so the streamed events
            // come from the right project's room/inbox stores.
            let sse_project_root = project_root_from_query(&target)
                .or_else(|| project_root_header.as_deref().map(PathBuf::from));
            let state = app.resolve_state(sse_project_root.as_deref());
            if let Err(err) = routes::validate_events_cursor(
                &state,
                &stream_request.team_id,
                stream_request.initial_cursor.as_deref(),
            ) {
                let response = routes::error_response(err);
                write_response(&mut stream, &response)?;
                return Ok(());
            }
            let elapsed_ms = started.elapsed().as_millis();
            eprintln!("[team_mode_web] {method} {target} -> 200 stream in {elapsed_ms}ms");
            return sse::stream_events(
                state,
                stream,
                stream_request.team_id,
                stream_request.initial_cursor,
                app.config.sse.clone(),
            );
        }
    }

    let response = app.handle_request_with_project(
        &method,
        &target,
        body_slice,
        project_root_header.as_deref(),
    );
    let elapsed_ms = started.elapsed().as_millis();
    write_response(&mut stream, &response)?;
    eprintln!(
        "[team_mode_web] {method} {target} -> {} in {elapsed_ms}ms",
        response.status as u16
    );
    Ok(())
}

#[derive(Debug)]
struct SseRequest {
    team_id: String,
    initial_cursor: Option<String>,
}

fn parse_sse_request(target: &str, headers: &[(String, String)]) -> Option<SseRequest> {
    let (path, query) = split_target(target);
    let segments = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let ["api", "teams", team, "events", "stream"] = segments.as_slice() else {
        return None;
    };
    let header_cursor = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("last-event-id"))
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty());
    Some(SseRequest {
        team_id: (*team).to_string(),
        initial_cursor: header_cursor.or_else(|| query_param(query, "cursor")),
    })
}

fn split_target(target: &str) -> (&str, Option<&str>) {
    target
        .split_once('?')
        .map(|(path, query)| (path, Some(query)))
        .unwrap_or((target, None))
}

fn query_param(query: Option<&str>, name: &str) -> Option<String> {
    let query = query?;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == name && !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn write_response(stream: &mut TcpStream, response: &routes::WebResponse) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status as u16,
        response.status.reason_phrase(),
        response.content_type,
        response.body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

#[cfg(test)]
mod multi_project_tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn project_root_from_query_extracts_urlencoded_path() {
        // Windows path with backslashes + colon — must round-trip through
        // %5C and %3A so the relay/CC can stash a path in `?project=`.
        let target = "/api/teams?project=E%3A%5Caigc%5Copencode";
        let parsed = project_root_from_query(target).unwrap();
        assert_eq!(parsed.to_string_lossy(), r"E:\aigc\opencode");
    }

    #[test]
    fn project_root_from_query_returns_none_when_absent() {
        assert!(project_root_from_query("/api/teams").is_none());
        assert!(project_root_from_query("/api/teams?other=value").is_none());
        assert!(project_root_from_query("/").is_none());
    }

    #[test]
    fn project_root_from_query_decodes_chinese_path() {
        // BUG-7 + the v3.1 chinese-path bug we fixed earlier on the HTTP MCP
        // service: per-project routing must not lose data when paths
        // contain CJK characters (Windows users routinely have those).
        let target = "/api/teams?project=E%3A%5Caigc%E5%86%85%E5%AE%B9%E6%95%B4%E7%90%86%5Copencode";
        let parsed = project_root_from_query(target).unwrap();
        assert_eq!(parsed.to_string_lossy(), r"E:\aigc内容整理\opencode");
    }

    #[test]
    fn resolve_state_returns_default_for_no_query() {
        let dir = tempdir().unwrap();
        let app = TeamModeWebApp::new(dir.path().join(".agent-teams"));
        let resolved = app.resolve_state(None);
        // Both pointers identify the same Arc.
        assert!(Arc::ptr_eq(&resolved, &app.default_state));
    }

    #[test]
    fn resolve_state_caches_per_project() {
        let dir = tempdir().unwrap();
        // Pretend two project roots that are different from the startup one.
        let project_b = dir.path().join("other-project-b");
        let project_c = dir.path().join("other-project-c");
        fs::create_dir_all(project_b.join(".agent-teams")).unwrap();
        fs::create_dir_all(project_c.join(".agent-teams")).unwrap();

        let app = TeamModeWebApp::new(dir.path().join(".agent-teams"));
        let b1 = app.resolve_state(Some(&project_b));
        let b2 = app.resolve_state(Some(&project_b));
        let c1 = app.resolve_state(Some(&project_c));

        // Same project resolves to same cached state.
        assert!(Arc::ptr_eq(&b1, &b2));
        // Different project resolves to a different state.
        assert!(!Arc::ptr_eq(&b1, &c1));
        // Neither is the startup default.
        assert!(!Arc::ptr_eq(&b1, &app.default_state));
        assert!(!Arc::ptr_eq(&c1, &app.default_state));
    }

    #[test]
    fn resolve_state_short_circuits_to_default_when_project_matches_startup() {
        // If `?project=` happens to resolve to the same on-disk base_dir as
        // the startup default, resolve_state should hand back the pinned
        // default state — not construct a duplicate. This avoids splitting
        // shared message-service state across two seemingly-different
        // states that point at the same files.
        let dir = tempdir().unwrap();
        let project_root = dir.path().to_path_buf();
        fs::create_dir_all(project_root.join(".agent-teams")).unwrap();
        let app = TeamModeWebApp::new(project_root.join(".agent-teams"));
        let resolved = app.resolve_state(Some(&project_root));
        assert!(Arc::ptr_eq(&resolved, &app.default_state));
    }
}
