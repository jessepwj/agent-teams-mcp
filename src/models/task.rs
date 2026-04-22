//! Task models compatible with Claude Code's
//! `~/.claude/tasks/{team-name}/{id}.json` format.

use serde::{Deserialize, Serialize};

/// Task status with forward-only transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Deleted,
}

impl TaskStatus {
    /// Check whether transitioning from `self` to `target` is valid.
    ///
    /// Allowed transitions:
    /// - `pending -> in_progress -> completed`
    /// - Any state -> `deleted`
    pub fn can_transition_to(self, target: TaskStatus) -> bool {
        if target == TaskStatus::Deleted {
            return true;
        }
        matches!(
            (self, target),
            (TaskStatus::Pending, TaskStatus::InProgress)
                | (TaskStatus::InProgress, TaskStatus::Completed)
        )
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Deleted => write!(f, "deleted"),
        }
    }
}

/// A task persisted as `{id}.json` in the tasks directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFile {
    pub id: String,
    pub subject: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,

    pub status: TaskStatus,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,

    /// Task IDs that this task blocks (they cannot start until this completes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,

    /// Task IDs that must complete before this task can start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,

    /// Arbitrary metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Request to create a new task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest {
    pub subject: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Partial update for an existing task.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,

    /// Task IDs to add to `blocks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_blocks: Option<Vec<String>>,

    /// Task IDs to add to `blocked_by`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_blocked_by: Option<Vec<String>>,

    /// Metadata keys to merge (set value to `null` to delete a key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Filter criteria for listing tasks.
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub status: Option<TaskStatus>,
    pub owner: Option<String>,
    /// If true, only return tasks whose `blocked_by` is empty or all completed.
    pub unblocked_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_transitions() {
        assert!(TaskStatus::Pending.can_transition_to(TaskStatus::InProgress));
        assert!(TaskStatus::InProgress.can_transition_to(TaskStatus::Completed));
        assert!(!TaskStatus::Pending.can_transition_to(TaskStatus::Completed));
        assert!(!TaskStatus::Completed.can_transition_to(TaskStatus::InProgress));

        // Deleted is always allowed
        assert!(TaskStatus::Pending.can_transition_to(TaskStatus::Deleted));
        assert!(TaskStatus::InProgress.can_transition_to(TaskStatus::Deleted));
        assert!(TaskStatus::Completed.can_transition_to(TaskStatus::Deleted));
    }

    #[test]
    fn serde_round_trip_task() {
        let task = TaskFile {
            id: "1".into(),
            subject: "Fix auth bug".into(),
            description: Some("The login endpoint returns 500".into()),
            active_form: Some("Fixing auth bug".into()),
            status: TaskStatus::Pending,
            owner: None,
            blocks: vec!["2".into()],
            blocked_by: vec![],
            metadata: None,
        };

        let json = serde_json::to_string_pretty(&task).unwrap();
        let parsed: TaskFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "1");
        assert_eq!(parsed.blocks, vec!["2"]);
    }

    #[test]
    fn deserialize_claude_code_format() {
        let json = r#"{
            "id": "42",
            "subject": "Implement feature",
            "description": "Add caching",
            "activeForm": "Implementing feature",
            "status": "in_progress",
            "owner": "coder",
            "blocks": [],
            "blockedBy": ["41"]
        }"#;

        let task: TaskFile = serde_json::from_str(json).unwrap();
        assert_eq!(task.id, "42");
        assert_eq!(task.status, TaskStatus::InProgress);
        assert_eq!(task.owner.as_deref(), Some("coder"));
        assert_eq!(task.blocked_by, vec!["41"]);
    }
}
