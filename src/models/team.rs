//! Team configuration models compatible with Claude Code's
//! `~/.claude/teams/{team-name}/config.json` format.
//!
//! Supports both the full native format (uses `"name"` key, includes
//! `createdAt`, `leadAgentId`, `leadSessionId`, and rich member fields)
//! and the simplified format (uses `"teamName"` key, minimal members).

use serde::{Deserialize, Serialize};

/// Top-level team configuration.
///
/// Accepts both `"teamName"` (our canonical key via `rename_all = "camelCase"`)
/// and `"name"` (Claude Code's native key) during deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamConfig {
    /// Human-readable team name.
    /// Serializes as `"teamName"`, deserializes from `"teamName"` or `"name"`.
    #[serde(alias = "name")]
    pub team_name: String,

    /// Optional description/purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Unix timestamp in milliseconds when the team was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,

    /// Fully qualified lead agent ID (e.g. `"team-lead@my-team"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_agent_id: Option<String>,

    /// Session UUID of the lead agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_session_id: Option<String>,

    /// Team members (lead + teammates).
    #[serde(default)]
    pub members: Vec<MemberUnion>,
}

/// A team member — either a lead or a teammate.
///
/// Uses `#[serde(untagged)]` with `TeammateMember` first so that the
/// presence of `prompt` disambiguates teammates from the lead.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MemberUnion {
    /// A teammate (has `prompt` field).
    Teammate(TeammateMember),
    /// The team lead (no `prompt` field).
    Lead(LeadMember),
}

impl MemberUnion {
    /// Get the member name regardless of variant.
    pub fn name(&self) -> &str {
        match self {
            MemberUnion::Lead(m) => &m.name,
            MemberUnion::Teammate(m) => &m.name,
        }
    }

    /// Get the agent ID regardless of variant.
    pub fn agent_id(&self) -> &str {
        match self {
            MemberUnion::Lead(m) => &m.agent_id,
            MemberUnion::Teammate(m) => &m.agent_id,
        }
    }

    /// Get the agent type regardless of variant.
    pub fn agent_type(&self) -> &str {
        match self {
            MemberUnion::Lead(m) => &m.agent_type,
            MemberUnion::Teammate(m) => &m.agent_type,
        }
    }

    /// Get the model, if set.
    pub fn model(&self) -> Option<&str> {
        match self {
            MemberUnion::Lead(m) => m.model.as_deref(),
            MemberUnion::Teammate(m) => m.model.as_deref(),
        }
    }

    /// Get the working directory, if set.
    pub fn cwd(&self) -> Option<&str> {
        match self {
            MemberUnion::Lead(m) => m.cwd.as_deref(),
            MemberUnion::Teammate(m) => m.cwd.as_deref(),
        }
    }

    /// Returns `true` if this is a teammate (not a lead).
    pub fn is_teammate(&self) -> bool {
        matches!(self, MemberUnion::Teammate(_))
    }
}

/// Team lead member — no `prompt` field.
///
/// Includes optional fields from Claude Code's native format that are
/// absent in the simplified format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeadMember {
    pub name: String,
    pub agent_id: String,
    pub agent_type: String,

    /// Model identifier (e.g. `"claude-opus-4-6"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Unix timestamp (ms) when the member joined the team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joined_at: Option<u64>,

    /// Tmux pane ID, or `""` for lead, `"in-process"` for in-process agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_pane_id: Option<String>,

    /// Working directory path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Event subscriptions (currently unused, always `[]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriptions: Option<Vec<serde_json::Value>>,
}

