use std::collections::HashMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::team_mode::domain::{
    AudiencePolicy, DeliveryDrop, DeliveryStatus, DropReason, MemberStatus, Message, MessageKind,
    VisibilityResolution, VisibilityRule,
};
use crate::team_mode::service::inbox_notifier::InboxNotifier;
use crate::team_mode::service::lead_pending::LeadPendingWriter;
use crate::team_mode::storage::{MemberStore, MessageStore, RoomStore, TeamStore};
use crate::util::validate_name;

#[derive(Debug, Clone)]
pub struct SendMessageRequest {
    pub team_id: String,
    pub room_id: String,
    /// Team-scoped name of the sender (e.g. `"lead"`, `"alice"`).
    pub sender: String,
    pub kind: MessageKind,
    pub subject: Option<String>,
    pub body: String,
    pub mentions: Vec<String>,
    pub visibility: Vec<VisibilityRule>,
    pub audience_policy: Option<AudiencePolicy>,
    pub reply_to: Option<String>,
    pub thread_id: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct MessageService {
    message_store: MessageStore,
    member_store: MemberStore,
    room_store: RoomStore,
    team_store: TeamStore,
    inbox_notifier: Option<InboxNotifier>,
    lead_pending_writer: Option<LeadPendingWriter>,
}

impl MessageService {
    pub fn new(
        message_store: MessageStore,
        member_store: MemberStore,
        room_store: RoomStore,
        team_store: TeamStore,
    ) -> Self {
        Self {
            message_store,
            member_store,
            room_store,
            team_store,
            inbox_notifier: None,
            lead_pending_writer: None,
        }
    }

    pub fn with_inbox_notifier(mut self, n: InboxNotifier) -> Self {
        self.inbox_notifier = Some(n);
        self
    }

    pub fn with_lead_pending_writer(mut self, w: LeadPendingWriter) -> Self {
        self.lead_pending_writer = Some(w);
        self
    }

    pub fn send(&self, request: SendMessageRequest) -> Result<Message> {
        let SendMessageRequest {
            team_id,
            room_id,
            sender,
            kind,
            subject,
            body,
            mentions,
            visibility,
            audience_policy,
            reply_to,
            thread_id,
            expires_at,
        } = request;

        validate_name(&team_id)?;
        validate_name(&room_id)?;
        validate_name(&sender)?;
        self.ensure_team_exists(&team_id)?;
        self.ensure_room_exists(&team_id)?;
        self.ensure_sender_exists(&team_id, &sender)?;

        let active_members = self.active_members_by_name(&team_id)?;

        let mut mention_candidates = parse_mentions_from_body(&body);
        for mention in mentions {
            push_unique_candidate(
                &mut mention_candidates,
                MentionCandidate {
                    raw: mention.clone(),
                    handle: normalize_mention(&mention),
                },
            );
        }

        let mut effective_recipients = Vec::new();
        let mut dropped_for = Vec::new();
        for mention in &mention_candidates {
            match active_members.get(&mention.handle) {
                Some(status) if *status == MemberStatus::Active => {
                    push_unique(&mut effective_recipients, mention.handle.clone());
                }
                Some(_) => dropped_for.push(DeliveryDrop {
                    recipient: mention.raw.clone(),
                    reason: DropReason::RemovedMember,
                }),
                None => dropped_for.push(DeliveryDrop {
                    recipient: mention.raw.clone(),
                    reason: DropReason::UnknownRecipient,
                }),
            }
        }

        if effective_recipients.contains(&sender) {
            return Err(Error::Other(format!(
                "member '{sender}' cannot send a message that mentions themselves"
            )));
        }

        // Reply auto-recipient: the sender of the parent message.
        if matches!(&kind, MessageKind::Reply) {
            if let Some(reply_to_id) = &reply_to {
                if let Ok(Some(parent)) = self.message_store.get(&team_id, reply_to_id) {
                    push_unique(&mut effective_recipients, parent.sender.clone());
                }
            }
        }

        if matches!(&kind, MessageKind::Dispatch) && effective_recipients.is_empty() {
            return Err(Error::Other(
                "dispatch messages must mention at least one valid team member".into(),
            ));
        }

        let thread_id = self.resolve_thread_id(&team_id, &room_id, thread_id, reply_to.clone())?;
        let audience_policy = audience_policy.or_else(|| {
            if effective_recipients.is_empty() {
                None
            } else {
                Some(AudiencePolicy::Mentions)
            }
        });
        let effective_visibility_reason =
            audience_policy.clone().map(|policy| VisibilityResolution {
                policy,
                reasons: if effective_recipients.is_empty() {
                    Vec::new()
                } else {
                    vec![crate::team_mode::domain::VisibilityReason::ExplicitMention]
                },
            });
        let delivery_status = derive_delivery_status(&effective_recipients, &dropped_for, &kind);
        let delivered_to = effective_recipients.clone();
        let mentions = mention_candidates
            .into_iter()
            .map(|c| c.handle)
            .collect();

        let message = Message {
            id: Uuid::new_v4().to_string(),
            room_id,
            team_id: Some(team_id.clone()),
            thread_id: Some(thread_id),
            reply_to,
            sender: sender.clone(),
            kind,
            subject,
            body,
            mentions,
            visibility,
            audience_policy,
            effective_visibility_reason,
            effective_recipients,
            delivered_to,
            dropped_for,
            read_by: Vec::new(),
            acked_by: Vec::new(),
            delivery_status,
            created_at: Utc::now(),
            expires_at,
        };

        self.message_store.save(&message)?;
        tracing::info!(
            sender = %message.sender,
            room = %message.room_id,
            kind = ?message.kind,
            recipients = ?message.effective_recipients,
            "message sent"
        );
        if let Some(n) = &self.inbox_notifier {
            n.notify();
        }

        // Best-effort: append to lead_pending.jsonl if the team's lead is
        // among the recipients.
        if let Some(writer) = &self.lead_pending_writer {
            if let Ok(Some(team)) = self.team_store.get(&team_id) {
                if let Some(lead_name) = team.lead_member_id.as_deref() {
                    if let Err(err) = writer.maybe_write(&message, lead_name, &sender) {
                        tracing::warn!(
                            error = %err,
                            msg_id = %message.id,
                            "lead_pending write failed; message still delivered via inbox"
                        );
                    }
                }
            }
        }

        Ok(message)
    }

