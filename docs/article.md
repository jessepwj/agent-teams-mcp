> **[HISTORICAL — 2026-04]** 本文是项目早期阶段的英文宣传文章，描述的是基于 TeamOrchestrator + 三后端（ClaudeCode/Codex/Gemini）的旧架构，包含 task/consensus/checkpoint 等现已移出核心的模块。当前项目已重构为以 7 个 MCP 工具为接口的 Team Mode runtime。仅作为项目演化记录保留，当前权威参见 docs/architecture-background.md。

# agent-teams: A Rust Framework for Orchestrating Heterogeneous AI Agent Teams

> Build multi-agent systems where Claude Code, Codex, and Gemini CLI collaborate as teammates — with type-safe traits, file-based coordination, and reduced backend coupling.

## The Problem: AI Agents Work Alone

Today's AI coding agents are powerful, but they work in silos. Claude Code, Codex, Gemini CLI — each has its own process model, protocol, and strengths:

- **Claude Code** excels at multi-turn, tool-rich coding sessions via its interactive SDK
- **Codex** brings persistent threads with JSON-RPC and a dedicated code-review mode
- **Gemini CLI** offers fast, stateless single-turn analysis with Google's latest models

What if you could combine them into a **team** — a lead orchestrator assigning tasks to heterogeneous agents, each running on the backend best suited to their role? A Claude Code agent writes the implementation, a Gemini agent reviews it, and a Codex agent validates the tests — all coordinated through a shared task list and inbox system.

This is what **`agent-teams`** does.

## Getting Started

Before diving into the architecture, here's a quick taste of the API:

### Prerequisites

You'll need at least one of the following CLI tools installed and authenticated:

| Tool | Install | Auth |
|------|---------|------|
| Claude Code | `npm install -g @anthropic-ai/claude-code` | `claude` (interactive login) |
| Codex | `npm install -g @openai/codex` | `OPENAI_API_KEY` or interactive login |
| Gemini CLI | `npm install -g @anthropic-ai/gemini-cli` or Homebrew | `gemini` (interactive login) |

### Minimal Example

```toml
[dependencies]
agent-teams = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
use agent_teams::prelude::*;

#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    let orch = TeamOrchestrator::builder()
        .with_gemini_cli(GeminiCliBackend::new()?)
        .build()?;

    orch.create_team("my-team", None).await?;

    let cfg = SpawnConfig::new("assistant", "You are a helpful coding assistant.");
    orch.spawn_teammate("my-team", cfg, BackendType::GeminiCli).await?;

    orch.send_input("my-team", "assistant", "What is the fastest sorting algorithm?").await?;

    // take_output_receiver() returns Option — None if already taken (take-once semantics)
    let mut rx = orch
        .take_output_receiver("my-team", "assistant")
        .await?
        .expect("receiver not yet taken");

    // Always use a timeout to avoid indefinite waits
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while let Some(output) = rx.recv().await {
            match output {
                AgentOutput::Delta(text) => print!("{text}"),
                AgentOutput::TurnComplete => { println!(); break; }
                AgentOutput::Error(e) => { eprintln!("Agent error: {e}"); break; }
                _ => {}
            }
        }
    }).await;

    if timeout.is_err() {
        eprintln!("Timed out waiting for agent response");
    }

    orch.shutdown_teammate("my-team", "assistant").await?;
    orch.delete_team("my-team").await?;
    Ok(())
}
```

## Architecture Overview

