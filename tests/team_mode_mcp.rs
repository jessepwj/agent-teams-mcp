//! Integration tests for the Team Mode MCP JSON-RPC layer.
//!
//! These tests exercise the full protocol flow through `TeamModeMcpRuntime::handle_request`,
//! covering: initialize → team lifecycle → member management → messaging → inbox → thread reply.

use agent_teams::TeamModeMcpRuntime;
use agent_teams::prelude::JsonRpcRequest;
use serde_json::{Value, json};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn req(id: u64, method: &str, params: Option<Value>) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(id)),
        method: method.into(),
        params,
    }
}

fn call(runtime: &mut TeamModeMcpRuntime, id: u64, tool: &str, args: Value) -> Value {
    let (resp, _) = runtime
        .handle_request(req(id, "tools/call", Some(json!({
            "name": tool,
            "arguments": args
        }))))
        .unwrap();
    resp.unwrap()
}

fn call_with_notifs(
    runtime: &mut TeamModeMcpRuntime,
    id: u64,
    tool: &str,
    args: Value,
) -> (Value, Vec<Value>) {
    let (resp, notifs) = runtime
        .handle_request(req(id, "tools/call", Some(json!({
            "name": tool,
            "arguments": args
        }))))
        .unwrap();
    (resp.unwrap(), notifs)
}

/// Assert that the tool call result has no isError flag set.
fn assert_tool_ok(resp: &Value) {
    let result = &resp["result"];
    assert!(
        result["isError"].is_null() || result["isError"] == json!(false),
        "tool returned isError: {resp}"
    );
    assert!(result["content"].is_array(), "expected content array, got: {resp}");
}

/// Assert that the tool call result has isError = true.
fn assert_tool_err(resp: &Value) {
    assert_eq!(
        resp["result"]["isError"],
        json!(true),
        "expected isError:true, got: {resp}"
    );
}

/// Extract text from the first content element.
fn first_text(resp: &Value) -> String {
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Parse the first content text as JSON Value.
fn first_json(resp: &Value) -> Value {
    let text = first_text(resp);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON: {e}\n  text: {text}"))
}

// ---------------------------------------------------------------------------
// Protocol-level tests
// ---------------------------------------------------------------------------

#[test]
fn reject_requests_before_initialize() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());

    let (resp, _) = rt
        .handle_request(req(1, "tools/list", None))
        .unwrap();
    let resp = resp.unwrap();
    assert!(resp["error"].is_object(), "expected error before init: {resp}");
    assert_eq!(resp["error"]["code"], json!(-32000));
}

#[test]
fn initialize_returns_capabilities() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());

    let (resp, notifs) = rt
        .handle_request(req(1, "initialize", None))
        .unwrap();
    let resp = resp.unwrap();
    assert!(resp["result"]["capabilities"]["tools"].is_object());
    assert!(notifs.is_empty());
}

#[test]
fn tools_list_exposes_minimal_7_surface() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    let (resp, _) = rt
        .handle_request(req(2, "tools/list", None))
        .unwrap();
    let resp = resp.unwrap();
    let tools = resp["result"]["tools"].as_array().unwrap();

    let names: Vec<&str> = tools.iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();

    // The final-minimal MCP surface: 7 tools only.
    let expected: &[&str] = &[
        "team_create", "team_list", "team_delete",
        "worker_add", "worker_list", "worker_remove",
        "send_message",
    ];
    for name in expected {
        assert!(names.contains(name), "missing tool: {name}");
    }
    assert_eq!(
        tools.len(),
        expected.len(),
        "unexpected tool count; names = {names:?}"
    );

    // Removed legacy tools must not come back.
    for gone in &[
        "team_get", "member_get", "member_update",
        "member_add", "member_remove", "member_list",
        "room_post_message", "room_read_messages", "room_list",
        "inbox_peek", "inbox_read", "inbox_ack", "inbox_count",
        "thread_read", "thread_reply",
        "execution_profile_set",
        "spawn_member", "shutdown_member",
        "member_spawn_managed", "member_shutdown_managed",
        "member_resume_managed", "member_session_status",
    ] {
        assert!(!names.contains(gone), "removed tool still exposed: {gone}");
    }
}

