#!/usr/bin/env node
// 后台延迟写入 pending.jsonl，用于测 asyncRewake
// 用法：node trigger-delayed.js <毫秒> <文本>
// 立即返回，不阻塞调用方。N 毫秒后由 detached 子进程写入。

const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

const MARKER = '__DELAYED_WRITE_CHILD__';

if (process.argv[2] === MARKER) {
    const delayMs = parseInt(process.argv[3], 10);
    const text = process.argv[4];
    setTimeout(() => {
        const msg = JSON.stringify({
            team: 'demo',
            from: 'alice',
            text,
            ts: new Date().toISOString(),
        });
        fs.appendFileSync(path.join(__dirname, 'pending.jsonl'), msg + '\n', 'utf8');
        fs.appendFileSync(
            path.join(__dirname, 'hook-run.log'),
            `[${new Date().toISOString()}] trigger-delayed wrote pending: "${text}"\n`,
            'utf8',
        );
    }, delayMs);
    return;
}

const delayMs = parseInt(process.argv[2] || '10000', 10);
const text = process.argv[3] || '场景B-默认触发';

const child = spawn(
    process.execPath,
    [__filename, MARKER, String(delayMs), text],
    { detached: true, stdio: 'ignore', cwd: __dirname },
);
child.unref();

console.log(`scheduled write to pending.jsonl in ${delayMs}ms: "${text}"`);
process.exit(0);
