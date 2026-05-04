# task-v3.1-project-isolation

> Owner: backend-dev
> Status: in-progress
> Started: 2026-05-04

## Scope

Implement v3.1 project isolation for the durable HTTP Team Mode service.

## Steps

- [x] Step 1: Propagate `X-Team-Mode-Project-Root` through relay/hooks/HTTP context and scope MCP handlers to caller project data.
- [x] Step 2: Harden `team_create` conflict handling, same-owner rebind, archived revive, and `overwrite=true`.
- [x] Step 3: Make `team_delete` archive by default and keep permanent delete explicit.
- [ ] Step 4: Port service lead-watchdog to auto-archive dead-owner teams without exiting the service.
- [ ] Step 5: Return active + archived teams from project-scoped `team_list` and update docs/ADR.

## Validation Plan

- Per step: `cargo fmt`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, then commit.
- Final: `python scripts/run_ci.py` if step-level validation is green.
