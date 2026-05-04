# Findings

## Step 1 Notes

- Current HTTP service constructed `TeamModeToolset` once against service startup `base_dir`; project isolation needs request context to choose `<project>/.agent-teams`.
- Current JS hooks queried `/lead-pending/my-teams` with only `Authorization`; without `X-Team-Mode-Project-Root` they would continue to use the service fallback base dir.
- Codex worker MCP config also needs project-root env header, otherwise worker-originated MCP calls fall back to service-global data.
- Implemented request-scoped service wiring by cloning lightweight stores/services for caller project root while reusing the shared orchestrator/runtime/loop_handles.
- Tightened explicit `--data-dir` relay discovery so tests and explicit local runtime use do not accidentally attach to a live global runtime.

## Step 2 Notes

- `team_create` now distinguishes same-name active teams owned by the current lead PID, same-name active teams owned by a live different PID, and archived teams that can be revived in place.
- `overwrite=true` is modeled as a hard discard of every existing team under the caller project base dir before creating the new team.
- Archived revive keeps the same team directory and flips `status` back to Active while refreshing caller-supplied metadata.

## Step 3 Notes

- `team_delete` now has two modes: archive by default and explicit permanent removal when requested.
- Archive mode keeps the team directory and only flips `team.json` to `Archived`, so a later revive path can restore it.
- The tool response now surfaces `archived` and `deleted` so callers can tell which path ran without inferring from side effects.

## Step 4 Notes

- Service startup now spawns the lead watchdog as a tokio task rather than a second thread, so the durable HTTP service keeps ownership of the lifecycle loop.
- The watchdog now keeps a per-team consecutive dead strike map and only archives a team after three consecutive dead-owner observations.
- Auto-archive reuses the same cleanup path as manual delete, then emits `team.auto_archived_dead_owner` as the audit event when the grace window expires.
