use std::collections::BTreeMap;
use std::env;
use std::io::{self, BufRead};
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use team_mode_native::host::IpcClient;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:17891", global = true)]
    host: String,
    #[arg(long, global = true)]
    token_env: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Status,
    Team {
        #[command(subcommand)]
        command: TeamCommand,
    },
    Member {
        #[command(subcommand)]
        command: MemberCommand,
    },
    Room {
        #[command(subcommand)]
        command: RoomCommand,
    },
    Thread {
        #[command(subcommand)]
        command: ThreadCommand,
    },
    Inbox {
        #[command(subcommand)]
        command: InboxCommand,
    },
    Dm {
        #[command(subcommand)]
        command: DmCommand,
    },
    Inject(InjectArgs),
    Codex {
        #[command(subcommand)]
        command: CodexCommand,
    },
    LoadConfig(LoadConfigArgs),
}

#[derive(Debug, Subcommand)]
enum TeamCommand {
    Create(TeamCreateArgs),
    Get(TeamIdArgs),
    List,
    Delete(TeamIdArgs),
}

#[derive(Debug, Args)]
struct TeamCreateArgs {
    id: String,
    name: String,
    #[arg(long)]
    description: Option<String>,
}

#[derive(Debug, Args)]
struct TeamIdArgs {
    team_id: String,
}

#[derive(Debug, Subcommand)]
enum MemberCommand {
    Add(MemberAddArgs),
    List(MemberListArgs),
    Get(MemberGetArgs),
    Update(MemberUpdateArgs),
    Remove(MemberGetArgs),
    ExecutionSet(MemberExecutionSetArgs),
    Spawn(MemberSpawnArgs),
    Shutdown(MemberShutdownArgs),
    Restart(MemberRestartArgs),
    Status(MemberStatusArgs),
    Tail(MemberTailArgs),
    Attach(MemberAttachArgs),
}

#[derive(Debug, Args)]
struct MemberAddArgs {
    #[arg(long)]
    team_id: String,
    #[arg(long)]
    id: String,
    #[arg(long)]
    handle: String,
    #[arg(long)]
    name: String,
    #[arg(long)]
    role_label: Option<String>,
}

#[derive(Debug, Args)]
struct MemberListArgs {
    #[arg(long)]
    team_id: Option<String>,
}

#[derive(Debug, Args)]
struct MemberGetArgs {
    #[arg(long)]
    team_id: String,
    member_id: String,
}

#[derive(Debug, Args)]
struct MemberUpdateArgs {
    #[arg(long)]
    team_id: String,
    member_id: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    handle: Option<String>,
    #[arg(long)]
    role_label: Option<String>,
    #[arg(long)]
    role_description: Option<String>,
    #[arg(long)]
    clear_role_description: bool,
}

#[derive(Debug, Args)]
struct MemberExecutionSetArgs {
    member_id: String,
    #[arg(long)]
    json: PathBuf,
}

#[derive(Debug, Args)]
struct MemberSpawnArgs {
    member_id: String,
    #[arg(long)]
    runner_host: Option<String>,
    #[arg(long)]
    token_env: Option<String>,
    #[arg(long)]
    runner_id: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    no_open_terminal: bool,
}

#[derive(Debug, Args)]
struct MemberShutdownArgs {
    member_id: String,
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct MemberRestartArgs {
    member_id: String,
    #[arg(long)]
    runner_host: Option<String>,
    #[arg(long)]
    token_env: Option<String>,
    #[arg(long)]
    runner_id: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    no_open_terminal: bool,
    #[arg(long)]
    force_shutdown: bool,
}

#[derive(Debug, Args)]
struct MemberStatusArgs {
    member_id: String,
}

#[derive(Debug, Args)]
struct MemberTailArgs {
    member_id: String,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Args)]
struct MemberAttachArgs {
    member_id: String,
}

#[derive(Debug, Subcommand)]
enum RoomCommand {
    Post(RoomPostArgs),
    List(RoomListArgs),
    Read(RoomReadArgs),
    Tail(RoomTailArgs),
}

