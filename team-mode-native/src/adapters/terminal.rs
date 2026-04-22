use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<EnvVar>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptFileSpec {
    pub path: PathBuf,
    pub contents: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudePromptMode {
    Append,
    Replace,
}

pub fn prompt_file_spec(path: impl Into<PathBuf>, contents: impl Into<String>) -> PromptFileSpec {
    PromptFileSpec {
        path: path.into(),
        contents: contents.into(),
    }
}

pub fn claude_command_spec(
    claude_bin: impl Into<String>,
    prompt_file: &Path,
    prompt_mode: ClaudePromptMode,
    mcp_config_file: Option<&Path>,
    cwd: Option<&Path>,
    extra_args: impl IntoIterator<Item = impl Into<String>>,
) -> CommandSpec {
    let mut args = Vec::new();
    match prompt_mode {
        ClaudePromptMode::Append => args.push("--append-system-prompt-file".to_string()),
        ClaudePromptMode::Replace => args.push("--system-prompt-file".to_string()),
    }
    args.push(prompt_file.display().to_string());

    if let Some(path) = mcp_config_file {
        args.push("--mcp-config".to_string());
        args.push(path.display().to_string());
    }

    args.extend(extra_args.into_iter().map(Into::into));

    CommandSpec {
        program: claude_bin.into(),
        args,
        env: Vec::new(),
        cwd: cwd.map(Path::to_path_buf),
    }
}

pub fn gemini_command_spec(
    gemini_bin: impl Into<String>,
    system_prompt_file: &Path,
    cwd: Option<&Path>,
    extra_args: impl IntoIterator<Item = impl Into<String>>,
) -> CommandSpec {
    CommandSpec {
        program: gemini_bin.into(),
        args: extra_args.into_iter().map(Into::into).collect(),
        env: vec![gemini_system_env(system_prompt_file)],
        cwd: cwd.map(Path::to_path_buf),
    }
}

pub fn gemini_system_env(system_prompt_file: &Path) -> EnvVar {
    EnvVar {
        key: "GEMINI_SYSTEM_MD".to_string(),
        value: system_prompt_file.display().to_string(),
    }
}

pub fn team_mode_mcp_config(
    proxy_command: impl Into<String>,
    host: impl Into<String>,
    member_id: impl Into<String>,
    token_env: impl Into<String>,
) -> serde_json::Value {
    json!({
        "mcpServers": {
            "team-mode": {
                "command": proxy_command.into(),
                "args": [
                    "--host",
                    host.into(),
                    "--member-id",
                    member_id.into(),
                    "--token-env",
                    token_env.into()
                ]
            }
        }
    })
}
