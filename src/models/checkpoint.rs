//! Checkpoint data model for attaching agent session context to Git commits.
//!
//! Checkpoints are structured JSON metadata stored as Git notes
//! (`refs/notes/agent-checkpoints`). Each checkpoint captures the full
//! agent session context alongside the code it produced.
//!
//! Two data tiers:
//! - **Core** (always, <10KB): session, commit, branch, files, team state, task summaries
//! - **Extended** (optional, 10-100KB): tool_calls, token_usage, full metadata

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// Re-export token types that were extracted to the feature-independent token module.
// This preserves backward compatibility for code importing from `models::checkpoint::`.
pub use super::token::{
    AgentTokenUsage, CostSummary, MAX_PROMPT_SUMMARY_LEN, MAX_TOOL_INPUT_SUMMARY_LEN, TokenUsage,
    ToolCallRecord, estimate_cost, truncate_string,
};

/// Top-level checkpoint attached to a Git commit via notes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    /// Unique checkpoint ID (derived from commit SHA + timestamp).
    pub id: String,

    /// The Git commit SHA this checkpoint is attached to.
    pub commit_sha: String,

    /// Branch name at checkpoint creation time.
    pub branch: String,

    /// When the checkpoint was created.
    pub created_at: DateTime<Utc>,

    /// Agent session context.
    pub session: CheckpointSession,

    /// Team state snapshot (if working in a team).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<CheckpointTeamState>,

    /// Lightweight task snapshots from the team's task list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<CheckpointTask>,

    /// Files involved in this commit (from git diff).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<CheckpointFile>,

    /// Token usage statistics (extended tier).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,

    /// Tool call records (extended tier).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRecord>,

    /// Arbitrary metadata for extensibility.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Agent session context at checkpoint time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointSession {
    /// Agent name (e.g. "team-lead", "coder-1").
    pub agent_name: String,

    /// Backend type string (e.g. "claude-code", "codex", "gemini-cli").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_type: Option<String>,

    /// Model used (e.g. "claude-sonnet-4-5-20250929").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Truncated prompt summary (max 2000 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_summary: Option<String>,

    /// Working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,

    /// Session ID (for correlation with JSONL session files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Team state snapshot embedded in a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointTeamState {
    /// Team name.
    pub team_name: String,

    /// Team description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Team members at checkpoint time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<CheckpointMember>,
}

/// A team member snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointMember {
    /// Member name.
    pub name: String,
    /// Agent type (e.g. "general-purpose", "researcher").
    pub agent_type: String,
}

/// Lightweight task snapshot for checkpoint embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointTask {
    /// Task ID.
    pub id: String,
    /// Task subject.
    pub subject: String,
    /// Task status.
    pub status: String,
    /// Task owner (agent name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// Role of a file in a checkpoint (derived from git diff).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    Created,
    Modified,
    Deleted,
    Referenced,
}

impl std::fmt::Display for FileRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileRole::Created => write!(f, "created"),
            FileRole::Modified => write!(f, "modified"),
            FileRole::Deleted => write!(f, "deleted"),
            FileRole::Referenced => write!(f, "referenced"),
        }
    }
}

/// A file involved in a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointFile {
    /// File path relative to repo root.
    pub path: String,
    /// Role of the file in this commit.
    pub role: FileRole,
    /// SHA-256 hash of file content (for integrity verification).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

// TokenUsage, ToolCallRecord, MAX_PROMPT_SUMMARY_LEN, MAX_TOOL_INPUT_SUMMARY_LEN,
// and truncate_string are re-exported from super::token (see top of file).

