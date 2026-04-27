#!/usr/bin/env node
/**
 * lead-pending-shared.js — common utilities for both lead-pending hooks.
 *
 * Used by:
 *   lead-pending-wake.js     (Stop hook — turn-boundary block)
 *   lead-pending-mid-turn.js (PostToolUse hook — mid-turn additionalContext)
 *
 * This module owns: process-tree ancestor walking, pending file
 * lock/classify/drain, base-dir resolution, log helper, and the
 * formatting helpers that render reminders for the LLM.
 *
 * Both hooks must agree on these or routing/dedup will break, so the
 * single source of truth lives here.
 */

'use strict';

const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

// ---- Shared filenames / constants -----------------------------------------

const PENDING_FILENAME = 'lead_pending.jsonl';
const ANCESTOR_CACHE_FILENAME = '.ancestor-cache.json';
const PENDING_LOCK_FILENAME = '.lead-pending.lock';
const LEAD_SESSIONS_FILENAME = '.lead-sessions.json';

const ANCESTOR_CACHE_TTL_MS = 5_000;
const ANCESTOR_CHAIN_MAX_DEPTH = 40;
const PROC_QUERY_TIMEOUT_MS = 5_000;

const PENDING_LOCK_RETRY_MS = 50;
const PENDING_LOCK_MAX_WAIT_MS = 3000;

const LEAD_SESSIONS_TTL_MS = 7 * 24 * 60 * 60 * 1000;

function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

// ---- Process-tree ancestor walking ----------------------------------------

function snapshotProcessTree() {
    try {
        if (process.platform === 'win32') {
            const out = execSync(
                'powershell -NoProfile -NonInteractive -Command ' +
                '"Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId | ConvertTo-Json -Compress"',
                {
                    encoding: 'utf8',
                    timeout: PROC_QUERY_TIMEOUT_MS,
                    windowsHide: true,
                    maxBuffer: 8 * 1024 * 1024,
                }
            );
            const parsed = JSON.parse(out);
            const arr = Array.isArray(parsed) ? parsed : [parsed];
            const map = new Map();
            for (const p of arr) {
                if (!p) continue;
                const pid = Number(p.ProcessId);
                const ppid = Number(p.ParentProcessId);
                if (Number.isFinite(pid) && Number.isFinite(ppid)) {
                    map.set(pid, ppid);
                }
            }
            return map;
        } else {
            const out = execSync('ps -eo pid=,ppid=', {
                encoding: 'utf8',
                timeout: PROC_QUERY_TIMEOUT_MS,
            });
            const map = new Map();
            for (const line of out.split('\n')) {
                const m = line.trim().match(/^(\d+)\s+(\d+)$/);
                if (m) map.set(Number(m[1]), Number(m[2]));
            }
            return map;
        }
    } catch (_) {
        return null;
    }
}

function walkAncestors(tree, startPid) {
    const chain = [];
    const visited = new Set();
    let pid = startPid;
    while (
        Number.isFinite(pid) &&
        pid > 0 &&
        !visited.has(pid) &&
        chain.length < ANCESTOR_CHAIN_MAX_DEPTH
    ) {
        visited.add(pid);
        chain.push(pid);
        const next = tree.get(pid);
        if (next === undefined) break;
        pid = next;
    }
    return chain;
}

function getAncestorPidSet(baseDir) {
    const myPpid = process.ppid;
    if (!Number.isFinite(myPpid) || myPpid <= 0) {
        return null;
    }
    try {
        const cachePath = path.join(baseDir, ANCESTOR_CACHE_FILENAME);
        const raw = fs.readFileSync(cachePath, 'utf8');
        const c = JSON.parse(raw);
        if (
            c &&
            c.ppid === myPpid &&
            typeof c.at === 'number' &&
            Date.now() - c.at < ANCESTOR_CACHE_TTL_MS &&
            Array.isArray(c.ancestors)
        ) {
            return new Set(c.ancestors);
        }
    } catch (_) { /* fall through */ }
    const tree = snapshotProcessTree();
    if (!tree) return null;
    const ancestors = walkAncestors(tree, myPpid);
    try {
        fs.writeFileSync(
            path.join(baseDir, ANCESTOR_CACHE_FILENAME),
            JSON.stringify({ ppid: myPpid, ancestors, at: Date.now() }),
            'utf8'
        );
    } catch (_) { /* not fatal */ }
    return new Set(ancestors);
}

// ---- Pending file classify / drain ----------------------------------------

