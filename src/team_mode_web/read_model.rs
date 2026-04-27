use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::runtime::ExecutionSessionState;
use crate::team_mode::domain::{MemberKind, MemberStatus, Message, MessageKind, Team};
use crate::team_mode::service::{AddMemberRequest, SendMessageRequest};
use crate::util::{codex_session_discovery, session_discovery};

use super::dto::*;
use super::error::WebError;
use super::state::TeamModeWebState;

mod conversation;
mod diagnostics;

pub use conversation::read_member_conversation;
pub use diagnostics::read_diagnostics;

/// Reserved sender name for messages originating from the read-only web UI.
///
/// Used both as the lazy-created member's `name` and as the sender stamped
/// on outgoing messages. Picked to be:
///   * lowercase + a-z only → passes `validate_slug_name`
///   * unlikely to collide with a real worker name (no one names a worker
///     after a generic role)
///   * obvious in transcripts ("user said …" reads naturally)
pub const WEB_USER_SENDER: &str = "user";
pub const WEB_USER_ROLE_LABEL: &str = "user";

pub fn redact_env(env: &std::collections::HashMap<String, String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (key, value) in env {
        out.insert(
            key.clone(),
            if should_redact_key(key) {
                "***".into()
            } else {
                value.clone()
            },
        );
    }
    out
}

fn should_redact_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD", "AUTH", "COOKIE"]
        .iter()
        .any(|needle| upper.contains(needle))
}

pub fn list_teams(state: &TeamModeWebState) -> Result<TeamsResponse, WebError> {
    let teams = state.team_service.list()?;
    let mut views = Vec::with_capacity(teams.len());
    for team in teams {
        views.push(team_summary_view(state, &team)?);
    }
    Ok(TeamsResponse { teams: views })
}

pub fn read_team(state: &TeamModeWebState, team_id: &str) -> Result<TeamResponse, WebError> {
    let team = state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;
    let counts = team_counts(state, &team)?;
    Ok(TeamResponse {
        team: team_detail_view(&team),
        counts,
    })
}

pub fn read_main_room(
    state: &TeamModeWebState,
    team_id: &str,
    limit: usize,
    sender: Option<&str>,
    mentioned: Option<&str>,
) -> Result<RoomResponse, WebError> {
    let team = state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;
    let room = state
        .room_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("room 'main' not found in team '{team_id}'")))?;
    let mut all_messages = state.message_service.list_by_room(team_id, "main")?;
    all_messages.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let reply_counts = compute_thread_reply_counts(&all_messages);
    let lead_member_id = team.lead_member_id.as_deref().unwrap_or("lead");
    let mut messages = all_messages;

    if let Some(sender) = sender {
        messages.retain(|message| message.sender == sender);
    }
    if let Some(mentioned) = mentioned {
        messages.retain(|message| {
            message
                .mentions
                .iter()
                .any(|candidate| candidate == mentioned)
                || message
                    .effective_recipients
                    .iter()
                    .any(|candidate| candidate == mentioned)
        });
    }

    let total = messages.len();
    let shown = messages
        .into_iter()
        .take(limit)
        .map(|message| {
            let thread_reply_count = if message.reply_to.is_none() {
                message
                    .thread_id
                    .as_ref()
                    .and_then(|thread_id| reply_counts.get(thread_id).copied())
                    .map(|count| count.saturating_sub(1))
                    .unwrap_or(0)
            } else {
                0
            };
            message_view(&message, thread_reply_count, lead_member_id)
        })
        .collect::<Vec<_>>();

    let shown_len = shown.len();
    let next_cursor = if total > shown_len {
        shown.last().map(|message| message.id.clone())
    } else {
        None
    };

    Ok(RoomResponse {
        room: RoomView {
            id: room.id,
            team_id: team_id.to_string(),
            status: format!("{:?}", room.status).to_ascii_lowercase(),
        },
        messages: shown,
        page: PageView {
            has_more_before: false,
            has_more_after: total > shown_len,
            next_cursor,
        },
    })
}

