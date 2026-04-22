//! Auto-checkpoint trigger: creates checkpoints when tasks are completed.
//!
//! Integrates with `TeamOrchestrator::update_task()` to automatically
//! create a checkpoint whenever a task transitions to `Completed` status.
//! Checkpoint creation is fire-and-forget: errors are logged, never propagated.

use std::path::PathBuf;

use tracing::{info, warn};

use super::collector::CheckpointCollector;
use super::storage::CheckpointStore;
use crate::error::Result;
use crate::models::checkpoint::{Checkpoint, CheckpointSession};

/// Automatic checkpoint trigger for task completion events.
pub struct AutoCheckpointTrigger {
    repo_path: PathBuf,
    teams_base: PathBuf,
    tasks_base: PathBuf,
}

impl AutoCheckpointTrigger {
    /// Create a new auto-checkpoint trigger.
    ///
    /// - `repo_path`: Path to the git repository.
    /// - `teams_base`: Base directory for team configs (e.g. `~/.claude/teams`).
    /// - `tasks_base`: Base directory for task files (e.g. `~/.claude/tasks`).
    pub fn new(repo_path: PathBuf, teams_base: PathBuf, tasks_base: PathBuf) -> Self {
        Self {
            repo_path,
            teams_base,
            tasks_base,
        }
    }

    /// Create using defaults: repo_path from CWD, base dirs from `~/.claude/`.
    ///
    /// Returns `None` if CWD is not a git repo or home dir can't be determined.
    pub fn from_defaults() -> Option<Self> {
        let repo_path = std::env::current_dir().ok()?;
        // Quick check: is this a git repo?
        if !is_git_repo(&repo_path) {
            return None;
        }
        let home = dirs::home_dir()?;
        let claude_dir = home.join(".claude");
        Some(Self {
            repo_path,
            teams_base: claude_dir.join("teams"),
            tasks_base: claude_dir.join("tasks"),
        })
    }

    /// Fire when a task is completed.
    ///
    /// Creates a checkpoint attached to the current HEAD commit.
    /// Returns `Some(Checkpoint)` on success, `None` on failure (logged).
    ///
    /// This method is designed to be called fire-and-forget:
    /// ```ignore
    /// if let Some(trigger) = &self.auto_checkpoint {
    ///     trigger.on_task_completed("my-team", &task, "agent-1");
    /// }
    /// ```
    pub fn on_task_completed(
        &self,
        team_name: Option<&str>,
        task_subject: &str,
        agent_name: &str,
    ) -> Option<Checkpoint> {
        match self.try_create_checkpoint(team_name, task_subject, agent_name) {
            Ok(checkpoint) => {
                info!(
                    checkpoint_id = %checkpoint.id,
                    agent = agent_name,
                    task = task_subject,
                    "Auto-checkpoint created on task completion"
                );
                Some(checkpoint)
            }
            Err(e) => {
                warn!(
                    agent = agent_name,
                    task = task_subject,
                    error = %e,
                    "Auto-checkpoint failed (non-fatal)"
                );
                None
            }
        }
    }

    /// Internal: attempt to create and save a checkpoint.
    fn try_create_checkpoint(
        &self,
        team_name: Option<&str>,
        task_subject: &str,
        agent_name: &str,
    ) -> Result<Checkpoint> {
        let mut collector = CheckpointCollector::new(&self.repo_path)
            .with_teams_base(&self.teams_base)
            .with_tasks_base(&self.tasks_base);

        if let Some(team) = team_name {
            collector = collector.with_team(team);
        }

        let mut session = CheckpointSession::new(agent_name);
        session.prompt_summary = Some(format!("Task completed: {task_subject}"));

        // Enrich with session discovery (best-effort)
        let discovered = crate::util::session_discovery::discover_sessions(&self.repo_path);
        let mut checkpoint = collector.collect(session)?;

        // If we found sessions, try to get token usage from the most recent one
        if let Some(latest) = discovered.first() {
            match crate::util::session_discovery::parse_token_usage(&latest.path) {
                Ok(Some(usage)) => {
                    checkpoint.token_usage = Some(usage);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(error = %e, "Failed to parse session for auto-checkpoint token data");
                }
            }
        }

        // Add auto-trigger metadata
        checkpoint
            .metadata
            .insert("autoTriggered".to_string(), serde_json::json!(true));

        let store = CheckpointStore::open(&self.repo_path)?;
        store.save(&checkpoint)?;

        Ok(checkpoint)
    }
}

/// Quick check if a path is inside a git repository.
fn is_git_repo(path: &std::path::Path) -> bool {
    git2::Repository::discover(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Repository;
    use std::path::Path;
    use tempfile::TempDir;

    fn setup_test_repo() -> (TempDir, Repository) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = {
                let mut index = repo.index().unwrap();
                std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
                index.add_path(Path::new("file.txt")).unwrap();
                index.write().unwrap();
                index.write_tree().unwrap()
            };
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
                .unwrap();
        }

        (dir, repo)
    }

    #[test]
    fn auto_checkpoint_on_task_completed() {
        let (dir, _repo) = setup_test_repo();

        let trigger = AutoCheckpointTrigger::new(
            dir.path().to_path_buf(),
            dir.path().join("teams"),
            dir.path().join("tasks"),
        );

        let result = trigger.on_task_completed(None, "Fix authentication bug", "coder-1");

        assert!(result.is_some());
        let ckpt = result.unwrap();
        assert_eq!(ckpt.session.agent_name, "coder-1");
        assert!(ckpt.metadata.contains_key("autoTriggered"));

        // Verify it was saved to git notes
        let store = CheckpointStore::open(dir.path()).unwrap();
        let loaded = store.load(&ckpt.commit_sha).unwrap();
        assert_eq!(loaded.id, ckpt.id);
    }

    #[test]
    fn auto_checkpoint_not_git_repo() {
        let dir = TempDir::new().unwrap();

        let trigger = AutoCheckpointTrigger::new(
            dir.path().to_path_buf(),
            dir.path().join("teams"),
            dir.path().join("tasks"),
        );

        let result = trigger.on_task_completed(None, "Test", "agent");
        assert!(result.is_none()); // Graceful failure
    }

    #[test]
    fn is_git_repo_works() {
        let dir = TempDir::new().unwrap();
        assert!(!is_git_repo(dir.path()));

        let _repo = Repository::init(dir.path()).unwrap();
        assert!(is_git_repo(dir.path()));
    }
}
