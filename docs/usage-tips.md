# 使用经验与最佳实践 / Usage Tips

这些是项目作者在数十轮真实使用中沉淀下来的"踩坑指南"。不读 spec 也能跑，但读了能少走很多弯路。

如果你是 **第一次部署** —— 请先读 [`open-source-deployment.md`](open-source-deployment.md)。
如果你想 **理解架构** —— 请读 [`team-mode-mcp-final.md`](team-mode-mcp-final.md) 和 [`hook-push-design.md`](hook-push-design.md)。
本文档专注 **怎么用得顺、避免常见坑**。

---

## 1. 配置该放哪：项目级 vs 全局

**铁律：team-mode 的 hook 和 MCP 配置只放项目级（`<repo>/.claude/settings.json`、`<repo>/.mcp.json`），永远不要放到 `~/.claude/settings.json` 全局。**

为什么：
- 全局 Stop hook 会在 **所有** Claude Code 会话里触发，包括你跟 team-mode 完全无关的项目。team-mode 的 Stop hook 设计成同步阻塞最多 30 分钟（默认 7200 秒），放全局会拖死其他项目的 CC。
- 项目级配置随仓库 clone 自动生效，开源用户拿到就能用，无需手改全局文件。
- 多项目隔离：每个 team-mode 项目都有自己的 `.agent-teams/` 和 `lead_pending.jsonl`，不会互相串台。

如果你确实有"在多个项目都跑 team-mode"的需求 —— 把每个项目当独立的 clone 用，不要走全局共享。

---

## 2. 修改 `.mcp.json` 后必须重启 Claude Code

Claude Code **只在启动时读一次 `.mcp.json`**。中途改了文件 → `/mcp reconnect` 仍然走的是旧路径（实测会是缓存里那条命令，即便文件已经更新）。

正确流程：
1. 改 `.mcp.json`
2. 完全退出 Claude Code（关闭所有窗口）
3. 重启 `claude` 命令
4. `/mcp` 检查是否连上

如果你只是 **重新 build 了二进制**（路径没变），`/mcp reconnect` 即可重新拉起 MCP 子进程，不必重启 CC。

---

## 3. 工具返回里的 `hint` 字段就是当下最重要的提示

team-mode 的 8 个 MCP 工具刻意把"操作建议/警告"放在 **响应** 里而不是工具描述里。具体来说：

| 工具 | 何时返回 hint | 内容 |
|---|---|---|
| `send_message` | 始终 | "不要 poll，回复会自动 push 到下一轮" |
| `send_message` | 含 dead worker 时 | `dead_recipients_hint` + 自动写入 `[SYSTEM]` 提示 |
| `inbox_read` | 收件箱为空时 | "Stop hook 才是规范通道，inbox_read 只是 fallback" |
| `worker_list` | 含 dead worker | "用 `worker_add on_existing=reuse` 复活" |
| `team_list` | 含 orphan team | "owner_cc_pid 已死，建议 `team_delete` 清理" |
| `worker_add` | 创建 worker 时 | "等 ready 状态，session_id 在第一轮交互后才落地" |

→ 看到 `hint` / `note` / `xxx_hint` 就读它。这些都是 **基于当前状态** 才会出现的，比静态文档更准确。

---

## 4. 别 poll，等就行

**Worker 回复会自动 push 到 lead 的下一轮。** 你不需要 `inbox_read`、不需要 `/mcp`、不需要任何刷新动作。

具体机制（懒得读完整架构的话）：
- Worker 回复 → Rust 写 `lead_pending.jsonl` → CC 的 Stop hook（项目级 settings.json 里那个）拦截 → exit 0 + JSON block → CC 重新进入一个新 turn，把内容当作 `<system-reminder>` 注入

如果你按 ESC 中断了 Stop hook（比如等 worker 回复太久），下一轮 CC 会继续等，但你也可以手动 `inbox_read({"team":"...","auto_ack":true})` 主动拉。

