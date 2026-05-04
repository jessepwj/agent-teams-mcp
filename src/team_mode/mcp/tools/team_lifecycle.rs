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
        let overwrite = optional_bool(args, "overwrite")?.unwrap_or(false);

        let outcome = self.team_service.create_with_outcome(CreateTeamRequest {
            id: Some(name.clone()),
            name: name.clone(),
            description: None,
            cwd: optional_text(args, "cwd")?,
            lead_member_id: Some(LEAD_NAME.into()),
            owner_cc_pid,
            overwrite,
        })?;
        let team = outcome.team.clone();

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
            if outcome.revived {
                obj.insert("revived".into(), Value::Bool(true));
            }
            if let Some(restored_from) = outcome.restored_from {
                obj.insert(
                    "restored_from".into(),
                    Value::String(restored_from.display().to_string()),
                );
            }
            if !outcome.discarded_teams.is_empty() {
                obj.insert("discarded_teams".into(), json!(outcome.discarded_teams));
            }
        }

        Ok(success_with_updates(
            response,
            vec![team_uri(&team.id), inbox_uri(&team.id, LEAD_NAME)],
        ))
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
