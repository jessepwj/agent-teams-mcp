//! Backend abstraction layer for spawning and managing agent processes.
//!
//! Provides [`AgentBackend`] (factory trait) and [`AgentSession`] (per-agent handle trait)
//! with concrete implementations for Claude Code (`cc-sdk`) and Codex (JSON-RPC subprocess).

pub mod claude_code;
pub mod codex;
pub mod codex_protocol;
pub mod delegation;
pub mod gemini;
pub mod router;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::Result;

// ---------------------------------------------------------------------------
// BackendType
// ---------------------------------------------------------------------------

/// Identifies which backend an agent is running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendType {
    /// Claude Code via `cc-sdk` interactive client.
    ClaudeCode,
    /// OpenAI Codex via JSON-RPC 2.0 subprocess.
    Codex,
    /// Google Gemini via one-shot CLI subprocess.
    GeminiCli,
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::ClaudeCode => write!(f, "claude-code"),
            BackendType::Codex => write!(f, "codex"),
            BackendType::GeminiCli => write!(f, "gemini-cli"),
        }
    }
}

impl std::str::FromStr for BackendType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "claude-code" => Ok(BackendType::ClaudeCode),
            "codex" => Ok(BackendType::Codex),
            "gemini-cli" => Ok(BackendType::GeminiCli),
            other => Err(format!("unknown backend type: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// SpawnConfig
// ---------------------------------------------------------------------------

/// Configuration passed to [`AgentBackend::spawn`] when creating a new agent session.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// Human-readable agent name (used for logging and routing).
    pub name: String,
    /// Initial prompt / system instruction to send to the agent.
    pub prompt: String,
    /// Model override (backend-specific; `None` = use default).
    pub model: Option<String>,
    /// Working directory for the agent process.
    pub cwd: Option<PathBuf>,
    /// Maximum conversation turns before the agent auto-stops.
    pub max_turns: Option<i32>,
    /// Tools the agent is allowed to use (auto-approval list).
    pub allowed_tools: Vec<String>,
    /// Permission mode string (e.g. `"default"`, `"plan"`, `"bypassPermissions"`).
    pub permission_mode: Option<String>,
    /// Reasoning effort level for the model (Codex: `"low"`, `"medium"`, `"high"`, `"xhigh"`).
    /// When `None`, the backend's default / global config is used.
    pub reasoning_effort: Option<String>,
    /// Extra environment variables passed to the child process.
    pub env: HashMap<String, String>,
    /// Memory configuration for cross-turn context injection.
    pub memory_config: Option<crate::memory::MemoryConfig>,
    /// CLI tools this agent should delegate to via Bash.
    pub delegations: Vec<delegation::CliDelegation>,
}

impl SpawnConfig {
    /// Create a minimal spawn config with just a name and prompt.
    pub fn new(name: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            prompt: prompt.into(),
            model: None,
            cwd: None,
            max_turns: None,
            allowed_tools: Vec::new(),
            permission_mode: None,
            reasoning_effort: None,
            env: HashMap::new(),
            memory_config: None,
            delegations: Vec::new(),
        }
    }

    /// Start building a spawn config with required fields.
    ///
    /// ```rust
    /// use agent_teams::SpawnConfig;
    ///
    /// let config = SpawnConfig::builder("reviewer", "You are a code reviewer")
    ///     .model("gemini-2.5-flash")
    ///     .max_turns(5)
    ///     .build();
    /// ```
    pub fn builder(name: impl Into<String>, prompt: impl Into<String>) -> SpawnConfigBuilder {
        SpawnConfigBuilder {
            name: name.into(),
            prompt: prompt.into(),
            model: None,
            cwd: None,
            max_turns: None,
            allowed_tools: Vec::new(),
            permission_mode: None,
            reasoning_effort: None,
            env: HashMap::new(),
            memory_config: None,
            delegations: Vec::new(),
        }
    }
}

