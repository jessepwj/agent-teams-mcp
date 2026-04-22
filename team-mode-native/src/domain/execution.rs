use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionProfile {
    pub member_id: String,
    pub adapter: AdapterKind,
    pub launch_mode: LaunchMode,
    pub viewer_mode: ViewerMode,
    pub command: CommandSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub system_prompt: String,
    pub prompt_mode: PromptMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_config: Option<PathBuf>,
    pub restart_policy: RestartPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterKind {
    ClaudeCodeTerminal,
    GeminiCliTerminal,
    CodexAppServer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    NativeTerminalPty,
    AppServerStdio,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ViewerMode {
    NativeTerminal,
    EventViewer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    Append,
    Replace,
    DeveloperInstructions,
    BootstrapTurn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    Never,
    OnFailure,
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandSpec {
    pub program: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

impl ExecutionProfile {
    pub fn terminal(
        member_id: impl Into<String>,
        adapter: AdapterKind,
        program: impl Into<String>,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            member_id: member_id.into(),
            adapter,
            launch_mode: LaunchMode::NativeTerminalPty,
            viewer_mode: ViewerMode::NativeTerminal,
            command: CommandSpec {
                program: program.into(),
                args: Vec::new(),
            },
            cwd: None,
            env: BTreeMap::new(),
            model: None,
            reasoning_effort: None,
            system_prompt: system_prompt.into(),
            prompt_mode: PromptMode::Append,
            mcp_config: None,
            restart_policy: RestartPolicy::Never,
        }
    }
}
