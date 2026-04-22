# Session Progress

## 2026-04-21 (Session 1-2)

### Bug fixes
- Bug 1: dead code `create_message` removed (no persistence bug risk)
- Bug 2: `format_injected_message` uses lowercase `message_kind_str()` instead of `{:?}`
- Import cleanup: app.rs and local_ipc.rs

### Phase 1: Heartbeat timeout supervisor [DONE]
- `ManagedSessionSummary`: +spawn_host, +spawn_token_env, +spawn_open_terminal
- `TeamModeHost::start_heartbeat_supervisor()`: tokio task, polls every 5s
- `check_runner_heartbeats()`: 10s threshold → Degraded; restart_policy=Always → auto-respawn
- `run_local_ipc`: calls `host.start_heartbeat_supervisor()` on startup

### Phase 2: YAML config loading [DONE]
- `Cargo.toml`: serde_yaml = "0.9"
- `teamctl load-config <file.yaml|json>`: batch create team + members + execution profiles
- Structs: `TeamYamlConfig` / `TeamYamlDef` / `MemberYamlDef` / `MemberYamlCommand`

### Phase 3: Codex steer/interrupt [DONE]
- `CodexMsg` enum: Turn(String) / Steer(String) / Interrupt
- `CodexRuntime.tx`: changed from `Sender<String>` to `Sender<CodexMsg>`
- `codex_steer()` + `codex_interrupt()` host methods
- IPC dispatch: `codex/steer` + `codex/interrupt`
- `teamctl codex steer/interrupt` subcommands
- MCP proxy: `codex_steer` + `codex_interrupt` tools

### Phase 4: Codex developer_instructions probe [DONE]
- `spawn_codex_pipe_logger`: +`probe_tx: Option<SyncSender<bool>>` param
  - Detects `thread/start` response (id==2 as u64 or "2" as str)
  - Sends true (success) or false (error) to stdin writer
- Stdin writer thread:
  - Creates `(probe_tx, probe_rx)` pair when developer_instructions.is_some()
  - After `write_codex_initialize`, waits `probe_rx.recv_timeout(5s)`
  - Success → log `probe_success`
  - Failure → log `probe_fallback`, retry `thread/start` (with cwd, without collaborationMode),
    spin-wait 3s for thread_id, send system_prompt as bootstrap turn
  - Events logged: `probe_success` / `probe_fallback` / `bootstrap_turn_sent`

### Structured logging [DONE]
- All bin files: `tracing_subscriber::fmt()` with env_filter, target, thread_ids, file, line_number
- `local_ipc.rs`: ipc server listening / new connection / runner registered/disconnected / ipc error
- `app.rs`: team created/deleted, member added/removed, spawn/shutdown managed session,
  runner registered/disconnected, heartbeat timeout→Degraded, auto-restart, message delivery,
  codex probe_success/probe_fallback
- `team_member_runner.rs`: connect success/fail, PTY spawn, message inject

---

## 2026-04-22 (Session 3)

### Build & test [DONE]
- `cargo build --release` succeeded (1 warning: unused imports fixed)
- Fixed: `local_ipc.rs` removed unused `CodexInterruptRequest, CodexSteerRequest` imports
- `cargo check` clean: 0 warnings, 0 errors

### MCP config updated [DONE]
- `E:\aigc内容整理\agent-teams-rs-team-mode\.mcp.json` updated to point to new project:
  - command: `team-mode-native\target\release\team_mode_mcp_proxy.exe`
  - args: `--host 127.0.0.1:17891 --member-id operator`
- Also created `team-mode-native\.mcp.json` (same content, for project-local use)

### Live test results [ALL PASS]
- Host starts, structured logs output correctly with timestamps/thread/file/line
- team create/list [OK]
- member add/list [OK]
- room post dispatch → effectiveRecipients correctly resolved [OK]
- inbox peek/count/read/ack [OK]
- member tail → [TEAM MODE MESSAGE] format with kind=dispatch [OK]
- thread read/reply [OK]
- dm send/list [OK]
- managed session dry-run → prompt file + mcp.json generated correctly [OK]
- member attach → returns teamctl tail viewer command [OK]
- MCP proxy tools/list → 31 tools listed [OK]
- Persistence: Host restart recovers team/member/messages/inbox/read+ack state from JSONL [OK]

### Tool count breakdown (31 total)
- Team (4): create/get/list/delete
- Member (6): add/get/update/remove/list + execution_profile_set
- Room (3): post_message/read_messages/list
- Thread (2): read/reply
- Direct (4): send/read/reply/list
- Inbox (4): peek/read/ack/count
- Session (6): spawn/shutdown/restart_managed + session_status/output_tail/attach
- Codex (2): steer/interrupt

---

## Current state: COMPLETE

All 4 implementation phases done. All Section 14 acceptance criteria verified live.

### Remaining (non-blocking, not in Section 14)
- `thread/resume` for Codex: resumes existing thread after Host restart (needs real Codex CLI)
- macOS/Linux terminal launcher: osascript/gnome-terminal template (Windows-first, sh fallback exists)
- `codex_auto_reply` fallback: auto-write Codex output back to thread (low priority)

### Key file locations
- Host core: `src/host/app.rs` (~2200 lines)
- IPC layer: `src/host/local_ipc.rs`
- MCP proxy: `src/mcp/proxy.rs`
- CLI tool: `src/bin/teamctl.rs`
- Binaries: `target/release/team_mode_host.exe`, `teamctl.exe`, `team_mode_mcp_proxy.exe`, `team_member_runner.exe`, `codex_viewer.exe`
- Verification doc: `docs/full-permission-verification.md`
- MCP config (outer project): `E:\aigc内容整理\agent-teams-rs-team-mode\.mcp.json`

### To run
```powershell
# Terminal A - Host
cd team-mode-native
./target/release/team_mode_host.exe --data-dir .team-mode --listen 127.0.0.1:17891

# Terminal B - CLI
./target/release/teamctl.exe --host 127.0.0.1:17891 status
```

### RUST_LOG levels
```powershell
$env:RUST_LOG="team_mode_native=debug"   # verbose
$env:RUST_LOG="info"                      # default
$env:RUST_LOG="team_mode_native::host::app=debug,info"  # host core only
```