/// Builder for [`SpawnConfig`] with fluent setter methods for optional fields.
#[derive(Debug)]
pub struct SpawnConfigBuilder {
    name: String,
    prompt: String,
    model: Option<String>,
    cwd: Option<PathBuf>,
    max_turns: Option<i32>,
    allowed_tools: Vec<String>,
    permission_mode: Option<String>,
    reasoning_effort: Option<String>,
    env: HashMap<String, String>,
    memory_config: Option<crate::memory::MemoryConfig>,
    delegations: Vec<delegation::CliDelegation>,
}

impl SpawnConfigBuilder {
    /// Set the model override.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the working directory for the agent process.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Set the maximum conversation turns.
    pub fn max_turns(mut self, turns: i32) -> Self {
        self.max_turns = Some(turns);
        self
    }

    /// Set the tools the agent is allowed to use.
    pub fn allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = tools;
        self
    }

    /// Set the permission mode (e.g., `"default"`, `"plan"`, `"bypassPermissions"`).
    pub fn permission_mode(mut self, mode: impl Into<String>) -> Self {
        self.permission_mode = Some(mode.into());
        self
    }

    /// Set the reasoning effort level.
    pub fn reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }

    /// Add a single environment variable.
    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set all environment variables.
    pub fn env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Set memory configuration for cross-turn context injection.
    pub fn memory(mut self, config: crate::memory::MemoryConfig) -> Self {
        self.memory_config = Some(config);
        self
    }

    /// Add a CLI delegation for this agent.
    ///
    /// The agent's prompt will be augmented with instructions on how and when
    /// to invoke the delegated CLI tool via Bash.
    ///
    /// ```rust
    /// use agent_teams::backend::delegation::CliDelegation;
    /// use agent_teams::SpawnConfig;
    ///
    /// let config = SpawnConfig::builder("coder", "You write Rust code.")
    ///     .delegate(CliDelegation::codex())
    ///     .delegate(CliDelegation::gemini("gemini-2.5-pro"))
    ///     .build();
    ///
    /// assert_eq!(config.delegations.len(), 2);
    /// ```
    pub fn delegate(mut self, delegation: delegation::CliDelegation) -> Self {
        self.delegations.push(delegation);
        self
    }

    /// Build the [`SpawnConfig`].
    pub fn build(self) -> SpawnConfig {
        SpawnConfig {
            name: self.name,
            prompt: self.prompt,
            model: self.model,
            cwd: self.cwd,
            max_turns: self.max_turns,
            allowed_tools: self.allowed_tools,
            permission_mode: self.permission_mode,
            reasoning_effort: self.reasoning_effort,
            env: self.env,
            memory_config: self.memory_config,
            delegations: self.delegations,
        }
    }
}

// ---------------------------------------------------------------------------
// AgentOutput
// ---------------------------------------------------------------------------

/// Events emitted by a running agent session, delivered via an mpsc channel.
#[derive(Debug, Clone)]
pub enum AgentOutput {
    /// A complete text message from the agent (typically the final
    /// assistant reply for the turn). When both `Message` and `Delta`
    /// events arrive in the same turn, callers SHOULD prefer the last
    /// `Message` text and discard the streaming deltas to avoid
    /// duplicating the body.
    Message(String),
    /// A streaming text delta (partial token of the assistant reply).
    Delta(String),
    /// Side-band output from a tool / shell command invoked by the
    /// agent (e.g. codex's `EVENT_COMMAND_OUTPUT_DELTA` from `bash` or
    /// `local_shell`). This is *observability data*, not part of the
    /// final assistant reply — callers MUST NOT include it in the
    /// message body that gets posted to the room. Surface it via logs
    /// or a dedicated tool-call transcript instead.
    ToolOutput(String),
    /// The agent finished a turn (ready for next input).
    TurnComplete,
    /// The agent is idle / waiting for work.
    Idle,
    /// An error occurred inside the agent.
    Error(String),
}

// ---------------------------------------------------------------------------
// AgentBackend  (factory)
// ---------------------------------------------------------------------------

