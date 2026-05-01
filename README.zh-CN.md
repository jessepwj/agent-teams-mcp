<p align="right">
  <a href="README.md">English →</a>
</p>

<p align="center">
  <em>一个 MCP server，将 Claude Code 变成一支 AI worker 团队的协调者——支持 worker 向 lead 的真正推送，以及用于实时观察整个团队的 Web UI。</em>
</p>

<p align="center">
  <a href="https://github.com/jessepwj/agent-teams-mcp/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.85%2B-orange.svg">
  <img alt="MCP" src="https://img.shields.io/badge/protocol-MCP-purple.svg">
  <img alt="Tests" src="https://img.shields.io/badge/tests-300%20passing-brightgreen.svg">
</p>

# agent-teams-mcp

`agent-teams-mcp` 是一个用 Rust 编写的 [Model Context Protocol](https://modelcontextprotocol.io) server，它将你的 Claude Code CLI 变成一个 **lead**（团队主控）。lead 可以将 AI worker 代理（Claude Code、Codex 或 Gemini CLI）作为受管子进程来启动和协调，通过 `@mention` 风格的消息在它们之间路由通信，并且——最关键的一点——**以无需轮询、无需手动调用 `inbox_read` 的方式，通过自动注入 `<system-reminder>` 将 worker 回复推送到 lead 的下一个 turn**。

Web UI（自动在 `http://127.0.0.1:8787` 启动）实时渲染 lead 与所有 worker 之间的对话，让你可以实时观察团队的工作进展，甚至可以作为人类成员随时介入。

<p align="center">
  <img alt="Push latency" src="https://img.shields.io/badge/worker--reply%20%E2%86%92%20lead-%7E50ms-brightgreen.svg">
  <img alt="Auth" src="https://img.shields.io/badge/API%20key%20auth-supported-brightgreen.svg">
  <img alt="Daemon" src="https://img.shields.io/badge/detached%20daemon-survives%20%2Fmcp%20reconnect-brightgreen.svg">
</p>

---

## TL;DR — 一条命令完成初始化

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp

# 跨平台初始化脚本：构建 HTTP service，生成 .mcp.json，
# 并运行 300 个单元测试。
bash scripts/setup.sh
# 或：  powershell -ExecutionPolicy Bypass -File scripts\setup.ps1

# 然后：
claude   # 从仓库根目录启动 Claude Code
```

进入 Claude Code 会话后，运行 `/mcp`——你应该会看到 `team-mode` 已连接。调用 `team_create` 时 Web UI 会自动打开。

> **⚠ 唯一的关键坑：** 任何对 `.mcp.json` 或 `.claude/settings.json` 的修改都需要**完整重启 Claude Code**（关闭所有 CC 窗口并重新启动）。`/mcp reconnect` **不会**重新加载 hook 配置。如果在配置变更后 worker 回复不再以 `<system-reminder>` 的形式到达，几乎百分之百是这个原因。详见 [§ 故障排查](#troubleshooting--worker-replies-arent-pushing)。

---

## 为何存在

Claude Code 暴露了 MCP 工具调用，但根据设计，它不会自动响应 MCP `resources/updated` 通知。因此，一个把 worker 回复放入"resource"的朴素 MCP server 无法向 lead 的终端推送任何内容。官方的 [`Channels` API](https://code.claude.com/docs/en/channels) 可以解决这个问题，但需要 claude.ai OAuth 登录；而很多人使用 API key 来运行 Claude Code。

本项目使用官方文档记载的 **Stop hook + `asyncRewake`** 模式，实现了一套在 API key 认证下可用的 service → client → session 推送机制：

```
Worker 回复
    ↓
team_mode_service 将一条 JSON 行追加到 .agent-teams/<team>/lead_pending.jsonl
    ↓
Claude Code 的 Stop hook（项目级，.claude/settings.json）在 CC turn 结束时触发
    ↓
scripts/hooks/lead-pending-async-wake.js 向 service 查询当前 CC 的 team
   并原子化 drain 对应的 per-team pending 文件
    ↓
命中时：向 stderr 写入回复内容并退出 2
    ↓
CC 进入一个新 turn，回复内容以 <system-reminder> 的形式注入
    ↓
Claude 读取该 reminder 并继续工作
```

无需轮询，无需消耗 token，无需特殊登录。端到端中位延迟：约 50 ms。

> **历史说明：** 早期版本使用长时间同步 Stop-hook shepherd loop 和 stdio MCP relay。ADR-020 已将默认控制面切到 durable localhost Streamable HTTP service；ADR-022 已将 worker 回复唤醒改为 `asyncRewake` + per-team pending 文件。旧的 stdio `team_mode_mcp` + `team_mode_daemon` 路径仅作为 legacy rollback / fallback 保留。

---

## 功能概览

- **精简的 MCP 接口（8 个工具）** — `team_create / team_list / team_delete / worker_add / worker_list / worker_remove / send_message / inbox_read`。就这些。
- **Durable HTTP service 架构** — `team_mode_service` 是运行在 `127.0.0.1:8786/mcp` 的长期本地 Streamable HTTP MCP service。Claude Code 通过 `.mcp.json` + `scripts/mcp-http-headers.js` 连接；service 持有 worker 子进程和 Web UI。旧的 stdio `team_mode_mcp` + `team_mode_daemon` 组合仅作为 legacy rollback / fallback 路径保留。
- **真正推送到 lead 终端** — Stop hook `asyncRewake` + per-team pending 文件路由，将 worker 回复自动以 `<system-reminder>` 的形式呈现。空闲的 CC 会话会在下一个 turn 边界到来时被唤醒。
- **`127.0.0.1:8787+` 上的实时 Web UI** — 三栏布局（团队列表 / 群组聊天 / 会话详情）。按发送者着色、`@mention` 高亮、点击过滤、完整的 Claude Code 和 Codex JSONL 会话记录，以及一个固定的输入框，允许人类用户以对等身份（包括 lead）向团队发送消息。在 `team_create` 时自动打开。
- **多后端 worker** — `claude-code`、`codex`、`gemini-cli`。lead 必须使用 Claude Code（见 [Codex 作为 Lead](#codex-as-lead)）。
- **严格的 `@mention` 路由** — `send_message` 会预先拒绝无法匹配的 handle，并返回当前活跃 worker 列表，方便调用方自我纠正。匹配不区分大小写（`@Alice` 可以找到 worker `alice`）。
- **严格的 slug 验证** — worker / 团队名称须匹配 `[a-z0-9_.-]{1,64}`（必须以小写字母或数字开头）。无法被 `@mention` 的名称在创建时即被拒绝，而不是留到后面出问题。
- **Worker 存活检测与复活** — `worker_remove` 是软删除（进程停止，配置保留，可快速复用）。`worker_add` 加上 `on_existing=reuse` 可复活一个 worker。通过 OS 进程检查来检测已死亡的 worker；响应中包含 `hint` 字段，精确告知下一步该做什么。
- **每次分发的终端消息保证** — 每次 inbox 分发都会向 lead 产生且只产生一条终端消息。worker 静默完成 turn → `[SYSTEM] worker 'X' completed its turn without producing any reply text`；管道在 turn 中途关闭 → `[SYSTEM] ... output channel closed mid-turn`。lead 永远不需要通过轮询来判断 worker 是否真的完成了。
- **即时运行时提示** — 操作指引通过工具响应的 `hint` / `note` / `dead_recipients_hint` 字段按需下发，而不是埋在静态工具描述里。工具描述保持简短（每个约 700 字符），不挤占上下文。
- **`team_delete` 的失败可见性** — 返回 `shutdown_failures` 数组，让调用方知道哪些子进程可能成了孤儿。
- **Stop hook 批量合并窗口** — 近乎同时到达的多个 worker 回复会被合并为一条 reminder（默认 500 ms 窗口，通过 `TEAM_MODE_STOP_BATCH_GRACE_MS` 配置）。
- **每个项目最多一个活跃团队** — `team_create` 会拒绝在另一个团队的 `owner_cc_pid` 仍存活时创建第二个团队；来自已死亡 CC 会话的孤儿团队会自动清理，并在 `cleaned_orphan_teams` 中汇报。
- **自描述数据目录** — 每次 daemon 启动时，会在 `.agent-teams/` 内自动重新生成一份描述目录结构的 `README.md`。
- **300 个单元测试，零警告。**

---

## 架构

```
┌────────────────────────────────────────────────────────────┐
│  Claude Code (your CLI session) — the LEAD                 │
│                                                            │
│  .mcp.json          ──► http://127.0.0.1:8786/mcp        ││
│  .claude/           ──► Stop hook = asyncRewake script    ││
│   settings.json                                           ││
└───────────────────────────────────────────────────────────┘│
                                                            │ Streamable HTTP MCP
┌─────────────────────────────────────────────────────────────┐
│  team_mode_service(.exe) (THIS REPO — durable localhost)   │
│   ┌────────────────────────────────────────────────────┐   │
│   │  MCP runtime — 8 tools                             │   │
│   ├────────────────────────────────────────────────────┤   │
│   │  Services                                          │   │
│   │   TeamService   MemberService   RoomService        │   │
│   │   MessageService  →  LeadPendingWriter             │   │
│   │   InboxService   (computed from messages.jsonl)    │   │
│   ├────────────────────────────────────────────────────┤   │
│   │  RuntimeOrchestrator — owns worker subprocesses    │   │
│   │   ClaudeCodeBackend   CodexBackend   GeminiBackend │   │
│   ├────────────────────────────────────────────────────┤   │
│   │  Storage (.agent-teams/)                           │   │
│   │   <team>/  team.json members.json(v=1)             │   │
│   │           room.json messages.jsonl                 │   │
│   │   runtime/http-mcp.json runtime/workers.json       │   │
│   │   .locks/ README.md (auto-generated)               │   │
│   ├────────────────────────────────────────────────────┤   │
│   │  team_mode_web — read-only web UI on :8787+        │   │
│   │   served from inside the daemon, auto-opens        │   │
│   └────────────────────────────────────────────────────┘   │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼ spawned as child processes
┌─────────────────────────────────────────────────────────────┐
│  Workers — each is a managed CLI subprocess                │
│   alice (claude-code)    bob (codex)    carol (gemini-cli) │
└─────────────────────────────────────────────────────────────┘
```

**为什么现在是 service？** ADR-020 退役了旧默认 stdio relay，因为 stdin EOF 与 ESC 行为在 Windows 上会让 MCP 生命周期不可靠。durable HTTP service 跨 Claude Code reconnect 存活并持有 worker 状态。需要退出时显式运行 `scripts/team-mode-service.ps1 stop`。旧的 stdio `team_mode_mcp` + `team_mode_daemon` 实现仅作为 legacy rollback / fallback 路径保留。

**数据流**：
- Lead → worker：`send_message` 写入 `messages.jsonl`。worker 的 `AgentLoop` 通过 `InboxNotifier` 被唤醒，并将消息注入 worker 的 stdin。
- Worker → lead：worker 的回复通过 `MessageService::send`（`Kind::Reply`）进入系统，`LeadPendingWriter` 将其追加到 `.agent-teams/<team>/lead_pending.jsonl`。`asyncRewake` Stop hook drain 对应 per-team 文件，并用 `<system-reminder>` 唤醒 Claude Code。

---

## MCP 工具参考

| 工具 | 必填参数 | 可选参数 | 说明 |
|---|---|---|---|
| `team_create` | `name` | `cwd` | 创建团队；虚拟的 `lead` 成员自动加入。自动清理来自已死亡 CC 的孤儿团队，并在 `cleaned_orphan_teams` 中汇报。自动为该团队打开 Web UI。 |
| `team_list` | — | — | 列出所有团队。每个团队附有 `ownerStatus`：`alive` / `orphan` / `unbound`。 |
| `team_delete` | `name` | — | 关闭所有 worker 并删除团队目录。对未能干净退出的 worker 返回 `shutdown_failures: [{member, reason}]`。 |
| `worker_add` | `team`, `name` | `adapter`, `model`, `cwd`, `system_prompt`, `env`, `on_existing` | 启动一个 worker。**当配置已存在时必须指定 `on_existing`**：`reuse`（快速复用已保存的配置）/ `overwrite`（替换，需要 `adapter`）/ `error`（默认，直接报错）。复活已死亡的 worker 时，返回 `revived_from_dead: true`。 |
| `worker_list` | `team` | — | 列出 worker（不含 lead）。已死亡的 worker 标记为 `sessionState: "dead"`，并附带 `hint` 提示你用 `worker_add on_existing=reuse` 复活。 |
| `worker_remove` | `team`, `name` | — | 软删除：进程停止，状态置为 `Removed`，执行配置**保留**，以便后续快速复用。 |
| `send_message` | `team`, `text` | — | 以 lead 身份发送消息。`text` 必须包含 `@handles`，且所有 handle 必须匹配活跃 worker；未匹配的 handle 会导致调用失败并返回活跃 worker 列表。混合活跃/死亡收件人列表时，返回 `dead_recipients_hint` 并向 lead 的 inbox 发送 `[SYSTEM]` 通知。 |
| `inbox_read` | `team` | `limit`, `unread_only`, `auto_ack` | lead 的 inbox 拉取模式备用方案。**这不是主要通道**——回复通过 Stop hook 自动到达；`inbox_read` 仅用于历史消息审查。 |

完整 schema 见 [`.plans/agent-teams-v2/docs/02-current-system/mcp-tools-reference.md`](.plans/agent-teams-v2/docs/02-current-system/mcp-tools-reference.md)。

---

## Web UI

service 在 `127.0.0.1:8787` 上运行一个内嵌的只读 Web 服务器（端口冲突时自动递增至 8799）。调用 `team_create` 时自动在默认浏览器中打开（通过 `TEAM_MODE_WEB_AUTO_OPEN=0` 禁用）。

**布局**：三栏——左侧（团队 / 成员 / 过滤列表），中间（群组聊天时间线），右侧（会话 / 详情 / 诊断标签页）。

**群组聊天**：气泡风格。发送者头像 + 名称 + 时间 + 正文。`@mention` 标记高亮显示，可点击过滤时间线。`[SYSTEM]` 状态消息渲染为居中的灰色提示。

**发送者颜色**：`lead` 固定为青色，`user` 固定为暖橙色，worker 获得基于 djb2 哈希的稳定颜色（跳过保留色相范围，确保 worker 颜色不与 lead/user 冲突）。

**会话记录**（右侧栏）：展示焦点成员的实际 Claude Code 或 Codex JSONL 会话内容，组织为"工作 turn"——工具调用与结果配对显示，最终回复高亮。每个 worker 的 `session_id` 从其后端流中捕获（Claude Code 的 `init` / `result` 事件；Codex 的 `thread.id`），用于精确的会话查找。Codex 的 rollout 文件（位于 `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`）以原生方式解析（5 秒 TTL 缓存）。

**人在回路消息**：底部固定的输入框允许你作为人类用户，通过 `@mention` 向任意团队房间发送消息——发送者名称为保留的 `user` handle。worker 回复你的方式与回复 lead 完全一致。lead 也能看到这些消息（通过 lead 可见性规则）。

设计理由与功能路线图见 [`.plans/agent-teams-v2/docs/04-web-ui/team-mode-web-guide.md`](.plans/agent-teams-v2/docs/04-web-ui/team-mode-web-guide.md) 和 [`.plans/agent-teams-v2/docs/04-web-ui/history/web-frontend-plan.md`](.plans/agent-teams-v2/docs/04-web-ui/history/web-frontend-plan.md)。

---

## 后端能力矩阵

| 能力 | `claude-code` | `codex` | `gemini-cli` |
|---|---|---|---|
| 持久化进程 | ✓（NDJSON stream-json）| ✓（`codex app-server` JSON-RPC）| —（每 turn 重新启动）|
| `session_id` 捕获 | ✓ | ✓（thread.id）| — |
| Web UI 会话记录 | ✓ | ✓（rollout JSONL）| —（仅 mtime 兜底）|
| 全权限模式 | ✓（`--permission-mode bypassPermissions`）| ✓（`sandbox_mode = "danger-full-access"`）| n/a |
| 系统提示机制 | `--system-prompt` 参数 | 前置到第一条用户消息 | 每条构造提示前加 `System:` 前缀 |
| 跨 turn 的对话记忆 | 原生（单进程）| 原生（单进程）| 内存滚动窗口（最近 50 turn）|

注意事项：
- **Claude Code worker** 在 Windows 上需要 `CLAUDE_CODE_GIT_BASH_PATH`。MCP relay 会在启动时从常见 Git 安装路径自动检测；如果你的安装路径非标准，请手动设置该环境变量。
- **Codex worker** 以 `approvalPolicy: "never"` 和 `sandbox_mode: "danger-full-access"` 启动，避免因等待权限提示而阻塞。推理精力（reasoning effort）字段有意不硬编码；如果你在 `~/.codex/config.toml` 中有设置，会走兜底路径读取。
- **Gemini worker** 没有持久化会话，因此 Web UI 无法展示其 JSONL 会话记录。每个 turn 的对话历史会从 `messages.jsonl` 在内存中重建。

---

## 安装

### 推荐方式：使用 setup 脚本

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp

bash scripts/setup.sh
# 或：  powershell -ExecutionPolicy Bypass -File scripts\setup.ps1
```

setup 脚本做了以下事情：
1. 验证前置条件（cargo 1.85+，node 14+）。
2. 构建 release 二进制文件：`team_mode_service(.exe)`。
3. **从 `.mcp.json.template` 生成 `.mcp.json`**，指向 `http://127.0.0.1:8786/mcp` 和 `scripts/mcp-http-headers.js`。
4. 运行 `cargo test --lib`（300 个测试）。
5. 打印后续步骤。

> **为何需要生成 `.mcp.json`？** `.mcp.json` 是本机配置且已加入 gitignore。版本跟踪的 `.mcp.json.template` 指向本地 HTTP MCP endpoint，并通过 `scripts/mcp-http-headers.js` 注入 runtime token 与 owner headers。移动仓库后重新运行 setup，并完整重启 Claude Code。

### 手动安装

```bash
cargo build --release --bin team_mode_service
```

然后复制 `.mcp.json.template` 为 `.mcp.json`，启动 service，并完整重启 Claude Code：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\team-mode-service.ps1 start
```

`.mcp.json` 应包含 HTTP endpoint：

```json
{
  "mcpServers": {
    "team-mode": {
      "type": "http",
      "url": "http://127.0.0.1:8786/mcp",
      "headersHelper": "node scripts/mcp-http-headers.js"
    }
  }
}
```

stdio `team_mode_mcp` + `team_mode_daemon` 安装路径仅是 legacy rollback / fallback 路径，不应作为默认安装方式。

### Worker cargo 命令在 Windows MSVC 上的前置

Codex worker 是 `team_mode_service` 的子进程。在 Windows MSVC target 下，`rustc` 可能无法从这个子进程中探测到 Visual Studio，进而误调用 Git Bash 的 `link.exe`，表现为 `link.exe was not found` 或来自错误 `link.exe` 的链接失败。

修复方式是在启动 service 前先 source `vcvars64.bat`，让 service 及其 worker 继承 `LIB`、`INCLUDE` 和 MSVC `PATH`。

使用项目脚本：

```powershell
.\scripts\team-mode-service.ps1 start
```

或手动执行：

```cmd
"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cargo run --release --bin team_mode_service
```

推荐的 Codex worker 配置：

```toml
[shell_environment_policy]
inherit = "all"

sandbox_mode = "danger-full-access"
approval_policy = "never"
```

非 Windows 用户可直接运行 `cargo run --release --bin team_mode_service`，或用 release binary 加 `--data-dir .agent-teams --project-root .` 后台启动。

### 推送通知——已内置

`.claude/settings.json` 已提交并包含 Stop hook 配置。**首次克隆后必须重启 Claude Code**，以便它加载 hook 配置（CC 只在启动时加载 hook）。之后，每次修改 `.mcp.json` 或 `.claude/settings.json` 都需要完整重启 CC。

### 验证清单

1. `bash scripts/setup.sh`（或 PowerShell 版本）——成功。
2. `target/release/team_mode_service(.exe)` 存在。
3. `scripts/team-mode-service.ps1 start` 输出 `running pid=... url=http://127.0.0.1:8786/mcp`。
4. 从仓库根目录启动 `claude` → `/mcp` 显示 `team-mode` 已连接。
5. `team_create({"name":"smoke"})` 成功；Web UI 自动打开。
6. `worker_add({"team":"smoke","name":"alice","adapter":"claude-code"})` 成功。
7. `team_delete({"name":"smoke"})` 成功。
8. 阅读 [`.plans/agent-teams-v2/docs/03-operations/usage-tips.md`](.plans/agent-teams-v2/docs/03-operations/usage-tips.md)，了解注意事项。

如果任何步骤失败，见 [`.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md`](.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md)——里面有完整的排查表。

---

## 故障排查 — Worker 回复没有推送过来

这是最常见的问题。症状：`send_message` 返回成功，但下一个 turn 没有收到 `<system-reminder>`。请按以下顺序逐步排查：

1. **首次克隆后 / 编辑 `.claude/settings.json` 后，你重启 CC 了吗？** Hook 只在 CC 启动时加载——`/mcp reconnect` **不会**重新加载。退出所有 CC 窗口，重新启动 `claude`，然后重试。
2. **Worker 真的在回复吗？** `tail -f .agent-teams/team-mode-service.log`——你应该能看到回复被 append 给 `lead`。如果没有，说明 worker 卡住了（检查后端 CLI，例如 `codex` 是否已安装并在 PATH 中）。
3. **Stop hook 在触发吗？** `tail -f .agent-teams/.lead-pending-wake.log`——你应该看到 async-wake 注入日志。完全没有条目 → hook 未加载 → 见步骤 1。service 查询失败 → 运行 `scripts/team-mode-service.ps1 status`。
4. **仍然没有效果？** 见 [`.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md`](.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md)，其中有完整的排查表（15+ 个场景及修复方案）。

`send_message` 工具响应的 `hint` 字段对此有意做了详细说明——如果你在工具返回结果中看到 "If reminders never arrive ..."，说明你已经遇到这个问题了，应该重启 CC。

---

## 数据目录结构

由 service 在首次工具调用时，在 lead 的 cwd 下创建：

```
.agent-teams/
├── README.md                  ← 每次 service 启动时自动重新生成
├── .locks/                    ← 文件锁（按团队 + lead_pending）
├── runtime/
│   ├── http-mcp.json          ← {pid, url, token_file, base_dir, project_root}
│   └── workers.json           ← worker 运行时附属文件（service 重启时孤儿标记为 dead）
├── team-mode-service.log      ← service stderr/tracing
└── <team-name>/
    ├── team.json              ← 团队元数据（含 owner_cc_pid）
    ├── members.json           ← v=1，统一身份 + 执行配置
    ├── room.json              ← 房间元数据
    ├── messages.jsonl         ← 只追加的消息历史（数据真相来源）
    └── lead_pending.jsonl     ← per-team 推送队列，由 hook 原子化 drain

# Hook 侧 scratch 文件：
.agent-teams/.lead-pending-wake.log
.agent-teams/.cc-identity.<session_id>.json
```

旧的项目根 `lead_pending.jsonl` 会在 service 启动时迁移到 per-team 文件。

遗留的 `.team-mode-data/` 目录会触发启动警告（不会自动迁移——请手动删除）。

---

## 开发

```bash
# 编译检查（快速，不链接）
cargo check --lib

# 运行 300 个单元测试（约 1 秒）
cargo test --lib

# 构建默认 HTTP MCP service
cargo build --release --bin team_mode_service

# 可选的 web 二进制文件（默认已内置到 service；独立构建供调试用）
cargo build --release --features team-mode-web --bin team_mode_web
```

相关设计文档：
- [`.plans/agent-teams-v2/decisions.md`](.plans/agent-teams-v2/decisions.md) — ADR-020/021/022 当前 HTTP service 与 async wake 决策
- [`.plans/agent-teams-v2/docs/05-design-history/legacy/team-mode-mcp-final.md`](.plans/agent-teams-v2/docs/05-design-history/legacy/team-mode-mcp-final.md) — legacy rollback / fallback stdio MCP 运行时 + 工具接口 + 存储布局
- [`.plans/agent-teams-v2/docs/02-current-system/worker-detach-refactor.md`](.plans/agent-teams-v2/docs/02-current-system/worker-detach-refactor.md) — legacy rollback / fallback daemon 架构设计理由
- [`.plans/agent-teams-v2/docs/05-design-history/hook-push-design.md`](.plans/agent-teams-v2/docs/05-design-history/hook-push-design.md) — Stop hook + JSON block 设计
- [`.plans/agent-teams-v2/docs/05-design-history/design-decisions.md`](.plans/agent-teams-v2/docs/05-design-history/design-decisions.md) — 完整的 bug 记录 + 备选方案讨论
- [`.plans/refactor-data-layout/spec.md`](.plans/refactor-data-layout/spec.md) — 当前数据布局规范

要添加新后端？参见 `src/backend/{claude_code,codex,gemini}.rs` 中 `Backend` trait 的参考实现。`AgentLoop` 以统一方式驱动所有后端。代码规范见 [`CONTRIBUTING.md`](CONTRIBUTING.md)。

---

## Codex 作为 Lead

简短版本：**当前不支持。** 基于 Stop hook 的推送是 Claude Code 的专属特性——Codex CLI 没有等效的阻塞式 hook（[openai/codex#8375](https://github.com/openai/codex/issues/8375)）。

支持 Codex 作为 lead 的唯一官方路径是 `codex app-server` JSON-RPC 模式，这需要围绕它构建一个适配层（约 2000+ 行 Rust 代码）。欢迎在 issue tracker 中讨论或研究。

Codex 作为 **worker** 则完全支持，在 Web UI 中与 Claude Code 拥有完整的会话记录对等性。

---

## 致谢

本项目**衍生自并构建于** [`github.com/ZhangHanDong/agent-teams-rs`](https://github.com/ZhangHanDong/agent-teams-rs)（MIT，© 2025 Zhang Han Dong），该项目提供了核心运行时、后端实现、team/task/inbox 领域模型和 CLI。本 fork 将项目重心转移到 `team_mode_service` HTTP MCP service，并新增了：

- Stop hook + JSON block + 祖先链路由推送架构
- 可跨 Claude Code reconnect 存活的 durable localhost `team_mode_service`
- `127.0.0.1:8787` 上的实时 Web UI，支持按发送者着色、完整会话记录（Claude Code + Codex）以及人在回路消息
- 统一成员文件布局（`members.json` v=1，身份与执行配置合并）
- 含自动生成 `README.md` 的按团队子目录数据布局
- `worker_add on_existing`、严格的 `send_message`、`team_delete shutdown_failures`、`worker_add` 就绪检查
- 每次分发的终端消息保证（静默 turn / 管道关闭 → `[SYSTEM]`）
- 严格的 slug 验证、大小写不敏感的 `@mention`、即时运行时提示
- Service observability watchdog、asyncRewake Stop hook 批量合并、单活跃团队强制约束
- `inbox_read` 拉取模式工具
- Hook 脚本、安装自动化与终端用户文档

---

## License

MIT — 见 [`LICENSE`](LICENSE)。
