---
name: agent-teams-mcp-setup
description: >
  在本仓库的 team-mode MCP（agent-teams-mcp）下，搭建一支 AI 团队（默认全员 codex，
  可选 claude-code 替代），并落盘文件式协作框架（.plans/ 目录、CLAUDE.md + AGENTS.md、
  Phase Gate、Doc-Code Sync、Custodian、Golden Rules）。
  适用场景：(1) 用户要求"搭建团队/swarm/多 agent 项目"；(2) 用户输入
  /agent-teams-mcp-setup；(3) 多模块并行开发（前后端同时推进）；(4) 需要 Codex
  GPT-5 编码能力 + 可选 Claude 对比；(5) 长期运行的复杂项目需要可恢复的进度记录。
  本 skill 不会自动调用 MCP；它会先与用户对齐需求、推荐角色和 backend 组合，
  得到确认后再创建 .plans/ 文件、调用 team_create + worker_add 拉起团队。
  **重要**：你（team-lead）必须亲自 Read 全部 reference 文件，不要委托给 subagent
  ——subagent 只返回摘要，会丢失关键细节，导致 onboarding prompt 残缺。
  **触发词**：team / swarm / 团队 / 多 agent / agent-teams-mcp / team-mode / codex 协作
  / set up project / 搭建项目。
---

# Team Project Setup（team-mode MCP 版）

在 team-mode MCP 框架下搭建多 agent 团队，并用文件协议持久化协作进度。

> **基础设施前提**：本 skill 假设 `agent-teams-mcp` 已经按仓库 README 安装好
> （`bash scripts/setup.sh` 跑完、`.mcp.json` 已生成、`.claude/settings.json` Stop hook
> 已加载、Claude Code 已重启）。如果 `/mcp` 列表里看不到 `team-mode` 已连接，先回去
> 完成 README §Installation 再调用本 skill。

## 前置阅读（开 Step 1 之前必须做）

team-lead（你）必须直接 Read 全部 reference 文件到自己的 context：

```
Read references/onboarding.md
Read references/roles.md
Read references/templates.md
Read references/adapters.md
```

不要委托给 subagent。subagent 只回摘要，会丢 onboarding 模板和角色边界细节。

## 总流程

**Step 0 检测（自动）**：检查是否已有活团队 / `.plans/` 目录，决定 reuse 还是新建
**Step 1 需求咨询**：介绍 team-mode 工作机制、收集用户需求、推荐 backend 组合
**Step 2 方案确认**：用 AskUserQuestion 让用户确认项目名/角色/phase/backend
**Step 3 创建 .plans/ 文件结构 + 项目 CLAUDE.md**
**Step 3.5 Harness Setup**：拷贝 golden_rules.py + 准备 CI 骨架（仅可测试代码项目）
**Step 4 调用 team_create + worker_add 拉起团队 + 写 team-snapshot.md**
**Step 5 引导用户 /compact**

---

## Step 0 检测：判断是新建还是恢复

在做任何动作前先确认两件事：

### 0.1 检查是否已有活团队（关键约束：1 项目 = 1 活团队）

调 `mcp__team-mode__team_list`，检查返回里有没有 `ownerStatus: alive` 的团队。

- **有活团队，且就是当前项目的**（看 cwd 字段对得上） → 直接进入"恢复模式"
- **有活团队，但不是这个项目** → 告诉用户："检测到当前项目已有活团队 `<name>`，team-mode-mcp 限制 1 项目 1 活团队。是删除重建还是直接 reuse？"
- **有 orphan 团队**（owner CC 已死） → 提醒用户调 `team_delete` 清理后再继续
- **没有团队** → 进入"新建模式"

### 0.2 检查 .plans/ 是否存在

```bash
ls .plans/ 2>/dev/null
```

- **存在** → 读项目 CLAUDE.md（已自动注入）+ 列子目录，问用户："发现已有项目 `<name>`，roster 是 [...]。恢复还是新建？"
- **恢复**：
  1. 读 `.plans/<project>/team-snapshot.md` 拿到 onboarding prompt 缓存
  2. 对每个 worker 调 `worker_add(name=..., on_existing="reuse")` 复活进程（如果进程已死，会自动 `revived_from_dead: true` 重开新会话）
  3. 复活后让每个 worker 读自己的 task_plan.md 和 progress.md 接续工作
