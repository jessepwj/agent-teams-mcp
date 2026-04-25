#!/usr/bin/env node
/**
 * lead-pending-wake.js — dual-purpose hook for team-mode lead pending queue.
 *
 * Handles TWO Claude Code hook events based on stdin `hook_event_name`:
 *
 *   FileChanged (idle push path):
 *     Reads `lead_pending.jsonl`, filters lines owned by this CC (matched via
 *     process.ppid vs. the entry's owner_cc_pid), emits stderr + exit 2 so
 *     `asyncRewake: true` surfaces a <system-reminder> when CC is idle.
 *
 *   Stop (shepherd-loop path):
 *     Triggered when CC finishes a turn and is about to go idle. Polls
 *     `lead_pending.jsonl` every 500ms. If there's traffic for this CC,
 *     blocks with exit 0 + stdout JSON `{decision:"block", reason:...}`
 *     (CC enters a new turn with the reason as a system-reminder). If
 *     pending is empty, keeps waiting up to TEAM_MODE_STOP_WAIT_SEC
 *     (default 7200s = 2 hours) regardless of whether this Stop was caused
 *     by a prior hook-inject. User ESC sends SIGINT so the user can
 *     reclaim the prompt mid-wait.
 *
 * Loop-guard design (important):
 *   Pre-Bug-11 code early-exited on `stop_hook_active=true` or a cooldown
 *   file to avoid re-blocking with "same" content. That guard was doubly
 *   wrong:
 *     1. It dropped genuinely NEW messages that arrived between the last
 *        inject and this Stop (Bug 11: 4-worker concurrent reply race).
 *     2. It was unnecessary — `tryBlock` drains `mine` from pending before
 *        blocking, so "same content" can never be re-delivered. Every
 *        block is for genuinely new work.
 *   Current design: every Stop hook runs a full shepherd wait. For
 *   team-mode's long-running workflows, the lead CC should stay attentive
 *   to worker replies indefinitely; the user's ESC key is the ONE
 *   mechanism that legitimately reclaims control.
 *   The `.stop-hook-cooldown` file is still written (forensic breadcrumb
 *   showing when we last injected) but does not affect control flow.
 *
 * Environment / args:
 *   stdin JSON                        — Claude Code passes `{file_path, cwd, ...}`
 *                                       on stdin for FileChanged events. The
 *                                       script uses `dirname(file_path)` as the
 *                                       authoritative base_dir. This is the
 *                                       recommended path: fully portable, no
 *                                       configuration needed.
 *   TEAM_MODE_BASE_DIR (env)          — explicit data dir. Overrides stdin.
 *   --base <dir>                      — data dir via CLI.
 *   positional arg                    — data dir via CLI (legacy).
 *
 * Resolution order (first match wins):
 *   env var → CLI arg → stdin file_path → stdin cwd+`.agent-teams` →
 *   script-relative (`__dirname/../.agent-teams`) → cwd+`.agent-teams` → cwd.
 *
 * Exit codes:
 *   0  — nothing to inject (file missing, empty, already drained).
 *   1  — fatal script error.
 *   2  — injected content to stderr; signals Claude Code to wake & surface
 *        the stderr as a `<system-reminder>`.
 */

'use strict';

const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const PENDING_FILENAME = 'lead_pending.jsonl';
const LOG_FILENAME = '.lead-pending-wake.log';
const COOLDOWN_FILENAME = '.stop-hook-cooldown';
const ANCESTOR_CACHE_FILENAME = '.ancestor-cache.json';
const ANCESTOR_CACHE_TTL_MS = 5_000;        // enough to span the Stop hook poll loop
const ANCESTOR_CHAIN_MAX_DEPTH = 40;         // deep enough for any real dispatcher, bounds runaway
const PROC_QUERY_TIMEOUT_MS = 5_000;
const COOLDOWN_MS = 10_000;                 // retained as a cooldown-file TTL breadcrumb; no longer gates hook behavior
const DEFAULT_STOP_WAIT_SEC = 7200;         // 2 hours; override via TEAM_MODE_STOP_WAIT_SEC
const POLL_INTERVAL_MS = 500;

function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

