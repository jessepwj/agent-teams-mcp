# 原始终端优先的 Team Mode 新项目实施文档

> 状态：Draft v1  
> 目标读者：准备在新文件夹中新建项目的实现者  
> 当前仓库参考：`agent-teams-rs-team-mode`  
> 核心决策：Claude Code / Gemini CLI 走原始终端常驻；Codex 走官方 `codex app-server`；MCP/Host 作为团队协作中心。

---

## 1. 最终目标

新项目要做的是一个**跨 CLI 的团队协作中心**：

1. 用户通过一个 MCP 工具或本地 CLI 创建团队。
2. Host 根据成员配置启动多个 AI CLI 成员。
3. Claude Code 和 Gemini CLI 以原始交互式终端方式常驻，用户能直接看到完整执行过程，也能手动接管。
4. Codex 使用官方优先的 `codex app-server` 常驻方式，由 Host 控制并提供 viewer 查看执行细节。
5. 成员之间通过 Team Mode 的 room/thread/direct message 协议沟通。
6. 发消息由 MCP tool 完成；收消息由 Host hook 主动注入到对应成员会话。
7. 用户可以查看群聊、查看任意成员终端、查看 raw log、并和某个成员私聊。

一句话：

**MCP/Host 是中心；Claude/Gemini 是可见终端成员；Codex 是 app-server 成员；消息协议统一，CLI 启动方式按适配器分开。**

---

## 2. 关键判断

### 2.1 为什么不能只靠 MCP push

MCP tools 很适合让 agent 主动调用：

- 发群聊消息
- 回复 thread
- ack inbox
- 读取 team state

但普通 MCP server 不能稳定保证“有新消息时把空闲中的模型唤醒”。Claude Channels 接近这个能力，但它是 Claude 专属，并且仍是 research preview。

因此 v1 不应依赖 MCP push 唤醒所有 CLI，而应采用：

```text
room_post_message / direct_send
  -> Host 写入消息总账
  -> Host 计算接收成员
  -> Host 触发 on_message_delivered hook
  -> adapter.inject_message(member, message)
  -> Claude/Gemini runner 写入终端 PTY
  -> Codex adapter 调用 turn/start 或 turn/steer
```

### 2.2 为什么仍然需要 MCP/Host 作为中心

三个 CLI 的启动和控制能力并不一致：

| CLI | 常驻方式 | system prompt/角色注入 | 外部发消息 | 查看方式 |
| --- | --- | --- | --- | --- |
| Claude Code | 原始交互终端 | `--system-prompt-file` 或 `--append-system-prompt-file` | runner 写入 PTY；可选 Channels | 原始终端直接看 |
| Gemini CLI | 原始交互终端 | `GEMINI_SYSTEM_MD=<file>` | runner 写入 PTY；可选 ACP | 原始终端直接看 |
| Codex | `codex app-server` | app-server `developer_instructions`/首轮角色注入，需要版本 probe | JSON-RPC `turn/start`/`turn/steer` | Host viewer 渲染事件 |

所以外层必须统一为：

```text
TeamModeHost
  - team/member/room/thread/direct message
  - execution profile
  - lifecycle supervisor
  - local runner IPC
  - MCP server
```

内层按 CLI 写 adapter。

---

## 3. 新项目推荐目录

建议新项目仍用 Rust + Tokio。原因是当前仓库已经有可复用 Rust 代码，且 Windows 下做进程、PTY、stdio JSON-RPC、MCP server 都比较合适。

