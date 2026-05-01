//! Lead pending queue writer (per-team, post-2026-04-30 refactor).
//!
//! Appends a JSON line to `<base_dir>/<team_id>/lead_pending.jsonl` whenever
//! a message is delivered to that team's lead. The file is watched by a
//! Claude Code Stop hook (`asyncRewake: true`) which polls files for the
//! teams the current CC owns and injects new entries as `<system-reminder>`.
//!
//! Per-team file naming replaces the previous single `<base>/lead_pending.jsonl`
//! design. The single-file model required hooks to filter entries by walking
//! their CC ancestor chain via PowerShell on Windows (~4.2s per fire), which
//! caused Stop hooks to be SIGKILL'd by CC before they could log progress.
//! Per-team files let the service decide ownership at write time, eliminating
//! the hook-side classification step entirely.
//!
//! Routing for hooks is supplied by the HTTP `GET /lead-pending/my-teams`
//! endpoint (see `mcp/http_transport.rs`), which the hook calls once per
//! process to learn which teams it owns.
//!
//! The writer is intentionally best-effort: a failure to append must never
//! block the underlying message send. Inbox projections and the MCP
//! `inbox_read` tool remain the source of truth; this file is only a
//! notification sidecar.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::team_mode::data_dir;
use crate::team_mode::domain::{Message, MessageKind};
use crate::util::file_lock::FileLock;

/// Filename for the pending queue inside a team's directory.
pub const PENDING_FILENAME: &str = "lead_pending.jsonl";
/// Lock file used while appending — sits next to the pending file.
const LOCK_FILENAME: &str = ".lead_pending.lock";
/// Legacy single-file location (relative to base_dir). Migrated on startup.
const LEGACY_FILENAME: &str = "lead_pending.jsonl";

/// Cap how much of a worker's reply body we copy into the pending queue.
/// The hook script feeds `text` directly to CC as the wake reason, so an
/// unbounded body bloats the lead's context. 16 KB ≈ 4K tokens, large
/// enough for normal "completion summary + next-step" replies. Original
/// body is preserved untouched in `messages.jsonl`.
const PENDING_TEXT_MAX_BYTES: usize = 16 * 1024;
const TRUNCATION_MARKER: &str = "\n\n[…body truncated by lead_pending. Call mcp__team-mode__inbox_read or read messages.jsonl for the full reply.]";

/// One line in a per-team pending queue.
///
/// `owner_cc_pid` is retained as `Option<u32>` for backwards compatibility
/// with legacy single-file entries during migration. New writes leave it
/// `None` because the file path itself encodes ownership (it lives under
/// `<team_id>/`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct LeadPendingEntry {
    pub team: String,
    pub from: String,
    pub from_id: String,
    pub msg_id: String,
    pub kind: String,
    pub text: String,
    pub ts: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_cc_pid: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct LeadPendingWriter {
    base_dir: PathBuf,
    /// Optional legacy location for one-shot migration. Pre-2026-04-30 the
    /// single-file `lead_pending.jsonl` lived at the project root (repo cwd)
    /// because of FileChanged hook matcher constraints; the new layout lives
    /// under `<base_dir>/<team_id>/`. Migration scans both this path and
    /// `<base_dir>/lead_pending.jsonl` so neither location is left orphaned.
    legacy_root_dir: Option<PathBuf>,
}

