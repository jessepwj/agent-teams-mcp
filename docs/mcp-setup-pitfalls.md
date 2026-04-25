> **[HISTORICAL — 2026-04]** 本文档描述的是 MCP server 早期接入阶段的踩坑记录（22 个工具，Content-Length → NDJSON 修复）。其中坑一（配置文件位置）和坑二（Windows 路径含中文需建 Junction）仍然有效；坑三（NDJSON 格式）已在当前实现中修复且 CLAUDE_CODE_GIT_BASH_PATH 自动探测已落地。仅作为接入参考保留。

# MCP Server 接入踩坑记录

> 项目：`agent-teams-rs-team-mode`
> 日期：2026-04-21
> 适用场景：将 Rust 编译的 MCP server 接入 Claude Code CLI

---

## 背景

本项目实现了一个基于 stdio 的 MCP server（`team_mode_mcp`），对外暴露 22 个 Team Mode 工具。在把它接入 Claude Code 的过程中，踩了三个坑，记录如下。

---

## 坑一：配置文件放错位置

### 现象

把 MCP 配置写在 `.claude/settings.json`，`/mcp` 完全检测不到。

### 原因

Claude Code 的 MCP 配置文件分两种：

| 文件 | 作用范围 | 是否进版本控制 |
|---|---|---|
| `~/.claude.json` | 用户全局 | 否 |
| `.mcp.json`（项目根目录） | 项目级 | 是，团队共享 |

`.claude/settings.json` 是 Claude Code 的其他设置（权限、hooks 等），**不是** MCP server 的注册位置。

### 正确做法

在项目根目录创建 `.mcp.json`：

```json
{
  "mcpServers": {
    "team-mode": {
      "command": "...",
      "args": [...],
      "env": {}
    }
  }
}
```

---

## 坑二：Windows 路径含中文/空格导致启动失败

### 现象

`.mcp.json` 里 `command` 直接写含中文和空格的完整路径，MCP 显示 `✘ failed`：

```json
{
  "mcpServers": {
    "team-mode": {
      "command": "E:/aigc内容整理/agent-teams-rs-team-mode/target/release/team_mode_mcp.exe"
    }
  }
}
```

用 `cmd /c` 包一层也不行：

```json
"command": "cmd",
"args": ["/c", "E:/aigc内容整理/agent-teams-rs-team-mode/target/release/team_mode_mcp.exe"]
```

Windows 的 `cmd.exe` 对含中文字符的路径解析不可靠，路径直接作为 `command` 同样失败。

### 原因

Claude Code 在 Windows 下启动进程时，路径中的中文字符会导致系统 API 调用失败。

### 解决方案：创建 Junction（目录软链接）

用 `mklink /J` 把含中文的目录映射到一个纯 ASCII 路径：

```cmd
mklink /J E:\agent-teams-rs E:\aigc内容整理\agent-teams-rs-team-mode
```

然后在 `.mcp.json` 里使用干净路径：

```json
{
  "mcpServers": {
    "team-mode": {
      "command": "E:\\agent-teams-rs\\target\\release\\team_mode_mcp.exe",
      "args": [
        "--data-dir",
        "E:\\agent-teams-rs\\.team-mode-data"
      ],
      "env": {}
    }
  }
}
```

Junction 对操作系统透明，程序读写文件时访问的仍是原始路径，没有任何副作用。

> **注意**：`type` 字段（`"type": "stdio"`）不是必须的，加了反而可能导致兼容性问题，去掉即可。

---

## 坑三：stdio 传输协议格式不匹配（根本原因）

### 现象

即使路径问题解决了，MCP 仍然 `failed`。二进制手动运行完全正常，但 Claude Code 就是连不上。

### 排查过程

对比参考项目（ORCH）和本项目的 stdio 实现：

| | ORCH（能连接） | 本项目（修复前） |
|---|---|---|
| **写出格式** | NDJSON：`{"jsonrpc":"2.0",...}\n` | Content-Length 帧：`Content-Length: 310\r\n\r\n{...}` |
| **读取格式** | NDJSON | 兼容两种 |

### 根本原因

MCP 有两种 stdio 传输格式：

1. **NDJSON**（换行分隔 JSON）：每条消息是一行 JSON，以 `\n` 结尾
2. **Content-Length 帧**（LSP 风格）：消息前带 `Content-Length: <n>\r\n\r\n` 头

**Claude Code 使用 NDJSON**，而本项目的 Rust server 回复的是 Content-Length 帧，Claude Code 解析不了，直接报 `failed`。

