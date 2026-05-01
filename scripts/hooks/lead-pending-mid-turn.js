#!/usr/bin/env node
/**
 * lead-pending-mid-turn.js — PostToolUse hook for live worker-reply delivery.
 *
 * Architecture (post-2026-04-30 refactor):
 *   1. CC fires PostToolUse after every tool call
 *   2. Hook resolves "my teams" via session-id-keyed cache
 *      (`<base>/.cc-identity.<session_id>.json`) → service `/lead-pending/my-teams`
 *      on cache miss.
 *   3. Hook drains all `<team_id>/lead_pending.jsonl` files for those teams
 *   4. If anything found: write JSON `{hookSpecificOutput.additionalContext}`
 *      to stdout, exit 0 → injected into LLM context without ending turn.
 *
 * No fallback. If service is down, hook exits 1 with stderr (PostToolUse
 * does NOT block the tool; it just won't inject this round).
 *
 * Cache rationale: PostToolUse fires every tool call. A fresh service round-trip
 * is fast (~ms) but adds noise. session_id is stable for the entire CC
 * lifetime, so caching by session is safe.
 *
 * Worker fast-path: codex worker child processes also inherit CC hooks
 * via env. Workers don't have a lead-pending queue and must not block.
 */
'use strict';

const fs = require('fs');
const path = require('path');

const PROBE_LOG = path.resolve(__dirname, '..', '..', '.mid-turn-probe.log');
const HTTP_RUNTIME_INFO = path.resolve(__dirname, '..', '..', '.agent-teams', 'runtime', 'http-mcp.json');
const PROBE_START_MS = Date.now();
const REPO_ROOT = path.resolve(__dirname, '..', '..');
const IDENTITY_CACHE_DIR = path.join(REPO_ROOT, '.agent-teams');

function probe(step, extra) {
    try {
        const elapsed = Date.now() - PROBE_START_MS;
        const ts = new Date().toISOString();
        const tag = extra ? ` ${extra}` : '';
        fs.appendFileSync(
            PROBE_LOG,
            `[${ts}] pid=${process.pid} +${elapsed}ms step=${step}${tag}\n`,
            'utf8'
        );
    } catch (_) { /* best-effort */ }
}

function readEvent() {
    try {
        const data = fs.readFileSync(0, 'utf8');
        return data && data.trim() ? JSON.parse(data) : null;
    } catch (_) { return null; }
}

function readRuntimeInfo() {
    const raw = fs.readFileSync(HTTP_RUNTIME_INFO, 'utf8');
    const info = JSON.parse(raw);
    const tokenFilePath = path.resolve(path.dirname(HTTP_RUNTIME_INFO), '..', '..', info.token_file || info.tokenFile);
    const token = fs.readFileSync(tokenFilePath, 'utf8').trim();
    return {
        url: info.url || `http://${info.host || '127.0.0.1'}:${info.port || 8786}`,
        token,
    };
}

function identityCachePath(sessionId) {
    // Sanitise session_id (UUID-ish) before using as filename.
    const safe = String(sessionId || '').replace(/[^a-zA-Z0-9_.-]/g, '_').slice(0, 80);
    return path.join(IDENTITY_CACHE_DIR, `.cc-identity.${safe}.json`);
}

function readIdentityCache(sessionId) {
    try {
        const p = identityCachePath(sessionId);
        if (!fs.existsSync(p)) return null;
        const raw = fs.readFileSync(p, 'utf8');
        const parsed = JSON.parse(raw);
        if (parsed && parsed.session_id === sessionId && Array.isArray(parsed.teams)) {
            return parsed;
        }
        return null;
    } catch (_) { return null; }
}

function writeIdentityCache(sessionId, payload) {
    try {
        const p = identityCachePath(sessionId);
        fs.mkdirSync(path.dirname(p), { recursive: true });
        const tmp = p + '.tmp';
        fs.writeFileSync(tmp, JSON.stringify({ ...payload, session_id: sessionId, cached_at: new Date().toISOString() }, null, 2), 'utf8');
        fs.renameSync(tmp, p);
    } catch (_) { /* best-effort */ }
}

async function fetchMyTeams(serviceUrl, token, sessionId) {
    const url = new URL('/lead-pending/my-teams', serviceUrl);
    url.searchParams.set('pid', String(process.pid));
    if (sessionId) url.searchParams.set('session_id', sessionId);
    const resp = await fetch(url, {
        headers: { Authorization: `Bearer ${token}` },
    });
    if (!resp.ok) {
        const body = await resp.text().catch(() => '');
        throw new Error(`my-teams HTTP ${resp.status}: ${body}`);
    }
    return await resp.json();
}