impl LeadPendingWriter {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            legacy_root_dir: None,
        }
    }

    /// Set an additional directory to scan during `migrate_legacy()`. Used
    /// to point at the project root when the data dir is nested under
    /// `<project>/.agent-teams/`.
    pub fn with_legacy_root(mut self, dir: impl Into<PathBuf>) -> Self {
        self.legacy_root_dir = Some(dir.into());
        self
    }

    /// Path to the per-team pending file: `<base>/<team_id>/lead_pending.jsonl`.
    pub fn path_for(&self, team_id: &str) -> PathBuf {
        data_dir::team_dir(&self.base_dir, team_id).join(PENDING_FILENAME)
    }

    /// Path to the per-team lock file. Each team has its own lock so two
    /// teams can write concurrently without contention.
    fn lock_path_for(&self, team_id: &str) -> PathBuf {
        data_dir::team_dir(&self.base_dir, team_id).join(LOCK_FILENAME)
    }

    /// Append an entry if the message's `effective_recipients` include the
    /// lead. Returns `Ok(true)` when an entry was written.
    pub fn maybe_write(
        &self,
        message: &Message,
        lead_member_id: &str,
        from_display_name: &str,
    ) -> Result<bool> {
        if !message
            .effective_recipients
            .iter()
            .any(|r| r == lead_member_id)
        {
            return Ok(false);
        }

        let team_id = match message.team_id.as_deref() {
            Some(id) if !id.is_empty() => id,
            _ => {
                tracing::warn!(
                    event = "lead_pending.append_skipped",
                    message_id = %message.id,
                    "skipping lead_pending append: message has no team_id"
                );
                return Ok(false);
            }
        };

        let team_dir = data_dir::team_dir(&self.base_dir, team_id);
        fs::create_dir_all(&team_dir)?;
        let _lock = FileLock::acquire(&self.lock_path_for(team_id))?;

        let entry = LeadPendingEntry {
            team: team_id.to_string(),
            from: from_display_name.to_string(),
            from_id: message.sender.clone(),
            msg_id: message.id.clone(),
            kind: kind_to_str(&message.kind).to_string(),
            text: truncate_body_for_pending(&message.body),
            ts: message.created_at,
            reply_to: message.reply_to.clone(),
            // owner_cc_pid intentionally omitted for new entries —
            // file path under `<team_id>/` is the authoritative owner.
            owner_cc_pid: None,
        };

        let json = serde_json::to_string(&entry)?;
        let path = self.path_for(team_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{json}")?;
        tracing::info!(
            event = "lead_pending.append",
            team_id = %team_id,
            message_id = %entry.msg_id,
            recipient_count = message.effective_recipients.len(),
            byte_size = json.len() + 1,
            from_id = %entry.from_id,
            kind = %entry.kind,
            path = %path.display(),
            "lead_pending append succeeded"
        );
        Ok(true)
    }

    /// One-shot startup migration of the legacy `<base>/lead_pending.jsonl`
    /// (single-file model) into per-team files.
    ///
    /// Behaviour:
    /// - Parses each line; entries with a non-empty `team` field are
    ///   appended to `<base>/<team_id>/lead_pending.jsonl` (creating the
    ///   team_dir if needed — even an orphan team with no team.json gets
    ///   its dir back so messages aren't lost).
    /// - Entries with malformed JSON or missing `team` are written to
    ///   `<base>/.legacy_unrouted_pending.jsonl` for forensic recovery.
    /// - On full success, the legacy file is deleted.
    /// - On any IO error, the legacy file is preserved so the next startup
    ///   can retry.
    ///
    /// Idempotent: if no legacy file is found (already migrated or never
    /// existed), this is a no-op. Scans both `<base_dir>/lead_pending.jsonl`
    /// and `<legacy_root_dir>/lead_pending.jsonl` (if configured) so
    /// pre-refactor data at the project root is not left behind.
    pub fn migrate_legacy(&self) -> Result<usize> {
        let mut total = 0;
        let mut paths: Vec<PathBuf> = vec![self.base_dir.join(LEGACY_FILENAME)];
        if let Some(root) = &self.legacy_root_dir {
            let candidate = root.join(LEGACY_FILENAME);
            // Avoid double-scan if base_dir == legacy_root_dir.
            if !paths.iter().any(|p| p == &candidate) {
                paths.push(candidate);
            }
        }
        for legacy_path in paths {
            total += self.migrate_legacy_path(&legacy_path)?;
        }
        Ok(total)
    }

    fn migrate_legacy_path(&self, legacy_path: &std::path::Path) -> Result<usize> {
        if !legacy_path.exists() {
            return Ok(0);
        }
        let content = match fs::read_to_string(legacy_path) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(
                    event = "lead_pending.migrate_legacy_read_failed",
                    error = %err,
                    path = %legacy_path.display()
                );
                return Ok(0);
            }
        };

        let mut appended = 0usize;
        let mut unrouted: Vec<String> = Vec::new();
        let mut by_team: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<LeadPendingEntry>(trimmed) {
                Ok(entry) if !entry.team.is_empty() => {
                    by_team
                        .entry(entry.team.clone())
                        .or_default()
                        .push(trimmed.to_string());
                }
                _ => {
                    unrouted.push(trimmed.to_string());
                }
            }
        }

        for (team_id, lines) in &by_team {
            let team_dir = data_dir::team_dir(&self.base_dir, team_id);
            if let Err(err) = fs::create_dir_all(&team_dir) {
                tracing::error!(
                    event = "lead_pending.migrate_legacy_mkdir_failed",
                    team_id = %team_id,
                    error = %err
                );
                return Ok(0); // bail — leave legacy in place for retry
            }
            let _lock = match FileLock::acquire(&self.lock_path_for(team_id)) {
                Ok(l) => l,
                Err(err) => {
                    tracing::error!(
                        event = "lead_pending.migrate_legacy_lock_failed",
                        team_id = %team_id,
                        error = %err
                    );
                    return Ok(0);
                }
            };
            let path = self.path_for(team_id);
            let mut existing_msg_ids = pending_msg_ids(&path);
            let mut file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => f,
                Err(err) => {
                    tracing::error!(
                        event = "lead_pending.migrate_legacy_open_failed",
                        path = %path.display(),
                        error = %err
                    );
                    return Ok(0);
                }
            };
            for ln in lines {
                let msg_id = serde_json::from_str::<LeadPendingEntry>(ln)
                    .ok()
                    .map(|entry| entry.msg_id)
                    .filter(|msg_id| !msg_id.is_empty());
                if msg_id
                    .as_ref()
                    .is_some_and(|msg_id| existing_msg_ids.contains(msg_id))
                {
                    continue;
                }
                if let Err(err) = writeln!(file, "{ln}") {
                    tracing::error!(
                        event = "lead_pending.migrate_legacy_write_failed",
                        path = %path.display(),
                        error = %err
                    );
                    return Ok(0);
                }
                if let Some(msg_id) = msg_id {
                    existing_msg_ids.insert(msg_id);
                }
                appended += 1;
            }
        }

        if !unrouted.is_empty() {
            let unrouted_path = self.base_dir.join(".legacy_unrouted_pending.jsonl");
            if let Ok(mut f) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&unrouted_path)
            {
                for ln in &unrouted {
                    let _ = writeln!(f, "{ln}");
                }
                tracing::warn!(
                    event = "lead_pending.migrate_legacy_unrouted",
                    count = unrouted.len(),
                    path = %unrouted_path.display(),
                    "legacy lead_pending entries lacked a team field; preserved for forensics"
                );
            }
        }

        if let Err(err) = fs::remove_file(legacy_path) {
            tracing::error!(
                event = "lead_pending.migrate_legacy_delete_failed",
                path = %legacy_path.display(),
                error = %err
            );
            return Ok(appended);
        }
        tracing::info!(
            event = "lead_pending.migrate_legacy_done",
            routed = appended,
            unrouted = unrouted.len(),
            teams = by_team.len(),
            "legacy lead_pending.jsonl distributed into per-team files"
        );
        Ok(appended)
    }
}

