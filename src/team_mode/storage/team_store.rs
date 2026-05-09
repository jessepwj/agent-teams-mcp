use std::fs;
use std::path::PathBuf;

use crate::error::Result;
use crate::team_mode::data_dir::{self, TEAM_FILE};
use crate::team_mode::domain::{Team, TeamStatus};
use crate::team_mode::storage::{
    acquire_lock_path, ensure_dir, read_json_opt, validate_storage_name,
};
use crate::util::atomic_write::atomic_write_json;

#[derive(Debug, Clone)]
pub struct TeamStore {
    base_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamDeleteMode {
    Archive,
    Permanent,
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

    pub(crate) fn with_teams_lock<T>(&self, f: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        ensure_dir(&self.base_dir.join(data_dir::DIR_LOCKS))?;
        let _lock = acquire_lock_path(&self.lock_for_teams())?;
        f(self)
    }

    pub fn save(&self, team: &Team) -> Result<()> {
        self.with_teams_lock(|store| store.save_unlocked(team))
    }

    pub(crate) fn save_unlocked(&self, team: &Team) -> Result<()> {
        validate_storage_name(&team.id)?;
        let team_dir = data_dir::team_dir(&self.base_dir, &team.id);
        ensure_dir(&team_dir)?;
        atomic_write_json(&self.team_file(&team.id), team)
    }

    pub fn get(&self, team_id: impl AsRef<str>) -> Result<Option<Team>> {
        let team_id = team_id.as_ref();
        validate_storage_name(team_id)?;
        read_json_opt(&self.team_file(team_id))
    }

    pub fn list(&self) -> Result<Vec<Team>> {
        self.with_teams_lock(|store| store.list_unlocked())
    }

    pub(crate) fn list_unlocked(&self) -> Result<Vec<Team>> {
        let mut teams = Vec::new();
        if !self.base_dir.exists() {
            return Ok(teams);
        }
        // Per-entry IO errors (Windows EACCES while another caller is mid-rename
        // during auto-archive, or a transient share-violation) must not fail
        // the whole list. We skip the entry and warn — the next list() call
        // (hooks fire frequently) will retry naturally. Same applies to the
        // per-file read of team.json: a half-written team_file mid-rename can
        // surface as Err here, and surfacing that to /lead-pending/my-teams
        // turns a transient I/O race into an error response that breaks the
        // whole hook fire.
        for entry in fs::read_dir(&self.base_dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(
                        event = "team_store.list_entry_skipped",
                        error = %err,
                        "skipping unreadable directory entry"
                    );
                    continue;
                }
            };
            let path = entry.path();
            match path.is_dir() {
                true => {}
                false => continue,
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
            match read_json_opt::<Team>(&team_file) {
                Ok(Some(team)) => teams.push(team),
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(
                        event = "team_store.list_entry_skipped",
                        path = %team_file.display(),
                        error = %err,
                        "skipping team with unreadable team.json"
                    );
                    continue;
                }
            }
        }
        teams.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(teams)
    }

    pub fn delete(&self, team_id: impl AsRef<str>, mode: TeamDeleteMode) -> Result<()> {
        let team_id = team_id.as_ref().to_string();
        self.with_teams_lock(|store| store.delete_unlocked(&team_id, mode))
    }

    pub(crate) fn delete_unlocked(&self, team_id: &str, mode: TeamDeleteMode) -> Result<()> {
        validate_storage_name(team_id)?;
        let team_dir = data_dir::team_dir(&self.base_dir, team_id);
        match mode {
            TeamDeleteMode::Permanent => {
                if team_dir.exists() {
                    fs::remove_dir_all(&team_dir)?;
                }
            }
            TeamDeleteMode::Archive => {
                if !team_dir.exists() {
                    return Ok(());
                }
                if let Some(mut team) = read_json_opt::<Team>(&self.team_file(team_id))? {
                    team.status = TeamStatus::Archived;
                    team.updated_at = chrono::Utc::now();
                    atomic_write_json(&self.team_file(team_id), &team)?;
                }
            }
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

        store.delete("demo", TeamDeleteMode::Permanent).unwrap();
        assert!(!dir.path().join("demo").exists());
        assert!(store.get("demo").unwrap().is_none());
    }

    #[test]
    fn archive_preserves_directory_and_marks_status() {
        let dir = tempdir().unwrap();
        let store = TeamStore::new(dir.path());
        store.save(&sample_team("demo")).unwrap();

        store.delete("demo", TeamDeleteMode::Archive).unwrap();
        assert!(dir.path().join("demo").exists());
        assert_eq!(
            store.get("demo").unwrap().unwrap().status,
            TeamStatus::Archived
        );
    }
}
