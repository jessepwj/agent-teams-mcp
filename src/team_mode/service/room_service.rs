use crate::error::Result;
use crate::team_mode::domain::{Room, RoomKind, RoomStatus};
use crate::team_mode::storage::RoomStore;
use crate::util::validate_name;

#[derive(Debug, Clone)]
pub struct RoomService {
    room_store: RoomStore,
}

impl RoomService {
    pub fn new(room_store: RoomStore) -> Self {
        Self { room_store }
    }

    pub fn save(&self, team_id: &str, room: &Room) -> Result<()> {
        validate_name(team_id)?;
        self.room_store.save(team_id, room)
    }

    pub fn get(&self, team_id: &str) -> Result<Option<Room>> {
        self.room_store.get(team_id)
    }

    pub fn ensure_main_room(&self, team_id: impl AsRef<str>) -> Result<Room> {
        let team_id = team_id.as_ref();
        validate_name(team_id)?;

        if let Some(existing) = self.room_store.get(team_id)? {
            return Ok(existing);
        }

        let room = Room {
            id: "main".to_string(),
            team_id: Some(team_id.to_string()),
            kind: RoomKind::Main,
            status: RoomStatus::Active,
        };
        self.room_store.save(team_id, &room)?;
        Ok(room)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn ensure_main_room_is_idempotent() {
        let dir = tempdir().unwrap();
        let service = RoomService::new(RoomStore::new(dir.path()));

        let first = service.ensure_main_room("demo").unwrap();
        let second = service.ensure_main_room("demo").unwrap();

        assert_eq!(first.id, "main");
        assert_eq!(second.id, "main");
        assert_eq!(first.kind, RoomKind::Main);
        assert_eq!(first.team_id.as_deref(), Some("demo"));
    }

    #[test]
    fn different_teams_have_isolated_rooms() {
        let dir = tempdir().unwrap();
        let service = RoomService::new(RoomStore::new(dir.path()));
        let a = service.ensure_main_room("alpha").unwrap();
        let b = service.ensure_main_room("bravo").unwrap();
        assert_eq!(a.team_id.as_deref(), Some("alpha"));
        assert_eq!(b.team_id.as_deref(), Some("bravo"));
    }
}
