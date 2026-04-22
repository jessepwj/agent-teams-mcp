//! Query and diff engine for checkpoints.
//!
//! Provides filtering, branch-scoped queries, timeline views,
//! and diff computation between two checkpoints.

use std::collections::HashSet;
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::error::{Error, Result};
use crate::models::checkpoint::{Checkpoint, CheckpointTask};

use super::storage::CheckpointStore;

/// Filter criteria for listing checkpoints.
#[derive(Debug, Clone, Default)]
pub struct CheckpointFilter {
    /// Filter by branch name.
    pub branch: Option<String>,
    /// Filter by agent name.
    pub agent: Option<String>,
    /// Filter by team name.
    pub team: Option<String>,
    /// Only include checkpoints created after this time.
    pub after: Option<DateTime<Utc>>,
    /// Only include checkpoints created before this time.
    pub before: Option<DateTime<Utc>>,
    /// Maximum number of results.
    pub limit: Option<usize>,
}

impl CheckpointFilter {
    /// Check whether a checkpoint matches this filter.
    pub fn matches(&self, checkpoint: &Checkpoint) -> bool {
        if let Some(ref branch) = self.branch {
            if &checkpoint.branch != branch {
                return false;
            }
        }
        if let Some(ref agent) = self.agent {
            if &checkpoint.session.agent_name != agent {
                return false;
            }
        }
        if let Some(ref team) = self.team {
            match &checkpoint.team {
                Some(t) if &t.team_name == team => {}
                _ => return false,
            }
        }
        if let Some(after) = self.after {
            if checkpoint.created_at <= after {
                return false;
            }
        }
        if let Some(before) = self.before {
            if checkpoint.created_at >= before {
                return false;
            }
        }
        true
    }
}

/// Diff between two checkpoints.
#[derive(Debug, Clone)]
pub struct CheckpointDiff {
    /// Commit SHA of the "from" checkpoint.
    pub from_sha: String,
    /// Commit SHA of the "to" checkpoint.
    pub to_sha: String,
    /// Files added between the two checkpoints.
    pub files_added: Vec<String>,
    /// Files removed between the two checkpoints.
    pub files_removed: Vec<String>,
    /// Files modified between the two checkpoints.
    pub files_modified: Vec<String>,
    /// Task changes.
    pub tasks_changed: Vec<TaskDelta>,
    /// Member changes.
    pub members_changed: Vec<MemberDelta>,
}

/// A change in task state between two checkpoints.
#[derive(Debug, Clone)]
pub struct TaskDelta {
    pub task_id: String,
    pub subject: String,
    pub change: DeltaKind,
    /// Previous status (if changed or removed).
    pub old_status: Option<String>,
    /// New status (if changed or added).
    pub new_status: Option<String>,
}

/// A change in team membership between two checkpoints.
#[derive(Debug, Clone)]
pub struct MemberDelta {
    pub name: String,
    pub change: DeltaKind,
}

/// Kind of change in a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaKind {
    Added,
    Removed,
    Changed,
}

impl std::fmt::Display for DeltaKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeltaKind::Added => write!(f, "added"),
            DeltaKind::Removed => write!(f, "removed"),
            DeltaKind::Changed => write!(f, "changed"),
        }
    }
}

/// Query engine for browsing checkpoints.
pub struct CheckpointQuery {
    store: CheckpointStore,
}

impl CheckpointQuery {
    /// Create a new query engine for the repository at `repo_path`.
    pub fn open(repo_path: &Path) -> Result<Self> {
        let store = CheckpointStore::open(repo_path)?;
        Ok(Self { store })
    }

    /// Create from an existing store.
    pub fn from_store(store: CheckpointStore) -> Self {
        Self { store }
    }

    /// Get a single checkpoint by commit SHA (or ref like HEAD, branch name).
    pub fn get(&self, commit_ref: &str) -> Result<Checkpoint> {
        self.store.load(commit_ref)
    }

    /// List checkpoints matching a filter.
    ///
    /// Results are sorted by creation time (newest first).
    pub fn list(&self, filter: &CheckpointFilter) -> Result<Vec<Checkpoint>> {
        let all = self.store.list()?;

        let mut filtered: Vec<Checkpoint> = all
            .into_iter()
            .map(|(_, ckpt)| ckpt)
            .filter(|ckpt| filter.matches(ckpt))
            .collect();

        // Sort by creation time descending (newest first)
        filtered.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        // Apply limit
        if let Some(limit) = filter.limit {
            filtered.truncate(limit);
        }

        Ok(filtered)
    }

