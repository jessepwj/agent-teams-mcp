#!/usr/bin/env node
/**
 * lead-pending-wake.js — FileChanged + asyncRewake hook for team-mode lead.
 *
 * Reads `<team-mode base_dir>/lead_pending.jsonl`, emits an aggregated message
 * to stderr formatted so Claude (the Lead) recognizes it as incoming worker
 * traffic, then clears the queue and exits with code 2 so Claude Code treats
 * the stderr as a `<system-reminder>` and wakes the session.
 *
 * Invocation: configured in ~/.claude/settings.json under `hooks.FileChanged`
 * with `matcher: "lead_pending.jsonl"`, `async: true`, `asyncRewake: true`.
 *
 * Environment / args:
 *   TEAM_MODE_BASE_DIR (env)          — explicit data dir. Overrides everything.
 *   --base <dir>                      — data dir via CLI.
 *   positional arg                    — data dir via CLI (legacy).
 *
 * Fallbacks (in order): env var → CLI arg → sibling of this script (parent
 * dir) → current working directory. The first directory that contains
 * `lead_pending.jsonl` wins.
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

const PENDING_FILENAME = 'lead_pending.jsonl';
const LOG_FILENAME = '.lead-pending-wake.log';

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

function resolveBaseDir(cliBase) {
    const envDir = process.env.TEAM_MODE_BASE_DIR;
    const candidates = [
        envDir,
        cliBase,
        path.resolve(__dirname, '..', '..'),
        process.cwd(),
    ].filter(Boolean);

    for (const dir of candidates) {
        if (fs.existsSync(path.join(dir, PENDING_FILENAME))) {
            return dir;
        }
    }
    // No file found yet — return the most specific candidate so logs land somewhere sane.
    return candidates[0] || process.cwd();
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

function main() {
    const { cliBase } = parseArgs(process.argv);
    const baseDir = resolveBaseDir(cliBase);
    const pendingPath = path.join(baseDir, PENDING_FILENAME);

    if (!fs.existsSync(pendingPath)) {
        log(baseDir, `no pending file at ${pendingPath}, exit 0`);
        process.exit(0);
    }

    let content;
    try {
        content = fs.readFileSync(pendingPath, 'utf8');
    } catch (e) {
        log(baseDir, `read failed: ${e.message}, exit 1`);
        process.stderr.write(`lead-pending-wake: read failed: ${e.message}\n`);
        process.exit(1);
    }

    const lines = content.split('\n').filter(Boolean);
    if (lines.length === 0) {
        log(baseDir, 'pending file empty, exit 0');
        process.exit(0);
    }

    const formatted = formatEntries(lines);
    // The first word "TEAM-MODE" gives Claude an anchor to recognize this as a
    // team-mode worker event regardless of any surrounding Claude Code wrapper.
    process.stderr.write(
        `TEAM-MODE worker messages have arrived for the lead (you).\n` +
            `Inspect and respond to them. You can also call the inbox_read tool for full structured data.\n\n` +
            `Messages:\n${formatted}\n`,
    );

    // Atomic clear: write an empty file back. A concurrent append between read
    // and write could be lost — acceptable because the Rust writer has a file
    // lock, and the worst-case miss is the next hook trigger picks up the
    // stragglers a few hundred ms later.
    try {
        fs.writeFileSync(pendingPath, '', 'utf8');
    } catch (e) {
        log(baseDir, `clear failed: ${e.message}`);
    }

    log(baseDir, `injected ${lines.length} message(s), exit 2`);
    process.exit(2);
}

try {
    main();
} catch (e) {
    process.stderr.write(`lead-pending-wake fatal: ${e.message}\n`);
    process.exit(1);
}
