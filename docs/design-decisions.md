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

## 多层 worker 死亡检测（2026-04-27）

worker 进程死亡（kill / OOM / 崩溃）的检测有 **3 层**，相互兜底：

| 层 | 触发时机 | 反应时延 | 用途 |
|---|---|---|---|
| **agent_loop active probe** | 每 3s 一次 `is_alive` 检查（recv 等待间隙） | ≤6s | mid-turn worker 死了立即触发 OutputClosed [SYSTEM] notice 给 lead |
| **worker-liveness watchdog** | daemon 后台 std::thread，每 5s 扫 sidecar | ≤10s | 把 sidecar workers.json 状态从 RUNNING 同步到 DEAD（web UI 读这个） |
| **worker_list MCP 工具** | 每次调用都查 orchestrator.is_alive | 即时 | lead 主动看 worker 状态时返回真实 dead |

**为什么需要三层**：

- agent_loop active probe 修 mid-turn 终结消息分类——之前 `tokio::select! biased` 让 recv 抢占 tick，Windows codex stdout buffer 持有时永不超时；改成 `tokio::time::timeout(3s, recv)` 强制每 3s 探测一次。
- watchdog 修 web UI 实时性——web read_model 不持有 orchestrator 引用，只读 sidecar；watchdog 周期把"已死但 sidecar 还说 running"同步过来。
- worker_list 是用户主动确认的最强 source of truth（实时查不缓存）。

3 层共同保证：worker 死了 lead 在 ≤10s 内通过任意一条路径得知。

**是 backend 层的隐藏 bug**：早期的 `CodexBackend::is_alive` / `ClaudeCodeBackend::is_alive`
只查内部 `Arc<AtomicBool>`，但这个 flag 仅在 `shutdown()` 显式调用时才被设 false。
**外部 kill（taskkill / kill -9 / OOM / 崩溃）不会触发 alive=false**，所以前面三层
liveness 都看到 alive=true 假信号。修复：每个 backend 的 `is_alive` 在 alive=true 快速
路径之外加 sysinfo 实查 child PID 是否还在 OS process 列表里——alive=false 路径不变（保留
显式 shutdown 语义）。代价：每次 is_alive 调用 ~1-3ms（sysinfo refresh 单 PID）。
3 个 backend（codex / claude-code / gemini-cli）同时修。

---

---

## 双轨投递（Stop hook + PostToolUse hook，2026-04-27）

**问题**：单 Stop hook 路径下，worker reply 必须等 lead 当前 turn 完全结束才能投递。
lead 跑长 turn（10+ 工具调用）时，最早收到 reply 的时刻是整个 turn 结束。

**为什么只能这样**：CC 客户端的 `<system-reminder>` 注入只在 turn 边界发生。
mid-turn 推送的官方 API 是 OAuth-only 的 Channels（API key 不支持）。

**调研发现可用 mid-turn 注入路径**：CC 的 PostToolUse hook 可以返回
`additionalContext`，CC 将其追加到下一次 LLM 调用的 context 里——
**这是 mid-turn 唯一一个能在 API key 模式下塞内容给 lead LLM 的合法路径**。

**双轨设计**：

```
Worker reply → daemon 写 lead_pending.jsonl
                ↓
   ┌────────────────────────┴────────────────────┐
   │  PostToolUse 路径（mid-turn 快路径）         │
   │  - lead-pending-mid-turn.js                  │
   │  - 锁 + 抽 + 渲染 → additionalContext       │
   │  - 适合 lead 在长 turn 跑工具时             │
   │                                              │
   │  Stop 路径（turn 结束兜底）                  │
   │  - lead-pending-wake.js（已有）             │
   │  - 锁 + 抽 + 渲染 → decision:"block"        │
   │  - 适合 lead 没工具调用 / 纯思考完成        │
   └─────────────────────────────────────────────┘
```

**为什么没有重复投递风险**：两个 hook 共享同一个 `lead_pending.jsonl`
+ 同一个 `.lead-pending.lock`。无论哪个先 fire，先抽就先得，pending 立即
被改写（peers-only 留下）。另一个 hook 看到 `mine.length === 0` 直接退出。
不需要单独的 delivered tracker。

**LLM 语义防混淆**：

