use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::runtime::ExecutionSessionState;
use crate::team_mode::domain::{MemberKind, MemberStatus, Message, MessageKind, Team};
use crate::util::session_discovery;

use super::dto::*;
use super::error::WebError;
use super::state::TeamModeWebState;

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
    let views = members
        .into_iter()
        .map(|member| member_summary_view(&team, &member, &messages))
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
        execution: member_execution_view(&member, team.lead_member_id.as_deref()),
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

pub fn read_member_conversation(
    state: &TeamModeWebState,
    team_id: &str,
    name: &str,
) -> Result<MemberConversationResponse, WebError> {
    let team = state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;
    let member = state.member_service.get(team_id, name)?.ok_or_else(|| {
        WebError::not_found(format!("member '{name}' not found in team '{team_id}'"))
    })?;

    let execution = member.execution.as_ref();
    let cwd = execution
        .and_then(|profile| profile.cwd.clone())
        .or_else(|| team.cwd.clone())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|path| path.display().to_string())
        });
    let provider = execution
        .and_then(|profile| profile.adapter.clone())
        .unwrap_or_else(|| "claude-code".into());

    let Some(cwd) = cwd else {
        return Ok(empty_conversation(
            name,
            provider,
            "no_cwd",
            None,
            vec!["No cwd is available for this member or team.".into()],
        ));
    };

    if provider != "claude-code" {
        return Ok(empty_conversation(
            name,
            provider,
            "unsupported_provider",
            Some(cwd),
            vec![
                "Conversation rendering currently supports Claude Code JSONL sessions only.".into(),
            ],
        ));
    }

    let sessions = session_discovery::discover_sessions(Path::new(&cwd));
    let requested_session_id = execution.and_then(|profile| profile.session_id.clone());
    let selected_session = requested_session_id
        .as_ref()
        .and_then(|session_id| {
            sessions
                .iter()
                .find(|session| &session.session_id == session_id)
        })
        .or_else(|| sessions.first());
    let Some(session) = selected_session else {
        return Ok(empty_conversation(
            name,
            provider,
            "no_session_file",
            Some(cwd),
            vec![
                "No Claude Code session JSONL file was found for this member cwd.".into(),
                "The lookup is scoped to the member cwd first, then team cwd.".into(),
            ],
        ));
    };

    let items = parse_claude_conversation(&session.path)?;
    let exact_match = requested_session_id
        .as_ref()
        .map(|session_id| session_id == &session.session_id)
        .unwrap_or(false);
    let mut limitations = Vec::new();
    if exact_match {
        limitations
            .push("The session is matched by the member's persisted backend session id.".into());
    } else {
        limitations.push(
            "The session is matched by cwd and latest modified Claude Code JSONL file.".into(),
        );
        limitations.push("Per-member exact session id is not available or was not found, so concurrent members sharing one cwd can be ambiguous.".into());
    }
    Ok(MemberConversationResponse {
        member: name.to_string(),
        source: ConversationSourceView {
            provider,
            confidence: if exact_match {
                "session_id".into()
            } else {
                "cwd_latest".into()
            },
            session_id: Some(session.session_id.clone()),
            path: Some(session.path.display().to_string()),
            updated_at: session.modified,
            cwd: Some(cwd),
        },
        items,
        limitations,
    })
}

fn empty_conversation(
    member: &str,
    provider: String,
    confidence: &str,
    cwd: Option<String>,
    limitations: Vec<String>,
) -> MemberConversationResponse {
    MemberConversationResponse {
        member: member.to_string(),
        source: ConversationSourceView {
            provider,
            confidence: confidence.into(),
            session_id: None,
            path: None,
            updated_at: None,
            cwd,
        },
        items: Vec::new(),
        limitations,
    }
}

