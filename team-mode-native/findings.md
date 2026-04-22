# Gap Analysis Findings

## What's Implemented (from static review)

### Core Protocol [DONE]
- Team/member/room/thread/direct/inbox CRUD with JSONL persistence
- Transcript-first messaging, thread/inbox projections rebuilt on restart
- 28 MCP tools in proxy
- IPC: 40+ methods with caller authentication

### Managed Sessions [DONE]
- dry_run plan (prompt file + mcp config generation)
- Terminal spawn: wt.exe + powershell fallback (Windows), sh fallback (non-Windows)
- Codex app-server spawn: initialize/thread_start/turn_start
- member_session_status, member_attach, member_shutdown, member_restart

### Runner Protocol [DONE]
- runner_hello, runner_heartbeat (updates last_seen_at), runner_output
- runner_input_injected, runner_child_exit
- PTY bridge (team_member_runner binary)

## What's Missing

### Gap 1: Background heartbeat timeout supervisor [CRITICAL]
- runner_heartbeat handler exists but NO background poller
- No auto-Degraded after 10s silence
- No auto-restart for restart_policy=Always
- Mentioned in verification doc as explicitly not done

### Gap 2: YAML config loading [MEDIUM]
- Requirements section 6 shows YAML format for batch team creation
- No teamctl load-config command exists
- No serde_yaml dependency

### Gap 3: Codex turn/steer + interrupt [MEDIUM]
- Only turn/start is implemented
- Requirements section 5.3 and 11.3 mention steer/interrupt/resume
- CodexRuntime.tx is a plain String channel (can't distinguish message types)

### Gap 4: Codex developer_instructions probe [LOW]
- collaborationMode.settings.developer_instructions sent unconditionally
- No probe/fallback to bootstrap turn if Codex version doesn't support it
- Just logs turn_start_deferred when thread_id unknown, no protocol error handling

### Gap 5: macOS/Linux terminal (cosmetic) [LOW]
- #[cfg(not(windows))] just runs `sh -lc <command>` in current terminal
- Doesn't open new terminal window
- Requirements mention osascript (macOS) / gnome-terminal (Linux) template

## Notes

- Two separate CommandSpec types in domain vs adapters::terminal - not a bug, by design
- Cargo.toml edition="2024" requires rustc 1.85+ - documented in verification doc
- All 17 test assertions verified correct via static analysis
