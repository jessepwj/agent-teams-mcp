# MCP Tools & Resources Reference (team-mode)

> **8 tools + 5 resource URIs.** The MCP caller is the Lead Agent (your Claude Code CLI session); all tools execute with Lead authority.

> **Runtime guidance vs. static descriptions.** Tool descriptions visible to the
> AI are deliberately terse — only the contract the model must know to call the
> tool. Operational guidance (don't poll, revive a dead worker via `reuse`,
> `session_id` capture timing, etc.) is delivered as a `hint` field in tool
> responses, just-in-time. See [§ Runtime hints](#runtime-hints-just-in-time-guidance)
> below for the full set.

---

## Tool 1 — `team_create`

**Purpose**: Create a new team. A virtual `lead` member is auto-created.

**Parameters**:

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | ✅ | Team name. Strict slug: `[a-z0-9_.-]{1,64}`, must start with a letter or digit. |
| `cwd` | string | ❌ | Team-default working directory; workers inherit when they don't override. |

**Constraint**: at most ONE live team per project. If an alive team already
exists, this call fails. Orphan teams (owner CC has died) are auto-cleaned
and their names returned in the `cleaned_orphan_teams` field.

**Side effects**:
- Creates `<base>/<name>/team.json`
- Appends the lead (`name: "lead"`, `kind: Lead`) into `<base>/<name>/members.json`
- Creates `<base>/<name>/room.json` (the `main` room)
- Binds `team.leadMemberId = "lead"`

**Returns**: full Team JSON.

---

## Tool 2 — `team_list`

**Purpose**: List all teams.

**Parameters**: none.

**Returns**:
```json
{
  "teams": [
    {
      "id": "demo",
      "name": "demo",
      "cwd": "E:\\project",
      "leadMemberId": "lead",
      "ownerCcPid": 21104,
      "ownerStatus": "alive",
      "status": "active",
      "createdAt": "...",
      "updatedAt": "..."
    }
  ]
}
```

`ownerStatus` is one of:
- `alive` — owner CC PID is still a running process (the canonical case)
- `orphan` — a previous CC created this team but has since died; workers
  are gone too. Run `team_delete` to clean up.
- `unbound` — legacy entry without a recorded owner.

When any team has `ownerStatus: orphan`, the response also includes a
`hint` field naming the orphans + suggesting `team_delete`.

---

## Tool 3 — `team_delete`

**Purpose**: Delete a team entirely.

**Parameters**:

| Field | Type | Required |
|---|---|---|
| `name` | string | ✅ |

**Side effects**:
- Best-effort `shutdown_managed_member` on every member
- Terminates each worker's `AgentLoop`
- Removes the entire `<base>/<team-name>/` directory

**Returns**:
```json
{
  "ok": true,
  "name": "demo",
  "shutdown_failures": [
    {"member": "alice", "reason": "..."}
  ]
}
```

`shutdown_failures` is an empty array when everything shut down cleanly.
Already-removed members and sessions the orchestrator no longer tracks
(daemon restart, prior `worker_remove`) are silently skipped — only true
"refused to shut down" cases surface here. When non-empty, those processes
may be orphaned and should be handled manually.

`pruned_pending_entries` is added to the response when `lead_pending.jsonl`
held undelivered entries for this team that were dropped during cleanup.

---

## Tool 4 — `worker_add`

**Purpose**: Add a worker to a team AND start its managed agent process.

**Parameters**:

| Field | Type | Required | Notes |
|---|---|---|---|
| `team` | string | ✅ | Team name. |
| `name` | string | ✅ | Worker name. Strict slug: `[a-z0-9_.-]{1,64}`, must start with a letter or digit, cannot be `"lead"`. Required to be addressable via `@mention`. |
| `adapter` | enum | conditional | `claude-code` \| `codex` \| `gemini-cli`. Required when creating or overwriting. |
| `model` | string | ❌ | Backend-specific model override. |
| `cwd` | string | ❌ | Falls back to `team.cwd` when omitted. |
| `system_prompt` | string | ❌ | Prompt prefix. |
| `env` | object | ❌ | Extra env vars (string values only). |
| `on_existing` | enum | **required if profile exists** | `reuse` \| `overwrite` \| `error`. Default: `error`. |

### `on_existing` semantics

| Profile exists? | `on_existing` value | Behavior |
|---|---|---|
| No | any | **Create**: fresh profile; `adapter` required. |
| Yes, process alive | `reuse` | **Idempotent fast-resume**: loads saved profile, skips re-spawn (already running), returns current state. |
| Yes, process dead | `reuse` | **Revival**: drops the stale orchestrator session, spawns a fresh process from the saved profile, returns `revived_from_dead: true`. The worker has a NEW conversation context (no memory of prior turns). |
| Yes | `overwrite` | **Replace**: overwrites saved profile with what you passed (`adapter` required). |
| Yes | `error` (default) | **Abort**: returns an error listing the existing profile path so the caller can decide. |

**Returns**:
```json
{
  "team": "demo",
  "name": "alice",
  "sessionState": "running",
  "mode": "create",
  "hint": "Worker process started. Its backend session_id is captured after the FIRST `type:result` event ..."
}
```

`mode` is `"create"`, `"reuse"`, or `"overwrite"`. `sessionState` is one of
`running | starting | failed` based on the 5-second ready-check. On the
revive path, `revived_from_dead: true` and a `note` field are also present.

---

## Tool 5 — `worker_list`

**Purpose**: List active (non-removed, non-lead) workers in a team.

**Parameters**:

| Field | Required |
|---|---|
| `team` | ✅ |

**Returns**:
```json
{
  "workers": [
    { "name": "alice", "adapter": "claude-code", "sessionState": "running" },
    { "name": "bob",   "adapter": "claude-code", "sessionState": "dead" }
  ],
  "hint": "Dead workers found: [bob]. Revive each with `worker_add name=<x> on_existing=reuse` ..."
}
```

`sessionState` cross-references stored profile state with live process
liveness from the orchestrator: a worker stored as `running` whose process
is gone is reported as `dead`. The `hint` field is only present when at
least one dead worker is in the list.

---

## Tool 6 — `worker_remove`

**Purpose**: Soft-remove a worker — stop the process and mark it removed, but keep the execution profile for fast-resume.

**Parameters**:

| Field | Required | Notes |
|---|---|---|
| `team` | ✅ | |
| `name` | ✅ | Cannot be `"lead"`. |

**Side effects**:
- Best-effort process shutdown
- Closes worker's `AgentLoop`
- Sets the member's `status` to `Removed` in `members.json`
- Keeps `execution` field intact for a subsequent `worker_add name on_existing=reuse`

---

## Tool 7 — `send_message`

**Purpose**: Send a message as the team's lead.

**Parameters**:

| Field | Required | Notes |
|---|---|---|
| `team` | ✅ | |
| `text` | ✅ | Must contain at least one `@handle`. |

**Strict routing rules**:
- `text` must have ≥1 `@handle`
- **Every** `@handle` in `text` must resolve to an active worker (not the lead itself)
- Unmatched handles cause the call to fail with the list of unmatched names + the list of active workers
- `@handle` matching is case-insensitive (`@Alice` matches worker `alice`)
- Recipients whose process is dead are detected before dispatch:
  - **All dead** → call fails fast with names; a `[SYSTEM]` notice is
    written to the lead inbox per dead recipient.
  - **Mixed** → live recipients receive normally; dead `@handle`s are
    rewritten in the body to `[worker unavailable: <name>]`; per-dead-worker
    `[SYSTEM]` notices are posted; the response surfaces
    `dead_recipients` + `system_notices` + `dead_recipients_hint`.

**Returns**:
```json
{
  "message": { /* full Message JSON */ },
  "matched_recipients": ["alice", "bob"],
  "hint": "Replies will arrive automatically as a <system-reminder> when your next turn starts. Do NOT call inbox_read or sleep — just end your turn and continue when reminded."
}
```

The `hint` is present on EVERY successful send to remind the model that
worker replies arrive via the Stop hook on the next turn — calling
`inbox_read` afterwards is wrong.

---

## Tool 8 — `inbox_read`

**Purpose**: Fallback pull-mode read of the lead's inbox. Worker replies normally arrive automatically via the Stop hook as `<system-reminder>` on your next turn — this tool is only for explicit backlog audits or when push delivery is unavailable.

**Parameters**:

| Field | Type | Required | Default | Notes |
|---|---|---|---|---|
| `team` | string | ✅ | — | |
| `limit` | int | ❌ | 20 | Range 1–100. |
| `unread_only` | bool | ❌ | `true` | Skips acked messages when true. |
| `auto_ack` | bool | ❌ | `false` | Marks returned messages read + acked atomically. |

**Returns**:
```json
{
  "team": "demo",
  "lead": "lead",
  "unread_count": 3,
  "total_returned": 3,
  "messages": [
    {
      "id": "msg-xxx",
      "from": "alice",
      "kind": "reply",
      "text": "...",
      "reply_to": "msg-yyy",
      "thread_id": "thread-1",
      "status": "unread",
      "created_at": "..."
    }
  ]
}
```

When `messages` is empty, the response also carries a `hint` field reminding
the caller that the Stop hook is the canonical delivery channel and
`inbox_read` is rarely needed.

---

## Resources (read interface)

All URIs take the form `team://<team>/...`. Clients can `subscribe` to receive `notifications/resources/updated` when the URI's content changes (best-effort, not a replacement for the FileChanged push hook).

### 1. `team://<team>` — team metadata

```json
{
  "id": "demo",
  "name": "demo",
  "cwd": "E:\\project",
  "leadMemberId": "lead",
  "status": "active",
  "createdAt": "...",
  "updatedAt": "..."
}
```

### 2. `team://<team>/rooms/main` — main-room history

```json
{
  "room": { "id": "main", "teamId": "demo", "kind": "main", "status": "active" },
  "messages": [ /* all messages, sorted */ ]
}
```

### 3. `team://<team>/threads/<thread_id>` — thread messages

```json
{
  "thread": { /* Thread metadata */ },
  "messages": [ /* this thread's messages */ ]
}
```

### 4. `team://<team>/members/<name>/inbox` — member inbox

```json
{
  "inbox": { "items": [...] },
  "counts": { "total": N, "unread": U, "read": R, "acked": A }
}
```

- Lead inbox: `team://demo/members/lead/inbox` (also readable via the `inbox_read` tool)
- Worker inbox: `team://demo/members/alice/inbox` (useful for debugging)

### 5. `team://<team>/members/<name>/session` — session state

```json
{
  "team": "demo",
  "name": "alice",
  "kind": "member",
  "status": "active",
  "sessionState": "running",
  "execution": {
    "adapter": "claude-code",
    "model": "...",
    "cwd": "...",
    "systemPrompt": "...",
    "env": { ... },
    "sessionState": "running"
  }
}
```

---

## Per-dispatch terminal message guarantee

Every successful `send_message` to a worker results in **exactly one**
terminal message back to the room — never zero, never two:

- If the worker produces any visible text in its turn → `Reply` with that text.
- If the worker's turn ends silently (LLM produced nothing, content
  filtered, etc.) → `Status` with `[SYSTEM] worker 'X' completed its turn
  without producing any reply text for msg <id>...`