pub fn list_members(state: &TeamModeWebState, team_id: &str) -> Result<MembersResponse, WebError> {
    let team = state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;
    let members = state.member_service.list_by_team(team_id)?;
    let messages = state.message_service.list_by_room(team_id, "main")?;
    // Hide members removed via worker_remove from the web UI: they exist on
    // disk so worker_add reuse can fast-resume them, but their `execution`
    // still carries the last-known sessionState ("running") which would
    // otherwise display as live workers in the Web sidebar.
    let views = members
        .into_iter()
        .filter(|member| member.profile.status == MemberStatus::Active)
        .map(|member| member_summary_view(&team, &member, &messages, &state.runtime_workers))
        .collect();
    Ok(MembersResponse { members: views })
}

pub fn read_member(
    state: &TeamModeWebState,
    team_id: &str,
    name: &str,
) -> Result<MemberResponse, WebError> {
    let team = state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;
    let member = state.member_service.get(team_id, name)?.ok_or_else(|| {
        WebError::not_found(format!("member '{name}' not found in team '{team_id}'"))
    })?;
    let messages = state.message_service.list_by_room(team_id, "main")?;
    let activity = build_member_activity(name, &messages).into_view();
    Ok(MemberResponse {
        profile: MemberProfileView {
            name: member.profile.name.clone(),
            kind: member.profile.kind.clone(),
            role_label: member.profile.role_label.clone(),
            status: member.profile.status.clone(),
            joined_at: member.profile.joined_at,
        },
        execution: member_execution_view(
            &member,
            team.lead_member_id.as_deref(),
            &state.runtime_workers,
        ),
        activity,
    })
}

pub fn read_member_activity(
    state: &TeamModeWebState,
    team_id: &str,
    name: &str,
) -> Result<MemberActivityResponse, WebError> {
    state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;
    state.member_service.get(team_id, name)?.ok_or_else(|| {
        WebError::not_found(format!("member '{name}' not found in team '{team_id}'"))
    })?;
    let messages = state.message_service.list_by_room(team_id, "main")?;
    let activity = build_member_activity(name, &messages);
    Ok(MemberActivityResponse {
        member: name.to_string(),
        source: "derived-from-messages".into(),
        items: activity.items,
        limitations: vec!["No stdout/stderr or tool-call events are available yet.".into()],
    })
}

fn team_summary_view(state: &TeamModeWebState, team: &Team) -> Result<TeamSummaryView, WebError> {
    let members = state.member_service.list_by_team(&team.id)?;
    let messages = state.message_service.list_by_room(&team.id, "main")?;
    let counts = team_counts_from_parts(team, &members, &messages, &state.inbox_service)?;
    Ok(TeamSummaryView {
        id: team.id.clone(),
        name: team.name.clone(),
        cwd: team.cwd.clone(),
        status: team.status.clone(),
        lead_member_id: team.lead_member_id.clone(),
        owner_cc_pid: team.owner_cc_pid,
        member_count: counts.member_count,
        active_worker_count: counts.active_worker_count,
        last_message_at: counts.last_message_at,
    })
}

fn team_detail_view(team: &Team) -> TeamDetailView {
    TeamDetailView {
        id: team.id.clone(),
        name: team.name.clone(),
        cwd: team.cwd.clone(),
        status: team.status.clone(),
        lead_member_id: team.lead_member_id.clone(),
        owner_cc_pid: team.owner_cc_pid,
        created_at: Some(team.created_at),
        updated_at: Some(team.updated_at),
    }
}

fn team_counts(state: &TeamModeWebState, team: &Team) -> Result<TeamCountsView, WebError> {
    let members = state.member_service.list_by_team(&team.id)?;
    let messages = state.message_service.list_by_room(&team.id, "main")?;
    team_counts_from_parts(team, &members, &messages, &state.inbox_service)
}

fn team_counts_from_parts(
    team: &Team,
    members: &[crate::team_mode::storage::MemberRecord],
    messages: &[Message],
    inbox_service: &crate::team_mode::service::InboxService,
) -> Result<TeamCountsView, WebError> {
    // Count only Active members so the sidebar header matches the visible
    // member list (which filters by status). Removed members linger on
    // disk as fast-resume sources but should not show up in headcounts.
    let member_count = members
        .iter()
        .filter(|member| member.profile.status == MemberStatus::Active)
        .count();
    let active_worker_count = members
        .iter()
        .filter(|member| {
            member.profile.kind == MemberKind::Member
                && member.profile.status == MemberStatus::Active
        })
        .count();
    let message_count = messages.len();
    let thread_count = messages
        .iter()
        .filter_map(|message| message.thread_id.as_ref())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let last_message_at = latest_message_at(messages);
    let unread_for_lead = team
        .lead_member_id
        .as_deref()
        .map(|lead| inbox_service.count(&team.id, lead, None))
        .transpose()?
        .map(|count| count.unread)
        .unwrap_or(0);

    Ok(TeamCountsView {
        member_count,
        active_worker_count,
        message_count,
        thread_count,
        unread_for_lead,
        last_message_at,
    })
}

