//! Lead pending queue writer.
//!
//! Appends a JSON line to `<base_dir>/lead_pending.jsonl` whenever a message
//! is delivered to the team's lead. The file is watched by a Claude Code
//! `FileChanged` hook whose `asyncRewake: true` command reads the queue and
//! injects the content as a `<system-reminder>`, waking Claude even when
//! the session is idle.
//!
//! The writer is intentionally best-effort: a failure to append must never
//! block the underlying message send. Inbox projections and the MCP
//! `inbox_read` tool remain the source of truth; this file is only a
//! notification sidecar.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::team_mode::domain::{Message, MessageKind};
use crate::util::file_lock::FileLock;

/// Filename for the pending queue, relative to `base_dir`.
pub const PENDING_FILENAME: &str = "lead_pending.jsonl";
const LOCK_FILENAME: &str = ".lead_pending.lock";

/// One line in the pending queue. Serialized with snake_case keys so the
/// hook script can decode it with either JSON or casual grep/jq.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct LeadPendingEntry {
    pub team: String,
    pub from: String,
    pub from_id: String,
    pub msg_id: String,
    pub kind: String,
    pub text: String,
    pub ts: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LeadPendingWriter {
    base_dir: PathBuf,
}

impl LeadPendingWriter {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.base_dir.join(PENDING_FILENAME)
    }

    /// Append an entry if the message's `effective_recipients` include the
    /// lead. Returns `Ok(true)` when an entry was written.
    pub fn maybe_write(
        &self,
        message: &Message,
        lead_member_id: &str,
        from_display_name: &str,
    ) -> Result<bool> {
        if !message
            .effective_recipients
            .iter()
            .any(|r| r == lead_member_id)
        {
            return Ok(false);
        }

        fs::create_dir_all(&self.base_dir)?;
        let _lock = FileLock::acquire(&self.base_dir.join(LOCK_FILENAME))?;

        let entry = LeadPendingEntry {
            team: message.team_id.clone().unwrap_or_default(),
            from: from_display_name.to_string(),
            from_id: message.sender.clone(),
            msg_id: message.id.clone(),
            kind: kind_to_str(&message.kind).to_string(),
            text: message.body.clone(),
            ts: message.created_at,
            reply_to: message.reply_to.clone(),
        };

        let json = serde_json::to_string(&entry)?;
        let path = self.path();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{json}")?;
        Ok(true)
    }
}

fn kind_to_str(kind: &MessageKind) -> &'static str {
    match kind {
        MessageKind::Dispatch => "dispatch",
        MessageKind::Discussion => "discussion",
        MessageKind::Reply => "reply",
        MessageKind::System => "system",
        MessageKind::Notice => "notice",
        MessageKind::Status => "status",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{DeliveryStatus, Message, MessageKind};

    fn sample_message(recipients: Vec<&str>) -> Message {
        Message {
            id: "msg-1".into(),
            room_id: "main".into(),
            team_id: Some("demo".into()),
            thread_id: Some("thread-1".into()),
            reply_to: Some("msg-0".into()),
            sender: "demo-alice".into(),
            kind: MessageKind::Reply,
            subject: None,
            body: "hello lead".into(),
            mentions: Vec::new(),
            visibility: Vec::new(),
            audience_policy: None,
            effective_visibility_reason: None,
            effective_recipients: recipients.into_iter().map(String::from).collect(),
            delivered_to: Vec::new(),
            dropped_for: Vec::new(),
            read_by: Vec::new(),
            acked_by: Vec::new(),
            delivery_status: DeliveryStatus::Delivered,
            created_at: Utc::now(),
            expires_at: None,
        }
    }

    #[test]
    fn writes_entry_when_lead_is_recipient() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());

        let msg = sample_message(vec!["demo-lead", "demo-bob"]);
        let wrote = writer.maybe_write(&msg, "demo-lead", "alice").unwrap();
        assert!(wrote);

        let content = fs::read_to_string(writer.path()).unwrap();
        assert_eq!(content.lines().count(), 1);
        let entry: LeadPendingEntry = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(entry.team, "demo");
        assert_eq!(entry.from, "alice");
        assert_eq!(entry.from_id, "demo-alice");
        assert_eq!(entry.kind, "reply");
        assert_eq!(entry.text, "hello lead");
        assert_eq!(entry.msg_id, "msg-1");
        assert_eq!(entry.reply_to.as_deref(), Some("msg-0"));
    }

    #[test]
    fn skips_when_lead_not_recipient() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());

        let msg = sample_message(vec!["demo-bob"]);
        let wrote = writer.maybe_write(&msg, "demo-lead", "alice").unwrap();
        assert!(!wrote);
        assert!(!writer.path().exists());
    }

    #[test]
    fn appends_multiple_entries() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());

        let msg = sample_message(vec!["demo-lead"]);
        writer.maybe_write(&msg, "demo-lead", "alice").unwrap();
        writer.maybe_write(&msg, "demo-lead", "alice").unwrap();
        writer.maybe_write(&msg, "demo-lead", "alice").unwrap();

        let content = fs::read_to_string(writer.path()).unwrap();
        assert_eq!(content.lines().count(), 3);
    }
}