```text
team-mode-native/
  Cargo.toml
  docs/
    native-terminal-team-mode-new-project-plan.md
  src/
    main.rs
    bin/
      team_mode_host.rs
      team_mode_mcp_proxy.rs
      team_member_runner.rs
      teamctl.rs
      codex_viewer.rs
    domain/
      team.rs
      member.rs
      room.rs
      message.rs
      inbox.rs
      thread.rs
      execution.rs
    storage/
      mod.rs
      team_store.rs
      member_store.rs
      message_store.rs
      projection_store.rs
      session_store.rs
    service/
      team_service.rs
      member_service.rs
      message_service.rs
      inbox_service.rs
      thread_service.rs
      direct_service.rs
      execution_service.rs
    mcp/
      runtime.rs
      tools.rs
      resources.rs
      schemas.rs
    host/
      app.rs
      event_bus.rs
      supervisor.rs
      router.rs
      local_ipc.rs
      terminal_launcher.rs
    runner/
      pty_bridge.rs
      child_process.rs
      control_client.rs
      output_log.rs
      input_injector.rs
    adapters/
      mod.rs
      claude_terminal.rs
      gemini_terminal.rs
      codex_app_server.rs
    viewer/
      member_log.rs
      room_tail.rs
      codex_events.rs
```

---

## 4. 核心组件设计

### 4.1 TeamModeHost

Host 是长期运行的中心进程。

职责：

1. 启动 MCP server。
2. 维护 team/member/room/thread/direct message。
3. 保存 `ExecutionProfile`。
4. 启动和监督成员会话。
5. 监听 runner 连接。
6. 将消息注入到对应成员。
7. 捕获成员输出事件并写 raw log。
8. 提供 `teamctl`/viewer 查询接口。

Host 对外暴露两类接口：

1. MCP stdio/HTTP：给 AI CLI 调用 Team Mode tools。
2. Local IPC：给 runner、viewer、`teamctl` 使用。

v1 推荐 Local IPC 用 localhost NDJSON TCP：

```text
127.0.0.1:{port}
auth: TEAM_MODE_RUNNER_TOKEN
frame: one JSON object per line
```

不要一开始就把 runner protocol 也做成 MCP。runner 是 Host 的内部执行通道，保持简单更好。

### 4.2 TeamModeMcpProxy

每个成员 CLI 都需要能调用 Team Mode MCP tools，但不要让每个成员的 `--mcp-config` 都启动一个完整 Host。

推荐做一个很薄的 stdio MCP proxy：

```text
Claude/Gemini/Codex MCP client
  -> team_mode_mcp_proxy --member-id reviewer --host 127.0.0.1:17891 --token ...
      -> TeamModeHost Local IPC
```

MCP proxy 职责：

1. 实现 MCP stdio framing。
2. 将 `tools/list` / `tools/call` / `resources/read` 转发给 Host。
3. 自动带上 `caller_member_id`，让 Host 知道是谁在发消息。
4. 不持有团队状态，不启动 CLI，不做调度。

per-member MCP config 示例：

```json
{
  "mcpServers": {
    "team-mode": {
      "command": "team_mode_mcp_proxy",
      "args": [
        "--host", "127.0.0.1:17891",
        "--member-id", "reviewer",
        "--token-env", "TEAM_MODE_RUNNER_TOKEN"
      ]
    }
  }
}
```

这样一个 Host 可以服务多个终端成员，每个成员通过自己的 proxy 身份进入同一个团队中心。

### 4.3 TeamMemberRunner

这是实现“原始终端常驻 + Host 可发消息”的关键。

不要让 Host 直接启动 `claude` 或 `gemini` 到一个不可控的外部 terminal。更稳的方式是：

```text
Windows Terminal / PowerShell 窗口
  -> team_member_runner --member-id alice --host 127.0.0.1:...
      -> portable-pty / ConPTY
          -> claude / gemini 原始 CLI
```

Runner 做 5 件事：

1. 在真实终端窗口里运行。
2. 用 PTY 启动原始 CLI。
3. 把用户键盘输入转发给 CLI。
4. 把 CLI 输出同时写到终端屏幕和 Host raw log。
5. 接收 Host 的 `inject_input`，写入 CLI 的 PTY stdin。

这样用户仍然看到的是原始 Claude/Gemini 交互界面，但 Host 也能把消息塞进去。

### 4.4 CodexAppServerAdapter

Codex 不走原始 TUI。理由是官方 app-server 是更稳定的常驻和程序化控制入口。

流程：

