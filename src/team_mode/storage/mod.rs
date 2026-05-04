//! Team Mode transcript-first storage layer.
//!
//! This layer persists Team Mode entities directly and keeps inbox/thread
//! views as projections over the message transcript.

use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::error::Result;
use crate::util::file_lock::FileLock;
use crate::util::validate_name;

pub mod member_store;
pub mod message_store;
pub mod projection_store;
pub mod room_store;
pub mod team_store;

pub use member_store::{MemberRecord, MemberStore};
pub use message_store::MessageStore;
pub use projection_store::ProjectionStore;
pub use room_store::RoomStore;
pub use team_store::{TeamDeleteMode, TeamStore};

pub(crate) fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    Ok(())
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let data = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&data)?)
}

pub(crate) fn read_json_opt<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    read_json(path).map(Some)
}

/// Acquire a FileLock at an explicit path. Caller is responsible for
/// making sure the parent directory exists.
pub(crate) fn acquire_lock_path(path: &Path) -> Result<FileLock> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    FileLock::acquire(path)
}

pub(crate) fn validate_storage_name(name: &str) -> Result<()> {
    validate_name(name)
}
