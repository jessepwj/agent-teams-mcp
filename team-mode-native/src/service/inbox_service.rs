use chrono::Utc;

use crate::domain::{InboxCounts, InboxItem, Message, MessageReceipt};
use crate::storage::JsonFileStore;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct InboxService {
    store: JsonFileStore,
}

impl InboxService {
    pub fn new(store: JsonFileStore) -> Self {
        Self { store }
    }

    pub fn peek(&self, team_id: &str, member_id: &str) -> Result<Vec<InboxItem>> {
        let mut items: Vec<InboxItem> = self
            .store
            .load_messages()?
            .iter()
            .filter(|message| is_delivered_to(message, team_id, member_id))
            .map(|message| to_item(message, member_id))
            .collect();
        items.sort_by_key(|item| item.delivered_at);
        Ok(items)
    }

    pub fn read(
        &self,
        team_id: &str,
        member_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Message>> {
        let mut messages = self.store.load_messages()?;
        let mut selected = Vec::new();
        let mut changed = false;
        let limit = limit.unwrap_or(usize::MAX);

        for message in messages
            .iter_mut()
            .filter(|message| is_delivered_to(message, team_id, member_id))
        {
            if selected.len() >= limit {
                break;
            }
            if !has_receipt(&message.read_by, member_id) {
                message.read_by.push(receipt(member_id));
                changed = true;
            }
            selected.push(message.clone());
        }

        if changed {
            self.store.save_messages(&messages)?;
        }
        Ok(selected)
    }

    pub fn ack(&self, team_id: &str, member_id: &str, message_id: &str) -> Result<Message> {
        let mut messages = self.store.load_messages()?;
        let message = messages
            .iter_mut()
            .find(|message| message.team_id == team_id && message.id == message_id)
            .ok_or_else(|| Error::NotFound(format!("message: {message_id}")))?;
        if !message
            .effective_recipients
            .iter()
            .any(|id| id == member_id)
        {
            return Err(Error::Invalid(format!(
                "message {message_id} is not in member inbox: {member_id}"
            )));
        }
        if !has_receipt(&message.read_by, member_id) {
            message.read_by.push(receipt(member_id));
        }
        if !has_receipt(&message.acked_by, member_id) {
            message.acked_by.push(receipt(member_id));
        }
        let updated = message.clone();
        self.store.save_messages(&messages)?;
        Ok(updated)
    }

    pub fn count(&self, team_id: &str, member_id: &str) -> Result<InboxCounts> {
        let items = self.peek(team_id, member_id)?;
        Ok(InboxCounts {
            total: items.len(),
            unread: items.iter().filter(|item| item.unread).count(),
            unacked: items.iter().filter(|item| item.unacked).count(),
        })
    }
}

fn is_delivered_to(message: &Message, team_id: &str, member_id: &str) -> bool {
    message.team_id == team_id
        && message.sender_member_id != member_id
        && message
            .effective_recipients
            .iter()
            .any(|id| id == member_id)
}

fn to_item(message: &Message, member_id: &str) -> InboxItem {
    InboxItem {
        message_id: message.id.clone(),
        team_id: message.team_id.clone(),
        room_id: message.room_id.clone(),
        thread_id: message.thread_id.clone(),
        sender_member_id: message.sender_member_id.clone(),
        unread: !has_receipt(&message.read_by, member_id),
        unacked: !has_receipt(&message.acked_by, member_id),
        delivered_at: message.created_at,
    }
}

fn has_receipt(receipts: &[MessageReceipt], member_id: &str) -> bool {
    receipts.iter().any(|receipt| receipt.actor == member_id)
}

fn receipt(member_id: &str) -> MessageReceipt {
    MessageReceipt {
        actor: member_id.to_string(),
        at: Utc::now(),
    }
}
