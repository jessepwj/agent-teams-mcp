//! Data directory layout and scaffolding.
//!
//! Owns the conventions for the on-disk layout of a `team_mode_mcp` data
//! directory:
//!
//! ```text
//! .agent-teams/
//! ├── README.md                  (auto-generated)
//! ├── lead_pending.jsonl         (legacy worker→lead push queue)
//! ├── .locks/                    (centralized file locks)
//! └── <team-name>/
//!     ├── team.json
//!     ├── members.json
//!     ├── room.json
//!     ├── messages.jsonl
//!     └── lead_pending.jsonl     (worker→lead push queue)
//! ```
//!
//! Rebuildable views (inbox / thread projections) are NOT persisted —
//! they live in memory, rebuilt from `messages.jsonl` at startup.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::error::Result;

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// New canonical data-directory name. Hidden; sits in the Lead's CWD.
pub const DEFAULT_NAME: &str = ".agent-teams";

/// Legacy data-directory name from v0.1.x. Not read; users are asked to
/// delete it manually.
pub const LEGACY_NAME: &str = ".team-mode-data";

/// Auto-generated README at the base-dir root.
pub const FILE_README: &str = "README.md";

/// Lead pending queue file name.
///
/// Canonical writes live under each team directory. The base-dir root path is
/// retained for legacy migration and forensic diagnostics.
pub const FILE_LEAD_PENDING: &str = "lead_pending.jsonl";

/// Directory that collects every advisory lock file.
pub const DIR_LOCKS: &str = ".locks";

/// Per-team file names (all under `<base>/<team_name>/`).
pub const TEAM_FILE: &str = "team.json";
pub const MEMBERS_FILE: &str = "members.json";
pub const ROOM_FILE: &str = "room.json";
pub const MESSAGES_FILE: &str = "messages.jsonl";

/// Schema version written into `members.json`.
pub const MEMBERS_FILE_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Return the directory holding locks for the given base dir.
pub fn locks_dir(base: &Path) -> PathBuf {
    base.join(DIR_LOCKS)
}

/// Resolve the lock path for a named concern (e.g. `"teams"`,
/// `"members-demo"`, `"lead_pending"`). Caller guarantees slug safety.
pub fn lock_path(base: &Path, stem: &str) -> PathBuf {
    locks_dir(base).join(format!("{stem}.lock"))
}

/// Directory for a given team (`<base>/<team_id>/`).
pub fn team_dir(base: &Path, team_id: &str) -> PathBuf {
    base.join(team_id)
}

pub fn team_file(base: &Path, team_id: &str) -> PathBuf {
    team_dir(base, team_id).join(TEAM_FILE)
}

pub fn members_file(base: &Path, team_id: &str) -> PathBuf {
    team_dir(base, team_id).join(MEMBERS_FILE)
}

pub fn room_file(base: &Path, team_id: &str) -> PathBuf {
    team_dir(base, team_id).join(ROOM_FILE)
}

pub fn messages_file(base: &Path, team_id: &str) -> PathBuf {
    team_dir(base, team_id).join(MESSAGES_FILE)
}

pub fn lead_pending_file_for_team(base: &Path, team_id: &str) -> PathBuf {
    team_dir(base, team_id).join(FILE_LEAD_PENDING)
}

pub fn lead_pending_file(base: &Path) -> PathBuf {
    base.join(FILE_LEAD_PENDING)
}

// ---------------------------------------------------------------------------
// Startup helpers
// ---------------------------------------------------------------------------

/// Pick the data directory to use when the user didn't pass `--data-dir`.
///
/// - If `<cwd>/.agent-teams/` exists, use it.
/// - Else if `<cwd>/.team-mode-data/` exists (legacy), warn and still
///   return the new path (we do NOT migrate; user is expected to delete
///   the legacy dir).
/// - Else return the new path (will be created by `ensure_scaffold`).
pub fn resolve_default_base_dir(cwd: &Path) -> PathBuf {
    let new = cwd.join(DEFAULT_NAME);
    if new.exists() {
        return new;
    }
    let legacy = cwd.join(LEGACY_NAME);
    if legacy.exists() {
        tracing::warn!(
            legacy = %legacy.display(),
            new = %new.display(),
            "found legacy data directory from a previous agent-teams-rs version; it is NOT read — delete it manually once you have confirmed nothing important lives there"
        );
    }
    new
}

pub fn base_dir_for_project_root(project_root: &Path) -> PathBuf {
    resolve_default_base_dir(project_root)
}

