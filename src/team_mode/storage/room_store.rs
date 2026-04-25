use std::fs;
use std::path::PathBuf;

use crate::error::Result;
use crate::team_mode::data_dir;
use crate::team_mode::domain::Room;
use crate::team_mode::storage::{
    acquire_lock_path, ensure_dir, read_json_opt, validate_storage_name,
};
use crate::util::atomic_write::atomic_write_json;

/// Single-room-per-team store. Current product only uses the "main" room.
#[derive(Debug, Clone)]
pub struct RoomStore {
    base_dir: PathBuf,
}

impl RoomStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    fn room_path(&self, team_id: &str) -> PathBuf {
        data_dir::room_file(&self.base_dir, team_id)
    }

    fn lock_for(&self, team_id: &str) -> PathBuf {
        data_dir::lock_path(&self.base_dir, &format!("room-{team_id}"))
    }

    pub fn save(&self, team_id: &str, room: &Room) -> Result<()> {
        validate_storage_name(team_id)?;
        if let Some(rt) = &room.team_id {
            validate_storage_name(rt)?;
        }
        let team_dir = data_dir::team_dir(&self.base_dir, team_id);
        ensure_dir(&team_dir)?;
        ensure_dir(&self.base_dir.join(data_dir::DIR_LOCKS))?;
        let _lock = acquire_lock_path(&self.lock_for(team_id))?;
        atomic_write_json(&self.room_path(team_id), room)
    }

    pub fn get(&self, team_id: &str) -> Result<Option<Room>> {
        validate_storage_name(team_id)?;
        read_json_opt(&self.room_path(team_id))
    }

    pub fn delete(&self, team_id: &str) -> Result<()> {
        validate_storage_name(team_id)?;
        ensure_dir(&self.base_dir.join(data_dir::DIR_LOCKS))?;
        let _lock = acquire_lock_path(&self.lock_for(team_id))?;
        let path = self.room_path(team_id);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{Room, RoomKind, RoomStatus};

    fn sample_room(team: &str) -> Room {
        Room {
            id: "main".into(),
            team_id: Some(team.into()),
            kind: RoomKind::Main,
            status: RoomStatus::Active,
        }
    }

    #[test]
    fn save_get_round_trip() {
        let dir = tempdir().unwrap();
        let store = RoomStore::new(dir.path());
        store.save("demo", &sample_room("demo")).unwrap();

        let loaded = store.get("demo").unwrap().unwrap();
        assert_eq!(loaded.id, "main");
        assert_eq!(loaded.team_id.as_deref(), Some("demo"));
    }

    #[test]
    fn delete_removes_room_file() {
        let dir = tempdir().unwrap();
        let store = RoomStore::new(dir.path());
        store.save("demo", &sample_room("demo")).unwrap();
        store.delete("demo").unwrap();
        assert!(store.get("demo").unwrap().is_none());
    }

    #[test]
    fn different_teams_isolated() {
        let dir = tempdir().unwrap();
        let store = RoomStore::new(dir.path());
        store.save("alpha", &sample_room("alpha")).unwrap();
        store.save("bravo", &sample_room("bravo")).unwrap();
        assert!(store.get("alpha").unwrap().is_some());
        assert!(store.get("bravo").unwrap().is_some());
        store.delete("alpha").unwrap();
        assert!(store.get("alpha").unwrap().is_none());
        assert!(store.get("bravo").unwrap().is_some());
    }
}
