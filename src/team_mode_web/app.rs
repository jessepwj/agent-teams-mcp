use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::routes;
use super::state::TeamModeWebState;

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
    state: Arc<TeamModeWebState>,
}

impl TeamModeWebApp {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            state: Arc::new(TeamModeWebState::new(base_dir)),
        }
    }

    pub fn handle_request(&self, method: &str, target: &str) -> routes::WebResponse {
        routes::handle_request(&self.state, method, target)
    }

    pub fn handle_request_with_body(
        &self,
        method: &str,
        target: &str,
        body: &[u8],
    ) -> routes::WebResponse {
        routes::handle_request_with_body(&self.state, method, target, body)
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
    let app = TeamModeWebApp::new(base_dir);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let app = app.clone();
                std::thread::spawn(move || {
                    let _ = handle_stream(app, stream);
                });
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

    // Parse Content-Length to know how many body bytes to expect (after the
    // \r\n\r\n boundary). 0 / missing → no body.
    let content_length = lines
        .filter_map(|line| {
            let (k, v) = line.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse::<usize>().ok()
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

    let response = app.handle_request_with_body(&method, &target, body_slice);
    let elapsed_ms = started.elapsed().as_millis();
    write_response(&mut stream, &response)?;
    eprintln!(
        "[team_mode_web] {method} {target} -> {} in {elapsed_ms}ms",
        response.status as u16
    );
    Ok(())
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
