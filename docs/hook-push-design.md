> **[HISTORICAL — 2026-04]** 本文档描述的是基于 Claude Code Hook / Stop Hook 推送消息的方案研究，以及方案 D（tokio::Notify 内部通知）的设计构想。当前实现已采用 `--input-format stream-json` NDJSON 注入 + idle 闸门方案，与本文分析的路径不同。仅作为项目演化记录保留，当前架构参见 docs/architecture-background.md。

# Hook-Based Message Push Design
# 基于 Claude Code Hook 的消息推送机制设计方案

> 作者：技术方案研究  
> 日期：2026-04-21  
> 目标：替代 `AgentLoop` 中的 timer 轮询，改为事件驱动推送

---

## 1. 背景与问题

### 现有轮询机制

`src/runtime/agent_loop.rs` 中的 `AgentLoop::run()` 采用"轮询 + sleep"模式：

```
loop {
    let unread = inbox_service.peek(...);   // 1. 每轮检查 inbox
    for item in unread {
        orchestrator.send_input(...)        // 2. 有消息就喂给 Claude
        collect output until TurnComplete   // 3. 等待回复
        message_service.send(reply)         // 4. 发回 room
    }
    sleep(poll_interval)                    // 5. 等待 N 秒再轮询（默认 5s）
}
```

**痛点**：
- 最多延迟 `poll_interval`（5 秒）才响应新消息
- 后台线程持续占用，即使没有任何消息
- 不优雅：事件本质上是即时的，但机制是轮询

### 架构关键点

在理解设计方案前，需理解项目的 Claude Code 调用方式：

`src/backend/claude_code.rs` 采用的是 **Plan B：`--resume <session-id>` 模式**。

每次 `send_input()` 调用会：
1. 启动一个新的 `claude --print -p <input> --resume <session-id> --output-format json` 子进程
2. 子进程运行完 → 退出 → 返回 JSON 结果
3. 下次再启动新进程，通过 `--resume` 恢复对话历史

这意味着：**Claude Code session 本身不是一个长驻进程**，而是每轮一个新进程。

---

## 2. Claude Code Hooks 完整说明

### 2.1 配置位置

| 文件 | 作用范围 |
|------|----------|
| `~/.claude/settings.json` | 全局（所有项目） |
| `.claude/settings.json` | 项目级（提交到 git） |
| `.claude/settings.local.json` | 项目本地（不提交） |

### 2.2 Hook 配置格式

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/path/to/script.sh",
            "timeout": 30
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "/path/to/pre-bash.sh"
          }
        ]
      }
    ]
  }
}
```

### 2.3 Handler 类型

| 类型 | 说明 |
|------|------|
| `command` | 执行 shell 命令，stdin 收 JSON，stdout 返回控制信息 |
| `http` | POST JSON 到 HTTP 端点 |
| `prompt` | 让 Claude 模型做 yes/no 判断 |
| `agent` | 启动子 agent（实验性） |

### 2.4 完整事件类型（截至 2026 年）

**会话级别：**
- `SessionStart` - 会话开始/恢复时
- `SessionEnd` - 会话终止时
- `InstructionsLoaded` - CLAUDE.md 加载时

**每轮级别：**
- `UserPromptSubmit` - Claude 处理用户提示前
- `Stop` - Claude 完成当前回复时（**关键事件**）
- `StopFailure` - 因 API 错误结束时

**工具执行级别：**
- `PreToolUse` - 工具执行前（可阻断）
- `PostToolUse` - 工具执行成功后
- `PostToolUseFailure` - 工具执行失败后
- `PermissionRequest` - 权限对话框出现时
- `PermissionDenied` - 自动模式拒绝时

**Agent/Task 级别：**
- `SubagentStart` / `SubagentStop`
- `TaskCreated` / `TaskCompleted`
- `TeammateIdle` - 团队 agent 即将 idle 时

**反应式事件：**
- `Notification` - Claude Code 发送通知时（fire-and-forget）
- `ConfigChange` - 配置文件变化时
- `FileChanged` - 被监视的文件发生变化时
- `PreCompact` / `PostCompact` - 上下文压缩前后

### 2.5 stdin JSON 数据格式（通用字段）

所有 hook 脚本通过 stdin 收到：

```json
{
  "session_id": "abc123",
  "transcript_path": "/Users/user/.claude/projects/.../transcript.jsonl",
  "cwd": "/current/working/dir",
  "permission_mode": "default",
  "hook_event_name": "Stop"
}
```

### 2.6 Stop Hook 详细说明

**触发时机**：Claude 完成一轮回复即将停止时（`stop_hook_active` 字段标识是否已被前一个 Stop hook 激活过）。

**输出控制方式**：

| 输出方式 | 效果 |
|----------|------|
| Exit code 0 + JSON `{"decision":"block","reason":"..."}` | 阻断停止，把 reason 作为下一条用户消息注入 |
| Exit code 2 | 阻断停止，stderr 内容作为错误消息 |
| Exit code 0 + JSON `additionalContext` | 添加上下文后正常停止 |
| Exit code 0（无输出） | 正常停止 |

**防无限循环**：检查 `stop_hook_active` 字段，若为 `true` 则不再阻断，避免 Stop hook 无限触发自己。

**关键示例**：
```bash
#!/bin/bash
INPUT=$(cat)
STOP_HOOK_ACTIVE=$(echo "$INPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('stop_hook_active', False))")