fn member_summary_view(
    team: &Team,
    member: &crate::team_mode::storage::MemberRecord,
    messages: &[Message],
    runtime_workers: &crate::team_mode::runtime_workers::RuntimeWorkerStore,
) -> MemberSummaryView {
    let activity = build_member_activity(&member.profile.name, messages);
    let execution = member.execution.as_ref();
    MemberSummaryView {
        name: member.profile.name.clone(),
        kind: member.profile.kind.clone(),
        role_label: member.profile.role_label.clone(),
        status: member.profile.status.clone(),
        session_state: execution_session_state_label(
            member,
            team.lead_member_id.as_deref(),
            Some((&team.id, runtime_workers)),
        ),
        badge: if member.profile.kind == MemberKind::Lead {
            "lead".into()
        } else {
            "worker".into()
        },
        adapter: execution.and_then(|profile| profile.adapter.clone()),
        model: execution.and_then(|profile| profile.model.clone()),
        cwd: execution.and_then(|profile| profile.cwd.clone()),
        last_activity_at: activity.last_activity_at,
    }
}

fn member_execution_view(
    member: &crate::team_mode::storage::MemberRecord,
    lead_member_id: Option<&str>,
    runtime_workers: &crate::team_mode::runtime_workers::RuntimeWorkerStore,
) -> MemberExecutionView {
    let execution = member.execution.as_ref();
    let env = execution
        .map(|profile| redact_env(&profile.env))
        .unwrap_or_default();
    let env_keys = env.keys().cloned().collect::<Vec<_>>();
    MemberExecutionView {
        execution_mode: execution
            .map(|profile| format!("{:?}", profile.execution_mode).to_ascii_lowercase())
            .unwrap_or_else(|| "unknown".into()),
        adapter: execution.and_then(|profile| profile.adapter.clone()),
        model: execution.and_then(|profile| profile.model.clone()),
        cwd: execution.and_then(|profile| profile.cwd.clone()),
        skills: execution
            .map(|profile| profile.skills.clone())
            .unwrap_or_default(),
        session_state: execution_session_state_label(
            member,
            lead_member_id,
            Some((&member.profile.team_id, runtime_workers)),
        ),
        has_system_prompt: execution
            .and_then(|profile| profile.system_prompt.as_ref())
            .is_some(),
        env_keys,
        redacted_env: env,
    }
}

struct MemberActivityDerived {
    sent_count: usize,
    received_count: usize,
    mentioned_count: usize,
    last_sent_at: Option<DateTime<Utc>>,
    last_received_at: Option<DateTime<Utc>>,
    last_activity_at: Option<DateTime<Utc>>,
    items: Vec<MemberActivityItemView>,
}

impl MemberActivityDerived {
    fn into_view(self) -> MemberActivityView {
        MemberActivityView {
            sent_count: self.sent_count,
            received_count: self.received_count,
            mentioned_count: self.mentioned_count,
            last_sent_at: self.last_sent_at,
            last_received_at: self.last_received_at,
            last_activity_at: self.last_activity_at,
        }
    }
}

fn build_member_activity(name: &str, messages: &[Message]) -> MemberActivityDerived {
    let mut sent_count = 0;
    let mut received_count = 0;
    let mut mentioned_count = 0;
    let mut last_sent_at = None;
    let mut last_received_at = None;
    let mut last_activity_at = None;
    let mut items = Vec::new();

    for message in messages {
        let created_at = message.created_at;
        if message.sender == name {
            sent_count += 1;
            last_sent_at = Some(latest(last_sent_at, created_at));
            last_activity_at = Some(latest(last_activity_at, created_at));
            items.push(MemberActivityItemView {
                item_type: if matches!(message.kind, MessageKind::Reply) {
                    "sent_reply".into()
                } else {
                    "sent_message".into()
                },
                message_id: message.id.clone(),
                summary: format!("{} sent a message", name),
                created_at,
            });
        }
        if message
            .effective_recipients
            .iter()
            .any(|recipient| recipient == name)
        {
            received_count += 1;
            last_received_at = Some(latest(last_received_at, created_at));
            last_activity_at = Some(latest(last_activity_at, created_at));
            items.push(MemberActivityItemView {
                item_type: "received_message".into(),
                message_id: message.id.clone(),
                summary: format!("{} received a message", name),
                created_at,
            });
        }
        if message.mentions.iter().any(|mention| mention == name) {
            mentioned_count += 1;
            last_activity_at = Some(latest(last_activity_at, created_at));
            items.push(MemberActivityItemView {
                item_type: "mentioned".into(),
                message_id: message.id.clone(),
                summary: format!("{} was mentioned", name),
                created_at,
            });
        }
    }

    items.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then(a.message_id.cmp(&b.message_id))
    });

    MemberActivityDerived {
        sent_count,
        received_count,
        mentioned_count,
        last_sent_at,
        last_received_at,
        last_activity_at,
        items,
    }
}

