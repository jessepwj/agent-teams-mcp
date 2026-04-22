---
description: "Free-form multi-agent delegation. Auto-builds team with DAG, parallelizes codex/gemini via background Bash."
---

Multi-agent orchestrator. Analyze task, build team with DAG, maximize parallelism.

**Task**: $ARGUMENTS

## Execution model

Two execution paths based on task type:

| Needs Claude intelligence? | Execution | Tool |
|---------------------------|-----------|------|
| No (CLI tool call) | Background Bash | `Bash(run_in_background: true)` |
| Yes (design, synthesis, complex edits) | Native teammate | `Task(team_name, name)` |

**Always**: Track ALL tasks with TaskCreate/TaskUpdate DAG, regardless of execution path.

## Step-by-step

1. Check tools: `which codex && which gemini && which agent-teams` via Bash
2. Scan project briefly (Glob, 1-2 files max)
3. Classify sub-tasks → pick execution path (see table above)
4. Design DAG: maximize parallel tasks, minimize sequential deps
5. `TeamCreate` with descriptive name
6. `TaskCreate` all tasks, `TaskUpdate` addBlockedBy for deps
7. **DAG analysis** (if `agent-teams` available):
   - `Bash("agent-teams dag validate --team <name>")` — catch cycles
   - `Bash("agent-teams dag show --team <name>")` — show DAG to user
8. Execute phase by phase:
   - **Parallel CLI tasks**: multiple `Bash(run_in_background)` in ONE message
   - **Parallel Claude tasks**: multiple `Task(team_name, name)` in ONE message
   - **Mixed**: fire both Bash + Task calls in ONE message
9. Poll `TaskOutput` for background tasks, mark completed
10. Receive `SendMessage` from teammates, mark completed
11. Spawn next phase when deps resolve (use `agent-teams dag next` to check)
12. Report summary to user
13. Shutdown teammates, TeamDelete

## Agent selection

| Task | Execution | Command |
|------|-----------|---------|
| Code generation | Bash background | `codex exec "PROMPT"` |
| Code review (git) | Bash background | `codex review "PROMPT"` |
| Analysis / review | Bash background | `gemini -m gemini-2.5-pro -y <<'EOF'` |
| Design / planning | Native teammate | `Task(team_name, name: "claude-architect")` |
| Synthesis / merge | Native teammate | `Task(team_name, name: "claude-synthesizer")` |
| Complex multi-step | Native teammate | `Task(team_name, name: "claude-worker")` |

## DAG design rules

- Independent tasks → NO deps → fire all in parallel
- Tasks needing previous output → blockedBy
- Always end with a synthesis/report task
- Aim for widest possible parallel phases (4+ tasks)
- Split large tasks into parallelizable chunks when possible

## Teammate prompt template
```
You are @<NAME> on team "<TEAM>".
Task: <DESCRIPTION>
Files to read: <PATHS>
<INSTRUCTIONS>
TaskUpdate completed. SendMessage results to team lead, summary: "<SHORT>"
```
