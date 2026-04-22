> **[HISTORICAL — 2026-04]** 本文档记录的是基于 Claude Code 原生 Task/TeamCreate 工具的 plugin 设计洞察（skills/commands 框架、Hybrid 执行模型、DAG CLI 等），对应的是本项目重构前的 plugin workflow 方向。当前项目已转向 MCP-first Team Mode runtime，不再以 plugin workflow 为核心。部分洞察（如上下文经济原则、Codex CLI 用法）仍有参考价值，作为演化记录保留。

# Agent-Teams Plugin Design Insights

Development insights from building the Claude Code plugin for multi-agent team orchestration.

## 1. Context Economy in Team Systems

Each teammate is an independent `general-purpose` agent with its own context window. The team lead should NOT read all files and embed them into teammate prompts — this duplicates data across contexts.

**Anti-pattern (context explosion):**
```
Team lead reads 17 files → embeds in teammate prompt → teammate receives copy
Result: same data occupies 2 context windows, team lead hits "Compacting conversation..."
```

**Correct pattern (context delegation):**
```
Team lead passes file paths → teammate reads files itself with Read tool
Result: team lead context stays light, teammate context focused on its task
```

**Rule**: Team lead sends "instructions + file paths", not "instructions + file contents".

## 2. Native Teams vs Direct Bash

Calling `codex exec` or `gemini` directly from the team lead via Bash is NOT using native Teams. It's fire-and-forget with no state tracking, no dependency management, no failure recovery.

**Direct Bash (not a team):**
```
Claude → Bash("codex exec ...") → stdout → Claude parses result
Claude → Bash("gemini ...") → stdout → Claude parses result
```

**Native Teams (proper orchestration):**
```
Claude (team lead)
  ├── TeamCreate → creates team
  ├── TaskCreate → creates DAG with dependencies
  ├── Task(team_name, name: "codex-reviewer") → spawns @codex-reviewer
  │     └── Teammate internally runs: Bash("codex exec ...")
  ├── Task(team_name, name: "gemini-reviewer") → spawns @gemini-reviewer
  │     └── Teammate internally runs: Bash("gemini ...")
  └── SendMessage ← receives results from teammates
```

**Key difference**: The `team_name` parameter on the `Task` tool registers the spawned agent as a team member (visible as `@name` in UI), enabling SendMessage/TaskUpdate coordination.

## 3. The `team_name` Parameter is Critical

Without `team_name` in the Task tool call, the spawned agent is just a regular subagent — no `@` prefix, no team membership, no SendMessage capability.

```
Task tool parameters for native Teams:
  - subagent_type: "general-purpose"
  - team_name: "review-backend"     ← THIS makes it a team member
  - name: "gemini-reviewer"         ← THIS becomes @gemini-reviewer
```

## 4. Command File Size vs Effectiveness

Claude Code loads the entire command `.md` file into context when a slash command is invoked. Oversized commands leave no room for actual work.

**Evolution:**
```
v1: 886 lines / 29KB  → Compacting immediately
v2: 176 lines / 7.7KB → Still heavy with embedded prompt templates
v3: 232 lines / 9KB   → Lean instructions, teammates read their own files
```

**Design principle**: Commands should be "minimum viable instructions". Claude is smart enough — it doesn't need step-by-step prompt templates with full examples. A concise workflow + teammate prompt skeleton is sufficient.

## 5. Skills Replace Commands (Updated 2026)

`commands/*.md` is **legacy**. Skills (`skills/*/SKILL.md`) now handle everything:

| Frontmatter | User invocable | Claude auto-invokes | Use case |
|-------------|---------------|-------------------|----------|
| (default) | Yes (`/name`) | Yes | Workflow skills (team-review, team-implement) |
| `disable-model-invocation: true` | Yes | No | Side-effect commands (teams-dag) |
| `user-invocable: false` | No | Yes | Reference knowledge (delegation patterns) |

**Migration**: Moved all `commands/*.md` → `skills/*/SKILL.md` with appropriate frontmatter. The `commands/` directory was deleted. The skill description field is key — Claude reads it to decide when to auto-load or suggest the skill.

## 6. Codex CLI Correct Usage

The Codex CLI evolved from `-q` (quiet) flag to a subcommand architecture:

```bash
# WRONG (old/invalid)
codex -q "PROMPT" --approval-mode full-auto

# CORRECT
codex exec "PROMPT"              # Non-interactive execution
codex review "PROMPT"            # Code review (reads git diff)
codex review --base main "PROMPT" # Review against branch
```

Key subcommands: `exec` (general), `review` (code review), `app-server` (JSON-RPC for libraries).

