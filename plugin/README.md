# agent-teams: Claude Code Plugin

Multi-agent team orchestration for Claude Code. Spawn Claude + Codex + Gemini agents using slash commands.

## Prerequisites

- [Claude Code](https://claude.com/claude-code) (latest version)
- [Codex CLI](https://github.com/openai/codex) (optional, for code generation delegation)
- [Gemini CLI](https://github.com/google-gemini/gemini-cli) (optional, for analysis delegation)
- `agent-teams` CLI (optional, for DAG analysis, checkpoints, and TUI dashboard)

```bash
# Install Codex CLI
npm install -g @openai/codex

# Install Gemini CLI
npm install -g @anthropic/gemini-cli

# Install agent-teams CLI — DAG analysis only
cargo install --path /path/to/agent-teams --features cli

# Install agent-teams CLI — DAG analysis + checkpoints
cargo install --path /path/to/agent-teams --features "cli,checkpoint"

# Install agent-teams CLI — full features (DAG + checkpoints + TUI dashboard)
cargo install --path /path/to/agent-teams --features "cli,tui"

```

> If Codex, Gemini, or agent-teams CLI are not installed, commands will gracefully degrade — CLI tools fall back to Claude-only, DAG analysis is skipped.

## Installation

### Quick Start (Local Development)

Launch Claude Code with the plugin loaded directly:

```bash
claude --plugin-dir /path/to/agent-teams/plugin
```

### Install via Plugin Manager

From within a Claude Code session:

```bash
# Option 1: Add as a local marketplace, then install
/plugin marketplace add /path/to/agent-teams/plugin
/plugin install agent-teams

# Option 2: Open the interactive plugin manager
/plugin
```

### Installation Scopes

```bash
# User scope (default) — available in all projects
/plugin install agent-teams --scope user

# Project scope — shared with collaborators via .claude/settings.json
/plugin install agent-teams --scope project

# Local scope — personal, this repo only
/plugin install agent-teams --scope local
```

## Skills (Slash Commands)

All workflows are implemented as **skills** (not legacy commands). Skills support auto-triggering, frontmatter control, and Claude auto-invocation.

### `/team-delegate` - Free-form Team Delegation

The most flexible skill. Describe what you need and Claude will:
1. Analyze the task to determine which agents are needed
2. Create a team with proper agent composition
3. Build a task DAG with dependencies
4. Spawn agents with delegation instructions
5. Coordinate execution and report results

```
/team-delegate Implement user authentication with JWT tokens.
  Backend in src/api/auth.rs, frontend in src/components/Login.tsx.
  Include tests for both.

/team-delegate Review the entire src/database/ module for
  performance issues and SQL injection vulnerabilities.

/team-delegate Refactor src/legacy/ to use async/await instead
  of callbacks. Maintain backwards compatibility.
```

### `/team-review` - Code Review Pipeline

Pre-built 5+1 agent review pipeline with hybrid execution (4 background CLI + 1 Claude agent, then Claude synthesizer):

- **Phase 1** (all 5 in parallel):
  - Gemini bg: Architecture review
  - Gemini bg: Security review
  - Codex bg: Bug detection
  - Codex bg: Test coverage analysis
  - @claude-reviewer: Rust idioms, ownership, domain logic (native teammate)
- **Phase 2** (blocked by all Phase 1):
  - @claude-synthesizer: De-duplicates and merges 5 reports into prioritized findings

```
/team-review src/backend/mod.rs

/team-review src/api/

/team-review src/utils/parser.rs src/utils/validator.rs
```

### `/team-implement` - Implementation Pipeline

Pre-built 4-phase implementation pipeline with hybrid execution:

1. **@claude-architect**: Designs approach, splits into parallelizable components
2. **Codex bg x2 + @claude-worker** (parallel): Codex handles mechanical code, Claude handles complex design decisions
3. **Gemini bg x2** (parallel): Quality review + security review
4. **@claude-finalizer**: Fixes review findings, runs tests

```
/team-implement Add a caching layer to the API endpoints in src/api/

/team-implement Create a CLI tool that converts Markdown to HTML
  with support for custom extensions.

/team-implement Add WebSocket support to the chat module in src/chat/
```

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                    Claude Code CLI                        │
│                                                          │
│  /team-delegate    /team-review    /team-implement        │
│        │                │               │                │
│        ▼                ▼               ▼                │
│  ┌──────────────────────────────────────────────────┐    │
│  │          Claude (Team Lead / Orchestrator)        │    │
│  │                                                   │    │
│  │  TeamCreate → TaskCreate → TaskUpdate addBlockedBy │    │
│  │       │                                           │    │
│  │       ▼                                           │    │
│  │  agent-teams dag validate  ← Rust CLI (optional)  │    │
│  │  agent-teams dag show      ← DAG visualization    │    │
│  │       │                                           │    │
│  │       ▼  Launch agents (all parallel in ONE msg)  │    │
│  └──┬────────────┬────────────┬────────────┬────────┘    │
│     │            │            │            │             │
│     ▼            ▼            ▼            ▼             │
│  ┌──────┐  ┌──────────┐  ┌─────────┐  ┌───────────┐    │
│  │Codex │  │  Gemini  │  │@claude- │  │ @claude-  │    │
│  │(Bash │  │  (Bash   │  │reviewer │  │ worker    │    │
│  │ bg)  │  │   bg)    │  │ (Task)  │  │  (Task)   │    │
│  └──────┘  └──────────┘  └─────────┘  └───────────┘    │
│                                                          │
│  SendMessage ← receives results from @teammates         │
│  TaskOutput  ← polls results from Bash bg tasks         │
│  agent-teams dag next ← check what's unblocked next     │
└──────────────────────────────────────────────────────────┘
```

**Hybrid execution model:**

- **Background Bash** for CLI tools (no Claude wrapper overhead): `codex exec`, `gemini`, `agent-teams dag`
- **Native teammates** for Claude intelligence: `Task(team_name, name)` → spawns `@name`
- **DAG analysis** via `agent-teams` Rust CLI: cycle detection, critical path, visualization

All coordination uses Claude Code's native team tools (TeamCreate, TaskCreate, TaskUpdate, SendMessage). The `agent-teams` CLI adds graph analysis by reading the same `~/.claude/tasks/` files.

## Auto-Loaded Reference

The `delegation` skill (`skills/delegation/SKILL.md`, `user-invocable: false`) loads automatically and provides:

- Hybrid execution model (Background Bash vs Native teammates)
- CLI syntax for Codex, Gemini, and agent-teams
- DAG analysis integration workflow
- Auto-trigger rules (maps user intent to the right skill)

### `/teams-dag` - DAG Analysis

Standalone DAG analysis command. Wraps the `agent-teams` Rust CLI for quick task graph inspection:

```
/teams-dag proxy-analysis                    # Show DAG (default)
/teams-dag show macos-cleaner-dev            # Terminal visualization
/teams-dag validate impl-auth               # Cycle detection
/teams-dag next review-api                   # Show ready (unblocked) tasks
/teams-dag critical-path impl-auth          # Longest dependency chain
```

Requires `agent-teams` CLI: `cargo install --path /path/to/agent-teams --features cli`

### `/checkpoint` - Agent Session Checkpoints

Attach agent session context (who, what, why) to Git commits via Git notes. Creates a browsable audit trail of human-machine collaboration embedded directly in the repository.

```
/checkpoint                          # List recent checkpoints
/checkpoint create --team my-team    # Create checkpoint for current HEAD
/checkpoint show HEAD                # Show checkpoint details
/checkpoint show HEAD --format json  # Machine-readable JSON output
/checkpoint diff HEAD~3 HEAD         # Diff between two checkpoints
/checkpoint list --agent coder -n 10 # Filter by agent name
/checkpoint cost                     # Show aggregated token costs
/checkpoint cost --agent coder       # Cost breakdown for a specific agent
/checkpoint hook install             # Install post-commit git hook
/checkpoint hook uninstall           # Remove post-commit git hook
```

Each checkpoint captures: agent session info, team state, task snapshots, file changes (from git diff), and optionally tool calls and token usage from JSONL session files.

**Auto-trigger**: When using the `TeamOrchestrator` with auto-checkpoint enabled, checkpoints are created automatically whenever a task completes. The post-commit hook (`checkpoint hook install`) provides the same functionality for CLI-only workflows.

**Token cost aggregation**: The `cost` subcommand auto-discovers Claude Code JSONL session files and aggregates token usage across all sessions, providing per-agent cost breakdown and USD estimates.

Data is stored as Git notes under `refs/notes/agent-checkpoints` — viewable with standard `git notes` commands and survives rebase/cherry-pick.

Requires `agent-teams` CLI with checkpoint feature: `cargo install --path /path/to/agent-teams --features "cli,checkpoint"`

### `/tui` - Terminal Dashboard

Launch a ratatui-powered terminal UI for browsing team status, checkpoints, and token costs.

```
/tui                          # Launch TUI (auto-detect team)
/tui --team my-team           # Focus on a specific team
/tui --repo /path/to/repo     # Target a different repository
```

Three tabs:

| Tab | Key | Content |
|-----|-----|---------|
| Team Status | `1` | Team config, members, task list with status indicators |
| Checkpoints | `2` | Scrollable checkpoint list, detail view with Enter |
| Token Costs | `3` | Aggregated token usage, per-agent breakdown, bar chart |

Navigation: `q` quit, `Tab`/`Shift-Tab` switch tabs, `j/k` navigate, `Enter` detail, `Esc` back, `r` refresh. Data auto-refreshes every 4 seconds.

Requires `agent-teams` CLI with TUI feature: `cargo install --path /path/to/agent-teams --features "cli,tui"`

## Relationship to the Rust Library

This plugin is the **CLI-user-facing** counterpart to the `agent-teams` Rust library. They share the same `~/.claude/tasks/` persistence layer and complement each other:

| Feature | Rust Library | Plugin | Bridge |
|---------|-------------|--------|--------|
| Target audience | Rust developers | Claude Code CLI users | Both |
| Agent spawning | `AgentBackend::spawn()` | `Task` / `Bash` tools | - |
| Coordination | `Orchestrator` struct | TeamCreate/SendMessage | - |
| Task persistence | `FileTaskManager` | TaskCreate/TaskUpdate | Same files |
| DAG analysis | `DependencyGraph` | - | `agent-teams` CLI |
| Cycle detection | `detect_cycles()` | - | `dag validate` |
| Critical path | `critical_path()` | - | `dag critical-path` |
| Visualization | Mermaid/DOT/terminal | - | `dag show` |
| Checkpoints | `CheckpointStore` | - | `checkpoint` CLI |
| Auto-checkpoint | `AutoCheckpointTrigger` | - | `checkpoint hook install` |
| Token costs | `session_discovery` | - | `checkpoint cost` |
| Audit trail | Git notes storage | `/checkpoint` skill | `checkpoint create/show/list/diff` |
| TUI dashboard | `tui` module | `/tui` skill | `agent-teams tui` |

The `agent-teams` CLI binary bridges these two worlds — the plugin's team lead calls it via `Bash` to access the Rust library's DAG algorithms on task files created by Claude Code's native tools.

## License

MIT
