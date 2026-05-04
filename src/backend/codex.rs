//! Codex backend -- spawns agents via the `codex app-server` stdio protocol.
//!
//! The Codex app-server uses a JSON-RPC-like protocol (without the `jsonrpc` field)
//! over stdin/stdout with newline-delimited JSON messages.
//!
//! The lifecycle of a Codex session:
//!
//! 1. Spawn `codex app-server` as a child process.
//! 2. Send `initialize` with `{clientInfo: {name, version}}` → receive `{userAgent}`.
//! 3. Send `initialized` notification (no `id`).
//! 4. Send `thread/start` with `{cwd, approvalPolicy}` → receive `{thread: {id, ...}}`.
//! 5. Emit a synthetic [`AgentOutput::Idle`] so the AgentLoop's ready-check
//!    completes without consuming an LLM turn — Codex has no `--system-prompt`
//!    flag, so any system instruction must ride on the first real user message.
//! 6. Background tasks read stdout/stderr line-by-line, parsing stdout
//!    messages into [`AgentOutput`] events while retaining stderr for
//!    diagnostics.
//! 7. The first `send_input` call prepends the stashed system prompt as
//!    `[System instructions]\n...\n\n---\n\n<user input>`, then dispatches
//!    `turn/start`. Subsequent inputs go through unchanged.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use super::codex_protocol::*;
use super::codex_stderr::{
    CODEX_STDERR_LOG_ENV, CodexStderrRing, STDERR_TAIL_LINES, open_stderr_log, redact_stderr_line,
};
use super::{
    AgentBackend, AgentOutput, AgentSession, BackendType, SpawnConfig, apply_hide_window,
    send_agent_output,
};
use crate::{Error, Result};

/// Channel buffer size for agent output events.
/// Capacity of the codex→agent_loop output channel. Must be large enough
/// to absorb a single bursty turn's full event stream without blocking the
/// reader, otherwise codex's stdout pump backs up, the JSON-RPC keepalive
/// stalls, and the worker is reported as `agent error`.
///
/// 256 was empirically too small: a `cargo build` worker turn emits
/// hundreds of `command/exec_command_output_delta` events per second on
/// large compile outputs, the channel fills, the reader logs
/// `Output channel full, dropping data message`, and the turn errors out.
/// 4096 is an order of magnitude past worst-case `cargo build` measured on
/// this repo (still bounded — runaway loops can't OOM the daemon — just
/// wide enough to absorb normal builds).
const OUTPUT_CHANNEL_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// CodexBackend  (factory)
// ---------------------------------------------------------------------------

/// Factory that creates Codex agent sessions by spawning a Codex subprocess.
#[derive(Debug, Clone)]
pub struct CodexBackend {
    /// Path to the `codex` CLI binary.
    codex_path: PathBuf,
}

impl CodexBackend {
    /// Locate the `codex` binary on `$PATH` via `which`.
    pub fn new() -> Result<Self> {
        let path = which::which("codex").map_err(|_| Error::CliNotFound {
            name: "codex".into(),
        })?;
        Ok(Self { codex_path: path })
    }

