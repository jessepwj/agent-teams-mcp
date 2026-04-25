use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::runtime::{Builder as TokioRuntimeBuilder, Runtime as TokioRuntime};

use crate::ExecutionSessionState;
use crate::RuntimeOrchestrator;
use crate::backend::claude_code::ClaudeCodeBackend;
use crate::backend::codex::CodexBackend;
use crate::backend::gemini::GeminiCliBackend;
use crate::backend::{AgentOutput, BackendType, SpawnConfig};
use crate::error::{Error, Result};
use crate::runtime::AgentLoop;
use crate::team_mode::domain::{
    ExecutionMode, ExecutionProfile, MemberKind, MemberStatus, MessageKind,
};
use crate::team_mode::mcp::resources::{inbox_uri, room_uri, team_uri, thread_uri};
use crate::team_mode::mcp::schemas::{ToolCallResult, ToolDescriptor, empty_object_schema};
use crate::team_mode::runtime_workers::{
    RuntimeWorkerStore, STATE_DEAD, STATE_FAILED, STATE_RUNNING, STATE_STARTING, STATE_STOPPED,
};
use crate::team_mode::service::{
    AddMemberRequest, CreateTeamRequest, InboxNotifier, InboxService, LeadPendingWriter,
    MemberService, MessageService, RoomService, SendMessageRequest, TeamService,
};
use crate::team_mode::storage::{
    MemberRecord, MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore,
};
use crate::util::validate_name;

/// Per-team lead is always a virtual member with this name.
const LEAD_NAME: &str = "lead";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    pub result: ToolCallResult,
    pub updated_resources: Vec<String>,
}

#[derive(Debug)]
pub struct TeamModeToolset {
    base_dir: PathBuf,
    team_service: TeamService,
    member_service: MemberService,
    member_store: MemberStore,
    room_service: RoomService,
    message_store: MessageStore,
    message_service: MessageService,
    inbox_service: InboxService,
    inbox_notifier: InboxNotifier,
    runtime_workers: RuntimeWorkerStore,
    /// Clone of the pending writer kept here so non-send paths (team_delete)
    /// can prune entries for a team being destroyed. The writer is otherwise
    /// plumbed via message_service; this is a second handle to the same
    /// underlying file + lock.
    lead_pending_writer: LeadPendingWriter,
    runtime_orchestrator: Arc<tokio::sync::Mutex<RuntimeOrchestrator>>,
    async_runtime: Arc<TokioRuntime>,
    loop_handles: std::sync::Mutex<HashMap<String, crate::runtime::AgentLoopHandle>>,
}

