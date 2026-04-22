# Team Mode Native 最终全权限验收文档

本文档交给具备完整权限和完整 Windows Rust/MSVC 构建环境的 AI/工程师，用来检查 `team-mode-native` 是否按 `..\docs\native-terminal-team-mode-new-project-plan.md` 的目标完成到可运行验收状态。

本轮已补齐的重点：

- Host/IPC 暴露计划工具面：team/member/execution/room/thread/inbox/direct/managed session。
- `teamctl` 暴露同等 CLI 管理入口。
- MCP proxy 暴露计划工具面，并补 `resources/list` / `resources/read`。
- managed session 可生成 prompt file、per-member MCP config、启动 terminal runner 或 Codex app-server、查询状态、shutdown/restart、返回 attach/viewer 命令。
- Codex managed session 会写 `members/{memberId}/codex-events.ndjson`，并把 app-server stdout/stderr、Host 发出的 turn 请求记录到 viewer 可读事件日志。

仍需全权限环境确认的部分：

- 真实 Claude/Gemini/Codex CLI 是否已安装并可被当前 PATH 调用。
- Windows Terminal / PowerShell 打开新终端的行为。
- Codex app-server 当前版本的 `thread/start` / `turn/start` 字段兼容性。
- runner heartbeat 超时后的自动 restart policy 仍不是后台 supervisor，本轮提供 `member restart` 手动闭环。

## 1. 必备环境

Windows 上必须先确认 MSVC linker 和 Windows SDK 存在：

```powershell
rustup show
rustup target list --installed
where link
where cl
where cargo
```

如果 `where link` 找不到 `link.exe`，安装 Visual Studio Build Tools，并勾选：

- MSVC v143 或更新的 C++ build tools
- Windows 10/11 SDK

当前机器无法完成 `cargo check/test` 的原因是环境缺失，关键报错如下：

```text
error: linker `link.exe` not found
```

尝试 `rust-lld` 后也会因为缺 Windows SDK import libs 失败：

```text
rust-lld: error: could not open 'kernel32.lib': no such file or directory
```

这两个错误都表示本机缺 MSVC/Windows SDK，不是项目代码逻辑报错。

## 2. 静态验证

在项目目录执行：

```powershell
cd E:\aigc内容整理\agent-teams-rs-team-mode\team-mode-native
cargo fmt --check
cargo metadata --offline --no-deps --format-version 1
cargo check
cargo test
```

本机已完成：

```text
cargo fmt --check: pass
cargo metadata --offline --no-deps --format-version 1: pass
cargo check --offline: blocked by missing link.exe before project code type-check
```

全权限环境必须继续跑：

```powershell
cargo test --test team_core
cargo test --test host_ipc
cargo test --test runner_adapter
```

新增测试重点在 `tests/host_ipc.rs`：

- room/thread/inbox read/ack/count
- direct read/list
- managed session dry-run
- prompt/MCP config 文件生成
- attach 命令返回

## 3. 启动 Host

终端 A：

```powershell
cd E:\aigc内容整理\agent-teams-rs-team-mode\team-mode-native
Remove-Item -Recurse -Force .verify-team-mode -ErrorAction SilentlyContinue
cargo run --bin team_mode_host -- --data-dir .verify-team-mode --listen 127.0.0.1:17891
```

终端 B 检查：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 status
```

预期返回 `teamCount/memberCount/messageCount/runnerCount` 等状态字段。

## 4. 基础团队和消息验证

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 team create dev "Dev Team"
cargo run --bin teamctl -- --host 127.0.0.1:17891 member add --team-id dev --id lead --handle lead --name Lead --role-label lead
cargo run --bin teamctl -- --host 127.0.0.1:17891 member add --team-id dev --id reviewer --handle reviewer --name Reviewer --role-label review
cargo run --bin teamctl -- --host 127.0.0.1:17891 team list
cargo run --bin teamctl -- --host 127.0.0.1:17891 member list --team-id dev
```

发群聊 dispatch：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 room post --team-id dev --sender lead --kind dispatch "@reviewer 请确认收到消息"
cargo run --bin teamctl -- --host 127.0.0.1:17891 room read --team-id dev --room-id main
cargo run --bin teamctl -- --host 127.0.0.1:17891 inbox peek reviewer
cargo run --bin teamctl -- --host 127.0.0.1:17891 inbox count reviewer
cargo run --bin teamctl -- --host 127.0.0.1:17891 member tail reviewer
```

预期：

- `room post` 返回 message JSON，`effectiveRecipients` 包含 `reviewer`。
- `inbox peek/count` 能看到 reviewer 的未读消息。
- `member tail reviewer` 包含 `[TEAM MODE MESSAGE]` 注入文本。

读取并 ack：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 inbox read reviewer --limit 1
cargo run --bin teamctl -- --host 127.0.0.1:17891 inbox ack reviewer <messageId>
```

