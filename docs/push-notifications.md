# Lead 主动推送（Stop Hook Shepherd Loop）

Worker 在 team-mode MCP 中回复 lead 时，消息由 Rust 侧写入队列文件，通过
Claude Code 的 **Stop hook shepherd loop** 模式推送到 lead CC 会话 ——
lead 无需自己调 `inbox_read`，消息会自动出现在下一个 turn。

本文档描述当前架构（2026-04-23 最终版）。历史设计文档 `hook-push-design.md`
仅作对照参考。

---

## 流程图

```
worker reply
   └─ Rust: message_service.send()
      ├─ 写 <team>/messages.jsonl (lead inbox 持久化)
      └─ LeadPendingWriter.maybe_write()
         → append 一行到 <cwd>/lead_pending.jsonl
         每行含：team / from / msg_id / text / ts / owner_cc_pid

CC turn 结束
   └─ Stop hook 触发 scripts/hooks/lead-pending-wake.js
      ├─ TEAM_MODE_WORKER=1 ? → exit 0 （worker 子进程 fast-path）
      ├─ stop_hook_active=true ? → exit 0 （官方防环）
      ├─ session_id 在 cooldown ? → exit 0 （Windows stdin bug 兜底）
      ├─ 读 pending；走 ancestor chain 找出自己的 CC 祖先 PID
      ├─ 只消费 owner_cc_pid ∈ 祖先集 的行；其他 CC 的行写回 pending
      ├─ 有消息 → stdout 写 JSON {decision:"block", reason:"…"} + exit 0
      └─ 无消息 → 每 500ms 轮询，最多 TEAM_MODE_STOP_WAIT_SEC (默认 1800s)
         ├─ 有新消息 → block via JSON
         ├─ SIGINT (用户 ESC) → exit 0 让出 prompt
         └─ 超时 → exit 0

CC 处理 JSON block
   → 注入 reason 字段作为 <system-reminder>
   → 触发新 turn
   → lead AI 看到消息
```

---

## 推送内容格式

**单条消息**：
```
[TEAM-MODE] 收到新消息 — alice (team: diag) 回复:

<完整正文>
```

**多条消息**（按到达顺序）：
```
[TEAM-MODE] 收到 3 条新消息：

alice (team: diag) 回复:
<正文>

---

bob (team: diag) 回复:
<正文>

---

charlie (team: other) 派发消息:
<正文>
```

kind 翻译：`reply→回复` / `dispatch→派发消息` / `discussion→讨论`

---

## 为什么选 `exit 0 + JSON block` 而不是 `exit 2 + stderr`

两种方式都能 block CC、触发新 turn、注入内容。区别：