    pub fn list_by_room(&self, team_id: &str, room_id: impl AsRef<str>) -> Result<Vec<Message>> {
        self.message_store.list_by_room(team_id, room_id)
    }

    fn ensure_team_exists(&self, team_id: &str) -> Result<()> {
        self.team_store
            .get(team_id)?
            .ok_or_else(|| Error::TeamNotFound {
                name: team_id.to_string(),
            })?;
        Ok(())
    }

    fn ensure_room_exists(&self, team_id: &str) -> Result<()> {
        match self.room_store.get(team_id)? {
            Some(_) => Ok(()),
            None => Err(Error::Other(format!(
                "main room not found for team '{team_id}'"
            ))),
        }
    }

    fn ensure_sender_exists(&self, team_id: &str, sender: &str) -> Result<()> {
        let record = self
            .member_store
            .get(team_id, sender)?
            .ok_or_else(|| Error::MemberNotFound {
                team: team_id.to_string(),
                member: sender.to_string(),
            })?;
        if matches!(record.profile.status, MemberStatus::Removed) {
            return Err(Error::MemberNotFound {
                team: team_id.to_string(),
                member: sender.to_string(),
            });
        }
        Ok(())
    }

    fn active_members_by_name(&self, team_id: &str) -> Result<HashMap<String, MemberStatus>> {
        let members = self.member_store.list_by_team(team_id)?;
        let mut out = HashMap::new();
        for r in members {
            out.insert(r.profile.name, r.profile.status);
        }
        Ok(out)
    }

