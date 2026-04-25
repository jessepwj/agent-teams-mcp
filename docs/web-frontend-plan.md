> **[HISTORICAL — 2026-04]** 这是 Web 前端的初始设计计划，写于"只读"阶段。当前实现已经超出只读范围（新增 `POST /api/teams/.../rooms/main/messages` 让人类用户从浏览器发消息，sender 写死 `user`，lazy-create 该成员）。Codex worker 的 conversation 也已渲染（解析 `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`）。**计划文档保留作演进记录，实际实现以 `team-mode-web-guide.md` 为准。**

---

# Team Mode Web 前端计划

> 状态：设计计划  
> 目标：在不破坏现有 MCP、旧 dashboard 和 TUI 功能的前提下，新增一个只读 Web 前端，用于查看 team-mode 群聊历史和成员执行状态。

---

## 1. 目标与边界

本前端要解决三个核心问题：

1. 提供一个直观的群聊室，展示 lead 与 workers 之间的完整聊天历史。
2. 只读展示成员，包括 lead 的执行状态、活动轨迹和可用会话信息。
3. 默认展示成员对应进程会话，诊断和原始 JSON 只作为排查入口。

本计划的关键边界：

- MVP 完全只读，不提供创建 team、启动 worker、删除 worker、发送消息、ack/read 等写操作。
- 不改坏现有 `src/dashboard.rs`。旧 dashboard 仍服务 legacy `TeamOrchestrator` 语义。
- 新实现拆到独立目录和模块中，避免和现有 MCP runtime、TUI、旧 dashboard 纠缠。
- 当前后端只提供 member `session` 快照，不提供 turn-by-turn 原始执行日志。因此 MVP 不能伪装成有完整“进程日志回放”。

当前产品定位：

> 微信/Slack 式 team 群聊浏览器，加可切换的进程会话阅读面板。

---

## 2. 三轮子智能体讨论结论

本计划经过三轮子智能体讨论后收敛：

1. 第一轮：确认产品主线不是通用 KPI dashboard，而是“聊天浏览器 + 执行审阅台”。现有消息模型足够支撑聊天历史，但执行过程还缺事件流。
2. 第二轮：确认应新增 `team_mode_web`，不要硬改 `src/dashboard.rs`。MVP 只做聊天、线程、成员快照和 lead 活动摘要。
3. 第三轮：加入用户约束，确认新实现必须独立目录、MVP 完全只读、视觉走高密度工程工具风格。

核心决策只有三条：

- 新 Web 前端独立成 `team_mode_web`，不破坏旧 dashboard。
- MVP 只做只读浏览。
- 执行过程先做“快照 + 活动摘要”，等后端补 `events/logs` 后再做真正回放。

---

## 3. 现状评估

当前可直接利用的数据：

- `team://<team>`：team 元数据。
- `team://<team>/rooms/main`：主房间和消息列表。
- `team://<team>/threads/<thread_id>`：线程元数据和线程消息。
- `team://<team>/members/<name>/inbox`：成员 inbox 投影。
- `team://<team>/members/<name>/session`：成员状态和 execution 快照。

关键源码位置：

- `src/team_mode/mcp/resources.rs`：MCP resource 读接口。
- `src/team_mode/domain/message.rs`：消息结构，包含 `kind`、`thread_id`、`reply_to`、`mentions`、`effective_recipients`、`delivery_status`、`read_by`、`acked_by`。
- `src/team_mode/domain/member.rs`：成员和 execution profile。
- `src/runtime/agent_loop.rs`：worker 收消息、发回复的循环。
- `src/dashboard.rs`：旧 dashboard，绑定 legacy `TeamOrchestrator`，不适合作为 team-mode Web 主线。

当前缺口：

- 没有成员级 event/log 资源。
- 没有 lead 的真实进程日志。lead 是调用 MCP 的 Claude Code 会话，不是 Rust 管理的 worker subprocess。
- 没有分页、搜索和聚合型 Web DTO。
- `resources/updated` 是 best-effort，不应作为 Web 前端唯一刷新机制。

---

## 4. 架构决策

### 4.1 新增独立模块

推荐新增：

```text
src/
  team_mode_web/
    mod.rs
    app.rs
    routes.rs
    state.rs
    dto.rs
    read_model.rs
    resource_adapter.rs
    sse.rs
    error.rs
  bin/
    team_mode_web.rs

web/
  team-mode/
    package.json
    index.html
    src/
      app.tsx
      routes/
      views/
      components/
      hooks/
      styles/
      types/
      test/

tests/
  team_mode_web/
    api_smoke.rs
    read_model.rs
```

