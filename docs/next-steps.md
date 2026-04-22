# Next Steps — 未来会话的接力清单

> 这份文档是给"上下文被压缩后"的未来会话准备的接力信息。
> 写于：2026-04-22，最后一次大改造完成后。

---

## 当前状态快照

- ✅ **MCP 工具面已终定**：7 个（见 [`mcp-tools-reference.md`](./mcp-tools-reference.md)）
- ✅ **lib 单元测试 264 个全绿**
- ✅ **`cargo check --tests` 编译通过**（集成测试未跑）
- ✅ **端到端真机验证通过（上一轮大改造之前的版本）**：team_create → spawn_member → send_message → worker 回复 → shutdown_member 全链路在 Windows + Claude Code + Git Bash 环境跑通
- ⚠️ **最新 7 工具版本的端到端真机验证未跑**：需要用户先 `/mcp` 断开 team-mode，rebuild exe，重连后测试
- ✅ **Claude Code NDJSON stream-json 注入已通过真机验证**（上一版本）

---

## 立即可做的事

### 1. 完成 7 工具版本的端到端真机回归

**前置**：用户在 Claude Code `/mcp` 里断开 team-mode 连接（exe 被占用，否则 `cargo build --bin team_mode_mcp` 链接失败）。

**步骤**：
```bash
cd "E:/aigc内容整理/agent-teams-rs-team-mode"
cargo build --bin team_mode_mcp           # 重 link 新二进制
```
然后用户重连 MCP，调用方（我）执行以下流程：

```
mcp__team-mode__team_create(name="demo", cwd="E:\\aigc内容整理\\agent-teams-rs-team-mode")
mcp__team-mode__worker_add(
  team="demo", name="alice",
  adapter="claude-code",
  system_prompt="You are a tester. Reply in one short sentence.",
  env={}  # CLAUDE_CODE_GIT_BASH_PATH 应自动探测
)
mcp__team-mode__send_message(team="demo", text="@alice 一个简单问题：1+1=?")
# 等 10 秒
ReadMcpResourceTool(uri="team://demo/rooms/main")
# 确认看到 alice 的回复
mcp__team-mode__worker_remove(team="demo", name="alice")
mcp__team-mode__team_delete(name="demo")
```

**成功标志**：alice 在 10-15s 内回复一句话，出现在 room messages 里。

### 2. 跑集成测试

```bash
cargo test --test team_mode_mcp
```
（同样需要先让 exe 释放。预期 10 个 test 全通过。）

---

## 已知小瑕疵 / 可选改进

| 项 | 严重程度 | 说明 |
|---|---|---|
| `team_mode_mcp.exe` 被 MCP 客户端占用，开发时 rebuild 受阻 | 低 | 开发时手动 `/mcp` 断开；或改用 lib + 另一个 MCP shim |
| Gemini 后端未在新工具面下重测 | 低 | 只有 claude-code / codex 真机验证过 |
| `worker_add` 失败时 member 身份文件已创建但进程启动失败 → 可能留下不一致状态 | 低 | 目前靠用户手动 `worker_remove` 清理；可考虑 add 时原子回滚 |
| 集成测试 `tests/team_mode_mcp.rs` 因要求真实 spawn 进程，不 easy 在 CI 环境跑 | 低 | 已经退化为"只测 MCP 接口不测 worker lifecycle"，真实启动由端到端覆盖 |
| 消息 kind 内部仍有 Dispatch/Discussion/Reply 等多种类型，外部不暴露 | 低 | 清理 domain 层 MessageKind 枚举是 follow-up |

---

## 可能的下一轮改造

- **HTTP + SSE MCP 传输**：目前 stdio 每个 Claude Code 实例独立进程，跨实例通知不广播。改 HTTP 后多实例共享一个 team_mode_mcp。但 Claude Code 自己的行为（不把 MCP 推送变成新回合）决定了"跨实例推送"意义有限。
- **External agent 支持**：当前 worker 都是 managed（由 Rust spawn）。external（人类控制的 Claude Code 自行连接 team-mode MCP）需要重新启用 inbox_* 工具给它自取消息。
- **Team.cwd + worker.cwd 在 session 跨机器时的路径迁移**（低优先级）

---

## 关键文件速查

读这几个就能上手：
1. `docs/architecture-background.md` — 全局架构
2. `docs/mcp-tools-reference.md` — MCP 工具/资源详细参考
3. `src/team_mode/mcp/tools.rs` — 工具 handler 实现
4. `src/backend/claude_code.rs` — NDJSON stream-json 注入与 idle 闸门
5. `src/runtime/agent_loop.rs` — worker 消息驱动循环

---

## 环境要点（Windows 本机）

- 项目路径：`E:\aigc内容整理\agent-teams-rs-team-mode`
- Claude CLI：`/c/Users/msi/AppData/Roaming/npm/claude`（version 2.1.112）
- Git Bash：`D:\Git\bin\bash.exe`
- MCP 二进制：`target/debug/team_mode_mcp.exe`
- `.mcp.json` 已配置 team-mode 指向该二进制
- `.team-mode-data/` 是当前项目默认数据目录

---

## 历史改造索引（commit 顺序）

1. **Claude Code stream-json NDJSON 持久进程**：`claude -p "" --input-format stream-json --verbose` + stdin NDJSON + idle 闸门
2. **删冗余工具**：23 → 17（删 8 个重复/无效工具）
3. **删 inbox_*+ 合并 spawn/shutdown**：17 → 9（send_message 替代 room_post_message，spawn_member 合并 profile_set/resume）
4. **极简参数**：9 → 7 工具；lead 变 team 虚拟属性；`worker_add` 统一 4 种使用模式；`send_message` 硬编 sender=lead；`team_create` 加 cwd；`CLAUDE_CODE_GIT_BASH_PATH` 启动时自动探测
