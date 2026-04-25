use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use super::routes;
use super::state::TeamModeWebState;

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
    let mut buf = [0_u8; 16 * 1024];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let mut lines = request.lines();
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or("/");
    let response = app.handle_request(method, target);
    let elapsed_ms = started.elapsed().as_millis();

    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status as u16,
        response.status.reason_phrase(),
        response.content_type,
        response.body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()?;
    eprintln!(
        "[team_mode_web] {method} {target} -> {} in {elapsed_ms}ms",
        response.status as u16
    );
    Ok(())
}
