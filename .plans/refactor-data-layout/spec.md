# Data Layout Refactor — Authoritative Spec

> **目的**：这份文档是本次大重构（目录结构 + domain 合并 + 工具面优化）的唯一权威参考。所有子智能体开工前必须完整读一遍。实施过程中如发现本文档与代码冲突，**以本文档为准**，并反馈给主协调方。

---

## 1. 新目录结构（最终形态）

```
.agent-teams/                       ← 默认 base_dir（隐藏目录，位于 Lead 工作目录下）
├── README.md                       ← 启动时自动生成，覆盖写；标注 AUTO-GENERATED
├── lead_pending.jsonl              ← Worker→Lead push 队列（跨 team，每行带 team 字段）
├── .locks/                         ← 所有文件锁集中在这里
│   ├── <team>.lock                 ← 每个 team 子目录一把锁
│   └── lead_pending.lock           ← pending 文件锁
└── <team-name>/                    ← 每个 team 一个目录（team.name 作为目录名）
    ├── team.json                   ← Team 元信息（单文件）
    ├── members.json                ← 成员列表（身份+execution 合并，数组），带 version
    ├── room.json                   ← 主房间记录（只有 "main"，单文件）
    └── messages.jsonl              ← 该 team 的消息流（append-only, source of truth）
```

**关键设计**：
- 投影（inbox/thread）**不持久化**。启动时扫 `messages.jsonl` 建内存索引，消息写入时同步更新。
- 旧目录 `.team-mode-data/` 不做迁移。启动时若发现，打 `tracing::warn!` 告知用户自行删除。

---

## 2. 命名常量

```rust
pub const DATA_DIR_DEFAULT: &str = ".agent-teams";
pub const DATA_DIR_LEGACY: &str = ".team-mode-data";

pub const FILE_README: &str = "README.md";
pub const FILE_LEAD_PENDING: &str = "lead_pending.jsonl";
pub const DIR_LOCKS: &str = ".locks";

pub const TEAM_FILE: &str = "team.json";
pub const MEMBERS_FILE: &str = "members.json";
pub const ROOM_FILE: &str = "room.json";
pub const MESSAGES_FILE: &str = "messages.jsonl";

pub const MEMBERS_FILE_VERSION: u32 = 1;
```

---

## 3. Domain 层变化（Phase 2）

### 3.1 `MemberProfile` — 简化

**改动前**：
```rust
pub struct MemberProfile {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub kind: MemberKind,
    pub handle: String,         // 与 name 重复，删除
    pub role_label: String,
    pub role_description: Option<String>,
    pub status: MemberStatus,
    pub joined_at: DateTime<Utc>,
}
```

**改动后**：
```rust
pub struct MemberProfile {
    // 去掉 id —— team_id + name 复合键作为身份
    pub team_id: String,
    pub name: String,                        // 同时承担原 name 和原 handle 职责
    pub kind: MemberKind,
    pub role_label: String,
    pub role_description: Option<String>,
    pub status: MemberStatus,
    pub joined_at: DateTime<Utc>,
}
```

**Lead 约定**：`name = "lead"`（小写）；ALL @mentions 匹配 `name`（对外已一致，内部也统一）。

### 3.2 `ExecutionProfile` — 去掉 member_id

**改动前**：
```rust
pub struct ExecutionProfile {
    pub member_id: String,              // 删除 —— 复合键从 MemberProfile 来
    pub execution_mode: ExecutionMode,
    ...
}
```

**改动后**：
```rust
pub struct ExecutionProfile {
    pub execution_mode: ExecutionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,     // 保留 —— 可以和 name 不同（别名）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_state: Option<ExecutionSessionState>,
}
```

### 3.3 `UnifiedMember` — 持久化形态（新）

`members.json` 整个文件的形态：
```rust
#[derive(Serialize, Deserialize)]
pub struct MembersFile {
    pub version: u32,                   // 当前 1
    pub members: Vec<UnifiedMember>,
}

#[derive(Serialize, Deserialize)]
pub struct UnifiedMember {
    pub kind: MemberKind,               // lead | member
    pub name: String,                   // team 内唯一
    pub status: MemberStatus,           // active/removed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_label: Option<String>,     // worker 默认 "worker"，lead 默认 "lead"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_description: Option<String>,
    pub joined_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionProfile>,
}
```

**状态语义**：
- `worker_remove` → 把 `status` 改 `Removed`，**execution 保留**（让 fast-resume 能找到）
- `worker_add` (no adapter) → 找 status=Removed 的 worker，改回 `Active`，execution 不动
- `worker_add` (with adapter, overwrite) → 覆盖 execution，status=Active
- `team_delete` → 整个 team 目录 `rm -rf`

