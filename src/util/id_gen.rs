//! Task ID generation.
//!
//! Scans the task directory for existing numeric IDs and returns max+1,
//! protected by a file lock for concurrent access.

use std::path::Path;

use crate::error::Result;

/// Generate the next task ID for a team's task directory.
///
/// Scans for existing `{N}.json` files and returns `max(N) + 1`.
///
/// **Important:** The caller MUST hold the team's task lock (`.lock`) across
/// both this call and the subsequent file write. This function does NOT
/// acquire its own lock — doing so caused redundant double-locking since
/// `create_task` already holds the directory lock.
pub fn next_task_id(task_dir: &Path) -> Result<String> {
    std::fs::create_dir_all(task_dir)?;

    let max_id = std::fs::read_dir(task_dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let stem = name.strip_suffix(".json")?;
            stem.parse::<u64>().ok()
        })
        .max()
        .unwrap_or(0);

    Ok((max_id + 1).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_id_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let id = next_task_id(dir.path()).unwrap();
        assert_eq!(id, "1");
    }

    #[test]
    fn next_id_with_existing() {
        let dir = tempfile::tempdir().unwrap();

        // Create some "task" files
        std::fs::write(dir.path().join("1.json"), "{}").unwrap();
        std::fs::write(dir.path().join("5.json"), "{}").unwrap();
        std::fs::write(dir.path().join("3.json"), "{}").unwrap();
        // Non-numeric files should be ignored
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();
        std::fs::write(dir.path().join(".id.lock"), "").unwrap();

        let id = next_task_id(dir.path()).unwrap();
        assert_eq!(id, "6");
    }
}