- If the worker's stdout pipe closes mid-turn (process crashed, killed) →
  `Status` with `[SYSTEM] worker 'X' output channel closed mid-turn...
  Use worker_add on_existing=reuse to revive`. The AgentLoop exits cleanly
  after this notice; the worker is reported `dead` on the next `worker_list`.
- If `send_input` itself fails (worker died before its turn started) →
  `Status` with `[SYSTEM] worker 'X' died while processing message...`
  (handled by the pre-existing Bug 17 fix.)

The lead always sees a single terminal event per dispatch. Combined with
the Stop hook delivering replies/statuses on the next turn, this means
the lead never has to poll to know "did the worker actually finish?".

---

## Runtime hints (just-in-time guidance)

Each tool's response may include a `hint` (and sometimes secondary
`note` / `dead_recipients_hint`) field carrying operational guidance
that depends on the current call's context. These replace the long
"DO NOT do X" preambles that used to live in tool descriptions —
attention is freshest right after a tool call, so guidance lands there
instead of in the system prompt.

| Trigger | Field | Message |
|---|---|---|
| `send_message` succeeded with at least one live recipient | `hint` | "Replies will arrive automatically as a `<system-reminder>` when your next turn starts. Do NOT call inbox_read or sleep — just end your turn and continue when reminded." |
| `send_message` had dead recipients in the mention list | `dead_recipients_hint` | "Workers [bob] were skipped because their process is gone. Revive each with `worker_add name=<x> on_existing=reuse` (the worker loses prior conversation context) before retrying." |
| `inbox_read` returned 0 messages | `hint` | "No messages in inbox. Worker replies arrive automatically via the Stop hook on your next turn — calling inbox_read is rarely needed; only useful for explicit backlog audits." |
| `worker_list` contains at least one `sessionState: "dead"` worker | `hint` | "Dead workers found: [bob, carol]. Revive each with `worker_add name=<x> on_existing=reuse` ..." |
| `team_list` contains at least one `ownerStatus: "orphan"` team | `hint` | "Orphan teams (owner CC has died): [old]. Their workers are gone; run `team_delete name=<x>` on each to free the one-live-team-per-project budget." |
| `worker_add` created a new worker (mode=create) | `hint` | "Worker process started. Its backend session_id is captured after the FIRST `type:result` event — i.e. once you send the first @mention message and the worker replies. Until then, the web UI 'process session' pane shows a placeholder for this worker." |
| `worker_add reuse` resurrected a dead worker | `revived_from_dead: true` + `note` | "Previous worker process was dead — its stale session was dropped and a fresh process spawned. The worker has a new conversation context (no memory of prior turns)." |

