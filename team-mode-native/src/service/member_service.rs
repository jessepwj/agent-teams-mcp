use chrono::Utc;
use uuid::Uuid;

use crate::domain::{ExecutionProfile, MemberKind, MemberProfile, MemberRecord, MemberStatus};
use crate::storage::JsonFileStore;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct MemberService {
    store: JsonFileStore,
}

#[derive(Debug, Clone)]
pub struct AddMember {
    pub id: Option<String>,
    pub team_id: String,
    pub name: String,
    pub kind: MemberKind,
    pub handle: String,
    pub role_label: String,
    pub role_description: Option<String>,
    pub execution: Option<ExecutionProfile>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateMember {
    pub name: Option<String>,
    pub handle: Option<String>,
    pub role_label: Option<String>,
    pub role_description: Option<Option<String>>,
    pub status: Option<MemberStatus>,
}

impl MemberService {
    pub fn new(store: JsonFileStore) -> Self {
        Self { store }
    }

    pub fn add(&self, input: AddMember) -> Result<MemberRecord> {
        self.ensure_team_exists(&input.team_id)?;
        let mut members = self.store.load_members()?;
        let handle = normalize_handle(&input.handle)?;
        if members.iter().any(|member| {
            member.profile.team_id == input.team_id
                && member.profile.status != MemberStatus::Removed
                && member.profile.handle == handle
        }) {
            return Err(Error::Conflict(format!(
                "member handle already exists in team {}: @{handle}",
                input.team_id
            )));
        }

        let id = input
            .id
            .unwrap_or_else(|| format!("mem_{}", Uuid::new_v4().simple()));
        if members.iter().any(|member| member.profile.id == id) {
            return Err(Error::Conflict(format!("member already exists: {id}")));
        }

        let record = MemberRecord {
            profile: MemberProfile {
                id,
                team_id: input.team_id,
                name: input.name,
                kind: input.kind,
                handle,
                role_label: input.role_label,
                role_description: input.role_description,
                status: MemberStatus::Active,
                joined_at: Utc::now(),
            },
            execution: input.execution,
        };
        members.push(record.clone());
        self.store.save_members(&members)?;
        Ok(record)
    }

    pub fn get(&self, team_id: &str, member_id_or_handle: &str) -> Result<MemberRecord> {
        let key = strip_at(member_id_or_handle);
        let handle_key = key.to_ascii_lowercase();
        self.store
            .load_members()?
            .into_iter()
            .find(|member| {
                member.profile.team_id == team_id
                    && member.profile.status != MemberStatus::Removed
                    && (member.profile.id == key || member.profile.handle == handle_key)
            })
            .ok_or_else(|| Error::NotFound(format!("member: {member_id_or_handle}")))
    }

    pub fn list(&self, team_id: &str) -> Result<Vec<MemberRecord>> {
        Ok(self
            .store
            .load_members()?
            .into_iter()
            .filter(|member| {
                member.profile.team_id == team_id && member.profile.status != MemberStatus::Removed
            })
            .collect())
    }

    pub fn update(
        &self,
        team_id: &str,
        member_id_or_handle: &str,
        input: UpdateMember,
    ) -> Result<MemberRecord> {
        let mut members = self.store.load_members()?;
        let index = find_member_index(&members, team_id, member_id_or_handle)?;

        if let Some(handle) = input.handle {
            let handle = normalize_handle(&handle)?;
            if members.iter().enumerate().any(|(i, member)| {
                i != index
                    && member.profile.team_id == team_id
                    && member.profile.status != MemberStatus::Removed
                    && member.profile.handle == handle
            }) {
                return Err(Error::Conflict(format!(
                    "member handle already exists in team {team_id}: @{handle}"
                )));
            }
            members[index].profile.handle = handle;
        }
        if let Some(name) = input.name {
            members[index].profile.name = name;
        }
        if let Some(role_label) = input.role_label {
            members[index].profile.role_label = role_label;
        }
        if let Some(role_description) = input.role_description {
            members[index].profile.role_description = role_description;
        }
        if let Some(status) = input.status {
            members[index].profile.status = status;
        }

        let record = members[index].clone();
        self.store.save_members(&members)?;
        Ok(record)
    }

    pub fn remove(&self, team_id: &str, member_id_or_handle: &str) -> Result<MemberRecord> {
        self.update(
            team_id,
            member_id_or_handle,
            UpdateMember {
                status: Some(MemberStatus::Removed),
                ..Default::default()
            },
        )
    }

    pub fn set_execution_profile(
        &self,
        team_id: &str,
        member_id_or_handle: &str,
        profile: ExecutionProfile,
    ) -> Result<MemberRecord> {
        let mut members = self.store.load_members()?;
        let index = find_member_index(&members, team_id, member_id_or_handle)?;
        if profile.member_id != members[index].profile.id {
            return Err(Error::Invalid(format!(
                "execution profile member_id {} does not match member {}",
                profile.member_id, members[index].profile.id
            )));
        }
        members[index].execution = Some(profile);
        let record = members[index].clone();
        self.store.save_members(&members)?;
        Ok(record)
    }

    pub fn execution_profile(
        &self,
        team_id: &str,
        member_id_or_handle: &str,
    ) -> Result<ExecutionProfile> {
        self.get(team_id, member_id_or_handle)?
            .execution
            .ok_or_else(|| Error::NotFound(format!("execution profile: {member_id_or_handle}")))
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

pub(crate) fn normalize_handle(handle: &str) -> Result<String> {
    let handle = strip_at(handle).to_ascii_lowercase();
    if handle.is_empty()
        || !handle
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(Error::Invalid(format!("invalid handle: {handle}")));
    }
    Ok(handle)
}

pub(crate) fn strip_at(value: &str) -> &str {
    value.trim().trim_start_matches('@')
}

pub(crate) fn find_member_index(
    members: &[MemberRecord],
    team_id: &str,
    member_id_or_handle: &str,
) -> Result<usize> {
    let key = strip_at(member_id_or_handle);
    let handle_key = key.to_ascii_lowercase();
    members
        .iter()
        .position(|member| {
            member.profile.team_id == team_id
                && member.profile.status != MemberStatus::Removed
                && (member.profile.id == key || member.profile.handle == handle_key)
        })
        .ok_or_else(|| Error::NotFound(format!("member: {member_id_or_handle}")))
}
