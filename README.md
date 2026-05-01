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
  <img alt="Tests" src="https://img.shields.io/badge/tests-300%20passing-brightgreen.svg">
</p>

# agent-teams-mcp

`agent-teams-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server, written in Rust, that turns your Claude Code CLI into a **team lead**. The lead can spawn and coordinate AI worker agents (Claude Code, Codex, or Gemini CLI) as managed subprocesses, route `@mention`-style messages between them, and — critically — **receive worker replies as automatic `<system-reminder>` injections in its next turn, with no polling and no manual `inbox_read`**.

A web UI (auto-launched at `http://127.0.0.1:8787`) renders the live chat between the lead and every worker, so you can watch the team work in real time and even chime in as a human.

<p align="center">
  <img alt="Push latency" src="https://img.shields.io/badge/worker--reply%20%E2%86%92%20lead-%7E50ms-brightgreen.svg">
  <img alt="Auth" src="https://img.shields.io/badge/API%20key%20auth-supported-brightgreen.svg">
  <img alt="Daemon" src="https://img.shields.io/badge/detached%20daemon-survives%20%2Fmcp%20reconnect-brightgreen.svg">
</p>

---

## TL;DR — fresh clone in one command

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp

# Cross-platform bootstrap: builds the HTTP service, generates .mcp.json,
# and runs 300 unit tests.
bash scripts/setup.sh
# or:  powershell -ExecutionPolicy Bypass -File scripts\setup.ps1

# Then:
claude   # launch Claude Code from the repo root
```

Inside the Claude Code session, run `/mcp` — you should see `team-mode` connected. The web UI auto-opens on `team_create`.

> **⚠ The one critical pitfall:** any change to `.mcp.json` or `.claude/settings.json` requires a **full Claude Code restart** (close all CC windows + relaunch). `/mcp reconnect` does NOT reload hook configuration. If worker replies stop arriving as `<system-reminder>` after a config change, this is almost always why. See [§ Troubleshooting](#troubleshooting--worker-replies-arent-pushing) for the full triage.

---

## Why this exists

Claude Code exposes MCP tool calls but, by design, does not auto-react to MCP `resources/updated` notifications. So a naive MCP server that puts worker replies in a "resource" gets no push back to the lead's terminal. The official [`Channels` API](https://code.claude.com/docs/en/channels) would solve this but requires claude.ai OAuth login; many people run Claude Code with an API key.

This project uses the documented **Stop hook + `asyncRewake`** pattern to implement a service → client → session push that works under API-key auth:

```
Worker reply
    ↓
team_mode_service appends a JSON line to .agent-teams/<team>/lead_pending.jsonl
    ↓
Claude Code's Stop hook (project-level, .claude/settings.json) fires when CC's turn ends
    ↓
scripts/hooks/lead-pending-async-wake.js asks the service for this CC's teams
   and atomically drains the matching per-team pending files
    ↓
On hit: writes the reply to stderr and exits 2
    ↓
CC enters a NEW turn with the reply injected as a <system-reminder>
    ↓
