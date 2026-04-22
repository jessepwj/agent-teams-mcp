# Why `agent-teams` vs Claude Code Native Agent Teams

## Background

When exploring multi-provider AI agent collaboration (Claude + Codex + Gemini), there are two possible approaches:

1. **Claude Code Native Agent Teams** — use Claude Code's built-in `Task` tool to spawn teammates, trying to proxy Codex/Gemini through them
2. **`agent-teams` Rust Library** — use a dedicated orchestration layer with pluggable backends

This document explains why Approach 2 is the correct choice and why Approach 1 hits fundamental limitations.

---

## Approach 1: Claude Code Native Agent Teams (Not Viable)

### Architecture

Claude Code's native `Task` tool spawns teammates via `subagent_type` options (`general-purpose`, `Explore`, `Plan`, etc.). All of these are **Claude Code subprocesses** communicating through the Anthropic API.

### The Thinking Block Problem

When Claude's **extended thinking** is enabled, the API returns `thinking` and `redacted_thinking` content blocks in assistant messages. On subsequent turns, these blocks **must be returned to the API byte-for-byte identical**. The API performs cryptographic-level validation:

```
API Error: 400
{"type":"error","error":{"type":"invalid_request_error","message":"messages.3.content.2:
thinking or redacted_thinking blocks in the latest assistant message cannot be modified.
These blocks must remain as they were in the original response."}}
```

This error occurs at `messages.{N}.content.{M}` — the Nth message's Mth content block — when a proxy layer modifies thinking blocks during serialization/deserialization.

### Why Proxying Fails

If you try to make a Claude Code team member "act as" a Codex or Gemini agent:

1. **Protocol mismatch**: Claude API messages contain `thinking`/`redacted_thinking` blocks that have no equivalent in Codex's JSON-RPC or Gemini's stdin/stdout protocols
2. **Serialization corruption**: Any intermediate layer that parses and reconstructs messages risks modifying thinking blocks (whitespace trimming, encoding changes, field reordering)
3. **`redacted_thinking` is opaque**: These blocks are security-redacted by the model — you cannot inspect, modify, or reconstruct them
4. **No opt-out**: Extended thinking is integral to Claude Code's agent capability; disabling it degrades performance

### Common Failure Modes

| Cause | Symptom |
|-------|---------|
| Proxy strips `thinking` blocks | `thinking blocks cannot be modified` error |
| JSON serialization alters whitespace | Same error (byte-level mismatch) |
| Proxy reconstructs message with only `text` blocks | Loses thinking context, breaks conversation |
| Encoding normalization (Unicode NFC/NFD) | Subtle byte changes trigger validation |

---

## Approach 2: `agent-teams` Rust Library (Recommended)

### Architecture

```
┌──────────────────────────────────────────────────────────┐
│                    TeamOrchestrator                        │
│                                                           │
│  ┌───────────────┐  ┌───────────────┐  ┌───────────────┐ │
│  │ ClaudeCode    │  │ Codex         │  │ GeminiCli     │ │
│  │ Backend       │  │ Backend       │  │ Backend        │ │
│  │ (cc-sdk)      │  │ (JSON-RPC)    │  │ (one-shot CLI) │ │
│  └───────────────┘  └───────────────┘  └───────────────┘ │
│           │                  │                  │          │
│           ▼                  ▼                  ▼          │
│  Shared Task List + Inbox (file-system coordination)      │
└──────────────────────────────────────────────────────────┘
```

### Why It Works

1. **Each backend talks directly to its own provider** — no API proxying, no thinking block issues
2. **Unified `AgentBackend` + `AgentSession` trait abstraction** — the orchestrator doesn't care which provider runs underneath
3. **File-based coordination layer** — tasks and messages use JSON files under `~/.claude/teams/` and `~/.claude/tasks/`, a universal protocol all backends can read/write

### Backend Comparison

| Feature | ClaudeCode | Codex | GeminiCli |
|---------|-----------|-------|-----------|
| Protocol | `cc-sdk` InteractiveClient | `codex app-server` JSON-RPC | One-shot CLI subprocess |
| Multi-turn | Yes | Yes | No (new process per turn) |
| Streaming | Yes (via session task) | Yes (stdout line-by-line) | Yes (stdout line-by-line) |
| Best for | Complex reasoning, planning | Code implementation, testing | Quick review, analysis, Q&A |
| Cost tier | High (3) | Medium (2) | Low (1) |

### Intelligent Routing

The `BackendRouter` trait system automatically selects the optimal backend:

```rust
// Keyword-based: route by task description
let router = KeywordRouter::new(BackendType::ClaudeCode)
    .word_boundary(true)
    .rule("review", BackendType::GeminiCli)    // cheap, fast for review
    .rule("implement", BackendType::Codex)      // good at coding
    .rule("plan", BackendType::ClaudeCode);     // best reasoning

// Capability-based: route by cost/latency optimization
let router = CapabilityRouter::new()
    .require_multi_turn(true)  // exclude GeminiCli
    .cost_weight(0.7);         // prefer cheaper backends

// Composable: keyword rules first, capability fallback
let router = ChainRouter::new()
    .push(keyword_router)
    .push(capability_router);
```

### Multi-Provider Collaboration Example

From `examples/codex_gemini_review.rs` — a real parallel code review pipeline:

```rust
// Phase 1: Parallel review on different backends
let orch = TeamOrchestrator::builder()
    .with_codex(CodexBackend::with_path("/opt/homebrew/bin/codex"))
    .with_gemini_cli(GeminiCliBackend::with_path("/opt/homebrew/bin/gemini"))
    .build()?;

orch.create_team("review", Some("Codex+Gemini code review")).await?;

// Codex reviews low-level code quality
orch.spawn_teammate("review", codex_config, BackendType::Codex).await?;
// Gemini reviews architecture/design
orch.spawn_teammate("review", gemini_config, BackendType::GeminiCli).await?;

// Both run in parallel, results collected and synthesized
// Phase 2 task is DAG-blocked until both Phase 1 tasks complete
```

### Key Design Decisions

1. **Strategy Pattern + Abstract Factory**: `AgentBackend` is the factory, `AgentSession` is the strategy. Adding a new provider (e.g., Ollama, GPT) requires implementing just these two traits.

2. **File-system as universal bus**: Instead of requiring providers to understand each other's wire protocols, all coordination happens through JSON files on disk — the lowest common denominator.

3. **Channel-based output**: All backends emit `AgentOutput` events through `tokio::sync::mpsc` channels, providing a uniform streaming interface regardless of the underlying protocol differences.

4. **Liveness via AtomicBool**: Each session has an `alive` flag checked by the orchestrator. No locks needed — just an atomic load. This keeps `is_alive()` O(1) and safe to call from any context.

---

## Summary

| Aspect | Native Claude Teams | `agent-teams` Library |
|--------|--------------------|-----------------------|
| Multi-provider | No (Claude only) | Yes (Claude + Codex + Gemini) |
| Thinking block issue | Fatal blocker | Not applicable |
| Protocol handling | Single (Anthropic API) | Per-backend native protocol |
| Task coordination | Built-in (Claude-specific) | Universal (file-based) |
| Routing intelligence | None (manual subagent_type) | Keyword / Capability / Chain |
| Extensibility | Limited to Claude models | Any CLI-based AI tool |
