# agent-teams-rs 架构背景文档

> ⚠️ **历史文档**。本文件描述的是旧版本（7 工具、分离的 members/ 和 member_execution/ 目录、member_id 复合键）。
>
> **最新架构请看**：
> - [`../README.md`](../README.md) — 项目总览（8 工具、push 架构）
> - [`./mcp-tools-reference.md`](./mcp-tools-reference.md) — 完整工具 schema
> - [`./push-notifications.md`](./push-notifications.md) — FileChanged + asyncRewake 配置
> - [`../.plans/refactor-data-layout/spec.md`](../.plans/refactor-data-layout/spec.md) — 当前数据布局的权威方案
>
> 当前状态（2026-04-22）：`cargo test --lib` **282** 个测试全绿；MCP 工具面从 7 升为 8（新增 `inbox_read`）；Lead 的收件箱有原生 push 机制。
>
> 本文件保留作为演化脉络参考，内容从"状态"之后起仍有价值，但细节已落后。

---

## 历史状态（v0.1.0 以前）

面向后续会话的技术背景说明（2026 年 4 月中旬之前）。
当前状态：`cargo test --lib` 264 个测试全绿；`cargo check --tests` 集成测试编译通过；真机端到端（Claude Code 客户端 → MCP → spawn worker → send_message → worker 回复）已验证通过。

---

## 1. 项目定位

Rust 实现的 **AI 团队协作系统**：
- 将多个 AI agent（Claude Code / Codex / Gemini）组织为一个"团队"
- 成员之间通过"群聊 + @提及"机制互相发消息、分配任务
- 外层通过 **MCP（Model Context Protocol）** 暴露 **7 个极简工具** 给调用方（Lead Agent，也就是人类对应的 Claude Code 实例）

项目路径：`E:\aigc内容整理\agent-teams-rs-team-mode`（Windows 10）

---

## 2. 核心心智模型

- **team** = 组织边界 + 默认工作目录 + 1 个隐式 lead + 0..N 个 workers
- **lead** = 调用 MCP 的"人类+客户端"；不是被 spawn 的进程；作为 team 的一个虚拟成员存在以支持 @mention/消息路由
- **worker** = 真正被 Rust 管理的 managed agent 进程；有进程才算"在团队里"
- **配置文件（execution profile）** = 位于磁盘，和 worker 身份记录分离；worker remove 后 profile 保留，便于 fast-resume

---

## 3. 整体架构分层

```
┌─────────────────────────────────────────────────────────────┐
│  Lead Agent（人类的 Claude Code 客户端）                      │
│  .mcp.json 挂载 team-mode MCP server                         │
│  调用 7 个 MCP 工具：team_create / worker_add / send_message …│
└───────────────────────┬─────────────────────────────────────┘
                        │ stdio（JSON-RPC）
                        ▼
┌─────────────────────────────────────────────────────────────┐
│  team_mode_mcp（MCP Server 进程，Rust）                       │
│  target/debug/team_mode_mcp.exe                              │
│  - 暴露 7 个 MCP 工具 + 5 类 Resource URI                      │
│  - 启动时自动探测 CLAUDE_CODE_GIT_BASH_PATH                   │
│  - 持有 RuntimeOrchestrator（spawn/管理 managed member 会话）  │
│  - 为每个 managed worker 启动一个 AgentLoop 消息驱动循环       │
└───────────────────────┬─────────────────────────────────────┘
                        │ spawn 子进程（stdin/stdout）
                        ▼
┌─────────────────────────────────────────────────────────────┐
│  Managed Worker（Claude Code / Codex 进程）                  │
│  由 Rust 层直接控制，不是通过 MCP 工具                        │
│  - stdin 接收 NDJSON user message                             │
│  - stdout 流出 stream-json 事件                               │
└─────────────────────────────────────────────────────────────┘
```

---

## 4. MCP 工具面（7 个，详细参数见 [`mcp-tools-reference.md`](./mcp-tools-reference.md)）

```
team_create(name, cwd?)                                   ← 自动创建 lead
team_list()
team_delete(name)                                         ← 级联停所有 worker + 删记录

worker_add(team, name, adapter?, model?, cwd?, system_prompt?, env?)
                                                          ← 合并 add + spawn + resume + 改配置
worker_list(team)                                         ← 精简字段：name/adapter/sessionState
worker_remove(team, name)                                 ← 停进程 + 删身份；保留 execution profile

send_message(team, text)                                  ← sender 硬编码为 lead；必须含 @handle
```

---

## 5. 消息路由

### 5.1 MCP 层发消息（`send_message` 工具）
- 调用方（Lead Agent）固定为 sender = `<team>-lead`
- `text` 必须含至少一个 `@handle` 匹配当前 team 内的某个活跃 worker，否则报错
- MessageService 解析 body 里的 `@handle` → 找到 member → 写入 `effective_recipients` → inbox 投影更新

