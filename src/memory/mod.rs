//! Cross-turn conversation memory for stateless backends.
//!
//! Provides [`ConversationMemory`] for accumulating turn records and
//! [`MemoryManager`] for file-based persistence. When enabled for an agent,
//! previous conversation context is prepended to each new input, giving
//! stateless backends (e.g., Gemini CLI) a form of multi-turn memory.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::util::atomic_write::atomic_write_json;
use crate::util::file_lock::FileLock;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Speaker role in a conversation turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "User"),
            Role::Assistant => write!(f, "Assistant"),
        }
    }
}

/// A single conversation turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub role: Role,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

/// Configuration for conversation memory limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Maximum number of turns to keep (oldest are evicted first).
    pub max_turns: usize,
    /// Maximum total characters in the formatted context string.
    pub budget_chars: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_turns: 5,
            budget_chars: 4000,
        }
    }
}

/// In-memory conversation history with configurable limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMemory {
    turns: Vec<TurnRecord>,
    config: MemoryConfig,
}

impl ConversationMemory {
    /// Create a new memory with the given config.
    pub fn new(config: MemoryConfig) -> Self {
        Self {
            turns: Vec::new(),
            config,
        }
    }

    /// Create a new memory with default configuration (5 turns, 4000 chars).
    pub fn with_defaults() -> Self {
        Self::new(MemoryConfig::default())
    }

    /// Record a new conversation turn, evicting the oldest if over `max_turns`.
    pub fn record_turn(&mut self, role: Role, content: impl Into<String>) {
        self.turns.push(TurnRecord {
            role,
            content: content.into(),
            timestamp: Utc::now(),
        });

        // Evict oldest turns if we exceed max_turns
        while self.turns.len() > self.config.max_turns {
            self.turns.remove(0);
        }
    }

    /// Format the stored turns as a context string for injection.
    ///
    /// Builds the context from most recent turns backward, stopping when
    /// adding the next turn would exceed `budget_chars`. Returns the
    /// context in chronological order.
    pub fn format_context(&self) -> String {
        if self.turns.is_empty() {
            return String::new();
        }

        let mut selected: Vec<String> = Vec::new();
        let mut total_chars = 0;
        let header = "[Conversation Context]\n";
        total_chars += header.len();

        // Walk from most recent to oldest
        for turn in self.turns.iter().rev() {
            let line = format!("{}: {}\n", turn.role, turn.content);
            if total_chars + line.len() > self.config.budget_chars {
                break;
            }
            total_chars += line.len();
            selected.push(line);
        }

        if selected.is_empty() {
            return String::new();
        }

        // Reverse to get chronological order
        selected.reverse();

        let mut out = header.to_string();
        for line in selected {
            out.push_str(&line);
        }
        out
    }

    /// Remove all recorded turns.
    pub fn clear(&mut self) {
        self.turns.clear();
    }

    /// Number of recorded turns.
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// Whether there are no recorded turns.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Current configuration.
    pub fn config(&self) -> &MemoryConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// MemoryManager — file-based persistence
// ---------------------------------------------------------------------------

/// File-based persistence for agent conversation memories.
///
/// Stores memories at `{teams_base}/{team}/memory/{agent}.json`.
#[derive(Debug, Clone)]
pub struct MemoryManager {
    teams_base: PathBuf,
}

impl MemoryManager {
    /// Create a new manager rooted at the given teams base directory.
    pub fn new(teams_base: PathBuf) -> Self {
        Self { teams_base }
    }

    /// Directory for a team's memory files.
    fn memory_dir(&self, team: &str) -> PathBuf {
        self.teams_base.join(team).join("memory")
    }

    /// File path for a specific agent's memory.
    fn memory_path(&self, team: &str, agent: &str) -> PathBuf {
        self.memory_dir(team).join(format!("{agent}.json"))
    }

    /// Lock file path for a team's memory directory.
    fn lock_path(&self, team: &str) -> PathBuf {
        self.memory_dir(team).join(".lock")
    }

    /// Save a conversation memory to disk (atomic write with file lock).
    pub fn save(&self, team: &str, agent: &str, memory: &ConversationMemory) -> Result<()> {
        let dir = self.memory_dir(team);
        std::fs::create_dir_all(&dir)?;

        let lock_path = self.lock_path(team);
        let _lock = FileLock::acquire(&lock_path)?;

        let path = self.memory_path(team, agent);
        atomic_write_json(&path, memory)?;
        Ok(())
    }