```
                    ┌──────────────────────────┐
                    │    TeamOrchestrator       │
                    │  (single entry point)     │
                    └─────┬──────┬──────┬───────┘
                          │      │      │
              ┌───────────┘      │      └───────────┐
              ▼                  ▼                  ▼
     ┌─────────────┐   ┌─────────────┐   ┌─────────────┐
     │ FileTeam     │   │ FileTask    │   │ FileInbox   │
     │ Manager      │   │ Manager     │   │ Manager     │
     └──────┬───────┘   └──────┬──────┘   └──────┬──────┘
            │                  │                  │
            ▼                  ▼                  ▼
     ~/.claude/teams/   ~/.claude/tasks/   inboxes/*.json
     {team}/config.json {team}/{id}.json

              ┌────────────────────────────┐
              │     Backend Abstraction    │
              │  AgentBackend (factory)    │
              │  AgentSession  (handle)    │
              └─────┬──────┬──────┬────────┘
                    │      │      │
        ┌───────────┘      │      └───────────┐
        ▼                  ▼                  ▼
  ClaudeCode          Codex             GeminiCli
  (cc-sdk)        (JSON-RPC)         (one-shot CLI)
```

The framework has four distinct layers:

1. **Foundation** (`error.rs`, `models/`, `util/`) — Data types, error handling, atomic file I/O
2. **Managers** (`team/`, `task/`, `messaging/`) — Trait-based managers for teams, tasks, and messaging
3. **Backend** (`backend/`) — The `AgentBackend`/`AgentSession` trait pair with three implementations
4. **Orchestrator** (`orchestrator/`) — `TeamOrchestrator` composes everything into a single API

## The Backend Abstraction: Two Traits, Three Worlds

The core insight of `agent-teams` is unifying vastly different agent runtimes behind two simple traits:

```rust
// Uses: async_trait, tokio::sync::mpsc::Receiver, and the crate's own Result type alias.

/// Factory trait: creates agent sessions for a specific backend.
#[async_trait]
pub trait AgentBackend: Send + Sync {
    fn backend_type(&self) -> BackendType;
    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>>;
}

/// A running agent session that can receive input and emit output.
#[async_trait]
pub trait AgentSession: Send + Sync {
    fn name(&self) -> &str;
    async fn send_input(&mut self, input: &str) -> Result<()>;
    fn output_receiver(&mut self) -> Option<Receiver<AgentOutput>>;
    async fn is_alive(&self) -> bool;
    async fn shutdown(&mut self) -> Result<()>;
    async fn force_kill(&mut self) -> Result<()>;
}
```

This design separates **creation** (factory) from **interaction** (session), allowing the orchestrator to be completely backend-agnostic. A `SpawnConfig` captures all the common parameters:

```rust
let config = SpawnConfig {
    name: "reviewer".into(),
    prompt: "You are a senior Rust code reviewer.".into(),
    model: Some("gemini-2.5-pro".into()),
    cwd: Some("/path/to/project".into()),
    permission_mode: Some("bypassPermissions".into()),
    ..Default::default()
};
```

### Three Process Models, One Interface

Each backend maps `SpawnConfig` to its native execution model:

| Backend | Process Model | Communication | State |
|---------|--------------|---------------|-------|
| **ClaudeCode** | Long-lived session task via `cc-sdk` | Command channel → session task → output channel | Multi-turn (SDK manages) |
| **Codex** | Persistent `codex app-server` subprocess | JSON-RPC over stdin/stdout (initialize → thread/start → turn/start) | Multi-turn (thread-based) |
| **Gemini CLI** | Ephemeral process per turn | Pipe stdin → read stdout line-by-line | Stateless (system prompt re-injected via `-p`) |

> **Note:** These process models reflect the current behavior of each tool's CLI. Upstream changes may alter these semantics.

A key design challenge was ensuring **output channel reuse**. All three backends create a single `mpsc::Sender<AgentOutput>` at `spawn()` time. For Claude Code and Codex, this sender is passed to a long-lived reader task. For Gemini CLI, the sender is **cloned** to each new ephemeral reader task, so the orchestrator's receiver stays valid across process lifetimes:

```
spawn()        → process 1 → reader task 1 → output_tx.clone()
send_input()   → kill proc 1 → process 2 → reader task 2 → output_tx.clone()
send_input()   → kill proc 2 → process 3 → reader task 3 → output_tx.clone()
                                                   ↓
                                          orchestrator's output_rx
                                          (valid for entire session)
```

### The Output Event Protocol

All backends emit the same `AgentOutput` enum:

