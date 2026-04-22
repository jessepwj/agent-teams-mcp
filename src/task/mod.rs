//! Task management: trait, file-based implementation, and dependency graph.

pub mod graph;

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::models::task::{CreateTaskRequest, TaskFile, TaskFilter, TaskStatus, TaskUpdate};
use crate::util::atomic_write::atomic_write_json;
use crate::util::file_lock::FileLock;
use crate::util::id_gen::next_task_id;
use crate::util::validate_name;

pub use graph::DependencyGraph;

/// Trait for task CRUD operations.
#[async_trait]
pub trait TaskManager: Send + Sync {
    /// Create a new task in the given team.
    async fn create_task(&self, team: &str, req: CreateTaskRequest) -> Result<TaskFile>;

    /// Update an existing task.
    async fn update_task(&self, team: &str, id: &str, update: TaskUpdate) -> Result<TaskFile>;

    /// Get a task by ID.
    async fn get_task(&self, team: &str, id: &str) -> Result<TaskFile>;

    /// List tasks, optionally filtered.
    async fn list_tasks(&self, team: &str, filter: Option<TaskFilter>) -> Result<Vec<TaskFile>>;

    /// Delete a task by ID.
    async fn delete_task(&self, team: &str, id: &str) -> Result<()>;
}

/// File-based task manager storing tasks as `{base_dir}/{team}/{id}.json`.
pub struct FileTaskManager {
    base_dir: PathBuf,
}

