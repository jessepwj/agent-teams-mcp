use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::Result;
use crate::host::IpcClient;

#[derive(Debug, Clone)]
pub struct McpProxyConfig {
    pub host: String,
    pub member_id: String,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonRpcRequest {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

pub async fn run_mcp_proxy(config: McpProxyConfig) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    let client = IpcClient::new(config.host.clone(), config.token.clone());
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|source| crate::Error::io("<stdin>", source))?
    {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(req) => {
                if req.id.is_none() && req.method.starts_with("notifications/") {
                    continue;
                }
                handle_json_rpc(&client, &config.member_id, req).await
            }
            Err(err) => JsonRpcResponse {
                jsonrpc: "2.0",
                id: None,
                result: None,
                error: Some(JsonRpcError {
                    code: -32700,
                    message: format!("Parse error: {err}"),
                }),
            },
        };
        let line = serde_json::to_string(&response)
            .map_err(|source| crate::Error::json("<mcp response>", source))?;
        stdout
            .write_all(line.as_bytes())
            .await
            .map_err(|source| crate::Error::io("<stdout>", source))?;
        stdout
            .write_all(b"\n")
            .await
            .map_err(|source| crate::Error::io("<stdout>", source))?;
        stdout
            .flush()
            .await
            .map_err(|source| crate::Error::io("<stdout>", source))?;
    }
    Ok(())
}

async fn handle_json_rpc(
    client: &IpcClient,
    caller_member_id: &str,
    req: JsonRpcRequest,
) -> JsonRpcResponse {
    let _jsonrpc_version = req.jsonrpc.unwrap_or_else(|| "2.0".into());
    let result = match req.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {
                "name": "team-mode-mcp-proxy",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": { "tools": {}, "resources": {} }
        })),
        "tools/list" => Ok(json!({ "tools": tool_schemas() })),
        "tools/call" => call_tool(client, caller_member_id, req.params).await,
        "resources/list" => Ok(json!({ "resources": resource_schemas(caller_member_id) })),
        "resources/read" => read_resource(client, caller_member_id, req.params).await,
        "notifications/initialized" => Ok(Value::Null),
        other => Err(format!("Method not found: {other}")),
    };
    match result {
        Ok(result) => JsonRpcResponse {
            jsonrpc: "2.0",
            id: req.id,
            result: Some(result),
            error: None,
        },
        Err(message) => JsonRpcResponse {
            jsonrpc: "2.0",
            id: req.id,
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message,
            }),
        },
    }
}

async fn call_tool(
    client: &IpcClient,
    caller_member_id: &str,
    params: Value,
) -> std::result::Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tools/call params.name is required".to_string())?;
    let mut arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Value::Object(map) = &mut arguments {
        map.remove("callerMemberId");
        map.remove("caller_member_id");
        if matches!(
            name,
            "inbox_peek"
                | "inbox_read"
                | "inbox_ack"
                | "inbox_count"
                | "member_output_tail"
                | "member_session_status"
                | "member_attach"
        ) {
            map.insert(
                "memberId".into(),
                Value::String(caller_member_id.to_string()),
            );
        }
        if matches!(name, "direct_read" | "direct_list") && !map.contains_key("memberId") {
            map.insert(
                "memberId".into(),
                Value::String(caller_member_id.to_string()),
            );
        }
    }
    let method = match name {
        "team_create" => "team/create",
        "team_get" => "team/get",
        "team_list" => "team/list",
        "team_delete" => "team/delete",
        "member_add" => "member/add",
        "member_get" => "member/get",
        "member_update" => "member/update",
        "member_remove" => "member/remove",
        "member_list" => "member/list",
        "execution_profile_set" => "execution/set",
        "room_post_message" => "room/post",
        "room_read_messages" => "room/read",
        "room_list" => "room/list",
        "thread_read" => "thread/read",
        "thread_reply" => "thread/reply",
        "direct_send" => "direct/send",
        "direct_read" => "direct/read",
        "direct_reply" => "direct/reply",
        "direct_list" => "direct/list",
        "inbox_peek" => "inbox/peek",
        "inbox_read" => "inbox/read",
        "inbox_ack" => "inbox/ack",
        "inbox_count" => "inbox/count",
        "member_spawn_managed" => "member/spawn_managed",
        "member_shutdown_managed" => "member/shutdown_managed",
        "member_restart_managed" => "member/restart_managed",
        "member_session_status" => "member/session_status",
        "member_output_tail" => "member/output_tail",
        "member_attach" => "member/attach",
        "codex_steer" => "codex/steer",
        "codex_interrupt" => "codex/interrupt",
        other => return Err(format!("unknown tool: {other}")),
    };
    let result = client
        .call_as(method, arguments, Some(caller_member_id.to_string()))
        .await
        .map_err(|err| err.to_string())?;
    Ok(json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
            }
        ],
        "isError": false
    }))
}

