# Team Mode Web 实现与排查指南

> 状态：当前仓库实现对应说明  
> 适用范围：`team_mode_web` Web 前端（含人类用户发消息）、诊断层、运行/排查/验收

---

## 1. 当前已实现范围

`team_mode_web` 是 Team Mode 的 Web 前端，跑在 daemon 内部线程，监听 `127.0.0.1:8787+`：

1. **群聊视图**：以聊天时间线方式查看 team `main` 房间历史。
2. **成员会话**：查看成员对应进程的会话内容（claude-code JSONL + codex rollout 都已支持）。
3. **诊断与详情**：右侧面板查看消息路由元数据、成员档案、token 用量、诊断来源等原始数据。
4. **人类发送消息**（2026-04-25 新增）：底部发送条让坐在浏览器前的人发消息进群聊，sender 固定为 `user`（非 lead 非 worker，独立身份）。

仍**没有**的写操作：启动/停止 worker、删除 team / 成员、ack/read 标记、修改成员配置。这些走 MCP 工具。

### 1.1 发消息功能（POST 端点）

- 路由：`POST /api/teams/<team>/rooms/main/messages`
- Body：`{"body": "...", "mentions": ["lead" | "<worker>"]}`
- 行为：
  - sender 写死 `user`（常量 `WEB_USER_SENDER` 在 `read_model.rs`）
  - 首次 POST 时 lazy-create 该成员（kind=Member, role_label="user", 无 execution profile）
  - 调用同一个 `MessageService::send`（与 MCP `send_message` 工具完全相同的路径），所以路由规则、`InboxNotifier` 唤醒、`LeadPendingWriter` 写 `lead_pending.jsonl` 都生效
  - kind=`Discussion`（非 dispatch / reply）
- 前端：`#composerForm` 在时间线下方 sticky；左侧 `<select>` 选 @ 谁（默认 `lead`），中间 textarea，右侧"发送"按钮。Enter 发送，Shift+Enter 换行
- `worker_list` 工具自动过滤 `role_label="user"` 的成员（避免它被当 worker 误标记 dead）
- `worker_add` 拒绝 `name="user"`（保留名）

### 1.2 成员颜色区分

每个成员（含 `user`）在群聊里有独立颜色：
- `lead` → 固定青色（hue 188，匹配品牌 accent）
- `user` → 固定暖橙色（hue 28，让人类输入在 AI 群聊里一眼可辨）
- 其他 worker → 名字 djb2 hash → hue（避开 14..50 / 175..205 两段保留色）
- 实现：`web/team-mode/app.js` 的 `senderHue()` + `renderSenderBadge()`，CSS 用 `hsl(var(--sender-hue), …)`

---

## 2. 代码入口

### Rust 入口

- `src/bin/team_mode_web.rs`
- `src/team_mode_web/app.rs`
- `src/team_mode_web/routes.rs`
- `src/team_mode_web/read_model.rs`
- `src/team_mode_web/dto.rs`
- `src/team_mode_web/state.rs`

### 前端入口

- `web/team-mode/index.html`
- `web/team-mode/app.js`
- `web/team-mode/styles.css`

### 测试

- `tests/team_mode_web_api.rs`
- `web/team-mode/app.smoke.test.mjs`

### 相关辅助

- `src/util/session_discovery.rs` — Claude Code JSONL 自动发现
- `src/util/codex_session_discovery.rs` — Codex `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` 扫描 + 5s TTL 缓存

### 实现对应关系

