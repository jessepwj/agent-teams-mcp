//! Message domain types.
//!
//! # Routing rules actually enforced today
//!
//! Despite the breadth of `AudiencePolicy` / `VisibilityRule` variants
//! defined here, the live send pipeline (`MessageService::send`)
//! implements exactly **two** rules — everything else is a reserved
//! placeholder for future routing modes (role-based fan-out, public/
//! private rooms, custom policies). They are stored on the message but
//! NOT consulted to decide who receives it.
//!
//! ```text
//! 1. effective_recipients = @mentions parsed from the body
//!      (case-insensitive; unknown / removed names go to dropped_for)
//! 2. lead observability: if sender ≠ lead, lead is auto-added to
//!      effective_recipients so the team's coordinator never falls behind.
//! ```
//!
//! Worker A → Worker B replies are **not** auto-routed; A must spell
//! `@b`. This was a deliberate fix for an LLM ping-pong loop where two
//! workers kept polite-acknowledging each other forever (Bug 12). Lead
//! observability is the only "automatic" recipient.
//!
//! When you add a new variant of `AudiencePolicy` / `VisibilityRule`,
//! make sure `MessageService::send` is taught to honor it — the type
//! system won't catch a forgotten match arm because we currently dispatch
//! on a single `Mentions` path. Read-only consumers (web UI, MCP
//! resources) can store unknown variants safely; they're surfaced as
//! diagnostic metadata only.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Transcript-first message record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub room_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub sender: String,
    pub kind: MessageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visibility: Vec<VisibilityRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience_policy: Option<AudiencePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_visibility_reason: Option<VisibilityResolution>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Dispatch,
    Discussion,
    Reply,
    System,
    Notice,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityRule {
    Public,
    Team,
    User,
    Lead,
    Member(String),
    Handle(String),
    Role(String),
    Members(Vec<String>),
    Handles(Vec<String>),
    Roles(Vec<String>),
}

/// High-level routing policy used to derive the audience of a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudiencePolicy {
    Public,
    Team,
    Direct,
    Mentions,
    Handles(Vec<String>),
    Roles(Vec<String>),
    Members(Vec<String>),
    Custom(String),
}

/// Explanation of why a message became visible to its final audience.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VisibilityResolution {
    pub policy: AudiencePolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<VisibilityReason>,
}

/// Fine-grained visibility explanation used in transcript projections.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityReason {
    ExplicitMention,
    TeamWide,
    DirectRecipient,
    RoleMatch,
    HandleMatch,
    UserVisible,
    LeadVisible,
    ThreadVisible,
    Custom(String),
}

/// A delivery drop entry explaining why a recipient did not receive a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryDrop {
    pub recipient: String,
    pub reason: DropReason,
}

/// Why a recipient was dropped from delivery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    HiddenByVisibility,
    NotMentioned,
    RemovedMember,
    UnknownRecipient,
    Expired,
    AlreadyDelivered,
    Custom(String),
}

/// Read or ack receipt recorded on the transcript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageReceipt {
    pub actor: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Draft,
    Pending,
    Delivered,
    Partial,
    Failed,
    Expired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_round_trip() {
        let message = Message {
            id: "msg-1".into(),
            room_id: "main".into(),
            team_id: Some("team-1".into()),
            thread_id: Some("thread-1".into()),
            reply_to: Some("msg-0".into()),
            sender: "alice".into(),
            kind: MessageKind::Dispatch,
            subject: Some("Review".into()),
            body: "Please review the change.".into(),
            mentions: vec!["bob".into()],
            visibility: vec![VisibilityRule::Team],
            audience_policy: Some(AudiencePolicy::Team),
            effective_visibility_reason: Some(VisibilityResolution {
                policy: AudiencePolicy::Team,
                reasons: vec![VisibilityReason::TeamWide],
            }),
            effective_recipients: vec!["bob".into()],
            delivered_to: vec!["bob".into()],
            dropped_for: vec![DeliveryDrop {
                recipient: "charlie".into(),
                reason: DropReason::HiddenByVisibility,
            }],
            read_by: vec![MessageReceipt {
                actor: "bob".into(),
                at: Utc::now(),
            }],
            acked_by: vec![],
            delivery_status: DeliveryStatus::Delivered,
            created_at: Utc::now(),
            expires_at: None,
        };

        let json = serde_json::to_string(&message).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "msg-1");
        assert_eq!(parsed.delivery_status, DeliveryStatus::Delivered);
        assert_eq!(parsed.dropped_for.len(), 1);
        assert_eq!(parsed.read_by.len(), 1);
    }
}
