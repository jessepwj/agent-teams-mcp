use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::runtime::{Builder as TokioRuntimeBuilder, Runtime as TokioRuntime};

use crate::backend::claude_code::ClaudeCodeBackend;
use crate::backend::codex::CodexBackend;
use crate::backend::gemini::GeminiCliBackend;
use crate::backend::{AgentOutput, BackendType, SpawnConfig};
use crate::error::{Error, Result};
use crate::team_mode::domain::{
    ExecutionMode, ExecutionProfile, MemberKind, MemberStatus, MessageKind,
};
use crate::team_mode::mcp::resources::{inbox_uri, room_uri, team_uri, thread_uri};
use crate::team_mode::mcp::schemas::{ToolCallResult, ToolDescriptor, empty_object_schema};
use crate::team_mode::service::{
    AddMemberRequest, CreateTeamRequest, InboxNotifier, InboxService, LeadPendingWriter,
    MemberService, MessageService, RoomService, SendMessageRequest, TeamService,
};
use crate::team_mode::storage::{
    MemberRecord, MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore,
};
use crate::runtime::AgentLoop;
use crate::util::validate_name;
use crate::RuntimeOrchestrator;

/// Per-team lead is always a virtual member with this name.
const LEAD_NAME: &str = "lead";

#[derive(Debug, Clone)]
pub struct ToolExecution {
    pub result: ToolCallResult,
    pub updated_resources: Vec<String>,
}

#[derive(Debug)]
pub struct TeamModeToolset {
    team_service: TeamService,
    member_service: MemberService,
    member_store: MemberStore,
    room_service: RoomService,
    message_store: MessageStore,
    message_service: MessageService,
    inbox_service: InboxService,
    inbox_notifier: InboxNotifier,
    runtime_orchestrator: Arc<tokio::sync::Mutex<RuntimeOrchestrator>>,
    async_runtime: Arc<TokioRuntime>,
    loop_handles: std::sync::Mutex<HashMap<String, crate::runtime::AgentLoopHandle>>,
}

impl TeamModeToolset {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        let team_store = TeamStore::new(base_dir.clone());
        let member_store = MemberStore::new(base_dir.clone());
        let room_store = RoomStore::new(base_dir.clone());
        let message_store = MessageStore::new(base_dir.clone());
        let projection_store = ProjectionStore::new(base_dir.clone());
        let lead_pending_writer = LeadPendingWriter::new(base_dir);
        let mut runtime_orchestrator = RuntimeOrchestrator::new();
        if let Ok(claude) = ClaudeCodeBackend::new() {
            runtime_orchestrator.register_backend(claude);
        }
        if let Ok(codex) = CodexBackend::new() {
            runtime_orchestrator.register_backend(codex);
        }
        if let Ok(gemini) = GeminiCliBackend::new() {
            runtime_orchestrator.register_backend(gemini);
        }

        let team_service = TeamService::new(team_store.clone());
        let member_service = MemberService::new(member_store.clone(), team_store.clone());
        let room_service = RoomService::new(room_store.clone());
        let inbox_notifier = InboxNotifier::new();
        let message_service = MessageService::new(
            message_store.clone(),
            member_store.clone(),
            room_store.clone(),
            team_store.clone(),
        )
        .with_inbox_notifier(inbox_notifier.clone())
        .with_lead_pending_writer(lead_pending_writer);
        let inbox_service = InboxService::new(projection_store, message_store.clone());

