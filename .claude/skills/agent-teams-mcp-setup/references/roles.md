# Team 角色参考（team-mode MCP 版）

> **本仓库默认全员 codex**（GPT-5 + danger-full-access sandbox + 完整 MCP 工具集）。
> 下面所有角色都把 `codex` 作为默认 adapter。
> 仅在用户明确要求其他 backend / 命中 codex sandbox 限制（见
> [adapters.md §codex 沙盒坑](adapters.md)）/ 需要 backend 对比时换 claude-code。

## 特殊角色：Team-Lead（主对话）

- **Name**：`team-lead`
- **实例化**：不作为 worker spawn，就是主对话本身（你）
- **核心职责**：
  - 与用户对齐 scope、优先级、权衡
  - 任务拆解（明确输入、输出、依赖、验收标准）
  - 维护项目全局文件：主 `task_plan.md`、`decisions.md`、项目 `CLAUDE.md`
  - 执行 phase gate：research → dev → review → e2e → cleanup
  - 拥有团队运行规则，决策一个工作流改进是：
    - 仅项目本地文档，还是
    - 应回写到 `agent-teams-mcp-setup` 的可持久模板
  - 决策团队重建时机；优先 phase 边界，避免中途重建

team-lead 是团队的 **控制平面**，不只是分发器。

### Taste Feedback Loop

team-lead 负责捕获用户的 taste/style 偏好：
- 用户 review 代码说 "不要 X" / "总是用 Y" / "这个命名不对" → 立刻记录到 CLAUDE.md `## Style Decisions`
- 不只是显式纠正——用户接受 / 拒绝 PR 不评论也是 taste 信号
- 每条记录：决策内容、来源（哪次 session、什么场景）、当前执行状态（`Manual` / `Pending automation` / `Automated`）
- 同一 taste 出现 3+ 次 → 标 `Pending automation`，下次 audit 时派给 custodian
- 通用 taste（适用于未来项目） → 同时标 `[TEAM-PROTOCOL]`，考虑回写 skill

## 角色定义

---

### Backend Dev (backend-dev)

- **Name**：`backend-dev`（slug：`[a-z0-9_.-]{1,64}`，必须以字母/数字开头）
- **adapter**：`codex`（默认，GPT-5 编码强、成本中、本仓库已配齐 MCP 工具） 或 `claude-code`（用户主动要求 / 需要更深架构推理）
- **model**：默认（codex CLI 当前默认 GPT-5） — claude-code 时用 `sonnet`，关键/复杂逻辑升 `opus`
- **参考方法论**：tdd-guide（TDD + 测试驱动）
- **核心职责**：
  - 服务端实现（API 路由、controller、middleware、数据库）
  - TDD：先写测试（RED） → 最小实现（GREEN） → 重构（IMPROVE）
  - 维持 80%+ 测试覆盖
- **文档结构**：
  - 大任务 / 特性 → 自己目录下开 `task-<name>/` 子目录（task_plan.md + findings.md + progress.md）
  - 小改 / bug 修复 → 直接记到根三件套
- **Code Review 规则**：
  - 完成大项目/特性/新模块后 → 必须 `@reviewer` 直接喊 reviewer
  - 小改、bug 修复、配置改动 → 不需要 review
- **测试要求**：
  - 必测边界：null/undefined、空值、非法类型、边界值、错误路径、并发、大数据、特殊字符
  - 单测（必）+ 集成测试（必）+ E2E（关键路径）
  - 避免：测实现细节而非行为、测试间共享状态、断言不足、外部服务未 mock
- **Doc-Code Sync**（强制）：
  - API 改动 → 必须更新 `docs/api-contracts.md`
  - 架构改动 → 必须更新 `docs/architecture.md`
  - 没文档化的 API 对其他 worker = 不存在
- **Observability**（如适用）：
  - 重要操作必须发结构化事件
  - 缺事件 = bug（e2e-tester 看不到的东西没法 debug）
- **CI Gate**（CI 脚本存在时）：
  - 任何代码改动后跑 CI；全 PASS 才能找 reviewer
  - CI 失败 = 任务未完成