原则：

- `src/dashboard.rs` 保留不动。
- `team_mode_web` 只读复用 team-mode store/service/resource 数据。
- Web API 层做页面读模型聚合，前端不直接拼 MCP JSON。
- 前端源码放 `web/team-mode/`，与 Rust 源码分离。
- 静态产物由独立 build 生成，Rust 仅托管产物。

### 4.2 Feature 与入口

建议新增 Cargo feature：

```toml
team-mode-web = ["axum", "tower-http"]
```

新增独立二进制：

```bash
cargo run --features team-mode-web --bin team_mode_web -- --data-dir .agent-teams --listen 127.0.0.1:8787
```

不建议把 Web server 塞进 `team_mode_mcp` 进程里。MCP 是 stdio 生命周期，Web 是 HTTP 生命周期，分开更容易调试和部署。

---

## 5. 信息架构

桌面端采用五区结构：

```text
┌────────────────────────────────────────────────────────────┐
│ Top bar: team selector | search | time range | live status │
├──────────────┬───────────────────────────────┬─────────────┤
│ Left nav     │ Group chat                    │ Session pane│
│ teams        │ room: main                    │ message     │
│ rooms        │ message bubbles               │ session     │
│ members      │ system notices                │ member      │
│ filters      │ mention highlights            │ diagnostics │
├──────────────┴───────────────────────────────┴─────────────┤
│ Bottom status: filters | counts | data source | shortcuts   │
└────────────────────────────────────────────────────────────┘
```

移动端退化：

- 左栏收进抽屉。
- 右栏详情变底部抽屉或二级页面。
- 线程详情从消息详情进入独立子页。

一级视图只保留：

- `Chat`：群聊历史。
- `Members`：成员快照和活动。
- `Session`：右侧成员进程会话阅读。
- `Explore`：搜索、过滤、原始 JSON，Phase 2 再做。

不要做营销首页，打开即是实际可用的 team 浏览界面。

---

## 6. MVP 范围

### 6.1 群聊室

MVP 必须展示：

- 当前 team 的 `main` 房间消息。
- sender、时间、正文气泡。
- `@mention` 高亮。
- 异常 `delivery_status` 的轻量提示。
- 点击消息打开右侧详情。
- 点击 sender 或 mention 进行快速过滤。

消息主群聊以 `team://<team>/rooms/main` 为事实源。

默认群聊列表不展示 `kind`、线程数量、收件人、read/ack 等调试字段。它们进入右侧详情或诊断页签，避免主聊天区像调试表格。

### 6.2 线程

MVP 支持：

- 从消息详情查看线程。
- 右侧详情固定 root message。
- 下方按时间显示同 `thread_id` 消息。
- 显示回复数量、最后更新时间。

线程参考 Slack 的右侧 thread panel，而不是把所有回复永久展开污染主群聊。

### 6.3 成员只读面板

MVP 必须展示：

- lead 与 active workers。
- `name`、`kind`、`role_label`、`status`。
- worker 的 `adapter`、`model`、`cwd`、`sessionState`。
- `system_prompt` 和 `env` 默认折叠，且 `env` 要脱敏。
- 最近消息、最近被提及、最近回复。

默认右栏优先显示成员进程会话，而不是 profile/execution 字段列表。profile、execution、raw JSON 应放在 `详情` 或折叠区。

### 6.4 Lead 活动

lead 不是 managed subprocess，所以页面必须用准确命名：

- 使用 `Lead Activity` 或 `Lead Coordination`。
- 不使用 `Lead Process`、`Lead stdout`、`Lead trace` 这类误导名称。

MVP 展示：

- lead 是否存在。
- lead 所在 team 的 `owner_cc_pid`。
- lead 发出的消息。
- lead 收到的 worker 回复。
- lead inbox 摘要。
- lead 参与的线程。

文案应明确：

> Lead 活动基于消息与资源状态推断，不是 Claude Code 内部进程日志。

### 6.5 MVP 不做

MVP 不做：

- 发送消息。
- 创建/删除 team。
- 启动/停止 worker。
- ack/read 改状态。
- 原始 stdout/stderr 日志流。
- turn replay。
- tool call replay。
- 权限管理界面。

---

## 7. 后续阶段

### Phase 2：可观测浏览能力