#[test]
fn unknown_method_returns_method_not_found() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    let (resp, _) = rt
        .handle_request(req(2, "no_such_method", None))
        .unwrap();
    let resp = resp.unwrap();
    assert_eq!(resp["error"]["code"], json!(-32601));
}

#[test]
fn ping_returns_empty_result() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    let (resp, _) = rt
        .handle_request(req(2, "ping", None))
        .unwrap();
    let resp = resp.unwrap();
    assert!(resp["result"].is_object());
    assert!(resp["error"].is_null());
}

// ---------------------------------------------------------------------------
// Full collaboration flow
// ---------------------------------------------------------------------------

#[test]
fn team_create_sets_up_lead_and_supports_worker_list() {
    // Integration-style test that does not spawn any real worker processes:
    // only exercises the MCP surface. worker_add's real spawn path is
    // covered by end-to-end live MCP testing, not here.
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    // 1. create team
    let resp = call(&mut rt, 2, "team_create", json!({
        "name": "alpha",
        "cwd": "E:\\project"
    }));
    assert_tool_ok(&resp);

    // 2. team_list shows alpha with a lead auto-bound
    let resp = call(&mut rt, 3, "team_list", json!({}));
    assert_tool_ok(&resp);
    let list = first_json(&resp);
    let teams = list["teams"].as_array().unwrap();
    let team = teams.iter().find(|t| t["name"] == "alpha").unwrap();
    assert_eq!(team["leadMemberId"].as_str(), Some("alpha-lead"));
    assert_eq!(team["cwd"].as_str(), Some("E:\\project"));

    // 3. worker_list should be empty (lead is never listed).
    let resp = call(&mut rt, 4, "worker_list", json!({"team": "alpha"}));
    assert_tool_ok(&resp);
    let obj = first_json(&resp);
    assert!(obj["workers"].as_array().unwrap().is_empty());

    // 4. reading the lead's inbox resource works
    let (resp, _) = rt
        .handle_request(req(5, "resources/read", Some(json!({
            "uri": "team://alpha/members/alpha-lead/inbox"
        }))))
        .unwrap();
    let resp = resp.unwrap();
    assert!(resp["result"]["contents"].is_array());

    // 5. cleanup
    let resp = call(&mut rt, 6, "team_delete", json!({"name": "alpha"}));
    assert_tool_ok(&resp);

    let resp = call(&mut rt, 7, "team_list", json!({}));
    let text = first_text(&resp);
    assert!(!text.contains("alpha"), "deleted team still in list: {text}");
}

// ---------------------------------------------------------------------------
// Error-path tests
// ---------------------------------------------------------------------------

#[test]
fn tool_call_missing_required_params_returns_error_content() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    // team_create without required "name"
    let resp = call(&mut rt, 2, "team_create", json!({}));
    assert_tool_err(&resp);
    let text = first_text(&resp);
    assert!(text.contains("name"), "error should mention 'name': {text}");
}

#[test]
fn worker_add_to_nonexistent_team_returns_error() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    let resp = call(&mut rt, 2, "worker_add", json!({
        "team": "does-not-exist",
        "name": "alice",
        "adapter": "claude-code"
    }));
    assert_tool_err(&resp);
    let text = first_text(&resp);
    assert!(
        text.to_lowercase().contains("not found") || text.to_lowercase().contains("team"),
        "error should mention team not found: {text}"
    );
}

#[test]
fn send_message_rejects_no_mention_in_text() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    call(&mut rt, 2, "team_create", json!({"name": "t1"}));
    let resp = call(&mut rt, 3, "send_message", json!({
        "team": "t1",
        "text": "talking into the void"
    }));
    assert_tool_err(&resp);
    let text = first_text(&resp);
    assert!(
        text.contains("@handle"),
        "error should mention mandatory @handle, got: {text}"
    );
}

#[test]
fn duplicate_team_name_is_rejected() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    call(&mut rt, 2, "team_create", json!({"name": "dup-team"}));
    let resp = call(&mut rt, 3, "team_create", json!({"name": "dup-team"}));
    assert_tool_err(&resp);
}

