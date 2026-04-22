use std::path::{Path, PathBuf};

use serde_json::Value;

pub fn codex_event_log_path(data_dir: impl AsRef<Path>, member_id: &str) -> PathBuf {
    data_dir
        .as_ref()
        .join("members")
        .join(member_id)
        .join("codex-events.ndjson")
}

pub fn render_codex_event_line(line: &str) -> String {
    match serde_json::from_str::<Value>(line) {
        Ok(value) => {
            let event = value
                .get("event")
                .or_else(|| value.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("codex_event");
            let text = value
                .get("text")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str);
            match text {
                Some(text) if !text.is_empty() => format!("[{event}] {text}"),
                _ => format!("[{event}] {value}"),
            }
        }
        Err(_) => line.to_string(),
    }
}