        Self {
            team_service,
            member_service,
            member_store,
            room_service,
            message_store,
            message_service,
            inbox_service,
            inbox_notifier,
            runtime_orchestrator: Arc::new(tokio::sync::Mutex::new(runtime_orchestrator)),
            async_runtime: Arc::new(
                TokioRuntimeBuilder::new_multi_thread()
                    .worker_threads(4)
                    .enable_all()
                    .build()
                    .expect("failed to create Team Mode MCP tokio runtime"),
            ),
            loop_handles: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn list_tools(&self) -> Vec<ToolDescriptor> {
        vec![
            tool(
                "team_create",
                "Create a team. A virtual 'lead' member is auto-created — the agent calling this MCP is that lead. `cwd` becomes the default working directory workers inherit unless they override it.",
                json_schema(
                    &[
                        ("name", json!({"type":"string"})),
                        ("cwd", json!({"type":"string"})),
                    ],
                    &["name"],
                ),
            ),
            tool(
                "team_list",
                "List all teams with their name, cwd and worker count.",
                empty_object_schema(),
            ),
            tool(
                "team_delete",
                "Delete a team. Running workers are automatically shut down and stored records for this team are removed. Returns any shutdown failures so you can manually clean up orphan processes.",
                json_schema(&[("name", json!({"type":"string"}))], &["name"]),
            ),
            tool(
                "worker_add",
                "Add a worker to a team AND start its managed agent process. \
                 On the first add pass `adapter` (and optionally other config). \
                 If a saved execution profile already exists for `name`, you MUST pass `on_existing` to tell MCP what to do: \
                 `reuse` = fast-resume with the saved config (extra config fields you pass are ignored), \
                 `overwrite` = replace saved config with what you pass now (adapter required), \
                 `error` = refuse and surface the path so you can investigate. \
                 `name` is a slug (letters/digits/_/-), cannot be 'lead', must be unique within the team. `cwd` defaults to the team's cwd when omitted.",
                json_schema(
                    &[
                        ("team", json!({"type":"string"})),
                        ("name", json!({"type":"string"})),
                        ("adapter", json!({"type":"string","enum":["claude-code","codex","gemini-cli"]})),
                        ("model", json!({"type":"string"})),
                        ("cwd", json!({"type":"string"})),
                        ("system_prompt", json!({"type":"string"})),
                        ("env", json!({"type":"object","additionalProperties":{"type":"string"}})),
                        ("on_existing", json!({"type":"string","enum":["reuse","overwrite","error"]})),
                    ],
                    &["team", "name"],
                ),
            ),
            tool(
                "worker_list",
                "List active workers in a team (lead is never listed). Returns `name`, `adapter`, `sessionState`.",
                json_schema(&[("team", json!({"type":"string"}))], &["team"]),
            ),
            tool(
                "worker_remove",
                "Remove a worker from the team: stop its process and mark the identity as removed. The on-disk execution profile is intentionally kept so the lead can later fast-resume via `worker_add` with `on_existing=reuse`.",
                json_schema(
                    &[
                        ("team", json!({"type":"string"})),
                        ("name", json!({"type":"string"})),
                    ],
                    &["team", "name"],
                ),
            ),
            tool(
                "send_message",
                "Send a message as the team's lead. `text` MUST contain at least one @handle AND every @handle in it MUST match an active worker — any unmatched @handles will cause the call to fail with the list of unmatched names. Example: `@alice please review the PR`.",
                json_schema(
                    &[
                        ("team", json!({"type":"string"})),
                        ("text", json!({"type":"string"})),
                    ],
                    &["team", "text"],
                ),
            ),
            tool(
                "inbox_read",
                "Read messages addressed to the team's lead. Pull-mode fallback when the FileChanged+asyncRewake push hook is not configured, or when you explicitly want to check. `limit` caps results (default 20, max 100). `unread_only=true` (default) skips already-acked messages. `auto_ack=true` marks returned messages as read+acked in the same call.",
                json_schema(
                    &[
                        ("team", json!({"type":"string"})),
                        ("limit", json!({"type":"integer","minimum":1,"maximum":100})),
                        ("unread_only", json!({"type":"boolean"})),
                        ("auto_ack", json!({"type":"boolean"})),
                    ],
                    &["team"],
                ),
            ),
        ]
    }

    pub fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<ToolExecution> {
        tracing::info!(tool = %name, "dispatching tool");
        let args = expect_object(arguments)?;
        match name {
            "team_create" => self.team_create(&args),
            "team_list" => Ok(success(json!({"teams": self.team_service.list()?}))),
            "team_delete" => self.team_delete(&args),
            "worker_add" => self.worker_add(&args),
            "worker_remove" => self.worker_remove(&args),
            "worker_list" => self.worker_list(&args),
            "send_message" => self.send_message(&args),
            "inbox_read" => self.inbox_read(&args),
            _ => Err(Error::Other(format!("unknown tool '{name}'"))),
        }
    }

    fn team_create(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let name = required_identifier(args, "name")?;
        if name == LEAD_NAME {
            return Err(Error::Other(
                "'lead' is reserved and cannot be used as a team name".into(),
            ));
        }

        let team = self.team_service.create(CreateTeamRequest {
            id: Some(name.clone()),
            name: name.clone(),
            description: None,
            cwd: optional_text(args, "cwd")?,
            lead_member_id: Some(LEAD_NAME.into()),
        })?;

        self.member_service.add(AddMemberRequest {
            team_id: team.id.clone(),
            name: LEAD_NAME.into(),
            kind: MemberKind::Lead,
            role_label: "lead".into(),
            role_description: None,
            execution: None,
        })?;

        self.room_service.ensure_main_room(&team.id)?;

        Ok(success_with_updates(
            json!(team.clone()),
            vec![team_uri(&team.id), inbox_uri(&team.id, LEAD_NAME)],
        ))
    }

    fn team_delete(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "name")?;
        let mut shutdown_failures: Vec<Value> = Vec::new();

        // Best-effort shutdown for every managed worker; collect failures
        // so the caller can clean up orphan processes if any.
        if let Ok(members) = self.member_service.list_by_team(&team_name) {
            for record in members {
                let key = spawn_key(&team_name, &record.profile.name);
                let orch = Arc::clone(&self.runtime_orchestrator);
                let key_clone = key.clone();
                let result = self.async_runtime.block_on(async move {
                    orch.lock().await.shutdown_managed_member(&key_clone).await
                });
                if let Err(err) = result {
                    if !matches!(record.profile.kind, MemberKind::Lead) {
                        shutdown_failures.push(json!({
                            "member": record.profile.name,
                            "reason": err.to_string(),
                        }));
                    }
                }
                if let Some(h) = self.loop_handles.lock().unwrap().remove(&key) {
                    let _ = h.shutdown_tx.send(());
                }
            }
        }

        self.team_service.delete(&team_name)?;

        Ok(success_with_updates(
            json!({
                "ok": true,
                "name": team_name.clone(),
                "shutdown_failures": shutdown_failures,
            }),
            vec![team_uri(&team_name)],
        ))
    }

