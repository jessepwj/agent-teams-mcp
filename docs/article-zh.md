> **[HISTORICAL — 2026-04]** 本文是项目早期阶段的中文宣传文章，描述的是基于 TeamOrchestrator + 三后端（ClaudeCode/Codex/Gemini）的旧架构，包含 task/consensus/checkpoint 等现已移出核心的模块。当前项目已重构为以 7 个 MCP 工具为接口的 Team Mode runtime。仅作为项目演化记录保留，当前权威参见 docs/architecture-background.md。

# agent-teams：编排异构 AI 智能体团队的 Rust 框架

> 构建多智能体系统，让 Claude Code、Codex 和 Gemini CLI 作为队友协作 —— 基于类型安全的 trait、文件协调机制，降低后端耦合。

## 问题：AI 智能体各自为战

当今的 AI 编程智能体功能强大，却各自为战。Claude Code、Codex、Gemini CLI —— 每个都有独立的进程模型、协议和优势：

- **Claude Code** 擅长多轮、工具丰富的编程会话，通过交互式 SDK 驱动
- **Codex** 提供持久化线程，基于 JSON-RPC 协议，带有专用的代码审查模式
- **Gemini CLI** 提供快速、无状态的单轮分析，使用 Google 最新模型

如果能把它们组成一个**团队**会怎样？一个领导编排者将任务分配给异构智能体，每个智能体运行在最适合其角色的后端上。Claude Code 智能体负责编写实现，Gemini 智能体负责审查代码，Codex 智能体负责验证测试 —— 全部通过共享的任务列表和收件箱系统协调。

这就是 **`agent-teams`** 做的事情。

## 快速开始

在深入架构之前，先体验一下 API：

### 前置条件

你至少需要安装并认证以下 CLI 工具中的一个：

| 工具 | 安装方式 | 认证方式 |
|------|---------|---------|
| Claude Code | `npm install -g @anthropic-ai/claude-code` | `claude`（交互式登录） |
| Codex | `npm install -g @openai/codex` | `OPENAI_API_KEY` 或交互式登录 |
| Gemini CLI | `npm install -g @anthropic-ai/gemini-cli` 或 Homebrew | `gemini`（交互式登录） |

### 最小示例

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

    // take_output_receiver() 返回 Option —— 如果已被取走则为 None（take-once 语义）
    let mut rx = orch
        .take_output_receiver("my-team", "assistant")
        .await?
        .expect("receiver not yet taken");

    // 始终使用超时，避免无限等待
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

## 架构总览

```
                    ┌──────────────────────────┐
                    │    TeamOrchestrator       │
                    │    （统一入口）             │
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
              │      后端抽象层             │
              │  AgentBackend （工厂）      │
              │  AgentSession （会话句柄）   │
              └─────┬──────┬──────┬────────┘
                    │      │      │
        ┌───────────┘      │      └───────────┐
        ▼                  ▼                  ▼
  ClaudeCode          Codex             GeminiCli
  (cc-sdk)        (JSON-RPC)         (一次性 CLI)
```

框架分为四个层次：

1. **基础层**（`error.rs`、`models/`、`util/`）—— 数据类型、错误处理、原子文件 I/O
2. **管理器层**（`team/`、`task/`、`messaging/`）—— 基于 trait 的团队、任务和消息管理器
3. **后端层**（`backend/`）—— `AgentBackend`/`AgentSession` trait 对及三种实现
4. **编排器层**（`orchestrator/`）—— `TeamOrchestrator` 将一切组合为统一 API

## 后端抽象：两个 Trait，三个世界

`agent-teams` 的核心洞察是：将截然不同的智能体运行时统一到两个简单的 trait 背后：

