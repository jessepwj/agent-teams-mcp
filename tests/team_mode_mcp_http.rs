use agent_teams::team_mode::mcp::http_transport::{HttpMcpState, router as http_mcp_router};
use agent_teams::team_mode::service::{CreateTeamRequest, TeamService};
use agent_teams::team_mode::storage::TeamStore;
use agent_teams::{TeamModeMcpRuntime, TeamModeToolset, util};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use std::time::Instant;
use tempfile::tempdir;
use tower::ServiceExt;

fn app(base: &std::path::Path) -> axum::Router {
    let toolset = TeamModeToolset::new_with_project_root(base, Some(base.to_path_buf()));
    let runtime = TeamModeMcpRuntime::with_tool_executor(base, Box::new(toolset));
    http_mcp_router(HttpMcpState::new(
        runtime,
        "test-token",
        base.to_path_buf(),
        base.join("runtime"),
        std::process::id(),
        Instant::now(),
    ))
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn post(body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", "Bearer test-token")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn post_with_owner(body: Value, owner_pid: u32) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", "Bearer test-token")
        .header("x-team-mode-owner-cc-pid", owner_pid.to_string())
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get_my_teams(pid: u32, session_id: Option<&str>) -> Request<Body> {
    let uri = match session_id {
        Some(session_id) => {
            format!("/lead-pending/my-teams?pid={pid}&session_id={session_id}")
        }
        None => format!("/lead-pending/my-teams?pid={pid}"),
    };
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", "Bearer test-token")
        .body(Body::empty())
        .unwrap()
}

fn get_my_teams_with_owner_header(pid: u32, owner_pid: u32) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/lead-pending/my-teams?pid={pid}&session_id=session-1"
        ))
        .header("authorization", "Bearer test-token")
        .header("x-team-mode-owner-cc-pid", owner_pid.to_string())
        .body(Body::empty())
        .unwrap()
}

fn get_my_teams_with_token(pid: u32, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri(format!("/lead-pending/my-teams?pid={pid}"));
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::empty()).unwrap()
}

fn get_healthz() -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/healthz")
        .body(Body::empty())
        .unwrap()
}

