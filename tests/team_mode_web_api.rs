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
use agent_teams::team_mode::service::member_service::AddMemberRequest;
use agent_teams::team_mode::service::{
    CreateTeamRequest, InboxService, MessageService, TeamService,
};
use agent_teams::team_mode::storage::{
    MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore,
};
use agent_teams::team_mode_web::router;
use agent_teams::team_mode_web::routes::WebResponse;
use chrono::{TimeZone, Utc};
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

#[test]
fn healthz_and_read_routes_work() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let app = router(dir.path());

    let response = app.handle_request("GET", "/healthz");
    assert_eq!(response.status as u16, 200);
    assert_eq!(response.body_text(), "ok");

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
    fs::write(
        repo_path.join("lead_pending.jsonl"),
        "{\"state\":\"queued\"}\n",
    )
    .unwrap();
    fs::write(repo_path.join("mcp.log"), "booted\n").unwrap();
    fs::write(repo_path.join(".lead-pending-wake.log"), "wake\n").unwrap();

    let encoded = agent_teams::util::session_discovery::encode_project_path(&repo_path);
    let session_dir = home_dir
        .path()
        .join(".claude")
        .join("projects")
        .join(encoded);
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("session-1.jsonl"),
        r#"{"type":"user","message":{"role":"user","content":"Please inspect the team"},"timestamp":"2025-01-01T00:00:00Z"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Team looks healthy."},{"type":"tool_use","name":"Read","input":{"path":"src/lib.rs"}}]},"timestamp":"2025-01-01T00:00:01Z"}
{"tool_name":"Read","tool_input":{"path":"src/lib.rs"},"timestamp":"2025-01-01T00:00:00Z"}
{"usage":{"input_tokens":111,"output_tokens":222,"cache_read_input_tokens":33,"cache_creation_input_tokens":44}}
"#,
    )
    .unwrap();

    with_temp_home(home_dir.path(), || {
        let app = router(data_dir.path());
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
        assert_eq!(sources.len(), 4);
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
        assert_eq!(conversation["source"]["confidence"], "cwd_latest");
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
        assert_eq!(diagnostics["sources"].as_array().unwrap().len(), 4);
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
    assert!(!pending_path.exists());

    let response = app.handle_request("POST", "/api/teams");
    assert_eq!(response.status as u16, 405);

    for target in [
        "/api/teams",
        "/api/teams/demo",
        "/api/teams/demo/diagnostics",
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
    assert!(html.contains("Team Mode Web"));
    assert!(html.contains("/app.js"));
    assert!(html.contains("/styles.css"));
    assert!(!html.contains("<form"), "homepage should not contain forms");
    for banned in ["Send", "Start", "Stop", "Delete", "Ack"] {
        assert!(
            !html.contains(banned),
            "homepage leaked write action text: {banned}"
        );
    }
    let index = app.handle_request("GET", "/index.html");
    assert_eq!(index.status as u16, 200);
    assert_eq!(index.body_text(), html);

    let response = app.handle_request("GET", "/app.js");
    assert_eq!(response.status as u16, 200);
    assert!(response.content_type.starts_with("application/javascript"));
    let js = response.body_text();
    assert!(js.contains("Lead Activity"));
    assert!(js.contains("Refresh failed"));
    assert!(js.contains("#message="));
    assert!(js.contains("#member="));
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