```rust
// 依赖：async_trait、tokio::sync::mpsc::Receiver 以及 crate 自身的 Result 类型别名。

/// 工厂 trait：为特定后端创建智能体会话。
#[async_trait]
pub trait AgentBackend: Send + Sync {
    fn backend_type(&self) -> BackendType;
    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>>;
}

/// 运行中的智能体会话，可以接收输入并发出输出。
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

这种设计将**创建**（工厂）与**交互**（会话）分离，使编排器完全与后端无关。`SpawnConfig` 捕获所有公共参数：

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

### 三种进程模型，一个接口

每个后端将 `SpawnConfig` 映射到其原生执行模型：

| 后端 | 进程模型 | 通信方式 | 状态管理 |
|------|---------|---------|---------|
| **ClaudeCode** | 通过 `cc-sdk` 的长生命周期会话任务 | 命令通道 → 会话任务 → 输出通道 | 多轮（SDK 管理） |
| **Codex** | 持久化 `codex app-server` 子进程 | 基于 stdin/stdout 的 JSON-RPC（initialize → thread/start → turn/start） | 多轮（基于线程） |
| **Gemini CLI** | 每轮一个临时进程 | stdin 管道 → 逐行读取 stdout | 无状态（系统提示通过 `-p` 重新注入） |

> **注意：** 这些进程模型反映了每个工具 CLI 的当前行为，上游变更可能会改变这些语义。

一个关键的设计挑战是确保**输出通道复用**。三个后端都在 `spawn()` 时创建单个 `mpsc::Sender<AgentOutput>`。对于 Claude Code 和 Codex，这个 sender 传递给长生命周期的 reader 任务。对于 Gemini CLI，sender 被**克隆**到每个新的临时 reader 任务，因此编排器的 receiver 在跨进程生命周期中始终有效：

```
spawn()        → 进程 1 → reader 任务 1 → output_tx.clone()
send_input()   → 杀死进程 1 → 进程 2 → reader 任务 2 → output_tx.clone()
send_input()   → 杀死进程 2 → 进程 3 → reader 任务 3 → output_tx.clone()
                                                   ↓
                                          编排器的 output_rx
                                          （在整个会话期间有效）
```

### 输出事件协议

所有后端发出相同的 `AgentOutput` 枚举：

```rust
pub enum AgentOutput {
    Message(String),    // 完整的文本消息
    Delta(String),      // 流式文本增量
    TurnComplete,       // 智能体完成一轮
    Idle,               // 智能体空闲/等待中
    Error(String),      // 发生错误
}
```

一个关键的实现细节涉及通道背压：共享的 `send_agent_output` 辅助函数区分**控制事件**和**数据事件**：

- `TurnComplete`、`Error`、`Idle` → 使用 `send().await`（保证送达 —— 丢弃这些事件会导致编排器挂起）
- `Delta`、`Message` → 使用 `try_send()`（可以在背压下丢弃 —— 文本丢失可以容忍，死锁不行）

默认通道容量为 256 个事件。这对大多数场景足够；如需调整，常量 `OUTPUT_CHANNEL_SIZE` 定义在每个后端模块中。

## 编排器：组合一切

`TeamOrchestrator` 是面向用户的入口。它通过 builder 模式将所有管理器与可插拔后端组合在一起：

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

之后，完整的生命周期管理非常直观：

```rust
// 创建团队
orchestrator.create_team("review-team", Some("Code review squad")).await?;

// 生成异构队友
let claude_cfg = SpawnConfig::new("implementer", "You write Rust code.");
orchestrator.spawn_teammate("review-team", claude_cfg, BackendType::ClaudeCode).await?;

let gemini_cfg = SpawnConfig {
    name: "reviewer".into(),
    prompt: "You review Rust code for correctness and style.".into(),
    model: Some("gemini-2.5-pro".into()),
    ..Default::default()
};
orchestrator.spawn_teammate("review-team", gemini_cfg, BackendType::GeminiCli).await?;

// 创建并分配任务
let task = orchestrator.create_task("review-team", CreateTaskRequest {
    subject: "Review authentication module".into(),
    description: Some("Check for SQL injection and auth bypass.".into()),
    ..Default::default()
}).await?;

orchestrator.assign_task("review-team", &task.id, "reviewer").await?;

// 向特定智能体发送输入
orchestrator.send_input("review-team", "reviewer", "Please review src/auth.rs").await?;

// 读取输出（包含超时和错误处理）
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

## 基于文件的协调：兼容 Claude Code

框架使用磁盘上的 JSON 文件存储所有协调状态 —— 团队、任务和收件箱。这种设计有意兼容 Claude Code 自身的 agent teams 格式：

```
~/.claude/
├── teams/
│   └── review-team/
│       ├── config.json          # 团队配置 + 成员列表
│       └── inboxes/
│           ├── implementer.json # 每个智能体的收件箱
│           └── reviewer.json
└── tasks/
    └── review-team/
        ├── 1.json               # 任务文件
        └── 2.json
```

所有文件操作使用**原子写入**（通过临时文件和重命名）和**建议性文件锁**来确保崩溃安全。在类 Unix 系统上，通过 `flock(2)` 实现，确保可靠的并发访问。（注意：这目前将框架限制在非 Windows 平台。）

