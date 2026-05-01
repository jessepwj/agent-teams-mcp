#![cfg(feature = "team-mode-web")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use agent_teams::runtime::ExecutionSessionState;
use agent_teams::team_mode::data_dir::ensure_scaffold;
use agent_teams::team_mode::domain::{
    DeliveryStatus, ExecutionMode, ExecutionProfile, MemberKind, Message, MessageKind,
    VisibilityRule,
};
use agent_teams::team_mode::runtime_workers::{RuntimeWorkerStore, STATE_RUNNING};
use agent_teams::team_mode::service::member_service::AddMemberRequest;
use agent_teams::team_mode::service::{
    CreateTeamRequest, InboxService, MessageService, TeamService,
};
use agent_teams::team_mode::storage::{
    MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore,
};
use agent_teams::team_mode_web::routes::WebResponse;
use agent_teams::team_mode_web::{
    StaticBundleMode, TeamModeWebApp, TeamModeWebServerConfig, router,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use tempfile::tempdir;

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn seed_data(base_dir: &std::path::Path) {
    seed_data_with_cwd(base_dir, Some("E:/project"));
}

fn seed_data_with_cwd(base_dir: &std::path::Path, cwd: Option<&str>) {
    ensure_scaffold(base_dir).unwrap();

    let team_store = TeamStore::new(base_dir);
    let member_store = MemberStore::new(base_dir);
    let room_store = RoomStore::new(base_dir);
    let message_store = MessageStore::new(base_dir);
    let projection_store = ProjectionStore::with_message_store(message_store.clone());

    let team_service = TeamService::new(team_store.clone());
    let member_service = agent_teams::team_mode::service::MemberService::new(
        member_store.clone(),
        team_store.clone(),
    );
    let room_service = agent_teams::team_mode::service::RoomService::new(room_store.clone());
    let message_service = MessageService::new(
        message_store.clone(),
        member_store.clone(),
        room_store.clone(),
        team_store.clone(),
    );
    let inbox_service = InboxService::new(projection_store, message_store);

    let execution_cwd = cwd.unwrap_or("E:/project").to_string();
    let team = team_service
        .create(CreateTeamRequest {
            id: Some("demo".into()),
            name: "demo".into(),
            description: Some("Demo team".into()),
            cwd: cwd.map(str::to_string),
            lead_member_id: Some("lead".into()),
            owner_cc_pid: Some(42),
        })
        .unwrap();

    member_service
        .add(AddMemberRequest {
            team_id: team.id.clone(),
            name: "lead".into(),
            kind: MemberKind::Lead,
            role_label: "lead".into(),
            role_description: None,
            execution: None,
        })
        .unwrap();
    member_service
        .add(AddMemberRequest {
            team_id: team.id.clone(),
            name: "alice".into(),
            kind: MemberKind::Member,
            role_label: "worker".into(),
            role_description: None,
            execution: Some(ExecutionProfile {
                execution_mode: ExecutionMode::Managed,
                adapter: Some("claude-code".into()),
                agent_name: Some("alice".into()),
                model: Some("default".into()),
                cwd: Some(execution_cwd),
                env: HashMap::from([
                    ("RUST_LOG".into(), "info".into()),
                    ("ANTHROPIC_API_KEY".into(), "abc".into()),
                ]),
                system_prompt: Some("help".into()),
                skills: vec!["review".into()],
                session_state: Some(ExecutionSessionState::Running),
                session_id: None,
                reasoning_effort: None,
            }),
        })
        .unwrap();

    room_service.ensure_main_room(&team.id).unwrap();

    let root = message_service
        .send(agent_teams::team_mode::service::SendMessageRequest {
            team_id: team.id.clone(),
            room_id: "main".into(),
            sender: "lead".into(),
            kind: MessageKind::Dispatch,
            subject: Some("Review".into()),
            body: "Please review @alice".into(),
            mentions: vec![],
            visibility: vec![VisibilityRule::Team],
            audience_policy: None,
            reply_to: None,
            thread_id: None,
            expires_at: None,
        })
        .unwrap();
    message_service
        .send(agent_teams::team_mode::service::SendMessageRequest {
            team_id: team.id.clone(),
            room_id: "main".into(),
            sender: "alice".into(),
            kind: MessageKind::Reply,
            subject: None,
            body: "Done".into(),
            mentions: vec!["lead".into()],
            visibility: vec![VisibilityRule::Team],
            audience_policy: None,
            reply_to: Some(root.id.clone()),
            thread_id: root.thread_id.clone(),
            expires_at: None,
        })
        .unwrap();

    inbox_service
        .read(&team.id, "alice", std::slice::from_ref(&root.id))
        .unwrap();
    inbox_service
        .ack(&team.id, "alice", std::slice::from_ref(&root.id))
        .unwrap();
}

fn with_temp_home<R>(home: &Path, f: impl FnOnce() -> R) -> R {
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let prev_home = std::env::var_os("HOME");
    let prev_userprofile = std::env::var_os("USERPROFILE");
    unsafe {
        std::env::set_var("HOME", home);
        std::env::set_var("USERPROFILE", home);
    }
    let result = f();
    unsafe {
        match prev_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match prev_userprofile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }
    }
    result
}

