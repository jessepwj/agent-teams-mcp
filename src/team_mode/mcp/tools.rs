use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::runtime::{Builder as TokioRuntimeBuilder, Runtime as TokioRuntime};

use crate::RuntimeOrchestrator;
use crate::backend::claude_code::ClaudeCodeBackend;
use crate::backend::codex::CodexBackend;
use crate::backend::gemini::GeminiCliBackend;
use crate::error::{Error, Result};
use crate::team_mode::data_dir;
use crate::team_mode::domain::MessageKind;
use crate::team_mode::mcp::resources::{inbox_uri, team_uri};
use crate::team_mode::mcp::schemas::{ToolCallResult, ToolDescriptor, empty_object_schema};
use crate::team_mode::runtime_workers::{RuntimeWorkerStore, STATE_STOPPED};
use crate::team_mode::service::{
    InboxNotifier, InboxService, LeadPendingWriter, MemberService, MessageService, RoomService,
    TeamService,
};
use crate::team_mode::storage::{MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore};
use crate::util::{current_cc_pid, validate_name};

mod mention;
mod message;
mod team_lifecycle;
#[cfg(test)]
mod tests;
mod web_open;
mod worker;

use mention::extract_dispatch_handles;
pub use web_open::ensure_team_web_server_public;

/// Per-team lead is always a virtual member with this name.
const LEAD_NAME: &str = "lead";
const LEAD_WATCH_GRACE_CHECKS: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    pub result: ToolCallResult,
    pub updated_resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamDeletionResult {
    pub archived: bool,
    pub deleted: bool,
    pub shutdown_failures: Vec<Value>,
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
    runtime_orchestrator: Arc<tokio::sync::Mutex<RuntimeOrchestrator>>,
    async_runtime: Arc<TokioRuntime>,
    loop_handles: Arc<Mutex<HashMap<String, crate::runtime::AgentLoopHandle>>>,
}

struct ToolsetServices {
    team_service: TeamService,
    member_service: MemberService,
    member_store: MemberStore,
    room_service: RoomService,
    message_store: MessageStore,
    message_service: MessageService,
    inbox_service: InboxService,
    runtime_workers: RuntimeWorkerStore,
}

