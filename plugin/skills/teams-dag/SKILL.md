---
description: "DAG analysis for team tasks: validate, visualize, find ready tasks, critical path. Requires agent-teams CLI."
disable-model-invocation: true
---

Analyze the task DAG for a team. Requires `agent-teams` CLI (`cargo install --path /path/to/agent-teams --features cli`).

**Arguments**: $ARGUMENTS

## Usage

Parse $ARGUMENTS to determine action and team name. Default action: `show`.

```
/teams-dag show <team>           → dag show (terminal visualization)
/teams-dag validate <team>       → dag validate (cycle detection)
/teams-dag next <team>           → dag next (ready tasks)
/teams-dag critical-path <team>  → dag critical-path
/teams-dag <team>                → dag show (default)
```

## Step-by-step

### 1. Check CLI availability
```bash
which agent-teams 2>/dev/null && echo "OK" || echo "MISSING: run 'cargo install --path /path/to/agent-teams --features cli'"
```
If missing, tell the user how to install and stop.

### 2. Parse arguments

Extract `<action>` and `<team>` from $ARGUMENTS:
- If only one arg: treat as team name, action = `show`
- If two args: first = action, second = team name
- If no args: list available teams via `ls ~/.claude/tasks/`

### 3. Execute

Run the appropriate command via Bash:

**show** (default):
```bash
agent-teams dag show --team <TEAM>
```

**validate**:
```bash
agent-teams dag validate --team <TEAM>
```

**next**:
```bash
agent-teams dag next --team <TEAM>
```

**critical-path**:
```bash
agent-teams dag critical-path --team <TEAM>
```

**show with format** (if user specifies mermaid/dot/plain):
```bash
agent-teams dag show --team <TEAM> --format <FORMAT>
```

### 4. Present results

Display the CLI output to the user. For `show`, the terminal visualization includes:
- Phase grouping (topological levels)
- Status symbols: `○` pending, `◉` in-progress, `●` completed, `✗` deleted
- `★` marks critical path tasks
- `← dep_ids` shows dependency hints
- `@owner` shows task assignment
