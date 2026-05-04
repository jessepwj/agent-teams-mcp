//! Gemini CLI backend -- spawns agents via the `gemini` CLI with conversation history accumulation.
//!
//! The Gemini CLI (`/opt/homebrew/bin/gemini` or any PATH-discovered binary) operates
//! in **headless (non-interactive) mode**: each turn spawns a fresh process, but the
//! session maintains **stateful conversation history** by accumulating (user, model) pairs
//! and prepending the full history to every new prompt sent to the CLI.
//!
//! ## Stateful via Conversation History Accumulation
//!
//! Since Gemini CLI has no programmatic JSON-RPC or persistent-process protocol suitable
//! for per-turn spawning, we implement multi-turn memory in the Session layer:
//!
//! 1. **System prompt injection**: The `system_prompt` is prepended once at the top of
//!    every constructed prompt using the `System:` prefix.
//!
//! 2. **History accumulation**: After each turn, the (user input, model response) pair is
//!    appended to `conversation_history`. On the next turn the full history is serialized
//!    into the prompt before the new user message.
//!
//! 3. **Sliding window**: To prevent unbounded growth, only the most recent
//!    `max_history_turns` pairs are kept (default: 50).
//!
//! ## Prompt format sent to gemini stdin on each turn
//!
//! ```text
//! System: <system_prompt>
//!
//! User: <turn1 input>
//! Model: <turn1 response>
//! User: <turn2 input>
//! Model: <turn2 response>
//! ...
//! User: <current input>
//! ```
//!
//! The `system_prompt` is also passed via the `-p` flag (Gemini CLI `--prompt` / system
//! context), so the model receives it both as a CLI argument and embedded in the history
//! block for maximum context continuity.
//!
//! ## Process model
//!
//! ```text
//! spawn()        -> first process  (init message -> stdout -> output_tx, history starts empty)
//! send_input()   -> kill old proc  -> new process  (full history + new input -> stdout -> output_tx)
//! send_input()   -> kill old proc  -> new process  (full history + new input -> stdout -> output_tx)
//! shutdown()     -> kill current proc, set alive=false
//! ```
//!
//! The `output_tx` channel is created once at `spawn()` time and reused across all
//! processes, so the orchestrator's `output_rx` remains valid for the session lifetime.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::{
    AgentBackend, AgentOutput, AgentSession, BackendType, SpawnConfig, apply_hide_window,
    send_agent_output,
};
use crate::{Error, Result};

/// Channel buffer size for agent output events.
const OUTPUT_CHANNEL_SIZE: usize = 256;

/// Default maximum number of conversation turns to retain in the sliding window.
const DEFAULT_MAX_HISTORY_TURNS: usize = 50;

// ---------------------------------------------------------------------------
// GeminiCliBackend  (factory)
// ---------------------------------------------------------------------------

/// Factory that creates Gemini CLI agent sessions by spawning the `gemini` binary.
#[derive(Debug, Clone)]
pub struct GeminiCliBackend {
    /// Path to the `gemini` CLI binary.
    gemini_path: PathBuf,
}

impl GeminiCliBackend {
    /// Locate the `gemini` binary on `$PATH` via `which`.
    pub fn new() -> Result<Self> {
        let path = which::which("gemini").map_err(|_| Error::CliNotFound {
            name: "gemini".into(),
        })?;
        Ok(Self { gemini_path: path })
    }

    /// Use an explicit path to the `gemini` binary.
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            gemini_path: path.into(),
        }
    }

    /// Build the CLI arguments from a [`SpawnConfig`] and the system prompt.
    ///
    /// User input is NOT passed as an argument -- it is piped via stdin by the caller.
    fn build_args(config: &SpawnConfig, system_prompt: &str) -> Vec<String> {
        let mut args = Vec::new();

        // System prompt via `-p`
        if !system_prompt.is_empty() {
            args.push("-p".into());
            args.push(system_prompt.to_string());
        }

        // Model via `-m` (default: gemini-2.5-pro)
        let model = config.model.as_deref().unwrap_or("gemini-2.5-pro");
        args.push("-m".into());
        args.push(model.to_string());

        // Always use `-y` (auto-approve): Gemini CLI is non-interactive in pipe mode,
        // so tool-call prompts would hang without this flag.
        args.push("-y".into());

        args
    }
}

