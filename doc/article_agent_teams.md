> **[HISTORICAL — 2026-04]** 本文是项目早期的中文技术文章，描述的是基于 TeamOrchestrator + task DAG + 共识协议 + ConversationMemory 的旧架构，包含 consensus/checkpoint/TUI 等现已移出产品核心的模块。当前项目已重构为以 7 个 MCP 工具为接口的 Team Mode runtime。仅作为项目演化记录保留，当前权威参见 docs/architecture-background.md。

# agent-teams：用 Rust 编排多 AI Agent 协作

在 AI Agent 快速发展的今天，单一 Agent 的能力已经无法满足复杂任务的需求。无论是代码审查、多步骤工程任务，还是需要多个专家视角的决策场景，多 Agent 协作都成为了必然趋势。然而，如何优雅地编排多个 Agent、管理它们的任务依赖、协调消息传递，并确保决策的一致性，是一个充满挑战的工程问题。

`agent-teams` 是一个用 Rust 构建的通用多 Agent 协作框架，它借鉴了 Claude Code Agent Teams 的架构设计，提供了完整的团队管理、任务编排、消息路由和共识协议能力。更重要的是，它通过插件化的后端抽象，同时支持 **Claude Code**（基于 `cc-sdk`）、**OpenAI Codex**（通过 JSON-RPC 2.0）以及 **Google Gemini CLI**（单次调用管道），让开发者能够根据场景需求灵活选择 AI 能力提供者。

## 一、为什么需要多 Agent 协作框架？

传统的单 Agent 系统在面对复杂任务时存在明显瓶颈：

1. **任务拆解困难**：一个大型工程任务（如重构代码库）需要人工拆解为多个子任务，单一 Agent 难以并行处理
2. **缺乏专业分工**：代码生成、测试、文档撰写、安全审计等环节需要不同的专家 Agent
3. **决策可靠性不足**：关键决策依赖单一模型输出，容易出现偏见或错误
4. **无状态限制**：许多 CLI 工具（如 Gemini）不支持多轮对话，缺乏上下文记忆

`agent-teams` 通过以下机制解决这些问题：
- **任务 DAG**：自动处理任务依赖关系，支持拓扑排序和关键路径分析
- **角色分工**：团队领导（Team Lead）与专家队友（Teammates）各司其职
- **共识协议**：多数表决、加权投票、一致同意等策略确保决策质量
- **持久化记忆**：为无状态后端提供文件化的对话历史，模拟多轮交互能力

## 二、架构概览：Backend + Session 分离与文件协议

### 核心设计：双 trait 抽象

`agent-teams` 的后端层采用了经典的**工厂模式 + 会话模式**设计：

```rust
#[async_trait]
pub trait AgentBackend: Send + Sync {
    fn backend_type(&self) -> BackendType;
    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>>;
}

#[async_trait]
pub trait AgentSession: Send {
    async fn send_input(&mut self, input: String) -> Result<AgentOutputStream>;
    async fn shutdown(&mut self) -> Result<()>;
    fn is_alive(&self) -> bool;
}
```

- **`AgentBackend`**：负责创建 Agent 会话的工厂，每种后端（Claude Code / Codex / Gemini）实现一次
- **`AgentSession`**：管理单个 Agent 的生命周期，提供输入发送、输出流读取和优雅关闭接口

这种分离带来两大优势：
1. **生命周期解耦**：`Backend` 可以全局共享（`Arc<dyn AgentBackend>`），而 `Session` 只属于特定 Agent 实例
2. **多态灵活性**：新增后端只需实现两个 trait，无需修改上层编排逻辑

### 文件协议：分布式状态管理

为了支持跨进程协作（团队成员可能运行在不同机器上），`agent-teams` 完全基于文件系统进行状态同步：

```
~/.claude/teams/{team-name}/
├── config.json          # 团队配置，包含成员列表
└── inbox/
    └── {agent-name}.json  # 每个 Agent 的消息收件箱

~/.claude/tasks/{team-name}/
├── task_001.json        # 任务详情：状态、依赖、执行者
├── task_002.json
└── ...
```

所有操作（创建任务、更新状态、发送消息）都通过**原子写入 + 文件锁**保证并发安全，这使得 Agent 可以在不同进程甚至不同主机上运行，只需共享文件系统即可协同工作。

## 三、三大后端：Claude Code、Codex、Gemini CLI

### 1. Claude Code Backend：交互式专家

基于 Anthropic 官方的 `cc-sdk`，`ClaudeCodeBackend` 提供了最丰富的交互能力：

```rust
impl AgentSession for ClaudeCodeSession {
    async fn send_input(&mut self, input: String) -> Result<AgentOutputStream> {
        let stream = self.client.send_message(&input).await?;
        Ok(stream)
    }
}
```

