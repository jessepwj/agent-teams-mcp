//! Git notes-based storage for checkpoints.
//!
//! Each checkpoint is stored as a Git note attached to a commit
//! under the `refs/notes/agent-checkpoints` namespace. This keeps
//! metadata alongside the code without modifying commit SHAs.

use std::path::Path;

use git2::{Oid, Repository};

use crate::error::{Error, Result};
use crate::models::checkpoint::Checkpoint;

/// Git notes ref namespace for agent checkpoints.
pub const NOTES_REF: &str = "refs/notes/agent-checkpoints";

/// Storage backend for checkpoints using Git notes.
pub struct CheckpointStore {
    repo: Repository,
}

impl CheckpointStore {
    /// Open a checkpoint store for the repository at `repo_path`.
    pub fn open(repo_path: &Path) -> Result<Self> {
        let repo = Repository::discover(repo_path).map_err(|e| {
            if e.code() == git2::ErrorCode::NotFound {
                Error::NotAGitRepo {
                    path: repo_path.to_path_buf(),
                }
            } else {
                Error::Git(e)
            }
        })?;
        Ok(Self { repo })
    }

    /// Get a reference to the underlying git2 Repository.
    pub fn repo(&self) -> &Repository {
        &self.repo
    }

    /// Save a checkpoint as a Git note on the given commit.
    ///
    /// If a note already exists for this commit in the checkpoint namespace,
    /// it will be overwritten (force = true).
    pub fn save(&self, checkpoint: &Checkpoint) -> Result<()> {
        let oid = self.resolve_oid(&checkpoint.commit_sha)?;
        let json = serde_json::to_string_pretty(checkpoint)?;
        let sig = self.default_signature()?;

        self.repo
            .note(&sig, &sig, Some(NOTES_REF), oid, &json, true)?;

        Ok(())
    }

    /// Load a checkpoint from the Git note on a commit.
    pub fn load(&self, commit_sha: &str) -> Result<Checkpoint> {
        let oid = self.resolve_oid(commit_sha)?;
        let note = self.repo.find_note(Some(NOTES_REF), oid).map_err(|e| {
            if e.code() == git2::ErrorCode::NotFound {
                Error::CheckpointNotFound {
                    sha: commit_sha.to_string(),
                }
            } else {
                Error::Git(e)
            }
        })?;

        let message = note.message().ok_or_else(|| Error::CheckpointError {
            reason: format!("Note for {commit_sha} contains non-UTF-8 data"),
        })?;

        let checkpoint: Checkpoint = serde_json::from_str(message)?;
        Ok(checkpoint)
    }

    /// List all checkpoints in the repository.
    ///
    /// Returns `(commit_oid, checkpoint)` pairs in no particular order.
    pub fn list(&self) -> Result<Vec<(String, Checkpoint)>> {
        let mut results = Vec::new();

        // Try to iterate notes; if the notes ref doesn't exist yet, return empty
        let notes = match self.repo.notes(Some(NOTES_REF)) {
            Ok(notes) => notes,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(results),
            Err(e) => return Err(Error::Git(e)),
        };

        for note_result in notes {
            let (note_oid, commit_oid) = note_result?;
            let note_blob = self.repo.find_blob(note_oid)?;
            let content =
                std::str::from_utf8(note_blob.content()).map_err(|_| Error::CheckpointError {
                    reason: format!("Non-UTF-8 note blob for commit {commit_oid}"),
                })?;

            match serde_json::from_str::<Checkpoint>(content) {
                Ok(checkpoint) => {
                    results.push((commit_oid.to_string(), checkpoint));
                }
                Err(e) => {
                    tracing::warn!(
                        commit = %commit_oid,
                        error = %e,
                        "Skipping unparseable checkpoint note"
                    );
                }
            }
        }

        Ok(results)
    }

    /// Delete a checkpoint note from a commit.
    pub fn delete(&self, commit_sha: &str) -> Result<()> {
        let oid = self.resolve_oid(commit_sha)?;
        let sig = self.default_signature()?;

        self.repo
            .note_delete(oid, Some(NOTES_REF), &sig, &sig)
            .map_err(|e| {
                if e.code() == git2::ErrorCode::NotFound {
                    Error::CheckpointNotFound {
                        sha: commit_sha.to_string(),
                    }
                } else {
                    Error::Git(e)
                }
            })?;

        Ok(())
    }

    /// Check whether a checkpoint exists for a given commit.
    pub fn exists(&self, commit_sha: &str) -> Result<bool> {
        let oid = self.resolve_oid(commit_sha)?;
        match self.repo.find_note(Some(NOTES_REF), oid) {
            Ok(_) => Ok(true),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(false),
            Err(e) => Err(Error::Git(e)),
        }
    }

