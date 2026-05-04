# agent-teams-v2 - Main Plan

> Status: **全部主线 + 全部 P5 backlog (B1~B7) 闭环**；项目可发布状态。Session 7~12 落地 ADR-019~030（latest ADR-030：v3.1 project-root isolation + archive/delete/watchdog 语义闭环，task v3.1 已 closed）。
> Created: 2026-04-26
> Updated: 2026-05-04 Session 12（v3.1 project isolation refactor：Step 1~4 complete, Step 5 docs closeout in progress；latest ADR-030）
> Team: agent-teams-v2 (backend-dev/codex, frontend-dev/codex, researcher/codex, e2e-tester/codex, reviewer/codex, custodian/codex)
> Decision Log: .plans/agent-teams-v2/decisions.md

---

## 1. Project Overview

在现有 agent-teams-mcp（Rust + axum 的 team-mode MCP server，自带 Web UI 在 http://127.0.0.1:8787）基础上，做两件事：

1. **可视化扩展** — 扩展现有 Web UI（`web/team-mode/index.html`），加 dashboard、worker 状态面板、任务进度面板、日志查看
2. **继续开发后端能力** — 给可视化所需的 API/事件流补齐，并按需推进 `src/team_mode/` 的功能演进

代码路径速览：
- 后端 HTTP service（ADR-020 默认）：`src/bin/team_mode_service.rs` + `src/team_mode/`（含 service / mcp / mcp/http_transport / runtime_workers）；端口 8786
- 后端 stdio MCP + daemon（fallback / rollback only，ADR-020 已退役默认）：`src/bin/team_mode_mcp.rs` + `src/team_mode_daemon/`
- 后端 Web 服务：`src/team_mode_web/`；端口 8787
- 前端：`web/team-mode/index.html`（vanilla HTML，单文件）
- Plugin / 安装脚本：`plugin/`、`scripts/setup.{sh,ps1}`、`scripts/team-mode-service.ps1`（HTTP service 启停）
- Hook 脚本（ADR-022 default）：`scripts/hooks/lead-pending-async-wake.js` + `lead-pending-mid-turn.js`

详细产品定义 → docs/product.md（researcher 在 P0 写出来）

---

## 2. Docs Index

| 文档 | 位置 | 内容 |
|------|------|------|
| Navigation Map | docs/index.md | 章节级导航（custodian 维护） |
| Architecture | docs/architecture.md | 系统组件、数据流、关键设计决策 |
| API Contracts | docs/api-contracts.md | 后端 → 前端接口定义 |
| Invariants | docs/invariants.md | 不可破系统边界 |

---

## 3. Phases Overview

任务通过 `.plans/agent-teams-v2/<agent>/task_plan.md` 文件 + `send_message` 双轨派发。

### 切片原则

垂直切片（tracer bullet）：每片穿过所有层（API → 前端 → 测试），scope 窄。
不要先全后端再全前端。

### Phases

- **P0 Discovery** — researcher 全面摸现有架构，产出 docs/architecture.md + docs/api-contracts.md 初稿；custodian 建 baseline harness
- **P1 Design** — researcher 出可视化方案推荐；team-lead 与用户对齐 UI scope
- **P2 Build** — backend-dev + frontend-dev 并行垂直切片实现（每片有 API + UI + 测试）
- **P3 E2E + Review** — e2e-tester 关键流测试；reviewer 全面 review
- **P4 Polish + Cleanup** — custodian 死代码清理，docs/ 终稿

---

## 4. Task Summary

| # | 任务 | Owner | Status | Plan File |
|---|------|-------|--------|-----------|
| T1 | 摸现有架构，写 docs/architecture.md + api-contracts.md 初稿 | researcher | ✅ complete (2026-04-26) | researcher/research-current-arch/ |
| T2 | 调研可视化方案（保留 vanilla HTML / 实时通道 / 图表库） | researcher | ✅ complete (2026-04-26) → 推荐 vanilla+模块化 / polling+SSE / 原生 SVG，全采纳 | researcher/research-viz-options/ |
| T3 | 建 baseline harness | custodian | ✅ complete (2026-04-26) | custodian/audit-baseline/ |
| T4 | 后端事件流/状态 API（polling v1 + SSE v2） | backend-dev | ✅ Step 2A polling + Step 2B SSE 全 reviewed [OK] | backend-dev/task-event-api/ |
| T5 | 前端 dashboard 全套（mock + polling + SSE client） | frontend-dev | ✅ 三阶段全 reviewed [OK]（含 race guard + SSE runtime fallback） | frontend-dev/task-dashboard/ |
| T6 | E2E 关键流测试（design + actual via Playwright MCP） | e2e-tester | ✅ design + 5/5 actual journey PASS | e2e-tester/test-dashboard-flow/ |
| T7 | 全面 code review（项目级跨模块） | reviewer | ✅ stage 1 RD-3/RD-4 [WARN] + stage 2 RD-1/RD-2 [BLOCK]→[OK] round 2 | reviewer/review-p2-rd3-rd4/ + review-p2-rd1-rd2/ |

