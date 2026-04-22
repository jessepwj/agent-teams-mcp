---
description: "5-agent parallel review: 2xGemini + 2xCodex + @claude-reviewer + @claude-synthesizer. DAG-tracked."
---

Run a 5+1 agent code review. 4 background CLI tasks + 1 Claude reviewer (all parallel), then Claude synthesizer.

**Targets**: $ARGUMENTS (if empty, ask the user)

## Architecture

```
Phase 1 — ALL PARALLEL:
  ├── [T1] Bash bg: Gemini architecture review
  ├── [T2] Bash bg: Gemini security review
  ├── [T3] Bash bg: Codex bug review
  ├── [T4] Bash bg: Codex test coverage review
  └── [T5] @claude-reviewer: Rust idioms, logic, domain-specific review  ← Claude agent

Phase 2 — BLOCKED BY [T1-T5]:
  └── [T6] @claude-synthesizer: merge 5 reports into final review
```

## Step-by-step

### 1. Check tools
```bash
which codex 2>/dev/null && echo "CODEX=true" || echo "CODEX=false"
which gemini 2>/dev/null && echo "GEMINI=true" || echo "GEMINI=false"
which agent-teams 2>/dev/null && echo "DAG_CLI=true" || echo "DAG_CLI=false"
```
Glob target files. Do NOT read contents — agents read their own.

### 2. TeamCreate
`TeamCreate: team_name "review-<short>"`

### 3. TaskCreate DAG (6 tasks)
```
T1: "Gemini: architecture review"       activeForm: "Reviewing architecture"
T2: "Gemini: security review"           activeForm: "Reviewing security"
T3: "Codex: bug review"                 activeForm: "Finding bugs"
T4: "Codex: test coverage review"       activeForm: "Reviewing tests"
T5: "Claude: idiom & domain review"     activeForm: "Reviewing Rust idioms"
T6: "Synthesize all findings"           activeForm: "Synthesizing review"

TaskUpdate T6 addBlockedBy: [T1, T2, T3, T4, T5]
```

### 3.5. DAG validation (if agent-teams CLI available)
```bash
agent-teams dag validate --team review-<short>
agent-teams dag show --team review-<short>
```

### 4. Phase 1 — Launch ALL 5 in parallel (ONE message)

Fire 4 Bash + 1 Task in a single message.

**T1 — Gemini architecture** (Bash, run_in_background):
```bash
gemini -m gemini-2.5-pro -y <<'EOF'
Review these files for architecture and design: <FILE_PATHS>
Analyze: separation of concerns, API design, patterns, coupling, naming.
Format: **[Critical/Warning/Suggestion]** `file:fn` — issue. Recommendation: fix.
EOF
```

**T2 — Gemini security** (Bash, run_in_background):
```bash
gemini -m gemini-2.5-pro -y <<'EOF'
Review these files for security and error handling: <FILE_PATHS>
Analyze: error handling, input validation, injection, resource leaks, panic paths.
Format: **[Critical/Warning/Suggestion]** `file:fn` — issue. Recommendation: fix.
EOF
```

**T3 — Codex bugs** (Bash, run_in_background):
```bash
codex exec "Review for bugs: <FILE_PATHS>. Focus: logic errors, off-by-one, null checks, type safety, concurrency. Format: **[Severity]** file:loc — issue. Fix: change."
```

**T4 — Codex tests** (Bash, run_in_background):
```bash
codex exec "Analyze test coverage: <FILE_PATHS>. Find: untested paths, missing edge cases, weak assertions. Format: **[Severity]** file:fn — gap. Suggested test: desc."
```

**T5 — @claude-reviewer** (Task, SAME message as Bash calls above):
```
Task:
  subagent_type: "general-purpose"
  team_name: "review-<short>"
  name: "claude-reviewer"
  prompt: |
    You are @claude-reviewer on team "review-<short>".

    Review these files: <FILE_PATHS>
    Read each file with the Read tool, then analyze:
    - Rust idioms: proper use of ownership, lifetimes, error handling patterns
    - Logic correctness: algorithm correctness, edge cases in business logic
    - Domain-specific: does the code follow domain best practices?
    - Code quality: unnecessary complexity, dead code, missing abstractions
    - Documentation: are public APIs documented? Are complex sections explained?

    Format each finding as:
    **[Critical/Warning/Suggestion]** `file:function` — issue. Recommendation: fix.

    TaskUpdate mark T5 completed.
    SendMessage your review to team lead, summary: "Claude review complete"
```

**Fallback**: If codex/gemini unavailable, replace T1-T4 with additional Task(general-purpose) teammates.

### 5. Collect Phase 1 results

- Poll `TaskOutput` (non-blocking) for T1-T4 background tasks
- Receive `SendMessage` from @claude-reviewer for T5
- `TaskUpdate` each as completed

### 6. Phase 2 — @claude-synthesizer (native Team)
```
Task:
  subagent_type: "general-purpose"
  team_name: "review-<short>"
  name: "claude-synthesizer"
  prompt: |
    You are @claude-synthesizer on team "review-<short>".

    Merge these 5 review reports into one consolidated review:
    ## Architecture (Gemini): <T1_OUTPUT>
    ## Security (Gemini): <T2_OUTPUT>
    ## Bugs (Codex): <T3_OUTPUT>
    ## Test Coverage (Codex): <T4_OUTPUT>
    ## Idioms & Logic (Claude): <T5_OUTPUT>

    1. De-duplicate across all 5 reports
    2. Prioritize: Critical → Warning → Suggestion
    3. Group by theme
    4. Executive summary with stats

    TaskUpdate T6 completed. SendMessage report to team lead, summary: "Synthesis complete"
```

### 7. Report + cleanup
Present report. Offer to fix issues.
Shutdown @claude-reviewer and @claude-synthesizer. TeamDelete.