    fn worker_add(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let worker_name = required_identifier(args, "name")?;
        if worker_name == LEAD_NAME {
            return Err(Error::Other(
                "'lead' is a reserved name and cannot be used for a worker".into(),
            ));
        }

        let team = self
            .team_service
            .get(&team_name)?
            .ok_or_else(|| Error::TeamNotFound { name: team_name.clone() })?;

        let existing = self.member_store.get(&team_name, &worker_name)?;
        let existing_execution = existing.as_ref().and_then(|r| r.execution.clone());
        let caller_adapter = optional_identifier(args, "adapter")?;
        let on_existing = optional_identifier(args, "on_existing")?
            .as_deref()
            .map(parse_on_existing)
            .transpose()?
            .unwrap_or(OnExisting::Error);

        // Decide the mode
        let mode = match (&existing_execution, &caller_adapter, on_existing) {
            (Some(_), _, OnExisting::Reuse) => WorkerAddMode::Reuse,
            (Some(_), Some(_), OnExisting::Overwrite) => WorkerAddMode::Overwrite,
            (Some(_), None, OnExisting::Overwrite) => {
                return Err(Error::Other(
                    "on_existing=overwrite requires `adapter`".into(),
                ));
            }
            (Some(_), _, OnExisting::Error) => {
                let members_path = self
                    .member_store_members_file_hint(&team_name);
                return Err(Error::Other(format!(
                    "worker '{worker_name}' already exists with an execution profile ({members_path}). \
                     Pass on_existing=reuse to fast-resume the saved config, \
                     on_existing=overwrite to replace it (adapter required), \
                     or choose a different worker name."
                )));
            }
            (None, Some(_), _) => WorkerAddMode::Create,
            (None, None, _) => {
                return Err(Error::Other(
                    "first-time worker_add requires `adapter`".into(),
                ));
            }
        };

        // Build / resolve execution profile
        let execution = match mode {
            WorkerAddMode::Reuse => existing_execution.clone().ok_or_else(|| {
                Error::Other(format!(
                    "worker '{worker_name}' has no saved profile to reuse"
                ))
            })?,
            WorkerAddMode::Overwrite | WorkerAddMode::Create => {
                let adapter = caller_adapter.clone().ok_or_else(|| {
                    Error::Other("adapter is required when overwriting or creating".into())
                })?;
                let cwd_for_worker = optional_text(args, "cwd")?.or_else(|| team.cwd.clone());
                ExecutionProfile {
                    execution_mode: ExecutionMode::Managed,
                    adapter: Some(adapter),
                    agent_name: Some(worker_name.clone()),
                    model: optional_identifier(args, "model")?,
                    cwd: cwd_for_worker,
                    env: parse_env_map(args.get("env"))?,
                    system_prompt: optional_text(args, "system_prompt")?,
                    skills: Vec::new(),
                    session_state: None,
                }
            }
        };

        // Upsert identity + execution
        let record = MemberRecord {
            profile: crate::team_mode::domain::MemberProfile {
                team_id: team_name.clone(),
                name: worker_name.clone(),
                kind: MemberKind::Member,
                role_label: "worker".into(),
                role_description: None,
                status: MemberStatus::Active,
                joined_at: existing
                    .as_ref()
                    .map(|e| e.profile.joined_at)
                    .unwrap_or_else(chrono::Utc::now),
            },
            execution: Some(execution.clone()),
        };
        self.member_store.upsert(record.clone())?;

        // Spawn managed process
        let backend_type: BackendType = execution
            .adapter
            .clone()
            .ok_or_else(|| Error::Other("execution profile missing adapter".into()))?
            .parse()
            .map_err(|e: String| Error::Other(e))?;
        let prompt = execution.system_prompt.clone().unwrap_or_else(|| {
            format!("You are {}", worker_name)
        });
        let mut config = SpawnConfig::new(
            execution.agent_name.clone().unwrap_or_else(|| worker_name.clone()),
            prompt,
        );
        config.model = execution.model.clone();
        config.cwd = execution.cwd.as_ref().map(PathBuf::from);
        config.env = execution.env.clone();
        config.reasoning_effort = Some("medium".into());

        let key = spawn_key(&team_name, &worker_name);
        let orch = Arc::clone(&self.runtime_orchestrator);
        let key_clone = key.clone();
        let wname = worker_name.clone();
        let handle = self.async_runtime.block_on(async move {
            orch.lock()
                .await
                .spawn_managed_member(key_clone, wname, config, backend_type)
                .await
        })?;

        let orch2 = Arc::clone(&self.runtime_orchestrator);
        let key_for_rx = key.clone();
        let output_rx = self
            .async_runtime
            .block_on(async move { orch2.lock().await.take_output_receiver(&key_for_rx) })
            .ok()
            .flatten();

        // Ready-check: drain until first TurnComplete or 5s timeout.
        let (rx_for_loop, ready_state) = if let Some(mut rx) = output_rx {
            let (reported_state, remaining_rx) = self.async_runtime.block_on(async move {
                let ready_timeout = Duration::from_secs(5);
                let result = tokio::time::timeout(ready_timeout, async {
                    loop {
                        match rx.recv().await {
                            Some(AgentOutput::TurnComplete) | Some(AgentOutput::Idle) => {
                                return Ok::<(), String>(());
                            }
                            Some(AgentOutput::Error(e)) => {
                                return Err(format!("agent error during startup: {e}"));
                            }
                            Some(_) => continue,
                            None => return Err("agent process exited during startup".into()),
                        }
                    }
                })
                .await;
                let state = match result {
                    Ok(Ok(())) => "running",
                    Ok(Err(_)) => "failed",
                    Err(_) => "starting",
                };
                (state, rx)
            });

            if reported_state == "failed" {
                // Clean up: remove the profile we just wrote, reset status
                let _ = self.member_store.mark_removed(&team_name, &worker_name);
                return Err(Error::Other(format!(
                    "worker '{worker_name}' failed to start"
                )));
            }

            (Some(remaining_rx), reported_state.to_string())
        } else {
            (None, handle.session_state.as_str().to_string())
        };

        if let Some(rx) = rx_for_loop {
            let agent_loop = AgentLoop {
                member_id: worker_name.clone(),
                team_id: team_name.clone(),
                room_id: "main".into(),
                orchestrator: Arc::clone(&self.runtime_orchestrator),
                inbox_service: self.inbox_service.clone(),
                message_store: self.message_store.clone(),
                message_service: self.message_service.clone(),
                poll_interval: Duration::from_secs(5),
                inbox_notifier: Some(self.inbox_notifier.clone()),
            };
            let loop_handle = agent_loop.start(rx);
            self.loop_handles
                .lock()
                .unwrap()
                .insert(key.clone(), loop_handle);
        }

        let mode_str = match mode {
            WorkerAddMode::Reuse => "reuse",
            WorkerAddMode::Overwrite => "overwrite",
            WorkerAddMode::Create => "create",
        };

        Ok(success_with_updates(
            json!({
                "team": team_name.clone(),
                "name": worker_name.clone(),
                "sessionState": ready_state,
                "mode": mode_str,
            }),
            vec![
                team_uri(&team_name),
                inbox_uri(&team_name, &worker_name),
            ],
        ))
    }

