use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::team_mode::mcp::executor::TeamModeToolExecutor;
use crate::team_mode::mcp::resources::TeamModeResourceRegistry;
use crate::team_mode::mcp::schemas::{
    EmptyResult, InitializeResult, JsonRpcErrorResponse, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, ListResourcesResult, ListToolsResult, ReadResourceResult,
    ResourcesUpdatedParams, ToolCallResult,
};
use crate::team_mode::mcp::tools::TeamModeToolset;

const JSON_RPC_VERSION: &str = "2.0";
const ERR_INVALID_REQUEST: i32 = -32600;
const ERR_METHOD_NOT_FOUND: i32 = -32601;
const ERR_RUNTIME: i32 = -32000;

#[derive(Debug, Clone)]
pub enum StdioExitReason {
    StdinEof,
    StdinReadError(String),
}

pub struct TeamModeMcpRuntime {
    tools: Box<dyn TeamModeToolExecutor>,
    resources: TeamModeResourceRegistry,
    initialized: bool,
}

impl TeamModeMcpRuntime {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        let tools = Box::new(TeamModeToolset::new(base_dir.clone()));
        Self::with_tool_executor(base_dir, tools)
    }

    pub fn with_tool_executor(
        base_dir: impl Into<PathBuf>,
        tools: Box<dyn TeamModeToolExecutor>,
    ) -> Self {
        let base_dir = base_dir.into();
        Self {
            tools,
            resources: TeamModeResourceRegistry::new(base_dir),
            initialized: false,
        }
    }

    pub fn handle_request(
        &mut self,
        request: JsonRpcRequest,
    ) -> Result<(Option<Value>, Vec<Value>)> {
        tracing::debug!(method = %request.method, id = ?request.id, "MCP request");

        let response_id = request.id.clone().unwrap_or(Value::Null);
        if request.jsonrpc != JSON_RPC_VERSION {
            tracing::warn!(method = %request.method, "invalid jsonrpc version");
            return Ok((
                Some(serde_json::to_value(JsonRpcErrorResponse::new(
                    response_id,
                    ERR_INVALID_REQUEST,
                    "jsonrpc must be '2.0'",
                ))?),
                Vec::new(),
            ));
        }

        if request.method.trim().is_empty() {
            tracing::warn!("empty method in MCP request");
            return Ok((
                Some(serde_json::to_value(JsonRpcErrorResponse::new(
                    response_id,
                    ERR_INVALID_REQUEST,
                    "method is required",
                ))?),
                Vec::new(),
            ));
        }

        match request.method.as_str() {
            "initialize" => {
                self.initialized = true;
                tracing::info!("MCP server initialized");
                let response =
                    JsonRpcResponse::success(response_id, InitializeResult::team_mode_default());
                Ok((Some(serde_json::to_value(response)?), Vec::new()))
            }
            "notifications/initialized" => {
                self.initialized = true;
                tracing::debug!("received notifications/initialized");
                Ok((None, Vec::new()))
            }
            method if !self.initialized => {
                tracing::warn!(method = %method, "request before initialization");
                Ok((
                    Some(serde_json::to_value(JsonRpcErrorResponse::new(
                        response_id,
                        ERR_RUNTIME,
                        format!("server is not initialized; cannot handle '{method}'"),
                    ))?),
                    Vec::new(),
                ))
            }
            "ping" => {
                tracing::debug!("ping");
                let response = JsonRpcResponse::success(response_id, EmptyResult {});
                Ok((Some(serde_json::to_value(response)?), Vec::new()))
            }
            "tools/list" => Self::wrap_runtime_result(response_id, || {
                let result = ListToolsResult {
                    tools: self.tools.list_tools()?,
                };
                Ok((
                    Some(serde_json::to_value(JsonRpcResponse::success(
                        Value::Null,
                        result,
                    ))?),
                    Vec::new(),
                ))
            }),
            "tools/call" => Self::wrap_runtime_result(response_id, || {
                let params = expect_object(request.params)?;
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::Other("tools/call requires a 'name'".into()))?;
                let arguments = params.get("arguments").cloned();

                tracing::info!(tool = %name, "tool call");

                match self.tools.call_tool(name, arguments) {
                    Ok(execution) => {
                        tracing::debug!(tool = %name, is_error = execution.result.is_error, "MCP response");
                        let notifications = self
                            .resources
                            .subscribed_updates(&execution.updated_resources)
                            .into_iter()
                            .map(|uri| {
                                serde_json::to_value(JsonRpcNotification {
                                    jsonrpc: JSON_RPC_VERSION.into(),
                                    method: "notifications/resources/updated".into(),
                                    params: Some(serde_json::to_value(ResourcesUpdatedParams {
                                        uri,
                                    })?),
                                })
                            })
                            .collect::<std::result::Result<Vec<_>, serde_json::Error>>()?;
                        let response = JsonRpcResponse::success(Value::Null, execution.result);
                        Ok((Some(serde_json::to_value(response)?), notifications))
                    }
                    Err(err) => {
                        tracing::warn!(tool = %name, error = %err, "tool call returned error");
                        let response = JsonRpcResponse::success(
                            Value::Null,
                            ToolCallResult::error(err.to_string()),
                        );
                        Ok((Some(serde_json::to_value(response)?), Vec::new()))
                    }
                }
            }),
            "resources/list" => Self::wrap_runtime_result(response_id, || {
                let result = ListResourcesResult {
                    resources: self.resources.list_resources()?,
                };
                Ok((
                    Some(serde_json::to_value(JsonRpcResponse::success(
                        Value::Null,
                        result,
                    ))?),
                    Vec::new(),
                ))
            }),
            "resources/read" => Self::wrap_runtime_result(response_id, || {
                let params = expect_object(request.params)?;
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::Other("resources/read requires 'uri'".into()))?;
                let result: ReadResourceResult = self.resources.read_resource(uri)?;
                Ok((
                    Some(serde_json::to_value(JsonRpcResponse::success(
                        Value::Null,
                        result,
                    ))?),
                    Vec::new(),
                ))
            }),
            "resources/subscribe" => Self::wrap_runtime_result(response_id, || {
                let params = expect_object(request.params)?;
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::Other("resources/subscribe requires 'uri'".into()))?;
                self.resources.subscribe(uri)?;
                Ok((
                    Some(serde_json::to_value(JsonRpcResponse::success(
                        Value::Null,
                        EmptyResult {},
                    ))?),
                    Vec::new(),
                ))
            }),
            "resources/unsubscribe" => Self::wrap_runtime_result(response_id, || {
                let params = expect_object(request.params)?;
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::Other("resources/unsubscribe requires 'uri'".into()))?;
                self.resources.unsubscribe(uri)?;
                Ok((
                    Some(serde_json::to_value(JsonRpcResponse::success(
                        Value::Null,
                        EmptyResult {},
                    ))?),
                    Vec::new(),
                ))
            }),
            _ => Ok((
                Some(serde_json::to_value(JsonRpcErrorResponse::new(
                    response_id,
                    ERR_METHOD_NOT_FOUND,
                    format!("method '{}' not found", request.method),
                ))?),
                Vec::new(),
            )),
        }
    }

    fn wrap_runtime_result<F>(
        response_id: Value,
        operation: F,
    ) -> Result<(Option<Value>, Vec<Value>)>
    where
        F: FnOnce() -> Result<(Option<Value>, Vec<Value>)>,
    {
        match operation() {
            Ok((response, notifications)) => {
                let response = response.map(|mut response| {
                    if let Some(id) = response.get_mut("id") {
                        *id = response_id.clone();
                    }
                    response
                });
                Ok((response, notifications))
            }
            Err(err) => Ok((
                Some(serde_json::to_value(JsonRpcErrorResponse::new(
                    response_id,
                    ERR_RUNTIME,
                    err.to_string(),
                ))?),
                Vec::new(),
            )),
        }
    }

    pub fn run_stdio(&mut self) -> Result<()> {
        self.run_stdio_with_exit_reason().map(|_| ())
    }

    pub fn run_stdio_with_exit_reason(&mut self) -> Result<StdioExitReason> {
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut reader = io::BufReader::new(stdin.lock());
        let mut writer = io::BufWriter::new(stdout.lock());
        let exit_reason;

        loop {
            let message = match read_json_rpc_message(&mut reader) {
                Ok(Some(message)) => message,
                Ok(None) => {
                    // Stdin closed by CC. Log this so we can distinguish
                    // "CC closed our pipe" (clean exit path, expected on
                    // session end) from "process signal-killed us" (we'd
                    // never reach here) when debugging disconnect issues.
                    tracing::warn!("MCP: stdin EOF — parent closed the pipe, exiting run_stdio");
                    exit_reason = StdioExitReason::StdinEof;
                    break;
                }
                Err(err) => {
                    let error = err.to_string();
                    tracing::error!(%error, "MCP: stdin read error, exiting run_stdio");
                    let response = serde_json::to_value(JsonRpcErrorResponse::new(
                        Value::Null,
                        ERR_INVALID_REQUEST,
                        error.clone(),
                    ))?;
                    write_json_rpc_message(&mut writer, &response)?;
                    exit_reason = StdioExitReason::StdinReadError(error);
                    break;
                }
            };

            let request: JsonRpcRequest = match serde_json::from_value(message) {
                Ok(request) => request,
                Err(err) => {
                    let response = serde_json::to_value(JsonRpcErrorResponse::new(
                        Value::Null,
                        ERR_INVALID_REQUEST,
                        err.to_string(),
                    ))?;
                    write_json_rpc_message(&mut writer, &response)?;
                    continue;
                }
            };

            let (response, notifications) = match self.handle_request(request) {
                Ok(result) => result,
                Err(err) => {
                    let response = serde_json::to_value(JsonRpcErrorResponse::new(
                        Value::Null,
                        ERR_RUNTIME,
                        err.to_string(),
                    ))?;
                    write_json_rpc_message(&mut writer, &response)?;
                    continue;
                }
            };
            if let Some(response) = response {
                write_json_rpc_message(&mut writer, &response)?;
            }
            for notification in notifications {
                write_json_rpc_message(&mut writer, &notification)?;
            }
        }

        writer.flush()?;
        Ok(exit_reason)
    }
}

