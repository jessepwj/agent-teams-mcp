use std::path::PathBuf;

use crate::error::Result;
use crate::team_mode::domain::{InboxItem, InboxStatus, Message};
use crate::team_mode::storage::message_store::MessageStore;

/// Compute-on-demand inbox and thread projections over a per-team message
/// transcript. Nothing is written to disk — projections are rebuilt from
/// `messages.jsonl` each time they are requested. For the current project
/// size (hundreds of messages) this is ~ms-level and avoids the
/// consistency burden of persisted caches.
#[derive(Debug, Clone)]
pub struct ProjectionStore {
    message_store: MessageStore,
}

impl ProjectionStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            message_store: MessageStore::new(base_dir),
        }
    }

    pub fn with_message_store(message_store: MessageStore) -> Self {
        Self { message_store }
    }

    pub fn project_inbox(
        &self,
        team_id: &str,
        recipient: impl AsRef<str>,
        thread_id: Option<&str>,
    ) -> Result<Vec<InboxItem>> {
        let recipient = recipient.as_ref();
        let messages = self.message_store.list(team_id)?;
        let mut items = Vec::new();

        for message in messages {
            if !message
                .effective_recipients
                .iter()
                .any(|candidate| candidate == recipient)
            {
                continue;
            }
            if let Some(thread) = thread_id {
                if message.thread_id.as_deref() != Some(thread) {
                    continue;
                }
            }

            items.push(InboxItem {
                message_id: message.id.clone(),
                recipient: recipient.to_string(),
                status: inbox_status(&message, recipient),
                created_at: message.created_at,
            });
        }

        items.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then(a.message_id.cmp(&b.message_id))
        });
        Ok(items)
    }

    pub fn project_thread(&self, team_id: &str, thread_id: impl AsRef<str>) -> Result<Vec<Message>> {
        let thread_id = thread_id.as_ref();
        let mut messages = self.message_store.list_by_thread(team_id, thread_id)?;
        messages.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(messages)
    }
}

fn inbox_status(message: &Message, recipient: &str) -> InboxStatus {
    if message
        .acked_by
        .iter()
        .any(|receipt| receipt.actor == recipient)
    {
        InboxStatus::Acked
    } else if message
        .read_by
        .iter()
        .any(|receipt| receipt.actor == recipient)
    {
        InboxStatus::Read
    } else {
        InboxStatus::Unread
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{
        AudiencePolicy, DeliveryDrop, DeliveryStatus, DropReason, Message, MessageKind,
        MessageReceipt, VisibilityReason, VisibilityResolution, VisibilityRule,
    };

    fn sample_message(team: &str, id: &str, recipient: &str, thread_id: &str) -> Message {
        Message {
            id: id.into(),
            room_id: "main".into(),
            team_id: Some(team.into()),
            thread_id: Some(thread_id.into()),
            reply_to: None,
            sender: "alice".into(),
            kind: MessageKind::Dispatch,
            subject: Some("Review".into()),
            body: "Please review.".into(),
            mentions: vec![recipient.into()],
            visibility: vec![VisibilityRule::Team],
            audience_policy: Some(AudiencePolicy::Team),
            effective_visibility_reason: Some(VisibilityResolution {
                policy: AudiencePolicy::Team,
                reasons: vec![VisibilityReason::TeamWide],
            }),
            effective_recipients: vec![recipient.into()],
            delivered_to: vec![recipient.into()],
            dropped_for: vec![DeliveryDrop {
                recipient: "other".into(),
                reason: DropReason::HiddenByVisibility,
            }],
            read_by: vec![MessageReceipt {
                actor: recipient.into(),
                at: Utc::now(),
            }],
            acked_by: vec![],
            delivery_status: DeliveryStatus::Delivered,
            created_at: Utc::now(),
            expires_at: None,
        }
    }

    #[test]
    fn inbox_projection_filters_non_recipients() {
        let dir = tempdir().unwrap();
        let store = MessageStore::new(dir.path());
        store
            .save(&sample_message("demo", "msg-1", "alice", "t1"))
            .unwrap();
        store
            .save(&sample_message("demo", "msg-2", "bob", "t1"))
            .unwrap();
        let projection = ProjectionStore::with_message_store(store);

        let inbox = projection.project_inbox("demo", "alice", None).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].status, InboxStatus::Read);
    }

    #[test]
    fn thread_projection_groups_by_thread_id() {
        let dir = tempdir().unwrap();
        let store = MessageStore::new(dir.path());
        store
            .save(&sample_message("demo", "msg-1", "alice", "t1"))
            .unwrap();
        store
            .save(&sample_message("demo", "msg-2", "alice", "t1"))
            .unwrap();
        store
            .save(&sample_message("demo", "msg-3", "alice", "t2"))
            .unwrap();
        let projection = ProjectionStore::with_message_store(store);

        let t1 = projection.project_thread("demo", "t1").unwrap();
        assert_eq!(t1.len(), 2);
        assert!(t1.iter().all(|m| m.thread_id.as_deref() == Some("t1")));
    }
}
