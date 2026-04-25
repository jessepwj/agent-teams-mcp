# 把 CCteam-creator 改造成 team-mode-mcp 的团队设置 skill — 设计文档

> 目标：基于 CCteam-creator（为 Claude Code 原生 Agent Team 设计的 skill）做一个新版 skill，
> 让它能在本仓库的 **team-mode MCP**（`agent-teams-mcp`）下工作。
>
> 本文档梳理：两边能力差异、必须改造的点、不能直接移植的部分、推荐的新 skill 结构、改造工作清单。

---

## 1. 背景

### 1.1 CCteam-creator 假设的运行环境（Claude Code 原生）

CCteam-creator 完全建立在 Claude Code 内置的多 agent 原语上：

| 原语 | 用途 |
|---|---|
| `TeamCreate(team_name)` | 创建团队 |
| `TaskCreate / TaskList / TaskUpdate / TaskGet` | 共享任务列表，所有 teammate 都能读写、依赖自动解阻塞 |
| `Agent(team_name, name, subagent_type, model, prompt, run_in_background)` | 拉起 teammate（subagent） |
| `SendMessage(to, message)` | 双向点对点消息（lead↔worker、worker↔worker 均可） |
| `subagent_type` | 决定 teammate 的工具面（`general-purpose` / `Explore` / `code-reviewer` / …） |
| Idle 通知 | teammate 每轮结束后自动通知 lead |
| CLAUDE.md 自动注入 | 项目根 CLAUDE.md 在会话启动 / `/compact` 后自动加载 |
| Stop hook | 用于自定义提醒（本仓库借这个机制做了推送） |

### 1.2 team-mode MCP 提供的运行环境

本仓库不是 Claude Code 原生 team，是一个独立的 **MCP 服务 + 后台 daemon**，
让 Claude Code 当 lead，把 worker 拉起为受管 CLI 子进程（claude-code / codex / gemini-cli），
通过文件系统 + Stop hook 实现 worker→lead 的推送。架构对应 README §Architecture。

设计上的根本差异：
- worker 不是 Claude Code subagent，是 **完整的独立 CLI 进程**（codex 是 GPT-5、gemini 是 Gemini）
- 没有共享的 TaskList（TaskCreate 只对 lead 自己可见，worker 看不到）
- worker 之间没有 `SendMessage(to:…)` 工具，只能在 reply 正文里写 `@name` 由 MCP 路由
- 多了一个 web UI（127.0.0.1:8787），人类用户可以以 `user` 身份直接发消息
- 一个项目 **同时只能有 1 个活团队**（README 强调）

---

## 2. team-mode MCP 工具清单（务必明确）

完整签名见 `docs/mcp-tools-reference.md`。下面列**新 skill 直接会用到的字段**：

| # | 工具 | 必填 | 选填 | 关键语义 |
|---|---|---|---|---|
| 1 | `team_create` | `name` | `cwd` | 自动创建 lead 成员、自动开 web UI、清理孤儿团队；slug：`[a-z0-9_.-]{1,64}` 必须以字母/数字开头 |
| 2 | `team_list` | — | — | 列所有团队，含 `ownerStatus: alive / orphan / unbound` |
| 3 | `team_delete` | `name` | — | 优雅关闭所有 worker；返回 `shutdown_failures[]` |
| 4 | `worker_add` | `team`, `name` | `adapter` (`claude-code`\|`codex`\|`gemini-cli`)、`model`、`cwd`、`system_prompt`、`env`、`on_existing` (`reuse`\|`overwrite`\|`error`) | **新建时 adapter 必填**；profile 已存在且想复活/复用必须显式 `on_existing` |
| 5 | `worker_list` | `team` | — | 含 `sessionState`，dead worker 给出 hint |
| 6 | `worker_remove` | `team`, `name` | — | 软删除：进程杀掉，profile 保留 |
| 7 | `send_message` | `team`, `text` | — | **lead → workers**；text 必须含 `@handle`；所有 handle 必须命中活 worker，否则失败 |
| 8 | `inbox_read` | `team` | `limit`, `unread_only`, `auto_ack` | 仅做兜底；正常路径是 Stop hook 自动推送 |

**注意：没有任何"task 任务列表"工具**。CCteam-creator 严重依赖的 TaskCreate/TaskList/TaskUpdate
体系，在 team-mode MCP 下只对 **lead 自己**有效（CC 内置工具，worker 进程是 codex/gemini 时根本拿不到）。

