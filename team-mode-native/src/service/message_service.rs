use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use uuid::Uuid;

use crate::domain::{
    DeliveryDrop, DeliveryStatus, DropReason, MemberRecord, MemberStatus, Message, MessageKind,
};
use crate::service::member_service::strip_at;
use crate::storage::JsonFileStore;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct MessageService {
    store: JsonFileStore,
}

#[derive(Debug, Clone)]
pub struct RoomPost {
    pub team_id: String,
    pub room_id: Option<String>,
    pub sender_member_id: String,
    pub kind: MessageKind,
    pub subject: Option<String>,
    pub body: String,
    pub explicit_mentions: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct DeliveryPlan {
    pub mentions: Vec<String>,
    pub recipients: Vec<String>,
    pub dropped_for: Vec<DeliveryDrop>,
    pub status: DeliveryStatus,
}

impl MessageService {
    pub fn new(store: JsonFileStore) -> Self {
        Self { store }
    }

    pub fn room_post(&self, input: RoomPost) -> Result<Message> {
        self.ensure_team_exists(&input.team_id)?;
        self.ensure_room_exists(&input.team_id, input.room_id.as_deref().unwrap_or("main"))?;
        self.ensure_active_member(&input.team_id, &input.sender_member_id)?;

        let members = self.store.load_members()?;
        let plan = resolve_delivery_plan(
            &members,
            &input.team_id,
            &input.sender_member_id,
            &input.body,
            &input.explicit_mentions,
        );

        if input.kind == MessageKind::Dispatch && plan.recipients.is_empty() {
            return Err(Error::Invalid(
                "dispatch requires at least one valid @handle or explicit mention".to_string(),
            ));
        }

        let id = message_id("msg");
        let thread_id = message_id("th");
        let message = Message {
            id,
            team_id: input.team_id,
            room_id: input.room_id.unwrap_or_else(|| "main".to_string()),
            thread_id,
            reply_to: None,
            sender_member_id: input.sender_member_id,
            kind: input.kind,
            subject: input.subject,
            body: input.body,
            mentions: plan.mentions,
            effective_recipients: plan.recipients.clone(),
            delivered_to: plan.recipients,
            dropped_for: plan.dropped_for,
            read_by: Vec::new(),
            acked_by: Vec::new(),
            delivery_status: plan.status,
            created_at: Utc::now(),
            metadata: BTreeMap::new(),
        };
        self.store.append_message(&message)?;
        Ok(message)
    }

    pub fn list_all(&self) -> Result<Vec<Message>> {
        self.store.load_messages()
    }

    pub(crate) fn append_reply(
        &self,
        team_id: &str,
        thread_id: &str,
        sender_member_id: &str,
        body: String,
        room_id: String,
        kind: MessageKind,
        metadata: BTreeMap<String, String>,
        recipients: Vec<String>,
    ) -> Result<Message> {
        self.ensure_team_exists(team_id)?;
        self.ensure_active_member(team_id, sender_member_id)?;

        let messages = self.store.load_messages()?;
        let thread_messages: Vec<&Message> = messages
            .iter()
            .filter(|message| message.team_id == team_id && message.thread_id == thread_id)
            .collect();
        if thread_messages.is_empty() {
            return Err(Error::NotFound(format!("thread: {thread_id}")));
        }
        let reply_to = thread_messages.last().map(|message| message.id.clone());

        let mut recipients = recipients
            .into_iter()
            .filter(|recipient| recipient != sender_member_id)
            .collect::<BTreeSet<_>>();
        if recipients.is_empty() && kind == MessageKind::Reply {
            for message in &thread_messages {
                if message.sender_member_id != sender_member_id {
                    recipients.insert(message.sender_member_id.clone());
                }
                for recipient in &message.effective_recipients {
                    if recipient != sender_member_id {
                        recipients.insert(recipient.clone());
                    }
                }
            }
        }
        let recipients: Vec<String> = recipients.into_iter().collect();

        let message = Message {
            id: message_id("msg"),
            team_id: team_id.to_string(),
            room_id,
            thread_id: thread_id.to_string(),
            reply_to,
            sender_member_id: sender_member_id.to_string(),
            kind,
            subject: None,
            body,
            mentions: recipients.clone(),
            effective_recipients: recipients.clone(),
            delivered_to: recipients.clone(),
            dropped_for: Vec::new(),
            read_by: Vec::new(),
            acked_by: Vec::new(),
            delivery_status: if recipients.is_empty() {
                DeliveryStatus::Pending
            } else {
                DeliveryStatus::Delivered
            },
            created_at: Utc::now(),
            metadata,
        };
        self.store.append_message(&message)?;
        Ok(message)
    }

