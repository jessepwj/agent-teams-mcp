//! Message models for the file-based inbox system.
//!
//! Compatible with Claude Code's
//! `~/.claude/teams/{team-name}/inboxes/{agent-name}.json` format.
//!
//! Claude Code's native format uses `"text"` for the content field and has
//! no `"id"` or `"to"` fields (recipient is implicit from file path).
//! Our library extends the format with `"id"` and `"to"` for convenience,
//! and accepts both `"text"` and `"content"` during deserialization.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A message stored in an agent's inbox file.
///
/// The `content` field serializes as `"content"` but also accepts `"text"`
/// (Claude Code's native key name) during deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxMessage {
    /// Unique message ID (library extension — absent in native format).
    /// Defaults to empty string when parsing native messages without an ID.
    #[serde(default)]
    pub id: String,

    /// Sender agent name.
    pub from: String,

    /// Recipient agent name (library extension — absent in native format;
    /// the recipient is implicit from the inbox file name).
    /// Defaults to empty string when parsing native messages.
    #[serde(default)]
    pub to: String,

    /// Plain-text content (may also carry structured JSON).
    /// Serializes as `"content"`, deserializes from `"content"` or `"text"`.
    #[serde(alias = "text")]
    pub content: String,

    /// Summary for UI preview (5-10 word description).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// ISO 8601 timestamp.
    pub timestamp: DateTime<Utc>,

    /// Whether the message has been read.
    #[serde(default)]
    pub read: bool,

    /// Sender's UI color hint (e.g. `"blue"`, `"green"`, `"yellow"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl InboxMessage {
    /// Create a new plain-text message.
    pub fn new(from: impl Into<String>, to: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            from: from.into(),
            to: to.into(),
            content: content.into(),
            summary: None,
            timestamp: Utc::now(),
            read: false,
            color: None,
        }
    }

    /// Create a message from a structured payload.
    pub fn from_structured(
        from: impl Into<String>,
        to: impl Into<String>,
        msg: &StructuredMessage,
    ) -> serde_json::Result<Self> {
        let content = serde_json::to_string(msg)?;
        let summary = Some(msg.summary());
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            from: from.into(),
            to: to.into(),
            content,
            summary,
            timestamp: Utc::now(),
            read: false,
            color: None,
        })
    }

    /// Try to parse the content as a structured message.
    pub fn try_as_structured(&self) -> Option<StructuredMessage> {
        serde_json::from_str(&self.content).ok()
    }
}

/// Structured message types used by the agent teams protocol.
///
/// Serialized with `#[serde(tag = "type", rename_all = "snake_case")]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StructuredMessage {
    /// Assign a task to a teammate.
    ///
    /// Native format uses camelCase (`taskId`, `assignedBy`).
    TaskAssignment {
        #[serde(alias = "taskId")]
        task_id: String,
        subject: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// Who assigned this task (native format field).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(alias = "assignedBy")]
        assigned_by: Option<String>,
        /// Assignment timestamp (native format field).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<String>,
    },

    /// Request a teammate to shut down.
    ShutdownRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Acknowledge a shutdown request.
    ShutdownApproved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },

    /// Teammate idle notification.
    ///
    /// Native format may use `"from"` instead of `"agent"`, and includes
    /// `"idleReason"` with values like `"available"`, `"working"`, `"blocked"`.
    IdleNotification {
        /// Agent name. Also accepts `"from"` (native format).
        #[serde(alias = "from")]
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(alias = "lastTaskId")]
        last_task_id: Option<String>,
        /// Idle reason from native format.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(alias = "idleReason")]
        idle_reason: Option<String>,
        /// Timestamp from native format.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<String>,
    },

    /// Request plan approval from lead.
    PlanApprovalRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        plan: String,
    },

    /// Response to a plan approval request.
    PlanApprovalResponse {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
    },
}