/// Factory trait: creates [`AgentSession`] instances for a specific backend.
///
/// Implementations are expected to be cheaply cloneable (or wrapped in `Arc`).
#[async_trait]
pub trait AgentBackend: Send + Sync {
    /// Which backend this factory produces.
    fn backend_type(&self) -> BackendType;

    /// Spawn a new agent and return a session handle.
    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>>;
}

// ---------------------------------------------------------------------------
// AgentSession  (per-agent handle)
// ---------------------------------------------------------------------------

/// A running agent session that can receive input and emit output.
#[async_trait]
pub trait AgentSession: Send + Sync {
    /// The agent's name (matches `SpawnConfig::name`).
    fn name(&self) -> &str;

    /// Send a follow-up user message to the agent (starts a new turn).
    async fn send_input(&mut self, input: &str) -> Result<()>;

    /// Take the output receiver.
    ///
    /// Returns `Some` on the first call, `None` thereafter (the receiver is moved
    /// out so that the caller owns it).
    fn output_receiver(&mut self) -> Option<tokio::sync::mpsc::Receiver<AgentOutput>>;

    /// Check whether the underlying process / connection is still alive.
    ///
    /// **Performance contract:** Implementations MUST return promptly with no I/O.
    /// This method may be called while the orchestrator holds internal locks (e.g.,
    /// inside [`TeamOrchestrator::are_alive`](crate::orchestrator::TeamOrchestrator::are_alive)).
    /// A typical implementation is a single [`AtomicBool::load`](std::sync::atomic::AtomicBool::load).
    async fn is_alive(&self) -> bool;

    /// Gracefully shut down the agent.
    async fn shutdown(&mut self) -> Result<()>;

    /// Forcefully kill the agent process.
    async fn force_kill(&mut self) -> Result<()>;

    /// The backend-assigned session identifier, if any. For Claude Code this
    /// is the UUID that names the `~/.claude/projects/<cwd>/<id>.jsonl` file
    /// the CLI writes for this session. Used by the web UI to display the
    /// worker's exact conversation rather than guessing by mtime when many
    /// sessions share the same project directory.
    ///
    /// Default `None` covers backends that don't expose a session id (codex
    /// and gemini-cli). Implementations should return promptly with no I/O.
    fn session_id(&self) -> Option<String> {
        None
    }