```rust
pub enum AgentOutput {
    Message(String),    // Complete text message
    Delta(String),      // Streaming text delta
    TurnComplete,       // Agent finished a turn
    Idle,               // Agent is idle / waiting
    Error(String),      // Error occurred
}
```

A critical implementation detail addresses channel backpressure: the shared `send_agent_output` helper differentiates **control events** from **data events**:

- `TurnComplete`, `Error`, `Idle` → use `send().await` (guaranteed delivery — dropping these would cause the orchestrator to hang)
- `Delta`, `Message` → use `try_send()` (acceptable to drop under backpressure — text loss is tolerable, deadlocks are not)

The default channel capacity is 256 events. This is sufficient for most use cases; if you need to tune it, the constant `OUTPUT_CHANNEL_SIZE` is defined in each backend module.

## The Orchestrator: Composing Everything

`TeamOrchestrator` is the user-facing entry point. It composes all managers with pluggable backends via a builder pattern:

```rust
use agent_teams::backend::claude_code::ClaudeCodeBackend;
use agent_teams::backend::codex::CodexBackend;
use agent_teams::backend::gemini::GeminiCliBackend;

let orchestrator = TeamOrchestrator::builder()
    .teams_base("/path/to/teams")
    .tasks_base("/path/to/tasks")
    .with_claude_code(ClaudeCodeBackend::new())
    .with_codex(CodexBackend::new()?)
    .with_gemini_cli(GeminiCliBackend::new()?)
    .build()?;
```

From here, the full lifecycle is straightforward:

```rust
// Create a team
orchestrator.create_team("review-team", Some("Code review squad")).await?;

// Spawn heterogeneous teammates
let claude_cfg = SpawnConfig::new("implementer", "You write Rust code.");
orchestrator.spawn_teammate("review-team", claude_cfg, BackendType::ClaudeCode).await?;

let gemini_cfg = SpawnConfig {
    name: "reviewer".into(),
    prompt: "You review Rust code for correctness and style.".into(),
    model: Some("gemini-2.5-pro".into()),
    ..Default::default()
};
orchestrator.spawn_teammate("review-team", gemini_cfg, BackendType::GeminiCli).await?;

// Create and assign tasks
let task = orchestrator.create_task("review-team", CreateTaskRequest {
    subject: "Review authentication module".into(),
    description: Some("Check for SQL injection and auth bypass.".into()),
    ..Default::default()
}).await?;

orchestrator.assign_task("review-team", &task.id, "reviewer").await?;

// Send input to a specific agent
orchestrator.send_input("review-team", "reviewer", "Please review src/auth.rs").await?;

// Read output (with timeout and error handling)
let mut rx = orchestrator
    .take_output_receiver("review-team", "reviewer")
    .await?
    .expect("receiver not yet taken");

while let Some(output) = rx.recv().await {
    match output {
        AgentOutput::Delta(text) => print!("{text}"),
        AgentOutput::Error(e) => { eprintln!("Error: {e}"); break; }
        AgentOutput::TurnComplete => break,
        _ => {}
    }
}
```

## File-Based Coordination: Claude Code Compatible

The framework uses JSON files on disk for all coordination state — teams, tasks, and inboxes. This design is intentionally compatible with Claude Code's own agent teams format:

```
~/.claude/
├── teams/
│   └── review-team/
│       ├── config.json          # Team config + member list
│       └── inboxes/
│           ├── implementer.json # Per-agent inbox
│           └── reviewer.json
└── tasks/
    └── review-team/
        ├── 1.json               # Task files
        └── 2.json
```

All file operations use **atomic writes** (via temporary files and renames) and **advisory file locking** for crash safety. On Unix-like systems, this is achieved with `flock(2)`, ensuring robust concurrent access. (Note: This currently limits the framework to non-Windows platforms.)

### Task Dependency Graph

Tasks support `blocks` / `blockedBy` dependencies with **cycle detection** (DFS-based) and **automatic cascade**: when a task is completed, it's automatically removed from the `blockedBy` lists of dependent tasks, potentially unblocking them:

