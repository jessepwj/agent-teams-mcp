use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::team_mode::data_dir;
use crate::team_mode::domain::Message;
use crate::team_mode::storage::{acquire_lock_path, ensure_dir, validate_storage_name};

/// Per-team append-only message transcript. `team_id` is a required
/// argument on most operations because messages are stored under
/// `<base>/<team>/messages.jsonl`.
#[derive(Debug, Clone)]
pub struct MessageStore {
    base_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "entryType", rename_all = "snake_case")]
enum TranscriptEntry {
    Message {
        message: Box<Message>,
    },
    Deleted {
        message_id: String,
        deleted_at: DateTime<Utc>,
    },
}

#[derive(Debug, Default)]
struct TranscriptSnapshot {
    order: Vec<String>,
    order_index: HashMap<String, usize>,
    active: HashMap<String, Message>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageUpdateResult {
    pub message: Message,
    pub changed: bool,
}

impl MessageStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    pub fn transcript_path(&self, team_id: &str) -> PathBuf {
        data_dir::messages_file(&self.base_dir, team_id)
    }

    fn lock_for(&self, team_id: &str) -> PathBuf {
        data_dir::lock_path(&self.base_dir, &format!("messages-{team_id}"))
    }

    fn append_entry(&self, team_id: &str, entry: &TranscriptEntry) -> Result<()> {
        ensure_dir(&data_dir::team_dir(&self.base_dir, team_id))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.transcript_path(team_id))?;
        let line = serde_json::to_string(entry)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    fn snapshot_locked(&self, team_id: &str) -> Result<TranscriptSnapshot> {
        let transcript = self.transcript_path(team_id);
        if !transcript.exists() {
            return Ok(TranscriptSnapshot::default());
        }

        let data = fs::read_to_string(transcript)?;
        let lines: Vec<&str> = data
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();

        let mut snapshot = TranscriptSnapshot::default();
        for (index, line) in lines.iter().enumerate() {
            match serde_json::from_str::<TranscriptEntry>(line) {
                Ok(TranscriptEntry::Message { message }) => {
                    let message = *message;
                    if !snapshot.order_index.contains_key(&message.id) {
                        snapshot
                            .order_index
                            .insert(message.id.clone(), snapshot.order.len());
                        snapshot.order.push(message.id.clone());
                    }
                    snapshot.active.insert(message.id.clone(), message);
                }
                Ok(TranscriptEntry::Deleted { message_id, .. }) => {
                    snapshot.active.remove(&message_id);
                }
                Err(_) if index + 1 == lines.len() => {
                    // Tolerate a torn final line (from a partial append).
                    break;
                }
                Err(err) => return Err(err.into()),
            }
        }

        Ok(snapshot)
    }

    fn snapshot(&self, team_id: &str) -> Result<TranscriptSnapshot> {
        ensure_dir(&self.base_dir.join(data_dir::DIR_LOCKS))?;
        let _lock = acquire_lock_path(&self.lock_for(team_id))?;
        self.snapshot_locked(team_id)
    }

    pub fn save(&self, message: &Message) -> Result<()> {
        let team_id = message
            .team_id
            .as_deref()
            .ok_or_else(|| Error::Other("message is missing team_id".into()))?;
        validate_storage_name(team_id)?;
        validate_storage_name(&message.id)?;
        validate_storage_name(&message.room_id)?;
        if let Some(thread_id) = &message.thread_id {
            validate_storage_name(thread_id)?;
        }

        ensure_dir(&self.base_dir.join(data_dir::DIR_LOCKS))?;
        let _lock = acquire_lock_path(&self.lock_for(team_id))?;
        self.append_entry(
            team_id,
            &TranscriptEntry::Message {
                message: Box::new(message.clone()),
            },
        )
    }

    pub fn get(&self, team_id: &str, id: impl AsRef<str>) -> Result<Option<Message>> {
        let id = id.as_ref();
        validate_storage_name(team_id)?;
        validate_storage_name(id)?;
        let snapshot = self.snapshot(team_id)?;
        Ok(snapshot.active.get(id).cloned())
    }

    pub fn list(&self, team_id: &str) -> Result<Vec<Message>> {
        validate_storage_name(team_id)?;
        let snapshot = self.snapshot(team_id)?;
        let mut messages = Vec::with_capacity(snapshot.order.len());
        for id in snapshot.order {
            if let Some(message) = snapshot.active.get(&id) {
                messages.push(message.clone());
            }
        }
        Ok(messages)
    }

    pub fn list_by_room(&self, team_id: &str, room_id: impl AsRef<str>) -> Result<Vec<Message>> {
        let room_id = room_id.as_ref();
        validate_storage_name(room_id)?;
        Ok(self
            .list(team_id)?
            .into_iter()
            .filter(|m| m.room_id == room_id)
            .collect())
    }

    pub fn list_by_thread(
        &self,
        team_id: &str,
        thread_id: impl AsRef<str>,
    ) -> Result<Vec<Message>> {
        let thread_id = thread_id.as_ref();
        validate_storage_name(thread_id)?;
        Ok(self
            .list(team_id)?
            .into_iter()
            .filter(|m| m.thread_id.as_deref() == Some(thread_id))
            .collect())
    }

    pub fn delete(&self, team_id: &str, id: impl AsRef<str>) -> Result<()> {
        let id = id.as_ref();
        validate_storage_name(team_id)?;
        validate_storage_name(id)?;
        ensure_dir(&self.base_dir.join(data_dir::DIR_LOCKS))?;
        let _lock = acquire_lock_path(&self.lock_for(team_id))?;
        self.append_entry(
            team_id,
            &TranscriptEntry::Deleted {
                message_id: id.to_string(),
                deleted_at: Utc::now(),
            },
        )
    }

