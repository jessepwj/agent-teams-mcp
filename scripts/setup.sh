#!/usr/bin/env bash
# setup.sh — one-shot post-clone bootstrap for agent-teams-mcp.
#
# What this does (idempotent — safe to re-run):
#   1. Verifies prerequisites (cargo, node)
#   2. Builds release binary (durable HTTP Team Mode service)
#   3. Generates HTTP .mcp.json
#   4. Verifies binaries exist + smoke-runs cargo test --lib
#   5. Prints next steps
#
# Designed for portability: pure bash, no awk-with-CSVs, no jq.
# Tested on macOS / Linux / Git-Bash on Windows.
#
# Re-running after a code change is fine — cargo handles incremental.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# ---------- pretty printing (ASCII only — no Unicode for Windows safety) ----------
print_step()  { printf "\n[STEP] %s\n" "$1"; }
print_ok()    { printf "  [OK]   %s\n" "$1"; }
print_warn()  { printf "  [WARN] %s\n" "$1"; }
print_fail()  { printf "  [FAIL] %s\n" "$1" >&2; }

print_step "1/5 Checking prerequisites"

if ! command -v cargo >/dev/null 2>&1; then
  print_fail "cargo not found. Install Rust 1.85+ from https://rustup.rs/"
  exit 1
fi
print_ok "cargo: $(cargo --version)"

if ! command -v node >/dev/null 2>&1; then
  print_fail "node not found. The Stop hook is a node script. Install Node.js LTS (14+)."
  exit 1
fi
print_ok "node:  $(node --version)"

# ---------- detect OS for binary suffix ----------
case "$(uname -s 2>/dev/null || echo unknown)" in
  MINGW*|MSYS*|CYGWIN*) IS_WINDOWS=1; EXE_SUFFIX=".exe" ;;
  *)                    IS_WINDOWS=0; EXE_SUFFIX=""     ;;
esac

# ---------- build ----------
print_step "2/5 Building release binary (cargo build --release)"
cargo build --release --bin team_mode_service

SERVICE_BIN="$REPO_ROOT/target/release/team_mode_service${EXE_SUFFIX}"

[ -f "$SERVICE_BIN" ] && print_ok "$SERVICE_BIN" || { print_fail "missing: $SERVICE_BIN"; exit 1; }

# ---------- generate .mcp.json from template ----------
print_step "3/5 Generating HTTP .mcp.json"

TEMPLATE="$REPO_ROOT/.mcp.json.template"
TARGET="$REPO_ROOT/.mcp.json"
[ -f "$TEMPLATE" ] || { print_fail ".mcp.json.template missing — corrupt clone?"; exit 1; }

cp "$TEMPLATE" "$TARGET"
print_ok "Wrote $TARGET"
print_ok "  url = http://127.0.0.1:8786/mcp"

# ---------- test sweep ----------
print_step "4/5 Smoke-running cargo test --lib (300 tests, <2s)"
cargo test --lib --quiet 2>&1 | tail -3

# ---------- next steps ----------
print_step "5/5 Setup complete. Next steps:"
cat <<EOF

  1. From this directory, launch Claude Code:
       # Windows:
       powershell -ExecutionPolicy Bypass -File scripts/team-mode-service.ps1 start
       # macOS/Linux:
       target/release/team_mode_service --data-dir .agent-teams --project-root . &
       claude

  2. In CC, run:  /mcp
     You should see:  team-mode  connected

  3. Try a smoke test:
       team_create({"name":"smoke"})
       worker_add({"team":"smoke","name":"alice","adapter":"claude-code"})
       send_message({"team":"smoke","text":"@alice say hi"})
       (wait for reply to push automatically)
       team_delete({"name":"smoke"})

  IMPORTANT — after editing .mcp.json or .claude/settings.json (or
  re-running this script with a moved repo):
     You MUST fully restart Claude Code (kill all CC windows + relaunch).
     '/mcp reconnect' alone does NOT pick up hook or path changes.

  Read .plans/agent-teams-v2/docs/03-operations/usage-tips.md for the do's and don'ts.

EOF