fn json(response: &WebResponse) -> serde_json::Value {
    serde_json::from_slice(&response.body).unwrap()
}

fn write_claude_session_fixture(home: &Path, repo_path: &Path, session_id: &str, content: &str) {
    let mut candidates = Vec::new();
    if let Ok(canonical) = repo_path.canonicalize() {
        candidates.push(strip_windows_ext_prefix_for_test(&canonical));
        candidates.push(canonical);
    }
    candidates.push(repo_path.to_path_buf());

    for candidate in candidates {
        let encoded = agent_teams::util::session_discovery::encode_project_path(&candidate);
        let session_dir = home.join(".claude").join("projects").join(encoded);
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join(format!("{session_id}.jsonl")), content).unwrap();
    }
}

fn strip_windows_ext_prefix_for_test(path: &Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return std::path::PathBuf::from(format!(r"\\{}", rest));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return std::path::PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

fn html_visible_text_contains(html: &str, needle: &str) -> bool {
    let mut visible = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag = String::new();
    for ch in html.chars() {
        if in_tag {
            if ch == '>' {
                let tag_name = tag.trim_start().to_ascii_lowercase();
                if tag_name.starts_with("script") {
                    in_script = true;
                } else if tag_name.starts_with("/script") {
                    in_script = false;
                } else if tag_name.starts_with("style") {
                    in_style = true;
                } else if tag_name.starts_with("/style") {
                    in_style = false;
                }
                tag.clear();
                in_tag = false;
            } else {
                tag.push(ch);
            }
        } else if ch == '<' {
            in_tag = true;
        } else if !in_script && !in_style {
            visible.push(ch);
        }
    }
    visible.contains(needle)
}

fn frontend_js_bundle(app: &agent_teams::team_mode_web::TeamModeWebApp) -> String {
    let mut bundle = String::new();
    for asset in [
        "/app.js",
        "/app-state.js",
        "/app-api.js",
        "/app-utils.js",
        "/app-diagnostics.js",
        "/app-render.js",
        "/app-conversation.js",
        "/app-dashboard.js",
        "/app-dashboard-render.js",
    ] {
        let response = app.handle_request("GET", asset);
        assert_eq!(response.status as u16, 200, "missing JS asset: {asset}");
        assert!(
            response.content_type.starts_with("application/javascript"),
            "unexpected JS content-type for {asset}: {}",
            response.content_type
        );
        bundle.push_str(&response.body_text());
        bundle.push('\n');
    }
    bundle
}

fn next_events_cursor(app: &agent_teams::team_mode_web::TeamModeWebApp) -> String {
    let response = app.handle_request("GET", "/api/teams/demo/events");
    assert_eq!(response.status as u16, 200);
    json(&response)["page"]["nextCursor"]
        .as_str()
        .unwrap()
        .to_string()
}

fn hex_encode_for_test(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn save_event_message(base_dir: &Path, id: &str, created_at: DateTime<Utc>) {
    MessageStore::new(base_dir)
        .save(&Message {
            id: id.into(),
            room_id: "main".into(),
            team_id: Some("demo".into()),
            thread_id: Some(format!("{id}-thread")),
            reply_to: None,
            sender: "lead".into(),
            kind: MessageKind::Dispatch,
            subject: Some("Event".into()),
            body: "Please inspect @alice".into(),
            mentions: vec!["alice".into()],
            visibility: vec![VisibilityRule::Team],
            audience_policy: None,
            effective_visibility_reason: None,
            effective_recipients: vec!["alice".into()],
            delivered_to: vec!["alice".into()],
            dropped_for: vec![],
            read_by: vec![],
            acked_by: vec![],
            delivery_status: DeliveryStatus::Delivered,
            created_at,
            expires_at: None,
        })
        .unwrap();
}

#[test]
fn events_empty_team_returns_empty_response_with_cursor() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());

    let response = app.handle_request("GET", "/api/teams/demo/events");
    assert_eq!(response.status as u16, 200);
    let events = json(&response);
    assert_eq!(events["teamId"], "demo");
    assert!(!events["generatedAt"].as_str().unwrap().is_empty());
    assert!(events["events"].as_array().unwrap().is_empty());
    assert_eq!(events["page"]["hasMoreAfter"], false);
    assert!(!events["page"]["nextCursor"].as_str().unwrap().is_empty());
    assert!(events["limitations"].as_array().unwrap().is_empty());

    let empty_cursor_response = app.handle_request("GET", "/api/teams/demo/events?cursor=");
    assert_eq!(empty_cursor_response.status as u16, 200);
    assert!(
        json(&empty_cursor_response)["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn events_invalid_cursor_returns_400() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());

    let response = app.handle_request("GET", "/api/teams/demo/events?cursor=not-hex");

    assert_eq!(response.status as u16, 400);
    assert_eq!(
        json(&response),
        serde_json::json!({ "error": "invalid cursor" })
    );
}

#[test]
fn events_tampered_cursor_payload_returns_400() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());
    let cursor = hex_encode_for_test("[\"not\", \"a\", \"cursor\", \"object\"]");

    let response = app.handle_request("GET", &format!("/api/teams/demo/events?cursor={cursor}"));

    assert_eq!(response.status as u16, 400);
    assert_eq!(
        json(&response),
        serde_json::json!({ "error": "invalid cursor" })
    );
}

