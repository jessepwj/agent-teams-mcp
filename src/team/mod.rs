//! Team management: trait definition and file-system implementation.
//!
//! The `TeamManager` trait abstracts team CRUD operations, while
//! `FileTeamManager` persists teams as JSON files under a base directory
//! (defaulting to `~/.claude/teams`), matching the Claude Code layout.

use std::path::PathBuf;

use async_trait::async_trait;
use tracing::{debug, info};

use crate::error::{Error, Result};
use crate::models::team::{MemberUnion, TeamConfig};
use crate::util::atomic_write::atomic_write_json;
use crate::util::file_lock::FileLock;
use crate::util::validate_name;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Manages team lifecycle: create, delete, read config, and member mutations.
///
/// Methods are `async` for forward-compatibility with remote/database backends,
/// even though the file-system implementation is synchronous under the hood.
#[async_trait]
pub trait TeamManager: Send + Sync {
    /// Create a new team with the given name and optional description.
    ///
    /// Returns `TeamAlreadyExists` if the team directory already exists.
    async fn create_team(&self, name: &str, description: Option<&str>) -> Result<TeamConfig>;

    /// Delete a team by name.
    ///
    /// Returns `TeamNotFound` if the team does not exist, and
    /// `TeamHasActiveMembers` if any teammates are still present.
    async fn delete_team(&self, name: &str) -> Result<()>;

    /// Read the team config from disk.
    async fn read_config(&self, name: &str) -> Result<TeamConfig>;

    /// Add a member (lead or teammate) to a team.
    ///
    /// Returns `MemberAlreadyExists` if a member with the same name is present.
    async fn add_member(&self, team: &str, member: MemberUnion) -> Result<()>;

    /// Remove a member by name from a team.
    ///
    /// Returns `MemberNotFound` if no member with that name exists.
    async fn remove_member(&self, team: &str, member_name: &str) -> Result<()>;

    /// List all team names (directory names under the base dir).
    async fn list_teams(&self) -> Result<Vec<String>>;
}

// ---------------------------------------------------------------------------
// FileTeamManager
// ---------------------------------------------------------------------------

/// File-system backed `TeamManager`.
///
/// Directory layout (mirrors Claude Code):
/// ```text
/// {base_dir}/
///   {team-name}/
///     config.json        # TeamConfig
///     config.json.lock   # flock guard
///     inboxes/           # per-agent message inboxes
///   ...
/// {tasks_base}/          # ~/.claude/tasks/{team-name}/  (task storage)
/// ```
pub struct FileTeamManager {
    base_dir: PathBuf,
}

impl FileTeamManager {
    /// Create a `FileTeamManager` rooted at the given `base_dir`.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Convenience path: `{base_dir}/{name}`
    fn team_dir(&self, name: &str) -> PathBuf {
        self.base_dir.join(name)
    }

    /// Convenience path: `{base_dir}/{name}/config.json`
    fn config_path(&self, name: &str) -> PathBuf {
        self.team_dir(name).join("config.json")
    }

    /// Convenience path: `{base_dir}/{name}/config.json.lock`
    fn lock_path(&self, name: &str) -> PathBuf {
        self.team_dir(name).join("config.json.lock")
    }

    /// The tasks directory for a team, derived from `base_dir`.
    ///
    /// If `base_dir` is `~/.claude/teams`, tasks go to `~/.claude/tasks/{name}`.
    /// If `base_dir` is `/tmp/foo/teams`, tasks go to `/tmp/foo/tasks/{name}`.
    fn tasks_dir(&self, name: &str) -> PathBuf {
        // Derive tasks base from teams base: sibling `tasks/` directory
        let tasks_base = self
            .base_dir
            .parent()
            .map(|p| p.join("tasks"))
            .unwrap_or_else(|| self.base_dir.join("tasks"));
        tasks_base.join(name)
    }

