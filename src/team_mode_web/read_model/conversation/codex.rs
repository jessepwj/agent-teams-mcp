use super::items::{
    ConversationToolPayload, compact_json, conversation_item, conversation_tool_item,
    json_timestamp, trim_conversation_text,
};
use super::*;

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
pub(super) fn parse_codex_conversation(path: &Path) -> Result<Vec<ConversationItemView>, WebError> {
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
