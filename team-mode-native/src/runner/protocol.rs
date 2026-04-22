use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RunnerFrame {
    #[serde(rename = "runner/hello")]
    Hello(RunnerHelloFrame),
    #[serde(rename = "runner/heartbeat")]
    Heartbeat(RunnerHeartbeatFrame),
    #[serde(rename = "runner/output")]
    Output(RunnerOutputFrame),
    #[serde(rename = "runner/input_injected")]
    InputInjected(InputInjectedFrame),
    #[serde(rename = "runner/child_exit")]
    ChildExit(ChildExitFrame),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HostToRunnerFrame {
    #[serde(rename = "host/inject_input")]
    InjectInput(InjectInputFrame),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerHelloFrame {
    pub member_id: String,
    pub runner_id: String,
    pub protocol_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerHeartbeatFrame {
    pub member_id: String,
    pub runner_id: String,
    pub unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerOutputFrame {
    pub member_id: String,
    pub runner_id: String,
    pub stream: OutputStream,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
    Pty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputInjectedFrame {
    pub member_id: String,
    pub runner_id: String,
    pub injection_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildExitFrame {
    pub member_id: String,
    pub runner_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub success: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectInputFrame {
    #[serde(alias = "memberId")]
    pub member_id: String,
    #[serde(default, alias = "runnerId")]
    pub runner_id: String,
    #[serde(
        default,
        alias = "message_id",
        alias = "messageId",
        alias = "injectionId"
    )]
    pub injection_id: String,
    pub text: String,
    #[serde(default)]
    pub strategy: InjectStrategyWire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InjectStrategyWire {
    #[default]
    PasteAndEnter,
    BracketedPaste,
    CtrlC,
}