```text
Host
  -> spawn: codex app-server
  -> initialize
  -> initialized
  -> thread/start
  -> turn/start
  -> read events:
       item/agentMessage/delta
       item/completed
       turn/completed
       item/commandExecution/outputDelta
```

Codex viewer 可以单独开终端：

```text
teamctl member attach @codex-reviewer
```

或：

```text
team-mode-codex-viewer --member-id codex-reviewer
```

viewer 不控制 Codex，只订阅 Host 里的 Codex event log。

---

## 5. 启动命令设计

### 5.1 Claude Code terminal adapter

推荐默认使用 append，而不是完全替换 Claude Code 的默认系统提示词：

```powershell
claude `
  --name "tm-architect" `
  --append-system-prompt-file "E:\team-mode\runtime\prompts\architect.system.md" `
  --mcp-config "E:\team-mode\runtime\mcp\architect.mcp.json" `
  --permission-mode "default"
```

如果用户明确要求完全替换：

```powershell
claude `
  --name "tm-architect" `
  --system-prompt-file "E:\team-mode\runtime\prompts\architect.system.md" `
  --mcp-config "E:\team-mode\runtime\mcp\architect.mcp.json"
```

Host 生成的 prompt 文件不是 `CLAUDE.md`，而是运行时文件：

```text
{data_dir}/runtime/prompts/{member_id}.system.md
```

Claude 官方参考：

- CLI reference: https://code.claude.com/docs/en/cli-usage
- Channels 可作为 Claude 专属增强: https://code.claude.com/docs/en/channels

### 5.2 Gemini CLI terminal adapter

Gemini 推荐通过环境变量替换 system prompt：

```powershell
$env:GEMINI_SYSTEM_MD = "E:\team-mode\runtime\prompts\reviewer.system.md"
gemini --model gemini-3-pro-preview --approval-mode auto_edit
```

或者在 runner 里设置 env 后启动：

```text
program = "gemini"
args = ["--model", "gemini-3-pro-preview", "--approval-mode", "auto_edit"]
env.GEMINI_SYSTEM_MD = "{prompt_file}"
```

这也不是 `GEMINI.md`，而是 Host 生成的 per-member system prompt 文件。

Gemini 官方参考：

- Configuration / `GEMINI_SYSTEM_MD`: https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md
- ACP mode 可作为后续程序化增强: https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/acp-mode.md

### 5.3 Codex app-server adapter

启动：

```powershell
codex app-server -c model="gpt-5.4" -c model_reasoning_effort="medium"
```

Host 负责 JSON-RPC：

```json
{"id":1,"method":"initialize","params":{"clientInfo":{"name":"team-mode-host","version":"0.1.0"}}}
{"method":"initialized"}
{"id":2,"method":"thread/start","params":{"cwd":"E:\\repo"}}
{"id":3,"method":"turn/start","params":{"threadId":"...","input":[{"type":"text","text":"..."}]}}
```

角色注入策略：

1. 优先使用 app-server 支持的 `collaborationMode.settings.developer_instructions` 或等价字段。
2. 启动时做 capability/probe：如果字段被当前 Codex 拒绝，降级为首轮 bootstrap prompt。
3. 文档和 UI 中明确标注：Codex CLI 没有 Claude 那种 `--system-prompt-file` 原生命令，严格意义上的 system prompt flag 不同。

Codex 官方参考：

- App Server: https://developers.openai.com/codex/app-server
- CLI reference: https://developers.openai.com/codex/cli/reference
- MCP: https://developers.openai.com/codex/mcp

Codex 也应该配置 Team Mode MCP proxy。推荐两条路都支持：

1. **标准路**：Codex 通过 MCP proxy 主动调用 `thread_reply` / `room_post_message`。
2. **兜底路**：因为 app-server 输出是结构化事件，Host 可以在“本次 turn 来源于某条 Team Mode 消息”时，把最终 agent message 自动写回对应 thread，并在消息 metadata 里标记 `source=codex_auto_reply`。

兜底路只给 Codex 用，不要拿它解析 Claude/Gemini TUI 输出。

---

## 6. ExecutionProfile

新项目里角色定义只存在 Host 数据中，不依赖 `AGENTS.md`、`CLAUDE.md` 或 `GEMINI.md`。

建议模型：

```rust
pub struct ExecutionProfile {
    pub member_id: String,
    pub adapter: AdapterKind,
    pub launch_mode: LaunchMode,
    pub viewer_mode: ViewerMode,
    pub command: CommandSpec,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub system_prompt: String,
    pub prompt_mode: PromptMode,
    pub mcp_config: Option<PathBuf>,
    pub restart_policy: RestartPolicy,
}

