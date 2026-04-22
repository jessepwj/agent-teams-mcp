use std::env;

use clap::Parser;
use team_mode_native::mcp::{McpProxyConfig, run_mcp_proxy};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:17891")]
    host: String,
    #[arg(long)]
    member_id: String,
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
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let token = args
        .token_env
        .as_deref()
        .and_then(|name| env::var(name).ok())
        .filter(|value| !value.is_empty());
    tracing::info!(
        host = %args.host,
        member_id = %args.member_id,
        "team_mode_mcp_proxy starting"
    );
    run_mcp_proxy(McpProxyConfig {
        host: args.host,
        member_id: args.member_id,
        token,
    })
    .await?;
    Ok(())
}