Claude reads the reminder and continues working
```

No polling, no token burn, no special login. Median end-to-end latency: ~50 ms.

> **Historical note:** earlier versions used a long synchronous Stop-hook shepherd loop and a stdio MCP relay. ADR-020 moved the default control plane to a durable localhost Streamable HTTP service, and ADR-022 moved worker-reply wakeups to `asyncRewake` with per-team pending files. The old stdio `team_mode_mcp` + `team_mode_daemon` path is kept only as a legacy rollback / fallback route.

---

## What you get

- **Tiny MCP surface (8 tools)** — `team_create / team_list / team_delete / worker_add / worker_list / worker_remove / send_message / inbox_read`. That's it.
- **Durable HTTP service architecture** — `team_mode_service` is a long-lived localhost Streamable HTTP MCP service on `127.0.0.1:8786/mcp`. Claude Code connects through `.mcp.json` + `scripts/mcp-http-headers.js`, while the service owns worker subprocesses and the web UI. The old stdio `team_mode_mcp` + `team_mode_daemon` pair remains documented only as a legacy rollback / fallback path.
- **True push to the lead's terminal** — Stop hook `asyncRewake` + per-team pending-file routing surface worker replies as `<system-reminder>` automatically. Idle CC sessions wake up when the next turn boundary arrives.
- **Live web UI on 127.0.0.1:8787+** — three-pane layout (teams list / group chat / session details). Per-sender colors, `@mention` highlighting, click-to-filter, full Claude Code & Codex JSONL session transcripts, and a sticky composer so a human user can type into the team as a peer (lead included). Auto-opens on `team_create`.
- **Multi-backend workers** — `claude-code`, `codex`, `gemini-cli`. The lead must remain Claude Code (see [Codex as Lead](#codex-as-lead)).
- **Strict `@mention` routing** — `send_message` rejects unmatched handles up-front and returns the active worker list (always including `@lead`) so the caller can self-correct. Matching is case-insensitive (`@Alice` finds worker `alice`). Workers with no `@mention` default to `@lead`.
- **Caller-attributed messaging (Bug 29)** — `send_message` derives `sender` from HTTP caller identity, not from a parameter. Claude Code headers → `sender = "lead"`; worker env-backed HTTP headers → `sender = <worker name>`. Workers can only send into their bound team. No forging.
- **Explicit-only worker replies (Bug 29)** — workers MUST call `mcp__team-mode__send_message` to communicate. Their stdout (LLM thinking, codex shell output, ANSI escapes) is treated as private working notes and never copied into messages. If a worker finishes a turn without calling the tool, the lead receives a `[SYSTEM]` "completed turn without sending message" notice.
- **Strict slug validation** — worker / team names match `[a-z0-9_.-]{1,64}` (must start lowercase letter or digit). Names that can't be `@mention`ed are rejected on creation, not silently broken later.
- **Worker liveness + revival** — `worker_remove` is a soft delete (process stopped, profile retained for fast resume). `worker_add` with `on_existing=reuse` brings a worker back. Dead workers are detected via OS process check; the response includes a `hint` telling you exactly what to do.
- **Per-dispatch terminal-message guarantee** — every inbox dispatch produces exactly one terminal message back to the lead. Silent turn → `[SYSTEM] worker 'X' completed its turn without producing any reply text`. Pipe close mid-turn → `[SYSTEM] ... output channel closed mid-turn`. Lead never has to poll to know whether a worker actually finished.
- **Mid-turn message delivery (PostToolUse hook)** — worker replies arriving while the lead is busy in a long turn show up via `additionalContext` after the next tool call (~3s typical), not at turn end. Falls back to the Stop hook's `decision:"block"` path when the lead is purely thinking with no tool activity.
- **Multi-layer worker liveness** — three independent paths flip a worker to "dead" on external kill: (1) per-turn 3s active probe inside the `agent_loop` produces a clean `[SYSTEM] OutputClosed` even when Windows pipes don't EOF, (2) a daemon-side 5s watchdog reconciles `runtime/workers.json` so the web UI shows dead status without an MCP roundtrip, (3) the `worker_list` tool always queries the orchestrator live.
- **Just-in-time runtime hints** — operational guidance is delivered in tool response `hint` / `note` / `dead_recipients_hint` fields when relevant, not buried in static tool descriptions. Tool descriptions are kept tight (~700 chars each) so they don't crowd context.
- **Crash-visible `team_delete`** — returns a `shutdown_failures` array so the caller knows which subprocesses might be orphans.
- **Stop-hook batch-grace** — near-concurrent worker replies are coalesced into a single reminder (default 500 ms window, `TEAM_MODE_STOP_BATCH_GRACE_MS`).
- **One-live-team-per-project enforcement** — `team_create` rejects creating a second live team while another's `owner_cc_pid` is still alive; orphan teams from dead CC sessions are auto-cleaned and reported in `cleaned_orphan_teams`.
- **Self-documenting data dir** — a `README.md` is auto-regenerated inside `.agent-teams/` on every daemon startup, describing the on-disk layout.
- **300 unit tests, zero warnings.**

---

## Architecture

```
┌────────────────────────────────────────────────────────────┐
│  Claude Code (your CLI session) — the LEAD                 │
│                                                            │
│  .mcp.json          ──► http://127.0.0.1:8786/mcp        ││
│  .claude/           ──► Stop hook = asyncRewake script    ││
│   settings.json                                           ││
└───────────────────────────────────────────────────────────┘│
                                                            │ Streamable HTTP MCP
