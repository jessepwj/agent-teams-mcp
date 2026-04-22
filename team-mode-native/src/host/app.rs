use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex as StdMutex};
use std::thread;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tokio::time;
use uuid::Uuid;

use crate::domain::{
    AdapterKind, CommandSpec, ExecutionProfile, InboxItem, LaunchMode, MemberKind,
    MemberRecord, MemberStatus, Message, MessageKind, PromptMode, RestartPolicy,
    Room, RoomKind, RoomStatus, Team, Thread, ThreadStatus, ViewerMode,
};
use crate::service::{AddMember, CreateTeam, RoomPost, UpdateMember};
pub type TeamModeServices = crate::service::TeamModeServices;
use crate::{Error, Result};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct TeamModeHost {
    pub services: TeamModeServices,
    data_dir: PathBuf,
    state: Arc<Mutex<HostState>>,
}

#[derive(Debug, Default)]
struct HostState {
    teams: BTreeMap<String, Team>,
    rooms: BTreeMap<String, Room>,
    members: BTreeMap<String, MemberRecord>,
    threads: BTreeMap<String, Thread>,
    messages: BTreeMap<String, Message>,
    inbox: BTreeMap<String, Vec<InboxItem>>,
    raw_logs: BTreeMap<String, VecDeque<RunnerLogLine>>,
    runners: BTreeMap<String, RunnerRuntime>,
    sessions: BTreeMap<String, ManagedSessionSummary>,
    codex_sessions: BTreeMap<String, CodexRuntime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub data_dir: PathBuf,
    pub team_count: usize,
    pub member_count: usize,
    pub message_count: usize,
    pub runner_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamCreateRequest {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamGetRequest {
    pub team_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamDeleteRequest {
    pub team_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberAddRequest {
    pub team_id: String,
    pub id: String,
    pub handle: String,
    pub name: String,
    #[serde(default)]
    pub kind: Option<MemberKind>,
    #[serde(default)]
    pub role_label: Option<String>,
    #[serde(default)]
    pub role_description: Option<String>,
    #[serde(default)]
    pub execution: Option<ExecutionProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberGetRequest {
    pub team_id: String,
    pub member_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberUpdateRequest {
    pub team_id: String,
    pub member_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub role_label: Option<String>,
    #[serde(default)]
    pub role_description: Option<String>,
    #[serde(default)]
    pub clear_role_description: bool,
    #[serde(default)]
    pub status: Option<MemberStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberRemoveRequest {
    pub team_id: String,
    pub member_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionSetRequest {
    pub member_id: String,
    pub execution: ExecutionProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomPostRequest {
    pub team_id: String,
    #[serde(default = "default_main_room")]
    pub room_id: String,
    pub sender_member_id: String,
    pub body: String,
    #[serde(default)]
    pub kind: Option<MessageKind>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomListRequest {
    pub team_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomReadMessagesRequest {
    pub team_id: String,
    #[serde(default = "default_main_room")]
    pub room_id: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReplyRequest {
    pub thread_id: String,
    pub sender_member_id: String,
    pub body: String,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadRequest {
    pub thread_id: String,
    #[serde(default)]
    pub team_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadResult {
    pub thread: Thread,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectSendRequest {
    pub team_id: String,
    pub sender_member_id: String,
    pub recipient_member_id: String,
    pub body: String,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectReplyRequest {
    pub thread_id: String,
    pub sender_member_id: String,
    pub body: String,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectReadRequest {
    pub team_id: String,
    pub thread_id: String,
    pub member_id: String,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectListRequest {
    pub team_id: String,
    pub member_id: String,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxPeekRequest {
    pub member_id: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxReadRequest {
    pub member_id: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxAckRequest {
    pub member_id: String,
    pub message_id: String,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxCountRequest {
    pub member_id: String,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberTailRequest {
    pub member_id: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerInjectRequest {
    pub member_id: String,
    pub text: String,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub caller_member_id: Option<String>,
    #[serde(default)]
    pub strategy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSteerRequest {
    pub member_id: String,
    pub text: String,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexInterruptRequest {
    pub member_id: String,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberSpawnManagedRequest {
    pub member_id: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub token_env: Option<String>,
    #[serde(default)]
    pub runner_id: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub open_terminal: bool,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberShutdownManagedRequest {
    pub member_id: String,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberRestartManagedRequest {
    pub member_id: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub token_env: Option<String>,
    #[serde(default)]
    pub runner_id: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub open_terminal: bool,
    #[serde(default)]
    pub force_shutdown: bool,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberSessionStatusRequest {
    pub member_id: String,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberAttachRequest {
    pub member_id: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub caller_member_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedLaunchResult {
    pub member_id: String,
    pub adapter: AdapterKind,
    pub runner_id: Option<String>,
    pub command: Vec<String>,
    pub command_line: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_config_file: Option<PathBuf>,
    pub launched: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub session_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSessionSummary {
    pub member_id: String,
    pub adapter: AdapterKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub started_at: DateTime<Utc>,
    pub command_line: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_config_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    // Stored for auto-restart by supervisor
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_token_env: Option<String>,
    #[serde(default)]
    pub spawn_open_terminal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberSessionStatus {
    pub member_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<RunnerStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<ManagedSessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberAttachResult {
    pub member_id: String,
    pub mode: String,
    pub command: Vec<String>,
    pub command_line: String,
    pub raw_log_hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerEventRequest {
    pub member_id: String,
    #[serde(default)]
    pub runner_id: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub child_pid: Option<u32>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerInjectResult {
    pub member_id: String,
    pub message_id: String,
    pub injected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerLogLine {
    pub ts: DateTime<Utc>,
    pub stream: String,
    pub data: String,
}

#[derive(Debug)]
struct RunnerRuntime {
    status: RunnerStatus,
    tx: Option<mpsc::UnboundedSender<Value>>,
}

enum CodexMsg {
    Turn(String),
    Steer(String),
    Interrupt,
}

struct CodexRuntime {
    tx: std_mpsc::Sender<CodexMsg>,
}

impl std::fmt::Debug for CodexRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexRuntime").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerStatus {
    pub member_id: String,
    pub runner_id: Option<String>,
    pub pid: Option<u32>,
    pub child_pid: Option<u32>,
    pub state: String,
    pub online: bool,
    pub last_seen_at: DateTime<Utc>,
    pub last_output_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone)]
struct PreparedLaunch {
    member_id: String,
    adapter: AdapterKind,
    runner_id: Option<String>,
    command: Vec<String>,
    command_line: String,
    cwd: Option<PathBuf>,
    env: BTreeMap<String, String>,
    prompt_file: Option<PathBuf>,
    mcp_config_file: Option<PathBuf>,
    codex_event_log: Option<PathBuf>,
    codex_developer_instructions: Option<String>,
}

struct SpawnResult {
    pid: Option<u32>,
    codex_tx: Option<std_mpsc::Sender<CodexMsg>>,
}

#[derive(Debug, Clone)]
struct ChildLaunchCommand {
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

impl TeamModeHost {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let services = TeamModeServices::new(data_dir.clone())
            .expect("failed to initialize Team Mode services");
        let initial_state = rebuild_state_from_services(&services);
        Self {
            services,
            data_dir,
            state: Arc::new(Mutex::new(initial_state)),
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub async fn status(&self) -> HostStatus {
        let state = self.state.lock().await;
        let teams = self.services.teams.list().unwrap_or_default();
        let member_count = if teams.is_empty() {
            state.members.len()
        } else {
            teams
                .iter()
                .map(|team| {
                    self.services
                        .members
                        .list(&team.id)
                        .map(|members| members.len())
                        .unwrap_or(0)
                })
                .sum()
        };
        HostStatus {
            data_dir: self.data_dir.clone(),
            team_count: teams.len().max(state.teams.len()),
            member_count,
            message_count: self
                .services
                .messages
                .list_all()
                .map(|messages| messages.len())
                .unwrap_or(state.messages.len()),
            runner_count: state.runners.len(),
        }
    }

    pub async fn team_create(&self, req: TeamCreateRequest) -> Result<Team> {
        if req.id.trim().is_empty() {
            return Err(Error::Invalid("team id is required".into()));
        }
        let team = self.services.teams.create(CreateTeam {
            id: Some(req.id),
            name: req.name,
            description: req.description,
            lead_member_id: req.lead_member_id,
        })?;
        let mut state = self.state.lock().await;
        state.rooms.insert(
            room_key(&team.id, "main"),
            Room {
                id: "main".into(),
                team_id: team.id.clone(),
                kind: RoomKind::Main,
                status: RoomStatus::Active,
            },
        );
        info!(team_id = %team.id, name = %team.name, "team created");
        state.teams.insert(team.id.clone(), team.clone());
        Ok(team)
    }

    pub async fn team_get(&self, req: TeamGetRequest) -> Result<Team> {
        self.services.teams.get(&req.team_id)
    }

    pub async fn team_list(&self) -> Result<Vec<Team>> {
        self.services.teams.list()
    }

    pub async fn team_delete(&self, req: TeamDeleteRequest) -> Result<Value> {
        self.services.teams.delete(&req.team_id)?;
        info!(team_id = %req.team_id, "team deleted");
        self.refresh_projection_state().await;
        Ok(json!({ "deleted": true, "teamId": req.team_id }))
    }

    pub async fn member_add(&self, req: MemberAddRequest) -> Result<MemberRecord> {
        let state = self.state.lock().await;
        if state.members.contains_key(&req.id) {
            return Err(Error::Conflict(format!(
                "member already exists: {}",
                req.id
            )));
        }
        drop(state);
        let record = self.services.members.add(AddMember {
            id: Some(req.id),
            team_id: req.team_id,
            name: req.name,
            kind: req.kind.unwrap_or(MemberKind::Member),
            handle: req.handle,
            role_label: req.role_label.unwrap_or_else(|| "member".into()),
            role_description: req.role_description,
            execution: req.execution,
        })?;
        let mut state = self.state.lock().await;
        info!(
            member_id = %record.profile.id,
            team_id = %record.profile.team_id,
            handle = %record.profile.handle,
            "member added"
        );
        state
            .members
            .insert(record.profile.id.clone(), record.clone());
        Ok(record)
    }

    pub async fn member_list(&self, team_id: Option<String>) -> Vec<MemberRecord> {
        if let Some(team_id) = team_id.as_deref() {
            if let Ok(members) = self.services.members.list(team_id) {
                return members;
            }
        } else if let Ok(teams) = self.services.teams.list() {
            let mut members = Vec::new();
            for team in teams {
                if let Ok(team_members) = self.services.members.list(&team.id) {
                    members.extend(team_members);
                }
            }
            if !members.is_empty() {
                return members;
            }
        }
        let state = self.state.lock().await;
        state
            .members
            .values()
            .filter(|member| {
                team_id
                    .as_ref()
                    .map(|team_id| member.profile.team_id == *team_id)
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    pub async fn member_get(&self, req: MemberGetRequest) -> Result<MemberRecord> {
        self.services.members.get(&req.team_id, &req.member_id)
    }

    pub async fn member_update(&self, req: MemberUpdateRequest) -> Result<MemberRecord> {
        let role_description = if req.clear_role_description {
            Some(None)
        } else {
            req.role_description.map(Some)
        };
        let updated = self.services.members.update(
            &req.team_id,
            &req.member_id,
            UpdateMember {
                name: req.name,
                handle: req.handle,
                role_label: req.role_label,
                role_description,
                status: req.status,
            },
        )?;
        let mut state = self.state.lock().await;
        state
            .members
            .insert(updated.profile.id.clone(), updated.clone());
        Ok(updated)
    }

    pub async fn member_remove(&self, req: MemberRemoveRequest) -> Result<MemberRecord> {
        let removed = self.services.members.remove(&req.team_id, &req.member_id)?;
        let mut state = self.state.lock().await;
        info!(
            member_id = %removed.profile.id,
            team_id = %removed.profile.team_id,
            "member removed"
        );
        state.members.remove(&removed.profile.id);
        state.runners.remove(&removed.profile.id);
        state.sessions.remove(&removed.profile.id);
        state.codex_sessions.remove(&removed.profile.id);
        Ok(removed)
    }

    pub async fn execution_set(&self, req: ExecutionSetRequest) -> Result<MemberRecord> {
        let mut state = self.state.lock().await;
        let member = state
            .members
            .get_mut(&req.member_id)
            .ok_or_else(|| Error::NotFound(format!("member not found: {}", req.member_id)))?;
        let updated = self.services.members.set_execution_profile(
            &member.profile.team_id,
            &req.member_id,
            req.execution.clone(),
        )?;
        member.execution = Some(req.execution);
        *member = updated.clone();
        Ok(updated)
    }

    pub async fn room_post(&self, req: RoomPostRequest) -> Result<Message> {
        assert_authenticated_sender(req.caller_member_id.as_deref(), &req.sender_member_id)?;
        let message = self.services.messages.room_post(RoomPost {
            team_id: req.team_id,
            room_id: Some(req.room_id),
            sender_member_id: req.sender_member_id,
            kind: req.kind.unwrap_or(MessageKind::Discussion),
            subject: req.subject,
            body: req.body,
            explicit_mentions: Vec::new(),
        })?;
        self.record_message_delivery(message).await
    }

    pub async fn room_list(&self, req: RoomListRequest) -> Result<Vec<Room>> {
        self.services.rooms.list(&req.team_id)
    }

    pub async fn room_read_messages(&self, req: RoomReadMessagesRequest) -> Result<Vec<Message>> {
        self.services.rooms.get(&req.team_id, &req.room_id)?;
        let mut messages: Vec<Message> = self
            .services
            .messages
            .list_all()?
            .into_iter()
            .filter(|message| message.team_id == req.team_id && message.room_id == req.room_id)
            .collect();
        messages.sort_by_key(|message| message.created_at);
        if let Some(limit) = req.limit {
            if messages.len() > limit {
                messages = messages.split_off(messages.len() - limit);
            }
        }
        Ok(messages)
    }

    pub async fn thread_reply(&self, req: ThreadReplyRequest) -> Result<Message> {
        assert_authenticated_sender(req.caller_member_id.as_deref(), &req.sender_member_id)?;
        let state = self.state.lock().await;
        let thread = state
            .threads
            .get(&req.thread_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("thread not found: {}", req.thread_id)))?;
        drop(state);
        let message = self.services.threads.reply(
            &thread.team_id,
            &req.thread_id,
            &req.sender_member_id,
            req.body,
        )?;
        self.record_message_delivery(message).await
    }

    pub async fn thread_read(&self, req: ThreadReadRequest) -> Result<ThreadReadResult> {
        let team_id = match req.team_id {
            Some(team_id) => team_id,
            None => {
                let state = self.state.lock().await;
                state
                    .threads
                    .get(&req.thread_id)
                    .map(|thread| thread.team_id.clone())
                    .ok_or_else(|| {
                        Error::NotFound(format!("thread not found: {}", req.thread_id))
                    })?
            }
        };
        let thread = self.services.threads.read(&team_id, &req.thread_id)?;
        let messages = self
            .services
            .threads
            .read_messages(&team_id, &req.thread_id)?;
        Ok(ThreadReadResult { thread, messages })
    }

    pub async fn direct_send(&self, req: DirectSendRequest) -> Result<Message> {
        assert_authenticated_sender(req.caller_member_id.as_deref(), &req.sender_member_id)?;
        let message = self.services.direct.direct_send(
            &req.team_id,
            &req.sender_member_id,
            &req.recipient_member_id,
            req.body,
        )?;
        self.record_message_delivery(message).await
    }

    pub async fn direct_reply(&self, req: DirectReplyRequest) -> Result<Message> {
        assert_authenticated_sender(req.caller_member_id.as_deref(), &req.sender_member_id)?;
        let thread = {
            let state = self.state.lock().await;
            let thread =
                state.threads.get(&req.thread_id).cloned().ok_or_else(|| {
                    Error::NotFound(format!("thread not found: {}", req.thread_id))
                })?;
            let root = state.messages.get(&thread.root_message_id).ok_or_else(|| {
                Error::NotFound(format!("message not found: {}", thread.root_message_id))
            })?;
            if root.kind != MessageKind::Direct {
                return Err(Error::Invalid(
                    "direct_reply requires a direct thread".into(),
                ));
            }
            thread
        };
        let message = self.services.direct.direct_reply(
            &thread.team_id,
            &req.thread_id,
            &req.sender_member_id,
            req.body,
        )?;
        self.record_message_delivery(message).await
    }

    pub async fn direct_read(&self, req: DirectReadRequest) -> Result<Vec<Message>> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "direct_read",
        )?;
        let messages =
            self.services
                .direct
                .direct_read(&req.team_id, &req.thread_id, &req.member_id)?;
        self.refresh_projection_state().await;
        Ok(messages)
    }

    pub async fn direct_list(
        &self,
        req: DirectListRequest,
    ) -> Result<Vec<crate::service::direct_service::DirectThreadSummary>> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "direct_list",
        )?;
        self.services
            .direct
            .direct_list(&req.team_id, &req.member_id)
    }

    pub async fn inbox_peek(&self, req: InboxPeekRequest) -> Result<Vec<InboxItem>> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "inbox_peek",
        )?;
        let state = self.state.lock().await;
        if !state.members.contains_key(&req.member_id) {
            return Err(Error::NotFound(format!(
                "member not found: {}",
                req.member_id
            )));
        }
        let limit = req.limit.unwrap_or(20);
        Ok(state
            .inbox
            .get(&req.member_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .take(limit)
            .collect())
    }

    pub async fn inbox_read(&self, req: InboxReadRequest) -> Result<Vec<Message>> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "inbox_read",
        )?;
        let team_id = self.member_team_id(&req.member_id).await?;
        let messages = self
            .services
            .inbox
            .read(&team_id, &req.member_id, req.limit)?;
        self.refresh_projection_state().await;
        Ok(messages)
    }

    pub async fn inbox_ack(&self, req: InboxAckRequest) -> Result<Message> {
        assert_member_scope(req.caller_member_id.as_deref(), &req.member_id, "inbox_ack")?;
        let team_id = self.member_team_id(&req.member_id).await?;
        let message = self
            .services
            .inbox
            .ack(&team_id, &req.member_id, &req.message_id)?;
        self.refresh_projection_state().await;
        Ok(message)
    }

    pub async fn inbox_count(&self, req: InboxCountRequest) -> Result<crate::domain::InboxCounts> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "inbox_count",
        )?;
        let team_id = self.member_team_id(&req.member_id).await?;
        self.services.inbox.count(&team_id, &req.member_id)
    }

    pub async fn member_tail(&self, req: MemberTailRequest) -> Result<Vec<RunnerLogLine>> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "member_tail",
        )?;
        let state = self.state.lock().await;
        if !state.members.contains_key(&req.member_id) {
            return Err(Error::NotFound(format!(
                "member not found: {}",
                req.member_id
            )));
        }
        let limit = req.limit.unwrap_or(100);
        Ok(state
            .raw_logs
            .get(&req.member_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect())
    }

    pub async fn member_spawn_managed(
        &self,
        req: MemberSpawnManagedRequest,
    ) -> Result<ManagedLaunchResult> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "member_spawn_managed",
        )?;
        info!(member_id = %req.member_id, dry_run = req.dry_run, "spawn managed session");
        let member = self.member_record_by_id(&req.member_id).await?;
        let execution = member
            .execution
            .clone()
            .unwrap_or_else(|| default_execution(req.member_id.clone()));
        let launch = self.prepare_managed_launch(&member, &execution, &req)?;
        let mut launched = false;
        let mut pid = None;
        let mut codex_tx = None;
        let mut note = None;

        if !req.dry_run {
            match spawn_launch_command(&launch, req.open_terminal) {
                Ok(spawned) => {
                    launched = true;
                    pid = spawned.pid;
                    codex_tx = spawned.codex_tx;
                    debug!(
                        member_id = %req.member_id,
                        pid = ?pid,
                        command_line = %launch.command_line,
                        "managed process spawned"
                    );
                }
                Err(err) => {
                    warn!(
                        member_id = %req.member_id,
                        error = %err,
                        command_line = %launch.command_line,
                        "failed to spawn managed process"
                    );
                    note = Some(err.to_string());
                }
            }
        }

        let session_state = if req.dry_run {
            "planned"
        } else if launched {
            "starting"
        } else {
            "failed"
        }
        .to_string();
        let summary = ManagedSessionSummary {
            member_id: req.member_id.clone(),
            adapter: execution.adapter.clone(),
            runner_id: launch.runner_id.clone(),
            state: session_state.clone(),
            pid,
            started_at: Utc::now(),
            command_line: launch.command_line.clone(),
            prompt_file: launch.prompt_file.clone(),
            mcp_config_file: launch.mcp_config_file.clone(),
            last_error: note.clone(),
            spawn_host: req.host.clone(),
            spawn_token_env: req.token_env.clone(),
            spawn_open_terminal: req.open_terminal,
        };
        let mut state = self.state.lock().await;
        state.sessions.insert(req.member_id.clone(), summary);
        if let Some(tx) = codex_tx {
            state
                .codex_sessions
                .insert(req.member_id.clone(), CodexRuntime { tx });
        }

        Ok(ManagedLaunchResult {
            member_id: req.member_id,
            adapter: execution.adapter,
            runner_id: launch.runner_id,
            command: launch.command,
            command_line: launch.command_line,
            cwd: launch.cwd,
            env: launch.env,
            prompt_file: launch.prompt_file,
            mcp_config_file: launch.mcp_config_file,
            launched,
            pid,
            session_state,
            note,
        })
    }

    pub async fn member_shutdown_managed(
        &self,
        req: MemberShutdownManagedRequest,
    ) -> Result<Value> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "member_shutdown_managed",
        )?;
        info!(member_id = %req.member_id, force = req.force, "shutdown managed session");
        let (tx, pid) = {
            let mut state = self.state.lock().await;
            let tx = state
                .runners
                .get(&req.member_id)
                .and_then(|runtime| runtime.status.online.then(|| runtime.tx.clone()).flatten());
            let pid = state.sessions.get(&req.member_id).and_then(|s| s.pid);
            if let Some(session) = state.sessions.get_mut(&req.member_id) {
                session.state = if req.force {
                    "stopped".into()
                } else {
                    "stopping".into()
                };
            }
            if req.force {
                state.codex_sessions.remove(&req.member_id);
            }
            (tx, pid)
        };

        let mut injected_ctrl_c = false;
        if let Some(tx) = tx {
            injected_ctrl_c = tx
                .send(json!({
                    "type": "host/inject_input",
                    "member_id": req.member_id.clone(),
                    "message_id": format!("shutdown_{}", Uuid::new_v4().simple()),
                    "text": "",
                    "strategy": "ctrl_c"
                }))
                .is_ok();
        }
        let terminated = if req.force {
            match pid {
                Some(pid) => terminate_process(pid).map(|()| true)?,
                None => false,
            }
        } else {
            false
        };
        Ok(json!({
            "memberId": req.member_id,
            "injectedCtrlC": injected_ctrl_c,
            "terminated": terminated
        }))
    }

    pub async fn member_restart_managed(
        &self,
        req: MemberRestartManagedRequest,
    ) -> Result<ManagedLaunchResult> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "member_restart_managed",
        )?;
        let _ = self
            .member_shutdown_managed(MemberShutdownManagedRequest {
                member_id: req.member_id.clone(),
                force: req.force_shutdown,
                caller_member_id: req.caller_member_id.clone(),
            })
            .await?;
        self.member_spawn_managed(MemberSpawnManagedRequest {
            member_id: req.member_id,
            host: req.host,
            token_env: req.token_env,
            runner_id: req.runner_id,
            dry_run: req.dry_run,
            open_terminal: req.open_terminal,
            caller_member_id: req.caller_member_id,
        })
        .await
    }

    pub async fn member_session_status(
        &self,
        req: MemberSessionStatusRequest,
    ) -> Result<MemberSessionStatus> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "member_session_status",
        )?;
        let member = self.member_record_by_id(&req.member_id).await?;
        let state = self.state.lock().await;
        Ok(MemberSessionStatus {
            member_id: req.member_id.clone(),
            execution: member.execution,
            runner: state
                .runners
                .get(&req.member_id)
                .map(|runtime| runtime.status.clone()),
            session: state.sessions.get(&req.member_id).cloned(),
        })
    }

    pub async fn member_attach(&self, req: MemberAttachRequest) -> Result<MemberAttachResult> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "member_attach",
        )?;
        let member = self.member_record_by_id(&req.member_id).await?;
        let execution = member
            .execution
            .clone()
            .unwrap_or_else(|| default_execution(req.member_id.clone()));
        let host = req.host.unwrap_or_else(|| "127.0.0.1:17891".into());
        let (mode, command) = match execution.viewer_mode {
            ViewerMode::EventViewer => (
                "codex_event_viewer".to_string(),
                vec![
                    sibling_binary("codex_viewer").display().to_string(),
                    "--data-dir".into(),
                    self.data_dir.display().to_string(),
                    "--member-id".into(),
                    req.member_id.clone(),
                    "--follow".into(),
                ],
            ),
            ViewerMode::NativeTerminal => (
                "member_raw_tail".to_string(),
                vec![
                    sibling_binary("teamctl").display().to_string(),
                    "--host".into(),
                    host,
                    "member".into(),
                    "tail".into(),
                    req.member_id.clone(),
                    "--limit".into(),
                    "200".into(),
                ],
            ),
        };
        Ok(MemberAttachResult {
            member_id: req.member_id,
            mode,
            command_line: command_line(&command),
            command,
            raw_log_hint: format!(
                "raw output is available from Host memory via member/tail for {}",
                member.profile.id
            ),
            note: Some(
                "v1 attach returns the viewer/tail command; terminal focus is launcher-dependent"
                    .into(),
            ),
        })
    }

    pub async fn runner_event(&self, event_type: &str, req: RunnerEventRequest) -> Result<Value> {
        match event_type {
            "runner/hello" => self.runner_hello(req, None).await,
            "runner/heartbeat" => self.runner_heartbeat(req).await,
            "runner/output" => self.runner_output(req).await,
            "runner/input_injected" => self.runner_input_injected(req).await,
            "runner/child_exit" => self.runner_child_exit(req).await,
            _ => Err(Error::Invalid(format!(
                "unsupported runner event: {event_type}"
            ))),
        }
    }

    pub async fn runner_hello(
        &self,
        req: RunnerEventRequest,
        tx: Option<mpsc::UnboundedSender<Value>>,
    ) -> Result<Value> {
        let now = Utc::now();
        info!(
            member_id = %req.member_id,
            runner_id = ?req.runner_id,
            pid = ?req.pid,
            "runner registered"
        );
        let status = RunnerStatus {
            member_id: req.member_id.clone(),
            runner_id: req.runner_id,
            pid: req.pid,
            child_pid: req.child_pid,
            state: req.state.unwrap_or_else(|| "running".into()),
            online: true,
            last_seen_at: now,
            last_output_at: None,
            exit_code: None,
        };
        let mut state = self.state.lock().await;
        state.runners.insert(
            req.member_id,
            RunnerRuntime {
                status: status.clone(),
                tx,
            },
        );
        Ok(json!({ "runner": status }))
    }

    pub async fn runner_heartbeat(&self, req: RunnerEventRequest) -> Result<Value> {
        let mut state = self.state.lock().await;
        let runtime = state
            .runners
            .get_mut(&req.member_id)
            .ok_or_else(|| Error::NotFound(format!("runner not found: {}", req.member_id)))?;
        runtime.status.online = true;
        runtime.status.last_seen_at = Utc::now();
        runtime.status.child_pid = req.child_pid.or(runtime.status.child_pid);
        runtime.status.state = req.state.unwrap_or_else(|| runtime.status.state.clone());
        Ok(json!({ "runner": runtime.status }))
    }

    pub async fn runner_output(&self, req: RunnerEventRequest) -> Result<Value> {
        let line = RunnerLogLine {
            ts: Utc::now(),
            stream: req.stream.unwrap_or_else(|| "pty".into()),
            data: req.data.unwrap_or_default(),
        };
        let mut state = self.state.lock().await;
        let log = state.raw_logs.entry(req.member_id.clone()).or_default();
        log.push_back(line);
        while log.len() > 1000 {
            log.pop_front();
        }
        if let Some(runtime) = state.runners.get_mut(&req.member_id) {
            runtime.status.last_seen_at = Utc::now();
            runtime.status.last_output_at = Some(Utc::now());
        }
        Ok(json!({ "recorded": true }))
    }

    pub async fn runner_input_injected(&self, req: RunnerEventRequest) -> Result<Value> {
        Ok(json!({
            "memberId": req.member_id,
            "messageId": req.message_id,
            "ok": req.ok.unwrap_or(true)
        }))
    }

    pub async fn runner_child_exit(&self, req: RunnerEventRequest) -> Result<Value> {
        let mut state = self.state.lock().await;
        let runtime = state
            .runners
            .get_mut(&req.member_id)
            .ok_or_else(|| Error::NotFound(format!("runner not found: {}", req.member_id)))?;
        runtime.status.online = false;
        runtime.status.state = "stopped".into();
        runtime.status.exit_code = req.exit_code;
        runtime.status.last_seen_at = Utc::now();
        runtime.tx = None;
        Ok(json!({ "runner": runtime.status }))
    }

    pub async fn runner_disconnected(&self, member_id: &str) {
        info!(member_id = %member_id, "runner disconnected");
        let mut state = self.state.lock().await;
        if let Some(runtime) = state.runners.get_mut(member_id) {
            runtime.status.online = false;
            runtime.status.state = "disconnected".into();
            runtime.status.last_seen_at = Utc::now();
            runtime.tx = None;
        }
    }

    pub async fn codex_steer(&self, req: CodexSteerRequest) -> Result<Value> {
        let state = self.state.lock().await;
        let runtime = state
            .codex_sessions
            .get(&req.member_id)
            .ok_or_else(|| Error::NotFound(format!("no codex session: {}", req.member_id)))?;
        runtime
            .tx
            .send(CodexMsg::Steer(req.text))
            .map_err(|_| Error::Other("codex session channel closed".into()))?;
        Ok(json!({ "memberId": req.member_id, "sent": true }))
    }

    pub async fn codex_interrupt(&self, req: CodexInterruptRequest) -> Result<Value> {
        let state = self.state.lock().await;
        let runtime = state
            .codex_sessions
            .get(&req.member_id)
            .ok_or_else(|| Error::NotFound(format!("no codex session: {}", req.member_id)))?;
        runtime
            .tx
            .send(CodexMsg::Interrupt)
            .map_err(|_| Error::Other("codex session channel closed".into()))?;
        Ok(json!({ "memberId": req.member_id, "sent": true }))
    }

    pub fn start_heartbeat_supervisor(&self) -> tokio::task::JoinHandle<()> {
        let host = self.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(std::time::Duration::from_secs(5));
            interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                host.check_runner_heartbeats().await;
            }
        })
    }

    async fn check_runner_heartbeats(&self) {
        let threshold = ChronoDuration::seconds(10);
        let now = Utc::now();

        let degraded: Vec<String> = {
            let mut state = self.state.lock().await;
            let mut degraded = Vec::new();
            for (member_id, runtime) in state.runners.iter_mut() {
                if !runtime.status.online {
                    continue;
                }
                if now.signed_duration_since(runtime.status.last_seen_at) > threshold {
                    warn!(member_id = %member_id, "runner heartbeat timeout, marking degraded");
                    runtime.status.online = false;
                    runtime.status.state = "degraded".into();
                    degraded.push(member_id.clone());
                }
            }
            degraded
        };

        for member_id in degraded {
            let spawn_params = {
                let state = self.state.lock().await;
                state.sessions.get(&member_id).map(|s| MemberSpawnManagedRequest {
                    member_id: member_id.clone(),
                    host: s.spawn_host.clone(),
                    token_env: s.spawn_token_env.clone(),
                    runner_id: Some(format!("restart_{}", Uuid::new_v4().simple())),
                    dry_run: false,
                    open_terminal: s.spawn_open_terminal,
                    caller_member_id: None,
                })
            };
            if let Some(req) = spawn_params {
                let needs_restart = self
                    .member_record_by_id(&member_id)
                    .await
                    .ok()
                    .and_then(|m| m.execution)
                    .map(|e| matches!(e.restart_policy, RestartPolicy::Always))
                    .unwrap_or(false);
                if needs_restart {
                    info!(member_id = %member_id, "auto-restart triggered by supervisor");
                    let _ = self.member_spawn_managed(req).await;
                }
            }
        }
    }

    pub async fn runner_inject(&self, req: RunnerInjectRequest) -> Result<RunnerInjectResult> {
        assert_member_scope(
            req.caller_member_id.as_deref(),
            &req.member_id,
            "runner_inject",
        )?;
        let message_id = req
            .message_id
            .clone()
            .unwrap_or_else(|| format!("inj_{}", Uuid::new_v4().simple()));
        let tx = {
            let mut state = self.state.lock().await;
            let log = state.raw_logs.entry(req.member_id.clone()).or_default();
            log.push_back(RunnerLogLine {
                ts: Utc::now(),
                stream: "host/inject_input".into(),
                data: req.text.clone(),
            });
            state
                .runners
                .get(&req.member_id)
                .and_then(|runtime| runtime.status.online.then(|| runtime.tx.clone()).flatten())
        };
        let inject_member_id = req.member_id.clone();
        let inject_text = req.text.clone();
        let inject_strategy = req.strategy.clone();
        let injected = tx
            .map(|tx| {
                let mut frame = json!({
                    "type": "host/inject_input",
                    "member_id": inject_member_id,
                    "message_id": message_id.clone(),
                    "text": inject_text
                });
                if let (Some(strategy), Some(map)) = (inject_strategy, frame.as_object_mut()) {
                    map.insert("strategy".into(), Value::String(strategy));
                }
                tx.send(frame).is_ok()
            })
            .unwrap_or(false);
        Ok(RunnerInjectResult {
            member_id: req.member_id,
            message_id,
            injected,
        })
    }

    async fn member_team_id(&self, member_id: &str) -> Result<String> {
        let state = self.state.lock().await;
        state
            .members
            .get(member_id)
            .map(|member| member.profile.team_id.clone())
            .ok_or_else(|| Error::NotFound(format!("member not found: {member_id}")))
    }

    async fn member_record_by_id(&self, member_id: &str) -> Result<MemberRecord> {
        let state = self.state.lock().await;
        state
            .members
            .get(member_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("member not found: {member_id}")))
    }

    async fn refresh_projection_state(&self) {
        let fresh = rebuild_state_from_services(&self.services);
        let mut state = self.state.lock().await;
        state.teams = fresh.teams;
        state.rooms = fresh.rooms;
        state.members = fresh.members;
        state.threads = fresh.threads;
        state.messages = fresh.messages;
        state.inbox = fresh.inbox;
    }

    fn prepare_managed_launch(
        &self,
        member: &MemberRecord,
        execution: &ExecutionProfile,
        req: &MemberSpawnManagedRequest,
    ) -> Result<PreparedLaunch> {
        let host = req.host.clone().unwrap_or_else(|| "127.0.0.1:17891".into());
        let token_env = req
            .token_env
            .clone()
            .unwrap_or_else(|| "TEAM_MODE_RUNNER_TOKEN".into());
        let runner_id = req
            .runner_id
            .clone()
            .unwrap_or_else(|| format!("run_{}", Uuid::new_v4().simple()));
        let runtime_dir = self.data_dir.join("runtime");
        let prompt_dir = runtime_dir.join("prompts");
        let mcp_dir = runtime_dir.join("mcp");
        fs::create_dir_all(&prompt_dir).map_err(|source| Error::io(&prompt_dir, source))?;
        fs::create_dir_all(&mcp_dir).map_err(|source| Error::io(&mcp_dir, source))?;

        let prompt_file = prompt_dir.join(format!("{}.system.md", member.profile.id));
        fs::write(&prompt_file, &execution.system_prompt)
            .map_err(|source| Error::io(&prompt_file, source))?;

        let mcp_config_file = mcp_dir.join(format!("{}.mcp.json", member.profile.id));
        let mcp_config = json!({
            "mcpServers": {
                "team-mode": {
                    "command": sibling_binary("team_mode_mcp_proxy").display().to_string(),
                    "args": [
                        "--host",
                        host.clone(),
                        "--member-id",
                        member.profile.id.clone(),
                        "--token-env",
                        token_env.clone()
                    ]
                }
            }
        });
        let mcp_config_text = serde_json::to_string_pretty(&mcp_config)
            .map_err(|source| Error::json("<mcp config>", source))?;
        fs::write(&mcp_config_file, mcp_config_text)
            .map_err(|source| Error::io(&mcp_config_file, source))?;

        let child = child_command_for_execution(execution, &prompt_file, &mcp_config_file);
        match &execution.adapter {
            AdapterKind::ClaudeCodeTerminal | AdapterKind::GeminiCliTerminal => {
                let runner_program = sibling_binary("team_member_runner");
                let mut command = vec![runner_program.display().to_string()];
                command.extend([
                    "--member-id".to_string(),
                    member.profile.id.clone(),
                    "--runner-id".to_string(),
                    runner_id.clone(),
                    "--host".to_string(),
                    req.host.clone().unwrap_or_else(|| "127.0.0.1:17891".into()),
                    "--token-env".to_string(),
                    req.token_env
                        .clone()
                        .unwrap_or_else(|| "TEAM_MODE_RUNNER_TOKEN".into()),
                ]);
                if let Some(cwd) = execution.cwd.as_ref() {
                    command.extend(["--cwd".into(), cwd.display().to_string()]);
                }
                for (key, value) in &child.env {
                    command.extend(["--env".into(), format!("{key}={value}")]);
                }
                command.push("--".into());
                command.push(child.program.clone());
                command.extend(child.args.clone());

                Ok(PreparedLaunch {
                    member_id: member.profile.id.clone(),
                    adapter: execution.adapter.clone(),
                    runner_id: Some(runner_id),
                    command_line: command_line(&command),
                    command,
                    cwd: execution.cwd.clone(),
                    env: child.env,
                    prompt_file: Some(prompt_file),
                    mcp_config_file: Some(mcp_config_file),
                    codex_event_log: None,
                    codex_developer_instructions: None,
                })
            }
            AdapterKind::CodexAppServer => {
                let mut command = vec![child.program.clone()];
                command.extend(child.args.clone());
                let codex_dir = self.data_dir.join("members").join(&member.profile.id);
                fs::create_dir_all(&codex_dir).map_err(|source| Error::io(&codex_dir, source))?;
                let codex_event_log = codex_dir.join("codex-events.ndjson");
                append_codex_event(
                    &codex_event_log,
                    json!({
                        "event": "managed_session_launch",
                        "memberId": member.profile.id.clone(),
                        "command": command.clone(),
                        "promptFile": prompt_file.clone(),
                        "mcpConfigFile": mcp_config_file.clone(),
                        "ts": Utc::now()
                    }),
                )?;
                Ok(PreparedLaunch {
                    member_id: member.profile.id.clone(),
                    adapter: execution.adapter.clone(),
                    runner_id: None,
                    command_line: command_line(&command),
                    command,
                    cwd: execution.cwd.clone(),
                    env: child.env,
                    prompt_file: Some(prompt_file),
                    mcp_config_file: Some(mcp_config_file),
                    codex_event_log: Some(codex_event_log),
                    codex_developer_instructions: Some(execution.system_prompt.clone()),
                })
            }
        }
    }

    async fn record_message_delivery(&self, message: Message) -> Result<Message> {
        info!(
            message_id = %message.id,
            sender = %message.sender_member_id,
            kind = %message_kind_str(&message.kind),
            recipients = ?message.delivered_to,
            "message posted"
        );
        let mut state = self.state.lock().await;
        state.messages.insert(message.id.clone(), message.clone());
        let now = message.created_at;
        let thread = state
            .threads
            .entry(message.thread_id.clone())
            .or_insert_with(|| Thread {
                id: message.thread_id.clone(),
                team_id: message.team_id.clone(),
                room_id: message.room_id.clone(),
                root_message_id: message.id.clone(),
                subject: message.subject.clone(),
                message_ids: Vec::new(),
                status: ThreadStatus::Open,
                created_at: now,
                updated_at: now,
            });
        if !thread.message_ids.iter().any(|id| id == &message.id) {
            thread.message_ids.push(message.id.clone());
        }
        thread.updated_at = now;

        let inject_text = format_injected_message(&message);
        for recipient in &message.delivered_to {
            state
                .inbox
                .entry(recipient.clone())
                .or_default()
                .push(InboxItem {
                    message_id: message.id.clone(),
                    team_id: message.team_id.clone(),
                    room_id: message.room_id.clone(),
                    thread_id: message.thread_id.clone(),
                    sender_member_id: message.sender_member_id.clone(),
                    unread: true,
                    unacked: true,
                    delivered_at: now,
                });
            state
                .raw_logs
                .entry(recipient.clone())
                .or_default()
                .push_back(RunnerLogLine {
                    ts: now,
                    stream: "host/deliver".into(),
                    data: inject_text.clone(),
                });
            if let Some(tx) = state
                .runners
                .get(recipient)
                .and_then(|runtime| runtime.status.online.then(|| runtime.tx.clone()).flatten())
            {
                let _ = tx.send(json!({
                    "type": "host/inject_input",
                    "member_id": recipient,
                    "message_id": message.id.clone(),
                    "text": inject_text.clone()
                }));
            }
            if let Some(runtime) = state.codex_sessions.get(recipient) {
                let _ = runtime.tx.send(CodexMsg::Turn(inject_text.clone()));
            }
        }
        Ok(message)
    }
}

fn child_command_for_execution(
    execution: &ExecutionProfile,
    prompt_file: &Path,
    mcp_config_file: &Path,
) -> ChildLaunchCommand {
    let mut env = execution.env.clone();
    match &execution.adapter {
        AdapterKind::ClaudeCodeTerminal => {
            let mut args = Vec::new();
            match execution.prompt_mode {
                PromptMode::Replace => args.push("--system-prompt-file".to_string()),
                _ => args.push("--append-system-prompt-file".to_string()),
            }
            args.push(prompt_file.display().to_string());
            args.push("--mcp-config".to_string());
            args.push(mcp_config_file.display().to_string());
            args.extend(execution.command.args.clone());
            ChildLaunchCommand {
                program: non_empty_or(&execution.command.program, "claude"),
                args,
                env,
            }
        }
        AdapterKind::GeminiCliTerminal => {
            env.insert(
                "GEMINI_SYSTEM_MD".to_string(),
                prompt_file.display().to_string(),
            );
            ChildLaunchCommand {
                program: non_empty_or(&execution.command.program, "gemini"),
                args: execution.command.args.clone(),
                env,
            }
        }
        AdapterKind::CodexAppServer => {
            let mut args = execution.command.args.clone();
            if args.first().map(String::as_str) != Some("app-server") {
                args.insert(0, "app-server".to_string());
            }
            if let Some(model) = &execution.model {
                if !args.iter().any(|arg| arg.starts_with("model=")) {
                    args.extend(["-c".to_string(), format!("model=\"{model}\"")]);
                }
            }
            if let Some(reasoning) = &execution.reasoning_effort {
                if !args
                    .iter()
                    .any(|arg| arg.starts_with("model_reasoning_effort="))
                {
                    args.extend([
                        "-c".to_string(),
                        format!("model_reasoning_effort=\"{reasoning}\""),
                    ]);
                }
            }
            env.insert(
                "TEAM_MODE_SYSTEM_PROMPT_FILE".to_string(),
                prompt_file.display().to_string(),
            );
            ChildLaunchCommand {
                program: non_empty_or(&execution.command.program, "codex"),
                args,
                env,
            }
        }
    }
}

fn spawn_launch_command(launch: &PreparedLaunch, open_terminal: bool) -> Result<SpawnResult> {
    if open_terminal
        && matches!(
            launch.adapter,
            AdapterKind::ClaudeCodeTerminal | AdapterKind::GeminiCliTerminal
        )
    {
        let title = format!("tm:{}", launch.member_id);
        return spawn_terminal_command(&title, &launch.command);
    }
    spawn_direct_command(launch)
}

fn spawn_direct_command(launch: &PreparedLaunch) -> Result<SpawnResult> {
    let Some((program, args)) = launch.command.split_first() else {
        return Err(Error::Invalid("managed launch command is empty".into()));
    };
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = &launch.cwd {
        command.current_dir(cwd);
    }
    command.envs(&launch.env);
    if launch.codex_event_log.is_some() {
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
    }
    let mut child = command
        .spawn()
        .map_err(|source| Error::io(program.clone(), source))?;
    if let Some(path) = &launch.codex_event_log {
        let codex_thread_id = Arc::new(StdMutex::new(None::<String>));
        let developer_instructions = launch.codex_developer_instructions.clone();
        let (probe_tx, probe_rx): (
            Option<std_mpsc::SyncSender<bool>>,
            Option<std_mpsc::Receiver<bool>>,
        ) = if developer_instructions.is_some() {
            let (tx, rx) = std_mpsc::sync_channel::<bool>(1);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        if let Some(stdout) = child.stdout.take() {
            spawn_codex_pipe_logger(
                path.clone(),
                "stdout",
                stdout,
                Some(codex_thread_id.clone()),
                probe_tx,
            );
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_codex_pipe_logger(path.clone(), "stderr", stderr, None, None);
        }
        let mut codex_tx = None;
        if let Some(mut stdin) = child.stdin.take() {
            let (tx, rx) = std_mpsc::channel::<CodexMsg>();
            codex_tx = Some(tx);
            let path = path.clone();
            let cwd = launch.cwd.clone();
            thread::spawn(move || {
                let _ = write_codex_initialize(&mut stdin, cwd.as_ref(), developer_instructions.clone());
                let _ = append_codex_event(
                    &path,
                    json!({ "event": "initialize_sent", "ts": Utc::now() }),
                );
                let mut next_id = 3_u64;
                if let Some(rx) = probe_rx {
                    let probe_ok = rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap_or(false);
                    if probe_ok {
                        tracing::info!("codex probe_success: collaborationMode accepted");
                        let _ = append_codex_event(
                            &path,
                            json!({ "event": "probe_success", "ts": Utc::now() }),
                        );
                    } else {
                        tracing::warn!("codex probe_fallback: collaborationMode rejected or timeout, falling back to thread/start");
                        let _ = append_codex_event(
                            &path,
                            json!({
                                "event": "probe_fallback",
                                "reason": "collaborationMode_rejected_or_timeout",
                                "ts": Utc::now()
                            }),
                        );
                        let mut retry_params = serde_json::Map::new();
                        if let Some(ref c) = cwd {
                            retry_params.insert("cwd".into(), json!(c.display().to_string()));
                        }
                        let retry = json!({ "id": next_id, "method": "thread/start", "params": Value::Object(retry_params) });
                        next_id += 1;
                        let _ = writeln!(stdin, "{retry}");
                        let _ = stdin.flush();
                        let bootstrap_tid = 'wait: {
                            for _ in 0..30 {
                                std::thread::sleep(std::time::Duration::from_millis(100));
                                let id = codex_thread_id.lock().ok().and_then(|g| g.clone());
                                if id.is_some() {
                                    break 'wait id;
                                }
                            }
                            None
                        };
                        if let (Some(instructions), Some(tid)) =
                            (developer_instructions.as_ref(), bootstrap_tid)
                        {
                            let bootstrap = json!({
                                "id": next_id,
                                "method": "turn/start",
                                "params": {
                                    "threadId": &tid,
                                    "input": [{ "type": "text", "text": instructions }]
                                }
                            });
                            next_id += 1;
                            let _ = writeln!(stdin, "{bootstrap}");
                            let _ = stdin.flush();
                            let _ = append_codex_event(
                                &path,
                                json!({
                                    "event": "bootstrap_turn_sent",
                                    "threadId": &tid,
                                    "ts": Utc::now()
                                }),
                            );
                        }
                    }
                }
                for msg in rx {
                    let thread_id = codex_thread_id.lock().ok().and_then(|guard| guard.clone());
                    let (method, text_opt): (&str, Option<String>) = match msg {
                        CodexMsg::Interrupt => ("turn/interrupt", None),
                        CodexMsg::Turn(t) => ("turn/start", Some(t)),
                        CodexMsg::Steer(t) => ("turn/steer", Some(t)),
                    };
                    let Some(thread_id) = thread_id else {
                        if let Some(text) = text_opt {
                            let _ = append_codex_event(
                                &path,
                                json!({
                                    "event": "turn_start_deferred",
                                    "reason": "thread_id_not_observed_yet",
                                    "text": text,
                                    "ts": Utc::now()
                                }),
                            );
                        }
                        continue;
                    };
                    let params = if let Some(ref text) = text_opt {
                        json!({ "threadId": &thread_id, "input": [{ "type": "text", "text": text }] })
                    } else {
                        json!({ "threadId": &thread_id })
                    };
                    let request = json!({ "id": next_id, "method": method, "params": params });
                    next_id += 1;
                    let _ = writeln!(stdin, "{request}");
                    let _ = stdin.flush();
                    let event_name = method.replace('/', "_");
                    let log_event = if let Some(text) = text_opt {
                        json!({ "event": format!("{event_name}_sent"), "threadId": &thread_id, "text": text, "ts": Utc::now() })
                    } else {
                        json!({ "event": format!("{event_name}_sent"), "threadId": &thread_id, "ts": Utc::now() })
                    };
                    let _ = append_codex_event(&path, log_event);
                }
            });
        }
        append_codex_event(
            path,
            json!({
                "event": "process_started",
                "pid": child.id(),
                "ts": Utc::now()
            }),
        )?;
        return Ok(SpawnResult {
            pid: Some(child.id()),
            codex_tx,
        });
    }
    Ok(SpawnResult {
        pid: Some(child.id()),
        codex_tx: None,
    })
}

fn spawn_codex_pipe_logger(
    path: PathBuf,
    stream: &'static str,
    reader: impl Read + Send + 'static,
    thread_id: Option<Arc<StdMutex<Option<String>>>>,
    probe_tx: Option<std_mpsc::SyncSender<bool>>,
) {
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        let mut probe_done = false;
        for line in reader.lines().map_while(std::result::Result::ok) {
            if let (Some(thread_id), Some(observed)) =
                (thread_id.as_ref(), codex_thread_id_from_line(&line))
            {
                if let Ok(mut guard) = thread_id.lock() {
                    *guard = Some(observed);
                }
            }
            if !probe_done {
                if let Some(ref tx) = probe_tx {
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        let is_thread_response = v.get("id").map_or(false, |id| {
                            id.as_u64() == Some(2) || id.as_str() == Some("2")
                        });
                        if is_thread_response {
                            probe_done = true;
                            let success = v.pointer("/result/threadId").is_some()
                                || v.pointer("/result/thread_id").is_some()
                                || v.pointer("/result/threadID").is_some();
                            let _ = tx.send(success);
                        }
                    }
                }
            }
            let _ = append_codex_event(
                &path,
                json!({
                    "event": "app_server_output",
                    "stream": stream,
                    "text": line,
                    "ts": Utc::now()
                }),
            );
        }
    });
}

fn write_codex_initialize(
    stdin: &mut impl Write,
    cwd: Option<&PathBuf>,
    developer_instructions: Option<String>,
) -> std::io::Result<()> {
    let initialize = json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "team-mode-host",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    });
    let initialized = json!({ "method": "initialized" });
    let mut thread_params = serde_json::Map::new();
    if let Some(cwd) = cwd {
        thread_params.insert("cwd".into(), Value::String(cwd.display().to_string()));
    }
    if let Some(instructions) = developer_instructions {
        thread_params.insert(
            "collaborationMode".into(),
            json!({ "settings": { "developer_instructions": instructions } }),
        );
    }
    let thread_start = json!({
        "id": 2,
        "method": "thread/start",
        "params": Value::Object(thread_params)
    });
    writeln!(stdin, "{initialize}")?;
    writeln!(stdin, "{initialized}")?;
    writeln!(stdin, "{thread_start}")?;
    stdin.flush()
}

fn codex_thread_id_from_line(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    value
        .pointer("/result/threadId")
        .or_else(|| value.pointer("/result/thread_id"))
        .or_else(|| value.pointer("/threadId"))
        .or_else(|| value.pointer("/thread_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn append_codex_event(path: &Path, event: Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| Error::io(path, source))?;
    let line =
        serde_json::to_string(&event).map_err(|source| Error::json(path.to_path_buf(), source))?;
    writeln!(file, "{line}").map_err(|source| Error::io(path, source))?;
    Ok(())
}

#[cfg(windows)]
fn spawn_terminal_command(title: &str, command: &[String]) -> Result<SpawnResult> {
    let Some((program, rest)) = command.split_first() else {
        return Err(Error::Invalid("terminal command is empty".into()));
    };
    // Run the runner directly in a wt.exe tab (no PowerShell wrapper).
    // Avoids a nested ConPTY layer: wt -> PS -> runner -> ConPTY vs wt -> runner -> ConPTY.
    let mut wt_args = vec!["new-tab".to_string(), "--title".to_string(), title.to_string(), "--".to_string(), program.clone()];
    wt_args.extend_from_slice(rest);
    let wt = Command::new("wt.exe").args(&wt_args).spawn();
    match wt {
        Ok(child) => Ok(SpawnResult {
            pid: Some(child.id()),
            codex_tx: None,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Fallback: cmd.exe /k keeps the window open after the runner exits.
            let child = Command::new("cmd.exe")
                .arg("/k")
                .arg(program)
                .args(rest)
                .spawn()
                .map_err(|source| Error::io("cmd.exe", source))?;
            Ok(SpawnResult {
                pid: Some(child.id()),
                codex_tx: None,
            })
        }
        Err(source) => Err(Error::io("wt.exe", source)),
    }
}

#[cfg(not(windows))]
fn spawn_terminal_command(_title: &str, command: &[String]) -> Result<SpawnResult> {
    let Some((program, rest)) = command.split_first() else {
        return Err(Error::Invalid("terminal command is empty".into()));
    };
    // macOS: use osascript; Linux: use gnome-terminal/xterm; fallback: sh -c
    let child = Command::new(program)
        .args(rest)
        .spawn()
        .map_err(|source| Error::io(program.clone(), source))?;
    Ok(SpawnResult {
        pid: Some(child.id()),
        codex_tx: None,
    })
}

fn terminate_process(pid: u32) -> Result<()> {
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .map_err(|source| Error::io("taskkill", source))?;
        if !status.success() {
            return Err(Error::Other(format!("taskkill failed for pid {pid}")));
        }
    }
    #[cfg(not(windows))]
    {
        let status = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .map_err(|source| Error::io("kill", source))?;
        if !status.success() {
            return Err(Error::Other(format!("kill failed for pid {pid}")));
        }
    }
    Ok(())
}

fn non_empty_or(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn sibling_binary(name: &str) -> PathBuf {
    let file_name = format!("{}{}", name, std::env::consts::EXE_SUFFIX);
    match std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(&file_name)))
    {
        Some(path) => path,
        None => PathBuf::from(file_name),
    }
}

fn command_line(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| quote_command_arg(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_command_arg(value: &str) -> String {
    if value.is_empty()
        || value
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\'' | '&' | '|' | '<' | '>'))
    {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn assert_authenticated_sender(caller: Option<&str>, sender: &str) -> Result<()> {
    if let Some(caller) = caller {
        if caller != sender {
            return Err(Error::Invalid(format!(
                "authenticated caller {caller} cannot send as {sender}"
            )));
        }
    }
    Ok(())
}

fn assert_member_scope(caller: Option<&str>, member_id: &str, action: &str) -> Result<()> {
    if let Some(caller) = caller {
        if caller != member_id {
            return Err(Error::Invalid(format!(
                "authenticated caller {caller} cannot {action} for {member_id}"
            )));
        }
    }
    Ok(())
}

fn room_key(team_id: &str, room_id: &str) -> String {
    format!("{team_id}/{room_id}")
}

fn default_main_room() -> String {
    "main".into()
}

fn message_kind_str(kind: &MessageKind) -> &'static str {
    match kind {
        MessageKind::Discussion => "discussion",
        MessageKind::Dispatch => "dispatch",
        MessageKind::Reply => "reply",
        MessageKind::Direct => "direct",
        MessageKind::System => "system",
        MessageKind::Status => "status",
    }
}

fn format_injected_message(message: &Message) -> String {
    if message.kind == MessageKind::Direct {
        format!(
            "[TEAM MODE DIRECT MESSAGE]\nmessage_id: {}\nthread: {}\nfrom: {}\n\n{}\n\nReply by using `direct_reply` or `thread_reply`.\n[/TEAM MODE DIRECT MESSAGE]\n",
            message.id, message.thread_id, message.sender_member_id, message.body
        )
    } else {
        format!(
            "[TEAM MODE MESSAGE]\nmessage_id: {}\nteam: {}\nroom: {}\nthread: {}\nfrom: {}\nkind: {}\n\n{}\n\nReply by using the Team Mode MCP tool `thread_reply` for this thread.\n[/TEAM MODE MESSAGE]\n",
            message.id,
            message.team_id,
            message.room_id,
            message.thread_id,
            message.sender_member_id,
            message_kind_str(&message.kind),
            message.body
        )
    }
}

pub fn default_execution(member_id: impl Into<String>) -> ExecutionProfile {
    let member_id = member_id.into();
    ExecutionProfile {
        member_id,
        adapter: AdapterKind::ClaudeCodeTerminal,
        launch_mode: LaunchMode::NativeTerminalPty,
        viewer_mode: ViewerMode::NativeTerminal,
        command: CommandSpec {
            program: "claude".into(),
            args: Vec::new(),
        },
        cwd: None,
        env: BTreeMap::new(),
        model: None,
        reasoning_effort: None,
        system_prompt: String::new(),
        prompt_mode: PromptMode::Append,
        mcp_config: None,
        restart_policy: RestartPolicy::Never,
    }
}

fn rebuild_state_from_services(services: &TeamModeServices) -> HostState {
    let mut state = HostState::default();
    let teams = services.teams.list().unwrap_or_default();
    for team in teams {
        if let Ok(rooms) = services.rooms.list(&team.id) {
            for room in rooms {
                state.rooms.insert(room_key(&room.team_id, &room.id), room);
            }
        }
        if let Ok(members) = services.members.list(&team.id) {
            for member in members {
                state.members.insert(member.profile.id.clone(), member);
            }
        }
        state.teams.insert(team.id.clone(), team);
    }

    let messages = services.messages.list_all().unwrap_or_default();
    for message in messages {
        let thread = state
            .threads
            .entry(message.thread_id.clone())
            .or_insert_with(|| Thread {
                id: message.thread_id.clone(),
                team_id: message.team_id.clone(),
                room_id: message.room_id.clone(),
                root_message_id: message.id.clone(),
                subject: message.subject.clone(),
                message_ids: Vec::new(),
                status: ThreadStatus::Open,
                created_at: message.created_at,
                updated_at: message.created_at,
            });
        if !thread.message_ids.iter().any(|id| id == &message.id) {
            thread.message_ids.push(message.id.clone());
        }
        if message.created_at > thread.updated_at {
            thread.updated_at = message.created_at;
        }
        for recipient in &message.delivered_to {
            if recipient == &message.sender_member_id {
                continue;
            }
            state
                .inbox
                .entry(recipient.clone())
                .or_default()
                .push(InboxItem {
                    message_id: message.id.clone(),
                    team_id: message.team_id.clone(),
                    room_id: message.room_id.clone(),
                    thread_id: message.thread_id.clone(),
                    sender_member_id: message.sender_member_id.clone(),
                    unread: !message
                        .read_by
                        .iter()
                        .any(|receipt| receipt.actor == *recipient),
                    unacked: !message
                        .acked_by
                        .iter()
                        .any(|receipt| receipt.actor == *recipient),
                    delivered_at: message.created_at,
                });
        }
        state.messages.insert(message.id.clone(), message);
    }
    state
}