- 搜索。
- 高级过滤。
- 时间范围。
- SSE 或轮询刷新。
- 消息虚拟列表。
- 原始 JSON 面板。
- stats 聚合。
- 深链到 message/thread/member。

### Phase 3：真正执行过程

后端新增事件流后再做：

- member events。
- turn start/complete。
- tool call/tool result。
- spawn/ready/exit/error。
- stdout/stderr 摘要。
- token、耗时、失败原因。
- 类 GitHub Actions 的 step/log 视图。

### Phase 4：管理模式

只有在只读模式稳定后再考虑：

- 发送消息。
- worker lifecycle 管理。
- team 管理。
- ack/read。
- 权限和确认弹窗。

管理模式必须显式开启，不能混入默认浏览界面。

---

## 8. 数据模型与接口草案

### 8.1 HTTP API

Web 前端建议读取 HTTP API，不直接读取 MCP resource。

```text
GET /api/teams
GET /api/teams/:team
GET /api/teams/:team/rooms/main
GET /api/teams/:team/threads/:threadId
GET /api/teams/:team/members
GET /api/teams/:team/members/:name
GET /api/teams/:team/members/:name/inbox
GET /api/teams/:team/members/:name/session
GET /api/teams/:team/members/:name/activity
GET /api/teams/:team/messages/:messageId
GET /api/teams/:team/search?q=...
GET /api/teams/:team/stats
GET /api/teams/:team/events
```

MVP 先实现前 8 个，`search/stats/events` 可放 Phase 2。

### 8.2 Endpoint 细化

#### `GET /api/teams`

用途：左上 team selector 和空状态判断。

返回草案：

```json
{
  "teams": [
    {
      "id": "demo",
      "name": "demo",
      "cwd": "E:/project",
      "status": "active",
      "leadMemberId": "lead",
      "memberCount": 3,
      "activeWorkerCount": 2,
      "lastMessageAt": "2026-04-24T10:30:00Z"
    }
  ]
}
```

派生规则：

- `memberCount` 从 `members.json` 或 member service 派生。
- `activeWorkerCount` 排除 `lead` 和 `removed`。
- `lastMessageAt` 从 `messages.jsonl` 最后一条有效消息派生。

#### `GET /api/teams/:team`

用途：页面初始加载的 team header。

返回草案：

```json
{
  "team": {
    "id": "demo",
    "name": "demo",
    "cwd": "E:/project",
    "status": "active",
    "leadMemberId": "lead",
    "ownerCcPid": 12345,
    "createdAt": "...",
    "updatedAt": "..."
  },
  "counts": {
    "members": 3,
    "workers": 2,
    "messages": 128,
    "threads": 12,
    "unreadForLead": 4
  }
}
```

#### `GET /api/teams/:team/rooms/main`

用途：主群聊数据源。

查询参数：

```text
limit=100
before=<message_id | timestamp>
after=<message_id | timestamp>
sender=<member>
mentioned=<member>
kind=dispatch|reply|status|notice|system|discussion
delivery=delivered|partial|failed|pending
q=<text>
```

MVP 可先支持 `limit`、`sender`、`mentioned`，其他参数 Phase 2。

返回草案：

```json
{
  "room": {
    "id": "main",
    "teamId": "demo",
    "status": "active"
  },
  "messages": [
    {
      "id": "msg-1",
      "sender": "lead",
      "senderKind": "lead",
      "kind": "dispatch",
      "body": "@alice please review this",
      "bodyPreview": "@alice please review this",
      "createdAt": "...",
      "mentions": ["alice"],
      "effectiveRecipients": ["alice"],
      "deliveryStatus": "delivered",
      "readCount": 0,
      "ackedCount": 0,
      "replyTo": null,
      "threadId": "thread-1",
      "threadReplyCount": 1,
      "isThreadRoot": true
    }
  ],
  "page": {
    "hasMoreBefore": false,
    "hasMoreAfter": true,
    "nextCursor": "..."
  }
}
```

#### `GET /api/teams/:team/threads/:threadId`

用途：右侧 thread panel。

返回草案：

```json
{
  "thread": {
    "id": "thread-1",
    "teamId": "demo",
    "roomId": "main",
    "rootMessageId": "msg-1",
    "replyCount": 3,
    "participants": ["lead", "alice"]
  },
  "rootMessage": { "id": "msg-1" },
  "messages": []
}
```

如果当前 `Thread` 模型没有 `rootMessageId`，read model 需要通过首条 `thread_id` 相同且 `reply_to == null` 的消息推断。