#[test]
fn events_after_cursor_returns_message_created() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());
    let cursor = next_events_cursor(&app);

    save_event_message(dir.path(), "msg-event-1", Utc::now());

    let response = app.handle_request("GET", &format!("/api/teams/demo/events?cursor={cursor}"));
    assert_eq!(response.status as u16, 200);
    let events = json(&response);
    let items = events["events"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["eventType"], "messageCreated");
    assert_eq!(items[0]["source"], "messages");
    assert_eq!(items[0]["payload"]["message"]["id"], "msg-event-1");
    assert_eq!(items[0]["payload"]["message"]["sender"], "lead");
}

#[test]
fn events_pagination_with_limit_does_not_skip_events() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());
    let mut cursor = next_events_cursor(&app);

    let first_at = Utc::now() + Duration::seconds(1);
    save_event_message(dir.path(), "msg-event-page-1", first_at);
    save_event_message(
        dir.path(),
        "msg-event-page-2",
        first_at + Duration::seconds(1),
    );
    save_event_message(
        dir.path(),
        "msg-event-page-3",
        first_at + Duration::seconds(2),
    );

    let mut seen = Vec::new();
    loop {
        let response = app.handle_request(
            "GET",
            &format!("/api/teams/demo/events?cursor={cursor}&limit=1"),
        );
        assert_eq!(response.status as u16, 200);
        let page = json(&response);
        let items = page["events"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        seen.push(
            items[0]["payload"]["message"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
        cursor = page["page"]["nextCursor"].as_str().unwrap().to_string();
        if !page["page"]["hasMoreAfter"].as_bool().unwrap() {
            break;
        }
    }

    assert_eq!(
        seen,
        vec!["msg-event-page-1", "msg-event-page-2", "msg-event-page-3"]
    );
}

#[test]
fn events_returns_file_changed_when_lead_pending_changes_without_consuming() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());
    let cursor = next_events_cursor(&app);
    let pending_path = dir.path().join("demo").join("lead_pending.jsonl");
    fs::write(&pending_path, "{\"team\":\"demo\",\"body\":\"wake\"}\n").unwrap();

    let response = app.handle_request("GET", &format!("/api/teams/demo/events?cursor={cursor}"));
    assert_eq!(response.status as u16, 200);
    let events = json(&response);
    let items = events["events"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["eventType"], "fileChanged");
    assert_eq!(items[0]["source"], "filesystem");
    assert_eq!(items[0]["payload"]["fileId"], "leadPending");
    assert_eq!(items[0]["payload"]["path"], "demo/lead_pending.jsonl");
    assert_eq!(items[0]["payload"]["changeKind"], "modified");
    assert_eq!(
        fs::read_to_string(&pending_path).unwrap(),
        "{\"team\":\"demo\",\"body\":\"wake\"}\n"
    );
}

