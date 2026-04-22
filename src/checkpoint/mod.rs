//! Checkpoint system for attaching agent session context to Git commits.
//!
//! Uses Git notes (`refs/notes/agent-checkpoints`) to store structured
//! JSON metadata alongside commits, creating a browsable audit trail
//! of human-machine collaboration.
//!
//! # Architecture
//!
//! ```text
//! Data Sources → CheckpointCollector → Checkpoint struct → CheckpointStore → Git Notes
//!                 ├── git2 (HEAD, branch, diff)
//!                 ├── ~/.claude/teams/{team}/config.json
//!                 ├── ~/.claude/tasks/{team}/*.json
//!                 └── ~/.claude/projects/{hash}/{session}.jsonl (optional)
//! ```
//!
//! # Feature Flag
//!
//! This module requires the `checkpoint` feature:
//! ```toml
//! agent-teams = { version = "0.1", features = ["checkpoint"] }
//! ```

pub mod auto_trigger;
pub mod collector;
pub mod query;
pub mod storage;

pub use auto_trigger::AutoCheckpointTrigger;
pub use collector::CheckpointCollector;
pub use query::{CheckpointDiff, CheckpointFilter, CheckpointQuery};
pub use storage::{CheckpointStore, NOTES_REF};