- 群聊/成员/diagnostics 聚合逻辑：`src/team_mode_web/read_model.rs`
- POST 发消息 + lazy-create user：`read_model::post_main_room_message` + `ensure_web_user_member`
- HTTP 路由挂载（含 POST 分支）：`src/team_mode_web/routes.rs`
- std-only HTTP 服务（含 Content-Length body 解析、256 KiB body cap、10s read timeout）：`src/team_mode_web/app.rs`
- 跨进程共享 `MessageService`（让 web POST 触发 inbox notifier + lead_pending writer）：`SHARED_MESSAGE_SERVICE: OnceLock<MessageService>` in `state.rs`，daemon 在 toolset 构造时调用 `install_shared_message_service()`
- Claude session 自动发现与解析：`src/util/session_discovery.rs`
- Codex rollout 解析（response_item / function_call / reasoning）：`parse_codex_conversation` in `read_model.rs`
- diagnostics DTO：`src/team_mode_web/dto.rs`
- 前端状态管理与渲染：`web/team-mode/app.js`

---

## 3. 当前 API

当前 `team_mode_web` 已暴露以下只读接口：

- `GET /healthz`
- `GET /api/teams`
- `GET /api/teams/:team`
- `GET /api/teams/:team/rooms/main`
- `GET /api/teams/:team/members`
- `GET /api/teams/:team/members/:name`
- `GET /api/teams/:team/members/:name/activity`
- `GET /api/teams/:team/diagnostics`

其中：

- `rooms/main` 用于主群聊。
- `members/:name/activity` 是从消息派生出的活动摘要，不是进程日志。
- `diagnostics` 是文件/会话级排查视图，不是 per-member stdout/stderr。

---

## 4. 前端信息架构

页面保持三栏结构：

- 左栏：房间、成员、筛选。
- 中栏：群聊。
- 右栏：会话、详情、诊断。

### 4.1 中栏群聊

中栏不是 diagnostics 列表，也不是事件调试流。它按类似微信群聊的方式显示：

- 发送人头像缩写。
- 发送人名称。
- 发送时间。
- 消息气泡。
- `@mention` 高亮并可点击筛选。
- 系统状态消息居中显示为灰色提示。

默认不会在群聊列表里铺开这些调试字段：

- `kind`
- `delivery_status`
- `thread_reply_count`
- `effective_recipients`
- `read_by`
- `acked_by`

这些字段仍可通过点击消息后在右侧详情里查看。异常投递状态，例如 `failed`、`expired`、`partial`，会在群聊气泡旁保留轻量提示。

### 4.2 右栏

右栏默认展示成员对应的**进程会话**，目的是让用户阅读该成员实际会话内容，而不是阅读大量诊断字段。

右栏页签：

- `会话`：默认页签。展示成员/lead 对应 Claude session 内容，按“工作轮次”组织。每轮先显示收到的输入或 Hook 输入，再显示中间执行步骤，最后突出最终回复。工具调用和工具结果会配对为可折叠行。
- `详情`：展示消息、成员、lead activity 的结构化详情。
- `诊断`：展示 Team Diagnostics、Diagnostics Sources、Lead Session Diagnostics 和 Raw JSON。诊断内容默认不作为主要阅读界面。

选中消息时，右栏切到 `详情`。选中成员时，右栏切到 `会话`。

会话阅读规则：

- 用户/Hook 输入显示为本轮起点。
- `tool_use` 和 `tool_result` 按 `toolUseId` 配对，作为执行步骤展示。
- 每轮最后一个没有后续工具调用的 assistant 文本会标记为 `最终回复`。
- 如果一轮只有工具或系统事件，没有最终文本回复，会显示 `暂无最终回复`。
- session setup、thinking、system/error 仍保留，但弱化为辅助信息。

### 4.3 分栏与刷新

桌面端默认：

- 左栏固定约 `260px`，可拖拽调整。
- 中栏群聊和右栏默认按剩余空间 `1:1`。
- 右栏可拖拽调整，手动调整后会持久化到 `localStorage`。
- 窗口尺寸变化时，如果右栏没有被用户手动改过，会重新计算 `1:1`。

页面每 2 秒刷新当前 team 的基本数据、主群聊和成员列表，因此新增 worker、worker 回复、lead 新消息会自动显示。手动刷新按钮仍然可用。