impl TeamModeToolset {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let project_root = std::env::current_dir().ok();
        Self::new_with_project_root(base_dir, project_root)
    }

    pub fn new_with_project_root(
        base_dir: impl Into<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Self {
        let base_dir = base_dir.into();
        let team_store = TeamStore::new(base_dir.clone());
        let member_store = MemberStore::new(base_dir.clone());
        let room_store = RoomStore::new(base_dir.clone());
        let message_store = MessageStore::new(base_dir.clone());
        let projection_store = ProjectionStore::new(base_dir.clone());
        // NOTE: lead_pending.jsonl intentionally lives in the MCP's cwd
        // (= project root when launched by Claude Code), NOT in `base_dir`.
        // Claude Code's FileChanged hook matcher only watches files at the
        // project root — files in hidden subdirs like `.agent-teams/` are
        // ignored. If cwd is unavailable we fall back to base_dir (hooks
        // won't fire but data is still recoverable via inbox_read).
        let pending_dir = project_root
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| base_dir.clone()));
        let lead_pending_writer = LeadPendingWriter::new(pending_dir);
        // Housekeeping: drop pending entries belonging to dead CC processes
        // from prior sessions. Without this, stale lines accumulate forever
        // (ancestor-chain routing never matches them, so the hook keeps
        // writing them back to disk unchanged each fire).
        lead_pending_writer.prune_dead_owners();
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
        .with_lead_pending_writer(lead_pending_writer.clone());
        let inbox_service = InboxService::new(projection_store, message_store.clone());
        let runtime_workers = RuntimeWorkerStore::new(base_dir.clone());

        Self {
            base_dir,
            team_service,
            member_service,
            member_store,
            room_service,
            message_store,
            message_service,
            inbox_service,
            inbox_notifier,
            runtime_workers,
            lead_pending_writer,
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
        // Tool descriptions are deliberately terse. Detailed guidance and
        // failure-mode hints come back as `hint` / `note` fields in tool
        // responses — that is just-in-time context, where attention is
        // freshest. Static descriptions only carry the contract the AI
        // must know to even begin a call.
        vec![
            tool(
                "team_create",
                "Create a team. The caller becomes its lead.",
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
                "List teams.",
                empty_object_schema(),
            ),
            tool(
                "team_delete",
                "Delete a team and stop all its workers.",
                json_schema(&[("name", json!({"type":"string"}))], &["name"]),
            ),
            tool(
                "worker_add",
                "Add a worker to a team and start its process. \
                 Pass `adapter` on first add. If a saved profile for this name \
                 already exists, also pass `on_existing` to choose what to do.",
                json_schema(
                    &[
                        ("team", json!({"type":"string"})),
                        ("name", json!({"type":"string"})),
                        (
                            "adapter",
                            json!({"type":"string","enum":["claude-code","codex","gemini-cli"]}),
                        ),
                        ("model", json!({"type":"string"})),
                        ("cwd", json!({"type":"string"})),
                        ("system_prompt", json!({"type":"string"})),
                        (
                            "env",
                            json!({"type":"object","additionalProperties":{"type":"string"}}),
                        ),
                        (
                            "on_existing",
                            json!({"type":"string","enum":["reuse","overwrite","error"]}),
                        ),
                    ],
                    &["team", "name"],
                ),
            ),
            tool(
                "worker_list",
                "List a team's workers and their state.",
                json_schema(&[("team", json!({"type":"string"}))], &["team"]),
            ),
            tool(
                "worker_remove",
                "Stop a worker's process. The on-disk profile is kept so it can be revived later with `worker_add on_existing=reuse`.",
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
                "Send a message as team lead. `text` must contain at least one \
                 @handle that matches an active worker (e.g. `@alice please review`). \
                 Replies arrive automatically as a system-reminder when your \
                 next turn starts — do not poll or sleep waiting for them.",
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
                "Read the lead's inbox. Replies normally arrive automatically \
                 via the Stop hook; this tool is a fallback for explicit backlog \
                 checks. `auto_ack=true` marks returned messages as read.",
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
            "team_list" => {
                // Enhance bare `team_service.list()` output with a live/orphan
                // marker per team (based on owner_cc_pid process liveness).
                // Lets callers immediately distinguish "my team" vs "dead
                // team from a previous CC session" without an extra tool call.
                let teams = self.team_service.list()?;
                let mut sys = sysinfo::System::new();
                sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
                let mut orphan_names: Vec<String> = Vec::new();
                let decorated = teams
                    .into_iter()
                    .map(|team| {
                        let status = match team.owner_cc_pid {
                            Some(pid) => {
                                if sys.process(sysinfo::Pid::from_u32(pid)).is_some() {
                                    "alive"
                                } else {
                                    orphan_names.push(team.name.clone());
                                    "orphan"
                                }
                            }
                            None => "unbound",
                        };
                        let mut val = serde_json::to_value(&team).unwrap_or(Value::Null);
                        if let Value::Object(obj) = &mut val {
                            obj.insert("ownerStatus".into(), json!(status));
                        }
                        val
                    })
                    .collect::<Vec<_>>();
                let mut payload = json!({ "teams": decorated });
                if !orphan_names.is_empty() {
                    if let Value::Object(map) = &mut payload {
                        map.insert(
                            "hint".into(),
                            Value::String(format!(
                                "Orphan teams (owner CC has died): [{}]. \
                                 Their workers are gone; run `team_delete name=<x>` \
                                 on each to free the one-live-team-per-project budget. \
                                 (team_create also auto-cleans orphans when called.)",
                                orphan_names.join(", ")
                            )),
                        );
                    }
                }
                Ok(success(payload))
            }
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
        // Strict slug check so the team name stays @mention-able and safe
        // for filesystem use across platforms. Catches uppercase, spaces,
        // unicode, leading dots/dashes, and >64-byte names — all of which
        // earlier silently slipped through and produced unreachable teams.
        crate::util::validate_slug_name(&name)?;

        // Bind this team to the CC process that owns our MCP stdio session
        // (we are CC's child; our parent PID == CC's PID). Push routing will
        // later use this to send worker replies only to the owner CC.
        let owner_cc_pid = optional_u32(args, "_owner_cc_pid")?.or_else(current_parent_pid);

        // Enforce: one project = at most one LIVE team at a time.
        //
        // Rationale: workers consume real resources (subprocesses, model
        // calls), and the lead CC is the coordinator. Allowing multiple
        // live teams per project would let a single CC spawn runaway work
        // by accident (forgetting to clean up old teams), and is the
        // natural model the user thinks in (one project, one team-in-flight).
        //
        // Behavior:
        //   - If any existing team has a still-alive owner_cc_pid → refuse
        //     with a clear error listing which team and who owns it. Caller
        //     must team_delete the existing one before creating a new one.
        //   - If only orphan teams exist (owner CC is dead) → auto-clean
        //     them and proceed. We report how many we reaped.
        //   - If no teams exist → proceed cleanly.
        let cleaned_orphans = self.enforce_single_live_team(owner_cc_pid)?;

        let team = self.team_service.create(CreateTeamRequest {
            id: Some(name.clone()),
            name: name.clone(),
            description: None,
            cwd: optional_text(args, "cwd")?,
            lead_member_id: Some(LEAD_NAME.into()),
            owner_cc_pid,
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

        let web_status = open_team_web_ui(&self.base_dir, &team.id);
        let mut response = json!(team.clone());
        if let Value::Object(obj) = &mut response {
            obj.insert("web".into(), web_status.to_json());
            if !cleaned_orphans.is_empty() {
                obj.insert("cleaned_orphan_teams".into(), json!(cleaned_orphans));
            }
        }

        Ok(success_with_updates(
            response,
            vec![team_uri(&team.id), inbox_uri(&team.id, LEAD_NAME)],
        ))
    }

    /// Enforce the "one live team per project" invariant.
    ///
    /// Returns the list of orphan team names that were auto-cleaned, so the
    /// caller can surface them to the user. Fails hard if any LIVE team
    /// (owner_cc_pid still alive per sysinfo) already exists — the caller
    /// must explicitly delete that team first.
    fn enforce_single_live_team(&self, _caller_pid: Option<u32>) -> Result<Vec<String>> {
        use sysinfo::{Pid, ProcessesToUpdate, System};

        let existing = self.team_service.list()?;
        if existing.is_empty() {
            return Ok(Vec::new());
        }

        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::All, true);
        let is_live = |pid: u32| -> bool { sys.process(Pid::from_u32(pid)).is_some() };

        let mut live = Vec::new();
        let mut orphans = Vec::new();
        for team in &existing {
            match team.owner_cc_pid {
                Some(pid) if is_live(pid) => live.push((team.id.clone(), pid)),
                Some(_) => orphans.push(team.id.clone()),
                None => {
                    // Legacy / unbound team — treat conservatively as live
                    // (can't verify death, don't auto-purge user data).
                    live.push((team.id.clone(), 0));
                }
            }
        }

        if !live.is_empty() {
            let listed = live
                .iter()
                .map(|(name, pid)| format!("'{}' (owner_cc_pid={})", name, pid))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::Other(format!(
                "this project already has {} live team(s): {}. \
                 Only one live team per project is allowed — call `team_delete` \
                 on the existing team before creating a new one.",
                live.len(),
                listed
            )));
        }

        // Only orphans remain. Auto-clean them so the new team_create succeeds,
        // but report back so the caller can surface what happened.
        for orphan_id in &orphans {
            if let Err(err) = self.team_service.delete(orphan_id) {
                tracing::warn!(
                    team = %orphan_id,
                    error = %err,
                    "failed to delete orphan team during team_create cleanup; will surface error"
                );
                return Err(Error::Other(format!(
                    "failed to auto-clean orphan team '{orphan_id}': {err}. \
                     Delete it manually and retry."
                )));
            }
            // Best-effort: also drop the worker runtime record for the orphan.
            let _ = self.runtime_workers.remove_team(orphan_id);
        }
        if !orphans.is_empty() {
            tracing::info!(
                count = orphans.len(),
                teams = ?orphans,
                "auto-cleaned orphan teams before creating new one"
            );
        }
        Ok(orphans)
    }

    fn team_delete(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "name")?;
        let mut shutdown_failures: Vec<Value> = Vec::new();

        // Best-effort shutdown for every managed worker; collect failures
        // so the caller can clean up orphan processes if any.
        //
        // Skip rules:
        //   - Members already in `Removed` status had `worker_remove` called
        //     earlier; their orchestrator slot was cleaned up at that point.
        //     Re-attempting shutdown only produces a noisy "session not
        //     registered" error in the response that confuses the lead.
        //   - "session not registered" / "MemberNotFound" failures during the
        //     loop are also benign — they just mean the orchestrator already
        //     forgot the spawn_key (daemon restart or earlier remove). We
        //     drop those from `shutdown_failures` so only real shutdown
        //     errors (a still-alive child the OS refused to kill) surface.
        if let Ok(members) = self.member_service.list_by_team(&team_name) {
            for record in members {
                if matches!(record.profile.status, MemberStatus::Removed) {
                    continue;
                }
                let key = spawn_key(&team_name, &record.profile.name);
                let orch = Arc::clone(&self.runtime_orchestrator);
                let key_clone = key.clone();
                let result = self.async_runtime.block_on(async move {
                    orch.lock().await.shutdown_managed_member(&key_clone).await
                });
                if let Err(err) = result {
                    let msg = err.to_string();
                    // Filter benign "this session was already gone" errors
                    // so the response only flags REAL shutdown failures
                    // (a still-alive child the OS refused to kill). The
                    // orchestrator surfaces session-not-found as either of
                    // these two strings depending on which call path missed:
                    //   - "no managed session registered for spawn_key '...'"
                    //   - "Member '...' not found in team 'runtime'" (legacy
                    //     path before the orchestrator dropped the "runtime"
                    //     placeholder team — kept for older daemons)
                    let is_already_gone = msg.contains("no managed session registered")
                        || (msg.contains("not found") && msg.contains("'runtime'"));
                    if !matches!(record.profile.kind, MemberKind::Lead) && !is_already_gone {
                        shutdown_failures.push(json!({
                            "team": team_name.clone(),
                            "member": record.profile.name,
                            "reason": msg,
                        }));
                    }
                }
                if let Some(h) = self.loop_handles.lock().unwrap().remove(&key) {
                    let _ = h.shutdown_tx.send(());
                }
                if !matches!(record.profile.kind, MemberKind::Lead) {
                    let _ = self.runtime_workers.upsert_state(
                        &team_name,
                        &record.profile.name,
                        &key,
                        record.execution.and_then(|e| e.adapter),
                        STATE_STOPPED,
                        None,
                    );
                }
            }
        }

        self.team_service.delete(&team_name)?;
        let _ = self.runtime_workers.remove_team(&team_name);
        // Prune any pending entries that still reference this team so the
        // lead doesn't get a stale reminder for a team that no longer exists.
        let pruned_pending = self.lead_pending_writer.prune_team(&team_name);

        let mut result = json!({
            "ok": true,
            "name": team_name.clone(),
            "shutdown_failures": shutdown_failures,
        });
        if pruned_pending > 0 {
            if let Value::Object(obj) = &mut result {
                obj.insert("pruned_pending_entries".into(), json!(pruned_pending));
            }
        }

        Ok(success_with_updates(result, vec![team_uri(&team_name)]))
    }

    fn worker_add(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let worker_name = required_identifier(args, "name")?;
        if worker_name == LEAD_NAME {
            return Err(Error::Other(
                "'lead' is a reserved name and cannot be used for a worker".into(),
            ));
        }
        // Same strict slug check as team_create. Without this, names with
        // spaces / uppercase / unicode were accepted but unreachable via
        // @mention — workers existed in members.json but no message could
        // ever route to them.
        crate::util::validate_slug_name(&worker_name)?;

        let team = self
            .team_service
            .get(&team_name)?
            .ok_or_else(|| Error::TeamNotFound {
                name: team_name.clone(),
            })?;

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
                let members_path = self.member_store_members_file_hint(&team_name);
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
                    session_id: None,
                }
            }
        };

        // Validate adapter up-front — BEFORE any state changes. Otherwise a
        // bad adapter leaves behind a phantom member record that later shows
        // up in worker_list and @mention resolution.
        let backend_type: BackendType = execution
            .adapter
            .clone()
            .ok_or_else(|| Error::Other("execution profile missing adapter".into()))?
            .parse()
            .map_err(|e: String| Error::Other(e))?;

        // Idempotent reuse: if the orchestrator already has a live session
        // for this spawn_key, don't try to re-spawn (which would error
        // "already registered"). Just report the current state.
        //
        // If the worker was registered but has died (process gone), we drop
        // the stale session entry now so the spawn path below can recreate
        // a fresh one. Without this cleanup `spawn_managed_member` rejects
        // any spawn_key already in its HashMap, so a `worker_add reuse` on
        // a dead worker would fail with "already registered" — exactly the
        // workflow the `[SYSTEM] worker died ... use on_existing=reuse to
        // restart` notice tells the lead to perform.
        let spawn_key_pre = spawn_key(&team_name, &worker_name);
        let (already_live, revived_from_dead) = self.async_runtime.block_on({
            let orch = Arc::clone(&self.runtime_orchestrator);
            let key = spawn_key_pre.clone();
            async move {
                let mut guard = orch.lock().await;
                let alive = guard.is_alive(&key).await.unwrap_or(false);
                let revived = if !alive && guard.has_session(&key) {
                    guard.remove_dead_session_if_any(&key).await
                } else {
                    false
                };
                (alive, revived)
            }
        });
        if matches!(mode, WorkerAddMode::Reuse) && already_live {
            // Make sure the on-disk session_state reflects reality and return.
            let _ = self.member_store.update(&team_name, &worker_name, |m| {
                if let Some(exec) = m.execution.as_mut() {
                    exec.session_state = Some(ExecutionSessionState::Running);
                }
            });
            let _ = self.runtime_workers.upsert_state(
                &team_name,
                &worker_name,
                &spawn_key_pre,
                execution.adapter.clone(),
                STATE_RUNNING,
                None,
            );
            return Ok(success_with_updates(
                json!({
                    "team": team_name.clone(),
                    "name": worker_name.clone(),
                    "sessionState": "running",
                    "mode": "reuse",
                }),
                vec![team_uri(&team_name), inbox_uri(&team_name, &worker_name)],
            ));
        }

        // Upsert identity + execution (only after validation and liveness check)
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
        let prompt = execution
            .system_prompt
            .clone()
            .unwrap_or_else(|| format!("You are {}", worker_name));
        let mut config = SpawnConfig::new(
            execution
                .agent_name
                .clone()
                .unwrap_or_else(|| worker_name.clone()),
            prompt,
        );
        config.model = execution.model.clone();
        config.cwd = execution.cwd.as_ref().map(PathBuf::from);
        config.env = execution.env.clone();
        config.reasoning_effort = Some("medium".into());

        let key = spawn_key(&team_name, &worker_name);
        self.runtime_workers.upsert_state(
            &team_name,
            &worker_name,
            &key,
            execution.adapter.clone(),
            STATE_STARTING,
            None,
        )?;
        let orch = Arc::clone(&self.runtime_orchestrator);
        let key_clone = key.clone();
        let wname = worker_name.clone();
        let handle = match self.async_runtime.block_on(async move {
            orch.lock()
                .await
                .spawn_managed_member(key_clone, wname, config, backend_type)
                .await
        }) {
            Ok(handle) => handle,
            Err(err) => {
                let _ = self.runtime_workers.upsert_state(
                    &team_name,
                    &worker_name,
                    &key,
                    execution.adapter.clone(),
                    STATE_FAILED,
                    Some(err.to_string()),
                );
                return Err(err);
            }
        };

        let orch2 = Arc::clone(&self.runtime_orchestrator);
        let key_for_rx = key.clone();
        let output_rx = self
            .async_runtime
            .block_on(async move { orch2.lock().await.take_output_receiver(&key_for_rx) })
            .ok()
            .flatten();

        // Ready-check: drain until first TurnComplete / Idle / Error / process exit.
        //
        // No timeout: different adapters have very different cold-start costs
        // (codex on Windows can take 10-15s, claude-code ~2s). Capping with
        // a fixed timeout caused the caller to receive `state="starting"` for
        // slow starters, which then never got updated to "running" because
        // nothing downstream transitions that state. Instead we wait for an
        // unambiguous terminal signal — success or explicit failure. If a
        // worker truly hangs forever, the caller can interrupt the MCP call.
        let (rx_for_loop, ready_state) = if let Some(mut rx) = output_rx {
            let (reported_state, remaining_rx) = self.async_runtime.block_on(async move {
                let result = async {
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
                }
                .await;
                let state = match result {
                    Ok(()) => "running",
                    Err(_) => "failed",
                };
                (state, rx)
            });

            if reported_state == "failed" {
                // Clean up: remove the profile we just wrote, reset status
                let _ = self.member_store.mark_removed(&team_name, &worker_name);
                let _ = self.runtime_workers.upsert_state(
                    &team_name,
                    &worker_name,
                    &key,
                    execution.adapter.clone(),
                    STATE_FAILED,
                    Some("agent process exited during startup".into()),
                );
                return Err(Error::Other(format!(
                    "worker '{worker_name}' failed to start"
                )));
            }

            (Some(remaining_rx), reported_state.to_string())
        } else {
            (None, handle.session_state.as_str().to_string())
        };

        // Persist runtime session_state to disk so worker_list reflects the live
        // status (otherwise it always reads back "not-spawned" from the initial
        // upsert done before spawn_managed_member).
        let persisted_state = match ready_state.as_str() {
            "running" => Some(ExecutionSessionState::Running),
            "starting" => Some(ExecutionSessionState::Starting),
            _ => None,
        };
        let session_id = self.async_runtime.block_on({
            let orch = Arc::clone(&self.runtime_orchestrator);
            let key = key.clone();
            async move { orch.lock().await.session_id_of(&key) }
        });
        if let Some(state) = persisted_state {
            let _ = self.member_store.update(&team_name, &worker_name, |m| {
                if let Some(exec) = m.execution.as_mut() {
                    exec.session_state = Some(state);
                    if session_id.is_some() {
                        exec.session_id = session_id.clone();
                    }
                }
            });
        }
        let _ = self.runtime_workers.upsert_state(
            &team_name,
            &worker_name,
            &key,
            execution.adapter.clone(),
            ready_state.as_str(),
            None,
        );

        if let Some(rx) = rx_for_loop {
            let agent_loop = AgentLoop {
                member_id: worker_name.clone(),
                session_key: key.clone(),
                team_id: team_name.clone(),
                room_id: "main".into(),
                orchestrator: Arc::clone(&self.runtime_orchestrator),
                inbox_service: self.inbox_service.clone(),
                message_store: self.message_store.clone(),
                message_service: self.message_service.clone(),
                poll_interval: Duration::from_secs(5),
                inbox_notifier: Some(self.inbox_notifier.clone()),
                member_store: Some(self.member_store.clone()),
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

        let mut payload = json!({
            "team": team_name.clone(),
            "name": worker_name.clone(),
            "sessionState": ready_state,
            "mode": mode_str,
        });
        if let Value::Object(map) = &mut payload {
            if revived_from_dead {
                map.insert("revived_from_dead".into(), Value::Bool(true));
                map.insert(
                    "note".into(),
                    Value::String(
                        "Previous worker process was dead — its stale session was \
                         dropped and a fresh process spawned. The worker has a new \
                         conversation context (no memory of prior turns)."
                            .into(),
                    ),
                );
            }
            // Reach out only on the create path: reuse already returns
            // because the worker has been observed before, so the lead
            // already knows the session_id timing trick.
            if matches!(mode, WorkerAddMode::Create) {
                map.insert(
                    "hint".into(),
                    Value::String(
                        "Worker process started. Its backend session_id is captured \
                         after the FIRST `type:result` event — i.e. once you send the \
                         first @mention message and the worker replies. Until then, \
                         the web UI 'process session' pane shows a placeholder for \
                         this worker."
                            .into(),
                    ),
                );
            }
        }

        Ok(success_with_updates(
            payload,
            vec![team_uri(&team_name), inbox_uri(&team_name, &worker_name)],
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
        let _ = self
            .async_runtime
            .block_on(async move { orch.lock().await.shutdown_managed_member(&key_clone).await });
        if let Some(h) = self.loop_handles.lock().unwrap().remove(&key) {
            let _ = h.shutdown_tx.send(());
        }
        let _ = self.runtime_workers.upsert_state(
            &team_name,
            &worker_name,
            &key,
            None,
            STATE_STOPPED,
            None,
        );

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
            vec![team_uri(&team_name), inbox_uri(&team_name, &worker_name)],
        ))
    }

    fn worker_list(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let members = self.member_service.list_active(&team_name)?;

        // Cross-reference stored session_state with actual process liveness
        // from the orchestrator. The stored state lags reality: if a worker
        // process crashes or is killed externally, the agent_loop detects
        // it and exits, but nothing updates member_store — so a subsequent
        // `worker_list` would still say "running". For the lead's benefit
        // (especially in long-running task coordination), expose the live
        // view: stored="running" but orchestrator says "not alive" -> "dead".
        let orch = Arc::clone(&self.runtime_orchestrator);
        let workers: Vec<Value> = members
            .into_iter()
            .filter(|r| !matches!(r.profile.kind, MemberKind::Lead))
            .map(|record| {
                let adapter = record
                    .execution
                    .as_ref()
                    .and_then(|e| e.adapter.clone())
                    .unwrap_or_default();
                let stored_state = record
                    .execution
                    .as_ref()
                    .and_then(|e| e.session_state.as_ref())
                    .map(|s| s.as_str())
                    .unwrap_or("not-spawned")
                    .to_string();

                // Only run the liveness query when the stored state CLAIMS
                // running — otherwise `not-spawned` / `failed` / `removed`
                // are already accurate and we save the orchestrator lock.
                let sidecar_state = self
                    .runtime_workers
                    .state_for(&team_name, &record.profile.name)
                    .ok()
                    .flatten();
                let session_state = if sidecar_state.as_deref() == Some(STATE_DEAD) {
                    STATE_DEAD.to_string()
                } else if stored_state == "running" {
                    let key = spawn_key(&team_name, &record.profile.name);
                    let orch = Arc::clone(&orch);
                    let alive = self.async_runtime.block_on(async move {
                        orch.lock().await.is_alive(&key).await.unwrap_or(false)
                    });
                    if alive {
                        STATE_RUNNING.to_string()
                    } else {
                        STATE_DEAD.to_string()
                    }
                } else {
                    sidecar_state.unwrap_or(stored_state)
                };

                json!({
                    "name": record.profile.name,
                    "adapter": adapter,
                    "sessionState": session_state,
                })
            })
            .collect();
        let dead_names: Vec<String> = workers
            .iter()
            .filter_map(|w| {
                let state = w.get("sessionState")?.as_str()?;
                if state == "dead" {
                    w.get("name")?.as_str().map(String::from)
                } else {
                    None
                }
            })
            .collect();
        let mut payload = json!({ "workers": workers });
        if !dead_names.is_empty() {
            if let Value::Object(map) = &mut payload {
                map.insert(
                    "hint".into(),
                    Value::String(format!(
                        "Dead workers found: [{}]. Revive each with \
                         `worker_add name=<x> on_existing=reuse`. The worker \
                         will lose its prior conversation context but its \
                         saved profile (adapter / model / system_prompt) \
                         is reused automatically.",
                        dead_names.join(", ")
                    )),
                );
            }
        }
        Ok(success(payload))
    }

    fn send_message(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let text = required_text(args, "text")?;

        let _team = self
            .team_service
            .get(&team_name)?
            .ok_or_else(|| Error::TeamNotFound {
                name: team_name.clone(),
            })?;

        // Mandatory: text must contain at least one @handle.
        let body_handles = extract_handles(&text);
        if body_handles.is_empty() {
            return Err(Error::Other(
                "send_message requires at least one @handle in `text` naming an active worker"
                    .into(),
            ));
        }

        // Build active worker set (excluding lead). Also build a lowercase
        // index so the mention parser can be case-insensitive while still
        // routing to the canonically-cased name on disk.
        let members = self.member_service.list_active(&team_name)?;
        let active_workers: Vec<String> = members
            .iter()
            .filter(|r| !matches!(r.profile.kind, MemberKind::Lead))
            .map(|r| r.profile.name.clone())
            .collect();
        let lc_index: HashMap<String, String> = active_workers
            .iter()
            .map(|n| (n.to_lowercase(), n.clone()))
            .collect();

        // Resolve @handles case-insensitively. Each user-visible handle in
        // `body_handles` becomes the on-disk worker name when matched, or
        // stays as the raw handle if unmatched (for the error message).
        let mut resolved: Vec<String> = Vec::new();
        let mut unmatched: Vec<String> = Vec::new();
        for h in &body_handles {
            match lc_index.get(&h.to_lowercase()) {
                Some(canonical) => {
                    if !resolved.iter().any(|r| r == canonical) {
                        resolved.push(canonical.clone());
                    }
                }
                None => {
                    if !unmatched.iter().any(|u| u == h) {
                        unmatched.push(h.clone());
                    }
                }
            }
        }
        if !unmatched.is_empty() {
            let mut active_sorted: Vec<_> = active_workers.iter().cloned().collect();
            active_sorted.sort();
            return Err(Error::Other(format!(
                "send_message: unmatched @mentions {:?}. Active workers in team '{}': {:?}. \
                 (Mention matching is case-insensitive; check spelling.)",
                unmatched, team_name, active_sorted
            )));
        }

        // Liveness pre-check: any recipient whose process is dead would
        // otherwise sit in their inbox forever (the agent_loop that would
        // post a [SYSTEM] death notice only exists while the worker is
        // alive, so a daemon-restart scenario silently strands the lead).
        // For each dead recipient we synthesize a [SYSTEM] Status reply
        // immediately, route it via lead-observability, and drop the
        // recipient from the dispatch.
        let room_id = "main".to_string();
        self.room_service.ensure_main_room(&team_name)?;

        let mut live_recipients: Vec<String> = Vec::new();
        let mut dead_recipients: Vec<String> = Vec::new();
        for recipient in &resolved {
            let key = spawn_key(&team_name, recipient);
            let alive = self.async_runtime.block_on({
                let orch = Arc::clone(&self.runtime_orchestrator);
                let key = key.clone();
                async move { orch.lock().await.is_alive(&key).await.unwrap_or(false) }
            });
            if alive {
                live_recipients.push(recipient.clone());
            } else {
                dead_recipients.push(recipient.clone());
            }
        }

        // Emit [SYSTEM] notice for each dead recipient up-front so the lead
        // does not wait on the Stop hook shepherd for a reply that will
        // never arrive.
        let mut system_notices: Vec<Value> = Vec::new();
        for dead in &dead_recipients {
            let notice = format!(
                "[SYSTEM] worker '{dead}' is not alive — message not delivered. \
                 Use `worker_add name={dead} on_existing=reuse` to spawn a fresh \
                 process (the worker will lose prior conversation context)."
            );
            let sys_msg = self.message_service.send(SendMessageRequest {
                team_id: team_name.clone(),
                room_id: room_id.clone(),
                sender: dead.clone(),
                kind: MessageKind::Status,
                subject: None,
                body: notice.clone(),
                mentions: Vec::new(),
                visibility: Vec::new(),
                audience_policy: None,
                reply_to: None,
                thread_id: None,
                expires_at: None,
            });
            match sys_msg {
                Ok(m) => system_notices.push(json!({
                    "worker": dead,
                    "message_id": m.id,
                    "text": notice,
                })),
                Err(err) => {
                    tracing::warn!(
                        worker = %dead,
                        error = %err,
                        "failed to write [SYSTEM] dead-worker notice"
                    );
                }
            }
        }

        if live_recipients.is_empty() {
            // All targeted workers were dead. Don't write a no-op dispatch
            // — return a structured error listing the dead names so the
            // tool caller sees the failure without scrolling through the
            // [SYSTEM] reply chain.
            return Err(Error::Other(format!(
                "send_message: all targeted workers are dead {:?} in team '{}'. \
                 [SYSTEM] notices have been posted to the lead inbox. \
                 Restart with `worker_add on_existing=reuse` before retrying.",
                dead_recipients, team_name
            )));
        }

        // Rewrite the body so the dispatch only carries live mentions. The
        // worker text routing already filters, but pruning here keeps the
        // visible body clean. We replace each dead @handle with [worker
        // unavailable: name] inline.
        let mut filtered_body = text.clone();
        for dead in &dead_recipients {
            // Try canonical case first, then the raw handle as it appeared.
            let pat = format!("@{dead}");
            filtered_body = filtered_body
                .replace(&pat, &format!("[worker unavailable: {dead}]"));
        }
        // Mentions for live recipients are already in `filtered_body`. The
        // message_service will re-parse and route to live ones only.

        let message = self.message_service.send(SendMessageRequest {
            team_id: team_name.clone(),
            room_id: room_id.clone(),
            sender: LEAD_NAME.into(),
            kind: MessageKind::Dispatch,
            subject: None,
            body: filtered_body,
            mentions: live_recipients.clone(),
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

        let mut payload = json!({
            "message": message,
            "matched_recipients": matched_recipients,
        });
        if let Value::Object(map) = &mut payload {
            // Always reinforce the "no polling" rule on every successful
            // send. The whole reason for putting it here (vs the static
            // tool description) is that this is the moment the model is
            // most tempted to follow up with a sleep / inbox_read loop.
            map.insert(
                "hint".into(),
                Value::String(
                    "Replies will arrive automatically as a <system-reminder> \
                     when your next turn starts. Do NOT call inbox_read or \
                     sleep — just end your turn and continue when reminded."
                        .into(),
                ),
            );
            if !dead_recipients.is_empty() {
                map.insert(
                    "dead_recipients".into(),
                    Value::Array(
                        dead_recipients
                            .iter()
                            .cloned()
                            .map(Value::String)
                            .collect::<Vec<_>>(),
                    ),
                );
                map.insert("system_notices".into(), Value::Array(system_notices));
                let dead_names = dead_recipients.join(", ");
                map.insert(
                    "dead_recipients_hint".into(),
                    Value::String(format!(
                        "Workers [{dead_names}] were skipped because their process is gone. \
                         Revive each with `worker_add name=<x> on_existing=reuse` (the worker \
                         loses prior conversation context) before retrying."
                    )),
                );
            }
        }

        Ok(success_with_updates(payload, updated))
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

        let inbox = self.inbox_service.peek(&team_name, LEAD_NAME, None)?;

        let mut items: Vec<_> = inbox
            .items
            .into_iter()
            .filter(|item| {
                if !unread_only {
                    return true;
                }
                !matches!(item.status, crate::team_mode::domain::InboxStatus::Acked)
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

        let mut payload = json!({
            "team": team_name,
            "lead": LEAD_NAME,
            "unread_count": unread_count,
            "total_returned": messages_out.len(),
            "messages": messages_out,
        });
        // Inbox is a fallback channel — when it returns nothing, surface a
        // hint so the model doesn't fall into a poll loop. The Stop hook
        // delivers replies as `<system-reminder>` automatically; calling
        // this tool without backlog-checking intent is wasted work.
        if messages_out.is_empty() {
            if let Value::Object(map) = &mut payload {
                map.insert(
                    "hint".into(),
                    Value::String(
                        "No messages in inbox. Worker replies arrive automatically \
                         via the Stop hook on your next turn — calling inbox_read \
                         is rarely needed; only useful for explicit backlog audits."
                            .into(),
                    ),
                );
            }
        }
        Ok(success(payload))
    }

    fn member_store_members_file_hint(&self, team_name: &str) -> String {
        crate::team_mode::data_dir::members_file(&self.base_dir_of(), team_name)
            .to_string_lossy()
            .to_string()
    }

    fn base_dir_of(&self) -> PathBuf {
        self.base_dir.clone()
    }
}

// ---------------------------------------------------------------------------
// Team Mode Web auto-open
// ---------------------------------------------------------------------------

const DEFAULT_WEB_HOST: &str = "127.0.0.1";
const DEFAULT_WEB_PORT: u16 = 8787;
const MAX_WEB_PORT: u16 = 8799;

#[derive(Debug, Clone)]
struct TeamWebStatus {
    enabled: bool,
    url: Option<String>,
    opened: bool,
    error: Option<String>,
}

impl TeamWebStatus {
    fn disabled(reason: impl Into<String>) -> Self {
        Self {
            enabled: false,
            url: None,
            opened: false,
            error: Some(reason.into()),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "url": self.url,
            "opened": self.opened,
            "error": self.error,
        })
    }
}

static WEB_SERVER_URL: OnceLock<String> = OnceLock::new();

fn open_team_web_ui(base_dir: &std::path::Path, team_id: &str) -> TeamWebStatus {
    if let Some(reason) = web_auto_open_disabled_reason() {
        return TeamWebStatus::disabled(reason);
    }

    let base_url = match ensure_team_web_server(base_dir) {
        Ok(url) => url,
        Err(err) => {
            return TeamWebStatus {
                enabled: true,
                url: None,
                opened: false,
                error: Some(err),
            };
        }
    };
    let url = format!("{base_url}/#team={team_id}");

    match open_url_in_browser(&url) {
        Ok(()) => TeamWebStatus {
            enabled: true,
            url: Some(url),
            opened: true,
            error: None,
        },
        Err(err) => TeamWebStatus {
            enabled: true,
            url: Some(url),
            opened: false,
            error: Some(err),
        },
    }
}

fn web_auto_open_disabled_reason() -> Option<String> {
    match std::env::var("TEAM_MODE_WEB_AUTO_OPEN") {
        Ok(value) if matches!(value.as_str(), "0" | "false" | "FALSE" | "off" | "OFF") => {
            return Some("TEAM_MODE_WEB_AUTO_OPEN disabled".into());
        }
        _ => {}
    }

    if std::env::var_os("CI").is_some() || looks_like_cargo_test_process() {
        return Some("disabled in test/CI process".into());
    }

    None
}

fn looks_like_cargo_test_process() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let in_deps_dir = exe
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("deps");
    let has_cargo_test_hash = exe
        .file_stem()
        .and_then(|name| name.to_str())
        .map(|name| name.rsplit_once('-').is_some())
        .unwrap_or(false);
    in_deps_dir && has_cargo_test_hash
}

/// Public wrapper so the daemon binary can pre-spawn the web server at
/// startup — otherwise the web server only exists inside the ephemeral
/// worker thread spawned during `team_create`. When the daemon restarts,
/// any previously-opened browser tab has stale API URLs and sees
/// ERR_CONNECTION_REFUSED until the user runs another team_create.
pub fn ensure_team_web_server_public(
    base_dir: &std::path::Path,
) -> std::result::Result<String, String> {
    ensure_team_web_server(base_dir)
}

fn ensure_team_web_server(base_dir: &std::path::Path) -> std::result::Result<String, String> {
    if let Some(url) = WEB_SERVER_URL.get() {
        return Ok(url.clone());
    }

    let mut last_error = None;
    for port in DEFAULT_WEB_PORT..=MAX_WEB_PORT {
        let addr = format!("{DEFAULT_WEB_HOST}:{port}");
        match std::net::TcpListener::bind(&addr) {
            Ok(listener) => {
                let url = format!("http://{addr}");
                let base_dir = base_dir.to_path_buf();
                std::thread::Builder::new()
                    .name("team-mode-web".into())
                    .spawn(move || {
                        if let Err(err) = crate::team_mode_web::serve_listener(base_dir, listener) {
                            tracing::warn!(error = %err, "team_mode_web server exited");
                        }
                    })
                    .map_err(|err| format!("failed to spawn team_mode_web thread: {err}"))?;
                let _ = WEB_SERVER_URL.set(url.clone());
                return Ok(url);
            }
            Err(err) => {
                last_error = Some(format!("{addr}: {err}"));
            }
        }
    }

    Err(format!(
        "could not bind Team Mode Web on ports {DEFAULT_WEB_PORT}-{MAX_WEB_PORT}: {}",
        last_error.unwrap_or_else(|| "no bind attempt completed".into())
    ))
}

fn open_url_in_browser(url: &str) -> std::result::Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("failed to open browser: {err}"))
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

/// Return the PID of the process that spawned this MCP server — in the
/// Claude Code host setup, that's the CC client itself. Used to bind
/// team ownership at team_create time so push-routing knows which CC
/// the `lead_pending.jsonl` entry is addressed to.
fn current_parent_pid() -> Option<u32> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    let me = Pid::from_u32(std::process::id());
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[me]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(me)
        .and_then(|p| p.parent())
        .map(|ppid| ppid.as_u32())
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
        Some(Value::Number(n)) => {
            n.as_u64().map(|v| v as usize).map(Some).ok_or_else(|| {
                Error::Other(format!("field '{key}' must be a non-negative integer"))
            })
        }
        Some(_) => Err(Error::Other(format!(
            "field '{key}' must be a non-negative integer"
        ))),
    }
}