fn message_view(message: &Message, thread_reply_count: usize, lead_member_id: &str) -> MessageView {
    MessageView {
        id: message.id.clone(),
        sender: message.sender.clone(),
        sender_kind: if message.sender == lead_member_id {
            "lead".into()
        } else {
            "member".into()
        },
        kind: message.kind.clone(),
        body: message.body.clone(),
        body_preview: body_preview(&message.body),
        created_at: message.created_at,
        mentions: message.mentions.clone(),
        effective_recipients: message.effective_recipients.clone(),
        delivery_status: message.delivery_status.clone(),
        read_count: message.read_by.len(),
        acked_count: message.acked_by.len(),
        reply_to: message.reply_to.clone(),
        thread_id: message.thread_id.clone(),
        thread_reply_count,
        is_thread_root: message.reply_to.is_none(),
    }
}

fn body_preview(body: &str) -> String {
    let mut preview = body.chars().take(120).collect::<String>();
    if body.chars().count() > 120 {
        preview.push_str("...");
    }
    preview
}

fn compute_thread_reply_counts(messages: &[Message]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for message in messages {
        if let Some(thread_id) = &message.thread_id {
            *counts.entry(thread_id.clone()).or_insert(0) += 1;
        }
    }
    counts
}

fn latest_message_at(messages: &[Message]) -> Option<DateTime<Utc>> {
    messages.iter().map(|message| message.created_at).max()
}

fn latest(a: Option<DateTime<Utc>>, b: DateTime<Utc>) -> DateTime<Utc> {
    a.map(|previous| previous.max(b)).unwrap_or(b)
}

fn execution_session_state_label(
    member: &crate::team_mode::storage::MemberRecord,
    lead_member_id: Option<&str>,
    runtime_lookup: Option<(&str, &crate::team_mode::runtime_workers::RuntimeWorkerStore)>,
) -> String {
    let is_lead = lead_member_id
        .map(|lead| lead == member.profile.name.as_str())
        .unwrap_or(matches!(member.profile.kind, MemberKind::Lead));
    if is_lead {
        return "coordinator".into();
    }
    // Skip the synthetic web-user member — it has no execution profile and
    // the sidecar would always say "not-spawned"; keep the existing label
    // (`not_spawned`) for it.
    if member.profile.role_label == WEB_USER_ROLE_LABEL
        && member.profile.name == WEB_USER_SENDER
    {
        return "not_spawned".into();
    }
    // Cross-reference the disk session_state with the runtime sidecar so
    // workers killed externally show up as `dead` in the web UI without
    // needing a `worker_list` MCP roundtrip. The sidecar is the same
    // source of truth used by the `worker_list` tool.
    if let Some((team_id, store)) = runtime_lookup {
        if let Ok(Some(state)) = store.state_for(team_id, &member.profile.name) {
            if state == crate::team_mode::runtime_workers::STATE_DEAD {
                return crate::team_mode::runtime_workers::STATE_DEAD.into();
            }
        }
    }
    member
        .execution
        .as_ref()
        .and_then(|profile| profile.session_state)
        .map(ExecutionSessionState::as_str)
        .unwrap_or("not_spawned")
        .to_string()
}

