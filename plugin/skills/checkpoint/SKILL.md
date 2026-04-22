---
description: "Checkpoint: attach agent session context to Git commits via notes. Create, show, list, and diff agent checkpoints."
disable-model-invocation: true
---

Manage agent checkpoints — structured metadata attached to Git commits via Git notes. Requires `agent-teams` CLI with checkpoint feature (`cargo install --path /path/to/agent-teams --features "cli,checkpoint"`).

**Arguments**: $ARGUMENTS

## Usage

Parse $ARGUMENTS to determine action. Default action: `list`.

```
/checkpoint                          → list (default)
/checkpoint create [--team <TEAM>]   → create checkpoint for HEAD
/checkpoint show [COMMIT]            → show checkpoint details
/checkpoint list [--agent <NAME>]    → list checkpoints
/checkpoint diff <FROM> <TO>         → diff two checkpoints
```

## Step-by-step

### 1. Check CLI availability
```bash
which agent-teams 2>/dev/null && agent-teams checkpoint --help 2>/dev/null | head -1 && echo "CHECKPOINT_OK" || echo "MISSING: run 'cargo install --path /path/to/agent-teams --features \"cli,checkpoint\"'"
```
If missing or checkpoint subcommand not available, tell the user how to install and stop.

### 2. Parse arguments

Extract `<action>` and options from $ARGUMENTS:
- If no args: action = `list`
- If one arg that looks like an action (create/show/list/diff): use it
- Otherwise: treat first arg as commit ref, action = `show`

### 3. Execute

**list** (default):
```bash
agent-teams checkpoint list [-n 20]
```
With optional filters:
```bash
agent-teams checkpoint list --agent <NAME> --branch <BRANCH> --team <TEAM> -n <LIMIT>
```

**create**:
```bash
agent-teams checkpoint create --agent <AGENT_NAME> [--team <TEAM>] [--model <MODEL>]
```
- Use the current agent name if available, otherwise default to "claude"
- If a team context exists, pass `--team`
- For extended data, add `--extended --session-jsonl <PATH>`

**show**:
```bash
agent-teams checkpoint show [COMMIT] [--format summary|json]
```
Default commit: HEAD. Use `--format json` for machine-readable output.

**diff**:
```bash
agent-teams checkpoint diff <FROM_REF> <TO_REF>
```
Accepts commit SHAs, HEAD~N, branch names, or any git ref.

### 4. Present results

Display the CLI output to the user. Key output formats:

- **list**: Table with COMMIT, BRANCH, AGENT, TEAM, FILES, CREATED columns
- **show summary**: Human-readable checkpoint details (session, team, tasks, files, tokens)
- **show json**: Full JSON for programmatic use
- **diff**: File/task/member changes with +/-/~ symbols
