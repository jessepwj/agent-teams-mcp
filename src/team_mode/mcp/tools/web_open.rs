use std::sync::OnceLock;

use serde_json::{Value, json};

const DEFAULT_WEB_HOST: &str = "127.0.0.1";
const DEFAULT_WEB_PORT: u16 = 8787;
const MAX_WEB_PORT: u16 = 8799;

#[derive(Debug, Clone)]
pub(super) struct TeamWebStatus {
    enabled: bool,
    url: Option<String>,
    opened: bool,
    error: Option<String>,
}

impl TeamWebStatus {
    fn disabled(reason: impl Into<String>) -> Self {
        Self {
            enabled: false,
            url: None,
            opened: false,
            error: Some(reason.into()),
        }
    }

    pub(super) fn to_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "url": self.url,
            "opened": self.opened,
            "error": self.error,
        })
    }
}

static WEB_SERVER_URL: OnceLock<String> = OnceLock::new();

pub(super) fn open_team_web_ui(base_dir: &std::path::Path, team_id: &str) -> TeamWebStatus {
    if let Some(reason) = web_auto_open_disabled_reason() {
        return TeamWebStatus::disabled(reason);
    }

    let base_url = match ensure_team_web_server(base_dir) {
        Ok(url) => url,
        Err(err) => {
            return TeamWebStatus {
                enabled: true,
                url: None,
                opened: false,
                error: Some(err),
            };
        }
    };
    let url = format!("{base_url}/#team={team_id}");

    match open_url_in_browser(&url) {
        Ok(()) => TeamWebStatus {
            enabled: true,
            url: Some(url),
            opened: true,
            error: None,
        },
        Err(err) => TeamWebStatus {
            enabled: true,
            url: Some(url),
            opened: false,
            error: Some(err),
        },
    }
}

fn web_auto_open_disabled_reason() -> Option<String> {
    match std::env::var("TEAM_MODE_WEB_AUTO_OPEN") {
        Ok(value) if matches!(value.as_str(), "0" | "false" | "FALSE" | "off" | "OFF") => {
            return Some("TEAM_MODE_WEB_AUTO_OPEN disabled".into());
        }
        _ => {}
    }

    if std::env::var_os("CI").is_some() || looks_like_cargo_test_process() {
        return Some("disabled in test/CI process".into());
    }

    None
}

fn looks_like_cargo_test_process() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let in_deps_dir = exe
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("deps");
    let has_cargo_test_hash = exe
        .file_stem()
        .and_then(|name| name.to_str())
        .map(|name| name.rsplit_once('-').is_some())
        .unwrap_or(false);
    in_deps_dir && has_cargo_test_hash
}

/// Public wrapper so the daemon binary can pre-spawn the web server at
/// startup - otherwise the web server only exists inside the ephemeral
/// worker thread spawned during `team_create`. When the daemon restarts,
/// any previously-opened browser tab has stale API URLs and sees
/// ERR_CONNECTION_REFUSED until the user runs another team_create.
pub fn ensure_team_web_server_public(
    base_dir: &std::path::Path,
) -> std::result::Result<String, String> {
    ensure_team_web_server(base_dir)
}

fn ensure_team_web_server(base_dir: &std::path::Path) -> std::result::Result<String, String> {
    if let Some(url) = WEB_SERVER_URL.get() {
        return Ok(url.clone());
    }

    let mut last_error = None;
    for port in DEFAULT_WEB_PORT..=MAX_WEB_PORT {
        let addr = format!("{DEFAULT_WEB_HOST}:{port}");
        match std::net::TcpListener::bind(&addr) {
            Ok(listener) => {
                let url = format!("http://{addr}");
                let base_dir = base_dir.to_path_buf();
                std::thread::Builder::new()
                    .name("team-mode-web".into())
                    .spawn(move || {
                        if let Err(err) = crate::team_mode_web::serve_listener(base_dir, listener) {
                            tracing::warn!(error = %err, "team_mode_web server exited");
                        }
                    })
                    .map_err(|err| format!("failed to spawn team_mode_web thread: {err}"))?;
                let _ = WEB_SERVER_URL.set(url.clone());
                return Ok(url);
            }
            Err(err) => {
                last_error = Some(format!("{addr}: {err}"));
            }
        }
    }

    Err(format!(
        "could not bind Team Mode Web on ports {DEFAULT_WEB_PORT}-{MAX_WEB_PORT}: {}",
        last_error.unwrap_or_else(|| "no bind attempt completed".into())
    ))
}

fn open_url_in_browser(url: &str) -> std::result::Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("failed to open browser: {err}"))
}
