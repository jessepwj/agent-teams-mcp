use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Projection of messages for a single recipient.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Inbox {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    pub recipient: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<InboxItem>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub message_id: String,
    pub recipient: String,
    pub status: InboxStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    Unread,
    Read,
    Acked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbox_round_trip() {
        let inbox = Inbox {
            team_id: Some("team-1".into()),
            recipient: "alice".into(),
            items: vec![InboxItem {
                message_id: "msg-1".into(),
                recipient: "alice".into(),
                status: InboxStatus::Unread,
                created_at: Utc::now(),
            }],
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&inbox).unwrap();
        let parsed: Inbox = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.recipient, "alice");
        assert_eq!(parsed.items.len(), 1);
    }
}