    /// Use an explicit path to the `codex` binary.
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            codex_path: path.into(),
        }
    }

    /// Spawn the Codex child process.
    fn spawn_child(&self, config: &SpawnConfig) -> Result<Child> {
        // Codex MCP registration via user's GLOBAL ~/.codex/config.toml.
        //
        // Why not CODEX_HOME isolation: codex auth lives under CODEX_HOME
        // and uses single-use refresh tokens (openai/codex#15410), so
        // copying auth.json into per-worker homes breaks on first refresh.
        //
        // Why not project-level `<cwd>/.codex/config.toml`: MCP servers
        // in project config are silently ignored by codex
        // (openai/codex#13025).
        //
        // Why not `-c mcp_servers.X.*` inline flags for managed workers:
        // historical codex versions ignored stdio MCP blocks supplied this
        // way. The global managed block has been the reliable path.
        //
        // What works: write a managed block into the user's global
        // `~/.codex/config.toml` (idempotent — the block is bracketed
        // by markers so it can be safely re-written or removed). codex
        // reads this on startup. For the HTTP service, `env_http_headers`
        // lets each worker supply its own team/member/worker id, while
        // `bearer_token_env_var` supplies the service token at spawn time.
        if let Err(e) = ensure_global_codex_mcp_config() {
            tracing::warn!(
                agent = %config.name,
                error = %e,
                "ensure_global_codex_mcp_config failed — codex worker may not see team-mode MCP server"
            );
        }

        let mut cmd = Command::new(&self.codex_path);
        // `codex app-server` reads JSON from stdin, writes JSON to stdout.
        // No additional flags needed -- stdio is the default transport.
        cmd.arg("app-server");

        // Pass model override via -c config flag
        if let Some(ref model) = config.model {
            cmd.arg("-c").arg(format!("model=\"{model}\""));
        }

        // Pass reasoning effort override via -c config flag.
        //
        // Project default is `high`: this codebase is large enough (Rust
        // workspace + Web UI + Hook scripts) that medium-effort codex turns
        // routinely under-explore the call graph and miss cross-file
        // invariants. High-effort spends more API budget but gives reliable
        // first-shot answers, which is net cheaper than re-prompting.
        //
        // Caller can still override via `worker_add(effort=...)` when a
        // specific worker should run cheaper (e.g. e2e-tester sweeps,
        // pure stylistic reviewer passes). Only when the caller leaves
        // it unset do we apply the project default.
        let effort = config.reasoning_effort.as_deref().unwrap_or("high");
        cmd.arg("-c")
            .arg(format!("model_reasoning_effort=\"{effort}\""));

        // Force full-access sandbox for managed workers.
        //
        // Why hardcode (consistent with `--permission-mode bypassPermissions`
        // on claude-code): this is the headless app-server transport. There
        // is no operator at a terminal who can react to a sandbox denial.
        // If the user's `~/.codex/config.toml` left `sandbox_mode` at
        // `read-only` or `workspace-write`, every shell/file tool the agent
        // tries silently gets blocked and the worker stalls. Passing the
        // override at spawn keeps each managed worker on equal footing
        // regardless of global config — a non-negotiable protocol invariant
        // for non-interactive agents, not a user preference.
        //
        // This pairs with `approvalPolicy: "never"` on `thread/start` (set
        // below in the spawn flow) to give the worker the same "act, don't
        // ask" semantics that `bypassPermissions` gives the claude-code
        // backend.
        cmd.arg("-c").arg("sandbox_mode=\"danger-full-access\"");

        // Note on env inheritance: codex's default `shell_environment_policy
        // .inherit = "core"` strips MSVC build vars (LIB / INCLUDE) needed
        // by `cargo build` inside worker shells. We do NOT override this via
        // a `-c` flag here because passing the literal string
        // `shell_environment_policy.inherit="all"` triggers an AgentError on
        // codex 0.124-alpha (TOML parse path differs from config.toml).
        //
        // Users who need worker `cargo build` should set the override in
        // their `~/.codex/config.toml`:
        //   [shell_environment_policy]
        //   inherit = "all"
        // and start the Team Mode service from an environment where vcvars64
        // reached the service process for inheritance.

        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        if let Some(ref cwd) = config.cwd {
            cmd.current_dir(cwd);
        }

        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        apply_hide_window(&mut cmd);
        cmd.kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| Error::SpawnFailed {
            name: config.name.clone(),
            reason: format!("Failed to start codex process: {e}"),
        })?;

        Ok(child)
    }
}

