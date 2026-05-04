#!/usr/bin/env node
/**
 * lead-pending-async-wake.js — Stop hook with asyncRewake.
 *
 * Architecture (post-2026-04-30 refactor):
 *   1. CC fires Stop event → settings.json spawns this hook
 *      (asyncRewake:true → background, CC continues idle).
 *   2. Hook fetches `/lead-pending/my-teams` from local team_mode_service
 *      to learn which `<base>/<team_id>/lead_pending.jsonl` files belong
 *      to this CC (one HTTP call, no PowerShell).
 *   3. Hook polls those per-team files every 500ms.
 *   4. On first non-empty: drain ALL of them (entire file is mine since
 *      service partitioned at write time), batch-grace-wait 300ms for
 *      stragglers, write stderr + exit 2 → CC wakes and injects.
 *   5. On 7200s timeout (settings.json, seconds) hook gets killed; next Stop
 *      event spawns a fresh instance.
 *
 * No fallback. If service is unreachable or returns error, hook logs the
 * failure and exits 1 — surface the bug, don't mask it. Per project
 * directive 2026-04-30.
 *
 * Coordination with mid-turn (PostToolUse) hook:
 *   Both hooks read the SAME per-team files. Each drain first atomically
 *   renames the pending file to a process-local temp path; the winner owns
 *   those entries and losers see ENOENT/empty. No shared lock is needed
 *   because per-team routing gives this CC sole logical ownership, and
 *   atomic rename prevents duplicate read+inject races.
 */
'use strict';

const fs = require('fs');
const path = require('path');

const POLL_INTERVAL_MS = 500;
const BATCH_GRACE_MS = 2000;
const PROJECT_ROOT = resolveProjectRoot();
const PROBE_LOG = path.join(PROJECT_ROOT, '.agent-teams', '.async-wake-probe.log');
const HTTP_RUNTIME_INFO = path.join(PROJECT_ROOT, '.agent-teams', 'runtime', 'http-mcp.json');
const PROBE_START_MS = Date.now();

function resolveProjectRoot() {
    if (process.env.CLAUDE_PROJECT_DIR) {
        return process.env.CLAUDE_PROJECT_DIR;
    }
    const candidates = [
        process.cwd(),
        path.resolve(__dirname, '..', '..'),
        path.resolve(__dirname, '..', '..', '..'),
    ];
    for (const candidate of candidates) {
        if (fs.existsSync(path.join(candidate, '.agent-teams', 'runtime', 'http-mcp.json'))) {
            return candidate;
        }
    }
    return process.cwd();
}

function probe(step, extra) {
    try {
        const elapsed = Date.now() - PROBE_START_MS;
        const ts = new Date().toISOString();
        const tag = extra ? ` ${extra}` : '';
        fs.appendFileSync(
            PROBE_LOG,
            `[${ts}] pid=${process.pid} ppid=${process.ppid} +${elapsed}ms step=${step}${tag}\n`,
            'utf8'
        );
    } catch (_) { /* best-effort */ }
}
probe('script-loaded');

function readEvent() {
    try {
        const data = fs.readFileSync(0, 'utf8');
        return data && data.trim() ? JSON.parse(data) : null;
    } catch (_) { return null; }
}

function readRuntimeInfo() {
    const raw = fs.readFileSync(HTTP_RUNTIME_INFO, 'utf8');
    const info = JSON.parse(raw);
    const tokenFilePath = path.resolve(PROJECT_ROOT, info.token_file || info.tokenFile);
    const token = fs.readFileSync(tokenFilePath, 'utf8').trim();
    return {
        url: info.url || `http://${info.host || '127.0.0.1'}:${info.port || 8786}`,
        token,
    };
}

async function fetchMyTeams(serviceUrl, token, sessionId) {
    const url = new URL('/lead-pending/my-teams', serviceUrl);
    url.searchParams.set('pid', String(process.pid));
    if (sessionId) url.searchParams.set('session_id', sessionId);
    const resp = await fetch(url, {
        headers: {
            Authorization: `Bearer ${token}`,
            'X-Team-Mode-Project-Root': PROJECT_ROOT,
        },
    });
    if (!resp.ok) {
        const body = await resp.text().catch(() => '');
        throw new Error(`my-teams HTTP ${resp.status}: ${body}`);
    }
    return await resp.json();
}