### 修复

将 `write_json_rpc_message` 从 Content-Length 改为 NDJSON：

```rust
// 修复前
fn write_json_rpc_message<W, T>(writer: &mut W, payload: &T) -> Result<()>
where W: Write, T: Serialize {
    let body = serde_json::to_vec(payload)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;  // ❌ Claude Code 不认
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

// 修复后
fn write_json_rpc_message<W, T>(writer: &mut W, payload: &T) -> Result<()>
where W: Write, T: Serialize {
    let body = serde_json::to_vec(payload)?;
    writer.write_all(&body)?;
    writer.write_all(b"\n")?;   // ✅ NDJSON
    writer.flush()?;
    Ok(())
}
```

读取侧保持兼容两种格式（已有实现支持）。

### 验证方式

修复后用 NDJSON 格式手动测试：

```bash
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}\n' \
  | E:\agent-teams-rs\target\release\team_mode_mcp.exe --data-dir E:\agent-teams-rs\.team-mode-data
```

输出应为一行 JSON（无 Content-Length 头）：

```
{"id":1,"jsonrpc":"2.0","result":{"capabilities":{...},...}}
```

---

## 最终工作配置

### 前置条件

1. 编译 release 版本：
   ```bash
   cargo build --release --bin team_mode_mcp
   ```

2. 创建 Junction（仅需执行一次）：
   ```cmd
   mklink /J E:\agent-teams-rs E:\aigc内容整理\agent-teams-rs-team-mode
   ```

### `.mcp.json`（项目根目录）

```json
{
  "mcpServers": {
    "team-mode": {
      "command": "E:\\agent-teams-rs\\target\\release\\team_mode_mcp.exe",
      "args": [
        "--data-dir",
        "E:\\agent-teams-rs\\.team-mode-data"
      ],
      "env": {}
    }
  }
}
```

### 验证

重启 Claude Code 会话后运行：

```
/mcp
```

看到 `team-mode · ✔ connected · 22 tools` 即为成功。

---

## 坑四：项目内 `.mcp.json` 指向 release，开发改代码不生效

### 现象

修改 Rust 源码 → `cargo build` 通过 → `/mcp reconnect` → MCP 工具行为没变。代码改动像被无视了。

### 原因

`cargo build`（不带 `--release`）默认产出到 `target/debug/`。但项目自带的 `.mcp.json` 长期指向 `target/release/team_mode_mcp.exe`（来自最初安装文档的 copy-paste），所以 MCP relay 启动的是几天前的旧 release binary，daemon 也跟着是旧的。

每次让代码生效都得 `cargo build --release` 一次，几分钟级链接时间，且 binary 还可能被运行中的进程占用导致 `os error 5: 拒绝访问`。

### 解决

项目内 `.mcp.json` 必须指向 `target/debug/team_mode_mcp.exe`：

```json
{
  "mcpServers": {
    "team-mode": {
      "command": "<repo>/target/debug/team_mode_mcp.exe"
    }
  }
}
```

这样标准开发循环（改代码 → `cargo build` → `/mcp reconnect`）零摩擦。

### 开发期 vs 安装期分工

- **开发期**：`.mcp.json`（git 跟踪、本地共享）→ `target/debug/`，`cargo build` 默认产出，立即生效。
- **安装期 / 给开源用户**：README 引导 `cargo build --release` + 把 `target/release/team_mode_mcp.exe` 拷到 PATH 或在用户级 `~/.claude.json` 写绝对路径。release 编译开销摊销到一次性安装，长跑 daemon 享受优化。

两条路线分离，互不干扰。**永远不要把项目内 `.mcp.json` 改回 release**。

---

## 四个坑总结

| # | 坑 | 表现 | 解决 |
|---|---|---|---|
| 1 | 配置文件位置错误 | `/mcp` 检测不到 server | 改用项目根目录的 `.mcp.json` |
| 2 | Windows 路径含中文 | `✘ failed`，cmd 无法解析路径 | 用 `mklink /J` 建 junction 到 ASCII 路径 |
| 3 | stdio 协议格式不匹配 | `✘ failed`，二进制正常但 Claude Code 连不上 | server 改用 NDJSON 输出，去掉 Content-Length 帧 |
| 4 | `.mcp.json` 指向 release | 改代码后 `/mcp reconnect` 不生效 | 项目内 `.mcp.json` 改 `target/debug/`；release 留给安装期 |