/// Idempotently inject a managed `[mcp_servers."team-mode"]` block into
/// the user's global `~/.codex/config.toml`. The block is bracketed by
/// markers so subsequent calls update or skip in place. We never edit
/// content outside the markers; user's other entries are preserved.
///
/// Why global instead of project-local: see `spawn_child` rationale.
fn ensure_global_codex_mcp_config() -> Result<()> {
    let codex_home = match std::env::var_os("CODEX_HOME") {
        Some(p) => PathBuf::from(p),
        None => {
            // Fall back to <user-home>/.codex. Use USERPROFILE on
            // Windows, HOME elsewhere — this matches what codex itself
            // does when CODEX_HOME is unset.
            let home = std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from)
                .ok_or_else(|| {
                    Error::Other(
                        "neither CODEX_HOME nor USERPROFILE/HOME is set; cannot \
                         locate codex config"
                            .into(),
                    )
                })?;
            home.join(".codex")
        }
    };

    std::fs::create_dir_all(&codex_home).map_err(|e| {
        Error::Other(format!(
            "failed to create codex home {}: {e}",
            codex_home.display()
        ))
    })?;

    let config_path = codex_home.join("config.toml");
    let url = std::env::var("TEAM_MODE_HTTP_MCP_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8786/mcp".to_string());

    const BEGIN: &str = "# === BEGIN agent-teams-mcp managed (team-mode) ===";
    const END: &str = "# === END agent-teams-mcp managed (team-mode) ===";
    let block = format!(
        "{begin}\n\
         # Auto-managed by agent-teams-mcp. Do not edit by hand — this entire\n\
         # block is regenerated on every codex worker spawn. Remove the markers\n\
         # to keep manual edits across spawns.\n\
         [mcp_servers.\"team-mode\"]\n\
         url = \"{url}\"\n\
         bearer_token_env_var = \"TEAM_MODE_HTTP_MCP_TOKEN\"\n\
         env_http_headers = {{ \"X-Team-Mode-Team\" = \"TEAM_MODE_TEAM\", \
         \"X-Team-Mode-Member\" = \"TEAM_MODE_MEMBER\", \
         \"X-Team-Mode-Worker-Id\" = \"TEAM_MODE_WORKER_ID\", \
         \"X-Team-Mode-Project-Root\" = \"TEAM_MODE_PROJECT_ROOT\" }}\n\
         {end}\n",
        begin = BEGIN,
        url = toml_escape(&url),
        end = END,
    );

    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();

    let new_contents = match (existing.find(BEGIN), existing.find(END)) {
        (Some(s), Some(e)) if e > s => {
            // Replace the managed block in place. `e + END.len()` plus a
            // possible trailing newline.
            let end_byte = e + END.len();
            let after_end = if existing[end_byte..].starts_with('\n') {
                end_byte + 1
            } else {
                end_byte
            };
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..s]);
            out.push_str(&block);
            out.push_str(&existing[after_end..]);
            out
        }
        _ => {
            // Append. Ensure exactly one blank line separator.
            let mut out = existing.clone();
            if !out.is_empty() && !out.ends_with("\n\n") {
                if out.ends_with('\n') {
                    out.push('\n');
                } else {
                    out.push_str("\n\n");
                }
            }
            out.push_str(&block);
            out
        }
    };

    if new_contents == existing {
        return Ok(()); // already up to date
    }

    // Atomic write: write to .tmp, rename. Avoids corrupting the user's
    // config if we crash mid-write.
    let tmp_path = config_path.with_extension("toml.agent-teams-tmp");
    std::fs::write(&tmp_path, &new_contents)
        .map_err(|e| Error::Other(format!("failed to write {}: {e}", tmp_path.display())))?;
    std::fs::rename(&tmp_path, &config_path).map_err(|e| {
        Error::Other(format!(
            "failed to rename {} -> {}: {e}",
            tmp_path.display(),
            config_path.display()
        ))
    })?;
    tracing::info!(
        path = %config_path.display(),
        "wrote team-mode MCP block to global codex config"
    );
    Ok(())
}