#### `GET /api/teams/:team/members`

用途：左栏成员列表。

返回草案：

```json
{
  "members": [
    {
      "name": "lead",
      "kind": "lead",
      "roleLabel": "lead",
      "status": "active",
      "sessionState": "coordinator",
      "badge": "lead",
      "lastActivityAt": "..."
    },
    {
      "name": "alice",
      "kind": "member",
      "roleLabel": "worker",
      "status": "active",
      "sessionState": "running",
      "adapter": "claude-code",
      "model": "default",
      "lastActivityAt": "..."
    }
  ]
}
```

#### `GET /api/teams/:team/members/:name`

用途：右侧成员详情。

返回草案：

```json
{
  "profile": {
    "name": "alice",
    "kind": "member",
    "roleLabel": "worker",
    "status": "active",
    "joinedAt": "..."
  },
  "execution": {
    "executionMode": "managed",
    "adapter": "claude-code",
    "model": null,
    "cwd": "E:/project",
    "skills": [],
    "sessionState": "running",
    "hasSystemPrompt": true,
    "envKeys": ["RUST_LOG"],
    "redactedEnv": {
      "RUST_LOG": "info"
    }
  },
  "activity": {
    "sentCount": 12,
    "receivedCount": 8,
    "mentionedCount": 9,
    "lastSentAt": "...",
    "lastReceivedAt": "..."
  }
}
```

`redactedEnv` 规则：

- 包含 `KEY`、`TOKEN`、`SECRET`、`PASSWORD`、`AUTH`、`COOKIE` 的 key 默认显示为 `"***"`。
- 其他 key 可显示值，但仍提供“隐藏全部值”的 UI 开关。

#### `GET /api/teams/:team/members/:name/activity`

MVP 先用消息派生活动，不依赖新增事件流。

返回草案：

```json
{
  "member": "alice",
  "source": "derived-from-messages",
  "items": [
    {
      "type": "received_message",
      "messageId": "msg-1",
      "summary": "lead dispatched a message to alice",
      "createdAt": "..."
    },
    {
      "type": "sent_reply",
      "messageId": "msg-2",
      "summary": "alice replied to lead",
      "createdAt": "..."
    }
  ],
  "limitations": [
    "No stdout/stderr or tool-call events are available yet."
  ]
}
```

### 8.3 DTO

页面读模型示例：

```text
TeamPageModel
  team
  members[]
  room
  messages[]
  counts

MessageView
  id
  sender
  kind
  body
  createdAt
  mentions[]
  effectiveRecipients[]
  deliveryStatus
  readCount
  ackedCount
  replyTo
  threadId
  threadReplyCount

MemberView
  name
  kind
  roleLabel
  status
  sessionState
  adapter
  model
  cwd
  recentActivity[]
  inboxCounts
```

### 8.4 读模型派生规则

#### 消息显示规则

- `sender == "lead"` 时使用 lead 样式，但仍按普通消息展示。
- `kind == "dispatch"` 和 `kind == "reply"` 默认都按普通聊天气泡展示，不在群聊中显示 kind 徽标。
- 若 `reply_to` 存在，则在详情中链接原消息。
- `kind == "system" | "notice" | "status"` 使用较弱视觉，不抢主消息注意力。
- `delivery_status == "failed" | "expired" | "partial"` 必须在群聊里轻量可见。
- 成功投递状态不在群聊里显示。
- `dropped_for` 不在群聊里平铺，放到详情面板。

#### Thread 派生规则

- 同一 `thread_id` 的消息归为一个 thread。
- `reply_to == null` 或该 thread 内最早消息作为 root 候选。
- 主群聊不默认显示 thread reply count。
- 如果某条消息是 reply 但没有可解析 root，则仍显示在群聊，并在详情里标记“thread metadata incomplete”。

#### Member 活动派生规则

- `sentCount`：`message.sender == member.name`。
- `receivedCount`：`effective_recipients` 包含 member.name。
- `mentionedCount`：`mentions` 包含 member.name。
- `lastActivityAt`：以上三类时间最大值。
- lead 的 `sessionState` 不从 execution profile 来，固定显示为 `coordinator` 或 `lead-activity`。

#### Inbox 状态派生规则

- `unread`：未出现在 `read_by` 和 `acked_by`。
- `read`：出现在 `read_by` 但未出现在 `acked_by`。
- `acked`：出现在 `acked_by`。
- UI 可同时显示 lead inbox 和单个 worker inbox，但 MVP 默认展示 lead inbox 摘要。