### 2.1 Worker 之间怎么通讯？

通过 **reply 正文里的 `@mention` 自动路由**：
- worker 在自己的输出里写 `@reviewer 请看 src/auth.ts:42`，MCP 解析正文里的 `@reviewer` 就把这条消息送给 reviewer
- lead 永远会自动加入收件人（lead-observability 规则）
- 不再有"自动把上一条消息的 sender 加为收件人"的规则（Bug 12 修复后取消，否则 worker 间会无限互相礼貌回复）

含义：**对 worker 来说，它没有"对谁说话"的工具调用，只有"在我的 reply 里写谁的 @"**。
所以 onboarding prompt 必须教 worker 显式 @ 收件人。

### 2.2 推送机制（决定 onboarding 该怎么写）

```
worker 写 reply → daemon 落盘 messages.jsonl
         → LeadPendingWriter append lead_pending.jsonl
         → 下一次 CC 轮次结束 → Stop hook lead-pending-wake.js
         → {decision:"block", reason:"<reply>"} 注入 <system-reminder>
```

延迟 ~50ms，不需要 lead 主动 `inbox_read`。

---

## 3. CCteam-creator 与 team-mode-mcp 能力对照表

| CCteam-creator 假设 | team-mode-mcp 现状 | 改造方向 |
|---|---|---|
| `TeamCreate(team_name)` | `team_create(name, cwd?)` | 直接替换；多了一个 `cwd` 概念，新 skill 应在 Step 4 询问/推断 |
| `Agent(team_name, name, subagent_type, model, prompt, run_in_background)` | `worker_add(team, name, adapter, model, cwd, system_prompt, env, on_existing)` | **`subagent_type` → `adapter` 是核心改造点**；prompt 通过 `system_prompt` 字段传入而不是 `prompt` 参数 |
| `subagent_type=general-purpose` 给 dev/reviewer 全工具 | `adapter=claude-code` 给完整 CC 工具集 | 软件项目默认 `claude-code` |
| `subagent_type=Explore` 强制只读 | **没有只读 adapter** | 只能用 system_prompt 约束 + role onboarding 强调 |
| `SendMessage(to: <name>, message: …)` 双向、点对点 | lead 用 `send_message` 发；worker 在 reply 文本里写 `@name` 让 MCP 解析路由 | onboarding 必须改：worker"调用 SendMessage" → "在回复正文里 @对方" |
| Worker→worker 直接对话（如 dev 直接喊 reviewer） | 通过 worker reply 里的 `@reviewer` 实现，但 lead 也会收到（lead-observability） | onboarding 提示语不需大改；强调"@reviewer 时 lead 会同步看到" |
| `TaskCreate / TaskList / TaskUpdate` 全员共享 | **不存在**；lead 自己的 TaskCreate 只是本会话的待办，worker 看不到 | 改造关键：放弃"任务认领制"，改用 **`@mention` 直接派单 + `.plans/` 文件作单一事实源** |
| 任务依赖自动解阻塞 (`addBlockedBy`) | 不存在 | 改造：在 `.plans/<project>/task_plan.md` 里手动维护依赖图，lead 手动派单 |
| Idle 通知（teammate 每轮自动通知） | 不存在；改用 Stop hook 推送 worker reply | onboarding 不再依赖 idle 语义；只要 worker 一回话 lead 就会被唤醒 |
| `run_in_background: true` | `worker_add` 默认就是后台 spawn | 不需要参数，删除即可 |
| Plan/Explore 等只读 subagent 类型 | 不存在 | researcher 角色用 `claude-code` adapter + 强 prompt 约束，或用 `gemini-cli`（成本低、适合一次性 query） |
| 一个项目可有多个 team | 一个项目同时只允许 1 个活团队 | Step 0 检测：如果已有活团队，应让用户决定 reuse / delete-and-recreate |
| Plugin 自检 + 版本广告（WebFetch GitHub raw） | 这个 MCP 没有 plugin 体系 | 删除 Step 0 Update Check |
| Snapshot 文件 `team-snapshot.md` 用于复活 | MCP 自带 `members.json` profile 持久化 + `worker_add on_existing=reuse` 复活 | 简化：snapshot 仍然写（lead 用），但 worker 进程的复活靠 MCP 自己；snapshot 主要存"onboarding prompt 原文"以便重建 worker |
| `TaskCreate description` 含 `.plans/` 路径 | lead 仍可以在自己会话里 TaskCreate 当 todo 用 | onboarding 中明确：TaskCreate 仅 lead 自用，worker 的"任务"通过 send_message + .plans/ 路径告知 |

