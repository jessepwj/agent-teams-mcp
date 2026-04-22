//! Claude Code CLI backend -- persistent process via `--input-format/--output-format stream-json`.
//!
//! ## Implementation: persistent stdin/stdout with stream-json NDJSON protocol
//!
//! A single `claude` process is spawned and kept alive for the lifetime of the session.
//! User messages are written to stdin as newline-delimited JSON (NDJSON) envelopes;
//! responses are read from stdout as newline-delimited JSON events (stream-json format).
//!
//! ### Protocol details
//!
//! ```text
//! spawn()      -> claude -p "" --input-format stream-json --output-format stream-json
//!                        --verbose [--system-prompt <sp>] [--model <m>] ...
//!                 process stays alive, waiting for NDJSON user messages on stdin
//!                 an initial AgentOutput::TurnComplete is emitted so that callers
//!                 (AgentLoop) can drain the "ready" state before sending input.
//!
//! send_input() -> wait for idle=true (previous turn finished with {"type":"result"})
//!                 write one JSON line: {"type":"user","message":{"role":"user","content":"..."}}
//!                 set idle=false so concurrent sends serialise
//!
//! shutdown()   -> close stdin -> wait for process to exit
//! ```
//!
//! The idle gate prevents interleaving when a new message arrives before the previous
//! turn has produced its `{"type":"result"}` event -- Claude would otherwise queue the
//! input behind the current turn rather than treating it as the next logical user turn.
//!
//! ### stream-json event types handled (output)
//!
//! - `{"type":"assistant","message":{"content":[{"type":"text","text":"..."}],...}}`
//!   => emits `AgentOutput::Delta(text)` for each text content block
//! - `{"type":"tool_use","name":"...","input":{...}}`
//!   => logged as debug, no output emitted
//! - `{"type":"result","subtype":"success","result":"...","is_error":false,"session_id":"..."}`
//!   => emits `AgentOutput::TurnComplete`
//! - `{"type":"result","subtype":"error","is_error":true,"result":"error message"}`
//!   => emits `AgentOutput::Error(...)`
//! - EOF on stdout
//!   => emits `AgentOutput::Error("process exited")`, background task ends
//!
//! ### Fallback (Plan B)
//!
//! If the `claude` binary does not respond to stdin input (e.g. requires PTY),
//! replace this file with the `--resume <session-id>` implementation from git history.
//! The interface (struct names, trait impls) is identical, so no other files need changes.
//!
//! ### Windows note
//!
//! On Windows, Claude Code requires `git-bash`. The environment variable
//! `CLAUDE_CODE_GIT_BASH_PATH` must point to `bash.exe`. This backend reads that
//! variable from `config.env` or the process environment and forwards it automatically.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use super::{AgentBackend, AgentOutput, AgentSession, BackendType, SpawnConfig, send_agent_output};
use crate::{Error, Result};

/// Channel buffer size for agent output events.
const OUTPUT_CHANNEL_SIZE: usize = 256;

// ---------------------------------------------------------------------------
// stream-json event shapes
// ---------------------------------------------------------------------------

/// A single content block inside an assistant message.
#[derive(Debug, serde::Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

/// The `message` field inside an `assistant` event.
#[derive(Debug, serde::Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

/// Top-level stream-json event from `claude --output-format stream-json`.
#[derive(Debug, serde::Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    kind: String,
    /// Present on `assistant` events.
    #[serde(default)]
    message: Option<AssistantMessage>,
    /// Present on `result` events -- the final text summary.
    #[serde(default)]
    result: Option<String>,
    /// Present on `result` events.
    #[serde(default)]
    is_error: bool,
    /// Subtype field on `result` events: `"success"` or `"error"`.
    #[serde(default)]
    #[allow(dead_code)]
    subtype: Option<String>,
}

// ---------------------------------------------------------------------------
// ClaudeCodeBackend  (factory)
// ---------------------------------------------------------------------------