### 3.4 `Team` — 不变
保持原样。（`id` 字段继续存在，当前等于 name；未来如果引入 id != name 的场景再说。）

---

## 4. Store 层变化（Phase 3）

**共同原则**：
- 新路径：`<base_dir>/<team_id>/<file>`（除 `lead_pending.jsonl`）
- 锁都集中到 `<base_dir>/.locks/` 下
- 对外 API 多数加 `team_id` 参数（因为路径需要知道是哪个 team）
- TeamStore 保持 base_dir 级别（因为它管理 team 目录本身）

### 4.1 TeamStore

```rust
impl TeamStore {
    pub fn save(&self, team: &Team) -> Result<()>;     // 写 <base>/<team.id>/team.json
    pub fn get(&self, team_id: &str) -> Result<Option<Team>>;
    pub fn list(&self) -> Result<Vec<Team>>;            // 扫 <base>/*/team.json
    pub fn delete(&self, team_id: &str) -> Result<()>;  // rm -rf <base>/<team_id>/
}
```

### 4.2 MemberStore

**核心变化**：读改写 `<base>/<team>/members.json` 整文件。

```rust
impl MemberStore {
    pub fn list(&self, team_id: &str) -> Result<Vec<UnifiedMember>>;
    pub fn get(&self, team_id: &str, name: &str) -> Result<Option<UnifiedMember>>;
    pub fn add(&self, team_id: &str, member: UnifiedMember) -> Result<()>;
    pub fn update<F>(&self, team_id: &str, name: &str, f: F) -> Result<()>
        where F: FnOnce(&mut UnifiedMember);
    pub fn remove(&self, team_id: &str, name: &str) -> Result<()>;          // 物理删除（从数组移除）
    pub fn set_status(&self, team_id: &str, name: &str, status: MemberStatus) -> Result<()>;
}
```

**MemberRecord 概念**：保留作为便利类型 — `struct MemberRecord { pub profile: MemberProfile, pub execution: Option<ExecutionProfile> }`。从 UnifiedMember 转来。

### 4.3 RoomStore

```rust
impl RoomStore {
    pub fn save(&self, team_id: &str, room: &Room) -> Result<()>;
    pub fn get(&self, team_id: &str) -> Result<Option<Room>>;          // 只有 main，无需 room_id
    pub fn ensure_main(&self, team_id: &str) -> Result<Room>;
}
```

### 4.4 MessageStore

```rust
impl MessageStore {
    pub fn save(&self, team_id: &str, msg: &Message) -> Result<()>;
    pub fn get(&self, team_id: &str, msg_id: &str) -> Result<Option<Message>>;
    pub fn list_by_room(&self, team_id: &str, room_id: &str) -> Result<Vec<Message>>;
    pub fn update<F>(&self, team_id: &str, msg_id: &str, f: F) -> Result<Option<MessageUpdateResult>>
        where F: FnOnce(&mut Message) -> Result<bool>;
    // transcript_path 返回 <base>/<team>/messages.jsonl
    pub fn transcript_path(&self, team_id: &str) -> PathBuf;
}
```

### 4.5 ProjectionStore → InboxCache

**删除**文件持久化。**新增** `src/team_mode/runtime/inbox_cache.rs`（或 service 层）：

```rust
pub struct InboxCache {
    // Keyed by (team_id, recipient_name) -> sorted Vec<InboxItem>
    inner: Arc<Mutex<HashMap<(String, String), Vec<InboxItem>>>>,
}

impl InboxCache {
    pub fn new() -> Self;
    /// Rebuild from messages.jsonl on startup or when store mutates.
    pub fn rebuild_from_store(&self, team_id: &str, store: &MessageStore) -> Result<()>;
    /// Incremental: call when a new message is saved.
    pub fn on_message_saved(&self, team_id: &str, message: &Message);
    /// Query API:
    pub fn project_inbox(&self, team_id: &str, recipient: &str, thread_id: Option<&str>)
        -> Vec<InboxItem>;
}
```

启动时 TeamModeToolset::new 先遍历所有 team 调 `rebuild_from_store`；MessageService::send 保存后调 `on_message_saved`。

---

## 5. 锁文件集中

所有 `.lock` 文件改为 `<base>/.locks/<stem>.lock`：
- 原 `teams/.lock` → `.locks/teams.lock`
- 原 `members/.lock` → `.locks/members-<team>.lock`（per-team）
- 原 `rooms/.lock` → `.locks/rooms-<team>.lock`
- 原 `messages/.lock` → `.locks/messages-<team>.lock`
- `lead_pending` 已存在：`.locks/lead_pending.lock`

