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
    /// PID of the Claude Code process that owns this team. Set by the MCP
    /// dispatch layer to `std::process::parent_id()` so push routing can
    /// later filter messages to the correct CC client.
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
        if existing.iter().any(|team| team.name == request.name) {
            return Err(Error::TeamAlreadyExists {
                name: request.name.clone(),
            });
        }

        let team_id = match request.id {
            Some(id) => {
                validate_name(&id)?;
                if existing.iter().any(|team| team.id == id) {
                    return Err(Error::TeamAlreadyExists { name: id });
                }
                id
            }
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
}