    /// Resolve a commit reference (SHA, HEAD, branch name, etc.) to an OID.
    fn resolve_oid(&self, spec: &str) -> Result<Oid> {
        let obj = self.repo.revparse_single(spec).map_err(|e| {
            if e.code() == git2::ErrorCode::NotFound {
                Error::CheckpointNotFound {
                    sha: spec.to_string(),
                }
            } else {
                Error::Git(e)
            }
        })?;
        // Peel to commit to ensure we have a valid commit
        let commit = obj.peel_to_commit().map_err(|_| Error::CheckpointError {
            reason: format!("'{spec}' does not resolve to a commit"),
        })?;
        Ok(commit.id())
    }

    /// Get or create a default signature for note operations.
    fn default_signature(&self) -> Result<git2::Signature<'_>> {
        // Try repo config first, fall back to a default
        self.repo.signature().or_else(|_| {
            git2::Signature::now("agent-teams", "agent-teams@checkpoint").map_err(|e| Error::Git(e))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::checkpoint::{CheckpointFile, CheckpointSession, FileRole};
    use tempfile::TempDir;

    /// Create a test git repo with an initial commit.
    fn setup_test_repo() -> (TempDir, Repository) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        // Create an initial commit (scoped so tree borrow is dropped before return)
        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = {
                let mut index = repo.index().unwrap();
                std::fs::write(dir.path().join("README.md"), "# Test").unwrap();
                index.add_path(Path::new("README.md")).unwrap();
                index.write().unwrap();
                index.write_tree().unwrap()
            };
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
                .unwrap();
        }

        (dir, repo)
    }

    #[test]
    fn save_and_load_checkpoint() {
        let (dir, _repo) = setup_test_repo();
        let store = CheckpointStore::open(dir.path()).unwrap();

        let session = CheckpointSession::new("test-agent");
        let mut checkpoint = Checkpoint::new("HEAD", "main", session);
        checkpoint.files.push(CheckpointFile {
            path: "README.md".into(),
            role: FileRole::Modified,
            content_hash: None,
        });

        // Resolve HEAD to actual SHA for later load
        let head_sha = {
            let obj = store.repo.revparse_single("HEAD").unwrap();
            obj.id().to_string()
        };
        checkpoint.commit_sha = head_sha.clone();

        store.save(&checkpoint).unwrap();

        // Load back
        let loaded = store.load(&head_sha).unwrap();
        assert_eq!(loaded.session.agent_name, "test-agent");
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[0].path, "README.md");
    }

    #[test]
    fn list_checkpoints() {
        let (dir, repo) = setup_test_repo();
        let store = CheckpointStore::open(dir.path()).unwrap();

        // Empty list initially
        let list = store.list().unwrap();
        assert!(list.is_empty());

        // Save a checkpoint
        let head_sha = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();
        let session = CheckpointSession::new("agent-1");
        let mut ckpt = Checkpoint::new(&head_sha, "main", session);
        ckpt.commit_sha = head_sha.clone();
        store.save(&ckpt).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1.session.agent_name, "agent-1");
    }

    #[test]
    fn delete_checkpoint() {
        let (dir, repo) = setup_test_repo();
        let store = CheckpointStore::open(dir.path()).unwrap();

        let head_sha = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();
        let session = CheckpointSession::new("agent-1");
        let mut ckpt = Checkpoint::new(&head_sha, "main", session);
        ckpt.commit_sha = head_sha.clone();
        store.save(&ckpt).unwrap();

        assert!(store.exists(&head_sha).unwrap());
        store.delete(&head_sha).unwrap();
        assert!(!store.exists(&head_sha).unwrap());
    }

    #[test]
    fn overwrite_checkpoint() {
        let (dir, repo) = setup_test_repo();
        let store = CheckpointStore::open(dir.path()).unwrap();

        let head_sha = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();

        // Save first version
        let session1 = CheckpointSession::new("agent-v1");
        let mut ckpt1 = Checkpoint::new(&head_sha, "main", session1);
        ckpt1.commit_sha = head_sha.clone();
        store.save(&ckpt1).unwrap();

        // Overwrite with second version
        let session2 = CheckpointSession::new("agent-v2");
        let mut ckpt2 = Checkpoint::new(&head_sha, "main", session2);
        ckpt2.commit_sha = head_sha.clone();
        store.save(&ckpt2).unwrap();

        let loaded = store.load(&head_sha).unwrap();
        assert_eq!(loaded.session.agent_name, "agent-v2");
    }

    #[test]
    fn load_nonexistent_checkpoint() {
        let (dir, repo) = setup_test_repo();
        let store = CheckpointStore::open(dir.path()).unwrap();

        let head_sha = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();
        let result = store.load(&head_sha);
        assert!(result.is_err());
    }

    #[test]
    fn not_a_git_repo() {
        let dir = TempDir::new().unwrap();
        let result = CheckpointStore::open(dir.path());
        assert!(matches!(result, Err(Error::NotAGitRepo { .. })));
    }
}
