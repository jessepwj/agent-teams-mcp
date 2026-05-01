use super::*;
use std::sync::Arc;

use crate::team_mode::domain::{MemberKind, MemberStatus};
use crate::team_mode::mcp::resources::{inbox_uri, team_uri};
use crate::team_mode::runtime_workers::STATE_STOPPED;
use crate::team_mode::service::{AddMemberRequest, CreateTeamRequest};

use super::web_open::open_team_web_ui;

impl TeamModeToolset {
    pub(super) fn team_create(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let name = required_identifier(args, "name")?;
        if name == LEAD_NAME {
            return Err(Error::Other(
                "'lead' is reserved and cannot be used as a team name".into(),
            ));
        }
        // Strict slug check so the team name stays @mention-able and safe
        // for filesystem use across platforms. Catches uppercase, spaces,
        // unicode, leading dots/dashes, and >64-byte names — all of which
        // earlier silently slipped through and produced unreachable teams.
        crate::util::validate_slug_name(&name)?;

        // Bind this team to the CC process that owns this MCP caller. HTTP
        // service calls inject the already-resolved owner header; fallback
        // stdio calls use the same Rust ancestor walk. Push routing later
        // uses this to send worker replies only to the owner CC.
        let owner_cc_pid = optional_u32(args, "_owner_cc_pid")?.or_else(current_cc_pid);

        // Enforce: one project = at most one LIVE team at a time.
        //
        // Rationale: workers consume real resources (subprocesses, model
        // calls), and the lead CC is the coordinator. Allowing multiple
        // live teams per project would let a single CC spawn runaway work
        // by accident (forgetting to clean up old teams), and is the
        // natural model the user thinks in (one project, one team-in-flight).
        //
        // Behavior:
        //   - If any existing team has a still-alive owner_cc_pid → refuse
        //     with a clear error listing which team and who owns it. Caller
        //     must team_delete the existing one before creating a new one.
        //   - If only orphan teams exist (owner CC is dead) → auto-clean
        //     them and proceed. We report how many we reaped.
        //   - If no teams exist → proceed cleanly.
        let existing_team = self.team_service.get(&name)?;
        let cleaned_orphans = if existing_team.is_some() {
            Vec::new()
        } else {
            self.enforce_single_live_team(owner_cc_pid)?
        };

        let team = self.team_service.create(CreateTeamRequest {
            id: Some(name.clone()),
            name: name.clone(),
            description: None,
            cwd: optional_text(args, "cwd")?,
            lead_member_id: Some(LEAD_NAME.into()),
            owner_cc_pid,
        })?;

        if self.member_service.get(&team.id, LEAD_NAME)?.is_none() {
            self.member_service.add(AddMemberRequest {
                team_id: team.id.clone(),
                name: LEAD_NAME.into(),
                kind: MemberKind::Lead,
                role_label: "lead".into(),
                role_description: None,
                execution: None,
            })?;
        }

        self.room_service.ensure_main_room(&team.id)?;

        let web_status = open_team_web_ui(&self.base_dir, &team.id);
        let mut response = json!(team.clone());
        if let Value::Object(obj) = &mut response {
            obj.insert("web".into(), web_status.to_json());
            if !cleaned_orphans.is_empty() {
                obj.insert("cleaned_orphan_teams".into(), json!(cleaned_orphans));
            }
        }

        Ok(success_with_updates(
            response,
            vec![team_uri(&team.id), inbox_uri(&team.id, LEAD_NAME)],
        ))
    }

    /// Enforce the "one live team per project" invariant.
    ///
    /// Returns the list of orphan team names that were auto-cleaned, so the
    /// caller can surface them to the user. Fails hard if any LIVE team
    /// (owner_cc_pid still alive per sysinfo) already exists — the caller
    /// must explicitly delete that team first.
    fn enforce_single_live_team(&self, _caller_pid: Option<u32>) -> Result<Vec<String>> {
        use sysinfo::{Pid, ProcessesToUpdate, System};

        let existing = self.team_service.list()?;
        if existing.is_empty() {
            return Ok(Vec::new());
        }

        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::All, true);
        let is_live = |pid: u32| -> bool { sys.process(Pid::from_u32(pid)).is_some() };

        let mut live = Vec::new();
        let mut orphans = Vec::new();
        for team in &existing {
            match team.owner_cc_pid {
                Some(pid) if is_live(pid) => live.push((team.id.clone(), pid)),
                Some(_) => orphans.push(team.id.clone()),
                None => {
                    // Legacy / unbound team — treat conservatively as live
                    // (can't verify death, don't auto-purge user data).
                    live.push((team.id.clone(), 0));
                }
            }
        }

