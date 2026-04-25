//! Tests that verify JSON format compatibility with Claude Code.
//!
//! These tests parse JSON fixtures matching the format produced by
//! Claude Code's team/task/inbox systems.

use agent_teams::models::{InboxMessage, StructuredMessage, TaskFile, TaskStatus, TeamConfig};

#[test]
fn parse_claude_code_team_config() {
    let json = r#"{
        "teamName": "my-project",
        "description": "Working on feature X",
        "members": [
            {
                "name": "team-lead",
                "agentId": "abc-123",
                "agentType": "researcher"
            },
            {
                "name": "coder",
                "agentId": "def-456",
                "agentType": "general-purpose",
                "prompt": "Implement the feature"
            }
        ]
    }"#;

    let config: TeamConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.team_name, "my-project");
    assert_eq!(config.description.as_deref(), Some("Working on feature X"));
    assert_eq!(config.members.len(), 2);
    assert_eq!(config.members[0].name(), "team-lead");
    assert_eq!(config.members[1].name(), "coder");
    assert!(config.members[1].is_teammate());
}

#[test]
fn parse_claude_code_task_file() {
    let json = r#"{
        "id": "42",
        "subject": "Implement caching layer",
        "description": "Add Redis caching to API endpoints",
        "activeForm": "Implementing caching layer",
        "status": "in_progress",
        "owner": "coder",
        "blocks": ["43", "44"],
        "blockedBy": ["41"],
        "metadata": {
            "priority": "high",
            "estimated_hours": 4
        }
    }"#;

    let task: TaskFile = serde_json::from_str(json).unwrap();
    assert_eq!(task.id, "42");
    assert_eq!(task.subject, "Implement caching layer");
    assert_eq!(task.status, TaskStatus::InProgress);
    assert_eq!(task.owner.as_deref(), Some("coder"));
    assert_eq!(task.blocks, vec!["43", "44"]);
    assert_eq!(task.blocked_by, vec!["41"]);
    assert!(task.metadata.is_some());
}

#[test]
fn parse_claude_code_task_minimal() {
    // Minimal task with only required fields
    let json = r#"{
        "id": "1",
        "subject": "Quick fix",
        "status": "pending"
    }"#;

    let task: TaskFile = serde_json::from_str(json).unwrap();
    assert_eq!(task.id, "1");
    assert!(task.description.is_none());
    assert!(task.owner.is_none());
    assert!(task.blocks.is_empty());
    assert!(task.blocked_by.is_empty());
}

#[test]
fn parse_claude_code_inbox_message() {
    let json = r#"{
        "id": "msg-001",
        "from": "team-lead",
        "to": "coder",
        "content": "Please start working on task #1",
        "summary": "Task assignment notification",
        "timestamp": "2025-01-15T10:30:00Z",
        "read": false
    }"#;

    let msg: InboxMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.from, "team-lead");
    assert_eq!(msg.to, "coder");
    assert!(!msg.read);
}

#[test]
fn parse_structured_message_task_assignment() {
    let json = r#"{
        "type": "task_assignment",
        "task_id": "1",
        "subject": "Fix authentication bug",
        "description": "The login endpoint returns 500 for valid credentials"
    }"#;

    let msg: StructuredMessage = serde_json::from_str(json).unwrap();
    match msg {
        StructuredMessage::TaskAssignment {
            task_id, subject, ..
        } => {
            assert_eq!(task_id, "1");
            assert_eq!(subject, "Fix authentication bug");
        }
        _ => panic!("Expected TaskAssignment"),
    }
}

#[test]
fn parse_structured_message_shutdown_request() {
    let json = r#"{
        "type": "shutdown_request",
        "request_id": "req-abc",
        "reason": "All tasks completed"
    }"#;

    let msg: StructuredMessage = serde_json::from_str(json).unwrap();
    match msg {
        StructuredMessage::ShutdownRequest { request_id, reason } => {
            assert_eq!(request_id.as_deref(), Some("req-abc"));
            assert_eq!(reason.as_deref(), Some("All tasks completed"));
        }
        _ => panic!("Expected ShutdownRequest"),
    }
}

#[test]
fn parse_structured_message_idle_notification() {
    let json = r#"{
        "type": "idle_notification",
        "agent": "worker-1",
        "last_task_id": "5"
    }"#;

    let msg: StructuredMessage = serde_json::from_str(json).unwrap();
    match msg {
        StructuredMessage::IdleNotification {
            agent,
            last_task_id,
            ..
        } => {
            assert_eq!(agent, "worker-1");
            assert_eq!(last_task_id.as_deref(), Some("5"));
        }
        _ => panic!("Expected IdleNotification"),
    }
}

#[test]
fn parse_structured_message_plan_approval() {
    let json = r#"{
        "type": "plan_approval_response",
        "request_id": "plan-001",
        "approved": false,
        "feedback": "Please add error handling for the API calls"
    }"#;

    let msg: StructuredMessage = serde_json::from_str(json).unwrap();
    match msg {
        StructuredMessage::PlanApprovalResponse {
            approved, feedback, ..
        } => {
            assert!(!approved);
            assert_eq!(
                feedback.as_deref(),
                Some("Please add error handling for the API calls")
            );
        }
        _ => panic!("Expected PlanApprovalResponse"),
    }
}

#[test]
fn team_config_round_trip_preserves_camel_case() {
    let config = TeamConfig {
        team_name: "my-team".into(),
        description: Some("Test".into()),
        created_at: None,
        lead_agent_id: None,
        lead_session_id: None,
        members: vec![],
    };

    let json = serde_json::to_string_pretty(&config).unwrap();
    assert!(json.contains("\"teamName\""));
    assert!(!json.contains("\"team_name\""));

    let parsed: TeamConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.team_name, "my-team");
}

#[test]
fn task_file_round_trip_preserves_camel_case() {
    let task = TaskFile {
        id: "1".into(),
        subject: "Test".into(),
        description: None,
        active_form: Some("Testing".into()),
        status: TaskStatus::Pending,
        owner: None,
        blocks: vec![],
        blocked_by: vec!["2".into()],
        metadata: None,
    };

    let json = serde_json::to_string_pretty(&task).unwrap();
    assert!(json.contains("\"activeForm\""));
    assert!(json.contains("\"blockedBy\""));
    assert!(!json.contains("\"active_form\""));
    assert!(!json.contains("\"blocked_by\""));

    let parsed: TaskFile = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.active_form.as_deref(), Some("Testing"));
}

#[test]
fn task_status_serializes_as_snake_case() {
    assert_eq!(
        serde_json::to_string(&TaskStatus::Pending).unwrap(),
        "\"pending\""
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::InProgress).unwrap(),
        "\"in_progress\""
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Completed).unwrap(),
        "\"completed\""
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Deleted).unwrap(),
        "\"deleted\""
    );
}
