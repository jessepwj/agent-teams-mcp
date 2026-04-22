use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::terminal::CommandSpec;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexAppServerCommand {
    pub codex_bin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

pub fn codex_app_server_command(codex_bin: impl Into<String>, cwd: Option<&Path>) -> CommandSpec {
    CommandSpec {
        program: codex_bin.into(),
        args: vec!["app-server".to_string()],
        env: Vec::new(),
        cwd: cwd.map(Path::to_path_buf),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexRequest {
    pub id: u64,
    pub method: CodexMethod,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodexMethod {
    Initialize,
    #[serde(rename = "thread/start")]
    ThreadStart,
    #[serde(rename = "turn/start")]
    TurnStart,
    #[serde(rename = "turn/steer")]
    TurnSteer,
    #[serde(rename = "turn/interrupt")]
    TurnInterrupt,
}

impl CodexRequest {
    pub fn initialize(
        id: u64,
        client_name: impl Into<String>,
        client_version: impl Into<String>,
    ) -> Self {
        let client_name = client_name.into();
        let client_version = client_version.into();
        Self {
            id,
            method: CodexMethod::Initialize,
            params: json!({
                "clientInfo": {
                    "name": client_name,
                    "version": client_version
                }
            }),
        }
    }

    pub fn thread_start(
        id: u64,
        cwd: Option<impl Into<String>>,
        developer_instructions: Option<impl Into<String>>,
    ) -> Self {
        Self {
            id,
            method: CodexMethod::ThreadStart,
            params: thread_start_params(
                cwd.map(Into::into),
                developer_instructions.map(Into::into),
            ),
        }
    }

    pub fn turn_start(id: u64, thread_id: impl Into<String>, prompt: impl Into<String>) -> Self {
        let thread_id = thread_id.into();
        let prompt = prompt.into();
        Self {
            id,
            method: CodexMethod::TurnStart,
            params: json!({
                "threadId": thread_id,
                "input": [
                    {
                        "type": "text",
                        "text": prompt
                    }
                ]
            }),
        }
    }

    pub fn turn_steer(
        id: u64,
        thread_id: impl Into<String>,
        turn_id: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        let thread_id = thread_id.into();
        let turn_id = turn_id.into();
        let prompt = prompt.into();
        Self {
            id,
            method: CodexMethod::TurnSteer,
            params: json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "input": [
                    {
                        "type": "text",
                        "text": prompt
                    }
                ]
            }),
        }
    }

    pub fn turn_interrupt(
        id: u64,
        thread_id: impl Into<String>,
        turn_id: impl Into<String>,
    ) -> Self {
        let thread_id = thread_id.into();
        let turn_id = turn_id.into();
        Self {
            id,
            method: CodexMethod::TurnInterrupt,
            params: json!({
                "threadId": thread_id,
                "turnId": turn_id
            }),
        }
    }
}

fn thread_start_params(cwd: Option<String>, developer_instructions: Option<String>) -> Value {
    let mut params = serde_json::Map::new();
    if let Some(cwd) = cwd {
        params.insert("cwd".to_string(), Value::String(cwd));
    }
    if let Some(developer_instructions) = developer_instructions {
        params.insert(
            "collaborationMode".to_string(),
            json!({
                "settings": {
                    "developer_instructions": developer_instructions
                }
            }),
        );
    }
    Value::Object(params)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CodexResponse {
    Result(CodexResultResponse),
    Error(CodexErrorResponse),
    Event(CodexEvent),
    Raw(Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexResultResponse {
    pub id: u64,
    pub result: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexErrorResponse {
    pub id: u64,
    pub error: CodexError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CodexEvent {
    ThreadStarted {
        thread_id: String,
    },
    TurnStarted {
        thread_id: String,
        turn_id: String,
    },
    TurnDelta {
        thread_id: String,
        turn_id: String,
        text: String,
    },
    ToolCall {
        thread_id: String,
        turn_id: String,
        call_id: String,
        name: String,
        #[serde(default)]
        arguments: Value,
    },
    TurnCompleted {
        thread_id: String,
        turn_id: String,
        #[serde(default)]
        success: bool,
    },
    Interrupted {
        thread_id: String,
        turn_id: String,
    },
    Raw {
        #[serde(default)]
        payload: Value,
    },
}

#[derive(Debug, Default)]
pub struct PendingResponseDispatcher {
    pending: HashMap<u64, CodexMethod>,
}

impl PendingResponseDispatcher {
    pub fn register(&mut self, request: &CodexRequest) {
        self.pending.insert(request.id, request.method.clone());
    }

    pub fn complete(&mut self, response: &CodexResponse) -> Option<CodexMethod> {
        match response {
            CodexResponse::Result(response) => self.pending.remove(&response.id),
            CodexResponse::Error(response) => self.pending.remove(&response.id),
            CodexResponse::Event(_) | CodexResponse::Raw(_) => None,
        }
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}
