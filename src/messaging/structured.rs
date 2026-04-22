//! Convenience constructors for [`StructuredMessage`] variants.
//!
//! These helpers make it easy to create well-formed structured messages
//! without manually constructing enum variants with `Option` fields.

use crate::models::StructuredMessage;

/// Create a task assignment message.
pub fn task_assignment(
    task_id: impl Into<String>,
    subject: impl Into<String>,
    description: impl Into<String>,
) -> StructuredMessage {
    StructuredMessage::TaskAssignment {
        task_id: task_id.into(),
        subject: subject.into(),
        description: Some(description.into()),
        assigned_by: None,
        timestamp: None,
    }
}

/// Create a shutdown request message.
pub fn shutdown_request(
    request_id: impl Into<String>,
    reason: impl Into<String>,
) -> StructuredMessage {
    StructuredMessage::ShutdownRequest {
        request_id: Some(request_id.into()),
        reason: Some(reason.into()),
    }
}

/// Create a shutdown approved response.
pub fn shutdown_approved(request_id: impl Into<String>) -> StructuredMessage {
    StructuredMessage::ShutdownApproved {
        request_id: Some(request_id.into()),
    }
}

/// Create an idle notification from an agent.
pub fn idle_notification(
    agent: impl Into<String>,
    last_task_id: Option<String>,
) -> StructuredMessage {
    StructuredMessage::IdleNotification {
        agent: agent.into(),
        last_task_id,
        idle_reason: None,
        timestamp: None,
    }
}

/// Create a plan approval request.
pub fn plan_approval_request(
    request_id: impl Into<String>,
    plan: impl Into<String>,
) -> StructuredMessage {
    StructuredMessage::PlanApprovalRequest {
        request_id: Some(request_id.into()),
        plan: plan.into(),
    }
}

/// Create a plan approval response.
pub fn plan_approval_response(
    request_id: impl Into<String>,
    approved: bool,
    feedback: Option<String>,
) -> StructuredMessage {
    StructuredMessage::PlanApprovalResponse {
        request_id: Some(request_id.into()),
        approved,
        feedback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_assignment_constructor() {
        let msg = task_assignment("1", "Fix bug", "Auth is broken");
        match msg {
            StructuredMessage::TaskAssignment {
                task_id,
                subject,
                description,
                ..
            } => {
                assert_eq!(task_id, "1");
                assert_eq!(subject, "Fix bug");
                assert_eq!(description.unwrap(), "Auth is broken");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn shutdown_request_constructor() {
        let msg = shutdown_request("req-1", "All tasks done");
        match msg {
            StructuredMessage::ShutdownRequest { request_id, reason } => {
                assert_eq!(request_id.unwrap(), "req-1");
                assert_eq!(reason.unwrap(), "All tasks done");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn shutdown_approved_constructor() {
        let msg = shutdown_approved("req-1");
        match msg {
            StructuredMessage::ShutdownApproved { request_id } => {
                assert_eq!(request_id.unwrap(), "req-1");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn idle_notification_constructor() {
        let msg = idle_notification("worker-1", Some("42".into()));
        match msg {
            StructuredMessage::IdleNotification {
                agent,
                last_task_id,
                ..
            } => {
                assert_eq!(agent, "worker-1");
                assert_eq!(last_task_id.unwrap(), "42");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn idle_notification_no_task() {
        let msg = idle_notification("worker-1", None);
        match msg {
            StructuredMessage::IdleNotification { last_task_id, .. } => {
                assert!(last_task_id.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn plan_approval_request_constructor() {
        let msg = plan_approval_request("req-2", "Step 1: read code\nStep 2: fix bug");
        match msg {
            StructuredMessage::PlanApprovalRequest { request_id, plan } => {
                assert_eq!(request_id.unwrap(), "req-2");
                assert!(plan.contains("Step 1"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn plan_approval_response_approved() {
        let msg = plan_approval_response("req-2", true, None);
        match msg {
            StructuredMessage::PlanApprovalResponse {
                request_id,
                approved,
                feedback,
            } => {
                assert_eq!(request_id.unwrap(), "req-2");
                assert!(approved);
                assert!(feedback.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn plan_approval_response_rejected_with_feedback() {
        let msg = plan_approval_response("req-2", false, Some("Add error handling".into()));
        match msg {
            StructuredMessage::PlanApprovalResponse {
                approved, feedback, ..
            } => {
                assert!(!approved);
                assert_eq!(feedback.unwrap(), "Add error handling");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn structured_messages_serialize_correctly() {
        let messages: Vec<StructuredMessage> = vec![
            task_assignment("1", "Do thing", "Details"),
            shutdown_request("r1", "Done"),
            shutdown_approved("r1"),
            idle_notification("w1", None),
            plan_approval_request("r2", "The plan"),
            plan_approval_response("r2", true, None),
        ];

        for msg in &messages {
            let json = serde_json::to_string(msg).unwrap();
            let parsed: StructuredMessage = serde_json::from_str(&json).unwrap();
            // Verify round-trip doesn't panic and summary works.
            let _ = parsed.summary();
        }
    }
}
