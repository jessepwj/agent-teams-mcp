use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::team_mode::data_dir::{self, MEMBERS_FILE_VERSION};
use crate::team_mode::domain::{
    ExecutionProfile, MemberKind, MemberProfile, MemberStatus, MembersFile, UnifiedMember,
};
use crate::team_mode::storage::{
    acquire_lock_path, ensure_dir, read_json_opt, validate_storage_name,
};
use crate::util::atomic_write::atomic_write_json;

/// Convenient bundle returned by `get()` — mirrors what services expect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberRecord {
    pub profile: MemberProfile,
    pub execution: Option<ExecutionProfile>,
}

impl MemberRecord {
    fn from_unified(team_id: &str, unified: UnifiedMember) -> Self {
        let UnifiedMember {
            kind,
            name,
            status,
            role_label,
            role_description,
            joined_at,
            execution,
        } = unified;
        Self {
            profile: MemberProfile {
                team_id: team_id.to_string(),
                name,
                kind,
                role_label,
                role_description,
                status,
                joined_at,
            },
            execution,
        }
    }

    fn into_unified(self) -> UnifiedMember {
        UnifiedMember {
            kind: self.profile.kind,
            name: self.profile.name,
            status: self.profile.status,
            role_label: self.profile.role_label,
            role_description: self.profile.role_description,
            joined_at: self.profile.joined_at,
            execution: self.execution,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemberStore {
    base_dir: PathBuf,
}

impl MemberStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    fn members_path(&self, team_id: &str) -> PathBuf {
        data_dir::members_file(&self.base_dir, team_id)
    }

    fn lock_for(&self, team_id: &str) -> PathBuf {
        data_dir::lock_path(&self.base_dir, &format!("members-{team_id}"))
    }

    fn load_file(&self, team_id: &str) -> Result<MembersFile> {
        match read_json_opt::<MembersFile>(&self.members_path(team_id))? {
            Some(file) => Ok(file),
            None => Ok(MembersFile {
                version: MEMBERS_FILE_VERSION,
                members: Vec::new(),
            }),
        }
    }

    fn save_file(&self, team_id: &str, file: &MembersFile) -> Result<()> {
        let team_dir = data_dir::team_dir(&self.base_dir, team_id);
        ensure_dir(&team_dir)?;
        ensure_dir(&self.base_dir.join(data_dir::DIR_LOCKS))?;
        let _lock = acquire_lock_path(&self.lock_for(team_id))?;
        atomic_write_json(&self.members_path(team_id), file)
    }

    /// List every member of a team (including lead and removed workers).
    pub fn list_by_team(&self, team_id: impl AsRef<str>) -> Result<Vec<MemberRecord>> {
        let team_id = team_id.as_ref();
        validate_storage_name(team_id)?;
        let file = self.load_file(team_id)?;
        let mut records: Vec<_> = file
            .members
            .into_iter()
            .map(|u| MemberRecord::from_unified(team_id, u))
            .collect();
        records.sort_by(|a, b| a.profile.name.cmp(&b.profile.name));
        Ok(records)
    }

    /// List only active members (filters out `status=Removed`).
    pub fn list_active(&self, team_id: impl AsRef<str>) -> Result<Vec<MemberRecord>> {
        Ok(self
            .list_by_team(team_id)?
            .into_iter()
            .filter(|r| !matches!(r.profile.status, MemberStatus::Removed))
            .collect())
    }

    pub fn get(
        &self,
        team_id: impl AsRef<str>,
        name: impl AsRef<str>,
    ) -> Result<Option<MemberRecord>> {
        let team_id = team_id.as_ref();
        let name = name.as_ref();
        validate_storage_name(team_id)?;
        validate_storage_name(name)?;
        let file = self.load_file(team_id)?;
        Ok(file
            .members
            .into_iter()
            .find(|m| m.name == name)
            .map(|u| MemberRecord::from_unified(team_id, u)))
    }

    /// Insert a new member. Fails if a member with the same name exists
    /// (regardless of status — removed members are kept for fast-resume).
    pub fn add(&self, record: MemberRecord) -> Result<()> {
        let team_id = record.profile.team_id.clone();
        validate_storage_name(&team_id)?;
        validate_storage_name(&record.profile.name)?;
        let mut file = self.load_file(&team_id)?;
        if file.members.iter().any(|m| m.name == record.profile.name) {
            return Err(Error::Other(format!(
                "member '{}' already exists in team '{team_id}'",
                record.profile.name
            )));
        }
        file.members.push(record.into_unified());
        file.version = MEMBERS_FILE_VERSION;
        self.save_file(&team_id, &file)
    }

    /// Upsert: create the member if missing, otherwise overwrite in place.
    pub fn upsert(&self, record: MemberRecord) -> Result<()> {
        let team_id = record.profile.team_id.clone();
        validate_storage_name(&team_id)?;
        validate_storage_name(&record.profile.name)?;
        let mut file = self.load_file(&team_id)?;
        if let Some(slot) = file
            .members
            .iter_mut()
            .find(|m| m.name == record.profile.name)
        {
            *slot = record.into_unified();
        } else {
            file.members.push(record.into_unified());
        }
        file.version = MEMBERS_FILE_VERSION;
        self.save_file(&team_id, &file)
    }

    /// Update an existing member in place using a closure; no-op if missing.
    pub fn update<F>(&self, team_id: &str, name: &str, f: F) -> Result<bool>
    where
        F: FnOnce(&mut UnifiedMember),
    {
        validate_storage_name(team_id)?;
        validate_storage_name(name)?;
        let mut file = self.load_file(team_id)?;
        let Some(slot) = file.members.iter_mut().find(|m| m.name == name) else {
            return Ok(false);
        };
        f(slot);
        file.version = MEMBERS_FILE_VERSION;
        self.save_file(team_id, &file)?;
        Ok(true)
    }

    /// Remove the member record entirely (identity + execution).
    pub fn delete(&self, team_id: &str, name: &str) -> Result<()> {
        validate_storage_name(team_id)?;
        validate_storage_name(name)?;
        let mut file = self.load_file(team_id)?;
        let before = file.members.len();
        file.members.retain(|m| m.name != name);
        if file.members.len() == before {
            return Ok(());
        }
        file.version = MEMBERS_FILE_VERSION;
        self.save_file(team_id, &file)
    }

    /// Semantic "soft remove": mark the member as `Removed` but keep the
    /// execution profile on the record so a subsequent fast-resume can
    /// re-activate it.
    pub fn mark_removed(&self, team_id: &str, name: &str) -> Result<bool> {
        // Setting top-level `status=Removed` is necessary but not sufficient:
        // the inner `execution.session_state` was originally left untouched
        // so a future `worker_add on_existing=reuse` could fast-resume from
        // a still-`Running` profile. The reuse path now consults the
        // orchestrator's live process map to decide whether the prior
        // session is actually alive (worker.rs:131-144), so we no longer
        // need to lie about the session state on disk. Flipping it to
        // Stopped here keeps `members.json` honest — readers like the web
        // UI and `worker_list` no longer show "alice running" inside a
        // team where alice was just removed. (BUG-9 follow-up: previous
        // patch fixed the archive code path but missed worker_remove.)
        self.update(team_id, name, |m| {
            m.status = MemberStatus::Removed;
            if let Some(exec) = m.execution.as_mut() {
                exec.session_state = Some(crate::ExecutionSessionState::Stopped);
            }
        })
    }

    /// Convenience accessor for the lead of a team, if any.
    pub fn get_lead(&self, team_id: &str) -> Result<Option<MemberRecord>> {
        validate_storage_name(team_id)?;
        let file = self.load_file(team_id)?;
        Ok(file
            .members
            .into_iter()
            .find(|m| matches!(m.kind, MemberKind::Lead))
            .map(|u| MemberRecord::from_unified(team_id, u)))
    }

    /// Low-level: replace the execution profile of a member in place.
    pub fn save_execution(
        &self,
        team_id: &str,
        name: &str,
        execution: ExecutionProfile,
    ) -> Result<bool> {
        self.update(team_id, name, |m| m.execution = Some(execution))
    }

    /// Remove only the execution profile while keeping identity.
    pub fn clear_execution(&self, team_id: &str, name: &str) -> Result<bool> {
        self.update(team_id, name, |m| m.execution = None)
    }

    /// True when the team has a members.json file (even if empty).
    pub fn has_team(&self, team_id: &str) -> bool {
        self.members_path(team_id).exists()
    }

    /// Delete the whole members.json for a team (used by team_delete).
    pub fn delete_team_members(&self, team_id: &str) -> Result<()> {
        validate_storage_name(team_id)?;
        let path = self.members_path(team_id);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::runtime::ExecutionSessionState;
    use crate::team_mode::domain::{ExecutionMode, MemberKind, MemberStatus};

    fn sample_lead(team: &str) -> MemberRecord {
        MemberRecord {
            profile: MemberProfile {
                team_id: team.into(),
                name: "lead".into(),
                kind: MemberKind::Lead,
                role_label: "lead".into(),
                role_description: None,
                status: MemberStatus::Active,
                joined_at: Utc::now(),
            },
            execution: None,
        }
    }

    fn sample_worker(team: &str, name: &str) -> MemberRecord {
        MemberRecord {
            profile: MemberProfile {
                team_id: team.into(),
                name: name.into(),
                kind: MemberKind::Member,
                role_label: "worker".into(),
                role_description: None,
                status: MemberStatus::Active,
                joined_at: Utc::now(),
            },
            execution: Some(ExecutionProfile {
                execution_mode: ExecutionMode::Managed,
                adapter: Some("claude-code".into()),
                agent_name: Some(name.into()),
                model: None,
                cwd: None,
                env: Default::default(),
                system_prompt: None,
                skills: vec![],
                session_state: Some(ExecutionSessionState::Running),
                session_id: None,
                reasoning_effort: None,
            }),
        }
    }

    #[test]
    fn add_get_list_round_trip() {
        let dir = tempdir().unwrap();
        let store = MemberStore::new(dir.path());
        store.add(sample_lead("demo")).unwrap();
        store.add(sample_worker("demo", "alice")).unwrap();

        let lead = store.get("demo", "lead").unwrap().unwrap();
        assert_eq!(lead.profile.kind, MemberKind::Lead);
        assert!(lead.execution.is_none());

        let alice = store.get("demo", "alice").unwrap().unwrap();
        assert_eq!(alice.profile.kind, MemberKind::Member);
        assert!(alice.execution.is_some());

        let all = store.list_by_team("demo").unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn add_rejects_duplicate_name() {
        let dir = tempdir().unwrap();
        let store = MemberStore::new(dir.path());
        store.add(sample_worker("demo", "alice")).unwrap();
        let err = store.add(sample_worker("demo", "alice")).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn mark_removed_preserves_execution_for_fast_resume() {
        let dir = tempdir().unwrap();
        let store = MemberStore::new(dir.path());
        store.add(sample_worker("demo", "alice")).unwrap();

        let changed = store.mark_removed("demo", "alice").unwrap();
        assert!(changed);

        let alice = store.get("demo", "alice").unwrap().unwrap();
        assert_eq!(alice.profile.status, MemberStatus::Removed);
        assert!(alice.execution.is_some(), "execution must survive removal");

        let active = store.list_active("demo").unwrap();
        assert!(active.iter().all(|r| r.profile.name != "alice"));
    }

    #[test]
    fn mark_removed_flips_execution_session_state_to_stopped() {
        // BUG-9 regression: removing a worker must update the inner
        // session_state, not just the top-level `status`. Otherwise
        // `members.json` advertises a removed worker as "running",
        // misleading every reader (web UI, worker_list, MCP responses).
        let dir = tempdir().unwrap();
        let store = MemberStore::new(dir.path());
        let mut worker = sample_worker("demo", "alice");
        worker.execution = Some(crate::team_mode::domain::ExecutionProfile {
            execution_mode: crate::team_mode::domain::ExecutionMode::Managed,
            adapter: Some("codex".into()),
            agent_name: Some("alice".into()),
            model: None,
            cwd: None,
            env: Default::default(),
            system_prompt: None,
            skills: Vec::new(),
            session_state: Some(crate::ExecutionSessionState::Running),
            session_id: Some("019df-running".into()),
            reasoning_effort: None,
        });
        store.add(worker).unwrap();

        store.mark_removed("demo", "alice").unwrap();

        let alice = store.get("demo", "alice").unwrap().unwrap();
        assert_eq!(alice.profile.status, MemberStatus::Removed);
        let exec = alice.execution.expect("execution preserved for fast-resume");
        assert_eq!(
            exec.session_state,
            Some(crate::ExecutionSessionState::Stopped)
        );
        // session_id stays so fast-resume still has the codex thread to
        // attach to — only the liveness flag flips.
        assert_eq!(exec.session_id.as_deref(), Some("019df-running"));
    }

    #[test]
    fn upsert_creates_or_replaces() {
        let dir = tempdir().unwrap();
        let store = MemberStore::new(dir.path());
        store.upsert(sample_worker("demo", "alice")).unwrap();
        let mut updated = sample_worker("demo", "alice");
        updated.profile.role_label = "senior-worker".into();
        store.upsert(updated).unwrap();

        let alice = store.get("demo", "alice").unwrap().unwrap();
        assert_eq!(alice.profile.role_label, "senior-worker");
        assert_eq!(store.list_by_team("demo").unwrap().len(), 1);
    }

    #[test]
    fn get_lead_returns_lead_member() {
        let dir = tempdir().unwrap();
        let store = MemberStore::new(dir.path());
        store.add(sample_worker("demo", "alice")).unwrap();
        store.add(sample_lead("demo")).unwrap();

        let lead = store.get_lead("demo").unwrap().unwrap();
        assert_eq!(lead.profile.name, "lead");
    }

    #[test]
    fn delete_removes_only_named_member() {
        let dir = tempdir().unwrap();
        let store = MemberStore::new(dir.path());
        store.add(sample_lead("demo")).unwrap();
        store.add(sample_worker("demo", "alice")).unwrap();
        store.add(sample_worker("demo", "bob")).unwrap();

        store.delete("demo", "alice").unwrap();
        let remaining: Vec<_> = store
            .list_by_team("demo")
            .unwrap()
            .into_iter()
            .map(|r| r.profile.name)
            .collect();
        assert!(!remaining.contains(&"alice".to_string()));
        assert!(remaining.contains(&"bob".to_string()));
        assert!(remaining.contains(&"lead".to_string()));
    }

    #[test]
    fn members_file_includes_schema_version_on_disk() {
        let dir = tempdir().unwrap();
        let store = MemberStore::new(dir.path());
        store.add(sample_lead("demo")).unwrap();
        let path = store.members_path("demo");
        let content = std::fs::read_to_string(path).unwrap();
        // atomic_write_json uses to_vec_pretty, so JSON has spaces after colons.
        assert!(content.contains("\"version\""));
        assert!(content.contains(": 1"));
    }
}