mid-turn 注入的 reminder 跟 turn-end 的格式**不同**：
- mid-turn: 视觉分隔块 `─── ─── ───`，明确标 "可稍后响应"
- turn-end: 紧凑 header + body，标 "收到新消息"

mid-turn 框架措辞强调"你可以继续手头任务，turn 结束会再次提醒"——
让 LLM 不要立即放下当前工作切换上下文。如果 LLM 觉得新消息紧急，
依然可以选择 mid-turn 响应；不紧急的就让 Stop hook 收尾。

**性能开销**：每次 PostToolUse hook 启动 ~50ms（Node 冷启动 + 文件锁 + 抽取）。
fast path（pending 文件不存在 / 文件为空）<10ms。

**worker fast-path**：worker 自己的 claude CLI 也会触发 PostToolUse
（项目级 .claude/settings.json 被任何 claude 进程读取）。新 hook 跟 Stop
hook 一样在脚本头检测 `TEAM_MODE_WORKER=1` env 并立即 exit 0。
否则 worker 会去抢 lead 的 pending 队列。

**实现**：
- `scripts/hooks/lead-pending-shared.js` — 抽取共用代码（process tree、锁、
  pending 抽取、reminder 渲染）
- `scripts/hooks/lead-pending-mid-turn.js` — 新 hook，PostToolUse 时跑
- `scripts/hooks/lead-pending-wake.js` — 现有 Stop hook，未改

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

### Bug 12: Worker 间 ping-pong 死循环（`parent.sender` auto-recipient）

**位置**：`src/team_mode/service/message_service.rs::send()`

**症状**：多 worker 团队下，alice 和 carol 互相 @ 之后，会无限互相礼貌回复
（"收到"→"等待指示"→"等待下一步"→"保持待命"→…），持续 10+ 分钟，消耗 token。
MCP log 可见每 5-15s 就有一次 `member=alice sender=carol` / `member=carol sender=alice` 交替。

**根因**：`send()` 对 Reply kind 自动把 parent.sender 加入 `effective_recipients`。
这是"自然聊天"语义：alice 回复 carol 的消息 → carol 自动收到 alice 的回复。但结合
LLM 喜欢产出礼貌确认的天性，worker 间一旦开启对话就停不下来。

**修复**：删除 parent.sender auto-recipient 规则。路由完全靠：
1. body 里的显式 `@mention`（`parse_mentions_from_body`）
2. Lead observability 规则（worker 发任何消息自动加 lead 为收件人）

worker 如果没在 body 里写 `@alice` 就不会路由给 alice，自然收敛。

**教训**：看起来符合直觉的"自动收件人"规则在 LLM 场景下要非常小心——人类知道何时
停止礼貌回复，LLM 不知道。

---

### Bug 13: CC ESC 关闭 MCP stdin → 所有 workers 陪葬

**位置**：架构层面，不是单一文件

**症状**：用户在 CC 里按 ESC（打断 hook 或任意 tool 调用）后，MCP 连接断开，
下次 /mcp reconnect 才能恢复；期间正在跑长任务的 workers 全部一起死。日志：
```
14:15:15 WARN MCP: stdin EOF — parent closed the pipe, exiting run_stdio
```

**根因**：**不是信号问题，是 IO 层面 CC 主动关了 MCP 的 stdin**。
- 多轮尝试（FreeConsole / SetConsoleCtrlHandler / tokio::signal）都是信号层面，
  对 stdin pipe close 无效
- stdio MCP 协议规定 EOF=会话结束，MCP 规范退出是正确的
- 但这导致 workers（MCP 的子进程）跟着 MCP 一起被 OS 收走

**修复**：架构重构为 Strategy H detached daemon：
- 新二进制 `team_mode_daemon` 持有 orchestrator + 所有 worker 进程
- MCP 变成 stdio relay，只转发 JSON-RPC 到 daemon（通过 127.0.0.1 localhost TCP）
- Daemon spawn 时 `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB`
- MCP 死/重启不影响 daemon + workers
- CC 全退出后 daemon 里的 `lead-watchdog` 发现 owner_cc_pid 都死 15s 后自杀

