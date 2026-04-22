//! Error types for agent-teams.

#![allow(clippy::enum_variant_names)]

use std::path::PathBuf;

/// Central error type for agent-teams operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Team '{name}' not found")]
    TeamNotFound { name: String },

    #[error("Team '{name}' already exists")]
    TeamAlreadyExists { name: String },

    #[error("Team '{name}' has active teammates, cannot delete")]
    TeamHasActiveMembers { name: String },

    #[error("Member '{member}' not found in team '{team}'")]
    MemberNotFound { team: String, member: String },

    #[error("Member '{member}' already exists in team '{team}'")]
    MemberAlreadyExists { team: String, member: String },

    #[error("Task '{id}' not found in team '{team}'")]
    TaskNotFound { team: String, id: String },

    #[error("Invalid task status transition: {from} -> {to}")]
    InvalidStatusTransition { from: String, to: String },

    #[error("Task '{id}' is blocked by: {blocked_by:?}")]
    TaskBlocked { id: String, blocked_by: Vec<String> },

    #[error("Adding dependency would create a cycle: {from} -> {to}")]
    CycleDetected { from: String, to: String },

    #[error("Inbox for agent '{agent}' not found in team '{team}'")]
    InboxNotFound { team: String, agent: String },

    #[error("Agent '{name}' is not alive")]
    AgentNotAlive { name: String },

    #[error("Agent '{name}' failed to spawn: {reason}")]
    SpawnFailed { name: String, reason: String },

    #[error("Backend '{backend}' not configured")]
    BackendNotConfigured { backend: String },

    #[error("CLI not found: {name}")]
    CliNotFound { name: String },

    #[error("Invalid name '{name}': {reason}")]
    InvalidName { name: String, reason: String },

    #[error("File lock failed on {path}: {reason}")]
    LockFailed { path: PathBuf, reason: String },

    #[error("Atomic write failed for {path}: {reason}")]
    AtomicWriteFailed { path: PathBuf, reason: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SDK error: {0}")]
    Sdk(#[from] cc_sdk::SdkError),

    #[error("Timeout after {seconds}s")]
    Timeout { seconds: u64 },

    #[error("Background task join error: {0}")]
    JoinError(String),

    #[error("Codex protocol error: {reason}")]
    CodexProtocol { reason: String },

    #[error("Gemini CLI error: {reason}")]
    GeminiCli { reason: String },

    #[error("Consensus failed: {reason}")]
    ConsensusFailed { reason: String },

    // Checkpoint errors
    #[cfg(feature = "checkpoint")]
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),

    #[error("Checkpoint not found for commit {sha}")]
    CheckpointNotFound { sha: String },

    #[error("Path is not a git repository: {path}")]
    NotAGitRepo { path: std::path::PathBuf },

    #[error("Checkpoint error: {reason}")]
    CheckpointError { reason: String },

    #[error("{0}")]
    Other(String),
}

/// Convenience Result alias.
pub type Result<T> = std::result::Result<T, Error>;
