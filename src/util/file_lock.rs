//! Cross-platform file locking via `fs2`.
//!
//! Provides exclusive file locks that work on both Unix (`flock(2)`) and
//! Windows (`LockFileEx`). Uses the `fs2` crate which abstracts over
//! platform-specific locking APIs.
//!
//! Mirrors the Python `fcntl.flock(fd, LOCK_EX)` pattern used in
//! `claude-code-teams-mcp` for concurrent file access protection.
//!
//! # Platform notes
//!
//! - **macOS / Linux (local FS)**: Reliable advisory locks via `flock(2)`.
//! - **Windows**: Uses `LockFileEx` kernel API, mandatory locks.
//! - **NFS / CIFS / network filesystems**: `flock(2)` behavior is
//!   implementation-dependent and may silently succeed without actual locking.
//!   If the teams base directory resides on a network filesystem, consider
//!   using a distributed lock service instead.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::error::{Error, Result};

/// An exclusive file lock (RAII guard).
///
/// The lock is released when this value is dropped (the underlying file
/// descriptor / handle is closed, which releases the lock).
pub struct FileLock {
    file: File,
    path: PathBuf,
}

impl FileLock {
    /// Acquire an exclusive lock on `path`, blocking until available.
    ///
    /// If the lock file does not exist, it is created.
    pub fn acquire(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| Error::LockFailed {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;

        file.lock_exclusive().map_err(|e| Error::LockFailed {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    /// Try to acquire an exclusive lock without blocking.
    ///
    /// Returns `Ok(None)` if the lock is held by another process.
    pub fn try_acquire(path: &Path) -> Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| Error::LockFailed {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;

        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self {
                file,
                path: path.to_path_buf(),
            })),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            // fs2 can surface platform lock contention as raw OS errors:
            // EAGAIN = 35 on macOS, 11 on Linux; Windows ERROR_LOCK_VIOLATION = 33.
            Err(ref e)
                if e.raw_os_error()
                    .is_some_and(|code| code == 11 || code == 35 || code == 33) =>
            {
                Ok(None)
            }
            Err(e) => Err(Error::LockFailed {
                path: path.to_path_buf(),
                reason: e.to_string(),
            }),
        }
    }

    /// Path of the lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Explicitly unlock before the fd/handle is closed.
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("test.lock");

        let lock = FileLock::acquire(&lock_path).unwrap();
        assert!(lock_path.exists());
        drop(lock);
    }

    #[test]
    fn try_acquire_non_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("test.lock");

        let lock1 = FileLock::try_acquire(&lock_path).unwrap();
        assert!(lock1.is_some());

        // Release and re-acquire to verify the API works.
        drop(lock1);

        let lock2 = FileLock::try_acquire(&lock_path).unwrap();
        assert!(lock2.is_some());
    }

    #[test]
    fn lock_path_accessor() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("test.lock");

        let lock = FileLock::acquire(&lock_path).unwrap();
        assert_eq!(lock.path(), lock_path);
    }
}