fn expect_object(value: Option<Value>) -> Result<Value> {
    match value {
        Some(Value::Object(map)) => Ok(Value::Object(map)),
        None => Ok(Value::Object(Default::default())),
        _ => Err(Error::Other("params must be an object".into())),
    }
}

fn read_json_rpc_message<R>(reader: &mut R) -> Result<Option<Value>>
where
    R: BufRead + Read,
{
    let mut first_line = String::new();
    loop {
        first_line.clear();
        let bytes = reader.read_line(&mut first_line)?;
        if bytes == 0 {
            return Ok(None);
        }
        if first_line.trim().is_empty() {
            continue;
        }
        break;
    }

    if first_line.trim_start().starts_with('{') || first_line.trim_start().starts_with('[') {
        Ok(Some(serde_json::from_str(first_line.trim())?))
    } else {
        let mut content_length = parse_content_length_header(&first_line)?;
        let mut header = String::new();
        loop {
            header.clear();
            let bytes = reader.read_line(&mut header)?;
            if bytes == 0 {
                return Err(Error::Other(
                    "unexpected EOF while reading JSON-RPC headers".into(),
                ));
            }
            if header == "\r\n" || header == "\n" {
                break;
            }
            if header.to_ascii_lowercase().starts_with("content-length:") {
                content_length = Some(
                    header
                        .split_once(':')
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .ok_or_else(|| Error::Other("invalid Content-Length header".into()))?,
                );
            }
        }

        let length =
            content_length.ok_or_else(|| Error::Other("missing Content-Length header".into()))?;
        let mut body = vec![0_u8; length];
        reader.read_exact(&mut body)?;
        Ok(Some(serde_json::from_slice(&body)?))
    }
}

