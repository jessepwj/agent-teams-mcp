use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::domain::{Message, MessageKind, MessageReceipt};
use crate::service::member_service::strip_at;
use crate::service::message_service::{MessageService, message_id};
use crate::storage::JsonFileStore;
use crate::{Error, Result};

const DIRECT_ROOM_ID: &str = "direct";
const DIRECT_PARTICIPANTS_KEY: &str = "direct_participants";

#[derive(Debug, Clone)]
pub struct DirectService {
    store: JsonFileStore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectThreadSummary {
    pub thread_id: String,
    pub participants: Vec<String>,
    pub last_message: Message,
    pub unread: usize,
}

impl DirectService {
    pub fn new(store: JsonFileStore) -> Self {
        Self { store }
    }

    pub fn direct_send(
        &self,
        team_id: &str,
        sender_member_id: &str,
        recipient_id_or_handle: &str,
        body: impl Into<String>,
    ) -> Result<Message> {
        let recipient_id = self.resolve_member_id(team_id, recipient_id_or_handle)?;
        self.ensure_member(team_id, sender_member_id)?;
        if recipient_id == sender_member_id {
            return Err(Error::Invalid(
                "direct message recipient must differ from sender".to_string(),
            ));
        }
        let participants = sorted_pair(sender_member_id, &recipient_id);
        let thread_id = direct_thread_id(&participants);
        let mut metadata = BTreeMap::new();
        metadata.insert(DIRECT_PARTICIPANTS_KEY.to_string(), participants.join(","));

        let existing = self
            .store
            .load_messages()?
            .into_iter()
            .any(|message| message.team_id == team_id && message.thread_id == thread_id);

        if existing {
            MessageService::new(self.store.clone()).append_reply(
                team_id,
                &thread_id,
                sender_member_id,
                body.into(),
                DIRECT_ROOM_ID.to_string(),
                MessageKind::Direct,
                metadata,
                vec![recipient_id],
            )
        } else {
            let message = Message {
                id: message_id("msg"),
                team_id: team_id.to_string(),
                room_id: DIRECT_ROOM_ID.to_string(),
                thread_id,
                reply_to: None,
                sender_member_id: sender_member_id.to_string(),
                kind: MessageKind::Direct,
                subject: None,
                body: body.into(),
                mentions: vec![recipient_id.clone()],
                effective_recipients: vec![recipient_id.clone()],
                delivered_to: vec![recipient_id],
                dropped_for: Vec::new(),
                read_by: Vec::new(),
                acked_by: Vec::new(),
                delivery_status: crate::domain::DeliveryStatus::Delivered,
                created_at: Utc::now(),
                metadata,
            };
            self.store.append_message(&message)?;
            Ok(message)
        }
    }

    pub fn direct_reply(
        &self,
        team_id: &str,
        thread_id: &str,
        sender_member_id: &str,
        body: impl Into<String>,
    ) -> Result<Message> {
        let messages = self.direct_read(team_id, thread_id, sender_member_id)?;
        let participants = participants_from_messages(&messages)
            .ok_or_else(|| Error::Invalid(format!("thread is not a direct thread: {thread_id}")))?;
        let recipients: Vec<String> = participants
            .into_iter()
            .filter(|participant| participant != sender_member_id)
            .collect();
        if recipients.is_empty() {
            return Err(Error::Invalid(format!(
                "member is not a participant in direct thread: {sender_member_id}"
            )));
        }
        let mut metadata = BTreeMap::new();
        let mut all = recipients.clone();
        all.push(sender_member_id.to_string());
        all.sort();
        metadata.insert(DIRECT_PARTICIPANTS_KEY.to_string(), all.join(","));

        MessageService::new(self.store.clone()).append_reply(
            team_id,
            thread_id,
            sender_member_id,
            body.into(),
            DIRECT_ROOM_ID.to_string(),
            MessageKind::Direct,
            metadata,
            recipients,
        )
    }

