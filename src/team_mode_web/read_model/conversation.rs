use super::*;

mod claude;
mod codex;
mod items;

use claude::parse_claude_conversation;
use codex::parse_codex_conversation;

pub fn read_member_conversation(
    state: &TeamModeWebState,
    team_id: &str,
    name: &str,
) -> Result<MemberConversationResponse, WebError> {
    let team = state
        .team_service
        .get(team_id)?
        .ok_or_else(|| WebError::not_found(format!("team '{team_id}' not found")))?;
    let member = state.member_service.get(team_id, name)?.ok_or_else(|| {
        WebError::not_found(format!("member '{name}' not found in team '{team_id}'"))
    })?;

    // Bug 27 fix: in multi-CC-same-cwd setups, picking the lead's JSONL by
    // mtime gives the wrong CC's transcript. The Stop hook writes a
    // {pid → claude_session_id} map on every fire; if `team.owner_cc_pid`
    // appears in that map we have an exact session id to match. None
    // means the file is missing (hook never fired) or this team's CC has
    // no recorded session — we fall back to the historical mtime path.
    let mapped_lead_session_id = team
        .owner_cc_pid
        .and_then(|pid| lookup_lead_session_id(state.base_dir(), pid));

    let execution = member.execution.as_ref();
    let cwd = execution
        .and_then(|profile| profile.cwd.clone())
        .or_else(|| team.cwd.clone())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|path| path.display().to_string())
        });
    let provider = execution
        .and_then(|profile| profile.adapter.clone())
        .unwrap_or_else(|| "claude-code".into());

    let Some(cwd) = cwd else {
        return Ok(empty_conversation(
            name,
            provider,
            "no_cwd",
            None,
            vec!["No cwd is available for this member or team.".into()],
        ));
    };

    let requested_session_id = execution.and_then(|profile| profile.session_id.clone());
    let is_lead = matches!(member.profile.kind, MemberKind::Lead);

    // Provider routing.
    //
    // Claude Code: per-project JSONL tree under `~/.claude/projects/<encoded-cwd>/`,
    //   precise lookup by stored session_id, mtime fallback for the lead only.
    // Codex: rollouts under `~/.codex/sessions/YYYY/MM/DD/`, indexed by the
    //   thread-id we persist via `CodexSession::session_id()`. cwd-scoped
    //   discovery scans + filters by `session_meta.cwd`.
    // Unknown providers (gemini-cli today, future ones): keep the original
    //   `unsupported_provider` placeholder so the UI shows an honest "we
    //   don't know how to read this" rather than a misleading empty page.
    if provider == "codex" {
        return read_codex_conversation(
            name,
            provider,
            cwd,
            requested_session_id.as_deref(),
            is_lead,
        );
    }
    if provider != "claude-code" {
        return Ok(empty_conversation(
            name,
            provider,
            "unsupported_provider",
            Some(cwd),
            vec![
                "Conversation rendering currently supports Claude Code and Codex sessions.".into(),
            ],
        ));
    }

    let sessions = match state.session_home() {
        Some(home) => session_discovery::discover_sessions_with_home(home, Path::new(&cwd)),
        None => session_discovery::discover_sessions(Path::new(&cwd)),
    };

    // For workers (not the lead), refuse the "latest mtime in cwd" fallback.
    // In every realistic setup the lead's own CC instance writes JSONL into
    // the same cwd, and CC writes far more often than any worker — so the
    // mtime-first match would silently surface the lead/CC's transcript as
    // the worker's "session". The user reads the worker's right pane and
    // sees the lead's tool calls, with no signal that the wrong file was
    // chosen.
    //
    // The correct behaviour for a worker that hasn't yet been observed:
    // return an empty placeholder. session_id gets persisted by
    // `agent_loop` after the FIRST `type:result` event, so as soon as the
    // worker handles one inbox message the conversation pane fills in.
    //
    // The lead is exempt — for the lead, "the latest JSONL in cwd" IS the
    // correct match (the lead IS the CC instance writing those events).
    // Effective session id for matching, in order of preference:
    //   1. The member's own persisted execution.session_id (always
    //      authoritative for workers; for lead it's None today).
    //   2. The Stop hook's {owner_cc_pid → session_id} map (Bug 27 fix —
    //      lead only; disambiguates multi-CC-same-cwd).
    //   3. None → mtime-first fallback for lead, empty for worker.
    let effective_session_id = requested_session_id.clone().or_else(|| {
        if is_lead {
            mapped_lead_session_id.clone()
        } else {
            None
        }
    });

    let selected_session = match (effective_session_id.as_ref(), is_lead) {
        (Some(session_id), _) => sessions
            .iter()
            .find(|session| &session.session_id == session_id)
            // For the lead, falling back to mtime-first is fine — the
            // lead IS the CC writing in this cwd. For workers we already
            // refuse the fallback above (worker without exact match shows
            // empty), so this branch only applies when we matched.
            .or_else(|| if is_lead { sessions.first() } else { None }),
        (None, true) => sessions.first(),
        (None, false) => None,
    };
    let Some(session) = selected_session else {
        let mut limitations = Vec::new();
        let confidence = if !is_lead && requested_session_id.is_none() {
            limitations.push(
                "Worker has no captured backend session_id yet. \
                 The session is recorded after the first `type:result` event \
                 (i.e. once the worker has answered at least one message). \
                 Refresh after sending the worker its first message."
                    .into(),
            );
            "no_session_yet"
        } else {
            limitations
                .push("No Claude Code session JSONL file was found for this member cwd.".into());
            limitations.push("The lookup is scoped to the member cwd first, then team cwd.".into());
            "no_session_file"
        };
        return Ok(empty_conversation(
            name,
            provider,
            confidence,
            Some(cwd),
            limitations,
        ));
    };

    let items = parse_claude_conversation(&session.path)?;
    let exact_match = effective_session_id
        .as_ref()
        .map(|session_id| session_id == &session.session_id)
        .unwrap_or(false);
    // Bug 27 fix telemetry: a "hook-mapped" match is exact-confidence but
    // came from `.lead-sessions.json` rather than the member's own
    // execution profile. Surface the distinction in `limitations` so the
    // UI / debugger knows where the disambiguation came from.
    let matched_via_hook_map = exact_match
        && requested_session_id.is_none()
        && mapped_lead_session_id.as_ref() == Some(&session.session_id);
    let mut limitations = Vec::new();
    if matched_via_hook_map {
        limitations.push(
            "The session is matched by the Stop hook's {owner_cc_pid → claude_session_id} map \
             (handles multi-CC-same-cwd disambiguation)."
                .into(),
        );
    } else if exact_match {
        limitations
            .push("The session is matched by the member's persisted backend session id.".into());
    } else {
        limitations.push(
            "The session is matched by cwd and latest modified Claude Code JSONL file.".into(),
        );
        limitations.push("Per-member exact session id is not available or was not found, so concurrent members sharing one cwd can be ambiguous.".into());
    }
    Ok(MemberConversationResponse {
        member: name.to_string(),
        source: ConversationSourceView {
            provider,
            confidence: if exact_match {
                "session_id".into()
            } else {
                "cwd_latest".into()
            },
            session_id: Some(session.session_id.clone()),
            path: Some(session.path.display().to_string()),
            updated_at: session.modified,
            cwd: Some(cwd),
        },
        items,
        limitations,
    })
}