    fn resolve_thread_id(
        &self,
        team_id: &str,
        room_id: &str,
        thread_id: Option<String>,
        reply_to: Option<String>,
    ) -> Result<String> {
        if let Some(reply_to) = reply_to {
            let parent = self
                .message_store
                .get(team_id, &reply_to)?
                .ok_or_else(|| Error::Other(format!("reply_to message '{reply_to}' not found")))?;

            if parent.team_id.as_deref() != Some(team_id) || parent.room_id != room_id {
                return Err(Error::Other(format!(
                    "reply_to message '{reply_to}' must belong to team '{team_id}' and room '{room_id}'"
                )));
            }

            let inherited_thread_id = parent.thread_id.ok_or_else(|| {
                Error::Other(format!(
                    "reply_to message '{reply_to}' has no thread_id to inherit"
                ))
            })?;

            if let Some(explicit_thread_id) = thread_id {
                validate_name(&explicit_thread_id)?;
                if explicit_thread_id != inherited_thread_id {
                    return Err(Error::Other(format!(
                        "reply thread mismatch: explicit thread_id '{explicit_thread_id}' does not match parent thread '{inherited_thread_id}'"
                    )));
                }
            }

            return Ok(inherited_thread_id);
        }

        if let Some(thread_id) = thread_id {
            validate_name(&thread_id)?;
            let existing = self.message_store.list_by_thread(team_id, &thread_id)?;
            if let Some(conflict) = existing.into_iter().find(|m| m.room_id != room_id) {
                return Err(Error::Other(format!(
                    "thread_id '{thread_id}' is already bound to room '{}' in team '{team_id}'",
                    conflict.room_id
                )));
            }
            return Ok(thread_id);
        }

        Ok(Uuid::new_v4().to_string())
    }
}

fn derive_delivery_status(
    effective_recipients: &[String],
    dropped_for: &[DeliveryDrop],
    kind: &MessageKind,
) -> DeliveryStatus {
    if effective_recipients.is_empty() {
        return match kind {
            MessageKind::Dispatch => DeliveryStatus::Failed,
            _ => DeliveryStatus::Pending,
        };
    }
    if dropped_for.is_empty() {
        DeliveryStatus::Delivered
    } else {
        DeliveryStatus::Partial
    }
}

#[derive(Debug, Clone)]
struct MentionCandidate {
    raw: String,
    handle: String,
}

fn normalize_mention(value: &str) -> String {
    value.trim().trim_start_matches('@').to_string()
}

fn push_unique(items: &mut Vec<String>, value: String) {
    if !value.is_empty() && !items.iter().any(|item| item == &value) {
        items.push(value);
    }
}

fn push_unique_candidate(items: &mut Vec<MentionCandidate>, candidate: MentionCandidate) {
    if candidate.handle.is_empty() {
        return;
    }
    if !items.iter().any(|item| item.handle == candidate.handle) {
        items.push(candidate);
    }
}

fn parse_mentions_from_body(body: &str) -> Vec<MentionCandidate> {
    let chars: Vec<char> = body.chars().collect();
    let mut mentions = Vec::new();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] == '@' && (index == 0 || !is_handle_char(chars[index - 1])) {
            let mut handle = String::new();
            let mut cursor = index + 1;
            while cursor < chars.len() && is_handle_char(chars[cursor]) {
                handle.push(chars[cursor]);
                cursor += 1;
            }
            if !handle.is_empty() {
                push_unique_candidate(
                    &mut mentions,
                    MentionCandidate {
                        raw: format!("@{handle}"),
                        handle,
                    },
                );
            }
            index = cursor;
            continue;
        }
        index += 1;
    }
    mentions
}

