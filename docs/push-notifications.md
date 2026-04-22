# Lead 主动推送配置（Claude Code）

当 Lead 是 Claude Code CLI 时，team-mode 通过 Claude Code 官方的
**`FileChanged` hook + `asyncRewake`** 组合实现"worker 回复 → 自动浮现到
Lead 对话流"的真 push。Lead 即便处于空闲等待输入状态，也会被唤醒并
进入新 turn 处理消息。

本文档说明如何启用该能力。

---

## 它怎么工作

```
Worker 回复 Lead
    │
    ▼
MessageService 写 inbox + 追加一行到
<base_dir>/lead_pending.jsonl
    │
    ▼ (文件变化事件，~50ms 内)
Claude Code FileChanged hook 被触发
    │
    ▼
scripts/hooks/lead-pending-wake.js 运行
    - 读 pending 文件
    - 把内容写入 stderr
    - 清空 pending
    - exit 2
    │
    ▼
asyncRewake: exit code 2 让 Claude 被唤醒
    - stderr 被包装成 <system-reminder> 注入新 turn
    - Claude 自然进入回复
```

**特征**：
- ✅ 零 token 空转（没消息就没触发）
- ✅ 约 50ms 延迟
- ✅ API key 登录可用（不依赖 Channels）
- ✅ Claude idle 状态也能被唤醒
- ✅ 一次多条消息会被聚合到一次注入

**限制**：
- 必须 Claude Code ≥ 某个支持 FileChanged + asyncRewake 的版本
  （已在 docs.claude.com/en/docs/claude-code/hooks 有官方文档的版本即可）
- 必须手动合并 `~/.claude/settings.json`
- 改动 hook 配置后**必须重启** Claude Code 才生效

---

## 一次性配置步骤

### 1. 找到 team-mode 的 base_dir

team-mode MCP server 的数据目录。默认：`~/.claude/teams/`。
如果你通过 `.mcp.json` 的 env 或命令行参数改过数据目录，用那个。

### 2. 合并 `~/.claude/settings.json`

打开你的 `~/.claude/settings.json`（没有就新建），合并以下 `hooks.FileChanged`
片段。**如果已有其他 FileChanged 配置，追加到数组里，不要整块覆盖**。

```json
{
  "hooks": {
    "FileChanged": [
      {
        "matcher": "lead_pending.jsonl",
        "hooks": [
          {
            "type": "command",
            "command": "node <仓库绝对路径>/scripts/hooks/lead-pending-wake.js",
            "async": true,
            "asyncRewake": true
          }
        ]
      }
    ]
  }
}
```

把 `<仓库绝对路径>` 换成你 clone 的 agent-teams-rs 仓库的绝对路径，例如：

```
node E:/aigc内容整理/agent-teams-rs-team-mode/scripts/hooks/lead-pending-wake.js
```

如果数据目录**不**是 `~/.claude/teams/`（即脚本默认搜索路径找不到），加
`TEAM_MODE_BASE_DIR` 环境变量：

```json
{
  "type": "command",
  "command": "node <仓库绝对路径>/scripts/hooks/lead-pending-wake.js",
  "async": true,
  "asyncRewake": true,
  "env": { "TEAM_MODE_BASE_DIR": "E:/my/data/dir" }
}
```

### 3. 重启 Claude Code

hook 配置只在会话启动时加载。**必须退出并重新启动** `claude`。

### 4. 验证

1. 启 Claude Code 在任意目录：`claude`
2. 让 Claude 随便说点什么，然后停下等你输入
3. 另开终端手动造一条假消息：
   ```bash
   echo '{"team":"demo","from":"alice","kind":"reply","text":"验证消息","msg_id":"test-1","ts":"2026-04-22T10:00:00Z"}' \
     >> ~/.claude/teams/lead_pending.jsonl
   ```
4. 看 Claude 是否自动开始处理 `[TEAM-MODE worker messages ...]`

如果 10 秒内没反应：
- 看 `~/.claude/teams/.lead-pending-wake.log` 是否有 `hook fired` 记录
- 确认 `~/.claude/settings.json` 中的路径是绝对路径，且 Node 装在 PATH 里
- 再次确认你**重启了** Claude Code

---

## 不配 hook 行不行？

可以。Lead 随时能用 `inbox_read` MCP 工具主动拉：

```
inbox_read(team="demo")
→ { team, lead_id, unread_count, total_returned, messages: [...] }
```

但这是 pull 模式，不会主动唤醒。Lead 空闲时看不到新消息直到自己调一次。
**把 push hook 加上，inbox_read 留作备用**是推荐用法。

---

## 多 team / 多 Lead 场景

- 所有 team 的 lead 共享一个 `lead_pending.jsonl` 文件（位于 base_dir 根）。
  每行记录的 `team` 字段标明属于哪个 team。Claude 看到 reminder 后
  会自己读出 team，不需要按 team 分文件。
- 如果你同时跑多个 Claude Code 作为不同 team 的 Lead，建议让每个 Lead
  只监听自己那份 base_dir 对应的 pending 文件——可以通过不同的
  `TEAM_MODE_BASE_DIR` + 不同的 matcher 区分。MVP 不推荐，复杂度收益比低。

---

## 卸载

删掉 `~/.claude/settings.json` 里那段 `FileChanged` 条目，重启 Claude Code。
pending 文件本身无害；不放心可以 `rm <base_dir>/lead_pending.jsonl`。

---

## 参考

- 官方 Claude Code Hooks 文档：https://code.claude.com/docs/en/hooks
  （`FileChanged` 和 `asyncRewake` 的权威说明）
- 本仓库 hook 脚本：`scripts/hooks/lead-pending-wake.js`
- Rust 侧写入逻辑：`src/team_mode/service/lead_pending.rs`