// ---------------------------------------------------------------------------
// Ancestor-chain process identity
//
// Why this exists:
//   Claude Code spawns hook scripts indirectly through a dispatcher process,
//   so `process.ppid` is NOT the CC client PID — it's some shell or node
//   wrapper. But the Rust MCP server IS a direct child of CC, so it knows
//   CC's real PID and tags every `lead_pending.jsonl` entry with it as
//   `owner_cc_pid`. To route messages correctly in multi-CC-same-project
//   scenarios, the hook script walks its ancestor PID chain: if any
//   ancestor PID matches a pending entry's `owner_cc_pid`, that entry
//   belongs to our CC.
//
// Platform notes:
//   Windows: PowerShell's `Get-CimInstance Win32_Process` gives a snapshot
//            of all processes + their ParentProcessId. ~1 second cold start.
//   Unix:    `ps -eo pid=,ppid=` is instantaneous.
//
// Caching:
//   A one-shot snapshot is cached per-ppid for 5 seconds in
//   `<baseDir>/.ancestor-cache.json`. The Stop hook poll loop calls this
//   repeatedly; without caching each 500ms poll would trigger a fresh
//   PowerShell invocation on Windows.
//
// Fallback:
//   If process-tree query fails (sandbox, missing ps/PowerShell), return
//   `null`. The caller then treats EVERY pending line as "ours" — strictly
//   less safe for multi-CC but guarantees we never silently drop messages.
// ---------------------------------------------------------------------------

function snapshotProcessTree() {
    try {
        if (process.platform === 'win32') {
            // Get-CimInstance is the supported replacement for deprecated wmic.
            // JSON output: array of {ProcessId, ParentProcessId} (or a single
            // object if only one match — unlikely here).
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
            // Portable `-o pid=,ppid=` elides the header; both columns are
            // whitespace-separated decimal PIDs.
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

// Returns Set<number> of ancestor PIDs including process.ppid. Returns null
// if we couldn't query the process tree (caller should fall back to "consume
// all", accepting multi-CC ambiguity but never dropping messages).
function getAncestorPidSet(baseDir) {
    const myPpid = process.ppid;
    if (!Number.isFinite(myPpid) || myPpid <= 0) {
        return null;
    }

    // Try cache first to avoid PowerShell startup cost in the Stop poll loop.
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
    } catch (_) {
        // No valid cache — fall through to fresh snapshot.
    }

    const tree = snapshotProcessTree();
    if (!tree) return null;

    const ancestors = walkAncestors(tree, myPpid);

    // Best-effort cache write.
    try {
        fs.writeFileSync(
            path.join(baseDir, ANCESTOR_CACHE_FILENAME),
            JSON.stringify({ ppid: myPpid, ancestors, at: Date.now() }),
            'utf8'
        );
    } catch (_) {
        // not fatal
    }

    return new Set(ancestors);
}

function readCooldown(dir) {
    try {
        const raw = fs.readFileSync(path.join(dir, COOLDOWN_FILENAME), 'utf8');
        return JSON.parse(raw);
    } catch (_) {
        return null;
    }
}

function writeCooldown(dir, sessionId) {
    if (!sessionId) return;
    try {
        fs.writeFileSync(
            path.join(dir, COOLDOWN_FILENAME),
            JSON.stringify({ session_id: sessionId, at: Date.now() }),
            'utf8'
        );
    } catch (_) {
        // best-effort
    }
}

function log(dir, msg) {
    try {
        const ts = new Date().toISOString();
        fs.appendFileSync(path.join(dir, LOG_FILENAME), `[${ts}] ${msg}\n`, 'utf8');
    } catch (_) {
        // logging is best-effort
    }
}

function parseArgs(argv) {
    let cliBase = null;
    for (let i = 2; i < argv.length; i++) {
        const a = argv[i];
        if (a === '--base' && argv[i + 1]) {
            cliBase = argv[++i];
        } else if (!a.startsWith('--')) {
            cliBase = a;
        }
    }
    return { cliBase };
}

function readHookEvent() {
    // Claude Code passes FileChanged payload as JSON on stdin: {file_path, cwd, ...}
    // stdin may be empty (manual invocation) — read non-blocking and tolerate.
    try {
        const data = fs.readFileSync(0, 'utf8');
        if (!data || !data.trim()) return null;
        return JSON.parse(data);
    } catch (_) {
        return null;
    }
}

function resolveBaseDir(cliBase, event) {
    const AGENT_TEAMS_SUBDIR = '.agent-teams';
    const envDir = process.env.TEAM_MODE_BASE_DIR;

    // Seed candidates in priority order. Each seed is probed both as-is and
    // with an appended `.agent-teams` subdir so a "repo root" or "project cwd"
    // candidate auto-resolves to its on-disk data dir.
    const seeds = [];
    if (envDir) seeds.push(envDir);
    if (cliBase) seeds.push(cliBase);
    if (event && typeof event.file_path === 'string') {
        // Most authoritative: the actual file that changed. dirname() = base_dir.
        seeds.push(path.dirname(event.file_path));
    }
    if (event && typeof event.cwd === 'string') seeds.push(event.cwd);
    seeds.push(path.resolve(__dirname, '..', '..')); // this script's repo root
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
    // No pending file yet — prefer the first existing directory so logs land somewhere sane.
    for (const dir of candidates) {
        try {
            if (fs.statSync(dir).isDirectory()) return dir;
        } catch (_) { /* try next */ }
    }
    return seeds.find(Boolean) || process.cwd();
}

function formatEntries(lines) {
    const parts = [];
    for (const raw of lines) {
        if (!raw.trim()) continue;
        let entry;
        try {
            entry = JSON.parse(raw);
        } catch (_) {
            parts.push(`  (malformed) ${raw}`);
            continue;
        }
        const team = entry.team || '?';
        const from = entry.from || entry.from_id || '?';
        const kind = entry.kind || 'message';
        const text = entry.text || '';
        parts.push(`  - [team=${team}] ${from} (${kind}): ${text}`);
    }
    return parts.join('\n');
}

// Read pending file and classify each line by ownership.
//
// Routing logic:
//   - `ancestorSet` is the Set of CC PIDs that are ancestors of this hook's
//     process (computed via walkAncestors over a platform process-tree
//     snapshot). If an entry's `owner_cc_pid` is in this set, the entry
//     was written by a team whose owner-CC is in our ancestry chain —
//     i.e. our CC. Consume it.
//   - Entries with `owner_cc_pid == null` (legacy / unbound) are treated
//     as ours to drain. Real Rust-written entries always carry the field.
//   - Entries whose `owner_cc_pid` is set but NOT in our ancestry belong
//     to another CC on the same machine. Preserve them verbatim; that
//     other CC's hook will claim them on its next fire.
//   - If `ancestorSet === null` (tree query failed), we can't route
//     safely. Degrade to "consume everything parseable" rather than
//     drop messages on the floor. Multi-CC setups on a misconfigured
//     machine will have duplicates in this case, but single-CC (the
//     99% case) still works.
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
            // Unparseable — keep verbatim rather than dropping.
            othersRaw.push(raw);
            continue;
        }
        const owner = entry.owner_cc_pid;
        if (ancestorSet === null) {
            mine.push(entry); // fallback — no routing possible
        } else if (owner == null) {
            mine.push(entry); // unbound / legacy entry
        } else if (ancestorSet.has(owner)) {
            mine.push(entry); // our CC
        } else {
            othersRaw.push(raw); // belongs to a peer CC
        }
    }
    return { present: true, mine, othersRaw };
}