#[async_trait]
impl AgentBackend for GeminiCliBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::GeminiCli
    }

    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>> {
        let agent_name = config.name.clone();
        // The prompt from SpawnConfig is used as the system prompt (injected via `-p`
        // on every turn). The initial user input is a brief init message -- NOT the
        // system prompt again -- to avoid sending it twice (once as `-p`, once as stdin).
        let system_prompt = config.prompt.clone();
        let initial_input = "Hello. Awaiting instructions.";

        info!(agent = %agent_name, "Spawning Gemini CLI agent");

        // Create the output channel (lives for the entire session)
        let (output_tx, output_rx) = mpsc::channel(OUTPUT_CHANNEL_SIZE);
        let alive = Arc::new(AtomicBool::new(true));

        // Build the first prompt (no history yet, just the init message)
        let first_prompt = build_history_prompt(&system_prompt, &[], initial_input);

        // Spawn the first process
        let (child, reader_handle) = spawn_gemini_process(
            &self.gemini_path,
            &config,
            &first_prompt,
            &system_prompt,
            output_tx.clone(),
            alive.clone(),
            &agent_name,
        )
        .await?;

        let session = GeminiCliSession {
            name: agent_name,
            gemini_path: self.gemini_path.clone(),
            config,
            system_prompt,
            conversation_history: Vec::new(),
            max_history_turns: DEFAULT_MAX_HISTORY_TURNS,
            pending_input: initial_input.to_string(),
            child: Some(child),
            reader_handle: Some(reader_handle),
            output_tx,
            output_rx: Some(output_rx),
            alive,
        };

        Ok(Box::new(session))
    }
}

// ---------------------------------------------------------------------------
// Prompt builder
// ---------------------------------------------------------------------------

/// Build the full prompt that is piped to gemini stdin.
///
/// Format:
/// ```text
/// System: <system_prompt>
///
/// User: <turn1 input>
/// Model: <turn1 response>
/// ...
/// User: <current_input>
/// ```
///
/// If `system_prompt` is empty, the "System:" block is omitted.
fn build_history_prompt(
    system_prompt: &str,
    history: &[(String, String)],
    current_input: &str,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    if !system_prompt.is_empty() {
        parts.push(format!("System: {system_prompt}"));
        parts.push(String::new()); // blank line separator
    }

    for (user_msg, model_msg) in history {
        parts.push(format!("User: {user_msg}"));
        parts.push(format!("Model: {model_msg}"));
    }

    parts.push(format!("User: {current_input}"));

    parts.join("\n")
}

// ---------------------------------------------------------------------------
// GeminiCliSession
// ---------------------------------------------------------------------------

/// A running Gemini CLI agent session with accumulated conversation history.
///
/// Each turn spawns a fresh `gemini` process. The full conversation history is
/// prepended to the prompt on every turn so the model retains memory across turns.
/// The `output_tx` channel is shared across all process lifetimes so the
/// orchestrator's receiver stays valid.
#[allow(dead_code)]
struct GeminiCliSession {
    /// Agent name.
    name: String,
    /// Path to the gemini binary.
    gemini_path: PathBuf,
    /// Original spawn config (for cwd, env, model, etc.).
    config: SpawnConfig,
    /// System prompt injected at the top of every turn's prompt and via `-p`.
    system_prompt: String,
    /// Accumulated conversation history: `(user_input, model_response)` pairs.
    /// "user" is what we sent; "model" is what Gemini replied.
    conversation_history: Vec<(String, String)>,
    /// Sliding-window limit: at most this many (user, model) pairs are kept.
    max_history_turns: usize,
    /// The user input for the currently executing turn; used to record history
    /// after we collect the response.
    pending_input: String,
    /// Current child process (if any).
    child: Option<Child>,
    /// Background reader task for current process.
    reader_handle: Option<JoinHandle<()>>,
    /// Shared output sender (reused across process lifetimes).
    output_tx: mpsc::Sender<AgentOutput>,
    /// Output receiver (taken once by the orchestrator).
    output_rx: Option<mpsc::Receiver<AgentOutput>>,
    /// Liveness flag.
    alive: Arc<AtomicBool>,
}

#[async_trait]
impl AgentSession for GeminiCliSession {
    fn name(&self) -> &str {
        &self.name
    }