impl TeamModeToolset {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let project_root = std::env::current_dir().ok();
        Self::new_with_project_root(base_dir, project_root)
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        Self::new_with_project_root(base_dir.clone(), Some(base_dir))
    }

    pub fn new_with_project_root(
        base_dir: impl Into<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Self {
        let base_dir = base_dir.into();

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

        let inbox_notifier = InboxNotifier::new();
        let services =
            Self::build_services(base_dir.clone(), project_root, inbox_notifier.clone(), true);
        // Hand the fully-wired service to the web layer so user-initiated
        // sends from the browser fire the same notifier + lead-pending writes
        // as MCP-tool sends. Both sides hit the exact same routing path.
        crate::team_mode_web::install_shared_message_service(services.message_service.clone());

        Self {
            base_dir,
            team_service: services.team_service,
            member_service: services.member_service,
            member_store: services.member_store,
            room_service: services.room_service,
            message_store: services.message_store,
            message_service: services.message_service,
            inbox_service: services.inbox_service,
            inbox_notifier,
            runtime_workers: services.runtime_workers,
            runtime_orchestrator: Arc::new(tokio::sync::Mutex::new(runtime_orchestrator)),
            async_runtime: Arc::new(
                TokioRuntimeBuilder::new_multi_thread()
                    .worker_threads(4)
                    .enable_all()
                    .build()
                    .expect("failed to create Team Mode MCP tokio runtime"),
            ),
            loop_handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn build_services(
        base_dir: PathBuf,
        project_root: Option<PathBuf>,
        inbox_notifier: InboxNotifier,
        migrate_legacy: bool,
    ) -> ToolsetServices {
        let team_store = TeamStore::new(base_dir.clone());
        let member_store = MemberStore::new(base_dir.clone());
        let room_store = RoomStore::new(base_dir.clone());
        let message_store = MessageStore::new(base_dir.clone());
        let projection_store = ProjectionStore::new(base_dir.clone());
        // Per-team pending files live under `<base_dir>/<team_id>/` (the
        // team data dir). The hook script polls only files belonging to
        // teams it owns (via GET /lead-pending/my-teams) and drains them
        // with atomic rename, so the old MCP-cwd single-file path is only
        // a legacy migration source.
        let mut writer = LeadPendingWriter::new(base_dir.clone());
        // Pre-2026-04-30 the single-file `lead_pending.jsonl` lived at the
        // project root (repo cwd) because the previous design relied on
        // FileChanged-hook matcher rules. Tell the migrator to scan that
        // location too so old data isn't orphaned.
        if let Some(root) = project_root.clone() {
            writer = writer.with_legacy_root(root);
        }
        let lead_pending_writer = writer;
        if migrate_legacy {
            match lead_pending_writer.migrate_legacy() {
                Ok(0) => {}
                Ok(n) => tracing::info!(
                    event = "lead_pending.startup_migration",
                    migrated = n,
                    "migrated legacy lead_pending.jsonl into per-team files"
                ),
                Err(err) => tracing::warn!(
                    event = "lead_pending.startup_migration_failed",
                    error = %err
                ),
            }
        }

        let team_service = TeamService::new(team_store.clone());
        let member_service = MemberService::new(member_store.clone(), team_store.clone());
        let room_service = RoomService::new(room_store.clone());
        let message_service = MessageService::new(
            message_store.clone(),
            member_store.clone(),
            room_store.clone(),
            team_store,
        )
        .with_inbox_notifier(inbox_notifier.clone())
        .with_lead_pending_writer(lead_pending_writer);
        let inbox_service = InboxService::new(projection_store, message_store.clone());
        let runtime_workers = RuntimeWorkerStore::new(base_dir);

        ToolsetServices {
            team_service,
            member_service,
            member_store,
            room_service,
            message_store,
            message_service,
            inbox_service,
            runtime_workers,
        }
    }

    fn scoped_to_project_root(&self, project_root: PathBuf) -> Self {
        let base_dir = data_dir::base_dir_for_project_root(&project_root);
        let services = Self::build_services(
            base_dir.clone(),
            Some(project_root),
            self.inbox_notifier.clone(),
            false,
        );
        Self {
            base_dir,
            team_service: services.team_service,
            member_service: services.member_service,
            member_store: services.member_store,
            room_service: services.room_service,
            message_store: services.message_store,
            message_service: services.message_service,
            inbox_service: services.inbox_service,
            inbox_notifier: self.inbox_notifier.clone(),
            runtime_workers: services.runtime_workers,
            runtime_orchestrator: Arc::clone(&self.runtime_orchestrator),
            async_runtime: Arc::clone(&self.async_runtime),
            loop_handles: Arc::clone(&self.loop_handles),
        }
    }

    /// Sweep sidecar workers marked RUNNING/STARTING and reconcile against
    /// the orchestrator's live process view. Workers whose backing process
    /// is gone (killed externally, OOM, crashed) get their sidecar state
    /// flipped to DEAD, with a note explaining the source of truth shift.
    ///
    /// Called periodically by the daemon's worker-liveness watchdog so the
    /// web UI (which reads only sidecar) sees real-time dead status without
    /// needing a worker_list MCP roundtrip.
    ///
    /// Returns the number of records flipped to DEAD on this tick.
    pub fn worker_liveness_tick(&self) -> usize {
        let workers = match self.runtime_workers.list_all() {
            Ok(workers) => workers,
            Err(err) => {
                tracing::warn!(error = %err, "worker-liveness watchdog: list_all failed");
                return 0;
            }
        };
        let mut flipped = 0;
        for worker in workers {
            // Only check workers we expect to be alive — STARTING (mid-spawn)
            // and RUNNING. STOPPED / DEAD / FAILED are already terminal
            // states; FAILED in particular means the worker never reached
            // running, no point probing.
            let claims_alive = matches!(
                worker.state.as_str(),
                crate::team_mode::runtime_workers::STATE_RUNNING
                    | crate::team_mode::runtime_workers::STATE_STARTING
            );
            if !claims_alive {
                continue;
            }
            let key = worker.spawn_key.clone();
            let orch = Arc::clone(&self.runtime_orchestrator);
            let (alive, stderr_log_hint) = self.async_runtime.block_on(async move {
                let guard = orch.lock().await;
                let alive = guard.is_alive(&key).await.unwrap_or(false);
                let stderr_log_hint = guard.stderr_log_hint_of(&key);
                (alive, stderr_log_hint)
            });
            if alive {
                continue;
            }
            let note = match stderr_log_hint {
                Some(ref hint) if !hint.is_empty() => {
                    tracing::warn!(
                        event = "codex_worker.stderr_tail",
                        team = %worker.team,
                        name = %worker.name,
                        stderr_log = %hint,
                        "worker-liveness watchdog: stderr captured locally before marking DEAD"
                    );
                    format!(
                        "liveness watchdog: orchestrator reported process gone (stderr captured locally at {hint})"
                    )
                }
                _ => "liveness watchdog: orchestrator reported process gone".into(),
            };
            // Process is gone — flip the sidecar. We deliberately keep the
            // member-store record (worker_remove handles full removal); this
            // is purely the runtime liveness sidecar.
            if let Err(err) = self.runtime_workers.upsert_state(
                &worker.team,
                &worker.name,
                &worker.spawn_key,
                worker.adapter.clone(),
                crate::team_mode::runtime_workers::STATE_DEAD,
                Some(note),
            ) {
                tracing::warn!(
                    team = %worker.team,
                    name = %worker.name,
                    error = %err,
                    "worker-liveness watchdog: failed to flip state to DEAD"
                );
                continue;
            }
            tracing::info!(
                team = %worker.team,
                name = %worker.name,
                "worker-liveness watchdog: flipped sidecar to DEAD (process gone)"
            );
            flipped += 1;
        }
        flipped
    }

    pub fn lead_watchdog_tick(&self, dead_strikes: &mut HashMap<String, u32>) -> usize {
        use std::collections::HashSet;
        use sysinfo::{Pid, ProcessesToUpdate, System};

        let teams = match self.team_service.list() {
            Ok(teams) => teams,
            Err(err) => {
                tracing::warn!(error = %err, "lead-watchdog: list failed");
                return 0;
            }
        };

        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::All, true);

        let known_team_ids: HashSet<String> = teams.iter().map(|team| team.id.clone()).collect();
        dead_strikes.retain(|team_id, _| known_team_ids.contains(team_id));

        let mut archived = 0;
        for team in teams {
            if matches!(team.status, crate::team_mode::domain::TeamStatus::Archived) {
                dead_strikes.remove(&team.id);
                continue;
            }
            let Some(owner_cc_pid) = team.owner_cc_pid else {
                dead_strikes.remove(&team.id);
                continue;
            };
            if sys.process(Pid::from_u32(owner_cc_pid)).is_some() {
                dead_strikes.remove(&team.id);
                continue;
            }

            let strikes = dead_strikes.entry(team.id.clone()).or_insert(0);
            *strikes += 1;
            tracing::debug!(
                event = "lead_watchdog.observation",
                team = %team.name,
                team_id = %team.id,
                owner_cc_pid,
                consecutive_dead = *strikes,
                grace = LEAD_WATCH_GRACE_CHECKS,
                "lead-watchdog: owner CC missing"
            );
            if *strikes < LEAD_WATCH_GRACE_CHECKS {
                continue;
            }
            dead_strikes.remove(&team.id);

            match self.delete_team_with_cleanup(&team.id, false) {
                Ok(result) => {
                    if !result.shutdown_failures.is_empty() {
                        tracing::warn!(
                            event = "team.auto_archived_dead_owner.shutdown_failures",
                            team = %team.name,
                            team_id = %team.id,
                            owner_cc_pid,
                            shutdown_failures = %result.shutdown_failures.len(),
                            "lead-watchdog: archived dead-owner team with shutdown failures"
                        );
                    }
                    tracing::info!(
                        event = "team.auto_archived_dead_owner",
                        team = %team.name,
                        team_id = %team.id,
                        owner_cc_pid,
                        shutdown_failures = %result.shutdown_failures.len(),
                        "lead-watchdog: auto-archived dead-owner team"
                    );
                    archived += 1;
                }
                Err(err) => {
                    tracing::warn!(
                        event = "team.auto_archived_dead_owner.error",
                        team = %team.name,
                        team_id = %team.id,
                        owner_cc_pid,
                        error = %err,
                        "lead-watchdog: failed to auto-archive dead-owner team"
                    );
                }
            }
        }

        archived
    }

    pub(super) fn lock_loop_handles(
        &self,
    ) -> Result<MutexGuard<'_, HashMap<String, crate::runtime::AgentLoopHandle>>> {
        self.loop_handles
            .lock()
            .map_err(|_| Error::Other("poisoned mutex: MCP loop_handles".into()))
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
                "WARNING: irreversibly removes existing team data when overwrite=true.",
                json_schema(
                    &[
                        ("name", json!({"type":"string"})),
                        ("cwd", json!({"type":"string"})),
                        ("overwrite", json!({"type":"boolean"})),
                    ],
                    &["name"],
                ),
            ),
            tool("team_list", "List teams.", "", empty_object_schema()),
            tool(
                "team_delete",
                "Delete a team and stop all its workers.",
                "Defaults to archive; set permanent=true to irreversibly remove existing team data.",
                json_schema(
                    &[
                        ("name", json!({"type":"string"})),
                        ("permanent", json!({"type":"boolean"})),
                    ],
                    &["name"],
                ),
            ),
            tool(
                "worker_add",
                "Add a worker to a team and start its process. \
                 Pass `adapter` on first add. If a saved profile for this name \
                 already exists, also pass `on_existing` to choose what to do.",
                "",
                json_schema(
                    &[
                        ("team", json!({"type":"string"})),
                        ("name", json!({"type":"string"})),
                        (
                            "adapter",
                            json!({"type":"string","enum":["claude-code","codex","gemini-cli"]}),
                        ),
                        ("model", json!({"type":"string"})),
                        // Reasoning effort override (codex only today). Omit
                        // to inherit the user's global config, e.g.
                        // `~/.codex/config.toml`'s `model_reasoning_effort`.
                        (
                            "effort",
                            json!({"type":"string","enum":["low","medium","high","xhigh"]}),
                        ),
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
                "",
                json_schema(&[("team", json!({"type":"string"}))], &["team"]),
            ),
            tool(
                "worker_remove",
                "Stop a worker's process. The on-disk profile is kept so it can be revived later with `worker_add on_existing=reuse`.",
                "",
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
                "Send a message into the team room. `text` must contain at least one \
                 @handle naming the recipient(s) (e.g. `@alice please review`, `@lead done`). \
                 \n\nWORKER USAGE — IMPORTANT: if you are a worker (e.g. alice, backend-dev, \
                 probe), this is the ONLY way to reply to lead or any other teammate. \
                 Ending your turn without calling send_message means NO reply is delivered — \
                 the lead's next turn will see only an `[INFO] worker completed turn without \
                 a routed team message` status, never your actual answer. Even if you wrote \
                 your reply as plain text in your turn output, lead cannot see it; you must \
                 call send_message with text=`@lead <your reply>` before ending the turn. \
                 \n\nLEAD USAGE: dispatch tasks to workers; replies arrive automatically as a \
                 system-reminder when your next turn starts — do not poll or sleep waiting for \
                 them. Set `preempt=true` to abort the recipient's in-flight turn so the new \
                 message is processed immediately (lead-only; no-op if recipient is idle; \
                 messages are always enqueued regardless of interrupt outcome).",
                "",
                json_schema(
                    &[
                        ("team", json!({"type":"string"})),
                        ("text", json!({"type":"string"})),
                        (
                            "mentions",
                            json!({"type":"array","items":{"type":"string"}}),
                        ),
                        ("preempt", json!({"type":"boolean"})),
                    ],
                    &["team", "text"],
                ),
            ),
            tool(
                "inbox_read",
                "Read the lead's inbox. Replies normally arrive automatically \
                 via the Stop hook; this tool is a fallback for explicit backlog \
                 checks. `auto_ack=true` marks returned messages as read.",
                "",
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
        if let Some(project_root) = optional_text(&args, "_project_root")? {
            return self
                .scoped_to_project_root(PathBuf::from(project_root))
                .call_tool_scoped(name, args);
        }
        self.call_tool_scoped(name, args)
    }

    fn call_tool_scoped(&self, name: &str, args: Map<String, Value>) -> Result<ToolExecution> {
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

fn tool(name: &str, description: &str, extra_note: &str, input_schema: Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: if extra_note.is_empty() {
            description.into()
        } else {
            format!("{description} {extra_note}")
        },
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

fn optional_identifier_list(args: &Map<String, Value>, key: &str) -> Result<Option<Vec<String>>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(values)) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let Some(handle) = value.as_str() else {
                    return Err(Error::Other(format!(
                        "field '{key}' must be an array of strings"
                    )));
                };
                validate_name(handle)?;
                if !out.iter().any(|existing| existing == handle) {
                    out.push(handle.to_string());
                }
            }
            Ok(Some(out))
        }
        Some(_) => Err(Error::Other(format!(
            "field '{key}' must be an array of strings"
        ))),
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
