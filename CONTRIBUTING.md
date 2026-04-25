# Contributing to agent-teams-mcp

Thanks for your interest! This project is a Rust MCP server that turns Claude Code into a coordinator for AI worker teams. Contributions are welcome — bug reports, fixes, new backends, docs improvements, the lot.

## Quick start

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp

# Bootstrap (cross-platform)
bash scripts/setup.sh                 # POSIX / Git-Bash
# or:  powershell -ExecutionPolicy Bypass -File scripts\setup.ps1   # Windows

# Then iterate:
cargo check --lib                     # fast compile check
cargo test --lib                      # 300 unit tests, <2s
cargo build --release --bin team_mode_mcp --bin team_mode_daemon
```

After you change Rust code, rebuilding is enough — `/mcp reconnect` in Claude Code spawns a fresh `team_mode_mcp` from the cached path. **But after you change `.mcp.json` or `.claude/settings.json`, you must fully restart Claude Code** (close all CC windows + relaunch) — hooks are loaded only at CC startup.

## Project layout

```
src/
├── bin/                      ← entrypoints (team_mode_mcp, team_mode_daemon, team_mode_web)
├── backend/                  ← per-CLI integrations (claude_code, codex, gemini_cli)
├── team_mode/
│   ├── domain/               ← team / member / message / room types
│   ├── service/              ← TeamService, MemberService, MessageService, ...
│   ├── storage/              ← persistence (.agent-teams/<team>/*.json)
│   ├── runtime/              ← agent_loop, runtime_orchestrator
│   └── mcp/tools.rs          ← the 8-tool MCP surface
├── team_mode_daemon/         ← detached daemon (IPC server, lifecycle)
├── team_mode_web/            ← read-only web UI (served from daemon)
└── util/                     ← session_discovery, codex_session_discovery, ...
scripts/
├── hooks/lead-pending-wake.js  ← Stop hook that pushes worker replies into CC
├── setup.sh / setup.ps1        ← post-clone bootstrap
docs/                            ← user-facing guides
.plans/refactor-data-layout/spec.md   ← design spec for current storage layout
```

For the architecture overview see [`README.md`](README.md) and [`docs/team-mode-mcp-final.md`](docs/team-mode-mcp-final.md).

## Where to add things

| Want to | Look at |
|---|---|
| Add a new backend (e.g. `cursor`) | `src/backend/<name>.rs` — implement the `Backend` trait; register in `src/backend/mod.rs`. AgentLoop drives all backends uniformly. |
| Change MCP tool schema / behaviour | `src/team_mode/mcp/tools.rs` (8 handlers). Keep operational guidance in **runtime hint fields** on responses, not in static descriptions — see [`docs/usage-tips.md`](docs/usage-tips.md) §3. |
| Change message routing rules | `src/team_mode/service/message_service.rs` — keep unit tests in sync. |
| Change storage layout | `src/team_mode/storage/*` + bump version field + handle migrations. Existing format is documented in [`.plans/refactor-data-layout/spec.md`](.plans/refactor-data-layout/spec.md). |
| Touch the Stop hook | `scripts/hooks/lead-pending-wake.js`. **Read [`docs/hook-push-design.md`](docs/hook-push-design.md) first** — there are non-obvious invariants (loop prevention, ESC handling, ancestor routing). |
| Touch web UI | `src/team_mode_web/` (Rust handlers) + `web/team-mode/` (JS / HTML / CSS). Smoke test: `cd web/team-mode && node app.smoke.test.mjs`. |

## Testing

- `cargo test --lib` — 300 unit tests, all under 2 seconds.
- For end-to-end push verification:
  1. Run `bash scripts/setup.sh`
  2. Launch CC from the repo root
  3. `team_create({"name":"smoke"})` → `worker_add({"team":"smoke","name":"alice","adapter":"claude-code"})` → `send_message({"team":"smoke","text":"@alice say hi"})`
  4. End your turn — alice's reply should appear as a `<system-reminder>` within ~5 seconds
  5. `team_delete({"name":"smoke"})`
- Python E2E driver: `python mcp_e2e.py` (drives the MCP over stdio, validates the full chain).

## Coding conventions

- **Errors are explicit.** Use the `Error` enum + `?` operator. Avoid `.unwrap()` outside tests.
- **No mutation** of data structures — use spread / clone patterns.
- **Small files** (~200-400 lines), small functions (<50 lines).
- **No Unicode glyphs** in code output (logs / hints / printlns) — use ASCII tags like `[OK]` / `[FAIL]` for cross-platform safety (Windows code pages).
- **Operational guidance** belongs in tool **response** `hint` fields (just-in-time), not in static tool descriptions. The latter waste tokens and miss the moment when guidance matters.
- **No hardcoded config defaults** that silently override the user's global config (`~/.codex/config.toml`, etc.). Make fields `Option<T>` and let the downstream CLI fall through to its own config.

## Before opening a PR

1. `cargo test --lib` passes
2. `cargo clippy -- -D warnings` clean (or note the exception)
3. `cargo fmt --check` clean
4. Run `bash scripts/setup.sh` to confirm a fresh-clone user would succeed
5. If you changed user-visible behaviour, update [`docs/usage-tips.md`](docs/usage-tips.md) and the README
6. If you fixed a bug worth remembering, add a one-line note to the bug journal in [`docs/design-decisions.md`](docs/design-decisions.md)

## License

By contributing, you agree your contribution is licensed under MIT (same as the project).
