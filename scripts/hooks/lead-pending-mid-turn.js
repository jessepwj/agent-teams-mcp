#!/usr/bin/env node
/**
 * lead-pending-mid-turn.js — PostToolUse hook for mid-turn worker-reply
 *                            delivery to the lead.
 *
 * Why this exists:
 *   The Stop hook (lead-pending-wake.js) only fires at turn end. If the
 *   lead is in a long turn (10+ tool calls), worker replies that arrive
 *   mid-turn must wait for the entire turn to complete before being
 *   surfaced. PostToolUse fires after every tool call and can return
 *   `additionalContext` that gets injected into the LLM's live context
 *   without ending the turn — so any pending worker replies can land
 *   in the model's view within ~50ms of the next tool boundary.
 *
 * Coordination with the Stop hook:
 *   Both hooks share the same `lead_pending.jsonl` file and exclusive
 *   lock. Whichever hook fires first acquires the lock, drains the
 *   entries that belong to its CC ancestor chain, and writes peers
 *   back. The other hook then sees an empty `mine` set and exits
 *   cheaply. No dedup tracker is needed — pending IS the source of
 *   truth for "still undelivered". Once an entry is drained, it's
 *   gone.
 *
 *   Practical interleaving:
 *     - Long lead turn with tool calls: PostToolUse drains as replies
 *       arrive; Stop sees empty pending and just waits for new traffic.
 *     - Short turn with no tool calls: PostToolUse never fires; Stop
 *       drains everything at turn end (current behavior unchanged).
 *     - Race (worker reply lands the instant turn ends): the file lock
 *       serializes; first hook wins. The other re-checks and exits.
 *
 * LLM semantic confusion avoidance:
 *   The injected text uses distinct visual banners ("─── ─── ───") so
 *   the LLM doesn't confuse it with tool output. Framing explicitly
 *   says "you can continue your task; turn end will re-surface if
 *   unaddressed" — this nudges the LLM to NOT abandon its current
 *   reasoning chain to respond. For unsolicited worker pings the LLM
 *   may still choose to respond mid-turn if the message is urgent.
 *
 * Output contract (CC PostToolUse hook):
 *   stdout JSON {
 *     hookSpecificOutput: {
 *       hookEventName: "PostToolUse",
 *       additionalContext: "<reminder text>"
 *     }
 *   }
 *   exit 0
 *
 *   When nothing to inject: write nothing, exit 0. Cheap path —
 *   should complete in <100ms when pending is empty.
 *
 * Loop safety:
 *   PostToolUse fires for EVERY tool call including the LLM acting on
 *   the additionalContext we just injected (e.g. lead calls
 *   `send_message` to reply to the worker). At that point pending is
 *   already drained, so the next PostToolUse hook fires with no
 *   `mine` and exits without injecting anything. No risk of
 *   self-amplifying loop.
 *
 * Worker fast-path:
 *   When this script is invoked from a team-mode WORKER's claude CLI
 *   subprocess (TEAM_MODE_WORKER=1), exit immediately. Workers don't
 *   need lead-pending notifications — those are addressed to lead.
 */

'use strict';

const fs = require('fs');
const path = require('path');
const shared = require('./lead-pending-shared.js');

const LOG_FILENAME = '.lead-pending-wake.log'; // shared log with the Stop hook for audit ease

async function main() {
    // Workers don't get notifications.
    if (process.env.TEAM_MODE_WORKER === '1') {
        process.exit(0);
    }

    const { cliBase } = shared.parseArgs(process.argv);
    const event = shared.readHookEvent();
    const baseDir = shared.resolveBaseDir(cliBase, event, __dirname);
    const pendingPath = path.join(baseDir, shared.PENDING_FILENAME);
    const log = shared.makeLogger(baseDir, LOG_FILENAME);

    // FAST PATH 1 — pending file doesn't exist (no team / nothing to deliver).
    if (!fs.existsSync(pendingPath)) {
        process.exit(0);
    }

    // FAST PATH 2 — file exists but is empty.
    let st;
    try {
        st = fs.statSync(pendingPath);
    } catch (_) {
        process.exit(0);
    }
    if (st.size === 0) {
        process.exit(0);
    }

    // Acquire the shared lock with a short budget. PostToolUse runs in the
    // hot path of every tool call, so we can't tolerate long contention —
    // if the Stop hook (or another concurrent PostToolUse) is holding the
    // lock, just exit fast. Pending is durable; we'll catch the entries
    // at the next tool call.
    const lockPath = await shared.acquirePendingLock(baseDir);
    if (!lockPath) {
        log('mid-turn: lock contention, skip this fire');
        process.exit(0);
    }

    try {
        const ancestorSet = shared.getAncestorPidSet(baseDir);
        const c = shared.classifyPending(pendingPath, ancestorSet);
        if (c.mine.length === 0) {
            // Either nothing for us or everything belongs to peer CCs —
            // exit cheaply without disturbing pending.
            process.exit(0);
        }

        // Drain `mine` by writing only peers back. This is the same
        // contract as Stop.tryBlock — once delivered, the entries leave
        // the pending file. Stop hook will see empty pending if no new
        // traffic arrives before turn end.
        shared.writePeersBack(pendingPath, c.othersRaw, log);

        const reminder = shared.renderMidTurnReminder(c.mine);
        const routingNote = ancestorSet === null
            ? 'NO-ROUTING'
            : `ancestors=${[...ancestorSet].slice(0, 5).join(',')}${ancestorSet.size > 5 ? '...' : ''}`;
        log(`mid-turn: injected ${c.mine.length}, kept ${c.othersRaw.length} for peers [${routingNote}]`);

        process.stdout.write(JSON.stringify({
            hookSpecificOutput: {
                hookEventName: 'PostToolUse',
                additionalContext: reminder,
            },
        }));
        process.exit(0);
    } finally {
        shared.releasePendingLock(lockPath);
    }
}

main().catch((e) => {
    // Don't crash the hook chain — log and exit 0. PostToolUse failures
    // would otherwise bubble up and break the lead's tool flow, which
    // is far worse than missing one inject (Stop will still pick the
    // entries up at turn end).
    process.stderr.write(`lead-pending-mid-turn fatal: ${e && e.message ? e.message : e}\n`);
    process.exit(0);
});
