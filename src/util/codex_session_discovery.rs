//! Auto-discovery of Codex CLI rollout JSONL files.
//!
//! Codex stores per-thread rollouts at
//! `~/.codex/sessions/YYYY/MM/DD/rollout-<UTC-ts>-<thread-id>.jsonl`.
//! Each file's first line is a `session_meta` event whose payload carries
//! the canonical `id` (matches the backend `thread_id`), `cwd`, and start
//! timestamp.
//!
//! Unlike Claude Code's per-project layout, codex groups rollouts by date,
//! so locating the file for a given worker requires either:
//!   * scanning the date tree and matching by stored `thread_id`, or
//!   * scanning + filtering by `session_meta.cwd` for "any session under
//!     this project".
//!
//! The full scan is O(N) in number of rollout files (only the first ~1MiB
//! of each file is read), so we cache the parsed result for 5 s. That's
//! plenty of room for the web UI's request bursts (each detail page hits
//! `/conversation` once and the team detail page can hit it for several
//! members in parallel) without going stale enough to mask a freshly
//! created session.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// One discovered codex rollout file.
#[derive(Debug, Clone)]
pub struct CodexSessionFile {
    /// Codex thread id (from `session_meta.payload.id`). Matches the
    /// `threadId` returned by `thread/start` and the `<id>` segment of the
    /// rollout filename.
    pub session_id: String,
    /// Absolute path to the rollout JSONL file.
    pub path: PathBuf,
    /// `session_meta.payload.cwd` as written by codex (raw, un-normalized).
    pub cwd: String,
    /// Session start time per the meta line (UTC).
    pub timestamp: Option<DateTime<Utc>>,
    /// File mtime (UTC).
    pub modified: Option<DateTime<Utc>>,
    /// File size in bytes.
    pub size: u64,
}

const FIRST_LINE_MAX_BYTES: u64 = 1024 * 1024;
const SCAN_CACHE_TTL: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct MetaLine {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    payload: Option<MetaPayload>,
}

#[derive(Deserialize)]
struct MetaPayload {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
}

/// Resolve the codex sessions directory root.
///
/// Resolution order: explicit `CODEX_SESSIONS_DIR`, then `CODEX_HOME/sessions`,
/// then platform default `~/.codex/sessions`. Returning a fallback (rather
/// than an error) lets callers report "no sessions" cleanly instead of
/// having to special-case missing-codex installs.
pub fn codex_sessions_root() -> PathBuf {
    if let Ok(env) = std::env::var("CODEX_SESSIONS_DIR") {
        if !env.is_empty() {
            return PathBuf::from(env);
        }
    }
    if let Ok(env) = std::env::var("CODEX_HOME") {
        if !env.is_empty() {
            return PathBuf::from(env).join("sessions");
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".codex").join("sessions"))
        .unwrap_or_else(|| PathBuf::from(".codex/sessions"))
}

/// Lossy comparison-friendly form of a filesystem path.
///
/// Codex writes the cwd to `session_meta` exactly as the host CLI saw it
/// (`E:\\aigc...` on Windows, with original case). Our caller passes a path
/// rebuilt from team config / `std::env::current_dir()`, which may differ
/// in slash style or case. Lower-casing + slash-normalizing both sides
/// avoids the false negatives that a strict string compare would produce.
fn normalize_path_for_compare(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push('/'),
            // Lowercase ASCII letters; non-ASCII (Chinese path segments)
            // pass through unchanged. Drive letters on Windows are ASCII so
            // this still folds them.
            c if c.is_ascii_uppercase() => out.push(c.to_ascii_lowercase()),
            c => out.push(c),
        }
    }
    while out.ends_with('/') {
        out.pop();
    }
    out
}

fn read_session_meta(path: &Path) -> Option<MetaLine> {
    let f = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(f.take(FIRST_LINE_MAX_BYTES));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn build_session_file(path: PathBuf) -> Option<CodexSessionFile> {
    let meta = read_session_meta(&path)?;
    if meta.kind != "session_meta" {
        return None;
    }
    let payload = meta.payload?;
    let id = payload.id?;
    let cwd = payload.cwd.unwrap_or_default();
    let timestamp = payload.timestamp.as_deref().and_then(parse_iso);
    let fs_meta = fs::metadata(&path).ok();
    let modified = fs_meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .map(DateTime::<Utc>::from);
    let size = fs_meta.as_ref().map(|m| m.len()).unwrap_or(0);
    Some(CodexSessionFile {
        session_id: id,
        path,
        cwd,
        timestamp,
        modified,
        size,
    })
}

/// Walk only the codex `YYYY/MM/DD` layout, ignoring stray top-level files.
/// Anything else (a `.bak`, an aborted partial dir) is silently skipped so
/// `read_dir` errors on individual subtrees don't poison the whole scan.
fn walk_date_tree(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(year_dirs) = fs::read_dir(root) else {
        return;
    };
    for ye in year_dirs.flatten() {
        if !ye.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(month_dirs) = fs::read_dir(ye.path()) else {
            continue;
        };
        for me in month_dirs.flatten() {
            if !me.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let Ok(day_dirs) = fs::read_dir(me.path()) else {
                continue;
            };
            for de in day_dirs.flatten() {
                if !de.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let Ok(files) = fs::read_dir(de.path()) else {
                    continue;
                };
                for fe in files.flatten() {
                    let p = fe.path();
                    if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                        out.push(p);
                    }
                }
            }
        }
    }
}

struct ScanCache {
    sessions: Vec<CodexSessionFile>,
    populated_at: Instant,
}

static CACHE: OnceLock<Mutex<Option<ScanCache>>> = OnceLock::new();

