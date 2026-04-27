use super::*;
use crate::team_mode::domain::MessageKind;
use crate::team_mode::mcp::resources::{room_uri, thread_uri};
use crate::team_mode::service::SendMessageRequest;

impl TeamModeToolset {
    pub(super) fn send_message(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
        let team_name = required_identifier(args, "team")?;
        let text = required_text(args, "text")?;

        // Bug 29: caller identity is injected by `inject_call_context` in
        // the daemon. Defaults to "lead" for the lead's own MCP relay (no
        // env vars set); workers' relays carry the worker's name.
        let caller_member = args
            .get("_caller_member")
            .and_then(|v| v.as_str())
            .unwrap_or("lead")
            .to_string();
        let caller_team = args
            .get("_caller_team")
            .and_then(|v| v.as_str())
            .map(String::from);
        let caller_is_lead = caller_member == LEAD_NAME;

        let _team = self
            .team_service
            .get(&team_name)?
            .ok_or_else(|| Error::TeamNotFound {
                name: team_name.clone(),
            })?;

        // Build the address book FIRST so error messages can show the user
        // what handles are valid even when their text has 0 mentions.
        let members = self.member_service.list_active(&team_name)?;
        let active_workers: Vec<String> = members
            .iter()
            .filter(|r| !matches!(r.profile.kind, MemberKind::Lead))
            .map(|r| r.profile.name.clone())
            .collect();
        let lc_index: HashMap<String, String> = active_workers
            .iter()
            .map(|n| (n.to_lowercase(), n.clone()))
            // Lead is always reachable via @lead — make it part of the
            // index so workers can address it case-insensitively.
            .chain(std::iter::once(("lead".into(), LEAD_NAME.into())))
            .collect();

        // Per-team scoping for worker callers: a worker bound to team A
        // cannot send into team B (would be a privilege escalation since
        // they could spam other teams' inboxes).
        if !caller_is_lead {
            match &caller_team {
                Some(t) if t == &team_name => { /* ok */ }
                Some(other) => {
                    return Err(Error::Other(format!(
                        "send_message: caller '{caller_member}' is bound to team \
                         '{other}' and may not send into team '{team_name}'."
                    )));
                }
                None => {
                    return Err(Error::Other(format!(
                        "send_message: caller '{caller_member}' has no team binding \
                         (TEAM_MODE_TEAM env missing). Refusing to send into team \
                         '{team_name}'."
                    )));
                }
            }
            // Workers must actually exist as active members of this team.
            if !active_workers.iter().any(|n| n == &caller_member) {
                let mut available = available_handles(&active_workers);
                available.sort();
                return Err(Error::Other(format!(
                    "send_message: caller '{caller_member}' is not an active member \
                     of team '{team_name}'. Active members (besides lead): {available:?}."
                )));
            }
        }

        // @mention parsing — empty body defaults to @lead (worker's
        // natural target when reporting back). Lead sending with no
        // @mention is an error: the lead has no implicit recipient.
        let body_handles = extract_handles(&text);
        let resolved_handles: Vec<String> = if body_handles.is_empty() {
            if caller_is_lead {
                let mut available = available_handles(&active_workers);
                available.sort();
                return Err(Error::Other(format!(
                    "send_message: text must contain at least one @handle. \
                     Active recipients in team '{team_name}': {available:?}."
                )));
            } else {
                // Worker default → @lead. Auto-prefix the body so the
                // routing path stays uniform (mentions parsed from body).
                vec!["lead".to_string()]
            }
        } else {
            body_handles
        };

        // Resolve @handles case-insensitively. Each user-visible handle
        // becomes the on-disk member name when matched, or stays as the
        // raw handle for the error message. Self-mention is rejected:
        // sending a message addressed to yourself is almost certainly a
        // typo and would otherwise create a no-op.
        let mut resolved: Vec<String> = Vec::new();
        let mut unmatched: Vec<String> = Vec::new();
        let mut self_mentioned = false;
        for h in &resolved_handles {
            match lc_index.get(&h.to_lowercase()) {
                Some(canonical) => {
                    if canonical == &caller_member {
                        self_mentioned = true;
                        continue;
                    }
                    if !resolved.iter().any(|r| r == canonical) {
                        resolved.push(canonical.clone());
                    }
                }
                None => {
                    if !unmatched.iter().any(|u| u == h) {
                        unmatched.push(h.clone());
                    }
                }
            }
        }
        if !unmatched.is_empty() {
            let mut available = available_handles(&active_workers);
            available.sort();
            return Err(Error::Other(format!(
                "send_message: unmatched @mentions {unmatched:?}. \
                 Active recipients in team '{team_name}': {available:?}. \
                 (Mention matching is case-insensitive; check spelling. \
                 Use @lead to address the team lead.)"
            )));
        }
        if self_mentioned && resolved.is_empty() {
            // For workers, suggesting @lead is helpful (it's the most common
            // recipient). For lead itself, suggesting @lead would be the
            // same self-mention that just got rejected — instead suggest
            // any active worker.
            let suggestion = if caller_is_lead {
                let mut workers: Vec<String> = active_workers
                    .iter()
                    .filter(|n| n.as_str() != LEAD_NAME)
                    .map(|n| format!("@{n}"))
                    .collect();
                workers.sort();
                if workers.is_empty() {
                    "Add a worker via worker_add and @-mention them.".to_string()
                } else {
                    format!("Did you mean one of {workers:?}?")
                }
            } else {
                "Did you mean @lead or another member?".to_string()
            };
            return Err(Error::Other(format!(
                "send_message: cannot send to yourself ('@{caller_member}'). {suggestion}"
            )));
        }

        // Liveness pre-check: any recipient whose process is dead would
        // otherwise sit in their inbox forever (the agent_loop that would
        // post a [SYSTEM] death notice only exists while the worker is
        // alive, so a daemon-restart scenario silently strands the lead).
        // For each dead recipient we synthesize a [SYSTEM] Status reply
        // immediately, route it via lead-observability, and drop the
        // recipient from the dispatch.
        let room_id = "main".to_string();
        self.room_service.ensure_main_room(&team_name)?;

        let mut live_recipients: Vec<String> = Vec::new();
        let mut dead_recipients: Vec<String> = Vec::new();
        for recipient in &resolved {
            // Lead is a virtual member, not a managed worker — it has no
            // spawned process to check. Always treat as alive (the lead's
            // inbox is delivered via the Stop hook push path, no agent_loop
            // required). Without this short-circuit, workers messaging
            // @lead would always hit "all targeted workers are dead [lead]".
            if recipient == LEAD_NAME {
                live_recipients.push(recipient.clone());
                continue;
            }
            let key = spawn_key(&team_name, recipient);
            let alive = self.async_runtime.block_on({
                let orch = Arc::clone(&self.runtime_orchestrator);
                let key = key.clone();
                async move { orch.lock().await.is_alive(&key).await.unwrap_or(false) }
            });
            if alive {
                live_recipients.push(recipient.clone());
            } else {
                dead_recipients.push(recipient.clone());
            }
        }

        // Emit [SYSTEM] notice for each dead recipient up-front so the lead
        // does not wait on the Stop hook shepherd for a reply that will
        // never arrive.
        let mut system_notices: Vec<Value> = Vec::new();
        for dead in &dead_recipients {
            let notice = format!(
                "[SYSTEM] worker '{dead}' is not alive — message not delivered. \
                 Use `worker_add name={dead} on_existing=reuse` to spawn a fresh \
                 process (the worker will lose prior conversation context)."
            );
            let sys_msg = self.message_service.send(SendMessageRequest {
                team_id: team_name.clone(),
                room_id: room_id.clone(),
                sender: dead.clone(),
                kind: MessageKind::Status,
                subject: None,
                body: notice.clone(),
                mentions: Vec::new(),
                visibility: Vec::new(),
                audience_policy: None,
                reply_to: None,
                thread_id: None,
                expires_at: None,
            });
            match sys_msg {
                Ok(m) => system_notices.push(json!({
                    "worker": dead,
                    "message_id": m.id,
                    "text": notice,
                })),
                Err(err) => {
                    tracing::warn!(
                        worker = %dead,
                        error = %err,
                        "failed to write [SYSTEM] dead-worker notice"
                    );
                }
            }
        }

        if live_recipients.is_empty() {
            // All targeted workers were dead. Don't write a no-op dispatch
            // — return a structured error listing the dead names so the
            // tool caller sees the failure without scrolling through the
            // [SYSTEM] reply chain.
            return Err(Error::Other(format!(
                "send_message: all targeted workers are dead {:?} in team '{}'. \
                 [SYSTEM] notices have been posted to the lead inbox. \
                 Restart with `worker_add on_existing=reuse` before retrying.",
                dead_recipients, team_name
            )));
        }

        // Rewrite the body so the dispatch only carries live mentions. The
        // worker text routing already filters, but pruning here keeps the
        // visible body clean. We replace each dead @handle with [worker
        // unavailable: name] inline.
        let mut filtered_body = text.clone();
        for dead in &dead_recipients {
            // Try canonical case first, then the raw handle as it appeared.
            let pat = format!("@{dead}");
            filtered_body = filtered_body.replace(&pat, &format!("[worker unavailable: {dead}]"));
        }
        // Mentions for live recipients are already in `filtered_body`. The
        // message_service will re-parse and route to live ones only.

        // If we defaulted to @lead because the worker omitted any handle,
        // splice "@lead " in front of the body so downstream consumers see
        // the routing the same way explicit @mentions are surfaced (and
        // the web UI / inbox view shows the recipient).
        let final_body = if !filtered_body.contains("@lead")
            && live_recipients.iter().any(|r| r == LEAD_NAME)
        {
            format!("@lead {filtered_body}")
        } else {
            filtered_body
        };

        // Sender = caller. Lead callers produce a Dispatch (canonical
        // command from the control plane); worker callers produce a
        // Reply (response in the conversation).
        let kind = if caller_is_lead {
            MessageKind::Dispatch
        } else {
            MessageKind::Reply
        };

        let message = self.message_service.send(SendMessageRequest {
            team_id: team_name.clone(),
            room_id: room_id.clone(),
            sender: caller_member.clone(),
            kind,
            subject: None,
            body: final_body,
            mentions: live_recipients.clone(),
            visibility: Vec::new(),
            audience_policy: None,
            reply_to: None,
            thread_id: None,
            expires_at: None,
        })?;

        let mut updated = vec![
            team_uri(&team_name),
            room_uri(&team_name, &room_id),
            thread_uri(&team_name, message.thread_id.as_deref().unwrap_or("")),
        ];
        updated.extend(
            message
                .effective_recipients
                .iter()
                .map(|recipient| inbox_uri(&team_name, recipient)),
        );

        let matched_recipients: Vec<Value> = message
            .effective_recipients
            .iter()
            .cloned()
            .map(Value::String)
            .collect();

        let mut payload = json!({
            "message": message,
            "matched_recipients": matched_recipients,
        });
        if let Value::Object(map) = &mut payload {
            // Always reinforce the "no polling" rule on every successful
            // send. The whole reason for putting it here (vs the static
            // tool description) is that this is the moment the model is
            // most tempted to follow up with a sleep / inbox_read loop.
            map.insert(
                "hint".into(),
                Value::String(
                    "Replies will arrive automatically as a <system-reminder> \
                     when your next turn starts. Do NOT call inbox_read or \
                     sleep — just end your turn and continue when reminded. \
                     If reminders never arrive (worker IS replying — check \
                     `.lead-pending-wake.log` for new entries), the Stop hook \
                     in `.claude/settings.json` is not loaded. Fully restart \
                     Claude Code (NOT just `/mcp reconnect` — hooks only load \
                     at CC startup). After any change to `.mcp.json` or \
                     `.claude/settings.json`, a full CC restart is required."
                        .into(),
                ),
            );
            if !dead_recipients.is_empty() {
                map.insert(
                    "dead_recipients".into(),
                    Value::Array(
                        dead_recipients
                            .iter()
                            .cloned()
                            .map(Value::String)
                            .collect::<Vec<_>>(),
                    ),
                );
                map.insert("system_notices".into(), Value::Array(system_notices));
                let dead_names = dead_recipients.join(", ");
                map.insert(
                    "dead_recipients_hint".into(),
                    Value::String(format!(
                        "Workers [{dead_names}] were skipped because their process is gone. \
                         Revive each with `worker_add name=<x> on_existing=reuse` (the worker \
                         loses prior conversation context) before retrying."
                    )),
                );
            }
        }

        Ok(success_with_updates(payload, updated))
    }

    pub(super) fn inbox_read(&self, args: &Map<String, Value>) -> Result<ToolExecution> {
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

/// Build a sorted list of `@handle` strings the caller can validly mention,
/// always including `@lead` first. Used for human-readable error messages
/// when a caller's text references unknown handles or omits all mentions.
fn available_handles(active_workers: &[String]) -> Vec<String> {
    let mut out: Vec<String> = std::iter::once(format!("@{}", LEAD_NAME))
        .chain(active_workers.iter().map(|n| format!("@{n}")))
        .collect();
    // Lead first; everything else alphabetical.
    out[1..].sort();
    out
}
