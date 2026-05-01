use super::*;

pub(super) fn append_conversation_items(
    items: &mut Vec<ConversationItemView>,
    index: usize,
    value: &Value,
) {
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

pub(super) fn conversation_item(
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

pub(super) fn conversation_tool_item(
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

pub(super) struct ConversationToolPayload {
    pub(super) tool_use_id: Option<String>,
    pub(super) tool_name: Option<String>,
    pub(super) input: Option<Value>,
    pub(super) result: Option<Value>,
    pub(super) is_error: bool,
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

pub(super) fn compact_json(value: &Value) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|_| value.to_string())
        .chars()
        .take(4000)
        .collect()
}

pub(super) fn trim_conversation_text(text: &str) -> String {
    let mut trimmed = text.trim().chars().take(6000).collect::<String>();
    if text.trim().chars().count() > 6000 {
        trimmed.push_str("\n...");
    }
    trimmed
}

pub(super) fn json_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
}
