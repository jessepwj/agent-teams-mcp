//! High-level team orchestrator.
//!
//! `TeamOrchestrator` composes the file-based managers ([`FileTeamManager`],
//! [`FileTaskManager`], [`FileInboxManager`]) with pluggable agent backends to
//! provide a single entry point for creating teams, spawning teammates, assigning
//! tasks, and exchanging messages.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::{info, warn};

/// Time-to-live for pending shutdown entries.
///
/// If a `send_input` call fails (e.g., process crash) after a shutdown was
/// requested, the pending entry would linger forever. Stale entries older
/// than this duration are cleaned up during `spawn_teammate`.
const PENDING_SHUTDOWN_TTL: Duration = Duration::from_secs(60);

use crate::backend::{AgentBackend, AgentSession, BackendType, SpawnConfig};
use crate::error::{Error, Result};
use crate::messaging::structured;
use crate::messaging::{FileInboxManager, InboxManager};
use crate::models::{
    CreateTaskRequest, InboxMessage, MemberUnion, TaskFile, TaskFilter, TaskUpdate, TeamConfig,
    TeammateMember,
};
use crate::task::{FileTaskManager, TaskManager};
use crate::team::{FileTeamManager, TeamManager};

/// Orchestrates team creation, agent lifecycle, task management, and messaging.
pub struct TeamOrchestrator {
    team_mgr: FileTeamManager,
    task_mgr: FileTaskManager,
    inbox_mgr: FileInboxManager,
    backends: HashMap<BackendType, Arc<dyn AgentBackend>>,
    /// Base directory for teams (used for session state persistence).
    teams_base: PathBuf,
    /// Live agent sessions keyed by `"{team}/{agent-name}"`.
    sessions: Arc<Mutex<HashMap<String, Box<dyn AgentSession>>>>,
    /// Agent keys with pending shutdown requests while temporarily removed by [`send_input`].
    ///
    /// Maps session key → timestamp when the shutdown was requested.
    /// Stale entries (older than [`PENDING_SHUTDOWN_TTL`]) are cleaned up
    /// during [`spawn_teammate`] to prevent unbounded memory growth.
    pending_shutdowns: Arc<Mutex<HashMap<String, Instant>>>,
    /// In-memory conversation memories keyed by `"{team}/{agent-name}"`.
    memories: Arc<Mutex<HashMap<String, crate::memory::ConversationMemory>>>,
    /// File-based memory persistence manager.
    memory_mgr: crate::memory::MemoryManager,
    /// Optional default router for [`spawn_teammate_smart`](Self::spawn_teammate_smart).
    default_router: Option<Arc<dyn crate::backend::router::BackendRouter>>,
    /// Optional auto-checkpoint trigger for task completion events.
    #[cfg(feature = "checkpoint")]
    auto_checkpoint: Option<crate::checkpoint::AutoCheckpointTrigger>,
}

impl TeamOrchestrator {
    /// Start building a `TeamOrchestrator`.
    pub fn builder() -> TeamOrchestratorBuilder {
        TeamOrchestratorBuilder::default()
    }

    // -----------------------------------------------------------------------
    // Team lifecycle
    // -----------------------------------------------------------------------

    /// Create a new team.
    pub async fn create_team(&self, name: &str, description: Option<&str>) -> Result<TeamConfig> {
        self.team_mgr.create_team(name, description).await
    }

    /// Read a team's config.
    pub async fn read_team(&self, name: &str) -> Result<TeamConfig> {
        self.team_mgr.read_config(name).await
    }

    /// List all teams.
    pub async fn list_teams(&self) -> Result<Vec<String>> {
        self.team_mgr.list_teams().await
    }

    /// Delete a team (fails if teammates are still registered).
    pub async fn delete_team(&self, name: &str) -> Result<()> {
        self.team_mgr.delete_team(name).await
    }

    // -----------------------------------------------------------------------
    // Teammate lifecycle
    // -----------------------------------------------------------------------

