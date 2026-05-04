use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;

pub(super) fn read_json_file(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content).map_err(|err| {
        io::Error::other(format!(
            "failed to parse JSON file '{}': {err}",
            path.display()
        ))
    })?)
}

pub(super) fn json_contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(s) => s.contains(needle),
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains_string(value, needle)),
        Value::Object(map) => map
            .values()
            .any(|value| json_contains_string(value, needle)),
        _ => false,
    }
}

pub(super) fn exit_with_error(err: Box<dyn std::error::Error>) -> ! {
    eprintln!("team_mode_service: {err}");
    std::process::exit(1)
}

pub(super) fn exit_with_hook_error(err: Box<dyn std::error::Error>) -> ! {
    eprintln!("team_mode_service hook: {err}");
    std::process::exit(1)
}

pub(super) fn exit_hook_error(code: i32, message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(code)
}
