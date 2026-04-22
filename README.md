<p align="center">
  <em>MCP server that lets Claude Code coordinate a team of AI worker agents — with true push from workers to the lead.</em>
</p>

<p align="center">
  <a href="https://github.com/jessepwj/agent-teams-mcp/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.85%2B-orange.svg">
  <img alt="MCP" src="https://img.shields.io/badge/protocol-MCP-purple.svg">
  <img alt="Tests" src="https://img.shields.io/badge/tests-282%20passing-brightgreen.svg">
</p>

# agent-teams-mcp

**`agent-teams-mcp`** is a [Model Context Protocol](https://modelcontextprotocol.io) server written in Rust that turns your Claude Code CLI into a **team lead** — it can spawn and coordinate worker agents (Claude Code, Codex, Gemini CLI) inside managed subprocesses, and route `@mention`-style messages between them.

The biggest differentiator: **true push notifications from workers back to the lead's terminal.** When a worker replies, the Claude Code Lead is automatically woken up (even from idle) and processes the message in a new turn — no polling, no manual `inbox_read`, no context switching required.

<p align="center">
  <img alt="Flow" src="https://img.shields.io/badge/worker--reply%20%E2%86%92%20lead-~50ms-brightgreen.svg">
  <img alt="Auth" src="https://img.shields.io/badge/API%20key%20auth-supported-brightgreen.svg">
</p>

---

## TL;DR

```bash
# 1. Build
cargo build --release --bin team_mode_mcp

# 2. Wire it into your Claude Code .mcp.json
cat > .mcp.json <<'EOF'
{
  "mcpServers": {
    "team-mode": {
      "command": "/abs/path/to/target/release/team_mode_mcp",
      "args": []
    }
  }
}
EOF

# 3. (Strongly recommended) Wire the FileChanged push hook
#    See docs/push-notifications.md
```

Then inside your Claude Code session:

```
> Please create a team called "demo" and add an alice worker.

Claude: [calls team_create(name="demo")]
        [calls worker_add(team="demo", name="alice", adapter="claude-code")]

> @alice analyze the logs in ./logs/*.log

Claude: [calls send_message(team="demo", text="@alice analyze ...")]

[~5 seconds later — you did nothing]

<system-reminder>
  alice (reply): I've analyzed 342 log lines. The critical errors are ...
</system-reminder>

Claude: Alice found critical errors. Let me look at them...
```

That last block happens **automatically** — you don't type, you don't `/mcp`, you don't call `inbox_read`. The worker's reply lands in your session the instant it arrives.

---

## Why does this exist?

Claude Code exposes MCP tool calls but by design does **not** auto-react to MCP `resources/updated` notifications — so a naive MCP server that puts worker replies in a "resource" gets no push. The [`Channels` API](https://code.claude.com/docs/en/channels) would solve this but requires claude.ai OAuth login; many people run Claude Code with an API key instead.

This project uses the officially documented **`FileChanged` + `asyncRewake`** hook combination to implement a server → client → session push that works under API-key auth:

```
Worker reply
    ↓
Rust MCP server appends a line to <base>/lead_pending.jsonl
    ↓                 (~50 ms later)
Claude Code's native FileChanged watcher triggers
    ↓
scripts/hooks/lead-pending-wake.js runs async, writes stderr, exits code 2
    ↓
asyncRewake wakes Claude with stderr injected as <system-reminder>
    ↓
Claude processes the reminder in a new turn
```

No polling, no token burn, no special login.

---

## Features

- **Minimal 8-tool MCP surface** — `team_create / team_list / team_delete / worker_add / worker_list / worker_remove / send_message / inbox_read`. That's it.
- **Unified member model** — identity + execution profile in one record; `worker_remove` soft-removes (keeps profile for fast-resume) while `worker_add` with `on_existing=reuse` picks it right back up.
- **Multi-backend workers** — each worker picks one of `claude-code`, `codex`, `gemini-cli`. The lead stays Claude Code (see [Codex as Lead](#codex-as-lead)).
- **True push to Claude Code lead** — via the documented `FileChanged + asyncRewake` hook chain; idle sessions get woken.
- **Pull fallback** — `inbox_read` tool always works for clients that don't set up the hook.
- **Strict `@mention` routing** — `send_message` rejects any unmatched `@handles` up-front and returns the active worker list so the caller can self-correct.
- **Ready-check on spawn** — `worker_add` blocks until the spawned agent emits its first `TurnComplete` or 5s elapses; clearly reports `starting` / `running` / `failed`.
- **Crash-visible `team_delete`** — returns a `shutdown_failures` array so the caller knows which processes might be orphans.
- **Self-documenting data dir** — a `README.md` is auto-generated inside the data directory on every startup and describes the layout.
- **282 unit tests, zero warnings.**

---

## Architecture

```
┌────────────────────────────────────────────────────────┐
│  Lead Agent  ( your Claude Code CLI session )          │
│                                                        │
│  .mcp.json loads team-mode  ─────► stdio JSON-RPC ────┐│
│  ~/.claude/settings.json    ─────► FileChanged hook ──┼┼── async
└───────────────────────────────────────────────────────┘│
                                                        │
┌──────────────────────────────────────────────────────▼─┐
│  team_mode_mcp  (Rust, this repo)                       │
│                                                         │
│  ┌─────────────────────────────────────────────────┐    │
│  │  MCP runtime — 8 tools + 5 resource URIs        │    │
│  ├─────────────────────────────────────────────────┤    │
│  │  Services                                        │    │
│  │   TeamService  MemberService  RoomService       │    │
│  │   MessageService (↓ lead_pending writer)        │    │
│  │   InboxService (computed from messages)         │    │
│  ├─────────────────────────────────────────────────┤    │
│  │  Storage                                         │    │
│  │   <base>/<team>/  team.json                      │    │
│  │                   members.json (v=1)             │    │
│  │                   room.json                      │    │
│  │                   messages.jsonl                 │    │
│  │   <base>/lead_pending.jsonl (cross-team push)    │    │
│  │   <base>/.locks/ (file locks)                    │    │
│  │   <base>/README.md (auto-generated)              │    │
│  ├─────────────────────────────────────────────────┤    │
│  │  RuntimeOrchestrator — spawns managed backends   │    │
│  │   ClaudeCodeBackend  CodexBackend  GeminiBackend │    │
│  │   (stdin NDJSON / JSON-RPC / subprocess)         │    │
│  └─────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
              │
              ▼ spawned as child processes
┌──────────────────────────────────────────────────────┐
│  Workers — each is a managed CLI subprocess          │
│                                                      │
│  alice (claude-code)    bob (codex)    …             │
└──────────────────────────────────────────────────────┘
```

**Data-flow highlights**:

- Worker ← lead: `send_message` writes to `messages.jsonl`; per-worker `AgentLoop` wakes on `InboxNotifier::notify` and injects the message into the worker's stdin.
- Worker → lead: worker's reply goes through `MessageService::send` with `Kind::Reply`; `LeadPendingWriter` appends it to `<base>/lead_pending.jsonl`; the Claude Code FileChanged hook picks it up and wakes the lead with the content as a `<system-reminder>`.

---

## MCP Tool Reference

| Tool | Required | Optional | Summary |
|---|---|---|---|
| `team_create` | `name` | `cwd` | Create a team; virtual `lead` member is auto-added. |
| `team_list` | — | — | List all teams. |
| `team_delete` | `name` | — | Shut down workers + delete team dir; returns `shutdown_failures` for orphans. |
| `worker_add` | `team`, `name` | `adapter`, `model`, `cwd`, `system_prompt`, `env`, `on_existing` | Spawn a worker. `on_existing` is REQUIRED when a profile already exists: `reuse` / `overwrite` / `error`. |
| `worker_list` | `team` | — | List active workers (lead excluded). |
| `worker_remove` | `team`, `name` | — | Soft-remove: process stopped, status=Removed, execution profile kept for fast-resume. |
| `send_message` | `team`, `text` | — | Send as lead. `text` MUST contain `@handles`, and ALL of them must match active workers — unmatched `@handles` fail the call with the list. |
| `inbox_read` | `team` | `limit`, `unread_only`, `auto_ack` | Pull-mode fallback for reading the lead's inbox. |

See [`docs/mcp-tools-reference.md`](docs/mcp-tools-reference.md) for full schemas.

---

## Installation

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp
cargo build --release --bin team_mode_mcp
```

The binary lives at `target/release/team_mode_mcp`. Copy it somewhere on your `PATH` or reference it by full path.

### Claude Code wiring

Add to `.mcp.json` at your project root (or `~/.claude/mcp.json` globally):

```json
{
  "mcpServers": {
    "team-mode": {
      "command": "/absolute/path/to/team_mode_mcp",
      "args": [],
      "env": {
        "RUST_LOG": "info"
      }
    }
  }
}
```

### FileChanged push hook (optional, strongly recommended)

See [`docs/push-notifications.md`](docs/push-notifications.md) for the full walkthrough. In short, add to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "FileChanged": [{
      "matcher": "lead_pending.jsonl",
      "hooks": [{
        "type": "command",
        "command": "node /abs/path/to/scripts/hooks/lead-pending-wake.js",
        "async": true,
        "asyncRewake": true
      }]
    }]
  }
}
```

Restart Claude Code (hook config is only loaded on startup). That's it — worker replies now appear in your session the moment they happen.

---

## Data Directory Layout

The MCP server creates `.agent-teams/` in the lead's CWD on startup:

```
.agent-teams/
├── README.md               ← auto-regenerated on each startup
├── lead_pending.jsonl      ← push queue, consumed by the FileChanged hook
├── .locks/                 ← file locks
└── <team-name>/
    ├── team.json
    ├── members.json        ← versioned; identity + execution profile
    ├── room.json
    └── messages.jsonl
```

A legacy data directory name `.team-mode-data/` is detected on startup and a warning is logged (not migrated — delete it manually).

---

## Development

```bash
# Compile check (fast, no link)
cargo check --lib

# Run the 282 unit tests
cargo test --lib

# Build the MCP binary
cargo build --bin team_mode_mcp
```

The design spec for the current storage layout lives at [`.plans/refactor-data-layout/spec.md`](.plans/refactor-data-layout/spec.md) — useful context when adding new stores or services.

---

## Codex as Lead

Short version: **not currently supported.** The `FileChanged + asyncRewake` trick is a Claude Code specific feature. Codex CLI has no equivalent hook and OpenAI has explicitly declined to add one ([openai/codex#8375](https://github.com/openai/codex/issues/8375)).

The only officially supported path for Codex-as-Lead is the `codex app-server` JSON-RPC mode — which would require building a harness around it (about 2000+ lines of Rust). Research / discussion welcome in the issue tracker.

Workers on Codex remain fully supported — the lead is what needs the hook, not the workers.

---

## Credits

This project is **derived from and builds on** [`github.com/ZhangHanDong/agent-teams-rs`](https://github.com/ZhangHanDong/agent-teams-rs) (MIT, © 2025 Zhang Han Dong), which provides the core runtime, backends, team/task/inbox domain, and CLI. This fork refocuses the project around the `team_mode_mcp` MCP server and adds:

- The `FileChanged + asyncRewake` push architecture
- A unified member file layout (`members.json` with merged identity + execution)
- Per-team subdirectory data layout with auto-generated `README.md`
- `worker_add on_existing`, strict `send_message`, `team_delete` failure reporting, `worker_add` ready-check
- The `inbox_read` pull-mode tool
- Hook scripts + user-facing documentation

---

## License

MIT — see [`LICENSE`](LICENSE).
