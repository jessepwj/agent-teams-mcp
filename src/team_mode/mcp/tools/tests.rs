use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tempfile::tempdir;
use tokio::sync::mpsc;

use super::*;
use crate::backend::{AgentBackend, AgentOutput, AgentSession, BackendType, SpawnConfig};
use crate::team_mode::tracing_capture::capture_events;
use crate::team_mode_web::{TeamModeWebState, read_model::read_events};

const TEST_OWNER_CC_PID: u32 = u32::MAX;

fn create_demo_team_for_tool_test(tools: &TeamModeToolset) {
    tools
        .call_tool(
            "team_create",
            Some(json!({
                "name": "demo",
                "_owner_cc_pid": TEST_OWNER_CC_PID
            })),
        )
        .unwrap();
}

#[test]
fn list_tools_exposes_minimal_surface() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path()).list_tools();
    let names: Vec<_> = tools.into_iter().map(|t| t.name).collect();

    let expected = [
        "team_create",
        "team_list",
        "team_delete",
        "worker_add",
        "worker_list",
        "worker_remove",
        "send_message",
        "inbox_read",
    ];
    for name in &expected {
        assert!(names.iter().any(|n| n == name), "missing tool '{name}'");
    }
    assert_eq!(names.len(), expected.len(), "unexpected tools: {names:?}");
}

#[test]
fn team_create_auto_creates_lead_member() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    create_demo_team_for_tool_test(&tools);

    let team_list = tools.call_tool("team_list", Some(json!({}))).unwrap();
    let v = team_list.result.structured_content.unwrap();
    let teams = v["teams"].as_array().unwrap();
    let team = teams.iter().find(|t| t["name"] == "demo").unwrap();
    assert_eq!(team["leadMemberId"].as_str().unwrap(), "lead");
}

#[test]
fn team_create_rebinds_existing_active_team_owner() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());

    let first = tools
        .call_tool(
            "team_create",
            Some(json!({
                "name": "demo",
                "_owner_cc_pid": TEST_OWNER_CC_PID
            })),
        )
        .unwrap()
        .result
        .structured_content
        .unwrap();
    let created_at = first["createdAt"].clone();

    let rebound = tools
        .call_tool(
            "team_create",
            Some(json!({
                "name": "demo",
                "_owner_cc_pid": 42
            })),
        )
        .unwrap()
        .result
        .structured_content
        .unwrap();

    assert_eq!(rebound["ownerCcPid"], json!(42));
    assert_eq!(rebound["createdAt"], created_at);
    assert!(rebound.get("cleaned_orphan_teams").is_none());

    let same_owner = tools
        .call_tool(
            "team_create",
            Some(json!({
                "name": "demo",
                "_owner_cc_pid": 42
            })),
        )
        .unwrap()
        .result
        .structured_content
        .unwrap();

    assert_eq!(same_owner["ownerCcPid"], json!(42));
    assert_eq!(same_owner["updatedAt"], rebound["updatedAt"]);
}

#[test]
fn lead_watchdog_auto_archives_dead_owner_after_grace() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());

    tools
        .call_tool(
            "team_create",
            Some(json!({
                "name": "demo",
                "_owner_cc_pid": TEST_OWNER_CC_PID
            })),
        )
        .unwrap();

    let mut strikes = HashMap::new();
    assert_eq!(tools.lead_watchdog_tick(&mut strikes), 0);
    assert_eq!(tools.lead_watchdog_tick(&mut strikes), 0);
    assert_eq!(tools.lead_watchdog_tick(&mut strikes), 1);

    let team_list = tools.call_tool("team_list", Some(json!({}))).unwrap();
    let v = team_list.result.structured_content.unwrap();
    let teams = v["teams"].as_array().unwrap();
    let team = teams.iter().find(|t| t["name"] == "demo").unwrap();
    assert_eq!(team["status"], json!("archived"));
}

