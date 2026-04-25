# Worker Detach Refactor — 让 MCP 可重启、workers 不死

**状态**：已落地基础版（MCP thin relay + daemon tool host + runtime sidecar）
**目标读者**：Rust 工程师，熟悉 tokio、process 管理、Windows IPC
**预估工作量**：基础版已实现；完整 E2E/Windows ESC smoke 仍需在具备 linker 与真实 Claude Code 环境的机器上验收

---

## 1. 问题陈述

当前架构下，用户在 Claude Code 中按 ESC（任何场景），CC 会关闭 MCP 进程的 stdin 管道。MCP 的 `run_stdio` 循环读到 EOF 后规范退出，**MCP 进程终止 → 所有 worker 子进程随之死亡**（`kill_on_drop(true)`）。

证据：`.agent-teams/mcp.log`
```
14:15:15.978 WARN MCP: stdin EOF — parent closed the pipe, exiting run_stdio
```

### 为什么任何信号 / FFI 方案都修不了

- 这不是信号，是 IO 层面的 stdin 关闭
- 管道由 CC 拥有，它 close 我们只能读到 EOF
- `SetConsoleCtrlHandler` / `FreeConsole` / `tokio::signal` 都是信号路径，对 IO 关闭无效
- stdio MCP 协议规定 EOF = 会话结束，违反协议也没意义（CC 那端早已认为断开）

### 用户场景与实际影响

Team Mode 目标是"lead 协调多个 worker 长时间执行大任务"。当前：
- 用户按 ESC（打断 hook、打断 CC 某个正在跑的操作）→ MCP 死 → 所有 worker 死 → 整个任务进度丢失
- 用户必须手动 `/mcp` 重连；workers 从头再来

预期：
- ESC / MCP 断开只影响 MCP 本身，**workers 继续跑**
- 用户 `/mcp` 重连 → 新 MCP 发现已有 workers，无缝接管
- 大任务跨越多次 ESC 仍能完成

---

## 2. 目标架构（Strategy H）

```
┌─────────────────────────────────┐
│  Claude Code (CC 主进程)         │
└────────┬────────────────────────┘
         │ stdin/stdout (stdio MCP)
         ↓
┌─────────────────────────────────┐
│  team_mode_mcp.exe (无状态 relay)│
│  - 接收 JSON-RPC tool 调用       │
│  - 通过 IPC 和 daemon 通信       │
│  - 自身无 worker 句柄            │
│  死了不影响 workers              │
└────────┬────────────────────────┘
         │ IPC (TCP localhost + u32 BE length-prefixed JSON)
         ↓
┌─────────────────────────────────┐
│  team_mode_daemon.exe (常驻)     │
│  - 拥有所有 worker 进程句柄      │
│  - 持久化 worker PID、IPC 端点   │
│  - 处理 spawn / send_input / etc │
│  - 提供重连语义                  │
└────────┬────────────────────────┘
         │ stdio pipes (direct)
         ↓
┌───────┴────────┬────────┬───────┐
│ claude CLI     │ codex  │ ...   │ (workers)
└────────────────┴────────┴───────┘
```

### 关键不变量

1. **MCP（CC 看到的进程）完全无状态**：所有状态在 daemon 或磁盘
2. **Daemon 常驻**：一个 project 一个，首次 MCP 启动时拉起
3. **Workers 由 daemon 拥有**：MCP 死 → daemon 活 → workers 活
4. **IPC 断开可恢复**：MCP 重启后以 IPC 重新连接 daemon
5. **Daemon 崩溃兜底**：下次 MCP 启动发现没 daemon → 重拉起 daemon，并尝试从磁盘恢复 worker 状态（发现孤立 worker 进程再接管或标记死亡）

---

## 3. 现有代码地图（实施前必读）

| 文件 | 职责 | 改造强度 |
|---|---|---|
| `src/bin/team_mode_mcp.rs` | MCP 入口，stdio loop | 大改：拆成 thin relay |
| `src/team_mode/mcp/runtime.rs` | JSON-RPC 协议解析 | 保留，改成 IPC client |
| `src/team_mode/mcp/tools.rs` | 所有工具实现（1410 行）| 大改：tool 调用转发给 daemon |
| `src/runtime/orchestrator.rs` | 运行时 session 管理 | **整体迁移到 daemon** |
| `src/runtime/agent_loop.rs` | worker inbox→reply 循环 | **整体迁移到 daemon** |
| `src/backend/claude_code.rs` | claude CLI spawn | 保留，daemon 调用 |
| `src/backend/codex.rs` | codex spawn | 保留，daemon 调用 |
| `src/backend/gemini.rs` | gemini spawn | 保留，daemon 调用 |
| `src/team_mode/service/*.rs` | 存储服务（团队/消息等）| 保留，daemon 和 MCP 共用（读操作可无锁）|
| `src/team_mode/storage/*.rs` | JSON 文件读写 | 保留 |
| `scripts/hooks/lead-pending-wake.js` | Stop hook | 不动 |