---

## 5. Worker 命名 = 严格 slug

```
^[a-z0-9_.-]{1,64}$
```

不允许大写、空格、中文、特殊字符。原因：worker 名要能用 `@mention` 在消息正文里被引用。

`@mention` 本身是 **大小写不敏感** 的（`@Bob` 和 `@bob` 都能命中 worker `bob`），但创建时 **必须用小写 slug**。

---

## 6. 一个项目同时只有一个 live team

`team_create` 会拒绝创建第二个 team（如果已有 team 的 `owner_cc_pid` 还活着）。

但 **死 team 会自动清理** —— 上次 CC 关掉后留下的孤儿 team 会在你下次 `team_create` 时被自动 prune，并在响应里以 `cleaned_orphan_teams` 字段告诉你。

如果你想强制重置，手动 `team_delete({"name":"..."})` 即可。

---

## 7. 不要在工具调用里"硬编码覆盖"用户的全局配置

如果你 **二次开发** team-mode（比如加新 backend），有一个原则要遵守：

> **三层优先级：`worker_add` 显式参数 > 用户全局 config 文件（如 `~/.codex/config.toml`）> backend CLI 自身默认。绝不插入"项目硬编码默认值"作为第四层。**

具体做法：
- backend 字段做成 `Option<T>`，未传不写到 CLI flag → 让下游 CLI 走它自己的 config
- 想让用户在工具层覆盖全局 → 在 `worker_add` schema 里暴露可选参数
- 不要在代码里写 `config.model = Some("gpt-5".into())` 这种静默默认值

适用：`model`、`effort`、`cwd`、`env`、`approval_policy`、`sandbox` 等所有可被用户全局配置的字段。

---

## 8. 看日志：四个文件，按顺序排查

| 文件 | 看什么 |
|---|---|
| `.lead-pending-wake.log`（项目根） | hook 是否被触发 / 是否成功 inject / 是否走了 cooldown |
| `.agent-teams/mcp.log` | MCP 是否收到 tool call、是否 spawn worker、是否 `posting reply` |
| `.agent-teams/daemon.log` | daemon 收到的 tool dispatch、worker lifecycle |
| `.agent-teams/runtime/workers.json` | worker runtime 状态 sidecar |

**典型问题判定**：
- "lead 收不到回复" → 先看 `.lead-pending-wake.log` 有没有 `injected N`。没有 → CC 没加载 hook（重启）。有 → 已成功，下一 turn 会到。
- "worker 不回复" → 看 `mcp.log` 有没有 `posting reply` 行。没有 → worker 子进程可能挂了。有但 lead 收不到 → 看 `lead-pending-wake.log`。
- "worker 显示 running 但发消息没反应" → 看 `runtime/workers.json` 是不是 stale；用 `worker_list` 触发活性检查。

---

## 9. `/mcp` 显示连接但工具调用报错（CC 客户端 bug）

**症状**：CC 的 `/mcp` 面板显示 `team-mode: connected`，但你下次调用任何工具就报错。`/mcp reconnect` 一下立刻就好。

**根因**：CC 在某些时机会单方面关闭 MCP relay 的 stdin（最常见触发：ESC 中断、长时间无 tool 调用、CC 自身的某些重置）。MCP 一收到 stdin EOF 就 exit（这是合规行为）。但 **CC 的 `/mcp` 面板显示是 lazy 的，不会主动 ping MCP 进程**，所以 UI 上还是 stale 的"connected"。下次你调用工具，CC 想往已关闭的 stdio 写入 → 报错。

这是 **CC 客户端的 bug**（MCP server 侧无法修复，因为 stdio 一断就该 exit）。我们能做的就是早识别、快恢复：

**确认是不是这个问题**：
```bash
tail -1 .agent-teams/mcp.log
# 看到这行就是命中了：
# WARN MCP: stdin EOF — parent closed the pipe, exiting run_stdio
```

