use crate::domain::{Room, RoomKind, RoomStatus};
use crate::storage::JsonFileStore;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct RoomService {
    store: JsonFileStore,
}

#[derive(Debug, Clone)]
pub struct CreateRoom {
    pub id: String,
    pub team_id: String,
    pub kind: RoomKind,
}

impl RoomService {
    pub fn new(store: JsonFileStore) -> Self {
        Self { store }
    }

    pub fn create(&self, input: CreateRoom) -> Result<Room> {
        self.ensure_team_exists(&input.team_id)?;
        if input.id.trim().is_empty() {
            return Err(Error::Invalid("room id is required".to_string()));
        }

        let mut rooms = self.store.load_rooms()?;
        if rooms
            .iter()
            .any(|room| room.team_id == input.team_id && room.id == input.id)
        {
            return Err(Error::Conflict(format!(
                "room already exists: {}",
                input.id
            )));
        }

        let room = Room {
            id: input.id,
            team_id: input.team_id,
            kind: input.kind,
            status: RoomStatus::Active,
        };
        rooms.push(room.clone());
        self.store.save_rooms(&rooms)?;
        Ok(room)
    }

    pub fn get(&self, team_id: &str, room_id: &str) -> Result<Room> {
        self.store
            .load_rooms()?
            .into_iter()
            .find(|room| room.team_id == team_id && room.id == room_id)
            .ok_or_else(|| Error::NotFound(format!("room: {room_id}")))
    }

    pub fn list(&self, team_id: &str) -> Result<Vec<Room>> {
        Ok(self
            .store
            .load_rooms()?
            .into_iter()
            .filter(|room| room.team_id == team_id)
            .collect())
    }

    fn ensure_team_exists(&self, team_id: &str) -> Result<()> {
        if self
            .store
            .load_teams()?
            .iter()
            .any(|team| team.id == team_id)
        {
            Ok(())
        } else {
            Err(Error::NotFound(format!("team: {team_id}")))
        }
    }
}
