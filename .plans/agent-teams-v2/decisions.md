# agent-teams-v2 - Architecture Decisions Log

> ADR 风格。每条决策含 Context / Decision / Consequences。

## Index

### Runtime

| ADR | Summary | Status |
|-----|---------|--------|
| [[#ADR-008: Poisoned mutex policy in production paths|ADR-008]] | Production locks surface poisoned mutexes as explicit errors instead of panics. | Active |
| [[#ADR-010: Structured logs for team-mode side effects|ADR-010]] | Lead-pending and runtime worker state changes emit stable structured events. | Active |
| [[#ADR-013: walk-ancestor `current_cc_pid` 替代裸 parent PID|ADR-013]] | Owner CC PID is resolved by walking ancestors and skipping shell wrappers. | Active |
| [[#ADR-014: D16 worker 网络命令受限决策废止（→ D18）|ADR-014]] | Retires the old assumption that codex workers cannot run DNS/curl/SSH. | Supersedes D16 |
| [[#ADR-017: MCP 进程加 parent CC liveness watchdog + 启动 zombie sweep|ADR-017]] | Adds stdio MCP parent liveness and zombie sweep for the legacy fallback path. | Active legacy/fallback |
| [[#ADR-018: sweep/watchdog 用 ancestor 链 + 进程名验证（治本 ADR-017 PID 复用 + worker MCP relay 误杀）|ADR-018]] | Hardens legacy sweep/watchdog against PID reuse and worker relay false kills. | Active legacy/fallback |
| [[#ADR-020: Team Mode 全切本地 Streamable HTTP service，退役 stdio MCP relay / daemon RPC|ADR-020]] | Makes local Streamable HTTP service the default MCP control plane. | Current default |
| [[#ADR-021: Team Mode service lead-watchdog 降级为 observability + HTTP 工具调用走 spawn_blocking|ADR-021]] | Keeps HTTP service durable and runs sync tool dispatch in `spawn_blocking`. | Active |
| [[#ADR-025: Reviewer BLOCK 修复：lead-pending migration 去重 + HTTP service PID/ancestor fail-closed|ADR-025]] | Hardens reviewer-blocked migration retry, service PID reuse, and owner header fallback paths. | Active |
| [[#ADR-026: `team_mode_service init` 支持全局安装后的任意项目初始化|ADR-026]] | Adds self-contained project initialization after `cargo install --path .`. | Active |
| [[#ADR-028: `/lead-pending/my-teams` 信任 caller-supplied CC PID，不再二次 ancestor walk|ADR-028]] | Service trusts the hook/relay's already-resolved CC PID instead of re-walking past it. | Active |
| [[#ADR-029: hook `fetch_my_teams` 必须 strip runtime URL 的 `/mcp` 后缀|ADR-029]] | Hook strips the `/mcp` suffix off the runtime MCP URL before appending `/lead-pending/my-teams`, fixing 100% hook non-fire. | Active |
| [[#ADR-030: v3.1 project-root isolation for Team Mode data and lifecycle|ADR-030]] | Caller project_root scopes team data, archive/delete semantics, and dead-owner watchdog auto-archive. | Active |

### Messaging

| ADR | Summary | Status |
|-----|---------|--------|
| [[#ADR-007: MCP send_message 只用首行 mention block 分发|ADR-007]] | Routes `send_message` by first-line mention block or explicit `mentions`. | Active |
| [[#ADR-019: `message.rs` inbox 子模块拆分继续执行 700 行警戒线|ADR-019]] | Splits `inbox_read` implementation out of the main message dispatch module. | Active |
| [[#ADR-022: Lead-pending hook 重构为 per-team 文件 + asyncRewake，砍 PowerShell ancestor walk|ADR-022]] | Reworks lead-pending delivery around asyncRewake, per-team files, and atomic drain. | Active |
| [[#ADR-023: `team_create` 对已存在 active team 执行 ownerCcPid rebind|ADR-023]] | Lets existing active teams rebind ownerCcPid after CC restart or takeover. | Active |

### Web

| ADR | Summary | Status |
|-----|---------|--------|
| [[#ADR-002: 前端起点保留 vanilla HTML|ADR-002]] | Keeps the first Web UI phase on vanilla HTML instead of a framework. | Active |
| [[#ADR-003: Worker lifecycle 事件先采用低风险 read-model 派生|ADR-003]] | Derives worker lifecycle view from runtime state before adding history logs. | Active |
| [[#ADR-005: Team-mode Web SSE 使用 std-only HTTP streaming 分支|ADR-005]] | Adds SSE streaming as a std-only branch beside buffered REST/static responses. | Active |
| [[#ADR-006: Claude session discovery 支持 home 注入|ADR-006]] | Allows session discovery to use an injected home for tests and embedding. | Active |
| [[#ADR-009: Static bundle revision uses build-time content hash|ADR-009]] | Exposes baked Web asset freshness through a build-time content hash. | Active |
| [[#ADR-012: Team Mode Web dev bundle reads static assets from disk|ADR-012]] | Adds explicit dev mode that reads whitelisted Web assets from disk. | Active |

### Governance

| ADR | Summary | Status |
|-----|---------|--------|
| [[#ADR-001: 全部 worker 用 codex backend|ADR-001]] | Chooses codex workers and records their project-level runtime assumptions. | Active |
| [[#ADR-004: MCP tools 聚合模块继续按职责拆分|ADR-004]] | Keeps MCP tool aggregation small by moving Web bootstrap helpers out. | Active |
| [[#ADR-011: File-size refactor by stable responsibility boundary|ADR-011]] | Establishes stable-boundary refactors and a proactive 700-line submodule guardrail. | Active |
| [[#ADR-015: codex worker 默认 reasoning_effort = high（显式 default，可被 worker_add 显式覆盖）|ADR-015]] | Sets high reasoning as the documented default for this repo's codex workers. | Active |
| [[#ADR-016（HOLD）: AGENTS.md → CLAUDE.md 合并 PoC 等待 quota 恢复|ADR-016]] | Holds the AGENTS.md-to-CLAUDE.md merge PoC until quota allows validation. | HOLD |

---

## ADR-001: 全部 worker 用 codex backend

- **Date**: 2026-04-26
- **Context**: 项目延续现有 Rust + Web 项目；用户希望降本+对比 GPT-5 效果
- **Decision**: 6 个 worker 全部用 `codex` adapter，模型走默认（GPT-5）
- **Consequences**:
  - + GPT-5 编码强项可在 Rust 项目验证
  - + 成本相对 Claude opus/sonnet 更低
  - ~~− e2e-tester 的 Playwright MCP 支持不确定~~ → **2026-04-26 更新**：e2e-tester P0 自检 PASS，Playwright MCP 完全可用（导航 / snapshot / DOM 读取均 OK），结论见 `.plans/agent-teams-v2/e2e-tester/test-mcp-capability/findings.md`
  - − codex 项目级配置文件是 `AGENTS.md` 不是 `CLAUDE.md` —— 已同时维护两份
  - codex worker 启动时配置 `approvalPolicy: never` + `sandbox_mode: danger-full-access`，可放心写文件 + 跑 cargo

## ADR-002: 前端起点保留 vanilla HTML

- **Date**: 2026-04-26
- **Context**: 现有 `web/team-mode/index.html` 是单文件 vanilla HTML
- **Decision**: P1 之前不引入 framework；researcher 调研后由 lead+用户决定是否引入轻 framework
- **Consequences**:
  - + 避免过早 architectural commitment
  - + Phase 推进灵活
  - − 如果可视化变复杂，单文件 HTML 维护成本会上升

## ADR-003: Worker lifecycle 事件先采用低风险 read-model 派生

- **Date**: 2026-04-27
- **Context**: T4 `task-event-api` 需要给前端暴露 worker 状态变更事件。当前可靠持久状态来自 `<base>/runtime/workers.json`，但它主要保存当前 runtime worker rows，不是完整 append-only lifecycle history。
- **Decision**: T4 默认从 `runtime/workers.json` 派生 worker status read-model，不为 `spawn` / `dead` / `revived` 强制新增完整历史事件日志。若后续 frontend-dev 或 e2e-tester 需要历史回放或精确 transition replay，再追加事件日志设计。
- **Consequences**:
  - + 实现风险低，适合先支撑 dashboard freshness 和当前状态展示。
  - + 避免过早修改 MCP mutating paths，降低对 worker lifecycle 管理路径的影响面。
  - − 不能保证完整回放每一次 `spawn` / `dead` / `revived` transition。
  - − 前端如需精确历史时间线，需要后续新增 append-only event source 并补充 API contract。

## ADR-004: MCP tools 聚合模块继续按职责拆分

- **Date**: 2026-04-27
- **Context**: baseline cleanup review 要求 `src/team_mode/mcp/tools.rs` 低于 Golden Rules 的 1200 行硬失败阈值；reviewer round 1 看到该文件 1265 行，违反 INV-Q3。worker/message 工具此前已拆到子模块，剩余可低风险拆分的大块是 Web UI auto-open/server bootstrap helper。
- **Decision**: 采用最小工程拆分：保留 `src/team_mode/mcp/tools.rs` 作为 MCP toolset 聚合与 team 级工具入口，仅把 Web auto-open/server bootstrap 逻辑迁到 `src/team_mode/mcp/tools/web_open.rs`。不改业务逻辑；外部调用继续通过 `crate::team_mode::mcp::tools`，其中 `ensure_team_web_server_public` 从 `web_open` re-export。
- **Consequences**:
  - + 消除 `tools.rs` 超过 1200 行的 Golden Rules 硬失败，baseline gate 可用 `run_ci.py --skip-test` 验证。
  - + Web auto-open/server bootstrap 有清晰模块边界，可避免把 UI bootstrap helper 继续堆回 MCP toolset 聚合文件。
  - + 保持外部 import 路径兼容，daemon 仍可调用 `crate::team_mode::mcp::tools::ensure_team_web_server_public`。
  - − Golden Rules 仍有 9 个 800 行 WARN；未来若相关文件继续增长，应按职责继续拆分。`backend-dev/findings.md` Quick Notes 已记录后续 MCP tools 优先拆子模块。

## ADR-005: Team-mode Web SSE 使用 std-only HTTP streaming 分支

- **Date**: 2026-04-27
- **Context**: T4 Step 2B 需要新增 `GET /api/teams/{team}/events/stream` SSE endpoint。现有 team-mode Web server 是 `std::net::TcpListener` + hand-written HTTP；普通 routes 返回 buffered `WebResponse`，由 `app.rs` 写 `Content-Length` 和 `Connection: close`。SSE 需要保持连接并持续写 frames，不能沿用 buffered response。
- **Decision**: 保持 std-only server，不引入 axum/hyper/tungstenite。`app.rs` 在解析 HTTP request 后识别 `/api/teams/{team}/events/stream`，直接写 `text/event-stream` headers 并把 `TcpStream` 交给 `src/team_mode_web/sse.rs`。普通 REST/static path 继续走 `routes.rs -> WebResponse`。SSE 复用 polling v1 `read_model::read_events`、`EventView` 和 opaque cursor；`Last-Event-ID` header 优先于 query `cursor`。
- **Consequences**:
  - + 保持 team-mode Web 的无 server crate 约束，避免扩大依赖面。
  - + buffered JSON/static path 与 streaming path 边界清晰，polling v1 行为不变。
  - + SSE 与 polling 共用 cursor/DTO，前端可在两种 transport 间 fallback。
  - − `app.rs` 需要承担少量 streaming-specific request dispatch；未来若 streaming path 继续增加，可能需要更明确的 HTTP dispatch abstraction。

## ADR-006: Claude session discovery 支持 home 注入

- **Date**: 2026-04-28
- **Context**: `session_discovery::discover_sessions()` 使用 `dirs::home_dir()` 定位 `~/.claude/projects`。Windows 上 `dirs::home_dir()` 来自系统 known folder profile，不跟随测试里临时设置的 `HOME` / `USERPROFILE`，导致 Team Mode Web 的 conversation/diagnostics feature-gated tests 无法 hermetic 地 seed Claude session JSONL。
- **Decision**: 新增 `session_discovery::discover_sessions_with_home(home, cwd)` 注入入口；`TeamModeWebServerConfig` 增加 `session_home: Option<PathBuf>` override；`TeamModeWebState` 保存该 override；conversation 与 diagnostics read-model 优先使用 `state.session_home()`，缺省 `None` 时继续走 `dirs::home_dir()` 的生产默认行为。
- **Consequences**:
  - + feature-aware tests 可以在 temp home 下 hermetic seed `.claude/projects/<encoded-cwd>/<session>.jsonl`。
  - + 未来 embedding / hosted Web 场景可以显式注入 Claude session home，而不必依赖进程真实用户 profile。
  - + 生产路径保持兼容：默认 `session_home = None`，行为仍由 `dirs::home_dir()` 决定。
  - − `TeamModeWebServerConfig` 多一个可选公共字段，嵌入者需要理解该字段只影响 session discovery，不影响 HTTP API。

## ADR-007: MCP send_message 只用首行 mention block 分发

- **Date**: 2026-04-28
- **Context**: Retrospective 发现 `send_message` 扫描整个正文中的 `@handle`，导致派单正文、示例和复盘文字里的 handle-like 文本触发 unmatched 或 self-mention 拒收。lead/worker 必须回避普通文字，心智负担高。
- **Decision**: MCP `send_message` 的 implicit dispatch 只解析第一行开头连续的 `@handle` block；第一行后续正文不参与路由。新增 optional `mentions: string[]`，当数组非空时完全使用该数组作为 dispatch recipients，body 不扫描；空数组等同未提供，回退到首行解析。Mention parser helper 拆入 `src/team_mode/mcp/tools/mention.rs`，避免继续放大 MCP toolset 聚合文件。
- **Consequences**:
  - + 派单正文可以安全描述 worker/lead 名字和示例，不再触发意外 unmatched/self-mention。
  - + 首行 `@worker` / `@a @b` 派单习惯保持可读，并与中心化调度协议一致。
  - + 结构化客户端可以通过 `mentions` array 显式路由，避免依赖 body 文本解析。
  - + mention parser 有独立模块边界，`tools.rs` 保持低于 Golden Rules 1200 行硬失败阈值。
  - − 不再支持在正文任意位置写 mention 来路由；调用方需要把 dispatch recipients 放在首行开头或 `mentions` array。
  - − 暂不支持 escape / markdown code-block 豁免；若后续需要更细粒度文本语义，可在当前契约上扩展。

## ADR-008: Poisoned mutex policy in production paths

- **Date**: 2026-04-28
- **Context**: T7 stage 1 review found production paths using mutex `unwrap()` for daemon cache and MCP loop handle state. If any guarded operation panics while holding those locks, Rust poisons the mutex; a later `unwrap()` then panics again, turning a recoverable tool/daemon failure into a process-level crash. A related production lock in the Claude backend session-id reader had the same failure mode.
- **Decision**: Production paths must not `unwrap()` poisoned mutexes. Result-returning paths map poisoned locks to a diagnostic `Error::Other("poisoned mutex: <name>")` and propagate normally. Background task paths that cannot return through the original API emit `AgentOutput::Error`, mark the affected session not alive, wake idle waiters, and stop the task. Test-only helpers may continue using unwraps where panic is the intended failure signal.
- **Consequences**:
  - + MCP and daemon clients now surface lock poisoning as explicit errors rather than panicking.
  - + Background Claude reader failures become visible as agent output and unblock waiters instead of leaving the session wedged.
  - + Lock error text is centralized for daemon cached info and MCP loop handles.
  - − The system does not attempt to recover or reuse potentially inconsistent poisoned state; callers must retry after the failed operation is visible.
  - − A small amount of lock helper code is required around shared mutable state.

## ADR-009: Static bundle revision uses build-time content hash

- **Date**: 2026-04-28
- **Context**: Team Mode Web static assets are baked into the daemon binary with `include_str!`. During development, changing `web/team-mode/*` without restarting the daemon can leave the browser connected to an old baked bundle, and the previous UI/API exposed no direct way to identify that stale bundle.
- **Decision**: Generate `TEAM_MODE_WEB_BUNDLE_REVISION` in `build.rs` from a std-only FNV-1a 64-bit hash over all `web/team-mode/` static files. Hash input includes sorted relative paths, NUL separators, and file bytes. `build.rs` also writes `OUT_DIR/index.processed.html`, replacing the bundle revision placeholder in `index.html` at build time. The revision is exposed via `GET /api/bundle-revision`, an index `<meta name="bundle-revision">`, and the UI footer. `/healthz` remains plain text `ok` for compatibility with health probes.
- **Consequences**:
  - + Revision changes only when baked static asset content/path changes, avoiding per-build noise.
  - + Does not depend on git metadata, so packaged builds and non-git environments behave consistently.
  - + Devs can compare UI/API revision against expected static assets to identify stale daemons quickly.
  - − Adds a build script and one new API endpoint.
  - − Only exposes the revision; it does not implement hot reload or automatic stale-bundle detection.

## ADR-010: Structured logs for team-mode side effects

- **Date**: 2026-04-28
- **Context**: Lead-pending append and `runtime/workers.json` state changes are critical side effects for worker-to-lead notification and dashboard freshness. Before B5, lead-pending append had no success log and only a warning on failure; runtime worker state writes had no structured success/failure log, making RD-4 observability weak when a push or lifecycle update silently drifted.
- **Decision**: Use `tracing` structured events for these side effects. Successful lead-pending append emits `info` with `event = "lead_pending.append"`. Failed lead-pending append emits `error` with `event = "lead_pending.append_failed"`. Successful runtime worker state updates emit `info` with `event = "runtime_worker.state_change"`. Failed runtime worker state updates emit `error` with `event = "runtime_worker.state_change_failed"`. Runtime worker state logging is wrapped inside `RuntimeWorkerStore::upsert_state()` only; lower-level `update_file()` remains generic and must not label unrelated writes as state changes.
- **Stable Schema**:
  - `lead_pending.append`: `team_id`, `message_id`, `recipient_count`, `byte_size`, optional diagnostic fields `owner_cc_pid`, `from_id`, `kind`, `path`.
  - `lead_pending.append_failed`: `team_id`, `message_id`, `lead_member_id`, `error`.
  - `runtime_worker.state_change`: `team_id`, `worker_name`, `prev_state`, `new_state`, `reason`, optional diagnostic fields `spawn_key`, `adapter`, `daemon_pid`.
  - `runtime_worker.state_change_failed`: `team_id`, `worker_name`, `new_state`, `reason`, `error`, optional diagnostic fields `spawn_key`, `adapter`, `daemon_pid`.
- **Consequences**:
  - + Operators and tests can distinguish durable side-effect success from ordinary tool dispatch logs.
  - + Failure paths become visible as `error` events with enough routing context to identify the affected team/message/worker.
  - + Field names and `event` values become a lightweight observability contract; future renames require updating this ADR and any consuming docs/checks.
  - − Adds a small amount of logging and test capture support in the affected modules.

## ADR-011: File-size refactor by stable responsibility boundary

- **Date**: 2026-04-28
- **Context**: Golden Rules treats files above 800 lines as a maintainability warning and files above 1200 lines as a hard failure. `src/team_mode/mcp/tools.rs` and `src/team_mode_web/read_model/conversation.rs` were both near 1000 lines after recent feature work, making future changes likely to cross the hard threshold and harder for agents/reviewers to navigate.
- **Decision**: Split large files by stable responsibility boundary without changing public API or behavior. `tools.rs` remains the MCP toolset aggregation, descriptor/dispatcher, constructor, and shared helper module; team lifecycle code moves to `tools/team_lifecycle.rs`, and root toolset tests move to `tools/tests.rs`. `conversation.rs` remains the public conversation read-model entry point and session-routing layer; source parsers and item projection move to `conversation/claude.rs`, `conversation/codex.rs`, and `conversation/items.rs`. New functionality should land in the relevant submodule rather than growing aggregation files. Any submodule that exceeds 700 lines should be split again using the same 30% margin principle instead of waiting until it approaches the 1200-line hard failure threshold.
- **Consequences**:
  - + `tools.rs` and `conversation.rs` retain public entry points while becoming small enough to leave substantial headroom under GR-1.
  - + Future MCP tool and conversation parser changes have clearer internal ownership boundaries.
  - + The 700-line submodule guardrail gives a proactive threshold before Golden Rules hard-fails.
  - − Internal modules require a few `pub(super)` boundaries and slightly more module wiring.

## ADR-012: Team Mode Web dev bundle reads static assets from disk

- **Date**: 2026-04-28
- **Context**: Team Mode Web static assets are baked into the daemon binary by default. That is correct for release/production because the daemon can run without a `web/` directory, but it hurts frontend development: editing `web/team-mode/*` does not affect an already-running daemon until it is restarted. ADR-009 exposed a bundle revision so devs can see stale bundles, but it did not remove the restart requirement.
- **Decision**: Keep baked assets as the default. Add explicit dev bundle mode selected by `TEAM_MODE_WEB_DEV_BUNDLE=1` or by `TeamModeWebServerConfig.static_bundle` injection in tests/embedding. Dev mode reads only exact whitelisted static filenames from a configured `web/team-mode` root (`TEAM_MODE_WEB_DEV_BUNDLE_DIR` or `<cwd>/web/team-mode`) on each static request. It never auto-detects production paths and never falls back to baked assets on disk read failure. Dev mode uses fixed bundle revision `"dev"` and replaces the index placeholder with `dev` at request time.
- **Consequences**:
  - + Frontend devs can refresh the browser and see changed static assets without restarting the daemon.
  - + Default release/production behavior remains self-contained and independent of `web/` files on disk.
  - + The whitelist prevents path traversal and keeps dev mode scoped to known Team Mode Web assets.
  - − Dev mode incurs file IO per static request.
  - − Dev mode must be explicitly enabled before the daemon/web app starts; it is not a watcher or automatic stale-bundle detector.

## ADR-013: walk-ancestor `current_cc_pid` 替代裸 parent PID

- **Date**: 2026-04-29
- **Context**: Group 2 改造引入 `scripts/mcp-launcher.cmd` 透传 vcvars64.bat env 给 daemon 子进程后，进程链变成 `CC node.exe → cmd.exe(launcher) → team_mode_mcp.exe → team_mode_daemon.exe`。原有 `current_parent_pid()` 只走 1 层 parent，把 cmd.exe 的 PID 当作 CC PID 写进 `team.owner_cc_pid` 持久化。该错位 PID 触发 9 个连锁失败：daemon lead-watchdog 误判 CC 死亡（grace 15s 后自杀）、`scripts/hooks/lead-pending-wake.js` ancestor-chain 路由把所有 pending 归到 `othersRaw` 跳过、`prune_dead_owners` 误删 lead 消息、`team_list` 把活 team 标 `orphan`、worker reply silent notice 兜底机制被同链路卡死。详情见 `.plans/agent-teams-v2/docs/05-design-history/refactor/2026-04/refactor-status-2026-04-29.md` §1+§3 与 v5 计划文档新增的 9 bug 清单。
- **Decision**: 把 owner PID 解析提取到唯一函数 `crate::util::current_cc_pid()`，行为是从自身 parent 开始向上 walk 最多 8 层，跳过名字属于 `SHELL_WRAPPER_NAMES = [cmd, sh, bash, zsh, pwsh, powershell, conhost]`（大小写无关、`.exe` 后缀无关）的进程，返回第一个非 wrapper ancestor。`src/team_mode/mcp/tools.rs` 与 `src/team_mode_daemon/client.rs` 两处独立实现合并到这一处；`team_lifecycle.rs::team_create` 与 `DaemonToolClient::new` 都改用此函数。worker 进程spawn 的 MCP relay（识别条件：`TEAM_MODE_TEAM` env 已设）不再计算 owner，直接传 `None` 避免覆盖 lead 写入的 owner。
- **Consequences**:
  - + 单一函数定义，任何新 launcher / wrapper / sandbox 入口都不会再制造同类错位。
  - + `team.owner_cc_pid` 总是真实 CC PID（实测 `ownerCcPid:69156` = 当前 CC node.exe 的 PID），watchdog / hook / prune / web UI 全部路径自动恢复正常。
  - + worker relay 不再污染 owner 绑定（Bug 8 的零回归保证）。
  - − walk 函数依赖 sysinfo 全进程 refresh（一次性成本，结果被 caller 缓存到 `team.json`，不影响热路径）。
  - − wrapper 名称写死在常量里；引入新 wrapper（如 nu / fish）需要改这条常量。

## ADR-014: D16 worker 网络命令受限决策废止（→ D18）

- **Date**: 2026-04-29
- **Context**: D16（`.plans/agent-teams-v2/docs/05-design-history/refactor/2026-04/refactor-plan-2026-04-28.md` 决策日志）记录"daemon 模式 worker 跑 SSH/curl/DNS 因 codex 0.124-alpha PowerShell shell tool 触发 SSPI/AppContainer DNS 工作线程限制"，对应 workaround 是网络密集任务由 lead 替跑。本次会话末尾的 ADR-013 落地完成后，按用户要求做了完整复测：起 codex worker，分别用 `cmd /c` 包装路径与裸命令（codex 自报实际走 PowerShell）跑 nslookup / curl HTTPS / ssh git@github.com / ssh root@8.136.7.144（生产部署服务器）。8/8 全部 exit 0 / 期望行为；SSH 真实部署服务器 hostname / git remote / git log 全部正确返回，与 lead 直连结果完全一致。
- **Decision**: 推翻 D16，写入 D18：当前环境下 worker 网络命令（DNS / curl / SSH / 公网 API）全部可用，含裸 PowerShell 路径。`.plans/agent-teams-v2/docs/05-design-history/refactor/2026-04/refactor-status-2026-04-29.md §3.5` 标记原结论"已废止（2026-04-29 复测）"，保留历史段供回溯，但不再作为派单约束。`CLAUDE.md` / `AGENTS.md` 的 Runtime 约定原本就描述 worker 可跑 SSH / HTTPS，未受 D16 影响，无需改动；新增的 Known Pitfalls 段把"过时假设"明确写出，防止 lead 重新引用 D16。
- **Consequences**:
  - + Lead 可放心派部署 / 远程运维 / HTTPS API 抓取等任务给 worker，按 worker 容量并行而不是 lead 串行替跑。
  - + 减少 lead context 消耗（不再被远程命令输出灌满）。
  - − 真实根因（codex 上游修复 / launcher env 注入 / Windows token 上下文变化）未被追到具体提交。如果未来某次 codex 升级或环境变更导致问题复现，需要重新走 §3.5 复测流程并视情况恢复 D16。
  - − Worker 跑 SSH 时使用 `~/.ssh/` 默认 key，与 lead 共享凭证；对部署任务来说是优点（无需额外 key 管理），但意味着 worker 拥有与 lead 相同的服务器写权限，权限分隔需要靠 system_prompt + 任务描述 + 服务器侧的访问控制实现。

## ADR-015: codex worker 默认 reasoning_effort = high（显式 default，可被 worker_add 显式覆盖）

- **Date**: 2026-04-29
- **Context**: 之前 `src/backend/codex.rs::spawn` 仅在 `config.reasoning_effort.is_some()` 时通过 `-c` 注入 `model_reasoning_effort`，否则不传，落到用户全局 `~/.codex/config.toml` 的 `model_reasoning_effort` 值（用户机器实测为 `medium`）。本仓代码体量（Rust workspace + Web UI + Hook scripts）使 medium-effort 的 codex turn 经常 under-explore call graph、漏看 cross-file invariants，导致重派任务，net cost 反而高于一开始就 high。memory 里有条 user feedback「不要 hardcoded silently override user 全局 config」，但该 feedback 的关键词是 *silently* —— 文档里写明的项目级 default + 允许 worker_add 显式覆盖，不属于 silent override，与 feedback 本意相符。
- **Decision**: 在 `src/backend/codex.rs:121-124` 把 effort 注入逻辑改成 `let effort = config.reasoning_effort.as_deref().unwrap_or("high"); cmd.arg("-c").arg(...)`。`worker_add` 没传 effort → 注入 `high`；显式传 `medium` / `low` → 尊重原值不再 override。CLAUDE.md / AGENTS.md Runtime 约定段加一条 1 行说明，让 lead 与 codex worker 都能从项目级 instructions 里看到该 default 的存在与覆盖方式。
- **Consequences**:
  - + 本仓默认 high-effort，复杂任务一次成功率提升，减少重派 round-trip cost。
  - + 显式 default 比 silent 全局覆盖更透明，user 仍可通过 `worker_add(effort=...)` 实时降级。
  - + 文档化在 Runtime 约定（CLAUDE.md/AGENTS.md byte-equal 同步），保证 worker 自身也能从注入 instructions 读到该 default。
  - − 提高单次 worker 任务的 codex usage cost（API 配额消耗加快）；高强度并发场景需关注 quota（参考本会话末尾因 ChatGPT Plus 配额超限导致 PoC 实测被卡死）。
  - − 与 user 全局 `model_reasoning_effort = "medium"` 不一致；用户其他工程跑 codex 默认仍是 medium，仅本仓内 daemon 启的 worker 是 high。

## ADR-016（HOLD）: AGENTS.md → CLAUDE.md 合并 PoC 等待 quota 恢复

- **Date**: 2026-04-29
- **Context**: 用户提出"让 codex worker 也读 CLAUDE.md，合并掉 AGENTS.md 双写"。WebSearch 调研确认 codex 0.125 的 `~/.codex/config.toml` 顶层字段 `project_doc_fallback_filenames = ["CLAUDE.md"]` 是正式支持的回退机制（来源：[OpenAI Codex AGENTS.md guide](https://developers.openai.com/codex/guides/agents-md) + [thepromptshelf 2026 setup guide](https://thepromptshelf.dev/blog/agents-md-codex-setup-guide-2026/)）。本会话做 PoC：临时往 `~/.codex/config.toml` 加该字段，`mv AGENTS.md AGENTS.md.poc-bak`，spawn 一个 worker 让它复述 CLAUDE.md "Known Pitfalls" 第一条来验证 fallback 是否真生效。worker 启动 OK（说明 codex 没在解析阶段拒绝该字段），但生成回复时报 `usageLimitExceeded`（"You've hit your usage limit. Upgrade to Plus..., or try again at May 6th, 2026"）—— ChatGPT Plus 配额超限，2026-05-06 才恢复，PoC 端到端验证被阻塞。
- **Decision**: PoC 状态 hold。已回滚（`~/.codex/config.toml` 还原 + `AGENTS.md` 恢复 + 项目代码未动）。等 ChatGPT Plus 配额恢复后重跑 PoC：worker 正确复述 CLAUDE.md 内容 → 字段生效 → 进入完整改造（codex.rs `ensure_global_codex_mcp_config` 注入字段、删 AGENTS.md、改 GR-6、清 14 处文档引用、改 CLAUDE.md 头部双写说明）。worker 复述失败或拿不到任何项目级 instructions → 字段被 codex 静默忽略 → 放弃合并方案，保持双写 + 现有 GR-6。
- **Consequences**:
  - + 字段名调研已确认（无需重做），落地阻塞仅在 PoC 实测一步，恢复后短时间内可推进。
  - + 已记录"worker 启动接受字段"的 partial signal；如果未来需求紧迫可基于此低强度信任做合并，但仍建议正式实测才落地。
  - − 双写约束（C4 决策）当前继续生效，lead 改 CLAUDE.md 后仍需 `cp CLAUDE.md AGENTS.md`，GR-6 仍 enforce byte-equal。
  - − PoC 备份文件（`~/.codex/config.toml.bak.*` / `~/.codex/config.toml.poc-trace`）残留在用户目录，可选择保留作为回滚锚点或手动清理（不影响项目）。

## ADR-017: MCP 进程加 parent CC liveness watchdog + 启动 zombie sweep

- **Date**: 2026-04-30
- **Context**: 项目根 `tasklist | grep team_mode_mcp.exe` 累积到 11+ 个僵尸（实际清出 20 个）。每次 `/mcp` reconnect 启动新 MCP 后旧的不死，连锁导致：(a) 多 MCP 抢同一 `runtime/daemon.json`，连接握手错乱；(b) tool schema 推不过来用户体验为 `"No such tool available"`；(c) team 文件 `owner_cc_pid` 被错误 PID 绑定，team 永远 orphan。根因：`src/bin/team_mode_mcp.rs` 只靠 `runtime.run_stdio()` 的 stdin EOF 感知 CC 父进程死亡，但 Windows 上当 CC 异常崩溃 / 被 taskkill /F / 进程组中断时 stdin pipe 的 EOF 不可靠送达，MCP 进程 hang 在 `read_json_rpc_message()` 系统调用上永不退出。daemon 已有 `lead-watchdog`（5s 轮询所有 team owner_cc_pid + 15s grace 自杀）所以不会累积；MCP 缺这一层。
- **Decision**: MCP bin 加 2 层防御。(1) 启动时 sweep 所有 `team_mode_mcp` peer 进程，对每个用 `util::resolve_cc_pid_from()`（从 `current_cc_pid` 抽出的复用助手）走 ancestor chain 找 CC PID；CC 不在 / 已死 → `Process::kill()`。只杀名字精确匹配 `team_mode_mcp(.exe)?` 且 parent CC 死的；活 CC 的 peer（其他 CC 会话）一律不动。(2) spawn 后台线程 `mcp-parent-liveness`，每 5s `sysinfo` 检查启动时记录的 owner_cc_pid 是否还在；不在 → log warn + `process::exit(0)` 立刻退出。无 grace（daemon 自己有 15s grace 防 `/mcp reconnect` 抖动；MCP stdio 协议预期 CC 自动重启 MCP）。`current_cc_pid()` 返 None → 跳过 watchdog（不能瞎杀自己）。
- **Consequences**:
  - + MCP 进程 ≤5s 跟随 CC 父进程死亡，与 daemon 节奏对齐；不再产生 zombie。
  - + 每个新 MCP 启动顺手清存量 zombie peer，自愈不需要 user 手动 taskkill。
  - + 复用 `current_cc_pid()` 的 walk-ancestor 算法（跳 SHELL_WRAPPER_NAMES，最多 8 层），与 Bug 1-9 治本修复 F1（ADR-013）保持一致语义。
  - − sysinfo 全 process refresh 每 5s 一次，CPU 开销 <1% 但有持续 syscall。可接受。
  - − PID recycling 风险：CC 死后 5s 窗口内若新进程恰好复用 CC PID，watchdog 误判 CC 还活。Windows PID 回收较慢、且复用进程不会恰好叫 node.exe，本次不解决。后续若需要：watchdog tick 加进程启动时间戳对比即可。
  - − 启动 sweep 用 `proc.kill()` 强杀 zombie，无 graceful shutdown 通道。可接受（zombie 已 hang 在 read，无消息要送达）。

## ADR-018: sweep/watchdog 用 ancestor 链 + 进程名验证（治本 ADR-017 PID 复用 + worker MCP relay 误杀）

- **Date**: 2026-04-30
- **Context**: ADR-017 落地后实测 `tasklist` 仍累积到 17 个 `team_mode_mcp.exe`，且新 sweep 报 `killed=0 spared=14/15/16`。逐个验证 mcp.log 里 12 个不同的死 `owner_cc_pid`：全部已死，但 `sys.process(p).is_some()` 全部返 `true`——本机 252 个 `node.exe` 进程（多 IDE/CC/Node tooling）让 PID 复用率极高，ADR-017 §Consequences 末尾已记录的 "PID recycling 罕见" 假设破灭。同时发现第二个独立 BUG：每个 codex worker 通过 daemon spawn 时会启动自己的 MCP relay（`mcp.log` 里 17:07:08-19 连续 7 次 "Team Mode MCP server starting"），它们的进程链是 `worker codex.exe → ... → team_mode_daemon.exe → ...`，没有 CC ancestor。ADR-017 sweep/watchdog 不区分 lead MCP 与 worker MCP relay，对 worker relay 跑会让 sweep 误杀活 worker（`owner_cc_pid` 是 fallback wrapper PID，跟 CC 无关），watchdog 误自杀（pid recycle 时偶发判活则不死，常态下应判死）。
- **Decision**: 三层修复，落地在 `src/bin/team_mode_mcp.rs`。(1) 引入 `TRUSTED_OWNER_STEMS = ["node", "claude", "team_mode_daemon", "codex"]` 和 `peer_has_trusted_ancestor(mcp_pid, sys)` —— sweep 走每个 peer mcp 的 parent 链最多 12 层，遇到 stem 在 trusted 集中的活进程立即 spared，否则 kill。换掉之前 `sys.process(cc_pid).is_some()` 的伪判活。(2) 引入 `CC_BIN_STEMS = ["node", "claude"]` 和 `cc_pid_alive(cc_pid, sys)` —— watchdog 不仅检查 PID 是否在系统进程表里，还要求该 PID 对应进程的 stem 是 CC 主进程名。Windows PID 复用给非 CC 进程时立刻判死。(3) `main()` 检测 `TEAM_MODE_TEAM` env，set 则跳过 sweep + watchdog —— worker MCP relay 的生命周期由 daemon `kill_on_drop=true` + codex stdin EOF 管理，不需要这两层防御；lead MCP（CC 直接启动，不带 `TEAM_MODE_TEAM`）继续保护。
- **Consequences**:
  - + Sweep 不再被 PID 复用欺骗：trusted 集走的是 ancestor 链上**任一**进程的 stem 匹配，PID 复用给非 CC/daemon/codex 进程时直接判死。
  - + Worker MCP relay 自动安全：(a) 它们不跑 sweep，自然不主动杀活 peer；(b) lead MCP 的 sweep 走它们的 chain 时会找到 daemon 或 codex stem，spared。
  - + 与 ADR-013 worker relay 识别（`TEAM_MODE_TEAM` env）保持单一约定，新代码无平行识别机制。
  - + Watchdog 增强：高密度 node.exe 环境下 PID 被复用给随机 binary（codex/cmd/python/etc）时立刻退出而非空转。
  - − `TRUSTED_OWNER_STEMS` 写死。如果未来 daemon 可执行被改名（如重命名为 `agent_teams_daemon`），sweep 会把所有 worker MCP relay 误杀。维护契约：改 daemon 二进制名时同步本常量。
  - − sweep 走 ancestor 链每层 sysinfo lookup，最多 12 层 × peer 数。Windows 进程数 ~500-1000 时单次 sweep <50ms，可接受（启动一次性成本）。
  - − Worker MCP relay 现在没有任何 self-watchdog；完全依赖 daemon kill_on_drop + codex stdin EOF。已知 EOF 不可靠（这就是 ADR-017 的起因），但 daemon 自己的 lead-watchdog 5s 死链路保护 worker codex 跟随 lead 死，从而间接限制 worker MCP relay 寿命。可接受。

## ADR-019: `message.rs` inbox 子模块拆分继续执行 700 行警戒线

- **Date**: 2026-04-30
- **Context**: ADR-011 把 MCP tools 聚合文件按稳定责任边界拆分，并约定子模块超过 700 行时应主动再拆。后续功能增长后 `src/team_mode/mcp/tools/message.rs` 已到 716 行，超过 700 行警戒线但尚未触发 1200 行 hard fail。该文件同时包含 `send_message` 调度路径和 `inbox_read` 兜底查询路径，二者共享少量 tool helpers 但用户路径独立。
- **Decision**: 保持 `send_message` 主调度逻辑留在 `message.rs`，把 inbox 兜底读取实现移动到 `src/team_mode/mcp/tools/message/inbox.rs`。不改变 MCP tool 名称、参数、响应 JSON、可见性或错误语义；只通过 Rust 模块边界降低文件大小并保留现有 helper 复用。
- **Consequences**:
  - + `message.rs` 从 716 行降到 633 行，重新低于 700 行警戒线。
  - + `inbox_read` 的 fallback/auto_ack/hint 逻辑有独立落点，后续改 inbox 语义不再挤压 dispatch 路径。
  - + 无新依赖、无 API 改动、无运行时行为变更。
  - − `message` 模块多一层 `message/inbox.rs` 文件，维护者需要知道 inbox tool 实现在子模块里。

## ADR-020: Team Mode 全切本地 Streamable HTTP service，退役 stdio MCP relay / daemon RPC

- **Date**: 2026-04-30
- **Context**: BUG-5 显示 Claude Code ESC / stdin EOF / 进程组中断会让 stdio MCP relay 与 daemon 间接层出现断连和僵尸风险。ADR-017/018 已给旧 stdio MCP 加 watchdog/sweep，但仍是在不可靠 stdin 生命周期上补防线。用户明确拍板"全切 HTTP / 不计成本 / 一次性做完"：lead 与 worker 都走本地 HTTP MCP，旧 `team_mode_mcp.exe` worker relay 与 `team_mode_daemon.exe` TCP RPC 间接层退役。
- **Decision**: 新默认 binary 命名为 `team_mode_service.exe`。它绑定 `127.0.0.1:8786`，直接拥有 `TeamModeToolset`、worker orchestrator、lead watchdog、worker-liveness watchdog，并预启动 8787 Web UI。MCP transport 落在 `src/team_mode/mcp/http_transport.rs`，提供 Streamable HTTP `POST /mcp` JSON-RPC、`GET /mcp` SSE、`DELETE /mcp` session termination；强制 bearer token、localhost bind、Origin 校验。启动时写 `.agent-teams/runtime/http-mcp.json` 和 token file。Claude Code `.mcp.json` 使用 HTTP + `scripts/mcp-http-headers.js` 动态 headers；Codex worker 全局 managed config 改为 `url` + `bearer_token_env_var` + `env_http_headers`，由 worker spawn env 注入 team/member/worker id/token。`axum` / `tower` / `tower-http` 进入默认 dependencies，不再 feature gate。
- **Consequences**:
  - + Lead/worker MCP 生命周期不再依赖 stdin EOF；ESC 不会杀 service，也不会产生 `team_mode_mcp.exe` 僵尸。
  - + 去掉 stdio relay → daemon TCP RPC 间接层；HTTP service 直接执行 toolset，减少 owner/caller context 多跳漂移面。
  - + Codex worker 不再生成 per-worker MCP relay，ADR-018 的 worker relay 误杀/保活问题随默认路径消失。
  - + `http-mcp.json` + token file 给 headersHelper、service script、swap/status 提供稳定发现点；token 留在 `.agent-teams/runtime/`，不入 git。
  - − 新增常驻 HTTP 端口 8786；如果端口被占用，需要显式改 service 启动参数和 `.mcp.json` URL。
  - − `axum`/`tower` 进入默认依赖，最小构建图变大。
  - − 旧 `team_mode_mcp.rs` / `team_mode_daemon/*` 源码仍保留作紧急回滚，短期内存在双路径维护成本；默认 setup/swap 不再使用它们。

## ADR-021: Team Mode service lead-watchdog 降级为 observability + HTTP 工具调用走 spawn_blocking

- **Date**: 2026-04-30
- **Context**: ADR-020 落地后实测两个新 bug：(1) **service 跟随 stale CC PID 自杀**：`team_mode_service` 启动时把 ADR-018 lead-watchdog 直接照搬过来——每 5s 用 `sysinfo::Process` 看 `team.json::owner_cc_pid` 是否还活，连续 3 次（15s）判死就 `process::exit(0)`。HTTP service 的设计承诺是"durable，独立于 CC 起落"，watchdog 这条规则与新架构语义矛盾——CC 重启 / Cursor 切换时 PID 变了，team.json `ownerCcPid` 还指向上一个会话的死 PID，watchdog 立刻把 service 也带走。实测 swap 后 user `/mcp reconnect` 永远 `Failed`，因为 service 在初始化握手 6-15s 后被自己的 watchdog kill。(2) **HTTP `worker_add` tool 触发 tokio nested runtime panic**：`tokio-rt-worker panicked: Cannot start a runtime from within a runtime`。所有 sync tool handler 用 `self.async_runtime.block_on(...)` 把 sync→async 桥起来（worker.rs / message.rs / team_lifecycle.rs 共 ~10 处），在旧 stdio MCP 中没问题（外层不是 tokio runtime）；HTTP service 在 axum/tokio 多线程 runtime 上跑，axum handler 直接调用 sync `handle_payload` → tool → `block_on` → tokio 拒绝在 runtime 内嵌套 runtime → panic → 该 worker thread 中毒，后续相同 endpoint 的请求永久失败。
- **Decision**: 两条独立修复，落地在新 service 二进制。(1) `src/bin/team_mode_service.rs::run_lead_watchdog` 删 `process::exit(0)`，保留循环 + 日志：threshold 触达只写一条 `event=lead_watchdog.observation teams_total=N` info log。Service lifecycle 由 `team-mode-service.ps1 stop` / 显式 shutdown / 操作系统杀进程控制。`_toolset` 参数加下划线表示故意保留（未来若加"无 active session N 分钟则 idle-shutdown"重新接线）。(2) `src/team_mode/mcp/http_transport.rs::post_mcp` 把 `handle_payload` 包进 `tokio::task::spawn_blocking` —— sync tool dispatch 跑在 blocking 线程，看不到 enclosing tokio runtime，内部 `block_on` 不再 panic。Mutex lock 移进 blocking closure，避免在 await 点持锁。`Arc<Mutex<TeamModeMcpRuntime>>` 已有 `Clone`，不需要协议层改动。
- **Consequences**:
  - + Service 真正 durable：CC 重启 / Cursor 切项目 / 多 CC 同 cwd 都不让 service 退；不再需要手工 patch `team.json::ownerCcPid`。
  - + HTTP `worker_add` 等所有 sync tool 在 service 上稳定工作；`block_on` 嵌套 panic 类问题全 endpoint 一并消失（spawn_blocking 是覆盖 entire dispatch 的统一入口）。
  - + ADR-018 watchdog 保留为可观测性信号——日志 `lead_watchdog.observation` 给未来"如多团队全 idle N 小时则建议管理员停 service"的策略一个落点。
  - + 没有新依赖；spawn_blocking 走 axum 默认的 tokio 运行时配置，无需额外 worker pool 配置。
  - − Service 不再自杀，意味着没有 owner 也会一直占着 8786 端口和文件句柄。`team-mode-service.ps1 stop` 与显式 shutdown 仍是干净的下线方式；用户责任更显式。
  - − `spawn_blocking` 默认线程池上限 512 (Tokio 默认)，并发 500+ 个 long-running tool 同时跑会等队，但本仓 lead 只有 1 个 + 6 worker，不会触及。
  - − Watchdog log 多了一条 info 行/15s（仅在 owner 死时打），老 `team-mode-service.log` consumers 需要忽略它，不当作 fatal。

## ADR-022: Lead-pending hook 重构为 per-team 文件 + asyncRewake，砍 PowerShell ancestor walk

- **Date**: 2026-04-30
- **Context**: 旧设计把 `lead_pending.jsonl` 放在项目根（单文件，跨所有 CC 共用），hook 启动时跑 PowerShell `Get-CimInstance Win32_Process` (~4.2s) 走 ancestor 链找当前 CC PID 然后 classify mine/others。三个连环问题：(1) Stop hook 用 sync shepherd-loop 模式跑 7200s，是不存在的 CC 特性——PowerShell 4.2s 期间 hook 被 CC SIGKILL（不是 SIGINT，无法 graceful），probe log 死在 `before-getAncestorPidSet` +5ms 没下文；(2) 跨 hook 共享锁 `.lead-pending.lock` 在高密度 PID 复用环境下 stale 不被清理，mid-turn 全 lock contention skip；(3) 单文件强制 hook 端做 routing，导致每个 hook fire 都跑一次 PowerShell。WebFetch 官方 docs `code.claude.com/docs/en/hooks` 确认 `asyncRewake: true` 才是"等异步事件唤醒 idle CC"的正确机制——hook 后台跑、exit 2 + stderr 唤醒。文档进一步证实：FileChanged hook 只 "Shows stderr to user only" 不注入；`timeout: 7200000` 写错单位（文档明确 "Seconds"），实际是无效或被截。
- **Decision**: 三层架构翻转。(1) **Per-team 文件**：service 写 `<base>/<team_id>/lead_pending.jsonl`，路径自带 routing 信息；新 entry 不再带 `owner_cc_pid` 字段（路径已编码）。`team_delete` 通过 `TeamStore::delete()` 的 `fs::remove_dir_all` 自然清理整个 team dir 含 pending 文件，不再需要 `prune_team`/`prune_dead_owners`。`LeadPendingWriter::with_legacy_root(project_root)` 配置一次性 migration，service 启动时把旧 `<root>/lead_pending.jsonl` 按 team 分发后删原文件。(2) **HTTP `/lead-pending/my-teams` endpoint**：`http_transport.rs` 加 `GET /lead-pending/my-teams?pid=<n>&session_id=<sid>`，service 内部 `resolve_cc_pid_from(pid, &sysinfo)` 走 ancestor 链找出 CC PID，比对 `TeamStore::list()` 的 `owner_cc_pid`，返回 `{cc_pid, teams: [{id, pending_path}]}`。Service 是常驻进程，sysinfo refresh 一次 cache 永远；hook 端 0 PowerShell。(3) **Hook 重写**：`lead-pending-async-wake.js` 152 行，settings.json `Stop` 加 `asyncRewake: true` + `timeout: 7200`；hook 启动 fetch `/my-teams`，poll 各 team 的 pending 文件 500ms，命中后 batch grace 2s 等同窗 burst 后 stderr + exit 2。`lead-pending-mid-turn.js` 192 行，PostToolUse sync 路径用 `.cc-identity.<session_id>.json` cache 避免每次 tool call 都打 service。**无 fallback**：service 不可达 → 写 stderr + exit 1，错误暴露不掩盖。删除 `lead-pending-wake.js` (891) 和 `lead-pending-shared.js` (404)。
- **Consequences**:
  - + Hook 端 PowerShell 完全消失：mid-turn 每次 fire 0-9ms（cache hit）/130-200ms（cache miss HTTP 解析），vs 旧版本 ~4200ms（PowerShell）—— 提升 ~500x。
  - + Stop hook 不再被 SIGKILL：asyncRewake 后台模式 + 短命循环（命中 exit 2 / 7200s timeout）。实测 hook 持续 polling 6 分 6 秒（715 次 poll）成功命中 worker reply 不死。
  - + 跨 hook 锁完全消失：每 CC 自己的 team file，多 hook 实例并发 poll 同文件靠 `fs.writeFileSync` truncate 的原子性做 drain race，赢家拿 entries 输家见空继续 poll。无双注入。
  - + Service 端 routing 准确性：写时分文件，hook 完全不需要 classify。CC 重启后 team.owner_cc_pid 经 team_create rebind，pending 文件位置不变，老消息不丢。
  - + 净删 ~1200 行代码（hook 891+404，+ 新增 ~150 行 hook + ~130 行 service endpoint）。
  - + 8 个状态文件砍到 1 个 cache（`.cc-identity.<sid>.json`）+ 2 个 probe（`.async-wake-probe.log` / `.mid-turn-probe.log` 保留作诊断）。
  - + 测试覆盖：`lead_pending` 11 个单测（per-team write、separate teams、migrate_legacy 含 unrouted forensic、UTF-8 边界）；`team_mode_mcp_http` integration 含新 endpoint signature；workspace 全部 13 个 test bin pass。
  - + 实测端到端：3 worker 慢回复（sleep 180/200/220s）错峰 20s 三次独立 stderr+exit 2 唤醒，全部 reply 注入。
  - − Service 必须 alive：hook 启动 query `/my-teams` 失败立即 exit 1（无 fallback）。Service down 时 hook 不工作；但 ADR-021 已承诺 service durable，且无 fallback 是为了"错误暴露不掩盖"的明确策略选择。
  - − Hook 跨 session 共享 cache：`.cc-identity.<sid>.json` 用 session_id 隔离不同 CC 实例，但写入是 best-effort（rename 失败默默吞掉）；极少数情况 cache 写不进会导致下次 mid-turn 重新打 service（性能略降不影响正确性）。
  - − Multi-CC 同 cwd 路由依赖 `team.owner_cc_pid` 在 team_create 时正确写入。该 PID 来源是 HTTP header `X-Team-Mode-Owner-CC-Pid`（由 `scripts/mcp-http-headers.js` 的 PowerShell ancestor walk 计算），所以本质上 PowerShell 还在，但只在 MCP 配置阶段跑一次而不是每个 hook fire。

## ADR-023: `team_create` 对已存在 active team 执行 ownerCcPid rebind

- **Date**: 2026-04-30
- **Context**: ADR-022 后 Stop hook 通过 HTTP `/lead-pending/my-teams` 把当前 hook 进程解析到 CC PID，再匹配 `team.json::ownerCcPid` 决定要 poll 哪些 per-team pending 文件。HTTP service 是 durable process，Claude Code 重启或切会话时 service 不重启，已有 team 的 `ownerCcPid` 可能仍指向旧 CC PID。旧 `team_create(name)` 对已存在 team 走 duplicate/no-op 路径，导致新 CC 无法通过重新创建同名 team 接管 owner，hook 查询返回 `count=0`，worker reply 永久不能自动注入，必须手工编辑 `team.json`。
- **Decision**: `TeamService::create` 对“id/name 都匹配且 status=active”的已存在 team 改为 idempotent rebind：当请求携带新的 `owner_cc_pid` 且与现值不同，更新 `owner_cc_pid` 并刷新 `updated_at`；当 owner 不变或请求未解析出 owner 时返回现有 team，不制造 `updated_at` 抖动。`team_create` 在发现同名 team 已存在时跳过 orphan cleanup，避免把旧 team 目录删除重建；lead member 只在缺失时补建。新建 team 仍走原有 one-live-team/orphan cleanup 规则，worker_add/worker_list/team_delete 和 `/lead-pending/my-teams` 语义不变。
- **Consequences**:
  - + CC 重启 / Cursor 切会话后再次调用 `team_create(name)` 即可把 hook 路由 rebind 到当前 CC，不再手改 `team.json`。
  - + Existing active team 的 messages/members/lead_pending 文件保留，避免旧 orphan cleanup 把同名 team 当死 team 删除重建。
  - + 同一 CC 重连重复 `team_create` 不刷新 `updated_at`，减少 dashboard / docs freshness 类观察面的无意义抖动。
  - − 同 cwd 多 CC 同名 team 现在是 last-writer-wins owner 模型；这与 `Team` 域注释里的 "created/last-took-over" 语义一致，但 reviewer 若要审多 CC 并发，需要把 `team_create` 视为显式接管操作。

## ADR-024: Team Mode Web events/diagnostics 适配 per-team lead_pending 文件

- **Date**: 2026-04-30
- **Context**: ADR-022 把 worker→lead push queue 从 legacy root/base-dir `lead_pending.jsonl` 迁到 canonical per-team `<base>/<team_id>/lead_pending.jsonl`，让路径自带 routing 信息并砍掉 hook 端 classify。Web events/diagnostics 仍读取旧 root single-file：dashboard `fileChanged` 看不到新 per-team pending 写入，diagnostics 也缺少当前真实 source；如果直接 fallback 旧 root 文件触发 events，又会把任意 legacy single-file 变化广播给当前 team，制造跨 team false positive。
- **Decision**: Web events endpoint 已按 `/api/teams/{team}/events` scoped，因此继续使用单 cursor，只把 lead-pending watermark 改为该 team 的 canonical `<base>/<team>/lead_pending.jsonl`。`fileChanged.payload.path` 改为相对 `<team>/lead_pending.jsonl`，不泄漏绝对路径。Events 不 fallback legacy root/base-dir single-file；legacy compatibility 由 service-side `migrate_legacy()` 和 diagnostics 可见性承担。Diagnostics sources 新增 canonical per-team file，同时保留 project-root/base-dir legacy single-file sources 作为 migration/forensic evidence。
- **Consequences**:
  - + Dashboard polling/SSE 重新能看到 ADR-022 per-team pending 写入，且不消费/截断 pending 文件。
  - + 单 cursor 语义保持稳定，cursor 字段名 `leadPendingSize` / `leadPendingModifiedAt` 不变；无需 glob `<base>/*/lead_pending.jsonl` 或 per-team cursor map。
  - + Legacy root/base-dir files 仍在 diagnostics 里可见，便于排查 migration 残留，但不会触发 events。
  - − 如果 service migration 未运行且只有 legacy root single-file 变化，Web events 不会报告它；这是有意取舍，用来避免跨 team false positive。

## ADR-025: Reviewer BLOCK 修复：lead-pending migration 去重 + HTTP service PID/ancestor fail-closed

- **Date**: 2026-04-30
- **Context**: ADR-020/022 reviewer BLOCK 指出两个生产风险面：(1) `/lead-pending/my-teams` 是 Stop/PostToolUse hook 的核心 routing contract，但缺少直接 endpoint 覆盖；`migrate_legacy()` 在 per-team append 成功后如果 legacy 删除失败，重启会重复 append 已迁移行；(2) `scripts/team-mode-service.ps1` 只凭 runtime JSON PID 判断 running/stop，遇到 Windows PID 复用会误报 service 活着或 `Stop-Process -Force` 杀错进程；`scripts/mcp-http-headers.js` 在 ancestor parent row missing 时会退回未验证 `current.ppid`，重新引入裸 parent PID 风险。
- **Decision**: 四项硬化同时落地。(1) `team_mode_mcp_http` 增加 `/lead-pending/my-teams` 集成覆盖：matching owner、多 team、0 team、query PID 优先于 owner header、Bearer token auth、response shape。该 endpoint 继续以 query `pid` 为唯一 owner 解析入口，owner header 不参与 my-teams 判定。(2) `LeadPendingWriter::migrate_legacy_path` 在写入 per-team file 前读取目标文件已有 `msg_id`，对已存在 msg_id 跳过 append；legacy 文件即使因旧失败残留，重试也不会制造重复 pending entry。(3) `team-mode-service.ps1` 的 runtime PID 必须对应 `ProcessName == team_mode_service` 才可信；`status`/`stop` 还要通过 authenticated `/mcp initialize` probe，probe 失败时 `stop` 拒绝 kill。(4) `mcp-http-headers.js` 抽出可测试 ancestor walk；parent row missing 时返回空 owner PID，不再发裸 `process.ppid`。
- **Consequences**:
  - + Hook routing endpoint contract 有直接回归测试，response shape 与 auth 行为稳定。
  - + Legacy migration 可安全重试：partial-success 后 legacy 残留不会重复注入已写入的 `msg_id`。
  - + Service wrapper 不再信任 stale runtime PID，降低 PID 复用造成误杀/误报风险。
  - + Headers helper 与 Rust owner identity 规则重新对齐：缺失 process row 时 fail closed，宁可不 rebind，也不绑定未经验证的 wrapper/stale PID。
  - − 如果 parent snapshot race 导致 helper fail closed，`team_create` 可能本轮不 rebind owner；用户/lead 可重试，且不会把错误 PID 写入 `team.json`。

## ADR-026: `team_mode_service init` 支持全局安装后的任意项目初始化

- **Date**: 2026-05-01
- **Context**: 用户目标是自用 MVP：执行 `cargo install --path .` 后，在本机任意项目目录 `cd <project> && claude` 都能使用 team-mode。ADR-020/022 后 service binary 已经能通过 `--project-root` / `--data-dir` 指向目标项目，Web UI 也 baked 在 binary 中；剩余不可移植点是 `.mcp.json`、`.claude/settings.json`、headers helper 和 hook scripts 仍依赖本仓 repo-relative `scripts/` 路径。用户明确不做 release binary CI、跨平台、开源完整化，也不希望污染目标项目 git。
- **Decision**: `team_mode_service` 新增 `init [<target-project-dir>]` subcommand（默认 current dir）。Binary 通过 `include_str!` 内嵌 `scripts/mcp-http-headers.js`、`scripts/hooks/lead-pending-async-wake.js`、`scripts/hooks/lead-pending-mid-turn.js`，init 时写到 `<target>/.agent-teams/scripts/`。`init` 会写入/merge `<target>/.mcp.json` 的 `mcpServers.team-mode`，headersHelper 固定为 `node .agent-teams/scripts/mcp-http-headers.js`；写入/merge `<target>/.claude/settings.json` 的 Stop asyncRewake hook 和 PostToolUse mid-turn hook，命令使用 `.agent-teams/scripts/hooks/...`；并在 `.gitignore` 追加 `.agent-teams/`。若目标脚本已存在、`.mcp.json` 已有 `team-mode` server，或 settings 已有 lead-pending hook，则 fail closed 并要求用户手动 merge；本版不实现 `--force`。
- **Consequences**:
  - + `cargo install --path .` 后不再要求用户 clone repo 才能在新项目使用 team-mode；helper/hook 随 binary 自包含落盘。
  - + 目标项目只新增 project-local `.agent-teams/`、`.mcp.json`、`.claude/settings.json` 和 `.gitignore` 条目；`.agent-teams/` 默认被 gitignore。
  - + Merge 策略保守：已有 team-mode/hook 配置不自动覆盖，避免破坏用户项目现有 Claude Code 配置。
- − `.mcp.json` / `.claude/settings.json` 仍是每项目文件，首次 init 后必须完整重启 Claude Code 才会加载。
- − 当前仍是 Windows-only MVP；hooks 继续通过 Node 脚本运行，用户机器仍需 Node 可用。

## ADR-027: v3 全局 MCP 架构 — lazy-spawn relay + global runtime + install-global

- **Date**: 2026-05-02
- **Context**: ADR-020~026 把默认控制面、hook 路径和 project-local init 跑通了，但 per-project 置入仍要求用户理解 `.mcp.json` / `.claude/settings.json` / project-local runtime 的组合；这对“在这台机器上的任意项目直接可用”还不够顺手。v3 的目标是把 relay、service、runtime install 和 migration 组织成一条清晰的机器级路径，同时保留 init 作为 isolation fallback。
- **Decision**: 架构分三段：1) **relay** 只负责 stdio JSON-RPC 转发与 service 就绪探测，必要时 lazy-spawn service；2) **service** 负责 durable HTTP MCP、global runtime、file lock、`/healthz` 和 worker orchestration；3) **per-team project_root** 仍是业务上下文的一部分，由调用项目的 `CLAUDE_PROJECT_DIR` / cwd 解析，不和 runtime 安装路径绑死。`install-global` 负责把机器级 Claude 配置接到这条链路上，`init` 保留为项目隔离路径，不互相替换。
- **Core flow**:
  - relay 先看全局 runtime，再兼容旧 project-local runtime 发现。
  - service 发现缺失时可被 relay lazy-spawn，随后通过健康探测确认 runtime / lock-holder 状态。
  - `install-global` 负责把全局 Claude 配置指向 relay / hooks，用户以后只需 `cd <project> && claude`。
- **Key decisions**:
  - runtime 默认放在 `~/.team-mode/runtime/`，而不是绑定当前项目目录。
  - 配置合并采用 fail-closed 语义，避免覆盖用户已有的 MCP server / hook 配置。
  - hook 以 Rust subcommand 替代 `.js` 入口作为默认全局路径；旧 `.js` 仍保留给历史 init 用户。
  - init 与 install-global 并存：前者服务于项目-local isolation，后者服务于机器级默认体验。
- **Trade-offs**:
  - + 更符合“装一次、到处用”的用户目标，降低新项目上手摩擦。
  - + relay / service / project_root 职责更清楚，调试时更容易判断问题属于哪一段。
  - − 机器级配置更显式，用户若想隔离仍需选择 init。
  - − 维护两条安装路径带来文档和测试的额外同步成本。
- **Relation to ADR-019~026**:
  - ADR-019~025 提供了消息、HTTP service、hook 路由和 owner rebinding 的底座。
  - ADR-026 证明了 project-local init 可移植，但只覆盖“当前项目可用”。
  - ADR-027 在此之上把默认体验上移到机器级 install-global，同时保留 ADR-026 作为兼容与隔离路径。

## ADR-028: `/lead-pending/my-teams` 信任 caller-supplied CC PID，不再二次 ancestor walk

- **Date**: 2026-05-03
- **Context**: v3 install-global 在用户 IDE 终端（Cursor、VS Code 等）里实测时 hook 路由全失效。复现：`team.json::ownerCcPid = 94516`（`node.exe` = 真实 CC），但 `GET /lead-pending/my-teams?pid=94516` 返回 `cc_pid=34536, teams=[]`，34536 是 `Cursor.exe`。根因是 `get_my_teams` handler 把 hook 端 walked 出来的 CC PID 又传给 `resolve_cc_pid_from`，而 `resolve_cc_pid_from` 的语义是“从 start 的 parent 起跳过 wrapper 找 CC”，于是从 node 跨到 IDE host。`team_create` 路径直接用 HTTP header 里的 `X-Team-Mode-Owner-CC-Pid`（已被 relay walked），不再 walk，因此 owner 写入正确，但 routing 永远 mismatch。
- **Decision**: `/lead-pending/my-teams` 把 query `pid` 视为已经解析过的 CC PID，直接用它匹配 `team.owner_cc_pid`，不再调用 `resolve_cc_pid_from`。Sanity check 用 `SHELL_WRAPPER_NAMES` 拒绝明显的 wrapper PID（`cmd` / `pwsh` / `bash` 等），让坏 caller 早 fail 而不是静默 mis-route。`SHELL_WRAPPER_NAMES` 由 `pub const` 在 `crate::util` 暴露，给 handler 复用。
- **Why not alternatives**:
  - “继续 walk + 再加 positive 白名单”：每加一个 IDE 都要补白名单（Cursor、VS Code、Cmder、Windows Terminal、tmux session…），脆且无穷无尽。
  - “两条调用都直接信 caller PID”：team_create 已经如此（依赖 relay 端 walked header），保持对称即可。
  - “my-teams 接受多个 candidate 同时匹配”：会让“同名 active team rebind”这条 ADR-023 路径出现歧义，弃。
- **Consequences**:
  - + IDE 终端用户立即可用，不再被 ancestor walk 越级到 IDE 进程。
  - + Service 端只信 caller 的 walked PID，符合“each step 只 walk 一次”的原则。
  - − Caller 必须自己 walk past wrappers；relay / hook 的 `current_cc_pid()` 已经做了，但任何新的 caller 也得遵守。
  - − Sanity check 让显式 wrapper PID 直接 5xx，而不是悄悄返回空 teams——属于 fail-loud 收紧。
- **Tests**:
  - 修正 `tests/team_mode_mcp_http.rs` 三个 my-teams 测试，让 caller 直接传 `std::process::id()`（“CC PID”），证明 service 不再向上 climb。
  - 新增 `lead_pending_my_teams_does_not_climb_past_caller_pid` 回归测试，断言 service 返回的 `cc_pid` 与 caller 提供的完全相等，并 sanity-assert `current_cc_pid()` 走出来的 PID 不等于 caller PID（确认 climb 行为客观存在，否则回归测试无效）。
- **Relation**:
  - 不替代 ADR-013（relay 端用 `current_cc_pid()` walk 仍是源头），只澄清 service handler 的 contract。
  - 不替代 ADR-023（`team_create` rebind 仍按既有逻辑），只让 owner write 与 owner read 用同一参考点。
  - 与 ADR-027 配套：使 v3 install-global 在 Cursor/VS Code 等 IDE 启动 CC 的场景里真正可用。

## ADR-029: hook `fetch_my_teams` 必须 strip runtime URL 的 `/mcp` 后缀

- **Date**: 2026-05-03
- **Context**: 用户在 v3 install-global + 真实多项目场景下端到端测，alice reply 真的写入 `lead_pending.jsonl` 但 Stop hook 100% 不 drain，`.async-wake-probe.log` 不存在。手动跑 `team_mode_service hook async-wake` 报 `lead-pending-async-wake: /my-teams query failed: HTTP 404 Not Found`。根因：runtime JSON 里 `url` 字段是 `http://127.0.0.1:8786/mcp`（给 stdio relay forwarding 用的），但 `/lead-pending/my-teams` 端点挂在 service base 上，不在 `/mcp` 子树里。hook 直接 `format!("{service_url}/lead-pending/my-teams")` 拼出 `/mcp/lead-pending/my-teams`，service router 无此路径，返回 404，hook 立即 exit 1，pending file 永远没被 drain。ADR-028 修的二次 walk bug 客观存在，但因为 hook 这一层 404 早 fail，my-teams handler 根本没被调，ADR-028 的修复在端到端层面还没显形。
- **Decision**: `fetch_my_teams` 在拼接前 strip 一次 `/mcp` 后缀（也 trim 末尾 `/`），并接受任何 caller 传入的 `service_url`。这样老的 runtime JSON、新的 runtime JSON、relay forwarding 用的同一个 url 字段都可以共享，不需要新增 schema 字段或 caller 端逻辑。
- **Why not alternatives**:
  - “给 RuntimeInfo 加 `base_url` 字段”：要改 schema、改 service 写 runtime json、迁移 user-scope 文件；blast radius 大，且让 caller 区分两种 URL 反而更易出错。
  - “service 在 /mcp 子树下也注册 /lead-pending/my-teams”：把 routing 噪声引进去，handler 实现要兼容两条路径，long-term 维护负担更大。
  - “hook 自己读 runtime json host/port 重新拼”：每个 hook 都得做一次，重复且和 `runtime_url` helper 不一致。
- **Consequences**:
  - + Hook 端立即恢复，async-wake / mid-turn / mid-turn fallback 三条路径都拿对端点。
  - + 修法对未来新加的非-/mcp 端点（如 `/lead-pending/*`、`/healthz`、`/web/*`）都自动适配。
  - − caller 必须知道 “base 用 base，MCP 用 base+/mcp” 的隐式规则；通过注释 + 测试固化。
  - − strip 是字符串操作而非 URL 解析，若未来 url 变成 `http://host/mcp/v1` 之类需重新审视。
- **Tests**: 新增 `fetch_my_teams_strips_mcp_suffix_from_service_url`，在 mock router 上同时注册 `/lead-pending/my-teams`（200）和 `/mcp/lead-pending/my-teams`（404）；caller 传 `http://addr/mcp` 必须 hit 前者，不能 hit 后者。
- **Relation**:
  - 不替代 ADR-028（service 端信任 caller PID 仍是正确收紧），只让 ADR-028 的修复有机会真正被触发。
  - 与 ADR-022（async-wake hook 重构）配套：补完 hook 在 v3 install-global 场景下从 stdio relay 路径迁到全局 runtime json 后被忽略的最后一公里。
  - ADR-027 的 install-global 路径在 ADR-028 + ADR-029 之后才算端到端可用。

## ADR-030: v3.1 project-root isolation for Team Mode data and lifecycle

- **Date**: 2026-05-04
- **Context**: ADR-027 把默认控制面切到 durable HTTP service，但最初仍把所有 team data 绑定到 service 启动时的单一 base_dir，导致不同 project 之间共享同一组 team / pending / lifecycle 文件，pending 注入和同名 team takeover 都可能跨项目串线。ADR-028/029 已经修好了 owner PID 和 hook URL 的端到端路由，但还没有把数据面真正按 caller project_root 隔离开。
- **Decision**: 把 `project_root` 视作调用方上下文的一部分，relay / hook / worker HTTP headers 都传 `X-Team-Mode-Project-Root`，service 端在 header 存在时按 `<project_root>/.agent-teams` 作用域读写 team data，缺 header 时才回退到 service 全局 base_dir 兼容老 caller。`team_create` 只允许同 owner active team rebind，archived team 可 revive；`overwrite=true` 会硬删当前 project scope 内所有 team 再新建；`team_delete` 默认 archive、`permanent=true` 才永久删除；lead-watchdog 用 5s * 3 grace 自动归档 dead-owner team；`team_list` 返回 active + archived 并提示可 revive archived team。
- **Consequences**:
  - + 不同 project 的 team data、members、pending 文件、归档状态互不污染。
  - + project-local isolation 仍兼容旧调用方：没 header 时继续用 service 全局 base_dir。
  - + archive / revive / overwrite / watchdog 语义统一收敛到同一套 project-scoped store。
  - − 需要 relay、hook、worker 以及 service handler 同步传 project_root，任何一处漏 header 都会回到兼容 fallback。
- **Tests**:
  - `project_root_context_isolates_team_data` 证明两个 project_root header 会落到不同的 `.agent-teams` 目录，且 service-global base_dir 不会被污染。
  - `create_existing_active_team_rejects_live_other_owner`、`create_revives_archived_team`、`delete_archives_by_default_and_permanently_deletes_when_requested` 和 `lead_watchdog_auto_archives_dead_owner_after_grace` 分别覆盖 rebind / revive / archive / watchdog。
- **Relation**:
  - 不替代 ADR-027/028/029，而是把它们已经修好的 transport / routing 约束落到真正的 project-scoped data model。
  - 与 ADR-020/021 配套：HTTP service 继续 durable，项目隔离只改变数据作用域，不改变 service 生命周期。