现有 `src/team_mode_web/` 独立 web server 可参考——它已经是"无 daemon 依赖，直接读磁盘数据"的只读视图。

---

## 4. 详细实施步骤

### 4.0 当前实现说明

本仓库当前采用一个务实的基础实现：

- `team_mode_mcp` 默认不再持有 `TeamModeToolset`，而是通过 `DaemonToolClient` 转发 `tools/list` 和 `tools/call`。
- `team_mode_daemon` 持有 `TeamModeToolset`、`RuntimeOrchestrator` 与 `AgentLoop`，因此 MCP stdin EOF 退出不会 drop worker session handle。
- IPC 使用 `127.0.0.1:<ephemeral-port>`，端点信息写入 `.agent-teams/runtime/daemon.json`，wire 格式为 `<u32 BE length><JSON bytes>`。
- daemon 启动由 MCP 负责，Windows 下使用 `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`，并把 stderr 写入 `.agent-teams/daemon.log`。
- worker runtime 状态写入 `.agent-teams/runtime/workers.json`。daemon 重启时会把旧 `starting/running` 记录标为 `dead`，不尝试热接管孤立 worker。
- `lead_pending.jsonl` 仍写项目根；MCP 会把当前 CC owner pid 作为 tool context 传给 daemon，避免 daemon 误用自身 parent pid。

设计偏离：文档原建议 Windows named pipe / Unix socket；当前基础版先采用 localhost TCP，原因是跨平台实现更小、无新增依赖、便于先验证 detach 生命周期。后续如需要更严格的本机 IPC 命名空间，可把 `src/team_mode_daemon/ipc.rs` 的 transport 替换为 named pipe/UDS，MCP runtime 和 toolset 不需要再大改。

### 4.1 新增 `team_mode_daemon` 二进制

**新文件**：`src/bin/team_mode_daemon.rs` + `src/team_mode_daemon/` 模块

**职责**：
- 监听 IPC 端点（见 §4.3）
- 实例化 `RuntimeOrchestrator` + 所有后台服务
- 处理 MCP 发来的 IPC 请求（tool 调用 / 查询 / 生命周期）
- 运行每个 worker 的 `AgentLoop`
- 持久化 worker runtime 状态到 `.agent-teams/runtime/workers.json`：
  ```json
  {
    "workers": [
      {
        "team_id": "foo",
        "name": "alice",
        "pid": 12345,
        "adapter": "claude-code",
        "spawned_at": "2026-04-24T...",
        "ipc_endpoint": null   // 如果后续用 worker-proxy 架构则填充
      }
    ]
  }
  ```

**启动流程**：
1. 读 `workers.json`
2. 对每个记录检查 PID 是否存活（`sysinfo::System`）
3. 活着 → 查是否能通过已有 stdio 接管（见 §4.5 研究项）
4. 死了 → 标记 `sessionState="dead"`，不重启，等 lead 决定
5. 开始监听 IPC

**关键问题（待研究）**：daemon 本身怎么常驻？
- 选项 A：MCP 检测不到 daemon 时 `Command::new("team_mode_daemon")` 拉起 + `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`（Windows）
- 选项 B：Windows 服务（复杂，需要管理员权限）
- 选项 C：用 PID 文件 + 进程探活；MCP 启动时若 PID 不活，拉起新 daemon
- **推荐选项 C**，简单可靠

### 4.2 重构 `team_mode_mcp` 为 thin relay

**修改**：`src/bin/team_mode_mcp.rs` + `src/team_mode/mcp/runtime.rs` + `src/team_mode/mcp/tools.rs`

**改造后的 MCP 行为**：
1. 启动检查 daemon 是否活（读 PID 文件 + 进程探活）
2. 不活 → 拉起 daemon，等它监听就绪（通过 IPC ping 轮询 ~2s）
3. 建立到 daemon 的 IPC 连接
4. 进入 stdio loop：
   - 读 CC 来的 JSON-RPC
   - 转发给 daemon（同样是 JSON-RPC over IPC）
   - 接收 daemon 响应
   - 写回 CC
5. stdin EOF → MCP 自己退出；**不杀 daemon**，不杀 workers

**MCP 保留的本地能力（无需 daemon）**：
- 纯读操作（比如查磁盘数据的 `team_list`）可以本地直接读磁盘，不必往 daemon 转。**但为简化设计，一致性建议统一走 daemon**。