    pub fn update<F>(
        &self,
        team_id: &str,
        id: impl AsRef<str>,
        updater: F,
    ) -> Result<Option<MessageUpdateResult>>
    where
        F: FnOnce(&mut Message) -> Result<bool>,
    {
        let id = id.as_ref();
        validate_storage_name(team_id)?;
        validate_storage_name(id)?;
        ensure_dir(&self.base_dir.join(data_dir::DIR_LOCKS))?;
        let _lock = acquire_lock_path(&self.lock_for(team_id))?;
        let snapshot = self.snapshot_locked(team_id)?;
        let Some(mut message) = snapshot.active.get(id).cloned() else {
            return Ok(None);
        };
        let changed = updater(&mut message)?;
        if changed {
            self.append_entry(
                team_id,
                &TranscriptEntry::Message {
                    message: Box::new(message.clone()),
                },
            )?;
        }
        Ok(Some(MessageUpdateResult { message, changed }))
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{
        AudiencePolicy, DeliveryDrop, DeliveryStatus, DropReason, Message, MessageKind,
        MessageReceipt, VisibilityReason, VisibilityResolution, VisibilityRule,
    };

    fn sample_message(team: &str, id: &str, recipient: &str, thread_id: &str) -> Message {
        Message {
            id: id.into(),
            room_id: "main".into(),
            team_id: Some(team.into()),
            thread_id: Some(thread_id.into()),
            reply_to: None,
            sender: "alice".into(),
            kind: MessageKind::Dispatch,
            subject: Some("Review".into()),
            body: "Please review.".into(),
            mentions: vec![recipient.into()],
            visibility: vec![VisibilityRule::Team],
            audience_policy: Some(AudiencePolicy::Team),
            effective_visibility_reason: Some(VisibilityResolution {
                policy: AudiencePolicy::Team,
                reasons: vec![VisibilityReason::TeamWide],
            }),
            effective_recipients: vec![recipient.into()],
            delivered_to: vec![recipient.into()],
            dropped_for: vec![DeliveryDrop {
                recipient: "other".into(),
                reason: DropReason::HiddenByVisibility,
            }],
            read_by: vec![MessageReceipt {
                actor: recipient.into(),
                at: Utc::now(),
            }],
            acked_by: vec![],
            delivery_status: DeliveryStatus::Delivered,
            created_at: Utc::now(),
            expires_at: None,
        }
    }

    #[test]
    fn save_and_get_round_trip() {
        let dir = tempdir().unwrap();
        let store = MessageStore::new(dir.path());
        store
            .save(&sample_message("demo", "msg-1", "alice", "t1"))
            .unwrap();
        assert_eq!(store.get("demo", "msg-1").unwrap().unwrap().id, "msg-1");
    }

    #[test]
    fn delete_hides_message_and_survives_truncated_tail() {
        let dir = tempdir().unwrap();
        let store = MessageStore::new(dir.path());
        store
            .save(&sample_message("demo", "msg-1", "alice", "t1"))
            .unwrap();
        store.delete("demo", "msg-1").unwrap();
        assert!(store.get("demo", "msg-1").unwrap().is_none());

        // torn final line
        OpenOptions::new()
            .append(true)
            .open(store.transcript_path("demo"))
            .unwrap()
            .write_all(b"{\"entryType\":\"message\",\"message\":{\"id\":\"broken\"")
            .unwrap();
        assert!(store.get("demo", "msg-1").unwrap().is_none());
    }

    #[test]
    fn update_appends_new_version() {
        let dir = tempdir().unwrap();
        let store = MessageStore::new(dir.path());
        store
            .save(&sample_message("demo", "msg-5", "member-1", "t1"))
            .unwrap();
        let out = store
            .update("demo", "msg-5", |m| {
                m.read_by.push(MessageReceipt {
                    actor: "member-1".into(),
                    at: Utc::now(),
                });
                Ok(true)
            })
            .unwrap()
            .unwrap();
        assert!(out.changed);
        assert_eq!(out.message.read_by.len(), 2);
        let transcript = std::fs::read_to_string(store.transcript_path("demo")).unwrap();
        assert_eq!(transcript.matches(r#""id":"msg-5""#).count(), 2);
    }

    #[test]
    fn list_by_room_and_thread_filter_correctly() {
        let dir = tempdir().unwrap();
        let store = MessageStore::new(dir.path());
        store
            .save(&sample_message("demo", "m1", "a", "t1"))
            .unwrap();
        store
            .save(&sample_message("demo", "m2", "a", "t2"))
            .unwrap();
        assert_eq!(store.list_by_room("demo", "main").unwrap().len(), 2);
        assert_eq!(store.list_by_thread("demo", "t1").unwrap().len(), 1);
    }

    #[test]
    fn per_team_isolation() {
        let dir = tempdir().unwrap();
        let store = MessageStore::new(dir.path());
        store
            .save(&sample_message("alpha", "a1", "x", "t"))
            .unwrap();
        store
            .save(&sample_message("bravo", "b1", "x", "t"))
            .unwrap();
        assert_eq!(store.list("alpha").unwrap().len(), 1);
        assert_eq!(store.list("bravo").unwrap().len(), 1);
        assert!(store.get("alpha", "b1").unwrap().is_none());
    }
}