| 方式 | UI 显示 | 说明 |
|---|---|---|
| `exit 2 + stderr` | `Stop hook error: [node ...]: <内容>` | CC 固定加前缀（[Issue #34600](https://github.com/anthropics/claude-code/issues/34600)），"error" 措辞干扰 AI 理解 |
| `exit 0 + stdout JSON {"decision":"block","reason":"..."}` | 干净的 reason 内容 | 无前缀、无包装、AI 直接读到消息 |

`exit 0 + JSON` 是官方 ralph-loop / claude-mem 同款推荐模式。

---

## 配置（零配置开箱即用）

仓库自带 `.claude/settings.json`：

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "node \"$CLAUDE_PROJECT_DIR/scripts/hooks/lead-pending-wake.js\""
          }
        ]
      }
    ]
  }
}
```

**原则**（见 `docs/design-decisions.md`）：
- 配在项目级，**绝不放全局**
- 无 `async` / `asyncRewake` 字段（Stop hook 就是同步阻塞）
- 无 `matcher`（Stop hook 没有这个字段）

---

## 环境变量

| 变量 | 默认 | 作用 |
|---|---|---|
| `TEAM_MODE_STOP_WAIT_SEC` | 1800 (30 min) | Stop hook 最多等待秒数。worker 长任务可设 7200 (2h) 或更长 |
| `TEAM_MODE_WORKER` | —（Rust 自动设） | Rust `claude_code.rs::spawn_child` 给每个 worker 子进程设 `1`；hook 看到立即 exit 0 不阻塞 worker |
| `TEAM_MODE_BASE_DIR` | 从 stdin event 推断 | 显式数据目录，一般不用 |
| `TEAM_MODE_LOG_FILE` | `<data_dir>/mcp.log` | MCP tracing 日志路径 |

---

## 多 CC 同项目路由（Ancestor Chain）

每个 CC 启动自己的 MCP 子进程；每个 CC 也会独立加载 project-level Stop hook。
要防止 CC_A 吃掉 CC_B 的消息：

- **Rust 侧**：`team_create` 时通过 `sysinfo` 查自己 MCP 进程的 `parent_id` = CC 真 PID，存到 `Team.owner_cc_pid`；`LeadPendingWriter` 写 pending 时把这个 PID 附到每行
- **Hook 脚本**：启动时 snapshot 全部进程树（Windows 用 PowerShell `Get-CimInstance Win32_Process`，Unix 用 `ps -eo pid=,ppid=`），从 `process.ppid` 沿 parent 链走到 PID=0 或 40 层深度，构建 `ancestorSet`
- **过滤**：`entry.owner_cc_pid ∈ ancestorSet` 的消息属于本 CC → 消费；否则保留给其他 CC 的 hook

**性能**：Windows 上 PowerShell 查询 ~1-2s，缓存到 `.ancestor-cache.json` TTL 5s 避免 Stop hook 轮询时反复触发。

**降级**：进程树查询失败（沙箱、权限）→ 消费所有消息（牺牲多 CC 去重，保证不丢消息）。

---

## 用户交互

- **lead 正常工作**：和无 hook 时一样
- **turn 结束后**：CC UI 显示类似 `Running stop hooks... 1/2 Xs`（短暂阻塞等消息）
- **用户按 ESC**：hook 收 SIGINT → exit 0 让出 prompt
- **新消息到达**：最多 500ms 内 hook 检测到 → JSON block → CC 进新 turn

---

## 防环双保险

| 机制 | 防什么 |
|---|---|
| **`stop_hook_active: true` 检测**（stdin 官方字段） | CC 因上次 Stop hook block 进入新 turn 后，再次结束 turn 时 stdin 标记该字段。hook 看到立即 exit 0 |
| **session_id cooldown 文件** (`.stop-hook-cooldown`, TTL 10s) | Windows stdin delivery bug（[Issue #46601](https://github.com/anthropics/claude-code/issues/46601)）兜底。即使 stdin 读不到，同 session_id 10s 内重复触发也 exit 0 |

---

## 故障排查

读 `.lead-pending-wake.log`：

| 日志片段 | 含义 |
|---|---|
| `stop: stop_hook_active=true, exit 0 (official loop-break)` | 正常防环 |
| `stop: cooldown active for session ..., exit 0` | Windows 兜底防环 |
| `stop: waiting up to Ns [ancestors=A,B,C,...] session=S` | 等待中；`ancestors=` 是当前 CC 的祖先 PID 集前 5 个 |
| `stop: injected N, kept M for peers [ancestors=...], exit 0 (block via JSON)` | 成功注入；N 条是本 CC 的，M 条保留给其他 CC |
| `stop: interrupted by signal, exit 0` | 用户按 ESC 打断 |
| `stop: wait timed out after Ns, exit 0` | 默认 30 分钟超时放手 |

读 `.agent-teams/mcp.log`：

| 日志片段 | 含义 |
|---|---|
| `Spawning Claude Code CLI agent ... agent=X` | worker spawn |
| `agent loop ready, polling inbox member=X` | agent_loop 启动 |
| `processing inbox message member=X ... sender=lead` | worker 接收消息 |
| `posting reply to room member=X reply_len=N` | worker 产出 reply |
| `message sent sender=X room=main kind=Reply recipients=["lead"]` | reply 写入 pending |

---

## 参考

- CC Hooks 官方文档：https://code.claude.com/docs/en/hooks
- 设计决策完整记录：[docs/design-decisions.md](./design-decisions.md)
- Bug 修复历史：[docs/design-decisions.md#bug-修复历史](./design-decisions.md#bug-修复历史)
- 开源部署注意事项：[docs/open-source-deployment.md](./open-source-deployment.md)