---

## 4. 必须改造的关键点（按改造工作量从大到小）

### 4.1 [大] 任务派发模型从"task 队列认领制"改为"@mention 派单 + 文件事实源"

CCteam-creator 的 `Step 4` 假设：
```
TaskCreate("scope, .plans/path") → TaskUpdate(owner=…) → worker 通过 TaskList 自动看到
```

team-mode-mcp 下必须改成：
```
1. lead 在 .plans/<project>/<agent>/task_plan.md 里写好任务详情
2. lead 调 send_message(team, "@<agent> 你的新任务在 .plans/<project>/<agent>/task-X/task_plan.md，
                                   验收标准是 X，依赖 Y，请确认。")
3. worker 自己读文件、回复确认、开干
4. worker 完工时在 reply 里写 "完成。报告：.plans/.../findings.md。下一步建议 Y。"
```

`TaskCreate` 仍可作为 **lead 自己的待办看板**保留，但**不再是派单通道**。

### 4.2 [大] 角色 → adapter 映射表

CCteam-creator 的角色全部用 `subagent_type: general-purpose` + model 切换。
新 skill 的角色配置必须在 `subagent_type` 那一栏改成 `adapter`：

| 角色 | 推荐 adapter | 推荐 model | 备注 |
|---|---|---|---|
| backend-dev / frontend-dev | `claude-code` | `sonnet`（默认） | 需要文件读写 + bash + 测试运行，必须 claude-code |
| researcher | `claude-code` 或 `gemini-cli` | sonnet / 默认 | 量大、纯检索的研究可以用 gemini-cli 省钱；要写 .plans/ 文件就必须 claude-code |
| reviewer | `claude-code` | sonnet（敏感场景升 opus） | 需要写 .plans/<project>/reviewer/findings.md，必须有写权限 |
| e2e-tester | `claude-code` | sonnet | 需要 Playwright MCP + 文件写权限 |
| custodian | `claude-code` | sonnet | 需要写 docs/index.md、check 脚本 |
| dev (代码实现) | `codex` 也可选 | 默认（GPT-5） | codex 写代码强；用作多 backend 编码对比 |

**新增"backend 选择"维度**：用户在 Step 1 应被询问"是否要用 codex/gemini 做对比/分担"。
这是这个 MCP 相对于原版的**核心增量价值**。

### 4.3 [中] Onboarding prompt 必须重写

需要重写的部分：
- **去掉 SendMessage 工具调用语法**：worker 没有这个工具，告诉它"在你回复正文里写 @reviewer"即可
- **去掉 TaskList / TaskUpdate 协议**：改为"读 `.plans/<project>/<your-name>/task_plan.md` 看任务"
- **去掉 idle 语义**：改为"完成后回复，正文里务必 `@lead`，否则 lead 收不到"——其实 lead-observability 会兜底，但 onboarding 应明确强调，因为 worker LLM 不一定知道
- **adapter 适配的差异**：
  - codex worker 不熟悉 .plans/ 这套约定，prompt 要更详细教
  - gemini-cli worker 没持久会话（每轮重新拉起），prompt 必须自包含
- **worker→worker 协议**：明确"dev 找 reviewer 直接 `@reviewer`，无需经 lead；但 lead 会同步看到"

### 4.4 [中] Step 0 / Step 1 / Step 2 流程调整

| 原 step | 改造 |
|---|---|
| Step 0 Update Check（GitHub plugin 版本对比） | **删除**。本仓库没有 plugin 自更新机制 |
| Step 0 Detect（检查 `.plans/` 是否存在） | **保留 + 增强**：还要调 `team_list` 看是否已有活团队，如有要让用户选 reuse / 杀掉重建 |
| Step 1 Requirements Consultation | 增加一个询问：**"是否需要多 backend 协作（claude-code / codex / gemini-cli）？"**——这是该 MCP 的独有能力 |
| Step 1.3 角色推荐 | 把"参考 agent (subagent_type)"列改成"adapter (model)"列 |
| Step 2 Confirm | 加 `cwd` 字段：worker 默认继承团队 cwd，但用户可能想让某个 worker 工作在其他目录 |

### 4.5 [小] Step 3 文件结构基本不变