pub enum AdapterKind {
    ClaudeCodeTerminal,
    GeminiCliTerminal,
    CodexAppServer,
}

pub enum LaunchMode {
    NativeTerminalPty,
    AppServerStdio,
}

pub enum ViewerMode {
    NativeTerminal,
    EventViewer,
}

pub enum PromptMode {
    Append,
    Replace,
    DeveloperInstructions,
    BootstrapTurn,
}
```

配置示例：

```yaml
team:
  id: dev-core
  name: Dev Core

members:
  - id: architect
    handle: architect
    name: Architect
    role_label: architecture
    adapter: claude-code-terminal
    model: claude-sonnet-4-6
    prompt_mode: append
    cwd: E:\workspace\project
    system_prompt: |
      You are @architect, responsible for architecture review.
      Use Team Mode MCP tools to reply to team messages.

  - id: reviewer
    handle: reviewer
    name: Reviewer
    role_label: code-review
    adapter: gemini-cli-terminal
    model: gemini-3-pro-preview
    cwd: E:\workspace\project
    system_prompt: |
      You are @reviewer, responsible for broad code review.

  - id: coder
    handle: coder
    name: Codex Coder
    role_label: implementation
    adapter: codex-app-server
    model: gpt-5.4
    reasoning_effort: medium
    cwd: E:\workspace\project
    system_prompt: |
      You are @coder, responsible for implementation.
```

---

## 7. 消息协议

### 7.1 统一消息流

所有消息都进入 transcript。Inbox 只是投影，不是主事实。

```text
Message
  id
  team_id
  room_id
  thread_id
  sender_member_id
  kind: discussion | dispatch | reply | direct | system | status
  body
  mentions
  visibility
  effective_recipients
  delivery_status
  created_at
```

### 7.2 群聊派工

正式派工只认 `@handle`。

```text
@reviewer 请检查 src/auth 的鉴权逻辑
```

Host 解析：

```text
body -> mentions -> effective_recipients -> inbox projection -> inject
```

### 7.3 私聊

私聊不要绕过消息系统。它应当是一种 direct thread：

```text
direct thread:
  participants = [user, member_id]
  visibility = [user, member_id]
```

用户命令：

```powershell
teamctl dm reviewer "你先不用看测试，先判断架构风险。"
teamctl dm reviewer --interactive
```

Host 做：

```text
direct_send
  -> write message(kind=direct)
  -> inject to reviewer
  -> reviewer replies through direct_reply/thread_reply MCP tool
```

### 7.4 注入到 CLI 的消息格式

注入文本要稳定、机器可读、对模型明确：

```text
[TEAM MODE MESSAGE]
message_id: msg_123
team: dev-core
room: main
thread: th_456
from: @lead
kind: dispatch

@reviewer 请检查 src/auth 的鉴权逻辑

Reply by using the Team Mode MCP tool `thread_reply` for this thread.
[/TEAM MODE MESSAGE]
```

私聊：

```text
[TEAM MODE DIRECT MESSAGE]
message_id: msg_789
thread: dm_user_reviewer
from: user

你先不用看测试，先判断架构风险。

Reply by using `direct_reply` or `thread_reply`.
[/TEAM MODE DIRECT MESSAGE]
```

---

## 8. 常驻与保活

### 8.1 Host supervisor

Host 要维护成员状态机：

```text
Configured
  -> Starting
  -> Running
  -> Busy
  -> Idle
  -> Degraded
  -> Stopped
  -> Failed