### 8.5 建议新增后端资源

为后续真正执行过程预留：

```text
team://<team>/members/<name>/activity
team://<team>/members/<name>/events
team://<team>/members/<name>/logs
team://<team>/messages/<message_id>
team://<team>/stats
```

事件模型建议：

```text
MemberEvent
  id
  team
  member
  kind: spawn | ready | input | output | tool_call | tool_result | turn_complete | error | exit
  summary
  payload
  createdAt
```

MVP 不依赖这些新增资源，但文档应明确 Phase 3 需要它们。

### 8.6 刷新策略

MVP 推荐先用轮询，当前实现已对当前 team 做 2 秒刷新：

- team list：10 秒。
- 当前 room：2 秒，当前实现已覆盖。
- 当前 members：2 秒，当前实现已覆盖。
- 当前 member session：2 秒。
- 非当前面板：15 秒。

Phase 2 再补 SSE：

```text
GET /api/teams/:team/events
event: message_created
event: message_updated
event: member_updated
event: team_deleted
```

SSE 初期可以是 server 轮询 store 后推送 diff，不要求先做完整事件总线。

### 8.7 URL 与深链

推荐路由：

```text
/teams
/teams/:team/chat
/teams/:team/chat/messages/:messageId
/teams/:team/threads/:threadId
/teams/:team/members/:name
/teams/:team/explore
```

选中消息、线程、成员都应能通过 URL 恢复右侧详情面板。这样用户可以分享定位，也便于调试。

当前静态前端使用 hash 深链：

```text
/#team=<team-id>
/#team=<team-id>&message=<message-id>
/#team=<team-id>&member=<member-name>
```

MCP `team_create` 创建成功后默认打开 `/#team=<team-id>`，因此页面会直接绑定并显示刚创建的 team。

---

## 9. UI 方案

### 9.1 视觉风格

采用高密度工程工具风格：

- 中性浅灰或深灰背景。
- 主交互色用青色/蓝绿色。
- 成功绿色、警告琥珀色、错误红色、中性灰色。
- 不使用大面积紫蓝渐变、营销式大卡片或装饰背景。
- 卡片只用于消息详情、成员详情、弹层，不做卡套卡。

### 9.2 群聊列表

主聊天区采用群聊气泡，而不是调试列表。

消息气泡显示：

- sender。
- 时间。
- 正文。
- mention 高亮。
- 异常 delivery 状态。

默认不显示：

- kind 徽标。
- delivery 成功状态。
- thread 数量。
- effective recipients。
- read/ack 摘要。

这些信息在右侧详情里看。系统状态消息，例如 worker 无输出、worker 不可用，居中显示为灰色系统提示。长正文允许换行并保持气泡宽度约束。

### 9.3 详情面板

右侧栏随选中对象变化，并使用页签降低信息噪声：

- `会话`：默认。展示成员对应 Claude session 内容，并按工作轮次组织：收到输入/Hook 输入、执行步骤、最终回复。工具调用和结果配对为可折叠行。
- `详情`：结构化字段和消息/成员摘要。
- `诊断`：日志、diagnostics source、raw JSON。

右侧详情内容包括：

- 选中消息：recipients、delivery、dropped_for、visibility、read_by、acked_by、raw JSON。
- 选中成员：profile、execution、sessionState、最近活动、raw JSON。
- 选中线程：root message、thread messages、参与者。
- 选中 lead：lead activity，不显示伪进程日志。

### 9.4 执行区

MVP 命名为 `Activity` 或 `Session Snapshot`。

展示内容：

- 当前状态。
- 最近消息输入输出摘要。
- 最近错误，如果有。
- execution config 折叠区。
- raw JSON 折叠区。

Phase 3 后再改成三页签：

- `Snapshot`
- `Events`
- `Raw JSON`

### 9.5 组件拆分

前端组件建议：

```text
AppShell
  TopBar
    TeamSelector
    GlobalSearch
    TimeRangePicker
    LiveStatus
  LeftNav
    RoomList
    MemberList
    ThreadShortcutList
    FilterSummary
  ChatTimeline
    ChatMessageBubble
    ChatSystemNotice
    MentionText
  DetailPane
    SessionTranscript
    WorkTurn
    ToolCallRow
    MessageDetail
    ThreadDetail
    MemberDetail
    LeadActivityDetail
    RawJsonPanel
  BottomStatusBar
```

数据 hooks：

