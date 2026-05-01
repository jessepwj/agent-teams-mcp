# Windows: source vcvars64.bat so worker cargo commands inherit MSVC LIB/INCLUDE/PATH.
# Then start the durable team-mode HTTP service from the same environment.

$ErrorActionPreference = "Stop"

$programFilesX86 = [Environment]::GetEnvironmentVariable("ProgramFiles(x86)")
if (-not $programFilesX86) {
    $programFilesX86 = $env:ProgramFiles
}
if (-not $programFilesX86) {
    $programFilesX86 = "C:\Program Files (x86)"
}

$vcvars = Join-Path $programFilesX86 "Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"

if (Test-Path $vcvars) {
    cmd /c "`"$vcvars`" && set" | ForEach-Object {
        if ($_ -match "^([^=]+)=(.*)$") {
            Set-Item -Path "env:$($matches[1])" -Value $matches[2]
        }
    }
    Write-Host "[OK] vcvars64 sourced"
} else {
    Write-Warning "[SKIP] vcvars64.bat not found at $vcvars - cargo on Windows MSVC workers may fail"
}

& cargo run --release --bin team_mode_service -- --data-dir ".agent-teams" --project-root "."