```

对 Claude/Gemini terminal runner：

1. runner 每 2 秒 heartbeat。
2. runner 报告 child PID、terminal title、last output time。
3. Host 如果 10 秒没有 heartbeat，标记 `Degraded`。
4. 如果 restart policy 是 `always`，重新打开终端并启动 runner。

对 Codex app-server：

1. Host 直接持有 child process。
2. stdout reader 断开则标记 `Failed`。
3. 可用 `thread/resume` 恢复 thread。

### 8.2 Terminal 不等于进程不可控

“原始终端”不能理解为 Host 完全放手。正确实现是：

```text
Host 控制 runner
runner 控制 PTY child
用户通过 terminal 直接操作 runner/child
```

这样既可见，又可注入，又能保活。

### 8.3 Windows 终端启动

Windows 优先：

```powershell
wt.exe new-tab --title "tm @reviewer" powershell.exe -NoExit -Command `
  "team-member-runner --member-id reviewer --host 127.0.0.1:17891 --token <token>"
```

没有 Windows Terminal 时 fallback：

```powershell
Start-Process powershell.exe -ArgumentList @(
  "-NoExit",
  "-Command",
  "team-member-runner --member-id reviewer --host 127.0.0.1:17891 --token <token>"
)
```

macOS/Linux 后续用 terminal command template：

```yaml
terminal_launcher:
  windows: 'wt.exe new-tab --title "{title}" powershell.exe -NoExit -Command "{command}"'
  macos: 'osascript ...'
  linux: 'gnome-terminal -- bash -lc "{command}"'
```

---

## 9. Runner IPC 协议

Runner 连接 Host 后发送：

```json
{"type":"runner/hello","member_id":"reviewer","runner_id":"run_123","pid":12345}
```

Runner 心跳：

```json
{"type":"runner/heartbeat","member_id":"reviewer","child_pid":23456,"state":"running"}
```

Runner 输出：

```json
{"type":"runner/output","member_id":"reviewer","stream":"pty","data":"...","ts":"..."}
```

Host 注入：

```json
{"type":"host/inject_input","message_id":"msg_123","text":"[TEAM MODE MESSAGE]..."}
```

Runner 确认：

```json
{"type":"runner/input_injected","message_id":"msg_123","ok":true}
```

Runner 子进程退出：

```json
{"type":"runner/child_exit","member_id":"reviewer","exit_code":0}
```

---

## 10. MCP tools

新项目可以直接沿用当前仓库的工具面，并补上 direct/viewer 能力。

### 10.1 Team / Member

```text
team_create
team_get
team_list
team_delete

member_add
member_update
member_remove
member_list
member_get
execution_profile_set
```

### 10.2 Room / Thread / Inbox

```text
room_post_message
room_read_messages
room_list

thread_read
thread_reply

inbox_peek
inbox_read
inbox_ack
inbox_count
```

### 10.3 Managed session

```text
member_spawn_managed
member_shutdown_managed
member_restart_managed
member_session_status
member_output_tail
member_attach
```

### 10.4 Direct message

```text
direct_send
direct_read
direct_reply
direct_list
```

`member_attach` 不一定真的附着到已有 PTY；v1 可以实现为：

1. 如果是 Claude/Gemini terminal member，聚焦或新开 viewer tail。
2. 如果是 Codex app-server member，打开 `codex_viewer`。
3. 如果终端不可控，则返回 raw log 路径和最近输出。

---

## 11. 可以复用和借鉴的现有代码

### 11.1 建议直接复制后改名的模块

当前仓库已经实现的 transcript-first Team Mode 很有价值，建议迁移：

```text
src/team_mode/domain/*
src/team_mode/storage/*
src/team_mode/service/*
src/team_mode/mcp/schemas.rs
src/team_mode/mcp/resources.rs
src/team_mode/mcp/runtime.rs
```

参考文件：