#[test]
fn events_ignore_legacy_root_lead_pending_file() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());
    let cursor = next_events_cursor(&app);
    let legacy_pending_path = dir.path().join("lead_pending.jsonl");
    fs::write(
        &legacy_pending_path,
        "{\"team\":\"demo\",\"body\":\"legacy wake\"}\n",
    )
    .unwrap();

    let response = app.handle_request("GET", &format!("/api/teams/demo/events?cursor={cursor}"));
    assert_eq!(response.status as u16, 200);
    let events = json(&response);
    let items = events["events"].as_array().unwrap();
    assert!(items.is_empty());
    assert_eq!(
        fs::read_to_string(&legacy_pending_path).unwrap(),
        "{\"team\":\"demo\",\"body\":\"legacy wake\"}\n"
    );
}

#[test]
fn events_returns_worker_status_changed_for_runtime_worker() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());
    let cursor = next_events_cursor(&app);

    RuntimeWorkerStore::new(dir.path())
        .upsert_state(
            "demo",
            "bob",
            "demo__bob",
            Some("codex".into()),
            STATE_RUNNING,
            Some("spawned for test".into()),
        )
        .unwrap();

    let response = app.handle_request("GET", &format!("/api/teams/demo/events?cursor={cursor}"));
    assert_eq!(response.status as u16, 200);
    let events = json(&response);
    let items = events["events"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["eventType"], "workerStatusChanged");
    assert_eq!(items[0]["source"], "runtimeWorkers");
    assert_eq!(items[0]["payload"]["workerName"], "bob");
    assert_eq!(items[0]["payload"]["sessionState"], STATE_RUNNING);
    assert_eq!(items[0]["payload"]["lifecycleEvent"], "alive");
}

#[test]
fn healthz_and_read_routes_work() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());

    let response = app.handle_request("GET", "/healthz");
    assert_eq!(response.status as u16, 200);
    assert_eq!(response.body_text(), "ok");

    let response = app.handle_request("GET", "/api/bundle-revision");
    assert_eq!(response.status as u16, 200);
    assert!(response.content_type.starts_with("application/json"));
    let bundle = json(&response);
    let revision = bundle["bundleRevision"].as_str().unwrap();
    assert_eq!(revision.len(), 16);
    assert!(revision.chars().all(|ch| ch.is_ascii_hexdigit()));

    let response = app.handle_request("GET", "/api/teams");
    assert_eq!(response.status as u16, 200);
    let teams = json(&response);
    assert_eq!(teams["teams"].as_array().unwrap().len(), 1);
    assert_eq!(teams["teams"][0]["memberCount"], 2);
    assert_eq!(teams["teams"][0]["activeWorkerCount"], 1);

    let response = app.handle_request("GET", "/api/teams/demo");
    assert_eq!(response.status as u16, 200);
    let team = json(&response);
    assert_eq!(team["counts"]["memberCount"], 2);
    assert_eq!(team["counts"]["activeWorkerCount"], 1);
    assert_eq!(team["counts"]["messageCount"], 2);
    assert_eq!(team["counts"]["threadCount"], 1);
    assert_eq!(team["counts"]["unreadForLead"], 1);

    let response = app.handle_request("GET", "/api/teams/demo/rooms/main?limit=10");
    assert_eq!(response.status as u16, 200);
    let room = json(&response);
    assert_eq!(room["messages"].as_array().unwrap().len(), 2);
    assert_eq!(room["messages"][0]["threadReplyCount"], 1);
    assert_eq!(room["messages"][0]["readCount"], 1);
    assert_eq!(room["messages"][0]["ackedCount"], 1);

    let response = app.handle_request("GET", "/api/teams/demo/rooms/main?limit=1");
    assert_eq!(response.status as u16, 200);
    let limited_room = json(&response);
    assert_eq!(limited_room["messages"].as_array().unwrap().len(), 1);
    assert_eq!(limited_room["page"]["hasMoreBefore"], false);
    assert_eq!(limited_room["page"]["hasMoreAfter"], true);

    let response = app.handle_request("GET", "/api/teams/demo/rooms/main?sender=lead");
    assert_eq!(response.status as u16, 200);
    let filtered_room = json(&response);
    assert_eq!(filtered_room["messages"].as_array().unwrap().len(), 1);
    assert_eq!(filtered_room["messages"][0]["threadReplyCount"], 1);

    let response = app.handle_request("GET", "/api/teams/demo/members");
    assert_eq!(response.status as u16, 200);
    let members = json(&response);
    assert_eq!(members["members"].as_array().unwrap().len(), 2);
    let lead = members["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|member| member["name"] == "lead")
        .unwrap();
    assert_eq!(lead["sessionState"], "coordinator");

    let response = app.handle_request("GET", "/api/teams/demo/members/alice");
    assert_eq!(response.status as u16, 200);
    let member = json(&response);
    assert_eq!(member["profile"]["name"], "alice");
    assert_eq!(member["execution"]["sessionState"], "running");
    assert_eq!(
        member["execution"]["redactedEnv"]["ANTHROPIC_API_KEY"],
        "***"
    );
    assert_eq!(member["activity"]["sentCount"], 1);
    assert_eq!(member["activity"]["receivedCount"], 1);
    assert_eq!(member["activity"]["mentionedCount"], 1);

    let response = app.handle_request("GET", "/api/teams/demo/members/alice/activity");
    assert_eq!(response.status as u16, 200);
    let activity = json(&response);
    assert_eq!(activity["source"], "derived-from-messages");
    assert_eq!(activity["items"].as_array().unwrap().len(), 3);
    assert!(
        activity["limitations"][0]
            .as_str()
            .unwrap()
            .contains("stdout/stderr")
    );
}

