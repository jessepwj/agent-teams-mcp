//! Inbox-based messaging for agent teams.
//!
//! Provides the [`InboxManager`] trait and its file-system implementation
//! [`FileInboxManager`], which stores messages as JSON arrays in
//! `~/.claude/teams/{team}/inboxes/{agent}.json`.

pub mod structured;

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::error::{Error, Result};
use crate::models::InboxMessage;
use crate::util::atomic_write::atomic_write_json;
use crate::util::file_lock::FileLock;
use crate::util::validate_name;

/// Trait for inbox-based messaging between agents.
#[async_trait]
pub trait InboxManager: Send + Sync {
    /// Send a message to the recipient specified in `message.to`.
    async fn send_message(&self, team: &str, message: InboxMessage) -> Result<()>;

    /// Broadcast a plain-text message to all `members` except the sender.
    async fn broadcast(
        &self,
        team: &str,
        from: &str,
        content: &str,
        members: &[String],
    ) -> Result<()>;

    /// Read all messages in an agent's inbox.
    async fn read_inbox(&self, team: &str, agent: &str) -> Result<Vec<InboxMessage>>;

    /// Read only unread messages in an agent's inbox.
    async fn read_unread(&self, team: &str, agent: &str) -> Result<Vec<InboxMessage>>;

    /// Mark a specific message as read.
    async fn mark_read(&self, team: &str, agent: &str, message_id: &str) -> Result<()>;

    /// Poll for new unread messages, blocking up to `timeout`.
    async fn poll_inbox(
        &self,
        team: &str,
        agent: &str,
        timeout: Duration,
    ) -> Result<Vec<InboxMessage>>;

    /// Clear all messages from an agent's inbox.
    async fn clear_inbox(&self, team: &str, agent: &str) -> Result<()>;
}

/// File-system backed [`InboxManager`].
///
/// Layout:
/// ```text
/// {base_dir}/{team}/inboxes/{agent}.json   # message array
/// {base_dir}/{team}/inboxes/{agent}.lock   # flock guard
/// ```
#[derive(Debug, Clone)]
pub struct FileInboxManager {
    base_dir: PathBuf,
}

impl Default for FileInboxManager {
    fn default() -> Self {
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".claude")
            .join("teams");
        Self { base_dir }
    }
}

impl FileInboxManager {
    /// Create a new `FileInboxManager` rooted at `base_dir`.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Directory containing all inboxes for a team.
    fn inbox_dir(&self, team: &str) -> PathBuf {
        self.base_dir.join(team).join("inboxes")
    }

    /// Path to a specific agent's inbox JSON file.
    fn inbox_path(&self, team: &str, agent: &str) -> PathBuf {
        self.inbox_dir(team).join(format!("{agent}.json"))
    }

    /// Path to a specific agent's inbox lock file.
    fn lock_path(&self, team: &str, agent: &str) -> PathBuf {
        self.inbox_dir(team).join(format!("{agent}.lock"))
    }

    /// Ensure the inbox directory for a team exists.
    fn ensure_inbox_dir(inbox_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(inbox_dir)?;
        Ok(())
    }