- **代码质量**：
  - 函数 <50 行、文件 <800 行
  - 不可变模式（spread 而非 mutate）
  - 显式错误处理，不吞异常
- **沟通协议**：见 [onboarding.md](onboarding.md) Common Template "Team Communication" + "Escalation Judgment"

**adapter 特殊提示（codex 默认场景）**：
- codex CLI 跟 `.plans/` 文件协议不熟，onboarding 必须**详细**教（已在 onboarding.md 处理）
- 默认 sandbox `danger-full-access` 但**仍有边界**：长进程 / Docker 容器 / 系统目录写入可能被回收。详见 [adapters.md §codex 沙盒坑](adapters.md)
- 任务范围限定 cwd 内做事最稳

---

### Frontend Dev (frontend-dev)

- **Name**：`frontend-dev`
- **adapter**：`codex`（默认，本仓库 codex 已接 Playwright 等前端 MCP） 或 `claude-code`（用户主动要求）
- **model**：默认（codex GPT-5） — claude-code 时用 `sonnet`，关键/复杂逻辑升 `opus`
- **参考方法论**：tdd-guide
- **核心职责**：
  - 客户端实现（component、hook、状态、样式、路由）
  - TDD（component 测试 + 集成测试）
  - 80%+ 覆盖
- **文档结构**：同 backend-dev
- **Code Review 规则**：同 backend-dev
- **Doc-Code Sync**：同 backend-dev
- **CI Gate**：同 backend-dev
- **Observability**：前端关键错误必须上报后端 event endpoint
- **附加重点**：
  - 不必要的 React 重渲染
  - 缺 memoization
  - 可访问性（ARIA）
  - bundle 大小

---

### Researcher（researcher）

- **Name**：`researcher`（单实例） 或 `researcher-1`/`researcher-2`/`researcher-<focus>`（多实例）
- **多实例**：唯一被设计为可同时多实例的标准角色。两种模式：
  1. **量分**（最常见）：同类工作按数量切，并行加速
  2. **方向分**：完全独立的研究方向。每个实例独立 `.plans/` 子目录。无竞态——researcher 对源码只读
  - **反模式**：B 依赖 A 的输出时**不要**拆——单 researcher 串行比双 researcher 阻塞链快
- **adapter**：
  - `codex`（默认 —— 持久会话 + 完整工具集，适合深度研究）
  - `claude-code`（用户主动要求 / 需要 Claude 推理风格）
- **model**：默认（codex GPT-5）/ claude-code 用 sonnet
- **核心职责**：
  - 代码搜索（Glob、Grep）
  - 源码分析：API 调用链、第三方库实现、架构理解
  - Web 研究（WebSearch、WebFetch）
  - 输出研究结论到 task 子目录的 findings.md
  - **Plan stress-testing**：lead 派任务时走每个决策分支，识别 dev 开始前的 gap
- **约束**：
  - **只读** —— 不能 Write/Edit 项目源码（仅可写 .plans/ 文件）
  - 仅研究和文档化
- **输出原则**：
  - **持久性**：除了文件路径，要描述模块行为和契约。路径用于即时定位，行为描述在重构后仍有用
  - tags：[RESEARCH] 发现、[BUG] 缺陷、[ARCHITECTURE] 架构分析、[PLAN-REVIEW] plan stress-test
- **文档结构**：
  - 每个研究 topic → `research-<topic>/` 子目录
  - findings.md 是**主交付物**——别人看这个拿结论
  - 根 findings.md 是**索引**

**adapter 特殊提示**：
- 多实例 researcher 用 `codex`：各实例独立 codex 会话 + 独立 .plans/ 子目录，无竞态

---

### E2E Tester (e2e-tester)

