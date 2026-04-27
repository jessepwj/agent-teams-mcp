use super::*;

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
    let mapped_lead_session_id =
        team.owner_cc_pid.and_then(|pid| lookup_lead_session_id(state.base_dir(), pid));

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

    let sessions = session_discovery::discover_sessions(Path::new(&cwd));

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
    let effective_session_id = requested_session_id
        .clone()
        .or_else(|| if is_lead { mapped_lead_session_id.clone() } else { None });

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
        Some(base_dir.join(".lead-sessions.json")),                // legacy: under .agent-teams/
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

fn parse_claude_conversation(path: &Path) -> Result<Vec<ConversationItemView>, WebError> {
    let content = fs::read_to_string(path)
        .map_err(|err| WebError::internal(format!("failed to read session file: {err}")))?;
    let mut items = Vec::new();

    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        append_conversation_items(&mut items, index, &value);
    }

    if items.len() > 200 {
        let start = items.len().saturating_sub(200);
        items = items.into_iter().skip(start).collect();
    }

    Ok(items)
}

/// Parse a codex rollout file (`~/.codex/sessions/.../rollout-*.jsonl`) into
/// the same flat `ConversationItemView` stream the front-end already renders
/// for Claude Code. The two formats look very different on disk but share
/// the same downstream concepts (user/assistant text, tool calls, results,
/// reasoning), so we normalize them at this layer.
///
/// What we keep / drop:
///   * `session_meta`, `turn_context`, `compacted` — discarded (routing /
///     bookkeeping events, not visible content).
///   * `event_msg` — discarded entirely. Codex emits `agent_message` deltas
///     for streaming and `user_message` echoes that duplicate `response_item`
///     content. The `response_item` form is always complete, so taking only
///     it gives a clean linear transcript.
///   * `response_item.message` — emitted as user / assistant / system items,
///     joining `input_text` + `output_text` parts of the `content` array.
///   * `response_item.reasoning` — emitted as a `thinking` kind. We surface
///     the joined `summary[].text` if present; the encrypted full content
///     is opaque so we drop it.
///   * `response_item.function_call` / `function_call_output` — tool_use /
///     tool_result, with `name` / `arguments` mapped to the existing
///     `tool_name` / `input` fields.
///
/// 200-item tail-trim mirrors the Claude path so very long sessions render
/// in bounded time.
fn parse_codex_conversation(path: &Path) -> Result<Vec<ConversationItemView>, WebError> {
    let content = fs::read_to_string(path)
        .map_err(|err| WebError::internal(format!("failed to read codex rollout: {err}")))?;
    let mut items = Vec::new();

    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let line_kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if line_kind != "response_item" {
            // session_meta / event_msg / turn_context / compacted: skipped
            // by design — see the function-level doc comment.
            continue;
        }
        let timestamp = json_timestamp(&value);
        let Some(payload) = value.get("payload") else {
            continue;
        };
        append_codex_response_item(&mut items, index, payload, timestamp);
    }

    if items.len() > 200 {
        let start = items.len().saturating_sub(200);
        items = items.into_iter().skip(start).collect();
    }

    Ok(items)
}