fn is_handle_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{
        MemberKind, MemberStatus, Room, RoomKind, RoomStatus, Team, TeamStatus,
    };
    use crate::team_mode::service::MemberService;
    use crate::team_mode::service::member_service::AddMemberRequest;
    use crate::team_mode::storage::{MemberRecord, MemberStore, MessageStore, RoomStore, TeamStore};

    fn seed_team(base_dir: &std::path::Path, team_id: &str) {
        TeamStore::new(base_dir)
            .save(&Team {
                id: team_id.into(),
                name: team_id.into(),
                description: None,
                cwd: None,
                status: TeamStatus::Active,
                lead_member_id: Some("lead".into()),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .unwrap();
        RoomStore::new(base_dir)
            .save(
                team_id,
                &Room {
                    id: "main".into(),
                    team_id: Some(team_id.into()),
                    kind: RoomKind::Main,
                    status: RoomStatus::Active,
                },
            )
            .unwrap();
    }

    fn add_member(
        base_dir: &std::path::Path,
        team_id: &str,
        name: &str,
        kind: MemberKind,
        status: MemberStatus,
    ) {
        let service = MemberService::new(MemberStore::new(base_dir), TeamStore::new(base_dir));
        let mut record = service
            .add(AddMemberRequest {
                team_id: team_id.into(),
                name: name.into(),
                kind,
                role_label: "role".into(),
                role_description: None,
                execution: None,
            })
            .unwrap();
        if status != MemberStatus::Active {
            MemberStore::new(base_dir)
                .update(team_id, name, |m| m.status = status.clone())
                .unwrap();
            record.profile.status = status;
        }
        let _ = record;
    }

    fn new_service(base_dir: &std::path::Path) -> MessageService {
        MessageService::new(
            MessageStore::new(base_dir),
            MemberStore::new(base_dir),
            RoomStore::new(base_dir),
            TeamStore::new(base_dir),
        )
    }

    #[test]
    fn dispatch_routes_mentions_to_recipients() {
        let dir = tempdir().unwrap();
        seed_team(dir.path(), "demo");
        add_member(dir.path(), "demo", "lead", MemberKind::Lead, MemberStatus::Active);
        add_member(dir.path(), "demo", "alice", MemberKind::Member, MemberStatus::Active);
        add_member(dir.path(), "demo", "bob", MemberKind::Member, MemberStatus::Active);

        let service = new_service(dir.path());
        let msg = service
            .send(SendMessageRequest {
                team_id: "demo".into(),
                room_id: "main".into(),
                sender: "lead".into(),
                kind: MessageKind::Dispatch,
                subject: None,
                body: "hi @alice and @bob".into(),
                mentions: vec![],
                visibility: vec![],
                audience_policy: None,
                reply_to: None,
                thread_id: None,
                expires_at: None,
            })
            .unwrap();
        assert!(msg.effective_recipients.contains(&"alice".to_string()));
        assert!(msg.effective_recipients.contains(&"bob".to_string()));
    }

    #[test]
    fn reply_auto_adds_parent_sender_as_recipient() {
        let dir = tempdir().unwrap();
        seed_team(dir.path(), "demo");
        add_member(dir.path(), "demo", "lead", MemberKind::Lead, MemberStatus::Active);
        add_member(dir.path(), "demo", "alice", MemberKind::Member, MemberStatus::Active);

        let service = new_service(dir.path());
        let first = service
            .send(SendMessageRequest {
                team_id: "demo".into(),
                room_id: "main".into(),
                sender: "lead".into(),
                kind: MessageKind::Dispatch,
                subject: None,
                body: "@alice do the thing".into(),
                mentions: vec![],
                visibility: vec![],
                audience_policy: None,
                reply_to: None,
                thread_id: None,
                expires_at: None,
            })
            .unwrap();

        let reply = service
            .send(SendMessageRequest {
                team_id: "demo".into(),
                room_id: "main".into(),
                sender: "alice".into(),
                kind: MessageKind::Reply,
                subject: None,
                body: "done".into(),
                mentions: vec![],
                visibility: vec![],
                audience_policy: None,
                reply_to: Some(first.id.clone()),
                thread_id: None,
                expires_at: None,
            })
            .unwrap();

        assert!(reply.effective_recipients.contains(&"lead".to_string()));
    }

    #[test]
    fn self_mention_rejected() {
        let dir = tempdir().unwrap();
        seed_team(dir.path(), "demo");
        add_member(dir.path(), "demo", "alice", MemberKind::Member, MemberStatus::Active);
        let service = new_service(dir.path());
        let err = service
            .send(SendMessageRequest {
                team_id: "demo".into(),
                room_id: "main".into(),
                sender: "alice".into(),
                kind: MessageKind::Dispatch,
                subject: None,
                body: "hey @alice".into(),
                mentions: vec![],
                visibility: vec![],
                audience_policy: None,
                reply_to: None,
                thread_id: None,
                expires_at: None,
            })
            .unwrap_err();
        assert!(err.to_string().contains("mentions themselves"));
    }

    // Silence "unused" lint for imports.
    #[allow(dead_code)]
    fn _keeps_import_alive(_: MemberRecord) {}
}