    fn worker_remove(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let worker_name = required_identifier(args, "name")?;
        if worker_name == LEAD_NAME {
            return Err(Error::Other(
                "the team lead cannot be removed via worker_remove".into(),
            ));
        }
        let key = spawn_key(&team_name, &worker_name);

        // Shut down process (best-effort)
        let orch = Arc::clone(&self.runtime_orchestrator);
        let key_clone = key.clone();
        let _ = self.async_runtime.block_on(async move {
            orch.lock().await.shutdown_managed_member(&key_clone).await
        });
        if let Some(h) = self.loop_handles.lock().unwrap().remove(&key) {
            let _ = h.shutdown_tx.send(());
        }

        // Soft-remove: status=Removed but keep execution for fast-resume.
        let changed = self.member_store.mark_removed(&team_name, &worker_name)?;
        if !changed {
            return Err(Error::MemberNotFound {
                team: team_name,
                member: worker_name,
            });
        }

        Ok(success_with_updates(
            json!({ "ok": true, "team": team_name.clone(), "name": worker_name.clone() }),
            vec![
                team_uri(&team_name),
                inbox_uri(&team_name, &worker_name),
            ],
        ))
    }

    fn worker_list(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let members = self.member_service.list_active(&team_name)?;
        let workers: Vec<Value> = members
            .into_iter()
            .filter(|r| !matches!(r.profile.kind, MemberKind::Lead))
            .map(|record| {
                let adapter = record
                    .execution
                    .as_ref()
                    .and_then(|e| e.adapter.clone())
                    .unwrap_or_default();
                let session_state = record
                    .execution
                    .as_ref()
                    .and_then(|e| e.session_state.as_ref())
                    .map(|s| s.as_str())
                    .unwrap_or("not-spawned")
                    .to_string();
                json!({
                    "name": record.profile.name,
                    "adapter": adapter,
                    "sessionState": session_state,
                })
            })
            .collect();
        Ok(success(json!({ "workers": workers })))
    }