fn cache_lock() -> &'static Mutex<Option<ScanCache>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Drop the cached scan. Web layer doesn't call this (5 s TTL covers all
/// realistic UI request patterns), but it's exposed for tests that mutate
/// the codex sessions tree mid-test.
pub fn invalidate_cache() {
    if let Ok(mut g) = cache_lock().lock() {
        *g = None;
    }
}

fn full_scan() -> Vec<CodexSessionFile> {
    let root = codex_sessions_root();
    let mut paths = Vec::new();
    walk_date_tree(&root, &mut paths);
    let mut sessions: Vec<CodexSessionFile> = paths
        .into_iter()
        .filter_map(build_session_file)
        .collect();
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
    sessions
}

fn scan_with_cache() -> Vec<CodexSessionFile> {
    let mut g = match cache_lock().lock() {
        Ok(g) => g,
        Err(_) => return full_scan(),
    };
    if let Some(c) = g.as_ref() {
        if c.populated_at.elapsed() < SCAN_CACHE_TTL {
            return c.sessions.clone();
        }
    }
    let sessions = full_scan();
    *g = Some(ScanCache {
        sessions: sessions.clone(),
        populated_at: Instant::now(),
    });
    sessions
}

/// Codex sessions whose `session_meta.cwd` matches the given path.
/// Newest first. Empty if codex isn't installed or no matching session has
/// been written yet.
pub fn discover_sessions_for_cwd(cwd: &str) -> Vec<CodexSessionFile> {
    let target = normalize_path_for_compare(cwd);
    scan_with_cache()
        .into_iter()
        .filter(|s| normalize_path_for_compare(&s.cwd) == target)
        .collect()
}

/// Find a single session by its codex thread id. Returns `None` if the
/// scan hasn't picked up that id yet (e.g. brand-new session that hasn't
/// flushed its meta line, or scan cache predates the session).
pub fn find_session_by_id(session_id: &str) -> Option<CodexSessionFile> {
    scan_with_cache()
        .into_iter()
        .find(|s| s.session_id == session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Both tests in this module mutate `CODEX_SESSIONS_DIR` and the global
    /// scan cache. Cargo runs tests in parallel by default, so without this
    /// lock test A would write its tree, set the env var, populate the cache;
    /// test B would set its own env var (now pointing at B's dir) but read
    /// A's cached scan. Serializing them is simpler than threading a custom
    /// scanner instance through the API.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn write_meta_line(dir: &Path, file: &str, id: &str, cwd: &str, ts: &str) -> PathBuf {
        let p = dir.join(file);
        let mut f = fs::File::create(&p).unwrap();
        // Build the JSON via serde so reverse-slashes in cwd ("E:\\foo")
        // get properly escaped to "E:\\\\foo" on disk — a hand-rolled
        // format!() loses that and produces invalid JSON.
        let line = serde_json::json!({
            "timestamp": ts,
            "type": "session_meta",
            "payload": {
                "id": id,
                "cwd": cwd,
                "timestamp": ts,
            }
        })
        .to_string();
        f.write_all(line.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
        // a couple of irrelevant follow-up lines to verify we only read one
        f.write_all(b"{\"type\":\"event_msg\",\"payload\":{}}\n")
            .unwrap();
        p
    }

    fn setup_tree() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let day = dir.path().join("2026").join("04").join("25");
        fs::create_dir_all(&day).unwrap();
        // SAFETY: tests run single-threaded for env mutation in this module;
        // each test must set CODEX_SESSIONS_DIR fresh + invalidate cache.
        unsafe {
            std::env::set_var("CODEX_SESSIONS_DIR", dir.path());
        }
        invalidate_cache();
        (dir, day)
    }

    #[test]
    fn finds_session_by_id_and_filters_by_cwd() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_keep, day) = setup_tree();
        write_meta_line(
            &day,
            "rollout-aaa.jsonl",
            "id-1",
            "E:\\aigc\\proj-a",
            "2026-04-25T10:00:00Z",
        );
        write_meta_line(
            &day,
            "rollout-bbb.jsonl",
            "id-2",
            "E:\\aigc\\proj-b",
            "2026-04-25T11:00:00Z",
        );
        invalidate_cache();

        let by_id = find_session_by_id("id-1").expect("id-1 found");
        assert_eq!(by_id.session_id, "id-1");

        // case-insensitive + slash-agnostic match
        let proj_a = discover_sessions_for_cwd("e:/aigc/proj-a");
        assert_eq!(proj_a.len(), 1);
        assert_eq!(proj_a[0].session_id, "id-1");

        let proj_b = discover_sessions_for_cwd("E:\\aigc\\proj-b");
        assert_eq!(proj_b.len(), 1);
        assert_eq!(proj_b[0].session_id, "id-2");
    }

    #[test]
    fn ignores_non_jsonl_and_non_meta_first_lines() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_keep, day) = setup_tree();
        // valid
        write_meta_line(
            &day,
            "rollout-ok.jsonl",
            "good",
            "E:\\proj",
            "2026-04-25T10:00:00Z",
        );
        // wrong extension — must be skipped
        fs::write(day.join("rollout-bad.txt"), "not jsonl").unwrap();
        // jsonl but first line isn't session_meta — must be skipped
        fs::write(
            day.join("rollout-noise.jsonl"),
            "{\"type\":\"event_msg\",\"payload\":{}}\n",
        )
        .unwrap();
        invalidate_cache();

        let all = scan_with_cache();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].session_id, "good");
    }

    #[test]
    fn normalize_handles_trailing_slash_and_case() {
        assert_eq!(
            normalize_path_for_compare("E:\\Foo\\Bar\\"),
            normalize_path_for_compare("e:/foo/bar")
        );
        assert_eq!(
            normalize_path_for_compare("/usr/local/"),
            normalize_path_for_compare("/usr/local")
        );
    }
}
