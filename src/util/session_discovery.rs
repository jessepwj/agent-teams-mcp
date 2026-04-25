//! Auto-discovery of Claude Code JSONL session files.
//!
//! Claude Code stores session logs at `~/.claude/projects/{encoded-path}/`
//! where path encoding replaces path separators and drive colons with `-`.
//! For example:
//! `/Users/alex/myproject` → `-Users-alex-myproject`
//!
//! This module auto-discovers matching session files, parses token usage,
//! and aggregates costs — eliminating the need for `--session-jsonl PATH`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::error::{Error, Result};
use crate::models::token::{
    AgentTokenUsage, CostSummary, MAX_TOOL_INPUT_SUMMARY_LEN, TokenUsage, ToolCallRecord,
    estimate_cost, truncate_string,
};

/// A discovered JSONL session file.
#[derive(Debug, Clone)]
pub struct SessionFile {
    /// Full path to the JSONL file.
    pub path: PathBuf,
    /// Session UUID (extracted from filename).
    pub session_id: String,
    /// File modification time.
    pub modified: Option<DateTime<Utc>>,
    /// File size in bytes.
    pub size: u64,
}

/// Encode a project path the way Claude Code does: replace path separators and
/// other non-ASCII filename-unsafe characters with `-`.
///
/// `/Users/alex/myproject` → `-Users-alex-myproject`
pub fn encode_project_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

/// Discover JSONL session files for a given repository path.
///
/// Looks in `~/.claude/projects/{encoded-path}/` for `*.jsonl` files.
/// Returns an empty vec if the directory doesn't exist or can't be read.
///
/// Windows quirk: `std::fs::canonicalize` returns a path with the `\\?\`
/// extended-length prefix (e.g. `\\?\E:\foo`). If we encode that directly
/// we get `---E--foo` which does NOT match what the `claude` CLI writes
/// (the CLI encodes from the non-prefixed form → `E--foo`). So on Windows
/// we strip the `\\?\` prefix before encoding. We also try a few path
/// variants as a fallback in case the CLI used a slightly different form
/// (e.g. called with short path vs. canonical path).
pub fn discover_sessions(repo_path: &Path) -> Vec<SessionFile> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let projects_root = home.join(".claude").join("projects");

    // Candidate paths to encode — try each until we find a matching dir.
    let canonical = repo_path.canonicalize().ok();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(ref c) = canonical {
        candidates.push(strip_windows_ext_prefix(c));
        candidates.push(c.clone());
    }
    candidates.push(repo_path.to_path_buf());

    for cand in &candidates {
        let encoded = encode_project_path(cand);
        let dir = projects_root.join(&encoded);
        let sessions = discover_sessions_in(&dir);
        if !sessions.is_empty() {
            return sessions;
        }
    }

    // No candidate matched. Return whatever the first candidate would have
    // yielded (usually empty) so callers still get the "not found" path
    // they're used to.
    let first = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| repo_path.to_path_buf());
    discover_sessions_in(&projects_root.join(encode_project_path(&first)))
}

/// Windows: strip the `\\?\` extended-length prefix that `canonicalize`
/// adds. Other platforms: return the path unchanged.
fn strip_windows_ext_prefix(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        // Handle both `\\?\C:\...` and `\\?\UNC\server\share\...` variants.
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{}", rest));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

/// Discover JSONL session files in a specific directory.
fn discover_sessions_in(dir: &Path) -> Vec<SessionFile> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if let Some(stem) = name.strip_suffix(".jsonl") {
            let metadata = entry.metadata().ok();
            let modified = metadata
                .as_ref()
                .and_then(|m| m.modified().ok().map(|t| DateTime::<Utc>::from(t)));
            let size = metadata.map(|m| m.len()).unwrap_or(0);

            sessions.push(SessionFile {
                path,
                session_id: stem.to_string(),
                modified,
                size,
            });
        }
    }

    // Sort by modification time (newest first)
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));

    sessions
}