// Atomic drain via rename: only one hook can rename a given file. Losers
// see ENOENT and skip. Without this, two hooks can `readFileSync` the same
// content before either calls `writeFileSync('')`, producing duplicate
// injects (observed in PoC: 3 old hooks all caught the same researcher
// reply, three Stop hook injections of identical content).
function drainPendingFile(pendingPath) {
    if (!fs.existsSync(pendingPath)) return [];
    const tmp = `${pendingPath}.draining-${process.pid}-${Date.now()}`;
    try {
        fs.renameSync(pendingPath, tmp);
    } catch (e) {
        // ENOENT: another hook drained it just now. Empty/missing files are
        // also handled here. Both are non-errors; just nothing for us.
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

function renderReminder(entries) {
    const banner = '─── [TEAM-MODE] 新团队消息 ───';
    const footer = '─── 你可以继续手头的任务 ───';
    if (entries.length === 1) {
        const e = entries[0];
        return `${banner}\n\n[team=${e.team || '?'}] ${e.from || e.from_id || '?'} (${e.kind || 'message'}):\n${(e.text || '').trim()}\n\n${footer}\n`;
    }
    const blocks = entries.map((e) =>
        `[team=${e.team || '?'}] ${e.from || e.from_id || '?'} (${e.kind || 'message'}):\n${(e.text || '').trim()}`
    );
    return `${banner}\n\n共 ${entries.length} 条新消息：\n\n${blocks.join('\n\n---\n\n')}\n\n${footer}\n`;
}

function sleep(ms) { return new Promise((r) => setTimeout(r, ms)); }

(async () => {
    probe('main-enter');

    // codex worker subprocesses also inherit CC hooks via env; they must
    // not block in shepherd loops or their MCP relay starves.
    if (process.env.TEAM_MODE_WORKER === '1') {
        probe('worker-fast-path-exit');
        process.exit(0);
    }

    const event = readEvent();
    const evName = event && event.hook_event_name;
    const sessionId = (event && event.session_id) || null;
    probe('after-readEvent', `evName=${evName} session=${sessionId || 'null'}`);
    if (evName !== 'Stop') {
        probe('non-stop-exit');
        process.exit(0);
    }

    let runtimeInfo;
    try {
        runtimeInfo = readRuntimeInfo();
    } catch (e) {
        probe('runtime-info-failed', `error=${e.message}`);
        process.stderr.write(`lead-pending-async-wake: cannot read service runtime info: ${e.message}\n`);
        process.exit(1);
    }

    let myTeams;
    try {
        const result = await fetchMyTeams(runtimeInfo.url, runtimeInfo.token, sessionId);
        myTeams = Array.isArray(result.teams) ? result.teams : [];
        probe('my-teams-resolved', `cc_pid=${result.cc_pid} count=${myTeams.length}`);
    } catch (e) {
        probe('my-teams-failed', `error=${e.message}`);
        process.stderr.write(`lead-pending-async-wake: /my-teams query failed: ${e.message}\n`);
        process.exit(1);
    }

    if (myTeams.length === 0) {
        // No teams owned by this CC — nothing to wait on. Sleep until
        // CC cancels (7200s in this repo) so a future team_create gets a fresh
        // hook spawn on the next Stop event.
        probe('no-teams-idle');
        // eslint-disable-next-line no-constant-condition
        while (true) await sleep(60_000);
    }

    // Polling loop: scan all my team pending files; on first hit, batch-
    // grace wait then exit 2.
    let pollCount = 0;
    while (true) {
        pollCount += 1;
        const found = [];
        for (const t of myTeams) {
            const entries = drainPendingFile(t.pending_path);
            if (entries.length > 0) found.push(...entries);
        }
        if (found.length > 0) {
            // Batch grace: re-scan after BATCH_GRACE_MS to catch a fan-out
            // burst (multiple workers replying within a few hundred ms).
            await sleep(BATCH_GRACE_MS);
            for (const t of myTeams) {
                const more = drainPendingFile(t.pending_path);
                if (more.length > 0) found.push(...more);
            }
            probe('inject-exit-2', `polls=${pollCount} entries=${found.length}`);
            process.stderr.write(renderReminder(found));
            process.exit(2);
        }
        await sleep(POLL_INTERVAL_MS);
    }
})().catch((e) => {
    probe('fatal', `error=${e && e.message ? e.message : String(e)}`);
    process.stderr.write(`lead-pending-async-wake fatal: ${e && e.message ? e.message : e}\n`);
    process.exit(1);
});