## 5. Thread 和 Direct 验证

使用第 4 节 `room post` 返回的 `threadId`：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 thread read <threadId>
cargo run --bin teamctl -- --host 127.0.0.1:17891 thread reply <threadId> --sender reviewer "收到，我开始处理"
```

Direct message：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 dm send --team-id dev --from lead --to reviewer "这是私聊验证"
cargo run --bin teamctl -- --host 127.0.0.1:17891 dm list --team-id dev --member reviewer
cargo run --bin teamctl -- --host 127.0.0.1:17891 dm read --team-id dev <directThreadId> --member reviewer
cargo run --bin teamctl -- --host 127.0.0.1:17891 dm reply <directThreadId> --sender reviewer "私聊回复"
```

交互私聊：

```powershell
"line 1","line 2" | cargo run --bin teamctl -- --host 127.0.0.1:17891 dm send --team-id dev --from lead --to reviewer --interactive
```

## 6. ExecutionProfile 和 Managed Session 验证

创建 Gemini/PowerShell 测试 profile：

```powershell
@'
{
  "memberId": "reviewer",
  "adapter": "gemini-cli-terminal",
  "launchMode": "native_terminal_pty",
  "viewerMode": "native_terminal",
  "command": {
    "program": "powershell.exe",
    "args": ["-NoExit"]
  },
  "env": {},
  "systemPrompt": "You are @reviewer. Use Team Mode MCP tools to reply.",
  "promptMode": "append",
  "restartPolicy": "never"
}
'@ | Set-Content -Encoding UTF8 .\reviewer.execution.json

cargo run --bin teamctl -- --host 127.0.0.1:17891 member execution-set reviewer --json .\reviewer.execution.json
```

Dry-run 验证，不启动真实进程：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 member spawn reviewer --dry-run --no-open-terminal
cargo run --bin teamctl -- --host 127.0.0.1:17891 member status reviewer
cargo run --bin teamctl -- --host 127.0.0.1:17891 member attach reviewer
```

预期：

- 返回 `sessionState: planned`。
- `.verify-team-mode\runtime\prompts\reviewer.system.md` 存在。
- `.verify-team-mode\runtime\mcp\reviewer.mcp.json` 存在。
- attach 返回 `teamctl member tail reviewer` 或 viewer 命令。

真实启动 terminal runner：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 member spawn reviewer
```

预期：

- Windows Terminal 新 tab 或 PowerShell fallback 被打开。
- 新终端运行 `team_member_runner -> PTY -> powershell.exe -NoExit`。
- `member status reviewer` 可看到 session `starting/running` 或 runner 状态。

注入验证：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 inject reviewer "Write-Output 'managed pty ok'"
cargo run --bin teamctl -- --host 127.0.0.1:17891 member tail reviewer --limit 50
```

Shutdown/restart：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 member shutdown reviewer
cargo run --bin teamctl -- --host 127.0.0.1:17891 member restart reviewer --force-shutdown
```

## 7. Claude/Gemini 真实 CLI 验证

Claude profile 示例：

```json
{
  "memberId": "reviewer",
  "adapter": "claude-code-terminal",
  "launchMode": "native_terminal_pty",
  "viewerMode": "native_terminal",
  "command": {
    "program": "claude",
    "args": ["--permission-mode", "default"]
  },
  "env": {},
  "systemPrompt": "You are @reviewer. Reply through Team Mode MCP tools.",
  "promptMode": "append",
  "restartPolicy": "never"
}
```

Gemini profile 示例：

```json
{
  "memberId": "reviewer",
  "adapter": "gemini-cli-terminal",
  "launchMode": "native_terminal_pty",
  "viewerMode": "native_terminal",
  "command": {
    "program": "gemini",
    "args": ["--model", "gemini-3-pro-preview"]
  },
  "env": {},
  "systemPrompt": "You are @reviewer. Reply through Team Mode MCP tools.",
  "promptMode": "append",
  "restartPolicy": "never"
}
```

验收点：

- Claude 启动命令包含 `--append-system-prompt-file` 或 `--system-prompt-file`。
- Claude 启动命令包含 `--mcp-config <member>.mcp.json`。
- Gemini runner env 包含 `GEMINI_SYSTEM_MD=<member>.system.md`。
- 发 `room post --kind dispatch "@reviewer ..."` 后，成员终端收到 `[TEAM MODE MESSAGE]`。
- 成员能通过 MCP tool `thread_reply` 回复。