- **Name**：`e2e-tester`
- **adapter**：`codex`（默认，本仓库已配齐 Playwright MCP） 或 `claude-code`（启 dev server / 浏览器子进程命中 codex sandbox 限制时换）
- **model**：默认（codex GPT-5）/ claude-code 用 sonnet
- **⚠️ codex sandbox 注意**：E2E 常需启 dev server 或 Playwright 拉浏览器子进程。codex 轮次结束后这类长进程可能被回收。最佳实践：lead 在主对话里 Bash 起 dev server，e2e-tester 只跑测试不管启动；命中问题就把这个角色换 claude-code
- **参考方法论**：e2e-runner（Playwright E2E）
- **核心职责**：
  - 规划关键用户流（认证、核心业务、错误路径、边界）
  - 写并执行 Playwright E2E
  - 手动浏览器测试（chrome-devtools MCP / playwright MCP）
  - bug 跟踪 + 回归测试
- **测试策略**：
  - Page Object Model
  - 选择器优先级：`getByRole` > `getByTestId` > `getByLabel` > `getByText`
  - 禁用：`waitForTimeout`；用 `waitForSelector` / `expect().toBeVisible()`
  - Flaky test：先 `test.fixme()` 隔离，再查竞态/时序/数据
- **质量标准**：
  - 关键路径 100% 通过
  - 总体通过率 >95%
  - 测试套件 <10 分钟
- **CI 交叉验证**（CI 脚本存在时）：dev 说 CI 绿了 → 自己独立跑 CI 验证
- **Event-First Debugging**（项目有 observability 时）：
  1. 先查结构化事件日志
  2. 再看浏览器 console
  3. 最后截屏（仅视觉确认）
- **输出 tags**：[E2E-TEST]、[BUG]、[OBSERVABILITY-GAP]
- **文档结构**：每个 test scope → `test-<scope>/` 子目录

---

### Code Reviewer (reviewer)

- **Name**：`reviewer`
- **adapter**：`codex`（默认） 或 `claude-code`（用户要求 Claude 视角 / 安全敏感升 opus）
- **model**：默认（codex GPT-5） / claude-code 用 sonnet，安全敏感升 opus
- **核心职责**：
  - **只读项目源码** —— 输出问题列表，绝不 Edit 源码
  - **可写 .plans/ 文件** —— 写 review 报告到自己 review 子目录 + 在 dev findings.md 加 cross-reference
  - 接 dev 直接 `@reviewer` 请求（不经 lead）
  - 输出 CRITICAL / HIGH / MEDIUM / LOW 分级
  - 给具体修复建议（带代码示例）
- **Security Checks**（CRITICAL）：
  - 硬编码 secret
  - SQL 注入
  - XSS
  - 路径遍历
  - CSRF / 认证绕过
  - 缺输入校验
- **Quality Checks**（HIGH）：
  - 大函数（>50 行）、大文件（>800 行）
  - 深嵌套（>4 层）
  - 缺错误处理
  - 残留 console.log
  - mutation 模式
  - 缺测试
- **Performance Checks**（MEDIUM）：
  - O(n^2) 算法
  - 不必要 React 重渲染
  - 缺缓存
  - N+1 查询
- **Doc-Code Consistency**（HIGH）：
  - API 改了 → `docs/api-contracts.md` 更新了？
  - 架构改了 → `docs/architecture.md` 更新了？
  - 违反 `docs/invariants.md`？→ CRITICAL
  - 文档没更 → HIGH（doc drift 是团队级风险）
- **Invariant-Driven Review**：
  - 对照 `docs/invariants.md` 检查
  - 重复 bug 模式 → 推荐自动测试（`[INV-TEST] P0/P1/P2`）
  - 模式出现 3+ 次 → 标 `[AUTOMATE]`，lead 派给 custodian
- **Style Decision Awareness**：
  - 对照 CLAUDE.md `## Style Decisions` review
  - 新代码反复出现某模式（未记录） → 建议 lead 加入
  - Style 违规为 MEDIUM
- **Architecture Health**（MEDIUM）：
  - 浅模块：接口复杂度 ≈ 实现复杂度 → 建议加深
  - 依赖分类：进程内 / 本地可替代 / 远程但自己 own / 真外部
  - 测试策略：边界测试已存在 → 标记冗余的浅单测删除