async fn read_resource(
    client: &IpcClient,
    caller_member_id: &str,
    params: Value,
) -> std::result::Result<Value, String> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| "resources/read params.uri is required".to_string())?;
    let result = if uri == "team-mode://self/inbox" {
        client
            .call_as(
                "inbox/peek",
                json!({ "memberId": caller_member_id }),
                Some(caller_member_id.to_string()),
            )
            .await
    } else if uri == "team-mode://self/tail" {
        client
            .call_as(
                "member/output_tail",
                json!({ "memberId": caller_member_id, "limit": 100 }),
                Some(caller_member_id.to_string()),
            )
            .await
    } else if let Some(rest) = uri.strip_prefix("team-mode://room/") {
        let mut parts = rest.splitn(2, '/');
        let team_id = parts
            .next()
            .filter(|part| !part.is_empty())
            .ok_or_else(|| "room resource requires team id".to_string())?;
        let room_id = parts
            .next()
            .filter(|part| !part.is_empty())
            .unwrap_or("main");
        client
            .call_as(
                "room/read",
                json!({ "teamId": team_id, "roomId": room_id, "limit": 100 }),
                Some(caller_member_id.to_string()),
            )
            .await
    } else {
        return Err(format!("unknown resource uri: {uri}"));
    }
    .map_err(|err| err.to_string())?;
    Ok(json!({
        "contents": [
            {
                "uri": uri,
                "mimeType": "application/json",
                "text": serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
            }
        ]
    }))
}

fn tool_schemas() -> Vec<Value> {
    vec![
        tool("team_create", "Create a Team Mode team", &["id", "name"]),
        tool("team_get", "Get a Team Mode team", &["teamId"]),
        tool("team_list", "List Team Mode teams", &[]),
        tool("team_delete", "Delete a Team Mode team", &["teamId"]),
        tool(
            "member_add",
            "Add a team member",
            &["teamId", "id", "handle", "name"],
        ),
        tool("member_get", "Get a team member", &["teamId", "memberId"]),
        tool(
            "member_update",
            "Update a team member",
            &["teamId", "memberId"],
        ),
        tool(
            "member_remove",
            "Remove a team member",
            &["teamId", "memberId"],
        ),
        tool("member_list", "List team members", &[]),
        tool(
            "execution_profile_set",
            "Set a member execution profile",
            &["memberId", "execution"],
        ),
        tool(
            "room_post_message",
            "Post a message to a room",
            &["teamId", "body"],
        ),
        tool(
            "room_read_messages",
            "Read room transcript messages",
            &["teamId"],
        ),
        tool("room_list", "List rooms for a team", &["teamId"]),
        tool(
            "thread_read",
            "Read a thread and its messages",
            &["threadId"],
        ),
        tool("thread_reply", "Reply to a thread", &["threadId", "body"]),
        tool(
            "direct_send",
            "Send a direct message",
            &["teamId", "recipientMemberId", "body"],
        ),
        tool(
            "direct_read",
            "Read a direct thread as the caller or given member",
            &["teamId", "threadId"],
        ),
        tool(
            "direct_reply",
            "Reply to a direct thread",
            &["threadId", "body"],
        ),
        tool(
            "direct_list",
            "List direct threads for the caller",
            &["teamId"],
        ),
        tool("inbox_peek", "Peek the caller member inbox", &[]),
        tool(
            "inbox_read",
            "Read and mark caller inbox messages read",
            &[],
        ),
        tool(
            "inbox_ack",
            "Acknowledge a caller inbox message",
            &["messageId"],
        ),
        tool("inbox_count", "Count caller inbox messages", &[]),
        tool(
            "member_spawn_managed",
            "Spawn this or another managed member session",
            &["memberId"],
        ),
        tool(
            "member_shutdown_managed",
            "Shut down this or another managed member session",
            &["memberId"],
        ),
        tool(
            "member_restart_managed",
            "Restart this or another managed member session",
            &["memberId"],
        ),
        tool(
            "member_session_status",
            "Read the caller member managed session status",
            &[],
        ),
        tool(
            "member_output_tail",
            "Tail the caller member raw output",
            &[],
        ),
        tool(
            "member_attach",
            "Get a viewer command for the caller member",
            &[],
        ),
        tool(
            "codex_steer",
            "Steer a running Codex turn mid-flight with additional input",
            &["memberId", "text"],
        ),
        tool(
            "codex_interrupt",
            "Interrupt the current Codex turn",
            &["memberId"],
        ),
    ]
}

fn resource_schemas(caller_member_id: &str) -> Vec<Value> {
    vec![
        resource(
            "team-mode://self/inbox",
            "Caller inbox",
            "Unread and unacked messages for this MCP member",
        ),
        resource(
            "team-mode://self/tail",
            "Caller raw output tail",
            "Recent raw output captured for this MCP member",
        ),
        resource(
            "team-mode://room/{teamId}/{roomId}",
            "Room transcript",
            "Read a room transcript by replacing {teamId}/{roomId}",
        ),
        resource(
            &format!("team-mode://member/{caller_member_id}"),
            "Caller member",
            "Identity-bound Team Mode member context",
        ),
    ]
}

fn tool(name: &str, description: &str, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {},
            "required": required
        }
    })
}

fn resource(uri: &str, name: &str, description: &str) -> Value {
    json!({
        "uri": uri,
        "name": name,
        "description": description,
        "mimeType": "application/json"
    })
}
