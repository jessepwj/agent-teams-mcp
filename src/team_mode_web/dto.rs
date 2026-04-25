use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::runtime::ExecutionSessionState;
use crate::team_mode::domain::{DeliveryStatus, MemberKind, MemberStatus, TeamStatus};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsResponse {
    pub teams: Vec<TeamSummaryView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamResponse {
    pub team: TeamDetailView,
    pub counts: TeamCountsView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamSummaryView {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub status: TeamStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_member_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_cc_pid: Option<u32>,
    pub member_count: usize,
    pub active_worker_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamDetailView {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub status: TeamStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_member_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_cc_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamCountsView {
    pub member_count: usize,
    pub active_worker_count: usize,
    pub message_count: usize,
    pub thread_count: usize,
    pub unread_for_lead: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomResponse {
    pub room: RoomView,
    pub messages: Vec<MessageView>,
    pub page: PageView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomView {
    pub id: String,
    pub team_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageView {
    pub has_more_before: bool,
    pub has_more_after: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub id: String,
    pub sender: String,
    pub sender_kind: String,
    pub kind: crate::team_mode::domain::MessageKind,
    pub body: String,
    pub body_preview: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effective_recipients: Vec<String>,
    pub delivery_status: DeliveryStatus,
    pub read_count: usize,
    pub acked_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub thread_reply_count: usize,
    pub is_thread_root: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembersResponse {
    pub members: Vec<MemberSummaryView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberSummaryView {
    pub name: String,
    pub kind: MemberKind,
    pub role_label: String,
    pub status: MemberStatus,
    pub session_state: String,
    pub badge: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberResponse {
    pub profile: MemberProfileView,
    pub execution: MemberExecutionView,
    pub activity: MemberActivityView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberProfileView {
    pub name: String,
    pub kind: MemberKind,
    pub role_label: String,
    pub status: MemberStatus,
    pub joined_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberExecutionView {
    pub execution_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    pub session_state: String,
    pub has_system_prompt: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_keys: Vec<String>,
    pub redacted_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberActivityView {
    pub sent_count: usize,
    pub received_count: usize,
    pub mentioned_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sent_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_received_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberActivityResponse {
    pub member: String,
    pub source: String,
    pub items: Vec<MemberActivityItemView>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberConversationResponse {
    pub member: String,
    pub source: ConversationSourceView,
    pub items: Vec<ConversationItemView>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSourceView {
    pub provider: String,
    pub confidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationItemView {
    pub id: String,
    pub role: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberActivityItemView {
    pub item_type: String,
    pub message_id: String,
    pub summary: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsResponse {
    pub team_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub generated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
    pub sources: Vec<DiagnosticsSourceView>,
    pub lead_session: LeadSessionDiagnosticsView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsSourceView {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub path: String,
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeadSessionDiagnosticsView {
    pub discovered: bool,
    pub session_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_modified_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_tool_calls: Vec<LeadSessionToolCallView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<LeadSessionTokenUsageView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeadSessionToolCallView {
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeadSessionTokenUsageView {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    pub total_tokens: u64,
}

pub fn execution_session_state_label(
    state: Option<ExecutionSessionState>,
    is_lead: bool,
) -> String {
    if is_lead {
        return "coordinator".to_string();
    }
    state
        .map(ExecutionSessionState::as_str)
        .unwrap_or("not_spawned")
        .to_string()
}

fn is_false(value: &bool) -> bool {
    !*value
}
