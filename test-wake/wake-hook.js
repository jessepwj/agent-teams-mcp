#!/usr/bin/env node
// FileChanged + asyncRewake hook 验证脚本
// 读 pending.jsonl，写 stderr，清空文件，exit 2 唤醒 Claude

const fs = require('fs');
const path = require('path');

const pendingFile = path.join(__dirname, 'pending.jsonl');
const logFile = path.join(__dirname, 'hook-run.log');

function log(msg) {
    const ts = new Date().toISOString();
    fs.appendFileSync(logFile, `[${ts}] ${msg}\n`, 'utf8');
}

try {
    log('hook fired');

    if (!fs.existsSync(pendingFile)) {
        log('no pending file, exit 0');
        process.exit(0);
    }

    const content = fs.readFileSync(pendingFile, 'utf8').trim();
    if (!content) {
        log('pending empty, exit 0');
        process.exit(0);
    }

    const lines = content.split('\n').filter(Boolean);
    const formatted = lines.map(line => {
        try {
            const msg = JSON.parse(line);
            return `  [team=${msg.team || '?'}] ${msg.from || '?'}: ${msg.text || line}`;
        } catch {
            return `  ${line}`;
        }
    }).join('\n');

    const reminder = `[Worker 新消息 — 来自 team-mode]\n${formatted}\n请处理并回复。`;
    process.stderr.write(reminder + '\n');

    // 清空文件
    fs.writeFileSync(pendingFile, '', 'utf8');
    log(`injected ${lines.length} msg(s), cleared pending, exit 2`);

    process.exit(2);
} catch (e) {
    log(`error: ${e.message}`);
    process.stderr.write(`wake-hook error: ${e.message}\n`);
    process.exit(1);
}