---

## 6. Service 层变化（Phase 4）

### 6.1 `MemberService`

**改动**：`AddMemberRequest`、`UpdateMemberRequest` 去掉 `id` 字段，用 (team_id, name)。

```rust
pub struct AddMemberRequest {
    pub team_id: String,
    pub name: String,
    pub kind: MemberKind,
    pub role_label: String,
    pub role_description: Option<String>,
    pub execution: Option<ExecutionProfile>,
}
// 去掉 handle 字段！
```

`member_service.get(...)` 改为 `get(team_id, name)`。

### 6.2 `MessageService`

**核心改动**：`sender_member_id: String` 语义变为"team 内的 name"。SendMessageRequest 里的 sender 就是 name，team_id 已在 request 里。

内部用 `(team_id, name)` 查 member。lead_pending_writer 的调用保持，但 `lead_member_id` 参数改为 "lead"（统一 name）。

### 6.3 `TeamService::create`

`create(CreateTeamRequest)`：自动创建 lead 成员写入 members.json，同时把 team.lead_member_id 设为 `"lead"`（不再是 `"{team}-lead"`）。

### 6.4 `InboxService`

改为**读 InboxCache**，不再读投影文件。`peek(recipient, Some(team_id), thread_id)` 保持 API 不变。

---

## 7. MCP Tools 变化（Phase 5）

### 7.1 `worker_add` 新增 `on_existing` 参数

```rust
"on_existing": {"type":"string","enum":["reuse","overwrite","error"]}
```

**逻辑**：
```
default = "error"

档案已存在:
  reuse      → 读档案 fast-resume；本次传的 adapter/model 等**忽略**并 warn
  overwrite  → 覆盖 execution；要求 adapter 必传
  error      → 报错："worker '<name>' already exists with execution profile at
                <path>. Pass on_existing=reuse to fast-resume, on_existing=overwrite
                to replace, or choose a different name."

档案不存在:
  reuse      → 报错："no profile to reuse, use on_existing=overwrite with adapter"
  overwrite  → 正常首次创建；adapter 必传
  error      → 正常首次创建；adapter 必传
```

成功返回字段：
```json
{
  "team": "demo",
  "name": "alice",
  "sessionState": "running",
  "mode": "reuse" | "overwrite" | "create"    // 新增：告知调用方实际走了哪条路
}
```

### 7.2 `worker_add` ready-check

spawn 成功后，等第一个 `AgentOutput::TurnComplete` 或 5s 超时：
- 拿到 → sessionState = "running"
- 5s 超时但进程还活 → sessionState = "starting" + warn
- 进程已死 → 报错，回滚 profile

通过 orchestrator 的 `take_output_receiver` 拿 `rx`，先在 worker_add 内 drain 一次，然后再把剩余 rx 传给 AgentLoop。

### 7.3 `send_message` 严格校验

```rust
// 所有 @mentions 必须全部命中活跃 worker；否则报错，错误里列 unmatched + 活跃 worker 清单
if !unmatched.is_empty() {
    return Err("unmatched @mentions: [...]. Active workers: [...]");
}
```

成功返回里额外加字段：`"matched_recipients": ["alice", "bob"]`。

### 7.4 `team_delete` 失败报告

```json
{
  "ok": true,
  "name": "demo",
  "shutdown_failures": []   // 或 [{"member":"alice","reason":"..."}]
}
```

### 7.5 内部 member_id 规则简化

- Lead 的 `id` 概念在 domain 层已经删除
- `compose_member_id` 函数删除
- `lead_member_id(team)` → 返回 `"lead"` 而非 `"{team}-lead"`
- `MessageService` 和其他 service 内部凡用到 "member_id 字符串"的地方，改成 `name`

---

## 8. 新模块 `src/team_mode/data_dir.rs`（Phase 1）

```rust
pub const DEFAULT_NAME: &str = ".agent-teams";
pub const LEGACY_NAME: &str = ".team-mode-data";

pub fn resolve_default_base_dir(cwd: &Path) -> PathBuf {
    let new = cwd.join(DEFAULT_NAME);
    if new.exists() {
        return new;
    }
    let legacy = cwd.join(LEGACY_NAME);
    if legacy.exists() {
        tracing::warn!(
            "Found legacy data directory '{}'. It is NOT read. Delete it and start fresh (new dir will be created at '{}').",
            legacy.display(),
            new.display()
        );
    }
    new
}

pub fn ensure_scaffold(base_dir: &Path) -> Result<()> {
    fs::create_dir_all(base_dir)?;
    fs::create_dir_all(base_dir.join(".locks"))?;
    let readme_path = base_dir.join("README.md");
    let content = render_readme();
    fs::write(&readme_path, content)?;
    Ok(())
}

fn render_readme() -> String {
    // 渲染完整 README，内容见 docs/push-notifications.md 的 README 模板
    // 最后加 "_Generated at {now}_."
    ...
}
```