```text
useTeams()
useTeam(team)
useRoomTimeline(team, filters)
useThread(team, threadId)
useMembers(team)
useMember(team, name)
useMemberActivity(team, name)
```

组件原则：

- `ChatTimeline` 不知道 API 细节，只吃 `MessageView[]`。
- `DetailPane` 通过 URL 或 selection state 决定展示对象。
- `RawJsonPanel` 默认折叠，且只读。
- 所有 loading/error/empty 状态由组件自己局部处理。

### 9.6 交互细节

主群聊：

- 点击消息气泡：右栏打开 Message Detail。
- 点击 sender：添加 `sender=<name>` 过滤。
- 点击 mention：添加 `mentioned=<name>` 过滤。
- 双击正文：无操作，避免误解为可编辑。
- 线程、投递、raw JSON 等调试信息只在右栏查看。

成员列表：

- 点击成员：右栏打开 Member Detail。
- 状态点颜色：
  - running：绿色。
  - starting：琥珀色。
  - failed：红色。
  - removed：灰色。
  - lead/coordinator：青色。

过滤器：

- 默认只展示少量快筛：sender、mentioned、kind、failed only。
- 高级过滤折叠：visibility、audience policy、read/acked、time range。
- 清除过滤必须一键可见。

键盘：

- `/` 聚焦搜索。
- `Esc` 关闭右侧详情或清除当前浮层。
- `j/k` 或上下键移动消息选择。
- `Enter` 打开选中项详情。
- `f` 打开过滤菜单。

这些快捷键不是 MVP 必须，但设计时预留。

### 9.7 响应式规则

桌面宽度大于 1200px：

- 三栏全展开。
- 左栏 260px。
- 中间群聊和右栏默认按剩余空间 `1:1`。
- 右栏可拖拽调整，手动调整后持久化。

平板 768px 到 1199px：

- 左栏可折叠。
- 右栏作为 overlay drawer。

手机小于 768px：

- 单栏。
- 顶部 team selector + 搜索。
- 左栏变菜单。
- 详情变二级页面或底部抽屉。

### 9.8 空状态和错误状态

空状态：

- 群聊为空：暂无消息。
- 线程为空：此消息暂无回复。
- 成员为空：暂无 workers，仅有 lead。
- 执行为空：当前仅有会话快照，没有事件流。

错误状态：

- 局部面板报错，不整页崩溃。
- 保留关键英文错误信息，再给中文解释。
- 支持重试。

---

## 10. 成熟应用借鉴

采用：

- `E:\aigc内容整理\yepanywhere`：进程会话渲染参考其消息预处理、assistant turn 分组、tool_use/tool_result 配对、工具行折叠和滚动保持策略。本项目在此基础上额外按“收到输入/Hook 输入 -> 执行步骤 -> 最终回复”组织每个成员的工作轮次。
- Slack：群聊信息流、右侧 thread panel、mention 高亮、unread badge。
- GitHub Actions：状态徽标、折叠详情、失败态突出、日志行深链思路。
- Linear：左栏导航、主列表、右侧详情、紧凑过滤。
- Grafana/Datadog：时间范围、局部刷新、结构化日志详情、facet/filter 思路。
- Discord forum：标签过滤和持久讨论概念可借鉴到 thread 列表。

不采用：

- Discord 的强社交化头像和娱乐化装饰。
- GitHub Actions 默认铺满原始日志的体验，因为 MVP 没有真实日志数据。
- 营销式 dashboard 的大 KPI 卡和 hero。
- 重动画和视觉噱头。

参考资料：

- Slack threads: https://slack.com/help/articles/115000769927-Use-threads-to-organize-conversations
- GitHub Actions logs: https://docs.github.com/en/actions/how-tos/monitor-workflows/use-workflow-run-logs
- Linear filters: https://linear.app/docs/filters
- Grafana log explore: https://grafana.com/docs/grafana/latest/visualizations/explore/logs-integration/
- Discord forum channels: https://support.discord.com/hc/en-us/articles/6208479917079-Forum-Channels-FAQ

---

## 11. 只读约束

MVP 的 HTTP API 只暴露 `GET`。

禁止：

- `POST /send_message`
- `POST /worker_add`
- `DELETE /worker`
- `POST /ack`
- 任何会写 `messages.jsonl`、`members.json` 或 `lead_pending.jsonl` 的操作。

前端也不显示这些按钮。即使未来后端 API 有能力写入，MVP UI 也不调用。

敏感字段处理：