fn append_codex_response_item(
    items: &mut Vec<ConversationItemView>,
    line_index: usize,
    payload: &Value,
    timestamp: Option<DateTime<Utc>>,
) {
    let kind = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match kind {
        "message" => append_codex_message(items, line_index, payload, timestamp),
        "reasoning" => {
            // `summary` is an array of `{type:"summary_text"|"thinking", text}`
            // chunks. The full `content` is encrypted (opaque) so summary is
            // the best we get on disk. If summary is empty too we still emit
            // a placeholder so the user sees that reasoning happened.
            let summary_text = payload
                .get("summary")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|part| {
                            part.get("text").and_then(Value::as_str).map(str::to_string)
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                })
                .filter(|s| !s.trim().is_empty());
            let text =
                summary_text.unwrap_or_else(|| "[reasoning, content not stored on disk]".into());
            items.push(conversation_item(
                line_index,
                0,
                "assistant",
                "thinking",
                None,
                Some(trim_conversation_text(&text)),
                timestamp,
            ));
        }
        "function_call" => {
            let tool_name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let call_id = payload
                .get("call_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            // `arguments` is a JSON-encoded string per OpenAI/codex convention.
            // Try to parse it for structured display; fall back to raw string.
            let raw_args = payload
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("");
            let parsed: Option<Value> = if raw_args.is_empty() {
                None
            } else {
                serde_json::from_str(raw_args).ok()
            };
            let display_input = parsed.clone().or_else(|| {
                if raw_args.is_empty() {
                    None
                } else {
                    Some(Value::String(raw_args.to_string()))
                }
            });
            let preview = parsed
                .as_ref()
                .map(compact_json)
                .or_else(|| {
                    if raw_args.is_empty() {
                        None
                    } else {
                        Some(raw_args.to_string())
                    }
                })
                .filter(|s| !s.is_empty());
            items.push(conversation_tool_item(
                line_index,
                0,
                "tool_use",
                Some(tool_name.clone()),
                preview,
                ConversationToolPayload {
                    tool_use_id: call_id,
                    tool_name: Some(tool_name),
                    input: display_input,
                    result: None,
                    is_error: false,
                },
                timestamp,
            ));
        }
        "function_call_output" => {
            let call_id = payload
                .get("call_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            // codex stores tool output under `output` (string) or `result`.
            let raw_output = payload
                .get("output")
                .and_then(Value::as_str)
                .or_else(|| payload.get("result").and_then(Value::as_str));
            let parsed_output = raw_output.and_then(|s| serde_json::from_str::<Value>(s).ok());
            let display_text = raw_output.map(trim_conversation_text);
            let is_error = payload
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            items.push(conversation_tool_item(
                line_index,
                0,
                "tool_result",
                None,
                display_text,
                ConversationToolPayload {
                    tool_use_id: call_id,
                    tool_name: None,
                    input: None,
                    result: parsed_output.or_else(|| raw_output.map(|s| Value::String(s.into()))),
                    is_error,
                },
                timestamp,
            ));
        }
        // ghost_snapshot, custom tool variants etc. — ignore silently to
        // keep forward compat. Adding noise here for unknown shapes only
        // hurts the user.
        _ => {}
    }
}

fn append_codex_message(
    items: &mut Vec<ConversationItemView>,
    line_index: usize,
    payload: &Value,
    timestamp: Option<DateTime<Utc>>,
) {
    let role = payload
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    // Map codex roles onto the kinds the UI already understands.
    //   user      → kind "user"
    //   assistant → kind "assistant"
    //   developer → kind "system" (developer messages are codex's harness
    //               instructions / permissions injection — folding them
    //               under "system" mirrors how they'd appear if Claude
    //               surfaced the same content).
    let (display_role, display_kind) = match role {
        "assistant" => ("assistant", "assistant"),
        "developer" => ("system", "system"),
        _ => ("user", "user"),
    };

    // content is `[{type:"input_text"|"output_text", text:"..."}, ...]`.
    // Concatenate all text parts so one logical message stays as one item.
    let text = payload
        .get("content")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|part| {
                    let part_type = part.get("type").and_then(Value::as_str)?;
                    if matches!(part_type, "input_text" | "output_text" | "summary_text") {
                        part.get("text").and_then(Value::as_str).map(str::to_string)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.trim().is_empty());

    if let Some(text) = text {
        items.push(conversation_item(
            line_index,
            0,
            display_role,
            display_kind,
            None,
            Some(trim_conversation_text(&text)),
            timestamp,
        ));
    }
}

fn append_conversation_items(items: &mut Vec<ConversationItemView>, index: usize, value: &Value) {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let timestamp = json_timestamp(value);

    match kind {
        "user" => append_user_items(items, index, value, timestamp),
        "assistant" => append_assistant_items(items, index, value, timestamp),
        "tool_use" => {
            let title = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let tool_use_id = value
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let input = value.get("input").cloned();
            let text = value
                .get("input")
                .map(compact_json)
                .filter(|text| !text.is_empty());
            items.push(conversation_tool_item(
                index,
                0,
                "tool_use",
                Some(title.clone()),
                text,
                ConversationToolPayload {
                    tool_use_id,
                    tool_name: Some(title),
                    input,
                    result: None,
                    is_error: false,
                },
                timestamp,
            ));
        }
        "result" => {
            let is_error = value
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Some(text) = value.get("result").and_then(Value::as_str) {
                if is_error || !text.trim().is_empty() {
                    items.push(conversation_item_with_payload(
                        ConversationItemBase {
                            line_index: index,
                            block_index: 0,
                            role: if is_error { "error" } else { "assistant" },
                            kind: if is_error { "error" } else { "result" },
                            title: None,
                            text: Some(trim_conversation_text(text)),
                        },
                        Some(ConversationToolPayload {
                            tool_use_id: None,
                            tool_name: None,
                            input: None,
                            result: value.get("result").cloned(),
                            is_error,
                        }),
                        timestamp,
                    ));
                }
            }
        }
        "system" => {
            let text = value
                .get("content")
                .and_then(Value::as_str)
                .or_else(|| value.get("message").and_then(Value::as_str))
                .map(trim_conversation_text);
            if text.is_some() {
                items.push(conversation_item(
                    index, 0, "system", "system", None, text, timestamp,
                ));
            }
        }
        _ => {}
    }
}

fn append_user_items(
    items: &mut Vec<ConversationItemView>,
    index: usize,
    value: &Value,
    timestamp: Option<DateTime<Utc>>,
) {
    let Some(content) = message_content(value) else {
        return;
    };
    append_content_blocks(items, index, "user", content, value, timestamp);
}

fn append_assistant_items(
    items: &mut Vec<ConversationItemView>,
    index: usize,
    value: &Value,
    timestamp: Option<DateTime<Utc>>,
) {
    let Some(content) = message_content(value) else {
        return;
    };
    append_content_blocks(items, index, "assistant", content, value, timestamp);
}

fn append_content_blocks(
    items: &mut Vec<ConversationItemView>,
    index: usize,
    role: &str,
    content: &Value,
    message: &Value,
    timestamp: Option<DateTime<Utc>>,
) {
    match content {
        Value::String(text) => {
            if !text.trim().is_empty() {
                items.push(conversation_item(
                    index,
                    0,
                    role,
                    "text",
                    None,
                    Some(trim_conversation_text(text)),
                    timestamp,
                ));
            }
        }
        Value::Array(blocks) => {
            for (block_index, block) in blocks.iter().enumerate() {
                if let Some(text) = block.as_str() {
                    if !text.trim().is_empty() {
                        items.push(conversation_item(
                            index,
                            block_index,
                            role,
                            "text",
                            None,
                            Some(trim_conversation_text(text)),
                            timestamp,
                        ));
                    }
                    continue;
                }

                let block_type = block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                match block_type {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            if !text.trim().is_empty() {
                                items.push(conversation_item(
                                    index,
                                    block_index,
                                    role,
                                    "text",
                                    None,
                                    Some(trim_conversation_text(text)),
                                    timestamp,
                                ));
                            }
                        }
                    }
                    "thinking" => {
                        let text = block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .or_else(|| block.get("text").and_then(Value::as_str))
                            .map(trim_conversation_text);
                        items.push(conversation_item(
                            index,
                            block_index,
                            "assistant",
                            "thinking",
                            Some("Thinking".into()),
                            text,
                            timestamp,
                        ));
                    }
                    "tool_use" => {
                        let title = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let tool_use_id = block
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToString::to_string);
                        let input = block.get("input").cloned();
                        let text = block
                            .get("input")
                            .map(compact_json)
                            .filter(|text| !text.is_empty());
                        items.push(conversation_tool_item(
                            index,
                            block_index,
                            "tool_use",
                            Some(title.clone()),
                            text,
                            ConversationToolPayload {
                                tool_use_id,
                                tool_name: Some(title),
                                input,
                                result: None,
                                is_error: false,
                            },
                            timestamp,
                        ));
                    }
                    "tool_result" => {
                        let tool_use_id = block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .map(ToString::to_string);
                        let title = tool_use_id
                            .as_ref()
                            .map(|id| format!("Tool result {id}"))
                            .unwrap_or_else(|| "Tool result".into());
                        let is_error = block
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let text = block
                            .get("content")
                            .map(extract_content_text)
                            .filter(|text| !text.is_empty());
                        let result = block
                            .get("content")
                            .cloned()
                            .or_else(|| message.get("toolUseResult").cloned())
                            .or_else(|| message.get("tool_use_result").cloned());
                        items.push(conversation_tool_item(
                            index,
                            block_index,
                            "tool_result",
                            Some(title),
                            text,
                            ConversationToolPayload {
                                tool_use_id,
                                tool_name: None,
                                input: None,
                                result,
                                is_error,
                            },
                            timestamp,
                        ));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn conversation_item(
    line_index: usize,
    block_index: usize,
    role: &str,
    kind: &str,
    title: Option<String>,
    text: Option<String>,
    timestamp: Option<DateTime<Utc>>,
) -> ConversationItemView {
    conversation_item_with_payload(
        ConversationItemBase {
            line_index,
            block_index,
            role,
            kind,
            title,
            text,
        },
        None,
        timestamp,
    )
}

fn conversation_tool_item(
    line_index: usize,
    block_index: usize,
    kind: &str,
    title: Option<String>,
    text: Option<String>,
    tool_payload: ConversationToolPayload,
    timestamp: Option<DateTime<Utc>>,
) -> ConversationItemView {
    conversation_item_with_payload(
        ConversationItemBase {
            line_index,
            block_index,
            role: if tool_payload.is_error {
                "error"
            } else {
                "tool"
            },
            kind,
            title,
            text,
        },
        Some(tool_payload),
        timestamp,
    )
}

struct ConversationItemBase<'a> {
    line_index: usize,
    block_index: usize,
    role: &'a str,
    kind: &'a str,
    title: Option<String>,
    text: Option<String>,
}

struct ConversationToolPayload {
    tool_use_id: Option<String>,
    tool_name: Option<String>,
    input: Option<Value>,
    result: Option<Value>,
    is_error: bool,
}

fn conversation_item_with_payload(
    base: ConversationItemBase<'_>,
    tool_payload: Option<ConversationToolPayload>,
    timestamp: Option<DateTime<Utc>>,
) -> ConversationItemView {
    let tool_payload = tool_payload.unwrap_or(ConversationToolPayload {
        tool_use_id: None,
        tool_name: None,
        input: None,
        result: None,
        is_error: false,
    });
    ConversationItemView {
        id: format!("{}:{}", base.line_index, base.block_index),
        role: base.role.into(),
        kind: base.kind.into(),
        title: base.title,
        text: base.text,
        tool_use_id: tool_payload.tool_use_id,
        tool_name: tool_payload.tool_name,
        input: tool_payload.input,
        result: tool_payload.result,
        is_error: tool_payload.is_error,
        timestamp,
    }
}

fn message_content(value: &Value) -> Option<&Value> {
    value
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| value.get("content"))
}

fn extract_content_text(value: &Value) -> String {
    match value {
        Value::String(text) => trim_conversation_text(text),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => text.clone(),
                Value::Object(_) => item
                    .get("text")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| compact_json(item)),
                _ => compact_json(item),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => compact_json(value),
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|_| value.to_string())
        .chars()
        .take(4000)
        .collect()
}

fn trim_conversation_text(text: &str) -> String {
    let mut trimmed = text.trim().chars().take(6000).collect::<String>();
    if text.trim().chars().count() > 6000 {
        trimmed.push_str("\n...");
    }
    trimmed
}

fn json_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
}
