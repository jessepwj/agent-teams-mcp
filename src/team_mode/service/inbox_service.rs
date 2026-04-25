use chrono::Utc;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::team_mode::domain::{Inbox, InboxStatus, MessageReceipt};
use crate::team_mode::storage::{MessageStore, ProjectionStore};
use crate::util::validate_name;

#[derive(Debug, Clone)]
pub struct InboxService {
    projection_store: ProjectionStore,
    message_store: MessageStore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InboxCount {
    pub total: usize,
    pub unread: usize,
    pub read: usize,
    pub acked: usize,
}

impl InboxService {
    pub fn new(projection_store: ProjectionStore, message_store: MessageStore) -> Self {
        Self {
            projection_store,
            message_store,
        }
    }

    pub fn peek(
        &self,
        team_id: &str,
        recipient: impl AsRef<str>,
        thread_id: Option<&str>,
    ) -> Result<Inbox> {
        let recipient = recipient.as_ref();
        validate_name(team_id)?;
        validate_name(recipient)?;
        if let Some(thread_id) = thread_id {
            validate_name(thread_id)?;
        }

        let items = self
            .projection_store
            .project_inbox(team_id, recipient, thread_id)?;
        let updated_at = items
            .last()
            .map(|item| item.created_at)
            .unwrap_or_else(Utc::now);

        Ok(Inbox {
            team_id: Some(team_id.to_string()),
            recipient: recipient.to_string(),
            items,
            updated_at,
        })
    }

    pub fn count(
        &self,
        team_id: &str,
        recipient: impl AsRef<str>,
        thread_id: Option<&str>,
    ) -> Result<InboxCount> {
        let inbox = self.peek(team_id, recipient, thread_id)?;
        let mut count = InboxCount {
            total: inbox.items.len(),
            unread: 0,
            read: 0,
            acked: 0,
        };
        for item in inbox.items {
            match item.status {
                InboxStatus::Unread => count.unread += 1,
                InboxStatus::Read => count.read += 1,
                InboxStatus::Acked => count.acked += 1,
            }
        }
        Ok(count)
    }

    pub fn read(
        &self,
        team_id: &str,
        recipient: impl AsRef<str>,
        message_ids: &[String],
    ) -> Result<usize> {
        self.apply_receipt(team_id, recipient, message_ids, false)
    }

    pub fn ack(
        &self,
        team_id: &str,
        recipient: impl AsRef<str>,
        message_ids: &[String],
    ) -> Result<usize> {
        self.apply_receipt(team_id, recipient, message_ids, true)
    }

    fn apply_receipt(
        &self,
        team_id: &str,
        recipient: impl AsRef<str>,
        message_ids: &[String],
        ack: bool,
    ) -> Result<usize> {
        let recipient = recipient.as_ref();
        validate_name(team_id)?;
        validate_name(recipient)?;

        let mut updated = 0;
        for message_id in message_ids {
            validate_name(message_id)?;
            let changed = self.message_store.update(team_id, message_id, |message| {
                if !message
                    .effective_recipients
                    .iter()
                    .any(|candidate| candidate == recipient)
                {
                    return Err(Error::Other(format!(
                        "message '{message_id}' is not visible to recipient '{recipient}'"
                    )));
                }

                let mut changed = ensure_receipt(&mut message.read_by, recipient);
                if ack {
                    changed |= ensure_receipt(&mut message.acked_by, recipient);
                }
                Ok(changed)
            })?;
            if changed.is_some_and(|result| result.changed) {
                updated += 1;
            }
        }
        Ok(updated)
    }
}

fn ensure_receipt(receipts: &mut Vec<MessageReceipt>, actor: &str) -> bool {
    if receipts.iter().any(|receipt| receipt.actor == actor) {
        return false;
    }
    receipts.push(MessageReceipt {
        actor: actor.to_string(),
        at: Utc::now(),
    });
    true
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{
        AudiencePolicy, DeliveryDrop, DeliveryStatus, DropReason, Message, MessageKind,
        MessageReceipt, VisibilityReason, VisibilityResolution, VisibilityRule,
    };
    use crate::team_mode::storage::MessageStore;

    fn sample_message(team: &str, id: &str, recipient: &str) -> Message {
        Message {
            id: id.into(),
            room_id: "main".into(),
            team_id: Some(team.into()),
            thread_id: Some("thread-1".into()),
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
    fn peek_returns_messages_for_recipient() {
        let dir = tempdir().unwrap();
        let store = MessageStore::new(dir.path());
        store.save(&sample_message("demo", "m1", "alice")).unwrap();
        let service = InboxService::new(ProjectionStore::with_message_store(store.clone()), store);

        let inbox = service.peek("demo", "alice", None).unwrap();
        assert_eq!(inbox.recipient, "alice");
        assert_eq!(inbox.items.len(), 1);
    }

    #[test]
    fn read_and_ack_update_receipts() {
        let dir = tempdir().unwrap();
        let store = MessageStore::new(dir.path());
        let mut m = sample_message("demo", "m1", "alice");
        m.read_by.clear();
        store.save(&m).unwrap();
        let service = InboxService::new(
            ProjectionStore::with_message_store(store.clone()),
            store.clone(),
        );

        assert_eq!(
            service
                .read("demo", "alice", &[String::from("m1")])
                .unwrap(),
            1
        );
        assert_eq!(
            service.ack("demo", "alice", &[String::from("m1")]).unwrap(),
            1
        );

        let after = store.get("demo", "m1").unwrap().unwrap();
        assert_eq!(after.read_by.len(), 1);
        assert_eq!(after.acked_by.len(), 1);
    }

    #[test]
    fn count_breaks_down_inbox_statuses() {
        let dir = tempdir().unwrap();
        let store = MessageStore::new(dir.path());

        let mut unread = sample_message("demo", "m1", "alice");
        unread.read_by.clear();
        let read = sample_message("demo", "m2", "alice");
        let mut acked = sample_message("demo", "m3", "alice");
        acked.acked_by.push(MessageReceipt {
            actor: "alice".into(),
            at: Utc::now(),
        });
        store.save(&unread).unwrap();
        store.save(&read).unwrap();
        store.save(&acked).unwrap();

        let service = InboxService::new(ProjectionStore::with_message_store(store.clone()), store);
        let count = service.count("demo", "alice", None).unwrap();
        assert_eq!(count.total, 3);
        assert_eq!(count.unread, 1);
        assert_eq!(count.read, 1);
        assert_eq!(count.acked, 1);
    }
}