### 恢复后追加 (2026-04-27 ~ 2026-04-28)

| # | 任务 | Owner | Status | Plan File |
|---|------|-------|--------|-----------|
| Q1 | task-clean-baseline-rust | backend-dev | ✅ reviewer Round 3 [OK] | backend-dev/task-clean-baseline-rust/ |
| Q2 | task-clean-baseline-frontend | frontend-dev | ✅ DONE (smoke 20/20) | frontend-dev/task-clean-baseline-frontend/ |
| Q3 | review-baseline (P0 baseline review) | reviewer | ✅ verdict [BLOCK]，已转单 fix | reviewer/review-baseline/ |
| Q4 | review-clean-baseline-rust（审 Q1） | reviewer | ✅ Round 1 [BLOCK] → Round 2 Recheck/Round 3 [OK] | reviewer/review-clean-baseline-rust/ |
| Q5 | audit-recovery-drift（恢复后簿记漂移扫） | custodian | ✅ complete (2026-04-27) | custodian/audit-recovery-drift/ |
| Q6 | audit-docs-freshness-2026-04-27（docs vs src 漂移） | researcher | ✅ complete (2026-04-27) | researcher/audit-docs-freshness-2026-04-27/ |
| Q7 | audit-ci-features-coverage（run_ci.py 加 features-aware Step 4b） | custodian | ✅ complete | custodian/audit-ci-features-coverage/ |
| Q8 | task-fix-baseline-static-assets（HTML brand kicker / form / banned text 5 层修复） | frontend-dev | ✅ complete-no-review-needed | frontend-dev/task-fix-baseline-static-assets/ |
| Q9 | task-fix-feature-gated-baseline-tests-backend（method handling + diagnostics + ADR-006 session_home injection） | backend-dev | ✅ complete-no-review-needed | backend-dev/task-fix-feature-gated-baseline-tests-backend/ |
| Q10 | task-mcp-mention-parser-relax（first-line only mention + ADR-007） | backend-dev | ✅ complete-no-review-needed | backend-dev/task-mcp-mention-parser-relax/ |
| Q11 | audit-claude-agents-sync-and-retrospective（GR-6 byte-for-byte 自动检查） | custodian | ✅ complete | custodian/audit-claude-agents-sync-and-retrospective/ |
| Q12 | task-events-cursor-400-on-invalid（reviewer T7 #3 修复） | backend-dev | ✅ complete-no-review-needed | backend-dev/task-events-cursor-400-on-invalid/ |
| Q13 | fix-dashboard-sse-runtime-fallback（reviewer T7 #1 HIGH 修复） | frontend-dev | ✅ Round 2 [OK] | frontend-dev/task-dashboard/fix-dashboard-sse-runtime-fallback/ |
| Q14 | task-v3-phase2d（ADR-027 + README / architecture / docs closeout） | backend-dev | ✅ done-ready-for-review | backend-dev/task-v3-phase2d/ |
| Q15 | task-v3.1-project-isolation（project-root isolation / archive / watchdog / docs） | backend-dev | ✅ closed (2026-05-04) | backend-dev/task-v3.1-project-isolation/ |

> task list 会随 phase 推进追加，team-lead 维护

---

## 5. Current Phase

**全部主线 + 全部 P5 backlog (B1~B7) 闭环**；v3 Phase 2d docs closeout ready-for-review。项目可发布状态。

### Last verified CI snapshot (2026-04-30 backend-dev ADR-023 rebind 任务后，全 P5 + ADR-019~023 闭环)
- Golden Rules: 0 FAIL（GR-1 file-size 既有 WARN，不阻塞）
- cargo fmt --check: PASS
- cargo clippy --all-targets -- -D warnings: PASS
- cargo test --workspace: PASS（含 ADR-022 lead_pending per-team 测 + ADR-023 rebind service/tool/HTTP-header 三类新测；message.rs 637 < 800 阈值）
- cargo test --workspace --features team-mode-web: PASS
- python scripts/run_ci.py: PASS（GR-1 既有 file-size WARN 留观察）
- flaky `lead_pending_append_emits_structured_info_log`: 10/10 PASS 已稳（2026-04-30 backend-dev 复跑）
- Node smoke (web/team-mode/app.smoke.test.mjs): 34/34 PASS（2026-04-28 baseline，未重跑）
- E2E (Playwright MCP markdown playbook): 5/5 journey PASS（2026-04-28 baseline，未重跑）