#[test]
fn diagnostics_route_reports_sources_and_lead_session() {
    let data_dir = tempdir().unwrap();
    let repo_dir = tempdir().unwrap();
    let home_dir = tempdir().unwrap();
    let repo_path = repo_dir.path().join("repo-root");
    fs::create_dir_all(&repo_path).unwrap();
    let repo_cwd = repo_path.to_string_lossy().to_string();

    seed_data_with_cwd(data_dir.path(), Some(repo_cwd.as_str()));
    MemberStore::new(data_dir.path())
        .update("demo", "alice", |member| {
            if let Some(execution) = member.execution.as_mut() {
                execution.session_id = Some("session-1".into());
            }
        })
        .unwrap();
    fs::write(
        repo_path.join("lead_pending.jsonl"),
        "{\"state\":\"queued\"}\n",
    )
    .unwrap();
    let team_pending_path = data_dir.path().join("demo").join("lead_pending.jsonl");
    fs::write(&team_pending_path, "{\"state\":\"team queued\"}\n").unwrap();
    fs::write(repo_path.join("mcp.log"), "booted\n").unwrap();
    fs::write(repo_path.join(".lead-pending-wake.log"), "wake\n").unwrap();
    fs::write(
        repo_path.join(".lead-sessions.json"),
        r#"{"42":{"session_id":"session-1"}}"#,
    )
    .unwrap();

    write_claude_session_fixture(
        home_dir.path(),
        &repo_path,
        "session-1",
        r#"{"type":"user","message":{"role":"user","content":"Please inspect the team"},"timestamp":"2025-01-01T00:00:00Z"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Team looks healthy."},{"type":"tool_use","name":"Read","input":{"path":"src/lib.rs"}}]},"timestamp":"2025-01-01T00:00:01Z"}
{"tool_name":"Read","tool_input":{"path":"src/lib.rs"},"timestamp":"2025-01-01T00:00:00Z"}
{"usage":{"input_tokens":111,"output_tokens":222,"cache_read_input_tokens":33,"cache_creation_input_tokens":44}}
"#,
    );

    with_temp_home(home_dir.path(), || {
        let app = TeamModeWebApp::with_config(
            data_dir.path(),
            TeamModeWebServerConfig {
                session_home: Some(home_dir.path().to_path_buf()),
                ..TeamModeWebServerConfig::default()
            },
        );
        let response = app.handle_request("GET", "/api/teams/demo/diagnostics");
        assert_eq!(response.status as u16, 200);
        let diagnostics = json(&response);

        assert_eq!(diagnostics["teamId"], "demo");
        assert!(!diagnostics["generatedAt"].as_str().unwrap().is_empty());
        assert!(
            diagnostics["limitations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str().unwrap().contains("stdout/stderr"))
        );

        let sources = diagnostics["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 5);
        assert!(
            sources
                .iter()
                .any(
                    |source| source["label"].as_str().unwrap() == "Lead Pending Queue (team)"
                        && source["exists"].as_bool().unwrap()
                        && source["preview"].as_str().unwrap().contains("team queued")
                )
        );
        assert!(
            sources
                .iter()
                .any(|source| source["label"].as_str().unwrap()
                    == "Lead Pending Queue (project root)"
                    && source["exists"].as_bool().unwrap())
        );
        assert!(
            sources
                .iter()
                .any(
                    |source| source["label"].as_str().unwrap() == "Lead Pending Queue (base dir)"
                        && !source["exists"].as_bool().unwrap()
                )
        );
        assert!(
            sources
                .iter()
                .any(|source| source["label"].as_str().unwrap() == "MCP Log"
                    && source["exists"].as_bool().unwrap())
        );
        assert!(
            sources
                .iter()
                .any(
                    |source| source["label"].as_str().unwrap() == "Lead Pending Wake Log"
                        && source["exists"].as_bool().unwrap()
                )
        );
        assert!(
            sources
                .iter()
                .any(|source| source["preview"].as_str().unwrap().contains("booted"))
        );

        let lead_session = &diagnostics["leadSession"];
        assert!(lead_session["discovered"].as_bool().unwrap());
        assert_eq!(lead_session["sessionCount"], 1);
        assert_eq!(lead_session["latestSessionId"], "session-1");
        assert_eq!(lead_session["recentToolCalls"].as_array().unwrap().len(), 1);
        assert_eq!(lead_session["tokenUsage"]["inputTokens"], 111);
        assert_eq!(lead_session["tokenUsage"]["outputTokens"], 222);
        assert_eq!(lead_session["tokenUsage"]["cacheReadTokens"], 33);
        assert_eq!(lead_session["tokenUsage"]["cacheWriteTokens"], 44);
        assert!(
            lead_session["limitations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str().unwrap().contains("stdout/stderr"))
        );

        let response = app.handle_request("GET", "/api/teams/demo/members/alice/conversation");
        assert_eq!(response.status as u16, 200);
        let conversation = json(&response);
        assert_eq!(conversation["member"], "alice");
        assert_eq!(conversation["source"]["confidence"], "session_id");
        assert_eq!(conversation["source"]["sessionId"], "session-1");
        let items = conversation["items"].as_array().unwrap();
        assert!(
            items
                .iter()
                .any(|item| item["role"] == "user" && item["text"] == "Please inspect the team")
        );
        assert!(
            items
                .iter()
                .any(|item| item["role"] == "assistant" && item["text"] == "Team looks healthy.")
        );
        assert!(
            items
                .iter()
                .any(|item| item["role"] == "tool" && item["title"] == "Read")
        );
    });
}

#[test]
fn diagnostics_route_is_stable_without_files() {
    let data_dir = tempdir().unwrap();
    let repo_dir = tempdir().unwrap();
    let home_dir = tempdir().unwrap();
    let repo_path = repo_dir.path().join("repo-root");
    fs::create_dir_all(&repo_path).unwrap();
    let repo_cwd = repo_path.to_string_lossy().to_string();
    seed_data_with_cwd(data_dir.path(), Some(repo_cwd.as_str()));

    let session_dir = home_dir.path().join(".claude").join("projects").join(
        agent_teams::util::session_discovery::encode_project_path(&repo_path),
    );
    fs::create_dir_all(&session_dir).unwrap();

    with_temp_home(home_dir.path(), || {
        let app = router(data_dir.path());
        let response = app.handle_request("GET", "/api/teams/demo/diagnostics");
        assert_eq!(response.status as u16, 200);
        let diagnostics = json(&response);
        assert_eq!(diagnostics["sources"].as_array().unwrap().len(), 5);
        assert!(
            diagnostics["sources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|source| !source["exists"].as_bool().unwrap())
        );
        assert!(!diagnostics["leadSession"]["discovered"].as_bool().unwrap());
        assert_eq!(diagnostics["leadSession"]["sessionCount"], 0);
    });
}

#[test]
fn api_only_allows_get_and_does_not_mutate_files() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());
    let messages_path = dir.path().join("demo").join("messages.jsonl");
    let before = fs::read_to_string(&messages_path).unwrap();
    let pending_path = dir.path().join("lead_pending.jsonl");
    let team_pending_path = dir.path().join("demo").join("lead_pending.jsonl");
    assert!(!pending_path.exists());
    assert!(!team_pending_path.exists());

    let response = app.handle_request("POST", "/api/teams");
    assert_eq!(response.status as u16, 405);

    for target in [
        "/api/teams",
        "/api/bundle-revision",
        "/api/teams/demo",
        "/api/teams/demo/diagnostics",
        "/api/teams/demo/events",
        "/api/teams/demo/rooms/main",
        "/api/teams/demo/members",
        "/api/teams/demo/members/alice",
        "/api/teams/demo/members/alice/activity",
        "/api/teams/demo/members/alice/conversation",
    ] {
        let response = app.handle_request("GET", target);
        assert_eq!(response.status as u16, 200, "{target}");
    }

    let after = fs::read_to_string(&messages_path).unwrap();
    assert_eq!(before, after);
    assert!(!pending_path.exists());
    assert!(!team_pending_path.exists());
}

#[test]
fn read_model_uses_configured_lead_and_latest_timestamp() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let store = MessageStore::new(dir.path());
    let old = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();
    store
        .save(&Message {
            id: "older-appended-last".into(),
            room_id: "main".into(),
            team_id: Some("demo".into()),
            thread_id: Some("old-thread".into()),
            reply_to: None,
            sender: "alice".into(),
            kind: MessageKind::Status,
            subject: None,
            body: "old status".into(),
            mentions: vec![],
            visibility: vec![VisibilityRule::Team],
            audience_policy: None,
            effective_visibility_reason: None,
            effective_recipients: vec![],
            delivered_to: vec![],
            dropped_for: vec![],
            read_by: vec![],
            acked_by: vec![],
            delivery_status: DeliveryStatus::Delivered,
            created_at: old,
            expires_at: None,
        })
        .unwrap();

    let app = router(dir.path());
    let response = app.handle_request("GET", "/api/teams/demo");
    assert_eq!(response.status as u16, 200);
    let team = json(&response);
    assert!(
        !team["counts"]["lastMessageAt"]
            .as_str()
            .unwrap()
            .starts_with("2000-01-01")
    );

    let custom_dir = tempdir().unwrap();
    ensure_scaffold(custom_dir.path()).unwrap();
    let team_store = TeamStore::new(custom_dir.path());
    let member_store = MemberStore::new(custom_dir.path());
    let room_store = RoomStore::new(custom_dir.path());
    let message_store = MessageStore::new(custom_dir.path());
    let team_service = TeamService::new(team_store.clone());
    let member_service = agent_teams::team_mode::service::MemberService::new(
        member_store.clone(),
        team_store.clone(),
    );
    let room_service = agent_teams::team_mode::service::RoomService::new(room_store.clone());
    let message_service =
        MessageService::new(message_store, member_store.clone(), room_store, team_store);
    let team = team_service
        .create(CreateTeamRequest {
            id: Some("custom".into()),
            name: "custom".into(),
            description: None,
            cwd: None,
            lead_member_id: Some("boss".into()),
            owner_cc_pid: None,
        })
        .unwrap();
    member_service
        .add(AddMemberRequest {
            team_id: team.id.clone(),
            name: "boss".into(),
            kind: MemberKind::Lead,
            role_label: "lead".into(),
            role_description: None,
            execution: None,
        })
        .unwrap();
    member_service
        .add(AddMemberRequest {
            team_id: team.id.clone(),
            name: "alice".into(),
            kind: MemberKind::Member,
            role_label: "worker".into(),
            role_description: None,
            execution: None,
        })
        .unwrap();
    room_service.ensure_main_room(&team.id).unwrap();
    message_service
        .send(agent_teams::team_mode::service::SendMessageRequest {
            team_id: team.id,
            room_id: "main".into(),
            sender: "boss".into(),
            kind: MessageKind::Dispatch,
            subject: None,
            body: "@alice hello".into(),
            mentions: vec![],
            visibility: vec![VisibilityRule::Team],
            audience_policy: None,
            reply_to: None,
            thread_id: None,
            expires_at: None,
        })
        .unwrap();

    let app = router(custom_dir.path());
    let response = app.handle_request("GET", "/api/teams/custom/rooms/main");
    assert_eq!(response.status as u16, 200);
    let room = json(&response);
    assert_eq!(room["messages"][0]["sender"], "boss");
    assert_eq!(room["messages"][0]["senderKind"], "lead");
}

#[test]
fn static_assets_and_root_are_served() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());

    let response = app.handle_request("GET", "/");
    assert_eq!(response.status as u16, 200);
    assert_eq!(response.content_type, "text/html; charset=utf-8");
    let html = response.body_text();
    assert!(
        html.contains("团队模式 Web") || html.contains("Team Mode Web"),
        "HTML root should contain brand kicker (中文 or English)"
    );
    let bundle_revision =
        json(&app.handle_request("GET", "/api/bundle-revision"))["bundleRevision"]
            .as_str()
            .unwrap()
            .to_string();
    assert!(html.contains(&format!(
        r#"<meta name="bundle-revision" content="{bundle_revision}""#
    )));
    assert!(html.contains(&format!("Bundle {bundle_revision}")));
    assert!(html.contains("/app.js"));
    assert!(html.contains("/styles.css"));
    for banned in ["Send", "Start", "Stop", "Delete", "Ack"] {
        assert!(
            !html_visible_text_contains(&html, banned),
            "homepage leaked write action text: {banned}"
        );
    }
    let index = app.handle_request("GET", "/index.html");
    assert_eq!(index.status as u16, 200);
    assert_eq!(index.body_text(), html);

    let response = app.handle_request("GET", "/app.js");
    assert_eq!(response.status as u16, 200);
    assert!(response.content_type.starts_with("application/javascript"));
    let js = frontend_js_bundle(&app);
    assert!(js.contains("Lead Activity"));
    assert!(js.contains("Refresh failed"));
    assert!(js.contains("params.get(\"message\")"));
    assert!(js.contains("params.get(\"member\")"));
    assert!(js.contains("failedTeamId"));
    assert!(js.contains("resolveSelectedTeamId"));
    assert!(js.contains("Team Diagnostics"));
    assert!(js.contains("Lead Session Diagnostics"));
    assert!(js.contains("These diagnostics are file/session-level observations"));
    for phrase in ["no teams", "no members", "no messages"] {
        assert!(js.contains(phrase), "missing empty-state phrase: {phrase}");
    }
    for banned in [
        "process log",
        "worker process log",
        "lead stdout",
        "trace",
        "replay",
    ] {
        assert!(
            !js.contains(banned),
            "frontend leaked misleading lead wording: {banned}"
        );
    }

    let response = app.handle_request("GET", "/styles.css");
    assert_eq!(response.status as u16, 200);
    assert_eq!(response.content_type, "text/css; charset=utf-8");
}