    /// Load a conversation memory from disk. Returns `None` if not found.
    pub fn load(&self, team: &str, agent: &str) -> Result<Option<ConversationMemory>> {
        let path = self.memory_path(team, agent);
        if !path.exists() {
            return Ok(None);
        }

        let dir = self.memory_dir(team);
        std::fs::create_dir_all(&dir)?;

        let lock_path = self.lock_path(team);
        let _lock = FileLock::acquire(&lock_path)?;

        let data = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Error::Other(format!("Memory file not found: {}", path.display()));
            }
            Error::Io(e)
        })?;

        let memory: ConversationMemory = serde_json::from_str(&data)?;
        Ok(Some(memory))
    }

    /// Delete a conversation memory from disk.
    pub fn delete(&self, team: &str, agent: &str) -> Result<()> {
        let path = self.memory_path(team, agent);
        if path.exists() {
            let dir = self.memory_dir(team);
            std::fs::create_dir_all(&dir)?;

            let lock_path = self.lock_path(team);
            let _lock = FileLock::acquire(&lock_path)?;

            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_format_context() {
        let mut mem = ConversationMemory::with_defaults();
        mem.record_turn(Role::User, "Hello");
        mem.record_turn(Role::Assistant, "Hi there!");

        let ctx = mem.format_context();
        assert!(ctx.starts_with("[Conversation Context]\n"));
        assert!(ctx.contains("User: Hello"));
        assert!(ctx.contains("Assistant: Hi there!"));
        assert_eq!(mem.len(), 2);
    }

    #[test]
    fn evicts_oldest_when_max_turns_exceeded() {
        let config = MemoryConfig {
            max_turns: 2,
            budget_chars: 10000,
        };
        let mut mem = ConversationMemory::new(config);

        mem.record_turn(Role::User, "first");
        mem.record_turn(Role::Assistant, "second");
        mem.record_turn(Role::User, "third");

        assert_eq!(mem.len(), 2);
        let ctx = mem.format_context();
        assert!(!ctx.contains("first"), "oldest turn should be evicted");
        assert!(ctx.contains("second"));
        assert!(ctx.contains("third"));
    }

    #[test]
    fn budget_truncation() {
        let config = MemoryConfig {
            max_turns: 100,
            budget_chars: 60, // very tight budget
        };
        let mut mem = ConversationMemory::new(config);

        mem.record_turn(Role::User, "AAAA BBBB CCCC DDDD");
        mem.record_turn(Role::Assistant, "EEEE FFFF GGGG HHHH");
        mem.record_turn(Role::User, "IIII JJJJ KKKK LLLL");

        let ctx = mem.format_context();
        // With a 60-char budget, not all 3 turns should fit
        // (header ~24 chars, each turn line ~30+ chars)
        assert!(ctx.len() <= 60 + 30); // some tolerance for the header
        // The most recent turns should be preferred
        assert!(ctx.contains("IIII") || ctx.contains("EEEE"));
    }

    #[test]
    fn empty_memory_formats_to_empty_string() {
        let mem = ConversationMemory::with_defaults();
        assert_eq!(mem.format_context(), "");
        assert!(mem.is_empty());
        assert_eq!(mem.len(), 0);
    }

    #[test]
    fn clear_removes_all_turns() {
        let mut mem = ConversationMemory::with_defaults();
        mem.record_turn(Role::User, "hello");
        mem.record_turn(Role::Assistant, "world");
        assert_eq!(mem.len(), 2);

        mem.clear();
        assert!(mem.is_empty());
        assert_eq!(mem.format_context(), "");
    }

    #[test]
    fn serde_round_trip() {
        let mut mem = ConversationMemory::with_defaults();
        mem.record_turn(Role::User, "question");
        mem.record_turn(Role::Assistant, "answer");

        let json = serde_json::to_string_pretty(&mem).unwrap();
        let parsed: ConversationMemory = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed.config().max_turns, 5);
        let ctx = parsed.format_context();
        assert!(ctx.contains("question"));
        assert!(ctx.contains("answer"));
    }

    #[test]
    fn memory_manager_save_load_delete() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());

        let mut mem = ConversationMemory::with_defaults();
        mem.record_turn(Role::User, "ping");
        mem.record_turn(Role::Assistant, "pong");

        // Save
        mgr.save("team1", "agent1", &mem).unwrap();

        // Load
        let loaded = mgr.load("team1", "agent1").unwrap().unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.format_context().contains("pong"));

        // Delete
        mgr.delete("team1", "agent1").unwrap();
        assert!(mgr.load("team1", "agent1").unwrap().is_none());
    }

    #[test]
    fn memory_manager_load_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = MemoryManager::new(dir.path().to_path_buf());

        let result = mgr.load("no-team", "no-agent").unwrap();
        assert!(result.is_none());
    }
}