    fn send_message(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let text = required_text(args, "text")?;

        let _team = self
            .team_service
            .get(&team_name)?
            .ok_or_else(|| Error::TeamNotFound { name: team_name.clone() })?;

        // Mandatory: text must contain at least one @handle.
        let body_handles = extract_handles(&text);
        if body_handles.is_empty() {
            return Err(Error::Other(
                "send_message requires at least one @handle in `text` naming an active worker"
                    .into(),
            ));
        }

        // Build active worker set (excluding lead).
        let members = self.member_service.list_active(&team_name)?;
        let active_worker_names: HashSet<_> = members
            .iter()
            .filter(|r| !matches!(r.profile.kind, MemberKind::Lead))
            .map(|r| r.profile.name.clone())
            .collect();

        // Strict check: ALL @handles must resolve to an active worker.
        let unmatched: Vec<_> = body_handles
            .iter()
            .filter(|h| !active_worker_names.contains(*h))
            .cloned()
            .collect();
        if !unmatched.is_empty() {
            let mut active_sorted: Vec<_> = active_worker_names.iter().cloned().collect();
            active_sorted.sort();
            return Err(Error::Other(format!(
                "send_message: unmatched @mentions {:?}. Active workers in team '{}': {:?}",
                unmatched, team_name, active_sorted
            )));
        }

        let room_id = "main".to_string();
        self.room_service.ensure_main_room(&team_name)?;

        let message = self.message_service.send(SendMessageRequest {
            team_id: team_name.clone(),
            room_id: room_id.clone(),
            sender: LEAD_NAME.into(),
            kind: MessageKind::Dispatch,
            subject: None,
            body: text,
            mentions: Vec::new(),
            visibility: Vec::new(),
            audience_policy: None,
            reply_to: None,
            thread_id: None,
            expires_at: None,
        })?;

        let mut updated = vec![
            team_uri(&team_name),
            room_uri(&team_name, &room_id),
            thread_uri(&team_name, message.thread_id.as_deref().unwrap_or("")),
        ];
        updated.extend(
            message
                .effective_recipients
                .iter()
                .map(|recipient| inbox_uri(&team_name, recipient)),
        );

        let matched_recipients: Vec<Value> = message
            .effective_recipients
            .iter()
            .cloned()
            .map(Value::String)
            .collect();

        Ok(success_with_updates(
            json!({
                "message": message,
                "matched_recipients": matched_recipients,
            }),
            updated,
        ))
    }

