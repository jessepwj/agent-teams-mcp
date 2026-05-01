use chrono::Utc;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::team_mode::domain::{Team, TeamStatus};
use crate::team_mode::storage::TeamStore;
use crate::util::validate_name;

#[derive(Debug, Clone)]
pub struct CreateTeamRequest {
    pub id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub cwd: Option<String>,
    pub lead_member_id: Option<String>,
    /// PID of the Claude Code process that owns this team. The MCP dispatch
    /// layer resolves it via the shared ancestor-walk helper so push routing
    /// can later filter messages to the correct CC client.
    pub owner_cc_pid: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct TeamService {
    team_store: TeamStore,
}

impl TeamService {
    pub fn new(team_store: TeamStore) -> Self {
        Self { team_store }
    }

    pub fn create(&self, request: CreateTeamRequest) -> Result<Team> {
        if request.name.trim().is_empty() {
            return Err(Error::InvalidName {
                name: request.name,
                reason: "name cannot be empty".into(),
            });
        }

        let existing = self.team_store.list()?;
        let request_id = request.id.as_deref();
        if let Some(id) = request_id {
            validate_name(id)?;
        }

        if let Some(team) = existing
            .iter()
            .find(|team| team.name == request.name || request_id == Some(team.id.as_str()))
        {
            let id_matches = request_id.map(|id| id == team.id).unwrap_or(true);
            if id_matches && team.name == request.name && team.status == TeamStatus::Active {
                return self.rebind_existing_active_team(team.clone(), request.owner_cc_pid);
            }
            return Err(Error::TeamAlreadyExists {
                name: request.name.clone(),
            });
        }

        let team_id = match request.id {
            Some(id) => id,
            None => loop {
                let candidate = Uuid::new_v4().to_string();
                if !existing.iter().any(|team| team.id == candidate) {
                    break candidate;
                }
            },
        };

        let now = Utc::now();
        let team = Team {
            id: team_id,
            name: request.name,
            description: request.description,
            cwd: request.cwd,
            status: TeamStatus::Active,
            lead_member_id: request.lead_member_id,
            owner_cc_pid: request.owner_cc_pid,
            created_at: now,
            updated_at: now,
        };

        self.team_store.save(&team)?;
        Ok(team)
    }

    fn rebind_existing_active_team(
        &self,
        mut team: Team,
        owner_cc_pid: Option<u32>,
    ) -> Result<Team> {
        let Some(owner_cc_pid) = owner_cc_pid else {
            return Ok(team);
        };
        if team.owner_cc_pid == Some(owner_cc_pid) {
            return Ok(team);
        }

        team.owner_cc_pid = Some(owner_cc_pid);
        team.updated_at = Utc::now();
        self.team_store.save(&team)?;
        Ok(team)
    }

    pub fn get(&self, team_id: impl AsRef<str>) -> Result<Option<Team>> {
        self.team_store.get(team_id)
    }

    pub fn list(&self) -> Result<Vec<Team>> {
        self.team_store.list()
    }

    pub fn delete(&self, team_id: impl AsRef<str>) -> Result<()> {
        let team_id = team_id.as_ref();
        match self.team_store.get(team_id)? {
            Some(_) => self.team_store.delete(team_id),
            None => Err(Error::TeamNotFound {
                name: team_id.to_string(),
            }),
        }
    }

    /// Set the team's lead member. Returns `Ok(true)` if the lead was actually
    /// written (no previous lead), `Ok(false)` if a lead already existed (no-op).
    pub fn set_lead_if_absent(
        &self,
        team_id: impl AsRef<str>,
        member_id: impl Into<String>,
    ) -> Result<bool> {
        let team_id = team_id.as_ref();
        let mut team = self
            .team_store
            .get(team_id)?
            .ok_or_else(|| Error::TeamNotFound {
                name: team_id.to_string(),
            })?;
        if team.lead_member_id.is_some() {
            return Ok(false);
        }
        team.lead_member_id = Some(member_id.into());
        team.updated_at = Utc::now();
        self.team_store.save(&team)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn create_team_rejects_duplicate_id_and_name() {
        let dir = tempdir().unwrap();
        let service = TeamService::new(TeamStore::new(dir.path()));

        let first = service
            .create(CreateTeamRequest {
                id: Some("team-1".into()),
                name: "Main".into(),
                description: None,
                cwd: None,
                lead_member_id: None,
                owner_cc_pid: None,
            })
            .unwrap();
        assert_eq!(first.id, "team-1");

        let duplicate_name = service.create(CreateTeamRequest {
            id: Some("team-2".into()),
            name: "Main".into(),
            description: None,
            cwd: None,
            lead_member_id: None,
            owner_cc_pid: None,
        });
        assert!(matches!(
            duplicate_name,
            Err(Error::TeamAlreadyExists { .. })
        ));

        let duplicate_id = service.create(CreateTeamRequest {
            id: Some("team-1".into()),
            name: "Secondary".into(),
            description: None,
            cwd: None,
            lead_member_id: None,
            owner_cc_pid: None,
        });
        assert!(matches!(duplicate_id, Err(Error::TeamAlreadyExists { .. })));
    }

    #[test]
    fn create_existing_active_team_rebinds_owner_without_same_owner_churn() {
        let dir = tempdir().unwrap();
        let store = TeamStore::new(dir.path());
        let service = TeamService::new(store.clone());

        let first = service
            .create(CreateTeamRequest {
                id: Some("team-1".into()),
                name: "Main".into(),
                description: None,
                cwd: None,
                lead_member_id: Some("lead".into()),
                owner_cc_pid: Some(111),
            })
            .unwrap();

        let same_owner = service
            .create(CreateTeamRequest {
                id: Some("team-1".into()),
                name: "Main".into(),
                description: None,
                cwd: None,
                lead_member_id: Some("lead".into()),
                owner_cc_pid: Some(111),
            })
            .unwrap();
        assert_eq!(same_owner.owner_cc_pid, Some(111));
        assert_eq!(same_owner.created_at, first.created_at);
        assert_eq!(same_owner.updated_at, first.updated_at);

        let mut stale = same_owner.clone();
        stale.updated_at -= chrono::Duration::seconds(60);
        store.save(&stale).unwrap();

        let rebound = service
            .create(CreateTeamRequest {
                id: Some("team-1".into()),
                name: "Main".into(),
                description: None,
                cwd: None,
                lead_member_id: Some("lead".into()),
                owner_cc_pid: Some(222),
            })
            .unwrap();

        assert_eq!(rebound.owner_cc_pid, Some(222));
        assert_eq!(rebound.created_at, first.created_at);
        assert!(rebound.updated_at > stale.updated_at);
    }
}