/// Teammate member — distinguished by having a `prompt` field.
///
/// Contains all native Claude Code fields including those only present
/// on teammates (`color`, `planModeRequired`, `backendType`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeammateMember {
    pub name: String,
    pub agent_id: String,
    pub agent_type: String,

    /// The initial prompt / system instruction for this teammate.
    pub prompt: String,

    /// Model identifier (e.g. `"claude-opus-4-6"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// UI color hint: `"blue"`, `"green"`, `"yellow"`, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,

    /// Whether this agent must submit plans for lead approval before executing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mode_required: Option<bool>,

    /// Unix timestamp (ms) when the member joined the team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joined_at: Option<u64>,

    /// Tmux pane ID or `"in-process"` for in-process agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_pane_id: Option<String>,

    /// Working directory path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Event subscriptions (currently unused, always `[]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriptions: Option<Vec<serde_json::Value>>,

    /// Backend execution type: `"in-process"`, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_team_config() {
        let config = TeamConfig {
            team_name: "test-team".into(),
            description: Some("A test team".into()),
            created_at: None,
            lead_agent_id: None,
            lead_session_id: None,
            members: vec![
                MemberUnion::Lead(LeadMember {
                    name: "lead".into(),
                    agent_id: "lead-001".into(),
                    agent_type: "team-lead".into(),
                    model: None,
                    joined_at: None,
                    tmux_pane_id: None,
                    cwd: None,
                    subscriptions: None,
                }),
                MemberUnion::Teammate(TeammateMember {
                    name: "worker-1".into(),
                    agent_id: "w1-001".into(),
                    agent_type: "general-purpose".into(),
                    prompt: "You are a helpful coding assistant.".into(),
                    model: None,
                    color: None,
                    plan_mode_required: None,
                    joined_at: None,
                    tmux_pane_id: None,
                    cwd: None,
                    subscriptions: None,
                    backend_type: None,
                }),
            ],
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: TeamConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.team_name, "test-team");
        assert_eq!(parsed.members.len(), 2);
        assert!(!parsed.members[0].is_teammate());
        assert!(parsed.members[1].is_teammate());
    }

    #[test]
    fn deserialize_simplified_format() {
        // The simplified format uses "teamName" — this is our canonical key.
        let json = r#"{
            "teamName": "my-project",
            "description": "Working on feature X",
            "members": [
                {
                    "name": "team-lead",
                    "agentId": "abc-123",
                    "agentType": "researcher"
                },
                {
                    "name": "coder",
                    "agentId": "def-456",
                    "agentType": "general-purpose",
                    "prompt": "Implement the feature"
                }
            ]
        }"#;

        let config: TeamConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.team_name, "my-project");
        assert!(config.created_at.is_none());
        assert!(config.lead_agent_id.is_none());
        assert_eq!(config.members[1].name(), "coder");
        assert!(config.members[1].is_teammate());
    }

    #[test]
    fn deserialize_native_format_with_name_key() {
        // Real Claude Code native format uses "name" at the top level.
        let json = r#"{
            "name": "proxy-analysis",
            "description": "Three @teammates analyzing agent-teams",
            "createdAt": 1770836587799,
            "leadAgentId": "team-lead@proxy-analysis",
            "leadSessionId": "a2485d01-5a05-4089-9dd4-32a061a1a1c8",
            "members": [
                {
                    "agentId": "team-lead@proxy-analysis",
                    "name": "team-lead",
                    "agentType": "team-lead",
                    "model": "claude-opus-4-6",
                    "joinedAt": 1770836587799,
                    "tmuxPaneId": "",
                    "cwd": "/Users/test/project",
                    "subscriptions": []
                },
                {
                    "agentId": "cc-writer@proxy-analysis",
                    "name": "cc-writer",
                    "agentType": "general-purpose",
                    "model": "claude-opus-4-6",
                    "prompt": "You are @cc-writer...",
                    "color": "blue",
                    "planModeRequired": false,
                    "joinedAt": 1770836740207,
                    "tmuxPaneId": "in-process",
                    "cwd": "/Users/test/project",
                    "subscriptions": [],
                    "backendType": "in-process"
                }
            ]
        }"#;

        let config: TeamConfig = serde_json::from_str(json).unwrap();

        // Top-level fields
        assert_eq!(config.team_name, "proxy-analysis");
        assert_eq!(config.created_at, Some(1770836587799));
        assert_eq!(
            config.lead_agent_id.as_deref(),
            Some("team-lead@proxy-analysis")
        );
        assert_eq!(
            config.lead_session_id.as_deref(),
            Some("a2485d01-5a05-4089-9dd4-32a061a1a1c8")
        );

        // Lead member
        let lead = &config.members[0];
        assert!(!lead.is_teammate());
        assert_eq!(lead.name(), "team-lead");
        assert_eq!(lead.model(), Some("claude-opus-4-6"));
        assert_eq!(lead.cwd(), Some("/Users/test/project"));

        // Teammate member
        let teammate = &config.members[1];
        assert!(teammate.is_teammate());
        assert_eq!(teammate.name(), "cc-writer");
        assert_eq!(teammate.model(), Some("claude-opus-4-6"));
        if let MemberUnion::Teammate(tm) = teammate {
            assert_eq!(tm.color.as_deref(), Some("blue"));
            assert_eq!(tm.plan_mode_required, Some(false));
            assert_eq!(tm.backend_type.as_deref(), Some("in-process"));
            assert_eq!(tm.tmux_pane_id.as_deref(), Some("in-process"));
        } else {
            panic!("Expected Teammate variant");
        }
    }

    #[test]
    fn accessors_work_for_both_variants() {
        let lead = MemberUnion::Lead(LeadMember {
            name: "lead".into(),
            agent_id: "lead@team".into(),
            agent_type: "team-lead".into(),
            model: Some("claude-opus-4-6".into()),
            joined_at: Some(1000),
            tmux_pane_id: Some("".into()),
            cwd: Some("/tmp".into()),
            subscriptions: None,
        });

        let teammate = MemberUnion::Teammate(TeammateMember {
            name: "worker".into(),
            agent_id: "worker@team".into(),
            agent_type: "general-purpose".into(),
            prompt: "Do work".into(),
            model: Some("claude-sonnet-4-5-20250929".into()),
            color: Some("green".into()),
            plan_mode_required: Some(true),
            joined_at: Some(2000),
            tmux_pane_id: Some("in-process".into()),
            cwd: Some("/home".into()),
            subscriptions: Some(vec![]),
            backend_type: Some("in-process".into()),
        });

        assert_eq!(lead.name(), "lead");
        assert_eq!(lead.model(), Some("claude-opus-4-6"));
        assert_eq!(lead.cwd(), Some("/tmp"));
        assert!(!lead.is_teammate());

        assert_eq!(teammate.name(), "worker");
        assert_eq!(teammate.model(), Some("claude-sonnet-4-5-20250929"));
        assert_eq!(teammate.cwd(), Some("/home"));
        assert!(teammate.is_teammate());
    }
}
