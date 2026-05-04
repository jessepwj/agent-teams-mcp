#![cfg(feature = "team-mode-web")]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration as StdDuration;

use agent_teams::runtime::ExecutionSessionState;
use agent_teams::team_mode::data_dir::ensure_scaffold;
use agent_teams::team_mode::domain::{
    DeliveryStatus, ExecutionMode, ExecutionProfile, MemberKind, Message, MessageKind,
    VisibilityRule,
};
use agent_teams::team_mode::service::member_service::AddMemberRequest;
use agent_teams::team_mode::service::{CreateTeamRequest, MessageService, TeamService};
use agent_teams::team_mode::storage::{MemberStore, MessageStore, RoomStore, TeamStore};
use agent_teams::team_mode_web::{
    SseConfig, TeamModeWebServerConfig, router, serve_listener_with_config,
};
use chrono::{DateTime, Duration, Utc};
use tempfile::tempdir;

fn seed_data(base_dir: &Path) {
    ensure_scaffold(base_dir).unwrap();
    let team_store = TeamStore::new(base_dir);
    let member_store = MemberStore::new(base_dir);
    let room_store = RoomStore::new(base_dir);
    let message_store = MessageStore::new(base_dir);
    let team_service = TeamService::new(team_store.clone());
    let member_service = agent_teams::team_mode::service::MemberService::new(
        member_store.clone(),
        team_store.clone(),
    );
    let room_service = agent_teams::team_mode::service::RoomService::new(room_store.clone());
    let message_service = MessageService::new(message_store, member_store, room_store, team_store);

    let team = team_service
        .create(CreateTeamRequest {
            id: Some("demo".into()),
            name: "demo".into(),
            description: Some("Demo team".into()),
            cwd: Some("E:/project".into()),
            lead_member_id: Some("lead".into()),
            owner_cc_pid: Some(42),
            overwrite: false,
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
                cwd: Some("E:/project".into()),
                env: HashMap::new(),
                system_prompt: None,
                skills: vec![],
                session_state: Some(ExecutionSessionState::Running),
                session_id: None,
                reasoning_effort: None,
            }),
        })
        .unwrap();
    room_service.ensure_main_room(&team.id).unwrap();
    message_service
        .send(agent_teams::team_mode::service::SendMessageRequest {
            team_id: team.id,
            room_id: "main".into(),
            sender: "lead".into(),
            kind: MessageKind::Dispatch,
            subject: None,
            body: "baseline @alice".into(),
            mentions: vec!["alice".into()],
            visibility: vec![VisibilityRule::Team],
            audience_policy: None,
            reply_to: None,
            thread_id: None,
            expires_at: None,
        })
        .unwrap();
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

fn start_server(base_dir: PathBuf, sse: SseConfig, max_connections: usize) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        serve_listener_with_config(
            base_dir,
            listener,
            TeamModeWebServerConfig {
                sse,
                max_connections: Some(max_connections),
                session_home: None,
                ..TeamModeWebServerConfig::default()
            },
        )
        .unwrap();
    });
    addr
}

fn test_sse_config() -> SseConfig {
    SseConfig {
        poll_interval: StdDuration::from_millis(50),
        heartbeat_interval: StdDuration::from_millis(100),
        max_stream_duration: Some(StdDuration::from_secs(2)),
    }
}

fn connect(addr: SocketAddr, request: &str) -> TcpStream {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(StdDuration::from_secs(3)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    stream
}

fn read_headers(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buf = [0_u8; 256];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&bytes).to_string()
}

fn read_frame(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buf = [0_u8; 256];
    while !bytes.windows(2).any(|window| window == b"\n\n") {
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&bytes).to_string()
}

fn next_events_cursor(base_dir: &Path) -> String {
    let app = router(base_dir);
    let response = app.handle_request("GET", "/api/teams/demo/events");
    let json: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    json["page"]["nextCursor"].as_str().unwrap().to_string()
}

