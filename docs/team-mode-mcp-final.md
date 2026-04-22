> **[HISTORICAL — 2026-04]** 本文档描述的是重写完成后的 v1 交付状态（22 个 MCP 工具，Content-Length 传输），与当前实现（7 个工具，NDJSON 传输，Lead 为虚拟属性）有重大出入。仅作为项目演化记录保留，当前权威参见 docs/architecture-background.md 与 docs/mcp-tools-reference.md。

# Team Mode MCP 最终完成文档

> 状态：Implemented v1
> 仓库：`agent-teams-rs-team-mode`
> 更新时间：2026-04-21

---

## 1. 文档目的

本文不是方案文档，而是**当前代码实际完成状态**的交付文档。

它回答 5 个问题：

1. 最终做成了什么
2. 代码落在哪些模块
3. MCP 接口现在长什么样
4. 怎么启动和使用
5. 还剩哪些已知限制

---

## 2. 最终结论

本仓库已经从原来的 `task + inbox mailbox` 协作模型，重构为：

**一个以 `team/member/room/message/inbox/thread` 为核心、通过 MCP stdio 暴露能力的 Team Mode runtime。**

这次交付已经完成以下核心目标：

1. 建立 transcript-first 的消息模型
2. 完成 Team Mode 领域模型
3. 完成服务层业务语义
4. 完成 MCP runtime、tools、resources、subscribe/updated 通知
5. 接通 managed member 的基础生命周期工具

当前更准确的状态是：

- **代码实现已完成**
- **静态检查与代码审查已完成**
- **真实 `cargo check / cargo test` 受当前环境网络依赖限制，未能完整跑通**

---

## 3. 已完成范围

### 3.1 领域模型

已完成目录：

```text
src/team_mode/domain/
  team.rs
  member.rs
  room.rs
  message.rs
  inbox.rs
  thread.rs
```

核心模型已经固定为：

1. `Team`
2. `MemberProfile`
3. `ExecutionProfile`
4. `Room`
5. `Message`
6. `Inbox`
7. `Thread`

关键原则已经落地：

1. `Message` 是主真相源
2. `Inbox` 和 `Thread` 都是投影
3. `@handle` 在发送时解析并固化
4. 收件人最终持久化为稳定 `member_id`

### 3.2 存储层

已完成目录：

```text
src/team_mode/storage/
  team_store.rs
  member_store.rs
  room_store.rs
  message_store.rs
  projection_store.rs
```

已经实现：

1. `TeamStore`
2. `MemberStore`
3. `RoomStore`
4. `MessageStore`
5. `ProjectionStore`

当前存储特征：

1. `messages/transcript.jsonl` 是消息总账
2. `messages/{message_id}.json` 保存最新快照
3. `members/` 与 `member_execution/` 分离
4. room 物理存储按 `team_id + room_id` 隔离
5. inbox/thread 通过 transcript 重建

### 3.3 服务层

已完成目录：

```text
src/team_mode/service/
  team_service.rs
  member_service.rs
  room_service.rs
  message_service.rs
  inbox_service.rs
  thread_service.rs
```

已经实现：

1. 建队、查队、删队
2. 成员增删改查
3. `main` room 幂等创建
4. `dispatch` 消息发送
5. `@handle` 解析
6. `effective_recipients` 计算
7. `inbox_peek`
8. `inbox_read`
9. `inbox_ack`
10. `inbox_count`
11. `thread_read`
12. `thread_read_messages`
13. `thread_reply`

已经修复的关键业务问题：

1. `sender` 使用稳定 `member_id`
2. `effective_recipients` 使用稳定 `member_id`
3. `dispatch` 必须至少命中一个有效成员
4. 非 `Active` 成员不会被当成有效派工目标
5. `reply_to` 必须属于同一 team、同一 room
6. thread 不能跨 room 复用
7. `inbox read/ack` 是幂等的，不会重复追加无意义 transcript 版本

### 3.4 MCP 层

已完成目录：

```text
src/team_mode/mcp/
  mod.rs
  schemas.rs
  resources.rs
  tools.rs
  runtime.rs
```

已完成内容：

1. JSON-RPC/MCP 基础 schema
2. `initialize`
3. `tools/list`
4. `tools/call`
5. `resources/list`
6. `resources/read`
7. `resources/subscribe`
8. `resources/unsubscribe`
9. `notifications/resources/updated`
10. stdio `Content-Length` 传输

并且已经补过两轮 review 后的关键修复：

1. 业务异常不再直接打崩 stdio server，而是返回 JSON-RPC 错误响应
2. `Content-Length` 解析支持非首行 header
3. header 提前 EOF 不会死循环

### 3.5 Managed Member

已经完成基础接入，但仍属于 v1 基础版：

1. `member_spawn_managed`
2. `member_shutdown_managed`
3. `member_resume_managed`
4. `member_session_status`

它们已接到：

1. `RuntimeOrchestrator`
2. `SessionRegistry`
3. 现有 `ClaudeCode / Codex / GeminiCli` backend 抽象

说明：

- 这里实现的是**生命周期接入**
- 不是完整的“群聊驱动 managed member 自动工作流”

---

## 4. 最终目录结构

本次重构的有效新核心主要是：

```text
src/
  runtime/
    orchestrator.rs
    managed_member.rs
    session_registry.rs
    session_state.rs
  team_mode/
    mod.rs
    domain/
    storage/
    service/
    mcp/
  bin/
    team_mode_mcp.rs
  lib.rs
```

其中：

1. `runtime/` 是执行层
2. `team_mode/domain` 是领域模型
3. `team_mode/storage` 是 transcript-first 持久化
4. `team_mode/service` 是业务层
5. `team_mode/mcp` 是对外 MCP 接入层
6. `bin/team_mode_mcp.rs` 是可启动入口

