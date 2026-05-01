//! Utility modules: atomic writes, file locking, ID generation, session discovery.

pub mod atomic_write;
pub mod codex_session_discovery;
pub mod file_lock;
pub mod id_gen;
pub mod session_discovery;

/// Validate that a name (team name, agent name, task ID) is safe for file path use.
pub fn validate_name(name: &str) -> crate::error::Result<()> {
    if name.is_empty() {
        return Err(crate::error::Error::InvalidName {
            name: name.to_string(),
            reason: "name cannot be empty".into(),
        });
    }
    if name.len() > 255 {
        return Err(crate::error::Error::InvalidName {
            name: name[..32].to_string(),
            reason: format!("name too long ({} bytes, max 255)", name.len()),
        });
    }
    if name.contains(['/', '\\', '\0']) {
        return Err(crate::error::Error::InvalidName {
            name: name.to_string(),
            reason: "name contains path separator or null byte".into(),
        });
    }
    if name == "." || name == ".." {
        return Err(crate::error::Error::InvalidName {
            name: name.to_string(),
            reason: "name cannot be '.' or '..'".into(),
        });
    }
    Ok(())
}

/// Stricter validation for user-facing identifiers (team name, worker name)
/// that must also be reachable via `@mention`. The mention parser only
/// recognizes `[A-Za-z0-9_\-.]`, so any character outside that set produces
/// a worker that exists on disk but cannot be addressed — confusing both
/// the lead AI and the user.
///
/// Rules:
/// - 1..=64 characters
/// - lowercase ASCII letters, digits, `_`, `-`, `.` only
/// - must start with a letter or digit (so `-foo` / `.bar` are rejected)
/// - cannot equal `.` or `..` (covered by length check + start rule, but
///   asserted explicitly for safety)
///
/// Lowercase is enforced so `@mention` matching can stay case-insensitive
/// without two workers `Bob` and `bob` colliding.
pub fn validate_slug_name(name: &str) -> crate::error::Result<()> {
    validate_name(name)?;
    if name.len() > 64 {
        return Err(crate::error::Error::InvalidName {
            name: name.to_string(),
            reason: format!(
                "name too long ({} bytes, max 64 for worker/team identifiers — \
                 they appear in @mentions and file paths)",
                name.len()
            ),
        });
    }
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty checked above");
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(crate::error::Error::InvalidName {
            name: name.to_string(),
            reason: format!(
                "name must start with a lowercase letter or digit (got '{first}'). \
                 Allowed characters: a-z 0-9 _ - ."
            ),
        });
    }
    let bad: Vec<char> = name
        .chars()
        .filter(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.')))
        .collect();
    if !bad.is_empty() {
        let preview: String = bad.iter().take(4).collect();
        return Err(crate::error::Error::InvalidName {
            name: name.to_string(),
            reason: format!(
                "name contains characters not allowed in @mentions: {preview:?}. \
                 Use lowercase letters, digits, '_', '-', or '.' only."
            ),
        });
    }
    Ok(())
}

/// Process names that are shell wrappers / launchers we should walk past
/// when locating the owning Claude Code (CC) process. Lowercased, no `.exe`
/// suffix — match against `process.name().to_lowercase().trim_end_matches(".exe")`.
///
/// Why: when `.mcp.json` invokes a wrapper script (e.g. `mcp-launcher.cmd`
/// to source vcvars64.bat on Windows), the MCP relay's direct parent is
/// `cmd.exe`, not the CC node process. Recording cmd.exe's PID as
/// `team.owner_cc_pid` causes:
///   - daemon's lead-watchdog to see that PID die first and self-terminate
///   - `lead-pending-wake.js` ancestor-chain routing to discard messages
///     because cmd.exe is not in the CC's ancestor chain (it's a sibling
///     branch under the same root)
///
/// We must walk past these wrappers to find the real CC.
const SHELL_WRAPPER_NAMES: &[&str] = &["cmd", "sh", "bash", "zsh", "pwsh", "powershell", "conhost"];

/// Maximum ancestors to walk when resolving the CC PID. A small bound
/// guards against pathological process trees (cycles shouldn't happen on
/// real OSes, but a stale parent slot pointing at a recycled PID could
/// loop us). Real CC ↔ MCP chains are 1–3 deep; 8 is generous.
const MAX_PARENT_WALK_DEPTH: u8 = 8;