`.plans/<project>/...` 这套结构（task_plan / findings / progress / docs / 各 agent 子目录）
是 **backend-agnostic 的**，CCteam-creator 的精华正是这套文件协议，改造时**完全保留**。
唯一要改的是模板里所有提到 `TaskCreate` 的地方都换成"通过 send_message + 文件路径派单"。

### 4.6 [小] Step 4 拉起 agent

```python
# 原版（伪代码）
Agent(team_name="x", name="alice", subagent_type="general-purpose",
      model="sonnet", prompt=onboarding_alice, run_in_background=True)

# 改造后
mcp__team-mode__worker_add(
  team="x", name="alice", adapter="claude-code",
  model="sonnet", system_prompt=onboarding_alice,
  cwd="<可选>", env={...},
  on_existing="error"  # 新建场景
)
```

注意：**第一次创建 worker 之前**先调 `team_create`，且如果项目已有活团队会失败——
要么 reuse（让所有 worker `worker_add on_existing=reuse`），要么先 `team_delete`。

### 4.7 [小] Step 5 `/compact` 警告依然适用

CLAUDE.md 自动注入 + lead-pending 推送都是 CC 机制，跟 MCP 没关系，原版的"压缩后失忆 → 让我读 team-snapshot.md"那段话**完全保留**，只需把恢复指令里 `Agent()` 改成 `worker_add(on_existing=reuse)`。

### 4.8 [小] 删除 / 简化的协议

- 删除：`subagent_type` 工具约束表、Plan/Explore agent 引用
- 简化：Idle Notification、Multi-Select Plan Mode 相关文字
- 保留：3-Strike、2-Action Rule、Phase Gate、Doc-Code Sync、CI Gate、Style Decisions、Custodian、Golden Rules——这套**全部 backend-agnostic**

---

## 5. 不能直接移植 / 需要 workaround

| 能力 | 原版方式 | MCP 现状 | Workaround |
|---|---|---|---|
| 任务依赖自动解阻塞 | `TaskCreate addBlockedBy` | 不存在 | lead 手动盯 `task_plan.md`，被依赖任务完成后再 send_message 派下游 |
| Worker 看共享 task list | `TaskList` 任意 teammate 调用 | 不可能（codex/gemini 没 TaskList 工具） | 全部用 `.plans/<project>/<agent>/task_plan.md` 文件 |
| Read-only 工具约束 | `subagent_type=Explore` | 没只读 adapter | 强 prompt 约束 + 在 onboarding 重复"绝不 Edit/Write 项目源码"，code review 时 reviewer 自查 |
| Worker 间 SendMessage 协议消息 | `SendMessage(to:bob, message:{type:"shutdown_request",…})` | 无结构化消息通道 | 全部走自然语言 + .plans/ 文件 |
| 一个项目多个团队并行 | 支持 | **强制 1 项目 = 1 团队** | 不支持。如果用户真的想多团队，得换 `cwd` 到不同目录 |
| Codex / Gemini 当 lead | n/a | 不支持，只能 Claude Code 当 lead | 在 onboarding 明说 |

---

## 6. 推荐的新 skill 结构

### 6.1 命名 + 位置

- 新 skill 名：`agent-teams-mcp-setup`（或 `team-mode-setup`）
- 放置位置：本仓库内 `skills/agent-teams-mcp-setup/`，让用户 clone 仓库后即可引用
  - 也可以发成独立 plugin/marketplace，但仓库内分发对早期用户最简单
- 与原 CCteam-creator 的关系：**fork + 改造**，保留 CC0 / MIT 来源声明

### 6.2 文件清单

```
skills/agent-teams-mcp-setup/
  SKILL.md                       -- 主流程（参照原版 SKILL.md，改 Step 0/1/4）
  references/
    onboarding.md                -- 重写：去 SendMessage、去 TaskList、加 @mention 协议
    roles.md                     -- 改 adapter 映射表
    templates.md                 -- 改 CLAUDE.md 模板（adapter 列、send_message 章节）
    adapters.md                  -- 新增：claude-code / codex / gemini-cli 各自能力 + 适用场景
  scripts/
    golden_rules.py              -- 原样保留
```

### 6.3 SKILL.md frontmatter 触发词建议

```
TRIGGER: "team-mode 团队"、"agent-teams-mcp"、"用 codex/gemini 做 worker"、
         "多 backend 协作"、"set up worker team"
```

### 6.4 与原 CCteam-creator 的对比卖点

