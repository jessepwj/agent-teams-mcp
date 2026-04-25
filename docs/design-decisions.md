# 架构决策 & Bug 修复历史

记录关键设计选型的背景和权衡，以及开发过程中修复的非平凡 bug。
为未来贡献者和开源用户理解"为什么是这样"提供上下文。

---

## 关键设计决策

### 为什么选 Stop hook shepherd loop？

**替代方案对比**：

| 方案 | 优点 | 放弃原因 |
|---|---|---|
| FileChanged + asyncRewake | 实现最简单 | **CC busy 态不唤醒**（实测）。Anthropic 官方 hooks issue (#17601, #50947) 证实 `<system-reminder>` 注入只发生在 turn 边界，asyncRewake 在 CC 处理 turn 中间会被丢弃。对 worker 长任务场景不可靠。 |
| Channels（v2.1.80+） | 官方设计的"mid-turn 推送" | **仅支持官方账号 OAuth 登录**。第三方 relay（`ANTHROPIC_BASE_URL` + `cr_` token）不兼容。[官方原文](https://code.claude.com/docs/en/channels-reference)："require claude.ai login. Console and API key authentication is not supported." |
| PostToolUse + additionalContext | 能在 busy 态注入 | 每次工具调用都阻塞 ~50-150ms；仅覆盖"正在调工具"的 busy 态，纯推理时漏；和其他插件的 PostToolUse hook 共存需管理优先级。 |
| MCP tool response payload 捎带 | 零 hook 配置 | 仅覆盖"lead 正在调 MCP 工具"场景，Bash/Read/Edit 这类系统工具期间不触发。 |
| **Stop hook shepherd loop** ✓ | turn 结束时阻塞等待、支持 ESC 打断、防环机制成熟 | 需要防环设计（`stop_hook_active` + session cooldown）。用户 turn 结束后 CC 短暂"等 hook"，体感稍有延迟但可接受。 |

**最终方案**：Stop hook 唯一路径，`inbox_read` MCP 工具作为手动兜底。

---

### 为什么 `lead_pending.jsonl` 必须在项目根？

Claude Code 的 `FileChanged` hook `matcher` 只接受**字面文件名**（`|` 分隔列表），
不支持路径 glob 或子目录监视。实测验证：写到 `.agent-teams/lead_pending.jsonl`
不触发 FileChanged；写到项目根触发。

虽然我们最终弃用了 FileChanged，但保留这个位置（对未来可能恢复的 FileChanged
兼容 + 直观位置）。Rust 侧 `LeadPendingWriter` 用 `std::env::current_dir()`
定位（= CC 启动时的项目根），而不是 base_dir 的 `.agent-teams/`。

---

### 为什么 Stop hook 用 `exit 0 + JSON block` 而不是 `exit 2 + stderr`

**共同点**：两种方式都让 CC 进新 turn + 注入内容。

**差异**：
- `exit 2 + stderr` 会被 CC UI 前缀包装成 `Stop hook error: [node "..."]: <content>`。
  "error" 字样会误导 lead AI（可能以为 hook 出错，去处理错误而不是 worker reply）；
  用户也困惑。参考 [CC Issue #34600](https://github.com/anthropics/claude-code/issues/34600)，
  官方标记 closed as not planned，不会修。
- `exit 0 + stdout JSON {"decision":"block","reason":"..."}` 纯净输出 reason 字段，
  无任何包装。官方 ralph-loop / claude-mem 同款模式。

**当前方案**：hook 脚本构造 JSON `{decision: "block", reason: renderReminder(...)}`
写 stdout，然后 `exit 0`。

---

### Reminder 排版设计

**原则**：
- 去掉重复指令（"inspect and respond"、"call inbox_read"），lead AI 自己会处理
- 成员 + 团队自然拼接 `alice (team: diag)`，不用机器味的 `[team=diag] alice`
- kind 翻译成自然语言（`reply→回复`, `dispatch→派发消息`, `discussion→讨论`）
- **单条**：紧凑内联 header + 正文段
- **多条**：统计开头 `N 条新消息`；每条独立块，`---` 分隔；多行正文保持原排版不挤一行

见 `scripts/hooks/lead-pending-wake.js::renderReminder()`。

---

### 进程绑定：用 ancestor chain 而不是直接 ppid

**历史**：最初版本用 `process.ppid` 做路由。**失败**：CC 不直接 spawn hook
脚本，中间有一个 dispatcher 进程（shell/node wrapper），`process.ppid` 指向
该 dispatcher，而 Rust MCP 是 CC 直接 spawn 的（parent_id() == CC PID），
两个 PID 永远对不上 —— 所有消息都被归为"他人的"不消费。

**当前方案**：hook 脚本启动时 snapshot 整个进程树（Windows PowerShell
`Get-CimInstance Win32_Process`，Unix `ps -eo pid=,ppid=`），从 `process.ppid`
沿 parent 链往上走到 PID=0 或深度 40 层，得到 ancestor PID set。CC PID
一定在这条链里（CC 是 hook 脚本的祖先），所以 `entry.owner_cc_pid ∈ ancestorSet`
等价于"该消息属于本 CC"。

**性能**：Windows 的 PowerShell 冷启动 ~1-2s。所以 `<baseDir>/.ancestor-cache.json`
做 5 秒 TTL 缓存，Stop hook 的 500ms 轮询循环只调一次 PowerShell。

**失败降级**：若进程树查询失败（沙箱、权限、PowerShell 不可用），返回 null，
调用方退化为"所有行都消费"。牺牲多 CC 去重，但保证不丢消息。



**需求**：多个 CC 同时在同一项目下运行（测试 AI + 开发者自己的 CC），每个
CC 创建自己的 team；worker 回复应只进入对应 CC 的 turn，不污染其他 CC。

**方案**：进程关系天然就是"绑定"，不需要 CC/MCP 协商：

```
CC (PID=X) ──spawn──▶ MCP 子进程 (ppid=X)         ← team_mode_mcp
          ──spawn──▶ hook 脚本子进程 (ppid=X)     ← lead-pending-wake.js
```

- Rust MCP 用 `sysinfo` crate 查 `current_parent_pid()` = CC 的 PID
- `team_create` 时把这个 PID 存到 `team.json.owner_cc_pid`
- `LeadPendingWriter.maybe_write()` 读 team 的 owner_cc_pid 附到每行 pending
- hook 脚本用 `process.ppid` 过滤只消费属于自己 CC 的行

**没用的替代方案**：
- CC session_id：MCP 启动时拿不到 CC 的 session_id（CC 不通过 stdin/env 传）
- MCP 内部 UUID：只知道自己，不知道 hook 脚本是哪个 CC 触发的

---

### 为什么 `sysinfo` 而非 `std::process::parent_id()`

Rust `std::process::parent_id()` 是 **nightly-only** API。`sysinfo` 跨平台（Linux/macOS/Windows）
且稳定。代价：依赖 ~200KB 编译后的 crate，包含 `system` 特性就够了。

---

### 为什么 hook 放项目级而非全局

**用户明确要求**：hook 只对 team-mode 项目生效，不应在其他 CC 会话里跑。

- 项目级 `.claude/settings.json` 随仓库 check-in → 开源者 clone 即用
- 全局会让所有 CC 会话加载 hook，可能阻塞非 team-mode 项目的 CC 工作流
- `$CLAUDE_PROJECT_DIR` 在项目级 settings 里展开稳定（hooks command 字段）

---

### 为什么 Stop hook 默认等 30 分钟

**取舍**：
- 太短（5-10 分钟）：worker 跑长任务期间如果恰好在 hook 超时后才回复，这次漏
- 太长（无限）：用户按 ESC 打断机制是 best-effort，hook 如果一直不放手可能让
  CC UI "看起来卡住"（Issue #7762：spinner 不显示）
- 30 分钟：覆盖 90% 长任务，超时后新 turn 的 Stop hook 会再等

**用户可调**：`TEAM_MODE_STOP_WAIT_SEC=7200` 即可改成 2 小时或任意时长。

---

### 为什么双重防环（`stop_hook_active` + session cooldown）

**官方机制**：CC 在"因 Stop hook exit 2 触发新 turn 后又到 turn 结束"时，
下一次 Stop hook 的 stdin JSON 含 `stop_hook_active: true`。脚本看到必须 exit 0
不再 block，否则死循环。

**为什么还要 cooldown**：Windows 上 CC 的 Stop hook 有 stdin delivery bug
（[CC Issue #46601](https://github.com/anthropics/claude-code/issues/46601)）。
如果 stdin 读不到 JSON，脚本看不到 `stop_hook_active` → 无法防环 → 死循环。

**cooldown 兜底**：脚本每次 block 把 `{session_id, timestamp}` 写到
`.stop-hook-cooldown`。10 秒内同一 session 再次触发 → exit 0。即使 stdin 完全
丢失，cooldown 也能打破循环。

---

## Bug 修复历史

记录排查过程中发现并修复的非平凡 bug，按时间顺序。

### Bug 1: `worker_list` 永远显示 `not-spawned`

**位置**：`src/team_mode/mcp/tools.rs` `worker_add`

**症状**：worker 明明已 spawn 且能正常工作，但 `worker_list` 显示 `sessionState: "not-spawned"`。

**根因**：`worker_add` 先 `member_store.upsert(record)` 把 `execution.session_state = None`
落盘，**spawn 成功后没回写 `Running` 到磁盘**。`worker_list` 读磁盘，永远看到 None。

**修复**：ready-check 通过后调 `member_store.update()` 把 `execution.session_state`
写成 `Running` 或 `Starting`。

---

### Bug 2: `agent_loop` 初始 drain 永久阻塞

**位置**：`src/runtime/agent_loop.rs::run()` 和 `src/backend/claude_code.rs`

**症状**：alice spawn 成功、agent_loop 起来，但不处理 inbox 消息。

**根因**：`claude_code.rs` spawn 时塞了**一个**合成 `TurnComplete` 到 rx 解锁。`tools.rs`
ready-check 消掉一次。`agent_loop.rs` 初始 drain 又要等一个 —— 没了，永久 `recv().await`
死锁，根本不进 `tokio::select!`。

**修复**：agent_loop 初始 drain 改为 100ms timeout 非阻塞：有 synthetic event 就消，
没有就继续（反正 tools.rs 已经消过）。

---

### Bug 3: `send_input` 报 "Member 'X' not found in team 'runtime'"

**位置**：`src/runtime/agent_loop.rs` `AgentLoop.member_id` 字段歧义使用

**症状**：worker spawn 成功，agent_loop 处理到消息，但立即失败 `Member 'alice' not found`。

**根因**：`AgentLoop.member_id` 字段被同时用作两种语义：
- `inbox_service.peek()` 需要 **worker name**（`"alice"`）
- `orchestrator.send_input()` 需要 **spawn_key**（`"diag__alice"`）

构造 AgentLoop 时 `member_id = worker_name = "alice"`，调 `orch.send_input("alice", ...)` →
orchestrator 的 `sessions` HashMap key 是 `"diag__alice"`，找不到。

**修复**：AgentLoop 加独立字段 `session_key: String` = spawn_key。`send_input` 用
session_key，inbox/ack 用 member_id。

**为什么没早发现**：单元测试和旧 `mcp_e2e.py` 巧合地 `member_id == spawn_key == "worker"`，
掩盖了 bug。只在真实 team/worker 场景（`diag__alice`）才触发。

---

### Bug 4: 无效 adapter 留下脏 member record

**位置**：`src/team_mode/mcp/tools.rs::worker_add`

**症状**：`worker_add adapter="bogus"` 报错 "unknown backend type"，但 `worker_list`
仍看到 "bogus" 的 member 残留。

**根因**：`member_store.upsert(record)` 在 `BackendType::parse()` **之前**执行。
parse 失败时 record 已落盘，没有回滚。

**修复**：把 `BackendType::parse()` 移到 `upsert()` 之前。adapter 无效直接返回 error，
不落盘。

---

### Bug 5: `reuse` 对运行中 worker 不幂等

**位置**：同 worker_add

**症状**：worker 还在运行时调 `worker_add on_existing="reuse"`，报 `"Managed member 'X' is already registered"`。

**根因**：reuse 模式下依然走 `spawn_managed_member`，orchestrator 检测到 sessions HashMap
已有这个 key → 报错。

**修复**：upsert 前先调 `orchestrator.is_alive(spawn_key)`：reuse 模式 + 进程活着 → 跳过
spawn，直接返回当前 session_state。

---

### Bug 6: hook 多实例并发读写 pending 的 race

**位置**：`scripts/hooks/lead-pending-wake.js`

**症状**：多个 CC 触发 hook 时，pending 的"谁消费哪条"不稳定。

**调查过程**：尝试过 atomic rename 方案（`rename → read → unlink`），但这让消息变成
"谁先抢到谁得"，随机丢到"错的 CC"。后回滚到 read + clear。最终用 `owner_cc_pid`
路由解决（上面的 Bug 6 的真正根因是没有路由机制）。

**当前状态**：基于 `owner_cc_pid` 过滤 —— 每个 CC 的 hook 脚本只消费属于自己的行，
别的行写回。不再 race。

---

### Bug 9: ppid 路由从设计上就失效（必须改 ancestor chain）

**位置**：`scripts/hooks/lead-pending-wake.js`

**症状**：pending 文件里有 worker reply，但 hook 反复触发都说 "kept N for peers"
不消费，lead 永远收不到。

**根因**：CC 通过一个中间 dispatcher 进程启动 hook，hook 的 `process.ppid` 指向
dispatcher 而不是 CC。Rust MCP 是 CC 的直接子进程，它拿到的 `parent_id()` 是
CC 真 PID。两者永远不等 → 所有消息被归为 "他人的"。

**现象证据**：hook log 里一堆不同 ppid（每次 hook 实例的 ppid 都不同，因为
每次都是新的 dispatcher subprocess），但 pending 里的 `owner_cc_pid` 是
stable 的（= CC 本体 PID）。

**修复**：hook 脚本不看 `process.ppid`，改看**整条祖先链**。启动时一次性
snapshot 进程树（PowerShell / ps），从 `process.ppid` 沿 parent 走到顶或 40 层
上限，得到 ancestor PID set。CC PID 一定在这条链里（CC 是 hook 脚本的
祖先进程），判定 `entry.owner_cc_pid ∈ ancestorSet`。缓存 5s 避免 Stop
poll loop 反复 PowerShell 启动。

---

### Bug 10: Stop hook 的 `Stop hook error:` UI 前缀干扰 AI

**位置**：同上 hook 脚本

**症状**：即使 hook 成功 inject，lead CC 的 reminder 显示成
`Stop hook error: [node "$CLAUDE_PROJECT_DIR/..."]: [TEAM-MODE] ...`。
"error" 字样让 lead AI 和用户困惑（是不是 hook 炸了？）。

**根因**：CC [Issue #34600](https://github.com/anthropics/claude-code/issues/34600)。
Exit code 2 + stderr 路径硬编码被 CC 包装成 "Stop hook error:" 前缀，官方不修。

**修复**：hook 从 `stderr + exit 2` 切换到 `stdout JSON + exit 0`：
```js
process.stdout.write(JSON.stringify({
    decision: "block",
    reason: renderReminder(c.mine)
}));
process.exit(0);
```
CC 识别 `decision:"block"` 后同样触发新 turn，但 reason 字段作为干净 reminder
注入，没有任何前缀。

---

### Bug 11: 多 worker 并发回复时只有第一条被投递（loop-guard 顺序错误）

**位置**：`scripts/hooks/lead-pending-wake.js::handleStop`

**症状**：lead 广播给 4 个 worker 让他们自我介绍。worker 回复几乎同时落 pending。
但 lead 只收到第一条 reply 的 reminder，剩余 3 条在 `lead_pending.jsonl` 里再也
不触发注入——直到用户手动输入任何内容创建新 turn，才被下一轮 Stop hook 捞到。

**根因**：Stop handler 的两道防环 guard 排在 `tryBlock()` **之前**：
```js
// 旧代码
if (event.stop_hook_active === true) process.exit(0);   // 第一个 Stop 注入后，CC 下一次 Stop 必带此标志
if (cooldown 命中)                 process.exit(0);     // 同理
tryBlock();                                              // 根本走不到
```

竞态还原：
1. T=0 lead turn 结束 → Stop hook #1 polling
2. T=3s bob 回复落 pending → hook 注入 bob → 退出
3. T=3~8s CC 处理 bob 的注入 turn，期间 alice/charlie/dave 陆续落 pending
4. T=8s 新 turn 结束 → Stop hook #2 启动，`stop_hook_active=true`
5. **旧代码直接 exit 0，pending 里 3 条新消息从未被扫描**

guard 的本意是防止"同样内容被反复注入成死循环"。但 `tryBlock()` 每次都
`writePeersBack()` 把自己的条目从文件里删掉——后续看到的条目必然是新的，
guard 的前提不成立。

**修复**（两步）：
1. **无条件先跑 `tryBlock()`**：pending 有我的新内容就注入，跟 `stop_hook_active`/
   cooldown 无关。已抽干的条目不可能重放，无死循环风险。
2. **guard 降级为"等待时长开关"**：pending 为空时，若检测到是 follow-up Stop，
   只等 `TEAM_MODE_STOP_TAIL_SEC`（默认 1s）就退出；fresh Stop 才等满 30 分钟。
   1s 尾等只是小优雅窗口——真正的正确性保证是"每次 Stop 开头都无条件查一次
   pending"，任何漏掉的消息下一次 Stop 必被捞起。

**验证**：单实例 4-worker 并发广播场景，lead 一次收到所有 reply。

**教训**：防御式代码的早退条件必须放在"无副作用的读操作"**之后**，不能盖住
真正的业务检查。"防止误操作"不能以"漏掉正确操作"为代价。

---

### Bug 8: worker claude CLI 子进程被 Stop hook 反噬（最隐蔽）

**位置**：`src/backend/claude_code.rs::spawn_child` + `scripts/hooks/lead-pending-wake.js`

**症状**：worker spawn 成功，agent_loop `processing inbox message` 后永远
等不到 `posting reply`。用户看起来 "worker 没响应"。`mcp.log` 无错误。
手工在外部跑 `claude -p "" --input-format stream-json ...` 能正常响应。

**根因**：**每个 claude CLI 进程（包括我们 spawn 的 worker）都会加载项目级
`.claude/settings.json`**。当 worker 完成 turn 时，它触发 *自己* 的 Stop
hook（也就是我们的 `lead-pending-wake.js`）。这个 hook 以 lead 身份阻塞
等 30 分钟 pending 消息 —— worker 不是 lead，永远等不到。期间 worker
进程的 `type:result` 事件被卡住不输出到 stdout，Rust agent_loop 永远收不到
`TurnComplete`，看起来 "worker 从不回复"。

症状非常隐蔽：
- `mcp.log` 看不到任何错误（worker 子进程技术上活着）
- `claude --version` 正常（shell script wrapper 自己没问题）
- 手工测试 work（because 手工场景里 Stop hook 超时后 result 会姗姗来迟）
- 第一次成功 → 以为是"偶尔 work"

**发现路径**：手工 `claude -p "" --input-format stream-json ...` 观察到
stdout 里一行 `{"type":"system","subtype":"notification","key":"stop-hook-error",...}`，
意识到 worker 自己也在触发 Stop hook。

**修复**：
1. Rust `claude_code.rs::spawn_child` 里给每个 worker 子进程设
   `TEAM_MODE_WORKER=1` env。
2. `lead-pending-wake.js` 开头加 fast-path：`if (process.env.TEAM_MODE_WORKER === '1') process.exit(0);`

**验证**：worker 场景下 hook 进程 0.2 秒秒退（甚至早于 log 调用），lead 场景不变。

**教训**：设计 hook 时要想清楚"谁会触发它"。项目级 hook 不仅影响 CC 主会话，
也影响**任何**在这个项目下跑的 claude CLI 子进程。worker 需要主动屏蔽。

---

### Bug 7: claude CLI 子进程 stderr 被吞，debug 困难

**位置**：`src/backend/claude_code.rs::spawn_child`

**症状**：worker 子进程启动问题时看不到任何错误输出。

**修复**：改 `Stdio::null()` → `Stdio::piped()`，spawn 一个 tokio task 后台读 stderr
转发到 tracing warn。现在能看到 claude CLI 自身报错。

---

## 用户明确的设计原则

以下是开发过程中用户多次强调的偏好，贡献者请尊重：

- **"没问题就别修"**：不在没有证据的情况下改架构或重构。只修实际暴露的 bug。
- **hook 只在有 team 的项目下生效，不配全局**：避免污染其他 CC 会话。
- **我们的 hook 放最后执行**：因为其他插件的 Stop hook 是非阻塞的，我们长阻塞
  必须让其他先跑完。项目级配置天然在全局插件之后加载。
- **ESC 应该能打断 hook**：SIGINT handler 必须就位。
- **别猜测**：涉及 CC 官方机制的问题（asyncRewake 行为、Channels 规则、Stop hook
  exit 2 语义）都要派 subagent 查官方文档 + 社区实现，不凭印象说。