┌─────────────────────────────────────────────────────────────┐
│  team_mode_service(.exe) (THIS REPO — durable localhost)   │
│   ┌────────────────────────────────────────────────────┐   │
│   │  MCP runtime — 8 tools                             │   │
│   ├────────────────────────────────────────────────────┤   │
│   │  Services                                          │   │
│   │   TeamService   MemberService   RoomService        │   │
│   │   MessageService  →  LeadPendingWriter             │   │
│   │   InboxService   (computed from messages.jsonl)    │   │
│   ├────────────────────────────────────────────────────┤   │
│   │  RuntimeOrchestrator — owns worker subprocesses    │   │
│   │   ClaudeCodeBackend   CodexBackend   GeminiBackend │   │
│   ├────────────────────────────────────────────────────┤   │
│   │  Storage (.agent-teams/)                           │   │
│   │   <team>/  team.json members.json(v=1)             │   │
│   │           room.json messages.jsonl                 │   │
│   │   runtime/http-mcp.json runtime/workers.json       │   │
│   │   .locks/ README.md (auto-generated)               │   │
│   ├────────────────────────────────────────────────────┤   │
│   │  team_mode_web — read-only web UI on :8787+        │   │
│   │   served from inside the daemon, auto-opens        │   │
│   └────────────────────────────────────────────────────┘   │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼ spawned as child processes
┌─────────────────────────────────────────────────────────────┐
│  Workers — each is a managed CLI subprocess                │
│   alice (claude-code)    bob (codex)    carol (gemini-cli) │
└─────────────────────────────────────────────────────────────┘
```

**Why a service?** ADR-020 retired the old default stdio relay because stdin EOF and ESC handling made MCP lifetime unreliable on Windows. The durable HTTP service stays up across Claude Code reconnects and owns all worker state. Stop it explicitly with `scripts/team-mode-service.ps1 stop` when you want it gone. The old stdio `team_mode_mcp` + `team_mode_daemon` implementation is retained only as a legacy rollback / fallback route.

**Data flow**:
- Lead → worker: `send_message` writes to `messages.jsonl`. The worker's `AgentLoop` wakes via `InboxNotifier` and injects the message into the worker's stdin.
- Worker → lead: worker's reply enters `MessageService::send` with `Kind::Reply`. `LeadPendingWriter` appends it to `.agent-teams/<team>/lead_pending.jsonl`. The `asyncRewake` Stop hook drains that per-team file and wakes Claude Code with a `<system-reminder>`.

---

## MCP tool reference

| Tool | Required | Optional | Summary |
|---|---|---|---|
| `team_create` | `name` | `cwd` | Create a team; virtual `lead` member auto-added. Auto-cleans orphan teams from dead CCs and returns them in `cleaned_orphan_teams`. Auto-launches the web UI for the team. |
| `team_list` | — | — | List all teams. Each team is decorated with `ownerStatus`: `alive` / `orphan` / `unbound`. |
| `team_delete` | `name` | — | Shut down all workers + delete team dir. Returns `shutdown_failures: [{member, reason}]` for any worker that didn't shut cleanly. |
| `worker_add` | `team`, `name` | `adapter`, `model`, `cwd`, `system_prompt`, `env`, `on_existing` | Spawn a worker. `on_existing` is **required when a profile already exists**: `reuse` (fast-resume saved profile) / `overwrite` (replace it; `adapter` required) / `error` (default fail-fast). On dead worker reuse, returns `revived_from_dead: true`. |
| `worker_list` | `team` | — | List workers (lead excluded). Marks dead workers with `sessionState: "dead"` and surfaces a `hint` telling you to revive with `worker_add on_existing=reuse`. |
| `worker_remove` | `team`, `name` | — | Soft-remove: process stopped, status = `Removed`, execution profile **kept** for fast-resume later. |
| `send_message` | `team`, `text` | — | Send into the team room; `sender` derived from caller identity (lead's relay → `"lead"`; a worker's relay → that worker, env-injected at `worker_add`). `text` SHOULD contain `@handles` — workers default to `@lead` when omitted, lead must specify; unmatched handles fail with the available handle list (always includes `@lead`). Workers can only send into their bound team. Mixed live/dead recipient lists return `dead_recipients_hint` plus `[SYSTEM]` notices delivered to the lead's inbox. |
| `inbox_read` | `team` | `limit`, `unread_only`, `auto_ack` | Pull-mode fallback for the lead's inbox. **Not the canonical channel** — replies arrive automatically via the Stop hook; `inbox_read` is for backlog audits only. |

Full schemas in [`.plans/agent-teams-v2/docs/02-current-system/mcp-tools-reference.md`](.plans/agent-teams-v2/docs/02-current-system/mcp-tools-reference.md).

---

## Web UI

The service runs an embedded read-only web server on `127.0.0.1:8787` (auto-increments to 8799 on port conflicts). It auto-opens in your default browser when you call `team_create` (disable with `TEAM_MODE_WEB_AUTO_OPEN=0`).

**Layout**: three panes — left (team / member / filter list), center (group chat timeline), right (session / details / diagnostics tabs).

**Group chat**: chat-bubble style. Sender avatar + name + time + text body. `@mention` tokens are highlighted and clickable to filter the timeline. `[SYSTEM]` status messages render as centered grey notices.

**Per-sender colors**: `lead` is fixed cyan, `user` is fixed warm orange, workers get a stable djb2-hash color (skipping reserved hue ranges so workers never collide with lead/user).

**Session transcripts** (right pane): the actual Claude Code or Codex JSONL session content for the focused member, organized into "work turns" — tool calls paired with their results, final reply highlighted. Each worker's `session_id` is captured from its backend stream (Claude Code `init` / `result` events; Codex `thread.id`) and used for precise session lookup. Codex rollouts under `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` are parsed natively (5 s TTL cache).

**Human-in-the-loop messaging**: a sticky composer at the bottom lets you, the human, send messages into any team room as `@mention`s — sender name is the reserved `user` handle. Workers reply to you the same way they reply to the lead. The lead also sees these messages (via the lead-observability rule).

For the design rationale and feature roadmap see [`.plans/agent-teams-v2/docs/04-web-ui/team-mode-web-guide.md`](.plans/agent-teams-v2/docs/04-web-ui/team-mode-web-guide.md) and [`.plans/agent-teams-v2/docs/04-web-ui/history/web-frontend-plan.md`](.plans/agent-teams-v2/docs/04-web-ui/history/web-frontend-plan.md).

---

## Backend matrix

| Capability | `claude-code` | `codex` | `gemini-cli` |
|---|---|---|---|
| Persistent process | ✓ (NDJSON stream-json) | ✓ (`codex app-server` JSON-RPC) | — (per-turn respawn) |
| `session_id` capture | ✓ | ✓ (thread.id) | — |
| Web UI session transcript | ✓ | ✓ (rollout JSONL) | — (mtime fallback only) |
| Full-access mode | ✓ (`--permission-mode bypassPermissions`) | ✓ (`sandbox_mode = "danger-full-access"`) | n/a |
| System prompt mechanism | `--system-prompt` flag | Prepended to first user message | `System:` prefix in every constructed prompt |
| Conversation memory across turns | Native (single process) | Native (single process) | In-memory rolling window (last 50 turns) |

Notes:
- **Claude Code workers** require `CLAUDE_CODE_GIT_BASH_PATH` on Windows. The MCP relay auto-detects it from common Git install paths at startup; set the env var manually if your install is non-standard.
- **Codex workers** are spawned with `approvalPolicy: "never"` and `sandbox_mode: "danger-full-access"` so they don't block waiting for permission prompts. The reasoning effort field is intentionally not hardcoded; it falls through to your `~/.codex/config.toml` if set.
- **Gemini workers** lack a persistent session, so the web UI cannot show their JSONL transcript. Their conversation history is reconstructed in memory from `messages.jsonl` for each turn.

---

## Installation

### Recommended: setup script

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp

bash scripts/setup.sh
# or:  powershell -ExecutionPolicy Bypass -File scripts\setup.ps1
```