- `env` 默认隐藏，值脱敏。
- `system_prompt` 默认折叠。
- 原始 JSON 面板默认折叠。
- 日志和消息正文只读复制，不编辑。

前端防误操作：

- 不渲染任何 destructive icon。
- 不显示“Send”、“Start”、“Stop”、“Delete”、“Ack”按钮。
- 不把状态徽标做成可点击按钮样式。
- 复制和跳转操作使用明确图标，并提供 tooltip。
- 所有详情字段用 readonly text/code block，而不是 input。

后端防误操作：

- `team_mode_web` route 只注册 GET。
- API state 不持有 `TeamModeToolset` 写工具。
- read model 使用 store/service 的只读方法。
- 如果未来新增写 API，放到独立 `admin_routes.rs`，并默认不编译或不挂载。

---

## 12. 风险与回退

| 风险 | 影响 | 处理 |
|---|---|---|
| `session` 资源不含真实过程日志 | 执行区容易误导 | MVP 明确叫快照和活动摘要 |
| 消息量增大 | 浏览卡顿 | Phase 2 加分页或虚拟列表 |
| `resources/updated` 不可靠 | 实时性不足 | Web 先用轮询，后续补 SSE |
| lead 不是 managed worker | 无法展示 lead stdout | 只展示 lead coordination activity |
| `env/system_prompt` 敏感 | 信息泄露 | 默认折叠与脱敏 |
| 新旧 dashboard 混用 | 破坏现有功能 | 新增 `team_mode_web`，旧文件不动 |

---

## 13. 测试与验收

MVP 验收标准：

- 能启动独立 Web server。
- 能列出 teams。
- 能打开 team 的 `main` 群聊历史，且默认是气泡式群聊而不是调试字段列表。
- 能查看每条消息详情。
- 能打开 thread 详情。
- 能查看 lead 和 worker 成员面板。
- 能查看 lead 和 worker 的进程会话内容。
- lead 面板不出现“process log/stdout/trace”误导表达。
- 页面没有写操作入口。
- API 错误只影响局部面板。

测试建议：

- Rust API smoke tests。
- read model 单元测试。
- 前端组件测试。
- 空数据 fixture。
- 多成员、多线程、大消息 fixture。
- 敏感字段脱敏测试。

详细测试矩阵：

| 场景 | 数据 | 预期 |
|---|---|---|
| 空项目 | 无 `.agent-teams` 或无 team | 显示空 team 状态，不崩溃 |
| 只有 lead | team 存在，无 worker | 左栏显示 lead，成员区说明暂无 workers |
| 普通派发 | lead dispatch 给 alice | 群聊显示 lead 气泡，alice mention 高亮，不显示 dispatch 徽标 |
| worker 回复 | alice reply 给 lead | 群聊显示 alice 气泡，lead inbox count 更新，不显示 reply 徽标 |
| 线程消息 | 多条同 `thread_id` | thread panel 正确分组 |
| 未知 recipient | `dropped_for` 非空 | 群聊轻量显示 partial，详情显示 dropped 原因 |
| failed delivery | `delivery_status=failed` | 群聊显示失败提示 |
| removed worker | status removed | 成员列表灰色分组，不算 active worker |
| 敏感 env | `ANTHROPIC_API_KEY` | 默认显示 `***` |
| 大消息 | 长正文多行 | 群聊气泡不撑破布局，详情完整展示 |
| API 失败 | team not found | 对应面板局部错误和重试 |

前端视觉验收：

- 1366x768 下三栏不重叠。
- 桌面默认中间群聊和右栏接近 `1:1`。
- 主群聊只显示头像、发送者、时间、消息气泡和必要异常状态。
- 不在主群聊里直接铺开 kind、delivered、thread count、recipients。
- 390x844 下可单栏使用。
- 长成员名、长 message id、长单词不会撑破布局。
- 状态徽标文字不溢出。
- 右栏 raw JSON 可滚动，不挤压主群聊。

---

## 14. 推荐实施顺序

1. 新建 `src/team_mode_web/` 和 `web/team-mode/` 空骨架。
2. 实现只读 HTTP API：teams、team、room、members、session。
3. 实现前端三栏布局。
4. 实现群聊时间线。
5. 实现消息详情和 thread 侧栏。
6. 实现成员详情和 lead activity。
7. 增加过滤、搜索和时间范围。
8. 增加 SSE 或轮询刷新。
9. 后端补 `events/logs` 后，升级执行过程视图。

---