pub fn read_diagnostics(
    state: &TeamModeWebState,
    team_id: &str,
) -> Result<DiagnosticsResponse, WebError> {
    let team = state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;
    let project_root = diagnostics_project_root(&team);
    let base_dir = state.base_dir().to_path_buf();
    let sources = diagnostics_sources(&project_root, &base_dir);

    let lead_session = read_lead_session_diagnostics(&project_root, &base_dir);

    Ok(DiagnosticsResponse {
        team_id: team.id.clone(),
        team_name: Some(team.name.clone()),
        cwd: team.cwd.clone(),
        generated_at: Utc::now(),
        limitations: vec![
            "These diagnostics are file/session-level observations, not per-member stdout/stderr.".into(),
            "Lead pending queue sources are probed in the project root and the web data base_dir; the real source may live in either place.".into(),
        ],
        sources,
        lead_session,
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
    let member_count = members.len();
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
) -> MemberSummaryView {
    let activity = build_member_activity(&member.profile.name, messages);
    let execution = member.execution.as_ref();
    MemberSummaryView {
        name: member.profile.name.clone(),
        kind: member.profile.kind.clone(),
        role_label: member.profile.role_label.clone(),
        status: member.profile.status.clone(),
        session_state: execution_session_state_label(member, team.lead_member_id.as_deref()),
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
        session_state: execution_session_state_label(member, lead_member_id),
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

fn parse_claude_conversation(path: &Path) -> Result<Vec<ConversationItemView>, WebError> {
    let content = fs::read_to_string(path)
        .map_err(|err| WebError::internal(format!("failed to read session file: {err}")))?;
    let mut items = Vec::new();

    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        append_conversation_items(&mut items, index, &value);
    }

    if items.len() > 200 {
        let start = items.len().saturating_sub(200);
        items = items.into_iter().skip(start).collect();
    }

    Ok(items)
}

fn append_conversation_items(items: &mut Vec<ConversationItemView>, index: usize, value: &Value) {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let timestamp = json_timestamp(value);

    match kind {
        "user" => append_user_items(items, index, value, timestamp),
        "assistant" => append_assistant_items(items, index, value, timestamp),
        "tool_use" => {
            let title = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let tool_use_id = value
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let input = value.get("input").cloned();
            let text = value
                .get("input")
                .map(compact_json)
                .filter(|text| !text.is_empty());
            items.push(conversation_tool_item(
                index,
                0,
                "tool_use",
                Some(title.clone()),
                text,
                tool_use_id,
                Some(title),
                input,
                None,
                false,
                timestamp,
            ));
        }
        "result" => {
            let is_error = value
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Some(text) = value.get("result").and_then(Value::as_str) {
                if is_error || !text.trim().is_empty() {
                    items.push(conversation_item_with_payload(
                        index,
                        0,
                        if is_error { "error" } else { "assistant" },
                        if is_error { "error" } else { "result" },
                        None,
                        Some(trim_conversation_text(text)),
                        None,
                        None,
                        None,
                        value.get("result").cloned(),
                        is_error,
                        timestamp,
                    ));
                }
            }
        }
        "system" => {
            let text = value
                .get("content")
                .and_then(Value::as_str)
                .or_else(|| value.get("message").and_then(Value::as_str))
                .map(trim_conversation_text);
            if text.is_some() {
                items.push(conversation_item(
                    index, 0, "system", "system", None, text, timestamp,
                ));
            }
        }
        _ => {}
    }
}

fn append_user_items(
    items: &mut Vec<ConversationItemView>,
    index: usize,
    value: &Value,
    timestamp: Option<DateTime<Utc>>,
) {
    let Some(content) = message_content(value) else {
        return;
    };
    append_content_blocks(items, index, "user", content, value, timestamp);
}

fn append_assistant_items(
    items: &mut Vec<ConversationItemView>,
    index: usize,
    value: &Value,
    timestamp: Option<DateTime<Utc>>,
) {
    let Some(content) = message_content(value) else {
        return;
    };
    append_content_blocks(items, index, "assistant", content, value, timestamp);
}

fn append_content_blocks(
    items: &mut Vec<ConversationItemView>,
    index: usize,
    role: &str,
    content: &Value,
    message: &Value,
    timestamp: Option<DateTime<Utc>>,
) {
    match content {
        Value::String(text) => {
            if !text.trim().is_empty() {
                items.push(conversation_item(
                    index,
                    0,
                    role,
                    "text",
                    None,
                    Some(trim_conversation_text(text)),
                    timestamp,
                ));
            }
        }
        Value::Array(blocks) => {
            for (block_index, block) in blocks.iter().enumerate() {
                if let Some(text) = block.as_str() {
                    if !text.trim().is_empty() {
                        items.push(conversation_item(
                            index,
                            block_index,
                            role,
                            "text",
                            None,
                            Some(trim_conversation_text(text)),
                            timestamp,
                        ));
                    }
                    continue;
                }

                let block_type = block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                match block_type {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            if !text.trim().is_empty() {
                                items.push(conversation_item(
                                    index,
                                    block_index,
                                    role,
                                    "text",
                                    None,
                                    Some(trim_conversation_text(text)),
                                    timestamp,
                                ));
                            }
                        }
                    }
                    "thinking" => {
                        let text = block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .or_else(|| block.get("text").and_then(Value::as_str))
                            .map(trim_conversation_text);
                        items.push(conversation_item(
                            index,
                            block_index,
                            "assistant",
                            "thinking",
                            Some("Thinking".into()),
                            text,
                            timestamp,
                        ));
                    }
                    "tool_use" => {
                        let title = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let tool_use_id = block
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToString::to_string);
                        let input = block.get("input").cloned();
                        let text = block
                            .get("input")
                            .map(compact_json)
                            .filter(|text| !text.is_empty());
                        items.push(conversation_tool_item(
                            index,
                            block_index,
                            "tool_use",
                            Some(title.clone()),
                            text,
                            tool_use_id,
                            Some(title),
                            input,
                            None,
                            false,
                            timestamp,
                        ));
                    }
                    "tool_result" => {
                        let tool_use_id = block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .map(ToString::to_string);
                        let title = tool_use_id
                            .as_ref()
                            .map(|id| format!("Tool result {id}"))
                            .unwrap_or_else(|| "Tool result".into());
                        let is_error = block
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let text = block
                            .get("content")
                            .map(extract_content_text)
                            .filter(|text| !text.is_empty());
                        let result = block
                            .get("content")
                            .cloned()
                            .or_else(|| message.get("toolUseResult").cloned())
                            .or_else(|| message.get("tool_use_result").cloned());
                        items.push(conversation_tool_item(
                            index,
                            block_index,
                            "tool_result",
                            Some(title),
                            text,
                            tool_use_id,
                            None,
                            None,
                            result,
                            is_error,
                            timestamp,
                        ));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn conversation_item(
    line_index: usize,
    block_index: usize,
    role: &str,
    kind: &str,
    title: Option<String>,
    text: Option<String>,
    timestamp: Option<DateTime<Utc>>,
) -> ConversationItemView {
    conversation_item_with_payload(
        line_index,
        block_index,
        role,
        kind,
        title,
        text,
        None,
        None,
        None,
        None,
        false,
        timestamp,
    )
}

fn conversation_tool_item(
    line_index: usize,
    block_index: usize,
    kind: &str,
    title: Option<String>,
    text: Option<String>,
    tool_use_id: Option<String>,
    tool_name: Option<String>,
    input: Option<Value>,
    result: Option<Value>,
    is_error: bool,
    timestamp: Option<DateTime<Utc>>,
) -> ConversationItemView {
    conversation_item_with_payload(
        line_index,
        block_index,
        if is_error { "error" } else { "tool" },
        kind,
        title,
        text,
        tool_use_id,
        tool_name,
        input,
        result,
        is_error,
        timestamp,
    )
}

fn conversation_item_with_payload(
    line_index: usize,
    block_index: usize,
    role: &str,
    kind: &str,
    title: Option<String>,
    text: Option<String>,
    tool_use_id: Option<String>,
    tool_name: Option<String>,
    input: Option<Value>,
    result: Option<Value>,
    is_error: bool,
    timestamp: Option<DateTime<Utc>>,
) -> ConversationItemView {
    ConversationItemView {
        id: format!("{line_index}:{block_index}"),
        role: role.into(),
        kind: kind.into(),
        title,
        text,
        tool_use_id,
        tool_name,
        input,
        result,
        is_error,
        timestamp,
    }
}

fn message_content(value: &Value) -> Option<&Value> {
    value
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| value.get("content"))
}

fn extract_content_text(value: &Value) -> String {
    match value {
        Value::String(text) => trim_conversation_text(text),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => text.clone(),
                Value::Object(_) => item
                    .get("text")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| compact_json(item)),
                _ => compact_json(item),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => compact_json(value),
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|_| value.to_string())
        .chars()
        .take(4000)
        .collect()
}

fn trim_conversation_text(text: &str) -> String {
    let mut trimmed = text.trim().chars().take(6000).collect::<String>();
    if text.trim().chars().count() > 6000 {
        trimmed.push_str("\n...");
    }
    trimmed
}

fn json_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn diagnostics_project_root(team: &Team) -> PathBuf {
    team.cwd
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn diagnostics_sources(project_root: &Path, base_dir: &Path) -> Vec<DiagnosticsSourceView> {
    let mut seen_paths = BTreeSet::new();
    let mut sources = Vec::new();
    let mcp_log_path = preferred_diagnostics_path(project_root, base_dir, "mcp.log");
    let wake_log_path =
        preferred_diagnostics_path(project_root, base_dir, ".lead-pending-wake.log");
    let candidates = [
        (
            "lead_pending_jsonl_project_root",
            "Lead Pending Queue (project root)",
            "file",
            project_root.join("lead_pending.jsonl"),
        ),
        (
            "lead_pending_jsonl_base_dir",
            "Lead Pending Queue (base dir)",
            "file",
            base_dir.join("lead_pending.jsonl"),
        ),
        ("mcp_log", "MCP Log", "file", mcp_log_path),
        (
            "lead_pending_wake_log",
            "Lead Pending Wake Log",
            "file",
            wake_log_path,
        ),
    ];

    for (id, label, kind, path) in candidates {
        let key = path.display().to_string();
        if !seen_paths.insert(key.clone()) {
            continue;
        }
        sources.push(read_diagnostics_source(&path, id, label, kind));
    }

    sources
}

fn preferred_diagnostics_path(project_root: &Path, base_dir: &Path, file_name: &str) -> PathBuf {
    let project_candidate = project_root.join(file_name);
    let base_candidate = base_dir.join(file_name);

    match (
        fs::metadata(&project_candidate),
        fs::metadata(&base_candidate),
    ) {
        (Ok(project_meta), Ok(base_meta)) => {
            let project_modified = project_meta.modified().ok();
            let base_modified = base_meta.modified().ok();
            if project_modified >= base_modified {
                project_candidate
            } else {
                base_candidate
            }
        }
        (Ok(_), Err(_)) => project_candidate,
        (Err(_), Ok(_)) => base_candidate,
        (Err(_), Err(_)) => project_candidate,
    }
}

fn read_diagnostics_source(
    path: &Path,
    id: &str,
    label: &str,
    kind: &str,
) -> DiagnosticsSourceView {
    let metadata = fs::metadata(path).ok();
    let exists = metadata.is_some();
    let size_bytes = metadata.as_ref().map(|meta| meta.len());
    let updated_at = metadata
        .as_ref()
        .and_then(|meta| meta.modified().ok().map(DateTime::<Utc>::from));
    let preview = if exists {
        read_preview(path)
    } else {
        "not found".into()
    };

    DiagnosticsSourceView {
        id: id.into(),
        label: label.into(),
        kind: kind.into(),
        path: path.display().to_string(),
        exists,
        size_bytes,
        updated_at,
        preview,
    }
}

fn read_preview(path: &Path) -> String {
    match fs::File::open(path) {
        Ok(mut file) => {
            let mut buf = vec![0_u8; 4096];
            match file.read(&mut buf) {
                Ok(0) => "empty".into(),
                Ok(n) => {
                    let content = String::from_utf8_lossy(&buf[..n]).to_string();
                    let snippet = content.lines().take(8).collect::<Vec<_>>().join("\n");
                    crate::models::token::truncate_string(&snippet, 800)
                }
                Err(err) => format!("unavailable: {err}"),
            }
        }
        Err(err) => format!("unavailable: {err}"),
    }
}

fn read_lead_session_diagnostics(
    project_root: &Path,
    base_dir: &Path,
) -> LeadSessionDiagnosticsView {
    let mut sessions = session_discovery::discover_sessions(project_root);
    if sessions.is_empty() && base_dir != project_root {
        sessions = session_discovery::discover_sessions(base_dir);
    }
    let discovered = !sessions.is_empty();
    let latest = sessions.first();

    let mut view = LeadSessionDiagnosticsView {
        discovered,
        session_count: sessions.len(),
        latest_session_id: latest.map(|session| session.session_id.clone()),
        latest_modified_at: latest.and_then(|session| session.modified.clone()),
        recent_tool_calls: Vec::new(),
        token_usage: None,
        limitations: vec![
            "Lead session diagnostics sample Claude session files only; they do not expose per-member stdout/stderr.".into(),
            "Recent tool calls are truncated and derived from the latest discovered Claude session.".into(),
        ],
        source_path: latest.map(|session| session.path.display().to_string()),
    };

    if let Some(session) = latest {
        match session_discovery::parse_session_file(&session.path) {
            Ok((tool_calls, token_usage)) => {
                view.recent_tool_calls = tool_calls
                    .into_iter()
                    .take(10)
                    .map(|call| LeadSessionToolCallView {
                        tool_name: call.tool_name,
                        input_summary: call.input_summary,
                        timestamp: call.timestamp,
                    })
                    .collect();
                view.token_usage = token_usage.map(|usage| LeadSessionTokenUsageView {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    cache_write_tokens: usage.cache_write_tokens,
                    total_tokens: usage.total(),
                });
            }
            Err(err) => {
                view.limitations
                    .push(format!("Latest session could not be parsed: {err}"));
            }
        }
    }

    view
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
) -> String {
    let is_lead = lead_member_id
        .map(|lead| lead == member.profile.name.as_str())
        .unwrap_or(matches!(member.profile.kind, MemberKind::Lead));
    if is_lead {
        "coordinator".into()
    } else {
        member
            .execution
            .as_ref()
            .and_then(|profile| profile.session_state)
            .map(ExecutionSessionState::as_str)
            .unwrap_or("not_spawned")
            .to_string()
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
            execution_session_state_label(&member, Some("lead")),
            "coordinator"
        );
    }
}
