use serde::{Deserialize, Serialize};

use crate::runtime::ExecutionSessionState;

/// Handle for a managed member session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedMemberHandle {
    pub member_id: String,
    pub member_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    pub session_state: ExecutionSessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}