if [ "$STOP_HOOK_ACTIVE" = "True" ]; then
  exit 0  # 已经被 hook 触发过，放行
fi

# 检查 inbox...
COUNT=$(check_inbox_count)
if [ "$COUNT" -gt 0 ]; then
  MESSAGE=$(fetch_next_message)
  echo "{\"decision\":\"block\",\"reason\":\"$MESSAGE\"}"
  exit 0
fi

exit 0  # 没有新消息，正常停止
```

### 2.7 Notification Hook 详细说明

**触发时机**（notification_type 值）：
- `permission_prompt` - 权限请求对话框
- `idle_prompt` - Claude 进入 idle 状态
- `auth_success` - 认证成功
- `elicitation_dialog` - MCP server 请求用户输入

**重要限制**：
- **Fire-and-forget**，不阻断执行
- **不能**向 Claude 注入消息或修改通知内容
- 只能触发副作用（告警、日志等）

**结论**：Notification hook 不适合用于消息推送注入。

### 2.8 --print / -p 模式下 Hook 的行为

根据官方文档确认：
- `claude -p`（非交互模式）**默认触发 hooks**，行为与交互模式相同
- `claude --bare -p` 则跳过所有 hooks
- 本项目的 `ClaudeCodeBackend` 使用 `-p` 模式但不加 `--bare`，因此 **hooks 会触发**

---

## 3. 四种方案分析

### 方案 A：Stop Hook + 检查 inbox + block 阻断

**原理**：
1. 每次 Claude 完成一轮（`run_turn()` 返回），Stop hook 触发
2. Hook 脚本调用 MCP 工具或直接读文件检查 inbox
3. 如果有未读消息，返回 `{"decision":"block","reason":"[Message from xxx]: ..."}` 阻断停止
4. Claude Code 将 reason 作为下一条用户输入继续处理

**流程图**：
```
Claude 完成回复
    |
Stop Hook 触发
    |
检查 inbox (HTTP/shell 调用 MCP)
    |-- 有新消息 --> 返回 block + 消息内容
    |                    |
    |             Claude 处理消息
    |             Claude 完成 --> 再次触发 Stop Hook (循环)
    |
    +-- 无新消息 --> exit 0 (正常停止)