impl FileTaskManager {
    /// Create a new `FileTaskManager` with a custom base directory.
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Create a `FileTaskManager` using the default `~/.claude/tasks` directory.
    pub fn default_dir() -> Result<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| Error::Other("Could not determine home directory".into()))?;
        Ok(Self::new(home.join(".claude").join("tasks")))
    }

    /// Directory for a specific team's tasks.
    fn task_dir(&self, team: &str) -> PathBuf {
        self.base_dir.join(team)
    }

    /// File path for a specific task.
    fn task_path(&self, team: &str, id: &str) -> PathBuf {
        self.task_dir(team).join(format!("{id}.json"))
    }

    /// Lock file path for a team's task directory.
    fn lock_path(&self, team: &str) -> PathBuf {
        self.task_dir(team).join(".lock")
    }

    /// Read a task from disk (caller must hold the lock).
    fn read_task_at(path: &Path, team: &str, id: &str) -> Result<TaskFile> {
        let data = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::TaskNotFound {
                    team: team.into(),
                    id: id.into(),
                }
            } else {
                Error::Io(e)
            }
        })?;
        let task: TaskFile = serde_json::from_str(&data)?;
        Ok(task)
    }

    /// Read all tasks in a team directory (caller must hold the lock).
    fn read_all_tasks_in(dir: &Path) -> Result<Vec<TaskFile>> {
        if !dir.exists() {
            return Ok(vec![]);
        }

        let mut tasks = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Only parse numeric .json files
            if let Some(stem) = name.strip_suffix(".json")
                && stem.parse::<u64>().is_ok()
            {
                let data = std::fs::read_to_string(entry.path())?;
                let task: TaskFile = serde_json::from_str(&data)?;
                tasks.push(task);
            }
        }

        Ok(tasks)
    }

    /// Apply a `TaskUpdate` to an existing `TaskFile`, validating transitions.
    fn apply_update(task: &mut TaskFile, update: &TaskUpdate) -> Result<()> {
        // Status transition validation
        if let Some(new_status) = update.status {
            if !task.status.can_transition_to(new_status) {
                return Err(Error::InvalidStatusTransition {
                    from: task.status.to_string(),
                    to: new_status.to_string(),
                });
            }
            task.status = new_status;
        }

        if let Some(ref subject) = update.subject {
            task.subject.clone_from(subject);
        }
        if let Some(ref desc) = update.description {
            task.description = Some(desc.clone());
        }
        if let Some(ref af) = update.active_form {
            task.active_form = Some(af.clone());
        }
        if let Some(ref owner) = update.owner {
            task.owner = Some(owner.clone());
        }

        // Merge dependency additions
        if let Some(ref add_blocks) = update.add_blocks {
            for id in add_blocks {
                if !task.blocks.contains(id) {
                    task.blocks.push(id.clone());
                }
            }
        }
        if let Some(ref add_blocked_by) = update.add_blocked_by {
            for id in add_blocked_by {
                if !task.blocked_by.contains(id) {
                    task.blocked_by.push(id.clone());
                }
            }
        }

        // Merge metadata
        if let Some(ref new_meta) = update.metadata
            && let Some(obj) = new_meta.as_object()
        {
            let existing = task.metadata.get_or_insert_with(|| serde_json::json!({}));
            if let Some(existing_obj) = existing.as_object_mut() {
                for (k, v) in obj {
                    if v.is_null() {
                        existing_obj.remove(k);
                    } else {
                        existing_obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }

        Ok(())
    }

    /// When a task is completed, remove it from other tasks' `blocked_by` lists.
    fn cascade_completion(dir: &Path, completed_id: &str) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();

            if let Some(stem) = name.strip_suffix(".json")
                && stem.parse::<u64>().is_ok()
                && stem != completed_id
            {
                let data = std::fs::read_to_string(&path)?;
                let mut task: TaskFile = serde_json::from_str(&data)?;

                if task.blocked_by.contains(&completed_id.to_string()) {
                    task.blocked_by.retain(|id| id != completed_id);
                    atomic_write_json(&path, &task)?;
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl TaskManager for FileTaskManager {
    async fn create_task(&self, team: &str, req: CreateTaskRequest) -> Result<TaskFile> {
        validate_name(team)?;

        let dir = self.task_dir(team);
        let lock_path = self.lock_path(team);

        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;

            let _lock = FileLock::acquire(&lock_path)?;

            let id = next_task_id(&dir)?;

            let task = TaskFile {
                id: id.clone(),
                subject: req.subject,
                description: req.description,
                active_form: req.active_form,
                status: TaskStatus::Pending,
                owner: None,
                blocks: vec![],
                blocked_by: vec![],
                metadata: req.metadata,
            };

            let path = dir.join(format!("{id}.json"));
            atomic_write_json(&path, &task)?;

            Ok(task)
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn update_task(&self, team: &str, id: &str, update: TaskUpdate) -> Result<TaskFile> {
        validate_name(team)?;
        validate_name(id)?;

        let dir = self.task_dir(team);
        let lock_path = self.lock_path(team);
        let task_path = self.task_path(team, id);
        let team = team.to_string();
        let id = id.to_string();

        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;

            // Phase 1: Lock
            let _lock = FileLock::acquire(&lock_path)?;

            // Phase 2: Read
            let mut task = Self::read_task_at(&task_path, &team, &id)?;

            // Phase 3: Validate + Mutate
            let was_not_completed = task.status != TaskStatus::Completed;
            Self::apply_update(&mut task, &update)?;
            let is_now_completed = task.status == TaskStatus::Completed;

            // Phase 4: Atomic write
            atomic_write_json(&task_path, &task)?;

            // Cascade: if task just became completed, unblock dependents
            if was_not_completed && is_now_completed {
                Self::cascade_completion(&dir, &id)?;
            }

            Ok(task)
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn get_task(&self, team: &str, id: &str) -> Result<TaskFile> {
        validate_name(team)?;
        validate_name(id)?;

        let dir = self.task_dir(team);
        let lock_path = self.lock_path(team);
        let task_path = self.task_path(team, id);
        let team = team.to_string();
        let id = id.to_string();

        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;

            let _lock = FileLock::acquire(&lock_path)?;
            Self::read_task_at(&task_path, &team, &id)
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn list_tasks(&self, team: &str, filter: Option<TaskFilter>) -> Result<Vec<TaskFile>> {
        validate_name(team)?;

        let dir = self.task_dir(team);
        let lock_path = self.lock_path(team);

        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;

            let _lock = FileLock::acquire(&lock_path)?;
            let mut tasks = Self::read_all_tasks_in(&dir)?;

            if let Some(filter) = filter {
                if let Some(status) = filter.status {
                    tasks.retain(|t| t.status == status);
                }
                if let Some(ref owner) = filter.owner {
                    tasks.retain(|t| t.owner.as_deref() == Some(owner.as_str()));
                }
                if filter.unblocked_only {
                    tasks.retain(|t| t.blocked_by.is_empty());
                }
            }

            // Sort by ID numerically for consistent ordering
            tasks.sort_by(|a, b| {
                let a_num = a.id.parse::<u64>().unwrap_or(u64::MAX);
                let b_num = b.id.parse::<u64>().unwrap_or(u64::MAX);
                a_num.cmp(&b_num)
            });

            Ok(tasks)
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn delete_task(&self, team: &str, id: &str) -> Result<()> {
        validate_name(team)?;
        validate_name(id)?;

        let dir = self.task_dir(team);
        let lock_path = self.lock_path(team);
        let task_path = self.task_path(team, id);
        let team = team.to_string();
        let id = id.to_string();

        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;

            let _lock = FileLock::acquire(&lock_path)?;

            // Read to verify it exists
            let _task = Self::read_task_at(&task_path, &team, &id)?;

            std::fs::remove_file(&task_path)?;

            // Also remove from other tasks' blocked_by
            Self::cascade_completion(&dir, &id)?;

            Ok(())
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manager(dir: &Path) -> FileTaskManager {
        FileTaskManager::new(dir.to_path_buf())
    }

    #[tokio::test]
    async fn create_task_basic() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let task = mgr
            .create_task(
                "test-team",
                CreateTaskRequest {
                    subject: "Fix bug".into(),
                    description: Some("A nasty bug".into()),
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(task.id, "1");
        assert_eq!(task.subject, "Fix bug");
        assert_eq!(task.status, TaskStatus::Pending);
        assert!(task.owner.is_none());
    }

    #[tokio::test]
    async fn create_multiple_tasks_increments_id() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let t1 = mgr
            .create_task(
                "team",
                CreateTaskRequest {
                    subject: "Task 1".into(),
                    description: None,
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap();

        let t2 = mgr
            .create_task(
                "team",
                CreateTaskRequest {
                    subject: "Task 2".into(),
                    description: None,
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(t1.id, "1");
        assert_eq!(t2.id, "2");
    }

    #[tokio::test]
    async fn get_task_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let err = mgr.get_task("team", "999").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn update_task_status_transition() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let task = mgr
            .create_task(
                "team",
                CreateTaskRequest {
                    subject: "Work item".into(),
                    description: None,
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap();

        // pending -> in_progress
        let updated = mgr
            .update_task(
                "team",
                &task.id,
                TaskUpdate {
                    status: Some(TaskStatus::InProgress),
                    owner: Some("agent-1".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.status, TaskStatus::InProgress);
        assert_eq!(updated.owner.as_deref(), Some("agent-1"));

        // in_progress -> completed
        let completed = mgr
            .update_task(
                "team",
                &task.id,
                TaskUpdate {
                    status: Some(TaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(completed.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn update_task_invalid_transition() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let task = mgr
            .create_task(
                "team",
                CreateTaskRequest {
                    subject: "Work".into(),
                    description: None,
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap();

        // pending -> completed is not allowed (must go through in_progress)
        let err = mgr
            .update_task(
                "team",
                &task.id,
                TaskUpdate {
                    status: Some(TaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Invalid task status transition"));
    }

    #[tokio::test]
    async fn delete_task_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let task = mgr
            .create_task(
                "team",
                CreateTaskRequest {
                    subject: "To delete".into(),
                    description: None,
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap();

        mgr.delete_task("team", &task.id).await.unwrap();

        let err = mgr.get_task("team", &task.id).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn list_tasks_with_filter() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let t1 = mgr
            .create_task(
                "team",
                CreateTaskRequest {
                    subject: "Task 1".into(),
                    description: None,
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap();

        mgr.create_task(
            "team",
            CreateTaskRequest {
                subject: "Task 2".into(),
                description: None,
                active_form: None,
                metadata: None,
            },
        )
        .await
        .unwrap();

        // Move t1 to in_progress
        mgr.update_task(
            "team",
            &t1.id,
            TaskUpdate {
                status: Some(TaskStatus::InProgress),
                owner: Some("agent-1".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // List all
        let all = mgr.list_tasks("team", None).await.unwrap();
        assert_eq!(all.len(), 2);

        // Filter by status
        let pending = mgr
            .list_tasks(
                "team",
                Some(TaskFilter {
                    status: Some(TaskStatus::Pending),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].subject, "Task 2");

        // Filter by owner
        let owned = mgr
            .list_tasks(
                "team",
                Some(TaskFilter {
                    owner: Some("agent-1".into()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].subject, "Task 1");
    }

    #[tokio::test]
    async fn completion_cascades_to_blocked_by() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let t1 = mgr
            .create_task(
                "team",
                CreateTaskRequest {
                    subject: "Prerequisite".into(),
                    description: None,
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap();

        let t2 = mgr
            .create_task(
                "team",
                CreateTaskRequest {
                    subject: "Dependent".into(),
                    description: None,
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap();

        // Make t2 blocked by t1
        mgr.update_task(
            "team",
            &t2.id,
            TaskUpdate {
                add_blocked_by: Some(vec![t1.id.clone()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // Verify blocked
        let t2_read = mgr.get_task("team", &t2.id).await.unwrap();
        assert_eq!(t2_read.blocked_by, vec![t1.id.clone()]);

        // Complete t1: pending -> in_progress -> completed
        mgr.update_task(
            "team",
            &t1.id,
            TaskUpdate {
                status: Some(TaskStatus::InProgress),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        mgr.update_task(
            "team",
            &t1.id,
            TaskUpdate {
                status: Some(TaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // t2 should no longer be blocked
        let t2_unblocked = mgr.get_task("team", &t2.id).await.unwrap();
        assert!(
            t2_unblocked.blocked_by.is_empty(),
            "t2 should be unblocked after t1 completed, but blocked_by = {:?}",
            t2_unblocked.blocked_by
        );
    }

    #[tokio::test]
    async fn update_task_metadata_merge() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        let task = mgr
            .create_task(
                "team",
                CreateTaskRequest {
                    subject: "Meta task".into(),
                    description: None,
                    active_form: None,
                    metadata: Some(serde_json::json!({"key1": "val1", "key2": "val2"})),
                },
            )
            .await
            .unwrap();

        // Merge: update key1, delete key2, add key3
        let updated = mgr
            .update_task(
                "team",
                &task.id,
                TaskUpdate {
                    metadata: Some(serde_json::json!({
                        "key1": "updated",
                        "key2": null,
                        "key3": "new"
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let meta = updated.metadata.unwrap();
        assert_eq!(meta["key1"], "updated");
        assert!(meta.get("key2").is_none());
        assert_eq!(meta["key3"], "new");
    }

    #[tokio::test]
    async fn path_traversal_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(dir.path());

        // Team name with path separator
        let err = mgr
            .create_task(
                "../escape",
                CreateTaskRequest {
                    subject: "Bad".into(),
                    description: None,
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidName { .. }));

        // Empty team name
        let err = mgr
            .create_task(
                "",
                CreateTaskRequest {
                    subject: "Bad".into(),
                    description: None,
                    active_form: None,
                    metadata: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidName { .. }));

        // Task ID with path separator in get_task
        // First create a valid task so we have a valid team
        mgr.create_task(
            "team",
            CreateTaskRequest {
                subject: "Good".into(),
                description: None,
                active_form: None,
                metadata: None,
            },
        )
        .await
        .unwrap();

        let err = mgr.get_task("team", "../1").await.unwrap_err();
        assert!(matches!(err, Error::InvalidName { .. }));

        let err = mgr.delete_task("team", "..").await.unwrap_err();
        assert!(matches!(err, Error::InvalidName { .. }));
    }
}
