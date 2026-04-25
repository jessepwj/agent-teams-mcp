use crate::team_mode::service::{
    InboxService, MemberService, MessageService, RoomService, TeamService,
};
use crate::team_mode::storage::{MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore};

#[derive(Debug, Clone)]
pub struct TeamModeWebState {
    pub(crate) base_dir: std::path::PathBuf,
    pub(crate) team_service: TeamService,
    pub(crate) member_service: MemberService,
    pub(crate) room_service: RoomService,
    pub(crate) message_service: MessageService,
    pub(crate) inbox_service: InboxService,
}

impl TeamModeWebState {
    pub fn new(base_dir: impl Into<std::path::PathBuf>) -> Self {
        let base_dir = base_dir.into();
        let team_store = TeamStore::new(base_dir.clone());
        let member_store = MemberStore::new(base_dir.clone());
        let room_store = RoomStore::new(base_dir.clone());
        let message_store = MessageStore::new(base_dir.clone());
        let projection_store = ProjectionStore::with_message_store(message_store.clone());

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

        Self {
            base_dir,
            team_service,
            member_service,
            room_service,
            message_service,
            inbox_service,
        }
    }

    pub(crate) fn base_dir(&self) -> &std::path::Path {
        &self.base_dir
    }
}
