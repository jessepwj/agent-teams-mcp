<p align="right">
  <a href="README.zh-CN.md">中文文档 →</a>
</p>

<p align="center">
  <em>An MCP server that turns Claude Code into a coordinator for a team of AI worker agents — with true push from workers back to the lead and a live web UI for observing the whole team.</em>
</p>

<p align="center">
  <a href="https://github.com/jessepwj/agent-teams-mcp/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.85%2B-orange.svg">
  <img alt="MCP" src="https://img.shields.io/badge/protocol-MCP-purple.svg">
  <img alt="Tests" src="https://img.shields.io/badge/tests-351%20passing-brightgreen.svg">
  <img alt="Push" src="https://img.shields.io/badge/worker--reply%20%E2%86%92%20lead-%7E50ms-brightgreen.svg">
</p>

# agent-teams-mcp

A Rust [MCP](https://modelcontextprotocol.io) server that lets your Claude Code session **lead** a team of AI workers (Claude Code, Codex, Gemini CLI). Worker replies are pushed back to the lead automatically as `<system-reminder>` injections — **no polling, no `inbox_read`, no manual checking**. A live web UI on `http://127.0.0.1:8787` shows the whole team chatting in real time.

## Install

Prerequisites: Rust 1.85+, Node.js 14+.

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp
cargo install --path .
team_mode_service install-global
```

Then **fully restart Claude Code** (close every window, relaunch). Run `/mcp` — you should see `team-mode connected`. After that, `cd` into any project and use it. Each project gets its own isolated team data.

> **The one pitfall to remember:** any edit to `.mcp.json` or `.claude/settings.json` needs a full CC restart. `/mcp reconnect` does NOT reload hooks. See [Troubleshooting](#troubleshooting).

## Why this exists

Claude Code exposes MCP tool calls but does not auto-react to MCP `resources/updated` notifications. The official [Channels API](https://code.claude.com/docs/en/channels) needs claude.ai OAuth — many people run Claude Code with an API key.

This project uses Claude Code's documented **Stop hook + `asyncRewake`** pattern to push worker replies all the way back into the lead's session:

```
Worker reply
  → service appends to .agent-teams/<team>/lead_pending.jsonl
  → Stop hook fires when CC's turn ends
  → hook drains the per-team file, writes stderr, exits 2
  → CC opens a new turn with the reply as a <system-reminder>
```

Median end-to-end latency: ~50 ms. No polling, no token burn.

## Architecture

```
┌───────────────────────────────────────────────────┐
│  Claude Code (your session) — the LEAD            │
│  .mcp.json  ──► HTTP MCP at 127.0.0.1:8786/mcp   │
│  .claude/   ──► Stop hook (asyncRewake)          │
└──────────────┬────────────────────────────────────┘
               │ Streamable HTTP
┌──────────────▼────────────────────────────────────┐
│  team_mode_service (durable localhost daemon)     │
│   • 8 MCP tools                                   │
│   • TeamService / MessageService / InboxService   │
│   • Worker subprocess orchestrator                │
│   • Web UI on :8787 (auto-opens)                  │
│   • Storage under .agent-teams/<team>/            │
└──────────────┬────────────────────────────────────┘
               │ spawns
   ┌───────────┼───────────┬──────────────┐
   ▼           ▼           ▼              ▼
 alice       bob        carol         lead-pending
(claude)   (codex)    (gemini)      (per-team queue)
```

The service stays up across Claude Code reconnects and owns all worker state. Stop it explicitly with `scripts/team-mode-service.ps1 stop` when you want it gone.

## What's in the box

- **8 MCP tools** — `team_create / team_list / team_delete / worker_add / worker_list / worker_remove / send_message / inbox_read`. That's the whole surface.
- **Push to the lead's terminal** — replies arrive as `<system-reminder>` automatically. No polling.
- **Live web UI** (`127.0.0.1:8787+`) — three-pane layout, per-sender colors, `@mention` highlighting, full Claude Code / Codex JSONL session transcripts, sticky composer for human input.
- **Multi-backend workers** — `claude-code` / `codex` / `gemini-cli`. Lead must stay Claude Code (see [Codex as Lead](#codex-as-lead)).
- **Strict routing** — `@mention` validation, caller-attributed senders, workers can only send into their bound team. No forging.
- **Per-project isolation** — one durable service hosts multiple CC sessions in different projects; teams never bleed across.
- **Worker liveness + revival** — `worker_remove` is soft-delete (profile kept). `worker_add on_existing=reuse` fast-resumes.
- **Mid-turn delivery** — when the lead is busy with tools, replies surface via `PostToolUse` hook within ~3 s instead of waiting for turn end.
- **351 unit tests, zero warnings.**

## MCP tools

| Tool | Required | Optional | Purpose |
|---|---|---|---|
| `team_create` | `name` | `cwd` | Create team; lead member auto-added. Auto-launches web UI. |
| `team_list` | — | — | List all teams with `ownerStatus`. |
| `team_delete` | `name` | — | Stop workers + remove team. Returns `shutdown_failures`. |
| `worker_add` | `team`, `name` | `adapter`, `model`, `cwd`, `system_prompt`, `env`, `on_existing` | Spawn worker. `on_existing` required if profile exists. |
| `worker_list` | `team` | — | List workers; dead ones marked, hint to revive. |
| `worker_remove` | `team`, `name` | — | Soft-remove; profile retained. |
| `send_message` | `team`, `text` | — | Send into the team room. `sender` derived from caller. `@mention` required. |
| `inbox_read` | `team` | `limit`, `unread_only`, `auto_ack` | Pull-mode fallback for audits. Not the canonical channel. |

Full schemas in [`.plans/agent-teams-v2/docs/02-current-system/mcp-tools-reference.md`](.plans/agent-teams-v2/docs/02-current-system/mcp-tools-reference.md).

## Backend matrix

| | `claude-code` | `codex` | `gemini-cli` |
|---|---|---|---|
| Persistent process | ✓ | ✓ | — (per-turn) |
| `session_id` capture | ✓ | ✓ (thread.id) | — |
| Web UI session transcript | ✓ | ✓ | — |
| Full-access mode | ✓ (`bypassPermissions`) | ✓ (`danger-full-access`) | n/a |
| Multi-turn memory | native | native | rolling 50-turn window |

Claude Code workers need `CLAUDE_CODE_GIT_BASH_PATH` on Windows (auto-detected from common Git paths; set manually if non-standard). On Windows MSVC, source `vcvars64.bat` before starting the service so workers inherit the MSVC linker — easiest way is to use `scripts/team-mode-service.ps1 start`.

## Troubleshooting

`send_message` returns success but no `<system-reminder>` arrives. Triage in this order:

1. **Did you fully restart Claude Code after install or after editing `.claude/settings.json`?** Hooks only load at CC startup. `/mcp reconnect` is not enough. Quit all CC windows, relaunch.
2. **Is the worker actually replying?** `tail -f ~/.team-mode/runtime/service.log` should show appends to `lead_pending`. If not, the backend CLI may be missing from PATH.
3. **Is the Stop hook firing?** `tail -f .agent-teams/.lead-pending-wake.log` should show injection lines. None → hook not loaded → step 1.
4. **Still stuck?** See [`.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md`](.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md) for the full triage table (15+ scenarios).

## Codex as Lead

Not currently supported. The Stop-hook push is a Claude Code feature — Codex CLI has no equivalent blocking hook ([openai/codex#8375](https://github.com/openai/codex/issues/8375)).

Codex as a **worker** is fully supported with full session-transcript parity in the web UI.

## Development

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp
bash scripts/setup.sh    # or: powershell -ExecutionPolicy Bypass -File scripts\setup.ps1
cargo test --lib         # 351 tests, ~1s
cargo build --release --bin team_mode_service
```

Adding a backend? See `src/backend/{claude_code,codex,gemini}.rs` for reference impls. `AgentLoop` drives all backends uniformly. Read [`CONTRIBUTING.md`](CONTRIBUTING.md) for conventions.

Design docs worth reading:
- [`.plans/agent-teams-v2/decisions.md`](.plans/agent-teams-v2/decisions.md) — ADRs for current HTTP service + async wake
- [`.plans/agent-teams-v2/docs/05-design-history/hook-push-design.md`](.plans/agent-teams-v2/docs/05-design-history/hook-push-design.md) — Stop hook design rationale

## Credits

Derived from and builds on [`github.com/ZhangHanDong/agent-teams-rs`](https://github.com/ZhangHanDong/agent-teams-rs) (MIT, © 2025 Zhang Han Dong), which provides the core runtime, backends, and domain. This fork adds:

- The Stop-hook + `asyncRewake` push architecture
- A durable localhost HTTP service that survives Claude Code reconnects
- The live web UI (per-sender colors, session transcripts, human-in-the-loop)
- Per-team data layout, unified `members.json` v=1, caller-attributed senders
- Strict `send_message` routing, worker revival, mid-turn delivery, per-project isolation

## License

MIT — see [`LICENSE`](LICENSE).
