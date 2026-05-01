param(
  [Parameter(Position=0)]
  [ValidateSet('start','stop','restart','status','install-startup','uninstall-startup')]
  [string]$Action = 'status',
  [string]$HostName = '127.0.0.1',
  [int]$Port = 8786
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path "$PSScriptRoot\.."
$DataDir = Join-Path $RepoRoot ".agent-teams"
$RuntimeDir = Join-Path $DataDir "runtime"
$RuntimeInfo = Join-Path $RuntimeDir "http-mcp.json"
$ServiceProcessName = "team_mode_service"

function ServiceExe {
  if ($env:TEAM_MODE_SERVICE_EXE -and (Test-Path $env:TEAM_MODE_SERVICE_EXE)) {
    return (Resolve-Path $env:TEAM_MODE_SERVICE_EXE).Path
  }
  $ReleaseExe = Join-Path $RepoRoot "target\release\team_mode_service.exe"
  if (Test-Path $ReleaseExe) { return $ReleaseExe }
  $DebugExe = Join-Path $RepoRoot "target\debug\team_mode_service.exe"
  if (Test-Path $DebugExe) { return $DebugExe }
  throw "team_mode_service.exe not found; build it first"
}

function RuntimeJson {
  if (-not (Test-Path $RuntimeInfo)) { return $null }
  return Get-Content $RuntimeInfo -Raw | ConvertFrom-Json
}

function Test-IsTeamModeServiceProcessName($ProcessName) {
  if (-not $ProcessName) { return $false }
  return $ProcessName -eq $ServiceProcessName
}

function ServiceProcess($PidValue) {
  if (-not $PidValue) { return $false }
  $process = Get-Process -Id $PidValue -ErrorAction SilentlyContinue
  if (-not $process) { return $null }
  if (-not (Test-IsTeamModeServiceProcessName $process.ProcessName)) {
    return $null
  }
  return $process
}

function TestServiceHttp($Info) {
  if (-not $Info) { return $false }
  try {
    $token = Get-Content $Info.token_file -Raw
    $body = @{ jsonrpc='2.0'; id=1; method='initialize'; params=@{} } | ConvertTo-Json -Compress
    $headers = @{ Authorization = "Bearer $($token.Trim())" }
    Invoke-RestMethod -Method Post -Uri $Info.url -Headers $headers -Body $body -ContentType 'application/json' | Out-Null
    return $true
  } catch {
    return $false
  }
}

function StartService {
  New-Item -ItemType Directory -Force -Path $RuntimeDir | Out-Null
  $info = RuntimeJson
  if ($info -and $info.pid) {
    $process = ServiceProcess $info.pid
    if ($process -and (TestServiceHttp $info)) {
      Write-Host "running pid=$($info.pid) url=$($info.url)"
      return
    }
    Write-Host "stale runtime pid=$($info.pid); starting service"
  }
  $exe = ServiceExe
  $log = Join-Path $DataDir "team-mode-service.log"
  $args = @(
    '--data-dir', $DataDir,
    '--project-root', $RepoRoot,
    '--host', $HostName,
    '--port', "$Port"
  )
  Start-Process -FilePath $exe -ArgumentList $args -WorkingDirectory $RepoRoot -WindowStyle Hidden -RedirectStandardError $log
  Start-Sleep -Milliseconds 500
  StatusService
}

function StopService {
  $info = RuntimeJson
  if ($info -and (ServiceProcess $info.pid)) {
    if (-not (TestServiceHttp $info)) {
      Write-Host "refusing to stop pid=$($info.pid): authenticated service probe failed"
      exit 1
    }
    Stop-Process -Id $info.pid -Force
    Write-Host "stopped pid=$($info.pid)"
  } else {
    Write-Host "not running"
  }
}

function StatusService {
  $info = RuntimeJson
  if (-not $info) {
    Write-Host "not running: missing $RuntimeInfo"
    exit 1
  }
  if (-not (ServiceProcess $info.pid)) {
    Write-Host "not running: stale pid=$($info.pid)"
    exit 1
  }
  if (-not (TestServiceHttp $info)) {
    Write-Host "not running: service pid=$($info.pid) failed authenticated probe"
    exit 1
  }
  Write-Host "running pid=$($info.pid) url=$($info.url)"
}

if ($MyInvocation.InvocationName -ne '.') {
  switch ($Action) {
    'start' { StartService }
    'stop' { StopService }
    'restart' { StopService; StartService }
    'status' { StatusService }
    'install-startup' { Write-Host "startup install is not implemented yet; use start from your shell profile if needed" }
    'uninstall-startup' { Write-Host "startup uninstall is not implemented yet" }
  }
}
