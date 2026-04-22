use std::path::Path;

use team_mode_native::adapters::codex_app_server::CodexRequest;
use team_mode_native::adapters::{
    ClaudePromptMode, claude_command_spec, gemini_command_spec, team_mode_mcp_config,
};
use team_mode_native::runner::protocol::{
    HostToRunnerFrame, InjectInputFrame, InjectStrategyWire, OutputStream, RunnerFrame,
    RunnerOutputFrame,
};
use team_mode_native::runner::{InjectionStrategy, format_injected_input};
use team_mode_native::viewer::{render_codex_event_line, tail_lines};

#[test]
fn runner_frame_serde_roundtrip() {
    let frame = RunnerFrame::Output(RunnerOutputFrame {
        member_id: "alice".to_string(),
        runner_id: "runner-1".to_string(),
        stream: OutputStream::Pty,
        data: "hello".to_string(),
    });

    let json = serde_json::to_string(&frame).unwrap();
    assert!(json.contains(r#""type":"runner/output""#));
    let decoded: RunnerFrame = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, frame);

    let inject = HostToRunnerFrame::InjectInput(InjectInputFrame {
        member_id: "alice".to_string(),
        runner_id: "runner-1".to_string(),
        injection_id: "inject-1".to_string(),
        text: "ping".to_string(),
        strategy: InjectStrategyWire::PasteAndEnter,
    });
    let json = serde_json::to_string(&inject).unwrap();
    assert!(json.contains(r#""type":"host/inject_input""#));
    let decoded: HostToRunnerFrame = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, inject);
}

#[test]
fn claude_and_gemini_command_generation() {
    let prompt = Path::new(r"E:\work dir\prompt.md");
    let mcp = Path::new(r"E:\work dir\mcp.json");

    let append = claude_command_spec(
        "claude",
        prompt,
        ClaudePromptMode::Append,
        Some(mcp),
        None,
        ["--verbose"],
    );
    assert_eq!(append.program, "claude");
    assert_eq!(append.args[0], "--append-system-prompt-file");
    assert_eq!(append.args[1], prompt.display().to_string());
    assert!(append.args.contains(&"--mcp-config".to_string()));

    let replace = claude_command_spec(
        "claude",
        prompt,
        ClaudePromptMode::Replace,
        None,
        None,
        Vec::<String>::new(),
    );
    assert_eq!(replace.args[0], "--system-prompt-file");

    let gemini = gemini_command_spec("gemini", prompt, None, ["--model", "gemini-pro"]);
    assert_eq!(gemini.program, "gemini");
    assert_eq!(gemini.env[0].key, "GEMINI_SYSTEM_MD");
    assert_eq!(gemini.env[0].value, prompt.display().to_string());
    assert_eq!(gemini.args, vec!["--model", "gemini-pro"]);

    let config = team_mode_mcp_config(
        "team_mode_mcp_proxy",
        "127.0.0.1:17891",
        "alice",
        "TEAM_MODE_RUNNER_TOKEN",
    );
    assert_eq!(
        config["mcpServers"]["team-mode"]["args"][3],
        serde_json::json!("alice")
    );
}

#[test]
fn inject_message_formatting() {
    assert_eq!(
        format_injected_input("hello", InjectionStrategy::PasteAndEnter),
        b"hello\n"
    );
    assert_eq!(
        format_injected_input("hello\n", InjectionStrategy::PasteAndEnter),
        b"hello\n"
    );
}

#[test]
fn codex_request_serde_has_no_mcp_jsonrpc_field() {
    let request = CodexRequest::turn_start(7, "thread-1", "do work");
    let value = serde_json::to_value(request).unwrap();
    assert_eq!(value["id"], serde_json::json!(7));
    assert_eq!(value["method"], serde_json::json!("turn/start"));
    assert_eq!(value["params"]["threadId"], serde_json::json!("thread-1"));
    assert!(value["params"].get("kind").is_none());
    assert!(value.get("jsonrpc").is_none());
}

#[test]
fn viewer_tail_and_codex_rendering() {
    let lines = tail_lines(["a", "b", "c", "d"], 2);
    assert_eq!(lines, vec!["c".to_string(), "d".to_string()]);

    let rendered = render_codex_event_line(r#"{"event":"turn_delta","text":"hello"}"#);
    assert_eq!(rendered, "[turn_delta] hello");
}
