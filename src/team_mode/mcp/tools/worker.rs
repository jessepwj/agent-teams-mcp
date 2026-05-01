use super::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::ExecutionSessionState;
use crate::backend::{AgentOutput, BackendType, SpawnConfig};
use crate::runtime::AgentLoop;
use crate::team_mode::domain::{ExecutionMode, ExecutionProfile, MemberKind, MemberStatus};
use crate::team_mode::mcp::resources::inbox_uri;
use crate::team_mode::runtime_workers::{STATE_DEAD, STATE_FAILED, STATE_RUNNING, STATE_STARTING};
use crate::team_mode::storage::MemberRecord;

impl TeamModeToolset {
    pub(super) fn worker_add(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let worker_name = required_identifier(args, "name")?;
        if worker_name == LEAD_NAME {
            return Err(Error::Other(
                "'lead' is a reserved name and cannot be used for a worker".into(),
            ));
        }
        // Reserve the synthetic web-UI sender name. If a user named a real
        // worker `user`, web messages from the browser would land in that
        // worker's inbox (sender == self), tripping the self-mention guard
        // and possibly other downstream confusion.
        if worker_name == crate::team_mode_web::read_model::WEB_USER_SENDER {
            return Err(Error::Other(format!(
                "'{}' is reserved for the web-UI sender and cannot be used for a worker",
                crate::team_mode_web::read_model::WEB_USER_SENDER
            )));
        }
        // Same strict slug check as team_create. Without this, names with
        // spaces / uppercase / unicode were accepted but unreachable via
        // @mention — workers existed in members.json but no message could
        // ever route to them.
        crate::util::validate_slug_name(&worker_name)?;

        let team = self
            .team_service
            .get(&team_name)?
            .ok_or_else(|| Error::TeamNotFound {
                name: team_name.clone(),
            })?;

        let existing = self.member_store.get(&team_name, &worker_name)?;
        let existing_execution = existing.as_ref().and_then(|r| r.execution.clone());
        let caller_adapter = optional_identifier(args, "adapter")?;
        let on_existing = optional_identifier(args, "on_existing")?
            .as_deref()
            .map(parse_on_existing)
            .transpose()?
            .unwrap_or(OnExisting::Error);

        // Decide the mode
        let mode = match (&existing_execution, &caller_adapter, on_existing) {
            (Some(_), _, OnExisting::Reuse) => WorkerAddMode::Reuse,
            (Some(_), Some(_), OnExisting::Overwrite) => WorkerAddMode::Overwrite,
            (Some(_), None, OnExisting::Overwrite) => {
                return Err(Error::Other(
                    "on_existing=overwrite requires `adapter`".into(),
                ));
            }
            (Some(_), _, OnExisting::Error) => {
                let members_path = self.member_store_members_file_hint(&team_name);
                return Err(Error::Other(format!(
                    "worker '{worker_name}' already exists with an execution profile ({members_path}). \
                     Pass on_existing=reuse to fast-resume the saved config, \
                     on_existing=overwrite to replace it (adapter required), \
                     or choose a different worker name."
                )));
            }
            (None, Some(_), _) => WorkerAddMode::Create,
            (None, None, _) => WorkerAddMode::Create,
        };

        // Build / resolve execution profile
        let execution = match mode {
            WorkerAddMode::Reuse => existing_execution.clone().ok_or_else(|| {
                Error::Other(format!(
                    "worker '{worker_name}' has no saved profile to reuse"
                ))
            })?,
            WorkerAddMode::Overwrite | WorkerAddMode::Create => {
                let adapter = caller_adapter
                    .clone()
                    .unwrap_or_else(|| "codex".to_string());
                let cwd_for_worker = optional_text(args, "cwd")?.or_else(|| team.cwd.clone());
                ExecutionProfile {
                    execution_mode: ExecutionMode::Managed,
                    adapter: Some(adapter),
                    agent_name: Some(worker_name.clone()),
                    model: optional_identifier(args, "model")?,
                    cwd: cwd_for_worker,
                    env: parse_env_map(args.get("env"))?,
                    system_prompt: optional_text(args, "system_prompt")?,
                    skills: Vec::new(),
                    session_state: None,
                    session_id: None,
                    // Optional. None = let the backend's own config file
                    // (e.g. `~/.codex/config.toml`) decide. Don't default
                    // to `"medium"` — that would silently override the
                    // user's global preference.
                    reasoning_effort: optional_identifier(args, "effort")?,
                }
            }
        };

        // Validate adapter up-front — BEFORE any state changes. Otherwise a
        // bad adapter leaves behind a phantom member record that later shows
        // up in worker_list and @mention resolution.
        let backend_type: BackendType = execution
            .adapter
            .clone()
            .ok_or_else(|| Error::Other("execution profile missing adapter".into()))?
            .parse()
            .map_err(|e: String| Error::Other(e))?;

        // Idempotent reuse: if the orchestrator already has a live session
        // for this spawn_key, don't try to re-spawn (which would error
        // "already registered"). Just report the current state.
        //
        // If the worker was registered but has died (process gone), we drop
        // the stale session entry now so the spawn path below can recreate
        // a fresh one. Without this cleanup `spawn_managed_member` rejects
        // any spawn_key already in its HashMap, so a `worker_add reuse` on
        // a dead worker would fail with "already registered" — exactly the
        // workflow the `[SYSTEM] worker died ... use on_existing=reuse to
        // restart` notice tells the lead to perform.
        let spawn_key_pre = spawn_key(&team_name, &worker_name);
        let (already_live, revived_from_dead) = self.async_runtime.block_on({
            let orch = Arc::clone(&self.runtime_orchestrator);
            let key = spawn_key_pre.clone();
            async move {
                let mut guard = orch.lock().await;
                let alive = guard.is_alive(&key).await.unwrap_or(false);
                let revived = if !alive && guard.has_session(&key) {
                    guard.remove_dead_session_if_any(&key).await
                } else {
                    false
                };
                (alive, revived)
            }
        });
        if matches!(mode, WorkerAddMode::Reuse) && already_live {
            // Make sure the on-disk session_state reflects reality and return.
            let _ = self.member_store.update(&team_name, &worker_name, |m| {
                if let Some(exec) = m.execution.as_mut() {
                    exec.session_state = Some(ExecutionSessionState::Running);
                }
            });
            let _ = self.runtime_workers.upsert_state(
                &team_name,
                &worker_name,
                &spawn_key_pre,
                execution.adapter.clone(),
                STATE_RUNNING,
                None,
            );
            return Ok(success_with_updates(
                json!({
                    "team": team_name.clone(),
                    "name": worker_name.clone(),
                    "sessionState": "running",
                    "mode": "reuse",
                }),
                vec![team_uri(&team_name), inbox_uri(&team_name, &worker_name)],
            ));
        }

        // Upsert identity + execution (only after validation and liveness check)
        let record = MemberRecord {
            profile: crate::team_mode::domain::MemberProfile {
                team_id: team_name.clone(),
                name: worker_name.clone(),
                kind: MemberKind::Member,
                role_label: "worker".into(),
                role_description: None,
                status: MemberStatus::Active,
                joined_at: existing
                    .as_ref()
                    .map(|e| e.profile.joined_at)
                    .unwrap_or_else(chrono::Utc::now),
            },
            execution: Some(execution.clone()),
        };
        self.member_store.upsert(record.clone())?;
        let prompt = execution
            .system_prompt
            .clone()
            .unwrap_or_else(|| format!("You are {}", worker_name));
        let mut config = SpawnConfig::new(
            execution
                .agent_name
                .clone()
                .unwrap_or_else(|| worker_name.clone()),
            prompt,
        );
        config.model = execution.model.clone();
        config.cwd = execution.cwd.as_ref().map(PathBuf::from);
        config.env = execution.env.clone();
        // Identity binding — when this worker spawns a `team_mode_mcp` relay
        // (via .mcp.json auto-discovery for claude-code; via injected codex
        // config.toml for codex), the relay reads these env vars to know
        // which member is making the MCP call. The daemon then attributes
        // `send_message` and other identity-aware tools to this worker
        // instead of defaulting to "lead".
        //
        // Without these, every worker's MCP call would silently appear to
        // come from the lead — workers could forge messages, and there'd
        // be no way to enforce per-member access scoping.
        config
            .env
            .insert("TEAM_MODE_TEAM".into(), team_name.clone());
        config
            .env
            .insert("TEAM_MODE_MEMBER".into(), worker_name.clone());
        config
            .env
            .insert("TEAM_MODE_WORKER_ID".into(), worker_name.clone());
        if let Ok(url) = std::env::var("TEAM_MODE_HTTP_MCP_URL") {
            config.env.insert("TEAM_MODE_HTTP_MCP_URL".into(), url);
        }
        if let Ok(token) = std::env::var("TEAM_MODE_HTTP_MCP_TOKEN") {
            config.env.insert("TEAM_MODE_HTTP_MCP_TOKEN".into(), token);
        }
        // Pass through whatever the caller stored — `None` means the
        // backend CLI consults its own config (e.g. `~/.codex/config.toml`).
        // No project-level hardcoding: a user who set `model_reasoning_effort
        // = "high"` globally must see that take effect.
        config.reasoning_effort = execution.reasoning_effort.clone();

        let key = spawn_key(&team_name, &worker_name);
        self.runtime_workers.upsert_state(
            &team_name,
            &worker_name,
            &key,
            execution.adapter.clone(),
            STATE_STARTING,
            None,
        )?;
        let orch = Arc::clone(&self.runtime_orchestrator);
        let key_clone = key.clone();
        let wname = worker_name.clone();
        let handle = match self.async_runtime.block_on(async move {
            orch.lock()
                .await
                .spawn_managed_member(key_clone, wname, config, backend_type)
                .await
        }) {
            Ok(handle) => handle,
            Err(err) => {
                let _ = self.runtime_workers.upsert_state(
                    &team_name,
                    &worker_name,
                    &key,
                    execution.adapter.clone(),
                    STATE_FAILED,
                    Some(err.to_string()),
                );
                return Err(err);
            }
        };

        let orch2 = Arc::clone(&self.runtime_orchestrator);
        let key_for_rx = key.clone();
        let output_rx = self
            .async_runtime
            .block_on(async move { orch2.lock().await.take_output_receiver(&key_for_rx) })
            .ok()
            .flatten();

        // Ready-check: drain until first TurnComplete / Idle / Error / process exit.
        //
        // No timeout: different adapters have very different cold-start costs
        // (codex on Windows can take 10-15s, claude-code ~2s). Capping with
        // a fixed timeout caused the caller to receive `state="starting"` for
        // slow starters, which then never got updated to "running" because
        // nothing downstream transitions that state. Instead we wait for an
        // unambiguous terminal signal — success or explicit failure. If a
        // worker truly hangs forever, the caller can interrupt the MCP call.
        let (rx_for_loop, ready_state) = if let Some(mut rx) = output_rx {
            let (reported_state, remaining_rx) = self.async_runtime.block_on(async move {
                let result = async {
                    loop {
                        match rx.recv().await {
                            Some(AgentOutput::TurnComplete) | Some(AgentOutput::Idle) => {
                                return Ok::<(), String>(());
                            }
                            Some(AgentOutput::Error(e)) => {
                                return Err(format!("agent error during startup: {e}"));
                            }
                            Some(_) => continue,
                            None => return Err("agent process exited during startup".into()),
                        }
                    }
                }
                .await;
                let state = match result {
                    Ok(()) => "running",
                    Err(_) => "failed",
                };
                (state, rx)
            });

            if reported_state == "failed" {
                // Clean up: remove the profile we just wrote, reset status
                let _ = self.member_store.mark_removed(&team_name, &worker_name);
                let _ = self.runtime_workers.upsert_state(
                    &team_name,
                    &worker_name,
                    &key,
                    execution.adapter.clone(),
                    STATE_FAILED,
                    Some("agent process exited during startup".into()),
                );
                return Err(Error::Other(format!(
                    "worker '{worker_name}' failed to start"
                )));
            }

            (Some(remaining_rx), reported_state.to_string())
        } else {
            (None, handle.session_state.as_str().to_string())
        };

        // Persist runtime session_state to disk so worker_list reflects the live
        // status (otherwise it always reads back "not-spawned" from the initial
        // upsert done before spawn_managed_member).
        let persisted_state = match ready_state.as_str() {
            "running" => Some(ExecutionSessionState::Running),
            "starting" => Some(ExecutionSessionState::Starting),
            _ => None,
        };
        let session_id = self.async_runtime.block_on({
            let orch = Arc::clone(&self.runtime_orchestrator);
            let key = key.clone();
            async move { orch.lock().await.session_id_of(&key) }
        });
        if let Some(state) = persisted_state {
            let _ = self.member_store.update(&team_name, &worker_name, |m| {
                if let Some(exec) = m.execution.as_mut() {
                    exec.session_state = Some(state);
                    if session_id.is_some() {
                        exec.session_id = session_id.clone();
                    }
                }
            });
        }
        let _ = self.runtime_workers.upsert_state(
            &team_name,
            &worker_name,
            &key,
            execution.adapter.clone(),
            ready_state.as_str(),
            None,
        );

        if let Some(rx) = rx_for_loop {
            let agent_loop = AgentLoop {
                member_id: worker_name.clone(),
                session_key: key.clone(),
                team_id: team_name.clone(),
                room_id: "main".into(),
                orchestrator: Arc::clone(&self.runtime_orchestrator),
                inbox_service: self.inbox_service.clone(),
                message_store: self.message_store.clone(),
                message_service: self.message_service.clone(),
                poll_interval: Duration::from_secs(5),
                inbox_notifier: Some(self.inbox_notifier.clone()),
                member_store: Some(self.member_store.clone()),
            };
            let loop_handle = agent_loop.start(rx);
            self.lock_loop_handles()?.insert(key.clone(), loop_handle);
        }

        let mode_str = match mode {
            WorkerAddMode::Reuse => "reuse",
            WorkerAddMode::Overwrite => "overwrite",
            WorkerAddMode::Create => "create",
        };

        let mut payload = json!({
            "team": team_name.clone(),
            "name": worker_name.clone(),
            "sessionState": ready_state,
            "mode": mode_str,
        });
        if let Value::Object(map) = &mut payload {
            if revived_from_dead {
                map.insert("revived_from_dead".into(), Value::Bool(true));
                map.insert(
                    "note".into(),
                    Value::String(
                        "Previous worker process was dead — its stale session was \
                         dropped and a fresh process spawned. The worker has a new \
                         conversation context (no memory of prior turns)."
                            .into(),
                    ),
                );
            }
            // Reach out only on the create path: reuse already returns
            // because the worker has been observed before, so the lead
            // already knows the session_id timing trick.
            if matches!(mode, WorkerAddMode::Create) {
                map.insert(
                    "hint".into(),
                    Value::String(
                        "Worker process started. Its backend session_id is captured \
                         after the FIRST `type:result` event — i.e. once you send the \
                         first @mention message and the worker replies. Until then, \
                         the web UI 'process session' pane shows a placeholder for \
                         this worker."
                            .into(),
                    ),
                );
            }
        }

        Ok(success_with_updates(
            payload,
            vec![team_uri(&team_name), inbox_uri(&team_name, &worker_name)],
        ))
    }