    fn inbox_read(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let limit = optional_usize(args, "limit")?.unwrap_or(20).clamp(1, 100);
        let unread_only = optional_bool(args, "unread_only")?.unwrap_or(true);
        let auto_ack = optional_bool(args, "auto_ack")?.unwrap_or(false);

        self.team_service
            .get(&team_name)?
            .ok_or_else(|| Error::TeamNotFound {
                name: team_name.clone(),
            })?;

        let inbox = self
            .inbox_service
            .peek(&team_name, LEAD_NAME, None)?;

        let mut items: Vec<_> = inbox
            .items
            .into_iter()
            .filter(|item| {
                if !unread_only {
                    return true;
                }
                !matches!(
                    item.status,
                    crate::team_mode::domain::InboxStatus::Acked
                )
            })
            .collect();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        items.truncate(limit);

        let mut messages_out: Vec<Value> = Vec::with_capacity(items.len());
        let mut ack_ids: Vec<String> = Vec::with_capacity(items.len());
        for item in items {
            let message = match self.message_store.get(&team_name, &item.message_id) {
                Ok(Some(m)) => m,
                _ => continue,
            };
            messages_out.push(json!({
                "id": message.id,
                "from": message.sender,
                "kind": kind_to_str(&message.kind),
                "text": message.body,
                "reply_to": message.reply_to,
                "thread_id": message.thread_id,
                "status": status_to_str(&item.status),
                "created_at": message.created_at,
            }));
            ack_ids.push(message.id);
        }

        if auto_ack && !ack_ids.is_empty() {
            let _ = self.inbox_service.read(&team_name, LEAD_NAME, &ack_ids);
            let _ = self.inbox_service.ack(&team_name, LEAD_NAME, &ack_ids);
        }

        let unread_count = self
            .inbox_service
            .count(&team_name, LEAD_NAME, None)
            .map(|c| c.unread)
            .unwrap_or(0);

        Ok(success(json!({
            "team": team_name,
            "lead": LEAD_NAME,
            "unread_count": unread_count,
            "total_returned": messages_out.len(),
            "messages": messages_out,
        })))
    }

    fn member_store_members_file_hint(&self, team_name: &str) -> String {
        crate::team_mode::data_dir::members_file(&self.base_dir_of(), team_name)
            .to_string_lossy()
            .to_string()
    }

    fn base_dir_of(&self) -> PathBuf {
        // Use team_service to recover a path via team store (not exposed).
        // Fallback: since we can't get base_dir from stores, use default.
        crate::team_mode::data_dir::DEFAULT_NAME.into()
    }
}

