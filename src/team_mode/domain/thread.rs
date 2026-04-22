use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Transcript thread projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    pub room_id: String,
    pub root_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_ids: Vec<String>,
    pub status: ThreadStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Open,
    Closed,
    Archived,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_round_trip() {
        let thread = Thread {
            id: "thread-1".into(),
            team_id: Some("team-1".into()),
            room_id: "main".into(),
            root_message_id: "msg-1".into(),
            subject: Some("Review".into()),
            message_ids: vec!["msg-1".into(), "msg-2".into()],
            status: ThreadStatus::Open,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&thread).unwrap();
        let parsed: Thread = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "thread-1");
        assert_eq!(parsed.status, ThreadStatus::Open);
    }
}