#[test]
fn project_root_context_isolates_team_data() {
    let service_dir = tempdir().unwrap();
    let project_a = tempdir().unwrap();
    let project_b = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(service_dir.path());

    for project in [project_a.path(), project_b.path()] {
        tools
            .call_tool(
                "team_create",
                Some(json!({
                    "name": "demo",
                    "_owner_cc_pid": TEST_OWNER_CC_PID,
                    "_project_root": project.display().to_string(),
                })),
            )
            .unwrap();
    }

    let list_a = tools
        .call_tool(
            "team_list",
            Some(json!({
                "_project_root": project_a.path().display().to_string(),
            })),
        )
        .unwrap()
        .result
        .structured_content
        .unwrap();
    let list_b = tools
        .call_tool(
            "team_list",
            Some(json!({
                "_project_root": project_b.path().display().to_string(),
            })),
        )
        .unwrap()
        .result
        .structured_content
        .unwrap();
    let list_service = tools
        .call_tool("team_list", Some(json!({})))
        .unwrap()
        .result
        .structured_content
        .unwrap();

    assert_eq!(list_a["teams"].as_array().unwrap().len(), 1);
    assert_eq!(list_b["teams"].as_array().unwrap().len(), 1);
    assert!(list_service["teams"].as_array().unwrap().is_empty());
    assert!(project_a.path().join(".agent-teams").join("demo").exists());
    assert!(project_b.path().join(".agent-teams").join("demo").exists());
    assert!(!service_dir.path().join("demo").exists());
}

#[test]
fn send_message_rejects_no_mention() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    create_demo_team_for_tool_test(&tools);

    let err = tools
        .call_tool(
            "send_message",
            Some(json!({
                "team": "demo",
                "text": "no mention here",
            })),
        )
        .unwrap_err();
    assert!(matches!(&err, Error::Other(msg) if msg.contains("@handle")));
}

#[test]
fn send_message_rejects_any_unmatched_mention() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    create_demo_team_for_tool_test(&tools);

    // Even if alice doesn't exist yet, @typo must fail.
    let err = tools
        .call_tool(
            "send_message",
            Some(json!({
                "team": "demo",
                "text": "@typo please",
            })),
        )
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("unmatched"), "got: {msg}");
    assert!(msg.contains("typo"), "got: {msg}");
}

#[test]
fn send_message_unmatched_lists_available_handles_with_lead() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    create_demo_team_for_tool_test(&tools);

    // Bug 29: error must point the model at @lead so a confused
    // caller can fall back to addressing the lead instead of guessing.
    let err = tools
        .call_tool(
            "send_message",
            Some(json!({
                "team": "demo",
                "text": "@nope please",
            })),
        )
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("@lead"),
        "error should suggest @lead as a valid handle, got: {msg}"
    );
}

#[test]
fn send_message_lead_no_mention_lists_available_handles() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    create_demo_team_for_tool_test(&tools);

    // Lead with no @mention: error must include the available handles
    // (so the LLM can self-correct without scrolling for `worker_list`).
    let err = tools
        .call_tool(
            "send_message",
            Some(json!({
                "team": "demo",
                "text": "no mention here",
                "_caller_member": "lead",
            })),
        )
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("@handle"), "got: {msg}");
    assert!(msg.contains("Active recipients"), "got: {msg}");
}

#[test]
fn worker_remove_refuses_lead() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    create_demo_team_for_tool_test(&tools);

    let err = tools
        .call_tool(
            "worker_remove",
            Some(json!({"team": "demo", "name": "lead"})),
        )
        .unwrap_err();
    assert!(matches!(&err, Error::Other(msg) if msg.contains("lead")));
}

#[test]
fn worker_add_refuses_reserved_name() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    create_demo_team_for_tool_test(&tools);

    let err = tools
        .call_tool(
            "worker_add",
            Some(json!({
                "team": "demo",
                "name": "lead",
                "adapter": "claude-code",
            })),
        )
        .unwrap_err();
    assert!(matches!(&err, Error::Other(msg) if msg.contains("reserved")));
}

#[test]
fn worker_list_excludes_the_lead() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    create_demo_team_for_tool_test(&tools);

    let resp = tools
        .call_tool("worker_list", Some(json!({"team": "demo"})))
        .unwrap();
    let v = resp.result.structured_content.unwrap();
    let workers = v["workers"].as_array().unwrap();
    assert!(workers.is_empty());
}

#[test]
fn inbox_read_on_empty_team_returns_empty() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    create_demo_team_for_tool_test(&tools);

    let resp = tools
        .call_tool("inbox_read", Some(json!({"team": "demo"})))
        .unwrap();
    let v = resp.result.structured_content.unwrap();
    assert_eq!(v["team"], "demo");
    assert_eq!(v["lead"], "lead");
    assert_eq!(v["unread_count"], 0);
    assert_eq!(v["total_returned"], 0);
}

#[test]
fn inbox_read_rejects_unknown_team() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    let err = tools
        .call_tool("inbox_read", Some(json!({"team": "no-such"})))
        .unwrap_err();
    assert!(matches!(&err, Error::TeamNotFound { name } if name == "no-such"));
}

struct FakeCodexBackend {
    stderr_tail: String,
    stderr_log_hint: String,
}