The setup script:
1. Verifies prerequisites (cargo 1.85+, node 14+).
2. Builds the release binary: `team_mode_service(.exe)`.
3. **Generates `.mcp.json` from `.mcp.json.template`** pointing at `http://127.0.0.1:8786/mcp` and `scripts/mcp-http-headers.js`.
4. Runs `cargo test --lib` (300 tests).
5. Prints next steps.

> **Why a generated `.mcp.json`?** `.mcp.json` is machine-local and gitignored. The tracked `.mcp.json.template` points Claude Code at the local HTTP MCP endpoint and uses `scripts/mcp-http-headers.js` to attach the runtime token and owner headers. Re-run setup after moving the repo so the helper path is correct, then fully restart Claude Code.

### Manual install

```bash
cargo build --release --bin team_mode_service
```

Then copy `.mcp.json.template` to `.mcp.json`, start the service, and fully restart Claude Code:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\team-mode-service.ps1 start
```

`.mcp.json` should contain the HTTP endpoint:

```json
{
  "mcpServers": {
    "team-mode": {
      "type": "http",
      "url": "http://127.0.0.1:8786/mcp",
      "headersHelper": "node scripts/mcp-http-headers.js"
    }
  }
}
```

The stdio `team_mode_mcp` + `team_mode_daemon` install path is a legacy rollback / fallback path only; do not use it for the default install.

### Worker cargo commands on Windows MSVC

Codex workers are child processes of `team_mode_service`. On Windows MSVC targets, `rustc` may fail to discover Visual Studio from that child process and can accidentally call Git Bash's `link.exe`, causing errors such as `link.exe was not found` or linker failures from the wrong `link.exe`.

Fix this by sourcing `vcvars64.bat` before starting the service so the service, and therefore its workers, inherit `LIB`, `INCLUDE`, and the MSVC `PATH`.

Use the provided script:

```powershell
.\scripts\team-mode-service.ps1 start
```

Or source it manually:

```cmd
"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cargo run --release --bin team_mode_service
```

Recommended Codex config for workers:

```toml
[shell_environment_policy]
inherit = "all"