/// Ensure the base dir + `.locks/` subdir exist, and (re)write
/// `README.md` with the latest layout description.
///
/// Called once at MCP server startup. Idempotent.
pub fn ensure_scaffold(base_dir: &Path) -> Result<()> {
    fs::create_dir_all(base_dir)?;
    fs::create_dir_all(locks_dir(base_dir))?;
    fs::write(base_dir.join(FILE_README), render_readme())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// README renderer
// ---------------------------------------------------------------------------

fn render_readme() -> String {
    let ts = Utc::now().to_rfc3339();
    format!(
        r#"<!-- AUTO-GENERATED by agent-teams-rs — DO NOT EDIT.
     Overwritten on every MCP server startup. -->

# agent-teams data directory

State for the `team_mode_mcp` server running in this project. Lead (you) is
the Claude Code CLI that spawned this MCP; workers are managed subprocesses
coordinated through here.

## Top-level layout

| Path | What it is | Safe to edit? |
|---|---|---|
| `README.md` | this file, auto-generated | regenerated on startup |
| `lead_pending.jsonl` | legacy worker → lead push queue kept for migration/diagnostics | managed automatically |
| `.locks/` | file locks | never |
| `<team-name>/` | per-team subdirectory, one per team | see below |

## Per-team subdirectory layout

| Path | What it is | Safe to edit? |
|---|---|---|
| `team.json` | team metadata (name, cwd, lead name) | avoid |
| `members.json` | unified member list (identity + execution profile, versioned) | avoid |
| `room.json` | main room record | avoid |
| `messages.jsonl` | append-only message transcript (source of truth) | no — corrupts projections |
| `lead_pending.jsonl` | worker → lead push queue for this team | managed automatically |

Inbox/thread views are NOT persisted. They are rebuilt from
`messages.jsonl` into an in-memory cache at startup and kept in sync
as new messages arrive.

## Want push notifications for worker replies?

See `docs/push-notifications.md` in the agent-teams-rs repo for how to
wire `~/.claude/settings.json` to read `<team-name>/lead_pending.jsonl` via the
`FileChanged` + `asyncRewake` hook chain.

## Commands

- List teams: MCP tool `team_list`
- Read lead inbox: MCP tool `inbox_read`
- Add a worker: MCP tool `worker_add`

_Generated at {ts}._
"#,
        ts = ts
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolves_new_dir_when_present() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(DEFAULT_NAME)).unwrap();
        let resolved = resolve_default_base_dir(dir.path());
        assert_eq!(resolved, dir.path().join(DEFAULT_NAME));
    }

    #[test]
    fn resolves_to_new_dir_when_only_legacy_exists() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(LEGACY_NAME)).unwrap();
        let resolved = resolve_default_base_dir(dir.path());
        assert_eq!(resolved, dir.path().join(DEFAULT_NAME));
    }

    #[test]
    fn resolves_to_new_dir_when_nothing_exists() {
        let dir = tempdir().unwrap();
        let resolved = resolve_default_base_dir(dir.path());
        assert_eq!(resolved, dir.path().join(DEFAULT_NAME));
    }

    #[test]
    fn ensure_scaffold_creates_expected_layout() {
        let dir = tempdir().unwrap();
        let base = dir.path().join(DEFAULT_NAME);
        ensure_scaffold(&base).unwrap();
        assert!(base.is_dir());
        assert!(base.join(DIR_LOCKS).is_dir());
        assert!(base.join(FILE_README).is_file());
    }

    #[test]
    fn ensure_scaffold_is_idempotent_and_refreshes_readme() {
        let dir = tempdir().unwrap();
        let base = dir.path().join(DEFAULT_NAME);
        ensure_scaffold(&base).unwrap();
        let first = fs::read_to_string(base.join(FILE_README)).unwrap();

        // Second run should succeed and still contain the header.
        ensure_scaffold(&base).unwrap();
        let second = fs::read_to_string(base.join(FILE_README)).unwrap();
        assert!(first.contains("agent-teams data directory"));
        assert!(second.contains("agent-teams data directory"));
    }

    #[test]
    fn path_helpers_compose_expected_paths() {
        let base = PathBuf::from("/tmp/base");
        assert_eq!(
            team_file(&base, "demo"),
            PathBuf::from("/tmp/base/demo/team.json")
        );
        assert_eq!(
            members_file(&base, "demo"),
            PathBuf::from("/tmp/base/demo/members.json")
        );
        assert_eq!(
            messages_file(&base, "demo"),
            PathBuf::from("/tmp/base/demo/messages.jsonl")
        );
        assert_eq!(
            lead_pending_file(&base),
            PathBuf::from("/tmp/base/lead_pending.jsonl")
        );
        assert_eq!(
            lead_pending_file_for_team(&base, "demo"),
            PathBuf::from("/tmp/base/demo/lead_pending.jsonl")
        );
        assert_eq!(
            lock_path(&base, "teams"),
            PathBuf::from("/tmp/base/.locks/teams.lock")
        );
    }
}