impl Checkpoint {
    /// Create a new checkpoint with minimal required fields.
    pub fn new(
        commit_sha: impl Into<String>,
        branch: impl Into<String>,
        session: CheckpointSession,
    ) -> Self {
        let commit_sha = commit_sha.into();
        let now = Utc::now();
        let id = format!(
            "ckpt-{}-{}",
            &commit_sha[..7.min(commit_sha.len())],
            now.timestamp()
        );
        Self {
            id,
            commit_sha,
            branch: branch.into(),
            created_at: now,
            session,
            team: None,
            tasks: Vec::new(),
            files: Vec::new(),
            token_usage: None,
            tool_calls: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Check whether this checkpoint has extended data (tool calls or token usage).
    pub fn has_extended_data(&self) -> bool {
        self.token_usage.is_some() || !self.tool_calls.is_empty()
    }

    /// Approximate JSON size in bytes.
    pub fn estimated_size(&self) -> usize {
        // Quick estimate: serialize and check length
        serde_json::to_string(self).map(|s| s.len()).unwrap_or(0)
    }
}

impl CheckpointSession {
    /// Create a session from a `SessionState`.
    pub fn from_session_state(state: &crate::models::session::SessionState) -> Self {
        Self {
            agent_name: state.name.clone(),
            backend_type: Some(state.backend_type.clone()),
            model: state.model.clone(),
            prompt_summary: Some(truncate_string(&state.prompt, MAX_PROMPT_SUMMARY_LEN)),
            cwd: state.cwd.clone(),
            session_id: None,
        }
    }

    /// Create a minimal session with just an agent name.
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
            backend_type: None,
            model: None,
            prompt_summary: None,
            cwd: None,
            session_id: None,
        }
    }
}

impl CheckpointTeamState {
    /// Create from a `TeamConfig`.
    pub fn from_team_config(config: &crate::models::team::TeamConfig) -> Self {
        Self {
            team_name: config.team_name.clone(),
            description: config.description.clone(),
            members: config
                .members
                .iter()
                .map(|m| CheckpointMember {
                    name: m.name().to_string(),
                    agent_type: m.agent_type().to_string(),
                })
                .collect(),
        }
    }
}

