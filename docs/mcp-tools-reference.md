# MCP Tools & Resources Reference (team-mode)

> **8 tools + 5 resource URIs.** The MCP caller is the Lead Agent (your Claude Code CLI session); all tools execute with Lead authority.

---

## Tool 1 — `team_create`

**Purpose**: Create a new team. A virtual `lead` member is auto-created.

**Parameters**:

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | ✅ | Team name, ASCII slug, globally unique. |
| `cwd` | string | ❌ | Team-default working directory; workers inherit when they don't override. |

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
      "status": "active",
      "createdAt": "...",
      "updatedAt": "..."
    }
  ]
}
```

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

`shutdown_failures` is an empty array when everything shut down cleanly. Non-empty means those processes may be orphaned; handle them manually.

---

## Tool 4 — `worker_add`

**Purpose**: Add a worker to a team AND start its managed agent process.

**Parameters**:

| Field | Type | Required | Notes |
|---|---|---|---|
| `team` | string | ✅ | Team name. |
| `name` | string | ✅ | Worker name, ASCII slug, team-unique, cannot be `"lead"`. |
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
| Yes | `reuse` | **Fast-resume**: loads saved profile, ignores passed-in fields, warns if caller passed config. |
| Yes | `overwrite` | **Replace**: overwrites saved profile with what you passed (`adapter` required). |
| Yes | `error` (default) | **Abort**: returns an error listing the existing profile path so the caller can decide. |

**Returns**:
```json
{
  "team": "demo",
  "name": "alice",
  "sessionState": "running",
  "mode": "create"
}
```

`mode` is `"create"`, `"reuse"`, or `"overwrite"`. `sessionState` is one of `running | starting | failed` based on the 5-second ready-check.

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
    {
      "name": "alice",
      "adapter": "claude-code",
      "sessionState": "running"
    }
  ]
}
```

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

**Returns**:
```json
{
  "message": { /* full Message JSON */ },
  "matched_recipients": ["alice", "bob"]
}
```

---

## Tool 8 — `inbox_read`

**Purpose**: Pull-mode read of the lead's inbox. Use when the FileChanged push hook is not configured, or when you explicitly want to check for replies.

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

- `team.id = team.name` (whatever slug you passed).
- Members are identified within a team by `name`. There is **no** separate `handle`, and no composite `{team}-{name}` id — that legacy format has been removed.
- Lead's `name` is always `"lead"` (lowercase).
- Worker `@mentions` resolve against `name`.

Resource URIs use the plain `name` segment, not a composite id.
