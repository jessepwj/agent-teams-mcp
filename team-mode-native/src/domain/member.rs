use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::ExecutionProfile;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemberProfile {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub kind: MemberKind,
    pub handle: String,
    pub role_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_description: Option<String>,
    pub status: MemberStatus,
    pub joined_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemberRecord {
    pub profile: MemberProfile,
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