fn optional_u32(args: &Map<String, Value>, key: &str) -> Result<Option<u32>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            let value = n.as_u64().ok_or_else(|| {
                Error::Other(format!("field '{key}' must be a non-negative integer"))
            })?;
            u32::try_from(value)
                .map(Some)
                .map_err(|_| Error::Other(format!("field '{key}' is too large for u32")))
        }
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
        let starts_mention = chars[i] == '@' && (i == 0 || !is_handle_char(chars[i - 1]));
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
            "team_create",
            "team_list",
            "team_delete",
            "worker_add",
            "worker_list",
            "worker_remove",
            "send_message",
            "inbox_read",
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
        tools
            .call_tool("team_create", Some(json!({"name": "demo"})))
            .unwrap();

        let err = tools
            .call_tool(
                "send_message",
                Some(json!({
                    "team": "demo",
                    "text": "no mention here",
                })),
            )
            .unwrap_err();
        assert!(matches!(&err, Error::Other(msg) if msg.contains("@handle")));
    }

    #[test]
    fn send_message_rejects_any_unmatched_mention() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools
            .call_tool("team_create", Some(json!({"name": "demo"})))
            .unwrap();

        // Even if alice doesn't exist yet, @typo must fail.
        let err = tools
            .call_tool(
                "send_message",
                Some(json!({
                    "team": "demo",
                    "text": "@typo please",
                })),
            )
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("unmatched"), "got: {msg}");
        assert!(msg.contains("typo"), "got: {msg}");
    }

    #[test]
    fn worker_remove_refuses_lead() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools
            .call_tool("team_create", Some(json!({"name": "demo"})))
            .unwrap();

        let err = tools
            .call_tool(
                "worker_remove",
                Some(json!({"team": "demo", "name": "lead"})),
            )
            .unwrap_err();
        assert!(matches!(&err, Error::Other(msg) if msg.contains("lead")));
    }

    #[test]
    fn worker_add_refuses_reserved_name() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools
            .call_tool("team_create", Some(json!({"name": "demo"})))
            .unwrap();

        let err = tools
            .call_tool(
                "worker_add",
                Some(json!({
                    "team": "demo",
                    "name": "lead",
                    "adapter": "claude-code",
                })),
            )
            .unwrap_err();
        assert!(matches!(&err, Error::Other(msg) if msg.contains("reserved")));
    }

    #[test]
    fn worker_list_excludes_the_lead() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools
            .call_tool("team_create", Some(json!({"name": "demo"})))
            .unwrap();

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
        tools
            .call_tool("team_create", Some(json!({"name": "demo"})))
            .unwrap();

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