    async fn send_input(&mut self, input: &str) -> Result<()> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err(Error::AgentNotAlive {
                name: self.name.clone(),
            });
        }

        // Kill the old process and reader (if still running).
        // Before killing, we have no way to get the response from the last turn
        // programmatically here (the reader task collected it). History recording
        // happens externally via `record_response()` called by the orchestrator.
        self.kill_current().await;

        // Build the full prompt: system + history + new input
        let full_prompt =
            build_history_prompt(&self.system_prompt, &self.conversation_history, input);

        // Remember the pending input so we can record it in history later
        self.pending_input = input.to_string();

        // Spawn a new process for this turn, piping the full prompt via stdin
        let (child, reader_handle) = spawn_gemini_process(
            &self.gemini_path,
            &self.config,
            &full_prompt,
            &self.system_prompt,
            self.output_tx.clone(),
            self.alive.clone(),
            &self.name,
        )
        .await?;

        self.child = Some(child);
        self.reader_handle = Some(reader_handle);

        Ok(())
    }

    fn output_receiver(&mut self) -> Option<mpsc::Receiver<AgentOutput>> {
        self.output_rx.take()
    }

    async fn is_alive(&self) -> bool {
        // Fast path: explicit shutdown sets `alive=false`.
        if !self.alive.load(Ordering::Relaxed) {
            return false;
        }
        // Slow path: gemini-cli spawns per-turn, so `child` is `None`
        // between turns. Treat that as "alive" (the session itself is
        // still valid; we just don't have a child process at this
        // instant). When a child IS present and the OS doesn't see it,
        // the worker was killed externally — flip to dead.
        let Some(pid) = self.child.as_ref().and_then(|c| c.id()) else {
            return true;
        };
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(
            sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
            true,
        );
        sys.process(sysinfo::Pid::from_u32(pid)).is_some()
    }

    async fn shutdown(&mut self) -> Result<()> {
        info!(agent = %self.name, "Shutting down Gemini CLI session");
        self.alive.store(false, Ordering::Relaxed);
        self.kill_current().await;
        Ok(())
    }

    async fn force_kill(&mut self) -> Result<()> {
        info!(agent = %self.name, "Force-killing Gemini CLI session");
        self.alive.store(false, Ordering::Relaxed);
        self.kill_current().await;
        Ok(())
    }
}

impl GeminiCliSession {
    /// Kill the current child process and abort the reader task.
    async fn kill_current(&mut self) {
        // Abort reader first so it stops reading stdout
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        // Kill the child process
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    /// Record a completed (input, response) pair into conversation history.
    ///
    /// Called by the orchestrator (or tests) after collecting the model's response.
    /// Applies the sliding-window: if history exceeds `max_history_turns`, the
    /// oldest entry is removed.
    #[allow(dead_code)]
    pub fn record_response(&mut self, response: &str) {
        self.conversation_history
            .push((self.pending_input.clone(), response.to_string()));

        // Sliding-window trim
        if self.conversation_history.len() > self.max_history_turns {
            let excess = self.conversation_history.len() - self.max_history_turns;
            self.conversation_history.drain(..excess);
        }
    }

    /// Return the current conversation history (for testing / inspection).
    #[cfg(test)]
    pub fn history(&self) -> &[(String, String)] {
        &self.conversation_history
    }
}

impl Drop for GeminiCliSession {
    fn drop(&mut self) {
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
        // Child process is killed by kill_on_drop(true) set during spawn.
    }
}

// ---------------------------------------------------------------------------
// Process spawning helper
// ---------------------------------------------------------------------------

/// Spawn a single Gemini CLI process, pipe `prompt` to its stdin, and start a
/// background reader task that forwards stdout lines to `output_tx`.
///
/// The `prompt` argument is the fully-assembled history+input string. The
/// `system_prompt` is additionally passed via `-p` for CLI-level context.
///
/// Returns the child process handle and the reader task handle.
async fn spawn_gemini_process(
    gemini_path: &std::path::Path,
    config: &SpawnConfig,
    prompt: &str,
    system_prompt: &str,
    output_tx: mpsc::Sender<AgentOutput>,
    alive: Arc<AtomicBool>,
    agent_name: &str,
) -> Result<(Child, JoinHandle<()>)> {
    let args = GeminiCliBackend::build_args(config, system_prompt);

    let mut cmd = Command::new(gemini_path);
    cmd.args(&args);

    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    // Capture stderr for logging instead of discarding it -- helps debug
    // auth failures, invalid models, and other CLI errors.
    cmd.stderr(std::process::Stdio::piped());

    if let Some(ref cwd) = config.cwd {
        cmd.current_dir(cwd);
    }

    for (k, v) in &config.env {
        cmd.env(k, v);
    }

    apply_hide_window(&mut cmd);
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| Error::SpawnFailed {
        name: agent_name.to_string(),
        reason: format!("Failed to start gemini process: {e}"),
    })?;