### 4.4 深链

深链已支持：

- `#team=<id>`
- `#message=<id>`
- `#member=<name>`

并且已覆盖：

- 创建 team 后默认打开并定位到新 team
- 当前 team 的成员和消息自动轮询刷新
- 错误态重试
- team 列表为空时回到 `no teams`
- 筛选状态下 thread detail 仍显示完整线程
- 默认中文 UI，并提供 English 切换按钮
- 群聊列表保持简洁，诊断信息不污染主阅读区

---

## 5. Diagnostics 数据源

`GET /api/teams/:team/diagnostics` 当前返回三类信息：

1. **团队诊断源列表**
2. **lead session 摘要**
3. **限制说明**

### 5.1 诊断源列表 `sources[]`

当前会探测：

- `lead_pending.jsonl`
  - 项目根
  - `base_dir`
- `mcp.log`
  - 在项目根和 `base_dir` 之间择优
- `.lead-pending-wake.log`
  - 在项目根和 `base_dir` 之间择优

每个源包含：

- `id`
- `label`
- `kind`
- `path`
- `exists`
- `sizeBytes`
- `updatedAt`
- `preview`

### 5.2 日志路径选择规则

#### `lead_pending.jsonl`

这是当前实现里最容易混淆的文件。它不一定只在 `.agent-teams/` 下。

因此 diagnostics 会分别探测：

- 项目根 `lead_pending.jsonl`
- `base_dir/lead_pending.jsonl`

这样不会漏掉实际在用的队列文件。

#### `mcp.log` / `.lead-pending-wake.log`

这两类文件当前通过 `preferred_diagnostics_path(project_root, base_dir, file_name)` 选取：

1. 如果只有一个存在，用它。
2. 如果两个都存在，优先更新时间更近的那个。
3. 如果两个都不存在，保留项目根候选路径用于展示。

### 5.3 预览截断规则

当前预览是受限的：

- 文件只读取前约 `4 KiB`
- 再截到约 `800` 字符
- 默认最多展示少量行，避免超大日志直接塞进接口返回

这是为了保持前端稳定和响应体可控。

---

## 6. Lead Session Diagnostics

`leadSession` 来源于 `src/util/session_discovery.rs`。

它会尝试基于 repo/cwd 去发现 Claude session 文件，并解析：

- `sessionCount`
- `latestSessionId`
- `latestModifiedAt`
- `recentToolCalls`
- `tokenUsage`
- `sourcePath`

### 6.1 这是什么

这是 **Claude session 文件摘要**，用于辅助排查 lead 侧行为。

### 6.2 这不是什么

它不是：

- lead 的真实 stdout
- lead 的 stderr
- worker 的进程日志
- tool 调用全量回放

前端文案刻意使用：

- `Lead Session Diagnostics`
- `Team Diagnostics`

避免误导成不存在的“进程日志面板”。

### 6.3 Windows 路径编码修复

`session_discovery.encode_project_path()` 现已修正为：

- 把非 ASCII 字母数字字符统一编码为 `-`
- 兼容 `/`
- 兼容 `\\`
- 兼容 `:`
- 覆盖 Windows 风格路径与非 ASCII 路径

否则在 Windows 上几乎一定发现不到 `~/.claude/projects/...` 下的 session 目录。

---

## 7. 服务日志

`src/team_mode_web/app.rs` 里已加入最小访问日志。

每个请求会向 `stderr` 输出一行：

```text
[team_mode_web] GET /api/teams/demo/diagnostics -> 200 in 4ms
```

字段包含：

- method
- path
- status
- elapsed ms

这部分不引入任何新依赖，适合本地排查。

---

## 8. 当前测试与验证

### 8.1 当前机器可运行并已通过

以下命令在当前机器上已通过：

```powershell
cargo check --features team-mode-web --bin team_mode_web
cargo check --features team-mode-web --test team_mode_web_api
cargo check --lib
```