#[test]
fn worker_add_refuses_reserved_name_lead() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    call(&mut rt, 2, "team_create", json!({"name": "t1"}));
    let resp = call(&mut rt, 3, "worker_add", json!({
        "team": "t1",
        "name": "lead",
        "adapter": "claude-code"
    }));
    assert_tool_err(&resp);
    let text = first_text(&resp);
    assert!(
        text.to_lowercase().contains("reserved"),
        "error should explain the reserved name, got: {text}"
    );
}

// ---------------------------------------------------------------------------
// Resources tests
// ---------------------------------------------------------------------------

#[test]
fn resources_list_and_read_after_team_created() {
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    call(&mut rt, 2, "team_create", json!({"name": "res-team"}));

    // resources/list
    let (resp, _) = rt
        .handle_request(req(3, "resources/list", None))
        .unwrap();
    let resp = resp.unwrap();
    let resources = resp["result"]["resources"].as_array().unwrap();
    assert!(!resources.is_empty(), "expected resources after team creation");

    let uris: Vec<&str> = resources.iter()
        .map(|r| r["uri"].as_str().unwrap_or(""))
        .collect();
    assert!(uris.iter().any(|u| u.contains("res-team")), "team URI missing: {uris:?}");

    // resources/read for team
    let (resp, _) = rt
        .handle_request(req(4, "resources/read", Some(json!({
            "uri": "team://res-team"
        }))))
        .unwrap();
    let resp = resp.unwrap();
    assert!(resp["result"]["contents"].is_array());
}

#[test]
fn subscribe_triggers_notification_after_team_delete() {
    // Verifies the subscribe → notify pipeline without requiring a real worker
    // process. We subscribe to the team URI and then delete the team, which
    // pushes the team URI as an updated_resource on the tool response.
    let dir = tempdir().unwrap();
    let mut rt = TeamModeMcpRuntime::new(dir.path());
    rt.handle_request(req(1, "initialize", None)).unwrap();

    call(&mut rt, 2, "team_create", json!({"name": "sub-team"}));

    // subscribe to team URI
    let (resp, _) = rt
        .handle_request(req(3, "resources/subscribe", Some(json!({
            "uri": "team://sub-team"
        }))))
        .unwrap();
    let resp = resp.unwrap();
    assert!(resp["error"].is_null(), "subscribe failed: {resp}");

    // delete the team — should trigger notification on team URI
    let (resp, notifs) = call_with_notifs(&mut rt, 4, "team_delete", json!({
        "name": "sub-team"
    }));
    assert_tool_ok(&resp);

    let notif_uris: Vec<&str> = notifs.iter()
        .filter_map(|n| n["params"]["uri"].as_str())
        .collect();
    assert!(
        notif_uris.contains(&"team://sub-team"),
        "expected team URI in notifications, got: {notif_uris:?}"
    );
    assert!(notifs.iter().all(|n| n["method"] == json!("notifications/resources/updated")));
}

// ---------------------------------------------------------------------------
// Binary smoke test (process-level)
// ---------------------------------------------------------------------------

#[test]
fn binary_responds_to_initialize_over_stdio() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let dir = tempdir().unwrap();

    let binary = env!("CARGO_BIN_EXE_team_mode_mcp");

    let mut child = Command::new(binary)
        .arg("--data-dir")
        .arg(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn team_mode_mcp binary");

    // Send NDJSON (JSON + newline) — same format Claude Code uses
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": null
    });
    let ndjson = format!("{}\n", serde_json::to_string(&payload).unwrap());

    let stdin = child.stdin.as_mut().unwrap();
    stdin.write_all(ndjson.as_bytes()).unwrap();
    drop(child.stdin.take()); // close stdin → server exits loop

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Response must also be NDJSON (no Content-Length header)
    assert!(
        !stdout.contains("Content-Length"),
        "response must not use Content-Length framing: {stdout}"
    );
    assert!(
        stdout.contains(r#""result""#),
        "expected JSON-RPC result in stdout, got: {stdout}"
    );
    assert!(
        stdout.contains("capabilities"),
        "expected capabilities in initialize response, got: {stdout}"
    );
}