### 5.2 Worker 回复（AgentLoop 内部路径，不走 MCP 工具）
- AgentLoop 检测到 inbox 有新消息 → 注入 stdin → 读 stdout 回复文本
- 调用 `MessageService::send(MessageKind::Reply, reply_to=<msg_id>, ...)`
- MessageService 见 Reply + reply_to → 自动把原 sender 作为 recipient（fallback 机制，不依赖 @mention）

### 5.3 存储布局

```
~/.claude/teams/<team-name>/  ← team_mode_mcp 默认数据目录（或项目 .team-mode-data/）
  config.json                 # 团队配置（id=name、cwd、leadMemberId）
  members/<member_id>.json    # 成员身份（member_id = "{team}-{name}"，lead=id "{team}-lead"）
  member_execution/<member_id>.json  # 执行配置：adapter/model/cwd/env/system_prompt/sessionState
  messages/*.jsonl            # 消息 append-only 流
  projections/                # inbox / thread 投影
  rooms/                      # 房间（当前只有 "main"）
```

---

## 6. Agent 会话管理

### 6.1 后端类型

| 后端 | 文件 | 模式 | 状态 |
|---|---|---|---|
| Claude Code | `src/backend/claude_code.rs` | 持久进程，stream-json NDJSON | ✅ 真机验证通过 |
| Codex | `src/backend/codex.rs` | 持久进程，JSON-RPC | ✅ 已验证 |
| Gemini | `src/backend/gemini.rs` | 每轮 spawn | 未重新验证 |

### 6.2 Claude Code 后端（关键实现）

**进程启动**：
```
claude -p "" --input-format stream-json --output-format stream-json --verbose \
       [--system-prompt <sp>] [--model <m>] [--permission-mode <pm>] [--allowedTools <t>]
```
进程常驻，等待 stdin NDJSON 输入。

**stdin 协议（NDJSON，每行一条）**：
```json
{"type":"user","message":{"role":"user","content":"消息文本"}}
```

**stdout 事件处理**：
| 事件 | 行为 |
|---|---|
| `{"type":"assistant","message":{"content":[{"type":"text","text":"..."}]}}` | 发 `AgentOutput::Delta(text)` |
| `{"type":"result","subtype":"success","is_error":false}` | 发 `AgentOutput::TurnComplete` + **idle 置 true** |
| `{"type":"result","is_error":true}` | 发 `AgentOutput::Error(msg)` |
| EOF | 发 `AgentOutput::Error("process exited")` |

**idle 闸门（关键正确性点）**：
- 维护 `idle: Arc<AtomicBool> + Notify`
- `send_input` 前必须等 `idle==true`（上一轮 `result` 已到）
- 写 NDJSON 前置 idle=false；reader 见 `result` 后置 idle=true + notify_waiters
- 双重检查 + `Notify::notified()` 先挂 permit 防止 lost-wakeup
- 30s 超时兜底防止死锁

**Windows 环境要求**：
- 子进程需要 `CLAUDE_CODE_GIT_BASH_PATH` 指向 `bash.exe`
- **team_mode_mcp 启动时自动探测**（`src/bin/team_mode_mcp.rs::ensure_git_bash_path`）：
  1. 父进程 env（已有就用）
  2. `C:\Program Files\Git\bin\bash.exe`
  3. `C:\Program Files (x86)\Git\bin\bash.exe`
  4. `D:\Git\bin\bash.exe`
  5. `which::which("bash.exe" | "bash")`
- 全找不到发 `tracing::warn!`，不阻塞启动
- 本机实际路径：`D:\Git\bin\bash.exe`

### 6.3 Codex 后端
- `codex app-server` stdio JSON-RPC
- 握手：initialize → initialized → thread/start → turn/start
- id 配对自带幂等

---

## 7. AgentLoop（消息驱动循环）

**文件**：`src/runtime/agent_loop.rs`

```rust
loop {
    wait: tokio::Notify（<1ms 唤醒） OR 30s 兜底 OR shutdown
    1. inbox_service.peek(member_id) → 过滤 Unread
    2. 跳过 sender == self 的消息（防 echo）
    3. orchestrator.send_input(member_id, "[Message from X]: ...") → stdin NDJSON
    4. 从 output_rx 收集 Delta / TurnComplete → 拼接回复文本
    5. message_service.send(Reply, reply_to=<msg_id>) → 回 room
    6. inbox_service.ack(message_id)
}
```

---

## 8. MCP Resources（读取接口）

声明了 `resources.subscribe: true` 能力。

| URI | 返回 |
|---|---|
| `team://<team>` | Team 元信息（name/cwd/leadMemberId/...） |
| `team://<team>/rooms/main` | `{room, messages[]}` |
| `team://<team>/threads/<id>` | `{thread, messages[]}` |
| `team://<team>/members/<id>/inbox` | `{inbox, counts}` — 注意 `<id>` 是内部 member_id 即 `<team>-<name>` |
| `team://<team>/members/<id>/session` | `{memberId, name, handle, sessionState, execution}` — 查 worker 状态 |

Lead 的 inbox 就是 `team://<team>/members/<team>-lead/inbox`。

**通知机制**：tool call 触发某些资源变更时，MCP server 会发 `notifications/resources/updated {uri}` 给订阅该 URI 的同一客户端。**跨进程不会广播**（stdio 传输的固有限制）。