    /// Spawn a new teammate, register it in the team config, and initialise its inbox.
    ///
    /// If the config contains CLI delegations, their instructions are prepended
    /// to the prompt before spawning so the agent knows how to invoke those tools.
    pub async fn spawn_teammate(
        &self,
        team: &str,
        config: SpawnConfig,
        backend_type: BackendType,
    ) -> Result<()> {
        let backend =
            self.backends
                .get(&backend_type)
                .ok_or_else(|| Error::BackendNotConfigured {
                    backend: backend_type.to_string(),
                })?;

        // Inject CLI delegation instructions into prompt
        let config = if !config.delegations.is_empty() {
            let delegation_prompt =
                crate::backend::delegation::format_delegation_prompt(&config.delegations);
            let mut config = config;
            config.prompt = format!("{delegation_prompt}\n\n{}", config.prompt);
            config
        } else {
            config
        };

        let mut session = backend.spawn(config.clone()).await?;

        // Register member in team config.
        // If registration fails, clean up the already-spawned session.
        let member = MemberUnion::Teammate(TeammateMember {
            name: config.name.clone(),
            agent_id: format!("{}@{}", config.name, team),
            agent_type: backend_type.to_string(),
            prompt: config.prompt.clone(),
            model: config.model.clone(),
            color: None,
            plan_mode_required: None,
            joined_at: None,
            tmux_pane_id: None,
            cwd: config.cwd.as_ref().map(|p| p.display().to_string()),
            subscriptions: None,
            backend_type: Some(backend_type.to_string()),
        });
        if let Err(e) = self.team_mgr.add_member(team, member).await {
            let _ = session.shutdown().await;
            return Err(e);
        }

        // Initialise empty inbox (cleanup on failure)
        if let Err(e) = self.inbox_mgr.clear_inbox(team, &config.name).await {
            let _ = session.shutdown().await;
            let _ = self.team_mgr.remove_member(team, &config.name).await;
            return Err(e);
        }

        // Store session (clean up any existing one to prevent leaks)
        let key = format!("{team}/{}", config.name);
        let old_session = {
            let mut sessions = self.sessions.lock().await;
            let old = sessions.remove(&key);
            sessions.insert(key.clone(), session);
            old
        };
        // Clear any stale pending shutdown for this key (new session supersedes it)
        // and sweep entries older than PENDING_SHUTDOWN_TTL to prevent unbounded growth.
        {
            let mut pending = self.pending_shutdowns.lock().await;
            pending.remove(&key);
            pending.retain(|_, ts| ts.elapsed() < PENDING_SHUTDOWN_TTL);
        }
        // Clean up old session outside the lock (if any)
        if let Some(mut old) = old_session {
            warn!(team, agent = %config.name, "Shutting down replaced session");
            let _ = old.shutdown().await;
        }

        // Persist session state for resume support
        let session_state = crate::models::SessionState::from_config(&config, &backend_type);
        if let Err(e) = self.save_session_state(team, &session_state).await {
            warn!(team, agent = %config.name, error = %e, "Failed to persist session state");
        }

        // Auto-enable memory if configured
        if let Some(mem_config) = config.memory_config.clone()
            && let Err(e) = self.enable_memory(team, &config.name, mem_config).await
        {
            warn!(team, agent = %config.name, error = %e, "Failed to enable memory");
        }

        info!(team, agent = %config.name, %backend_type, "Teammate spawned");
        Ok(())
    }

    /// Spawn a teammate using a [`BackendRouter`](crate::backend::router::BackendRouter)
    /// to automatically select the best backend.
    ///
    /// The router's [`route()`](crate::backend::router::BackendRouter::route) method is called
    /// with the list of backends registered in this orchestrator. Returns the chosen
    /// `BackendType` alongside the `Ok(())`.
    pub async fn spawn_teammate_routed(
        &self,
        team: &str,
        config: SpawnConfig,
        router: &dyn crate::backend::router::BackendRouter,
    ) -> Result<BackendType> {
        let available: Vec<BackendType> = self.backends.keys().copied().collect();
        let chosen = router.route(&config, &available).await.ok_or_else(|| {
            Error::Other("Router found no suitable backend for the given config".into())
        })?;
        self.spawn_teammate(team, config, chosen).await?;
        Ok(chosen)
    }

