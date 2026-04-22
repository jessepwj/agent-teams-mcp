//! Basic workflow example: team creation, task management, and messaging.
//!
//! This example uses NO real CLI backends — it demonstrates the pure
//! file-based team/task/messaging system that works standalone.
//!
//! Run with:
//!   cargo run --example basic_workflow

use agent_teams::models::{CreateTaskRequest, TaskStatus, TaskUpdate};
use agent_teams::orchestrator::TeamOrchestrator;

#[tokio::main]
async fn main() -> agent_teams::Result<()> {
    // Use a temp directory so we don't pollute ~/.claude
    let tmp = tempfile::tempdir().expect("failed to create temp dir");
    let teams_dir = tmp.path().join("teams");
    let tasks_dir = tmp.path().join("tasks");

    println!("=== Agent Teams Basic Workflow ===\n");
    println!("Working directory: {}\n", tmp.path().display());

    // 1. Build orchestrator (no backends needed for task/messaging)
    let orch = TeamOrchestrator::builder()
        .teams_base(&teams_dir)
        .tasks_base(&tasks_dir)
        .build()?;

    // 2. Create a team
    let team = orch.create_team("demo-project", Some("A demo team")).await?;
    println!("[Team] Created: {} — {:?}", team.team_name, team.description);

    // 3. Create tasks with dependencies
    let t1 = orch
        .create_task(
            "demo-project",
            CreateTaskRequest {
                subject: "Design the API schema".into(),
                description: Some("Define REST endpoints and data models".into()),
                active_form: Some("Designing API schema".into()),
                metadata: None,
            },
        )
        .await?;
    println!("[Task] #{} created: {}", t1.id, t1.subject);

    let t2 = orch
        .create_task(
            "demo-project",
            CreateTaskRequest {
                subject: "Implement API endpoints".into(),
                description: Some("Build handlers for all endpoints".into()),
                active_form: Some("Implementing API".into()),
                metadata: None,
            },
        )
        .await?;
    println!("[Task] #{} created: {}", t2.id, t2.subject);

    let t3 = orch
        .create_task(
            "demo-project",
            CreateTaskRequest {
                subject: "Write integration tests".into(),
                description: Some("Test all API endpoints".into()),
                active_form: Some("Writing tests".into()),
                metadata: None,
            },
        )
        .await?;
    println!("[Task] #{} created: {}", t3.id, t3.subject);

    // 4. Set up dependencies: t2 blocked by t1, t3 blocked by t2
    orch.update_task(
        "demo-project",
        &t2.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t1.id.clone()]),
            ..Default::default()
        },
    )
    .await?;

    orch.update_task(
        "demo-project",
        &t3.id,
        TaskUpdate {
            add_blocked_by: Some(vec![t2.id.clone()]),
            ..Default::default()
        },
    )
    .await?;
    println!("\n[Deps] #{} -> #{} -> #{}", t1.id, t2.id, t3.id);

    // 5. Check what's available
    let next = orch.get_next_available_task("demo-project").await?;
    println!(
        "\n[Queue] Next available task: #{} \"{}\"",
        next.as_ref().unwrap().id,
        next.as_ref().unwrap().subject
    );

    // 6. Work through the pipeline
    println!("\n--- Working through tasks ---\n");

    // Start and complete t1
    orch.update_task(
        "demo-project",
        &t1.id,
        TaskUpdate {
            status: Some(TaskStatus::InProgress),
            owner: Some("alice".into()),
            ..Default::default()
        },
    )
    .await?;
    println!("[t1] In progress (alice)");

    orch.update_task(
        "demo-project",
        &t1.id,
        TaskUpdate {
            status: Some(TaskStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    println!("[t1] Completed! t2 is now unblocked.");

    // t2 should now be available
    let next = orch.get_next_available_task("demo-project").await?;
    println!(
        "[Queue] Next: #{} \"{}\"",
        next.as_ref().unwrap().id,
        next.as_ref().unwrap().subject
    );

    // Start and complete t2
    orch.update_task(
        "demo-project",
        &t2.id,
        TaskUpdate {
            status: Some(TaskStatus::InProgress),
            owner: Some("bob".into()),
            ..Default::default()
        },
    )
    .await?;
    println!("[t2] In progress (bob)");

    orch.update_task(
        "demo-project",
        &t2.id,
        TaskUpdate {
            status: Some(TaskStatus::Completed),
            ..Default::default()
        },
    )
    .await?;
    println!("[t2] Completed! t3 is now unblocked.");

    // t3 should now be available
    let next = orch.get_next_available_task("demo-project").await?;
    println!(
        "[Queue] Next: #{} \"{}\"",
        next.as_ref().unwrap().id,
        next.as_ref().unwrap().subject
    );

    // 7. Send some messages
    println!("\n--- Messaging ---\n");

    orch.send_message("demo-project", "alice", "bob", "Great work on the API!")
        .await?;
    println!("[Msg] alice -> bob: \"Great work on the API!\"");

    orch.send_shutdown_request("demo-project", "lead", "bob", Some("All tasks done"))
        .await?;
    println!("[Msg] lead -> bob: shutdown request");

    // 8. List all tasks with final status
    println!("\n--- Final Task Status ---\n");
    let all_tasks = orch.list_tasks("demo-project", None).await?;
    for t in &all_tasks {
        println!(
            "  #{} [{}] {} (owner: {})",
            t.id,
            t.status,
            t.subject,
            t.owner.as_deref().unwrap_or("-")
        );
    }

    // 9. Show the generated JSON files
    println!("\n--- Generated Files ---\n");
    let team_config = std::fs::read_to_string(teams_dir.join("demo-project/config.json"))?;
    println!("config.json:\n{team_config}");

    let task1_json = std::fs::read_to_string(tasks_dir.join("demo-project/1.json"))?;
    println!("tasks/1.json:\n{task1_json}");

    // Cleanup
    orch.delete_team("demo-project").await?;
    println!("\n[Done] Team deleted. Temp dir will be cleaned up on exit.");

    Ok(())
}
