use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct ClaudeCodeOptions {
    pub model: Option<String>,
    pub cwd: Option<PathBuf>,
    pub max_turns: Option<i32>,
    pub allowed_tools: Vec<String>,
    pub permission_mode: PermissionMode,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub enum PermissionMode {
    #[default]
    Default,
    Plan,
    AcceptEdits,
    BypassPermissions,
}

#[derive(Debug, Clone)]
pub struct TextBlock {
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text(TextBlock),
    ToolUse,
}

#[derive(Debug, Clone, Default)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Assistant { message: AssistantMessage },
    Result { is_error: bool },
    System,
}

#[derive(Debug, Clone)]
pub struct SdkError {
    message: String,
}

impl SdkError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SdkError {}

#[derive(Debug, Default)]
pub struct InteractiveClient {
    connected: bool,
    pending_response: Option<Vec<Message>>,
}

impl InteractiveClient {
    pub fn new(_options: ClaudeCodeOptions) -> Result<Self, SdkError> {
        Ok(Self::default())
    }

    pub async fn connect(&mut self) -> Result<(), SdkError> {
        self.connected = true;
        Ok(())
    }

    pub async fn disconnect(&mut self) -> Result<(), SdkError> {
        self.connected = false;
        Ok(())
    }

    pub async fn send_message(&mut self, input: String) -> Result<(), SdkError> {
        if !self.connected {
            return Err(SdkError::new("client is not connected"));
        }

        self.pending_response = Some(vec![
            Message::Assistant {
                message: AssistantMessage {
                    content: vec![ContentBlock::Text(TextBlock { text: input })],
                },
            },
            Message::Result { is_error: false },
        ]);
        Ok(())
    }

    pub async fn receive_response(&mut self) -> Result<Vec<Message>, SdkError> {
        if !self.connected {
            return Err(SdkError::new("client is not connected"));
        }

        Ok(self
            .pending_response
            .take()
            .unwrap_or_else(|| vec![Message::Result { is_error: false }]))
    }
}