### 4.3 IPC 协议

**选型**：Windows 命名管道（`\\.\pipe\team-mode-daemon-{hash}`），Unix domain socket（`/tmp/team-mode-daemon-{hash}.sock`）。Rust crate 推荐：`interprocess` 或 `tokio-named-pipes`。

**命名空间**：基于 project path 的哈希：
```rust
let endpoint = format!(
    "\\\\.\\pipe\\team-mode-daemon-{}",
    hex::encode(&Sha256::digest(base_dir.as_os_str().as_encoded_bytes())[..8])
);
```
防止多 project 互串。

**Wire 格式**：复用现有 `JsonRpcRequest` / `JsonRpcResponse`，加长度前缀（避免消息粘包）：
```
<u32 BE length><JSON bytes>
```

**方法集**：MCP 暴露给 CC 的 tool 名称 + `ping` / `shutdown`（daemon 管理专用）。**tool 名和参数不变**，只是 dispatch 到 daemon。

### 4.4 Worker 存活策略

**核心矛盾**：daemon 死了 workers 怎么办？

方案：
- Daemon 启动时用 **`CREATE_NEW_PROCESS_GROUP`**（Windows）/ **`setsid()`**（Unix）让自己脱离父进程组
- Worker spawn 时对每个 child **不要** `kill_on_drop(true)`，改用显式 shutdown 路径
- Daemon 崩溃 → worker 孤立但活着 → 新 daemon 从 `workers.json` 读 PID，**但无法接管已死 daemon 拥有的 stdio**
  - 接管选项 1：workers 本质上就 detached，stdio 不再共享；daemon 通过命名管道和每个 worker 通信（需要给 claude/codex/gemini 包一层 proxy）——工作量大
  - 接管选项 2：daemon 崩溃后 workers 作废，标 dead，lead 视情况 reuse——工作量小但用户感知差
- **推荐选项 2**，daemon 设计成尽量不崩（做好 panic unwind）

### 4.5 待研究 / 不确定点（engineer 需先调研再做）

1. **Windows 命名管道在两个独立 Rust 进程间的正确用法**：
   - `tokio::net::windows::named_pipe::{NamedPipeServer, NamedPipeClient}` 是否满足需求？
   - 是否需要 SecurityDescriptor？
   - 多 MCP 同时连 daemon 时 server 如何 accept 并发？

2. **Daemon 进程怎么"完全脱离" MCP 生命周期**：
   - Windows: `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` 标志是否足够？会不会还是被 CC 的 console group 杀到？
   - 若 CC 是 Job Object 的 owner 并把所有后代加进来，daemon 即使 CREATE_BREAKAWAY_FROM_JOB 能不能挣脱？需要先 `JobObjectExtendedLimitInformation.JOB_OBJECT_LIMIT_BREAKAWAY_OK`

3. **IPC 连接重建语义**：
   - MCP 死重启，daemon 里之前的"MCP 身份"如何识别？owner_cc_pid？哪个字段？
   - Daemon 如何把异步 worker reply 推给当前连接的 MCP？（MCP 只是工具转发，不长轮询——所以异步推送其实还是靠 `lead_pending.jsonl` + hook，与 daemon 无关）

4. **现有 `lead_pending.jsonl` + hook 机制是否需要改**：
   - 应该**不用改**：pending writer 在 daemon 里跑，文件写入不变；hook 仍按 owner_cc_pid 路由
   - 验证：worker reply → daemon 的 `MessageService` → `LeadPendingWriter` 写文件 → hook 读 → 注入 CC，全程不依赖 MCP 进程存活

5. **重连后 `worker_list` 怎么报告**：
   - 正常：daemon 从内存直接返回
   - daemon 死过：daemon 从 `workers.json` 读 + PID 探活（已有 `worker_list` live check 改造思路）

6. **并发 MCP 连接**：如果用户开了两个 CC 指向同一个 project，两个 MCP 都连到同一个 daemon。daemon 需要按 `owner_cc_pid` / `team.owner_cc_pid` 隔离 tool 调用影响

---

## 5. 测试计划

### 单元测试
- `team_mode_daemon`：mock IPC，测 tool dispatch 正确性
- `team_mode_mcp` relay：mock daemon，测 stdio→IPC 转发
- Worker 持久化：`workers.json` 读写 + PID 探活逻辑

### 集成测试（end-to-end，新增 `tests/detach_e2e.rs`）
必须全过：
1. **MCP 重启，daemon 和 workers 存活**：
   - 启 daemon + MCP + 1 worker，发消息收到回复
   - `kill` MCP 进程，验证 daemon 活、worker 活
   - 重启 MCP，`worker_list` 显示 worker `running`
   - 再发消息，仍能收到回复（同一 worker 处理）