    pub(super) fn worker_remove(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let worker_name = required_identifier(args, "name")?;
        if worker_name == LEAD_NAME {
            return Err(Error::Other(
                "the team lead cannot be removed via worker_remove".into(),
            ));
        }
        let key = spawn_key(&team_name, &worker_name);

        // Shut down process (best-effort)
        let orch = Arc::clone(&self.runtime_orchestrator);
        let key_clone = key.clone();
        let _ = self
            .async_runtime
            .block_on(async move { orch.lock().await.shutdown_managed_member(&key_clone).await });
        if let Some(h) = self.lock_loop_handles()?.remove(&key) {
            let _ = h.shutdown_tx.send(());
        }
        let _ = self.runtime_workers.upsert_state(
            &team_name,
            &worker_name,
            &key,
            None,
            STATE_STOPPED,
            None,
        );

        // Soft-remove: status=Removed but keep execution for fast-resume.
        let changed = self.member_store.mark_removed(&team_name, &worker_name)?;
        if !changed {
            return Err(Error::MemberNotFound {
                team: team_name,
                member: worker_name,
            });
        }

        Ok(success_with_updates(
            json!({ "ok": true, "team": team_name.clone(), "name": worker_name.clone() }),
            vec![team_uri(&team_name), inbox_uri(&team_name, &worker_name)],
        ))
    }