- **不存在** → 进入 Step 1

> **注意**：本 skill 不做 plugin 自更新检查（没有 plugin marketplace），跳过 CCteam-creator 原版的 Update Check 步骤。

---

## Step 1：需求咨询（先聊清楚，再动手）

**目标**：让用户理解 team-mode MCP 跟 Claude Code 原生 team 的差异，同时把项目需求摸清楚。不要急着创建任何文件或团队。

### 1.1 介绍 team-mode 工作机制

用自然口吻向用户介绍以下要点（不要照念，按上下文调整）：

**什么是 team-mode 团队**：
- 你（Claude Code）是 team-lead，是主对话/控制平面，**不是被拉起的 agent**
- worker 是 **独立的 CLI 子进程**，**默认 `codex`**（GPT-5 + danger-full-access sandbox），可选 `claude-code`
- 不同于 Claude 原生 team，这里 worker 不共享 Claude 的 thinking block，也不共享 TaskList
- 每个 worker 有自己的工作目录（`.plans/<project>/<worker-name>/`），任务/findings/进度都落盘
- 通过 Stop hook 推送：worker 一回话，下一轮你（lead）就会自动收到 `<system-reminder>`，无需主动 `inbox_read`
- 有 **Web UI**（http://127.0.0.1:8787）能看实时群聊，人类用户也能进群以 `user` 身份发言

**适合的场景**：
- 多模块并行开发（前后端同时推进）
- 多 phase 工作（research → 实现 → 测试 → review）
- 需要 GPT-5 编码 + 可选 Claude 对比 / 升 opus 做安全 review
- 大代码库需要 review 和质量保障

**不适合**：
- 单文件改动 / 小 bug 修复（单 agent 更快）
- 任务只需 1 个角色

**协作方式**：
- team-lead（你）负责对齐需求、拆任务、维护主 plan、决策 phase 转换
- worker 之间可以直接对话（`@reviewer 请看 src/auth.ts`），但 lead 会自动同步收到（lead-observability 规则）
- 任务通过文件（`.plans/<agent>/task_plan.md`）+ `send_message` 通知双轨派发
- worker 完成后自动通过 Stop hook 推送给 lead

### 1.2 收集用户需求

通过聊天逐步了解（不要一次问完）：

1. **工作语言**：观察用户语言。中文用户 → CLAUDE.md / AGENTS.md / onboarding 都用中文
2. **任务类型**：软件开发 / 研究分析 / 内容创作 / 数据处理 / 混合？决定标准角色是否适用
3. **目标和验收标准**：项目想达成什么？deliverable 是什么？
4. **项目状态**：从零开始还是已有代码？已有的工具/技术栈/资源？
5. **用户参与度**：是否每个决策都要参与？还是希望团队自治？
6. **特殊要求**：领域规范、质量标准、时间限制、约束
7. **质量优先级**：除了"代码能跑"还看重什么？产品深度 / 视觉打磨 / 性能 / API 设计 / 测试覆盖。这些会变成 Review Dimensions
8. **Backend 偏好**：默认全员 codex，是否需要某些角色换 claude-code？（沙盒敏感 / 想用 Claude 推理 / 用 opus 做安全 review 等）

详细 backend 选择决策见 [references/adapters.md](references/adapters.md)。

### 1.3 推荐团队配置

基于需求推荐角色和 backend 组合。每个角色解释为什么推荐。

**软件开发标准角色**（非软件项目按本 skill 框架原则改造，见下文）：

| 角色 | 默认 adapter | 替代方案 | 核心能力 |
|------|------------|---------|---------|
| Backend Dev | `codex` | `claude-code` | 写代码 + TDD + 大任务拆 task 子目录 |
| Frontend Dev | `codex` | `claude-code` | 同上 |
| Researcher | `codex` | `claude-code` | 代码搜索 + Web 研究；只读 |
| E2E Tester | `codex` | `claude-code`（启 dev server / 浏览器子进程命中沙盒时换） | Playwright + 浏览器自动化 |
| Code Reviewer | `codex` | `claude-code`（安全敏感升 opus） | 只读 review；写自己的 review 报告 |
| Custodian | `codex` | `claude-code` | 合规审计 + 文档治理 + check 脚本 |