    // Write the full prompt (history + current input) to stdin, then close it
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().ok_or_else(|| Error::SpawnFailed {
            name: agent_name.to_string(),
            reason: "Failed to capture gemini stdin".into(),
        })?;
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| Error::GeminiCli {
                reason: format!("Failed to write to gemini stdin: {e}"),
            })?;
        // Drop stdin to close it -- signals EOF to the gemini process
    }

    // Take stdout for reading
    let stdout = child.stdout.take().ok_or_else(|| Error::SpawnFailed {
        name: agent_name.to_string(),
        reason: "Failed to capture gemini stdout".into(),
    })?;

    // Spawn a lightweight stderr drain task that logs any error output.
    // This runs fire-and-forget -- it will stop when the child process exits (EOF).
    if let Some(stderr) = child.stderr.take() {
        let stderr_name = agent_name.to_string();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line_buf = String::new();
            loop {
                line_buf.clear();
                match reader.read_line(&mut line_buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let trimmed = line_buf.trim();
                        if !trimmed.is_empty() {
                            warn!(agent = %stderr_name, stderr = %trimmed, "Gemini CLI stderr");
                        }
                    }
                }
            }
        });
    }

    // Spawn background reader task
    let reader_alive = alive.clone();
    let reader_name = agent_name.to_string();
    let reader_tx = output_tx;

    let reader_handle = tokio::spawn(async move {
        debug!(agent = %reader_name, "Gemini reader task started");
        let mut reader = BufReader::new(stdout);
        let mut line_buf = String::new();

        loop {
            if !reader_alive.load(Ordering::Relaxed) {
                break;
            }

            line_buf.clear();
            match reader.read_line(&mut line_buf).await {
                Ok(0) => {
                    // EOF -- process exited
                    debug!(agent = %reader_name, "Gemini stdout EOF");
                    let _ = send_agent_output(
                        &reader_tx,
                        AgentOutput::TurnComplete,
                        &reader_alive,
                        &reader_name,
                    )
                    .await;
                    break;
                }
                Ok(_) => {
                    let text = line_buf.trim_end_matches('\n').to_string();
                    if !text.is_empty()
                        && send_agent_output(
                            &reader_tx,
                            AgentOutput::Delta(text),
                            &reader_alive,
                            &reader_name,
                        )
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    warn!(agent = %reader_name, error = %e, "Error reading gemini stdout");
                    let _ = send_agent_output(
                        &reader_tx,
                        AgentOutput::Error(format!("Read error: {e}")),
                        &reader_alive,
                        &reader_name,
                    )
                    .await;
                    break;
                }
            }
        }
        debug!(agent = %reader_name, "Gemini reader task stopped");
    });

    Ok((child, reader_handle))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_backend_type() {
        let backend = GeminiCliBackend {
            gemini_path: PathBuf::from("/usr/bin/gemini"),
        };
        assert_eq!(backend.backend_type(), BackendType::GeminiCli);
    }

    #[test]
    fn test_spawn_config_to_args_default() {
        let config = SpawnConfig::new("test-agent", "You are a code reviewer");
        let args = GeminiCliBackend::build_args(&config, "You are a code reviewer");

        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"You are a code reviewer".to_string()));
        assert!(args.contains(&"-m".to_string()));
        assert!(args.contains(&"gemini-2.5-pro".to_string()));
        assert!(args.contains(&"-y".to_string()));
    }

    #[test]
    fn test_spawn_config_to_args_custom_model() {
        let mut config = SpawnConfig::new("test-agent", "system prompt");
        config.model = Some("gemini-2.5-flash".to_string());
        let args = GeminiCliBackend::build_args(&config, "system prompt");

        assert!(args.contains(&"-m".to_string()));
        assert!(args.contains(&"gemini-2.5-flash".to_string()));
        assert!(!args.contains(&"gemini-2.5-pro".to_string()));
    }

    #[test]
    fn test_spawn_config_to_args_empty_system_prompt() {
        let config = SpawnConfig::new("test-agent", "");
        let args = GeminiCliBackend::build_args(&config, "");

        // Empty system prompt should not include -p
        assert!(!args.contains(&"-p".to_string()));
    }

    #[test]
    fn test_backend_type_display() {
        assert_eq!(BackendType::GeminiCli.to_string(), "gemini-cli");
    }

    // -----------------------------------------------------------------------
    // History accumulation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_history_prompt_no_history() {
        let result = build_history_prompt("You are a helper", &[], "What is Rust?");
        let expected = "System: You are a helper\n\nUser: What is Rust?";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_build_history_prompt_with_history() {
        let history = vec![
            (
                "What is Rust?".to_string(),
                "Rust is a systems language.".to_string(),
            ),
            ("Is it fast?".to_string(), "Yes, very fast.".to_string()),
        ];
        let result = build_history_prompt("You are a helper", &history, "Tell me more.");

        assert!(result.starts_with("System: You are a helper\n\n"));
        assert!(result.contains("User: What is Rust?"));
        assert!(result.contains("Model: Rust is a systems language."));
        assert!(result.contains("User: Is it fast?"));
        assert!(result.contains("Model: Yes, very fast."));
        assert!(result.ends_with("User: Tell me more."));
    }

    #[test]
    fn test_build_history_prompt_empty_system_prompt() {
        let result = build_history_prompt("", &[], "Hello");
        // No "System:" block when system_prompt is empty
        assert!(!result.contains("System:"));
        assert_eq!(result, "User: Hello");
    }

    #[test]
    fn test_record_response_accumulates_history() {
        let config = SpawnConfig::new("test-agent", "You are a helper");
        let (tx, _rx) = mpsc::channel(8);
        let alive = Arc::new(AtomicBool::new(true));

        let mut session = GeminiCliSession {
            name: "test".to_string(),
            gemini_path: PathBuf::from("/usr/bin/gemini"),
            config,
            system_prompt: "You are a helper".to_string(),
            conversation_history: Vec::new(),
            max_history_turns: DEFAULT_MAX_HISTORY_TURNS,
            pending_input: "Hello?".to_string(),
            child: None,
            reader_handle: None,
            output_tx: tx,
            output_rx: None,
            alive,
        };

        // Simulate first turn response
        session.record_response("Hi there!");
        assert_eq!(session.history().len(), 1);
        assert_eq!(session.history()[0].0, "Hello?");
        assert_eq!(session.history()[0].1, "Hi there!");

        // Simulate second turn
        session.pending_input = "How are you?".to_string();
        session.record_response("I am doing well.");
        assert_eq!(session.history().len(), 2);
        assert_eq!(session.history()[1].0, "How are you?");
        assert_eq!(session.history()[1].1, "I am doing well.");
    }

    #[test]
    fn test_sliding_window_trims_old_history() {
        let config = SpawnConfig::new("test-agent", "sys");
        let (tx, _rx) = mpsc::channel(8);
        let alive = Arc::new(AtomicBool::new(true));

        let mut session = GeminiCliSession {
            name: "test".to_string(),
            gemini_path: PathBuf::from("/usr/bin/gemini"),
            config,
            system_prompt: "sys".to_string(),
            conversation_history: Vec::new(),
            max_history_turns: 3, // small window for testing
            pending_input: String::new(),
            child: None,
            reader_handle: None,
            output_tx: tx,
            output_rx: None,
            alive,
        };

        // Add 4 turns -- window is 3, so oldest should be evicted
        for i in 0..4usize {
            session.pending_input = format!("question {i}");
            session.record_response(&format!("answer {i}"));
        }

        assert_eq!(session.history().len(), 3);
        // Oldest entry (question 0) should have been evicted
        assert_eq!(session.history()[0].0, "question 1");
        // Newest entry should be present
        assert_eq!(session.history()[2].0, "question 3");
    }
}
