# 开源部署注意事项 / Gotchas

这是**开源用户第一次 clone 仓库后**需要了解的配置坑和平台差异清单。覆盖
各种已知的"为什么跑不起来"场景。

---

## 快速开始

```bash
git clone <repo>
cd <repo>
cargo build --release --bin team_mode_mcp --bin team_mode_daemon
claude                                         # 在项目根启动 Claude Code
```

仓库里已经有：
- `.mcp.json` —— 指向 release 二进制（用 `${CLAUDE_PROJECT_DIR}` 相对路径）
- `.claude/settings.json` —— 配置 Stop hook 实现消息推送
- `scripts/hooks/lead-pending-wake.js` —— hook 脚本

---

## 跨平台坑

### Windows 用户

1. **二进制后缀**：`.mcp.json` 的 command 结尾是 `${EXE_EXT:-.exe}`，默认值
   `.exe` 对 Windows 友好。不用改。
2. **Git-Bash 自动探测**：`team_mode_mcp` 启动时会自动探测
   `D:\Git\bin\bash.exe` / `C:\Program Files\Git\bin\bash.exe` 等路径并设置
   `CLAUDE_CODE_GIT_BASH_PATH`。如果你的 git-bash 不在常见位置，手动设该 env。
3. **Stop hook 的 stdin 在 Windows 有 bug**（[CC Issue #46601](https://github.com/anthropics/claude-code/issues/46601)）：
   hook 脚本有兜底（session_id cooldown 文件），但如果发现推送异常（比如无限循环），
   说明这个 bug 影响到了，留 issue 反馈。
4. **中文路径**：代码走 `PathBuf` + UTF-8，已测 `E:\aigc内容整理\...` 路径正常。

### macOS / Linux 用户

1. **二进制无 `.exe` 后缀**：需要在启动 CC 前 export：
   ```bash
   export EXE_EXT=""
   ```
   或者写到 `~/.bashrc` / `~/.zshrc`。不设会导致 MCP 找不到 `team_mode_mcp.exe`。
2. **`.mcp.json` 里 `${CLAUDE_PROJECT_DIR}` 可能不展开**（CC 实现版本差异）：
   如果 `/mcp` 报 "failed to reconnect"，把 `${CLAUDE_PROJECT_DIR}` 换成实际
   绝对路径或者把 MCP config 搬到 `.claude/settings.json`（那里展开稳定）。
3. **`node` 必须在 PATH**：hook 脚本用 node 跑。

---

## Claude Code 登录方式

### 官方账号 (OAuth via `claude login`)
- 完全支持 ✓
- 未来若接入 Channels（研究预览），需要官方账号

### API key 登录 (`ANTHROPIC_API_KEY` sk-ant-...)
- 完全支持 ✓

### 第三方中转 Token + 自定义 BASE_URL
- **基本推送路径 Stop hook 完全可用** ✓
- **未来 Channels 功能不可用** —— Anthropic 在 2026 年限制了第三方 OAuth token
  使用 Channels，所以如果你用 `ANTHROPIC_BASE_URL=<relay>` + `ANTHROPIC_AUTH_TOKEN=cr_...`
  这种配置，Channels 会被拒（但不影响本项目 Stop hook 推送）

---

## 配置位置规范

### MUST

| 放哪 | 放什么 | 为什么 |
|---|---|---|
| 项目级 `.claude/settings.json` | Stop hook | 只在本项目生效，不污染其他项目 |
| 项目级 `.mcp.json` | team-mode MCP server 配置 | CC 项目级 MCP 发现 |
| `scripts/hooks/lead-pending-wake.js` | hook 脚本 | 随仓库 check-in，开源用户直接用 |

### MUST NOT

| 别放哪 | 别放什么 | 原因 |
|---|---|---|
| 全局 `~/.claude/settings.json` | Stop hook / FileChanged hook for team-mode | 会在所有项目生效，Stop hook 会阻塞非 team-mode 项目的 CC |
| 全局 `~/.claude/settings.json` | 硬编码的二进制绝对路径 | 不可移植；开源时需手改 |

---

## 常见错误与原因

### `/mcp` 显示 "failed to reconnect to team-mode"
- **原因 1**：`target/release/team_mode_mcp(.exe)` 或 `team_mode_daemon(.exe)` 不存在 → `cargo build --release --bin team_mode_mcp --bin team_mode_daemon`
- **原因 2**：路径含中文但 shell 编码不对 → 用 PowerShell 或 Git-Bash
- **原因 3**：`${EXE_EXT:-.exe}` 在 POSIX 展成 `.exe` 但实际文件没后缀 → `export EXE_EXT=""`
- **原因 4**：MCP 进程被占着删不掉 → `tasklist /FI "IMAGENAME eq team_mode_mcp.exe"` 杀掉残留
- **原因 5**：daemon 二进制不在 MCP 同目录 → 设置 `TEAM_MODE_DAEMON_EXE` 为 `team_mode_daemon` 的绝对路径

### MCP 退出后 worker 是否还会继续跑？
- 新架构下 `team_mode_mcp` 是 thin relay，worker 进程由 `team_mode_daemon` 持有。
- Claude Code 关闭 MCP stdin 或 `/mcp` 重连时，只应影响 MCP relay，不应 drop worker session handle。
- 如果要临时回退到旧的 MCP 内执行模式，可设置：
  ```bash
  export TEAM_MODE_DAEMON=0
  ```

### `worker_add` 报 "unknown backend type: claude_code"
- **原因**：adapter 值必须是 `"claude-code"`（**连字符**），不是 `"claude_code"`（下划线）
- **其他合法值**：`"codex"`、`"gemini-cli"`

### `team_create` 报 "Team 'diag' already exists"
- **原因**：上次测试残留在磁盘 `.agent-teams/diag/`
- **修**：`team_delete({"name":"diag"})`，或手动 `rm -rf .agent-teams/diag/`

### Worker 状态显示 "running" 但 `send_message` 后永远没回复
- **应该已修复**。如果仍出现，检查 `.agent-teams/mcp.log` 有无 `send_input failed` 错误
- 相关 bug history 见 `docs/design-decisions.md#bug-journal`

### Push hook 不 surface（lead 收不到消息）
- **检查 `.lead-pending-wake.log`**：
  - 没有新条目 → hook 没被触发。CC 未加载 `.claude/settings.json`？重启 CC 试试
  - 有 `stop: injected N, kept M for peers [ancestors=...], exit 0 (block via JSON)` → hook 成功，消息已注入
  - 有 `stop: waiting up to 1800s [ancestors=...]` 但没 injected → Rust 侧没写 pending（查 mcp.log 看 `posting reply`）
  - 有 `stop: stop_hook_active=true, exit 0` 或 `cooldown active` → 正常防环，等下一 turn
- **检查 `lead_pending.jsonl`**：有行但 hook 不消费？
  - 该行 `owner_cc_pid` 可能不在本 CC 的 ancestor chain 里 → 属于其他 CC 或是历史残留
  - 手动清理：`rm lead_pending.jsonl`
- **验证 MCP 写入**：`tail .agent-teams/mcp.log` 应看到 `posting reply` + `message sent ... kind=Reply recipients=["lead"]`

### Reminder 显示 "Stop hook error:" 前缀
**只在旧版本发生。新版已改 `exit 0 + JSON block` 消除此前缀。** 如果你仍看到，
说明 hook 脚本是老版本：`git pull` 最新仓库。

### 多个 CC 同项目下消息被"错的" CC 吃掉
- Hook 脚本通过 ancestor chain 路由：每条消息只进入创建该 team 的 CC
- 需要进程树查询（Windows PowerShell / Unix ps）可用
- 沙箱环境下如果查询失败，会降级为"所有消息都消费"（保消息不丢，牺牲去重）—— 此时多 CC 会冲突

### Hook 卡住 CC 太久
- 默认 Stop hook 最多等 1800 秒（30 分钟）。太久：
  ```bash
  export TEAM_MODE_STOP_WAIT_SEC=300    # 改 5 分钟
  # 或 7200 改 2 小时（如果 worker 是长任务）
  ```
  然后重启 CC
- **用户按 ESC**：SIGINT → hook 立即 exit 0 让出 prompt

### Worker 永远不 reply（mcp.log 只有 `processing inbox message` 没有 `posting reply`）
- **最可能原因**：worker 子进程被自己的 Stop hook 阻塞
- **已修**：Rust 给 worker 子进程设 `TEAM_MODE_WORKER=1` env，hook 脚本看到就 fast-exit 不 block
- 如果仍出现：确认 `cargo build --release --bin team_mode_mcp` 重新编译过

---

## 与其他 CC Hook 共存

Stop hook 是**同步阻塞**的（最多 30 分钟等待）。如果你的全局 settings 有其他
Stop hook（比如 notification 插件），它们可能：
- **并行执行**：CC 同时启动所有 Stop hook 的子进程，不互相影响 ✓
- **串行执行**：如果 CC 版本选择串行，先执行的 hook 阻塞后面的

**推荐**：让本项目的 Stop hook 是**最后一个**执行。项目级配置一般在全局插件
hook 之后加载，所以天然排序正确。

若发现插件 Stop hook 的通知没触发（说明串行且我们在前）：
- 临时把 `TEAM_MODE_STOP_WAIT_SEC` 改小（5-10s）测试
- 或者把你的项目级 Stop hook 改成 async（脚本需要调整为非阻塞模式）

---

## 数据目录 `.agent-teams/`

| 文件 | 用途 | 是否需要清理 |
|---|---|---|
| `<team>/team.json` | team 元数据（含 owner_cc_pid） | 手动（team_delete） |
| `<team>/members.json` | worker 身份 + execution profile | 手动（team_delete） |
| `<team>/messages.jsonl` | 消息历史 | 手动（team_delete） |
| `<team>/room.json` | 房间元数据 | 手动 |
| `mcp.log` | MCP tracing 日志 | 可定期清（不影响功能） |
| `daemon.log` | daemon tracing 日志 | 可定期清（不影响功能） |
| `runtime/daemon.json` | daemon IPC 端点与 PID | 自动重建 |
| `runtime/workers.json` | worker runtime 状态 sidecar | 自动维护 |
| `.locks/` | 文件锁 | 不要删 |
| `lead_pending.jsonl` | **项目根**（不在 `.agent-teams/` 内！）推送队列 | 自动消费 |
| `.lead-pending-wake.log` | 项目根，hook 执行日志 | 可定期清 |
| `.stop-hook-cooldown` | 项目根，防环 cooldown | 可删，自动重建 |

**重要**：`lead_pending.jsonl` 在 **项目根目录**，不在 `.agent-teams/` 子目录。
Claude Code 的 FileChanged hook matcher 限制（只监视项目根的字面文件名），
决定了这个位置。已加入 `.gitignore`。

---

## 版本要求

- **Rust**：1.85+（edition 2024）
- **Claude Code**：2.1+ 推荐。Stop hook 所需的 `stop_hook_active` 标志和 `asyncRewake`
  在 2.1.x 都存在。
- **Node.js**：hook 脚本跑在 node，任何 LTS 版本（14+）都可。
- **Git-Bash**（Windows）：claude CLI 本身依赖，本项目不额外要求。

---

## 日志定位

| 文件 | 含义 |
|---|---|
| `.agent-teams/mcp.log` | Rust MCP server 的 tracing（spawn / send / reply） |
| `.agent-teams/daemon.log` | 常驻 daemon 的 tracing（tool dispatch / worker lifecycle） |
| `.agent-teams/runtime/workers.json` | worker runtime sidecar；daemon 重启后旧 running/starting 会标为 dead |
| `.lead-pending-wake.log`（项目根） | hook 脚本每次触发的状态 |
| CC 客户端自己的日志 | 见 CC 官方文档 `/status` 命令 |
