//! Execution-layer runtime for managed members.
//!
//! This layer is intentionally separate from the Team Mode protocol layer.

pub mod agent_loop;
pub mod managed_member;
pub mod orchestrator;
pub mod session_registry;
pub mod session_state;

pub use agent_loop::{AgentLoop, AgentLoopHandle};
pub use managed_member::*;
pub use orchestrator::*;
pub use session_registry::*;
pub use session_state::*;
