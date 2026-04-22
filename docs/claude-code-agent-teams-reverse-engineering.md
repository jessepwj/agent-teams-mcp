> **[HISTORICAL — 2026-02]** 本文档是对 Claude Code 原生 Agent Teams 文件格式的逆向分析报告（基于 ~/.claude/teams/ 和 ~/.claude/tasks/ 实测数据），涵盖 config.json、inbox、task 文件格式等观察。这些观察描述的是 Claude Code 原生格式，而非本项目当前 MCP 实现的数据格式。仅作为项目调研参考保留，当前架构参见 docs/architecture-background.md。

# Claude Code Agent Teams: Reverse Engineering Report

> Based on forensic analysis of real production data at `~/.claude/teams/` and `~/.claude/tasks/` across 4 teams and 55+ task directories. Every field documented here was verified against actual files on disk. No speculation without evidence.
>
> **Date**: 2026-02-12
> **Claude Code version observed**: 2.1.37
> **Methodology**: Direct file inspection, GitHub issue verification, inbox/task/config file parsing

---

## 1. Filesystem Layout

```
~/.claude/
├── teams/
│   └── {team-name}/
│       ├── config.json              # Team configuration
│       ├── config.json.lock         # Advisory lock for config writes
│       └── inboxes/
│           ├── {agent-name}.json    # Agent inbox (JSON array)
│           └── {agent-name}.lock    # Advisory lock for inbox writes
├── tasks/
│   ├── {team-name}/                 # Named team task directories
│   │   ├── .lock                    # Advisory lock for task operations
│   │   ├── 1.json
│   │   ├── 2.json
│   │   └── ...
│   └── {session-uuid}/             # Session-scoped task directories (non-team)
│       ├── .lock
│       └── *.json
└── projects/
    └── {hash}/
        └── {session-id}.jsonl       # Session transcript (tool calls, messages)
```

**Observed data**:
- 4 team directories: `code-review-agent-teams`, `macos-cleaner-chat`, `macos-cleaner-dev`, `proxy-analysis`
- 55+ task directories (mix of UUID session dirs and named team dirs)
- Lock files are **separate `.lock` companion files**, NOT flock on the main file
- Inbox files: up to 90KB observed (`macos-cleaner-dev/inboxes/team-lead.json`)

---

## 2. Team Configuration (`config.json`)

### 2.1 Full Native Format (TeamCreate)

