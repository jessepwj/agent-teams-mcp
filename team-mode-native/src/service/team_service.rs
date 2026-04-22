use chrono::Utc;
use uuid::Uuid;

use crate::domain::{Room, RoomKind, RoomStatus, Team, TeamStatus};
use crate::storage::JsonFileStore;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct TeamService {
    store: JsonFileStore,
}

#[derive(Debug, Clone)]
pub struct CreateTeam {
    pub id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub lead_member_id: Option<String>,
}

impl TeamService {
    pub fn new(store: JsonFileStore) -> Self {
        Self { store }
    }

    pub fn create(&self, input: CreateTeam) -> Result<Team> {
        let mut teams = self.store.load_teams()?;
        let id = input
            .id
            .unwrap_or_else(|| format!("team_{}", Uuid::new_v4().simple()));
        if teams.iter().any(|team| team.id == id) {
            return Err(Error::Conflict(format!("team already exists: {id}")));
        }

        let now = Utc::now();
        let team = Team {
            id: id.clone(),
            name: input.name,
            description: input.description,
            status: TeamStatus::Active,
            lead_member_id: input.lead_member_id,
            created_at: now,
            updated_at: now,
        };
        teams.push(team.clone());
        self.store.save_teams(&teams)?;

        let mut rooms = self.store.load_rooms()?;
        if !rooms
            .iter()
            .any(|room| room.team_id == id && room.id == "main")
        {
            rooms.push(Room {
                id: "main".to_string(),
                team_id: id,
                kind: RoomKind::Main,
                status: RoomStatus::Active,
            });
            self.store.save_rooms(&rooms)?;
        }

        Ok(team)
    }

    pub fn get(&self, team_id: &str) -> Result<Team> {
        self.store
            .load_teams()?
            .into_iter()
            .find(|team| team.id == team_id)
            .ok_or_else(|| Error::NotFound(format!("team: {team_id}")))
    }

    pub fn list(&self) -> Result<Vec<Team>> {
        self.store.load_teams()
    }

    pub fn delete(&self, team_id: &str) -> Result<()> {
        let mut teams = self.store.load_teams()?;
        let before = teams.len();
        teams.retain(|team| team.id != team_id);
        if teams.len() == before {
            return Err(Error::NotFound(format!("team: {team_id}")));
        }
        self.store.save_teams(&teams)?;

        let mut rooms = self.store.load_rooms()?;
        rooms.retain(|room| room.team_id != team_id);
        self.store.save_rooms(&rooms)?;

        let mut members = self.store.load_members()?;
        members.retain(|member| member.profile.team_id != team_id);
        self.store.save_members(&members)?;
        Ok(())
    }
}
