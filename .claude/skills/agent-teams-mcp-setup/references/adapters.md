# Adapter 选择参考

team-mode MCP 当前在本项目里使用两种 worker backend：**`codex`（默认）** 和 **`claude-code`**。

> **本仓库默认 `codex`**（GPT-5 + `danger-full-access` sandbox）。
> `worker_add` 不显式传 adapter 时落 codex（`src/team_mode/mcp/tools.rs:624`）。
> codex worker 已经接入了完整 MCP 工具集（Read/Edit/Bash/Grep/Web/Playwright 等），
> 没有任何角色需要因为"工具不够"而切回 claude-code。
>
> claude-code 是**可选项**，仅在以下情况换：
> 1. 用户主动要求"想用 Claude"
> 2. 命中下面"codex 沙盒可能踩坑"的场景（长进程 / Docker / 跨目录写）
> 3. 安全敏感场景需要升 `opus`
> 4. 想做 backend 对比实验
>
> （`gemini-cli` adapter 在 daemon 代码里仍然存在，但**本项目当前不推荐使用**。）

## 能力矩阵

| 能力 | `codex`（默认） | `claude-code` |
|------|--------------|---------|
| 持久会话（跨轮记忆） | ✅ 单进程持续运行 | ✅ 单进程持续运行 |
| `session_id` 捕获 | ✅（thread.id） | ✅ |
| Web UI session transcript | ✅（rollout JSONL） | ✅ |
| 完整工具集（Read/Edit/Bash/Grep/Web 等） | ✅ | ✅ |
| MCP 工具调用（含 Playwright） | ✅ 本仓库已配齐 | ✅ |
| 文件读写 | ✅（在 sandbox 边界内） | ✅ |
| Bash / 命令执行 | ✅（受 sandbox 约束） | ✅ |
| 写自己的 `.plans/` 文件 | ✅ | ✅ |
| Web 搜索 / WebFetch | ✅ | ✅ |
| 适合长流程多步任务 | ✅ | ✅ |
| 沙盒 | `danger-full-access`（仍有边界） | 无 |
| 项目上下文文件 | **`AGENTS.md`** | **`CLAUDE.md`** |
| 成本档 | 中（GPT-5） | 高 |
| 在 Windows 需要 | codex CLI on PATH | `CLAUDE_CODE_GIT_BASH_PATH`（自动探测） |
| 自报模型 | GPT-5 | Claude Sonnet/Opus |
| System prompt 机制 | 拼到首条 user message | `--system-prompt` flag |

> ⚠️ **AGENTS.md vs CLAUDE.md**：codex worker 启动时读 `AGENTS.md`，claude-code worker / Claude Code lead 读 `CLAUDE.md`。**本 skill Step 3.5 同时生成两份内容相同的文件**，保持同步。任何更新两边一起改。

## 角色 × Adapter 推荐（默认全 codex）

| 角色 | 默认 | 替代方案 | 何时切替代 |
|------|------|---------|-----------|
| backend-dev | `codex` | `claude-code` | 需要更深架构推理；用户主动指定 Claude |
| frontend-dev | `codex` | `claude-code` | 同上 |
| researcher | `codex` | `claude-code` | 需要 Claude 推理风格 |
| e2e-tester | `codex` | `claude-code` | E2E 测试需启 dev server / 浏览器子进程，且命中 codex sandbox 限制时 |
| reviewer | `codex` | `claude-code` (opus) | 安全敏感 / 复杂架构需要更强推理 |
| custodian | `codex` | `claude-code` | 写复杂 check 脚本需要更深推理 |
| 文档撰写 | `codex` | `claude-code` | 长篇内容希望 Claude 风格 |

> **决策原则**：先全 codex 起手；只有遇到下面的沙盒坑或明确质量需求时再单点替换。

---

## ⚠️ codex 沙盒坑（部署/测试时易踩）

codex worker 启动时配置 `sandbox_mode: "danger-full-access"` + `approvalPolicy: "never"`，
名字虽叫 "danger-full-access" 但**仍有边界**。下面是常见踩坑场景，遇到时考虑把对应角色换成 claude-code。

### 1. 跨进程 / 后台守护进程

| 操作 | 是否可能踩坑 |
|------|------------|
| `npm run dev` / `cargo run` 启常驻 dev server | ⚠️ codex 的轮次结束 sandbox 可能回收子进程 |
| `docker run -d ...` 启容器 | ⚠️ 看 codex 配置；某些场景容器在 codex 退出时被杀 |
| systemd / pm2 启服务 | ⚠️ 同上 |
| nohup / setsid 脱离父进程 | ⚠️ sandbox 可能拦截 daemonization |

**症状**：worker 跑 `npm run dev`，命令"返回成功"，但下一轮再 curl localhost 拒连。

