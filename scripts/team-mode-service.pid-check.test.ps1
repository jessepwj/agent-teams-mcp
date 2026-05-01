$ErrorActionPreference = 'Stop'

. "$PSScriptRoot\team-mode-service.ps1"

if (-not (Test-IsTeamModeServiceProcessName 'team_mode_service')) {
  throw "expected team_mode_service to be trusted"
}

foreach ($name in @('powershell', 'pwsh', 'cmd', 'node', 'notepad')) {
  if (Test-IsTeamModeServiceProcessName $name) {
    throw "expected $name to be rejected as stale runtime PID"
  }
}

Write-Host "team-mode-service PID name checks passed"
