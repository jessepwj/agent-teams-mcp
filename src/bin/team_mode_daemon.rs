use std::env;
use std::path::PathBuf;

use agent_teams::team_mode_daemon::serve_daemon;
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut data_dir: Option<PathBuf> = None;
    let mut project_root: Option<PathBuf> = None;
    let mut token: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => {
                let Some(value) = args.next() else {
                    return Err("--data-dir requires a value".into());
                };
                data_dir = Some(PathBuf::from(value));
            }
            "--project-root" => {
                let Some(value) = args.next() else {
                    return Err("--project-root requires a value".into());
                };
                project_root = Some(PathBuf::from(value));
            }
            "--token" => {
                let Some(value) = args.next() else {
                    return Err("--token requires a value".into());
                };
                token = Some(value);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let data_dir = data_dir.ok_or("--data-dir is required")?;
    let project_root = project_root.unwrap_or(env::current_dir()?);
    let token = token.ok_or("--token is required")?;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| EnvFilter::new("info,agent_teams=debug")),
        )
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_thread_ids(false)
        .init();

    // Absorb console control events. Daemon was spawned from MCP with
    // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP, which SHOULD isolate it
    // from CC's console broadcasts — but on Windows, Job Object inheritance
    // can still cause the daemon to be killed when CC terminates its job
    // (unless CREATE_BREAKAWAY_FROM_JOB was also set; we try, but it can
    // fail if the parent job doesn't permit breakaway).
    //
    // This absorber is a second line of defense: if any CTRL_C_EVENT or
    // CTRL_BREAK_EVENT does reach the daemon, we eat it and keep serving.
    // Normal shutdown paths are: stdin never closes (we don't have stdin),
    // daemon/shutdown RPC explicit, or the OS force-killing us.
    std::thread::Builder::new()
        .name("daemon-ctrl-c-absorber".into())
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
                    tracing::info!("daemon: Ctrl+C absorbed");
                }
            });
        })
        .ok();

    serve_daemon(data_dir, project_root, token)?;
    Ok(())
}