// Format the reminder body surfaced to Claude (as JSON "reason" for Stop
// hook block, or plain stderr for FileChanged). Optimized for readability:
//   - single message → compact inline header + body
//   - multiple messages → grouped list with per-entry header
//   - no repeated boilerplate ("inspect and respond", "call inbox_read")
//     once is enough; the lead AI knows what to do
//   - team & sender rendered naturally (`alice (team: diag)`) not as
//     machine-ish `[team=diag] alice`
//   - original message body preserved verbatim as its own block so
//     multi-line worker replies stay readable
function renderReminder(inject) {
    const kindLabel = (k) => {
        switch (k) {
            case 'reply': return '回复';
            case 'dispatch': return '派发消息';
            case 'discussion': return '讨论';
            default: return k || 'message';
        }
    };

    const header = (entry) => {
        const from = entry.from || entry.from_id || '?';
        const team = entry.team || '?';
        return `${from} (team: ${team}) ${kindLabel(entry.kind)}:`;
    };

    if (inject.length === 1) {
        const e = inject[0];
        return (
            `[TEAM-MODE] 收到新消息 — ${header(e)}\n\n` +
            `${(e.text || '').trim()}\n`
        );
    }

    // Multiple messages — list form.
    const blocks = inject.map((e) => {
        return `${header(e)}\n${(e.text || '').trim()}`;
    });
    return (
        `[TEAM-MODE] 收到 ${inject.length} 条新消息：\n\n` +
        `${blocks.join('\n\n---\n\n')}\n`
    );
}

