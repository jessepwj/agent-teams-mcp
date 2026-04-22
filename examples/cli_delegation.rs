//! CLI Delegation example: Claude agents that delegate to Codex and Gemini.
//!
//! This example demonstrates how to configure Claude agents with CLI delegations
//! so they can shell out to Codex, Gemini, or custom tools via Bash.
//!
//! **Note**: This example only constructs configs and prints prompts — it does
//! NOT spawn real agent processes (no backend is registered).
//!
//! Run with:
//!   cargo run --example cli_delegation

use agent_teams::backend::delegation::{format_delegation_prompt, CliDelegation};
use agent_teams::SpawnConfig;

fn main() {
    println!("=== CLI Delegation Examples ===\n");

    // -----------------------------------------------------------------------
    // 1. Claude agent that delegates code generation to Codex
    // -----------------------------------------------------------------------
    let coder = SpawnConfig::builder("coder", "You are a Rust developer. Write clean, tested code.")
        .delegate(
            CliDelegation::codex()
                .with_description("Use for code generation and implementation tasks."),
        )
        .model("haiku")
        .build();

    println!("--- Coder Agent (Claude + Codex) ---");
    println!("Name: {}", coder.name);
    println!("Model: {:?}", coder.model);
    println!("Delegations: {}\n", coder.delegations.len());
    println!(
        "Generated delegation prompt:\n{}\n",
        format_delegation_prompt(&coder.delegations)
    );

    // -----------------------------------------------------------------------
    // 2. Claude agent that delegates analysis to Gemini
    // -----------------------------------------------------------------------
    let reviewer = SpawnConfig::builder("reviewer", "You are a code reviewer.")
        .delegate(
            CliDelegation::gemini("gemini-2.5-pro")
                .with_description("Use for architectural analysis and design review."),
        )
        .model("haiku")
        .build();

    println!("--- Reviewer Agent (Claude + Gemini) ---");
    println!("Name: {}", reviewer.name);
    println!("Model: {:?}", reviewer.model);
    println!("Delegations: {}\n", reviewer.delegations.len());
    println!(
        "Generated delegation prompt:\n{}\n",
        format_delegation_prompt(&reviewer.delegations)
    );

    // -----------------------------------------------------------------------
    // 3. Claude agent with multiple delegations (Codex + Gemini)
    // -----------------------------------------------------------------------
    let multi = SpawnConfig::builder("lead", "You coordinate code reviews and implementation.")
        .delegate(
            CliDelegation::codex()
                .with_description("Use for generating implementation code."),
        )
        .delegate(
            CliDelegation::gemini("gemini-2.5-pro")
                .with_description("Use for reviewing and analyzing code quality."),
        )
        .build();

    println!("--- Lead Agent (Claude + Codex + Gemini) ---");
    println!("Name: {}", multi.name);
    println!("Delegations: {}\n", multi.delegations.len());
    println!(
        "Generated delegation prompt:\n{}\n",
        format_delegation_prompt(&multi.delegations)
    );

    // -----------------------------------------------------------------------
    // 4. Custom CLI tool delegation
    // -----------------------------------------------------------------------
    let custom = SpawnConfig::builder("experimenter", "You try different AI tools.")
        .delegate(
            CliDelegation::custom("Aider", "aider")
                .with_usage_hint("aider --model gpt-4.1 --yes-always \"YOUR_PROMPT\"")
                .with_description("Use Aider for pair-programming style code changes."),
        )
        .build();

    println!("--- Custom Tool Agent (Claude + Aider) ---");
    println!("Name: {}", custom.name);
    println!("Delegations: {}\n", custom.delegations.len());
    println!(
        "Generated delegation prompt:\n{}\n",
        format_delegation_prompt(&custom.delegations)
    );

    // -----------------------------------------------------------------------
    // 5. Show how the full augmented prompt looks
    // -----------------------------------------------------------------------
    println!("=== Full Augmented Prompt Preview ===\n");
    let config = SpawnConfig::builder("demo-agent", "You are a helpful assistant.")
        .delegate(CliDelegation::codex())
        .delegate(CliDelegation::gemini("gemini-2.5-flash"))
        .build();

    let delegation_prompt = format_delegation_prompt(&config.delegations);
    let augmented = format!("{delegation_prompt}\n\n{}", config.prompt);
    println!("{augmented}");
}
