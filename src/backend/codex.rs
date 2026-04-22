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
//! 5. Send `turn/start` with `{threadId, input: [{type: "text", text: "..."}]}`.
//! 6. A background task reads stdout line-by-line, parsing messages
//!    and forwarding them as [`AgentOutput`] events through an mpsc channel.
//! 7. Follow-up inputs are sent as additional `turn/start` requests.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use super::codex_protocol::*;
use super::{AgentBackend, AgentOutput, AgentSession, BackendType, SpawnConfig, send_agent_output};
use crate::{Error, Result};

/// Channel buffer size for agent output events.
const OUTPUT_CHANNEL_SIZE: usize = 256;

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
        let mut cmd = Command::new(&self.codex_path);
        // `codex app-server` reads JSON from stdin, writes JSON to stdout.
        // No additional flags needed -- stdio is the default transport.
        cmd.arg("app-server");

        // Pass model override via -c config flag
        if let Some(ref model) = config.model {
            cmd.arg("-c").arg(format!("model=\"{model}\""));
        }

        // Pass reasoning effort override via -c config flag
        if let Some(ref effort) = config.reasoning_effort {
            cmd.arg("-c")
                .arg(format!("model_reasoning_effort=\"{effort}\""));
        }

        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        // Discard stderr to avoid pipe-buffer deadlock: if the child writes
        // enough to stderr without anyone reading it, the OS buffer fills and
        // the child blocks, stalling stdout as well.
        cmd.stderr(std::process::Stdio::null());

        if let Some(ref cwd) = config.cwd {
            cmd.current_dir(cwd);
        }

        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        cmd.kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| Error::SpawnFailed {
            name: config.name.clone(),
            reason: format!("Failed to start codex process: {e}"),
        })?;

        Ok(child)
    }
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

        // Take ownership of stdin/stdout
        let stdin = child.stdin.take().ok_or_else(|| Error::SpawnFailed {
            name: agent_name.clone(),
            reason: "Failed to capture stdin".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| Error::SpawnFailed {
            name: agent_name.clone(),
            reason: "Failed to capture stdout".into(),
        })?;

        let stdin_writer = Arc::new(Mutex::new(BufWriter::new(stdin)));
        let mut stdout_reader = BufReader::new(stdout);
        let request_id = Arc::new(AtomicU64::new(1));
        let alive = Arc::new(AtomicBool::new(true));

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

        // ----- Step 4: Send initial prompt as first turn -----
        let turn_id = next_id(&request_id);
        let turn_req = JsonRpcRequest::new(
            turn_id,
            METHOD_TURN_START,
            Some(serde_json::json!({
                "threadId": thread_id,
                "input": [
                    {
                        "type": "text",
                        "text": initial_prompt
                    }
                ]
            })),
        );
        send_request(&stdin_writer, &turn_req).await?;

        // ----- Step 5: Spawn background reader -----
        let (output_tx, output_rx) = tokio::sync::mpsc::channel(OUTPUT_CHANNEL_SIZE);
        let reader_alive = alive.clone();
        let reader_name = agent_name.clone();

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

        let session = CodexSession {
            name: agent_name,
            child: Some(child),
            stdin: stdin_writer,
            thread_id,
            request_id,
            output_rx: Some(output_rx),
            alive,
            reader_handle: Some(reader_handle),
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
    thread_id: String,
    request_id: Arc<AtomicU64>,
    output_rx: Option<tokio::sync::mpsc::Receiver<AgentOutput>>,
    alive: Arc<AtomicBool>,
    reader_handle: Option<JoinHandle<()>>,
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

        let id = next_id(&self.request_id);
        let req = JsonRpcRequest::new(
            id,
            METHOD_TURN_START,
            Some(serde_json::json!({
                "threadId": self.thread_id,
                "input": [
                    {
                        "type": "text",
                        "text": input
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
        self.alive.load(Ordering::Relaxed)
    }

    async fn shutdown(&mut self) -> Result<()> {
        info!(agent = %self.name, "Shutting down Codex session");
        self.alive.store(false, Ordering::Relaxed);

        // Abort the reader task so it doesn't block on stdout reads
        if let Some(handle) = self.reader_handle.take() {
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

    let timeout_duration = std::time::Duration::from_secs(30);
    let deadline = tokio::time::Instant::now() + timeout_duration;

    loop {
        line_buf.clear();

        let read_result = tokio::time::timeout_at(deadline, reader.read_line(&mut line_buf))
            .await
            .map_err(|_| Error::Timeout { seconds: 30 })?
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
            // Command execution output delta from `params.delta`
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