```

**与现有架构的契合度分析**：

本项目使用 `--print -p` 模式，每轮 Claude 调用是一个独立的短暂子进程。**Stop hook 的触发发生在 claude 子进程内部，而不是在 Rust 进程中**。

这意味着：
- Stop hook 脚本需要独立地能访问 inbox（不能通过 Rust 进程内的 `InboxService`）
- 需要通过 HTTP 调用 MCP server 的 `inbox_count` 或 `inbox_peek` 工具
- 或者直接读写共享文件（team-mode-data 目录）

**致命问题**：

在 `--print/-p` 模式下（非交互式），Stop hook 是否真的会在子进程退出前触发？根据 Claude Code 文档，hooks 在 `-p` 模式下触发，但这指的是 **交互会话中运行 `/command`** 还是 **命令行调用 `claude -p`** 还不完全明确。

如果 Stop hook 在 `-p` 短暂子进程模式下**不触发**，该方案失效。

**另一个问题**：

即使 Stop hook 触发并 block，block 的效果是向 Claude 发送下一条消息，但这发生在 **claude 进程内部**，Rust 端的 `run_turn()` 已经在等待该进程退出。Block 会让该进程继续跑第二轮，这与 Plan B 的"一次 -p 一轮"模型**不兼容**。

**结论**：方案 A 与现有架构存在根本性冲突，不推荐。

---

### 方案 B：外部事件 → Notification Hook

**原理**：当新消息进入 inbox 时，MCP server 触发 Notification → Hook 脚本注入消息。

**致命问题**：
- Notification hook 是 fire-and-forget，不能注入消息给 Claude
- MCP server 无法主动触发 Claude Code 的 Notification 事件（Notification 是 Claude Code 内部事件，外部无法推送）

**结论**：Notification hook 无法实现消息注入，方案 B 不可行。

---

### 方案 C：Stop Hook + 持久化轮询替代（最接近可行）

**原理**：如果 Stop hook 在 `-p` 模式下触发，则可以：
1. Claude 每轮结束（`run_turn()` 退出）后，Rust 端立即检查 inbox
2. 如有新消息，立即调用 `send_input()` 开启下一轮（无 sleep）
3. 如无新消息，进入短暂 sleep 或条件等待

这本质上是**在 Rust 层把 `sleep(poll_interval)` 改为条件等待**，而非真正利用 Hook 机制。

**方案 C 的 Rust 实现**：

在 `AgentLoop::run()` 中，将：
```rust
tokio::select! {
    _ = tokio::time::sleep(self.poll_interval) => {}
    _ = &mut shutdown_rx => { return; }
}
```

改为：
```rust
// 立即检查 inbox（已在循环开头），无需等待
// 如果没有新消息，进入短暂等待（100ms 左右）后再检查
tokio::select! {
    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
    _ = &mut shutdown_rx => { return; }
}
```

但这只是降低轮询间隔，不是事件驱动。

**结论**：方案 C 是对现有 timer 的改良，不是真正的 Hook 推送。

---

### 方案 D：完全不用 Hook，改用内部通知机制（最推荐）

**原理**：在 MCP server 收到 `room_post_message` 工具调用（新消息入库）时，通过内部 channel 或条件变量通知 `AgentLoop` 有新消息。

**这是真正的事件驱动**，不需要任何 Claude Code Hook。

**核心机制**：

```
MCP 工具 room_post_message
    |
message_service.send() 成功
    |
触发内部通知 (tokio::Notify 或 broadcast channel)
    |
AgentLoop 正在等待通知
    |
立即唤醒，检查 inbox
    |
调用 orchestrator.send_input()
```

**实现步骤**：

**Step 1**：新增 `InboxNotifier`（共享的通知句柄）

在 `src/team_mode/service/` 或 `src/runtime/` 新增：

```rust
// src/team_mode/service/inbox_notifier.rs
use std::sync::Arc;
use tokio::sync::Notify;

/// 共享的 inbox 到达通知器。
/// 当新消息被投递到 inbox 时，调用 notify_waiters() 唤醒等待中的 AgentLoop。
#[derive(Clone, Debug)]
pub struct InboxNotifier {
    inner: Arc<Notify>,
}

impl InboxNotifier {
    pub fn new() -> Self {
        Self { inner: Arc::new(Notify::new()) }
    }

    /// 通知所有等待者（有新消息到达）
    pub fn notify(&self) {
        self.inner.notify_waiters();
    }

    /// 等待通知（AgentLoop 调用此方法挂起等待）
    pub async fn notified(&self) {
        self.inner.notified().await;
    }
}
```

**Step 2**：`MessageService` 持有 `InboxNotifier`，在 `send()` 成功后调用 `notify()`

修改 `src/team_mode/service/message_service.rs`：

```rust
pub struct MessageService {
    message_store: MessageStore,
    member_store: MemberStore,
    room_store: RoomStore,
    team_store: TeamStore,
    inbox_notifier: Option<InboxNotifier>,  // 新增
}

impl MessageService {
    pub fn with_notifier(/* 原参数... */, notifier: InboxNotifier) -> Self {
        // ...
        Self { /* ... */, inbox_notifier: Some(notifier) }
    }