前端 smoke：

```powershell
$env:SystemRoot='C:\Windows'
bun test web/team-mode/app.smoke.test.mjs
```

当前前端 smoke 结果：

```text
20 pass
0 fail
```

当前 smoke 覆盖：

- 空态
- `#message=` 深链恢复
- `#member=lead` 深链恢复
- 默认中文与 English 切换
- 左/右分栏拖拽调整
- 中栏群聊和右栏默认 `1:1`
- 群聊列表按气泡方式渲染，不直接铺开投递、线程、kind 等调试字段
- diagnostics UI 渲染
- 筛选状态下 thread detail 完整性
- 刷新失败与 Retry 恢复
- stale detail error 清理后重新打开 Lead Activity
- 成员进程会话打开
- 成员进程会话首次打开默认滚动到底部
- 会话内容中工具调用和工具结果配对渲染
- 会话按工作轮次展示，并突出最终回复
- 后台成员刷新失败时保留已缓存详情

### 8.2 当前机器受限

以下验证在当前机器受环境阻塞：

#### `cargo test` / `cargo build`

关键报错：

```text
link.exe not found
```

原因：

- 当前 Windows MSVC linker 不可用
- 这不是 Rust 源码本身的类型检查错误

#### `node`

当前机器 `node --check ...` 启动阶段报：

```text
Assertion failed: ncrypto::CSPRNG(nullptr, 0)
```

因此前端脚本运行态验证改用 `bun test`。

#### 本地临时 HTTP server / socket

本机还出现过：

```text
WinError 10106 无法加载或初始化请求的服务提供程序
```

所以浏览器级验证在当前环境不稳定，不能作为唯一验收手段。

### 8.3 当前机器不适合作为全量回归环境

当前最可靠的验证组合是：

- `cargo check`
- `bun test`
- 代码审阅

而不是把本机当成完整的最终回归环境。

---

## 9. 浏览器级验证建议

当本机网络/套接字环境正常后，建议补以下人工验证：

### 桌面

- 1366x768
- 三栏不重叠
- 中栏群聊和右栏默认接近等宽
- 群聊列表只显示头像、发送者、时间、消息气泡和必要异常状态
- 诊断内容只在右栏 `诊断` 页签里查看，不把默认会话页撑坏
- 长路径、长日志预览可滚动

### 平板断点

- 960px 左右
- 顶栏换行正常
- 右栏 diagnostics 不遮挡主群聊

### 手机断点

- 720px 以下
- detail grid 变单列
- diagnostics preview 不溢出父容器

建议优先看：

- Team Diagnostics
- Diagnostics Sources
- Lead Session Diagnostics

---

## 10. 运行方式

### 10.1 创建 team 后默认打开

通过 MCP `team_create` 创建 team 成功后，当前实现会默认：

1. 在当前持有 toolset 的进程内启动只读 Team Mode Web server。默认新架构下这是 `team_mode_daemon`，禁用 daemon relay 时才是 MCP 进程。
2. 优先监听 `127.0.0.1:8787`，如果端口被占用则尝试 `8788` 到 `8799`。
3. 打开浏览器到刚创建的 team：

```text
http://127.0.0.1:<port>/#team=<team-id>
```

`team_create` 的结构化返回里会附带：

```json
{
  "web": {
    "enabled": true,
    "url": "http://127.0.0.1:8787/#team=demo",
    "opened": true,
    "error": null
  }
}
```

这个动作是 best-effort：即使浏览器打开失败或端口不可用，team 创建仍然成功，错误会写在 `web.error`。

如需关闭自动打开：

```powershell
$env:TEAM_MODE_WEB_AUTO_OPEN='0'
```

CI / cargo test 进程会自动禁用该行为，避免测试时弹浏览器。

页面打开后会每 2 秒刷新当前 team 的基本数据、主群聊和成员列表，因此新增 worker、worker 回复、lead 新消息会自动显示。手动刷新按钮仍然可用。

