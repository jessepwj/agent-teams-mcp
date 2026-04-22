use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub message_id: String,
    pub team_id: String,
    pub room_id: String,
    pub thread_id: String,
    pub sender_member_id: String,
    pub unread: bool,
    pub unacked: bool,
    pub delivered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InboxCounts {
    pub total: usize,
    pub unread: usize,
    pub unacked: usize,
}