#[async_trait]
impl AgentBackend for FakeCodexBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::Codex
    }

    async fn spawn(&self, config: SpawnConfig) -> Result<Box<dyn AgentSession>> {
        Ok(Box::new(FakeCodexSession {
            name: config.name,
            alive: Arc::new(AtomicBool::new(false)),
            stderr_tail: self.stderr_tail.clone(),
            stderr_log_hint: self.stderr_log_hint.clone(),
        }))
    }
}

struct FakeCodexSession {
    name: String,
    alive: Arc<AtomicBool>,
    stderr_tail: String,
    stderr_log_hint: String,
}

#[async_trait]
impl AgentSession for FakeCodexSession {
    fn name(&self) -> &str {
        &self.name
    }

    async fn send_input(&mut self, _input: &str) -> Result<()> {
        Ok(())
    }

    fn output_receiver(&mut self) -> Option<mpsc::Receiver<AgentOutput>> {
        None
    }

    async fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.alive.store(false, Ordering::Relaxed);
        Ok(())
    }

    async fn force_kill(&mut self) -> Result<()> {
        self.alive.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn stderr_tail(&self) -> Option<String> {
        Some(self.stderr_tail.clone())
    }

    fn stderr_log_hint(&self) -> Option<String> {
        Some(self.stderr_log_hint.clone())
    }
}

#[test]
fn worker_liveness_dead_note_keeps_stderr_local_and_redacts_events() {
    let dir = tempdir().unwrap();
    let tools = TeamModeToolset::new_for_test(dir.path());
    create_demo_team_for_tool_test(&tools);

    let stderr_tail = "Authorization: Bearer sk-secret-123\napi_key=abc123\n";
    let stderr_log_path =
        crate::team_mode_daemon::runtime_dir(dir.path()).join("codex-stderr-demo__worker.log");
    let stderr_log_hint = format!("{}:lines 7-9", stderr_log_path.display());

    tools.async_runtime.block_on({
        let orch = Arc::clone(&tools.runtime_orchestrator);
        let stderr_tail = stderr_tail.to_string();
        let stderr_log_hint = stderr_log_hint.clone();
        async move {
            orch.lock().await.register_backend(FakeCodexBackend {
                stderr_tail,
                stderr_log_hint,
            });
        }
    });

    tools
        .call_tool(
            "worker_add",
            Some(json!({
                "team": "demo",
                "name": "worker",
                "adapter": "codex",
            })),
        )
        .unwrap();

    let (flipped, events) = capture_events(|| tools.worker_liveness_tick());
    assert_eq!(flipped, 1);

    let workers = tools.runtime_workers.list_all().unwrap();
    let worker = workers
        .iter()
        .find(|worker| worker.team == "demo" && worker.name == "worker")
        .expect("worker record missing");
    let note = worker.note.as_deref().expect("dead worker note missing");
    assert!(note.contains("stderr captured locally at"), "note: {note}");
    assert!(
        note.contains("codex-stderr-demo__worker.log"),
        "note should reference the local log path: {note}"
    );
    assert!(
        !note.contains("sk-secret-123"),
        "note leaked secret: {note}"
    );
    assert!(
        !note.contains("Authorization"),
        "note leaked stderr: {note}"
    );
    assert!(!note.contains("api_key"), "note leaked stderr: {note}");

    let state_change = events
        .iter()
        .find(|event| event.field("event") == Some("runtime_worker.state_change"))
        .expect("missing runtime_worker.state_change event");
    assert_eq!(state_change.field("reason"), Some(note));
    assert!(
        !state_change
            .field("reason")
            .unwrap_or_default()
            .contains("sk-secret-123"),
        "state change reason leaked secret"
    );

    let stderr_tail_event = events
        .iter()
        .find(|event| event.field("event") == Some("codex_worker.stderr_tail"))
        .expect("missing stderr tail warning event");
    assert_eq!(
        stderr_tail_event.field("stderr_log"),
        Some(stderr_log_hint.as_str())
    );

    let web_state = TeamModeWebState::new(dir.path());
    let events_resp = read_events(&web_state, "demo", Some("7b7d"), None).unwrap();
    let worker_event = events_resp
        .events
        .iter()
        .find(|event| event.event_type == "workerStatusChanged")
        .expect("missing workerStatusChanged event");
    let payload_note = worker_event.payload["note"].as_str().unwrap();
    assert!(payload_note.contains("stderr captured locally at"));
    assert!(!payload_note.contains("sk-secret-123"));
    assert!(!payload_note.contains("Authorization"));
    assert!(!payload_note.contains("api_key"));
}