// Persist "peer" lines back to pending after consuming our own. Shared by
// FileChanged and Stop branches.
function writePeersBack(pendingPath, othersRaw, baseDir) {
    try {
        const remaining = othersRaw.length
            ? othersRaw.join('\n') + '\n'
            : '';
        fs.writeFileSync(pendingPath, remaining, 'utf8');
    } catch (e) {
        log(baseDir, `write-back failed: ${e.message}`);
    }
}

// ---------------------------------------------------------------------------
// FileChanged handler — idle-push path (asyncRewake).
// ---------------------------------------------------------------------------
function handleFileChanged(baseDir, pendingPath) {
    const ancestorSet = getAncestorPidSet(baseDir);
    const c = classifyPending(pendingPath, ancestorSet);
    if (!c.present) {
        log(baseDir, `no pending file at ${pendingPath}, exit 0`);
        process.exit(0);
    }
    if (c.mine.length === 0 && c.othersRaw.length === 0) {
        log(baseDir, 'pending file empty, exit 0');
        process.exit(0);
    }
    writePeersBack(pendingPath, c.othersRaw, baseDir);

    const routingNote = ancestorSet === null
        ? 'NO-ROUTING'
        : `ancestors=${[...ancestorSet].slice(0, 5).join(',')}${ancestorSet.size > 5 ? '...' : ''}`;

    if (c.mine.length === 0) {
        log(baseDir, `filechanged: 0 for me, kept ${c.othersRaw.length} for peers [${routingNote}], exit 0`);
        process.exit(0);
    }
    process.stderr.write(renderReminder(c.mine));
    log(baseDir, `filechanged: injected ${c.mine.length}, kept ${c.othersRaw.length} for peers [${routingNote}], exit 2`);
    process.exit(2);
}