## 7. Graceful Degradation

Commands should always check tool availability and fall back:

```bash
which codex 2>/dev/null && echo "CODEX_AVAILABLE=true" || echo "CODEX_AVAILABLE=false"
which gemini 2>/dev/null && echo "GEMINI_AVAILABLE=true" || echo "GEMINI_AVAILABLE=false"
```

If Codex/Gemini aren't installed, teammates do the work directly as Claude agents using native tools (Read/Edit/Write/Bash). The workflow (team creation, task DAG, coordination) remains identical.

## 8. Auto-Trigger Design

SKILL.md can include trigger rules that make Claude proactively suggest team commands:

```
| User Intent         | Command          |
|---------------------|------------------|
| "review", "审查"    | /team-review     |
| "implement" + complex | /team-implement |
```

This is a "soft trigger" — Claude's semantic understanding matches user intent to commands. It's not programmatic pattern matching. Include "When NOT to trigger" rules to prevent over-engineering simple tasks.

## 9. Plugin Installation

```bash
# Development/testing (recommended for iteration)
claude --plugin-dir /path/to/plugin

# Production (via plugin manager, inside Claude Code session)
/plugin marketplace add /path/to/plugin
/plugin install agent-teams
```

Note: `claude plugin add` does NOT exist. Plugin management happens inside Claude Code sessions via `/plugin` slash commands, or via `--plugin-dir` CLI flag.

## 10. Hybrid Execution Model: Bash Background + Native Teammates

Wrapping every CLI tool call (codex, gemini) in a Claude teammate adds unnecessary overhead — a full Claude agent context just to run a shell command. The hybrid model eliminates this:

| Needs Claude intelligence? | Execution | Tool |
|---------------------------|-----------|------|
| No (CLI tool call) | Background Bash | `Bash(run_in_background: true)` |
| Yes (design, synthesis, complex edits) | Native teammate | `Task(team_name, name)` |

**Performance difference:**
```
Pure native Teams (all Claude wrappers):
  Team lead → spawn @codex-reviewer (Claude agent) → agent runs Bash("codex exec ...") → SendMessage back
  Overhead: Claude agent startup + context allocation + tool calling round-trip

Hybrid model:
  Team lead → Bash("codex exec ...", run_in_background: true) → poll TaskOutput
  Overhead: just the CLI process itself
```

This enables 4+ parallel background tasks in a single message, with native teammates reserved for tasks that genuinely need Claude's intelligence (design, synthesis, complex multi-step edits).

**Critical rule**: Track ALL tasks with TaskCreate/TaskUpdate DAG regardless of execution path. Background Bash tasks and native teammates share the same task list.

## 11. Claude Agents Must Participate as Workers

In a multi-agent team, Claude should not only serve as team lead and synthesizer — it should also contribute at least one `@claude-worker` or `@claude-reviewer` that does actual implementation or review work alongside Codex/Gemini.

**Anti-pattern (Claude = coordination only):**
```
Team lead (Claude) → coordinates
  ├── Codex → implements
  ├── Gemini → reviews
  └── @claude-synthesizer → merges reports (no original analysis)
```

**Correct pattern (Claude = active participant):**
```
Team lead (Claude) → coordinates
  ├── Codex bg → implements mechanical parts
  ├── Gemini bg → reviews architecture/security
  ├── @claude-reviewer → reviews Rust idioms, ownership, domain logic  ← actual work
  └── @claude-synthesizer → merges all reports
```

**Why**: Claude excels at things Codex/Gemini can't do well — understanding Rust ownership semantics, lifetime correctness, domain-specific logic validation, and nuanced design trade-offs. A team without a Claude worker misses these strengths.

## 12. Shared Persistence: Rust DAG + Claude Code Native Tasks

The Rust library's `DependencyGraph` (`src/task/graph.rs`) and Claude Code's native task system both persist to the same file path:

```
~/.claude/tasks/{team-name}/{task-id}.json
```

This means they are **complementary, not competing**:

| Capability | Claude Code Native | Rust `DependencyGraph` |
|-----------|-------------------|----------------------|
| Create/update tasks | TaskCreate/TaskUpdate | `FileTaskManager` |
| Dependency tracking | `addBlockedBy` | `add_dependency()` |
| Cycle detection | None | `detect_cycles()` |
| Topological sort | None | `topological_sort()` |
| Critical path | None | `critical_path()` |
| Visualization | None | Mermaid, DOT, terminal |

**Integration path**: The Rust library can provide pre-execution validation (cycle detection) and post-execution analysis (critical path, visualization) for DAGs created by Claude Code's native tools — reading and writing the same task files.