/// TOML basic-string escape — handles backslash + double-quote (the
/// only chars that can break our generated config). We don't need
/// full TOML escaping because the values we emit (paths and
/// alphanumeric names) only contain those two problematic chars.
fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[async_trait]
impl AgentBackend for CodexBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::Codex
    }

    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>> {
        let agent_name = config.name.clone();
        let initial_prompt = config.prompt.clone();

        info!(agent = %agent_name, "Spawning Codex agent");

        let mut child = self.spawn_child(&config)?;

        // Take ownership of stdin/stdout/stderr
        let stdin = child.stdin.take().ok_or_else(|| Error::SpawnFailed {
            name: agent_name.clone(),
            reason: "Failed to capture stdin".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| Error::SpawnFailed {
            name: agent_name.clone(),
            reason: "Failed to capture stdout".into(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| Error::SpawnFailed {
            name: agent_name.clone(),
            reason: "Failed to capture stderr".into(),
        })?;

        let stdin_writer = Arc::new(Mutex::new(BufWriter::new(stdin)));
        let mut stdout_reader = BufReader::new(stdout);
        let request_id = Arc::new(AtomicU64::new(1));
        let alive = Arc::new(AtomicBool::new(true));
        let stderr_log_path = config.env.get(CODEX_STDERR_LOG_ENV).map(PathBuf::from);
        let stderr_tail = Arc::new(StdMutex::new(CodexStderrRing::new(stderr_log_path.clone())));

        // ----- Step 1: Initialize handshake -----
        let init_id = next_id(&request_id);
        let init_req = JsonRpcRequest::new(
            init_id,
            METHOD_INITIALIZE,
            Some(serde_json::json!({
                "clientInfo": {
                    "name": "agent-teams",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
        );
        send_request(&stdin_writer, &init_req).await?;
        let init_resp = wait_for_response(&mut stdout_reader, init_id).await?;
        let user_agent = init_resp
            .result
            .as_ref()
            .and_then(|r| r.get("userAgent"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        debug!(agent = %agent_name, user_agent = %user_agent, "Initialize handshake complete");

        // ----- Step 2: Send `initialized` notification -----
        let initialized_notif = JsonRpcClientNotification::new(METHOD_INITIALIZED);
        send_notification(&stdin_writer, &initialized_notif).await?;
        debug!(agent = %agent_name, "Sent 'initialized' notification");

        // ----- Step 3: Start a thread -----
        let thread_id_num = next_id(&request_id);
        let cwd = config
            .cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_default()
                    .display()
                    .to_string()
            });

        let thread_req = JsonRpcRequest::new(
            thread_id_num,
            METHOD_THREAD_START,
            Some(serde_json::json!({
                "cwd": cwd,
                "approvalPolicy": "never"
            })),
        );
        send_request(&stdin_writer, &thread_req).await?;
        let thread_resp = wait_for_response(&mut stdout_reader, thread_id_num).await?;

        let thread_id = thread_resp
            .result
            .as_ref()
            .and_then(|r| r.get("thread"))
            .and_then(|t| t.get("id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| Error::SpawnFailed {
                name: agent_name.clone(),
                reason: "thread/start response missing thread.id".into(),
            })?;

        debug!(
            agent = %agent_name,
            thread_id = %thread_id,
            "Thread created"
        );

        // ----- Step 4: Stash system prompt for the first real input -----
        //
        // Codex has no `--system-prompt` flag (compare to Claude Code). The
        // app-server protocol only consumes user-role messages via `turn/start`.
        // Sending the system prompt as a standalone `turn/start` here would:
        //   - waste an LLM round-trip (the model "answers" the instruction),
        //   - delay spawn readiness by 5–15s on cold start,
        //   - leave a noisy "Got it!" reply on the very first turn.
        //
        // Instead we stash it and prepend it to whatever the AgentLoop sends
        // next. yepanywhere does the same trick (their `globalInstructions`
        // ride on the first user message). The AgentLoop's ready-check waits
        // for TurnComplete/Idle/Error, so we have to inject a synthetic Idle
        // (step 6 below) — there's no real turn in flight.
        let pending_system_prompt = if initial_prompt.trim().is_empty() {
            None
        } else {
            Some(initial_prompt)
        };

        // ----- Step 5: Spawn background reader -----
        let (output_tx, output_rx) = tokio::sync::mpsc::channel(OUTPUT_CHANNEL_SIZE);
        // Clone the sender so we can push the synthetic ready signal (step 6)
        // after the reader task takes ownership of `output_tx`.
        let ready_tx = output_tx.clone();
        let reader_alive = alive.clone();
        let reader_name = agent_name.clone();
        // Active turn tracking for `interrupt_turn`. The reader task captures
        // the turn id from `turn/started` events and clears it on
        // `turn/completed`. `interrupt_turn` reads this snapshot to decide
        // whether to issue a protocol-level interrupt or report "idle".
        let active_turn_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let reader_active_turn_id = active_turn_id.clone();

        let stderr_tail_reader = Arc::clone(&stderr_tail);
        let stderr_name = agent_name.clone();
        let stderr_handle = tokio::spawn(async move {
            debug!(agent = %stderr_name, "Background Codex stderr reader started");
            let mut stderr_reader = BufReader::new(stderr);
            let mut stderr_log = open_stderr_log(stderr_log_path.as_ref(), &stderr_name);
            let mut line_buf = String::new();

            loop {
                line_buf.clear();
                match stderr_reader.read_line(&mut line_buf).await {
                    Ok(0) => {
                        debug!(agent = %stderr_name, "Codex stderr EOF");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line_buf.trim_end_matches(&['\n', '\r'][..]);
                        if trimmed.is_empty() {
                            continue;
                        }
                        let timestamp = chrono::Utc::now().to_rfc3339();
                        let sequence = {
                            match stderr_tail_reader.lock() {
                                Ok(mut guard) => {
                                    guard.push_line(timestamp.clone(), trimmed.to_string())
                                }
                                Err(_) => {
                                    error!(
                                        agent = %stderr_name,
                                        "codex stderr ring buffer poisoned"
                                    );
                                    0
                                }
                            }
                        };
                        if let Some(log) = stderr_log.as_mut() {
                            let _ = writeln!(log, "[{timestamp} #{sequence}] {trimmed}");
                            let _ = log.flush();
                        }
                        debug!(
                            event = "codex_worker.stderr",
                            worker_name = %stderr_name,
                            line = %redact_stderr_line(trimmed),
                            timestamp = %timestamp,
                            sequence,
                            "codex worker stderr"
                        );
                    }
                    Err(e) => {
                        error!(
                            agent = %stderr_name,
                            error = %e,
                            "Error reading Codex stderr"
                        );
                        break;
                    }
                }
            }
            debug!(agent = %stderr_name, "Background Codex stderr reader stopped");
        });

        let reader_handle = tokio::spawn(async move {
            debug!(agent = %reader_name, "Background Codex reader started");
            let mut line_buf = String::new();

            loop {
                if !reader_alive.load(Ordering::Relaxed) {
                    break;
                }

                line_buf.clear();
                match stdout_reader.read_line(&mut line_buf).await {
                    Ok(0) => {
                        // EOF -- process exited. Idle is a control event: must guarantee delivery.
                        debug!(agent = %reader_name, "Codex stdout EOF");
                        reader_alive.store(false, Ordering::Relaxed);
                        let _ = output_tx.send(AgentOutput::Idle).await;
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line_buf.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        // Try to parse as a JSON-RPC message
                        match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                            Ok(JsonRpcMessage::Notification(notif)) => {
                                // Track active turn id from `turn/started` /
                                // `turn/completed` events. Stored in
                                // `reader_active_turn_id` so `interrupt_turn`
                                // can address the in-flight turn by id.
                                match notif.method.as_str() {
                                    EVENT_TURN_STARTED => {
                                        if let Some(tid) = notif
                                            .params
                                            .as_ref()
                                            .and_then(|p| p.get("turn"))
                                            .and_then(|t| t.get("id"))
                                            .and_then(|v| v.as_str())
                                        {
                                            *reader_active_turn_id.lock().await =
                                                Some(tid.to_string());
                                        }
                                    }
                                    EVENT_TURN_COMPLETED => {
                                        *reader_active_turn_id.lock().await = None;
                                    }
                                    _ => {}
                                }

                                if let Some(output) = map_notification_to_output(&notif)
                                    && send_agent_output(
                                        &output_tx,
                                        output,
                                        &reader_alive,
                                        &reader_name,
                                    )
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Ok(JsonRpcMessage::Response(resp)) => {
                                // Responses to our requests; check for errors
                                if let Some(err) = resp.error {
                                    // Error is a control event: guaranteed delivery
                                    if output_tx
                                        .send(AgentOutput::Error(err.to_string()))
                                        .await
                                        .is_err()
                                    {
                                        reader_alive.store(false, Ordering::Relaxed);
                                        break;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    agent = %reader_name,
                                    line = %trimmed,
                                    error = %e,
                                    "Failed to parse Codex output line"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(agent = %reader_name, error = %e, "Error reading Codex stdout");
                        // Error is a control event: guaranteed delivery
                        let _ = output_tx
                            .send(AgentOutput::Error(format!("Read error: {e}")))
                            .await;
                        reader_alive.store(false, Ordering::Relaxed);
                        break;
                    }
                }
            }
            debug!(agent = %reader_name, "Background Codex reader stopped");
        });

        // ----- Step 6: Emit synthetic Idle so AgentLoop's ready-check unblocks
        //
        // The reader task is wired up but the child has no work in flight —
        // it's sitting at thread/start completion, waiting for a turn/start.
        // AgentLoop drains output until it sees TurnComplete/Idle/Error, so
        // without this push the spawn-time ready-check would hang until the
        // 30s init timeout (the protocol read timeout for thread/start).
        // `Idle` is the right signal: nothing is happening, the worker is
        // ready for input.
        if ready_tx.send(AgentOutput::Idle).await.is_err() {
            warn!(
                agent = %agent_name,
                "spawn-time Idle dropped — receiver already gone"
            );
        }
        drop(ready_tx);

        let session = CodexSession {
            name: agent_name,
            child: Some(child),
            stdin: stdin_writer,
            thread_id,
            request_id,
            output_rx: Some(output_rx),
            alive,
            reader_handle: Some(reader_handle),
            stderr_handle: Some(stderr_handle),
            stderr_tail,
            pending_system_prompt: Arc::new(Mutex::new(pending_system_prompt)),
            active_turn_id,
        };

        Ok(Box::new(session))
    }
}

// ---------------------------------------------------------------------------
// CodexSession
// ---------------------------------------------------------------------------

/// A running Codex agent session.
struct CodexSession {
    name: String,
    child: Option<Child>,
    stdin: Arc<Mutex<BufWriter<tokio::process::ChildStdin>>>,
    /// Codex's `thread/start` response carries this UUID. It also names the
    /// rollout JSONL file under `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`,
    /// so it doubles as the backend session id surfaced via [`AgentSession::session_id`].
    thread_id: String,
    request_id: Arc<AtomicU64>,
    output_rx: Option<tokio::sync::mpsc::Receiver<AgentOutput>>,
    alive: Arc<AtomicBool>,
    reader_handle: Option<JoinHandle<()>>,
    stderr_handle: Option<JoinHandle<()>>,
    stderr_tail: Arc<StdMutex<CodexStderrRing>>,
    /// System instructions captured at spawn that have not yet been delivered
    /// to the model. Consumed (set to `None`) on the first `send_input` so it
    /// rides along with the user's first real message instead of burning a
    /// dedicated turn at startup.
    pending_system_prompt: Arc<Mutex<Option<String>>>,
    /// Set by the reader task on `turn/started` events; cleared on
    /// `turn/completed`. `interrupt_turn` reads this to decide whether
    /// to issue a `turn/interrupt` JSON-RPC (turn id present = active turn,
    /// `None` = idle, no-op).
    active_turn_id: Arc<Mutex<Option<String>>>,
}

#[async_trait]
impl AgentSession for CodexSession {
    fn name(&self) -> &str {
        &self.name
    }

    async fn send_input(&mut self, input: &str) -> Result<()> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err(Error::AgentNotAlive {
                name: self.name.clone(),
            });
        }

        // Drain any pending system prompt and prepend it to this turn.
        // After the first send the slot is None and this is a no-op.
        let final_input = {
            let mut slot = self.pending_system_prompt.lock().await;
            match slot.take() {
                Some(sp) => format!("[System instructions]\n{sp}\n\n---\n\n{input}"),
                None => input.to_string(),
            }
        };

        let id = next_id(&self.request_id);
        let req = JsonRpcRequest::new(
            id,
            METHOD_TURN_START,
            Some(serde_json::json!({
                "threadId": self.thread_id,
                "input": [
                    {
                        "type": "text",
                        "text": final_input
                    }
                ]
            })),
        );
        send_request(&self.stdin, &req).await
    }

    fn output_receiver(&mut self) -> Option<tokio::sync::mpsc::Receiver<AgentOutput>> {
        self.output_rx.take()
    }

    async fn is_alive(&self) -> bool {
        // Fast path: if our internal flag is already false (shutdown was
        // called explicitly), the session is definitely dead.
        if !self.alive.load(Ordering::Relaxed) {
            return false;
        }
        // Slow path: the flag is stale when the child is killed
        // externally (taskkill, kill -9, OOM, host crash). Verify
        // against the OS via sysinfo. Without this check, the active
        // liveness probe + worker-liveness watchdog both think a
        // killed worker is still alive, and the lead never receives
        // an OutputClosed [SYSTEM] notice.
        let Some(pid) = self.child.as_ref().and_then(|c| c.id()) else {
            return false;
        };
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(
            sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
            true,
        );
        sys.process(sysinfo::Pid::from_u32(pid)).is_some()
    }

    fn session_id(&self) -> Option<String> {
        // Codex's `thread/start` UUID is the canonical session identifier:
        // it names the rollout file on disk (.../rollout-<ts>-<thread_id>.jsonl)
        // and is the only stable handle for `thread/resume`. Persisting it
        // into `member.execution.session_id` lets the web UI render the
        // worker's exact transcript instead of falling back to mtime guesses.
        Some(self.thread_id.clone())
    }

    fn stderr_tail(&self) -> Option<String> {
        self.stderr_tail
            .lock()
            .ok()
            .and_then(|ring| ring.snapshot_tail(STDERR_TAIL_LINES))
    }

    fn stderr_log_hint(&self) -> Option<String> {
        self.stderr_tail
            .lock()
            .ok()
            .and_then(|ring| ring.log_hint(STDERR_TAIL_LINES))
    }

    /// Issue a `turn/interrupt` JSON-RPC if the worker is currently
    /// processing a turn; no-op if idle. Used by `send_message(preempt=true)`
    /// to abort an in-flight turn so a follow-up message can be processed
    /// immediately. Per app-server semantics the turn ends with
    /// `turn/completed status="interrupted"`; in-flight shell commands the
    /// turn already issued continue running (the protocol doesn't kill them).
    async fn interrupt_turn(&self) -> Result<bool> {
        if !self.alive.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let turn_id = match self.active_turn_id.lock().await.clone() {
            Some(id) => id,
            None => {
                debug!(agent = %self.name, "interrupt_turn: no active turn, no-op");
                return Ok(false);
            }
        };
        let id = next_id(&self.request_id);
        let req = JsonRpcRequest::new(
            id,
            METHOD_TURN_INTERRUPT,
            Some(serde_json::json!({
                "threadId": self.thread_id,
                "turnId": turn_id,
            })),
        );
        debug!(
            agent = %self.name,
            thread_id = %self.thread_id,
            turn_id = %turn_id,
            "interrupt_turn: sending turn/interrupt"
        );
        send_request(&self.stdin, &req).await?;
        Ok(true)
    }

    async fn shutdown(&mut self) -> Result<()> {
        info!(agent = %self.name, "Shutting down Codex session");
        self.alive.store(false, Ordering::Relaxed);

        // Abort the reader task so it doesn't block on stdout reads
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        if let Some(handle) = self.stderr_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        // Close stdin to signal the child to exit
        {
            let mut writer = self.stdin.lock().await;
            let _ = writer.shutdown().await;
        }

        // Wait briefly for the child to exit, then kill if needed
        if let Some(ref mut child) = self.child {
            let timeout =
                tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;

            if timeout.is_err() {
                warn!(agent = %self.name, "Codex child did not exit in time, killing");
                let _ = child.kill().await;
            }
        }

        Ok(())
    }

    async fn force_kill(&mut self) -> Result<()> {
        info!(agent = %self.name, "Force-killing Codex session");
        self.alive.store(false, Ordering::Relaxed);

        // Abort the reader task first
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        if let Some(handle) = self.stderr_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        if let Some(ref mut child) = self.child {
            child.kill().await.map_err(|e| Error::CodexProtocol {
                reason: format!("Failed to kill Codex process for {}: {e}", self.name),
            })?;
        }

        Ok(())
    }
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        // Abort the reader task if it was not already taken by shutdown/force_kill.
        // The child process is handled by kill_on_drop(true).
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.stderr_handle.take() {
            handle.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Atomically increment and return the next request ID.
fn next_id(counter: &AtomicU64) -> u64 {
    counter.fetch_add(1, Ordering::Relaxed)
}

/// Serialize a JSON-RPC request and write it to the writer followed by a newline.
async fn send_request(
    writer: &Arc<Mutex<BufWriter<tokio::process::ChildStdin>>>,
    request: &JsonRpcRequest,
) -> Result<()> {
    let line = serde_json::to_string(request)?;
    let mut w = writer.lock().await;
    w.write_all(line.as_bytes())
        .await
        .map_err(|e| Error::CodexProtocol {
            reason: format!("Failed to write to Codex stdin: {e}"),
        })?;
    w.write_all(b"\n").await.map_err(|e| Error::CodexProtocol {
        reason: format!("Failed to write newline to Codex stdin: {e}"),
    })?;
    w.flush().await.map_err(|e| Error::CodexProtocol {
        reason: format!("Failed to flush Codex stdin: {e}"),
    })?;
    Ok(())
}

/// Serialize a client notification and write it to the writer followed by a newline.
async fn send_notification(
    writer: &Arc<Mutex<BufWriter<tokio::process::ChildStdin>>>,
    notification: &JsonRpcClientNotification,
) -> Result<()> {
    let line = serde_json::to_string(notification)?;
    let mut w = writer.lock().await;
    w.write_all(line.as_bytes())
        .await
        .map_err(|e| Error::CodexProtocol {
            reason: format!("Failed to write notification to Codex stdin: {e}"),
        })?;
    w.write_all(b"\n").await.map_err(|e| Error::CodexProtocol {
        reason: format!("Failed to write newline to Codex stdin: {e}"),
    })?;
    w.flush().await.map_err(|e| Error::CodexProtocol {
        reason: format!("Failed to flush Codex stdin: {e}"),
    })?;
    Ok(())
}

/// Read lines from the reader until we find a response matching the given `id`.
/// Returns the response. Non-matching lines (notifications, other responses) are
/// consumed and discarded during this blocking wait.
async fn wait_for_response(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    expected_id: u64,
) -> Result<JsonRpcResponse> {
    let expected_val = serde_json::Value::Number(expected_id.into());
    let mut line_buf = String::new();

    // 30s default fits Codex cold start on Windows (10–15s) with margin.
    // Override via `TEAM_MODE_CODEX_INIT_TIMEOUT_SEC` for ultra-cold disks
    // or constrained CI runners; clamped to >=5s to stay sane.
    let timeout_secs: u64 = std::env::var("TEAM_MODE_CODEX_INIT_TIMEOUT_SEC")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|n| n.max(5))
        .unwrap_or(30);
    let timeout_duration = std::time::Duration::from_secs(timeout_secs);
    let deadline = tokio::time::Instant::now() + timeout_duration;

    loop {
        line_buf.clear();

        let read_result = tokio::time::timeout_at(deadline, reader.read_line(&mut line_buf))
            .await
            .map_err(|_| Error::Timeout {
                seconds: timeout_secs,
            })?
            .map_err(|e| Error::CodexProtocol {
                reason: format!("Read error waiting for response: {e}"),
            })?;

        if read_result == 0 {
            return Err(Error::CodexProtocol {
                reason: "Codex process closed stdout before responding".into(),
            });
        }

        let trimmed = line_buf.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Try to parse as a response with matching id
        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(trimmed)
            && resp.id == expected_val
        {
            if let Some(ref err) = resp.error {
                return Err(Error::CodexProtocol {
                    reason: format!("Codex RPC error: {err}"),
                });
            }
            return Ok(resp);
        }
        // Notifications and non-matching responses are silently skipped during handshake.
    }
}

fn map_notification_to_output(notif: &JsonRpcNotification) -> Option<AgentOutput> {
    match notif.method.as_str() {
        EVENT_AGENT_MESSAGE_DELTA => {
            // Extract streaming text delta from `params.delta`
            let text = notif
                .params
                .as_ref()
                .and_then(|p| p.get("delta"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            if text.is_empty() {
                None
            } else {
                Some(AgentOutput::Delta(text.to_string()))
            }
        }
        EVENT_COMMAND_OUTPUT_DELTA => {
            // Shell/command execution output (e.g. `cat`, `grep`, `ls`,
            // PowerShell terminal output with ANSI escapes). This is
            // *tool transcript data*, NOT the assistant's reply. The
            // previous mapping to `AgentOutput::Delta` caused the entire
            // command stdout (file dumps, grep results, ANSI codes) to
            // be concatenated into the worker's reply body, inflating a
            // 200-word summary into a 50,000+ token payload.
            //
            // Map to `ToolOutput` so the agent loop can drop it from the
            // body but still surface it via tracing for observability.
            let text = notif
                .params
                .as_ref()
                .and_then(|p| p.get("delta"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            if text.is_empty() {
                None
            } else {
                Some(AgentOutput::ToolOutput(text.to_string()))
            }
        }
        EVENT_ITEM_COMPLETED => {
            // Extract text content from a completed agentMessage item.
            // The item structure is: {type: "agentMessage", content: [{type: "text", text: "..."}]}
            let item = notif.params.as_ref().and_then(|p| p.get("item"));

            let is_agent_message =
                item.and_then(|i| i.get("type")).and_then(|t| t.as_str()) == Some("agentMessage");

            if !is_agent_message {
                return None;
            }

            // Collect ALL text blocks from content array (not just the first)
            let text: String = item
                .and_then(|i| i.get("content"))
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|part| {
                            if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                                part.get("text").and_then(|t| t.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();

            if text.is_empty() {
                None
            } else {
                Some(AgentOutput::Message(text))
            }
        }
        EVENT_TURN_COMPLETED => Some(AgentOutput::TurnComplete),
        EVENT_ERROR => {
            let message = notif
                .params
                .as_ref()
                .and_then(|p| p.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            tracing::warn!(
                event = "codex_error",
                params = ?notif.params,
                "codex EVENT_ERROR"
            );
            Some(AgentOutput::Error(message.to_string()))
        }
        // Informational events -- no output needed
        EVENT_THREAD_STARTED | EVENT_TURN_STARTED | EVENT_ITEM_STARTED => None,
        // Ignore internal/legacy codex events and other unknowns
        other => {
            if !other.starts_with("codex/event/") {
                debug!(method = %notif.method, "Unhandled Codex notification");
            }
            None
        }
    }
}
