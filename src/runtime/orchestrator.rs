use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc::Receiver;
use uuid::Uuid;

use crate::backend::{AgentBackend, AgentOutput, AgentSession, BackendType, SpawnConfig};
use crate::models::SessionState;
use crate::runtime::ExecutionSessionState;
use crate::runtime::managed_member::ManagedMemberHandle;
use crate::runtime::session_registry::SessionRegistry;
use crate::{Error, Result};

struct ManagedSession {
    session_key: String,
    session_state: SessionState,
    runtime_state: ExecutionSessionState,
    session: Box<dyn AgentSession>,
    output_receiver: Option<Receiver<AgentOutput>>,
    last_error: Option<String>,
}

impl ManagedSession {
    fn new(
        session_key: String,
        session_state: SessionState,
        runtime_state: ExecutionSessionState,
        session: Box<dyn AgentSession>,
        output_receiver: Option<Receiver<AgentOutput>>,
    ) -> Self {
        Self {
            session_key,
            session_state,
            runtime_state,
            session,
            output_receiver,
            last_error: None,
        }
    }

    fn handle(&self, member_id: &str, member_name: &str) -> ManagedMemberHandle {
        ManagedMemberHandle {
            member_id: member_id.to_string(),
            member_name: member_name.to_string(),
            session_key: Some(self.session_key.clone()),
            session_state: self.runtime_state,
            last_error: self.last_error.clone(),
        }
    }

    fn set_runtime_state(&mut self, state: ExecutionSessionState) {
        self.runtime_state = state;
        self.session_state
            .metadata
            .insert("runtime_state".into(), state.as_str().into());
    }
}

/// Runtime-side orchestrator for managed member lifecycle.
pub struct RuntimeOrchestrator {
    backends: HashMap<BackendType, Arc<dyn AgentBackend>>,
    sessions: HashMap<String, ManagedSession>,
    session_registry: SessionRegistry,
}

impl Default for RuntimeOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RuntimeOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeOrchestrator")
            .field("backend_count", &self.backends.len())
            .field("session_count", &self.sessions.len())
            .field("session_registry", &self.session_registry)
            .finish()
    }
}

