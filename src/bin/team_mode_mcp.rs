use std::env;
use std::path::PathBuf;

use agent_teams::TeamModeMcpRuntime;
use agent_teams::team_mode::data_dir;
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    tracing::info!("Team Mode MCP server starting");

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
    tracing::info!(data_dir = %data_dir.display(), "using data directory");

    let mut runtime = TeamModeMcpRuntime::new(data_dir);
    runtime.run_stdio()?;
    Ok(())
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
            tracing::info!(
                "CLAUDE_CODE_GIT_BASH_PATH auto-detected: {}",
                candidate
            );
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