/// Walk the parent chain starting from `start_pid`, skipping shell wrapper
/// processes, to find the owning CC process PID. The caller must have
/// refreshed the System view first (so `sys.process(...)` returns valid
/// data for ancestors).
///
/// Same algorithm as `current_cc_pid` but lets the caller supply the
/// starting PID and reuse a System view across multiple walks (e.g. the
/// MCP startup zombie sweep walks every peer's chain in one refresh).
pub fn resolve_cc_pid_from(start_pid: u32, sys: &sysinfo::System) -> Option<u32> {
    use sysinfo::Pid;

    let me = Pid::from_u32(start_pid);
    let mut current = sys.process(me)?.parent()?;

    for _ in 0..MAX_PARENT_WALK_DEPTH {
        let proc = match sys.process(current) {
            Some(p) => p,
            // Parent vanished mid-walk — return what we have so far. Don't
            // fall through to None, because the most-recently-walked PID
            // is still our best estimate of "real CC", just possibly
            // through a wrapper. Trade a slightly-wrong PID for a usable
            // owner binding.
            None => return Some(current.as_u32()),
        };
        let name_lc = proc.name().to_string_lossy().to_lowercase();
        let stem = name_lc.trim_end_matches(".exe");
        if !SHELL_WRAPPER_NAMES.contains(&stem) {
            return Some(current.as_u32());
        }
        current = match proc.parent() {
            Some(p) => p,
            None => return Some(current.as_u32()),
        };
    }

    // Walked the limit and still in wrappers — return whatever we ended on
    // rather than None so the team isn't left unbound. This is a defensive
    // fallback; real chains never need this many hops.
    Some(current.as_u32())
}

/// Return the PID of the Claude Code process that owns this MCP relay,
/// walking past shell wrappers (cmd / bash / pwsh / etc) that may sit
/// between us and the real CC.
///
/// Used by `team_create` to bind a team to its lead CC, and by the
/// daemon RPC layer to attach `owner_cc_pid` to every tool invocation
/// so push-routing can attribute pending messages to the right CC.
///
/// Returns `None` if the process tree query fails or no non-wrapper
/// ancestor is found within `MAX_PARENT_WALK_DEPTH` hops. Callers MUST
/// tolerate `None` — for legacy reasons, downstream code treats a missing
/// owner as "unbound" rather than erroring out.
pub fn current_cc_pid() -> Option<u32> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

    // Refresh ALL processes — we don't know up-front which ancestor PIDs
    // we'll need. The cost is one-time per call (callers cache the result
    // in `team.owner_cc_pid`), and the alternative (refresh-as-we-walk)
    // requires multiple syscalls plus complicated process-not-found
    // handling on Windows where PIDs can recycle quickly.
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());

    resolve_cc_pid_from(std::process::id(), &sys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_rejects_too_long() {
        let long_name = "a".repeat(256);
        let err = validate_name(&long_name).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("too long"), "expected 'too long' in: {msg}");
    }

    #[test]
    fn validate_name_accepts_max_length() {
        let name = "a".repeat(255);
        assert!(validate_name(&name).is_ok());
    }

    #[test]
    fn slug_accepts_canonical_examples() {
        for n in ["alice", "bob_2", "v1.2", "team-a", "x", "team-mode-test"] {
            assert!(
                validate_slug_name(n).is_ok(),
                "expected '{n}' to be a valid slug"
            );
        }
    }

    #[test]
    fn slug_rejects_uppercase() {
        let err = validate_slug_name("BOB").unwrap_err().to_string();
        assert!(err.contains("lowercase"), "got: {err}");
    }

    #[test]
    fn slug_rejects_spaces() {
        let err = validate_slug_name("has space").unwrap_err().to_string();
        assert!(err.contains("not allowed"), "got: {err}");
    }

    #[test]
    fn slug_rejects_unicode() {
        let err = validate_slug_name("中文名").unwrap_err().to_string();
        assert!(
            err.contains("not allowed") || err.contains("lowercase"),
            "got: {err}"
        );
    }

    #[test]
    fn slug_rejects_leading_punctuation() {
        for n in ["-foo", ".bar", "_baz"] {
            let err = validate_slug_name(n).unwrap_err().to_string();
            assert!(err.contains("must start with"), "for {n}: {err}");
        }
    }

    #[test]
    fn slug_rejects_too_long() {
        let n = "a".repeat(65);
        let err = validate_slug_name(&n).unwrap_err().to_string();
        assert!(err.contains("too long"), "got: {err}");
    }

    #[test]
    fn slug_accepts_max_length_64() {
        let n = "a".repeat(64);
        assert!(validate_slug_name(&n).is_ok());
    }
}