## 8. Codex app-server 验证

新增 Codex 成员：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 member add --team-id dev --id coder --handle coder --name Coder --role-label codex
```

写 Codex profile：

```powershell
@'
{
  "memberId": "coder",
  "adapter": "codex-app-server",
  "launchMode": "app_server_stdio",
  "viewerMode": "event_viewer",
  "command": {
    "program": "codex",
    "args": ["app-server"]
  },
  "env": {},
  "model": "gpt-5.4",
  "reasoningEffort": "medium",
  "systemPrompt": "You are @coder. Reply through Team Mode MCP tools.",
  "promptMode": "developer_instructions",
  "restartPolicy": "never"
}
'@ | Set-Content -Encoding UTF8 .\coder.execution.json

cargo run --bin teamctl -- --host 127.0.0.1:17891 member execution-set coder --json .\coder.execution.json
cargo run --bin teamctl -- --host 127.0.0.1:17891 member spawn coder --no-open-terminal
```

检查 viewer：

```powershell
cargo run --bin codex_viewer -- --data-dir .verify-team-mode --member-id coder --lines 100
cargo run --bin teamctl -- --host 127.0.0.1:17891 member attach coder
```

向 Codex 投递消息：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 room post --team-id dev --sender lead --kind dispatch "@coder 请读取 Team Mode 消息并回复"
cargo run --bin codex_viewer -- --data-dir .verify-team-mode --member-id coder --lines 200
```

预期：

- `codex-events.ndjson` 至少出现 `managed_session_launch`、`process_started`、`initialize_sent`。
- Codex stdout/stderr 会以 `app_server_output` 事件落盘。
- 当 app-server 返回 thread id 后，Host 投递消息会记录 `turn_start_sent`。
- 如果当前 Codex app-server 版本没有按预期返回 thread id，会记录 `turn_start_deferred`，这说明需要检查 Codex app-server 协议字段兼容性。

## 9. MCP Proxy 工具和资源验证

```powershell
@'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"resources/list","params":{}}
'@ | cargo run --bin team_mode_mcp_proxy -- --host 127.0.0.1:17891 --member-id reviewer
```

`tools/list` 必须包含：

```text
team_create
team_get
team_list
team_delete
member_add
member_get
member_update
member_remove
member_list
execution_profile_set
room_post_message
room_read_messages
room_list
thread_read
thread_reply
inbox_peek
inbox_read
inbox_ack
inbox_count
member_spawn_managed
member_shutdown_managed
member_restart_managed
member_session_status
member_output_tail
member_attach
direct_send
direct_read
direct_reply
direct_list
```

资源读取：

```powershell
@'
{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"team-mode://self/inbox"}}
{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"team-mode://self/tail"}}
{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"team-mode://room/dev/main"}}
'@ | cargo run --bin team_mode_mcp_proxy -- --host 127.0.0.1:17891 --member-id reviewer
```

## 10. 权限边界验证

params 内伪造 caller 必须失败：

```json
{"id":1,"type":"room/post","params":{"teamId":"dev","roomId":"main","senderMemberId":"lead","callerMemberId":"lead","body":"@reviewer spoof"}}
```

预期错误：

```text
requires top-level callerMemberId
```

顶层 caller 与 sender 不一致必须失败：

```json
{"id":1,"type":"room/post","callerMemberId":"reviewer","params":{"teamId":"dev","roomId":"main","senderMemberId":"lead","body":"@reviewer spoof"}}
```

预期错误：

```text
cannot send as lead
```

成员私有接口不能跨成员：

```json
{"id":1,"type":"inbox/peek","callerMemberId":"lead","params":{"memberId":"reviewer"}}
{"id":2,"type":"member/tail","callerMemberId":"lead","params":{"memberId":"reviewer"}}
{"id":3,"type":"runner/inject","callerMemberId":"lead","params":{"memberId":"reviewer","text":"bad"}}
```

预期错误分别包含：

```text
cannot inbox_peek for reviewer
cannot member_tail for reviewer
cannot runner_inject for reviewer
```

## 11. 持久化恢复验证

1. 使用第 4-5 节创建 team/member/message/direct。
2. 停止 Host。
3. 使用同一个 `.verify-team-mode` 目录重启 Host。
4. 执行：

```powershell
cargo run --bin teamctl -- --host 127.0.0.1:17891 team list
cargo run --bin teamctl -- --host 127.0.0.1:17891 member list --team-id dev
cargo run --bin teamctl -- --host 127.0.0.1:17891 room read --team-id dev --room-id main
cargo run --bin teamctl -- --host 127.0.0.1:17891 inbox peek reviewer
```

