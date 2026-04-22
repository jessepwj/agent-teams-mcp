use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use team_mode_native::runner::protocol::{
    ChildExitFrame, HostToRunnerFrame, InputInjectedFrame, OutputStream, RunnerFrame,
    RunnerHeartbeatFrame, RunnerHelloFrame, RunnerOutputFrame,
};
use team_mode_native::runner::{
    InjectionStrategy, RunnerControlClient, command_spec_from_parts, spawn_pty_bridge,
};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    member_id: String,
    #[arg(long)]
    runner_id: String,
    #[arg(long)]
    host: String,
    #[arg(long, default_value = "TEAM_MODE_RUNNER_TOKEN")]
    token_env: String,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long = "env")]
    env: Vec<String>,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
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
    if args.command.is_empty() {
        run_fake(args).await
    } else {
        run_pty(args).await
    }
}

async fn connect_and_hello(args: &Args) -> anyhow::Result<RunnerControlClient> {
    let token = std::env::var(&args.token_env).ok();
    tracing::info!(
        host = %args.host,
        member_id = %args.member_id,
        runner_id = %args.runner_id,
        "connecting to host"
    );
    let mut client = RunnerControlClient::connect(&args.host, token.clone()).await.map_err(|err| {
        tracing::error!(host = %args.host, error = %err, "failed to connect to host");
        err
    })?;
    tracing::info!(host = %args.host, member_id = %args.member_id, "connected, sending hello");
    client
        .send(&RunnerFrame::Hello(RunnerHelloFrame {
            member_id: args.member_id.clone(),
            runner_id: args.runner_id.clone(),
            protocol_version: 1,
            token,
            cwd: args.cwd.as_ref().map(|path| path.display().to_string()),
            pid: Some(std::process::id()),
        }))
        .await?;
    Ok(client)
}

async fn run_fake(args: Args) -> anyhow::Result<()> {
    let mut client = connect_and_hello(&args).await?;
    let mut last_heartbeat = Instant::now();
    eprintln!(
        "fake runner connected: member={} runner={}",
        args.member_id, args.runner_id
    );

    loop {
        if last_heartbeat.elapsed() >= Duration::from_secs(2) {
            send_heartbeat(&mut client, &args).await?;
            last_heartbeat = Instant::now();
        }
        match tokio::time::timeout(Duration::from_millis(200), client.recv()).await {
            Ok(Ok(Some(frame))) => match frame {
                HostToRunnerFrame::InjectInput(inject) => {
                    println!("{}", inject.text);
                    client
                        .send(&RunnerFrame::InputInjected(InputInjectedFrame {
                            member_id: args.member_id.clone(),
                            runner_id: args.runner_id.clone(),
                            injection_id: inject.injection_id,
                            ok: true,
                            error: None,
                        }))
                        .await?;
                }
            },
            Ok(Ok(None)) => break,
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => {}
        }
    }

    Ok(())
}

async fn send_heartbeat(client: &mut RunnerControlClient, args: &Args) -> anyhow::Result<()> {
    client
        .send(&RunnerFrame::Heartbeat(RunnerHeartbeatFrame {
            member_id: args.member_id.clone(),
            runner_id: args.runner_id.clone(),
            unix_ms: now_unix_ms(),
        }))
        .await?;
    Ok(())
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn parse_env(values: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    let mut env = Vec::new();
    for value in values {
        let Some((key, val)) = value.split_once('=') else {
            anyhow::bail!("--env must be KEY=VALUE, got {value}");
        };
        env.push((key.to_string(), val.to_string()));
    }
    Ok(env)
}

async fn run_pty(args: Args) -> anyhow::Result<()> {
    let mut client = connect_and_hello(&args).await?;
    let mut spec = command_spec_from_parts(&args.command, args.cwd.as_deref())?;
    spec.env = parse_env(&args.env)?;
    tracing::info!(
        member_id = %args.member_id,
        command = %args.command.join(" "),
        "spawning PTY bridge"
    );
    let mut bridge = spawn_pty_bridge(spec)?;
    tracing::info!(member_id = %args.member_id, "PTY bridge spawned");
    let stdin_input = bridge.input.clone();
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0_u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => { let _ = stdin_input.write_raw(&buf[..n]); }
                Err(_) => break,
            }
        }
    });
    let mut last_heartbeat = Instant::now();

    'runner: loop {
        if last_heartbeat.elapsed() >= Duration::from_secs(2) {
            send_heartbeat(&mut client, &args).await?;
            last_heartbeat = Instant::now();
        }

        while let Ok(event) = bridge.events.try_recv() {
            match event {
                team_mode_native::runner::pty_bridge::PtyEvent::Output(text) => {
                    print!("{text}");
                    let _ = io::stdout().flush();
                    let _ = client
                        .send(&RunnerFrame::Output(RunnerOutputFrame {
                            member_id: args.member_id.clone(),
                            runner_id: args.runner_id.clone(),
                            stream: OutputStream::Pty,
                            data: text,
                        }))
                        .await;
                }
                team_mode_native::runner::pty_bridge::PtyEvent::Exit { exit_code, success } => {
                    tracing::info!(
                        member_id = %args.member_id,
                        exit_code,
                        success,
                        "PTY child process exited"
                    );
                    client
                        .send(&RunnerFrame::ChildExit(ChildExitFrame {
                            member_id: args.member_id.clone(),
                            runner_id: args.runner_id.clone(),
                            exit_code,
                            success,
                        }))
                        .await?;
                    break 'runner;
                }
            }
        }

        match tokio::time::timeout(Duration::from_millis(100), client.recv()).await {
            Ok(Ok(Some(frame))) => match frame {
                HostToRunnerFrame::InjectInput(inject) => {
                    tracing::debug!(
                        member_id = %args.member_id,
                        injection_id = %inject.injection_id,
                        "injecting input"
                    );
                    let result = bridge
                        .input
                        .inject(&inject.text, InjectionStrategy::from(inject.strategy));
                    if let Err(ref err) = result {
                        tracing::warn!(
                            member_id = %args.member_id,
                            injection_id = %inject.injection_id,
                            error = %err,
                            "injection failed"
                        );
                    }
                    client
                        .send(&RunnerFrame::InputInjected(InputInjectedFrame {
                            member_id: args.member_id.clone(),
                            runner_id: args.runner_id.clone(),
                            injection_id: inject.injection_id,
                            ok: result.is_ok(),
                            error: result.err().map(|error| error.to_string()),
                        }))
                        .await?;
                }
            },
            Ok(Ok(None)) => break,
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => {}
        }
    }

    Ok(())
}
