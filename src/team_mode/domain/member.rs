use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::runtime::ExecutionSessionState;

/// Identity layer for a team member. Scoped within a `team_id`.
/// `name` is unique within that team and serves as the @-mention handle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemberProfile {
    pub team_id: String,
    pub name: String,
    pub kind: MemberKind,
    pub role_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_description: Option<String>,
    pub status: MemberStatus,
    pub joined_at: DateTime<Utc>,
}

/// Execution layer for a member. A member's composite identity
/// (`team_id`, `name`) is carried by the enclosing `MemberRecord`; this
/// struct is pure execution configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionProfile {
    pub execution_mode: ExecutionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_state: Option<ExecutionSessionState>,
    /// Backend-assigned conversation session identifier. For Claude Code
    /// workers this is the UUID Claude Code assigns when it starts the
    /// stream-json session and uses for the
    /// `~/.claude/projects/<encoded-cwd>/<id>.jsonl` filename. The web UI
    /// uses it to display the worker's exact conversation rather than
    /// guessing by mtime among multiple jsonl files in the same directory.
    /// `None` for backends that don't expose a session id (codex, gemini-cli).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Reasoning-effort override for backends that take one (codex:
    /// `low|medium|high|xhigh`). `None` means "don't pass `-c
    /// model_reasoning_effort=…`" — the backend CLI then falls back to
    /// whatever the user has configured in their own config file (e.g.
    /// `~/.codex/config.toml`). We deliberately do NOT default to
    /// `"medium"` in this codebase; doing so would silently override a
    /// user who set `high` globally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// On-disk envelope for `<base>/<team>/members.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MembersFile {
    pub version: u32,
    #[serde(default)]
    pub members: Vec<UnifiedMember>,
}

/// One row in `members.json` — identity + optional execution merged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedMember {
    pub kind: MemberKind,
    pub name: String,
    pub status: MemberStatus,
    pub role_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_description: Option<String>,
    pub joined_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemberKind {
    Lead,
    Member,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    Active,
    Invited,
    Suspended,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    External,
    Managed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_profile_round_trip() {
        let profile = MemberProfile {
            team_id: "team-1".into(),
            name: "alice".into(),
            kind: MemberKind::Member,
            role_label: "reviewer".into(),
            role_description: Some("Reviews changes".into()),
            status: MemberStatus::Active,
            joined_at: Utc::now(),
        };

        let json = serde_json::to_string(&profile).unwrap();
        let parsed: MemberProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "alice");
        assert_eq!(parsed.kind, MemberKind::Member);
        assert_eq!(parsed.team_id, "team-1");
    }

    #[test]
    fn execution_profile_round_trip() {
        let execution = ExecutionProfile {
            execution_mode: ExecutionMode::Managed,
            adapter: Some("codex".into()),
            agent_name: Some("alice-agent".into()),
            model: Some("gpt-4.1".into()),
            cwd: Some("E:/workspace".into()),
            env: HashMap::new(),
            system_prompt: Some("You are Alice.".into()),
            skills: vec!["review".into()],
            session_state: Some(ExecutionSessionState::Running),
            session_id: None,
                    reasoning_effort: None,};

        let json = serde_json::to_string(&execution).unwrap();
        let parsed: ExecutionProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.execution_mode, ExecutionMode::Managed);
        assert_eq!(parsed.adapter.as_deref(), Some("codex"));
        assert_eq!(parsed.session_state, Some(ExecutionSessionState::Running));
    }

    #[test]
    fn members_file_round_trip_with_version() {
        let file = MembersFile {
            version: 1,
            members: vec![
                UnifiedMember {
                    kind: MemberKind::Lead,
                    name: "lead".into(),
                    status: MemberStatus::Active,
                    role_label: "lead".into(),
                    role_description: None,
                    joined_at: Utc::now(),
                    execution: None,
                },
                UnifiedMember {
                    kind: MemberKind::Member,
                    name: "alice".into(),
                    status: MemberStatus::Active,
                    role_label: "worker".into(),
                    role_description: None,
                    joined_at: Utc::now(),
                    execution: Some(ExecutionProfile {
                        execution_mode: ExecutionMode::Managed,
                        adapter: Some("claude-code".into()),
                        agent_name: Some("alice".into()),
                        model: None,
                        cwd: None,
                        env: HashMap::new(),
                        system_prompt: None,
                        skills: vec![],
                        session_state: Some(ExecutionSessionState::Running),
                        session_id: None,
                    reasoning_effort: None,}),
                },
            ],
        };

        let json = serde_json::to_string(&file).unwrap();
        let parsed: MembersFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.members.len(), 2);
        assert_eq!(parsed.members[0].name, "lead");
        assert!(parsed.members[0].execution.is_none());
        assert_eq!(parsed.members[1].name, "alice");
        assert!(parsed.members[1].execution.is_some());
    }
}