fn create_team_with_owner(base: &std::path::Path, id: &str, owner_cc_pid: u32) {
    TeamService::new(TeamStore::new(base))
        .create(CreateTeamRequest {
            id: Some(id.into()),
            name: id.into(),
            description: None,
            cwd: None,
            lead_member_id: Some("lead".into()),
            owner_cc_pid: Some(owner_cc_pid),
            overwrite: false,
        })
        .unwrap();
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn lead_pending_my_teams_returns_only_teams_owned_by_resolved_cc_pid() {
    // The hook caller is responsible for walking past shell wrappers and
    // passing the real CC PID; the service trusts that PID directly.
    // Use std::process::id() to model that already-resolved CC PID — the
    // test harness process IS the "CC" from the service's perspective.
    let dir = tempdir().unwrap();
    let app = app(dir.path());
    let cc_pid = std::process::id();
    create_team_with_owner(dir.path(), "mine-a", cc_pid);
    create_team_with_owner(dir.path(), "mine-b", cc_pid);
    create_team_with_owner(dir.path(), "not-mine", cc_pid.saturating_add(1));

    block_on(async {
        let response = app
            .clone()
            .oneshot(get_my_teams(cc_pid, Some("session-1")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["cc_pid"], json!(cc_pid));
        assert_eq!(body["session_id"], json!("session-1"));
        let teams = body["teams"].as_array().unwrap();
        assert_eq!(teams.len(), 2);
        let ids: Vec<_> = teams
            .iter()
            .map(|team| team["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"mine-a"));
        assert!(ids.contains(&"mine-b"));
        assert!(!ids.contains(&"not-mine"));
        for team in teams {
            let pending_path = team["pending_path"].as_str().unwrap();
            assert!(
                pending_path.ends_with(&format!(
                    "{}{}lead_pending.jsonl",
                    team["id"].as_str().unwrap(),
                    std::path::MAIN_SEPARATOR
                )),
                "unexpected pending_path: {pending_path}"
            );
        }
    });
}

#[test]
fn lead_pending_my_teams_returns_zero_teams_when_owner_does_not_match() {
    let dir = tempdir().unwrap();
    let app = app(dir.path());
    let cc_pid = std::process::id();
    create_team_with_owner(dir.path(), "not-mine", cc_pid.saturating_add(1));

    block_on(async {
        let response = app
            .clone()
            .oneshot(get_my_teams(cc_pid, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["cc_pid"], json!(cc_pid));
        assert!(body["session_id"].is_null());
        assert!(body["teams"].as_array().unwrap().is_empty());
    });
}

#[test]
fn lead_pending_my_teams_uses_query_pid_not_owner_header() {
    let dir = tempdir().unwrap();
    let app = app(dir.path());
    let cc_pid = std::process::id();
    create_team_with_owner(dir.path(), "mine", cc_pid);
    create_team_with_owner(dir.path(), "header-owner", 999_999);

    block_on(async {
        let response = app
            .clone()
            .oneshot(get_my_teams_with_owner_header(cc_pid, 999_999))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let teams = body["teams"].as_array().unwrap();
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0]["id"], json!("mine"));
    });
}

#[test]
fn lead_pending_my_teams_does_not_climb_past_caller_pid() {
    // Regression for owner-identity ancestor-walk bug: with v3 install-global
    // running inside Cursor.exe, the service used to re-walk q.pid (the CC
    // node.exe PID) and climb up to Cursor.exe, so my-teams returned empty.
    // The fix is that the service trusts the caller's resolved CC PID
    // verbatim and does NOT call resolve_cc_pid_from on it. We verify by
    // creating a team owned by std::process::id() (the test harness, which
    // always has a real ancestor) and asserting cc_pid in the response is
    // exactly that PID — never an ancestor.
    let dir = tempdir().unwrap();
    let app = app(dir.path());
    let cc_pid = std::process::id();
    create_team_with_owner(dir.path(), "anchored", cc_pid);

    block_on(async {
        let response = app
            .clone()
            .oneshot(get_my_teams(cc_pid, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(
            body["cc_pid"],
            json!(cc_pid),
            "service must trust caller PID, not climb to ancestor"
        );
        let teams = body["teams"].as_array().unwrap();
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0]["id"], json!("anchored"));
    });

    // Sanity: util::current_cc_pid() (the relay/hook walker) DOES climb,
    // and the climbed PID would NOT match the team's owner — proving the
    // service's old behavior of re-walking would have broken routing.
    let walked = util::current_cc_pid().expect("test process should have an ancestor");
    assert_ne!(
        walked, cc_pid,
        "test invariant: cargo test runner has a real ancestor different from itself"
    );
}

#[test]
fn lead_pending_my_teams_requires_valid_bearer_token() {
    let dir = tempdir().unwrap();
    let app = app(dir.path());
    let caller_pid = std::process::id();

    block_on(async {
        let missing = app
            .clone()
            .oneshot(get_my_teams_with_token(caller_pid, None))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let invalid = app
            .clone()
            .oneshot(get_my_teams_with_token(caller_pid, Some("wrong")))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
    });
}

#[test]
fn http_initialize_then_tools_list_matches_runtime_schema() {
    let dir = tempdir().unwrap();
    let app = app(dir.path());

    block_on(async {
        let init = app
            .clone()
            .oneshot(post(json!({"jsonrpc":"2.0","id":1,"method":"initialize"})))
            .await
            .unwrap();
        assert_eq!(init.status(), StatusCode::OK);
        let init = response_json(init).await;
        assert!(init["result"]["capabilities"]["tools"].is_object());

        let tools = app
            .clone()
            .oneshot(post(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})))
            .await
            .unwrap();
        assert_eq!(tools.status(), StatusCode::OK);
        let tools = response_json(tools).await;
        assert!(tools["result"]["tools"].as_array().unwrap().len() >= 7);
    });
}

#[test]
fn healthz_returns_expected_shape() {
    let dir = tempdir().unwrap();
    let app = app(dir.path());

    block_on(async {
        let response = app.clone().oneshot(get_healthz()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["status"], json!("ok"));
        assert_eq!(body["version"], json!(env!("CARGO_PKG_VERSION")));
        assert_eq!(body["lock_holder_pid"], json!(std::process::id()));
        assert!(body["uptime_seconds"].as_u64().unwrap() <= 1);
        assert_eq!(
            body["runtime_dir"],
            json!(dir.path().join("runtime").display().to_string())
        );
    });
}

#[test]
fn http_rejects_missing_token_invalid_token_and_bad_origin() {
    let dir = tempdir().unwrap();
    let app = app(dir.path());
    block_on(async {
        let body = Body::from(json!({"jsonrpc":"2.0","id":1,"method":"initialize"}).to_string());
        let missing = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .body(body)
            .unwrap();
        assert_eq!(
            app.clone().oneshot(missing).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        let invalid = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("authorization", "Bearer wrong")
            .body(Body::from(
                json!({"jsonrpc":"2.0","id":1,"method":"initialize"}).to_string(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(invalid).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        let bad_origin = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("authorization", "Bearer test-token")
            .header("origin", "https://example.com")
            .body(Body::from(
                json!({"jsonrpc":"2.0","id":1,"method":"initialize"}).to_string(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(bad_origin).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    });
}

#[test]
fn owner_header_binds_team_owner_pid() {
    let dir = tempdir().unwrap();
    let app = app(dir.path());
    block_on(async {
        app.clone()
            .oneshot(post(json!({"jsonrpc":"2.0","id":1,"method":"initialize"})))
            .await
            .unwrap();

        let create = post_with_owner(
            json!({
                "jsonrpc":"2.0",
                "id":2,
                "method":"tools/call",
                "params":{"name":"team_create","arguments":{"name":"http-owner"}}
            }),
            1234,
        );
        let response = app.clone().oneshot(create).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(
            body["result"]["structuredContent"]["ownerCcPid"],
            json!(1234)
        );
    });
}

#[test]
fn owner_header_rebinds_existing_team_owner_pid() {
    let dir = tempdir().unwrap();
    let app = app(dir.path());
    block_on(async {
        app.clone()
            .oneshot(post(json!({"jsonrpc":"2.0","id":1,"method":"initialize"})))
            .await
            .unwrap();

        let create_body = json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{"name":"team_create","arguments":{"name":"http-owner"}}
        });
        let first = app
            .clone()
            .oneshot(post_with_owner(create_body.clone(), 1234))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first = response_json(first).await;
        let created_at = first["result"]["structuredContent"]["createdAt"].clone();

        let rebound = app
            .clone()
            .oneshot(post_with_owner(create_body, 5678))
            .await
            .unwrap();
        assert_eq!(rebound.status(), StatusCode::OK);
        let rebound = response_json(rebound).await;
        assert_eq!(
            rebound["result"]["structuredContent"]["ownerCcPid"],
            json!(5678)
        );
        assert_eq!(
            rebound["result"]["structuredContent"]["createdAt"],
            created_at
        );

        let stored = TeamStore::new(dir.path())
            .get("http-owner")
            .unwrap()
            .unwrap();
        assert_eq!(stored.owner_cc_pid, Some(5678));
    });
}
