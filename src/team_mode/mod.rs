//! Team Mode domain and future MCP-facing abstractions.
//!
//! Phase 0/1 keeps this layer intentionally small: we define the core
//! transcript-first data shapes first, while the old workflow-oriented
//! modules remain available during the transition.

pub mod data_dir;
pub mod domain;
pub mod mcp;
pub mod runtime_workers;
pub mod service;
pub mod storage;
#[cfg(test)]
mod tracing_capture;

pub use domain::*;
pub use mcp::*;
pub use runtime_workers::*;
pub use service::*;
pub use storage::*;