#[derive(Debug, Args)]
struct RoomPostArgs {
    #[arg(long)]
    team_id: String,
    #[arg(long, default_value = "main")]
    room_id: String,
    #[arg(long)]
    sender: String,
    #[arg(long)]
    kind: Option<String>,
    #[arg(long)]
    subject: Option<String>,
    body: String,
}

#[derive(Debug, Args)]
struct RoomListArgs {
    #[arg(long)]
    team_id: String,
}

#[derive(Debug, Args)]
struct RoomReadArgs {
    #[arg(long)]
    team_id: String,
    #[arg(long, default_value = "main")]
    room_id: String,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Args)]
struct RoomTailArgs {
    #[arg(long)]
    team_id: String,
    #[arg(long, default_value = "main")]
    room_id: String,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Subcommand)]
enum ThreadCommand {
    Read(ThreadReadArgs),
    Reply(ThreadReplyArgs),
}

#[derive(Debug, Args)]
struct ThreadReadArgs {
    thread_id: String,
    #[arg(long)]
    team_id: Option<String>,
}

#[derive(Debug, Args)]
struct ThreadReplyArgs {
    thread_id: String,
    #[arg(long)]
    sender: String,
    body: String,
}

#[derive(Debug, Subcommand)]
enum InboxCommand {
    Peek(InboxMemberArgs),
    Read(InboxMemberArgs),
    Ack(InboxAckArgs),
    Count(InboxCountArgs),
}

#[derive(Debug, Args)]
struct InboxMemberArgs {
    member_id: String,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Args)]
struct InboxAckArgs {
    member_id: String,
    message_id: String,
}

#[derive(Debug, Args)]
struct InboxCountArgs {
    member_id: String,
}

#[derive(Debug, Subcommand)]
enum DmCommand {
    Send(DmSendArgs),
    Reply(DmReplyArgs),
    Read(DmReadArgs),
    List(DmListArgs),
}

#[derive(Debug, Args)]
struct DmSendArgs {
    #[arg(long)]
    team_id: String,
    #[arg(long)]
    from: String,
    #[arg(long)]
    to: String,
    #[arg(long)]
    interactive: bool,
    body: Option<String>,
}

#[derive(Debug, Args)]
struct DmReplyArgs {
    thread_id: String,
    #[arg(long)]
    sender: String,
    body: String,
}

#[derive(Debug, Args)]
struct DmReadArgs {
    #[arg(long)]
    team_id: String,
    thread_id: String,
    #[arg(long)]
    member: String,
}

#[derive(Debug, Args)]
struct DmListArgs {
    #[arg(long)]
    team_id: String,
    #[arg(long)]
    member: String,
}

#[derive(Debug, Args)]
struct InjectArgs {
    member_id: String,
    text: String,
    #[arg(long)]
    strategy: Option<String>,
}

#[derive(Debug, Subcommand)]
enum CodexCommand {
    Steer(CodexSteerArgs),
    Interrupt(CodexInterruptArgs),
}

#[derive(Debug, Args)]
struct CodexSteerArgs {
    member_id: String,
    text: String,
}

#[derive(Debug, Args)]
struct CodexInterruptArgs {
    member_id: String,
}

