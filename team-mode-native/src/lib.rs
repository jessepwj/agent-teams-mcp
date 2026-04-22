pub mod adapters;
pub mod domain;
pub mod error;
pub mod host;
pub mod mcp;
pub mod runner;
pub mod service;
pub mod storage;
pub mod viewer;

pub use error::{Error, Result};
pub use host::TeamModeHost;
pub use service::TeamModeServices;