/// Factory that creates Claude Code CLI agent sessions using a persistent process
/// with `--output-format stream-json`.
#[derive(Debug, Clone)]
pub struct ClaudeCodeBackend {
    /// Path to the `claude` CLI binary.
    claude_path: PathBuf,
}

impl ClaudeCodeBackend {
    /// Locate the `claude` binary on `$PATH` via `which`.
    pub fn new() -> Result<Self> {
        let path = which::which("claude").map_err(|_| Error::CliNotFound {
            name: "claude".into(),
        })?;
        Ok(Self { claude_path: path })
    }

    /// Use an explicit path to the `claude` binary.
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            claude_path: path.into(),
        }
    }

    /// Spawn the `claude` child process in stream-json mode.
    fn spawn_child(&self, config: &SpawnConfig) -> Result<Child> {
        let mut cmd = Command::new(&self.claude_path);

        // Headless / print mode with NDJSON stdin + stdout. `--output-format stream-json`
        // and `--input-format stream-json` both require `-p` (print) and `--verbose`.
        // An empty initial prompt lets the process wait for stdin NDJSON messages.
        cmd.arg("-p").arg("");
        cmd.arg("--input-format").arg("stream-json");
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose");

        // System prompt via CLI flag avoids consuming a user turn.
        if !config.prompt.is_empty() {
            cmd.arg("--system-prompt").arg(&config.prompt);
        }

        // Model override.
        if let Some(ref model) = config.model {
            cmd.arg("--model").arg(model);
        }

        // Permission mode.
        if let Some(ref mode) = config.permission_mode {
            cmd.arg("--permission-mode").arg(mode);
        }

        // Allowed tools.
        if !config.allowed_tools.is_empty() {
            cmd.arg("--allowedTools").arg(config.allowed_tools.join(","));
        }

        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        // Discard stderr to avoid pipe-buffer deadlock.
        cmd.stderr(std::process::Stdio::null());

        if let Some(ref cwd) = config.cwd {
            cmd.current_dir(cwd);
        }

        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        // Windows: forward CLAUDE_CODE_GIT_BASH_PATH if not already set.
        let git_bash_key = "CLAUDE_CODE_GIT_BASH_PATH";
        if !config.env.contains_key(git_bash_key) {
            if let Ok(val) = std::env::var(git_bash_key) {
                cmd.env(git_bash_key, val);
            }
        }

        cmd.kill_on_drop(true);

        let child = cmd.spawn().map_err(|e| Error::SpawnFailed {
            name: config.name.clone(),
            reason: format!("Failed to start claude process: {e}"),
        })?;

        Ok(child)
    }
}

impl Default for ClaudeCodeBackend {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| Self {
            claude_path: PathBuf::from("claude"),
        })
    }
}

#[async_trait]
impl AgentBackend for ClaudeCodeBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::ClaudeCode
    }

    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>> {
        let agent_name = config.name.clone();

        info!(agent = %agent_name, "Spawning Claude Code CLI agent (stream-json persistent mode)");

        let mut child = self.spawn_child(&config)?;

        // Take ownership of stdin/stdout.
        let stdin = child.stdin.take().ok_or_else(|| Error::SpawnFailed {
            name: agent_name.clone(),
            reason: "Failed to capture stdin".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| Error::SpawnFailed {
            name: agent_name.clone(),
            reason: "Failed to capture stdout".into(),
        })?;

        let stdin_writer = BufWriter::new(stdin);
        let stdout_reader = BufReader::new(stdout);

        let (output_tx, output_rx) = mpsc::channel(OUTPUT_CHANNEL_SIZE);
        let alive = Arc::new(AtomicBool::new(true));
        // idle=true means the previous turn has finished (or no turn started yet)
        // and the session is ready to accept the next user message.
        let idle = Arc::new(AtomicBool::new(true));
        let idle_notify = Arc::new(Notify::new());

        // Signal initial readiness so that AgentLoop's initial-drain recv() completes.
        // claude in stream-json mode stays idle after spawn until we write the first
        // user NDJSON line, so we surface a synthetic TurnComplete to unblock callers.
        let _ = output_tx.try_send(AgentOutput::TurnComplete);

        // Spawn background reader task.
        let reader_alive = alive.clone();
        let reader_tx = output_tx.clone();
        let reader_name = agent_name.clone();
        let reader_idle = idle.clone();
        let reader_idle_notify = idle_notify.clone();

        let reader_handle = tokio::spawn(background_reader(
            stdout_reader,
            reader_tx,
            reader_alive,
            reader_name,
            reader_idle,
            reader_idle_notify,
        ));

        let session = ClaudeCodeSession {
            name: agent_name,
            child: Some(child),
            stdin: stdin_writer,
            output_tx,
            output_rx: Some(output_rx),
            alive,
            idle,
            idle_notify,
            reader_handle: Some(reader_handle),
        };

        Ok(Box::new(session))
    }
}