/// Render a codex worker's transcript.
///
/// Codex rollouts live at `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`.
/// We resolve the file by the worker's persisted backend session id (which is
/// codex's `thread/start` UUID, surfaced via `CodexSession::session_id`) when
/// available. For workers that haven't completed their first turn yet, we
/// return the same `no_session_yet` placeholder used by the Claude path so
/// the UI shows a consistent "wait for first reply" message instead of
/// silently surfacing some unrelated session.
///
/// The lead-mtime fallback only kicks in when the lead happens to be running
/// codex (rare in practice — leads are normally Claude Code). It picks the
/// newest session under the lead's cwd.
fn read_codex_conversation(
    name: &str,
    provider: String,
    cwd: String,
    requested_session_id: Option<&str>,
    is_lead: bool,
) -> Result<MemberConversationResponse, WebError> {
    let selected = match requested_session_id {
        Some(sid) => codex_session_discovery::find_session_by_id(sid),
        None if is_lead => codex_session_discovery::discover_sessions_for_cwd(&cwd)
            .into_iter()
            .next(),
        None => None,
    };

    let Some(session) = selected else {
        let mut limitations = Vec::new();
        let confidence = if !is_lead && requested_session_id.is_none() {
            limitations.push(
                "Worker has no captured codex session id yet. The id is recorded \
                 after the worker spawns its codex thread; it shows up here on \
                 the next refresh after the first message is processed."
                    .into(),
            );
            "no_session_yet"
        } else {
            limitations.push(
                "No codex rollout file was found for this member. The rollout \
                 lives under ~/.codex/sessions/YYYY/MM/DD/. Either codex hasn't \
                 written one yet, or the session id we stored doesn't match any \
                 file (e.g. the rollout was hand-deleted)."
                    .into(),
            );
            "no_session_file"
        };
        return Ok(empty_conversation(
            name,
            provider,
            confidence,
            Some(cwd),
            limitations,
        ));
    };

    let items = parse_codex_conversation(&session.path)?;
    let exact_match = requested_session_id == Some(session.session_id.as_str());
    let mut limitations = Vec::new();
    if exact_match {
        limitations
            .push("Matched by the codex thread id persisted from the backend session.".into());
    } else {
        limitations.push(
            "Matched by the most recent codex rollout under this cwd. \
             Concurrent codex sessions in the same cwd can be ambiguous."
                .into(),
        );
    }
    Ok(MemberConversationResponse {
        member: name.to_string(),
        source: ConversationSourceView {
            provider,
            confidence: if exact_match {
                "session_id".into()
            } else {
                "cwd_latest".into()
            },
            session_id: Some(session.session_id.clone()),
            path: Some(session.path.display().to_string()),
            updated_at: session.modified,
            cwd: Some(cwd),
        },
        items,
        limitations,
    })
}