// ---------------------------------------------------------------------------
// Tool-add mode logic
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnExisting {
    Reuse,
    Overwrite,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerAddMode {
    Create,
    Overwrite,
    Reuse,
}

fn parse_on_existing(s: &str) -> Result<OnExisting> {
    match s {
        "reuse" => Ok(OnExisting::Reuse),
        "overwrite" => Ok(OnExisting::Overwrite),
        "error" => Ok(OnExisting::Error),
        other => Err(Error::Other(format!(
            "invalid on_existing value '{other}' (must be reuse|overwrite|error)"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compose the orchestrator key from (team, worker_name). Kept as a
/// compound string for backward compatibility with RuntimeOrchestrator,
/// which still uses single-string keys.
fn spawn_key(team: &str, name: &str) -> String {
    format!("{team}__{name}")
}

fn parse_env_map(value: Option<&Value>) -> Result<HashMap<String, String>> {
    let Some(value) = value else {
        return Ok(HashMap::new());
    };
    let obj = value
        .as_object()
        .ok_or_else(|| Error::Other("env must be a JSON object".into()))?;
    let mut map = HashMap::new();
    for (k, v) in obj {
        let v = v
            .as_str()
            .ok_or_else(|| Error::Other(format!("env value for '{k}' must be a string")))?;
        map.insert(k.clone(), v.to_string());
    }
    Ok(map)
}

fn tool(name: &str, description: &str, input_schema: Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema,
    }
}

fn json_schema(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let mut props = Map::new();
    for (name, schema) in properties {
        props.insert((*name).into(), schema.clone());
    }
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

fn expect_object(arguments: Option<Value>) -> Result<Map<String, Value>> {
    match arguments {
        Some(Value::Object(map)) => Ok(map),
        None => Ok(Map::new()),
        _ => Err(Error::Other("tool arguments must be an object".into())),
    }
}

fn required_identifier(args: &Map<String, Value>, key: &str) -> Result<String> {
    optional_identifier(args, key)?
        .ok_or_else(|| Error::Other(format!("missing required field '{key}'")))
}

fn optional_identifier(args: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            validate_name(value)?;
            Ok(Some(value.clone()))
        }
        Some(_) => Err(Error::Other(format!("field '{key}' must be a string"))),
    }
}

fn required_text(args: &Map<String, Value>, key: &str) -> Result<String> {
    optional_text(args, key)?.ok_or_else(|| Error::Other(format!("missing required field '{key}'")))
}

fn optional_text(args: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            if value.trim().is_empty() {
                return Err(Error::Other(format!("field '{key}' cannot be empty")));
            }
            Ok(Some(value.clone()))
        }
        Some(_) => Err(Error::Other(format!("field '{key}' must be a string"))),
    }
}

fn optional_usize(args: &Map<String, Value>, key: &str) -> Result<Option<usize>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|v| v as usize)
            .map(Some)
            .ok_or_else(|| Error::Other(format!("field '{key}' must be a non-negative integer"))),
        Some(_) => Err(Error::Other(format!(
            "field '{key}' must be a non-negative integer"
        ))),
    }
}

fn optional_bool(args: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(Error::Other(format!("field '{key}' must be a boolean"))),
    }
}

fn kind_to_str(kind: &MessageKind) -> &'static str {
    match kind {
        MessageKind::Dispatch => "dispatch",
        MessageKind::Discussion => "discussion",
        MessageKind::Reply => "reply",
        MessageKind::System => "system",
        MessageKind::Notice => "notice",
        MessageKind::Status => "status",
    }
}

fn status_to_str(status: &crate::team_mode::domain::InboxStatus) -> &'static str {
    use crate::team_mode::domain::InboxStatus;
    match status {
        InboxStatus::Unread => "unread",
        InboxStatus::Read => "read",
        InboxStatus::Acked => "acked",
    }
}

fn extract_handles(body: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut handles: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let starts_mention = chars[i] == '@'
            && (i == 0 || !is_handle_char(chars[i - 1]));
        if starts_mention {
            let mut h = String::new();
            let mut j = i + 1;
            while j < chars.len() && is_handle_char(chars[j]) {
                h.push(chars[j]);
                j += 1;
            }
            if !h.is_empty() && !handles.contains(&h) {
                handles.push(h);
            }
            i = j;
            continue;
        }
        i += 1;
    }
    handles
}

fn is_handle_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')
}

fn success(value: Value) -> ToolExecution {
    ToolExecution {
        result: ToolCallResult::success(value),
        updated_resources: Vec::new(),
    }
}

