//! Atomic file writes via tempfile + rename.
//!
//! Mirrors the Python pattern of `os.replace(tmp, target)` to ensure
//! readers never see a partially-written file.

use std::path::Path;

use crate::error::{Error, Result};

/// Atomically write `data` to `path`.
///
/// Creates a temporary file in the same directory, writes `data`,
/// then renames it to `path`. On Unix this is atomic because
/// rename(2) on the same filesystem is guaranteed atomic.
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| Error::AtomicWriteFailed {
        path: path.to_path_buf(),
        reason: "no parent directory".into(),
    })?;

    // Ensure parent directory exists
    std::fs::create_dir_all(dir)?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| Error::AtomicWriteFailed {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    std::io::Write::write_all(&mut tmp, data).map_err(|e| Error::AtomicWriteFailed {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    tmp.persist(path).map_err(|e| Error::AtomicWriteFailed {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Atomically write a serde-serializable value as pretty-printed JSON.
pub fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let data = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");

        atomic_write(&path, b"hello world").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[test]
    fn atomic_write_json_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");

        let data = serde_json::json!({"key": "value", "num": 42});
        atomic_write_json(&path, &data).unwrap();

        let read: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read["key"], "value");
        assert_eq!(read["num"], 42);
    }
}
