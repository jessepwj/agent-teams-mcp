use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::{RunnerEventRequest, TeamModeHost};
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct LocalIpcConfig {
    pub listen: String,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(alias = "type")]
    pub method: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IpcClient {
    host: String,
    token: Option<String>,
}

impl IpcClient {
    pub fn new(host: impl Into<String>, token: Option<String>) -> Self {
        Self {
            host: host.into(),
            token,
        }
    }

    pub async fn call(&self, method: impl Into<String>, params: Value) -> Result<Value> {
        self.call_as(method, params, None).await
    }

    pub async fn call_as(
        &self,
        method: impl Into<String>,
        mut params: Value,
        caller_member_id: Option<String>,
    ) -> Result<Value> {
        let mut stream = TcpStream::connect(&self.host)
            .await
            .map_err(|source| Error::io(&self.host, source))?;
        if params.is_null() {
            params = json!({});
        }
        let req = IpcRequest {
            id: Some(json!(1)),
            method: method.into(),
            params,
            token: self.token.clone(),
            caller_member_id,
        };
        let line =
            serde_json::to_string(&req).map_err(|source| Error::json("<ipc request>", source))?;
        stream
            .write_all(line.as_bytes())
            .await
            .map_err(|source| Error::io(&self.host, source))?;
        stream
            .write_all(b"\n")
            .await
            .map_err(|source| Error::io(&self.host, source))?;
        let mut reader = BufReader::new(stream);
        let mut response_line = String::new();
        reader
            .read_line(&mut response_line)
            .await
            .map_err(|source| Error::io(&self.host, source))?;
        let response: IpcResponse = serde_json::from_str(&response_line)
            .map_err(|source| Error::json("<ipc response>", source))?;
        if response.ok {
            Ok(response.result.unwrap_or(Value::Null))
        } else {
            Err(Error::Other(
                response.error.unwrap_or_else(|| "ipc error".into()),
            ))
        }
    }
}

pub async fn run_local_ipc(host: TeamModeHost, config: LocalIpcConfig) -> Result<()> {
    let listener = TcpListener::bind(&config.listen)
        .await
        .map_err(|source| Error::io(&config.listen, source))?;
    info!(listen = %config.listen, "ipc server listening");
    let _supervisor = host.start_heartbeat_supervisor();
    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .map_err(|source| Error::io(&config.listen, source))?;
        debug!(peer = %peer, "new ipc connection");
        let host = host.clone();
        let token = config.token.clone();
        tokio::spawn(async move {
            let _ = handle_connection(host, stream, token).await;
        });
    }
}

