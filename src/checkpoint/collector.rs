//! Checkpoint data collector — gathers context from git, team config, tasks, and session files.
//!
//! The collector assembles a [`Checkpoint`] by reading:
//! 1. **Git state**: HEAD commit SHA, branch name, diff file list (via `git2`)
//! 2. **Team state**: `~/.claude/teams/{team}/config.json`
//! 3. **Task state**: `~/.claude/tasks/{team}/*.json`
//! 4. **Session data** (optional extended): JSONL session files for tool calls and token usage

use std::path::{Path, PathBuf};

use git2::Repository;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::models::checkpoint::{
    Checkpoint, CheckpointFile, CheckpointSession, CheckpointTask, CheckpointTeamState, FileRole,
    MAX_TOOL_INPUT_SUMMARY_LEN, TokenUsage, ToolCallRecord, truncate_string,
};
use crate::models::task::TaskFile;
use crate::models::team::TeamConfig;

/// Collects data from multiple sources to build a checkpoint.
pub struct CheckpointCollector {
    repo_path: PathBuf,
    team_name: Option<String>,
    teams_base: PathBuf,
    tasks_base: PathBuf,
}

impl CheckpointCollector {
    /// Create a new collector for the repository at `repo_path`.
    pub fn new(repo_path: impl Into<PathBuf>) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            repo_path: repo_path.into(),
            team_name: None,
            teams_base: home.join(".claude").join("teams"),
            tasks_base: home.join(".claude").join("tasks"),
        }
    }

    /// Set the team name to collect team/task state for.
    pub fn with_team(mut self, team: impl Into<String>) -> Self {
        self.team_name = Some(team.into());
        self
    }

    /// Override the base directory for team configs.
    pub fn with_teams_base(mut self, path: impl Into<PathBuf>) -> Self {
        self.teams_base = path.into();
        self
    }

    /// Override the base directory for task files.
    pub fn with_tasks_base(mut self, path: impl Into<PathBuf>) -> Self {
        self.tasks_base = path.into();
        self
    }

    /// Collect a checkpoint for the current HEAD commit.
    ///
    /// Gathers git state (HEAD, branch, diff), and optionally team/task state
    /// if a team name was provided.
    pub fn collect(&self, session: CheckpointSession) -> Result<Checkpoint> {
        let repo = Repository::discover(&self.repo_path).map_err(|e| {
            if e.code() == git2::ErrorCode::NotFound {
                Error::NotAGitRepo {
                    path: self.repo_path.clone(),
                }
            } else {
                Error::Git(e)
            }
        })?;

        // Get HEAD commit
        let head = repo.head().map_err(|e| Error::CheckpointError {
            reason: format!("Failed to read HEAD: {e}"),
        })?;
        let commit = head.peel_to_commit().map_err(|e| Error::CheckpointError {
            reason: format!("HEAD does not point to a commit: {e}"),
        })?;
        let commit_sha = commit.id().to_string();

        // Get branch name
        let branch = head.shorthand().unwrap_or("HEAD").to_string();

        // Build file list from diff (HEAD vs parent)
        let files = self.collect_files(&repo, &commit)?;

        // Build checkpoint
        let mut checkpoint = Checkpoint::new(&commit_sha, &branch, session);
        checkpoint.commit_sha = commit_sha;
        checkpoint.files = files;

        // Collect team state if team name provided
        if let Some(ref team_name) = self.team_name {
            checkpoint.team = self.collect_team_state(team_name);
            checkpoint.tasks = self.collect_tasks(team_name);
        }

        Ok(checkpoint)
    }

    /// Convenience: collect from a `SessionState`.
    pub fn collect_from_session_state(
        &self,
        state: &crate::models::session::SessionState,
    ) -> Result<Checkpoint> {
        let session = CheckpointSession::from_session_state(state);
        self.collect(session)
    }

    /// Collect the file list from the diff between HEAD and its parent.
    fn collect_files(
        &self,
        repo: &Repository,
        commit: &git2::Commit<'_>,
    ) -> Result<Vec<CheckpointFile>> {
        let mut files = Vec::new();

        let tree = commit.tree()?;

        // Get parent tree (or empty tree for initial commit)
        let parent_tree = if commit.parent_count() > 0 {
            Some(commit.parent(0)?.tree()?)
        } else {
            None
        };

        let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;

        diff.foreach(
            &mut |delta, _progress| {
                let (path, role) = match delta.status() {
                    git2::Delta::Added => {
                        let p = delta.new_file().path().unwrap_or(Path::new(""));
                        (p.to_string_lossy().to_string(), FileRole::Created)
                    }
                    git2::Delta::Deleted => {
                        let p = delta.old_file().path().unwrap_or(Path::new(""));
                        (p.to_string_lossy().to_string(), FileRole::Deleted)
                    }
                    git2::Delta::Modified | git2::Delta::Renamed | git2::Delta::Copied => {
                        let p = delta.new_file().path().unwrap_or(Path::new(""));
                        (p.to_string_lossy().to_string(), FileRole::Modified)
                    }
                    _ => {
                        let p = delta.new_file().path().unwrap_or(Path::new(""));
                        (p.to_string_lossy().to_string(), FileRole::Referenced)
                    }
                };

                // Compute content hash from the blob OID
                let content_hash = if role != FileRole::Deleted {
                    let oid = delta.new_file().id();
                    if oid.is_zero() {
                        None
                    } else {
                        Some(oid.to_string())
                    }
                } else {
                    None
                };

                files.push(CheckpointFile {
                    path,
                    role,
                    content_hash,
                });

                true // continue iteration
            },
            None,
            None,
            None,
        )?;

        Ok(files)
    }

    /// Read team config from disk.
    fn collect_team_state(&self, team_name: &str) -> Option<CheckpointTeamState> {
        let config_path = self.teams_base.join(team_name).join("config.json");
        let data = std::fs::read_to_string(&config_path).ok()?;
        let config: TeamConfig = serde_json::from_str(&data).ok()?;
        Some(CheckpointTeamState::from_team_config(&config))
    }

    /// Read task files from disk.
    fn collect_tasks(&self, team_name: &str) -> Vec<CheckpointTask> {
        let task_dir = self.tasks_base.join(team_name);
        let entries = match std::fs::read_dir(&task_dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        let mut tasks = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".json") {
                if stem.parse::<u64>().is_ok() {
                    if let Ok(data) = std::fs::read_to_string(entry.path()) {
                        if let Ok(task) = serde_json::from_str::<TaskFile>(&data) {
                            tasks.push(CheckpointTask::from_task_file(&task));
                        }
                    }
                }
            }
        }

        tasks.sort_by(|a, b| {
            let a_num = a.id.parse::<u64>().unwrap_or(u64::MAX);
            let b_num = b.id.parse::<u64>().unwrap_or(u64::MAX);
            a_num.cmp(&b_num)
        });

        tasks
    }

    /// Parse a Claude Code JSONL session file for extended data.
    ///
    /// Extracts tool call records and token usage from the session log.
    /// Lines that fail to parse are silently skipped.
    pub fn parse_jsonl_session(path: &Path) -> Result<(Vec<ToolCallRecord>, Option<TokenUsage>)> {
        let content = std::fs::read_to_string(path).map_err(|e| Error::CheckpointError {
            reason: format!("Failed to read JSONL session file {}: {e}", path.display()),
        })?;

        let mut tool_calls = Vec::new();
        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;
        let mut total_cache_read: u64 = 0;
        let mut total_cache_write: u64 = 0;
        let mut has_usage = false;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let value: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Extract tool calls
            if let Some(tool_name) = value.get("tool_name").and_then(|v| v.as_str()) {
                let input_summary = value
                    .get("tool_input")
                    .map(|v| truncate_string(&v.to_string(), MAX_TOOL_INPUT_SUMMARY_LEN));

                let timestamp = value
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc));

                tool_calls.push(ToolCallRecord {
                    tool_name: tool_name.to_string(),
                    input_summary,
                    timestamp,
                });
            }

            // Also check for "type": "tool_use" format (Claude API format)
            if value.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                    let input_summary = value
                        .get("input")
                        .map(|v| truncate_string(&v.to_string(), MAX_TOOL_INPUT_SUMMARY_LEN));

                    tool_calls.push(ToolCallRecord {
                        tool_name: name.to_string(),
                        input_summary,
                        timestamp: None,
                    });
                }
            }

            // Extract token usage
            if let Some(usage) = value.get("usage") {
                has_usage = true;
                if let Some(n) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                    total_input_tokens += n;
                }
                if let Some(n) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
                    total_output_tokens += n;
                }
                if let Some(n) = usage
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_u64())
                {
                    total_cache_read += n;
                }
                if let Some(n) = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64())
                {
                    total_cache_write += n;
                }
            }
        }

        let token_usage = if has_usage {
            Some(TokenUsage {
                input_tokens: total_input_tokens,
                output_tokens: total_output_tokens,
                cache_read_tokens: if total_cache_read > 0 {
                    Some(total_cache_read)
                } else {
                    None
                },
                cache_write_tokens: if total_cache_write > 0 {
                    Some(total_cache_write)
                } else {
                    None
                },
            })
        } else {
            None
        };

        Ok((tool_calls, token_usage))
    }

    /// Compute SHA-256 hash of file contents at a given path.
    pub fn hash_file(path: &Path) -> Option<String> {
        let content = std::fs::read(path).ok()?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        Some(format!("{:x}", hasher.finalize()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn setup_test_repo_with_second_commit() -> (TempDir, Repository) {
        let (dir, repo) = setup_test_repo();

        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            std::fs::write(dir.path().join("file.txt"), "hello world").unwrap();
            std::fs::write(dir.path().join("new_file.rs"), "fn main() {}").unwrap();

            let tree_id = {
                let mut index = repo.index().unwrap();
                index.add_path(Path::new("file.txt")).unwrap();
                index.add_path(Path::new("new_file.rs")).unwrap();
                index.write().unwrap();
                index.write_tree().unwrap()
            };
            let tree = repo.find_tree(tree_id).unwrap();
            let parent = repo.head().unwrap().peel_to_commit().unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "Second commit", &tree, &[&parent])
                .unwrap();
        }

        (dir, repo)
    }

    #[test]
    fn collect_basic_checkpoint() {
        let (dir, _repo) = setup_test_repo();
        let collector = CheckpointCollector::new(dir.path());

        let session = CheckpointSession::new("test-agent");
        let checkpoint = collector.collect(session).unwrap();

        assert!(!checkpoint.commit_sha.is_empty());
        assert!(!checkpoint.branch.is_empty());
        assert_eq!(checkpoint.session.agent_name, "test-agent");
        // Initial commit should show file.txt as Created
        assert!(!checkpoint.files.is_empty());
        assert_eq!(checkpoint.files[0].path, "file.txt");
        assert_eq!(checkpoint.files[0].role, FileRole::Created);
    }

    #[test]
    fn collect_checkpoint_with_diff() {
        let (dir, _repo) = setup_test_repo_with_second_commit();
        let collector = CheckpointCollector::new(dir.path());

        let session = CheckpointSession::new("test-agent");
        let checkpoint = collector.collect(session).unwrap();

        // Second commit should show modifications and additions
        assert!(checkpoint.files.len() >= 2);

        let file_map: std::collections::HashMap<&str, &CheckpointFile> = checkpoint
            .files
            .iter()
            .map(|f| (f.path.as_str(), f))
            .collect();

        assert_eq!(file_map["file.txt"].role, FileRole::Modified);
        assert_eq!(file_map["new_file.rs"].role, FileRole::Created);
    }

    #[test]
    fn collect_with_team_state() {
        let (dir, _repo) = setup_test_repo();

        // Create mock team config
        let teams_dir = dir.path().join("teams").join("test-team");
        std::fs::create_dir_all(&teams_dir).unwrap();
        std::fs::write(
            teams_dir.join("config.json"),
            r#"{
                "teamName": "test-team",
                "description": "A test team",
                "members": [
                    {"name": "lead", "agentId": "001", "agentType": "team-lead"},
                    {"name": "coder", "agentId": "002", "agentType": "general-purpose", "prompt": "Code stuff"}
                ]
            }"#,
        )
        .unwrap();

        // Create mock tasks
        let tasks_dir = dir.path().join("tasks").join("test-team");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        std::fs::write(
            tasks_dir.join("1.json"),
            r#"{"id":"1","subject":"Task one","status":"pending"}"#,
        )
        .unwrap();

        let collector = CheckpointCollector::new(dir.path())
            .with_team("test-team")
            .with_teams_base(dir.path().join("teams"))
            .with_tasks_base(dir.path().join("tasks"));

        let session = CheckpointSession::new("lead");
        let checkpoint = collector.collect(session).unwrap();

        let team = checkpoint.team.as_ref().unwrap();
        assert_eq!(team.team_name, "test-team");
        assert_eq!(team.members.len(), 2);

        assert_eq!(checkpoint.tasks.len(), 1);
        assert_eq!(checkpoint.tasks[0].subject, "Task one");
    }

    #[test]
    fn parse_jsonl_session_basic() {
        let dir = TempDir::new().unwrap();
        let jsonl_path = dir.path().join("session.jsonl");
        std::fs::write(
            &jsonl_path,
            r#"{"tool_name":"Read","tool_input":{"path":"src/main.rs"},"timestamp":"2025-01-01T00:00:00Z"}
{"tool_name":"Write","tool_input":{"path":"src/lib.rs","content":"hello"}}
{"usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":200}}
{"invalid json line
{"usage":{"input_tokens":2000,"output_tokens":300}}
"#,
        )
        .unwrap();

        let (tool_calls, token_usage) =
            CheckpointCollector::parse_jsonl_session(&jsonl_path).unwrap();

        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].tool_name, "Read");
        assert!(tool_calls[0].timestamp.is_some());
        assert_eq!(tool_calls[1].tool_name, "Write");

        let usage = token_usage.unwrap();
        assert_eq!(usage.input_tokens, 3000);
        assert_eq!(usage.output_tokens, 800);
        assert_eq!(usage.cache_read_tokens, Some(200));
    }

    #[test]
    fn hash_file_works() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "hello world").unwrap();

        let hash = CheckpointCollector::hash_file(&path).unwrap();
        assert!(!hash.is_empty());
        // SHA-256 of "hello world" is deterministic
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn hash_file_nonexistent() {
        let result = CheckpointCollector::hash_file(Path::new("/nonexistent/file.txt"));
        assert!(result.is_none());
    }
}
