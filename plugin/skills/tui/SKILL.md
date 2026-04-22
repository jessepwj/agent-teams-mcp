---
description: "TUI Dashboard: browse team status, checkpoints, and token costs in a terminal UI."
disable-model-invocation: true
---

Launch the agent-teams TUI dashboard — a ratatui terminal UI for browsing team status, checkpoints, and token costs. Requires `agent-teams` CLI with TUI feature (`cargo install --path /path/to/agent-teams --features "cli,tui"`).

**Arguments**: $ARGUMENTS

## Usage

Parse $ARGUMENTS to determine options. Default: launch TUI with auto-detected team and current repo.

```
/tui                          -> launch TUI (auto-detect team)
/tui --team <TEAM>            -> launch TUI focused on a specific team
/tui --repo <PATH>            -> launch TUI for a different repository
```

## Step-by-step

### 1. Check CLI availability
```bash
which agent-teams 2>/dev/null && agent-teams tui --help 2>/dev/null | head -1 && echo "TUI_OK" || echo "MISSING: run 'cargo install --path /path/to/agent-teams --features \"cli,tui\"'"
```
If missing or tui subcommand not available, tell the user how to install and stop.

### 2. Parse arguments

Extract options from $ARGUMENTS:
- `--team <TEAM>`: focus on a specific team
- `--repo <PATH>`: target a specific git repository

### 3. Execute

```bash
agent-teams tui [--team <TEAM>] [--repo <PATH>]
```

### 4. Tab navigation

The TUI has three tabs:

| Tab | Key | Content |
|-----|-----|---------|
| Team Status | `1` | Team config, members, task list with status indicators |
| Checkpoints | `2` | Scrollable checkpoint list, detail view with Enter |
| Token Costs | `3` | Aggregated token usage, per-agent breakdown, bar chart |

Global keys: `q` quit, `Tab`/`Shift-Tab` switch tabs, `j/k` navigate, `Enter` detail, `Esc` back, `r` refresh.