impl CheckpointTask {
    /// Create from a `TaskFile`.
    pub fn from_task_file(task: &crate::models::task::TaskFile) -> Self {
        Self {
            id: task.id.clone(),
            subject: task.subject.clone(),
            status: task.status.to_string(),
            owner: task.owner.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_checkpoint() {
        let session = CheckpointSession {
            agent_name: "test-agent".into(),
            backend_type: Some("claude-code".into()),
            model: Some("claude-sonnet-4-5-20250929".into()),
            prompt_summary: Some("You are a helpful assistant.".into()),
            cwd: Some(PathBuf::from("/tmp/test")),
            session_id: None,
        };

        let checkpoint = Checkpoint {
            id: "ckpt-abc1234-1700000000".into(),
            commit_sha: "abc1234567890".into(),
            branch: "main".into(),
            created_at: Utc::now(),
            session,
            team: Some(CheckpointTeamState {
                team_name: "my-team".into(),
                description: Some("Test team".into()),
                members: vec![CheckpointMember {
                    name: "lead".into(),
                    agent_type: "team-lead".into(),
                }],
            }),
            tasks: vec![CheckpointTask {
                id: "1".into(),
                subject: "Fix bug".into(),
                status: "in_progress".into(),
                owner: Some("coder".into()),
            }],
            files: vec![CheckpointFile {
                path: "src/main.rs".into(),
                role: FileRole::Modified,
                content_hash: Some("abc123".into()),
            }],
            token_usage: Some(TokenUsage {
                input_tokens: 1000,
                output_tokens: 500,
                cache_read_tokens: Some(200),
                cache_write_tokens: None,
            }),
            tool_calls: vec![ToolCallRecord {
                tool_name: "Read".into(),
                input_summary: Some("src/main.rs".into()),
                timestamp: Some(Utc::now()),
            }],
            metadata: HashMap::new(),
        };

        let json = serde_json::to_string_pretty(&checkpoint).unwrap();
        let parsed: Checkpoint = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "ckpt-abc1234-1700000000");
        assert_eq!(parsed.commit_sha, "abc1234567890");
        assert_eq!(parsed.branch, "main");
        assert_eq!(parsed.session.agent_name, "test-agent");
        assert_eq!(parsed.team.as_ref().unwrap().team_name, "my-team");
        assert_eq!(parsed.tasks.len(), 1);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].role, FileRole::Modified);
        assert!(parsed.token_usage.is_some());
        assert_eq!(parsed.tool_calls.len(), 1);
    }

    #[test]
    fn truncate_string_works() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 8), "hello...");
        assert_eq!(truncate_string("", 5), "");
        assert_eq!(truncate_string("ab", 2), "ab");
    }

    #[test]
    fn checkpoint_new_generates_id() {
        let session = CheckpointSession::new("test-agent");
        let ckpt = Checkpoint::new("abc1234567890", "main", session);

        assert!(ckpt.id.starts_with("ckpt-abc1234-"));
        assert_eq!(ckpt.commit_sha, "abc1234567890");
        assert_eq!(ckpt.branch, "main");
        assert!(!ckpt.has_extended_data());
    }

    #[test]
    fn checkpoint_has_extended_data() {
        let session = CheckpointSession::new("test-agent");
        let mut ckpt = Checkpoint::new("abc1234567890", "main", session);

        assert!(!ckpt.has_extended_data());

        ckpt.token_usage = Some(TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: None,
            cache_write_tokens: None,
        });
        assert!(ckpt.has_extended_data());
    }

    #[test]
    fn file_role_display() {
        assert_eq!(FileRole::Created.to_string(), "created");
        assert_eq!(FileRole::Modified.to_string(), "modified");
        assert_eq!(FileRole::Deleted.to_string(), "deleted");
        assert_eq!(FileRole::Referenced.to_string(), "referenced");
    }

    #[test]
    fn checkpoint_session_from_session_state() {
        let state = crate::models::session::SessionState {
            name: "coder-1".into(),
            backend_type: "claude-code".into(),
            prompt: "You are a Rust expert.".into(),
            model: Some("claude-opus-4-6".into()),
            cwd: Some(PathBuf::from("/project")),
            max_turns: None,
            allowed_tools: vec![],
            permission_mode: None,
            reasoning_effort: None,
            env: HashMap::new(),
            memory_config: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        };

        let session = CheckpointSession::from_session_state(&state);
        assert_eq!(session.agent_name, "coder-1");
        assert_eq!(session.backend_type.as_deref(), Some("claude-code"));
        assert_eq!(session.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(
            session.prompt_summary.as_deref(),
            Some("You are a Rust expert.")
        );
    }

    #[test]
    fn checkpoint_task_from_task_file() {
        let task = crate::models::task::TaskFile {
            id: "42".into(),
            subject: "Implement auth".into(),
            description: Some("Add JWT tokens".into()),
            active_form: None,
            status: crate::models::task::TaskStatus::InProgress,
            owner: Some("coder".into()),
            blocks: vec![],
            blocked_by: vec![],
            metadata: None,
        };

        let ckpt_task = CheckpointTask::from_task_file(&task);
        assert_eq!(ckpt_task.id, "42");
        assert_eq!(ckpt_task.subject, "Implement auth");
        assert_eq!(ckpt_task.status, "in_progress");
        assert_eq!(ckpt_task.owner.as_deref(), Some("coder"));
    }

    #[test]
    fn deserialize_minimal_checkpoint() {
        let json = r#"{
            "id": "ckpt-test-1",
            "commitSha": "abc123",
            "branch": "main",
            "createdAt": "2025-01-01T00:00:00Z",
            "session": {
                "agentName": "test"
            }
        }"#;

        let ckpt: Checkpoint = serde_json::from_str(json).unwrap();
        assert_eq!(ckpt.id, "ckpt-test-1");
        assert!(ckpt.team.is_none());
        assert!(ckpt.tasks.is_empty());
        assert!(ckpt.files.is_empty());
        assert!(ckpt.token_usage.is_none());
        assert!(ckpt.tool_calls.is_empty());
    }
}
