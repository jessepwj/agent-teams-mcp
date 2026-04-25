use chrono::Utc;

use crate::error::{Error, Result};
use crate::team_mode::domain::{ExecutionProfile, MemberKind, MemberProfile, MemberStatus};
use crate::team_mode::storage::{MemberRecord, MemberStore, TeamStore};
use crate::util::validate_name;

#[derive(Debug, Clone)]
pub struct AddMemberRequest {
    pub team_id: String,
    pub name: String,
    pub kind: MemberKind,
    pub role_label: String,
    pub role_description: Option<String>,
    pub execution: Option<ExecutionProfile>,
}

#[derive(Debug, Clone)]
pub struct UpdateMemberRequest {
    pub team_id: String,
    pub name: String,
    pub role_label: Option<String>,
    pub role_description: Option<String>,
    pub execution: Option<ExecutionProfile>,
}

#[derive(Debug, Clone)]
pub struct MemberService {
    member_store: MemberStore,
    team_store: TeamStore,
}

impl MemberService {
    pub fn new(member_store: MemberStore, team_store: TeamStore) -> Self {
        Self {
            member_store,
            team_store,
        }
    }

    pub fn add(&self, request: AddMemberRequest) -> Result<MemberRecord> {
        validate_name(&request.team_id)?;
        validate_name(&request.name)?;
        self.ensure_team_exists(&request.team_id)?;

        if self
            .member_store
            .get(&request.team_id, &request.name)?
            .is_some()
        {
            return Err(Error::MemberAlreadyExists {
                team: request.team_id.clone(),
                member: request.name.clone(),
            });
        }

        let record = MemberRecord {
            profile: MemberProfile {
                team_id: request.team_id.clone(),
                name: request.name,
                kind: request.kind,
                role_label: request.role_label,
                role_description: request.role_description,
                status: MemberStatus::Active,
                joined_at: Utc::now(),
            },
            execution: request.execution,
        };
        self.member_store.add(record.clone())?;
        Ok(record)
    }

    pub fn get(
        &self,
        team_id: impl AsRef<str>,
        name: impl AsRef<str>,
    ) -> Result<Option<MemberRecord>> {
        self.member_store.get(team_id, name)
    }

    pub fn list_by_team(&self, team_id: impl AsRef<str>) -> Result<Vec<MemberRecord>> {
        let team_id = team_id.as_ref();
        self.ensure_team_exists(team_id)?;
        self.member_store.list_by_team(team_id)
    }

    pub fn list_active(&self, team_id: impl AsRef<str>) -> Result<Vec<MemberRecord>> {
        let team_id = team_id.as_ref();
        self.ensure_team_exists(team_id)?;
        self.member_store.list_active(team_id)
    }

    pub fn update(&self, request: UpdateMemberRequest) -> Result<MemberRecord> {
        validate_name(&request.team_id)?;
        validate_name(&request.name)?;
        let _existing = self
            .member_store
            .get(&request.team_id, &request.name)?
            .ok_or_else(|| Error::MemberNotFound {
                team: request.team_id.clone(),
                member: request.name.clone(),
            })?;

        self.member_store
            .update(&request.team_id, &request.name, |m| {
                if let Some(role_label) = request.role_label.clone() {
                    m.role_label = role_label;
                }
                if let Some(role_description) = request.role_description.clone() {
                    m.role_description = Some(role_description);
                }
                if let Some(execution) = request.execution.clone() {
                    m.execution = Some(execution);
                }
            })?;

        self.member_store
            .get(&request.team_id, &request.name)?
            .ok_or_else(|| Error::MemberNotFound {
                team: request.team_id,
                member: request.name,
            })
    }

    /// Hard-remove a member entirely. Prefer `mark_removed` when you want
    /// to keep the execution profile for fast-resume.
    pub fn remove(&self, team_id: &str, name: &str) -> Result<()> {
        match self.member_store.get(team_id, name)? {
            Some(_) => self.member_store.delete(team_id, name),
            None => Err(Error::MemberNotFound {
                team: team_id.to_string(),
                member: name.to_string(),
            }),
        }
    }

    /// Soft-remove: mark status=Removed but keep execution profile so
    /// a subsequent `worker_add` can fast-resume.
    pub fn mark_removed(&self, team_id: &str, name: &str) -> Result<()> {
        if !self.member_store.mark_removed(team_id, name)? {
            return Err(Error::MemberNotFound {
                team: team_id.to_string(),
                member: name.to_string(),
            });
        }
        Ok(())
    }

