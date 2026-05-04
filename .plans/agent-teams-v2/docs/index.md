# agent-teams-v2 Documentation Book

> BOOK_INDEX. custodian 维护。根 `architecture.md`、`api-contracts.md`、`invariants.md` 仍是当前权威文件。

## Preface

这本书整理 agent-teams-mcp 的产品叙事、当前系统、运行手册、Web UI、设计历史、团队工作流和遗留研究。它位于 `.plans/agent-teams-v2/docs/`，面向 team-mode worker、reviewer、maintainer 和需要恢复上下文的 lead。

归档 scratch 文件刻意不进入正文目录：`.plans/agent-teams-v2/archive/docs-scratch/` 与 `.plans/agent-teams-v2/archive/docs-extra-scratch/` 只保留原始备忘。

## Reader Personas

- Developer：先看当前架构、API contracts、Web UI 和 invariants，再按需看 refactor history。
- Operator：先看 deployment、usage tips、notification behavior、daemon lifecycle 和 troubleshooting。
- Agent worker：先看当前系统 reference、team workflow、invariants 和任务相关章节 README。
- Project reviewer：先看 invariants、API contracts、architecture，再看 design decisions 和相关 refactor history。

## Recommended Reading Paths

- New reader：`01-orientation/README.md` -> `02-current-system/README.md` -> `03-operations/README.md`。
- Implementer：`02-current-system/README.md` -> `04-web-ui/README.md` -> `05-design-history/refactor/2026-04/`。
- Operations：`03-operations/README.md` -> `02-current-system/push-notifications.md` -> `02-current-system/worker-detach-refactor.md` -> `05-design-history/README.md`。
- Governance：`06-team-workflow/README.md` -> root `AGENTS.md` / `CLAUDE.md` -> `05-design-history/README.md`。

## Full TOC

```text
.plans/agent-teams-v2/docs/
  index.md
  architecture.md
  api-contracts.md
  invariants.md
  01-orientation/
    README.md
    article.md
    article-zh.md
    why-agent-teams-over-native-claude-teams.md
  02-current-system/
    README.md
    mcp-tools-reference.md
    push-notifications.md
    worker-detach-refactor.md
  03-operations/
    README.md
    global-install.md
    open-source-deployment.md
    usage-tips.md
    mcp-setup-pitfalls.md
  04-web-ui/
    README.md
    team-mode-web-guide.md
    history/web-frontend-plan.md
  05-design-history/
    README.md
    design-decisions.md
    hook-push-design.md
    architecture-background-2026-04.md
    legacy/team-mode-mcp-final.md
    refactor/2026-04/group1-refactor-v1-change-summary-2026-04-28.md
    refactor/2026-04/refactor-plan-2026-04-28.md
    refactor/2026-04/refactor-status-2026-04-29.md
  06-team-workflow/
    README.md
    team-collaboration-retrospective-2026-04-28.md
    internal-guides/plugin-design-insights.md
    internal-guides/team-role-behavior-guidelines.md
  07-research-audits/
    README.md
    claude-code-agent-teams-reverse-engineering.md
    audits/legacy-v0.1.0/AUDIT_REPORT.md
    audits/legacy-v0.1.0/CODE_REVIEW.md
```

## Authority Rules

- Root `architecture.md`、`api-contracts.md`、`invariants.md` beat historical docs for current behavior.
- Chapter 2 current-system docs beat Chapter 5 historical/refactor docs when behavior appears to conflict.
- Root `AGENTS.md` and `CLAUDE.md` beat Chapter 6 governance proposals for runtime protocol.
- `.plans/agent-teams-v2/decisions.md` remains the active execution ADR log; latest current entry is ADR-030, with ADR-020/021/022 defining the HTTP service + asyncRewake control plane, ADR-024 covering Web events/diagnostics per-team pending adaptation, ADR-025 hardening reviewer-blocked PID/migration failure modes, ADR-026 covering cargo-install global project initialization, ADR-027 covering the v3 global MCP lazy-spawn / install-global architecture, ADR-028 fixing `/lead-pending/my-teams` so the service trusts the caller-supplied CC PID instead of re-walking past it, ADR-029 fixing the hook URL bug where `runtime.url`'s `/mcp` suffix made `fetch_my_teams` 404, and ADR-030 covering caller project_root isolation + archive/delete/watchdog semantics.
- HARD integration backlog is explicit: `hook-push-design.md`、`mcp-setup-pitfalls.md`、`architecture-background-2026-04.md` are moved but not merged in this batch.

## Terminology

- Lead：主 Claude Code 会话，拥有用户对齐、scope 控制和派单权。
- Worker：由 team-mode service（ADR-020 默认）或旧 daemon 管理的 codex / 其他 adapter 子进程。
- HTTP MCP service（ADR-020 默认）：`team_mode_service.exe` 长驻本地 HTTP server（127.0.0.1:8786），处理 MCP / lead-pending / web 等请求；bearer token 在 `.agent-teams/runtime/http-mcp.token`。
- MCP relay（legacy / fallback only）：旧 thin stdio 进程 `team_mode_mcp.exe`；ADR-020 退役为 rollback 路径。
- Daemon（legacy / fallback only）：旧项目级长驻 `team_mode_daemon`；ADR-020 后由 HTTP service 取代为默认。
- Stop hook async-wake（ADR-022 默认）：worker reply 唤醒 lead 的机制；`asyncRewake: true` + per-team `<base>/<team>/lead_pending.jsonl` + atomic `fs.renameSync` drain 防双注入。
- Doc-Code Sync：改架构或 API 的任务必须同步更新对应 docs。

## Active ADR snapshot（runtime defaults）

ADR-019/020/021/022/023/024/025/026/027/028/029/030 是当前 runtime 默认；旧 stdio relay + daemon 路径仍可作 rollback 但不是默认。完整 ADR 列表见 `.plans/agent-teams-v2/decisions.md`（researcher H 任务在加分类 index）。

## Maintenance Notes

- 新增章节文件时，同时更新对应 chapter `README.md` 和本 `index.md` TOC。
- 当前系统事实变更时，优先更新根 `architecture.md`、`api-contracts.md` 或 `invariants.md`。
- 历史、研究、复盘文档不能静默覆盖 runtime instructions；需要成为规则时同步修改 `AGENTS.md` 和 `CLAUDE.md`。
- 迁移或重命名文档后必须做 old-path grep、GR-6 byte-equal 检查和项目 CI gate。