**应对**：把启 dev server 这类长进程交给 lead（你）在主对话里直接 Bash 执行，或者把 e2e-tester 换成 claude-code。

### 2. 网络访问

| 操作 | 是否踩坑 |
|------|---------|
| `curl https://api.example.com` | ✅ 一般 OK |
| `npm install` / `pip install` | ✅ OK |
| 调用付费 API（写 OPENAI_API_KEY 之类） | ⚠️ codex 默认可能没把 env 透传，需 worker_add 时显式 `env: {...}` |
| 私有 registry / VPN | ⚠️ 看 codex 网络策略，可能要预先 npm config 改 |

**应对**：worker_add 时通过 `env` 字段显式传需要的环境变量，不要假设 codex 自动继承。

### 3. 文件系统范围

| 操作 | 是否踩坑 |
|------|---------|
| 写项目 cwd 内 | ✅ 完全 OK |
| 写 cwd 外（系统目录、其他项目） | ⚠️ sandbox 可能拒绝 |
| 写 ~/.config/* | ⚠️ 看 codex 沙盒规则 |
| 装全局 npm 包（npm i -g） | ⚠️ 一般会失败 |

**应对**：worker 任务限定在自己的 cwd 内做事；全局安装类操作交给 lead。

### 4. 部署 / CD 类操作

| 操作 | 推荐做法 |
|------|---------|
| `git push` | OK（凭据问题除外） |
| `gh pr create` | OK |
| `vercel deploy` / `netlify deploy` | ⚠️ 可能要交互登录，最好 lead 手动 |
| `kubectl apply` / `terraform apply` | ⚠️ 不要让 codex worker 干这个，太重，凭据/状态问题大 |
| 数据库 migration | ⚠️ 同上 |

**应对**：部署类高风险操作 lead 自己来，不要派给任何 worker（不只是 codex）。

### 5. 测试与 CI

| 场景 | 是否踩坑 | 应对 |
|------|---------|------|
| `pytest` / `cargo test` 单测 | ✅ OK | — |
| Playwright 启浏览器测试 | ⚠️ 浏览器是子进程，可能被沙盒回收 | e2e-tester 换 claude-code，或让 e2e-tester 在 lead 监督下跑（lead 自己起 browser） |
| 集成测试需启 server | ⚠️ 见上面"后台守护进程" | lead 起 server，worker 跑测试 |
| 跑 `golden_rules.py` / 自定义 check | ✅ OK（纯文件分析） | — |

### 6. 调试技巧

worker 卡住或报莫名错误时：
1. 看 `.agent-teams/<team>/messages.jsonl` —— worker 的完整对话历史
2. 看 `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` —— codex 的 rollout 日志，含工具调用细节
3. 调 `mcp__team-mode__inbox_read(team)` 看 lead 这边收到了什么 [SYSTEM] 通知
4. Web UI（http://127.0.0.1:8787）右栏的 session transcript 直接看 worker 内部步骤

---

## 平台特定注意事项

### codex（默认）
- worker 启动时配置 `approvalPolicy: "never"` + `sandbox_mode: "danger-full-access"`
- `reasoning effort` 字段不强制，落到用户 `~/.codex/config.toml`
- system_prompt 拼到首条 user message 前——意味着首条对话比正常稍长，是正常的
- 模型选择：默认走 codex CLI 的当前默认（GPT-5），可通过 `model` 参数 override
- session_id 在第一条 reply 时被捕获；Web UI session transcript 用 codex rollout JSONL（5 秒 TTL 缓存）
- **项目上下文文件**：codex 启动时读 cwd 下的 `AGENTS.md`（不读 CLAUDE.md），所以本 skill Step 3.5 必须同时生成 AGENTS.md

### claude-code
- **Windows**：需要 `CLAUDE_CODE_GIT_BASH_PATH`。MCP relay 启动时从 Git 标准安装路径自动探测；非标准安装需手动设置环境变量
- **权限**：worker 用 `--permission-mode bypassPermissions`，不卡用户授权弹窗
- **system_prompt**：通过 `--system-prompt` flag 传，在 worker 整个生命周期生效
- **不受 codex 沙盒限制**：长进程、Docker、跨目录写都比 codex 友好
- **项目上下文文件**：claude-code 读 `CLAUDE.md`

---

## 反模式

❌ **不要**让 codex worker 跑长进程后台 server / Docker 容器，然后期望它跨轮活着——会被 sandbox 回收
❌ **不要**让 codex worker 干部署（vercel/k8s/terraform）—— 凭据 + 沙盒双重风险
❌ **不要**期望 codex worker 自动继承 lead 的环境变量——`worker_add` 时显式 `env: {...}`
❌ **不要**只更新 CLAUDE.md 不更 AGENTS.md ——codex worker 拿到的项目上下文会陈旧
❌ **不要**给一个 worker 同时配多 adapter：一个 worker = 一个 adapter，要混就配多 worker