// Atomic drain via rename — same pattern as async-wake hook.
// Only one process can rename a given file; losers see ENOENT and skip.
// Prevents two hooks from both reading + injecting the same entry.
function drainPendingFile(pendingPath) {
    if (!fs.existsSync(pendingPath)) return [];
    const tmp = `${pendingPath}.draining-${process.pid}-${Date.now()}`;
    try {
        fs.renameSync(pendingPath, tmp);
    } catch (e) {
        if (e && e.code !== 'ENOENT') {
            probe('drain-rename-error', `path=${pendingPath} error=${e.message}`);
        }
        return [];
    }
    let raw = '';
    try { raw = fs.readFileSync(tmp, 'utf8'); } catch (_) { /* empty/missing */ }
    try { fs.unlinkSync(tmp); } catch (_) { /* swallow */ }
    if (!raw.trim()) return [];
    const entries = [];
    for (const line of raw.split('\n')) {
        if (!line.trim()) continue;
        try { entries.push(JSON.parse(line)); }
        catch (_) { /* skip malformed */ }
    }
    return entries;
}

function renderMidTurnReminder(entries) {
    const banner = '─── [TEAM-MODE] mid-turn 团队消息（worker 主动推送，可稍后响应）───';
    const footer = '─── 你可以继续手头的任务；turn 结束时如果未回应会再次提醒 ───';
    if (entries.length === 1) {
        const e = entries[0];
        return `${banner}\n\n[team=${e.team || '?'}] ${e.from || e.from_id || '?'} (${e.kind || 'message'}):\n${(e.text || '').trim()}\n\n${footer}\n`;
    }
    const blocks = entries.map((e) =>
        `[team=${e.team || '?'}] ${e.from || e.from_id || '?'} (${e.kind || 'message'}):\n${(e.text || '').trim()}`
    );
    return `${banner}\n\n共 ${entries.length} 条新消息：\n\n${blocks.join('\n\n---\n\n')}\n\n${footer}\n`;
}

(async () => {
    probe('main-enter');

    if (process.env.TEAM_MODE_WORKER === '1') {
        probe('worker-fast-path-exit');
        process.exit(0);
    }

    const event = readEvent();
    const sessionId = event && event.session_id;
    probe('after-readEvent', `session=${sessionId || 'null'}`);
    if (!sessionId) {
        // No session_id → can't cache identity. Skip — Stop hook will pick up.
        probe('no-session-skip');
        process.exit(0);
    }

    // Identity resolution: cache → service.
    let identity = readIdentityCache(sessionId);
    if (!identity) {
        let runtimeInfo;
        try {
            runtimeInfo = readRuntimeInfo();
        } catch (e) {
            probe('runtime-info-failed', `error=${e.message}`);
            process.stderr.write(`lead-pending-mid-turn: cannot read service runtime info: ${e.message}\n`);
            process.exit(1);
        }
        try {
            const result = await fetchMyTeams(runtimeInfo.url, runtimeInfo.token, sessionId);
            identity = {
                cc_pid: result.cc_pid,
                teams: Array.isArray(result.teams) ? result.teams : [],
            };
            writeIdentityCache(sessionId, identity);
            probe('my-teams-resolved', `cc_pid=${identity.cc_pid} count=${identity.teams.length}`);
        } catch (e) {
            probe('my-teams-failed', `error=${e.message}`);
            process.stderr.write(`lead-pending-mid-turn: /my-teams query failed: ${e.message}\n`);
            process.exit(1);
        }
    } else {
        probe('cache-hit', `count=${identity.teams.length}`);
    }

    if (identity.teams.length === 0) {
        probe('no-teams-skip');
        process.exit(0);
    }

    // Drain all my teams' pending files.
    const found = [];
    for (const t of identity.teams) {
        const entries = drainPendingFile(t.pending_path);
        if (entries.length > 0) found.push(...entries);
    }
    if (found.length === 0) {
        probe('nothing-to-inject');
        process.exit(0);
    }

    probe('inject', `entries=${found.length}`);
    process.stdout.write(JSON.stringify({
        hookSpecificOutput: {
            hookEventName: 'PostToolUse',
            additionalContext: renderMidTurnReminder(found),
        },
    }));
    process.exit(0);
})().catch((e) => {
    probe('fatal', `error=${e && e.message ? e.message : String(e)}`);
    process.stderr.write(`lead-pending-mid-turn fatal: ${e && e.message ? e.message : e}\n`);
    process.exit(0); // PostToolUse: never bubble errors into tool flow
});