async fn handle_connection(
    host: TeamModeHost,
    stream: TcpStream,
    expected_token: Option<String>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let mut lines = reader.lines();
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let writer_task = tokio::spawn(async move {
        while let Some(value) = rx.recv().await {
            let line = match serde_json::to_string(&value) {
                Ok(line) => line,
                Err(_) => continue,
            };
            if writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if writer.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    let mut runner_member_id: Option<String> = None;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|source| Error::io("<ipc>", source))?
    {
        if line.trim().is_empty() {
            continue;
        }
        let req: IpcRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(err) => {
                let _ = tx.send(json!(IpcResponse {
                    id: None,
                    ok: false,
                    result: None,
                    error: Some(format!("invalid json: {err}")),
                }));
                continue;
            }
        };
        if let Err(err) = check_token(expected_token.as_deref(), req.token.as_deref()) {
            let _ = tx.send(json!(IpcResponse {
                id: req.id,
                ok: false,
                result: None,
                error: Some(err.to_string()),
            }));
            continue;
        }
        let method = req.method.clone();
        let id = req.id.clone();
        let response = if method == "runner/hello" {
            match parse_params::<RunnerEventRequest>(req.params, req.caller_member_id.clone()) {
                Ok(params) => {
                    let member_id = params.member_id.clone();
                    let runner_id = params.runner_id.clone().unwrap_or_default();
                    let pid = params.pid;
                    runner_member_id = Some(member_id.clone());
                    match host.runner_hello(params, Some(tx.clone())).await {
                        Ok(value) => {
                            info!(
                                member_id = %member_id,
                                runner_id = %runner_id,
                                pid = ?pid,
                                "runner registered"
                            );
                            ok(id, value)
                        }
                        Err(err) => {
                            warn!(member_id = %member_id, error = %err, "runner hello failed");
                            fail(id, err)
                        }
                    }
                }
                Err(err) => {
                    warn!(method = %method, error = %err, "ipc parse error");
                    fail(id, err)
                }
            }
        } else {
            debug!(method = %method, "ipc request");
            match dispatch(&host, req).await {
                Ok(value) => ok(id, value),
                Err(err) => {
                    warn!(method = %method, error = %err, "ipc error");
                    fail(id, err)
                }
            }
        };
        let _ = tx.send(json!(response));
    }

    if let Some(member_id) = runner_member_id {
        info!(member_id = %member_id, "runner disconnected");
        host.runner_disconnected(&member_id).await;
    }
    drop(tx);
    let _ = writer_task.await;
    Ok(())
}

async fn dispatch(host: &TeamModeHost, req: IpcRequest) -> Result<Value> {
    match req.method.as_str() {
        "host/status" => json_result(host.status().await),
        "team/create" => json_result(
            host.team_create(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "team/get" => json_result(
            host.team_get(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "team/list" => json_result(host.team_list().await?),
        "team/delete" => json_result(
            host.team_delete(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "member/add" => json_result(
            host.member_add(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "member/list" => {
            let team_id = req
                .params
                .get("teamId")
                .or_else(|| req.params.get("team_id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            json_result(host.member_list(team_id).await)
        }
        "member/get" => json_result(
            host.member_get(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "member/update" => json_result(
            host.member_update(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "member/remove" => json_result(
            host.member_remove(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "execution/set" => json_result(
            host.execution_set(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "room/post" => {
            let caller = require_caller(req.caller_member_id, "room/post")?;
            json_result(
                host.room_post(parse_auth_params(req.params, Some(caller), true)?)
                    .await?,
            )
        }
        "room/list" => json_result(
            host.room_list(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "room/read" => json_result(
            host.room_read_messages(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "thread/read" => json_result(
            host.thread_read(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "thread/reply" => {
            let caller = require_caller(req.caller_member_id, "thread/reply")?;
            json_result(
                host.thread_reply(parse_auth_params(req.params, Some(caller), true)?)
                    .await?,
            )
        }
        "direct/send" => {
            let caller = require_caller(req.caller_member_id, "direct/send")?;
            json_result(
                host.direct_send(parse_auth_params(req.params, Some(caller), true)?)
                    .await?,
            )
        }
        "direct/reply" => {
            let caller = require_caller(req.caller_member_id, "direct/reply")?;
            json_result(
                host.direct_reply(parse_auth_params(req.params, Some(caller), true)?)
                    .await?,
            )
        }
        "direct/read" => {
            let caller = require_caller(req.caller_member_id, "direct/read")?;
            json_result(
                host.direct_read(parse_params(req.params, Some(caller))?)
                    .await?,
            )
        }
        "direct/list" => {
            let caller = require_caller(req.caller_member_id, "direct/list")?;
            json_result(
                host.direct_list(parse_params(req.params, Some(caller))?)
                    .await?,
            )
        }
        "inbox/peek" => {
            let caller = require_caller(req.caller_member_id, "inbox/peek")?;
            json_result(
                host.inbox_peek(parse_params(req.params, Some(caller))?)
                    .await?,
            )
        }
        "inbox/read" => {
            let caller = require_caller(req.caller_member_id, "inbox/read")?;
            json_result(
                host.inbox_read(parse_params(req.params, Some(caller))?)
                    .await?,
            )
        }
        "inbox/ack" => {
            let caller = require_caller(req.caller_member_id, "inbox/ack")?;
            json_result(
                host.inbox_ack(parse_params(req.params, Some(caller))?)
                    .await?,
            )
        }
        "inbox/count" => {
            let caller = require_caller(req.caller_member_id, "inbox/count")?;
            json_result(
                host.inbox_count(parse_params(req.params, Some(caller))?)
                    .await?,
            )
        }
        "member/tail" => {
            let caller = require_caller(req.caller_member_id, "member/tail")?;
            json_result(
                host.member_tail(parse_params(req.params, Some(caller))?)
                    .await?,
            )
        }
        "member/output_tail" => {
            let caller = require_caller(req.caller_member_id, "member/output_tail")?;
            json_result(
                host.member_tail(parse_params(req.params, Some(caller))?)
                    .await?,
            )
        }
        "member/spawn_managed" => json_result(
            host.member_spawn_managed(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "member/shutdown_managed" => json_result(
            host.member_shutdown_managed(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "member/restart_managed" => json_result(
            host.member_restart_managed(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "member/session_status" => json_result(
            host.member_session_status(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "member/attach" => json_result(
            host.member_attach(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "runner/inject" => {
            let caller = require_caller(req.caller_member_id, "runner/inject")?;
            json_result(
                host.runner_inject(parse_params(req.params, Some(caller))?)
                    .await?,
            )
        }
        "codex/steer" => json_result(
            host.codex_steer(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        "codex/interrupt" => json_result(
            host.codex_interrupt(parse_params(req.params, req.caller_member_id)?)
                .await?,
        ),
        method @ ("runner/heartbeat"
        | "runner/output"
        | "runner/input_injected"
        | "runner/child_exit") => {
            host.runner_event(method, parse_params(req.params, req.caller_member_id)?)
                .await
        }
        other => Err(Error::Invalid(format!("unknown ipc method: {other}"))),
    }
}

fn require_caller(caller_member_id: Option<String>, method: &str) -> Result<String> {
    caller_member_id.ok_or_else(|| {
        Error::Invalid(format!(
            "{method} requires top-level callerMemberId in the IPC envelope"
        ))
    })
}

fn parse_params<T: for<'de> Deserialize<'de>>(
    mut params: Value,
    caller_member_id: Option<String>,
) -> Result<T> {
    if let Value::Object(map) = &mut params {
        map.remove("caller_member_id");
        if let Some(caller_member_id) = caller_member_id {
            map.insert("callerMemberId".into(), Value::String(caller_member_id));
        } else {
            map.remove("callerMemberId");
        }
    }
    serde_json::from_value(params).map_err(|source| Error::json("<ipc params>", source))
}

fn parse_auth_params<T: for<'de> Deserialize<'de>>(
    mut params: Value,
    caller_member_id: Option<String>,
    default_sender_to_caller: bool,
) -> Result<T> {
    if let Some(caller) = caller_member_id {
        if let Value::Object(map) = &mut params {
            map.remove("caller_member_id");
            map.insert("callerMemberId".into(), Value::String(caller.clone()));
            if default_sender_to_caller
                && !map.contains_key("senderMemberId")
                && !map.contains_key("sender_member_id")
            {
                map.insert("senderMemberId".into(), Value::String(caller));
            }
        }
    } else if let Value::Object(map) = &mut params {
        map.remove("callerMemberId");
        map.remove("caller_member_id");
    }
    serde_json::from_value(params).map_err(|source| Error::json("<ipc params>", source))
}

fn json_result<T: Serialize>(value: T) -> Result<Value> {
    serde_json::to_value(value).map_err(|source| Error::json("<ipc result>", source))
}

fn check_token(expected: Option<&str>, actual: Option<&str>) -> Result<()> {
    if let Some(expected) = expected {
        if Some(expected) != actual {
            return Err(Error::Invalid("invalid local ipc token".into()));
        }
    }
    Ok(())
}

fn ok(id: Option<Value>, result: Value) -> IpcResponse {
    IpcResponse {
        id,
        ok: true,
        result: Some(result),
        error: None,
    }
}

fn fail(id: Option<Value>, err: Error) -> IpcResponse {
    IpcResponse {
        id,
        ok: false,
        result: None,
        error: Some(err.to_string()),
    }
}
