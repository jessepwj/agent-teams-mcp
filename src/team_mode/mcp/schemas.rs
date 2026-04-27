use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JsonRpcResponse<T>
where
    T: Serialize,
{
    pub jsonrpc: String,
    pub id: Value,
    pub result: T,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub error: JsonRpcErrorObject,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolsCapability {
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesCapability {
    #[serde(default)]
    pub subscribe: bool,
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<TextContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDescriptor {
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextResourceContents {
    pub uri: String,
    pub mime_type: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListToolsResult {
    pub tools: Vec<ToolDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListResourcesResult {
    pub resources: Vec<ResourceDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadResourceResult {
    pub contents: Vec<TextResourceContents>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmptyResult {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourcesUpdatedParams {
    pub uri: String,
}

impl<T> JsonRpcResponse<T>
where
    T: Serialize,
{
    pub fn success(id: Value, result: T) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result,
        }
    }
}

impl JsonRpcErrorResponse {
    pub fn new(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            error: JsonRpcErrorObject {
                code,
                message: message.into(),
                data: None,
            },
        }
    }
}

impl InitializeResult {
    pub fn team_mode_default() -> Self {
        Self {
            protocol_version: "2025-06-18".into(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: false,
                }),
                resources: Some(ResourcesCapability {
                    subscribe: true,
                    list_changed: false,
                }),
            },
            server_info: ServerInfo {
                name: "agent-teams-team-mode".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            instructions: Some(
                "Use room/message/inbox/thread tools to collaborate in Team Mode.".into(),
            ),
        }
    }
}

impl ToolCallResult {
    pub fn success(value: Value) -> Self {
        Self {
            content: vec![TextContentBlock {
                kind: "text".into(),
                text: serde_json::to_string(&value).unwrap_or_else(|_| "{}".into()),
            }],
            structured_content: Some(value),
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![TextContentBlock {
                kind: "text".into(),
                text: message.into(),
            }],
            structured_content: None,
            is_error: true,
        }
    }
}

pub fn empty_object_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_result_has_expected_capabilities() {
        let result = InitializeResult::team_mode_default();
        assert_eq!(result.protocol_version, "2025-06-18");
        assert!(!result.capabilities.tools.unwrap().list_changed);
        assert!(result.capabilities.resources.unwrap().subscribe);
    }

    #[test]
    fn tool_call_result_serializes_structured_content() {
        let result = ToolCallResult::success(json!({ "ok": true }));
        let text = &result.content[0].text;
        assert!(text.contains(r#""ok":true"#));
        assert_eq!(result.structured_content, Some(json!({ "ok": true })));
        assert!(!result.is_error);
    }

    #[test]
    fn json_rpc_error_response_serializes() {
        let response = JsonRpcErrorResponse::new(json!(1), -32601, "method not found");
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains(r#""code":-32601"#));
        assert!(serialized.contains(r#""method not found""#));
    }
}
