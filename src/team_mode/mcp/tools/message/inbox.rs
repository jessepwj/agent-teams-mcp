use super::super::*;

impl TeamModeToolset {
    pub(in crate::team_mode::mcp::tools) fn inbox_read(
        &self,
        args: &Map<String, Value>,
    ) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let limit = optional_usize(args, "limit")?.unwrap_or(20).clamp(1, 100);
        let unread_only = optional_bool(args, "unread_only")?.unwrap_or(true);
        let auto_ack = optional_bool(args, "auto_ack")?.unwrap_or(false);

        self.team_service
            .get(&team_name)?
            .ok_or_else(|| Error::TeamNotFound {
                name: team_name.clone(),
            })?;

        let inbox = self.inbox_service.peek(&team_name, LEAD_NAME, None)?;

        let mut items: Vec<_> = inbox
            .items
            .into_iter()
            .filter(|item| {
                if !unread_only {
                    return true;
                }
                !matches!(item.status, crate::team_mode::domain::InboxStatus::Acked)
            })
            .collect();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        items.truncate(limit);

        let mut messages_out: Vec<Value> = Vec::with_capacity(items.len());
        let mut ack_ids: Vec<String> = Vec::with_capacity(items.len());
        for item in items {
            let message = match self.message_store.get(&team_name, &item.message_id) {
                Ok(Some(m)) => m,
                _ => continue,
            };
            messages_out.push(json!({
                "id": message.id,
                "from": message.sender,
                "kind": kind_to_str(&message.kind),
                "text": message.body,
                "reply_to": message.reply_to,
                "thread_id": message.thread_id,
                "status": status_to_str(&item.status),
                "created_at": message.created_at,
            }));
            ack_ids.push(message.id);
        }

        if auto_ack && !ack_ids.is_empty() {
            let _ = self.inbox_service.read(&team_name, LEAD_NAME, &ack_ids);
            let _ = self.inbox_service.ack(&team_name, LEAD_NAME, &ack_ids);
        }

        let unread_count = self
            .inbox_service
            .count(&team_name, LEAD_NAME, None)
            .map(|c| c.unread)
            .unwrap_or(0);

        let mut payload = json!({
            "team": team_name,
            "lead": LEAD_NAME,
            "unread_count": unread_count,
            "total_returned": messages_out.len(),
            "messages": messages_out,
        });
        // Inbox is a fallback channel — when it returns nothing, surface a
        // hint so the model doesn't fall into a poll loop. The Stop hook
        // delivers replies as `<system-reminder>` automatically; calling
        // this tool without backlog-checking intent is wasted work.
        if messages_out.is_empty() {
            if let Value::Object(map) = &mut payload {
                map.insert(
                    "hint".into(),
                    Value::String(
                        "No messages in inbox. Worker replies arrive automatically \
                         via the Stop hook on your next turn — calling inbox_read \
                         is rarely needed; only useful for explicit backlog audits."
                            .into(),
                    ),
                );
            }
        }
        Ok(success(payload))
    }
}
