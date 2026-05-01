use std::fs;
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::team_mode::data_dir;

use super::{WebError, message_view};
use crate::team_mode_web::dto::{EventView, EventsPageView, EventsResponse};
use crate::team_mode_web::state::TeamModeWebState;

const DEFAULT_EVENT_LIMIT: usize = 100;
const MAX_EVENT_LIMIT: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct EventCursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    messages_last_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    messages_last_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workers_updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workers_last_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lead_pending_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lead_pending_modified_at: Option<DateTime<Utc>>,
}

pub fn read_events(
    state: &TeamModeWebState,
    team_id: &str,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<EventsResponse, WebError> {
    let team = state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;
    let current = current_cursor(state, team_id)?;
    let previous = decode_optional_cursor(cursor)?;
    let limit = limit
        .unwrap_or(DEFAULT_EVENT_LIMIT)
        .clamp(1, MAX_EVENT_LIMIT);

    let (events, has_more_after, next_cursor) = if let Some(previous) = previous {
        let mut out = Vec::new();
        out.extend(message_events(
            state,
            team_id,
            team.lead_member_id.as_deref(),
            &previous,
        )?);
        out.extend(file_changed_events(state, team_id, &previous, &current)?);
        out.extend(worker_events(state, team_id, &previous)?);
        out.sort_by(|a, b| {
            a.event
                .occurred_at
                .cmp(&b.event.occurred_at)
                .then(a.event.id.cmp(&b.event.id))
        });
        finalize_events(out, previous, current, limit)?
    } else {
        (Vec::new(), false, encode_cursor(&current)?)
    };

    Ok(EventsResponse {
        team_id: team_id.to_string(),
        generated_at: Utc::now(),
        events,
        page: EventsPageView {
            has_more_after,
            next_cursor,
        },
        limitations: Vec::new(),
    })
}

#[derive(Debug, Clone)]
struct PendingEvent {
    event: EventView,
    cursor_after: EventCursor,
}

fn finalize_events(
    pending: Vec<PendingEvent>,
    previous: EventCursor,
    current: EventCursor,
    limit: usize,
) -> Result<(Vec<EventView>, bool, String), WebError> {
    let has_more_after = pending.len() > limit;
    let mut cursor = previous;
    let mut events = Vec::with_capacity(pending.len().min(limit));
    for pending in pending.into_iter().take(limit) {
        cursor = cursor.merge_after(&pending.cursor_after);
        let mut event = pending.event;
        event.cursor = encode_cursor(&cursor)?;
        events.push(event);
    }
    let next_cursor = if events.is_empty() && !has_more_after {
        encode_cursor(&current)?
    } else {
        encode_cursor(&cursor)?
    };
    Ok((events, has_more_after, next_cursor))
}

impl EventCursor {
    fn merge_after(&self, after: &Self) -> Self {
        Self {
            messages_last_at: after.messages_last_at.or(self.messages_last_at),
            messages_last_id: after
                .messages_last_id
                .clone()
                .or_else(|| self.messages_last_id.clone()),
            workers_updated_at: after.workers_updated_at.or(self.workers_updated_at),
            workers_last_name: after
                .workers_last_name
                .clone()
                .or_else(|| self.workers_last_name.clone()),
            lead_pending_size: after.lead_pending_size.or(self.lead_pending_size),
            lead_pending_modified_at: after
                .lead_pending_modified_at
                .or(self.lead_pending_modified_at),
        }
    }
}

fn current_cursor(state: &TeamModeWebState, team_id: &str) -> Result<EventCursor, WebError> {
    let messages = state.message_service.list_by_room(team_id, "main")?;
    let latest_message = messages
        .iter()
        .max_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let workers = state.runtime_workers.list_all()?;
    let latest_worker = workers
        .iter()
        .filter(|worker| worker.team == team_id)
        .max_by(|a, b| a.updated_at.cmp(&b.updated_at).then(a.name.cmp(&b.name)));
    let lead_pending = file_watermark(&data_dir::lead_pending_file_for_team(
        state.base_dir(),
        team_id,
    ));

    Ok(EventCursor {
        messages_last_at: latest_message.map(|message| message.created_at),
        messages_last_id: latest_message.map(|message| message.id.clone()),
        workers_updated_at: latest_worker.map(|worker| worker.updated_at),
        workers_last_name: latest_worker.map(|worker| worker.name.clone()),
        lead_pending_size: lead_pending.as_ref().map(|watermark| watermark.size),
        lead_pending_modified_at: lead_pending.map(|watermark| watermark.modified_at),
    })
}

fn message_events(
    state: &TeamModeWebState,
    team_id: &str,
    lead_member_id: Option<&str>,
    previous: &EventCursor,
) -> Result<Vec<PendingEvent>, WebError> {
    let mut messages = state.message_service.list_by_room(team_id, "main")?;
    messages.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let lead_member_id = lead_member_id.unwrap_or("lead");
    let mut events = Vec::new();
    for message in messages {
        if !message_after_cursor(&message, previous) {
            continue;
        }
        let cursor_after = EventCursor {
            messages_last_at: Some(message.created_at),
            messages_last_id: Some(message.id.clone()),
            ..EventCursor::default()
        };
        events.push(PendingEvent {
            event: EventView {
                id: format!("{}:message:{}", team_id, message.id),
                team_id: team_id.to_string(),
                event_type: "messageCreated".into(),
                occurred_at: message.created_at,
                source: "messages".into(),
                cursor: String::new(),
                payload: json!({
                    "message": message_view(&message, 0, lead_member_id)
                }),
            },
            cursor_after,
        });
    }
    Ok(events)
}

fn message_after_cursor(message: &crate::team_mode::domain::Message, cursor: &EventCursor) -> bool {
    match (cursor.messages_last_at, cursor.messages_last_id.as_deref()) {
        (Some(last_at), Some(last_id)) => {
            message.created_at > last_at
                || (message.created_at == last_at && message.id.as_str() > last_id)
        }
        (Some(last_at), None) => message.created_at > last_at,
        (None, _) => true,
    }
}

fn file_changed_events(
    _state: &TeamModeWebState,
    team_id: &str,
    previous: &EventCursor,
    current: &EventCursor,
) -> Result<Vec<PendingEvent>, WebError> {
    if previous.lead_pending_size == current.lead_pending_size
        && previous.lead_pending_modified_at == current.lead_pending_modified_at
    {
        return Ok(Vec::new());
    }
    let Some(size_bytes) = current.lead_pending_size else {
        return Ok(Vec::new());
    };
    let Some(modified_at) = current.lead_pending_modified_at else {
        return Ok(Vec::new());
    };
    Ok(vec![PendingEvent {
        event: EventView {
            id: format!(
                "{}:file:leadPending:{}",
                team_id,
                modified_at.timestamp_nanos_opt().unwrap_or_default()
            ),
            team_id: team_id.to_string(),
            event_type: "fileChanged".into(),
            occurred_at: modified_at,
            source: "filesystem".into(),
            cursor: String::new(),
            payload: json!({
                "fileId": "leadPending",
                "path": format!("{team_id}/{}", data_dir::FILE_LEAD_PENDING),
                "changeKind": "modified",
                "sizeBytes": size_bytes,
                "modifiedAt": modified_at,
            }),
        },
        cursor_after: EventCursor {
            lead_pending_size: Some(size_bytes),
            lead_pending_modified_at: Some(modified_at),
            ..EventCursor::default()
        },
    }])
}

fn worker_events(
    state: &TeamModeWebState,
    team_id: &str,
    previous: &EventCursor,
) -> Result<Vec<PendingEvent>, WebError> {
    let workers = state.runtime_workers.list_all()?;
    let mut events = Vec::new();
    for worker in workers.into_iter().filter(|worker| worker.team == team_id) {
        if !worker_after_cursor(&worker, previous) {
            continue;
        }
        let worker_name = worker.name.clone();
        events.push(PendingEvent {
            event: EventView {
                id: format!(
                    "{}:worker:{}:{}",
                    team_id,
                    worker.name,
                    worker.updated_at.timestamp_nanos_opt().unwrap_or_default()
                ),
                team_id: team_id.to_string(),
                event_type: "workerStatusChanged".into(),
                occurred_at: worker.updated_at,
                source: "runtimeWorkers".into(),
                cursor: String::new(),
                payload: json!({
                    "workerName": worker.name,
                    "lifecycleEvent": lifecycle_event(&worker.state),
                    "sessionState": worker.state,
                    "previousSessionState": Value::Null,
                    "adapter": worker.adapter,
                    "model": Value::Null,
                    "cwd": Value::Null,
                    "note": worker.note,
                }),
            },
            cursor_after: EventCursor {
                workers_updated_at: Some(worker.updated_at),
                workers_last_name: Some(worker_name),
                ..EventCursor::default()
            },
        });
    }
    Ok(events)
}

fn worker_after_cursor(
    worker: &crate::team_mode::runtime_workers::RuntimeWorkerRecord,
    cursor: &EventCursor,
) -> bool {
    match (
        cursor.workers_updated_at,
        cursor.workers_last_name.as_deref(),
    ) {
        (Some(last_at), Some(last_name)) => {
            worker.updated_at > last_at
                || (worker.updated_at == last_at && worker.name.as_str() > last_name)
        }
        (Some(last_at), None) => worker.updated_at > last_at,
        (None, _) => true,
    }
}

fn lifecycle_event(state: &str) -> &'static str {
    match state {
        crate::team_mode::runtime_workers::STATE_DEAD
        | crate::team_mode::runtime_workers::STATE_FAILED
        | crate::team_mode::runtime_workers::STATE_STOPPED => "dead",
        crate::team_mode::runtime_workers::STATE_RUNNING => "alive",
        _ => "spawn",
    }
}