- `src/team_mode/domain/member.rs`：`MemberProfile` / `ExecutionProfile`
- `src/team_mode/service/message_service.rs`：`@handle` 解析、投递语义、thread 约束
- `src/team_mode/service/inbox_service.rs`：inbox projection/read/ack
- `src/team_mode/service/thread_service.rs`：thread read/reply
- `src/team_mode/mcp/runtime.rs`：stdio MCP JSON-RPC runtime
- `src/team_mode/mcp/tools.rs`：当前工具面

### 11.2 建议借鉴但不要照搬的模块

```text
src/runtime/orchestrator.rs
src/runtime/session_registry.rs
src/runtime/agent_loop.rs
```

原因：

1. 当前 `RuntimeOrchestrator` 已经有 session registry 和 spawn/shutdown/status 思路，可以借鉴。
2. 新项目需要改成 `Host -> Runner/AppServer` 的 session 模型。
3. 当前 `AgentLoop` 是 polling inbox，再喂给 backend；新项目应改成事件驱动 hook，但测试里的 mock session 思路可复用。

### 11.3 Codex 可以大量复用

当前 Codex app-server 代码与新方案一致度最高：

```text
src/backend/codex.rs
src/backend/codex_protocol.rs
```

可复用内容：

1. `codex app-server` 启动。
2. initialize/initialized/thread/start/turn/start。
3. stdout reader。
4. Codex event -> AgentOutput 映射。
5. shutdown/kill_on_drop。

需要补的内容：

1. `turn/steer`。
2. `turn/interrupt`。
3. thread resume。
4. developer instructions/system role probe。
5. Codex event viewer。

### 11.4 Claude/Gemini 后端不要照搬

当前：

```text
src/backend/claude_code.rs
src/backend/gemini.rs
```

不适合直接搬到新项目作为主路径。

原因：

1. `claude_code.rs` 走 `claude -p --resume`，不是原始终端常驻。
2. `gemini.rs` 是 one-shot CLI，不是原始终端常驻。

可借鉴：

1. CLI path 查找。
2. env/cwd/model 参数构造。
3. output channel 事件抽象。
4. spawn/shutdown 错误处理。

新实现要改为：

```text
ClaudeTerminalAdapter -> TerminalRunner -> claude interactive
GeminiTerminalAdapter -> TerminalRunner -> gemini interactive
```

### 11.5 不建议复用的旧模块

```text
src/task/*
src/consensus/*
src/checkpoint/*
src/tui/*
src/messaging/*
src/team/*
plugin/*
```

这些都围绕旧 task/workflow/Claude-compatible inbox，不适合作为新项目核心。

---

## 12. 实施阶段

### Phase 0：新项目脚手架

目标：建一个干净的新 repo。

工作：

1. 新建 Rust workspace。
2. 添加基础依赖：`tokio`、`serde`、`serde_json`、`clap`、`tracing`、`thiserror`、`uuid`、`chrono`。
3. 添加 PTY 依赖：优先调查 `portable-pty` 在 Windows ConPTY 下的表现。
4. 添加本地 IPC：先用 `tokio::net::TcpListener` + NDJSON。
5. 建立 `team_mode_host`、`team_mode_mcp_proxy`、`team_member_runner`、`teamctl` 四个 bin。

完成标准：

```powershell
cargo run --bin team_mode_host -- --data-dir .team-mode
cargo run --bin team_mode_mcp_proxy -- --host 127.0.0.1:17891 --member-id lead
cargo run --bin teamctl -- status
```

### Phase 1：迁移 Team Mode 核心协议

目标：先不启动任何 CLI，仅跑通团队和消息。

工作：

1. 迁移 `domain/storage/service/mcp`。
2. 补 `direct_service`。
3. 补 `execution_service`。
4. 写端到端测试：建队、加人、发 `@member`、inbox、thread reply、direct send。

完成标准：

```text
不用 CLI，也能完整跑通 room/thread/direct/inbox。
```

### Phase 2：实现 runner 和 PTY bridge

目标：在终端中运行一个可被 Host 注入输入的原始 CLI 子进程。