fn parse_content_length_header(line: &str) -> Result<Option<usize>> {
    if line.to_ascii_lowercase().starts_with("content-length:") {
        Ok(Some(
            line.split_once(':')
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .ok_or_else(|| Error::Other("invalid Content-Length header".into()))?,
        ))
    } else if line.contains(':') {
        Ok(None)
    } else {
        Err(Error::Other(
            "invalid JSON-RPC transport header or body".into(),
        ))
    }
}

fn write_json_rpc_message<W, T>(writer: &mut W, payload: &T) -> Result<()>
where
    W: Write,
    T: Serialize,
{
    let body = serde_json::to_vec(payload)?;
    writer.write_all(&body)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn initialize_and_tools_list_round_trip() {
        let dir = tempdir().unwrap();
        let mut runtime = TeamModeMcpRuntime::new(dir.path());

        let (initialize, _) = runtime
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(1)),
                method: "initialize".into(),
                params: None,
            })
            .unwrap();
        assert!(initialize.unwrap()["result"]["capabilities"]["tools"].is_object());

        let (tools_list, _) = runtime
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(2)),
                method: "tools/list".into(),
                params: None,
            })
            .unwrap();
        assert!(
            tools_list.unwrap()["result"]["tools"]
                .as_array()
                .unwrap()
                .len()
                >= 7
        );
    }

    #[test]
    fn subscribed_team_uri_receives_update_on_team_delete() {
        // This test exercises the subscribe → notify pipeline without needing to
        // spawn a real managed worker. We subscribe to a team's URI and then
        // delete the team, which pushes team_uri to updated_resources.
        let dir = tempdir().unwrap();
        let mut runtime = TeamModeMcpRuntime::new(dir.path());
        runtime
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(1)),
                method: "initialize".into(),
                params: None,
            })
            .unwrap();
        runtime
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(2)),
                method: "tools/call".into(),
                params: Some(json!({
                    "name": "team_create",
                    "arguments": { "name": "sub-team" }
                })),
            })
            .unwrap();
        runtime
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(3)),
                method: "resources/subscribe".into(),
                params: Some(json!({ "uri": "team://sub-team" })),
            })
            .unwrap();

        let (_, notifications) = runtime
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(4)),
                method: "tools/call".into(),
                params: Some(json!({
                    "name": "team_delete",
                    "arguments": { "name": "sub-team" }
                })),
            })
            .unwrap();

        assert!(
            notifications
                .iter()
                .any(|n| n["params"]["uri"] == json!("team://sub-team")),
            "expected team://sub-team in notifications, got {:?}",
            notifications
        );
        assert!(
            notifications
                .iter()
                .all(|n| n["method"] == json!("notifications/resources/updated"))
        );
    }

    #[test]
    fn invalid_runtime_params_return_json_rpc_error_response() {
        let dir = tempdir().unwrap();
        let mut runtime = TeamModeMcpRuntime::new(dir.path());
        runtime
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(1)),
                method: "initialize".into(),
                params: None,
            })
            .unwrap();

        let (response, notifications) = runtime
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(2)),
                method: "resources/read".into(),
                params: Some(json!({})),
            })
            .unwrap();

        assert!(notifications.is_empty());
        assert_eq!(response.unwrap()["error"]["code"], json!(ERR_RUNTIME));
    }

    #[test]
    fn ndjson_transport_round_trip() {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "ping"
        });
        let mut output = Vec::new();
        write_json_rpc_message(&mut output, &payload).unwrap();

        // written output must be JSON followed by a newline (NDJSON)
        assert!(output.ends_with(b"\n"), "output must end with newline");
        assert!(
            !output.starts_with(b"Content-Length"),
            "must not use Content-Length framing"
        );

        let mut reader = io::Cursor::new(output);
        let parsed = read_json_rpc_message(&mut reader).unwrap().unwrap();
        assert_eq!(parsed["method"], "ping");
    }

    #[test]
    fn content_length_parser_accepts_non_first_headers() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let payload = format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        let mut reader = io::Cursor::new(payload.into_bytes());
        let parsed = read_json_rpc_message(&mut reader).unwrap().unwrap();
        assert_eq!(parsed["method"], "ping");
    }

    #[test]
    fn content_length_parser_errors_on_incomplete_headers() {
        let payload = b"Content-Length: 10\r\n";
        let mut reader = io::Cursor::new(payload.to_vec());
        let err = read_json_rpc_message(&mut reader).unwrap_err();
        assert!(err.to_string().contains("unexpected EOF"));
    }
}