> **本仓库默认全员 `codex`**（GPT-5）。codex worker 已配齐完整 MCP 工具集（Read/Edit/Bash/Grep/Web/Playwright 等）。
> 仅在以下情况换 adapter：(1) 用户主动要求 Claude；(2) 命中 codex sandbox 限制（长进程 / Docker / 跨目录写）；(3) 想做 backend 对比实验。

**adapter 要点**：
- `codex`（默认）：GPT-5、写代码强、`danger-full-access` sandbox（**仍有边界**：长进程 / Docker / 跨目录写易被回收，详见 adapters.md §codex 沙盒坑）
- `claude-code`：无沙盒、可升 opus、Windows 需 `CLAUDE_CODE_GIT_BASH_PATH`

详见 [references/adapters.md](references/adapters.md)。

**模型默认**：codex 走 GPT-5；换 claude-code 时默认 `sonnet`，仅在用户要求高质量 / 成本不敏感 / 关键逻辑时升 `opus`。

详细角色定义见 [references/roles.md](references/roles.md)。

**推荐原则**：
- 角色不是越多越好，按实际需求选
- 小项目可能只需 1 dev + 1 researcher
- 大项目可上完整角色集
- **researcher 多实例**：研究量大可拆多个 researcher 并行（量分 / 方向分），各自独立子目录，无竞态
- **custodian 推荐 4+ agent 或长期项目**用，小团队（2-3 agent）lead 自己吸收合规检查
- 用户可加自定义角色（需指定：name、adapter、model、责任）

**team-mode 独有的非软件项目**：
- 框架（task 子目录、findings.md、progress.md、3-Strike、phase gate、context recovery）通用
- 角色名和职责按实际工作改：创作分离评审、研究并行、质量门和验证分离

### 1.4 用户可定制项

明确告知用户可调整：

- **角色组合**：选哪些角色、不要哪些
- **自定义角色**：标准角色不够时新增
- **任务 phase**：项目分几个 phase、每个 phase 目标
- **技术决策**：技术栈、框架、编码规范
- **Review 严格度**：是否需要 code review / security review
- **Backend 组合**：每个角色用哪个 backend（默认 codex；可选 claude-code）

team-lead = 主对话（你），**不要给 lead 自己生成 worker**。

如果用户是改进 **已有团队系统** 而不是从头建，明确判断：
- 仅本项目 → 改项目文档
- 通用协议改进（角色边界、onboarding、CLAUDE.md 结构、文件约定） → 先改 `agent-teams-mcp-setup` skill 源文件

不要在团队活跃时立即重建——先把模板改回来、找 phase 边界再重建。

---

## Step 2：方案确认

充分讨论后，用 AskUserQuestion 让用户最终确认：

- **项目名**：短、ASCII、kebab-case（`chatr`、`data-pipeline`）
- **简短描述**：1-2 句
- **角色清单 + 每个角色的 backend/model**
- **Phase 计划**：项目主要步骤的初步划分
- **Review Dimensions**：3-5 个项目质量维度（每个有 weight + STRONG/WEAK 锚定）

只有用户确认后才进入创建步骤。

---

## Step 3：创建 .plans/ 文件结构

详见 [references/templates.md](references/templates.md)。

### 目录结构

```
.plans/<project>/
  task_plan.md                -- 主 plan（精简导航图）
  findings.md                 -- 团队级摘要
  progress.md                 -- 工作日志
  decisions.md                -- 架构决策日志
  team-snapshot.md            -- 团队 onboarding 缓存（Step 4 末尾生成）
  docs/                       -- 项目知识库
    index.md                  -- 导航图（custodian 维护）
    architecture.md
    api-contracts.md
    invariants.md
  archive/                    -- 归档历史

  <agent-name>/               -- 每个 worker 一个目录
    task_plan.md              -- 该 worker 的任务清单
    findings.md               -- INDEX（不堆内容）
    progress.md               -- 工作日志
    <prefix>-<task>/          -- 任务子目录
      task_plan.md / findings.md / progress.md
```

### 任务子目录规则