特性：
- **流式输出**：逐步返回推理内容、工具调用和执行结果
- **工具权限管理**：通过 `allowed_tools` 和 `permission_mode` 控制沙箱安全
- **多轮对话**：原生支持上下文保持，适合复杂交互场景

### 2. Codex Backend：JSON-RPC 标准化

通过 JSON-RPC 2.0 协议与 OpenAI Codex 进程通信，适配团队已有的 Codex 工具链：

```rust
pub struct CodexBackend {
    codex_bin: PathBuf,
    default_reasoning: String,  // "low" | "medium" | "high" | "xhigh"
}
```

特性：
- **推理等级控制**：支持四档推理深度（`reasoning_effort`），平衡速度与质量
- **子进程隔离**：每个 Agent 独立进程，故障互不影响
- **协议标准化**：符合 JSON-RPC 2.0 规范，易于调试和扩展

### 3. Gemini CLI Backend：轻量级单次调用

针对 Google Gemini CLI 工具的适配器，通过标准输入输出管道完成一次性查询：

```rust
impl GeminiCliBackend {
    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>> {
        let bin = which::which("gemini-cli")?;
        
        let memory = if let Some(mem_cfg) = config.memory_config {
            Some(self.memory_mgr.load_or_create(team, agent_name, mem_cfg)?)
        } else {
            None
        };
        
        Ok(Box::new(GeminiCliSession { bin, memory, ... }))
    }
}
```

特性：
- **零依赖**：直接调用系统命令，无需 SDK
- **记忆补偿**：通过 `ConversationMemory` 为无状态 CLI 提供伪多轮能力（见第六节）
- **快速启动**：进程生命周期仅限单次调用，适合高频短任务

## 四、任务 DAG：拓扑排序与可视化

### DependencyGraph：依赖关系分析

`DependencyGraph` 基于任务快照构建有向无环图（DAG），核心功能包括：

```rust
impl DependencyGraph {
    /// 检测添加依赖是否会产生环
    pub fn would_create_cycle(&self, task_id: &str, depends_on: &str) -> bool {
        // BFS 遍历，检查 depends_on 能否回溯到 task_id
    }
    
    /// Kahn 算法拓扑排序，返回可执行顺序
    pub fn topological_order(&self) -> Result<Vec<String>> {
        // 入度为 0 的节点优先入队
    }
    
    /// 计算关键路径（最长依赖链）
    pub fn critical_path(&self) -> Vec<String> {
        // 动态规划求解
    }
}
```

实战示例：

```rust
// 创建任务链：design → implement → test
//                ↑
//          setup-ci ──┘

let design = orch.create_task("team", CreateTaskRequest {
    subject: "Design API schema".into(),
    ..Default::default()
}).await?;

let implement = orch.create_task("team", CreateTaskRequest {
    subject: "Implement endpoints".into(),
    blocked_by: vec![design.id.clone()],
    ..Default::default()
}).await?;

// 构建 DAG 并分析
let tasks = orch.list_tasks("team", TaskFilter::default()).await?;
let graph = DependencyGraph::from_tasks(&tasks);

// 输出拓扑顺序
let order = graph.topological_order()?;
println!("执行顺序: {:?}", order);  // ["design", "setup-ci", "implement", "test"]

// 识别关键路径
let critical = graph.critical_path();
println!("关键路径: {:?}", critical);  // ["design", "implement", "test"]
```

### 终端可视化

框架内置了 `render_graph` 方法，可在终端输出 ASCII 艺术风格的 DAG 图：

```
design          setup-ci
  │                │
  └────→ implement ←┘
            │
         test
```

这对于调试复杂任务依赖关系非常有用。

## 五、共识协议：四种决策策略

当多个 Agent 针对同一问题给出不同答案时，`consensus` 模块提供了纯函数的决策算法：

### 1. Majority（多数表决）

要求超过半数 Agent 给出相同答案：

```rust
pub fn resolve_majority(responses: &[AgentResponse]) -> ConsensusResult {
    let counts = responses.iter()
        .filter(|r| !r.timed_out)
        .fold(HashMap::new(), |mut acc, r| {
            *acc.entry(&r.content).or_insert(0) += 1;
            acc
        });
    
    let threshold = responses.len() / 2;
    let decision = counts.iter()
        .find(|(_, &count)| count > threshold)
        .map(|(content, _)| content.to_string());
    
    // ...
}
```

适用场景：二选一决策（如"是否合并 PR"）。

### 2. Weighted（加权投票）

根据 Agent 的置信度或历史准确率进行加权：