fn success_with_updates(value: Value, updated_resources: Vec<String>) -> ToolExecution {
    ToolExecution {
        result: ToolCallResult::success(value),
        updated_resources,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn list_tools_exposes_minimal_surface() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path()).list_tools();
        let names: Vec<_> = tools.into_iter().map(|t| t.name).collect();

        let expected = [
            "team_create", "team_list", "team_delete",
            "worker_add", "worker_list", "worker_remove",
            "send_message", "inbox_read",
        ];
        for name in &expected {
            assert!(names.iter().any(|n| n == name), "missing tool '{name}'");
        }
        assert_eq!(names.len(), expected.len(), "unexpected tools: {names:?}");
    }

    #[test]
    fn team_create_auto_creates_lead_member() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools
            .call_tool("team_create", Some(json!({"name": "demo"})))
            .unwrap();

        let team_list = tools.call_tool("team_list", Some(json!({}))).unwrap();
        let v = team_list.result.structured_content.unwrap();
        let teams = v["teams"].as_array().unwrap();
        let team = teams.iter().find(|t| t["name"] == "demo").unwrap();
        assert_eq!(team["leadMemberId"].as_str().unwrap(), "lead");
    }

    #[test]
    fn send_message_rejects_no_mention() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools.call_tool("team_create", Some(json!({"name": "demo"}))).unwrap();

        let err = tools
            .call_tool("send_message", Some(json!({
                "team": "demo",
                "text": "no mention here",
            })))
            .unwrap_err();
        assert!(matches!(&err, Error::Other(msg) if msg.contains("@handle")));
    }

    #[test]
    fn send_message_rejects_any_unmatched_mention() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools.call_tool("team_create", Some(json!({"name": "demo"}))).unwrap();

        // Even if alice doesn't exist yet, @typo must fail.
        let err = tools
            .call_tool("send_message", Some(json!({
                "team": "demo",
                "text": "@typo please",
            })))
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("unmatched"), "got: {msg}");
        assert!(msg.contains("typo"), "got: {msg}");
    }

    #[test]
    fn worker_remove_refuses_lead() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools.call_tool("team_create", Some(json!({"name": "demo"}))).unwrap();

        let err = tools
            .call_tool("worker_remove", Some(json!({"team": "demo", "name": "lead"})))
            .unwrap_err();
        assert!(matches!(&err, Error::Other(msg) if msg.contains("lead")));
    }

    #[test]
    fn worker_add_refuses_reserved_name() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools.call_tool("team_create", Some(json!({"name": "demo"}))).unwrap();

        let err = tools
            .call_tool("worker_add", Some(json!({
                "team": "demo",
                "name": "lead",
                "adapter": "claude-code",
            })))
            .unwrap_err();
        assert!(matches!(&err, Error::Other(msg) if msg.contains("reserved")));
    }

    #[test]
    fn worker_list_excludes_the_lead() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools.call_tool("team_create", Some(json!({"name": "demo"}))).unwrap();

        let resp = tools
            .call_tool("worker_list", Some(json!({"team": "demo"})))
            .unwrap();
        let v = resp.result.structured_content.unwrap();
        let workers = v["workers"].as_array().unwrap();
        assert!(workers.is_empty());
    }

    #[test]
    fn inbox_read_on_empty_team_returns_empty() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools.call_tool("team_create", Some(json!({"name": "demo"}))).unwrap();

        let resp = tools
            .call_tool("inbox_read", Some(json!({"team": "demo"})))
            .unwrap();
        let v = resp.result.structured_content.unwrap();
        assert_eq!(v["team"], "demo");
        assert_eq!(v["lead"], "lead");
        assert_eq!(v["unread_count"], 0);
        assert_eq!(v["total_returned"], 0);
    }

    #[test]
    fn inbox_read_rejects_unknown_team() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        let err = tools
            .call_tool("inbox_read", Some(json!({"team": "no-such"})))
            .unwrap_err();
        assert!(matches!(&err, Error::TeamNotFound { name } if name == "no-such"));
    }

    #[test]
    fn extract_handles_parses_valid_at_mentions() {
        assert_eq!(extract_handles("hi @alice and @bob"), vec!["alice", "bob"]);
        assert_eq!(extract_handles("no mentions here"), Vec::<String>::new());
        assert_eq!(extract_handles("@x @x @x"), vec!["x"]);
        assert_eq!(extract_handles("foo@example.com"), Vec::<String>::new());
    }
}