2. **Daemon 重启，孤立 workers 被标 dead**：
   - 启 daemon + 1 worker
   - `kill` daemon 和 MCP 都
   - 重启 MCP → 拉起新 daemon
   - `worker_list` 显示 worker `dead`
   - 发消息给 dead worker → 立即收到 `[SYSTEM]` 死亡通知

3. **ESC 场景模拟**：
   - 模拟 CC 关闭 MCP stdin
   - MCP 规范退出，daemon + workers 无变化
   - 重启 MCP，状态完整保留

4. **多 MCP 同时连一个 daemon**：
   - 启 daemon
   - 启 MCP_A（project 同路径，PID X）+ team_A
   - 启 MCP_B（project 同路径，PID Y）+ team_B
   - 两者互不干扰，`worker_list` 各看各的 team
   - 各自 lead_pending.jsonl 路由正确（按 owner_cc_pid）

### Smoke 测试（手工）
1. 开 CC，`/mcp` 连接，创建团队 + 2 workers
2. 发广播消息，确认 shepherd 捕获回复
3. **按 ESC** 打断 hook
4. 确认：`/mcp` 状态显示 disconnect
5. `/mcp` 重连
6. 确认：workers 仍然在 `worker_list` 里 `running`
7. 再发消息，收到回复（验证 worker 进程未死）
8. `team_delete` → worker 进程应该被正常杀掉
9. 关闭 CC → MCP 死 → daemon 应该仍活着
10. **手动杀 daemon** → 再开 CC → 新 daemon 拉起 → 之前的 workers 标 dead

---

## 6. 验收标准

所有以下必须为 TRUE 才算完成：

- [x] `team_mode_daemon` 二进制入口存在
- [x] `team_mode_mcp` 默认通过 daemon relay 执行 team-mode tools
- [x] `.agent-teams/runtime/daemon.json` 记录 daemon 端点
- [x] `.agent-teams/runtime/workers.json` 记录 worker runtime 状态
- [x] daemon 重启时把旧 `starting/running` worker 标为 `dead`
- [x] 文档：更新 `docs/open-source-deployment.md` 描述新架构 + daemon 启动方式
- [ ] `cargo build --release` 通过，三个二进制都产出（当前机器 MSVC `link.exe` 缺失，需在完整工具链机器验收）
- [ ] `cargo test --release` 全绿（当前机器 MSVC `link.exe` 缺失，`cargo check --tests` 已通过）
- [ ] 集成测试 §5 四个场景全过
- [ ] Smoke 测试 §5 十步全过
- [ ] 在 Windows 上按 ESC 后 `workers.json` 内容不变、worker 进程仍存在（`tasklist`）
- [ ] `.agent-teams/mcp.log`、`.agent-teams/daemon.log` 分别存在，无 panic/错误
- [ ] 回归：现有单元测试全过（当前机器只能做 check 级验证）

---

## 7. 非目标（明确排除）

- **worker 进程跨机器**：所有 workers 本地执行，不搞远程
- **worker 热迁移 / HA**：daemon 死 workers 标 dead，不试图复活
- **MCP 主动持续推送**：MCP 仍是被动响应 CC 请求，异步推送继续走 hook + `lead_pending.jsonl`
- **重写 hook**：`scripts/hooks/lead-pending-wake.js` 改动为 0

---

## 8. 实施顺序建议

1. 先做 §4.5 调研（1-2 小时）：确认 Windows 命名管道 + DETACHED_PROCESS 可行
2. 写 daemon 骨架 + IPC 服务端（半天）
3. 重构 MCP 为 relay + IPC 客户端（半天）
4. 迁移 orchestrator 和 agent_loop 到 daemon（几个小时）
5. worker 持久化 + 恢复逻辑（几个小时）
6. 测试（半天）
7. 文档 + 代码审查（1 小时）

**不确定的先做小验证 demo**：一个最小的 daemon（1 个 tool：`echo`）+ 最小的 relay，验证 IPC 通路 + daemon 存活性，再动大的。

---

## 9. 交付物清单

Engineer 完成后需要提交：
- 所有代码变更
- 新增测试
- 更新的 `docs/open-source-deployment.md`（daemon 启动 + 故障处理）
- **简短实施报告**：哪些 §4.5 待研究点选了什么方案、为什么；有没有设计偏离本文档
- 通过全部验收标准的 CI 日志

提交后我（Claude）会做代码审查 + 跑 §5 测试清单验证。
