//! Team Mode service layer.
//!
//! This layer binds the transcript-first storage model to business-level
//! operations for teams, members, rooms, messages, inboxes, and threads.

pub mod inbox_notifier;
pub mod inbox_service;
pub mod lead_pending;
pub mod member_service;
pub mod message_service;
pub mod room_service;
pub mod team_service;
pub mod thread_service;

pub use inbox_notifier::InboxNotifier;
pub use inbox_service::{InboxCount, InboxService};
pub use lead_pending::{LeadPendingEntry, LeadPendingWriter};
pub use member_service::{AddMemberRequest, MemberService, UpdateMemberRequest};
pub use message_service::{MessageService, SendMessageRequest};
pub use room_service::RoomService;
pub use team_service::{CreateTeamRequest, TeamService};
pub use thread_service::{ReplyToThreadRequest, ThreadService};

pub use crate::team_mode::storage::MemberRecord;

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{MemberKind, MessageKind, TeamStatus, VisibilityRule};
    use crate::team_mode::storage::{
        MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore,
    };

    #[test]
    fn team_mode_services_support_dispatch_inbox_and_thread_reply_flow() {
        let dir = tempdir().unwrap();
        let team_store = TeamStore::new(dir.path());
        let member_store = MemberStore::new(dir.path());
        let room_store = RoomStore::new(dir.path());
        let message_store = MessageStore::new(dir.path());
        let projection_store = ProjectionStore::new(dir.path());

        let team_service = TeamService::new(team_store.clone());
        let member_service = MemberService::new(member_store.clone(), team_store.clone());
        let room_service = RoomService::new(room_store.clone());
        let message_service = MessageService::new(
            message_store.clone(),
            member_store.clone(),
            room_store.clone(),
            team_store.clone(),
        );
        let inbox_service = InboxService::new(projection_store.clone(), message_store.clone());
        let thread_service = ThreadService::new(
            projection_store,
            message_store.clone(),
            message_service.clone(),
        );

        let team = team_service
            .create(CreateTeamRequest {
                id: Some("demo".into()),
                name: "demo".into(),
                description: Some("Main team".into()),
                cwd: None,
                lead_member_id: Some("lead".into()),
                owner_cc_pid: None,
            })
            .unwrap();
        assert_eq!(team.status, TeamStatus::Active);

        member_service
            .add(AddMemberRequest {
                team_id: team.id.clone(),
                name: "lead".into(),
                kind: MemberKind::Lead,
                role_label: "lead".into(),
                role_description: None,
                execution: None,
            })
            .unwrap();
        member_service
            .add(AddMemberRequest {
                team_id: team.id.clone(),
                name: "bob".into(),
                kind: MemberKind::Member,
                role_label: "worker".into(),
                role_description: None,
                execution: None,
            })
            .unwrap();

        let main_room = room_service.ensure_main_room(&team.id).unwrap();
        assert_eq!(main_room.id, "main");

        let dispatch = message_service
            .send(SendMessageRequest {
                team_id: team.id.clone(),
                room_id: main_room.id.clone(),
                sender: "lead".into(),
                kind: MessageKind::Dispatch,
                subject: Some("Review".into()),
                body: "Please review this patch @bob".into(),
                mentions: Vec::new(),
                visibility: vec![VisibilityRule::Team],
                audience_policy: None,
                reply_to: None,
                thread_id: None,
                expires_at: None,
            })
            .unwrap();
        assert_eq!(dispatch.effective_recipients, vec!["bob".to_string()]);

        let inbox = inbox_service.peek(&team.id, "bob", None).unwrap();
        assert_eq!(inbox.items.len(), 1);
        assert_eq!(inbox.items[0].message_id, dispatch.id.clone());

        let count = inbox_service.count(&team.id, "bob", None).unwrap();
        assert_eq!(count.unread, 1);

        inbox_service
            .read(&team.id, "bob", std::slice::from_ref(&dispatch.id))
            .unwrap();
        inbox_service
            .ack(&team.id, "bob", std::slice::from_ref(&dispatch.id))
            .unwrap();
        let after_ack = inbox_service.count(&team.id, "bob", None).unwrap();
        assert_eq!(after_ack.acked, 1);

        let reply = thread_service
            .reply(thread_service::ReplyToThreadRequest {
                team_id: team.id.clone(),
                sender: "bob".into(),
                reply_to_message_id: dispatch.id.clone(),
                subject: None,
                body: "Working on it @lead".into(),
                mentions: Vec::new(),
                visibility: vec![VisibilityRule::Team],
                audience_policy: None,
                expires_at: None,
            })
            .unwrap();
        assert_eq!(reply.kind, MessageKind::Reply);
        assert_eq!(reply.reply_to.as_deref(), Some(dispatch.id.as_str()));

        let thread = thread_service
            .read(&team.id, dispatch.thread_id.as_deref().unwrap())
            .unwrap();
        assert_eq!(thread.message_ids.len(), 2);
        let messages = thread_service
            .read_messages(&team.id, dispatch.thread_id.as_deref().unwrap())
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].reply_to.as_deref(), Some(dispatch.id.as_str()));
    }
}