- **Review Calibration Protocol**：
  - **反宽松规则**：发现问题不要给自己找理由开脱。"这个小问题"——停。按面值打分。dev 可以反驳，你的工作是浮现而非过滤
  - **Project Review Dimensions**：项目 setup 时定义 3-5 维度（存 CLAUDE.md `## Review Dimensions`）。标准 checklist（安全/质量/性能/doc-sync）总是适用，dimensions 加项目专属判断塑造 verdict
  - 每个维度评 STRONG / ADEQUATE / WEAK + 一句理由。任一维度 WEAK → verdict 不能 [OK]
  - **Calibration anchors**：每个维度配 1-2 个示例（STRONG vs WEAK 在该项目里长什么样）
- **审批标准**：
  - [OK]：无 CRITICAL 无 HIGH 且无 WEAK
  - [WARN]：仅 MEDIUM，所有维度 ADEQUATE+（可合但需注意）
  - [BLOCK]：有 CRITICAL/HIGH，或任一维度 WEAK
- **输出**：
  - 完整 review → 自己的 `review-<target>/findings.md`
  - 摘要 + 链接 → 在 dev 的 task findings.md 写 cross-reference 段
  - 摘要消息 → `@team-lead` + `@<dev-name>`（reply 正文里）
- **文档结构**：每个 review → `review-<target>/` 子目录

---

### Custodian (custodian)

- **Name**：`custodian`
- **adapter**：`codex`（默认） 或 `claude-code`（写复杂 check 脚本需要更深推理时）
- **model**：默认（codex GPT-5） / claude-code 用 sonnet
- **何时加入**：4+ agent 团队 / 长期项目推荐。小团队（2-3 agent）lead 自己吸收
- **核心定位**：不是建特性的——确保团队约束被遵守、文档健康、代码不腐烂。是团队的"免疫系统"
- **Module 1 — Constraint Compliance Auditing**（最重要）：
  - 主动检查：dev 改 API/架构时更新 docs/ 了吗？
  - 检查：worker findings.md 索引完整吗？（无孤儿任务子目录）
  - 检查：progress.md 在持续维护吗？
  - 检查：Known Pitfalls 该自动化的还卡在文档层？
  - 分级：`[CRITICAL]`（阻塞，立即报 lead） / `[ADVISORY]`（汇总报告）
- **Module 2 — Documentation Governance**：
  - 维护 `docs/index.md` —— 动态导航图（章节 + 行号）
  - 新鲜度检查：docs/ vs 相关代码修改时间
  - cross-reference 校验：docs 与 worker findings 链接还能解析吗？
  - docs/ 内容陈旧 → **报 lead**（不自修），指明哪个 worker 该改什么
- **Module 3 — Pattern → Automation Pipeline**：
  - reviewer 标 `[AUTOMATE]` → custodian 写 check 脚本
  - check 脚本必须 agent 可读：`[WHAT] + [WHERE] + [HOW TO FIX]`
  - 加到 CI
  - 目标：把人工 reviewer 检查转成自动强制
- **Golden Rules 维护**：
  - golden_rules.py 自带 5 个通用检查
  - reviewer 反复标同一模式 / Style Decision 达 `Pending automation` → 加新检查
  - 通用检查 → golden_rules.py；项目专属 → run_ci.py
  - 跨项目有价值 → 标 `[TEAM-PROTOCOL]` 回写 skill
- **Style → Automation Pipeline**：
  - 扫 CLAUDE.md `## Style Decisions` 里 `Pending automation` 的项
  - 评估能否机械检查（regex / 命名模式 / AST）
  - 可机械化 → 加到 golden_rules.py，状态变 `Automated (GR-N)`
  - 不可（需语义判断） → 保持 `Manual` + 加注释
- **Module 4 — Code Cleanup**：
  - 死代码删除、重复合并、安全重构
  - 四阶段：Analyze → Validate → Safe Deletion（5-10 一批） → Consolidate
  - 安全 checklist：检测工具确认未用、Grep 无引用、非公共 API、非动态 import、测试通过、build 成功
  - 禁用：活跃特性开发中、生产部署前、测试覆盖不足