function classifyPending(pendingPath, ancestorSet) {
    if (!fs.existsSync(pendingPath)) {
        return { present: false, mine: [], othersRaw: [] };
    }
    let content;
    try {
        content = fs.readFileSync(pendingPath, 'utf8');
    } catch (_) {
        return { present: true, mine: [], othersRaw: [] };
    }
    const allLines = content.split('\n').filter(Boolean);
    const mine = [];
    const othersRaw = [];
    for (const raw of allLines) {
        let entry;
        try {
            entry = JSON.parse(raw);
        } catch (_) {
            othersRaw.push(raw);
            continue;
        }
        const owner = entry.owner_cc_pid;
        if (ancestorSet === null) {
            mine.push(entry);
        } else if (owner == null) {
            mine.push(entry);
        } else if (ancestorSet.has(owner)) {
            mine.push(entry);
        } else {
            othersRaw.push(raw);
        }
    }
    return { present: true, mine, othersRaw };
}

function writePeersBack(pendingPath, othersRaw, logFn) {
    try {
        const remaining = othersRaw.length ? othersRaw.join('\n') + '\n' : '';
        fs.writeFileSync(pendingPath, remaining, 'utf8');
    } catch (e) {
        if (logFn) logFn(`write-back failed: ${e.message}`);
    }
}

// ---- Pending file exclusive lock ------------------------------------------

async function acquirePendingLock(dir) {
    const lockPath = path.join(dir, PENDING_LOCK_FILENAME);
    const startedAt = Date.now();
    let backoff = PENDING_LOCK_RETRY_MS;
    while (Date.now() - startedAt < PENDING_LOCK_MAX_WAIT_MS) {
        try {
            const fd = fs.openSync(lockPath, 'wx');
            try {
                fs.writeSync(fd, JSON.stringify({ pid: process.pid, at: Date.now() }));
            } catch (_) { /* forensic only */ }
            fs.closeSync(fd);
            return lockPath;
        } catch (err) {
            if (err && err.code === 'EEXIST') {
                try {
                    const stat = fs.statSync(lockPath);
                    if (Date.now() - stat.mtimeMs > 30_000) {
                        try { fs.unlinkSync(lockPath); } catch (_) { }
                        continue;
                    }
                } catch (_) { /* race with release */ }
                await sleep(backoff);
                backoff = Math.min(backoff * 2, 200);
                continue;
            }
            return null;
        }
    }
    return null;
}

function releasePendingLock(lockPath) {
    if (!lockPath) return;
    try { fs.unlinkSync(lockPath); } catch (_) { /* gone is fine */ }
}

// ---- Base-dir resolution --------------------------------------------------

function readHookEvent() {
    try {
        const data = fs.readFileSync(0, 'utf8');
        if (!data || !data.trim()) return null;
        return JSON.parse(data);
    } catch (_) {
        return null;
    }
}

function resolveBaseDir(cliBase, event, scriptDirname) {
    const AGENT_TEAMS_SUBDIR = '.agent-teams';
    const envDir = process.env.TEAM_MODE_BASE_DIR;
    const seeds = [];
    if (envDir) seeds.push(envDir);
    if (cliBase) seeds.push(cliBase);
    if (event && typeof event.file_path === 'string') {
        seeds.push(path.dirname(event.file_path));
    }
    if (event && typeof event.cwd === 'string') seeds.push(event.cwd);
    if (scriptDirname) seeds.push(path.resolve(scriptDirname, '..', '..'));
    seeds.push(process.cwd());
    const candidates = [];
    for (const seed of seeds) {
        if (!seed) continue;
        candidates.push(seed);
        const sub = path.join(seed, AGENT_TEAMS_SUBDIR);
        if (sub !== seed) candidates.push(sub);
    }
    for (const dir of candidates) {
        if (fs.existsSync(path.join(dir, PENDING_FILENAME))) {
            return dir;
        }
    }
    for (const dir of candidates) {
        try {
            if (fs.statSync(dir).isDirectory()) return dir;
        } catch (_) { /* try next */ }
    }
    return seeds.find(Boolean) || process.cwd();
}

// ---- Generic appendable log ----------------------------------------------

function makeLogger(dir, filename) {
    return (msg) => {
        try {
            const ts = new Date().toISOString();
            fs.appendFileSync(path.join(dir, filename), `[${ts}] ${msg}\n`, 'utf8');
        } catch (_) { /* best-effort */ }
    };
}

// ---- Lead session id mapping (Bug 27) -------------------------------------