/// POST /api/teams/{team}/rooms/main/messages — Web user sends a message.
///
/// Body: `{"body": "...", "mentions": ["lead", ...]}` — `mentions` is
/// optional; @mentions written into `body` are also parsed by the routing
/// layer. The message is recorded with `sender = "user"` (a lazy-created
/// non-managed member; see `WEB_USER_SENDER`) and routed through the
/// SAME `MessageService::send` that the MCP `send_message` tool uses, so
/// inbox-notifier wakeups + lead_pending writes happen as usual. Workers
/// see the message exactly as if the lead had `@`-mentioned them.
///
/// `kind = Discussion` (not `Dispatch`) so the agent-loop treats it as a
/// regular conversational input rather than a task assignment — workers
/// reply but the type stays neutral. Bug 12's "no auto-add parent.sender"
/// rule still applies, so a worker reply only goes back to lead (via the
/// observability rule) unless the worker explicitly mentions someone.
pub fn post_main_room_message(
    state: &TeamModeWebState,
    team_id: &str,
    body_bytes: &[u8],
) -> Result<Message, WebError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PostMessageRequest {
        body: String,
        #[serde(default)]
        mentions: Vec<String>,
    }

    if body_bytes.is_empty() {
        return Err(WebError::bad_request(
            "request body is empty; expected JSON {body, mentions?}",
        ));
    }
    let req: PostMessageRequest = serde_json::from_slice(body_bytes)
        .map_err(|err| WebError::bad_request(format!("invalid JSON body: {err}")))?;

    let body_text = req.body.trim();
    if body_text.is_empty() {
        return Err(WebError::bad_request(
            "message body is empty; type something to send",
        ));
    }

    // Confirm the team exists before touching the member store. Without
    // this, lazy-create would fabricate a `user` member under a non-existent
    // team and the subsequent `send` would fail with a more confusing error.
    state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;

    ensure_web_user_member(state, team_id)?;

    let message = state.message_service.send(SendMessageRequest {
        team_id: team_id.to_string(),
        room_id: "main".to_string(),
        sender: WEB_USER_SENDER.to_string(),
        kind: MessageKind::Discussion,
        subject: None,
        body: req.body,
        mentions: req.mentions,
        visibility: Vec::new(),
        audience_policy: None,
        reply_to: None,
        thread_id: None,
        expires_at: None,
    })?;

    Ok(message)
}

/// Idempotently make sure the `user` member exists for this team.
///
/// Re-creates the member if it was previously soft-removed (status ==
/// `Removed`). The reactivation path mirrors what worker_add does for
/// dead workers — operationally equivalent, just without an execution
/// profile (the web user doesn't spawn a backend process).
fn ensure_web_user_member(state: &TeamModeWebState, team_id: &str) -> Result<(), WebError> {
    match state.member_service.get(team_id, WEB_USER_SENDER)? {
        Some(record) => {
            if matches!(record.profile.status, MemberStatus::Removed) {
                state.member_service.mark_active(team_id, WEB_USER_SENDER)?;
            }
            Ok(())
        }
        None => {
            state.member_service.add(AddMemberRequest {
                team_id: team_id.to_string(),
                name: WEB_USER_SENDER.to_string(),
                kind: MemberKind::Member,
                role_label: WEB_USER_ROLE_LABEL.to_string(),
                role_description: Some(
                    "Human user sending messages from the web UI. Not a managed agent.".into(),
                ),
                execution: None,
            })?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;

    use super::*;

    #[test]
    fn redact_env_masks_sensitive_keys() {
        let mut env = HashMap::new();
        env.insert("RUST_LOG".into(), "info".into());
        env.insert("ANTHROPIC_API_KEY".into(), "abc".into());
        env.insert("session_token".into(), "xyz".into());
        env.insert("COOKIE_JAR".into(), "123".into());

        let redacted = redact_env(&env);
        assert_eq!(redacted["RUST_LOG"], "info");
        assert_eq!(redacted["ANTHROPIC_API_KEY"], "***");
        assert_eq!(redacted["session_token"], "***");
        assert_eq!(redacted["COOKIE_JAR"], "***");
    }

    #[test]
    fn execution_state_labels_lead_as_coordinator() {
        let member = crate::team_mode::storage::MemberRecord {
            profile: crate::team_mode::domain::MemberProfile {
                team_id: "demo".into(),
                name: "lead".into(),
                kind: MemberKind::Lead,
                role_label: "lead".into(),
                role_description: None,
                status: MemberStatus::Active,
                joined_at: Utc::now(),
            },
            execution: None,
        };

        assert_eq!(
            execution_session_state_label(&member, Some("lead"), None),
            "coordinator"
        );
    }
}