---

## 5. 最终 MCP 工具面

当前 `src/team_mode/mcp/tools.rs` 中实际提供的工具共 22 个：

### 5.1 Team

1. `team_create`
2. `team_get`
3. `team_list`
4. `team_delete`

### 5.2 Member

1. `member_add`
2. `member_remove`
3. `member_update`
4. `member_list`
5. `member_get`

### 5.3 Room

1. `room_post_message`
2. `room_read_messages`
3. `room_list`

### 5.4 Inbox

1. `inbox_peek`
2. `inbox_read`
3. `inbox_ack`
4. `inbox_count`

### 5.5 Thread

1. `thread_read`
2. `thread_reply`

### 5.6 Managed Member

1. `member_spawn_managed`
2. `member_shutdown_managed`
3. `member_resume_managed`
4. `member_session_status`

---

## 6. 最终 Resources 设计

当前资源 URI 已固定为：

1. `team://{team_id}`
2. `team://{team_id}/rooms/{room_id}`
3. `team://{team_id}/threads/{thread_id}`
4. `team://{team_id}/members/{member_id}/inbox`

对应实现位置：

- [resources.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/team_mode/mcp/resources.rs>)

当前行为：

1. `resources/list` 会列出 team、room、thread、inbox
2. `resources/read` 返回 `application/json` 文本
3. `subscribe/unsubscribe` 维护订阅集合
4. tool 写操作后会向已订阅 URI 发 `notifications/resources/updated`

---

## 7. 运行入口

当前可直接启动的 MCP server 入口是：

- [src/bin/team_mode_mcp.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/bin/team_mode_mcp.rs>)

启动方式：

```powershell
cargo run --bin team_mode_mcp -- --data-dir .team-mode-data
```

说明：

1. 默认数据目录是 `.team-mode-data`
2. server 走 stdio JSON-RPC
3. transport 使用 `Content-Length` framing

---

## 8. 关键代码映射

如果后面你要继续开发，最重要的文件是这些：

### 8.1 真相源与投影

- [src/team_mode/storage/message_store.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/team_mode/storage/message_store.rs>)
- [src/team_mode/storage/projection_store.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/team_mode/storage/projection_store.rs>)

### 8.2 派工与消息语义

- [src/team_mode/service/message_service.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/team_mode/service/message_service.rs>)

### 8.3 Inbox/Thread 行为

- [src/team_mode/service/inbox_service.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/team_mode/service/inbox_service.rs>)
- [src/team_mode/service/thread_service.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/team_mode/service/thread_service.rs>)

### 8.4 MCP 接入面

- [src/team_mode/mcp/runtime.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/team_mode/mcp/runtime.rs>)
- [src/team_mode/mcp/tools.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/team_mode/mcp/tools.rs>)
- [src/team_mode/mcp/resources.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/team_mode/mcp/resources.rs>)

### 8.5 Managed Member 执行层

- [src/runtime/orchestrator.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/runtime/orchestrator.rs>)
- [src/runtime/session_registry.rs](</E:/aigc内容整理/agent-teams-rs-team-mode/src/runtime/session_registry.rs>)

---

## 9. 已完成验证

### 9.1 已做

已完成的验证包括：

1. 多轮静态代码审查
2. Phase 4 服务层 review
3. MCP runtime review
4. 针对本次修改文件的 `rustfmt --edition 2024 --check`
5. `cargo metadata --format-version 1 --no-deps`

### 9.2 未完成

以下验证**没有在当前环境中跑通**：

1. `cargo check --lib --no-default-features`
2. `cargo test`
3. 真实 MCP client 端到端联调
4. 真实 managed backend 启动验证

原因不是代码逻辑本身，而是当前环境依赖拉取失败。

关键报错原样如下：

```text
failed to get `async-trait` as a dependency
Failed to connect to 127.0.0.1 port 7897
failed to download from https://index.crates.io/config.json
Could not resolve hostname (getaddrinfo() thread failed to start)
```

也就是说当前问题是：

1. 环境代理指向 `127.0.0.1:7897`，但代理不可用
2. 去掉代理后，当前 shell 仍无法正常解析 crates 域名
3. 本机 cargo cache 里缺少至少 `cc-sdk`

所以这份文档必须诚实地说：

**实现完成，但编译级最终验收受环境网络限制，尚未完全闭环。**

---

## 10. 已知限制

当前版本还有这些明确限制：

1. `legacy_prelude` 和旧 workflow 模块仍然保留在导出面中，还没有彻底清场
2. managed member 目前只接通生命周期，不包含完整的自主协作编排
3. MCP 工具没有做 caller 身份鉴别与权限体系
4. `resources/updated` 目前是 tool 写入驱动，不是全量事件总线
5. 还没有做生产级别的持久订阅、重放和恢复策略
6. 还没有针对真实 Codex / Claude / Gemini backend 做完整联调验收

---

## 11. 下一步建议

如果继续做，我建议按下面顺序推进：

1. 先修通 cargo 网络与代理，跑通 `cargo check` 和 `cargo test`
2. 用一个真实 MCP client 连 `team_mode_mcp` 做端到端联调
3. 补 managed member 的真实 spawn/resume/shutdown 验证
4. 视情况清理旧 `task/consensus/tui/checkpoint` 暴露面
5. 再决定是否补 caller 身份绑定、权限和 push 体验增强

---

## 12. 一句话总结

这次改造已经把项目主干从“多 CLI 的 task/mailbox 编排框架”，推进成了“可通过 MCP 暴露 `room/message/inbox/thread` 协议的 Team Mode runtime”。

代码主线已经完成，当前唯一没有彻底闭环的是：

**受环境代理/网络限制，尚未完成真实 cargo 编译与运行级验收。**