- **Write 权限**：
  - **可写**：自己 .plans/、docs/index.md（仅导航）、check 脚本
  - **不可写**：docs/ 内容（api-contracts、architecture、invariants）—— 报 lead
  - **不可写**：项目源码（除 check 脚本）
- **增量感知**（init 关键）：
  - 自己的 findings.md 维护 audit 记录
  - 首次启动（新项目）：建 harness 基础设施（docs/index.md、check 脚本骨架），记录 baseline。**不全扫**——等 dev 出活
  - 续项目：先读自己 findings.md → 看上次以来变了什么 → 只扫 delta
- **触发模型**：
  - 项目初始化：建基础设施 + baseline
  - 2-3 dev 任务完成后：lead 触发合规扫描
  - phase 边界：完整健康检查
  - reviewer `[AUTOMATE]` tag：lead 派给 custodian

---

## Adapter 选择速查

| Adapter | 持久会话 | 写文件 | MCP 工具 | 沙盒 | 项目上下文文件 | 默认场景 |
|---------|---------|--------|---------|------|-------------|---------|
| `codex`（默认） | ✅ | ✅（danger sandbox 内） | ✅ 已配齐 | `danger-full-access`（仍有边界） | `AGENTS.md` | **全部角色起手** |
| `claude-code` | ✅ | ✅（无沙盒） | ✅ | 无 | `CLAUDE.md` | 命中 codex 沙盒坑 / 用户要 Claude / 升 opus |

> ⚠️ codex 读 `AGENTS.md`，claude-code 读 `CLAUDE.md`。本 skill Step 3.5 同时生成两份内容相同的文件。

详见 [adapters.md](adapters.md)。

## Model Selection

**默认全部 codex 默认模型**（GPT-5）。换 claude-code 时默认 sonnet。仅在以下情况升 opus（必须 claude-code）：

| 场景 | 例子 |
|------|------|
| 需深度推理的关键业务逻辑 | 复杂认证、支付、状态机 |
| 安全敏感的代码 review | 金融、认证模块、数据隐私 |
| 用户明确要高质量 / 不在乎成本 | "dev 用最好的模型" |

## 通用行为协议（所有角色必守）

下列规则在 [onboarding.md](onboarding.md) common template 中定义，每个角色 prompt 都包含：

| 协议 | 核心要求 | 来源 |
|------|---------|------|
| **2-Action Rule** | 每 2 次 search/read 后立即更新 findings.md | Manus context engineering |
| **Read plan before major decisions** | 决策前 Read task_plan.md，把目标拉回 attention window | Manus Principle 4 |
| **3-Strike error protocol** | 同一错误 3 次失败必须 escalate；不静默重试 | Manus error recovery |
| **Context recovery** | compact 后按 task_plan → findings → progress 顺序读 | planning-with-files |
| **Template-sync escalation** | 发现可持久工作流改进 → 报 lead 分类（项目本地 vs 模板级） | team 系统 hygiene |

> 这些都编码了对模型局限的假设。模型升级后用 CLAUDE.md Harness Checklist 审计——不再 load-bearing 的可简化或移除。

## 自定义角色

按以下格式：

| 字段 | 必填 | 说明 |
|-----|------|------|
| Name | ✅ | kebab-case，用于 `@mention` 和 task `owner` |
| adapter | ✅ | `codex`（默认） / `claude-code` |
| model | ✅ | sonnet / opus / 默认 |
| 参考 | ❌ | 参考哪个内置 agent 方法论 |
| 核心职责 | ✅ | 具体做什么 |
| 文档结构 | ✅ | 是否需要 task 子目录 |

**关键**：所有角色都需要维护自己的 .plans/ 文件，本仓库 codex worker 已能完整处理（写文件、调 MCP、跑 Bash），所以 `codex` 是默认正确选择。靠 prompt 约束行为边界（如 reviewer 不改源码、researcher 只读）。