Observed in `proxy-analysis` and `macos-cleaner-dev` (teams created via Claude Code's native `TeamCreate` tool):

```json
{
  "name": "proxy-analysis",
  "description": "Three @teammates: CC writer + Codex proxy + Gemini proxy",
  "createdAt": 1770836587799,
  "leadAgentId": "team-lead@proxy-analysis",
  "leadSessionId": "a2485d01-5a05-4089-9dd4-32a061a1a1c8",
  "members": [
    {
      "agentId": "team-lead@proxy-analysis",
      "name": "team-lead",
      "agentType": "team-lead",
      "model": "claude-opus-4-6",
      "joinedAt": 1770836587799,
      "tmuxPaneId": "",
      "cwd": "/Users/zhangalex/Work/Projects/FW/rust-claude-code-api",
      "subscriptions": [],
      "backendType": "in-process"
    },
    {
      "agentId": "cc-writer@proxy-analysis",
      "name": "cc-writer",
      "agentType": "general-purpose",
      "model": "claude-opus-4-6",
      "prompt": "你是 @cc-writer...(详细系统指令)",
      "color": "blue",
      "planModeRequired": false,
      "joinedAt": 1770836740207,
      "tmuxPaneId": "in-process",
      "cwd": "/Users/zhangalex/Work/Projects/FW/rust-claude-code-api",
      "subscriptions": [],
      "backendType": "in-process"
    }
  ]
}
```

### 2.2 Complete Field Reference

| Field | Type | Present On | Description |
|-------|------|-----------|-------------|
| `name` | string | team | Team identifier (ASCII, kebab-case) |
| `description` | string | team | Human-readable purpose |
| `createdAt` | number | team | Unix timestamp (milliseconds) |
| `leadAgentId` | string | team | Fully qualified: `"{name}@{team}"` |
| `leadSessionId` | string (UUID) | team | Session UUID of the lead agent |
| `members` | array | team | All members including lead |
| `agentId` | string | member | Fully qualified: `"{name}@{team}"` |
| `name` | string | member | Short name (used in SendMessage, TaskUpdate) |
| `agentType` | string | member | `"team-lead"` or `"general-purpose"` |
| `model` | string | member | e.g. `"claude-opus-4-6"` |
| `prompt` | string | teammate only | System instruction (absent on lead) |
| `color` | string | teammate only | UI color hint: `"blue"`, `"green"`, `"yellow"` |
| `planModeRequired` | boolean | teammate only | Whether agent needs plan approval before execution |
| `joinedAt` | number | member | Unix timestamp (milliseconds) |
| `tmuxPaneId` | string | member | `""` for lead, `"in-process"` for in-process agents |
| `cwd` | string | member | Working directory path |
| `subscriptions` | array | member | Event subscriptions (always `[]` in observed data) |
| `backendType` | string | teammate | `"in-process"` observed; empty/absent on lead |

### 2.3 Simplified Format Variant

Observed in `macos-cleaner-chat` — a team with a different creation path:

```json
{
  "teamName": "macos-cleaner-chat",
  "description": "macOS Cleaner AI Chat",
  "members": [
    {
      "name": "assistant",
      "agentId": "assistant@macos-cleaner-chat",
      "agentType": "claude-code",
      "prompt": "You are the AI assistant for macOS Cleaner..."
    }
  ]
}
```

**Critical difference**: Uses `"teamName"` instead of `"name"`, and lacks `createdAt`, `leadAgentId`, `leadSessionId`, `joinedAt`, `tmuxPaneId`, `cwd`, `subscriptions`, `backendType`, `planModeRequired`, `color` fields.

**Possible explanations**:
1. Created by a different code path (not the native `TeamCreate` tool)
2. An older Claude Code version that used a simpler schema
3. A lightweight "chat team" mode distinct from full agent teams

### 2.4 Agent Identification Convention

```
agentId = "{short-name}@{team-name}"

Examples:
  team-lead@proxy-analysis
  cc-writer@proxy-analysis
  codex-proxy@proxy-analysis
  gemini-proxy@proxy-analysis
```

All inter-agent communication (SendMessage, TaskUpdate owner) uses the **short name** only.

---

## 3. Inbox Message System

### 3.1 File Format

Each agent has an inbox file at `~/.claude/teams/{team}/inboxes/{agent-name}.json` containing a JSON array of messages.

```json
[
  {
    "from": "team-lead",
    "text": "Hi! I see you're working on Task #1...",
    "timestamp": "2026-02-11T08:27:54.622Z",
    "read": true,
    "summary": "Task sequence guidance for setup",
    "color": "blue"
  }
]
```

### 3.2 Message Field Reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `from` | string | Yes | Sender short name |
| `text` | string | Yes | Content (plain text or serialized JSON) |
| `timestamp` | string (ISO-8601) | Yes | e.g. `"2026-02-11T19:06:06.012Z"` |
| `read` | boolean | Yes | Whether recipient has processed this message |
| `summary` | string | No | 5-10 word UI preview |
| `color` | string | No | Sender's color from config (`"blue"`, `"green"`, `"yellow"`) |

**Key observation**: There is NO `to` field, NO `id` field, and NO `message_id`. The recipient is implicit from the inbox file name. Messages have no unique identifier for deduplication.

### 3.3 Protocol Messages (JSON-in-JSON)

When the `text` field contains a serialized JSON string, it's a **protocol message**. The outer message is a regular inbox entry; the inner JSON carries structured data.

**Observed protocol types**:

#### `task_assignment`
```json
{
  "type": "task_assignment",
  "taskId": "1",
  "subject": "Set up project structure and Cargo.toml",
  "description": "Create all directories and module files...",
  "assignedBy": "team-lead",
  "timestamp": "2026-02-11T08:27:04.754Z"
}
```

#### `idle_notification`
```json
{
  "type": "idle_notification",
  "from": "cc-writer",
  "timestamp": "2026-02-11T19:08:12.345Z",
  "idleReason": "available"
}
```

**How to distinguish**: Parse `text` as JSON. If it has a `"type"` field, it's a protocol message. Otherwise it's plain text/markdown.

### 3.4 Lazy Inbox Creation

Inbox files are created on first message delivery, NOT pre-allocated at team creation time. In `code-review-agent-teams`, the team config has 1 member (lead only) and the inboxes directory may not exist at all.

### 3.5 Delivery Model

- **No heartbeat**: No periodic keepalive mechanism
- **No ACK**: Only `read: boolean`, no delivery confirmation
- **No message ordering guarantee**: Timestamp is the only ordering hint
- **No message ID**: No deduplication mechanism
- **File-level locking**: `{agent-name}.lock` companion file for concurrent access

---

## 4. Task Files

### 4.1 File Format

Each task is stored as `~/.claude/tasks/{team-or-session}/{id}.json`:

```json
{
  "id": "1",
  "subject": "Set up project structure and Cargo.toml",
  "description": "Create all directories and module files for the project...",
  "activeForm": "Setting up project structure",
  "status": "completed",
  "owner": "setup-specialist",
  "blocks": ["2", "3"],
  "blockedBy": []
}
```

### 4.2 Task Field Reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | Numeric string: `"1"`, `"2"`, etc. |
| `subject` | string | Yes | Imperative form title |
| `description` | string | No | Detailed requirements |
| `activeForm` | string | No | Present continuous for spinner: `"Running tests"` |
| `status` | string | Yes | `"pending"`, `"in_progress"`, `"completed"`, `"deleted"` |
| `owner` | string | No | Agent short name |
| `blocks` | array[string] | No | Task IDs this task blocks (forward references) |
| `blockedBy` | array[string] | No | Task IDs that must complete first |
| `metadata` | object | No | Arbitrary key-value pairs |

### 4.3 Task ID Assignment

IDs are monotonically increasing numeric strings starting from `"1"`. The `.lock` file in the task directory serializes creation operations to avoid ID conflicts.

### 4.4 Status Transitions

Observed transitions: `pending → in_progress → completed`, and `any → deleted`. No evidence of `completed → pending` regression.

### 4.5 Directory Duality

`~/.claude/tasks/` contains BOTH:
- **Named team directories** (e.g., `proxy-analysis/`, `macos-cleaner-dev/`) — tasks belonging to agent teams
- **UUID session directories** (e.g., `04318e4f-5c02-4319-90b5-b24b27ea6cea/`) — tasks from standalone Claude Code sessions (no team)

Both use the same task file format. This confirms that Claude Code's task system is session-scoped by default and team-scoped when a team exists.

---

## 5. Lock File Analysis

### 5.1 Observed Lock Files

| Path | Purpose |
|------|---------|
| `~/.claude/teams/{team}/config.json.lock` | Serialize config.json modifications |
| `~/.claude/teams/{team}/inboxes/{agent}.lock` | Serialize inbox message append/read |
| `~/.claude/tasks/{team}/.lock` | Serialize task creation/modification |

### 5.2 Lock Mechanism

Lock files are **0-byte companion files** used as advisory locks (likely via `flock(2)` or Node.js `fs-ext` equivalent). They are NOT the actual data files being locked.

Evidence from `macos-cleaner-chat`:
```
-rw-r--r--  0B  config.json.lock
-rw-r--r--  0B  inboxes/assistant.lock
```

### 5.3 Known Concurrency Issues

- **MEMORY.md corruption** (Issue #24130): Multiple agents writing `~/.claude/projects/{hash}/MEMORY.md` concurrently, no lock file for this path
- **Task state desync** (Issue #23629): TaskUpdate status not reliably synced between team and session task lists, suggesting the lock mechanism has gaps

---

## 6. Agent Backend Types

### 6.1 `backendType: "in-process"`

Observed on all teammates in `proxy-analysis` and `macos-cleaner-dev`. This means:
- The agent runs **in the same process** as Claude Code (not in a separate tmux pane)
- `tmuxPaneId` is set to `"in-process"` (not a real tmux pane ID)
- Communication is via the file-based inbox (same as tmux-based agents)

### 6.2 `tmuxPaneId: ""`

Observed on team leads. The lead is the main Claude Code session itself — it doesn't need a separate pane.

### 6.3 Absence of tmux-based agents in observed data

All observed teammates use `backendType: "in-process"`. No tmux-based teams were found in the local data. This may indicate that in-process is now the default/preferred mode, or that tmux mode requires explicit opt-in.

---

## 7. Execution Flow Reconstruction

Based on inbox message timestamps and task state analysis across `macos-cleaner-dev`:

```
Timeline (2026-02-11):
08:27:04  task_assignment → setup-specialist (Task #1)
08:27:54  team-lead → setup-specialist: guidance message
08:28:51  team-lead → setup-specialist: status check
08:29:08  team-lead → ALL: CRITICAL - wrong framework detected
08:29:13  task_assignment → setup-specialist (Task #2, after correction)
08:31:40  task_assignment → setup-specialist (Task #3)
08:32:38  team-lead → ALL: foundation complete, work begins

[~4 hours of parallel execution]

16:27    Team config last modified (5 members)
16:40    runtime-developer, setup-specialist inboxes last modified
16:44    core-developer inbox last modified
16:49    ui-developer inbox last modified
16:50    team-lead inbox last modified (90KB — heaviest)
```

**Key observations**:
1. Team lead is the **most active communicator** — its inbox (90KB) is 4x larger than any teammate
2. Task assignments flow in BOTH directions: lead → teammate AND teammate → self (self-assignment)
3. Team lead sends broadcast-like messages manually (same message to each inbox)
4. Error recovery (wrong framework) was handled by team lead broadcasting a STOP message

---

## 8. GitHub Issue Verification

Six issues from the article were verified on 2026-02-12:

| Issue | Title | Article Claims | Verified Status | Verdict |
|-------|-------|---------------|----------------|---------|
| [#23620](https://github.com/anthropics/claude-code/issues/23620) | Agent team lost when lead's context gets compacted | OPEN | **OPEN** | Correct |
| [#25131](https://github.com/anthropics/claude-code/issues/25131) | Catastrophic agent lifecycle failures, duplicate spawning | OPEN | **OPEN** | Correct |
| [#24108](https://github.com/anthropics/claude-code/issues/24108) | Teammates stuck at idle prompt in tmux split-pane mode | OPEN | **CLOSED** | **WRONG** |
| [#24130](https://github.com/anthropics/claude-code/issues/24130) | Auto memory file not safe for concurrent agent teams | OPEN | **OPEN** | Correct |
| [#24977](https://github.com/anthropics/claude-code/issues/24977) | Task completion updates flood context window | OPEN | **OPEN** | Correct |
| [#23629](https://github.com/anthropics/claude-code/issues/23629) | TaskUpdate status not synced between team and session | OPEN | **OPEN** | Correct |

**Article accuracy**: 5/6 correct (83%). Issue #24108 was CLOSED at time of verification.

---

## 9. What the Article Got Right

1. **Filesystem-based coordination** — Accurate. Teams, tasks, and messages are all plain JSON files.
2. **JSON-in-JSON protocol messages** — Accurate. `task_assignment` and `idle_notification` types confirmed.
3. **No heartbeat, no ACK** — Accurate. The `read` boolean is the only state tracking.
4. **File locking via companion files** — Accurate. `.lock` files observed at all three levels.
5. **Task DAG with `blocks`/`blockedBy`** — Accurate. Exact field names confirmed.
6. **5 of 6 GitHub issues** — Accurate status at time of writing.

## 10. What the Article Got Wrong or Missed

### 10.1 Incorrect
- **Issue #24108 status**: Article says OPEN, actually CLOSED
- **Simplified config.json example**: Article's example omits critical fields (`createdAt`, `leadSessionId`, `tmuxPaneId`, `cwd`, `subscriptions`, `backendType`, `planModeRequired`, `color`, `joinedAt`)

### 10.2 Not mentioned
- **`planModeRequired` field**: Controls whether a teammate must have its plan approved before executing. This is a significant architectural feature enabling supervised vs autonomous agent modes.
- **`backendType: "in-process"`**: Article focuses on tmux split-pane model, but real data shows all modern teammates run in-process.
- **`color` field**: On both member config and inbox messages, enabling UI differentiation.
- **`summary` field**: On inbox messages, providing UI preview text.
- **`subscriptions` array**: Empty in all observed data but present in schema — suggests planned event-based messaging.
- **`leadSessionId`**: Ties the team to a specific Claude Code session.
- **Two config format variants**: `"name"` vs `"teamName"` key, full vs minimal member schemas.
- **Self-assignment**: Agents can send `task_assignment` to themselves (observed in `setup-specialist`).
- **Team lead inbox is the heaviest**: 90KB vs ~15-25KB for teammates — the lead receives all reports and coordination messages.

---

## 11. Architectural Insights

### 11.1 The "In-Process" Shift

The article's narrative centers on tmux-based agent spawning. But the real data tells a different story: **every observed teammate uses `backendType: "in-process"`**. This suggests Claude Code has moved (or is moving) from tmux subprocess agents to in-process agents, which:
- Eliminates tmux dependency and pane management overhead
- Enables tighter memory sharing (same Node.js process)
- Explains why Issue #24108 (tmux polling bug) was CLOSED — the tmux codepath may be deprecated

### 11.2 Plan Mode as Quality Gate

`planModeRequired: false/true` on teammates is an underappreciated feature. It enables a **two-tier execution model**:
- **Autonomous agents** (`planModeRequired: false`): Execute immediately, report results
- **Supervised agents** (`planModeRequired: true`): Must submit a plan, wait for lead approval, then execute

This is essentially a pull-request-like workflow for agent actions.

### 11.3 The Inbox Bottleneck

Team lead's inbox (90KB) is 4x the size of any teammate's. All coordination flows through the lead. This is a single-point-of-bottleneck architecture:
- Lead context compaction (Issue #23620) kills the entire team
- Lead must parse all teammate reports serially
- No direct peer-to-peer coordination observed (all messages route through lead)

### 11.4 Missing: Message Deduplication

No `message_id` on inbox messages means:
- Duplicate deliveries cannot be detected
- Exactly-once processing is not guaranteed
- Crash recovery may replay already-processed messages

### 11.5 Missing: Agent Health Monitoring

No heartbeat, no process health check, no timeout. If an agent silently dies:
- Its inbox keeps accepting messages (file system doesn't care)
- Team lead has no way to detect the failure except timeout heuristics
- Issue #25131 (duplicate spawning) may be a symptom of this gap

---

## 12. Data Sizes and Performance Characteristics

| Metric | Observed |
|--------|----------|
| Config file size (full team) | 585B – 5.9KB |
| Inbox file size (small team) | 100B – 1KB |
| Inbox file size (active team) | 15KB – 90KB |
| Task file size | ~200B – 1KB each |
| Task count per team | 1 – 23 |
| Lock file size | 0 bytes (advisory only) |
| Task directories total | 55+ |

---

## Appendix A: Comparison with agent-teams Library Models

| Claude Code Native | agent-teams Library | Gap |
|-------------------|---------------------|-----|
| Config `"name"` key | `"teamName"` key | Library matches simplified format only |
| `createdAt` (ms timestamp) | Not present | Missing field |
| `leadAgentId`, `leadSessionId` | Not present | Missing fields |
| `joinedAt` (ms timestamp) | Not present | Missing field |
| `tmuxPaneId` | Not present | Missing field |
| `cwd` | Not present | Missing field |
| `subscriptions` | Not present | Missing field |
| `backendType` | Not present | Missing field |
| `planModeRequired` | Not present | Missing field |
| `color` on member | Not present | Missing field |
| `color` on inbox message | Not present | Missing field |
| `summary` on inbox message | Present | Aligned |
| No `id` on inbox message | Has `id` (UUID) | Library extends the format |
| No `to` on inbox message | Has `to` | Library extends the format |
| `from`, `text`, `timestamp`, `read` | `from`, `content`(not `text`), `timestamp`, `read` | Key name mismatch: `text` vs `content` |
| Task format | Task format | Fully aligned |

**Priority gaps for compatibility**:
1. Config `"name"` vs `"teamName"` — need to accept both
2. Inbox `"text"` vs `"content"` — need to accept both
3. Missing member fields (11 fields) — need to add as optional
