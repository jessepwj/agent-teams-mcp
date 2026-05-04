use std::path::PathBuf;

use chrono::Utc;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::team_mode::domain::{Team, TeamStatus};
use crate::team_mode::storage::{TeamDeleteMode, TeamStore};
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
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct CreateTeamOutcome {
    pub team: Team,
    pub revived: bool,
    pub restored_from: Option<PathBuf>,
    pub discarded_teams: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DeleteTeamOutcome {
    pub archived: bool,
    pub deleted: bool,
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
        Ok(self.create_with_outcome(request)?.team)
    }

    pub fn create_with_outcome(&self, request: CreateTeamRequest) -> Result<CreateTeamOutcome> {
        self.create_with_outcome_impl(request, None::<fn()>)
    }

    #[cfg(test)]
    pub(crate) fn create_with_outcome_with_hook<F>(
        &self,
        request: CreateTeamRequest,
        overwrite_hook: F,
    ) -> Result<CreateTeamOutcome>
    where
        F: FnOnce(),
    {
        self.create_with_outcome_impl(request, Some(overwrite_hook))
    }

    fn create_with_outcome_impl<F>(
        &self,
        request: CreateTeamRequest,
        overwrite_hook: Option<F>,
    ) -> Result<CreateTeamOutcome>
    where
        F: FnOnce(),
    {
        if request.name.trim().is_empty() {
            return Err(Error::InvalidName {
                name: request.name,
                reason: "name cannot be empty".into(),
            });
        }

        let request_id = request.id.as_deref();
        if let Some(id) = request_id {
            validate_name(id)?;
        }

        if request.overwrite {
            let request_id = request.id.clone();
            let request_name = request.name.clone();
            let request_description = request.description.clone();
            let request_cwd = request.cwd.clone();
            let request_lead_member_id = request.lead_member_id.clone();
            let request_owner_cc_pid = request.owner_cc_pid;
            return self.team_store.with_teams_lock(|store| {
                let existing = store.list_unlocked()?;
                let mut discarded_teams = Vec::new();
                for team in &existing {
                    store.delete_unlocked(&team.id, TeamDeleteMode::Permanent)?;
                    discarded_teams.push(team.id.clone());
                }
                if let Some(hook) = overwrite_hook {
                    hook();
                }
                let team_id = match request_id.clone() {
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
                    name: request_name,
                    description: request_description,
                    cwd: request_cwd,
                    status: TeamStatus::Active,
                    lead_member_id: request_lead_member_id,
                    owner_cc_pid: request_owner_cc_pid,
                    created_at: now,
                    updated_at: now,
                };
                store.save_unlocked(&team)?;
                Ok(CreateTeamOutcome {
                    team,
                    revived: false,
                    restored_from: None,
                    discarded_teams,
                })
            });
        }

        let existing = self.team_store.list()?;
        let discarded_teams = Vec::new();

        let same_name = existing.iter().find(|team| team.name == request.name);
        let same_id = request_id.and_then(|id| existing.iter().find(|team| team.id == id));

        if let Some(team) = same_name {
            let id_matches = request_id.map(|id| id == team.id).unwrap_or(true);
            if id_matches && team.status == TeamStatus::Active {
                let team = self.rebind_existing_active_team(team.clone(), request.owner_cc_pid)?;
                return Ok(CreateTeamOutcome {
                    team,
                    revived: false,
                    restored_from: None,
                    discarded_teams,
                });
            }
            if id_matches && team.status == TeamStatus::Archived {
                let team = self.revive_archived_team(team.clone(), &request)?;
                let restored_from = self.team_store.team_dir(&team.id);
                return Ok(CreateTeamOutcome {
                    team,
                    revived: true,
                    restored_from: Some(restored_from),
                    discarded_teams,
                });
            }
            return Err(Error::TeamAlreadyExists {
                name: request.name.clone(),
            });
        }

        if let Some(team) = same_id {
            if team.name != request.name {
                return Err(Error::TeamAlreadyExists {
                    name: request.name.clone(),
                });
            }
            return Err(Error::Other(format!(
                "this project already has team '{}' (status={}). Pass overwrite=true to discard and create '{}', or call team_create({{name:'{}'}}) to revive.",
                team.name,
                team.status.as_str(),
                request.name,
                team.name
            )));
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
        Ok(CreateTeamOutcome {
            team,
            revived: false,
            restored_from: None,
            discarded_teams,
        })
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
        if let Some(existing_owner) = team.owner_cc_pid {
            if owner_pid_is_alive(existing_owner) {
                return Err(Error::Other(format!(
                    "team '{}' is owned by another live lead PID {existing_owner}",
                    team.name
                )));
            }
        }

        team.owner_cc_pid = Some(owner_cc_pid);
        team.updated_at = Utc::now();
        self.team_store.save(&team)?;
        Ok(team)
    }

    fn revive_archived_team(&self, mut team: Team, request: &CreateTeamRequest) -> Result<Team> {
        team.status = TeamStatus::Active;
        team.owner_cc_pid = request.owner_cc_pid;
        if let Some(description) = request.description.clone() {
            team.description = Some(description);
        }
        if let Some(cwd) = request.cwd.clone() {
            team.cwd = Some(cwd);
        }
        if let Some(lead_member_id) = request.lead_member_id.clone() {
            team.lead_member_id = Some(lead_member_id);
        }
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

    pub fn delete(&self, team_id: impl AsRef<str>, permanent: bool) -> Result<DeleteTeamOutcome> {
        let team_id = team_id.as_ref();
        match self.team_store.get(team_id)? {
            Some(_) => {
                let mode = if permanent {
                    TeamDeleteMode::Permanent
                } else {
                    TeamDeleteMode::Archive
                };
                self.team_store.delete(team_id, mode)?;
                Ok(DeleteTeamOutcome {
                    archived: !permanent,
                    deleted: permanent,
                })
            }
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

fn owner_pid_is_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.process(Pid::from_u32(pid)).is_some()
}

impl TeamStatus {
    fn as_str(&self) -> &'static str {
        match self {
            TeamStatus::Active => "active",
            TeamStatus::Archived => "archived",
        }
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
                overwrite: false,
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
            overwrite: false,
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
            overwrite: false,
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
                overwrite: false,
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
                overwrite: false,
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
                overwrite: false,
            })
            .unwrap();

        assert_eq!(rebound.owner_cc_pid, Some(222));
        assert_eq!(rebound.created_at, first.created_at);
        assert!(rebound.updated_at > stale.updated_at);
    }

    #[test]
    fn create_existing_active_team_rejects_live_other_owner() {
        let dir = tempdir().unwrap();
        let service = TeamService::new(TeamStore::new(dir.path()));

        service
            .create(CreateTeamRequest {
                id: Some("team-1".into()),
                name: "Main".into(),
                description: None,
                cwd: None,
                lead_member_id: None,
                owner_cc_pid: Some(std::process::id()),
                overwrite: false,
            })
            .unwrap();

        let err = service.create(CreateTeamRequest {
            id: Some("team-1".into()),
            name: "Main".into(),
            description: None,
            cwd: None,
            lead_member_id: None,
            owner_cc_pid: Some(std::process::id().saturating_add(1)),
            overwrite: false,
        });
        assert!(matches!(&err, Err(Error::Other(msg)) if msg.contains("another live lead PID")));
    }

    #[test]
    fn create_revives_archived_team() {
        let dir = tempdir().unwrap();
        let store = TeamStore::new(dir.path());
        let service = TeamService::new(store.clone());
        let archived = Team {
            id: "team-1".into(),
            name: "Main".into(),
            description: Some("old".into()),
            cwd: None,
            status: TeamStatus::Archived,
            lead_member_id: Some("lead".into()),
            owner_cc_pid: Some(777),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.save(&archived).unwrap();

        let outcome = service
            .create_with_outcome(CreateTeamRequest {
                id: Some("team-1".into()),
                name: "Main".into(),
                description: Some("new".into()),
                cwd: Some("E:/proj".into()),
                lead_member_id: Some("lead".into()),
                owner_cc_pid: Some(888),
                overwrite: false,
            })
            .unwrap();

        assert!(outcome.revived);
        assert_eq!(outcome.restored_from, Some(store.team_dir("team-1")));
        assert_eq!(outcome.team.status, TeamStatus::Active);
        assert_eq!(outcome.team.owner_cc_pid, Some(888));
        assert_eq!(outcome.team.description.as_deref(), Some("new"));
        assert_eq!(outcome.team.cwd.as_deref(), Some("E:/proj"));
    }

    #[test]
    fn overwrite_discards_existing_teams_before_create() {
        let dir = tempdir().unwrap();
        let service = TeamService::new(TeamStore::new(dir.path()));

        service
            .create(CreateTeamRequest {
                id: Some("team-1".into()),
                name: "Old".into(),
                description: None,
                cwd: None,
                lead_member_id: None,
                owner_cc_pid: None,
                overwrite: false,
            })
            .unwrap();
        service
            .create(CreateTeamRequest {
                id: Some("team-2".into()),
                name: "AlsoOld".into(),
                description: None,
                cwd: None,
                lead_member_id: None,
                owner_cc_pid: None,
                overwrite: false,
            })
            .unwrap();

        let outcome = service
            .create_with_outcome(CreateTeamRequest {
                id: Some("team-new".into()),
                name: "Fresh".into(),
                description: None,
                cwd: None,
                lead_member_id: None,
                owner_cc_pid: None,
                overwrite: true,
            })
            .unwrap();

        assert_eq!(
            outcome.discarded_teams,
            vec!["team-1".to_string(), "team-2".to_string()]
        );
        let teams = service.list().unwrap();
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].name, "Fresh");
    }

    #[test]
    fn overwrite_holds_project_scope_lock_across_snapshot_delete_and_create() {
        use std::sync::{Arc, Barrier, mpsc};
        use std::thread;
        use std::time::Duration;

        let dir = tempdir().unwrap();
        let service = TeamService::new(TeamStore::new(dir.path()));

        service
            .create(CreateTeamRequest {
                id: Some("stale".into()),
                name: "Stale".into(),
                description: None,
                cwd: None,
                lead_member_id: None,
                owner_cc_pid: None,
                overwrite: false,
            })
            .unwrap();

        let overwrite_service = service.clone();
        let create_service = service.clone();
        let listed = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (done_tx, done_rx) = mpsc::channel();

        let overwrite_handle = {
            let listed = Arc::clone(&listed);
            let release = Arc::clone(&release);
            thread::spawn(move || {
                overwrite_service
                    .create_with_outcome_with_hook(
                        CreateTeamRequest {
                            id: Some("fresh".into()),
                            name: "Fresh".into(),
                            description: None,
                            cwd: None,
                            lead_member_id: None,
                            owner_cc_pid: None,
                            overwrite: true,
                        },
                        move || {
                            listed.wait();
                            release.wait();
                        },
                    )
                    .unwrap()
            })
        };

        listed.wait();
        let create_handle = thread::spawn(move || {
            let outcome = create_service
                .create(CreateTeamRequest {
                    id: Some("late".into()),
                    name: "Late".into(),
                    description: None,
                    cwd: None,
                    lead_member_id: None,
                    owner_cc_pid: None,
                    overwrite: false,
                })
                .unwrap();
            done_tx.send(outcome.id.clone()).unwrap();
            outcome
        });

        assert!(
            done_rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "concurrent create finished before overwrite released the project-scope lock"
        );

        release.wait();
        let overwrite_outcome = overwrite_handle.join().unwrap();
        let create_outcome = create_handle.join().unwrap();

        assert_eq!(overwrite_outcome.discarded_teams, vec!["stale".to_string()]);
        assert_eq!(overwrite_outcome.team.name, "Fresh");
        assert_eq!(create_outcome.name, "Late");

        let teams = service.list().unwrap();
        assert!(!teams.iter().any(|team| team.name == "Stale"));
        assert!(teams.iter().any(|team| team.name == "Fresh"));
        assert!(teams.iter().any(|team| team.name == "Late"));
    }

    #[test]
    fn delete_archives_by_default_and_permanently_deletes_when_requested() {
        let dir = tempdir().unwrap();
        let service = TeamService::new(TeamStore::new(dir.path()));

        service
            .create(CreateTeamRequest {
                id: Some("team-1".into()),
                name: "Demo".into(),
                description: None,
                cwd: None,
                lead_member_id: None,
                owner_cc_pid: None,
                overwrite: false,
            })
            .unwrap();

        let archived = service.delete("team-1", false).unwrap();
        assert!(archived.archived);
        assert!(!archived.deleted);
        assert_eq!(
            service.get("team-1").unwrap().unwrap().status,
            TeamStatus::Archived
        );

        let deleted = service.delete("team-1", true).unwrap();
        assert!(!deleted.archived);
        assert!(deleted.deleted);
        assert!(service.get("team-1").unwrap().is_none());
    }
}
