# Findings

## Step 1 Notes

- Current HTTP service constructed `TeamModeToolset` once against service startup `base_dir`; project isolation needs request context to choose `<project>/.agent-teams`.
- Current JS hooks queried `/lead-pending/my-teams` with only `Authorization`; without `X-Team-Mode-Project-Root` they would continue to use the service fallback base dir.
- Codex worker MCP config also needs project-root env header, otherwise worker-originated MCP calls fall back to service-global data.
- Implemented request-scoped service wiring by cloning lightweight stores/services for caller project root while reusing the shared orchestrator/runtime/loop_handles.
- Tightened explicit `--data-dir` relay discovery so tests and explicit local runtime use do not accidentally attach to a live global runtime.