```rust
// Task 2 depends on Task 1
orchestrator.update_task("team", "2", TaskUpdate {
    add_blocked_by: Some(vec!["1".into()]),
    ..Default::default()
}).await?;

// Completing Task 1 automatically unblocks Task 2
orchestrator.update_task("team", "1", TaskUpdate {
    status: Some(TaskStatus::Completed),
    ..Default::default()
}).await?;
```

### Structured Messaging

The inbox system supports both plain-text messages and structured protocol messages:

```rust
// Plain message
orchestrator.send_message("team", "lead", "worker", "Please start task 3").await?;

// Structured messages (auto-generated by assign_task, send_shutdown_request, etc.)
// TaskAssignment, ShutdownRequest, ShutdownApproved,
// IdleNotification, PlanApprovalRequest, PlanApprovalResponse
```

## Why Three Backends?

Each backend has distinct strengths that make them optimal for different roles:

| Role | Best Backend | Why |
|------|-------------|-----|
| **Implementation** | Claude Code | Rich tool access (file edit, shell, web search), multi-turn state |
| **Code Review** | Gemini CLI | Fast single-turn analysis, large context window, no tool overhead |
| **Testing** | Codex | Persistent thread with dedicated code-review mode |
| **Quick Analysis** | Gemini CLI (flash) | Fastest response time, lowest latency |
| **Complex Debugging** | Claude Code | Extended thinking, interactive tool use |
| **Parallel Validation** | All three | Cross-model agreement increases confidence |

## Numbers

| Metric | Value |
|--------|-------|
| Source lines | 5,718 |
| Source files | 21 |
| Unit tests | 86 |
| Integration tests | 19 |
| Backends | 3 (Claude Code, Codex, Gemini CLI) |
| Dependencies | 14 (tokio, serde, thiserror, cc-sdk, etc.) |
| Rust edition | 2024 (`edition = "2024"` in Cargo.toml) |

## Design Principles

1. **Trait-first abstraction**: Backend differences are hidden behind `AgentBackend` + `AgentSession`. Adding a fourth backend (e.g., Aider, Cursor) requires implementing just two traits.

2. **File-based coordination**: No database, no server. JSON files with atomic writes and flock — simple, debuggable, and compatible with Claude Code's native format.

3. **Control events never drop**: The `send_agent_output` helper guarantees delivery of `TurnComplete` and `Error` events while gracefully dropping text under backpressure. This prevents deadlocks without losing liveness signals.

4. **Defensive resource cleanup**: Every backend implements `Drop` to abort reader tasks, uses `kill_on_drop(true)` for child processes, and provides both `shutdown()` (graceful) and `force_kill()` (immediate) paths.

5. **Reduced backend coupling**: The orchestrator doesn't know or care which backend runs which agent. You can swap a Claude Code agent for a Gemini agent by changing one line — the `BackendType` parameter in `spawn_teammate()`. Note that you still depend on the respective CLI tools being installed and authenticated.

## Known Limitations

- **Unix-only**: File locking uses `flock(2)`, which is not available on Windows.
- **Single receiver ownership**: `output_receiver()` uses take-once semantics — only one consumer can read an agent's output stream.
- **External CLI dependency**: Each backend requires its respective CLI tool to be installed, authenticated, and on `$PATH`.
- **No built-in retry**: If a backend process crashes, the orchestrator does not automatically restart it. The caller is responsible for re-spawning.

## What's Next

- **Streaming event adapter**: `tokio_stream::wrappers::ReceiverStream` for `async for` ergonomics
- **Session resume**: Persist and resume Codex threads across orchestrator restarts
- **Dynamic routing**: Route tasks to the optimal backend based on cost, latency, and capability
- **Web dashboard**: Real-time visualization of team state, task progress, and agent output
- **Cross-platform locking**: Windows-compatible file locking via `fs2` or similar

---

*`agent-teams` is open-source under the MIT license. Contributions welcome.*