    /// Read the inbox file, returning an empty vec if the file doesn't exist.
    fn read_inbox_file(path: &Path) -> Result<Vec<InboxMessage>> {
        match std::fs::read_to_string(path) {
            Ok(data) => {
                let messages: Vec<InboxMessage> = serde_json::from_str(&data)?;
                Ok(messages)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }
}

#[async_trait]
impl InboxManager for FileInboxManager {
    async fn send_message(&self, team: &str, message: InboxMessage) -> Result<()> {
        validate_name(team)?;
        validate_name(&message.to)?;

        let recipient = message.to.clone();
        let inbox_dir = self.inbox_dir(team);
        let lock_path = self.lock_path(team, &recipient);
        let inbox_path = self.inbox_path(team, &recipient);

        debug!(team, to = %recipient, id = %message.id, "sending message");

        // File locking is blocking, so offload to a blocking thread.
        tokio::task::spawn_blocking(move || {
            Self::ensure_inbox_dir(&inbox_dir)?;
            let _lock = FileLock::acquire(&lock_path)?;
            let mut messages = Self::read_inbox_file(&inbox_path)?;
            messages.push(message);
            atomic_write_json(&inbox_path, &messages)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn broadcast(
        &self,
        team: &str,
        from: &str,
        content: &str,
        members: &[String],
    ) -> Result<()> {
        validate_name(team)?;

        debug!(team, from, count = members.len(), "broadcasting message");

        for member in members {
            if member == from {
                continue;
            }
            let msg = InboxMessage::new(from, member.as_str(), content);
            self.send_message(team, msg).await?;
        }
        Ok(())
    }

    async fn read_inbox(&self, team: &str, agent: &str) -> Result<Vec<InboxMessage>> {
        validate_name(team)?;
        validate_name(agent)?;

        let inbox_dir = self.inbox_dir(team);
        let lock_path = self.lock_path(team, agent);
        let inbox_path = self.inbox_path(team, agent);

        tokio::task::spawn_blocking(move || {
            Self::ensure_inbox_dir(&inbox_dir)?;
            let _lock = FileLock::acquire(&lock_path)?;
            Self::read_inbox_file(&inbox_path)
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn read_unread(&self, team: &str, agent: &str) -> Result<Vec<InboxMessage>> {
        validate_name(team)?;
        validate_name(agent)?;

        let all = self.read_inbox(team, agent).await?;
        Ok(all.into_iter().filter(|m| !m.read).collect())
    }

    async fn mark_read(&self, team: &str, agent: &str, message_id: &str) -> Result<()> {
        validate_name(team)?;
        validate_name(agent)?;

        let inbox_dir = self.inbox_dir(team);
        let lock_path = self.lock_path(team, agent);
        let inbox_path = self.inbox_path(team, agent);
        let message_id = message_id.to_owned();

        debug!(team, agent, id = %message_id, "marking message as read");

        tokio::task::spawn_blocking(move || {
            Self::ensure_inbox_dir(&inbox_dir)?;
            let _lock = FileLock::acquire(&lock_path)?;
            let mut messages = Self::read_inbox_file(&inbox_path)?;

            let found = messages.iter_mut().find(|m| m.id == message_id);
            match found {
                Some(msg) => {
                    msg.read = true;
                    atomic_write_json(&inbox_path, &messages)?;
                    Ok(())
                }
                None => {
                    warn!(id = %message_id, "message not found in inbox");
                    Ok(())
                }
            }
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn poll_inbox(
        &self,
        team: &str,
        agent: &str,
        timeout: Duration,
    ) -> Result<Vec<InboxMessage>> {
        validate_name(team)?;
        validate_name(agent)?;

        let deadline = Instant::now() + timeout;

        loop {
            let unread = self.read_unread(team, agent).await?;
            if !unread.is_empty() {
                return Ok(unread);
            }

            if Instant::now() >= deadline {
                return Ok(Vec::new());
            }

            // Sleep before polling again, but don't overshoot the deadline.
            let remaining = deadline.saturating_duration_since(Instant::now());
            let sleep_dur = remaining.min(Duration::from_millis(500));
            if sleep_dur.is_zero() {
                return Ok(Vec::new());
            }
            tokio::time::sleep(sleep_dur).await;
        }
    }

    async fn clear_inbox(&self, team: &str, agent: &str) -> Result<()> {
        validate_name(team)?;
        validate_name(agent)?;

        let inbox_dir = self.inbox_dir(team);
        let lock_path = self.lock_path(team, agent);
        let inbox_path = self.inbox_path(team, agent);

        debug!(team, agent, "clearing inbox");

        tokio::task::spawn_blocking(move || {
            Self::ensure_inbox_dir(&inbox_dir)?;
            let _lock = FileLock::acquire(&lock_path)?;
            let empty: Vec<InboxMessage> = Vec::new();
            atomic_write_json(&inbox_path, &empty)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::StructuredMessage;

    fn make_manager(dir: &Path) -> FileInboxManager {
        FileInboxManager::new(dir)
    }

    #[tokio::test]
    async fn send_and_read_single_message() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let msg = InboxMessage::new("lead", "worker-1", "Hello worker!");
        mgr.send_message("test-team", msg).await.unwrap();

        let inbox = mgr.read_inbox("test-team", "worker-1").await.unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].from, "lead");
        assert_eq!(inbox[0].to, "worker-1");
        assert_eq!(inbox[0].content, "Hello worker!");
        assert!(!inbox[0].read);
    }

    #[tokio::test]
    async fn send_multiple_and_read_unread() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let msg1 = InboxMessage::new("lead", "worker-1", "Task 1");
        let msg2 = InboxMessage::new("lead", "worker-1", "Task 2");
        mgr.send_message("test-team", msg1).await.unwrap();
        mgr.send_message("test-team", msg2).await.unwrap();

        let unread = mgr.read_unread("test-team", "worker-1").await.unwrap();
        assert_eq!(unread.len(), 2);
    }

    #[tokio::test]
    async fn mark_read_filters_unread() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let msg = InboxMessage::new("lead", "worker-1", "Read me");
        let msg_id = msg.id.clone();
        mgr.send_message("test-team", msg).await.unwrap();

        mgr.mark_read("test-team", "worker-1", &msg_id)
            .await
            .unwrap();

        let unread = mgr.read_unread("test-team", "worker-1").await.unwrap();
        assert!(unread.is_empty());

        // But the message is still in the full inbox.
        let all = mgr.read_inbox("test-team", "worker-1").await.unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].read);
    }

    #[tokio::test]
    async fn broadcast_sends_to_all_except_sender() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let members = vec![
            "lead".to_string(),
            "worker-1".to_string(),
            "worker-2".to_string(),
        ];
        mgr.broadcast("test-team", "lead", "Announcement!", &members)
            .await
            .unwrap();

        // lead should NOT have a message
        let lead_inbox = mgr.read_inbox("test-team", "lead").await.unwrap();
        assert!(lead_inbox.is_empty());

        // Both workers should have the message
        let w1 = mgr.read_inbox("test-team", "worker-1").await.unwrap();
        assert_eq!(w1.len(), 1);
        assert_eq!(w1[0].content, "Announcement!");

        let w2 = mgr.read_inbox("test-team", "worker-2").await.unwrap();
        assert_eq!(w2.len(), 1);
        assert_eq!(w2[0].content, "Announcement!");
    }

    #[tokio::test]
    async fn clear_inbox_removes_all() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        mgr.send_message("t", InboxMessage::new("a", "b", "1"))
            .await
            .unwrap();
        mgr.send_message("t", InboxMessage::new("a", "b", "2"))
            .await
            .unwrap();

        mgr.clear_inbox("t", "b").await.unwrap();

        let inbox = mgr.read_inbox("t", "b").await.unwrap();
        assert!(inbox.is_empty());
    }

    #[tokio::test]
    async fn poll_returns_immediately_when_messages_exist() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        mgr.send_message("t", InboxMessage::new("a", "b", "hi"))
            .await
            .unwrap();

        let start = Instant::now();
        let msgs = mgr
            .poll_inbox("t", "b", Duration::from_secs(5))
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert_eq!(msgs.len(), 1);
        assert!(
            elapsed < Duration::from_secs(1),
            "poll should return immediately"
        );
    }

    #[tokio::test]
    async fn poll_times_out_with_empty_result() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let start = Instant::now();
        let msgs = mgr
            .poll_inbox("t", "agent", Duration::from_millis(600))
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert!(msgs.is_empty());
        assert!(
            elapsed >= Duration::from_millis(500),
            "should wait near timeout"
        );
        assert!(elapsed < Duration::from_secs(3), "should not wait too long");
    }

    #[tokio::test]
    async fn read_empty_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let inbox = mgr.read_inbox("team", "nobody").await.unwrap();
        assert!(inbox.is_empty());
    }

    #[tokio::test]
    async fn structured_message_round_trip_via_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let structured = StructuredMessage::TaskAssignment {
            task_id: "42".into(),
            subject: "Fix the bug".into(),
            description: Some("It's broken".into()),
            assigned_by: None,
            timestamp: None,
        };
        let msg = InboxMessage::from_structured("lead", "worker-1", &structured).unwrap();
        mgr.send_message("t", msg).await.unwrap();

        let inbox = mgr.read_inbox("t", "worker-1").await.unwrap();
        assert_eq!(inbox.len(), 1);

        let parsed = inbox[0].try_as_structured().unwrap();
        match parsed {
            StructuredMessage::TaskAssignment {
                task_id, subject, ..
            } => {
                assert_eq!(task_id, "42");
                assert_eq!(subject, "Fix the bug");
            }
            other => panic!("expected TaskAssignment, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mark_read_nonexistent_message_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        // Marking read on a non-existent inbox is fine (no panic, no error).
        mgr.mark_read("t", "agent", "nonexistent-id").await.unwrap();
    }
}
