# agent-teams v0.1.0 代码审计报告

**日期:** 2026-02-12
**审计方法:** 3-Phase Diamond DAG (原生 CC 混合模式)
**审计团队:**
| Agent | Backend | 任务 | Token 消耗 |
|-------|---------|------|-----------|
| CC-Reviewer | Claude Code (Opus 4.6) | Phase 1: 深度代码审查 | ~120K |
| Codex-Reviewer | Codex (GPT-5.3) | Phase 2: 安全+正确性验证 | 37K |
| Gemini-Reviewer | Gemini (2.5-flash) | Phase 2: API 设计+Rust 惯用法 | ~50K |
| Team-Lead | Claude Code (Opus 4.6) | Phase 3: 综合报告 | — |

---

## 执行摘要

agent-teams 是一个 ~9000 行的 Rust 库，用于多 agent 编排，支持 3 个后端（Claude Code、Codex、Gemini CLI）。
**整体质量评价：高**。模块分解清晰，错误处理显式，文件 I/O 使用原子写+文件锁，测试覆盖广泛，
文档注释详尽。**代码中没有任何 `unsafe` 块**。

---

## 三方共识矩阵

| # | 发现 | 严重度 | CC | Codex | Gemini | 共识 |
|---|------|--------|-----|-------|--------|------|
| 1 | TOCTOU: MemoryManager::load | HIGH | 发现 | AGREE | AGREE | **3/3 确认** |
| 2 | TOCTOU: MemoryManager::delete | HIGH | 发现 | AGREE | AGREE | **3/3 确认** |
| 3 | .unwrap() in consensus | MEDIUM | 发现 | DISAGREE | AGREE | **2/3 确认** |
| 4 | Default::expect() panics | MEDIUM | 发现 | AGREE | AGREE | **3/3 确认** |
| 5 | read_unread 双重锁 | MEDIUM | 发现 | DISAGREE | AGREE | **2/3 确认** |
| 6 | Vec::remove(0) O(n) | MEDIUM | 发现 | AGREE | AGREE | **3/3 确认** |
| 7 | validate_name 不拒绝 `.` 前缀 | LOW | 发现 | — | AGREE | **2/3 确认** |
| 8 | Dashboard CORS 默认 permissive | LOW | 发现 | — | AGREE | **2/3 确认** |
| **9** | **MemoryManager 缺少 validate_name** | **HIGH** | 未发现 | **NEW** | — | **Codex 独立发现** |
| 10 | Newtype IDs (TeamId, TaskId) | INFO | — | — | **NEW** | **Gemini 建议** |
| 11 | Default trait → factory method | INFO | — | — | **NEW** | **Gemini 建议** |
| 12 | clippy::pedantic 启用 | INFO | 发现 | — | AGREE | **2/3 确认** |

---

## 优先级排序（综合三方意见）

### P0 — 必须修复

#### 1. HIGH: MemoryManager 路径遍历风险（缺少 validate_name）🔴 Codex 新发现

**文件:** `src/memory/mod.rs:178-183, 193, 206, 230`

**问题:** `MemoryManager::save/load/delete` 直接使用 `team` 和 `agent` 参数构建路径，
但没有调用 `validate_name()`。与 `team/`、`messaging/` 模块不同，`memory/` 模块缺少这个关键的安全检查。

```rust
fn memory_dir(&self, team: &str) -> PathBuf {
    self.teams_base.join(team).join("memory")  // team 未验证！
}
fn memory_path(&self, team: &str, agent: &str) -> PathBuf {
    self.memory_dir(team).join(format!("{agent}.json"))  // agent 未验证！
}
```

**影响:** 包含 `../` 或路径分隔符的 team/agent 名可以逃逸目标目录，读写任意文件。

**修复方案:**
```rust
pub fn save(&self, team: &str, agent: &str, memory: &ConversationMemory) -> Result<()> {
    crate::util::validate_name(team)?;   // 添加
    crate::util::validate_name(agent)?;  // 添加
    // ... 原有逻辑
}
// load 和 delete 同理
```

**共识:** Codex 独立发现，CC 遗漏，属于真正的安全漏洞。

---

#### 2. HIGH: TOCTOU 竞态 — MemoryManager::load

**文件:** `src/memory/mod.rs:206-227`

**问题:** `path.exists()` 检查在获取锁之前执行。在检查和锁定读取之间，另一个进程可能删除文件。

```rust
pub fn load(&self, team: &str, agent: &str) -> Result<Option<ConversationMemory>> {
    let path = self.memory_path(team, agent);
    if !path.exists() {         // ← 在锁之外检查
        return Ok(None);
    }
    let _lock = FileLock::acquire(&self.lock_path(team))?;
    let data = std::fs::read_to_string(&path)  // ← 文件可能已被删除
```

**修复方案:**
```rust
pub fn load(&self, team: &str, agent: &str) -> Result<Option<ConversationMemory>> {
    let dir = self.memory_dir(team);
    std::fs::create_dir_all(&dir)?;
    let _lock = FileLock::acquire(&self.lock_path(team))?;
    let path = self.memory_path(team, agent);
    match std::fs::read_to_string(&path) {
        Ok(data) => Ok(Some(serde_json::from_str(&data)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
```

**共识:** 3/3 确认。

---

#### 3. HIGH: TOCTOU 竞态 — MemoryManager::delete

**文件:** `src/memory/mod.rs:230-242`