fn empty_conversation(
    member: &str,
    provider: String,
    confidence: &str,
    cwd: Option<String>,
    limitations: Vec<String>,
) -> MemberConversationResponse {
    MemberConversationResponse {
        member: member.to_string(),
        source: ConversationSourceView {
            provider,
            confidence: confidence.into(),
            session_id: None,
            path: None,
            updated_at: None,
            cwd,
        },
        items: Vec::new(),
        limitations,
    }
}

/// Bug 27 fix — read the Stop hook's {pid → claude_session_id} side
/// channel to disambiguate which Claude Code instance owns a team in
/// multi-CC-same-cwd setups.
///
/// The hook script writes `.lead-sessions.json` to the **project root**
/// (alongside `lead_pending.jsonl`), NOT under `<base>/.agent-teams/`.
/// This is consistent with all other hook-side sidecars
/// (`lead_pending.jsonl`, `.lead-pending.lock`, `.ancestor-cache.json`,
/// `.stop-hook-cooldown`) which all land at the project root because
/// `lead_pending.jsonl`'s position there is fixed (FileChanged hook
/// matcher takes only literal filenames, no subpath glob).
///
/// `state.base_dir()` returns `<project>/.agent-teams/`, so we look one
/// level up. We try both locations to stay forward-compatible if the
/// hook ever migrates the sidecar into `.agent-teams/`.
///
/// Returns None if the file is missing (hook never fired since the team
/// was created), corrupt, or doesn't contain an entry for `pid`. Caller
/// falls back to the historical mtime-based lookup in that case.
fn lookup_lead_session_id(base_dir: &Path, pid: u32) -> Option<String> {
    let candidates = [
        base_dir.parent().map(|p| p.join(".lead-sessions.json")), // project root
        Some(base_dir.join(".lead-sessions.json")),               // legacy: under .agent-teams/
    ];
    for candidate in candidates.into_iter().flatten() {
        let Ok(content) = fs::read_to_string(&candidate) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        if let Some(entry) = parsed.get(pid.to_string()) {
            if let Some(s) = entry.get("session_id").and_then(|v| v.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    None
}