README 完整内容（硬编码模板）：见 Section 10。

---

## 9. Main 入口变化（Phase 6）

`src/bin/team_mode_mcp.rs`：

```rust
let mut data_dir: Option<PathBuf> = None;
// 解析 --data-dir 参数，有则覆盖
...

let data_dir = data_dir.unwrap_or_else(||
    data_dir::resolve_default_base_dir(&std::env::current_dir().unwrap())
);
data_dir::ensure_scaffold(&data_dir)?;
```

---

## 10. README.md 模板（硬编码于 data_dir.rs）

```markdown
<!-- AUTO-GENERATED by agent-teams-rs — DO NOT EDIT.
     Overwritten on every MCP server startup. -->

# agent-teams data directory

State for the `team_mode_mcp` server running in this project. Lead (you) is
the Claude Code CLI that spawned this MCP; workers are managed subprocesses
coordinated through here.

## Top-level layout

| Path | What it is | Safe to edit? |
|---|---|---|
| `README.md` | this file, auto-generated | regenerated on startup |
| `lead_pending.jsonl` | worker→lead push queue (consumed by Claude Code FileChanged hook) | managed automatically |
| `.locks/` | file locks | never |
| `<team-name>/` | per-team subdirectory, one per team | see below |

## Per-team subdirectory layout

| Path | What it is | Safe to edit? |
|---|---|---|
| `team.json` | team metadata (name, cwd, lead name) | avoid |
| `members.json` | unified member list (identity + execution profile, versioned) | avoid |
| `room.json` | main room record | avoid |
| `messages.jsonl` | append-only message transcript (source of truth) | no — corrupts projections |

Inbox/thread views are NOT persisted. They are rebuilt from
`messages.jsonl` into an in-memory cache at startup and kept in sync
as new messages arrive.

## Want push notifications for worker replies?

See `docs/push-notifications.md` in the agent-teams-rs repo for how to
wire `~/.claude/settings.json` to read `lead_pending.jsonl` via
`FileChanged` + `asyncRewake`.

## Commands

- List teams: MCP tool `team_list`
- Read lead inbox: MCP tool `inbox_read`

_Generated at {{TIMESTAMP}}._
```

---

## 11. 测试策略

- 每个 store 的 unit tests 全部改写以适应新签名
- domain 层的 round-trip 测试更新为新 schema
- tool 层现有测试（list_tools_exposes_minimal_surface、team_create、send_message、worker_add 等）照常更新期望值
- 加新测试：
  - `worker_add` 的 3 种 `on_existing` 路径
  - `send_message` 严格 @mention + unmatched_handles
  - `team_delete` shutdown_failures
  - data_dir 的 `resolve_default_base_dir`、`ensure_scaffold`、README 生成
  - members.json 的 version 字段序列化
- 旧的 `member_id = {team}-{name}` 拼接相关测试全部删除/改写

## 12. 本次不做的事（明确范围）

- 不做 Codex-as-Lead app-server 模式（另起一战）
- 不做 Lead 的其他客户端（Cursor/Cline 等）支持
- 不做 Windows Toast / `install-hooks` 子命令（推到后续）
- 不做多 Lead 协作（多个 Claude Code 同时当一个 team 的 lead）

---

## 13. 实施阶段划分

| Phase | 内容 | 谁做 |
|---|---|---|
| 1 | data_dir 模块 + README 生成器 | 主协调 |
| 2 | Domain 层改造 | 主协调 |
| 3 | 5 个 Store 重写 | 子智能体（sonnet） |
| 4 | Service 层适配 | 主协调 |
| 5 | MCP 工具面 5 项改进 | 子智能体（sonnet） |
| 6 | main 入口 + 集成测试更新 | 主协调 |
| 7 | 全量测试回归 | 主协调 |

**子智能体接到任务时必须先读本 spec**，不得偏离。

---

## 14. 风险 & 兜底

1. **测试回归量大**：预计 ≥30% 测试需要重写。先跑 `cargo check` 看编译错误范围。
2. **InboxCache 引入并发复杂度**：初版用 `Arc<Mutex<...>>` 简单粗暴；性能够用。
3. **TeamStore::list() 扫 `<base>/*/team.json`**：目录遍历，性能够用（team 数 < 10）。
4. **lead_pending.jsonl 跨 team 共享**：每行记录带 `team` 字段，hook 能区分。保持现状不拆分。
