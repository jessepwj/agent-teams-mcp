use chrono::Utc;

use crate::error::{Error, Result};
use crate::team_mode::domain::{
    AudiencePolicy, Message, MessageKind, Thread, ThreadStatus, VisibilityRule,
};
use crate::team_mode::service::{MessageService, SendMessageRequest};
use crate::team_mode::storage::{MessageStore, ProjectionStore};
use crate::util::validate_name;

#[derive(Debug, Clone)]
pub struct ThreadService {
    projection_store: ProjectionStore,
    message_store: MessageStore,
    message_service: MessageService,
}

#[derive(Debug, Clone)]
pub struct ReplyToThreadRequest {
    pub team_id: String,
    /// Team-scoped name of the sender.
    pub sender: String,
    pub reply_to_message_id: String,
    pub subject: Option<String>,
    pub body: String,
    pub mentions: Vec<String>,
    pub visibility: Vec<VisibilityRule>,
    pub audience_policy: Option<AudiencePolicy>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
}

impl ThreadService {
    pub fn new(
        projection_store: ProjectionStore,
        message_store: MessageStore,
        message_service: MessageService,
    ) -> Self {
        Self {
            projection_store,
            message_store,
            message_service,
        }
    }

    pub fn read(&self, team_id: &str, thread_id: impl AsRef<str>) -> Result<Thread> {
        let thread_id = thread_id.as_ref();
        validate_name(team_id)?;
        validate_name(thread_id)?;

        let messages = self.projection_store.project_thread(team_id, thread_id)?;
        let Some(root) = messages.first() else {
            return Err(Error::Other(format!("thread '{thread_id}' not found")));
        };

        let subject = messages.iter().find_map(|m| m.subject.clone());
        let updated_at = messages
            .last()
            .map(|m| m.created_at)
            .unwrap_or(root.created_at);

        Ok(Thread {
            id: thread_id.to_string(),
            team_id: Some(team_id.to_string()),
            room_id: root.room_id.clone(),
            root_message_id: root.id.clone(),
            subject,
            message_ids: messages.iter().map(|m| m.id.clone()).collect(),
            status: ThreadStatus::Open,
            created_at: root.created_at,
            updated_at,
        })
    }

    pub fn read_messages(&self, team_id: &str, thread_id: impl AsRef<str>) -> Result<Vec<Message>> {
        let thread_id = thread_id.as_ref();
        validate_name(team_id)?;
        validate_name(thread_id)?;
        self.projection_store.project_thread(team_id, thread_id)
    }

    pub fn reply(&self, request: ReplyToThreadRequest) -> Result<Message> {
        validate_name(&request.team_id)?;
        validate_name(&request.sender)?;
        validate_name(&request.reply_to_message_id)?;
        let parent = self
            .message_store
            .get(&request.team_id, &request.reply_to_message_id)?
            .ok_or_else(|| {
                Error::Other(format!(
                    "reply target '{}' not found",
                    request.reply_to_message_id
                ))
            })?;

        self.message_service.send(SendMessageRequest {
            team_id: request.team_id,
            room_id: parent.room_id,
            sender: request.sender,
            kind: MessageKind::Reply,
            subject: request.subject,
            body: request.body,
            mentions: request.mentions,
            visibility: request.visibility,
            audience_policy: request.audience_policy,
            reply_to: Some(request.reply_to_message_id),
            thread_id: parent.thread_id,
            expires_at: request.expires_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{
        MemberKind, MemberStatus, Room, RoomKind, RoomStatus, Team, TeamStatus,
    };
    use crate::team_mode::service::MemberService;
    use crate::team_mode::service::member_service::AddMemberRequest;
    use crate::team_mode::storage::{MemberStore, RoomStore, TeamStore};

    fn seed(base_dir: &std::path::Path) {
        TeamStore::new(base_dir)
            .save(&Team {
                id: "demo".into(),
                name: "demo".into(),
                description: None,
                cwd: None,
                status: TeamStatus::Active,
                lead_member_id: Some("lead".into()),
                owner_cc_pid: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .unwrap();
        RoomStore::new(base_dir)
            .save(
                "demo",
                &Room {
                    id: "main".into(),
                    team_id: Some("demo".into()),
                    kind: RoomKind::Main,
                    status: RoomStatus::Active,
                },
            )
            .unwrap();
        let ms = MemberService::new(MemberStore::new(base_dir), TeamStore::new(base_dir));
        ms.add(AddMemberRequest {
            team_id: "demo".into(),
            name: "lead".into(),
            kind: MemberKind::Lead,
            role_label: "lead".into(),
            role_description: None,
            execution: None,
        })
        .unwrap();
        ms.add(AddMemberRequest {
            team_id: "demo".into(),
            name: "bob".into(),
            kind: MemberKind::Member,
            role_label: "worker".into(),
            role_description: None,
            execution: None,
        })
        .unwrap();
        let _ = MemberStatus::Active;
    }

    #[test]
    fn reply_posts_message_into_parent_thread() {
        let dir = tempdir().unwrap();
        seed(dir.path());
        let store = MessageStore::new(dir.path());
        let message_service = MessageService::new(
            store.clone(),
            MemberStore::new(dir.path()),
            RoomStore::new(dir.path()),
            TeamStore::new(dir.path()),
        );
        let parent = message_service
            .send(SendMessageRequest {
                team_id: "demo".into(),
                room_id: "main".into(),
                sender: "lead".into(),
                kind: MessageKind::Dispatch,
                subject: None,
                body: "@bob please".into(),
                mentions: vec![],
                visibility: vec![VisibilityRule::Team],
                audience_policy: None,
                reply_to: None,
                thread_id: None,
                expires_at: None,
            })
            .unwrap();

        let service = ThreadService::new(
            ProjectionStore::with_message_store(store.clone()),
            store,
            message_service,
        );

        let reply = service
            .reply(ReplyToThreadRequest {
                team_id: "demo".into(),
                sender: "bob".into(),
                reply_to_message_id: parent.id.clone(),
                subject: None,
                body: "done".into(),
                mentions: vec![],
                visibility: vec![VisibilityRule::Team],
                audience_policy: None,
                expires_at: None,
            })
            .unwrap();
        assert_eq!(reply.kind, MessageKind::Reply);
        assert_eq!(reply.reply_to.as_deref(), Some(parent.id.as_str()));
    }
}