| 角色 | 前缀 | 例子 |
|------|------|------|
| backend-dev / frontend-dev | `task-` | `task-auth/`、`task-payments/` |
| researcher | `research-` | `research-tech-stack/` |
| e2e-tester | `test-` | `test-auth-flow/` |
| reviewer | `review-` | `review-auth-module/` |
| custodian | `audit-` | `audit-phase1-compliance/` |

简单 bug 修复 / 配置改动可以直接在根文件记，不必开子目录。

---

## Step 3.5：生成项目 CLAUDE.md **和** AGENTS.md

⚠️ **关键**：本仓库默认 worker 是 codex，**codex 读 `AGENTS.md` 不读 `CLAUDE.md`**。
team-lead（你）是 Claude Code，读 `CLAUDE.md`。所以**两个文件都要生成**，内容一致。

### 生成两个文件（内容完全相同）

在项目根目录（不是 `.plans/` 内）创建或追加：

1. `CLAUDE.md` —— Claude Code（lead）会自动注入主会话 context
2. `AGENTS.md` —— codex worker 启动时会读取

**两个文件内容应保持一致**。最简单做法：

```bash
# 先按模板生成 CLAUDE.md
# 然后复制
cp CLAUDE.md AGENTS.md
```

或在 Step 5 末尾加一行验证：`diff CLAUDE.md AGENTS.md` 应该无输出。

模板见 [references/templates.md](references/templates.md)。**根据实际选定的角色动态填**：
- 只列 Step 2 确认的角色
- 填项目名、目录路径、每个 worker 的 adapter
- 包含自定义角色（如有）

### 已有 CLAUDE.md / AGENTS.md 时

**追加** team operations 段落（加分隔符 `---`），不要覆盖。两个文件都追加。

### 后续维护

任何时候你（lead）更新 CLAUDE.md 的团队相关段落（roster、protocol、Known Pitfalls 等），
**必须同步更新 AGENTS.md**。否则 codex worker 拿到的项目上下文会陈旧。

> **可选**：用符号链接 `ln -s CLAUDE.md AGENTS.md` 让两者天然同步。
> 但 Windows 上需要管理员权限或开发者模式，且 git diff 会显示两个文件——视团队偏好选

### docs/index.md

Step 3 同时创建 `docs/index.md`——动态导航图，custodian 维护。详见 templates.md。

---

## Step 3.6：Harness Setup（如有可测代码）

仅适用有可测代码（backend / frontend / 两者）的项目。

### Golden Rules

复制本 skill 的 `scripts/golden_rules.py` 到项目：

```bash
cp <skill-path>/scripts/golden_rules.py <project>/scripts/golden_rules.py
```

然后在 copy 的文件底部配置 `SRC_DIRS` 匹配项目源码目录。

5 个内置检查：
- **GR-1 文件大小**：>800 行 WARN，>1200 行 FAIL
- **GR-2 硬编码 secret**：API key/token/password 正则扫描
- **GR-3 console.log**：生产代码里的 console.log
- **GR-4 文档新鲜度**：docs/ 比源码晚的部分
- **GR-5 invariant 覆盖**：invariants.md 没自动测试的项

custodian 后续可往 golden_rules.py 加项目专属检查。

### CI 骨架

创建 `scripts/run_ci.py`：
- 第一步调 `golden_rules.check_all()`
- 一条命令跑全部质量检查（golden rules + tests + 类型检查 + 契约校验）
- exit 0 = 全过，exit 1 = 失败
- dev 写测试时持续往里加项目专属检查

### Check 脚本错误信息标准

所有 check 脚本（CI、契约校验、架构 lint）必须输出 **agent 可读** 的错误信息：

```
# BAD：agent 不知道怎么修
ERROR: api-contracts.md out of sync

# GOOD：agent 能直接修
[CONTRACT-SYNC] POST /api/auth/refresh — 代码里有但 docs 缺
  File: src/auth/controller.py:142
  FIX: 加到 docs/api-contracts.md 的 "Auth API" 段
  格式: | POST | /api/auth/refresh | Refresh JWT token | { token: string } |
```

骨架不必一次写完，随项目成长。但**第一天就要把文件创出来**，否则后面没人会主动建。

---

## Step 4：创建团队 + 拉起 worker

### 4.1 创建团队