    pub fn direct_read(
        &self,
        team_id: &str,
        thread_id: &str,
        reader_member_id: &str,
    ) -> Result<Vec<Message>> {
        let mut messages = self.store.load_messages()?;
        let mut changed = false;
        let mut selected = Vec::new();
        for message in messages.iter_mut() {
            if !(message.team_id == team_id && message.thread_id == thread_id && is_direct(message))
            {
                continue;
            }
            let participants = participants_from_message(message);
            if !participants
                .iter()
                .any(|participant| participant == reader_member_id)
            {
                return Err(Error::Invalid(format!(
                    "member is not a participant in direct thread: {reader_member_id}"
                )));
            }
            if message.sender_member_id != reader_member_id
                && message
                    .effective_recipients
                    .iter()
                    .any(|recipient| recipient == reader_member_id)
                && !message
                    .read_by
                    .iter()
                    .any(|receipt| receipt.actor == reader_member_id)
            {
                message.read_by.push(MessageReceipt {
                    actor: reader_member_id.to_string(),
                    at: Utc::now(),
                });
                changed = true;
            }
            selected.push(message.clone());
        }
        if selected.is_empty() {
            return Err(Error::NotFound(format!("direct thread: {thread_id}")));
        }
        if changed {
            self.store.save_messages(&messages)?;
        }
        selected.sort_by_key(|message| message.created_at);
        Ok(selected)
    }

    pub fn direct_list(&self, team_id: &str, member_id: &str) -> Result<Vec<DirectThreadSummary>> {
        let mut by_thread: BTreeMap<String, Vec<Message>> = BTreeMap::new();
        for message in self.store.load_messages()? {
            if message.team_id != team_id || !is_direct(&message) {
                continue;
            }
            let participants = participants_from_message(&message);
            if participants
                .iter()
                .any(|participant| participant == member_id)
            {
                by_thread
                    .entry(message.thread_id.clone())
                    .or_default()
                    .push(message);
            }
        }

        let mut summaries = Vec::new();
        for (thread_id, mut messages) in by_thread {
            messages.sort_by_key(|message| message.created_at);
            let Some(last_message) = messages.last().cloned() else {
                continue;
            };
            let participants = participants_from_messages(&messages).unwrap_or_default();
            let unread = messages
                .iter()
                .filter(|message| {
                    message.sender_member_id != member_id
                        && message
                            .effective_recipients
                            .iter()
                            .any(|recipient| recipient == member_id)
                        && !message
                            .read_by
                            .iter()
                            .any(|receipt| receipt.actor == member_id)
                })
                .count();
            summaries.push(DirectThreadSummary {
                thread_id,
                participants,
                last_message,
                unread,
            });
        }
        summaries.sort_by_key(|summary| summary.last_message.created_at);
        Ok(summaries)
    }

    fn resolve_member_id(&self, team_id: &str, member_id_or_handle: &str) -> Result<String> {
        let key = strip_at(member_id_or_handle);
        let handle_key = key.to_ascii_lowercase();
        self.store
            .load_members()?
            .into_iter()
            .find(|member| {
                member.profile.team_id == team_id
                    && (member.profile.id == key || member.profile.handle == handle_key)
                    && member.profile.status == crate::domain::MemberStatus::Active
            })
            .map(|member| member.profile.id)
            .ok_or_else(|| Error::NotFound(format!("member: {member_id_or_handle}")))
    }

    fn ensure_member(&self, team_id: &str, member_id: &str) -> Result<()> {
        if self.store.load_members()?.iter().any(|member| {
            member.profile.team_id == team_id
                && member.profile.id == member_id
                && member.profile.status == crate::domain::MemberStatus::Active
        }) {
            Ok(())
        } else {
            Err(Error::NotFound(format!("member: {member_id}")))
        }
    }
}

fn is_direct(message: &Message) -> bool {
    message.kind == MessageKind::Direct
        && message.room_id == DIRECT_ROOM_ID
        && message.metadata.contains_key(DIRECT_PARTICIPANTS_KEY)
}

fn participants_from_message(message: &Message) -> Vec<String> {
    message
        .metadata
        .get(DIRECT_PARTICIPANTS_KEY)
        .map(|value| {
            value
                .split(',')
                .filter(|part| !part.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn participants_from_messages(messages: &[Message]) -> Option<Vec<String>> {
    messages.iter().find_map(|message| {
        let participants = participants_from_message(message);
        if participants.is_empty() {
            None
        } else {
            Some(participants)
        }
    })
}

fn sorted_pair(left: &str, right: &str) -> Vec<String> {
    let mut set = BTreeSet::new();
    set.insert(left.to_string());
    set.insert(right.to_string());
    set.into_iter().collect()
}

fn direct_thread_id(participants: &[String]) -> String {
    format!("dm_{}", participants.join("_"))
}