impl RuntimeOrchestrator {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
            sessions: HashMap::new(),
            session_registry: SessionRegistry::new(),
        }
    }

    pub fn with_backend<B>(mut self, backend: B) -> Self
    where
        B: AgentBackend + 'static,
    {
        self.register_backend(backend);
        self
    }

    pub fn register_backend<B>(&mut self, backend: B) -> &mut Self
    where
        B: AgentBackend + 'static,
    {
        self.backends
            .insert(backend.backend_type(), Arc::new(backend));
        self
    }

    pub fn session_registry(&self) -> &SessionRegistry {
        &self.session_registry
    }

    pub fn session_registry_mut(&mut self) -> &mut SessionRegistry {
        &mut self.session_registry
    }

    pub async fn spawn_managed_member(
        &mut self,
        member_id: impl Into<String>,
        member_name: impl Into<String>,
        mut config: SpawnConfig,
        backend_type: BackendType,
    ) -> Result<ManagedMemberHandle> {
        let member_id = member_id.into();
        let member_name = member_name.into();

        tracing::info!(member_id = %member_id, backend = ?backend_type, "spawning managed member session");

        if self.sessions.contains_key(&member_id) {
            return Err(Error::Other(format!(
                "Managed member '{member_id}' is already registered"
            )));
        }

        let backend = self.backends.get(&backend_type).cloned().ok_or_else(|| {
            Error::BackendNotConfigured {
                backend: backend_type.to_string(),
            }
        })?;

        config.name = member_name.clone();
        let mut session_state = SessionState::from_config(&config, &backend_type);
        let session_key = session_state
            .metadata
            .get("session_key")
            .cloned()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        session_state
            .metadata
            .insert("session_key".into(), session_key.clone());
        session_state.metadata.insert(
            "runtime_state".into(),
            ExecutionSessionState::Starting.as_str().into(),
        );

        let mut session = backend
            .spawn(config)
            .await
            .map_err(|e| Error::SpawnFailed {
                name: member_name.clone(),
                reason: e.to_string(),
            })?;

        let output_receiver = session.output_receiver();
        let mut managed = ManagedSession::new(
            session_key,
            session_state,
            ExecutionSessionState::Running,
            session,
            output_receiver,
        );
        managed.set_runtime_state(ExecutionSessionState::Running);

        let handle = managed.handle(&member_id, &member_name);
        self.session_registry.register_handle(handle.clone());
        self.sessions.insert(member_id.clone(), managed);
        tracing::info!(member_id = %member_id, "managed member session ready");
        Ok(handle)
    }

    pub async fn resume_managed_member(
        &mut self,
        member_id: impl Into<String>,
        member_name: impl Into<String>,
        mut persisted_state: SessionState,
    ) -> Result<ManagedMemberHandle> {
        let member_id = member_id.into();
        let member_name = member_name.into();
        let backend_type = persisted_state.parse_backend_type().ok_or_else(|| {
            Error::Other(format!(
                "Persisted session for '{member_id}' has invalid backend_type '{}'",
                persisted_state.backend_type
            ))
        })?;

        persisted_state.name = member_name.clone();
        let config = persisted_state.to_spawn_config();
        self.spawn_managed_member(member_id, member_name, config, backend_type)
            .await
    }

    pub async fn send_input(&mut self, member_id: impl AsRef<str>, input: &str) -> Result<()> {
        let member_id = member_id.as_ref().to_string();
        tracing::debug!(member_id = %member_id, input_len = input.len(), "sending input to session");
        let handle = {
            let managed = self.sessions.get_mut(&member_id).ok_or_else(|| {
                Error::Other(format!(
                    "no managed session registered for spawn_key '{member_id}' \
                     (worker may have died or was never spawned)"
                ))
            })?;

            managed.set_runtime_state(ExecutionSessionState::Running);
            if let Err(err) = managed.session.send_input(input).await {
                managed.last_error = Some(err.to_string());
                managed.set_runtime_state(ExecutionSessionState::Failed);
                let handle = managed.handle(&member_id, &managed.session_state.name);
                self.session_registry.register_handle(handle);
                return Err(err);
            }

            managed.last_error = None;
            managed.handle(&member_id, &managed.session_state.name)
        };

        self.session_registry.register_handle(handle);
        Ok(())
    }

    pub fn take_output_receiver(
        &mut self,
        member_id: impl AsRef<str>,
    ) -> Result<Option<Receiver<AgentOutput>>> {
        let member_id = member_id.as_ref().to_string();
        let managed = self.sessions.get_mut(&member_id).ok_or_else(|| {
            Error::Other(format!(
                "no managed session registered for spawn_key '{member_id}'"
            ))
        })?;
        Ok(managed.output_receiver.take())
    }

    pub async fn is_alive(&self, member_id: impl AsRef<str>) -> Result<bool> {
        let member_id = member_id.as_ref();
        let managed = self.sessions.get(member_id).ok_or_else(|| {
            Error::Other(format!(
                "no managed session registered for spawn_key '{member_id}'"
            ))
        })?;
        Ok(managed.session.is_alive().await)
    }

    /// Whether the orchestrator currently has a registered session entry
    /// for `member_id`, regardless of whether the underlying child process
    /// is alive. Useful when the caller wants to decide whether re-spawn
    /// is allowed without taking the (mutable) lock for `is_alive`.
    pub fn has_session(&self, member_id: impl AsRef<str>) -> bool {
        self.sessions.contains_key(member_id.as_ref())
    }

    /// Drop a session entry whose child process has already died, returning
    /// `true` if a stale entry was removed. Does NOT shut down the session
    /// (that is for the live-process path); the assumption is the OS
    /// already reaped the child. This unblocks `worker_add on_existing=reuse`
    /// for a worker that died externally — without this, `spawn_managed_member`
    /// refuses to recreate the spawn_key because a stale `ManagedSession`
    /// occupies the slot.
    ///
    /// Safe to call even when the session is still live: it will check
    /// `session.is_alive()` and refuse to drop a live session, returning
    /// `false`. Callers that mean "force kill" should use
    /// `shutdown_managed_member` instead.
    pub async fn remove_dead_session_if_any(&mut self, member_id: impl AsRef<str>) -> bool {
        let member_id = member_id.as_ref().to_string();
        let Some(managed) = self.sessions.get(&member_id) else {
            return false;
        };
        if managed.session.is_alive().await {
            return false;
        }
        if let Some(_managed) = self.sessions.remove(&member_id) {
            self.session_registry.remove_handle(&member_id);
            tracing::info!(
                spawn_key = %member_id,
                "removed dead managed session entry to allow respawn"
            );
            true
        } else {
            false
        }
    }

    /// Returns the backend-assigned session id for the given managed
    /// member, if the backend exposes one. Currently only Claude Code
    /// backends populate this — the id matches the
    /// `~/.claude/projects/<cwd>/<id>.jsonl` filename.
    pub fn session_id_of(&self, member_id: impl AsRef<str>) -> Option<String> {
        self.sessions.get(member_id.as_ref())?.session.session_id()
    }

    pub async fn shutdown_managed_member(&mut self, member_id: impl AsRef<str>) -> Result<()> {
        let member_id = member_id.as_ref().to_string();
        tracing::info!(member_id = %member_id, "shutting down managed member");
        let mut managed = self.sessions.remove(&member_id).ok_or_else(|| {
            Error::Other(format!(
                "no managed session registered for spawn_key '{member_id}' (already shut down or never started)"
            ))
        })?;

        managed.set_runtime_state(ExecutionSessionState::Stopped);
        match managed.session.shutdown().await {
            Ok(()) => {
                self.session_registry.remove_handle(&member_id);
                Ok(())
            }
            Err(err) => {
                managed.last_error = Some(err.to_string());
                managed.set_runtime_state(ExecutionSessionState::Failed);
                let handle = managed.handle(&member_id, managed.session_state.name.as_str());
                self.session_registry.register_handle(handle);
                self.sessions.insert(member_id, managed);
                Err(err)
            }
        }
    }

    pub fn list_session_states(&self) -> Vec<SessionState> {
        let mut states: Vec<_> = self
            .sessions
            .values()
            .map(|managed| managed.session_state.clone())
            .collect();
        states.sort_by(|a, b| a.name.cmp(&b.name));
        states
    }

    pub fn list_persisted_sessions(&self) -> Vec<SessionState> {
        self.list_session_states()
    }
}