### 任务依赖图

任务支持 `blocks` / `blockedBy` 依赖关系，带有**环检测**（基于 DFS）和**自动级联**：当任务完成时，它会自动从依赖任务的 `blockedBy` 列表中移除，可能解锁它们：

```rust
// 任务 2 依赖任务 1
orchestrator.update_task("team", "2", TaskUpdate {
    add_blocked_by: Some(vec!["1".into()]),
    ..Default::default()
}).await?;

// 完成任务 1 自动解锁任务 2
orchestrator.update_task("team", "1", TaskUpdate {
    status: Some(TaskStatus::Completed),
    ..Default::default()
}).await?;
```

### 结构化消息

收件箱系统支持纯文本消息和结构化协议消息：

```rust
// 纯文本消息
orchestrator.send_message("team", "lead", "worker", "Please start task 3").await?;

// 结构化消息（由 assign_task、send_shutdown_request 等自动生成）
// TaskAssignment、ShutdownRequest、ShutdownApproved、
// IdleNotification、PlanApprovalRequest、PlanApprovalResponse
```

## 为什么是三个后端？

每个后端都有独特的优势，使其最适合不同的角色：

| 角色 | 最佳后端 | 原因 |
|------|---------|------|
| **代码实现** | Claude Code | 丰富的工具访问（文件编辑、Shell、网页搜索），多轮状态 |
| **代码审查** | Gemini CLI | 快速单轮分析，大上下文窗口，无工具开销 |
| **测试验证** | Codex | 持久化线程，带专用代码审查模式 |
| **快速分析** | Gemini CLI (flash) | 最快响应时间，最低延迟 |
| **复杂调试** | Claude Code | 扩展思考，交互式工具使用 |
| **并行验证** | 三者皆可 | 跨模型一致性提升信心 |

## 数据概览

| 指标 | 数值 |
|------|------|
| 源代码行数 | 5,718 |
| 源文件数 | 21 |
| 单元测试 | 86 |
| 集成测试 | 19 |
| 后端数 | 3（Claude Code、Codex、Gemini CLI） |
| 依赖数 | 14（tokio、serde、thiserror、cc-sdk 等） |
| Rust edition | 2024（Cargo.toml 中 `edition = "2024"`） |

## 设计原则

1. **Trait 优先的抽象**：后端差异隐藏在 `AgentBackend` + `AgentSession` 背后。添加第四个后端（如 Aider、Cursor）只需实现两个 trait。

2. **基于文件的协调**：无数据库，无服务器。JSON 文件配合原子写入和 flock —— 简单、可调试，且兼容 Claude Code 的原生格式。

3. **控制事件永不丢失**：`send_agent_output` 辅助函数保证 `TurnComplete` 和 `Error` 事件的送达，同时在背压下优雅地丢弃文本。这在不丢失活跃信号的前提下防止死锁。

4. **防御性资源清理**：每个后端都实现 `Drop` 来终止 reader 任务，使用 `kill_on_drop(true)` 管理子进程，并提供 `shutdown()`（优雅关闭）和 `force_kill()`（立即终止）两种路径。

5. **降低后端耦合**：编排器不知道也不关心哪个后端运行哪个智能体。你可以通过更改一行代码 —— `spawn_teammate()` 中的 `BackendType` 参数 —— 将 Claude Code 智能体替换为 Gemini 智能体。注意，你仍然依赖于相应的 CLI 工具已安装并已认证。

## 已知限制

- **仅限 Unix**：文件锁使用 `flock(2)`，在 Windows 上不可用。
- **单接收者所有权**：`output_receiver()` 使用 take-once 语义 —— 只有一个消费者可以读取智能体的输出流。
- **外部 CLI 依赖**：每个后端需要其对应的 CLI 工具已安装、已认证并在 `$PATH` 中。
- **无内置重试**：如果后端进程崩溃，编排器不会自动重启。调用者负责重新生成。

## 下一步

- **流式事件适配器**：`tokio_stream::wrappers::ReceiverStream`，提供 `async for` 的人体工程学
- **会话恢复**：跨编排器重启持久化和恢复 Codex 线程
- **动态路由**：根据成本、延迟和能力将任务路由到最优后端
- **Web 仪表盘**：团队状态、任务进度和智能体输出的实时可视化
- **跨平台锁**：通过 `fs2` 或类似方案实现 Windows 兼容的文件锁

---

*`agent-teams` 基于 MIT 许可证开源，欢迎贡献。*