    /// List checkpoints along a branch, walking the commit history.
    ///
    /// Uses `git2::Revwalk` to follow the branch's commit history,
    /// returning only commits that have checkpoint notes attached.
    pub fn by_branch(&self, branch: &str, limit: Option<usize>) -> Result<Vec<Checkpoint>> {
        let repo = self.store.repo();

        // Resolve branch to OID
        let branch_oid = repo
            .revparse_single(branch)
            .map_err(|e| Error::CheckpointError {
                reason: format!("Branch '{branch}' not found: {e}"),
            })?
            .id();

        let mut revwalk = repo.revwalk()?;
        revwalk.push(branch_oid)?;
        revwalk.set_sorting(git2::Sort::TIME)?;

        let mut results = Vec::new();
        let limit = limit.unwrap_or(usize::MAX);

        for oid_result in revwalk {
            if results.len() >= limit {
                break;
            }

            let oid = oid_result?;
            let sha = oid.to_string();

            // Try to load checkpoint for this commit
            match self.store.load(&sha) {
                Ok(checkpoint) => results.push(checkpoint),
                Err(Error::CheckpointNotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }

        Ok(results)
    }

    /// Get a reverse-chronological timeline of all checkpoints.
    pub fn timeline(&self, limit: Option<usize>) -> Result<Vec<Checkpoint>> {
        self.list(&CheckpointFilter {
            limit,
            ..Default::default()
        })
    }

    /// Compute the diff between two checkpoints.
    pub fn diff(&self, from_ref: &str, to_ref: &str) -> Result<CheckpointDiff> {
        let from = self.store.load(from_ref)?;
        let to = self.store.load(to_ref)?;

        Ok(compute_diff(&from, &to))
    }

    /// Get access to the underlying store.
    pub fn store(&self) -> &CheckpointStore {
        &self.store
    }
}

/// Compute a diff between two checkpoints.
pub fn compute_diff(from: &Checkpoint, to: &Checkpoint) -> CheckpointDiff {
    // File diff
    let from_files: HashSet<&str> = from.files.iter().map(|f| f.path.as_str()).collect();
    let to_files: HashSet<&str> = to.files.iter().map(|f| f.path.as_str()).collect();

    let files_added: Vec<String> = to_files
        .difference(&from_files)
        .map(|s| s.to_string())
        .collect();
    let files_removed: Vec<String> = from_files
        .difference(&to_files)
        .map(|s| s.to_string())
        .collect();

    // Files in both — check for hash changes
    let from_hashes: std::collections::HashMap<&str, Option<&str>> = from
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.content_hash.as_deref()))
        .collect();
    let to_hashes: std::collections::HashMap<&str, Option<&str>> = to
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.content_hash.as_deref()))
        .collect();

    let files_modified: Vec<String> = from_files
        .intersection(&to_files)
        .filter(|path| from_hashes.get(*path) != to_hashes.get(*path))
        .map(|s| s.to_string())
        .collect();

    // Task diff
    let from_tasks: std::collections::HashMap<&str, &CheckpointTask> =
        from.tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let to_tasks: std::collections::HashMap<&str, &CheckpointTask> =
        to.tasks.iter().map(|t| (t.id.as_str(), t)).collect();

    let mut tasks_changed = Vec::new();

    // Tasks added in `to`
    for (id, task) in &to_tasks {
        if !from_tasks.contains_key(id) {
            tasks_changed.push(TaskDelta {
                task_id: id.to_string(),
                subject: task.subject.clone(),
                change: DeltaKind::Added,
                old_status: None,
                new_status: Some(task.status.clone()),
            });
        }
    }

    // Tasks removed from `from`
    for (id, task) in &from_tasks {
        if !to_tasks.contains_key(id) {
            tasks_changed.push(TaskDelta {
                task_id: id.to_string(),
                subject: task.subject.clone(),
                change: DeltaKind::Removed,
                old_status: Some(task.status.clone()),
                new_status: None,
            });
        }
    }

    // Tasks in both — check for status changes
    for (id, from_task) in &from_tasks {
        if let Some(to_task) = to_tasks.get(id) {
            if from_task.status != to_task.status {
                tasks_changed.push(TaskDelta {
                    task_id: id.to_string(),
                    subject: to_task.subject.clone(),
                    change: DeltaKind::Changed,
                    old_status: Some(from_task.status.clone()),
                    new_status: Some(to_task.status.clone()),
                });
            }
        }
    }

    // Member diff
    let from_members: HashSet<&str> = from
        .team
        .as_ref()
        .map(|t| t.members.iter().map(|m| m.name.as_str()).collect())
        .unwrap_or_default();
    let to_members: HashSet<&str> = to
        .team
        .as_ref()
        .map(|t| t.members.iter().map(|m| m.name.as_str()).collect())
        .unwrap_or_default();

    let mut members_changed = Vec::new();

    for name in to_members.difference(&from_members) {
        members_changed.push(MemberDelta {
            name: name.to_string(),
            change: DeltaKind::Added,
        });
    }
    for name in from_members.difference(&to_members) {
        members_changed.push(MemberDelta {
            name: name.to_string(),
            change: DeltaKind::Removed,
        });
    }

    CheckpointDiff {
        from_sha: from.commit_sha.clone(),
        to_sha: to.commit_sha.clone(),
        files_added,
        files_removed,
        files_modified,
        tasks_changed,
        members_changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::checkpoint::{
        Checkpoint, CheckpointFile, CheckpointMember, CheckpointSession, CheckpointTask,
        CheckpointTeamState, FileRole,
    };
    use git2::Repository;
    use tempfile::TempDir;

    fn setup_test_repo() -> (TempDir, Repository) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = {
                let mut index = repo.index().unwrap();
                std::fs::write(dir.path().join("README.md"), "# Test").unwrap();
                index.add_path(Path::new("README.md")).unwrap();
                index.write().unwrap();
                index.write_tree().unwrap()
            };
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
                .unwrap();
        }

        (dir, repo)
    }

    fn add_commit(repo: &Repository, dir: &Path, filename: &str, content: &str, msg: &str) {
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        std::fs::write(dir.join(filename), content).unwrap();
        let tree_id = {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new(filename)).unwrap();
            index.write().unwrap();
            index.write_tree().unwrap()
        };
        let tree = repo.find_tree(tree_id).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent])
            .unwrap();
    }

    #[test]
    fn filter_matches() {
        let session = CheckpointSession::new("agent-1");
        let ckpt = Checkpoint::new("abc123", "feature-branch", session);

        let filter = CheckpointFilter {
            branch: Some("feature-branch".into()),
            ..Default::default()
        };
        assert!(filter.matches(&ckpt));

        let filter = CheckpointFilter {
            branch: Some("main".into()),
            ..Default::default()
        };
        assert!(!filter.matches(&ckpt));

        let filter = CheckpointFilter {
            agent: Some("agent-1".into()),
            ..Default::default()
        };
        assert!(filter.matches(&ckpt));

        let filter = CheckpointFilter {
            agent: Some("agent-2".into()),
            ..Default::default()
        };
        assert!(!filter.matches(&ckpt));
    }

    #[test]
    fn list_and_filter_checkpoints() {
        let (dir, repo) = setup_test_repo();
        let store = CheckpointStore::open(dir.path()).unwrap();
        let head_sha = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();

        // Save a checkpoint
        let session = CheckpointSession::new("agent-1");
        let mut ckpt = Checkpoint::new(&head_sha, "main", session);
        ckpt.commit_sha = head_sha.clone();
        store.save(&ckpt).unwrap();

        let query = CheckpointQuery::from_store(store);

        // List all
        let results = query.list(&CheckpointFilter::default()).unwrap();
        assert_eq!(results.len(), 1);

        // Filter by agent
        let results = query
            .list(&CheckpointFilter {
                agent: Some("agent-1".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(results.len(), 1);

        // Filter by wrong agent
        let results = query
            .list(&CheckpointFilter {
                agent: Some("agent-2".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn by_branch_walks_history() {
        let (dir, repo) = setup_test_repo();
        let store = CheckpointStore::open(dir.path()).unwrap();

        // Save checkpoint for initial commit
        let sha1 = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();
        let session1 = CheckpointSession::new("agent-1");
        let mut ckpt1 = Checkpoint::new(&sha1, "master", session1);
        ckpt1.commit_sha = sha1.clone();
        store.save(&ckpt1).unwrap();

        // Add a second commit with checkpoint
        add_commit(&repo, dir.path(), "file2.txt", "content", "Second commit");
        let sha2 = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();
        let session2 = CheckpointSession::new("agent-2");
        let mut ckpt2 = Checkpoint::new(&sha2, "master", session2);
        ckpt2.commit_sha = sha2.clone();
        store.save(&ckpt2).unwrap();

        // Add a third commit WITHOUT checkpoint
        add_commit(
            &repo,
            dir.path(),
            "file3.txt",
            "content",
            "Third commit (no checkpoint)",
        );

        // Re-open store so it sees all commits
        let query = CheckpointQuery::open(dir.path()).unwrap();
        let results = query.by_branch("HEAD", None).unwrap();

        // Should find 2 checkpoints (skipping the third commit which has no checkpoint)
        assert_eq!(results.len(), 2);
        let names: Vec<&str> = results
            .iter()
            .map(|c| c.session.agent_name.as_str())
            .collect();
        assert!(names.contains(&"agent-1"));
        assert!(names.contains(&"agent-2"));
    }

    #[test]
    fn diff_checkpoints() {
        let from = Checkpoint {
            id: "from".into(),
            commit_sha: "aaa".into(),
            branch: "main".into(),
            created_at: Utc::now(),
            session: CheckpointSession::new("agent"),
            team: Some(CheckpointTeamState {
                team_name: "team".into(),
                description: None,
                members: vec![
                    CheckpointMember {
                        name: "alice".into(),
                        agent_type: "coder".into(),
                    },
                    CheckpointMember {
                        name: "bob".into(),
                        agent_type: "reviewer".into(),
                    },
                ],
            }),
            tasks: vec![
                CheckpointTask {
                    id: "1".into(),
                    subject: "Task 1".into(),
                    status: "pending".into(),
                    owner: None,
                },
                CheckpointTask {
                    id: "2".into(),
                    subject: "Task 2".into(),
                    status: "in_progress".into(),
                    owner: Some("alice".into()),
                },
            ],
            files: vec![
                CheckpointFile {
                    path: "a.rs".into(),
                    role: FileRole::Created,
                    content_hash: Some("hash1".into()),
                },
                CheckpointFile {
                    path: "b.rs".into(),
                    role: FileRole::Modified,
                    content_hash: Some("hash2".into()),
                },
            ],
            token_usage: None,
            tool_calls: vec![],
            metadata: Default::default(),
        };

        let to = Checkpoint {
            id: "to".into(),
            commit_sha: "bbb".into(),
            branch: "main".into(),
            created_at: Utc::now(),
            session: CheckpointSession::new("agent"),
            team: Some(CheckpointTeamState {
                team_name: "team".into(),
                description: None,
                members: vec![
                    CheckpointMember {
                        name: "alice".into(),
                        agent_type: "coder".into(),
                    },
                    CheckpointMember {
                        name: "charlie".into(),
                        agent_type: "tester".into(),
                    },
                ],
            }),
            tasks: vec![
                CheckpointTask {
                    id: "1".into(),
                    subject: "Task 1".into(),
                    status: "completed".into(),
                    owner: Some("alice".into()),
                },
                CheckpointTask {
                    id: "3".into(),
                    subject: "Task 3".into(),
                    status: "pending".into(),
                    owner: None,
                },
            ],
            files: vec![
                CheckpointFile {
                    path: "a.rs".into(),
                    role: FileRole::Modified,
                    content_hash: Some("hash1_modified".into()),
                },
                CheckpointFile {
                    path: "c.rs".into(),
                    role: FileRole::Created,
                    content_hash: Some("hash3".into()),
                },
            ],
            token_usage: None,
            tool_calls: vec![],
            metadata: Default::default(),
        };

        let diff = compute_diff(&from, &to);

        assert_eq!(diff.from_sha, "aaa");
        assert_eq!(diff.to_sha, "bbb");

        // Files: a.rs modified (hash changed), b.rs removed, c.rs added
        assert!(diff.files_added.contains(&"c.rs".to_string()));
        assert!(diff.files_removed.contains(&"b.rs".to_string()));
        assert!(diff.files_modified.contains(&"a.rs".to_string()));

        // Tasks: #1 changed (pending→completed), #2 removed, #3 added
        assert_eq!(diff.tasks_changed.len(), 3);
        let task1_delta = diff
            .tasks_changed
            .iter()
            .find(|t| t.task_id == "1")
            .unwrap();
        assert_eq!(task1_delta.change, DeltaKind::Changed);
        assert_eq!(task1_delta.old_status.as_deref(), Some("pending"));
        assert_eq!(task1_delta.new_status.as_deref(), Some("completed"));

        // Members: bob removed, charlie added
        assert_eq!(diff.members_changed.len(), 2);
        let bob = diff
            .members_changed
            .iter()
            .find(|m| m.name == "bob")
            .unwrap();
        assert_eq!(bob.change, DeltaKind::Removed);
        let charlie = diff
            .members_changed
            .iter()
            .find(|m| m.name == "charlie")
            .unwrap();
        assert_eq!(charlie.change, DeltaKind::Added);
    }

    #[test]
    fn timeline_returns_newest_first() {
        let (dir, repo) = setup_test_repo();
        let store = CheckpointStore::open(dir.path()).unwrap();

        let sha1 = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();
        let session1 = CheckpointSession::new("first");
        let mut ckpt1 = Checkpoint::new(&sha1, "master", session1);
        ckpt1.commit_sha = sha1.clone();
        ckpt1.created_at = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        store.save(&ckpt1).unwrap();

        add_commit(&repo, dir.path(), "f2.txt", "c", "second");
        let sha2 = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();
        let session2 = CheckpointSession::new("second");
        let mut ckpt2 = Checkpoint::new(&sha2, "master", session2);
        ckpt2.commit_sha = sha2.clone();
        ckpt2.created_at = chrono::DateTime::parse_from_rfc3339("2025-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        store.save(&ckpt2).unwrap();

        let query = CheckpointQuery::from_store(store);
        let timeline = query.timeline(None).unwrap();

        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0].session.agent_name, "second"); // newest first
        assert_eq!(timeline[1].session.agent_name, "first");
    }
}