Hints are advisory; they are NOT included in success responses where the
relevant condition does not apply (e.g. `worker_list` with no dead workers
returns no `hint`).

The Stop hook batch-grace window (`TEAM_MODE_STOP_BATCH_GRACE_MS`,
default 500ms) coalesces concurrent worker replies into a single
reminder; this is independent of any tool call.

---

## Typical usage flows

### First-time team + worker + task

```
team_create(name="demo", cwd="E:\\project")
  → virtual lead auto-created
worker_add(team="demo", name="alice", adapter="claude-code", system_prompt="...")
  → alice spawned, AgentLoop listening, ready
send_message(team="demo", text="@alice please add 1+1")
  → alice's inbox receives it
  → AgentLoop injects stdin NDJSON
  → alice reasons → stdout result
  → AgentLoop posts Reply to the room
  → lead_pending.jsonl gets a new line
  → FileChanged hook fires → Claude Code lead wakes up and processes it
```

### Pause and resume a worker

```
worker_remove(team="demo", name="alice")
  → alice stopped; member status=Removed; execution profile kept
worker_add(team="demo", name="alice", on_existing="reuse")
  → saved profile reloaded, alice re-spawned
```

### Change an existing worker's config

```
worker_add(team="demo", name="alice", adapter="claude-code",
           system_prompt="new prompt", on_existing="overwrite")
  → saved profile overwritten, alice restarted with new config
```

### Clean teardown

```
team_delete(name="demo")
  → shuts down every worker, removes the whole team directory
```

---

## Internal naming rules

- `team.id = team.name` — whatever slug you passed.
- Slug rules: `[a-z0-9_.-]{1,64}`, must start with a lowercase letter or
  digit. Enforced at `team_create` and `worker_add`. Names that would
  break `@mention` parsing (spaces, uppercase, unicode, leading
  punctuation) are rejected with a clear error explaining which
  character offended.
- Members are identified within a team by `name`. There is **no** separate
  `handle`, and no composite `{team}-{name}` id — that legacy format has
  been removed.
- Lead's `name` is always `"lead"` (lowercase) and is reserved.
- Worker `@mentions` resolve against `name` case-insensitively, but the
  canonical stored form is lowercase.

Resource URIs use the plain `name` segment, not a composite id.