    /// Read + deserialize config.json (no lock -- caller should hold one if mutating).
    fn read_config_sync(config_path: &std::path::Path, name: &str) -> Result<TeamConfig> {
        if !config_path.exists() {
            return Err(Error::TeamNotFound {
                name: name.to_string(),
            });
        }
        let data = std::fs::read_to_string(config_path)?;
        let config: TeamConfig = serde_json::from_str(&data)?;
        Ok(config)
    }
}

impl Default for FileTeamManager {
    /// Default base directory: `~/.claude/teams`
    ///
    /// # Panics
    ///
    /// Panics if the home directory cannot be determined.
    fn default() -> Self {
        let base = dirs::home_dir()
            .expect("could not determine home directory")
            .join(".claude")
            .join("teams");
        Self::new(base)
    }
}

#[async_trait]
impl TeamManager for FileTeamManager {
    async fn create_team(&self, name: &str, description: Option<&str>) -> Result<TeamConfig> {
        validate_name(name)?;

        let base_dir = self.base_dir.clone();
        let team_dir = self.team_dir(name);
        let config_path = self.config_path(name);
        let tasks_dir = self.tasks_dir(name);
        let name = name.to_string();
        let description = description.map(String::from);

        tokio::task::spawn_blocking(move || {
            // Ensure the parent base directory exists
            std::fs::create_dir_all(&base_dir)?;

            // Atomically create the team directory -- fails if it already exists,
            // avoiding the TOCTOU race of `exists()` + `create_dir_all()`.
            match std::fs::create_dir(&team_dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(Error::TeamAlreadyExists { name: name.clone() });
                }
                Err(e) => return Err(Error::Io(e)),
            }

            // Create subdirectories (team dir already exists so these are safe)
            std::fs::create_dir_all(team_dir.join("inboxes"))?;

            // Create tasks directory alongside teams
            std::fs::create_dir_all(&tasks_dir)?;
            debug!(path = %tasks_dir.display(), "created tasks directory");

            let config = TeamConfig {
                team_name: name.clone(),
                description,
                created_at: None,
                lead_agent_id: None,
                lead_session_id: None,
                members: Vec::new(),
            };

            atomic_write_json(&config_path, &config)?;
            info!(team = %name, "created team");

            Ok(config)
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn delete_team(&self, name: &str) -> Result<()> {
        validate_name(name)?;

        let team_dir = self.team_dir(name);
        let lock_path = self.lock_path(name);
        let config_path = self.config_path(name);
        let tasks_dir = self.tasks_dir(name);
        let name = name.to_string();

        tokio::task::spawn_blocking(move || {
            if !team_dir.exists() {
                return Err(Error::TeamNotFound { name: name.clone() });
            }

            // Hold the config lock through both the check AND the deletion
            // to prevent a concurrent `add_member` from sneaking in (TOCTOU fix).
            let _lock = FileLock::acquire(&lock_path)?;

            let config = Self::read_config_sync(&config_path, &name)?;
            let has_teammates = config.members.iter().any(|m| m.is_teammate());
            if has_teammates {
                return Err(Error::TeamHasActiveMembers { name: name.clone() });
            }

            // Lock is held -- safe to delete now
            std::fs::remove_dir_all(&team_dir)?;
            info!(team = %name, "deleted team directory");

            // Also remove tasks directory if it exists
            if tasks_dir.exists() {
                std::fs::remove_dir_all(&tasks_dir)?;
                debug!(team = %name, "deleted tasks directory");
            }

            Ok(())
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn read_config(&self, name: &str) -> Result<TeamConfig> {
        validate_name(name)?;

        let config_path = self.config_path(name);
        let name = name.to_string();

        tokio::task::spawn_blocking(move || Self::read_config_sync(&config_path, &name))
            .await
            .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn add_member(&self, team: &str, member: MemberUnion) -> Result<()> {
        validate_name(team)?;

        let team_dir = self.team_dir(team);
        let lock_path = self.lock_path(team);
        let config_path = self.config_path(team);
        let team = team.to_string();
        let member_name = member.name().to_string();

        tokio::task::spawn_blocking(move || {
            if !team_dir.exists() {
                return Err(Error::TeamNotFound { name: team.clone() });
            }

            let _lock = FileLock::acquire(&lock_path)?;
            debug!(team = %team, "acquired config lock");

            let mut config = Self::read_config_sync(&config_path, &team)?;

            if config.members.iter().any(|m| m.name() == member_name) {
                return Err(Error::MemberAlreadyExists {
                    team: team.clone(),
                    member: member_name.clone(),
                });
            }
            config.members.push(member);
            info!(team = %team, member = %member_name, "added member");

            atomic_write_json(&config_path, &config)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn remove_member(&self, team: &str, member_name: &str) -> Result<()> {
        validate_name(team)?;

        let team_dir = self.team_dir(team);
        let lock_path = self.lock_path(team);
        let config_path = self.config_path(team);
        let team = team.to_string();
        let member_name = member_name.to_string();

        tokio::task::spawn_blocking(move || {
            if !team_dir.exists() {
                return Err(Error::TeamNotFound { name: team.clone() });
            }

            let _lock = FileLock::acquire(&lock_path)?;
            debug!(team = %team, "acquired config lock");

            let mut config = Self::read_config_sync(&config_path, &team)?;

            let idx = config
                .members
                .iter()
                .position(|m| m.name() == member_name)
                .ok_or_else(|| Error::MemberNotFound {
                    team: team.clone(),
                    member: member_name.clone(),
                })?;
            config.members.remove(idx);
            info!(team = %team, member = %member_name, "removed member");

            atomic_write_json(&config_path, &config)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }

    async fn list_teams(&self) -> Result<Vec<String>> {
        let base_dir = self.base_dir.clone();

        tokio::task::spawn_blocking(move || {
            if !base_dir.exists() {
                return Ok(Vec::new());
            }

            let mut teams = Vec::new();
            for entry in std::fs::read_dir(&base_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    // Only include directories that contain a config.json
                    let config_path = entry.path().join("config.json");
                    if config_path.exists()
                        && let Some(name) = entry.file_name().to_str()
                    {
                        teams.push(name.to_string());
                    }
                }
            }
            teams.sort();
            Ok(teams)
        })
        .await
        .map_err(|e| Error::JoinError(format!("{e}")))?
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::team::{LeadMember, TeammateMember};
    use std::path::Path;

    fn make_manager(dir: &Path) -> FileTeamManager {
        FileTeamManager::new(dir)
    }

    fn lead(name: &str) -> MemberUnion {
        MemberUnion::Lead(LeadMember {
            name: name.into(),
            agent_id: format!("{name}-id"),
            agent_type: "team-lead".into(),
            model: None,
            joined_at: None,
            tmux_pane_id: None,
            cwd: None,
            subscriptions: None,
        })
    }

    fn teammate(name: &str) -> MemberUnion {
        MemberUnion::Teammate(TeammateMember {
            name: name.into(),
            agent_id: format!("{name}-id"),
            agent_type: "general-purpose".into(),
            prompt: format!("You are {name}."),
            model: None,
            color: None,
            plan_mode_required: None,
            joined_at: None,
            tmux_pane_id: None,
            cwd: None,
            subscriptions: None,
            backend_type: None,
        })
    }

    #[tokio::test]
    async fn create_and_read_team() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        let config = mgr.create_team("alpha", Some("first team")).await.unwrap();
        assert_eq!(config.team_name, "alpha");
        assert_eq!(config.description.as_deref(), Some("first team"));
        assert!(config.members.is_empty());

        // Config persisted to disk
        let read = mgr.read_config("alpha").await.unwrap();
        assert_eq!(read.team_name, "alpha");

        // Directory structure
        assert!(tmp.path().join("alpha/inboxes").is_dir());
    }

    #[tokio::test]
    async fn create_duplicate_team_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        mgr.create_team("dup", None).await.unwrap();
        let err = mgr.create_team("dup", None).await.unwrap_err();
        assert!(matches!(err, Error::TeamAlreadyExists { .. }));
    }

    #[tokio::test]
    async fn add_and_remove_member() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        mgr.create_team("beta", None).await.unwrap();

        mgr.add_member("beta", lead("boss")).await.unwrap();
        mgr.add_member("beta", teammate("worker")).await.unwrap();

        let config = mgr.read_config("beta").await.unwrap();
        assert_eq!(config.members.len(), 2);
        assert_eq!(config.members[0].name(), "boss");
        assert_eq!(config.members[1].name(), "worker");

        // Remove the teammate
        mgr.remove_member("beta", "worker").await.unwrap();
        let config = mgr.read_config("beta").await.unwrap();
        assert_eq!(config.members.len(), 1);
    }

    #[tokio::test]
    async fn add_duplicate_member_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        mgr.create_team("gamma", None).await.unwrap();
        mgr.add_member("gamma", lead("lead")).await.unwrap();

        let err = mgr.add_member("gamma", lead("lead")).await.unwrap_err();
        assert!(matches!(err, Error::MemberAlreadyExists { .. }));
    }

    #[tokio::test]
    async fn remove_nonexistent_member_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        mgr.create_team("delta", None).await.unwrap();

        let err = mgr.remove_member("delta", "ghost").await.unwrap_err();
        assert!(matches!(err, Error::MemberNotFound { .. }));
    }

    #[tokio::test]
    async fn delete_team_with_no_teammates() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        mgr.create_team("ephemeral", None).await.unwrap();
        mgr.add_member("ephemeral", lead("lead")).await.unwrap();

        mgr.delete_team("ephemeral").await.unwrap();
        assert!(!tmp.path().join("ephemeral").exists());
    }

    #[tokio::test]
    async fn delete_team_with_teammates_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        mgr.create_team("sticky", None).await.unwrap();
        mgr.add_member("sticky", teammate("worker")).await.unwrap();

        let err = mgr.delete_team("sticky").await.unwrap_err();
        assert!(matches!(err, Error::TeamHasActiveMembers { .. }));
    }

    #[tokio::test]
    async fn delete_nonexistent_team_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        let err = mgr.delete_team("nope").await.unwrap_err();
        assert!(matches!(err, Error::TeamNotFound { .. }));
    }

    #[tokio::test]
    async fn list_teams_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        mgr.create_team("zulu", None).await.unwrap();
        mgr.create_team("alpha", None).await.unwrap();
        mgr.create_team("mike", None).await.unwrap();

        let teams = mgr.list_teams().await.unwrap();
        assert_eq!(teams, vec!["alpha", "mike", "zulu"]);
    }

    #[tokio::test]
    async fn list_teams_empty_base_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // Point to a non-existent subdirectory
        let mgr = FileTeamManager::new(tmp.path().join("nonexistent"));
        let teams = mgr.list_teams().await.unwrap();
        assert!(teams.is_empty());
    }

    #[tokio::test]
    async fn read_config_nonexistent_team_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        let err = mgr.read_config("ghost").await.unwrap_err();
        assert!(matches!(err, Error::TeamNotFound { .. }));
    }

    #[tokio::test]
    async fn operations_on_nonexistent_team_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        let err = mgr.add_member("nope", lead("a")).await.unwrap_err();
        assert!(matches!(err, Error::TeamNotFound { .. }));

        let err = mgr.remove_member("nope", "a").await.unwrap_err();
        assert!(matches!(err, Error::TeamNotFound { .. }));
    }

    #[tokio::test]
    async fn path_traversal_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(tmp.path());

        let err = mgr.create_team("../escape", None).await.unwrap_err();
        assert!(matches!(err, Error::InvalidName { .. }));

        let err = mgr.create_team("..", None).await.unwrap_err();
        assert!(matches!(err, Error::InvalidName { .. }));

        let err = mgr.create_team(".", None).await.unwrap_err();
        assert!(matches!(err, Error::InvalidName { .. }));

        let err = mgr.create_team("", None).await.unwrap_err();
        assert!(matches!(err, Error::InvalidName { .. }));

        let err = mgr.read_config("foo/bar").await.unwrap_err();
        assert!(matches!(err, Error::InvalidName { .. }));

        let err = mgr.delete_team("a\\b").await.unwrap_err();
        assert!(matches!(err, Error::InvalidName { .. }));
    }
}