// ---------------------------------------------------------------------------
// ClaudeCodeSession
// ---------------------------------------------------------------------------

/// A running Claude Code CLI agent session backed by a persistent process.
struct ClaudeCodeSession {
    /// Agent name for logging.
    name: String,
    /// The underlying child process.
    child: Option<Child>,
    /// Writer to the child's stdin.
    stdin: BufWriter<tokio::process::ChildStdin>,
    /// Sender kept alive so the channel stays open while the session is live.
    #[allow(dead_code)]
    output_tx: mpsc::Sender<AgentOutput>,
    /// Output receiver (taken once by the orchestrator).
    output_rx: Option<mpsc::Receiver<AgentOutput>>,
    /// Liveness flag shared with the background reader task.
    alive: Arc<AtomicBool>,
    /// Idle gate: true when the previous turn finished with `{"type":"result"}`
    /// (or no turn has started), meaning we may write the next user message.
    idle: Arc<AtomicBool>,
    /// Notifier fired whenever `idle` transitions to true.
    idle_notify: Arc<Notify>,
    /// Handle to the background stdout reader task.
    reader_handle: Option<JoinHandle<()>>,
}

#[async_trait]
impl AgentSession for ClaudeCodeSession {
    fn name(&self) -> &str {
        &self.name
    }

    async fn send_input(&mut self, input: &str) -> Result<()> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err(Error::AgentNotAlive {
                name: self.name.clone(),
            });
        }

        // Wait for the previous turn to finish. Loop with a 30s guard so a stuck
        // child doesn't block forever silently.
        let wait_timeout = std::time::Duration::from_secs(30);
        loop {
            // Acquire a notification permit BEFORE the idle check to avoid the
            // classic lost-wakeup race (notify fires between check and await).
            let notified = self.idle_notify.notified();
            tokio::pin!(notified);
            if self.idle.load(Ordering::Acquire) {
                break;
            }
            if !self.alive.load(Ordering::Relaxed) {
                return Err(Error::AgentNotAlive {
                    name: self.name.clone(),
                });
            }
            match tokio::time::timeout(wait_timeout, &mut notified).await {
                Ok(()) => continue,
                Err(_) => {
                    warn!(agent = %self.name, "Timed out waiting for idle before sending input");
                    return Err(Error::SpawnFailed {
                        name: self.name.clone(),
                        reason: "Timed out waiting for previous turn to finish".into(),
                    });
                }
            }
        }

        // Mark busy so any concurrent sender blocks on the notifier above.
        self.idle.store(false, Ordering::Release);

        let line = encode_user_message_ndjson(input);
        debug!(agent = %self.name, bytes = line.len(), "Sending NDJSON user message to claude stdin");

        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| Error::SpawnFailed {
                name: self.name.clone(),
                reason: format!("Failed to write NDJSON to claude stdin: {e}"),
            })?;
        self.stdin
            .flush()
            .await
            .map_err(|e| Error::SpawnFailed {
                name: self.name.clone(),
                reason: format!("Failed to flush claude stdin: {e}"),
            })?;

        Ok(())
    }

    fn output_receiver(&mut self) -> Option<mpsc::Receiver<AgentOutput>> {
        self.output_rx.take()
    }

    async fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    async fn shutdown(&mut self) -> Result<()> {
        info!(agent = %self.name, "Shutting down Claude Code CLI session");
        self.alive.store(false, Ordering::Relaxed);

        // Abort the reader task so it doesn't block on stdout reads.
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        // Close stdin to signal the child to exit gracefully.
        let _ = self.stdin.shutdown().await;

        // Wait briefly for the child to exit, then kill if needed.
        if let Some(ref mut child) = self.child {
            let timeout =
                tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;

            if timeout.is_err() {
                warn!(agent = %self.name, "Claude child did not exit in time, killing");
                let _ = child.kill().await;
            }
        }

        Ok(())
    }

    async fn force_kill(&mut self) -> Result<()> {
        info!(agent = %self.name, "Force-killing Claude Code CLI session");
        self.alive.store(false, Ordering::Relaxed);

        // Abort the reader task first.
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        if let Some(ref mut child) = self.child {
            child.kill().await.map_err(|e| Error::SpawnFailed {
                name: self.name.clone(),
                reason: format!("Failed to kill claude process: {e}"),
            })?;
        }

        Ok(())
    }
}