    /// Spawn a teammate using the orchestrator's built-in default router.
    ///
    /// This is a convenience wrapper around [`spawn_teammate_routed`](Self::spawn_teammate_routed)
    /// that uses the router set via [`TeamOrchestratorBuilder::with_smart_router`].
    ///
    /// Returns the chosen `BackendType`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Other`] if no default router was configured on the builder.
    pub async fn spawn_teammate_smart(
        &self,
        team: &str,
        config: SpawnConfig,
    ) -> Result<BackendType> {
        let router = self.default_router.as_ref().ok_or_else(|| {
            Error::Other(
                "No default router configured; use TeamOrchestratorBuilder::with_smart_router()"
                    .into(),
            )
        })?;
        self.spawn_teammate_routed(team, config, router.as_ref())
            .await
    }

    /// Gracefully shut down a teammate and remove it from the team.
    pub async fn shutdown_teammate(&self, team: &str, name: &str) -> Result<()> {
        let key = format!("{team}/{name}");
        // Remove session from map first (releases the lock)
        let session = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(&key)
        };
        if let Some(mut session) = session {
            // Session found — shut it down outside the lock
            session.shutdown().await?;
        } else {
            // Session temporarily removed by send_input — mark for deferred shutdown
            // so send_input cleans it up on re-insert instead of creating a zombie.
            let mut pending = self.pending_shutdowns.lock().await;
            pending.insert(key, Instant::now());
        }
        // Remove from team config (ignore error if already removed)
        let _ = self.team_mgr.remove_member(team, name).await;
        // Clean up persisted session state
        if let Err(e) = self.remove_session_state(team, name).await {
            warn!(team, agent = name, error = %e, "Failed to remove session state file");
        }
        info!(team, agent = name, "Teammate shut down");
        Ok(())
    }

    /// Force-kill a teammate.
    pub async fn force_kill_teammate(&self, team: &str, name: &str) -> Result<()> {
        let key = format!("{team}/{name}");
        let session = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(&key)
        };
        if let Some(mut session) = session {
            session.force_kill().await?;
        } else {
            // Session temporarily removed by send_input — mark for deferred shutdown
            let mut pending = self.pending_shutdowns.lock().await;
            pending.insert(key, Instant::now());
        }
        let _ = self.team_mgr.remove_member(team, name).await;
        // Clean up persisted session state
        if let Err(e) = self.remove_session_state(team, name).await {
            warn!(team, agent = name, error = %e, "Failed to remove session state file");
        }
        info!(team, agent = name, "Teammate force-killed");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Messaging
    // -----------------------------------------------------------------------

    /// Send a message from one agent to another.
    pub async fn send_message(
        &self,
        team: &str,
        from: &str,
        to: &str,
        content: &str,
    ) -> Result<()> {
        let msg = InboxMessage::new(from, to, content);
        self.inbox_mgr.send_message(team, msg).await
    }

    /// Broadcast a message to all team members except the sender.
    pub async fn broadcast(&self, team: &str, from: &str, content: &str) -> Result<()> {
        let config = self.team_mgr.read_config(team).await?;
        let members: Vec<String> = config
            .members
            .iter()
            .map(|m| m.name().to_string())
            .collect();
        self.inbox_mgr
            .broadcast(team, from, content, &members)
            .await
    }

    /// Send a shutdown request message.
    pub async fn send_shutdown_request(
        &self,
        team: &str,
        from: &str,
        to: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        let structured_msg = structured::shutdown_request(
            uuid::Uuid::new_v4().to_string(),
            reason.unwrap_or("Shutdown requested"),
        );
        let msg = InboxMessage::from_structured(from, to, &structured_msg)?;
        self.inbox_mgr.send_message(team, msg).await
    }

    // -----------------------------------------------------------------------
    // Task management
    // -----------------------------------------------------------------------

    /// Create a task in a team.
    pub async fn create_task(&self, team: &str, request: CreateTaskRequest) -> Result<TaskFile> {
        self.task_mgr.create_task(team, request).await
    }

    /// Assign a task to a teammate and automatically send a TaskAssignment message.
    pub async fn assign_task(&self, team: &str, task_id: &str, assignee: &str) -> Result<TaskFile> {
        let update = TaskUpdate {
            owner: Some(assignee.to_string()),
            ..Default::default()
        };
        let task = self.task_mgr.update_task(team, task_id, update).await?;

        // Send a TaskAssignment message
        let structured_msg = structured::task_assignment(
            task.id.clone(),
            task.subject.clone(),
            task.description.clone().unwrap_or_default(),
        );
        let msg = InboxMessage::from_structured("system", assignee, &structured_msg)?;
        self.inbox_mgr.send_message(team, msg).await?;

        Ok(task)
    }

    /// Update a task.
    ///
    /// If auto-checkpoint is enabled and the task transitions to `Completed`,
    /// a checkpoint is created as a fire-and-forget side effect.
    pub async fn update_task(
        &self,
        team: &str,
        task_id: &str,
        update: TaskUpdate,
    ) -> Result<TaskFile> {
        let is_completing = update.status == Some(crate::models::TaskStatus::Completed);
        let task = self.task_mgr.update_task(team, task_id, update).await?;

        // Fire-and-forget auto-checkpoint on task completion
        #[cfg(feature = "checkpoint")]
        if is_completing {
            if let Some(ref trigger) = self.auto_checkpoint {
                let agent_name = task.owner.as_deref().unwrap_or("unknown");
                trigger.on_task_completed(Some(team), &task.subject, agent_name);
            }
        }

        // Suppress unused variable warning when checkpoint feature is off
        let _ = is_completing;

        Ok(task)
    }

    /// Get a single task.
    pub async fn get_task(&self, team: &str, task_id: &str) -> Result<TaskFile> {
        self.task_mgr.get_task(team, task_id).await
    }

    /// List tasks with optional filtering.
    pub async fn list_tasks(
        &self,
        team: &str,
        filter: Option<TaskFilter>,
    ) -> Result<Vec<TaskFile>> {
        self.task_mgr.list_tasks(team, filter).await
    }

    // -----------------------------------------------------------------------
    // Task graph visualization & analysis
    // -----------------------------------------------------------------------

    /// Export the task dependency graph as a Mermaid diagram.
    pub async fn export_task_graph_mermaid(&self, team: &str) -> Result<String> {
        let tasks = self.task_mgr.list_tasks(team, None).await?;
        let graph = crate::task::DependencyGraph::from_tasks(&tasks);
        Ok(graph.to_mermaid(&tasks))
    }

    /// Export the task dependency graph as a Graphviz DOT diagram.
    pub async fn export_task_graph_dot(&self, team: &str) -> Result<String> {
        let tasks = self.task_mgr.list_tasks(team, None).await?;
        let graph = crate::task::DependencyGraph::from_tasks(&tasks);
        Ok(graph.to_dot(&tasks))
    }

    /// Get the critical path (longest dependency chain) as task objects.
    pub async fn get_critical_path(&self, team: &str) -> Result<Vec<TaskFile>> {
        let tasks = self.task_mgr.list_tasks(team, None).await?;
        let graph = crate::task::DependencyGraph::from_tasks(&tasks);
        let path_ids = graph.critical_path(&tasks);
        let task_map: std::collections::HashMap<&str, &TaskFile> =
            tasks.iter().map(|t| (t.id.as_str(), t)).collect();
        Ok(path_ids
            .iter()
            .filter_map(|id| task_map.get(id.as_str()).map(|t| (*t).clone()))
            .collect())
    }

    /// Export the task graph as a terminal-friendly Unicode diagram with ANSI colors.
    ///
    /// Groups tasks by topological level (phase) and renders with colored status
    /// symbols, dependency hints, and critical path highlighting.
    pub async fn export_task_graph_terminal(&self, team: &str) -> Result<String> {
        let tasks = self.task_mgr.list_tasks(team, None).await?;
        let graph = crate::task::DependencyGraph::from_tasks(&tasks);
        Ok(graph.to_terminal(&tasks))
    }

    /// Export the task graph as a plain-text terminal diagram (no ANSI colors).
    pub async fn export_task_graph_terminal_plain(&self, team: &str) -> Result<String> {
        let tasks = self.task_mgr.list_tasks(team, None).await?;
        let graph = crate::task::DependencyGraph::from_tasks(&tasks);
        Ok(graph.to_terminal_plain(&tasks))
    }

    /// Get the next available task (pending, unowned, unblocked).
    pub async fn get_next_available_task(&self, team: &str) -> Result<Option<TaskFile>> {
        let filter = TaskFilter {
            status: Some(crate::models::TaskStatus::Pending),
            owner: None,
            unblocked_only: true,
        };
        let mut tasks = self.task_mgr.list_tasks(team, Some(filter)).await?;

        // Filter to only unowned tasks
        tasks.retain(|t| t.owner.is_none());

        // Sort by ID (numeric order)
        tasks.sort_by(|a, b| {
            let a_id: u64 = a.id.parse().unwrap_or(u64::MAX);
            let b_id: u64 = b.id.parse().unwrap_or(u64::MAX);
            a_id.cmp(&b_id)
        });

        Ok(tasks.into_iter().next())
    }

    // -----------------------------------------------------------------------
    // Consensus
    // -----------------------------------------------------------------------

    /// Resolve pre-collected agent responses into a consensus result.
    ///
    /// This is a pure function: the caller is responsible for collecting
    /// responses (e.g., via [`take_output_receiver`](Self::take_output_receiver)),
    /// and this method applies the chosen strategy.
    pub fn resolve_consensus(
        &self,
        responses: &[crate::consensus::AgentResponse],
        strategy: crate::consensus::ConsensusStrategy,
    ) -> crate::consensus::ConsensusResult {
        crate::consensus::resolve(strategy, responses)
    }

    // -----------------------------------------------------------------------
    // Agent memory
    // -----------------------------------------------------------------------

    /// Enable conversation memory for an agent.
    pub async fn enable_memory(
        &self,
        team: &str,
        agent: &str,
        config: crate::memory::MemoryConfig,
    ) -> Result<()> {
        let key = format!("{team}/{agent}");

        // Try to load existing memory from disk
        let memory = {
            let team_s = team.to_string();
            let agent_s = agent.to_string();
            let mgr = self.memory_mgr.clone();
            tokio::task::spawn_blocking(move || mgr.load(&team_s, &agent_s))
                .await
                .map_err(|e| Error::JoinError(e.to_string()))??
        };

        let memory = memory.unwrap_or_else(|| crate::memory::ConversationMemory::new(config));

        let mut memories = self.memories.lock().await;
        memories.insert(key, memory);
        Ok(())
    }

    /// Disable conversation memory for an agent and persist remaining state.
    pub async fn disable_memory(&self, team: &str, agent: &str) -> Result<()> {
        let key = format!("{team}/{agent}");
        let memory = {
            let mut memories = self.memories.lock().await;
            memories.remove(&key)
        };

        // Persist final state before disabling
        if let Some(mem) = memory {
            let team_s = team.to_string();
            let agent_s = agent.to_string();
            let mgr = self.memory_mgr.clone();
            tokio::task::spawn_blocking(move || mgr.save(&team_s, &agent_s, &mem))
                .await
                .map_err(|e| Error::JoinError(e.to_string()))??;
        }

        Ok(())
    }

    /// Record an assistant output into the agent's memory.
    pub async fn record_assistant_output(
        &self,
        team: &str,
        agent: &str,
        content: &str,
    ) -> Result<()> {
        let key = format!("{team}/{agent}");
        let mut memories = self.memories.lock().await;
        if let Some(memory) = memories.get_mut(&key) {
            memory.record_turn(crate::memory::Role::Assistant, content);
        }
        Ok(())
    }

    /// Clear all recorded turns in an agent's memory.
    pub async fn clear_memory(&self, team: &str, agent: &str) -> Result<()> {
        let key = format!("{team}/{agent}");
        let mut memories = self.memories.lock().await;
        if let Some(memory) = memories.get_mut(&key) {
            memory.clear();
        }

        // Also clear persisted state
        let team_s = team.to_string();
        let agent_s = agent.to_string();
        let mgr = self.memory_mgr.clone();
        tokio::task::spawn_blocking(move || mgr.delete(&team_s, &agent_s))
            .await
            .map_err(|e| Error::JoinError(e.to_string()))??;

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Send input to a live session
    // -----------------------------------------------------------------------

    /// Send input to a running agent session.
    ///
    /// If memory is enabled for this agent, the conversation context is
    /// prepended to the input, and the user turn is recorded afterward.
    ///
    /// The session is temporarily removed from the map so the global lock is
    /// not held across the (potentially slow) `.await` on `send_input`.
    /// If a shutdown is requested while the session is in-flight, the session
    /// is shut down instead of being re-inserted (prevents zombie sessions).
    pub async fn send_input(&self, team: &str, agent: &str, input: &str) -> Result<()> {
        let key = format!("{team}/{agent}");

        // Prepend memory context if available
        let effective_input = {
            let memories = self.memories.lock().await;
            if let Some(memory) = memories.get(&key) {
                let ctx = memory.format_context();
                if ctx.is_empty() {
                    input.to_string()
                } else {
                    format!("{ctx}\n---\n{input}")
                }
            } else {
                input.to_string()
            }
        };

        // Remove session from map (releases global lock immediately)
        let mut session = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(&key).ok_or_else(|| Error::AgentNotAlive {
                name: agent.to_string(),
            })?
        };

        // Perform the async send without holding the lock
        let result = session.send_input(&effective_input).await;

        // Record user turn in memory (fire-and-forget persistence)
        {
            let mut memories = self.memories.lock().await;
            if let Some(memory) = memories.get_mut(&key) {
                memory.record_turn(crate::memory::Role::User, input);

                // Fire-and-forget async persistence
                let mem_clone = memory.clone();
                let team_s = team.to_string();
                let agent_s = agent.to_string();
                let mgr = self.memory_mgr.clone();
                tokio::spawn(async move {
                    let _ = tokio::task::spawn_blocking(move || {
                        mgr.save(&team_s, &agent_s, &mem_clone)
                    })
                    .await;
                });
            }
        }

        // Check if a shutdown was requested while we were sending
        let shutdown_requested = {
            let mut pending = self.pending_shutdowns.lock().await;
            pending.remove(&key).is_some()
        };

        if shutdown_requested {
            // Shutdown was requested during our send — honor it instead of re-inserting
            warn!(
                team,
                agent, "Honoring deferred shutdown for in-flight session"
            );
            let _ = session.shutdown().await;
        } else {
            // Normal case — put the session back
            let mut sessions = self.sessions.lock().await;
            sessions.insert(key, session);
        }

        result
    }

    /// Take the output receiver from an agent session.
    ///
    /// This can only be called once per agent -- the receiver is moved out.
    /// Returns `Ok(None)` if the receiver was already taken.
    pub async fn take_output_receiver(
        &self,
        team: &str,
        agent: &str,
    ) -> Result<Option<tokio::sync::mpsc::Receiver<crate::backend::AgentOutput>>> {
        let key = format!("{team}/{agent}");
        // output_receiver() is a synchronous Option::take — fast, no .await needed
        let mut sessions = self.sessions.lock().await;
        let session = sessions.get_mut(&key).ok_or_else(|| Error::AgentNotAlive {
            name: agent.to_string(),
        })?;
        Ok(session.output_receiver())
    }

    /// Take the output as a [`Stream`](tokio_stream::Stream) for ergonomic use with `StreamExt`.
    ///
    /// This is a convenience wrapper around [`take_output_receiver`](Self::take_output_receiver)
    /// that wraps the `mpsc::Receiver` in a [`ReceiverStream`](tokio_stream::wrappers::ReceiverStream).
    ///
    /// ```ignore
    /// use tokio_stream::StreamExt;
    ///
    /// let mut stream = orch.take_output_stream("team", "agent").await?
    ///     .expect("receiver not yet taken");
    ///
    /// while let Some(event) = stream.next().await {
    ///     // ...
    /// }
    /// ```
    pub async fn take_output_stream(
        &self,
        team: &str,
        agent: &str,
    ) -> Result<Option<crate::backend::AgentOutputStream>> {
        let rx = self.take_output_receiver(team, agent).await?;
        Ok(rx.map(tokio_stream::wrappers::ReceiverStream::new))
    }

    /// Check if a teammate's session is alive.
    pub async fn is_alive(&self, team: &str, agent: &str) -> bool {
        let key = format!("{team}/{agent}");
        // is_alive() is a cheap AtomicBool load — keeping the lock is fine here
        let sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(&key) {
            session.is_alive().await
        } else {
            false
        }
    }

    /// Check liveness for all members of a team in a single lock acquisition.
    ///
    /// Returns a map of `agent_name → alive`. More efficient than calling
    /// [`is_alive`](Self::is_alive) in a loop when you need status for the
    /// whole team (e.g., dashboard rendering).
    pub async fn are_alive(&self, team: &str) -> Result<HashMap<String, bool>> {
        let config = self.team_mgr.read_config(team).await?;
        let sessions = self.sessions.lock().await;
        let mut result = HashMap::with_capacity(config.members.len());
        for m in &config.members {
            let key = format!("{team}/{}", m.name());
            let alive = if let Some(session) = sessions.get(&key) {
                session.is_alive().await
            } else {
                false
            };
            result.insert(m.name().to_string(), alive);
        }
        Ok(result)
    }

    /// Shut down all teammates in a team and remove them from the config.
    ///
    /// Returns the number of agents successfully shut down. Errors during
    /// individual shutdowns are logged but do not stop the process.
    pub async fn shutdown_all_teammates(&self, team: &str) -> Result<usize> {
        let config = self.team_mgr.read_config(team).await?;
        let names: Vec<String> = config
            .members
            .iter()
            .map(|m| m.name().to_string())
            .collect();
        let mut count = 0;
        for name in &names {
            match self.shutdown_teammate(team, name).await {
                Ok(()) => count += 1,
                Err(e) => {
                    warn!(team, agent = %name, error = %e, "Failed to shut down teammate");
                }
            }
        }
        Ok(count)
    }

    // -----------------------------------------------------------------------
    // Session persistence (resume support)
    // -----------------------------------------------------------------------

    /// Directory for session state files: `{teams_base}/{team}/sessions/`
    fn sessions_dir(&self, team: &str) -> PathBuf {
        self.teams_base.join(team).join("sessions")
    }

    /// Persist a session's state to disk so it can be resumed later.
    async fn save_session_state(
        &self,
        team: &str,
        state: &crate::models::SessionState,
    ) -> Result<()> {
        let dir = self.sessions_dir(team);
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(format!("{}.json", state.name));
        let state = state.clone();
        tokio::task::spawn_blocking(move || {
            crate::util::atomic_write::atomic_write_json(&path, &state)
        })
        .await
        .map_err(|e| Error::JoinError(e.to_string()))??;
        Ok(())
    }

    /// Remove a session's persisted state (called on shutdown).
    async fn remove_session_state(&self, team: &str, agent: &str) -> Result<()> {
        let path = self.sessions_dir(team).join(format!("{agent}.json"));
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
    }

    /// List all persisted session states for a team.
    pub async fn list_persisted_sessions(
        &self,
        team: &str,
    ) -> Result<Vec<crate::models::SessionState>> {
        let dir = self.sessions_dir(team);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut states = Vec::new();
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let content = tokio::fs::read_to_string(&path).await?;
                match serde_json::from_str::<crate::models::SessionState>(&content) {
                    Ok(state) => states.push(state),
                    Err(e) => {
                        warn!(
                            team,
                            path = %path.display(),
                            error = %e,
                            "Failed to parse session state file, skipping"
                        );
                    }
                }
            }
        }
        Ok(states)
    }

    /// Resume all persisted sessions for a team.
    ///
    /// Re-spawns each agent with its saved configuration. Returns the number
    /// of successfully resumed sessions. Sessions that fail to resume (e.g.,
    /// backend not configured) are logged and skipped.
    pub async fn resume_teammates(&self, team: &str) -> Result<usize> {
        let states = self.list_persisted_sessions(team).await?;
        let mut resumed = 0;

        for state in &states {
            let backend_type = match state.parse_backend_type() {
                Some(bt) => bt,
                None => {
                    warn!(
                        team,
                        agent = %state.name,
                        backend = %state.backend_type,
                        "Unknown backend type, skipping resume"
                    );
                    continue;
                }
            };

            let config = state.to_spawn_config();
            match self.spawn_teammate(team, config, backend_type).await {
                Ok(()) => {
                    info!(team, agent = %state.name, "Session resumed");
                    resumed += 1;
                }
                Err(e) => {
                    warn!(
                        team,
                        agent = %state.name,
                        error = %e,
                        "Failed to resume session, skipping"
                    );
                }
            }
        }

        Ok(resumed)
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for [`TeamOrchestrator`] with customizable base directories and backends.
///
/// # Example
///
/// ```rust,no_run
/// use agent_teams::orchestrator::TeamOrchestrator;
///
/// let orch = TeamOrchestrator::builder()
///     .teams_base("/tmp/teams")
///     .tasks_base("/tmp/tasks")
///     .build()
///     .unwrap();
/// ```
#[derive(Default)]
pub struct TeamOrchestratorBuilder {
    teams_base: Option<PathBuf>,
    tasks_base: Option<PathBuf>,
    backends: HashMap<BackendType, Arc<dyn AgentBackend>>,
    default_router: Option<Arc<dyn crate::backend::router::BackendRouter>>,
    #[cfg(feature = "checkpoint")]
    auto_checkpoint_repo: Option<PathBuf>,
}

impl TeamOrchestratorBuilder {
    /// Set a custom base directory for team configs (default: `~/.claude/teams`).
    pub fn teams_base(mut self, path: impl Into<PathBuf>) -> Self {
        self.teams_base = Some(path.into());
        self
    }

    /// Set a custom base directory for task storage (default: `~/.claude/tasks`).
    pub fn tasks_base(mut self, path: impl Into<PathBuf>) -> Self {
        self.tasks_base = Some(path.into());
        self
    }

    /// Register a Claude Code backend.
    pub fn with_claude_code(mut self, backend: impl AgentBackend + 'static) -> Self {
        self.backends
            .insert(BackendType::ClaudeCode, Arc::new(backend));
        self
    }

    /// Register a Codex backend.
    pub fn with_codex(mut self, backend: impl AgentBackend + 'static) -> Self {
        self.backends.insert(BackendType::Codex, Arc::new(backend));
        self
    }

    /// Register a Gemini CLI backend.
    pub fn with_gemini_cli(mut self, backend: impl AgentBackend + 'static) -> Self {
        self.backends
            .insert(BackendType::GeminiCli, Arc::new(backend));
        self
    }

    /// Register a generic backend.
    pub fn with_backend(
        mut self,
        backend_type: BackendType,
        backend: impl AgentBackend + 'static,
    ) -> Self {
        self.backends.insert(backend_type, Arc::new(backend));
        self
    }

    /// Enable auto-checkpoint on task completion.
    ///
    /// When enabled, completing a task via `update_task()` automatically
    /// creates a checkpoint attached to the current HEAD commit.
    ///
    /// `repo_path` should point to the git repository root.
    #[cfg(feature = "checkpoint")]
    pub fn with_auto_checkpoint(mut self, repo_path: impl Into<PathBuf>) -> Self {
        self.auto_checkpoint_repo = Some(repo_path.into());
        self
    }

    /// Set a default router for [`TeamOrchestrator::spawn_teammate_smart`].
    ///
    /// Any type implementing [`BackendRouter`](crate::backend::router::BackendRouter) can be
    /// used, but [`SmartRouter`](crate::backend::router::SmartRouter) is the recommended choice:
    ///
    /// ```rust,no_run
    /// use agent_teams::prelude::*;
    /// use agent_teams::backend::router::SmartRouter;
    /// use agent_teams::TeamOrchestrator;
    ///
    /// let orch = TeamOrchestrator::builder()
    ///     .with_smart_router(SmartRouter::new(BackendType::ClaudeCode)
    ///         .simple_backend(BackendType::GeminiCli)
    ///         .complex_backend(BackendType::ClaudeCode))
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn with_smart_router(
        mut self,
        router: impl crate::backend::router::BackendRouter + 'static,
    ) -> Self {
        self.default_router = Some(Arc::new(router));
        self
    }

    /// Build the orchestrator.
    pub fn build(self) -> Result<TeamOrchestrator> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let claude_dir = home.join(".claude");

        let teams_base = self.teams_base.unwrap_or_else(|| claude_dir.join("teams"));
        let tasks_base = self.tasks_base.unwrap_or_else(|| claude_dir.join("tasks"));

        let memory_mgr = crate::memory::MemoryManager::new(teams_base.clone());

        #[cfg(feature = "checkpoint")]
        let auto_checkpoint = self.auto_checkpoint_repo.map(|repo_path| {
            crate::checkpoint::AutoCheckpointTrigger::new(
                repo_path,
                teams_base.clone(),
                tasks_base.clone(),
            )
        });

        Ok(TeamOrchestrator {
            team_mgr: FileTeamManager::new(teams_base.clone()),
            task_mgr: FileTaskManager::new(tasks_base),
            inbox_mgr: FileInboxManager::new(teams_base.clone()),
            backends: self.backends,
            teams_base,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pending_shutdowns: Arc::new(Mutex::new(HashMap::new())),
            memories: Arc::new(Mutex::new(HashMap::new())),
            memory_mgr,
            default_router: self.default_router,
            #[cfg(feature = "checkpoint")]
            auto_checkpoint,
        })
    }
}