    pub fn send(&self, request: SendMessageRequest) -> Result<Message> {
        // ...（原有逻辑不变）...
        self.message_store.save(&message)?;

        // 新增：如果有收件人，触发 inbox 通知
        if !message.effective_recipients.is_empty() {
            if let Some(ref notifier) = self.inbox_notifier {
                notifier.notify();
            }
        }

        Ok(message)
    }
}
```

**Step 3**：`AgentLoop` 持有 `InboxNotifier`，将 sleep 改为等待通知

修改 `src/runtime/agent_loop.rs`：

```rust
pub struct AgentLoop {
    pub member_id: String,
    pub team_id: String,
    pub room_id: String,
    pub orchestrator: Arc<Mutex<RuntimeOrchestrator>>,
    pub inbox_service: InboxService,
    pub message_store: MessageStore,
    pub message_service: MessageService,
    pub poll_interval: Duration,         // 保留作为最大等待时间（超时兜底）
    pub inbox_notifier: InboxNotifier,   // 新增
}

// 在 run() 的末尾，将 sleep 改为：
tokio::select! {
    // 等待 inbox 通知（新消息到达立即唤醒）
    _ = self.inbox_notifier.notified() => {}
    // 超时兜底（防止 notifier 丢失通知的极端情况）
    _ = tokio::time::sleep(self.poll_interval) => {}
    // 优雅停机
    _ = &mut shutdown_rx => {
        tracing::info!(member_id = %self.member_id, "agent loop shutting down");
        return;
    }
}
```

**Step 4**：`TeamModeToolset` 创建共享 `InboxNotifier`，注入到 `MessageService` 和 `AgentLoop`

修改 `src/team_mode/mcp/tools.rs`：

```rust
pub struct TeamModeToolset {
    // ...（现有字段不变）...
    inbox_notifier: InboxNotifier,   // 新增
}