**应对**：
- **`/mcp reconnect` 即可**，不用重启 CC（detached daemon 让 worker / 消息历史 / session_id 全部存活，只是 stdio 桥要重建）
- daemon 进程仍在跑（可以从 `tasklist | grep team_mode_daemon` 验证），所以 web UI、worker 子进程、消息推送给其他 CC 都不受影响
- reconnect 后第一次 tool call 会触发 MCP 重新连 daemon → 拿到完整的 in-flight 状态

**预防**：
- 少按 ESC（尤其是在 worker 还在工作的时候）
- 长时间无操作前可以先做一次 `team_list` 之类的轻量调用，让 stdio 保持活跃
- 多窗口同时 CC 的话，最容易触发（CC 的 stdio 管理在多 session 下有竞态）

未来如果 Anthropic 修了 `/mcp` UI 的状态检测，这个坑就会消失。但现状下，请按上面流程处理。

---

## 10. 跨平台几个老坑

**Windows**：
- 路径包含中文是 OK 的（PathBuf + UTF-8 已测）
- Stop hook stdin 在 Win 有 [CC #46601](https://github.com/anthropics/claude-code/issues/46601) bug，hook 脚本有兜底（`.stop-hook-cooldown` session_id）
- Git-Bash 路径会自动探测，不在常见位置就手动 export `CLAUDE_CODE_GIT_BASH_PATH`

**macOS / Linux**：
- 启动 CC 前 `export EXE_EXT=""` —— 否则 `.mcp.json` 里的 `${EXE_EXT:-.exe}` 会展开成 `.exe` 找不到二进制
- `node` 必须在 PATH（hook 脚本是 node 写的）

---

## 11. ESC 永远是出路

任何时候 Stop hook 卡住 → **按 ESC**。hook 脚本注册了 SIGINT handler，会立即 `exit 0` 让出 prompt。

如果你想从源头改默认等待时长：

```bash
export TEAM_MODE_STOP_WAIT_SEC=300    # 改 5 分钟（默认 7200 秒 = 2 小时）
```

然后重启 CC。

---

## 12. 关于 daemon 与 MCP relay 的关系

新架构（2026-04 之后）下：
- `team_mode_mcp.exe` = **薄 relay**，CC 启动时 spawn，挂了不影响 worker
- `team_mode_daemon.exe` = **常驻 daemon**，跨 `/mcp reconnect` 存活，持有所有 worker 进程
- ESC 关闭 MCP 的 stdin 不会 drop worker session

要回退到旧的"MCP 内执行"模式（调试用）：
```bash
export TEAM_MODE_DAEMON=0
```

daemon 自动 self-kill：当 15 秒内所有 team 都没有 live owner_cc_pid 时，daemon 退出，worker 跟着死。

---

## 13. 工具调用前先做 `team_list`，看清状态

习惯性养成：每次 session 开始或 `/mcp reconnect` 之后，先 `team_list`，看：
- 有没有 orphan team（owner CC 死了）
- 有没有需要 cleanup 的残留

这一条是经验之谈 —— team-mode 的状态都在 `.agent-teams/` 里持久化，CC 退出再启动会发现"上一轮的世界还在"。先看清楚，再决定 reuse 还是 delete。

---

## 14. 想新增 backend / 改架构？读这两份 spec

- [`docs/team-mode-mcp-final.md`](team-mode-mcp-final.md) —— MCP 工具集、消息路由、storage 布局
- [`.plans/refactor-data-layout/spec.md`](../.plans/refactor-data-layout/spec.md) —— 当前 `<base>/<team>/` 子目录布局的 design spec

新增 backend 见 `src/backend/` 目录的现有实现（`claude_code.rs` / `codex.rs` / `gemini_cli.rs`）。每个 backend 实现 `Backend` trait，AgentLoop 做 driver。

---

**最后**：发现新坑 / 改进建议 → 提 issue 或 PR 进这份文档。