        if !live.is_empty() {
            let listed = live
                .iter()
                .map(|(name, pid)| format!("'{}' (owner_cc_pid={})", name, pid))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::Other(format!(
                "this project already has {} live team(s): {}. \
                 Only one live team per project is allowed — call `team_delete` \
                 on the existing team before creating a new one.",
                live.len(),
                listed
            )));
        }

        // Only orphans remain. Auto-clean them so the new team_create succeeds,
        // but report back so the caller can surface what happened.
        for orphan_id in &orphans {
            if let Err(err) = self.team_service.delete(orphan_id) {
                tracing::warn!(
                    team = %orphan_id,
                    error = %err,
                    "failed to delete orphan team during team_create cleanup; will surface error"
                );
                return Err(Error::Other(format!(
                    "failed to auto-clean orphan team '{orphan_id}': {err}. \
                     Delete it manually and retry."
                )));
            }
            // Best-effort: also drop the worker runtime record for the orphan.
            let _ = self.runtime_workers.remove_team(orphan_id);
        }
        if !orphans.is_empty() {
            tracing::info!(
                count = orphans.len(),
                teams = ?orphans,
                "auto-cleaned orphan teams before creating new one"
            );
        }
        Ok(orphans)
    }

    pub(super) fn team_delete(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "name")?;
        let mut shutdown_failures: Vec<Value> = Vec::new();

        // Best-effort shutdown for every managed worker; collect failures
        // so the caller can clean up orphan processes if any.
        //
        // Skip rules:
        //   - Members already in `Removed` status had `worker_remove` called
        //     earlier; their orchestrator slot was cleaned up at that point.
        //     Re-attempting shutdown only produces a noisy "session not
        //     registered" error in the response that confuses the lead.
        //   - "session not registered" / "MemberNotFound" failures during the
        //     loop are also benign — they just mean the orchestrator already
        //     forgot the spawn_key (daemon restart or earlier remove). We
        //     drop those from `shutdown_failures` so only real shutdown
        //     errors (a still-alive child the OS refused to kill) surface.
        if let Ok(members) = self.member_service.list_by_team(&team_name) {
            for record in members {
                if matches!(record.profile.status, MemberStatus::Removed) {
                    continue;
                }
                let key = spawn_key(&team_name, &record.profile.name);
                let orch = Arc::clone(&self.runtime_orchestrator);
                let key_clone = key.clone();
                let result = self.async_runtime.block_on(async move {
                    orch.lock().await.shutdown_managed_member(&key_clone).await
                });
                if let Err(err) = result {
                    let msg = err.to_string();
                    // Filter benign "this session was already gone" errors
                    // so the response only flags REAL shutdown failures
                    // (a still-alive child the OS refused to kill). The
                    // orchestrator surfaces session-not-found as either of
                    // these two strings depending on which call path missed:
                    //   - "no managed session registered for spawn_key '...'"
                    //   - "Member '...' not found in team 'runtime'" (legacy
                    //     path before the orchestrator dropped the "runtime"
                    //     placeholder team — kept for older daemons)
                    let is_already_gone = msg.contains("no managed session registered")
                        || (msg.contains("not found") && msg.contains("'runtime'"));
                    if !matches!(record.profile.kind, MemberKind::Lead) && !is_already_gone {
                        shutdown_failures.push(json!({
                            "team": team_name.clone(),
                            "member": record.profile.name,
                            "reason": msg,
                        }));
                    }
                }
                if let Some(h) = self.lock_loop_handles()?.remove(&key) {
                    let _ = h.shutdown_tx.send(());
                }
                if !matches!(record.profile.kind, MemberKind::Lead) {
                    let _ = self.runtime_workers.upsert_state(
                        &team_name,
                        &record.profile.name,
                        &key,
                        record.execution.and_then(|e| e.adapter),
                        STATE_STOPPED,
                        None,
                    );
                }
            }
        }

        self.team_service.delete(&team_name)?;
        let _ = self.runtime_workers.remove_team(&team_name);
        // Per-team pending file lives under `<base_dir>/<team_id>/` and is
        // removed by `team_service.delete()` (which calls TeamStore::delete
        // → fs::remove_dir_all on the team_dir). No separate prune needed.

        let result = json!({
            "ok": true,
            "name": team_name.clone(),
            "shutdown_failures": shutdown_failures,
        });

        Ok(success_with_updates(result, vec![team_uri(&team_name)]))
    }

    pub(super) fn member_store_members_file_hint(&self, team_name: &str) -> String {
        crate::team_mode::data_dir::members_file(&self.base_dir_of(), team_name)
            .to_string_lossy()
            .to_string()
    }

    fn base_dir_of(&self) -> PathBuf {
        self.base_dir.clone()
    }
}