    /// Restore a previously removed member to active status.
    pub fn mark_active(&self, team_id: &str, name: &str) -> Result<()> {
        self.member_store
            .update(team_id, name, |m| m.status = MemberStatus::Active)?
            .then_some(())
            .ok_or_else(|| Error::MemberNotFound {
                team: team_id.to_string(),
                member: name.to_string(),
            })
    }

    fn ensure_team_exists(&self, team_id: &str) -> Result<()> {
        self.team_store
            .get(team_id)?
            .ok_or_else(|| Error::TeamNotFound {
                name: team_id.to_string(),
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::runtime::ExecutionSessionState;
    use crate::team_mode::domain::{ExecutionMode, Team, TeamStatus};
    use crate::team_mode::storage::{MemberStore, TeamStore};

    fn new_service(base_dir: &std::path::Path) -> MemberService {
        MemberService::new(MemberStore::new(base_dir), TeamStore::new(base_dir))
    }

    fn create_team(base_dir: &std::path::Path, team_id: &str) {
        TeamStore::new(base_dir)
            .save(&Team {
                id: team_id.into(),
                name: team_id.into(),
                description: None,
                cwd: None,
                status: TeamStatus::Active,
                lead_member_id: None,
                owner_cc_pid: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
            .unwrap();
    }

    #[test]
    fn add_and_list_member_scoped_by_team() {
        let dir = tempdir().unwrap();
        create_team(dir.path(), "demo");
        let service = new_service(dir.path());

        let record = service
            .add(AddMemberRequest {
                team_id: "demo".into(),
                name: "alice".into(),
                kind: MemberKind::Member,
                role_label: "reviewer".into(),
                role_description: None,
                execution: Some(ExecutionProfile {
                    execution_mode: ExecutionMode::Managed,
                    adapter: Some("codex".into()),
                    agent_name: Some("alice-agent".into()),
                    model: None,
                    cwd: None,
                    env: Default::default(),
                    system_prompt: None,
                    skills: vec![],
                    session_state: Some(ExecutionSessionState::Running),
                    session_id: None,
                }),
            })
            .unwrap();

        assert_eq!(record.profile.name, "alice");
        assert!(record.execution.is_some());

        let all = service.list_by_team("demo").unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn duplicate_name_is_rejected() {
        let dir = tempdir().unwrap();
        create_team(dir.path(), "demo");
        let service = new_service(dir.path());
        service
            .add(AddMemberRequest {
                team_id: "demo".into(),
                name: "alice".into(),
                kind: MemberKind::Member,
                role_label: "r".into(),
                role_description: None,
                execution: None,
            })
            .unwrap();
        let err = service
            .add(AddMemberRequest {
                team_id: "demo".into(),
                name: "alice".into(),
                kind: MemberKind::Member,
                role_label: "r".into(),
                role_description: None,
                execution: None,
            })
            .unwrap_err();
        assert!(matches!(err, Error::MemberAlreadyExists { .. }));
    }

    #[test]
    fn mark_removed_is_recoverable() {
        let dir = tempdir().unwrap();
        create_team(dir.path(), "demo");
        let service = new_service(dir.path());
        service
            .add(AddMemberRequest {
                team_id: "demo".into(),
                name: "alice".into(),
                kind: MemberKind::Member,
                role_label: "r".into(),
                role_description: None,
                execution: Some(ExecutionProfile {
                    execution_mode: ExecutionMode::Managed,
                    adapter: Some("claude-code".into()),
                    agent_name: Some("alice".into()),
                    model: None,
                    cwd: None,
                    env: Default::default(),
                    system_prompt: None,
                    skills: vec![],
                    session_state: None,
                    session_id: None,
                }),
            })
            .unwrap();

        service.mark_removed("demo", "alice").unwrap();
        let after = service.get("demo", "alice").unwrap().unwrap();
        assert_eq!(after.profile.status, MemberStatus::Removed);
        assert!(after.execution.is_some());

        service.mark_active("demo", "alice").unwrap();
        let restored = service.get("demo", "alice").unwrap().unwrap();
        assert_eq!(restored.profile.status, MemberStatus::Active);
    }
}