---

## 9. 改造历程要点（给未来会话的索引）

### 已完成的重大变化
1. **Claude Code stream-json NDJSON 注入**（`claude_code.rs`）
   - 从纯文本 stdin → NDJSON `{"type":"user",...}`
   - 增加 idle 闸门避免消息排队
   - 初始 emit 一次 `TurnComplete` 让 AgentLoop drain
2. **MCP 工具面精简**：23 → 17 → 15 → 9 → **7**
   - 删除：team_get/member_get/member_update/inbox_*/thread_*/execution_profile_set/spawn_member/shutdown_member 等
   - 合并：member_add + spawn_member + resume + 改配置 → `worker_add` 单工具四模式
   - 合并：member_remove + shutdown_member → `worker_remove`
3. **Lead 设计**：lead 从"显式 member"变成"team 虚拟属性"，由 `team_create` 自动创建，`worker_list` 不显示
4. **send_message 极简**：3 参数 → 2 参数（team + text），sender 硬编码 lead，必须含 @
5. **CLAUDE_CODE_GIT_BASH_PATH 启动时自动探测**，不用每次 spawn 传
6. **member 内部 id 规则**：`{team_name}-{worker_name}`，全局唯一；MCP 面全用 name

### 已回答的关键设计问题
- **MCP 是双向的吗？** 是半双向——Server→Client 有 notifications/resources/updated 等，但 Claude Code **不会把 MCP 推送转为新用户回合**；空闲会话无法被推送唤醒。
- **Worker 怎么收消息？** 通过 Rust 层 AgentLoop 往 stdin 注入，不走 MCP。
- **Worker 怎么发消息？** AgentLoop 读 stdout 回复 → MessageService::send(Reply, reply_to=...) 自动回帖。
- **配置文件的意义？** 持久化 "worker 身份 + 执行配置 + session_state"；remove 后 profile 保留用于 fast-resume；彻底清理由 Lead 手动 `rm` 实现，MCP 不管。
- **为什么 `team_create` 自动创建 lead？** 调用 MCP 的人就是 lead；避免"先有鸡还是先有蛋"的 lead_member_id 参数。

### 尚未完成的小项
- 集成测试 `tests/team_mode_mcp.rs` 编译通过但真实跑需要 rebuild exe（exe 被 MCP 客户端占用时阻塞）
- Gemini 后端未在新工具面下重测

---

## 10. 关键文件索引

| 文件 | 职责 |
|------|------|
| `src/bin/team_mode_mcp.rs` | MCP server 入口；启动时探测 Git Bash |
| `src/team_mode/mcp/tools.rs` | 所有 7 个 MCP 工具 handler |
| `src/team_mode/mcp/runtime.rs` | MCP JSON-RPC 请求处理、通知生成、订阅管理 |
| `src/team_mode/mcp/resources.rs` | Resource URI 解析 + list_resources + read_resource |
| `src/runtime/agent_loop.rs` | Managed worker 消息驱动循环 |
| `src/runtime/orchestrator.rs` | 会话注册、send_input 路由、shutdown |
| `src/backend/claude_code.rs` | Claude Code stream-json NDJSON 持久进程后端 |
| `src/backend/codex.rs` | Codex 后端 |
| `src/team_mode/service/message_service.rs` | 消息发送、@mention 解析、Reply 回退 |
| `src/team_mode/service/inbox_service.rs` | 收件箱读取/ack |
| `src/team_mode/service/inbox_notifier.rs` | tokio::Notify 事件推送 |
| `src/team_mode/service/team_service.rs` | Team CRUD + `set_lead_if_absent` |
| `src/team_mode/storage/member_store.rs` | 成员存储；`delete_profile_only` 只删身份 |
| `src/team_mode/domain/team.rs` | Team 结构（含 `cwd` 字段） |

---

## 11. 构建与测试

```bash
cd "E:/aigc内容整理/agent-teams-rs-team-mode"

# 快速编译检查（不链接 exe，不受 MCP 客户端占用影响）
cargo check --lib
cargo check --tests

# 单元测试（264 个通过，不需要 exe）
cargo test --lib

# 构建 MCP server 二进制（若 exe 被 MCP 客户端锁定会失败，需先断开 /mcp）
cargo build --bin team_mode_mcp

# 集成测试（需要 exe，会重 link）
cargo test --test team_mode_mcp

# 启动 MCP server（手动，通常由 Claude Code 客户端自动启动）
cargo run --bin team_mode_mcp
```

### `.mcp.json` 配置（本机）

```json
{
  "mcpServers": {
    "team-mode": {
      "command": "E:\\aigc内容整理\\agent-teams-rs-team-mode\\target\\debug\\team_mode_mcp.exe",
      "args": [],
      "env": {
        "CLAUDE_CODE_GIT_BASH_PATH": "D:\\Git\\bin\\bash.exe",
        "RUST_LOG": "info"
      }
    }
  }
}
```

`CLAUDE_CODE_GIT_BASH_PATH` 可以省略，会自动探测。