    pub(super) fn worker_list(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let members = self.member_service.list_active(&team_name)?;

        // Cross-reference stored session_state with actual process liveness
        // from the orchestrator. The stored state lags reality: if a worker
        // process crashes or is killed externally, the agent_loop detects
        // it and exits, but nothing updates member_store — so a subsequent
        // `worker_list` would still say "running". For the lead's benefit
        // (especially in long-running task coordination), expose the live
        // view: stored="running" but orchestrator says "not alive" -> "dead".
        let orch = Arc::clone(&self.runtime_orchestrator);
        let workers: Vec<Value> = members
            .into_iter()
            .filter(|r| !matches!(r.profile.kind, MemberKind::Lead))
            // Exclude the synthetic `user` member auto-created when the web
            // UI sends its first message. It has no execution profile, so
            // the liveness probe below would falsely flag it as "dead" and
            // suggest revival via `worker_add reuse` — confusing UX. The
            // web user is a sender identity, not a worker.
            .filter(|r| {
                let label = r.profile.role_label.as_str();
                let name = r.profile.name.as_str();
                label != crate::team_mode_web::read_model::WEB_USER_ROLE_LABEL
                    && name != crate::team_mode_web::read_model::WEB_USER_SENDER
            })
            .map(|record| {
                let adapter = record
                    .execution
                    .as_ref()
                    .and_then(|e| e.adapter.clone())
                    .unwrap_or_default();
                let stored_state = record
                    .execution
                    .as_ref()
                    .and_then(|e| e.session_state.as_ref())
                    .map(|s| s.as_str())
                    .unwrap_or("not-spawned")
                    .to_string();

                // Only run the liveness query when the stored state CLAIMS
                // running — otherwise `not-spawned` / `failed` / `removed`
                // are already accurate and we save the orchestrator lock.
                let sidecar_state = self
                    .runtime_workers
                    .state_for(&team_name, &record.profile.name)
                    .ok()
                    .flatten();
                let session_state = if sidecar_state.as_deref() == Some(STATE_DEAD) {
                    STATE_DEAD.to_string()
                } else if stored_state == "running" {
                    let key = spawn_key(&team_name, &record.profile.name);
                    let orch = Arc::clone(&orch);
                    let alive = self.async_runtime.block_on(async move {
                        orch.lock().await.is_alive(&key).await.unwrap_or(false)
                    });
                    if alive {
                        STATE_RUNNING.to_string()
                    } else {
                        STATE_DEAD.to_string()
                    }
                } else {
                    sidecar_state.unwrap_or(stored_state)
                };

                json!({
                    "name": record.profile.name,
                    "adapter": adapter,
                    "sessionState": session_state,
                })
            })
            .collect();
        let dead_names: Vec<String> = workers
            .iter()
            .filter_map(|w| {
                let state = w.get("sessionState")?.as_str()?;
                if state == "dead" {
                    w.get("name")?.as_str().map(String::from)
                } else {
                    None
                }
            })
            .collect();
        let mut payload = json!({ "workers": workers });
        if !dead_names.is_empty() {
            if let Value::Object(map) = &mut payload {
                map.insert(
                    "hint".into(),
                    Value::String(format!(
                        "Dead workers found: [{}]. Revive each with \
                         `worker_add name=<x> on_existing=reuse`. The worker \
                         will lose its prior conversation context but its \
                         saved profile (adapter / model / system_prompt) \
                         is reused automatically.",
                        dead_names.join(", ")
                    )),
                );
            }
        }
        Ok(success(payload))
    }
}
