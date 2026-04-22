pub mod direct_service;
pub mod inbox_service;
pub mod member_service;
pub mod message_service;
pub mod room_service;
pub mod team_service;
pub mod thread_service;

use std::path::PathBuf;

pub use direct_service::*;
pub use inbox_service::*;
pub use member_service::*;
pub use message_service::*;
pub use room_service::*;
pub use team_service::*;
pub use thread_service::*;

use crate::Result;
use crate::storage::JsonFileStore;

#[derive(Debug, Clone)]
pub struct TeamModeServices {
    pub teams: TeamService,
    pub members: MemberService,
    pub rooms: RoomService,
    pub messages: MessageService,
    pub inbox: InboxService,
    pub threads: ThreadService,
    pub direct: DirectService,
}

impl TeamModeServices {
    pub fn new(data_dir: impl Into<PathBuf>) -> Result<Self> {
        let store = JsonFileStore::new(data_dir)?;
        Ok(Self::from_store(store))
    }

    pub fn from_store(store: JsonFileStore) -> Self {
        Self {
            teams: TeamService::new(store.clone()),
            members: MemberService::new(store.clone()),
            rooms: RoomService::new(store.clone()),
            messages: MessageService::new(store.clone()),
            inbox: InboxService::new(store.clone()),
            threads: ThreadService::new(store.clone()),
            direct: DirectService::new(store),
        }
    }
}
