use serde::{Deserialize, Serialize};

/// Shared runtime technical state for managed sessions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSessionState {
    Unknown,
    Starting,
    Running,
    Paused,
    Stopped,
    Failed,
}

impl ExecutionSessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionSessionState::Unknown => "unknown",
            ExecutionSessionState::Starting => "starting",
            ExecutionSessionState::Running => "running",
            ExecutionSessionState::Paused => "paused",
            ExecutionSessionState::Stopped => "stopped",
            ExecutionSessionState::Failed => "failed",
        }
    }

    fn parse_label(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            "starting" => Some(Self::Starting),
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "stopped" => Some(Self::Stopped),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

impl std::str::FromStr for ExecutionSessionState {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_label(value).ok_or(())
    }
}
