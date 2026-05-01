use super::items::append_conversation_items;
use super::*;

pub(super) fn parse_claude_conversation(
    path: &Path,
) -> Result<Vec<ConversationItemView>, WebError> {
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