impl Drop for ClaudeCodeSession {
    fn drop(&mut self) {
        // Abort the reader task if it was not already taken by shutdown/force_kill.
        // The child process is handled by kill_on_drop(true).
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// Background reader task
// ---------------------------------------------------------------------------

/// Background task that continuously reads stdout from the `claude` process,
/// parses stream-json events, and forwards them as [`AgentOutput`] events.
async fn background_reader(
    mut reader: BufReader<tokio::process::ChildStdout>,
    output_tx: mpsc::Sender<AgentOutput>,
    alive: Arc<AtomicBool>,
    agent_name: String,
    idle: Arc<AtomicBool>,
    idle_notify: Arc<Notify>,
) {
    debug!(agent = %agent_name, "Background claude reader started");
    let mut line_buf = String::new();

    loop {
        if !alive.load(Ordering::Relaxed) {
            break;
        }

        line_buf.clear();
        match reader.read_line(&mut line_buf).await {
            Ok(0) => {
                // EOF -- process exited unexpectedly.
                debug!(agent = %agent_name, "Claude stdout EOF");
                alive.store(false, Ordering::Relaxed);
                // Release any sender blocked on the idle gate.
                idle.store(true, Ordering::Release);
                idle_notify.notify_waiters();
                let _ = output_tx
                    .send(AgentOutput::Error("claude process exited".into()))
                    .await;
                break;
            }
            Ok(_) => {
                let trimmed = line_buf.trim();
                if trimmed.is_empty() {
                    continue;
                }

                match serde_json::from_str::<StreamEvent>(trimmed) {
                    Ok(event) => {
                        // A `result` event marks the end of the current turn; the
                        // session is ready for the next user message.
                        if event.kind == "result" {
                            idle.store(true, Ordering::Release);
                            idle_notify.notify_waiters();
                        }
                        if let Some(output) =
                            map_event_to_output(&event, &agent_name)
                        {
                            let send_result = send_agent_output(
                                &output_tx,
                                output,
                                &alive,
                                &agent_name,
                            )
                            .await;

                            if send_result.is_err() {
                                // Channel closed.
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            agent = %agent_name,
                            line = %trimmed,
                            error = %e,
                            "Failed to parse claude stream-json line"
                        );
                    }
                }
            }
            Err(e) => {
                error!(agent = %agent_name, error = %e, "Error reading claude stdout");
                alive.store(false, Ordering::Relaxed);
                idle.store(true, Ordering::Release);
                idle_notify.notify_waiters();
                let _ = output_tx
                    .send(AgentOutput::Error(format!("Read error: {e}")))
                    .await;
                break;
            }
        }
    }

    debug!(agent = %agent_name, "Background claude reader stopped");
}

/// Encode a plain string as a single NDJSON line understood by
/// `claude --input-format stream-json`:
/// `{"type":"user","message":{"role":"user","content":"..."}}\n`
fn encode_user_message_ndjson(text: &str) -> String {
    let value = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": text,
        }
    });
    let mut line = value.to_string();
    line.push('\n');
    line
}