### 10.2 手动启动

环境完整时，建议这样启动：

```powershell
cargo run --features team-mode-web --bin team_mode_web -- --data-dir .agent-teams --listen 127.0.0.1:8787
```

说明：

- `--data-dir` 指向 Team Mode 数据目录
- 默认只监听本机
- 启动后通过 `/healthz` 确认服务存活

---

## 11. 排查顺序

如果页面看不到数据，建议按这个顺序排：

1. `GET /healthz` 是否返回 `ok`
2. `GET /api/teams` 是否有 team
3. `GET /api/teams/:team` 是否能读到 team 头信息
4. `GET /api/teams/:team/rooms/main` 是否有消息
5. `GET /api/teams/:team/members` 是否有成员
6. `GET /api/teams/:team/diagnostics` 是否返回 diagnostics
7. 先看 `diagnostics.sources[]`，确认当前真实源落在项目根还是 `base_dir`

如果 diagnostics 为空，再继续看：

1. 项目根是否有 `lead_pending.jsonl`
2. `base_dir` 是否有 `lead_pending.jsonl`
3. 项目根或 `base_dir` 是否有 `mcp.log`
4. 项目根或 `base_dir` 是否有 `.lead-pending-wake.log`
5. `team.cwd` 是否正确
6. `~/.claude/projects/` 下是否存在对应项目的 session 目录

---

## 12. 常见问题

### Q1：为什么没有 worker stdout/stderr 面板？

因为当前后端并没有稳定暴露 per-member 进程日志流。当前能诚实展示的是：

- 消息活动
- execution snapshot
- 文件级 diagnostics
- lead session 摘要

### Q2：为什么 Lead 只能看到 session diagnostics，不是完整进程日志？

因为 lead 不是 Rust 管理的 worker subprocess。当前更准确的来源是：

- 群聊消息
- pending 队列
- hook log
- Claude session 文件

### Q3：为什么 diagnostics 有时显示项目根，有时显示 `base_dir`？

因为当前实现里不同文件的真实落点并不完全一致，尤其是：

- `lead_pending.jsonl`
- `.lead-pending-wake.log`

所以 diagnostics 做了多位置探测或择优。

### Q4：如果 `leadSession.discovered = false` 怎么办？

先看：

1. `team.cwd` 是否为空或错误
2. `GET /api/teams/:team/diagnostics` 里的 `cwd` 和 `sources[]` 是否符合预期
3. `~/.claude/projects/` 下是否有对应项目目录
4. 当前平台路径编码是否匹配

如果 repo 在 Windows 且路径含中文/点号，当前编码修复已覆盖常见情况，但仍建议人工确认目录是否真实存在。

---

## 13. 已知限制

当前实现仍有这些限制：

1. 没有真正的 per-member stdout/stderr 流。
2. diagnostics 预览是截断视图，不是原始全量日志。
3. lead session 目前优先使用成员 execution 里持久化的 `session_id` 精确匹配；缺失时会回退到 cwd 最新 session，仍可能不是用户期望的会话。
4. 浏览器级验证在当前机器因 socket/运行环境问题不完整。
5. `cargo test` / `cargo build` 仍受 `link.exe` 缺失影响。

---

## 14. 后续建议

下一步最值得做的是：

1. 补真实浏览器级布局验证。
2. 把 thread detail 拆成独立深链面板。
3. 为 message detail 增加 `read_by / acked_by / dropped_for` 的直观摘要。
4. 如果后端未来补 `events/logs`，再把 execution 视图升级成真正的事件/步骤/日志视图。

---

## 15. 对照计划

本实现对应并推进了：

- `docs/web-frontend-plan.md`

计划文档是设计基线，本文件是当前实现和排查说明。后续继续演进时，优先更新本文件里的：

- 数据源
- 路径规则
- 验证命令
- 已知限制
- 排查顺序