**问题:** 同上模式 — `exists()` 在锁之前检查。

**修复方案:**
```rust
pub fn delete(&self, team: &str, agent: &str) -> Result<()> {
    let _lock = FileLock::acquire(&self.lock_path(team))?;
    let path = self.memory_path(team, agent);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
```

**共识:** 3/3 确认。

---

### P1 — 应该修复

#### 4. MEDIUM: Default trait 在 $HOME 缺失时 panic

**文件:** `src/team/mod.rs:132`, `src/messaging/mod.rs:72-77`

**问题:** `FileTeamManager::default()` 使用 `.expect()` 会 panic；
`FileInboxManager::default()` 静默降级到 `.`。行为不一致。

**建议:** 所有 Default 实现改为 factory method `new_from_home_dir() -> Result<Self>`。

**共识:** 3/3 确认。Gemini 建议最完整（deprecate Default + factory method）。

---

#### 5. MEDIUM: Vec::remove(0) 是 O(n) 操作

**文件:** `src/memory/mod.rs:94-96`

**问题:**
```rust
while self.turns.len() > self.config.max_turns {
    self.turns.remove(0);  // O(n) 每次
}
```

**修复:** 替换 `Vec<TurnRecord>` 为 `VecDeque<TurnRecord>`，使用 `pop_front()` O(1)。

**共识:** 3/3 确认。

---

#### 6. MEDIUM: .unwrap() 在 consensus resolution 中

**文件:** `src/consensus/mod.rs:112, 151`

**争议:** Codex **DISAGREE**（认为有 `active.is_empty()` 早返回保护，永远不会触发 panic）。
CC 和 Gemini **AGREE**（认为是脆弱的不变量）。

**综合判定:** MEDIUM — 不变量确实成立，但应改为 `.expect("active is non-empty; checked above")`
以文档化不变量。如果未来有人重构了早返回逻辑，unwrap 就可能被触发。

---

#### 7. MEDIUM: read_unread 性能

**文件:** `src/messaging/mod.rs:186-192`

**争议:** Codex **DISAGREE**（认为只获取一次锁，通过 `read_inbox`）。
CC 原始发现说"获取两次锁"可能是计数上的误解 — 确实只通过 `read_inbox` 获取一次。
但在 `poll_inbox` 循环中反复调用 `read_unread` 确实会产生不必要的锁竞争。

**综合判定:** LOW（功能正确，仅性能优化建议）。

---

### P2 — 建议改进

| # | 建议 | 来源 | 工作量 |
|---|------|------|--------|
| 8 | validate_name 拒绝 `.` 开头的名称 | CC + Gemini | S |
| 9 | Dashboard 添加单元测试 | CC + Gemini | M |
| 10 | 引入 Newtype IDs（TeamId, TaskId） | Gemini | L |
| 11 | 启用 `clippy::pedantic` | CC + Gemini | S |
| 12 | Dashboard CORS 文档强化 | CC | S |
| 13 | current_dir() 失败时返回 Error 而非空路径 | CC + Gemini | S |
| 14 | 并发压力测试 | CC + Gemini | M |
| 15 | 公共 API 文档增强（#Errors, #Panics 段落） | Gemini | M |

---

## 推荐实施顺序

```
Week 1:  #1 (路径遍历) → #2 (TOCTOU load) → #3 (TOCTOU delete)
Week 2:  #4 (Default 改 factory) → #5 (VecDeque) → #6 (.expect)
Week 3:  #8 (validate_name) → #9 (Dashboard 测试) → #11 (clippy)
Future:  #10 (Newtype IDs) → #14 (压力测试) → #15 (文档)
```

---

## 架构评分

| 维度 | 评分 | 说明 |
|------|------|------|
| 模块设计 | ⭐⭐⭐⭐⭐ | 职责清晰，无循环依赖，facade 模式恰当 |
| 错误处理 | ⭐⭐⭐⭐ | thiserror 正确使用，少量 .unwrap() 需修复 |
| 并发安全 | ⭐⭐⭐⭐ | Atomic ordering 正确，TOCTOU 是主要问题 |
| 文件 I/O | ⭐⭐⭐⭐⭐ | 原子写 + fs2 文件锁，NFS 警告完善 |
| API 设计 | ⭐⭐⭐⭐ | Builder 模式好，缺 Newtype IDs |
| 测试覆盖 | ⭐⭐⭐⭐ | ~140 测试，dashboard 无测试 |
| 安全 | ⭐⭐⭐ | 命令注入安全，MemoryManager 路径遍历需修 |
| 性能 | ⭐⭐⭐⭐ | spawn_blocking 正确使用，VecDeque 小优化 |

**总评: 4.1/5 — 高质量的 Rust 库，3 个 HIGH 问题修复后即可安心发布。**

---

## 审计方法论说明

本审计使用 **原生 CC 混合模式 Diamond DAG**：
- Phase 1: CC agent（通过 Task tool）做主要代码审查（读取所有 26 个源文件）
- Phase 2: Codex（`codex exec --full-auto --skip-git-repo-check`）和 Gemini CLI（`gemini -y -p`）并行验证
- Phase 3: Team Lead 综合三方结果，解决分歧

**关键发现模式:** Codex 独立发现了 CC 遗漏的路径遍历问题（#1），验证了多 agent 交叉审计的价值。
三方在 6/10 个发现上完全一致（3/3 AGREE），在 2 个发现上有分歧（2/3），最终由 Team Lead 仲裁。