新 skill 应该在介绍时强调：
1. **多 backend**：可以让 alice 用 claude-code、bob 用 codex、carol 用 gemini，不是只能 Claude
2. **真后台进程**：worker 是独立 CLI 子进程，crash 不影响 lead；可以 worker_remove 暂停后再 reuse 复活
3. **web UI**：人类可以以 user 身份直接进群发言（CCteam-creator 没有）
4. **/mcp reconnect 不影响 worker**：因为 daemon detached

---

## 7. 改造工作清单（建议执行顺序）

| # | 任务 | 输出 | 估时 |
|---|---|---|---|
| 1 | 把原版 4 个文件复制到 `skills/agent-teams-mcp-setup/` | 4 文件 | 5 min |
| 2 | 改写 `SKILL.md` 的 Step 0 / 1 / 2 / 4（删 Update Check、加 backend 询问、改工具调用） | 改 ~150 行 | 1h |
| 3 | 重写 `references/roles.md` 中所有角色的 `subagent_type` 列为 `adapter` 列 | 改 ~40 行 | 30 min |
| 4 | 重写 `references/onboarding.md` 的 Common Template — 去 SendMessage、改 @mention 协议、删 idle 语义 | 改 ~80 行 | 1h |
| 5 | 改 `references/templates.md` 的 CLAUDE.md 模板 — Team Roster 表加 adapter 列、Communication 表改 send_message 语法 | 改 ~30 行 | 20 min |
| 6 | 新增 `references/adapters.md` — 三个 backend 各自的能力矩阵（搬 README §Backend matrix）+ 选择决策树 | 新 ~100 行 | 40 min |
| 7 | `team-snapshot.md` 模板 — 仍然保留 onboarding prompt 原文，但加上 adapter / model / cwd / env 字段以便用 worker_add 复活 | 改 ~20 行 | 15 min |
| 8 | 写一个 `examples/` 跑通 demo（3 worker discussion / dev+reviewer 流水线） | 1-2 文件 | 30 min |
| 9 | 在仓库 README 加一段"如何使用这个 skill" + 在 `docs/` 加用户文档 | 改 ~30 行 | 30 min |

**合计约 4-5 小时**。

---

## 8. 待用户确认的设计选择（建议在动工前一起拍板）

1. **skill 命名**：`agent-teams-mcp-setup` / `team-mode-setup` / 其他？
2. **分发方式**：仓库内目录 / 独立 plugin marketplace 仓库 / 两者都做？
3. **TaskCreate 在新 skill 里的定位**：完全不用 / 仅作 lead 私人 todo / 用一种约定方式让 lead 把它的 TaskList 同步到 `.plans/task_plan.md`？
4. **多 backend 询问要不要默认推荐**：要不要在 Step 1 默认推荐 dev=codex + reviewer=claude-code 这样的组合？
5. **保不保留 `team-snapshot.md`**：MCP 已经有 `members.json` 持久化，snapshot 只是给 lead 用的 onboarding 缓存——还是值得保留？
6. **是否要做 plugin 自更新检查**：可以做（仓库内 git pull 检查），也可以彻底删掉以简化流程
7. **是否同时出中文版（参照原版 `cn/` 子目录结构）**：仓库主要面向中文用户，建议直接做中文为主、英文为辅

---

## 9. 风险与限制

- **一个项目只能有一个活团队**：与 CCteam-creator 假设的"可以多团队并行"冲突；新 skill 必须把这点放在 Step 0 的检测里明确告知
- **Codex/Gemini worker 不熟 .plans/ 约定**：他们没受过这个约定的训练，onboarding 必须比 claude-code worker 更啰嗦地教
- **Gemini 无持久会话**：每轮重启，意味着大型多步骤任务（如 dev 实现完整模块）不适合 gemini，新 skill 应在 backend 选择时明确建议
- **Hook 必须装好**：team-mode-mcp 推送依赖 `.claude/settings.json` 里的 Stop hook + 一次完整 CC 重启，新 skill 应在 Step 5 提醒用户
- **Worker 间无限 ping-pong 历史**：Bug 12 已修，但新版 onboarding 仍要提醒"完成后给一个 terminal 句子，不要持续礼貌回复"

---

## 10. 下一步

如果你确认方向 OK，我可以分两批落地：

- **批次 A（最小可用）**：完成第 7 节的 1-5 步，能让用户跑起一个混合 backend 团队
- **批次 B（增强）**：完成 6-9 步，加 adapter 文档、demo、用户文档

需要我现在就开工 A 批次，还是先把第 8 节的 7 个待确认问题逐一过一遍？
