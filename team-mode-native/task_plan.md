# team-mode-native Gap Analysis & Implementation Plan

## Phase 1: Heartbeat Timeout Supervisor [DONE]

**Goal:** When runner heartbeat times out (>10s), mark Degraded; when restart_policy=always, auto-respawn.

**Tasks:**
- [x] Identify current state: runner_heartbeat updates last_seen_at but no background check exists
- [x] Add spawn_params to ManagedSessionSummary (host, token_env, open_terminal)
- [x] Add start_heartbeat_supervisor() → background tokio task every 5s
- [x] Supervisor logic: degraded detection + auto-restart dispatch
- [x] Call supervisor from run_local_ipc

## Phase 2: YAML Team Config Loading [DONE]

**Goal:** `teamctl load-config <file.yaml>` to batch-create team/members/profiles.

**Tasks:**
- [x] Add serde_yaml to Cargo.toml
- [x] Define TeamYamlConfig struct (team + members)
- [x] Add teamctl load-config subcommand
- [x] Call team/create + member/add + execution/set IPC

## Phase 3: Codex turn/steer + interrupt [DONE]

**Goal:** Send mid-turn steer or interrupt to running Codex app-server.

**Tasks:**
- [x] Change CodexRuntime channel to enum (Turn | Steer | Interrupt)
- [x] Add codex_steer() and codex_interrupt() to TeamModeHost
- [x] Add IPC dispatch: codex/steer, codex/interrupt
- [x] Add teamctl codex steer/interrupt subcommands
- [x] Add to MCP proxy tool list (codex_steer, codex_interrupt)

## Phase 4: Codex developer_instructions capability probe [DONE]

**Goal:** Test if current Codex version accepts collaborationMode.settings.developer_instructions.

**Tasks:**
- [x] Parse thread/start response for error about collaborationMode (probe_tx SyncSender in stdout reader, detects id=2 response)
- [x] If rejected: resend thread/start without collaborationMode, spin-wait for thread_id, send system_prompt as bootstrap turn
- [x] Log probe result as codex event (probe_success / probe_fallback / bootstrap_turn_sent)

## Decisions

- Heartbeat supervisor spawned from run_local_ipc (not TeamModeHost::new since not async)
- Supervisor holds host.clone() (Arc-based) and loops every 5s
- Auto-restart uses same params stored in ManagedSessionSummary
- YAML uses serde_yaml; fallback JSON also supported by same loader
