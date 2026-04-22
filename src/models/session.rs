//! Persisted session state for agent resume across orchestrator restarts.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Persisted state for a single agent session.
///
/// Stored at `~/.claude/teams/{team}/sessions/{agent}.json` and used to
/// re-spawn agents with the same configuration after an orchestrator restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    /// Agent name.
    pub name: String,
    /// Backend type string (e.g. "claude-code", "codex", "gemini-cli").
    pub backend_type: String,
    /// The prompt / system instruction.
    pub prompt: String,
    /// Model override (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Max turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<i32>,
    /// Allowed tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// Permission mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Reasoning effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Extra environment variables.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Memory configuration for cross-turn context injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_config: Option<crate::memory::MemoryConfig>,
    /// Backend-specific metadata (e.g. Codex thread ID for informational purposes).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// Timestamp when the session was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl SessionState {
    /// Create a new session state from a spawn config and backend type.
    pub fn from_config(
        config: &crate::backend::SpawnConfig,
        backend_type: &crate::backend::BackendType,
    ) -> Self {
        Self {
            name: config.name.clone(),
            backend_type: backend_type.to_string(),
            prompt: config.prompt.clone(),
            model: config.model.clone(),
            cwd: config.cwd.clone(),
            max_turns: config.max_turns,
            allowed_tools: config.allowed_tools.clone(),
            permission_mode: config.permission_mode.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
            env: config.env.clone(),
            memory_config: config.memory_config.clone(),
            // Note: delegations are not persisted separately because the prompt
            // is already augmented with delegation instructions before this is called.
            metadata: HashMap::new(),
            created_at: chrono::Utc::now(),
        }
    }

    /// Convert back to a SpawnConfig for re-spawning.
    pub fn to_spawn_config(&self) -> crate::backend::SpawnConfig {
        crate::backend::SpawnConfig {
            name: self.name.clone(),
            prompt: self.prompt.clone(),
            model: self.model.clone(),
            cwd: self.cwd.clone(),
            max_turns: self.max_turns,
            allowed_tools: self.allowed_tools.clone(),
            permission_mode: self.permission_mode.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            env: self.env.clone(),
            memory_config: self.memory_config.clone(),
            // Delegations are not restored — the prompt already contains
            // the baked-in delegation instructions from the original spawn.
            delegations: Vec::new(),
        }
    }

    /// Parse the backend type string back to a BackendType enum.
    ///
    /// Uses `BackendType::from_str()` so this stays in sync with `Display`.
    pub fn parse_backend_type(&self) -> Option<crate::backend::BackendType> {
        self.backend_type.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_state_round_trip() {
        let state = SessionState {
            name: "test-agent".into(),
            backend_type: "codex".into(),
            prompt: "You are a test assistant.".into(),
            model: Some("gpt-4.1".into()),
            cwd: Some(PathBuf::from("/tmp")),
            max_turns: None,
            allowed_tools: vec![],
            permission_mode: None,
            reasoning_effort: Some("medium".into()),
            env: HashMap::new(),
            memory_config: None,
            metadata: {
                let mut m = HashMap::new();
                m.insert("thread_id".into(), "abc-123".into());
                m
            },
            created_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string_pretty(&state).unwrap();
        let parsed: SessionState = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.name, "test-agent");
        assert_eq!(parsed.backend_type, "codex");
        assert_eq!(parsed.model.as_deref(), Some("gpt-4.1"));
        assert_eq!(parsed.metadata.get("thread_id").unwrap(), "abc-123");
    }

    #[test]
    fn from_config_and_back() {
        let config = crate::backend::SpawnConfig::new("reviewer", "You review code.");
        let state = SessionState::from_config(&config, &crate::backend::BackendType::GeminiCli);

        assert_eq!(state.name, "reviewer");
        assert_eq!(state.backend_type, "gemini-cli");
        assert_eq!(
            state.parse_backend_type(),
            Some(crate::backend::BackendType::GeminiCli)
        );

        let restored = state.to_spawn_config();
        assert_eq!(restored.name, "reviewer");
        assert_eq!(restored.prompt, "You review code.");
    }
}