sandbox_mode = "danger-full-access"
approval_policy = "never"
```

Non-Windows users can keep using `cargo run --release --bin team_mode_service` directly, or run the release binary in the background with `--data-dir .agent-teams --project-root .`.

### Push notifications — already wired

`.claude/settings.json` is committed with the Stop hook entry. **You must restart Claude Code after the first clone** so it picks up the hook config (CC only loads hooks at startup). After that, every change to `.mcp.json` or `.claude/settings.json` requires a full CC restart.

### Sanity checklist

1. `bash scripts/setup.sh` (or the PowerShell variant) — succeeds.
2. `target/release/team_mode_service(.exe)` exists.
3. `scripts/team-mode-service.ps1 start` reports `running pid=... url=http://127.0.0.1:8786/mcp`.
4. `claude` launched from the repo root → `/mcp` shows `team-mode` connected.
5. `team_create({"name":"smoke"})` succeeds; the web UI auto-opens.
6. `worker_add({"team":"smoke","name":"alice","adapter":"claude-code"})` succeeds.
7. `team_delete({"name":"smoke"})` succeeds.
8. Read [`.plans/agent-teams-v2/docs/03-operations/usage-tips.md`](.plans/agent-teams-v2/docs/03-operations/usage-tips.md) for the do's and don'ts.

If any step fails, see [`.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md`](.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md) — it has a full triage table.

---

## Troubleshooting — worker replies aren't pushing

The single most common issue. Symptoms: `send_message` returns success, but no `<system-reminder>` arrives in your next turn. Triage in this exact order:

1. **Did you restart CC after first clone / after editing `.claude/settings.json`?** Hooks load only at CC startup — `/mcp reconnect` does NOT pick them up. Quit all CC windows, relaunch `claude`, retry.
2. **Is the worker actually replying?** `tail -f .agent-teams/team-mode-service.log` — you should see the reply being appended for `lead`. If not, the worker is stuck (check that the backend CLI, e.g. `codex`, is installed and on PATH).
3. **Is the Stop hook firing?** `tail -f .agent-teams/.lead-pending-wake.log` — you should see async-wake injection lines. No entries at all → hook not loaded → see step 1. Service lookup errors → run `scripts/team-mode-service.ps1 status`.
4. **Still nothing?** See [`.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md`](.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md) for the full table (15+ scenarios with fixes).

The `send_message` tool's response `hint` field is intentionally chatty about this — if you ever see "If reminders never arrive ..." in a tool result, you've already hit the issue and should restart CC.

---

## Data directory layout

Created by the service under the lead's CWD on first tool call:

