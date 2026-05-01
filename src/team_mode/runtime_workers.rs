use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::team_mode::data_dir;

pub const WORKERS_FILE_VERSION: u32 = 1;
pub const STATE_STARTING: &str = "starting";
pub const STATE_RUNNING: &str = "running";
pub const STATE_STOPPED: &str = "stopped";
pub const STATE_FAILED: &str = "failed";
pub const STATE_DEAD: &str = "dead";

#[derive(Debug, Clone)]
pub struct RuntimeWorkerStore {
    base_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeWorkersFile {
    pub version: u32,
    #[serde(default)]
    pub workers: Vec<RuntimeWorkerRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeWorkerRecord {
    pub team: String,
    pub name: String,
    pub spawn_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl RuntimeWorkerStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    pub fn path(&self) -> PathBuf {
        crate::team_mode_daemon::runtime_dir(&self.base_dir).join("workers.json")
    }

    pub fn mark_daemon_restart_dead(&self, daemon_pid: u32) -> Result<usize> {
        self.update_file(|file| {
            let mut changed = 0;
            for worker in &mut file.workers {
                if matches!(worker.state.as_str(), STATE_STARTING | STATE_RUNNING) {
                    worker.state = STATE_DEAD.into();
                    worker.daemon_pid = Some(daemon_pid);
                    worker.note =
                        Some("daemon restarted; previous process ownership was lost".into());
                    worker.updated_at = Utc::now();
                    changed += 1;
                }
            }
            Ok(changed)
        })
    }

    pub fn upsert_state(
        &self,
        team: &str,
        name: &str,
        spawn_key: &str,
        adapter: Option<String>,
        state: &str,
        note: Option<String>,
    ) -> Result<()> {
        let daemon_pid = std::process::id();
        let reason = note.clone().unwrap_or_else(|| "unspecified".to_string());
        let result = self.update_file(|file| {
            let now = Utc::now();
            if let Some(worker) = file
                .workers
                .iter_mut()
                .find(|worker| worker.team == team && worker.name == name)
            {
                let prev_state = worker.state.clone();
                worker.spawn_key = spawn_key.into();
                worker.adapter = adapter.clone();
                worker.state = state.into();
                worker.daemon_pid = Some(daemon_pid);
                worker.note = note.clone();
                worker.updated_at = now;
                return Ok(prev_state);
            }

            file.workers.push(RuntimeWorkerRecord {
                team: team.into(),
                name: name.into(),
                spawn_key: spawn_key.into(),
                adapter: adapter.clone(),
                state: state.into(),
                daemon_pid: Some(daemon_pid),
                note,
                updated_at: now,
            });
            Ok("absent".to_string())
        });

        match result {
            Ok(prev_state) => {
                tracing::info!(
                    event = "runtime_worker.state_change",
                    team_id = %team,
                    worker_name = %name,
                    prev_state = %prev_state,
                    new_state = %state,
                    reason = %reason,
                    spawn_key = %spawn_key,
                    adapter = ?adapter,
                    daemon_pid,
                    "runtime worker state changed"
                );
                Ok(())
            }
            Err(err) => {
                tracing::error!(
                    event = "runtime_worker.state_change_failed",
                    team_id = %team,
                    worker_name = %name,
                    new_state = %state,
                    reason = %reason,
                    spawn_key = %spawn_key,
                    adapter = ?adapter,
                    daemon_pid,
                    error = %err,
                    "runtime worker state change failed"
                );
                Err(err)
            }
        }
    }

    pub fn remove_team(&self, team: &str) -> Result<()> {
        self.update_file(|file| {
            file.workers.retain(|worker| worker.team != team);
            Ok(())
        })
    }

    pub fn state_for(&self, team: &str, name: &str) -> Result<Option<String>> {
        let file = self.read_file()?;
        Ok(file
            .workers
            .into_iter()
            .find(|worker| worker.team == team && worker.name == name)
            .map(|worker| worker.state))
    }

    /// Snapshot every worker record currently in the sidecar. Used by the
    /// background liveness watchdog to scan for processes that died
    /// externally (kill, OOM, host crash) so the sidecar's `state` field
    /// stays in sync with reality.
    pub fn list_all(&self) -> Result<Vec<RuntimeWorkerRecord>> {
        let file = self.read_file()?;
        Ok(file.workers)
    }

    fn update_file<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut RuntimeWorkersFile) -> Result<T>,
    {
        std::fs::create_dir_all(crate::team_mode_daemon::runtime_dir(&self.base_dir))?;
        std::fs::create_dir_all(data_dir::locks_dir(&self.base_dir))?;
        let lock_path = data_dir::lock_path(&self.base_dir, "runtime-workers");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        lock.lock_exclusive().map_err(|err| Error::LockFailed {
            path: lock_path.clone(),
            reason: err.to_string(),
        })?;

        let result = (|| {
            let mut file = self.read_file_unlocked()?;
            let value = f(&mut file)?;
            self.write_file_unlocked(&file)?;
            Ok(value)
        })();

        let _ = fs2::FileExt::unlock(&lock);
        result
    }

    fn read_file(&self) -> Result<RuntimeWorkersFile> {
        std::fs::create_dir_all(crate::team_mode_daemon::runtime_dir(&self.base_dir))?;
        self.read_file_unlocked()
    }

    fn read_file_unlocked(&self) -> Result<RuntimeWorkersFile> {
        let path = self.path();
        if !path.exists() {
            return Ok(RuntimeWorkersFile {
                version: WORKERS_FILE_VERSION,
                workers: Vec::new(),
            });
        }
        let mut file: RuntimeWorkersFile = serde_json::from_slice(&std::fs::read(path)?)?;
        if file.version == 0 {
            file.version = WORKERS_FILE_VERSION;
        }
        Ok(file)
    }

    fn write_file_unlocked(&self, file: &RuntimeWorkersFile) -> Result<()> {
        let path = self.path();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(file)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::tracing_capture::capture_events;

    #[test]
    fn daemon_restart_marks_live_workers_dead() {
        let dir = tempdir().unwrap();
        let store = RuntimeWorkerStore::new(dir.path());
        store
            .upsert_state("team", "worker", "team__worker", None, STATE_RUNNING, None)
            .unwrap();

        let changed = store.mark_daemon_restart_dead(42).unwrap();

        assert_eq!(changed, 1);
        assert_eq!(
            store.state_for("team", "worker").unwrap().as_deref(),
            Some(STATE_DEAD)
        );
    }

    #[test]
    fn runtime_worker_upsert_emits_state_change_info_log() {
        let mut last_events = Vec::new();
        let event = (0..25)
            .find_map(|_| {
                let dir = tempdir().unwrap();
                let store = RuntimeWorkerStore::new(dir.path());
                let (result, events) = capture_events(|| {
                    store.upsert_state(
                        "team",
                        "worker",
                        "team__worker",
                        Some("codex".into()),
                        STATE_RUNNING,
                        Some("spawned by worker_add".into()),
                    )
                });
                result.unwrap();
                let event = events
                    .iter()
                    .find(|event| event.field("event") == Some("runtime_worker.state_change"))
                    .cloned();
                last_events = events;
                event
            })
            .unwrap_or_else(|| {
                panic!(
                    "runtime worker state_change log should be emitted; last captured events: {last_events:?}"
                )
            });
        assert_eq!(event.field("team_id"), Some("team"));
        assert_eq!(event.field("worker_name"), Some("worker"));
        assert_eq!(event.field("prev_state"), Some("absent"));
        assert_eq!(event.field("new_state"), Some(STATE_RUNNING));
        assert_eq!(event.field("reason"), Some("spawned by worker_add"));
        assert_eq!(event.field("adapter"), Some("Some(\"codex\")"));
        assert_eq!(event.field("spawn_key"), Some("team__worker"));
    }
}
