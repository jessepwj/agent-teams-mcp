#!/usr/bin/env bash
set -euo pipefail

exec cargo run --release --bin team_mode_service -- --data-dir .agent-teams --project-root .