#[test]
fn dev_static_bundle_reads_disk_without_restarting_app() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let web_dir = dir.path().join("web").join("team-mode");
    fs::create_dir_all(&web_dir).unwrap();
    fs::write(
        web_dir.join("index.html"),
        r#"<html><head><meta name="bundle-revision" content="__TEAM_MODE_WEB_BUNDLE_REVISION__"></head><body>Bundle __TEAM_MODE_WEB_BUNDLE_REVISION__</body></html>"#,
    )
    .unwrap();
    fs::write(web_dir.join("app.js"), "const version = 'one';").unwrap();
    fs::write(web_dir.join("styles.css"), "body { color: red; }").unwrap();
    fs::write(web_dir.join("dashboard.css"), ".dashboard { color: blue; }").unwrap();

    let app = TeamModeWebApp::with_config(
        dir.path(),
        TeamModeWebServerConfig {
            static_bundle: StaticBundleMode::Dev {
                root: web_dir.clone(),
            },
            ..TeamModeWebServerConfig::default()
        },
    );

    let revision = json(&app.handle_request("GET", "/api/bundle-revision"));
    assert_eq!(revision["bundleRevision"].as_str().unwrap(), "dev");
    let html = app.handle_request("GET", "/").body_text();
    assert!(html.contains(r#"content="dev""#));
    assert!(html.contains("Bundle dev"));

    let js = app.handle_request("GET", "/app.js").body_text();
    assert_eq!(js, "const version = 'one';");
    fs::write(web_dir.join("app.js"), "const version = 'two';").unwrap();
    let js = app.handle_request("GET", "/app.js").body_text();
    assert_eq!(js, "const version = 'two';");

    let nested = app.handle_request("GET", "/nested/app.js");
    assert_eq!(nested.status as u16, 404);
}

#[test]
fn dev_static_bundle_missing_whitelisted_asset_returns_500_without_baked_fallback() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let web_dir = dir.path().join("web").join("team-mode");
    fs::create_dir_all(&web_dir).unwrap();

    let app = TeamModeWebApp::with_config(
        dir.path(),
        TeamModeWebServerConfig {
            static_bundle: StaticBundleMode::Dev { root: web_dir },
            ..TeamModeWebServerConfig::default()
        },
    );

    let response = app.handle_request("GET", "/app.js");
    assert_eq!(response.status as u16, 500);
    assert_eq!(response.content_type, "application/json; charset=utf-8");
    let body = response.body_text();
    assert!(
        body.contains("failed to read dev static asset"),
        "unexpected body: {body}"
    );
    assert!(
        !body.contains("Lead Activity") && !body.contains("Refresh failed"),
        "missing dev asset fell back to baked JavaScript: {body}"
    );
}
