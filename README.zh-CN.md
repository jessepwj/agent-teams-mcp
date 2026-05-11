<p align="right">
  <a href="README.md">English →</a>
</p>

<p align="center">
  <em>一个用 Rust 写的 MCP server，把 Claude Code 变成一支 AI worker 团队的协调者——worker 回复实时推送给 lead，自带 Web UI 实时观察整支团队。</em>
</p>

<p align="center">
  <a href="https://github.com/jessepwj/agent-teams-mcp/blob/main/LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.85%2B-orange.svg">
  <img alt="MCP" src="https://img.shields.io/badge/protocol-MCP-purple.svg">
  <img alt="Tests" src="https://img.shields.io/badge/tests-351%20passing-brightgreen.svg">
  <img alt="Push" src="https://img.shields.io/badge/worker--reply%20%E2%86%92%20lead-%7E50ms-brightgreen.svg">
</p>

# agent-teams-mcp

一个 [MCP](https://modelcontextprotocol.io) server，让你的 Claude Code session **领导**一支 AI worker 团队（Claude Code、Codex、Gemini CLI）。Worker 回复**自动**作为 `<system-reminder>` 注入到 lead 的下一个 turn——不用轮询、不用 `inbox_read`、不用手动检查。Web UI 跑在 `http://127.0.0.1:8787`，实时看整支团队对话。

## 安装

前提：Rust 1.85+、Node.js 14+。

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp
cargo install --path .
team_mode_service install-global
```

然后 **完全重启 Claude Code**（关闭所有窗口、重开）。`/mcp` 应该显示 `team-mode connected`。之后 `cd` 进任意项目都能用，每个项目自动隔离自己的团队数据。

> **唯一要记的坑**：改了 `.mcp.json` 或 `.claude/settings.json` 后必须完全重启 CC。`/mcp reconnect` 不会重新加载 hooks。见 [故障排查](#故障排查)。

## 为什么有这个项目

Claude Code 暴露了 MCP tool 调用，但**不**会自动响应 MCP `resources/updated` 通知。官方的 [Channels API](https://code.claude.com/docs/en/channels) 能解决但需要 claude.ai OAuth——很多人用的是 API key。

本项目利用 Claude Code 文档化的 **Stop hook + `asyncRewake`** 机制，把 worker 回复一路推回 lead session：

```
Worker 回复
  → service 追加到 .agent-teams/<team>/lead_pending.jsonl
  → CC turn 结束时 Stop hook 触发
  → hook 原子排空 per-team 文件,写 stderr,exit 2
  → CC 开新 turn,把回复作为 <system-reminder> 注入
```

端到端延迟中位数 ~50 ms，不轮询、不烧 token。

## 架构

```
┌───────────────────────────────────────────────────┐
│  Claude Code (你的 session) — LEAD                │
│  .mcp.json  ──► HTTP MCP at 127.0.0.1:8786/mcp   │
│  .claude/   ──► Stop hook (asyncRewake)          │
└──────────────┬────────────────────────────────────┘
               │ Streamable HTTP
┌──────────────▼────────────────────────────────────┐
│  team_mode_service (常驻 localhost daemon)        │
│   • 8 个 MCP tool                                 │
│   • TeamService / MessageService / InboxService   │
│   • Worker 子进程编排器                           │
│   • Web UI 在 :8787 (自动打开)                    │
│   • 数据存 .agent-teams/<team>/                   │
└──────────────┬────────────────────────────────────┘
               │ spawn
   ┌───────────┼───────────┬──────────────┐
   ▼           ▼           ▼              ▼
 alice       bob        carol         lead-pending
(claude)   (codex)    (gemini)      (per-team 队列)
```

Service 跨 Claude Code 重连不死,所有 worker 状态归它管。要彻底关掉跑 `scripts/team-mode-service.ps1 stop`。

## 功能清单

- **8 个 MCP tool** —— `team_create / team_list / team_delete / worker_add / worker_list / worker_remove / send_message / inbox_read`。就这些。
- **推送到 lead 终端** —— 回复自动作为 `<system-reminder>` 到达,无轮询。
- **实时 Web UI** (`127.0.0.1:8787+`) —— 三栏布局、按发送者着色、`@mention` 高亮、完整 Claude Code / Codex JSONL session 转录、底部 sticky 输入框允许人类介入。
- **多 backend worker** —— `claude-code` / `codex` / `gemini-cli`。Lead 必须是 Claude Code(详见 [Codex 当 Lead](#codex-当-lead))。
- **严格路由** —— `@mention` 校验、sender 由 caller 身份决定、worker 只能往绑定的 team 发,不可伪造。
- **每项目隔离** —— 一个常驻 service 可同时服务多个项目的 CC session,team / worker / message 互不串味。
- **Worker 复活** —— `worker_remove` 是软删(profile 保留)。`worker_add on_existing=reuse` 快速恢复。
- **Mid-turn 投递** —— lead 在执行工具时,回复通过 `PostToolUse` hook 在 ~3s 内浮现,无需等 turn 结束。
- **351 个单测,零警告。**

## MCP tool

| Tool | 必填 | 可选 | 作用 |
|---|---|---|---|
| `team_create` | `name` | `cwd` | 建团,lead member 自动加入。自动开 Web UI。 |
| `team_list` | — | — | 列所有 team,带 `ownerStatus`。 |
| `team_delete` | `name` | — | 停所有 worker + 删团。返回 `shutdown_failures`。 |
| `worker_add` | `team`, `name` | `adapter`, `model`, `cwd`, `system_prompt`, `env`, `on_existing` | 启 worker。如果 profile 已存在 `on_existing` 必填。 |
| `worker_list` | `team` | — | 列 worker,死掉的会被标记 + hint 怎么复活。 |
| `worker_remove` | `team`, `name` | — | 软删,profile 保留。 |
| `send_message` | `team`, `text` | — | 向 team 发消息。`sender` 由 caller 决定。`@mention` 必填。 |
| `inbox_read` | `team` | `limit`, `unread_only`, `auto_ack` | 拉模式 fallback,仅审计用,不是主通道。 |

完整 schema 见 [`.plans/agent-teams-v2/docs/02-current-system/mcp-tools-reference.md`](.plans/agent-teams-v2/docs/02-current-system/mcp-tools-reference.md)。

## Backend 能力对比

| | `claude-code` | `codex` | `gemini-cli` |
|---|---|---|---|
| 常驻进程 | ✓ | ✓ | — (按 turn 重启) |
| `session_id` 捕获 | ✓ | ✓ (thread.id) | — |
| Web UI 转录 | ✓ | ✓ | — |
| Full-access 模式 | ✓ (`bypassPermissions`) | ✓ (`danger-full-access`) | n/a |
| 多轮记忆 | 原生 | 原生 | 滚动 50 轮窗口 |

Windows 上 Claude Code worker 需要 `CLAUDE_CODE_GIT_BASH_PATH`(从常见 Git 路径自动检测,非常规位置请手工设)。Windows MSVC 用户启动 service 前 source `vcvars64.bat` 让 worker 继承链接器,最简单的办法是用 `scripts/team-mode-service.ps1 start`。

## 故障排查

`send_message` 返回成功但 `<system-reminder>` 没到。按顺序排查:

1. **装好后/改完 `.claude/settings.json` 后,完全重启过 CC 吗?** Hook 只在 CC 启动时加载。`/mcp reconnect` 不够。关所有 CC 窗口,重开。
2. **Worker 真的回复了吗?** `tail -f ~/.team-mode/runtime/service.log` 应该看到追加到 `lead_pending`。没有 → backend CLI 可能不在 PATH 里。
3. **Stop hook 触发了吗?** `tail -f .agent-teams/.lead-pending-wake.log` 应该有 injection 行。完全无 → hook 没加载 → 回 step 1。
4. **还是不行?** 见 [`.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md`](.plans/agent-teams-v2/docs/03-operations/open-source-deployment.md),完整排查表(15+ 个场景)。

## Codex 当 Lead

暂不支持。Stop hook 推送是 Claude Code 独有的特性——Codex CLI 没有等价的阻塞 hook([openai/codex#8375](https://github.com/openai/codex/issues/8375))。

Codex 当 **worker** 完全支持,Web UI 转录跟 Claude Code 同等。

## 开发

```bash
git clone https://github.com/jessepwj/agent-teams-mcp
cd agent-teams-mcp
bash scripts/setup.sh    # 或: powershell -ExecutionPolicy Bypass -File scripts\setup.ps1
cargo test --lib         # 351 个测试,~1s
cargo build --release --bin team_mode_service
```

加新 backend 看 `src/backend/{claude_code,codex,gemini}.rs` 的实现。`AgentLoop` 统一驱动所有 backend。约定见 [`CONTRIBUTING.md`](CONTRIBUTING.md)。

设计文档:
- [`.plans/agent-teams-v2/decisions.md`](.plans/agent-teams-v2/decisions.md) —— 当前 HTTP service + async wake 的 ADR
- [`.plans/agent-teams-v2/docs/05-design-history/hook-push-design.md`](.plans/agent-teams-v2/docs/05-design-history/hook-push-design.md) —— Stop hook 设计取舍

## 致谢

本项目派生自 [`github.com/ZhangHanDong/agent-teams-rs`](https://github.com/ZhangHanDong/agent-teams-rs)(MIT, © 2025 Zhang Han Dong),原项目提供核心 runtime、backend、领域模型。本 fork 在其基础上加了:

- Stop hook + `asyncRewake` 推送架构
- 常驻 localhost HTTP service(跨 CC 重连不死)
- 实时 Web UI(按发送者着色、session 转录、人类介入)
- per-team 数据布局、统一 `members.json` v=1、caller 身份的 sender
- 严格的 `send_message` 路由、worker 复活、mid-turn 投递、每项目隔离

## 许可

MIT —— 见 [`LICENSE`](LICENSE)。