/// Parse a Claude Code JSONL session file for token usage and tool calls.
///
/// This replicates the parsing logic from `CheckpointCollector::parse_jsonl_session`
/// but works independently of the checkpoint feature.
pub fn parse_session_file(path: &Path) -> Result<(Vec<ToolCallRecord>, Option<TokenUsage>)> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        Error::Other(format!(
            "Failed to read JSONL session file {}: {e}",
            path.display()
        ))
    })?;

    let mut tool_calls = Vec::new();
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut total_cache_read: u64 = 0;
    let mut total_cache_write: u64 = 0;
    let mut has_usage = false;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Extract tool calls (tool_name format)
        if let Some(tool_name) = value.get("tool_name").and_then(|v| v.as_str()) {
            let input_summary = value
                .get("tool_input")
                .map(|v| truncate_string(&v.to_string(), MAX_TOOL_INPUT_SUMMARY_LEN));

            let timestamp = value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));

            tool_calls.push(ToolCallRecord {
                tool_name: tool_name.to_string(),
                input_summary,
                timestamp,
            });
        }

        // Also check for "type": "tool_use" format (Claude API format)
        if value.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
            if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                let input_summary = value
                    .get("input")
                    .map(|v| truncate_string(&v.to_string(), MAX_TOOL_INPUT_SUMMARY_LEN));

                tool_calls.push(ToolCallRecord {
                    tool_name: name.to_string(),
                    input_summary,
                    timestamp: None,
                });
            }
        }

        // Extract token usage
        if let Some(usage) = value.get("usage") {
            has_usage = true;
            if let Some(n) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                total_input_tokens += n;
            }
            if let Some(n) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
                total_output_tokens += n;
            }
            if let Some(n) = usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
            {
                total_cache_read += n;
            }
            if let Some(n) = usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
            {
                total_cache_write += n;
            }
        }
    }

    let token_usage = if has_usage {
        Some(TokenUsage {
            input_tokens: total_input_tokens,
            output_tokens: total_output_tokens,
            cache_read_tokens: if total_cache_read > 0 {
                Some(total_cache_read)
            } else {
                None
            },
            cache_write_tokens: if total_cache_write > 0 {
                Some(total_cache_write)
            } else {
                None
            },
        })
    } else {
        None
    };

    Ok((tool_calls, token_usage))
}

/// Parse only the token usage from a JSONL session file (ignores tool calls).
pub fn parse_token_usage(path: &Path) -> Result<Option<TokenUsage>> {
    let (_, usage) = parse_session_file(path)?;
    Ok(usage)
}

/// Aggregate costs across multiple session files.
///
/// If `agent_name` is provided, all usage is attributed to that agent.
/// Otherwise, tries to extract agent names from session files.
pub fn aggregate_cost(sessions: &[SessionFile], agent_name: Option<&str>) -> Result<CostSummary> {
    let mut total_usage = TokenUsage {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: None,
        cache_write_tokens: None,
    };
    let mut per_agent: Vec<AgentTokenUsage> = Vec::new();
    let mut session_count = 0;

    for session in sessions {
        match parse_token_usage(&session.path)? {
            Some(usage) => {
                total_usage.merge(&usage);
                session_count += 1;

                let name = agent_name
                    .map(String::from)
                    .unwrap_or_else(|| extract_agent_name(&session.path));

                // Merge into per_agent
                if let Some(existing) = per_agent.iter_mut().find(|a| a.agent_name == name) {
                    existing.usage.merge(&usage);
                } else {
                    per_agent.push(AgentTokenUsage {
                        agent_name: name,
                        usage,
                    });
                }
            }
            None => continue,
        }
    }

    let estimated_cost_usd = estimate_cost(&total_usage);

    Ok(CostSummary {
        total_usage,
        session_count,
        per_agent,
        estimated_cost_usd,
    })
}