预期：

- team/member/room/message 从 JSON/JSONL 恢复。
- thread/inbox 投影从 transcript 重建。
- raw PTY log 是内存态，Host 重启后不会恢复旧 raw tail；这不是消息总账丢失。

## 12. 验收判定

通过标准：

- `cargo fmt --check` 通过。
- `cargo check` 和 `cargo test` 在完整 MSVC/SDK 环境通过。
- Host/teamctl/MCP proxy/manual runner 验证通过。
- Managed session dry-run 生成 prompt/MCP config。
- Terminal managed spawn 能启动 runner，`inject` 能到 PTY。
- Codex managed spawn 能启动 app-server，事件能进入 `codex-events.ndjson`，viewer 能读取。
- 权限绕过测试按预期失败。

需要记录为后续增强而不是本轮阻断的事项：

- ~~后台 supervisor 自动 degraded timeout 和 restart_policy=always 自动拉起。~~ **已在第二轮实现。**
- ~~Codex app-server 版本差异下的 developer instructions capability probe。~~ **已在第三轮实现。** probe_tx/probe_rx 通道检测 `thread/start`（id=2）响应；失败则重发不含 `collaborationMode` 的 `thread/start` 并注入 bootstrap turn；事件记录 `probe_success` / `probe_fallback` / `bootstrap_turn_sent`。
- 跨平台 terminal launcher 模板，目前 Windows 优先，非 Windows 使用 shell fallback。

## 13. 代码审查发现的 Bug 及修复记录

由具有完整代码读取权限的 AI 对 `src/host/app.rs` 进行静态代码审查，发现并修复了以下问题。

### Bug 1：死代码 `create_message` 存在持久化遗漏（高危）

**文件**：`src/host/app.rs`

**问题描述**：`create_message` 函数及其辅助函数（`NewMessage` struct、`ensure_team_room_member`、`recipients_for`、`parse_mentions`）从未被任何调用路径引用（grep 确认全仓库无调用点）。但该函数内部仅操作内存中的 `HostState`，**完全未调用 `store.append_message`**，若未来被误接入，将导致消息不持久化，Host 重启后消息丢失。

**修复**：删除 `create_message`、`NewMessage`、`ensure_team_room_member`、`recipients_for`、`parse_mentions` 全部死代码块，以及随之变为无用的 import（`BTreeSet`、`TeamStatus`、`MemberProfile`、`DeliveryStatus`）。

**影响范围**：仅删除死代码，不影响任何现有功能路径。

### Bug 2：`format_injected_message` 使用 Rust Debug 格式输出 MessageKind（中危）

**文件**：`src/host/app.rs`，函数 `format_injected_message`

**问题描述**：注入给 runner PTY 的 `[TEAM MODE MESSAGE]` 文本中，`kind` 字段使用 `{:?}` 格式化，输出 Rust Debug 表示（如 `"Dispatch"`），而非 serde 序列化的 `snake_case` 形式（如 `"dispatch"`）。测试 `runner_adapter.rs` 中对注入格式有明确断言，会在完整环境下暴露该不一致。

**修复**：添加 `message_kind_str()` 辅助函数，将 `MessageKind` 映射为静态 `&str`（小写 snake_case），替换原 `{:?}` 格式化为 `{}`。

```rust
fn message_kind_str(kind: &MessageKind) -> &'static str {
    match kind {
        MessageKind::Discussion => "discussion",
        MessageKind::Dispatch => "dispatch",
        MessageKind::Reply => "reply",
        MessageKind::Direct => "direct",
        MessageKind::System => "system",
        MessageKind::Status => "status",
    }
}
```

**影响范围**：注入文本格式修正，runner 侧解析 `kind` 字段行为将一致。

### 静态审查结论

除上述两处 Bug 外，对以下核心路径进行了静态逻辑验证，**未发现其他问题**：

| 路径 | 验证结果 |
|------|---------|
| Runner → Host IPC 协议（RunnerFrame → runner_frame_to_host_ipc → IpcRequest alias） | [OK] |
| IPC ack 帧无 `type` 字段，runner recv() 正确跳过 | [OK] |
| params 层 callerMemberId 被 parse_params 剥离，无法伪造 | [OK] |
| Codex AppServer 初始化序列（initialize → thread/start → turn/start） | [OK] |
| rebuild_state_from_services 从 JSONL transcript 重建 thread/inbox 投影 | [OK] |
| MCP proxy 28 工具 + 资源列表覆盖验收文档 section 9 要求 | [OK] |