impl TeamModeToolset {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let inbox_notifier = InboxNotifier::new();
        // ...
        let message_service = MessageService::with_notifier(
            message_store.clone(),
            member_store.clone(),
            room_store.clone(),
            team_store.clone(),
            inbox_notifier.clone(),  // 注入
        );
        // ...
        Self {
            // ...
            inbox_notifier,
        }
    }

    fn member_spawn_managed(&self, args: ...) -> Result<ToolExecution> {
        // ...（现有逻辑）...
        if let Some(rx) = output_rx {
            let agent_loop = AgentLoop {
                // ...（现有字段）...
                poll_interval: Duration::from_secs(30),   // 改为 30s 超时兜底
                inbox_notifier: self.inbox_notifier.clone(),  // 注入
            };
            // ...
        }
    }
}
```

---

## 4. 推荐方案：方案 D

### 推荐理由

| 维度 | 方案 A (Stop Hook) | 方案 B (Notification) | 方案 C (降轮询) | 方案 D (内部 Notify) |
|------|-------------------|----------------------|-----------------|---------------------|
| 可行性 | 存疑（-p 模式兼容性） | 不可行 | 可行但无提升 | **完全可行** |
| 延迟 | 未知 | N/A | 100ms+ | **<1ms** |
| 依赖外部 | 需要 Hook 脚本 | 需要 Hook 脚本 | 无 | **无** |
| 架构侵入 | 高（需改 claude 调用方式） | 高 | 低 | **低** |
| 可维护性 | 差（外部脚本）| 差 | 好 | **好** |
| 兜底机制 | 无 | 无 | 本身是轮询 | **有（timeout 兜底）** |

### 方案 D 的优势

1. **零延迟**：`room_post_message` 入库后立即 `notify_waiters()`，`AgentLoop` 在同一毫秒内被唤醒
2. **纯 Rust 实现**：无需外部脚本、无需 Hook 配置、无跨进程通信
3. **向后兼容**：保留 `poll_interval` 作为超时兜底，极端情况（通知丢失）下仍能正常工作
4. **精准唤醒**：只在真正有新消息时唤醒，空闲时完全挂起
5. **最小改动**：涉及文件少，不改变任何外部接口

### 方案 D 的局限

- 同进程内有效：如果 MCP server 和 AgentLoop 在不同进程，需改为跨进程通知（如文件、Unix socket）
- 但本项目 `TeamModeToolset` 和 `AgentLoop` 在同一进程，此限制不影响

---

## 5. 为什么 Hook 方案难以实现

根据对项目架构的分析，Claude Code Hook 方案面临一个根本性障碍：

**本项目使用 `--print/-p` 无状态模式，每轮是一个短暂的子进程**。

Claude Code 的 Hook 系统设计是针对**交互式会话**的（用户在 REPL 中工作，Claude 在后台持续运行）。在这种模式下：
- Stop hook 触发后可以 block → Claude 处理新输入 → 再次触发 Stop hook → ...形成循环

但本项目的模式是：
- Rust 启动 `claude -p input --resume session-id` 子进程
- 子进程运行、返回结果、退出
- Rust 读取结果，决定是否发起下一轮

Hook 的 block 机制会让子进程继续运行（变成多轮），但这违反了 Plan B 的设计假设（一个 `-p` 调用 = 一轮）。Stop hook block 产生的额外轮次不会反映在 `run_turn()` 的返回值中，导致 Rust 层的 `AgentLoop` 无法感知。

此外，Hook 脚本需要独立于 Rust 进程访问 inbox，意味着要么通过 HTTP 调用 MCP（增加复杂度）、要么直接读写磁盘（绕过 `InboxService` 的逻辑）。

---

## 6. 实现清单

方案 D 需要修改以下文件（均在 `src/` 下）：

| 文件 | 改动类型 | 说明 |
|------|----------|------|
| `team_mode/service/inbox_notifier.rs` | **新建** | `InboxNotifier` 结构体 |
| `team_mode/service/mod.rs` | 修改 | pub use inbox_notifier::* |
| `team_mode/service/message_service.rs` | 修改 | `MessageService` 添加 `inbox_notifier` 字段，`send()` 末尾调用 `notify()` |
| `runtime/agent_loop.rs` | 修改 | `AgentLoop` 添加 `inbox_notifier` 字段，sleep 改为 `notified().await + timeout` |
| `team_mode/mcp/tools.rs` | 修改 | `TeamModeToolset` 持有 `InboxNotifier`，在构造和 spawn 时注入 |

代码改动量：约 80-120 行（新增），约 30-40 行（修改），不涉及任何接口破坏性变更。

---

## 7. 潜在问题和注意事项

### 7.1 通知丢失问题

`tokio::Notify` 是边缘触发（edge-triggered）：如果 `AgentLoop` 还没调用 `notified()` 就触发了两次 `notify_waiters()`，第二次通知会被合并。

**解决方案**：保留 `poll_interval`（改为 30s）作为兜底超时，即使通知丢失，最多 30s 后也会检查一次。

### 7.2 多成员场景

如果有多个 `AgentLoop`（多个被管理成员），它们共用同一个 `InboxNotifier`。一条消息到达时，**所有** AgentLoop 都会被唤醒，然后各自检查自己的 inbox（自然过滤）。

这是可以接受的：被唤醒但没有新消息的 loop 会立即重新等待，开销极低。

如果需要精准唤醒（只唤醒被 mention 的成员），可以改为 `HashMap<String, Arc<Notify>>` 按 `member_id` 存储，`message_service.send()` 时按 `effective_recipients` 精准唤醒。

### 7.3 与 AgentLoop 处理中的竞态

`AgentLoop` 在处理一条消息（等待 `TurnComplete`）时，新消息可能到达并触发通知。但此时 `notified()` 还没有被 select! 监听（AgentLoop 正在 `output_rx.recv()` 中）。

**解决方案**：通知会被暂存在 `Notify` 内部，当 AgentLoop 完成当前消息处理、进入下一次循环检查 inbox 时，新消息会被立即发现（不需要等待通知，因为 inbox 已有未读）。

即：在循环开头的 `inbox_service.peek()` 已经是同步检查，通知机制只是用来替代 sleep，不影响功能正确性。

### 7.4 跨进程场景（未来扩展）

如果未来 MCP server 和 AgentLoop 运行在不同进程，可以用以下方式替代 `tokio::Notify`：
- **文件锁 + inotify**：写入一个 sentinel 文件，AgentLoop 监听该文件变化
- **Unix domain socket**：MCP server 向 socket 发送通知，AgentLoop 监听
- **Redis pub/sub**：适合分布式部署

---

## 8. 参考资料

- [Claude Code Hooks Reference](https://code.claude.com/docs/en/hooks)
- [Run Claude Code programmatically (headless)](https://code.claude.com/docs/en/headless)
- [Claude Code Hooks Complete Guide - SmartScope](https://smartscope.blog/en/generative-ai/claude/claude-code-hooks-guide/)
- [Claude Code Hooks: All 12 Events - claudefa.st](https://claudefa.st/blog/tools/hooks/hooks-guide)
- [Claude Code Hooks: 12 Lifecycle Events - Pixelmojo](https://www.pixelmojo.io/blogs/claude-code-hooks-production-quality-ci-cd-patterns)