/// Try to extract agent name from JSONL file by scanning for agent-related fields.
fn extract_agent_name(path: &Path) -> String {
    if let Ok(content) = std::fs::read_to_string(path) {
        // Check first few lines for an agent name hint
        for line in content.lines().take(20) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                // Claude Code session files sometimes have a "slug" or "agentId" field
                if let Some(name) = value.get("agentId").and_then(|v| v.as_str()) {
                    return name.to_string();
                }
                if let Some(slug) = value.get("slug").and_then(|v| v.as_str()) {
                    return slug.to_string();
                }
            }
        }
    }
    // Fall back to session ID from filename
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn encode_project_path_basic() {
        let path = Path::new("/Users/alex/myproject");
        assert_eq!(encode_project_path(path), "-Users-alex-myproject");
    }

    #[test]
    fn encode_project_path_root() {
        let path = Path::new("/");
        assert_eq!(encode_project_path(path), "-");
    }

    #[test]
    fn encode_project_path_windows_style() {
        let path = Path::new(r"C:\Users\msi\.codex\ascii-workspaces\afc10e97887a");
        assert_eq!(
            encode_project_path(path),
            "C--Users-msi--codex-ascii-workspaces-afc10e97887a"
        );
    }

    #[test]
    fn encode_project_path_replaces_non_ascii_segments() {
        let path = Path::new("/mnt/e/aigc内容整理/opencode");
        assert_eq!(encode_project_path(path), "-mnt-e-aigc-----opencode");
    }

    #[test]
    fn discover_sessions_empty_dir() {
        let dir = tempdir().unwrap();
        let sessions = discover_sessions_in(dir.path());
        assert!(sessions.is_empty());
    }

    #[test]
    fn discover_sessions_with_jsonl_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("abc-123.jsonl"), "{}").unwrap();
        std::fs::write(dir.path().join("def-456.jsonl"), "{}").unwrap();
        std::fs::write(dir.path().join("not-session.json"), "{}").unwrap();

        let sessions = discover_sessions_in(dir.path());
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|s| s.session_id == "abc-123"));
        assert!(sessions.iter().any(|s| s.session_id == "def-456"));
    }

    #[test]
    fn parse_session_file_basic() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            r#"{"tool_name":"Read","tool_input":{"path":"src/main.rs"},"timestamp":"2025-01-01T00:00:00Z"}
{"usage":{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":200}}
{"invalid json
{"usage":{"input_tokens":2000,"output_tokens":300}}
"#,
        )
        .unwrap();

        let (tool_calls, token_usage) = parse_session_file(&path).unwrap();

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].tool_name, "Read");

        let usage = token_usage.unwrap();
        assert_eq!(usage.input_tokens, 3000);
        assert_eq!(usage.output_tokens, 800);
        assert_eq!(usage.cache_read_tokens, Some(200));
    }

    #[test]
    fn aggregate_cost_multiple_sessions() {
        let dir = tempdir().unwrap();

        let path1 = dir.path().join("session1.jsonl");
        std::fs::write(
            &path1,
            r#"{"usage":{"input_tokens":1000,"output_tokens":500}}
"#,
        )
        .unwrap();

        let path2 = dir.path().join("session2.jsonl");
        std::fs::write(
            &path2,
            r#"{"usage":{"input_tokens":2000,"output_tokens":300}}
"#,
        )
        .unwrap();

        let sessions = vec![
            SessionFile {
                path: path1,
                session_id: "s1".into(),
                modified: None,
                size: 0,
            },
            SessionFile {
                path: path2,
                session_id: "s2".into(),
                modified: None,
                size: 0,
            },
        ];

        let cost = aggregate_cost(&sessions, Some("test-agent")).unwrap();
        assert_eq!(cost.session_count, 2);
        assert_eq!(cost.total_usage.input_tokens, 3000);
        assert_eq!(cost.total_usage.output_tokens, 800);
        assert_eq!(cost.per_agent.len(), 1);
        assert_eq!(cost.per_agent[0].agent_name, "test-agent");
        assert!(cost.estimated_cost_usd > 0.0);
    }

    #[test]
    fn aggregate_cost_empty_sessions() {
        let cost = aggregate_cost(&[], None).unwrap();
        assert_eq!(cost.session_count, 0);
        assert_eq!(cost.total_usage.input_tokens, 0);
        assert_eq!(cost.estimated_cost_usd, 0.0);
    }
}
