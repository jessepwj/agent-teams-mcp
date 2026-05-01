use tempfile::tempdir;

use super::*;

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
