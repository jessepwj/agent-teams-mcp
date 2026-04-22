---
description: "Auto-loaded reference: delegation patterns, CLI syntax, hybrid execution model for multi-agent teams."
user-invocable: false
---

# Delegation Patterns

Auto-loaded reference for `/team-delegate`, `/team-review`, `/team-implement`, `/teams-dag`.

## Auto-Trigger

| User Intent | Skill |
|-------------|-------|
| "review", "审查", "帮我看看代码" | `/team-review` |
| "implement", "实现", "开发" + complex task | `/team-implement` |
| "team", "多agent", "delegate", "用Codex/Gemini" | `/team-delegate` |
| "dag", "依赖图", "关键路径", "环检测", "task graph" | `/teams-dag` |

Skip for: simple edits, questions, single-file tasks.

## Hybrid Execution Model

| Needs Claude intelligence? | Execution | Tool |
|---------------------------|-----------|------|
| No (CLI tool call) | Background Bash | `Bash(run_in_background: true)` |
| Yes (design, synthesis) | Native teammate | `Task(team_name, name)` |

**DAG**: Track ALL tasks with TaskCreate/TaskUpdate regardless of execution path.

## CLI Reference

```bash
# Codex
codex exec "PROMPT"                       # Non-interactive
codex exec "PROMPT" -m o3                 # Model override
codex review "PROMPT"                     # Review git changes
codex review --base main "PROMPT"         # Review vs branch

# Gemini
gemini -m gemini-2.5-pro -y <<'EOF'       # Heredoc
PROMPT
EOF

# agent-teams DAG analysis (reads ~/.claude/tasks/{team}/ directly)
agent-teams dag validate --team <name>       # Cycle detection
agent-teams dag show --team <name>           # Terminal DAG visualization
agent-teams dag show --team <name> -f mermaid  # Mermaid output
agent-teams dag next --team <name>           # Show unblocked ready tasks
agent-teams dag critical-path --team <name>  # Longest dependency chain
```

## DAG Analysis Integration

After `TaskCreate` + `TaskUpdate addBlockedBy`, run `agent-teams` CLI before launching agents:

1. `agent-teams dag validate --team <name>` — catch cycles early
2. `agent-teams dag show --team <name>` — visualize the full DAG for the user
3. `agent-teams dag next --team <name>` — confirm which tasks to fire in parallel

During execution, use `agent-teams dag next` to determine what's unblocked after each phase completes.

**Prerequisite**: `cargo install --path /path/to/agent-teams --features cli`

## Key Rules

1. **Background Bash for CLI tools**: no Claude wrapper overhead
2. **Native teammates for Claude tasks**: Task(team_name, name) for @-prefixed members
3. **DAG always**: TaskCreate + TaskUpdate addBlockedBy for dependencies
4. **DAG analysis**: run `agent-teams dag validate` after creating DAG, before launching agents
5. **Parallel max**: fire all independent tasks in ONE message
6. **Poll + coordinate**: TaskOutput for Bash, SendMessage for teammates
7. **Fallback**: if CLI missing, use Task(general-purpose) instead