```
mcp__team-mode__team_create(name="<project>", cwd="<可选，默认继承>")
```

返回里会有 `web.url`，告诉用户 Web UI 已自动打开。

### 4.2 拉起 worker

每个角色调一次 `worker_add`，**串行调用**（一次性 spawn 太快可能冲掉 daemon）：

```
mcp__team-mode__worker_add(
  team="<project>",
  name="<role-name>",      # 如 alice, backend-dev, reviewer
  adapter="<codex|claude-code>",  # 默认 codex；省略时也是 codex
  model="<可选>",
  cwd="<可选>",
  system_prompt="<完整 onboarding prompt——见 references/onboarding.md>",
  env={...},               # 可选环境变量
  on_existing="error"      # 新建场景；恢复时用 "reuse"
)
```

**关键**：`system_prompt` 必须是**完整渲染的 onboarding**（common template + 角色特定段 + 项目特定上下文），按 [references/onboarding.md](references/onboarding.md) 拼接。

返回里带 `sessionState` 和 `hint`。如果 `sessionState != "running"`，停下来检查日志。

### 4.3 第一条派发消息（也是 session_id 捕获时机）

worker 进程的 `session_id` 是在收到第一条 `@mention` 消息后被捕获的。
拉起后立刻用 `send_message` 给每个 worker 一条"启动确认"消息：

```
mcp__team-mode__send_message(team="<project>", text="@<name> 团队已就绪，请 Read .plans/<project>/<name>/task_plan.md 并简短确认你看到的任务和你的第一步计划。")
```

每个 worker 一条。Stop hook 会自动把回复推回来。

### 4.4 写 team-snapshot.md

所有 worker spawned 后，写 `.plans/<project>/team-snapshot.md`：包含完整 onboarding prompt 原文 + skill 源文件时间戳，供恢复模式使用。模板见 [references/templates.md](references/templates.md)。

**关键**：onboarding prompt 必须**完整保存**——不要摘要、不要截断。恢复时直接当 `system_prompt` 传给 `worker_add(on_existing="reuse")`。

---

## Step 5：确认 + Compact

### 5.1 给用户一个表格

```
| Worker | Role | Adapter | Model | cwd | Plan 路径 |
|--------|------|---------|-------|-----|-----------|
| alice  | backend-dev | codex | (default) | <cwd> | .plans/<project>/alice/ |
| ...    | ...  | ...     | ...   | ... | ...       |
```

附带：
- `.plans/<project>/` 路径
- `CLAUDE.md` 路径（lead 用） + `AGENTS.md` 路径（codex worker 用）
- 验证：`diff CLAUDE.md AGENTS.md` 应无输出
- Web UI 地址（http://127.0.0.1:8787）
- 已活的 worker 数 + adapter 分布

### 5.2 引导 `/compact`

告诉用户跑 `/compact` 释放 context。原因：
- Setup 过程消耗了大量 context（读模板、创文件、拉 agent）
- 操作知识已经持久化在 CLAUDE.md（每次 session 启动自动加载）+ AGENTS.md（codex worker 启动时读）+ `.plans/` 文件里
- Compact 后能腾出 context 做实际团队管理

### 5.3 必须警告（不能省！）

照念或改写以下内容给用户：

> **Compact 后 team-lead 可能"失忆"**——忘掉 worker 名字、协议、当前项目上下文。这是 Claude Code compact 的正常行为：CLAUDE.md 只在 session 启动时注入（Codex worker 不受影响，它每次启动都读 AGENTS.md），compact 会重写历史摘要。
>
> **如果 compact 后 lead 看起来糊涂了，告诉它一句**：
>
> > "Read `.plans/<project>/team-snapshot.md` 恢复团队状态"
>
> 这会让 lead 重新加载完整 roster 和所有 onboarding prompt，立即回到工作状态。所有进度都在 `.plans/` 文件里，没有丢失。

这条警告**必须**在引导 `/compact` 前给出——否则用户碰到失忆 lead 不知道怎么救。

---

## 关键规则