## 15. 详细任务拆分

### Task 1：Rust Web 骨架

产出：

- `src/team_mode_web/mod.rs`
- `src/team_mode_web/routes.rs`
- `src/team_mode_web/state.rs`
- `src/team_mode_web/error.rs`
- `src/bin/team_mode_web.rs`

验收：

- `GET /healthz` 返回 ok。
- `GET /api/teams` 能从 `.agent-teams` 读取 team。
- 不依赖旧 `TeamOrchestrator`。
- MCP `team_create` 默认启动/复用只读 Web server，并打开 `/#team=<team-id>`。

### Task 2：Read Model 层

产出：

- `dto.rs`
- `read_model.rs`
- `resource_adapter.rs`

职责：

- 把 store/service 的 domain model 转成 Web DTO。
- 做成员 count、thread count、last activity 派生。
- 做 env 脱敏。
- 做 body preview。

验收：

- 有独立单元测试覆盖派生规则。
- DTO 不泄漏不必要内部字段。

### Task 3：Group Chat API

产出：

- `GET /api/teams/:team/rooms/main`
- `GET /api/teams/:team/messages/:messageId`
- 基础过滤：sender、mentioned、limit。

验收：

- 能正确展示 dispatch/reply/status。
- thread reply count 正确。
- message detail 能返回完整 raw JSON。

### Task 4：Members API

产出：

- `GET /api/teams/:team/members`
- `GET /api/teams/:team/members/:name`
- `GET /api/teams/:team/members/:name/activity`

验收：

- lead 显示为 coordinator。
- worker 显示 sessionState。
- removed worker 正确分组。
- activity 明确标注 `derived-from-messages`。

### Task 5：前端 Shell

产出：

- `web/team-mode` 前端项目。
- `AppShell`、`TopBar`、`LeftNav`、`ChatTimeline`、`DetailPane`。

验收：

- 桌面三栏可用。
- 移动端单栏可用。
- 所有面板有 loading/error/empty 状态。

### Task 6：聊天与线程 UI

产出：

- `ChatMessageBubble`
- `ChatSystemNotice`
- `MentionText`
- `ThreadDetail`
- `MessageDetail`

验收：

- mention 高亮。
- 主群聊默认不显示 kind、delivered、thread count、recipients。
- 系统状态消息居中弱化显示。
- 异常状态提示准确。
- 长消息不撑破布局。

### Task 7：成员与 Lead UI

产出：

- `MemberList`
- `MemberDetail`
- `LeadActivityDetail`
- `SessionSnapshot`
- `SessionTranscript`
- `WorkTurn`
- `ToolCallRow`

验收：

- lead 不显示伪日志。
- 右栏默认展示会话阅读视图。
- 每轮会话能明显区分发给该成员的输入、Hook 注入内容、中间工具步骤和最终回复。
- 工具调用和工具结果配对展示，并可折叠查看详情。
- worker execution 默认折叠敏感内容。
- raw JSON 只读。

### Task 8：刷新与性能

产出：

- 轮询刷新。
- 消息列表初步分页或限制。
- 后续可替换为 SSE。

验收：

- 新消息 2 秒内出现在当前 room。
- 切换 team 不残留旧状态。
- 1000 条消息不明显卡顿。如果未实现虚拟列表，MVP 至少限制默认加载数量。

---

## 16. Open Questions

这些问题不阻塞 MVP，但实现前应记录：

- Web server 是否和 MCP server 共用 data dir 参数，还是默认解析当前项目 `.agent-teams`？
- Web 是否需要本机访问限制，只监听 `127.0.0.1`？
- 是否允许暴露 `system_prompt` 原文？默认建议折叠并提示敏感。
- 是否需要读取项目根的 `lead_pending.jsonl` 来显示未消费提醒？MVP 可不做，因为 inbox/message 才是事实源。
- 前端技术栈用 React/Vite 还是纯静态 HTML？推荐 React/Vite，因为时间线、详情面板和状态管理更复杂。

---

## 17. 最小可交付定义

如果只做第一版，交付物应包括：

1. 独立 Web server 可启动。
2. 独立前端目录。
3. team 列表。
4. main room 消息列表。
5. message detail。
6. thread detail。
7. member list。
8. member detail。
9. lead activity。
10. 明确只读，无写操作入口。

只要这 10 项完成，就已经满足用户当前“web 即可、群聊历史、成员执行过程查看”的核心需求。真正的执行日志回放属于后续后端事件流能力。
