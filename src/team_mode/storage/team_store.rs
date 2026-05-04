use std::fs;
use std::path::PathBuf;

use crate::error::Result;
use crate::team_mode::data_dir::{self, TEAM_FILE};
use crate::team_mode::domain::Team;
use crate::team_mode::storage::{
    acquire_lock_path, ensure_dir, read_json_opt, validate_storage_name,
};
use crate::util::atomic_write::atomic_write_json;

#[derive(Debug, Clone)]
pub struct TeamStore {
    base_dir: PathBuf,
}

impl TeamStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    fn team_file(&self, team_id: &str) -> PathBuf {
        data_dir::team_file(&self.base_dir, team_id)
    }

    pub fn team_dir(&self, team_id: &str) -> PathBuf {
        data_dir::team_dir(&self.base_dir, team_id)
    }

    fn lock_for_teams(&self) -> PathBuf {
        data_dir::lock_path(&self.base_dir, "teams")
    }

    pub fn save(&self, team: &Team) -> Result<()> {
        validate_storage_name(&team.id)?;
        let team_dir = data_dir::team_dir(&self.base_dir, &team.id);
        ensure_dir(&team_dir)?;
        ensure_dir(&self.base_dir.join(data_dir::DIR_LOCKS))?;
        let _lock = acquire_lock_path(&self.lock_for_teams())?;
        atomic_write_json(&self.team_file(&team.id), team)
    }

    pub fn get(&self, team_id: impl AsRef<str>) -> Result<Option<Team>> {
        let team_id = team_id.as_ref();
        validate_storage_name(team_id)?;
        read_json_opt(&self.team_file(team_id))
    }

    pub fn list(&self) -> Result<Vec<Team>> {
        let mut teams = Vec::new();
        if !self.base_dir.exists() {
            return Ok(teams);
        }
        for entry in fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Skip reserved non-team directories (.locks, etc).
            if matches!(
                path.file_name()
                    .and_then(|n| n.to_str()),
                Some(name) if name.starts_with('.')
            ) {
                continue;
            }
            let team_file = path.join(TEAM_FILE);
            if let Some(team) = read_json_opt::<Team>(&team_file)? {
                teams.push(team);
            }
        }
        teams.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(teams)
    }

    pub fn delete(&self, team_id: impl AsRef<str>) -> Result<()> {
        let team_id = team_id.as_ref();
        validate_storage_name(team_id)?;
        ensure_dir(&self.base_dir.join(data_dir::DIR_LOCKS))?;
        let _lock = acquire_lock_path(&self.lock_for_teams())?;
        let team_dir = data_dir::team_dir(&self.base_dir, team_id);
        if team_dir.exists() {
            fs::remove_dir_all(&team_dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{Team, TeamStatus};

    fn sample_team(id: &str) -> Team {
        Team {
            id: id.into(),
            name: id.into(),
            description: Some("Team".into()),
            cwd: None,
            status: TeamStatus::Active,
            lead_member_id: Some("lead".into()),
            owner_cc_pid: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn save_get_round_trip() {
        let dir = tempdir().unwrap();
        let store = TeamStore::new(dir.path());
        let team = sample_team("demo");
        store.save(&team).unwrap();

        let loaded = store.get("demo").unwrap().unwrap();
        assert_eq!(loaded.id, "demo");
        assert_eq!(loaded.status, TeamStatus::Active);
    }

    #[test]
    fn list_returns_all_teams_sorted() {
        let dir = tempdir().unwrap();
        let store = TeamStore::new(dir.path());
        store.save(&sample_team("bravo")).unwrap();
        store.save(&sample_team("alpha")).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "alpha");
        assert_eq!(list[1].id, "bravo");
    }

    #[test]
    fn list_skips_dot_directories() {
        let dir = tempdir().unwrap();
        let store = TeamStore::new(dir.path());
        store.save(&sample_team("demo")).unwrap();
        // .locks/ dir was created by save via ensure; ensure it doesn't surface.
        std::fs::create_dir_all(dir.path().join(".scratch")).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "demo");
    }

    #[test]
    fn delete_removes_team_directory() {
        let dir = tempdir().unwrap();
        let store = TeamStore::new(dir.path());
        store.save(&sample_team("demo")).unwrap();
        assert!(dir.path().join("demo").exists());

        store.delete("demo").unwrap();
        assert!(!dir.path().join("demo").exists());
        assert!(store.get("demo").unwrap().is_none());
    }
}
