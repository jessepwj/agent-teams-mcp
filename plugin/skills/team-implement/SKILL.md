---
description: "Multi-phase implementation: @claude-architect → Codex parallel + @claude-worker → Gemini parallel → @claude-finalizer."
---

Run a multi-phase implementation. Claude teammates do design/finalize, Codex/Gemini run as parallel background tasks.

**Task**: $ARGUMENTS (if unclear, ask for details)

## Architecture

```
Phase 1 — Design (native Team):
  └── [T1] @claude-architect

Phase 2 — Implement (parallel, mixed):
  ├── [T2] Bash bg: Codex component A
  ├── [T3] Bash bg: Codex component B (if splittable)
  └── [T4] @claude-worker: implement parts needing Claude intelligence

Phase 3 — Review (parallel Bash):
  ├── [T5] Bash bg: Gemini quality review
  └── [T6] Bash bg: Gemini security review

Phase 4 — Finalize (native Team):
  └── [T7] @claude-finalizer
```

## Step-by-step

### 1. Check tools + scan project
```bash
which codex 2>/dev/null && echo "CODEX=true" || echo "CODEX=false"
which gemini 2>/dev/null && echo "GEMINI=true" || echo "GEMINI=false"
which agent-teams 2>/dev/null && echo "DAG_CLI=true" || echo "DAG_CLI=false"
```
Glob key dirs. Read 1-2 files max.

### 2. TeamCreate
`TeamCreate: team_name "impl-<short>"`

### 3. TaskCreate DAG
```
T1: "Design approach"                  (no deps)
T2: "Codex: implement component A"     (blockedBy: [T1])
T3: "Codex: implement component B"     (blockedBy: [T1]) — skip if not splittable
T4: "Claude: implement complex parts"  (blockedBy: [T1])
T5: "Gemini: quality review"           (blockedBy: [T2, T3, T4])
T6: "Gemini: security review"          (blockedBy: [T2, T3, T4])
T7: "Finalize + test"                  (blockedBy: [T5, T6])
```

### 3.5. DAG validation (if agent-teams CLI available)
```bash
agent-teams dag validate --team impl-<short>
agent-teams dag show --team impl-<short>
```

### 4. Phase 1 — @claude-architect
```
Task:
  subagent_type: "general-purpose"
  team_name: "impl-<short>"
  name: "claude-architect"
  prompt: |
    You are @claude-architect. Design implementation for: <TASK>
    Read relevant files: <PATHS>
    Deliver: approach, file plan (split into parallelizable components),
    step-by-step, interfaces, test plan.
    Identify which parts are mechanical (good for Codex) vs which need
    careful design decisions (good for Claude teammate).
    TaskUpdate T1 completed. SendMessage design to team lead.
```

### 5. Phase 2 — Implement (parallel, ONE message)

Based on architect's design, fire Codex background + Claude teammate in parallel:

**T2** (Bash, run_in_background):
```bash
codex exec "Implement component A: <DESIGN_EXCERPT>. Files: <PATHS>. Run build."
```

**T3** (Bash, run_in_background, if splittable):
```bash
codex exec "Implement component B: <DESIGN_EXCERPT>. Files: <PATHS>. Run build."
```

**T4 — @claude-worker** (Task, same message):
```
Task:
  subagent_type: "general-purpose"
  team_name: "impl-<short>"
  name: "claude-worker"
  prompt: |
    You are @claude-worker. Implement the complex parts from this design:
    <DESIGN_EXCERPT_FOR_COMPLEX_PARTS>
    These parts need careful handling: <WHY>
    Read files: <PATHS>, then implement using Edit/Write tools.
    Run build to verify. TaskUpdate T4 completed. SendMessage changes to team lead.
```

Poll TaskOutput for T2/T3. Receive SendMessage from @claude-worker for T4.

### 6. Phase 3 — Review (parallel Bash, ONE message)

**T5** (Bash, run_in_background):
```bash
gemini -m gemini-2.5-pro -y <<'EOF'
Review implementation quality. Design: <BRIEF>. Files: <MODIFIED_PATHS>
Check: design compliance, code quality, error handling, edge cases, tests.
EOF
```

**T6** (Bash, run_in_background):
```bash
gemini -m gemini-2.5-pro -y <<'EOF'
Security review. Files: <MODIFIED_PATHS>
Check: input validation, injection, resource leaks, unsafe, panic paths.
EOF
```

Poll TaskOutput, mark completed.

### 7. Phase 4 — @claude-finalizer
```
Task:
  subagent_type: "general-purpose"
  team_name: "impl-<short>"
  name: "claude-finalizer"
  prompt: |
    You are @claude-finalizer. Fix review findings:
    Quality: <T5_OUTPUT>
    Security: <T6_OUTPUT>
    Fix Critical + Warning issues. Run build + full test suite.
    TaskUpdate T7 completed. SendMessage status to team lead.
```

### 8. Report + cleanup
Summary: design, changes, findings, fixes, test status.
Shutdown all teammates. TeamDelete.
