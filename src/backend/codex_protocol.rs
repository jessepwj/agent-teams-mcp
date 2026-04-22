//! Lightweight JSON-RPC types for the Codex `app-server` stdio protocol.
//!
//! Codex communicates over stdin/stdout using newline-delimited JSON messages.
//! The protocol is JSON-RPC-like but does **not** include the `"jsonrpc": "2.0"` field.
//!
//! Protocol flow:
//! 1. Client sends `initialize` request → server responds with `{userAgent}`
//! 2. Client sends `initialized` notification (no `id`)
//! 3. Client sends `thread/start` → server responds with `{thread: {id, ...}, model, ...}`
//! 4. Client sends `turn/start` with `{threadId, input}` → server responds + streams events
//! 5. Events: `turn/started`, `item/started`, `item/agentMessage/delta`, `item/completed`, `turn/completed`

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// JSON-RPC wire types (Codex variant -- no `jsonrpc` field)
// ---------------------------------------------------------------------------

/// A request (client -> server). Has an `id` for correlation.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    /// Correlation ID (number or string).
    pub id: serde_json::Value,
    /// Method name.
    pub method: String,
    /// Parameters (required for most methods).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    /// Build a new request with a numeric `id`.
    pub fn new(id: u64, method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            id: serde_json::Value::Number(id.into()),
            method: method.into(),
            params,
        }
    }
}

/// A client-to-server notification (no `id`, no response expected).
#[derive(Debug, Serialize)]
pub struct JsonRpcClientNotification {
    /// Method name.
    pub method: String,
}

impl JsonRpcClientNotification {
    pub fn new(method: &str) -> Self {
        Self {
            method: method.into(),
        }
    }
}

/// A response (server -> client). Has an `id` matching the request.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    /// Correlation ID that matches the request.
    pub id: serde_json::Value,
    /// Successful result (mutually exclusive with `error`).
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    /// Error payload (mutually exclusive with `result`).
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error object.
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    /// Numeric error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured error data.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

/// A server-to-client notification (no `id` field).
#[derive(Debug, Deserialize)]
pub struct JsonRpcNotification {
    /// Notification method.
    pub method: String,
    /// Optional parameters.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Envelope: unifies Response and Notification for line-parsing
// ---------------------------------------------------------------------------

/// A line from Codex stdout can be either a response (has `id`) or a notification (no `id`).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    /// A response to a previously sent request.
    Response(JsonRpcResponse),
    /// An unsolicited notification from the server.
    Notification(JsonRpcNotification),
}

// ---------------------------------------------------------------------------
// Method constants (client -> server requests)
// ---------------------------------------------------------------------------

/// Initialize handshake.
pub const METHOD_INITIALIZE: &str = "initialize";
/// Client notification sent after initialize response.
pub const METHOD_INITIALIZED: &str = "initialized";
/// Start a new thread (conversation).
pub const METHOD_THREAD_START: &str = "thread/start";
/// Start a new turn within a thread.
pub const METHOD_TURN_START: &str = "turn/start";
/// Interrupt the current turn.
pub const METHOD_TURN_INTERRUPT: &str = "turn/interrupt";

// ---------------------------------------------------------------------------
// Server notification events
// ---------------------------------------------------------------------------

/// Thread has been started/loaded.
pub const EVENT_THREAD_STARTED: &str = "thread/started";
/// A turn has started processing.
pub const EVENT_TURN_STARTED: &str = "turn/started";
/// A turn has completed.
pub const EVENT_TURN_COMPLETED: &str = "turn/completed";
/// An output item has started.
pub const EVENT_ITEM_STARTED: &str = "item/started";
/// A streaming text delta from the agent.
pub const EVENT_AGENT_MESSAGE_DELTA: &str = "item/agentMessage/delta";
/// An output item has completed.
pub const EVENT_ITEM_COMPLETED: &str = "item/completed";
/// Command execution output delta.
pub const EVENT_COMMAND_OUTPUT_DELTA: &str = "item/commandExecution/outputDelta";
/// Error notification.
pub const EVENT_ERROR: &str = "error";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_request() {
        let req = JsonRpcRequest::new(
            1,
            METHOD_INITIALIZE,
            Some(serde_json::json!({"clientInfo": {"name": "test", "version": "0.1.0"}})),
        );
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""id":1"#));
        assert!(json.contains(r#""method":"initialize""#));
        // No jsonrpc field in Codex protocol
        assert!(!json.contains("jsonrpc"));
    }

    #[test]
    fn serialize_client_notification() {
        let notif = JsonRpcClientNotification::new(METHOD_INITIALIZED);
        let json = serde_json::to_string(&notif).unwrap();
        assert!(json.contains(r#""method":"initialized""#));
        assert!(!json.contains("id"));
    }

    #[test]
    fn deserialize_response_ok() {
        let json = r#"{"id":1,"result":{"userAgent":"agent-teams/0.87.0"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_none());
        assert_eq!(
            resp.result.unwrap()["userAgent"].as_str().unwrap(),
            "agent-teams/0.87.0"
        );
    }

    #[test]
    fn deserialize_response_err() {
        let json = r#"{"id":2,"error":{"code":-32600,"message":"bad request"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
    }

    #[test]
    fn deserialize_notification() {
        let json = r#"{"method":"item/agentMessage/delta","params":{"delta":"hello","threadId":"t1","turnId":"0","itemId":"i1"}}"#;
        let notif: JsonRpcNotification = serde_json::from_str(json).unwrap();
        assert_eq!(notif.method, EVENT_AGENT_MESSAGE_DELTA);
        assert_eq!(notif.params.unwrap()["delta"].as_str().unwrap(), "hello");
    }

    #[test]
    fn deserialize_envelope_response() {
        let json = r#"{"id":1,"result":null}"#;
        let msg: JsonRpcMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, JsonRpcMessage::Response(_)));
    }

    #[test]
    fn deserialize_envelope_notification() {
        let json = r#"{"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"0","items":[],"status":"completed"}}}"#;
        let msg: JsonRpcMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, JsonRpcMessage::Notification(_)));
    }

    #[test]
    fn deserialize_thread_start_response() {
        let json = r#"{"id":2,"result":{"thread":{"id":"abc-123","preview":"","modelProvider":"openai","createdAt":1700000000,"path":"/tmp","source":"cli","turns":[],"cwd":"/tmp","cliVersion":"0.87.0"},"model":"gpt-4","modelProvider":"openai","cwd":"/tmp","approvalPolicy":"never","sandbox":{"type":"workspaceWrite"}}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        let result = resp.result.unwrap();
        let thread_id = result["thread"]["id"].as_str().unwrap();
        assert_eq!(thread_id, "abc-123");
    }

    #[test]
    fn deserialize_turn_start_response() {
        let json = r#"{"id":3,"result":{"turn":{"id":"0","items":[],"status":"inProgress","error":null}}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        let result = resp.result.unwrap();
        let turn_id = result["turn"]["id"].as_str().unwrap();
        assert_eq!(turn_id, "0");
    }
}