- **1 项目 1 活团队**：`team_create` 限制；要新团队先 `team_delete` 旧的
- **派发是双轨**：lead 维护 `.plans/<agent>/task_plan.md` 文件 + `send_message` @mention 通知。文件是事实源，消息是触发器
- **lead 自己用 TaskCreate 当 todo 没问题**：纯本会话，跟团队协作不冲突；只是不能拿 TaskCreate 当跨 worker 派发渠道
- **team-lead 是控制平面**：主对话拥有需求对齐、任务拆分、phase gate、主 plan 维护、CLAUDE.md + AGENTS.md 维护
- **CLAUDE.md ↔ AGENTS.md 同步**：任何对 CLAUDE.md 的更新都要同步到 AGENTS.md（`cp CLAUDE.md AGENTS.md`），否则 codex worker 拿到的项目上下文会陈旧
- **Context Recovery**：worker 被 compact 后必须先读自己 task 子目录文件
- **所有角色都用 task 子目录**：根 findings.md 是 INDEX
- **Review 触发**：dev 完成大特性/新模块后 → `@reviewer` 直接喊；小改 / bug 修复跳过
- **researcher 用 sonnet**：研究需要深度
- **串行 spawn worker**：一次一个，避免 daemon 冲突
- **不要在团队建好后再 spawn 标准 subagent**：任何工作都通过 worker 走（除非要永久加新 worker）
- **代码是事实源**：文档跟代码走。dev 改 API 必须同步更新 `docs/api-contracts.md`
- **高风险边界 invariant 优先**：循环出现的 bug → Known Pitfalls → invariants.md → 自动测试
- **Pattern → Automation**：reviewer 标 `[AUTOMATE]` → lead 派给 custodian 写 check 脚本
- **CI gate before review**：CI 脚本存在时，dev 必须 CI 全绿才能找 reviewer
- **Template-first**：发现通用工作流改进，先改 skill 源文件，再同步项目文档
- **Phase 边界重建**：不要中途重建活团队，找 phase 边界
- **No archiving**：完成的 task 子目录留在原位，只在根 INDEX 里标 `Status: complete`
- **Worker 互相 @ 不会无限 ping-pong**：Bug 12 已修，但 onboarding 仍要提醒"完成后给一个 terminal 句子，不要持续礼貌回复"
- **Assumption Audit**：每个 harness 组件都编码了一个对模型能力的假设。模型升级后用 CLAUDE.md Harness Checklist 审计、简化

---

## team-lead 操作指南

### 在 team-mode MCP context 下应用 planning-with-files

planning-with-files 的核心思想：**文件系统 = 磁盘，context = 内存，重要东西必须落盘**。

在 team 项目里这条原则在三层运作：

| 层级 | 维护人 | 文件位置 | 焦点 |
|------|-------|---------|------|
| 项目全局 | team-lead | `.plans/<project>/task_plan.md` | Phase 进度、架构决策、任务分配 |
| Agent 级 | 每个 worker | `.plans/<project>/<agent>/` | 任务索引、笔记、工作日志 |
| 任务级 | 每个 worker | `.plans/<project>/<agent>/<prefix>-<name>/` | 详细步骤、findings、进度 |

每个 worker 的 onboarding 已包含等价自检协议（5 问周期检查、2-Action Rule、3-Strike），lead 不需要手动触发，worker 自驱。

> **关于 `/planning-with-files:status`**：该命令读项目根的单一 task_plan.md，不感知 team 多层结构。要看主 plan 直接 Read `.plans/<project>/task_plan.md`。

### Team 状态自检

team-lead 自己也要周期自检。建议在以下时刻主动看：

**快速扫**（并行读每个 worker 的 progress.md）：
```
Read .plans/<project>/backend-dev/progress.md
Read .plans/<project>/frontend-dev/progress.md
...
```

**深入**（觉得有问题时读 findings.md）：
```
Read .plans/<project>/<agent>/findings.md
```

**决策对齐**（方向需要调整时读主 plan）：
```
Read .plans/<project>/task_plan.md
```

阅读顺序：**progress（在哪）→ findings（遇到什么）→ task_plan（目标是什么）**

### team-lead 拥有控制平面

不仅仅是派发：

- 用户需求对齐 + scope 控制
- 任务拆解（明确输入、输出、依赖、验收标准）
- 维护 `.plans/<project>/task_plan.md`、`decisions.md`、项目 `CLAUDE.md`
- 决策 phase gate：research → dev → review → e2e → cleanup
- 决策工作流改进是项目本地还是要回写到 `agent-teams-mcp-setup` 源文件