#[derive(Debug, Args)]
struct LoadConfigArgs {
    file: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct TeamYamlConfig {
    team: TeamYamlDef,
    #[serde(default)]
    members: Vec<MemberYamlDef>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TeamYamlDef {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MemberYamlDef {
    id: String,
    handle: String,
    name: String,
    #[serde(default)]
    role_label: Option<String>,
    #[serde(default)]
    adapter: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    prompt_mode: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    restart_policy: Option<String>,
    #[serde(default)]
    command: Option<MemberYamlCommand>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MemberYamlCommand {
    program: String,
    #[serde(default)]
    args: Vec<String>,
}

struct IpcCall {
    method: &'static str,
    params: Value,
    caller: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let token = cli
        .token_env
        .as_deref()
        .and_then(|name| env::var(name).ok())
        .filter(|value| !value.is_empty());
    let client = IpcClient::new(cli.host.clone(), token);

    if let Command::Dm {
        command: DmCommand::Send(args),
    } = &cli.command
    {
        if args.interactive {
            run_dm_interactive(&client, args).await?;
            return Ok(());
        }
    }

    if let Command::LoadConfig(args) = &cli.command {
        run_load_config(&client, args).await?;
        return Ok(());
    }

    let default_runner_host = cli.host.clone();
    let call = command_to_ipc(cli.command, &default_runner_host)?;
    let result = client
        .call_as(call.method, call.params, call.caller)
        .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn run_dm_interactive(client: &IpcClient, args: &DmSendArgs) -> anyhow::Result<()> {
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let body = line?;
        if body.trim().is_empty() {
            continue;
        }
        let result = client
            .call_as(
                "direct/send",
                json!({
                    "teamId": args.team_id.clone(),
                    "senderMemberId": args.from.clone(),
                    "recipientMemberId": args.to.clone(),
                    "body": body,
                }),
                Some(args.from.clone()),
            )
            .await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
}

async fn run_load_config(client: &IpcClient, args: &LoadConfigArgs) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(&args.file)?;
    let config: TeamYamlConfig = if args
        .file
        .extension()
        .map(|e| e == "json")
        .unwrap_or(false)
    {
        serde_json::from_str(&text)?
    } else {
        serde_yaml::from_str(&text)?
    };

    let result = client
        .call(
            "team/create",
            json!({
                "id": config.team.id,
                "name": config.team.name,
                "description": config.team.description,
            }),
        )
        .await?;
    println!("team/create: {}", serde_json::to_string_pretty(&result)?);

    for member in &config.members {
        let add_result = client
            .call(
                "member/add",
                json!({
                    "teamId": config.team.id,
                    "id": member.id,
                    "handle": member.handle,
                    "name": member.name,
                    "roleLabel": member.role_label,
                }),
            )
            .await?;
        println!("member/add {}: {}", member.id, serde_json::to_string_pretty(&add_result)?);

        if member.adapter.is_some() || member.system_prompt.is_some() {
            let adapter = member.adapter.as_deref().unwrap_or("claude-code-terminal");
            let (launch_mode, viewer_mode) = if adapter == "codex-app-server" {
                ("app_server_stdio", "event_viewer")
            } else {
                ("native_terminal_pty", "native_terminal")
            };
            let command = if let Some(cmd) = &member.command {
                json!({ "program": cmd.program, "args": cmd.args })
            } else {
                let program = match adapter {
                    "claude-code-terminal" => "claude",
                    "gemini-cli-terminal" => "gemini",
                    "codex-app-server" => "codex",
                    _ => "claude",
                };
                json!({ "program": program, "args": [] })
            };
            let execution = json!({
                "memberId": member.id,
                "adapter": adapter,
                "launchMode": launch_mode,
                "viewerMode": viewer_mode,
                "command": command,
                "cwd": member.cwd,
                "env": member.env,
                "model": member.model,
                "reasoningEffort": member.reasoning_effort,
                "systemPrompt": member.system_prompt.as_deref().unwrap_or(""),
                "promptMode": member.prompt_mode.as_deref().unwrap_or("append"),
                "restartPolicy": member.restart_policy.as_deref().unwrap_or("never"),
            });
            let exec_result = client
                .call(
                    "execution/set",
                    json!({ "memberId": member.id, "execution": execution }),
                )
                .await?;
            println!(
                "execution/set {}: {}",
                member.id,
                serde_json::to_string_pretty(&exec_result)?
            );
        }
    }
    Ok(())
}

fn command_to_ipc(command: Command, default_runner_host: &str) -> anyhow::Result<IpcCall> {
    Ok(match command {
        Command::Status => IpcCall {
            method: "host/status",
            params: json!({}),
            caller: None,
        },
        Command::Team { command } => match command {
            TeamCommand::Create(args) => IpcCall {
                method: "team/create",
                params: json!({
                    "id": args.id,
                    "name": args.name,
                    "description": args.description,
                }),
                caller: None,
            },
            TeamCommand::Get(args) => IpcCall {
                method: "team/get",
                params: json!({ "teamId": args.team_id }),
                caller: None,
            },
            TeamCommand::List => IpcCall {
                method: "team/list",
                params: json!({}),
                caller: None,
            },
            TeamCommand::Delete(args) => IpcCall {
                method: "team/delete",
                params: json!({ "teamId": args.team_id }),
                caller: None,
            },
        },
        Command::Member { command } => match command {
            MemberCommand::Add(args) => IpcCall {
                method: "member/add",
                params: json!({
                    "teamId": args.team_id,
                    "id": args.id,
                    "handle": args.handle,
                    "name": args.name,
                    "roleLabel": args.role_label,
                }),
                caller: None,
            },
            MemberCommand::List(args) => IpcCall {
                method: "member/list",
                params: json!({ "teamId": args.team_id }),
                caller: None,
            },
            MemberCommand::Get(args) => IpcCall {
                method: "member/get",
                params: json!({ "teamId": args.team_id, "memberId": args.member_id }),
                caller: None,
            },
            MemberCommand::Update(args) => IpcCall {
                method: "member/update",
                params: json!({
                    "teamId": args.team_id,
                    "memberId": args.member_id,
                    "name": args.name,
                    "handle": args.handle,
                    "roleLabel": args.role_label,
                    "roleDescription": args.role_description,
                    "clearRoleDescription": args.clear_role_description,
                }),
                caller: None,
            },
            MemberCommand::Remove(args) => IpcCall {
                method: "member/remove",
                params: json!({ "teamId": args.team_id, "memberId": args.member_id }),
                caller: None,
            },
            MemberCommand::ExecutionSet(args) => {
                let text = std::fs::read_to_string(&args.json)?;
                let mut execution: Value = serde_json::from_str(&text)?;
                if let Value::Object(map) = &mut execution {
                    map.entry("memberId".to_string())
                        .or_insert_with(|| Value::String(args.member_id.clone()));
                }
                IpcCall {
                    method: "execution/set",
                    params: json!({ "memberId": args.member_id, "execution": execution }),
                    caller: None,
                }
            }
            MemberCommand::Spawn(args) => IpcCall {
                method: "member/spawn_managed",
                params: json!({
                    "memberId": args.member_id,
                    "host": args.runner_host.unwrap_or_else(|| default_runner_host.to_string()),
                    "tokenEnv": args.token_env,
                    "runnerId": args.runner_id,
                    "dryRun": args.dry_run,
                    "openTerminal": !args.no_open_terminal,
                }),
                caller: None,
            },
            MemberCommand::Shutdown(args) => IpcCall {
                method: "member/shutdown_managed",
                params: json!({ "memberId": args.member_id, "force": args.force }),
                caller: None,
            },
            MemberCommand::Restart(args) => IpcCall {
                method: "member/restart_managed",
                params: json!({
                    "memberId": args.member_id,
                    "host": args.runner_host.unwrap_or_else(|| default_runner_host.to_string()),
                    "tokenEnv": args.token_env,
                    "runnerId": args.runner_id,
                    "dryRun": args.dry_run,
                    "openTerminal": !args.no_open_terminal,
                    "forceShutdown": args.force_shutdown,
                }),
                caller: None,
            },
            MemberCommand::Status(args) => IpcCall {
                method: "member/session_status",
                params: json!({ "memberId": args.member_id }),
                caller: None,
            },
            MemberCommand::Tail(args) => IpcCall {
                method: "member/tail",
                params: json!({ "memberId": args.member_id.clone(), "limit": args.limit }),
                caller: Some(args.member_id),
            },
            MemberCommand::Attach(args) => IpcCall {
                method: "member/attach",
                params: json!({ "memberId": args.member_id.clone(), "host": default_runner_host }),
                caller: None,
            },
        },
        Command::Room { command } => match command {
            RoomCommand::Post(args) => IpcCall {
                method: "room/post",
                params: json!({
                    "teamId": args.team_id,
                    "roomId": args.room_id,
                    "senderMemberId": args.sender.clone(),
                    "kind": args.kind,
                    "subject": args.subject,
                    "body": args.body,
                }),
                caller: Some(args.sender),
            },
            RoomCommand::List(args) => IpcCall {
                method: "room/list",
                params: json!({ "teamId": args.team_id }),
                caller: None,
            },
            RoomCommand::Read(args) => IpcCall {
                method: "room/read",
                params: json!({
                    "teamId": args.team_id,
                    "roomId": args.room_id,
                    "limit": args.limit
                }),
                caller: None,
            },
            RoomCommand::Tail(args) => IpcCall {
                method: "room/read",
                params: json!({
                    "teamId": args.team_id,
                    "roomId": args.room_id,
                    "limit": args.limit
                }),
                caller: None,
            },
        },
        Command::Thread { command } => match command {
            ThreadCommand::Read(args) => IpcCall {
                method: "thread/read",
                params: json!({ "threadId": args.thread_id, "teamId": args.team_id }),
                caller: None,
            },
            ThreadCommand::Reply(args) => IpcCall {
                method: "thread/reply",
                params: json!({
                    "threadId": args.thread_id,
                    "senderMemberId": args.sender.clone(),
                    "body": args.body,
                }),
                caller: Some(args.sender),
            },
        },
        Command::Inbox { command } => match command {
            InboxCommand::Peek(args) => IpcCall {
                method: "inbox/peek",
                params: json!({ "memberId": args.member_id.clone(), "limit": args.limit }),
                caller: Some(args.member_id),
            },
            InboxCommand::Read(args) => IpcCall {
                method: "inbox/read",
                params: json!({ "memberId": args.member_id.clone(), "limit": args.limit }),
                caller: Some(args.member_id),
            },
            InboxCommand::Ack(args) => IpcCall {
                method: "inbox/ack",
                params: json!({
                    "memberId": args.member_id.clone(),
                    "messageId": args.message_id
                }),
                caller: Some(args.member_id),
            },
            InboxCommand::Count(args) => IpcCall {
                method: "inbox/count",
                params: json!({ "memberId": args.member_id.clone() }),
                caller: Some(args.member_id),
            },
        },
        Command::Dm { command } => match command {
            DmCommand::Send(args) => {
                let body = args.body.ok_or_else(|| {
                    anyhow::anyhow!("dm send body is required unless --interactive is used")
                })?;
                IpcCall {
                    method: "direct/send",
                    params: json!({
                        "teamId": args.team_id,
                        "senderMemberId": args.from.clone(),
                        "recipientMemberId": args.to,
                        "body": body,
                    }),
                    caller: Some(args.from),
                }
            }
            DmCommand::Reply(args) => IpcCall {
                method: "direct/reply",
                params: json!({
                    "threadId": args.thread_id,
                    "senderMemberId": args.sender.clone(),
                    "body": args.body,
                }),
                caller: Some(args.sender),
            },
            DmCommand::Read(args) => IpcCall {
                method: "direct/read",
                params: json!({
                    "teamId": args.team_id,
                    "threadId": args.thread_id,
                    "memberId": args.member.clone(),
                }),
                caller: Some(args.member),
            },
            DmCommand::List(args) => IpcCall {
                method: "direct/list",
                params: json!({ "teamId": args.team_id, "memberId": args.member.clone() }),
                caller: Some(args.member),
            },
        },
        Command::Inject(args) => IpcCall {
            method: "runner/inject",
            params: json!({
                "memberId": args.member_id.clone(),
                "text": args.text,
                "strategy": args.strategy,
            }),
            caller: Some(args.member_id),
        },
        Command::Codex { command } => match command {
            CodexCommand::Steer(args) => IpcCall {
                method: "codex/steer",
                params: json!({
                    "memberId": args.member_id.clone(),
                    "text": args.text,
                }),
                caller: Some(args.member_id),
            },
            CodexCommand::Interrupt(args) => IpcCall {
                method: "codex/interrupt",
                params: json!({ "memberId": args.member_id.clone() }),
                caller: Some(args.member_id),
            },
        },
        Command::LoadConfig(_) => unreachable!("load-config handled before command_to_ipc"),
    })
}