function writeLeadSessionMapping(dir, sessionId, ancestorPidSet) {
    if (!sessionId || !ancestorPidSet || ancestorPidSet.size === 0) {
        return;
    }
    const mapPath = path.join(dir, LEAD_SESSIONS_FILENAME);
    let existing = {};
    try {
        const raw = fs.readFileSync(mapPath, 'utf8');
        const parsed = JSON.parse(raw);
        if (parsed && typeof parsed === 'object') existing = parsed;
    } catch (_) { /* start fresh */ }
    const cutoffMs = Date.now() - LEAD_SESSIONS_TTL_MS;
    for (const [pid, entry] of Object.entries(existing)) {
        const ts = entry && entry.updated_at ? Date.parse(entry.updated_at) : NaN;
        if (!Number.isFinite(ts) || ts < cutoffMs) {
            delete existing[pid];
        }
    }
    const nowIso = new Date().toISOString();
    for (const pid of ancestorPidSet) {
        existing[String(pid)] = { session_id: sessionId, updated_at: nowIso };
    }
    try {
        const tmp = mapPath + '.tmp';
        fs.writeFileSync(tmp, JSON.stringify(existing, null, 2), 'utf8');
        fs.renameSync(tmp, mapPath);
    } catch (_) { /* best-effort */ }
}

// ---- Reminder formatting -------------------------------------------------

function kindLabel(k) {
    switch (k) {
        case 'reply': return '回复';
        case 'dispatch': return '派发消息';
        case 'discussion': return '讨论';
        default: return k || 'message';
    }
}

function entryHeader(entry) {
    const from = entry.from || entry.from_id || '?';
    const team = entry.team || '?';
    return `${from} (team: ${team}) ${kindLabel(entry.kind)}:`;
}

// Stop hook (turn-boundary) reminder — terse, the LLM enters a new turn
// to address this so framing is "you got new messages, handle them".
function renderTurnEndReminder(inject) {
    if (inject.length === 1) {
        const e = inject[0];
        return (
            `[TEAM-MODE] 收到新消息 — ${entryHeader(e)}\n\n` +
            `${(e.text || '').trim()}\n`
        );
    }
    const blocks = inject.map((e) => `${entryHeader(e)}\n${(e.text || '').trim()}`);
    return (
        `[TEAM-MODE] 收到 ${inject.length} 条新消息：\n\n` +
        `${blocks.join('\n\n---\n\n')}\n`
    );
}

// PostToolUse (mid-turn) reminder — explicitly framed as a non-disruptive
// push so the LLM doesn't abandon its current task to respond. Uses
// distinct visual delimiters so the LLM doesn't conflate this with tool
// output. Phrasing tells the LLM it can continue working and address the
// message at its own pace; the Stop hook will surface anything still
// unanswered at turn end.
function renderMidTurnReminder(inject) {
    const banner = `─── [TEAM-MODE] mid-turn 团队消息（worker 主动推送，可稍后响应）───`;
    const footer = `─── 你可以继续手头的任务；turn 结束时如果未回应会再次提醒 ───`;
    if (inject.length === 1) {
        const e = inject[0];
        return (
            `${banner}\n\n` +
            `${entryHeader(e)}\n${(e.text || '').trim()}\n\n` +
            `${footer}\n`
        );
    }
    const blocks = inject.map((e) => `${entryHeader(e)}\n${(e.text || '').trim()}`);
    return (
        `${banner}\n\n` +
        `共 ${inject.length} 条新消息：\n\n` +
        `${blocks.join('\n\n---\n\n')}\n\n` +
        `${footer}\n`
    );
}

// ---- Common arg parser ---------------------------------------------------

function parseArgs(argv) {
    let cliBase = null;
    for (let i = 2; i < argv.length; i++) {
        const a = argv[i];
        if (a === '--base' && argv[i + 1]) cliBase = argv[++i];
        else if (!a.startsWith('--')) cliBase = a;
    }
    return { cliBase };
}

module.exports = {
    // Constants
    PENDING_FILENAME,
    ANCESTOR_CACHE_FILENAME,
    PENDING_LOCK_FILENAME,
    LEAD_SESSIONS_FILENAME,
    // Helpers
    sleep,
    snapshotProcessTree,
    walkAncestors,
    getAncestorPidSet,
    classifyPending,
    writePeersBack,
    acquirePendingLock,
    releasePendingLock,
    readHookEvent,
    resolveBaseDir,
    makeLogger,
    writeLeadSessionMapping,
    kindLabel,
    entryHeader,
    renderTurnEndReminder,
    renderMidTurnReminder,
    parseArgs,
};
