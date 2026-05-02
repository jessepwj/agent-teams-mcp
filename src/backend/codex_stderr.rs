use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;

use tracing::warn;

pub(crate) const CODEX_STDERR_LOG_ENV: &str = "TEAM_MODE_CODEX_STDERR_LOG";
pub(crate) const STDERR_RING_BUFFER_SIZE: usize = 4096;
pub(crate) const STDERR_TAIL_LINES: usize = 50;

#[derive(Debug, Clone)]
struct CodexStderrLine {
    sequence: u64,
    timestamp: String,
    line: String,
}

#[derive(Debug, Default)]
pub(crate) struct CodexStderrRing {
    lines: VecDeque<CodexStderrLine>,
    next_sequence: u64,
    log_path: Option<PathBuf>,
}

impl CodexStderrRing {
    pub(crate) fn new(log_path: Option<PathBuf>) -> Self {
        Self {
            lines: VecDeque::with_capacity(STDERR_RING_BUFFER_SIZE),
            next_sequence: 1,
            log_path,
        }
    }

    pub(crate) fn push_line(&mut self, timestamp: String, line: String) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        if self.lines.len() == STDERR_RING_BUFFER_SIZE {
            self.lines.pop_front();
        }
        self.lines.push_back(CodexStderrLine {
            sequence,
            timestamp,
            line,
        });
        sequence
    }

    pub(crate) fn snapshot_tail(&self, tail_lines: usize) -> Option<String> {
        if self.lines.is_empty() {
            return None;
        }
        let start = self.lines.len().saturating_sub(tail_lines);
        Some(
            self.lines
                .iter()
                .skip(start)
                .map(|entry| format!("[{} #{}] {}", entry.timestamp, entry.sequence, entry.line))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    pub(crate) fn log_hint(&self, tail_lines: usize) -> Option<String> {
        let path = self.log_path.as_ref()?;
        let last = self.lines.back()?;
        let first = self
            .lines
            .iter()
            .rev()
            .take(tail_lines)
            .next_back()
            .map(|line| line.sequence)
            .unwrap_or(last.sequence);
        Some(format!(
            "{}:lines {first}-{}",
            path.display(),
            last.sequence
        ))
    }
}

pub(crate) fn open_stderr_log(path: Option<&PathBuf>, agent_name: &str) -> Option<File> {
    let path = path?;
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        warn!(
            event = "codex_worker.stderr_log_open_failed",
            worker_name = %agent_name,
            path = %path.display(),
            error = %err,
            "failed to create codex stderr log directory"
        );
        return None;
    }
    match OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
    {
        Ok(file) => Some(file),
        Err(err) => {
            warn!(
                event = "codex_worker.stderr_log_open_failed",
                worker_name = %agent_name,
                path = %path.display(),
                error = %err,
                "failed to open codex stderr log"
            );
            None
        }
    }
}

pub(crate) fn redact_stderr_line(line: &str) -> String {
    let mut out = line.to_string();
    out = redact_authorization_header(&out);
    out = redact_token_prefixes(&out, &["sk-", "sess-", "Bearer "]);
    out = redact_key_values(
        &out,
        &[
            "api_key=",
            "apikey=",
            "authorization=",
            "access_token=",
            "refresh_token=",
            "token=",
            "password=",
        ],
    );
    out
}

fn redact_authorization_header(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let Some(index) = lower.find("authorization:") else {
        return line.to_string();
    };
    let end = line[index..]
        .find(['\n', '\r'])
        .map(|offset| index + offset)
        .unwrap_or(line.len());
    format!(
        "{}Authorization: <redacted>{}",
        &line[..index],
        &line[end..]
    )
}

fn redact_token_prefixes(line: &str, prefixes: &[&str]) -> String {
    let mut out = line.to_string();
    for prefix in prefixes {
        let mut search_from = 0;
        while let Some(relative) = out[search_from..].find(prefix) {
            let start = search_from + relative + prefix.len();
            let end = out[start..]
                .find(|ch: char| {
                    ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | ')' | ']' | '}')
                })
                .map(|offset| start + offset)
                .unwrap_or(out.len());
            if end > start {
                out.replace_range(start..end, "<redacted>");
            }
            search_from = start + "<redacted>".len();
        }
    }
    out
}

fn redact_key_values(line: &str, keys: &[&str]) -> String {
    let mut out = line.to_string();
    for key in keys {
        let mut search_from = 0;
        loop {
            let lower = out[search_from..].to_ascii_lowercase();
            let Some(relative) = lower.find(key) else {
                break;
            };
            let value_start = search_from + relative + key.len();
            let value_end = out[value_start..]
                .find(|ch: char| {
                    ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | ')' | ']' | '}')
                })
                .map(|offset| value_start + offset)
                .unwrap_or(out.len());
            if value_end > value_start {
                out.replace_range(value_start..value_end, "<redacted>");
            }
            search_from = value_start + "<redacted>".len();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn stderr_ring_buffer_keeps_most_recent_lines() {
        let mut ring = CodexStderrRing::new(Some(PathBuf::from("codex-stderr-worker.log")));
        for idx in 0..(STDERR_RING_BUFFER_SIZE + 3) {
            ring.push_line(format!("2026-05-02T00:00:{idx:02}Z"), format!("line-{idx}"));
        }

        assert_eq!(ring.lines.len(), STDERR_RING_BUFFER_SIZE);
        let tail = ring.snapshot_tail(STDERR_TAIL_LINES).unwrap();
        assert!(
            tail.contains("line-4098"),
            "tail did not include latest line: {tail}"
        );
        assert!(
            !tail.contains("line-0"),
            "tail unexpectedly retained oldest line: {tail}"
        );
        assert_eq!(
            ring.log_hint(2).unwrap(),
            "codex-stderr-worker.log:lines 4098-4099"
        );
    }

    #[test]
    fn stderr_tracing_redaction_masks_common_secret_shapes() {
        let redacted = redact_stderr_line(
            "Authorization: Bearer sk-live-secret api_key=abc123 token=xyz password=pw",
        );

        assert!(!redacted.contains("sk-live-secret"));
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("token=xyz"));
        assert!(!redacted.contains("password=pw"));
        assert!(redacted.contains("Authorization: <redacted>"));
    }
}