impl StructuredMessage {
    /// A short human-readable summary of this message.
    pub fn summary(&self) -> String {
        match self {
            StructuredMessage::TaskAssignment { subject, .. } => {
                format!("Task assigned: {subject}")
            }
            StructuredMessage::ShutdownRequest { .. } => "Shutdown requested".into(),
            StructuredMessage::ShutdownApproved { .. } => "Shutdown approved".into(),
            StructuredMessage::IdleNotification { agent, .. } => {
                format!("{agent} is idle")
            }
            StructuredMessage::PlanApprovalRequest { .. } => "Plan approval requested".into(),
            StructuredMessage::PlanApprovalResponse { approved, .. } => {
                if *approved {
                    "Plan approved".into()
                } else {
                    "Plan rejected".into()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_structured_message() {
        let msg = StructuredMessage::TaskAssignment {
            task_id: "1".into(),
            subject: "Fix bug".into(),
            description: Some("Critical auth issue".into()),
            assigned_by: None,
            timestamp: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"task_assignment"#));

        let parsed: StructuredMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, StructuredMessage::TaskAssignment { .. }));
    }

    #[test]
    fn inbox_message_structured_round_trip() {
        let structured = StructuredMessage::ShutdownRequest {
            request_id: Some("req-1".into()),
            reason: Some("All done".into()),
        };

        let msg = InboxMessage::from_structured("lead", "worker", &structured).unwrap();
        assert!(msg.summary.is_some());

        let parsed = msg.try_as_structured().unwrap();
        assert!(matches!(parsed, StructuredMessage::ShutdownRequest { .. }));
    }

    #[test]
    fn deserialize_all_structured_variants() {
        let variants = vec![
            r#"{"type":"task_assignment","task_id":"1","subject":"Do thing"}"#,
            r#"{"type":"shutdown_request"}"#,
            r#"{"type":"shutdown_approved"}"#,
            r#"{"type":"idle_notification","agent":"worker-1"}"#,
            r#"{"type":"plan_approval_request","plan":"Step 1: ..."}"#,
            r#"{"type":"plan_approval_response","approved":true}"#,
        ];

        for json in variants {
            let msg: StructuredMessage = serde_json::from_str(json).unwrap();
            let _ = msg.summary(); // Should not panic
        }
    }

    #[test]
    fn deserialize_native_inbox_message_with_text_key() {
        // Real Claude Code inbox message uses "text" not "content",
        // and has no "id" or "to" fields.
        let json = r#"{
            "from": "team-lead",
            "text": "Hi! I see you're working on Task #1.",
            "timestamp": "2026-02-11T08:27:54.622Z",
            "read": true,
            "summary": "Task sequence guidance",
            "color": "blue"
        }"#;

        let msg: InboxMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.from, "team-lead");
        assert_eq!(msg.content, "Hi! I see you're working on Task #1.");
        assert_eq!(msg.summary.as_deref(), Some("Task sequence guidance"));
        assert_eq!(msg.color.as_deref(), Some("blue"));
        assert!(msg.read);
        // id and to default to empty strings when absent
        assert_eq!(msg.id, "");
        assert_eq!(msg.to, "");
    }

    #[test]
    fn deserialize_native_task_assignment_protocol() {
        // Real native task_assignment uses camelCase field names.
        let json = r#"{
            "type": "task_assignment",
            "taskId": "1",
            "subject": "Set up project structure",
            "description": "Create all directories...",
            "assignedBy": "team-lead",
            "timestamp": "2026-02-11T08:27:04.754Z"
        }"#;

        let msg: StructuredMessage = serde_json::from_str(json).unwrap();
        match msg {
            StructuredMessage::TaskAssignment {
                task_id,
                subject,
                description,
                assigned_by,
                timestamp,
            } => {
                assert_eq!(task_id, "1");
                assert_eq!(subject, "Set up project structure");
                assert!(description.is_some());
                assert_eq!(assigned_by.as_deref(), Some("team-lead"));
                assert_eq!(timestamp.as_deref(), Some("2026-02-11T08:27:04.754Z"));
            }
            _ => panic!("Expected TaskAssignment"),
        }
    }

    #[test]
    fn deserialize_native_idle_notification() {
        // Real native idle_notification uses "from" and "idleReason".
        let json = r#"{
            "type": "idle_notification",
            "from": "cc-writer",
            "timestamp": "2026-02-11T19:08:12.345Z",
            "idleReason": "available"
        }"#;

        let msg: StructuredMessage = serde_json::from_str(json).unwrap();
        match msg {
            StructuredMessage::IdleNotification {
                agent,
                idle_reason,
                timestamp,
                ..
            } => {
                assert_eq!(agent, "cc-writer");
                assert_eq!(idle_reason.as_deref(), Some("available"));
                assert!(timestamp.is_some());
            }
            _ => panic!("Expected IdleNotification"),
        }
    }

    #[test]
    fn deserialize_native_json_in_json_inbox() {
        // Real inbox: text field contains serialized JSON protocol message.
        let json = r#"{
            "from": "gemini-proxy",
            "text": "{\"type\":\"task_assignment\",\"taskId\":\"3\",\"subject\":\"Gemini proxy review\",\"assignedBy\":\"gemini-proxy\",\"timestamp\":\"2026-02-11T19:06:06.765Z\"}",
            "timestamp": "2026-02-11T19:06:06.765Z",
            "color": "yellow",
            "read": false
        }"#;

        let msg: InboxMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.from, "gemini-proxy");
        assert_eq!(msg.color.as_deref(), Some("yellow"));
        assert!(!msg.read);

        // Parse inner protocol message
        let structured = msg.try_as_structured().unwrap();
        match structured {
            StructuredMessage::TaskAssignment {
                task_id,
                subject,
                assigned_by,
                ..
            } => {
                assert_eq!(task_id, "3");
                assert_eq!(subject, "Gemini proxy review");
                assert_eq!(assigned_by.as_deref(), Some("gemini-proxy"));
            }
            _ => panic!("Expected TaskAssignment"),
        }
    }
}