### ADRs（全部）
ADR-001 ~ 007 主线 → 008 mutex policy → 009 bundle revision hash → 010 structured logs → 011 file-size refactor + 700 阈值守护 → 012 dev bundle env switch → 013 walk-ancestor `current_cc_pid` 替代裸 parent PID → 014 D16 worker 网络命令受限决策废止（→ D18） → 015 codex worker 默认 reasoning_effort = high（可被 worker_add 显式覆盖） → 016 (HOLD) AGENTS.md→CLAUDE.md 合并 PoC 等 ChatGPT Plus quota → 017 MCP 进程加 parent CC liveness watchdog + 启动 zombie sweep → 018 sweep/watchdog 用 ancestor 链 + 进程名验证（治本 ADR-017 PID 复用 + worker MCP relay 误杀） → 019 message.rs 拆 inbox 子模块 → 020 全切本地 HTTP service 退役 stdio MCP / daemon RPC → 021 service lead-watchdog 降级 observability + sync tool 走 spawn_blocking → 022 Lead-pending hook 重构 per-team 文件 + asyncRewake，砍 PowerShell ancestor walk（hook 端 0 ms / 旧 4200 ms）+ atomic rename drain 防双注入 → **023 team_create 已存在 team 时 rebind ownerCcPid + updatedAt（修 ADR-022 follow-up，hook 路由不再因 CC 重启失效）**

---

## 6. Backlog（P5 — MEDIUM，不阻塞）

来源：T7 stage 1+2 reviewer 报告 + e2e-tester actual 报告 + retrospective。

| # | 类别 | 摘要 | 来源 | 推荐 owner | Status |
|---|------|------|------|------------|--------|
| B1 | RD-2 实时性 | dashboard messageCreated reply attribution 错（worker 回复给 lead 时挂到 lead 而非 reply worker） | T7 stage 2 #2 | frontend-dev | ✅ done 2026-04-28（attribution helper + 2 smoke；no DTO change） |
| B2 | RD-1 视觉/i18n | dashboard zh i18n 不完整（中文模式仍有 Dashboard / polling connected / sse reconnecting 等英文残留） | T7 stage 2 #3 | frontend-dev | ✅ done 2026-04-28（chrome/transport/status localization + 1 smoke；动态数据未本地化为 known limit） |
| B3 | RD-3 Rust 质量 | 生产路径 mutex `unwrap()`（daemon cache + MCP loop_handles），mutex poison 时 panic 而非可恢复 | T7 stage 1 #1 | backend-dev | ✅ done 2026-04-28（reviewer [OK]，ADR-008 落地） |
| B4 | RD-3 文件大小压力 | tools.rs 1004 + conversation.rs 992（接近 1200 阈值），未来加功能易破 | T7 stage 1 #2 | backend-dev | ✅ done 2026-04-28（reviewer [OK] RD-3 STRONG；ADR-011 700 阈值守护文档化；message.rs 626 接近阈值留观察） |
| B5 | RD-4 可观测性 | lead-pending append + runtime/workers.json 状态变更缺统一结构化成功日志 | T7 stage 1 #5 | backend-dev | ✅ done 2026-04-28（reviewer [OK] RD-4 STRONG；ADR-010 契约） |
| B6 | DX | daemon 静态 bundle 在 daemon 进程 start 时 baked，dev 改前端要重启 daemon 才生效 | e2e-tester actual [BUG] | backend-dev | ✅ done 2026-04-28（reviewer [OK] RD-3 STRONG + RD-4 ADEQUATE；ADR-012 dev bundle env switch + path traversal whitelist；reviewer 留 2 项测试覆盖 backlog 不阻断） |
| B7 | DX | UI/API 不暴露静态 bundle revision，stale asset 只能通过缺 DOM 推断 | e2e-tester [OBSERVABILITY-GAP] | backend-dev | ✅ done 2026-04-28（build.rs FNV-1a + `/api/bundle-revision` + meta + footer + ADR-009；不破 `/healthz`） |

### Retrospective 推荐改进（独立）
来源：`.plans/agent-teams-v2/docs/06-team-workflow/team-collaboration-retrospective-2026-04-28.md`：
- MCP send_message 加 escape / markdown code-block mention 豁免
- worker_add 支持 env_capability_hints
- Stop hook BATCH_GRACE_MS 自适应
- 详见 retrospective §2 + §6 (35 条 friction)
