//! Utility modules: atomic writes, file locking, ID generation, session discovery.

pub mod atomic_write;
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
}