**验证**：手动 `Stop-Process team_mode_mcp`，worker 进程 + daemon 均存活；
重新 /mcp 连接后 `worker_list` 显示原 worker 仍 running，发消息正常走通。

**详见**：`docs/worker-detach-refactor.md`

---

### Bug 14: 同项目无限创建 team 无约束

**位置**：`src/team_mode/mcp/tools.rs::team_create`

**症状**：同一 CC 可以无限调 `team_create` 创建多个 team，每个又能加 worker，
资源失控。另外过去 session 留下的 orphan team（owner CC 已死）永远躺在磁盘上，
新 team 创建时没反馈。

**修复**：
1. `team_create` 启动时枚举所有 team，按 `owner_cc_pid` 检查 liveness：
   - 有活 team → 拒绝并列出
   - 只剩 orphan → 自动清理，返回 `cleaned_orphan_teams` 字段
2. `team_list` 为每个 team 加 `ownerStatus`（alive / orphan / unbound）字段，
   用户一眼看清历史遗留
3. `team_delete` 同时 prune `lead_pending.jsonl` 里该 team 的条目

---

### Bug 15: Web UI 显示的 worker 会话是 lead CC 的

**位置**：`src/team_mode_web/read_model.rs::conversation_for_member`

**症状**：web UI 右栏"进程会话 · worker"显示了 JSONL 文件路径，但内容是当前
CC（lead）的对话，不是 worker 的。表现为用户问 worker 一个简单问题，web 却渲染
出 lead 和用户讨论架构的上下文。

**根因**：discovery 只按 mtime 排序取第一个 JSONL。CC 比 worker 写得更频繁，
总是 CC 的 session 排在最前面。

**修复**（两步）：
1. 从 claude CLI 的 stream-json `system`/`init` 和 `result` 事件里捕获
   `session_id`（UUID，= JSONL 文件名），存到 ClaudeCodeSession 的
   `Arc<Mutex<Option<String>>>` 里
2. `agent_loop` 每次处理完 turn 后查 `orchestrator.session_id_of()`，把最新
   session_id 写回 `member.execution.session_id`（worker_add 时写不了，因为
   stream-json 模式 spawn 后 claude CLI 静默，等首个用户消息才发 init 事件）
3. web `read_model.conversation_for_member` 优先用 `execution.session_id`
   在 discover_sessions 结果里做精确查找，找不到才回落到 mtime-first