#[test]
fn sse_stream_returns_text_event_stream() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let addr = start_server(dir.path().to_path_buf(), test_sse_config(), 1);
    let mut stream = connect(
        addr,
        "GET /api/teams/demo/events/stream HTTP/1.1\r\nAccept: text/event-stream\r\n\r\n",
    );
    let headers = read_headers(&mut stream);
    assert!(headers.starts_with("HTTP/1.1 200 OK"));
    assert!(headers.contains("Content-Type: text/event-stream"));
}

#[test]
fn sse_stream_invalid_last_event_id_returns_4xx() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let addr = start_server(dir.path().to_path_buf(), test_sse_config(), 1);
    let mut stream = connect(
        addr,
        "GET /api/teams/demo/events/stream HTTP/1.1\r\n\
Accept: text/event-stream\r\n\
Last-Event-ID: not-hex\r\n\r\n",
    );

    let headers = read_headers(&mut stream);

    assert!(headers.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(headers.contains("Content-Type: application/json"));
    assert!(!headers.contains("Content-Type: text/event-stream"));
}

#[test]
fn sse_stream_pushes_message_created_after_connect() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let addr = start_server(dir.path().to_path_buf(), test_sse_config(), 1);
    let mut stream = connect(
        addr,
        "GET /api/teams/demo/events/stream HTTP/1.1\r\nAccept: text/event-stream\r\n\r\n",
    );
    read_headers(&mut stream);
    save_event_message(dir.path(), "sse-msg-1", Utc::now() + Duration::seconds(1));

    for _ in 0..5 {
        let frame = read_frame(&mut stream);
        if frame.contains("event: messageCreated") {
            assert!(frame.contains("\"id\":\"sse-msg-1\""));
            return;
        }
    }
    panic!("messageCreated frame not received");
}

#[test]
fn sse_stream_sends_heartbeat_frames() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let addr = start_server(dir.path().to_path_buf(), test_sse_config(), 1);
    let mut stream = connect(
        addr,
        "GET /api/teams/demo/events/stream HTTP/1.1\r\nAccept: text/event-stream\r\n\r\n",
    );
    read_headers(&mut stream);

    let mut heartbeats = 0;
    for _ in 0..4 {
        let frame = read_frame(&mut stream);
        if frame.contains("event: heartbeat") {
            heartbeats += 1;
        }
        if heartbeats >= 2 {
            return;
        }
    }
    panic!("expected at least two heartbeat frames, got {heartbeats}");
}

#[test]
fn sse_stream_last_event_id_resumes_after_cursor() {
    let dir = tempdir().unwrap();
    seed_data(dir.path());
    let initial = next_events_cursor(dir.path());
    save_event_message(
        dir.path(),
        "sse-before-reconnect",
        Utc::now() + Duration::seconds(1),
    );
    let app = router(dir.path());
    let response = app.handle_request("GET", &format!("/api/teams/demo/events?cursor={initial}"));
    let json: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    let last_event_id = json["events"][0]["cursor"].as_str().unwrap();

    let addr = start_server(dir.path().to_path_buf(), test_sse_config(), 1);
    let mut stream = connect(
        addr,
        &format!(
            "GET /api/teams/demo/events/stream?cursor={initial} HTTP/1.1\r\n\
Accept: text/event-stream\r\nLast-Event-ID: {last_event_id}\r\n\r\n"
        ),
    );
    read_headers(&mut stream);
    save_event_message(
        dir.path(),
        "sse-after-reconnect",
        Utc::now() + Duration::seconds(2),
    );

    for _ in 0..5 {
        let frame = read_frame(&mut stream);
        assert!(!frame.contains("sse-before-reconnect"));
        if frame.contains("event: messageCreated") {
            assert!(frame.contains("sse-after-reconnect"));
            return;
        }
    }
    panic!("post-reconnect messageCreated frame not received");
}
