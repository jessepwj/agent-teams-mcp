use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub team_id: String,
    pub room_id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub sender_member_id: String,
    pub kind: MessageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effective_recipients: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delivered_to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_for: Vec<DeliveryDrop>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_by: Vec<MessageReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acked_by: Vec<MessageReceipt>,
    pub delivery_status: DeliveryStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Discussion,
    Dispatch,
    Reply,
    Direct,
    System,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryDrop {
    pub recipient: String,
    pub reason: DropReason,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    UnknownRecipient,
    RemovedMember,
    NotMentioned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageReceipt {
    pub actor: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
    Delivered,
    Partial,
    Failed,
}