    fn ensure_team_exists(&self, team_id: &str) -> Result<()> {
        if self
            .store
            .load_teams()?
            .iter()
            .any(|team| team.id == team_id)
        {
            Ok(())
        } else {
            Err(Error::NotFound(format!("team: {team_id}")))
        }
    }

    fn ensure_room_exists(&self, team_id: &str, room_id: &str) -> Result<()> {
        if room_id == "direct" {
            return Ok(());
        }
        if self
            .store
            .load_rooms()?
            .iter()
            .any(|room| room.team_id == team_id && room.id == room_id)
        {
            Ok(())
        } else {
            Err(Error::NotFound(format!("room: {room_id}")))
        }
    }

    fn ensure_active_member(&self, team_id: &str, member_id: &str) -> Result<()> {
        if self.store.load_members()?.iter().any(|member| {
            member.profile.team_id == team_id
                && member.profile.id == member_id
                && member.profile.status == MemberStatus::Active
        }) {
            Ok(())
        } else {
            Err(Error::NotFound(format!("member: {member_id}")))
        }
    }
}

pub(crate) fn resolve_delivery_plan(
    members: &[MemberRecord],
    team_id: &str,
    sender_member_id: &str,
    body: &str,
    explicit_mentions: &[String],
) -> DeliveryPlan {
    let mut mentions = BTreeSet::new();
    let mut recipients = BTreeSet::new();
    let mut dropped_for = Vec::new();

    for handle in parse_handles(body) {
        resolve_one(
            members,
            team_id,
            sender_member_id,
            &handle,
            &mut mentions,
            &mut recipients,
            &mut dropped_for,
        );
    }

    for mention in explicit_mentions {
        resolve_one(
            members,
            team_id,
            sender_member_id,
            mention,
            &mut mentions,
            &mut recipients,
            &mut dropped_for,
        );
    }

    let status = if recipients.is_empty() {
        if dropped_for.is_empty() {
            DeliveryStatus::Pending
        } else {
            DeliveryStatus::Failed
        }
    } else if dropped_for.is_empty() {
        DeliveryStatus::Delivered
    } else {
        DeliveryStatus::Partial
    };

    DeliveryPlan {
        mentions: mentions.into_iter().collect(),
        recipients: recipients.into_iter().collect(),
        dropped_for,
        status,
    }
}

fn resolve_one(
    members: &[MemberRecord],
    team_id: &str,
    sender_member_id: &str,
    mention: &str,
    mentions: &mut BTreeSet<String>,
    recipients: &mut BTreeSet<String>,
    dropped_for: &mut Vec<DeliveryDrop>,
) {
    let raw_key = strip_at(mention);
    let handle_key = raw_key.to_ascii_lowercase();
    if raw_key.is_empty() {
        return;
    }
    match members.iter().find(|member| {
        member.profile.team_id == team_id
            && (member.profile.id == raw_key || member.profile.handle == handle_key)
    }) {
        Some(member) if member.profile.status == MemberStatus::Active => {
            mentions.insert(member.profile.id.clone());
            if member.profile.id != sender_member_id {
                recipients.insert(member.profile.id.clone());
            }
        }
        Some(member) => dropped_for.push(DeliveryDrop {
            recipient: member.profile.id.clone(),
            reason: DropReason::RemovedMember,
        }),
        None => dropped_for.push(DeliveryDrop {
            recipient: raw_key.to_string(),
            reason: DropReason::UnknownRecipient,
        }),
    }
}

fn parse_handles(body: &str) -> Vec<String> {
    let mut handles = Vec::new();
    let chars: Vec<char> = body.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '@' {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < chars.len()
            && (chars[end].is_ascii_alphanumeric() || chars[end] == '_' || chars[end] == '-')
        {
            end += 1;
        }
        if end > start {
            handles.push(chars[start..end].iter().collect());
        }
        index = end.max(index + 1);
    }
    handles
}

pub(crate) fn message_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}
