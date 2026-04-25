use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Team aggregate for the Team Mode runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Team-level default working directory. Workers inherit this when
    /// their own `cwd` is not specified at spawn time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub status: TeamStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_member_id: Option<String>,
    /// PID of the Claude Code process that "owns" this team (i.e. the CC
    /// client whose MCP invocation created/last-took-over the team). Used
    /// by the push hook to route `lead_pending.jsonl` lines only to the
    /// owner CC, avoiding cross-CC races when multiple clients run in the
    /// same project. Ownership auto-reclaims if the PID is no longer
    /// alive (see MCP startup owner-scrub).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_cc_pid: Option<u32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Lifecycle state of a team.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TeamStatus {
    Active,
    Archived,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_round_trip() {
        let team = Team {
            id: "team-1".into(),
            name: "main".into(),
            description: Some("Team Mode".into()),
            cwd: Some("E:\\proj".into()),
            status: TeamStatus::Active,
            lead_member_id: Some("member-1".into()),
            owner_cc_pid: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&team).unwrap();
        let parsed: Team = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "team-1");
        assert_eq!(parsed.status, TeamStatus::Active);
    }
}