    /// Attempt to interrupt the agent's current turn so a follow-up message
    /// can be processed immediately. Returns `Ok(true)` if a protocol-level
    /// interrupt was issued, `Ok(false)` if the backend doesn't support
    /// interrupt or the agent is currently idle (no active turn). Errors
    /// indicate transport-level failures.
    ///
    /// Used by the `send_message(preempt=true)` MCP tool path. Backends that
    /// can't interrupt return the default `Ok(false)`; callers always treat
    /// the new message as enqueued normally regardless of interrupt outcome.
    async fn interrupt_turn(&self) -> Result<bool> {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// AgentOutputStream  (streaming adapter)
// ---------------------------------------------------------------------------

/// A stream of [`AgentOutput`] events from an agent session.
///
/// This wraps `tokio::sync::mpsc::Receiver<AgentOutput>` in a `Stream` for
/// ergonomic use with `StreamExt` combinators:
///
/// ```ignore
/// use tokio_stream::StreamExt;
///
/// let mut stream = orch.take_output_stream("team", "agent").await?
///     .expect("receiver not yet taken");
///
/// while let Some(event) = stream.next().await {
///     match event {
///         AgentOutput::Delta(text) => print!("{text}"),
///         AgentOutput::TurnComplete => break,
///         _ => {}
///     }
/// }
/// ```
pub type AgentOutputStream = tokio_stream::wrappers::ReceiverStream<AgentOutput>;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Timeout for control event delivery.
///
/// If a control event (`TurnComplete`, `Error`, `Idle`) cannot be delivered within
/// this duration, the session is marked dead. This prevents indefinite blocking when
/// the consumer stops draining the channel.
const CONTROL_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Send an output event from a backend reader/session task.
///
/// Control events (`TurnComplete`, `Error`, `Idle`) use `send().await` with a timeout
/// to guarantee delivery -- dropping these would violate liveness (callers may hang
/// forever). If delivery times out, the session is marked dead.
/// Data events (`Delta`, `Message`) use `try_send` -- dropping text is acceptable
/// under backpressure.
///
/// Returns `Err(())` if the channel is closed or control delivery timed out.
pub(crate) async fn send_agent_output(
    tx: &tokio::sync::mpsc::Sender<AgentOutput>,
    output: AgentOutput,
    alive: &Arc<AtomicBool>,
    agent_name: &str,
) -> std::result::Result<(), ()> {
    let is_control = matches!(
        output,
        AgentOutput::TurnComplete | AgentOutput::Error(_) | AgentOutput::Idle
    );

    if is_control {
        match tokio::time::timeout(CONTROL_SEND_TIMEOUT, tx.send(output)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => {
                debug!(agent = %agent_name, "Output channel closed");
                alive.store(false, Ordering::Relaxed);
                Err(())
            }
            Err(_) => {
                warn!(agent = %agent_name, "Control event delivery timed out, marking session dead");
                alive.store(false, Ordering::Relaxed);
                Err(())
            }
        }
    } else {
        match tx.try_send(output) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                warn!(agent = %agent_name, "Output channel full, dropping data message");
                Ok(())
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                debug!(agent = %agent_name, "Output channel closed");
                alive.store(false, Ordering::Relaxed);
                Err(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Ensures `Display` and `FromStr` stay in sync for all `BackendType` variants.
    /// If a new variant is added and either impl is not updated, this test fails.
    #[test]
    fn backend_type_display_fromstr_round_trip() {
        let variants = [
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];
        for variant in &variants {
            let s = variant.to_string();
            let parsed: BackendType = s.parse().unwrap_or_else(|e| {
                panic!("FromStr failed for Display output \"{s}\": {e}");
            });
            assert_eq!(*variant, parsed, "Round-trip failed for {s}");
        }
    }

    #[test]
    fn backend_type_fromstr_rejects_unknown() {
        assert!("unknown".parse::<BackendType>().is_err());
        assert!("Claude-Code".parse::<BackendType>().is_err()); // case-sensitive
    }

    #[test]
    fn spawn_config_builder_all_fields() {
        let config = SpawnConfig::builder("test-agent", "system prompt")
            .model("gpt-4.1")
            .cwd("/tmp")
            .max_turns(5)
            .allowed_tools(vec!["Read".into(), "Write".into()])
            .permission_mode("plan")
            .reasoning_effort("high")
            .env_var("API_KEY", "secret")
            .env_var("DEBUG", "1")
            .build();

        assert_eq!(config.name, "test-agent");
        assert_eq!(config.prompt, "system prompt");
        assert_eq!(config.model.as_deref(), Some("gpt-4.1"));
        assert_eq!(config.cwd.as_ref().unwrap().to_str().unwrap(), "/tmp");
        assert_eq!(config.max_turns, Some(5));
        assert_eq!(config.allowed_tools, vec!["Read", "Write"]);
        assert_eq!(config.permission_mode.as_deref(), Some("plan"));
        assert_eq!(config.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(config.env.len(), 2);
        assert_eq!(config.env["API_KEY"], "secret");
    }

    #[test]
    fn spawn_config_builder_minimal() {
        let config = SpawnConfig::builder("agent", "prompt").build();
        assert_eq!(config.name, "agent");
        assert_eq!(config.prompt, "prompt");
        assert!(config.model.is_none());
        assert!(config.cwd.is_none());
        assert!(config.max_turns.is_none());
        assert!(config.allowed_tools.is_empty());
        assert!(config.env.is_empty());
    }

    #[test]
    fn spawn_config_builder_is_debug() {
        // Verify Debug derive compiles and produces output
        let builder = SpawnConfig::builder("a", "b").model("gpt-4.1");
        let debug_str = format!("{builder:?}");
        assert!(debug_str.contains("SpawnConfigBuilder"));
    }
}