```rust
let weighted_votes: HashMap<String, f32> = responses.iter()
    .filter(|r| !r.timed_out)
    .fold(HashMap::new(), |mut acc, r| {
        *acc.entry(r.content.clone()).or_insert(0.0) += r.weight;
        acc
    });
```

适用场景：专家意见不平等（如资深审查员权重更高）。

### 3. Unanimous（一致同意）

所有 Agent 必须给出相同答案：

```rust
let unique_answers: HashSet<_> = responses.iter()
    .filter(|r| !r.timed_out)
    .map(|r| &r.content)
    .collect();

if unique_answers.len() == 1 {
    // 达成共识
}
```

适用场景：安全关键决策（如"是否删除生产数据库"）。

### 4. HumanInTheLoop（人类最终决策）

收集所有 Agent 意见后，暂停并等待人类审核：

```rust
ConsensusResult {
    decision: None,
    responses: all_responses,
    consensus_reached: false,
    strategy_used: ConsensusStrategy::HumanInTheLoop,
}
```

适用场景：模型能力不确定或伦理敏感的决策。

### 使用示例

```rust
let request = ConsensusRequest {
    prompt: "这段代码是否存在 SQL 注入风险？".into(),
    agents: vec!["security-expert".into(), "code-reviewer".into()],
    strategy: ConsensusStrategy::Weighted,
    timeout: Duration::from_secs(30),
    weights: Some(HashMap::from([
        ("security-expert".into(), 2.0),
        ("code-reviewer".into(), 1.0),
    ])),
};

let result = orchestrator.request_consensus(request).await?;
if result.consensus_reached {
    println!("共识结果: {}", result.decision.unwrap());
}
```

## 六、Agent 记忆：让无状态 CLI 拥有上下文

### ConversationMemory：滑动窗口历史

对于 Gemini CLI 这类单次调用工具，`ConversationMemory` 提供了文件化的对话历史：

```rust
pub struct ConversationMemory {
    turns: Vec<TurnRecord>,
    config: MemoryConfig,
}

impl ConversationMemory {
    pub fn add_user(&mut self, content: String) {
        self.turns.push(TurnRecord {
            role: Role::User,
            content,
            timestamp: Utc::now(),
        });
        self.evict_if_needed();
    }
    
    pub fn format_context(&self) -> String {
        self.turns.iter()
            .map(|t| format!("{}: {}", t.role, t.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
```

### MemoryManager：持久化与恢复

```rust
pub struct MemoryManager {
    base_dir: PathBuf,
}

impl MemoryManager {
    pub fn load_or_create(&self, team: &str, agent: &str, config: MemoryConfig) 
        -> Result<ConversationMemory> {
        let path = self.memory_path(team, agent);
        if path.exists() {
            let _lock = FileLock::new(&path)?;
            let content = fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(ConversationMemory::new(config))
        }
    }
}
```

### 实际效果

当向 Gemini CLI Agent 发送第二条消息时：

```rust
// 第一轮
session.send_input("什么是 Rust 的所有权？").await?;

// 第二轮（自动注入历史）
session.send_input("能举个例子吗？").await?;
// 实际发送给 CLI 的输入：
// """
// User: 什么是 Rust 的所有权？
// Assistant: [第一轮的回答...]
//
// User: 能举个例子吗？
// """
```

通过这种方式，无状态工具也能"记住"之前的对话，实现伪多轮交互。

## 七、TeamOrchestrator：一站式编排 API

`TeamOrchestrator` 是框架的门面，整合了所有底层能力：

```rust
pub struct TeamOrchestrator {
    team_mgr: FileTeamManager,
    task_mgr: FileTaskManager,
    inbox_mgr: FileInboxManager,
    backends: HashMap<BackendType, Arc<dyn AgentBackend>>,
    sessions: Arc<Mutex<HashMap<String, Box<dyn AgentSession>>>>,
    memories: Arc<Mutex<HashMap<String, ConversationMemory>>>,
    memory_mgr: MemoryManager,
}
```

核心方法：

```rust
impl TeamOrchestrator {
    // 团队生命周期
    async fn create_team(&self, name: &str, desc: Option<&str>) -> Result<TeamConfig>;
    async fn delete_team(&self, name: &str) -> Result<()>;
    
    // Agent 生命周期
    async fn spawn_teammate(&self, team: &str, config: SpawnConfig, backend: BackendType) -> Result<()>;
    async fn send_input(&self, team: &str, agent: &str, input: String) -> Result<AgentOutputStream>;
    async fn shutdown_teammate(&self, team: &str, agent: &str) -> Result<()>;
    
    // 任务管理
    async fn create_task(&self, team: &str, req: CreateTaskRequest) -> Result<TaskFile>;
    async fn update_task(&self, team: &str, task_id: &str, update: TaskUpdate) -> Result<()>;
    async fn list_tasks(&self, team: &str, filter: TaskFilter) -> Result<Vec<TaskFile>>;
    
    // 消息传递
    async fn send_message(&self, team: &str, from: &str, to: &str, content: String) -> Result<()>;
    async fn read_inbox(&self, team: &str, agent: &str) -> Result<Vec<InboxMessage>>;
    
    // 共识协议
    async fn request_consensus(&self, team: &str, req: ConsensusRequest) -> Result<ConsensusResult>;
}
```

