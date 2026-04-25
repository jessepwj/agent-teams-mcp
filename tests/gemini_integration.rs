//! Integration tests for the Gemini CLI backend.
//!
//! These tests require the `gemini` CLI to be installed and authenticated.
//! They are gated behind the `gemini-cli-tests` feature flag so they don't
//! run in CI (where gemini may not be available).
//!
//! Run manually with:
//!   cargo test --test gemini_integration -- --nocapture --include-ignored

use std::time::Duration;

use agent_teams::backend::gemini::GeminiCliBackend;
use agent_teams::backend::{AgentBackend, AgentOutput, BackendType, SpawnConfig};

/// Skip the test if gemini CLI is not available.
fn require_gemini() -> GeminiCliBackend {
    match GeminiCliBackend::new() {
        Ok(b) => b,
        Err(_) => {
            eprintln!("⚠ Skipping: gemini CLI not found on PATH");
            std::process::exit(0);
        }
    }
}

#[tokio::test]
async fn gemini_backend_type_is_correct() {
    let backend = require_gemini();
    assert_eq!(backend.backend_type(), BackendType::GeminiCli);
}

#[tokio::test]
#[ignore = "requires live Gemini CLI — run with --include-ignored"]
async fn gemini_spawn_and_receive_output() {
    let backend = require_gemini();

    let config = SpawnConfig {
        name: "test-gemini".into(),
        prompt: "You are a test assistant. Always reply briefly.".into(),
        model: Some("gemini-2.5-flash".into()),
        cwd: None,
        max_turns: None,
        allowed_tools: vec![],
        permission_mode: None,
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    let mut session = backend.spawn(config).await.expect("spawn should succeed");

    assert_eq!(session.name(), "test-gemini");
    assert!(session.is_alive().await);

    // Take the output receiver
    let mut rx = session
        .output_receiver()
        .expect("first call should return receiver");

    // Second call should return None (take-once semantics)
    assert!(session.output_receiver().is_none());

    // Collect output from the initial prompt
    let mut got_turn_complete = false;
    let mut collected_text = String::new();

    let timeout = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(output) = rx.recv().await {
            match output {
                AgentOutput::Delta(text) => {
                    collected_text.push_str(&text);
                    collected_text.push('\n');
                }
                AgentOutput::Message(text) => {
                    collected_text.push_str(&text);
                }
                AgentOutput::TurnComplete => {
                    got_turn_complete = true;
                    break;
                }
                AgentOutput::Error(e) => {
                    panic!("Unexpected error: {e}");
                }
                AgentOutput::Idle => break,
            }
        }
    })
    .await;

    assert!(timeout.is_ok(), "Timed out waiting for Gemini response");
    assert!(
        got_turn_complete,
        "Should receive TurnComplete after process exits"
    );
    // The initial prompt is the system prompt itself -- Gemini should produce some output
    // (even if it's just acknowledging the system prompt)
    println!(
        "Initial output ({} bytes):\n{collected_text}",
        collected_text.len()
    );

    // Now send a follow-up input (spawns a new process)
    session
        .send_input("What is 2 + 2? Reply with just the number.")
        .await
        .expect("send_input should succeed");

    // Collect output from the second turn
    let mut second_text = String::new();
    let mut got_second_complete = false;

    let timeout2 = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(output) = rx.recv().await {
            match output {
                AgentOutput::Delta(text) => {
                    second_text.push_str(&text);
                    second_text.push('\n');
                }
                AgentOutput::Message(text) => {
                    second_text.push_str(&text);
                }
                AgentOutput::TurnComplete => {
                    got_second_complete = true;
                    break;
                }
                AgentOutput::Error(e) => {
                    panic!("Unexpected error on second turn: {e}");
                }
                AgentOutput::Idle => break,
            }
        }
    })
    .await;

    assert!(
        timeout2.is_ok(),
        "Timed out waiting for second Gemini response"
    );
    assert!(
        got_second_complete,
        "Should receive TurnComplete for second turn"
    );
    println!("Second turn output:\n{second_text}");
    assert!(
        second_text.contains('4'),
        "Response should contain '4', got: {second_text}"
    );

    // Shutdown
    session.shutdown().await.expect("shutdown should succeed");
    assert!(!session.is_alive().await);
}

#[tokio::test]
#[ignore = "requires live Gemini CLI — run with --include-ignored"]
async fn gemini_via_orchestrator() {
    let backend = require_gemini();
    let dir = tempfile::tempdir().unwrap();

    let orch = agent_teams::orchestrator::TeamOrchestrator::builder()
        .teams_base(dir.path().join("teams"))
        .tasks_base(dir.path().join("tasks"))
        .with_gemini_cli(backend)
        .build()
        .unwrap();

    // Create team and spawn a Gemini teammate
    orch.create_team("gemini-team", Some("Gemini test"))
        .await
        .unwrap();

    let config = SpawnConfig {
        name: "reviewer".into(),
        prompt: "You are a code reviewer. Be concise.".into(),
        model: Some("gemini-2.5-flash".into()),
        cwd: None,
        max_turns: None,
        allowed_tools: vec![],
        permission_mode: None,
        reasoning_effort: None,
        env: Default::default(),
        memory_config: None,
        delegations: Vec::new(),
    };

    orch.spawn_teammate("gemini-team", config, BackendType::GeminiCli)
        .await
        .unwrap();

    assert!(orch.is_alive("gemini-team", "reviewer").await);

    // Take output receiver and drain initial prompt output
    let mut rx = orch
        .take_output_receiver("gemini-team", "reviewer")
        .await
        .unwrap()
        .expect("should get receiver");

    let timeout = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(output) = rx.recv().await {
            match output {
                AgentOutput::TurnComplete | AgentOutput::Idle => break,
                AgentOutput::Error(e) => panic!("Error: {e}"),
                _ => {}
            }
        }
    })
    .await;
    assert!(timeout.is_ok(), "Timed out on initial prompt");

    // Send a review request via orchestrator
    orch.send_input(
        "gemini-team",
        "reviewer",
        "Review this: fn add(a: i32, b: i32) -> i32 { a + b }",
    )
    .await
    .unwrap();

    let mut review_text = String::new();
    let timeout2 = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(output) = rx.recv().await {
            match output {
                AgentOutput::Delta(t) => {
                    review_text.push_str(&t);
                    review_text.push('\n');
                }
                AgentOutput::Message(t) => review_text.push_str(&t),
                AgentOutput::TurnComplete => break,
                AgentOutput::Error(e) => panic!("Error: {e}"),
                AgentOutput::Idle => break,
            }
        }
    })
    .await;

    assert!(timeout2.is_ok(), "Timed out on review");
    println!("Review output:\n{review_text}");
    assert!(!review_text.is_empty(), "Review should produce output");

    // Cleanup
    orch.shutdown_teammate("gemini-team", "reviewer")
        .await
        .unwrap();
    orch.delete_team("gemini-team").await.unwrap();
}