## 13. CLI Binary for DAG Analysis — Implemented

The `agent-teams` CLI binary bridges Claude Code's native task system with Rust DAG algorithms. It reads `~/.claude/tasks/{team}/*.json` directly — the same files Claude Code creates via TaskCreate/TaskUpdate.

**Installation**: `cargo install --path /path/to/agent-teams --features cli`

**Commands**:
```bash
agent-teams dag validate --team review-api       # Cycle detection
agent-teams dag show --team review-api           # Terminal visualization (ANSI)
agent-teams dag show --team review-api -f plain  # No ANSI (for logging)
agent-teams dag show --team review-api -f mermaid  # Mermaid diagram
agent-teams dag show --team review-api -f dot    # Graphviz DOT
agent-teams dag next --team review-api           # Show unblocked ready tasks
agent-teams dag critical-path --team review-api  # Longest dependency chain
```

**Plugin integration** — team lead calls CLI via Bash after creating DAG:
```
Step 6: TaskCreate all tasks + TaskUpdate addBlockedBy
Step 7: Bash("agent-teams dag validate --team impl-auth")  ← catch cycles
        Bash("agent-teams dag show --team impl-auth")      ← show user the DAG
Step 8: Launch agents based on dag next output
```

**Architecture**: The CLI is a thin wrapper over the library's `DependencyGraph::from_tasks()`. It reads task JSON files synchronously (no async needed), builds the graph, and outputs analysis. The `clap` dependency is behind an optional `cli` feature flag to avoid bloating the library.

```toml
# Cargo.toml
[features]
cli = ["clap"]

[[bin]]
name = "agent-teams"
path = "src/bin/agent_teams.rs"
required-features = ["cli"]
```

**Tested against real Claude Code tasks**: Successfully parsed and visualized task DAGs from actual Claude Code team sessions (`macos-cleaner-dev` with 23 tasks, `proxy-analysis` with 7 tasks showing Phase 0 → Phase 1 dependency structure).

## 14. Checkpoint — Agent Session Context as Git-Native Version Data

When agents generate code and it gets committed, the *why* — session context, prompts, tool calls, token usage, files accessed — is normally lost. **Checkpoints** solve this by attaching structured JSON metadata to Git commits via **Git notes** (`refs/notes/agent-checkpoints`).

### Why Git Notes?

| Alternative | Problem |
|-------------|---------|
| Git trailers | Limited to key-value pairs, can't store structured JSON |
| Custom refs | Manual management, no natural 1:1 commit mapping |
| `.git/` subdirectories | Not standard Git objects, no replication |
| Separate database | Decoupled from repo, loses git-native queryability |

Git notes are first-class Git objects that attach arbitrary data to commits without modifying the commit SHA. They survive rebase/cherry-pick, support custom namespaces, and are queryable via both `git2` and the `git notes` CLI.

### Two-Tier Data Model

- **Core** (always, <10KB): session info, commit, branch, files (from git diff), team state, task summaries
- **Extended** (opt-in, 10-100KB): tool_calls, token_usage (parsed from Claude Code JSONL session files)

This keeps the default fast (<10KB per note) while allowing deep auditing when needed via `--extended`.

### Feature Gating

The checkpoint system requires `git2` (~30s compile) and `sha2`. These are behind the `checkpoint` feature flag so users who only need DAG analysis don't pay the compile cost:

```toml
[features]
checkpoint = ["git2", "sha2"]
```

Install: `cargo install --path . --features "cli,checkpoint"`

### Architecture

```
Data Sources → CheckpointCollector → Checkpoint struct → CheckpointStore → Git Notes
                ├── git2 (HEAD, branch, diff)
                ├── ~/.claude/teams/{team}/config.json
                ├── ~/.claude/tasks/{team}/*.json
                └── ~/.claude/projects/{hash}/{session}.jsonl (optional)
```

The `CheckpointCollector` gathers data from multiple sources, the `CheckpointStore` handles Git notes I/O, and `CheckpointQuery` provides filtering, branch-scoped queries, and diff computation.

### CLI Commands

```bash
agent-teams checkpoint create --agent test-agent [--team my-team] [--model claude-opus-4-6]
agent-teams checkpoint show HEAD [--format summary|json]
agent-teams checkpoint list [--branch main] [--agent coder] [-n 20] [--format table|json]
agent-teams checkpoint diff HEAD~1 HEAD
```

### Plugin Integration

The `/checkpoint` skill wraps the CLI, following the same pattern as `/teams-dag`. The team lead can create checkpoints after completing work, building a browsable audit trail of human-machine collaboration embedded directly in the repository.