## 八、实战示例：3 步创建代码审查团队

```rust
use agent_teams::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. 初始化编排器，注册后端
    let orch = TeamOrchestrator::builder()
        .teams_base("~/.claude/teams")
        .tasks_base("~/.claude/tasks")
        .register_backend(BackendType::ClaudeCode, ClaudeCodeBackend::new()?)
        .register_backend(BackendType::GeminiCli, GeminiCliBackend::new()?)
        .build()?;
    
    // 2. 创建团队并生成 3 个专家 Agent
    orch.create_team("code-review", Some("代码审查团队")).await?;
    
    orch.spawn_teammate("code-review", 
        SpawnConfig::new("security-expert", "你是安全审计专家"),
        BackendType::ClaudeCode
    ).await?;
    
    orch.spawn_teammate("code-review",
        SpawnConfig::builder("perf-analyst", "你专注于性能分析")
            .memory_config(MemoryConfig::default())
            .build(),
        BackendType::GeminiCli
    ).await?;
    
    orch.spawn_teammate("code-review",
        SpawnConfig::new("style-checker", "你负责代码风格检查"),
        BackendType::ClaudeCode
    ).await?;
    
    // 3. 创建审查任务并分配
    let task = orch.create_task("code-review", CreateTaskRequest {
        subject: "审查 auth.rs 模块".into(),
        description: Some("重点关注 SQL 注入和性能".into()),
        ..Default::default()
    }).await?;
    
    orch.update_task("code-review", &task.id, TaskUpdate {
        owner: Some("security-expert".into()),
        ..Default::default()
    }).await?;
    
    // 4. 发送审查请求并等待结果
    let mut stream = orch.send_input(
        "code-review",
        "security-expert",
        format!("请审查任务 {} 中的代码", task.id)
    ).await?;
    
    while let Some(output) = stream.next().await {
        match output? {
            AgentOutput::Content(text) => println!("{}", text),
            AgentOutput::ToolUse { name, input } => {
                println!("[工具调用] {}: {:?}", name, input);
            }
            AgentOutput::Completed => break,
        }
    }
    
    // 5. 多 Agent 共识决策
    let consensus = orch.request_consensus("code-review", ConsensusRequest {
        prompt: "这段代码是否可以合并到主分支？".into(),
        agents: vec!["security-expert".into(), "perf-analyst".into(), "style-checker".into()],
        strategy: ConsensusStrategy::Unanimous,
        timeout: Duration::from_secs(60),
        weights: None,
    }).await?;
    
    if consensus.consensus_reached {
        println!("✓ 审查通过，可以合并");
    } else {
        println!("✗ 审查未通过，需要修改");
    }
    
    Ok(())
}
```

## 九、总结与展望

`agent-teams` 通过精心设计的抽象层和文件协议，实现了一个**生产级**的多 Agent 协作框架。其核心优势包括：

1. **后端无关性**：通过 `AgentBackend` 和 `AgentSession` trait 解耦，轻松接入新模型
2. **分布式友好**：基于文件系统的状态管理，天然支持跨进程/跨主机协作
3. **工程化完备**：任务 DAG、共识协议、记忆管理等生产特性开箱即用
4. **类型安全**：Rust 的强类型系统避免了常见的运行时错误

### 未来发展方向

- **Web Dashboard**：已通过 `dashboard` feature 提供可视化监控面板（基于 Axum）
- **更多后端**：计划支持 Anthropic API、OpenAI API、本地 LLM（llama.cpp）
- **智能路由**：根据任务类型自动选择最优后端（`BackendRouter`）
- **容错机制**：Agent 崩溃自动重启、任务重试策略
- **性能优化**：任务并行执行、批量消息传递

如果你正在构建需要多个 AI Agent 协同工作的系统——无论是自动化 DevOps、智能客服团队，还是多角色内容创作平台——`agent-teams` 都能为你提供坚实的基础设施。框架的设计哲学是"**Simple things simple, complex things possible**"：简单场景 3 行代码启动，复杂编排也有完整的工具链支持。

项目地址：[github.com/ZhangHanDong/agent-teams](https://github.com/ZhangHanDong/agent-teams)  
欢迎通过 Issue 和 PR 参与贡献，让我们一起构建更强大的 AI Agent 协作生态！