**附带 Windows 路径 bug**：`std::fs::canonicalize` 在 Windows 上会加 `\\?\`
前缀，encoding 出来 `---E--aigc-...` 跟 claude CLI 实际创建的 `E--aigc-...`
不匹配。`session_discovery.rs` 现在会 strip 该前缀并尝试多个路径变体。

---

### Bug 16: `worker_add on_existing=reuse` 对 dead worker 失败

**位置**：`src/team_mode/mcp/tools.rs::worker_add` + `src/runtime/orchestrator.rs`

**症状**：worker 进程死后调 `worker_add on_existing=reuse` 报
`"Managed member 'edgetest__bob' is already registered"`。讽刺的是
`[SYSTEM] worker died` 通知里就明确建议用户 "use worker_add with
on_existing=reuse to restart"，按指引操作必败。

**根因**：reuse 短路只在 `already_live==true` 时生效；dead worker 落到
`spawn_managed_member`，那里 hard-rejects 任何已注册 spawn_key。Bug 5 fix
只覆盖"进程活"场景，没清理 dead session 注册。

**修复**：
1. orchestrator 加 `remove_dead_session_if_any(&spawn_key)`：检测进程是否
   还活、活的不动、死了 drop HashMap entry + session_registry handle。
2. `worker_add` reuse 路径在 liveness 查询同事务里：dead → 调上述清理 →
   走正常 spawn 路径。响应里加 `revived_from_dead: true` + `note` 字段
   说明"新进程不继承旧对话上下文"。

---

### Bug 17: daemon 重启后给 dead worker 发消息永远无 [SYSTEM] 反馈

**位置**：`src/team_mode/mcp/tools.rs::send_message`

**症状**：daemon 被外部杀后自动重建，新 daemon 把所有旧 worker 标 dead；
但 `send_message @dead_worker` 仍返回 `delivered`，lead 永远收不到死亡
通知，Stop hook 阻塞到 7200s 超时。

**根因**：现有 `[SYSTEM]` 通知由 `agent_loop::send_input failure` 发出。
daemon 重启后没起 agent_loop（worker 已死），dispatch 进了无主 inbox 没
路径触发死亡反馈。

**修复**：`send_message` 派发前对每个 effective_recipient 查
`orchestrator.is_alive`：
- 全 dead → fail-fast 错误，列名提示用 reuse 复活；同时已写
  [SYSTEM] notice 到 lead inbox。
- 部分 dead → body 自动改写为 `[worker unavailable: <name>]`，对 alive
  recipient 正常派发；为每个 dead recipient 立即写 [SYSTEM] reply；
  返回 `dead_recipients` + `system_notices` + `dead_recipients_hint`。

---

### Bug 18: orchestrator 错误信息暴露内部 "runtime" 假名

**位置**：`src/runtime/orchestrator.rs`

**症状**：`team_delete` 的 `shutdown_failures` 里出现
`"Member 'edgetest__alice' not found in team 'runtime'"` —— 用户困惑
"team 'runtime' 是什么？"。其实 spawn_key 已含真 team 名，不需要再生造一个。

**修复**：4 处 `Error::MemberNotFound { team: "runtime", ... }` 替换为
`Error::Other("no managed session registered for spawn_key '...'")`，
错误文本直接说明问题（spawn_key 没注册，可能已 shutdown 或从未启动）。

附带：`team_delete` 加过滤——把 "no managed session registered" 与
"not found in team 'runtime'"（兼容旧 daemon）从 shutdown_failures 中
剔除；把 status=Removed 的成员一开始就跳过 shutdown 循环。

---

### Bug 19: `worker_add` / `team_create` 名字校验过松

**位置**：`src/team_mode/mcp/tools.rs` + `src/util/mod.rs`

**症状**：`worker_add name="has space"` / `name="中文名"` / `name="BadName"`
全部接受。但 `@mention` 解析器只识别 `[A-Za-z0-9_\-.]`——这些 worker 创建
后无法 @mention，成为不可达僵尸。

**修复**：新增 `validate_slug_name`：要求小写 ASCII 字母/数字/`_`/`-`/`.`，
长度 1-64，必须以字母或数字开头。`team_create` 与 `worker_add` 调用前先校验，
失败时错误信息明确指出哪个字符不合规、规则是什么。统一小写之后 @mention
就能做 case-insensitive 匹配（见 Bug 21）。

---

### Bug 20: Web UI 把已 remove 的 worker 显示为"运行中"

**位置**：`src/team_mode_web/read_model.rs::list_members` 和 `team_counts`

**症状**：worker 已 `worker_remove`，但 web UI 仍把它列在成员侧栏，状态
"运行中"。原因：`list_members` 不按 `MemberStatus::Active` 过滤；而成员
disk record 里的 `execution.sessionState` 不会随 remove 改写（保留以备
未来 reuse fast-resume），所以一直显示最后已知状态。

**修复**：`list_members` 与 `team_counts_from_parts` 都按 `status==Active`
过滤；`member_count` 改为只数 active 成员，与侧栏可见数量一致。

---

### Bug 21: @mention 大小写敏感

**位置**：`src/team_mode/mcp/tools.rs::send_message`、
`src/team_mode/service/message_service.rs::send`

**症状**：`@BOB` 不匹配 worker `bob`，报 unmatched mention。lead AI 写消息
时常自然大写，每次踩坑都得改回小写。

**修复**：name 校验已强制小写（Bug 19），所以 case-insensitive 匹配不会有
两个名字冲突。`send_message` 与 `message_service::send` 都建 lowercase→
canonical 索引，对 `mention.handle.to_lowercase()` 做查找；display 保留
原大小写，路由用规范名。错误信息提示 "Mention matching is
case-insensitive; check spelling."。

---

### Bug 22: lead-watchdog 在 `teams.is_empty()` 时永不自杀

**位置**：`src/team_mode_daemon/server.rs::run_lead_watchdog`

**症状**：用户 `team_delete` 删完最后一个 team 后，daemon 仍占内存 + 端口，
永远不退出。`consecutive_dead` 计数器在 empty branch 里被重置为 0。

**修复**：empty branch 也累加 `consecutive_dead`，并使用同一 grace
（15s）。下次 `team_create` 重新拉起 daemon 仅 ~1s。日志改成
`"lead-watchdog: no teams left for grace period, shutting down daemon"`。

---

### Bug 23: Stop hook 单次只投第一条到达消息

**位置**：`scripts/hooks/lead-pending-wake.js::tryBlock`

**症状**：lead 广播给 N 个 worker，第一条 reply 到达 pending 时 Stop hook
立即注入并 exit；后续 ~毫秒内到达的 reply 只能等下次 Stop。Bug 11 fix
解决了"被漏掉"的问题，但用户感受为"消息分批到"。

**修复**：`tryBlock` 检测到至少一条消息后等
`TEAM_MODE_STOP_BATCH_GRACE_MS`（默认 500ms）再 reclassify，把同窗 reply
合并成一条 reminder。窗口可调，0 即关闭。日志加 `(batch grace 500ms)`
标记便于审计。

---

### Bug 25: worker turn 静默结束 / 进程崩溃时 lead 没反馈

**位置**：`src/runtime/agent_loop.rs::run` step 4-5

**症状**：
- worker 完整收到 dispatch 但 LLM 没产出任何文字（指令不遵循 / 拒答 /
  内容被过滤），`type:result` 一来就 break，旧代码 `if !body.is_empty()`
  跳过 send，inbox 默默 ack，lead 永远等不到反馈。
- worker 子进程 stdout pipe 中途关闭（崩溃 / OOM / 被外部 kill），
  `output_rx.recv()` 返回 None，旧代码 `None => return` 直接退出
  AgentLoop，连 [SYSTEM] 都不发，lead 完全失明。
- 与 Bug 17（send_input fail）互补：Bug 17 处理"进 turn 之前进程已死"，
  Bug 25 处理"进 turn 之后进程死"和"进 turn 但产出为空"。

**用户说法**：worker 处理完任务后 lead 应该都能感知，无论是回复还是
"什么都没说就停了"。但是不能因此发两条（reply + 完成通知）—— reply
本身就是"完成"信号，多发就是噪音。

**修复**：每次处理一条 inbox dispatch，AgentLoop 必须输出**恰好一条**
终结消息回 room：

| 终结条件 | end_cause | 内容 | kind |
|---|---|---|---|
| 有可见文字（trim 后非空） | 任意 | worker 实际输出 | `Reply` |
| 空输出 + TurnComplete | TurnComplete | `[SYSTEM] worker 'X' completed its turn without producing any reply text for msg <id>...` | `Status` |
| 空输出 + AgentError | AgentError | `[SYSTEM] worker 'X' raised an agent error...` | `Status` |
| 任意 + 输出 pipe 关闭 | OutputClosed | `[SYSTEM] worker 'X' output channel closed mid-turn... use worker_add on_existing=reuse to revive` | `Status` |

实现上把 `let mut end_cause` 模式换成 `let end_cause: TurnEndCause = loop { ... break <expr> ... }`。
`OutputClosed` 路径在发完 [SYSTEM] + ack 后退出 AgentLoop（pipe 死了
没法处理新消息）。

测试加两个：`agent_loop_emits_system_status_on_silent_turn`（验证空输出
也产出 Status）+ `agent_loop_emits_system_status_on_output_pipe_close`
（验证 None 路径仍发 [SYSTEM] 并把已收到的 partial 内容附在 body 里）。

**为什么不加超时**（防 worker hang 永不返回 type:result）：现实中
hang 几乎都是 LLM 长思考的合法场景，加错超时就误报；当真 hang 时进程
通常活着，用户可手动 kill 触发 OutputClosed 路径。如果未来真出问题，
再加可配置的 `TEAM_MODE_TURN_HARD_TIMEOUT_SEC`。

---

### Bug 24: Web UI 右栏对无 session_id 的 worker 串台到 lead

**位置**：`src/team_mode_web/read_model.rs::conversation_for_member`

**症状**：刚 `worker_add` 的新 worker 在产生第一条 reply 之前没有
session_id；旧代码用 `or_else(|| sessions.first())` 兜底——按 mtime 排序，
最近的 JSONL 是 lead/CC 自己的，结果右栏显示 lead 的全部对话与工具调用。
用户一直以为"右栏不管点谁都是 lead"，其实是 sessionless worker 全部
fallback 到 CC 的 JSONL。

**修复**：non-lead member 取消 mtime fallback——sessionId 不在
discover_sessions 列表时返回 `confidence: "no_session_yet"` 占位，并附
`limitations` 字段说明"等首个 type:result 后刷新即可"。lead 仍允许 mtime
fallback（因为 lead 就是当前 CC，最近的 JSONL 本来就是它）。

---

## 工具描述压缩 + 运行时 hint 模式（2026-04-25）

**问题**：8 个 tool descriptions 总长 ~2.5KB，全量加载到上下文。其中大量是
"操作注意事项 / 反例警告"：
- `send_message` 描述里塞着"DO NOT sleep-and-poll, see docs/push-notifications.md"
- `inbox_read` 描述里"FALLBACK ONLY"长篇说明
- `worker_add` 描述把 reuse/overwrite/error 三个 enum 值各展开两行解释

**坏处**：
1. token 浪费——每次会话开头都加载，不管 AI 是否要用这个工具。
2. 注意力不连续——AI 看一次后 attention 衰减，过几轮就忘"不要轮询"这事。
3. 不动态——警告固定不变，不能针对当下场景细化。

**修复方向**：
- 静态描述只保留"AI 第一次碰这工具就必须知道的契约"（参数语义 + 关键
  调用前提）。删除文档链接、长篇 fallback 描述、enum 值展开。
- "操作建议 / 反例警告 / 复活方式" 移到工具返回值的 `hint` 字段，按当前
  状态动态生成：
  - `send_message` 成功 → `hint`: "Replies will arrive automatically..."
  - `send_message` 部分 dead → `dead_recipients_hint`: 复活指令
  - `inbox_read` 空 → `hint`: "无消息；Stop hook 自动投递"
  - `worker_list` 含 dead → `hint`: "Dead workers: [...]. Revive with..."
  - `team_list` 含 orphan → `hint`: "Orphan teams: [...]. team_delete 清理"
  - `worker_add` 新建 → `hint`: "session_id 在首个 type:result 后捕获"

**结果**：工具 description 总长从 ~2.5KB 缩到 ~700 chars（节省 ~70%），
关键提示出现在 AI 最需要它的时刻——刚调用完工具、正要决定下一步动作时。

**为什么不全部塞 description**：description 是 system prompt 的一部分，
"全量预加载 + 静态" 的本质决定它不能承载所有指引；动态返回 hint 才能
做到 just-in-time。硬规则（必须含 @mention、name 必须 slug）继续在代码层
强制 + fail-fast 错误信息说清原因；建议性指引（不要轮询、复活方式）放
hint。

---

### Bug 26: codex worker reply body 膨胀（命令 stdout + delta/message 双写）

**位置**：`src/backend/codex.rs::map_notification_to_output` + `src/runtime/agent_loop.rs` step 4

**症状**：lead 自检报告——单次 Stop hook 通知 "2 new messages" 但 payload **50,000+ tokens**。
真正的 200 词总结被埋在大量"看起来像工具调用 transcript"的内容里：完整文件源码 dump、
PowerShell 终端输出 + ANSI 颜色码（`[31;1m`）、数百条 grep file:line:content 行。同一条
backend-dev 的 message body 出现两次（顶部一次 + 嵌在 researcher reply 下半段）。

**根因**（两条独立 bug 叠加）：

26a — Shell stdout 污染：codex 协议的 `EVENT_COMMAND_OUTPUT_DELTA`（`bash`/`local_shell`
工具的实时 stdout 流，含 `cat src/foo.rs`、`grep -rn`、ANSI 转义码）被映射成
`AgentOutput::Delta`，与 LLM 文本回复同一类型，被 AgentLoop 的 `parts.push()` 路径无差别拼进 body。

26b — `agentMessage` 双轨写入：codex 对同一条 LLM 回复**先**逐 token 发
`EVENT_AGENT_MESSAGE_DELTA`，**再**在消息结束时发 `EVENT_ITEM_COMPLETED`（type=agentMessage,
含完整文本）。两个事件都被处理为有效输出（`Delta` + `Message`），AgentLoop 等同对待
→ 同一条 worker 回复被拼进 body 两次。

**修复**：
- 新增 `AgentOutput::ToolOutput(String)` 变体；`EVENT_COMMAND_OUTPUT_DELTA` 映射到它。
  AgentLoop 收到 `ToolOutput` → trace-log 后丢弃，永不入 body。
- 重构 step 4 累积逻辑：`final_message: Option<String>` 单独存 `Message` 事件，
  `deltas: Vec<String>` 单独存 `Delta` 事件。组装 body 时**优先** `final_message`，
  否则 fallback 到 `deltas.join("")`。codex 双写 → 只取 `Message`；纯 delta 流（claude_code、
  gemini）→ join deltas。

> **注**：Bug 26 的修复在 Bug 29 引入"workers 必须显式调 `send_message`"后被进一步
> 简化——AgentLoop 不再 auto-capture stdout，body 拼接逻辑整段删除。但 `ToolOutput`
> 变体保留作为防御性记录（trace 用），万一未来有 backend 漏洞重新泄漏 stdout。

---

### Bug 6 & 7: lead_pending text 未截断 + Stop hook 双 hook 竞态

**位置**：`src/team_mode/service/lead_pending.rs::write_for_lead` + `scripts/hooks/lead-pending-wake.js::tryBlock`

**Bug 6 症状**：Stop hook 把 `entry.text` 直接当 `{decision:"block", reason:...}` 推 lead。
当 worker reply 体积大（Bug 26 中 50K+ tokens；未来 backend 类似漏洞），整段灌入 lead 的
`<system-reminder>`，每次 worker 回复都烧 lead context。

**Bug 6 修复**：`truncate_body_for_pending()` 在 `write_for_lead` 入口截断到 16 KB
（≈ 4K tokens），UTF-8 安全（walk back 到 char boundary），追加清晰标记
`[…body truncated. Call mcp__team-mode__inbox_read or read messages.jsonl for the full reply.]`。
**原 body 在 messages.jsonl 完整保留**——截断只影响 Stop hook 推送的通知 payload。

**Bug 7 症状**：CC 快速连续完工两轮 → 两个 Stop hook 实例并发跑 `tryBlock`。
`classifyPending → writePeersBack` 之间无锁，两个实例都看到同一批 `mine`，都 drain，
都 inject → 同批消息推 lead 两次。

**Bug 7 修复**：`acquirePendingLock()` 用 `fs.openSync(path, 'wx')` 做独占创建文件锁
（`.lead-pending.lock`），包住 `tryBlock` 的整个临界段。stale 锁（mtime > 30s）自动
强解。指数退避（50→100→200ms）真定时器 sleep（不烧 CPU），总等待 3s 后放弃，
caller 跳本轮，poll loop 下一周期重试。

---

### Bug 27: 多 CC 同 cwd 时 web UI 显示错的 lead session

**位置**：`src/team_mode_web/read_model/conversation.rs::read_member_conversation`

**症状**：用户在同一项目目录开两个 CC（PID A 和 PID B），其中 A 调 `team_create` →
`team.owner_cc_pid = A`。Web UI 拉 lead session JSONL 时走 mtime-fallback
（`sessions.first()` = newest）→ 拿到 PID B（更近活动）的 JSONL 而非 A 的。结果
"lead session" 标签下显示**另一个 CC** 的工具调用历史。

**根因**：Claude Code 的 JSONL 文件名是 session_id（无 PID 信息），daemon 没法靠
`team.owner_cc_pid` 反查具体哪个 JSONL 是该 team 的 lead。fallback 用 mtime 是
单 CC 场景的合理简化，多 CC 时崩。

**修复**：Stop hook 是唯一同时知道 `process.ppid`（CC PID）和 `event.session_id`
（Claude session id）的地方。让它每次 fire 时把 `{ancestor_pid: claude_session_id}`
映射写到 `<base_dir>/.lead-sessions.json`（atomic write，7 天 TTL GC 防累积）。
read_model 的 lead session 选择路径 lookup `team.owner_cc_pid` → 拿 session_id →
精准匹配 JSONL，不再 mtime fallback。

接管场景（lead 死、新 CC 接班）：team.owner_cc_pid 仍指旧死 PID → 映射查不到 →
fallback 到 mtime（保留旧的"宽容"行为，至少能显示一些东西）。**真正的 takeover
工具未做**——需要新 `team_takeover` MCP tool 把 owner_cc_pid 改到当前 CC PID，
并清掉旧映射。

---

### Bug 29: workers 显式 MCP 消息 + caller identity（架构调整）

**位置**：`src/team_mode_daemon/{client.rs,server.rs}` + `src/team_mode/mcp/tools/{worker.rs,message.rs}` + `src/runtime/agent_loop.rs` + `src/backend/codex.rs`

**问题**：原架构 worker 子进程的整轮 stdout 被 AgentLoop 直接当 reply body 拼起来发 lead。
这导致：
1. Bug 26 类的体积膨胀（codex shell tool stdout 变成"消息"）
2. worker 没有"私有 thinking" vs "对外 say" 的语义边界——所有输出都是公开的
3. 防御性代码越加越多（`ToolOutput` 区分、delta/message 优先级、lead_pending 截断）
   但都是治标
4. claude-code worker 实际上**已经**自动加载了项目 `.mcp.json`（claude CLI 默认行为），
   能调 `mcp__team-mode__send_message`——但 `send_message` 硬编码 `sender = LEAD_NAME`，
   worker 调它会**冒充 lead 发消息**，daemon 完全无 caller identity 概念

**架构调整**：

1. **身份绑定**：`worker_add` spawn worker 时注入 `TEAM_MODE_TEAM` + `TEAM_MODE_MEMBER`
   两个 env vars。env 透传给 worker 的子进程（包括 worker 自己拉起的 team-mode MCP
   relay）。relay 启动时 read env 构造 `MemberIdentity`，每次 RPC 携带在 `context.caller_*`
   字段。lead 的 relay（无 env）默认 `caller_member = "lead"`。

2. **codex MCP 注入**（claude-code 自动 work；codex 不读 .mcp.json，需特殊处理）：
   spawn codex 时 build `<cwd>/.agent-teams/<team>/codex-homes/<member>/config.toml`
   含 `[mcp_servers.team-mode]` 节 + env passthrough，并设 `CODEX_HOME=<那个目录>`。
   per-worker 隔离，不污染用户全局 `~/.codex/config.toml`。

3. **`send_message` 改造**：sender 取自 `caller_member`（不再硬编码 lead）。
   workers 只能发自己 bound 的 team。`@mention` 错误把 `@lead` + 所有活 worker 列出
   供模型自纠。worker text 没 @ 任何人 → 默认 `@lead`。

4. **AgentLoop 删 stdout-as-reply**：删 `parts/deltas/final_message/raw_body` 整套，
   不再 auto-publish stdout。新方法 `count_self_messages_after()` 通过扫
   messages.jsonl 检测 worker 这一轮调过几次 send_message，决定是否需要发 [SYSTEM]
   "silent turn" 通知。pipe-close / agent-error 仍发 [SYSTEM] 终结通知（lead 必须知道
   worker 死了）。

5. **Onboarding 重写**：worker prompt 教"必须调 `mcp__team-mode__send_message` 才算
   说话；stdout 是私有 thinking，不会被对方看到；不调 send_message → lead 收到
   '白干'通知"。

**结果**：worker 必须**显式**调工具发消息。stdout（思考、bash 输出、ANSI、文件 dump）
永不进入 messages.jsonl。Bug 26 整类问题从根源消除——不是靠过滤，是靠不给路径。
`ToolOutput` / Bug 6 截断保留作纵深防御。

**未做**：
- `team_takeover` 工具（Bug 27 的接管完整修复）——优先级低，单 CC 场景已 work
- 运行时验证 codex 真的接受 `CODEX_HOME` + per-worker config.toml 这套（subagent
  调研基于 codex 官方文档）

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
- **不靠 docs 操作 MCP**：MCP 使用注意点写到工具返回 `hint` 里，不要让 AI
  去读 markdown。硬规则用代码层校验 + 清晰错误信息；建议性指引用动态 hint。