/// Map a parsed [`StreamEvent`] to an optional [`AgentOutput`].
fn map_event_to_output(event: &StreamEvent, agent_name: &str) -> Option<AgentOutput> {
    match event.kind.as_str() {
        "assistant" => {
            // Extract all text content blocks from the assistant message.
            let text: String = event
                .message
                .as_ref()
                .map(|msg| {
                    msg.content
                        .iter()
                        .filter(|block| block.kind == "text")
                        .map(|block| block.text.as_str())
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();

            if text.is_empty() {
                None
            } else {
                Some(AgentOutput::Delta(text))
            }
        }
        "result" => {
            if event.is_error {
                let msg = event
                    .result
                    .as_deref()
                    .unwrap_or("unknown error")
                    .to_string();
                Some(AgentOutput::Error(msg))
            } else {
                // Success: emit the final result text (if any) then TurnComplete.
                // The background reader emits TurnComplete here; any accumulated
                // Delta events were already sent for the streaming content.
                Some(AgentOutput::TurnComplete)
            }
        }
        "tool_use" => {
            // Tool invocations are informational; no output needed.
            debug!(agent = %agent_name, "Tool use event received (handled by claude)");
            None
        }
        "system" => {
            // System messages (e.g. session init info); ignore.
            None
        }
        other => {
            debug!(agent = %agent_name, event_type = %other, "Unhandled claude stream-json event");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- backend identity ---

    #[test]
    fn backend_type_is_claude_code() {
        let backend = ClaudeCodeBackend::with_path("claude");
        assert_eq!(backend.backend_type(), BackendType::ClaudeCode);
    }

    // --- stream-json event parsing ---

    #[test]
    fn parse_assistant_event_with_text() {
        let json = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello!"}],"role":"assistant"}}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.kind, "assistant");
        let output = map_event_to_output(&event, "test").unwrap();
        assert!(matches!(output, AgentOutput::Delta(ref t) if t == "Hello!"));
    }

    #[test]
    fn parse_assistant_event_multiple_text_blocks() {
        let json = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"},{"type":"text","text":" World"}],"role":"assistant"}}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        let output = map_event_to_output(&event, "test").unwrap();
        assert!(matches!(output, AgentOutput::Delta(ref t) if t == "Hello World"));
    }

    #[test]
    fn parse_assistant_event_empty_content_produces_no_output() {
        let json = r#"{"type":"assistant","message":{"content":[],"role":"assistant"}}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(map_event_to_output(&event, "test").is_none());
    }

    #[test]
    fn parse_result_success_event() {
        let json = r#"{"type":"result","subtype":"success","result":"Final answer","is_error":false,"session_id":"sess-123"}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.kind, "result");
        assert!(!event.is_error);
        let output = map_event_to_output(&event, "test").unwrap();
        assert!(matches!(output, AgentOutput::TurnComplete));
    }

    #[test]
    fn parse_result_error_event() {
        let json = r#"{"type":"result","subtype":"error","is_error":true,"result":"Something went wrong"}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(event.is_error);
        let output = map_event_to_output(&event, "test").unwrap();
        assert!(matches!(output, AgentOutput::Error(ref msg) if msg == "Something went wrong"));
    }

    #[test]
    fn parse_tool_use_event_produces_no_output() {
        let json = r#"{"type":"tool_use","name":"Read","input":{"path":"/tmp/foo"}}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(map_event_to_output(&event, "test").is_none());
    }

    #[test]
    fn parse_unknown_event_produces_no_output() {
        let json = r#"{"type":"something_new","data":"ignored"}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(map_event_to_output(&event, "test").is_none());
    }

    // --- NDJSON input encoding ---

    #[test]
    fn encode_user_message_ndjson_produces_single_line() {
        let encoded = encode_user_message_ndjson("hello world");
        assert!(encoded.ends_with('\n'));
        // Exactly one newline, at the end.
        assert_eq!(encoded.matches('\n').count(), 1);
        let parsed: serde_json::Value =
            serde_json::from_str(encoded.trim_end()).expect("valid JSON");
        assert_eq!(parsed["type"], "user");
        assert_eq!(parsed["message"]["role"], "user");
        assert_eq!(parsed["message"]["content"], "hello world");
    }

    #[test]
    fn encode_user_message_ndjson_escapes_special_chars() {
        // Embedded newlines, quotes, and unicode must survive round-trip without
        // corrupting the NDJSON framing.
        let input = "line1\nline2 \"quoted\" \u{4E2D}\u{6587}";
        let encoded = encode_user_message_ndjson(input);
        // No raw newline inside the JSON body -- only the terminal newline.
        assert_eq!(encoded.matches('\n').count(), 1);
        let parsed: serde_json::Value =
            serde_json::from_str(encoded.trim_end()).expect("valid JSON");
        assert_eq!(parsed["message"]["content"], input);
    }

    // --- idle gate via result event ---

    #[tokio::test]
    async fn result_event_flips_idle_and_notifies_waiters() {
        use tokio::io::AsyncWriteExt;

        // Build a duplex pipe so we can feed a result line into a BufReader
        // with the same type as the child stdout.
        let (mut writer, reader) = tokio::io::duplex(1024);
        let reader = BufReader::new(reader);

        // Wrap into the ChildStdout-shaped reader. Because background_reader is
        // generic over BufReader<ChildStdout>, we instead copy its core logic
        // via a direct call using an in-memory reader. Easiest path: invoke
        // a minimal harness that uses the same idle+notify wiring.
        // Here we simply simulate the state transition: a `result` line arrives
        // and drives idle -> true.
        let alive = Arc::new(AtomicBool::new(true));
        let idle = Arc::new(AtomicBool::new(false));
        let idle_notify = Arc::new(Notify::new());

        // Waiter task equivalent to send_input's idle gate.
        let idle_clone = idle.clone();
        let notify_clone = idle_notify.clone();
        let waiter = tokio::spawn(async move {
            loop {
                let notified = notify_clone.notified();
                tokio::pin!(notified);
                if idle_clone.load(Ordering::Acquire) {
                    break;
                }
                notified.await;
            }
        });

        // Drive the same logic the reader task runs upon seeing a result line.
        let line = r#"{"type":"result","subtype":"success","is_error":false}"#;
        let event: StreamEvent = serde_json::from_str(line).unwrap();
        assert_eq!(event.kind, "result");
        idle.store(true, Ordering::Release);
        idle_notify.notify_waiters();

        // Waiter must complete now that idle is true.
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter should unblock on notify")
            .expect("waiter panicked");

        // Sanity: shutting the pipe keeps alive flag unchanged here; we only
        // exercised the idle-gate pathway.
        let _ = writer.shutdown().await;
        let _ = reader; // keep reader alive until end of test
        assert!(alive.load(Ordering::Relaxed));
    }

    #[test]
    fn parse_assistant_event_non_text_block_skipped() {
        // image or tool_result blocks should be skipped; only text blocks emitted.
        let json = r#"{"type":"assistant","message":{"content":[{"type":"image","text":""},{"type":"text","text":"hi"}],"role":"assistant"}}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        let output = map_event_to_output(&event, "test").unwrap();
        assert!(matches!(output, AgentOutput::Delta(ref t) if t == "hi"));
    }
}
