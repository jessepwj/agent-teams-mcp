use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::runtime::managed_member::ManagedMemberHandle;

/// Lightweight registry for runtime-managed sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRegistry {
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sessions: HashMap<String, ManagedMemberHandle>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_handle(&mut self, handle: ManagedMemberHandle) -> Option<ManagedMemberHandle> {
        self.sessions.insert(handle.member_id.clone(), handle)
    }

    pub fn remove_handle(&mut self, member_id: impl AsRef<str>) -> Option<ManagedMemberHandle> {
        self.sessions.remove(member_id.as_ref())
    }

    pub fn get_handle(&self, member_id: impl AsRef<str>) -> Option<&ManagedMemberHandle> {
        self.sessions.get(member_id.as_ref())
    }

    pub fn list_handles(&self) -> Vec<ManagedMemberHandle> {
        self.sessions.values().cloned().collect()
    }
}