如果这些不留在主对话里，团队还能跑，但会漂移。

### 处理 worker 3-Strike escalation

worker 报 "3 次失败，escalate to team-lead" 时：
1. 读它的 progress.md 看尝试过的步骤
2. 评估主 plan（task_plan.md）是否需要修改
3. 给清晰的新方向，或重派给其他 worker
4. **Guardrail 检查**：这种失败模式会复发吗？
   - 本项目会 → 追加到 CLAUDE.md `## Known Pitfalls`
   - 未来项目通用 → 同时记 `[TEAM-PROTOCOL]`，考虑改 skill 模板
   - 一次性 → 不动作

### Phase 推进节奏

- Research phase 完 → 读 researcher findings.md → 更新主 task_plan.md 架构决策段
- Dev phase 完 → 等 reviewer 结果 → 确认 [OK] / [WARN] 才推进
- 全部完 → 并行读各 worker progress.md，确认全标 complete

**Phase 边界健康检查**：
- 各 worker 根 findings.md INDEX 是否最新？
- TaskList 里是否有过期的 in_progress？
- 主 task_plan.md phase 状态是否对得上实际？
- CLAUDE.md Known Pitfalls 有没有要带入下个 phase 的？
- 跑 Harness Checklist（见 CLAUDE.md 模板）

---

## 故障排查（team-mode 特有）

| 症状 | 排查 |
|------|------|
| `team_create` 失败 "team already exists" | 调 `team_list` 看 ownerStatus；orphan 的先 `team_delete` |
| `worker_add` 失败 "profile exists" | 加 `on_existing="reuse"` 复用 / `"overwrite"` 替换 |
| `send_message` 失败 "unmatched mention" | 检查拼写；@mention 大小写不敏感但要存在；调 `worker_list` 看活的 worker |
| Worker 不回复 / 回复不到达 | 1) `tail -f .agent-teams/mcp.log` 看 worker 是否在出 reply；2) `tail -f .lead-pending-wake.log` 看 Stop hook 是否 fire；3) 没 fire → 100% 是 Claude Code 没重启加载 hook → 关掉所有 CC 重开 |
| `worker_list` 显示 dead worker | `worker_add(name=..., on_existing="reuse")` 复活——会拿到 `revived_from_dead: true`，新会话无前文记忆 |
| `/mcp` 显示 team-mode 未连接 | 不是 daemon 死了——多半是 CC 的 MCP client 断了；`/mcp` 重连 |

### codex worker 沙盒相关（高频踩坑）

| 症状 | 可能原因 | 应对 |
|------|---------|------|
| codex worker 跑 `npm run dev` 报"成功"，下一轮 curl 拒连 | 长进程被 sandbox 在轮次结束时回收 | lead 在主对话里 Bash 起 dev server，worker 只负责跑测试 |
| codex worker 写 `~/.config/...` 或系统目录失败 | sandbox 限制 cwd 外写入 | 任务范围限定 cwd 内；全局类操作 lead 自己来 |
| codex worker 调付费 API 报"未授权" | env 变量没透传 | `worker_add` 时显式 `env: {OPENAI_API_KEY: "...", ...}` |
| codex worker 起 Docker 容器，下一轮容器没了 | sandbox 子进程回收 | 容器类操作 lead 自己起，worker 只交互 |
| codex e2e-tester 跑 Playwright 浏览器中途死 | 浏览器子进程被回收 | 把 e2e-tester 换 `claude-code` adapter（worker_remove + worker_add 切换）|
| 不知道 codex worker 内部干了什么 | — | 看 `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` 或 Web UI 右栏 session transcript |

详细沙盒坑清单见 [references/adapters.md §codex 沙盒坑](references/adapters.md)。

### 部署/上线类操作建议（任何 adapter）

❌ **不要**派给 worker：`vercel deploy`、`netlify deploy`、`kubectl apply`、`terraform apply`、数据库 migration——凭据 + 状态 + 沙盒三重风险
✅ **lead 在主对话里 Bash 执行**这些操作；worker 只负责生成 / 验证产物