// ---------------------------------------------------------------------------
// Stop handler — shepherd-loop path. On a fresh Stop, polls for new traffic
// up to TEAM_MODE_STOP_WAIT_SEC. On a follow-up Stop (one that fired right
// after a previous inject), polls only TEAM_MODE_STOP_TAIL_SEC — the
// primary guarantee for catching undelivered messages is that every Stop
// runs tryBlock() unconditionally at the top, so stragglers are always
// picked up on the NEXT hook fire even if this tail exits immediately.
// Exits 0 with JSON stdout `{decision:"block",reason:...}` when content
// is injected; exits 0 cleanly on wait timeout or SIGINT (user ESC).
// ---------------------------------------------------------------------------
async function handleStop(event, baseDir, pendingPath) {
    const sessionId = event && event.session_id;

    // User ESC / process termination: exit cleanly so the user can type.
    const gracefulExit = () => {
        log(baseDir, 'stop: interrupted by signal, exit 0');
        process.exit(0);
    };
    process.on('SIGINT', gracefulExit);
    process.on('SIGTERM', gracefulExit);
    process.on('SIGHUP', gracefulExit);

    // Compute ancestor set once at hook entry — the process tree doesn't
    // change while this hook lives (CC PID is stable), and the cache layer
    // also shelters the Stop poll loop from repeat PowerShell invocations.
    const ancestorSet = getAncestorPidSet(baseDir);
    const routingNote = ancestorSet === null
        ? 'NO-ROUTING'
        : `ancestors=${[...ancestorSet].slice(0, 5).join(',')}${ancestorSet.size > 5 ? '...' : ''}`;

    // Try-and-block helper shared by initial check + poll loop.
    //
    // Block via `exit 0 + JSON stdout {decision:"block", reason:"..."}`
    // instead of the older `exit 2 + stderr` path. Both trigger a new
    // turn identically, but the JSON form gives Claude a clean reason
    // string. The exit-2 form makes CC wrap the stderr with a
    // "Stop hook error: [node ...]:" prefix (GitHub CC issue #34600),
    // which is just cosmetic but confuses the lead AI and the user.
    //
    // IMPORTANT: `tryBlock` always drains `mine` via `writePeersBack`, so
    // any entries we see on a subsequent call are guaranteed to be NEW
    // — they cannot be a replay of something we already injected. That
    // makes the "infinite-loop" risk from repeatedly blocking effectively
    // zero; the loop markers below only change how long we are willing
    // to WAIT, never whether we inject real content.
    // BATCH_GRACE_MS: when we detect at least one message ready to inject,
    // pause briefly and re-scan once before actually blocking. This catches
    // the case where the lead broadcasts to N workers and replies arrive
    // staggered — the first reply hits pending, we'd inject it solo, and
    // the next 2-3 replies (landing milliseconds later) would only surface
    // on the *next* Stop hook cycle. 500ms is short enough to feel
    // immediate to the user and long enough to coalesce a typical fan-out
    // into a single "N 条新消息" reminder.
    const BATCH_GRACE_MS = parseInt(
        process.env.TEAM_MODE_STOP_BATCH_GRACE_MS || '500',
        10
    );

    const tryBlock = async () => {
        let c = classifyPending(pendingPath, ancestorSet);
        if (c.mine.length === 0) return false;
        if (BATCH_GRACE_MS > 0) {
            await sleep(BATCH_GRACE_MS);
            // Re-classify; new entries from concurrent worker replies are
            // additive — `c.mine` cannot shrink because nobody else drains
            // pending while this hook holds it.
            c = classifyPending(pendingPath, ancestorSet);
        }
        writePeersBack(pendingPath, c.othersRaw, baseDir);
        writeCooldown(baseDir, sessionId);
        process.stdout.write(
            JSON.stringify({ decision: 'block', reason: renderReminder(c.mine) })
        );
        log(baseDir, `stop: injected ${c.mine.length} (batch grace ${BATCH_GRACE_MS}ms), kept ${c.othersRaw.length} for peers [${routingNote}], exit 0 (block via JSON)`);
        process.exit(0);
    };

    // ---- UNCONDITIONAL first check ----
    // Check pending BEFORE entering the wait loop. Pending can already hold
    // messages that arrived between the last hook fire and this one (e.g.
    // workers replied while CC was processing an injected batch).
    await tryBlock();

    // ---- Poll loop ----
    // Always wait the full TEAM_MODE_STOP_WAIT_SEC window regardless of
    // whether this is a "fresh" Stop or a follow-up (just injected on the
    // previous hook fire). Team-mode is designed for long-running work:
    // workers may reply any time during their task, and the lead CC should
    // stay attentive — injecting each reply the moment it arrives — rather
    // than giving control back to the user after every inject.
    //
    // The user can always press ESC to break out of the wait and reclaim
    // the prompt; SIGINT is handled above. That's the ONE correct way to
    // say "I want control back" — there's no meaningful difference
    // between "fresh" and "follow-up" from a shepherding perspective.
    //
    // `stop_hook_active` and the cooldown file are NOT consulted for
    // exit-early decisions anymore. `tryBlock` always drains before
    // blocking, so "same content re-injected" can't happen; the original
    // guard rationale (avoid infinite loop of identical blocks) does not
    // apply. Cooldown file is still written (by `tryBlock`) purely as a
    // forensic breadcrumb.
    const waitSec = parseInt(
        process.env.TEAM_MODE_STOP_WAIT_SEC || String(DEFAULT_STOP_WAIT_SEC),
        10
    );
    const deadline = Date.now() + waitSec * 1000;
    log(baseDir, `stop: shepherd wait up to ${waitSec}s [${routingNote}] session=${sessionId || '?'}`);
    while (Date.now() < deadline) {
        await sleep(POLL_INTERVAL_MS);
        await tryBlock();
    }
    log(baseDir, `stop: wait timed out after ${waitSec}s, exit 0`);
    process.exit(0);
}

async function main() {
    // FAST-PATH: if this hook was fired from a team-mode worker's own
    // claude CLI subprocess (marked by TEAM_MODE_WORKER=1 env set in
    // src/backend/claude_code.rs::spawn_child), short-circuit immediately.
    // Workers are not leads — they must not wait on the lead-pending queue,
    // or their own turn completion ("type:result") stays blocked until
    // hook timeout. Blocking workers starves the parent MCP's agent_loop
    // and breaks every reply.
    if (process.env.TEAM_MODE_WORKER === '1') {
        process.exit(0);
    }

    const { cliBase } = parseArgs(process.argv);
    const event = readHookEvent();
    const baseDir = resolveBaseDir(cliBase, event);
    const pendingPath = path.join(baseDir, PENDING_FILENAME);
    const evName = event && event.hook_event_name;

    if (evName === 'Stop') {
        await handleStop(event, baseDir, pendingPath);
        return;
    }
    // FileChanged is the default branch (also covers absent stdin / other
    // unknown events — conservatively treat as one-shot drain).
    handleFileChanged(baseDir, pendingPath);
}

main().catch((e) => {
    process.stderr.write(`lead-pending-wake fatal: ${e && e.message ? e.message : e}\n`);
    process.exit(1);
});
