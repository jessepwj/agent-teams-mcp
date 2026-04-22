# FileChanged + asyncRewake 验证步骤

目标：确认 Claude Code 能否在 **idle 状态下**被外部文件变化唤醒并自动开始新 turn。

---

## 0. 已准备的文件

- `wake-hook.js` — 读 pending、写 stderr、清空、exit 2
- `pending.jsonl` — 空的待触发文件
- `settings-snippet.json` — 要合并到 `~/.claude/settings.json` 的片段
- `hook-run.log` — 脚本运行时自动追加的日志（执行后才产生）

---

## 1. 配置 hook

把 `settings-snippet.json` 的 `hooks.FileChanged` 合并到你 `~/.claude/settings.json`。

**注意**：如果已有 `hooks` 配置，要做深合并，不要整块覆盖。

合并后的示例（假设你已有别的 hook）：
```json
{
  "hooks": {
    "UserPromptSubmit": [ /* ... 已有 ... */ ],
    "FileChanged": [
      {
        "matcher": "pending.jsonl",
        "hooks": [
          {
            "type": "command",
            "command": "node E:/aigc内容整理/agent-teams-rs-team-mode/test-wake/wake-hook.js",
            "async": true,
            "asyncRewake": true
          }
        ]
      }
    ]
  }
}
```

---

## 2. 重启 Claude Code

hook 配置**必须重启** Claude Code 才会加载（文件监听器在启动时注册）。

在 `test-wake` 目录下启动一个新 Claude Code 会话：
```bash
cd "E:/aigc内容整理/agent-teams-rs-team-mode/test-wake"
claude
```

---

## 3. 三种场景分别测试

### 场景 A — Claude 正在回复中（已知应工作）

1. 在 Claude 里发："请写一首 100 字的诗"
2. 趁 Claude 还在生成，**另开一个终端**执行：
   ```bash
   echo '{"team":"demo","from":"alice","text":"场景A测试消息"}' >> "E:/aigc内容整理/agent-teams-rs-team-mode/test-wake/pending.jsonl"
   ```
3. **观察**：Claude 是否在当前回复里注意到 `[Worker 新消息]` 并回应它？

**预期**：是（docs 明确说 asyncRewake 在 turn 中注入 system reminder）。

---

### 场景 B — Claude 刚停下，光标等用户输入（**关键测试**）

1. 在 Claude 里发："说 ok"，Claude 回复"ok"后停下
2. 不要输入任何东西，盯着终端看
3. 另开终端：
   ```bash
   echo '{"team":"demo","from":"alice","text":"场景B测试消息"}' >> "E:/aigc内容整理/agent-teams-rs-team-mode/test-wake/pending.jsonl"
   ```
4. **观察**：Claude 是否**自动起一轮新 turn** 处理这条消息？还是光标继续静止等你输入？

**预期**：这是官方 "wake" 承诺的核心验证。如果真 wake → 方案成立。

---

### 场景 C — 长时间 idle（验证事件不因时间丢失）

1. Claude 回完一句，等 5 分钟不动任何东西
2. 5 分钟后 echo 到 pending.jsonl
3. **观察**：Claude 是否被唤醒？

**预期**：和场景 B 一样。测这个是为了确认长 idle 没有特殊行为（例如文件监听器被暂停）。

---

### 场景 D — 一次写入多条消息

1. Claude 回完一句，停下
2. 一次性追加 3 条：
   ```bash
   for i in 1 2 3; do
     echo '{"team":"demo","from":"bob","text":"消息 '$i'"}' >> pending.jsonl
   done
   ```
3. **观察**：Claude 是否看到**全部 3 条**？hook 触发 1 次还是 3 次？

**预期**：hook 触发 1 次（因为 `wake-hook.js` 一次读全部 3 条后清空），Claude 收到全部 3 条内容。

---

## 4. 结果记录

每个场景记录：

- **是否唤醒**：Y / N
- **消息是否正确显示**：Y / N（展示为 system reminder？ `<system-reminder>` 样式？还是以别的形式？）
- **hook-run.log 是否有对应记录**：对齐时间
- **Claude 的反应**：引用一小段它说的话

把四个场景结果回给我，我就能定方案。

---

## 5. 可能的失败模式 & 诊断

| 现象 | 可能原因 | 排查 |
|---|---|---|
| `hook-run.log` 没更新 | hook 没触发 | 检查 settings.json 合并是否正确；`claude` 是否重启；FileChanged matcher 是否匹配（尝试改为 `**/pending.jsonl`） |
| log 有但 Claude 没反应 | exit code 2 无法唤醒 idle | 场景 B 的关键结果；这意味着 wake 只对活跃 turn 有效 |
| Claude 反应但内容错 | stderr 格式不符合注入要求 | 看 Claude 实际看到的文本，调整 wake-hook.js 输出格式 |
| 只触发一次后再触发失败 | 文件监听器有去重/节流 | 多测几次，观察触发间隔阈值 |

---

## 6. 清理

测完 settings.json 里的 FileChanged hook 可以直接删掉；`test-wake/` 目录可以整个删除。