工作：

1. `team_member_runner` 连接 Host。
2. runner 创建 PTY。
3. runner 启动一个测试命令，例如 PowerShell 或 `cmd`。
4. 用户输入能到 child。
5. child 输出能显示到终端并写给 Host。
6. Host 能发送 `inject_input`，runner 写入 child stdin。

完成标准：

```powershell
teamctl inject reviewer "echo hello"
```

能在 reviewer 终端看到并执行。

### Phase 3：Claude terminal adapter

目标：让 Claude Code 以原始终端方式成为 managed member。

工作：

1. Host 生成 prompt 文件。
2. Host 生成 per-member MCP config。
3. Host 用 terminal launcher 打开 runner。
4. runner 根据 profile 启动 `claude ...`。
5. `room_post_message` 派工后，Host 注入消息。
6. Claude 通过 Team Mode MCP tool 回复 thread。

完成标准：

```text
用户能看到 Claude 终端。
Host 能向 Claude 注入消息。
Claude 能调用 MCP thread_reply。
```

### Phase 4：Gemini terminal adapter

目标：让 Gemini CLI 以原始终端方式成为 managed member。

工作：

1. Host 生成 system prompt 文件。
2. runner 启动 Gemini 前设置 `GEMINI_SYSTEM_MD`。
3. 启动 `gemini --model ...`。
4. 注入 Team Mode 消息。
5. Gemini 通过 MCP tool 回复。

完成标准同 Claude。

### Phase 5：Codex app-server adapter

目标：让 Codex 以官方 app-server 方式成为 managed member。

工作：

1. 迁移并改造 `src/backend/codex.rs`。
2. 支持 `thread/start`、`thread/resume`。
3. 支持 `turn/start`、`turn/steer`、`turn/interrupt`。
4. 角色注入做 capability probe。
5. Codex 事件写入 `member_event_log`。
6. `codex_viewer`/`teamctl member attach` 可以看事件流。

完成标准：

```text
Host 能给 Codex 发 direct/dispatch 消息。
Codex app-server 能持续响应。
用户能看到 Codex 的事件流和最终回复。
```

### Phase 6：保活与恢复

目标：成员常态存活。

工作：

1. runner heartbeat。
2. app-server heartbeat/process status。
3. session store。
4. restart policy。
5. Host 重启后 runner reconnect。
6. 用户关闭终端后的状态更新。

完成标准：

```text
关闭某个成员终端后，Host 能感知。
restart_policy=always 时能重新拉起。
Host 重启后，存活 runner 能重新注册。
```

### Phase 7：查看与私聊

目标：用户可以随时观察和单独沟通。

工作：

1. `teamctl room tail`
2. `teamctl member list`
3. `teamctl member status <member>`
4. `teamctl member tail <member>`
5. `teamctl member attach <member>`
6. `teamctl dm <member> "message"`
7. `teamctl dm <member> --interactive`

完成标准：

```text
用户无需进 MCP client，也能用 teamctl 管团队。
```

---

## 13. 最小可用版本范围

MVP 只做这些：

1. 一个 Host。
2. 一个 Local IPC。
3. Claude terminal runner。
4. Codex app-server adapter。
5. Team/Member/Room/Thread/Direct。
6. `teamctl dm` 和 `room_post_message`。

Gemini 可以作为第二个 terminal adapter 补进来，因为它与 Claude terminal runner 大部分逻辑共用。

不进 MVP：

1. DAG。
2. 自动任务拆分。
3. 复杂多 agent 调度。
4. 权限系统。
5. Web UI。
6. 云端同步。

---

## 14. 验收清单

### 14.1 启动

- [ ] `team_mode_host` 能启动。
- [ ] `member_spawn_managed` 能打开 Claude terminal。
- [ ] `member_spawn_managed` 能打开 Gemini terminal。
- [ ] `member_spawn_managed` 能启动 Codex app-server。
- [ ] Host 能记录 member pid/runner id/session id。

### 14.2 角色定义

