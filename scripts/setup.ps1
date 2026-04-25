# setup.ps1 — one-shot post-clone bootstrap for agent-teams-mcp (Windows / PowerShell).
#
# What this does (idempotent — safe to re-run):
#   1. Verifies prerequisites (cargo, node)
#   2. Builds release binaries (mcp relay + daemon)
#   3. Verifies binaries exist + smoke-runs cargo test --lib
#   4. Prints next steps
#
# Run from repo root:
#   powershell -ExecutionPolicy Bypass -File scripts\setup.ps1
#
# Re-running after a code change is fine — cargo handles incremental.

$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path "$PSScriptRoot\.."
Set-Location $RepoRoot

function Step($msg) { Write-Host "`n[STEP] $msg" -ForegroundColor Cyan }
function Ok($msg)   { Write-Host "  [OK]   $msg" -ForegroundColor Green }
function Warn($msg) { Write-Host "  [WARN] $msg" -ForegroundColor Yellow }
function Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; exit 1 }

Step "1/4 Checking prerequisites"

$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) { Fail "cargo not found. Install Rust 1.85+ from https://rustup.rs/" }
Ok "cargo: $(cargo --version)"

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) { Fail "node not found. The Stop hook is a node script. Install Node.js LTS (14+)." }
Ok "node:  $(node --version)"

Step "2/4 Building release binaries (cargo build --release)"
cargo build --release --bin team_mode_mcp --bin team_mode_daemon
if ($LASTEXITCODE -ne 0) { Fail "cargo build failed (exit $LASTEXITCODE)" }

$McpBin    = Join-Path $RepoRoot "target\release\team_mode_mcp.exe"
$DaemonBin = Join-Path $RepoRoot "target\release\team_mode_daemon.exe"

if (-not (Test-Path $McpBin))    { Fail "missing: $McpBin" }
if (-not (Test-Path $DaemonBin)) { Fail "missing: $DaemonBin" }
Ok $McpBin
Ok $DaemonBin

Step "3/4 Generating .mcp.json with absolute path"

$Template = Join-Path $RepoRoot ".mcp.json.template"
$Target   = Join-Path $RepoRoot ".mcp.json"
if (-not (Test-Path $Template)) { Fail ".mcp.json.template missing - corrupt clone?" }

# Use forward slashes in the JSON value: Windows spawn accepts them, AND
# JSON would otherwise turn '\a' (e.g. in 'E:\aigc\...') into BEL etc.
$McpBinForJson = $McpBin -replace '\\', '/'
(Get-Content $Template -Raw) -replace 'REPLACE_ME_WITH_ABS_PATH', $McpBinForJson | Set-Content -Path $Target -NoNewline -Encoding UTF8
Ok "Wrote $Target"
Ok "  command = $McpBinForJson"

Step "4/5 Smoke-running cargo test --lib (300 tests, <2s)"
cargo test --lib --quiet 2>&1 | Select-Object -Last 3
if ($LASTEXITCODE -ne 0) { Fail "cargo test failed (exit $LASTEXITCODE)" }

Step "5/5 Setup complete. Next steps:"
@"

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

  IMPORTANT — after editing .mcp.json or .claude\settings.json:
     You MUST fully restart Claude Code (kill all CC windows + relaunch).
     '/mcp reconnect' alone does NOT pick up hook changes.

  Read docs\usage-tips.md for the do's and don'ts.

"@
