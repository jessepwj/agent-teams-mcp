use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::runtime::{Builder as TokioRuntimeBuilder, Runtime as TokioRuntime};

use crate::RuntimeOrchestrator;
use crate::backend::claude_code::ClaudeCodeBackend;
use crate::backend::codex::CodexBackend;
use crate::backend::gemini::GeminiCliBackend;
use crate::error::{Error, Result};
use crate::team_mode::domain::{MemberKind, MemberStatus, MessageKind};
use crate::team_mode::mcp::resources::{inbox_uri, team_uri};
use crate::team_mode::mcp::schemas::{ToolCallResult, ToolDescriptor, empty_object_schema};
use crate::team_mode::runtime_workers::{RuntimeWorkerStore, STATE_STOPPED};
use crate::team_mode::service::{
    AddMemberRequest, CreateTeamRequest, InboxNotifier, InboxService, LeadPendingWriter,
    MemberService, MessageService, RoomService, TeamService,
};
use crate::team_mode::storage::{MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore};
use crate::util::validate_name;

mod message;
mod worker;

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
        // Hand the fully-wired service to the web layer so user-initiated
        // sends from the browser fire the same notifier + lead-pending writes
        // as MCP-tool sends. Both sides hit the exact same routing path.
        crate::team_mode_web::install_shared_message_service(message_service.clone());
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
            tool("team_list", "List teams.", empty_object_schema()),
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
    fn send_message_unmatched_lists_available_handles_with_lead() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools
            .call_tool("team_create", Some(json!({"name": "demo"})))
            .unwrap();

        // Bug 29: error must point the model at @lead so a confused
        // caller can fall back to addressing the lead instead of guessing.
        let err = tools
            .call_tool(
                "send_message",
                Some(json!({
                    "team": "demo",
                    "text": "@nope please",
                })),
            )
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("@lead"),
            "error should suggest @lead as a valid handle, got: {msg}"
        );
    }

    #[test]
    fn send_message_lead_no_mention_lists_available_handles() {
        let dir = tempdir().unwrap();
        let tools = TeamModeToolset::new(dir.path());
        tools
            .call_tool("team_create", Some(json!({"name": "demo"})))
            .unwrap();

        // Lead with no @mention: error must include the available handles
        // (so the LLM can self-correct without scrolling for `worker_list`).
        let err = tools
            .call_tool(
                "send_message",
                Some(json!({
                    "team": "demo",
                    "text": "no mention here",
                    "_caller_member": "lead",
                })),
            )
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("@handle"), "got: {msg}");
        assert!(msg.contains("Active recipients"), "got: {msg}");
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
