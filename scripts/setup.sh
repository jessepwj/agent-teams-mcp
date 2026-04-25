#!/usr/bin/env bash
# setup.sh — one-shot post-clone bootstrap for agent-teams-mcp.
#
# What this does (idempotent — safe to re-run):
#   1. Verifies prerequisites (cargo, node)
#   2. Builds release binaries (mcp relay + daemon)
#   3. POSIX: warns / suggests `export EXE_EXT=""` if the .mcp.json
#      default would break on this OS
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
print_step "2/5 Building release binaries (cargo build --release)"
cargo build --release --bin team_mode_mcp --bin team_mode_daemon

MCP_BIN="$REPO_ROOT/target/release/team_mode_mcp${EXE_SUFFIX}"
DAEMON_BIN="$REPO_ROOT/target/release/team_mode_daemon${EXE_SUFFIX}"

[ -f "$MCP_BIN"    ] && print_ok "$MCP_BIN"    || { print_fail "missing: $MCP_BIN"; exit 1; }
[ -f "$DAEMON_BIN" ] && print_ok "$DAEMON_BIN" || { print_fail "missing: $DAEMON_BIN"; exit 1; }

# ---------- POSIX EXE_EXT advice ----------
print_step "3/5 Verifying .mcp.json compatibility on this OS"

if [ "$IS_WINDOWS" = "0" ]; then
  if [ "${EXE_EXT:-}" != "" ]; then
    print_warn "EXE_EXT is currently '$EXE_EXT'. The .mcp.json default uses \${EXE_EXT:-.exe}."
    print_warn "POSIX users should run:  export EXE_EXT=\"\""
    print_warn "(Add to ~/.bashrc or ~/.zshrc to persist across shells.)"
  else
    print_warn "Detected POSIX OS. Before launching Claude Code, run:"
    print_warn "    export EXE_EXT=\"\""
    print_warn "(Otherwise .mcp.json's \${EXE_EXT:-.exe} will resolve to '.exe' and CC won't find the binary.)"
  fi
else
  print_ok "Windows detected — .mcp.json default works as-is."
fi

# ---------- test sweep ----------
print_step "4/5 Smoke-running cargo test --lib (300 tests, <2s)"
cargo test --lib --quiet 2>&1 | tail -3

# ---------- next steps ----------
print_step "5/5 Setup complete. Next steps:"
cat <<EOF

  1. From this directory, launch Claude Code:
       claude

  2. In CC, run:  /mcp
     You should see:  team-mode  connected

  3. Try a smoke test:
       team_create({"name":"smoke"})
       worker_add({"team":"smoke","name":"alice","adapter":"claude-code"})
       send_message({"team":"smoke","text":"@alice say hi"})
       (wait for reply to push automatically)
       team_delete({"name":"smoke"})

  IMPORTANT — after editing .mcp.json or .claude/settings.json:
     You MUST fully restart Claude Code (kill all CC windows + relaunch).
     '/mcp reconnect' alone does NOT pick up hook changes.

  Read docs/usage-tips.md for the do's and don'ts.

EOF