fn pending_msg_ids(path: &Path) -> HashSet<String> {
    let Ok(content) = fs::read_to_string(path) else {
        return HashSet::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<LeadPendingEntry>(line.trim()).ok())
        .map(|entry| entry.msg_id)
        .filter(|msg_id| !msg_id.is_empty())
        .collect()
}

/// Truncate a worker reply body for the pending-queue payload so
/// catastrophic blow-ups (Bug 26 codex bloat and similar future leaks)
/// can't single-handedly flood the lead's context. UTF-8 safe.
pub(crate) fn truncate_body_for_pending(body: &str) -> String {
    if body.len() <= PENDING_TEXT_MAX_BYTES {
        return body.to_string();
    }
    let mut end = PENDING_TEXT_MAX_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + TRUNCATION_MARKER.len());
    out.push_str(&body[..end]);
    out.push_str(TRUNCATION_MARKER);
    out
}

fn kind_to_str(kind: &MessageKind) -> &'static str {
    match kind {
        MessageKind::Dispatch => "dispatch",
        MessageKind::Discussion => "discussion",
        MessageKind::Reply => "reply",
        MessageKind::System => "system",
        MessageKind::Notice => "notice",
        MessageKind::Status => "status",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::team_mode::domain::{DeliveryStatus, Message, MessageKind};
    use crate::team_mode::tracing_capture::capture_events;

    fn sample_message(team_id: &str, recipients: Vec<&str>) -> Message {
        Message {
            id: "msg-1".into(),
            room_id: "main".into(),
            team_id: Some(team_id.into()),
            thread_id: Some("thread-1".into()),
            reply_to: Some("msg-0".into()),
            sender: "demo-alice".into(),
            kind: MessageKind::Reply,
            subject: None,
            body: "hello lead".into(),
            mentions: Vec::new(),
            visibility: Vec::new(),
            audience_policy: None,
            effective_visibility_reason: None,
            effective_recipients: recipients.into_iter().map(String::from).collect(),
            delivered_to: Vec::new(),
            dropped_for: Vec::new(),
            read_by: Vec::new(),
            acked_by: Vec::new(),
            delivery_status: DeliveryStatus::Delivered,
            created_at: Utc::now(),
            expires_at: None,
        }
    }

    #[test]
    fn writes_entry_under_team_dir() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());

        let msg = sample_message("demo", vec!["demo-lead", "demo-bob"]);
        let wrote = writer.maybe_write(&msg, "demo-lead", "alice").unwrap();
        assert!(wrote);

        let expected = dir.path().join("demo").join(PENDING_FILENAME);
        assert!(expected.exists(), "expected per-team file at {expected:?}");
        let content = fs::read_to_string(&expected).unwrap();
        assert_eq!(content.lines().count(), 1);
        let entry: LeadPendingEntry =
            serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(entry.team, "demo");
        assert_eq!(entry.from, "alice");
        assert!(
            entry.owner_cc_pid.is_none(),
            "new entries omit owner_cc_pid"
        );
    }

    #[test]
    fn skips_when_lead_not_recipient() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());

        let msg = sample_message("demo", vec!["demo-bob"]);
        let wrote = writer.maybe_write(&msg, "demo-lead", "alice").unwrap();
        assert!(!wrote);
        let path = writer.path_for("demo");
        assert!(!path.exists());
    }

    #[test]
    fn skips_when_message_has_no_team_id() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());

        let mut msg = sample_message("demo", vec!["demo-lead"]);
        msg.team_id = None;
        let wrote = writer.maybe_write(&msg, "demo-lead", "alice").unwrap();
        assert!(!wrote, "messages with no team should not be written");
    }

    #[test]
    fn appends_multiple_entries_to_same_team() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());

        let msg = sample_message("demo", vec!["demo-lead"]);
        writer.maybe_write(&msg, "demo-lead", "alice").unwrap();
        writer.maybe_write(&msg, "demo-lead", "alice").unwrap();
        writer.maybe_write(&msg, "demo-lead", "alice").unwrap();

        let content = fs::read_to_string(writer.path_for("demo")).unwrap();
        assert_eq!(content.lines().count(), 3);
    }

    #[test]
    fn separate_teams_get_separate_files() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());

        writer
            .maybe_write(
                &sample_message("alpha", vec!["alpha-lead"]),
                "alpha-lead",
                "a",
            )
            .unwrap();
        writer
            .maybe_write(
                &sample_message("bravo", vec!["bravo-lead"]),
                "bravo-lead",
                "b",
            )
            .unwrap();

        assert!(writer.path_for("alpha").exists());
        assert!(writer.path_for("bravo").exists());
        let alpha = fs::read_to_string(writer.path_for("alpha")).unwrap();
        let bravo = fs::read_to_string(writer.path_for("bravo")).unwrap();
        assert!(alpha.contains("alpha"));
        assert!(bravo.contains("bravo"));
    }

    #[test]
    fn lead_pending_append_emits_structured_info_log() {
        let mut last_events = Vec::new();
        let event = (0..25)
            .find_map(|_| {
                let dir = tempdir().unwrap();
                let writer = LeadPendingWriter::new(dir.path());
                let msg = sample_message("demo", vec!["demo-lead", "demo-bob"]);
                let (wrote, events) = capture_events(|| {
                    writer.maybe_write(&msg, "demo-lead", "alice").unwrap()
                });
                assert!(wrote);
                let event = events
                    .iter()
                    .find(|event| event.field("event") == Some("lead_pending.append"))
                    .cloned();
                last_events = events;
                event
            })
            .unwrap_or_else(|| {
                panic!(
                    "lead_pending append log should be emitted; last captured events: {last_events:?}"
                )
            });
        assert_eq!(event.field("team_id"), Some("demo"));
        assert_eq!(event.field("message_id"), Some("msg-1"));
        assert_eq!(event.field("recipient_count"), Some("2"));
        assert_eq!(event.field("from_id"), Some("demo-alice"));
        assert_eq!(event.field("kind"), Some("reply"));
    }

    #[test]
    fn migrate_legacy_distributes_entries_by_team() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());

        // Hand-craft a legacy file with 2 teams + 1 unrouted line.
        let legacy = dir.path().join(LEGACY_FILENAME);
        let now = Utc::now();
        let mut buf = String::new();
        for (team, msg_id) in [("alpha", "m1"), ("alpha", "m2"), ("bravo", "m3")] {
            let entry = LeadPendingEntry {
                team: team.into(),
                from: "x".into(),
                from_id: "x".into(),
                msg_id: msg_id.into(),
                kind: "reply".into(),
                text: "body".into(),
                ts: now,
                reply_to: None,
                owner_cc_pid: Some(1234),
            };
            buf.push_str(&serde_json::to_string(&entry).unwrap());
            buf.push('\n');
        }
        // Malformed line — should land in unrouted forensic file.
        buf.push_str("not json\n");
        fs::write(&legacy, &buf).unwrap();

        let routed = writer.migrate_legacy().unwrap();
        assert_eq!(routed, 3);
        assert!(!legacy.exists(), "legacy file should be deleted on success");

        let alpha = fs::read_to_string(writer.path_for("alpha")).unwrap();
        let bravo = fs::read_to_string(writer.path_for("bravo")).unwrap();
        assert_eq!(alpha.lines().count(), 2);
        assert_eq!(bravo.lines().count(), 1);

        let unrouted = dir.path().join(".legacy_unrouted_pending.jsonl");
        assert!(unrouted.exists());
        let u = fs::read_to_string(&unrouted).unwrap();
        assert!(u.contains("not json"));
    }

    #[test]
    fn migrate_legacy_is_noop_when_file_missing() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());
        let routed = writer.migrate_legacy().unwrap();
        assert_eq!(routed, 0);
    }

    #[test]
    fn migrate_legacy_retry_does_not_duplicate_already_appended_entries() {
        let dir = tempdir().unwrap();
        let writer = LeadPendingWriter::new(dir.path());
        let legacy = dir.path().join(LEGACY_FILENAME);
        let entry = LeadPendingEntry {
            team: "demo".into(),
            from: "alice".into(),
            from_id: "alice".into(),
            msg_id: "msg-already-routed".into(),
            kind: "reply".into(),
            text: "body".into(),
            ts: Utc::now(),
            reply_to: None,
            owner_cc_pid: Some(1234),
        };
        let line = serde_json::to_string(&entry).unwrap();

        fs::create_dir_all(data_dir::team_dir(dir.path(), "demo")).unwrap();
        fs::write(writer.path_for("demo"), format!("{line}\n")).unwrap();
        fs::write(&legacy, format!("{line}\n")).unwrap();

        let routed = writer.migrate_legacy().unwrap();

        assert_eq!(routed, 0);
        let content = fs::read_to_string(writer.path_for("demo")).unwrap();
        assert_eq!(content.lines().count(), 1);
        assert!(
            !legacy.exists(),
            "legacy file should be cleared after retry"
        );
    }

    #[test]
    fn truncate_body_passes_through_small_input() {
        let body = "short reply";
        assert_eq!(truncate_body_for_pending(body), body);
    }

    #[test]
    fn truncate_body_caps_at_max_with_marker() {
        let oversize = TRUNCATION_MARKER.len() + 1024;
        let body = "x".repeat(PENDING_TEXT_MAX_BYTES + oversize);
        let out = truncate_body_for_pending(&body);
        assert!(out.len() <= PENDING_TEXT_MAX_BYTES + TRUNCATION_MARKER.len());
        assert!(out.len() < body.len());
        assert!(out.starts_with("xxxxxxxx"));
        assert!(out.contains("inbox_read"));
    }

    #[test]
    fn truncate_body_respects_utf8_boundary() {
        let prefix_bytes = PENDING_TEXT_MAX_BYTES - 1;
        let mut body = "x".repeat(prefix_bytes);
        body.push('中');
        body.push_str(&"y".repeat(2000));
        let out = truncate_body_for_pending(&body);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }
}
