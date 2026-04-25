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
        assert!(err.contains("not allowed") || err.contains("lowercase"), "got: {err}");
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