#[derive(Debug)]
struct FileWatermark {
    size: u64,
    modified_at: DateTime<Utc>,
}

fn file_watermark(path: &Path) -> Option<FileWatermark> {
    let metadata = fs::metadata(path).ok()?;
    Some(FileWatermark {
        size: metadata.len(),
        modified_at: system_time_to_utc(metadata.modified().ok()?),
    })
}

fn system_time_to_utc(value: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(value)
}

fn encode_cursor(cursor: &EventCursor) -> Result<String, WebError> {
    let bytes = serde_json::to_vec(cursor).map_err(|err| WebError::internal(err.to_string()))?;
    Ok(hex_encode(&bytes))
}

fn decode_optional_cursor(value: Option<&str>) -> Result<Option<EventCursor>, WebError> {
    match value {
        None | Some("") => Ok(None),
        Some(value) => decode_cursor(value).map(Some),
    }
}

fn decode_cursor(value: &str) -> Result<EventCursor, WebError> {
    let bytes = hex_decode(value).ok_or_else(invalid_cursor)?;
    serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())
}

fn invalid_cursor() -> WebError {
    WebError::bad_request("invalid cursor")
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let hi = hex_value(pair[0])?;
        let lo = hex_value(pair[1])?;
        bytes.push((hi << 4) | lo);
    }
    Some(bytes)
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
