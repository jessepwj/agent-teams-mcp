<p align="right">
  <a href="README.md">English →</a>
</p>

<p align="center">
  <em>让 Claude Code 协调一支 AI worker 团队的 MCP 服务器 —— 支持 worker 真·主动推送给 lead。</em>
</p>

<p align="center">
  <a href="https://github.com/jessepwj/agent-teams-mcp/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.85%2B-orange.svg">
  <img alt="MCP" src="https://img.shields.io/badge/protocol-MCP-purple.svg">
  <img alt="Tests" src="https://img.shields.io/badge/tests-300%20passing-brightgreen.svg">
</p>

# agent-teams-mcp

**`agent-teams-mcp`** 是一个用 Rust 写的 [Model Context Protocol](https://modelcontextprotocol.io) 服务器，把你的 Claude Code CLI 变成 **team lead** —— 它可以 spawn 并协调多个 worker agent（Claude Code、Codex、Gemini CLI），把它们当作子进程管理，并在它们之间路由 `@mention` 风格的消息。

最大的卖点：**worker 回复能真·推送回 lead 的终端。** 当 worker 写完一段回复，Claude Code Lead 会自动被唤醒（即使空闲挂起也能被唤），并在下一个 turn 处理这条消息 —— 不用 poll、不用手动 `inbox_read`、不用切窗口。

<p align="center">
  <img alt="Flow" src="https://img.shields.io/badge/worker--reply%20%E2%86%92%20lead-~50ms-brightgreen.svg">
  <img alt="Auth" src="https://img.shields.io/badge/API%20key%20auth-supported-brightgreen.svg">
</p>

---

## TL;DR —— 全新 clone 一行命令

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp

# 一键 bootstrap（跨平台）：
bash scripts/setup.sh
# 或者 Windows PowerShell：
#   powershell -ExecutionPolicy Bypass -File scripts\setup.ps1

# 然后：
claude   # 在仓库根目录启动 Claude Code
```

setup 脚本会校验前置条件（cargo、node）、build 两个 release 二进制（`team_mode_mcp` + `team_mode_daemon`）、跑 300 个单元测试、并打印下一步指引。改完代码任何时候都能重跑。

仓库已经自带 `.mcp.json`（用 `${CLAUDE_PROJECT_DIR}` 的项目相对路径）和 `.claude/settings.json`（用于真·推送的 Stop hook） —— 两者都会在启动 CC 时自动加载，无需手动接线。

进入 Claude Code 会话后，用 `/mcp` 检查 —— 应该能看到 `team-mode` 已连接。如有问题请看 [`docs/open-source-deployment.md`](docs/open-source-deployment.md)。

> **⚠ 一个关键坑：** 任何对 `.mcp.json` 或 `.claude/settings.json` 的修改都需要 **完全重启 Claude Code**（关掉所有 CC 窗口再重启）。`/mcp reconnect` **不会** 重新加载 hook 配置。如果你改完配置后 worker 回复不再以 `<system-reminder>` 形式出现，几乎一定是这个原因。详见下面的 [疑难排查节](#疑难排查--worker-回复推不到)。

随后在 Claude Code 会话里：

```
> 帮我创建一个叫 "demo" 的 team，加一个 alice worker。

Claude: [calls team_create(name="demo")]
        [calls worker_add(team="demo", name="alice", adapter="claude-code")]

> @alice 分析下 ./logs/*.log 里的日志

Claude: [calls send_message(team="demo", text="@alice 分析下 ...")]

[~5 秒后 —— 你什么都没做]

<system-reminder>
  alice (reply): 我分析了 342 行日志，关键错误是 ...
</system-reminder>

Claude: Alice 找到了关键错误，我来看看……
```

最后那段 `<system-reminder>` 是 **自动出现的** —— 你不用打字、不用 `/mcp`、不用 `inbox_read`。Worker 一回完，消息就到你 session 里了。

---

## 这东西为什么存在？

Claude Code 暴露了 MCP tool calls，但它 **设计上不会自动响应** MCP 的 `resources/updated` 通知 —— 所以一个朴素地把 worker 回复塞进 "resource" 的 MCP server 完全推不动 lead。官方的 [`Channels` API](https://code.claude.com/docs/en/channels) 能解决这问题，但要求 claude.ai OAuth 登录；很多人是用 API key 跑 Claude Code。

本项目用 Claude Code 官方文档里的 **Stop hook + `exit 0` + JSON `decision:"block"`** 实现了 server → client → session 的推送链路，在 API key 模式下也能工作：

```
Worker 回复
    ↓
Rust MCP server 把一行 append 到 <base>/lead_pending.jsonl
    ↓
Claude Code 的 Stop hook（项目级 settings.json 里那个）拦截
    ↓
scripts/hooks/lead-pending-wake.js 检测到 pending → exit 0 + JSON block
    ↓
Claude Code 进入新 turn，把 pending 内容当作 <system-reminder> 注入
    ↓
Claude 在新 turn 处理这条 reminder
```

不 poll、不烧 token、不用特殊登录方式。

> **历史**：早期版本用的是 `FileChanged + asyncRewake` 组合，后来改成 Stop hook 主链路，原因详见 [`docs/hook-push-design.md`](docs/hook-push-design.md)。

---

## Features

- **极简的 8 个 MCP 工具** —— `team_create / team_list / team_delete / worker_add / worker_list / worker_remove / send_message / inbox_read`，就这些。
- **统一的 member 模型** —— 身份 + 执行配置合一；`worker_remove` 是软删除（保留 profile 便于快速复用），`worker_add` 配 `on_existing=reuse` 一键拉回来。
- **多后端 worker** —— 每个 worker 在 `claude-code` / `codex` / `gemini-cli` 三选一。Lead 必须是 Claude Code（参见 [Codex as Lead](#codex-as-lead)）。
- **真·推送给 Claude Code lead** —— 通过官方文档的 Stop hook + JSON block 链路；空闲会话也能被唤醒。
- **Pull 后备方案** —— `inbox_read` 工具任何时候都可用，给没配 hook 的客户端做 fallback。
- **严格的 `@mention` 路由** —— `send_message` 在前置阶段就拒绝任何匹配不上的 `@handle`，并把当前活跃 worker 列表返回给调用者，让它自我修正。
- **Spawn 时 ready check** —— `worker_add` 阻塞到 spawn 出来的 agent 发出第一条 `TurnComplete` 或 5 秒超时，明确返回 `starting` / `running` / `failed`。
- **`team_delete` 暴露失败** —— 返回 `shutdown_failures` 数组让调用者知道哪些进程可能成了孤儿。
- **数据目录自描述** —— 每次启动会在数据目录里自动重写一份 `README.md` 描述当前布局。
- **300 个单元测试，零 warnings。**

---

## Architecture

```
┌────────────────────────────────────────────────────────┐
│  Lead Agent  ( 你的 Claude Code CLI 会话 )             │
│                                                        │
│  .mcp.json     ─────► spawn team_mode_mcp via stdio ──┐│
│  .claude/      ─────► Stop hook = lead-pending-wake.js│
│  settings.json                                         │
└───────────────────────────────────────────────────────┘│
                                                        │
┌──────────────────────────────────────────────────────▼─┐
│  team_mode_mcp（薄 relay，本仓库）                      │
│  └── 通过 IPC 转发所有 tool call 到 ↓                   │
│                                                         │
│  team_mode_daemon（常驻，持有所有 worker）              │
│   ┌─────────────────────────────────────────────────┐   │
│   │  MCP runtime — 8 工具 + 5 资源 URI              │   │
│   ├─────────────────────────────────────────────────┤   │
│   │  Services                                        │   │
│   │   TeamService  MemberService  RoomService       │   │
│   │   MessageService（↓ lead_pending writer）       │   │
│   │   InboxService（从 messages 计算）              │   │
│   ├─────────────────────────────────────────────────┤   │
│   │  Storage                                         │   │
│   │   <base>/<team>/  team.json                      │   │
│   │                   members.json (v=1)             │   │
│   │                   room.json                      │   │
│   │                   messages.jsonl                 │   │
│   │   <base>/lead_pending.jsonl（跨 team 推送队列） │   │
│   │   <base>/.locks/（文件锁）                      │   │
│   ├─────────────────────────────────────────────────┤   │
│   │  RuntimeOrchestrator —— spawn 各 backend         │   │
│   │   ClaudeCodeBackend  CodexBackend  GeminiBackend │   │
│   └─────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
              │
              ▼ spawn 为子进程
┌──────────────────────────────────────────────────────┐
│  Workers —— 每个都是被托管的 CLI 子进程              │
│                                                      │
│  alice (claude-code)    bob (codex)    …             │
└──────────────────────────────────────────────────────┘
```

**数据流要点**：

- Worker ← lead：`send_message` 写 `messages.jsonl`；每个 worker 的 `AgentLoop` 在 `InboxNotifier::notify` 上唤醒，把消息注入 worker 的 stdin。
- Worker → lead：worker 的回复走 `MessageService::send` 标 `Kind::Reply`；`LeadPendingWriter` 把它 append 到 `<base>/lead_pending.jsonl`；Claude Code 的 Stop hook 拦截后用 JSON block 把 lead 拉进新 turn，把内容当作 `<system-reminder>` 注入。

---

## MCP 工具速查

| 工具 | 必填 | 可选 | 简介 |
|---|---|---|---|
| `team_create` | `name` | `cwd` | 创建 team；自动加一个虚拟 `lead` 成员。 |
| `team_list` | — | — | 列出所有 team。 |
| `team_delete` | `name` | — | 关掉所有 worker + 删除 team 目录；返回 `shutdown_failures` 标记孤儿进程。 |
| `worker_add` | `team`, `name` | `adapter`, `model`, `cwd`, `system_prompt`, `env`, `on_existing` | Spawn 一个 worker。已存在 profile 时 **必须** 传 `on_existing`：`reuse` / `overwrite` / `error`。 |
| `worker_list` | `team` | — | 列出活跃 worker（不含 lead）。 |
| `worker_remove` | `team`, `name` | — | 软删除：进程停掉、状态置 Removed、execution profile 保留以便快速复用。 |
| `send_message` | `team`, `text` | — | 以 lead 身份发送。`text` **必须** 含 `@handles`，且全部要匹配活跃 worker —— 任意一个不匹配整条调用就失败并返回当前 worker 列表。 |
| `inbox_read` | `team` | `limit`, `unread_only`, `auto_ack` | Pull 模式后备方案，读 lead 的收件箱。 |

完整 schema 见 [`docs/mcp-tools-reference.md`](docs/mcp-tools-reference.md)。

---

## 安装

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp
cargo build --release --bin team_mode_mcp --bin team_mode_daemon
```

**两个二进制都要编** —— `team_mode_mcp` 是 Claude Code spawn 出来的薄 stdio relay，`team_mode_daemon` 是常驻进程，持有所有 worker 子进程，跨 `/mcp reconnect` 存活。

### 零配置接线（推荐）

仓库根目录已自带可用的 `.mcp.json` 和 `.claude/settings.json`，两者都用 `${CLAUDE_PROJECT_DIR}` 指向刚 build 出来的 release 二进制。**直接在仓库根目录启动 Claude Code**，MCP server 和 Stop hook 都会在第一个 turn 自动加载。

仅 macOS / Linux 用户需要：启动 CC 前 `export EXE_EXT=""`，否则 `.mcp.json` 里的 `${EXE_EXT:-.exe}` 会展开成 `.exe` 找不到二进制。

### 自定义路径 / 装到 PATH

如果想把二进制装到 `PATH` 而不是项目相对路径：

```bash
cp target/release/team_mode_mcp target/release/team_mode_daemon /usr/local/bin/
```

然后改 `.mcp.json` 用 `"command": "team_mode_mcp"`（不写路径）。设 `TEAM_MODE_DAEMON_EXE=/usr/local/bin/team_mode_daemon` 让 relay 知道去哪 spawn daemon。

### 推送已经接好

`.claude/settings.json` 已经包含了把 worker 回复变成 `<system-reminder>` 注入下一个 CC turn 的 Stop hook。**第一次 clone 后必须重启 Claude Code** 一次让它加载 hook 配置（CC 只在启动时读 hook）。之后每次改 `.mcp.json` / `.claude/settings.json` 都需要完全重启 CC，**不是** `/mcp reconnect`。

设计动机（为什么用 Stop hook + JSON block + ancestor routing，而不是更早的 FileChanged + asyncRewake）见 [`docs/push-notifications.md`](docs/push-notifications.md) 和 [`docs/hook-push-design.md`](docs/hook-push-design.md)。

### 第一次 clone 之后的自检清单

1. `bash scripts/setup.sh`（或 Windows 上 `powershell scripts\setup.ps1`）成功
2. `target/release/team_mode_mcp(.exe)` 和 `team_mode_daemon(.exe)` 存在
3. 在仓库根目录 `claude` → `/mcp` 显示 `team-mode` 已连接
4. 试一下 `team_create({"name":"smoke"})` 然后 `team_delete({"name":"smoke"})` —— 都成功
5. 读一下 [`docs/usage-tips.md`](docs/usage-tips.md)（"该做什么、不该做什么"）

如果第 3 步失败，看 [`docs/open-source-deployment.md`](docs/open-source-deployment.md) —— 里面有完整的 `/mcp` 连接错误排查表。

---

## 疑难排查 — worker 回复推不到

这是 OSS 用户最常踩的坑。症状：调用 `send_message` 工具返回成功，但下一个 turn 没有任何 `<system-reminder>` 出现。按以下顺序排查：

1. **第一次 clone 之后 / 改完 `.claude/settings.json` 之后，重启 CC 了吗？** Hook **只在** CC 启动时加载 —— `/mcp reconnect` 不会拉新 hook。退出所有 CC 窗口、重新 `claude` 启动、再试。
2. **Worker 真的回复了吗？** `tail -f .agent-teams/mcp.log` —— 应该能看到 `posting reply ... kind=Reply recipients=["lead"]`。
   - 没有 → worker 卡住了（检查它的 backend，比如 `codex` 是否在 PATH）。
   - 有 → 继续往下。
3. **Stop hook 触发了吗？** `tail -f .lead-pending-wake.log` —— 应该能看到 `stop: injected N ...` 行。
   - 完全没条目 → hook 没加载 → 回到第 1 步。
   - `cooldown active` 或 `stop_hook_active=true` → 是正常的 loop guard，等下一 turn。
   - `injected 0, ancestors=[...]` → 这条消息属于另一个 CC（多 CC 场景）；`rm lead_pending.jsonl` 清掉残留。
4. **以上都做了还是不行？** 看 [`docs/open-source-deployment.md`](docs/open-source-deployment.md) —— 里面有 15+ 种场景的完整排查表。

`send_message` 的响应 `hint` 字段会主动提醒这件事 —— 如果你在工具结果里看到"If reminders never arrive ..."这段话，说明你已经踩到了，按上面 4 步排查即可。

---

## 数据目录布局

MCP server 启动时会在 lead 的 CWD 下创建 `.agent-teams/`：

```
.agent-teams/
├── README.md               ← 每次启动自动重新生成
├── lead_pending.jsonl      ← 推送队列，由 Stop hook 消费
├── .locks/                 ← 文件锁
└── <team-name>/
    ├── team.json
    ├── members.json        ← 带版本号；身份 + execution profile
    ├── room.json
    └── messages.jsonl
```

如果检测到旧的 `.team-mode-data/` 目录会打 warning（**不会** 自动迁移 —— 自己手动删）。

---

## 开发

```bash
# 编译检查（快，不 link）
cargo check --lib

# 跑 300 个单元测试
cargo test --lib

# Build MCP 二进制
cargo build --bin team_mode_mcp
```

当前数据布局的 design spec 在 [`.plans/refactor-data-layout/spec.md`](.plans/refactor-data-layout/spec.md) —— 加新 store / service 时建议读一下。

---

## Codex 作为 Lead

短答：**目前不支持。** Stop hook + JSON block 这套是 Claude Code 特有功能，Codex CLI 没有等价 hook，OpenAI 也明确表态不会加（[openai/codex#8375](https://github.com/openai/codex/issues/8375)）。

唯一官方支持的 Codex-as-Lead 路径是 `codex app-server` JSON-RPC 模式 —— 但要在它外面写一层 harness（约 2000+ 行 Rust）。研究 / 讨论欢迎在 issue tracker。

Codex 作为 **worker** 完全支持 —— 需要 hook 的是 lead，不是 worker。

---

## Credits

本项目 **fork 自** [`github.com/ZhangHanDong/agent-teams-rs`](https://github.com/ZhangHanDong/agent-teams-rs)（MIT, © 2025 Zhang Han Dong），上游提供了核心 runtime、各 backend、team/task/inbox domain 和 CLI。本 fork 把项目重新聚焦到 `team_mode_mcp` MCP server 上，并新增了：

- Stop hook + JSON block + ancestor routing 推送架构（取代了早期的 FileChanged + asyncRewake）
- 检测到 worker 死掉的 lead 可观察性 + 自动 [SYSTEM] 通知
- 统一的 member 文件布局（`members.json` 合并身份 + execution）
- 每个 team 自己一个子目录的数据布局 + 自动生成的 `README.md`
- `worker_add on_existing`、严格的 `send_message`、`team_delete` 失败上报、`worker_add` ready check
- `inbox_read` pull 模式工具
- 常驻 daemon 架构（`team_mode_daemon`），跨 `/mcp reconnect` 存活
- Hook 脚本 + 面向用户的文档

---

## License

MIT —— 见 [`LICENSE`](LICENSE)。