- [ ] Claude 使用 Host 生成的 prompt file。
- [ ] Gemini 使用 Host 生成的 `GEMINI_SYSTEM_MD` file。
- [ ] Codex 使用 developer instructions 或 bootstrap turn，并记录降级原因。
- [ ] 不依赖 `AGENTS.md`、`CLAUDE.md`、`GEMINI.md`。

### 14.3 沟通

- [ ] `room_post_message` 能解析 `@handle`。
- [ ] `dispatch` 无有效 `@handle` 会失败。
- [ ] Host 能注入消息到 Claude/Gemini terminal。
- [ ] Host 能发送消息到 Codex app-server。
- [ ] agent 能通过 MCP `thread_reply` 回复。
- [ ] direct message 独立可查。

### 14.4 查看

- [ ] 可以查看 room transcript。
- [ ] 可以 tail 任意 member raw output。
- [ ] 可以打开/聚焦 member terminal。
- [ ] Codex 有可读 viewer。

### 14.5 保活

- [ ] runner heartbeat。
- [ ] Codex process health check。
- [ ] 终端关闭可感知。
- [ ] restart policy 可配置。
- [ ] Host 重启后可恢复或标记状态。

---

## 15. 最大风险

### 风险一：外部终端直接跑 CLI，Host 无法注入

不要让 terminal 直接运行：

```text
claude ...
```

而要运行：

```text
team_member_runner -> PTY -> claude ...
```

否则 Host 想给成员发消息只能依赖键盘自动化或 terminal 私有 API，Windows 下会非常脆。

### 风险二：从 TUI 输出里解析 agent 回复

不要把 stdout/TUI 输出解析成正式消息。正式消息必须由 agent 调用 MCP tool 写入 Host。

raw output 只用于查看和调试。

例外：Codex app-server 的事件是结构化协议输出，不是 TUI 文本。它可以做 `codex_auto_reply` 兜底，但仍应优先让 Codex 使用 Team Mode MCP tool 主动回复。

### 风险三：Codex system prompt 语义不等同 Claude

Codex CLI 没有 Claude 那种明确 `--system-prompt-file` flag。实现上要：

1. 优先 app-server developer instructions。
2. 启动时 probe。
3. 不支持时降级 bootstrap turn。
4. UI/日志明确标注这是 Codex 的角色注入方式，不叫 Claude 式 system prompt flag。

### 风险四：忙碌状态下强行注入

Terminal TUI 的 busy/idle 不一定可结构化识别。v1 可以先允许注入进入 PTY 缓冲，但必须记录：

```text
message_id -> injected_at -> runner_ack
```

后续再做每个 CLI 的 idle detector。

---

## 16. 推荐实现顺序

最稳顺序：

1. 先迁移当前仓库的 Team Mode 消息核心。
2. 再做 runner PTY bridge，不碰真实 AI CLI。
3. 用 PowerShell/cmd 验证注入。
4. 接 Claude terminal。
5. 接 Codex app-server。
6. 补 direct message 和 viewer。
7. 接 Gemini terminal。
8. 做保活和恢复。

原因：

```text
消息中心正确
  -> runner 可控
  -> 单个真实 CLI 可跑
  -> 多 CLI 扩展
```

这样每一步都有清楚的验收点。

---

## 17. 最终建议

新项目不要再从“旧 agent teams workflow”出发，而应从这三个核心对象出发：

1. `TeamModeHost`
2. `TeamMemberRunner`
3. `Adapter`

具体判断：

1. Claude/Gemini：原始终端体验优先，但必须通过 runner/PTY 托管。
2. Codex：官方 app-server 优先，再补 viewer。
3. MCP：是团队协作协议入口，不是所有进程控制的唯一通道。
4. 消息：transcript-first，inbox/thread/direct 都是投影。
5. 角色：统一存在 `ExecutionProfile.system_prompt`，再由 adapter 映射到各 CLI。

这条路线既保留“终端里看得见、能手动接管”的体验，也保留了 Host 对多 agent 团队的控制力。