```
.agent-teams/
├── README.md                  ← auto-regenerated on each daemon start
├── .locks/                    ← file locks (per-team + lead_pending)
├── runtime/
│   ├── http-mcp.json          ← {pid, url, token_file, base_dir, project_root}
│   └── workers.json           ← worker runtime sidecar (orphans marked dead on daemon restart)
├── team-mode-service.log      ← service stderr/tracing
└── <team-name>/
    ├── team.json              ← team metadata (incl. owner_cc_pid)
    ├── members.json           ← v=1, unified identity + execution profile
    ├── room.json              ← room metadata
    ├── messages.jsonl         ← append-only message history (source of truth)
    └── lead_pending.jsonl     ← per-team push queue, atomically drained by hook

# Hook-side scratch files:
.agent-teams/.lead-pending-wake.log
.agent-teams/.cc-identity.<session_id>.json
```

Old project-root `lead_pending.jsonl` files are migrated into per-team files by the service at startup.

A legacy `.team-mode-data/` directory triggers a startup warning (not migrated — delete manually).

---

## Development

```bash
# Compile check (fast, no link)
cargo check --lib

# Run the 300 unit tests (~1s)
cargo test --lib

# Build the default HTTP MCP service
cargo build --release --bin team_mode_service

# Optional web binary (built into the daemon by default; standalone build for hacking)
cargo build --release --features team-mode-web --bin team_mode_web
```

Useful design specs:
- [`.plans/agent-teams-v2/decisions.md`](.plans/agent-teams-v2/decisions.md) — ADR-020/021/022 current HTTP service and async wake decisions
- [`.plans/agent-teams-v2/docs/05-design-history/legacy/team-mode-mcp-final.md`](.plans/agent-teams-v2/docs/05-design-history/legacy/team-mode-mcp-final.md) — legacy rollback / fallback stdio MCP runtime + tool surface + storage layout
- [`.plans/agent-teams-v2/docs/02-current-system/worker-detach-refactor.md`](.plans/agent-teams-v2/docs/02-current-system/worker-detach-refactor.md) — legacy rollback / fallback daemon architecture rationale
- [`.plans/agent-teams-v2/docs/05-design-history/hook-push-design.md`](.plans/agent-teams-v2/docs/05-design-history/hook-push-design.md) — Stop hook + JSON block design
- [`.plans/agent-teams-v2/docs/05-design-history/design-decisions.md`](.plans/agent-teams-v2/docs/05-design-history/design-decisions.md) — full bug journal + alternatives considered
- [`.plans/refactor-data-layout/spec.md`](.plans/refactor-data-layout/spec.md) — current data layout spec

Adding a backend? See `src/backend/{claude_code,codex,gemini}.rs` for reference implementations of the `Backend` trait. `AgentLoop` drives all backends uniformly. Read [`CONTRIBUTING.md`](CONTRIBUTING.md) for code conventions.

---

## Codex as Lead

Short version: **not currently supported.** The Stop-hook-based push is a Claude Code specific feature — Codex CLI has no equivalent blocking hook ([openai/codex#8375](https://github.com/openai/codex/issues/8375)).

The only officially supported path for Codex-as-Lead would be the `codex app-server` JSON-RPC mode, which would require building a harness around it (~2000+ lines of Rust). Research / discussion welcome in the issue tracker.

Codex as a **worker** is fully supported and has full session-transcript parity with Claude Code in the web UI.

---

## Credits

This project is **derived from and builds on** [`github.com/ZhangHanDong/agent-teams-rs`](https://github.com/ZhangHanDong/agent-teams-rs) (MIT, © 2025 Zhang Han Dong), which provides the core runtime, backends, team/task/inbox domain, and CLI. This fork refocuses the project around the `team_mode_service` HTTP MCP service and adds:

- The Stop-hook + JSON-block + ancestor-routing push architecture
- A durable localhost `team_mode_service` that survives Claude Code reconnects
- A live web UI on `127.0.0.1:8787` with per-sender colors, full session transcripts (Claude Code + Codex), and human-in-the-loop messaging
- A unified member file layout (`members.json` v=1 with merged identity + execution)
- Per-team subdirectory data layout with auto-generated `README.md`
- `worker_add on_existing`, strict `send_message`, `team_delete shutdown_failures`, `worker_add` ready-check
- Per-dispatch terminal-message guarantee (silent turn / pipe close → `[SYSTEM]`)
- Strict slug validation, case-insensitive `@mention`, just-in-time runtime hints
- Service observability watchdog, asyncRewake Stop-hook batching, one-live-team enforcement
- The `inbox_read` pull-mode tool
- Hook scripts, setup automation, and end-user documentation

---

## License

MIT — see [`LICENSE`](LICENSE).
