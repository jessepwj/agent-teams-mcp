use std::env;
use std::path::PathBuf;

use clap::Parser;
use team_mode_native::host::{LocalIpcConfig, TeamModeHost, run_local_ipc};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = ".team-mode")]
    data_dir: PathBuf,
    #[arg(long, default_value = "127.0.0.1:17891")]
    listen: String,
    #[arg(long)]
    token_env: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .init();
    let args = Args::parse();
    let token = args
        .token_env
        .as_deref()
        .and_then(|name| env::var(name).ok())
        .filter(|value| !value.is_empty());
    // Canonicalize data_dir to absolute so runners (which start in a different cwd) see correct paths.
    let data_dir = if args.data_dir.is_relative() {
        std::env::current_dir()?.join(&args.data_dir)
    } else {
        args.data_dir
    };
    let host = TeamModeHost::new(data_dir);
    tracing::info!(listen = %args.listen, "team_mode_host starting");
    run_local_ipc(
        host,
        LocalIpcConfig {
            listen: args.listen,
            token,
        },
    )
    .await?;
    Ok(())
}
