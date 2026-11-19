# PowerShell Build Script for Tailscale Remote
$ErrorActionPreference = "Stop"

$repoDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $repoDir

Write-Host "=== Stopping any running Tailscale Remote process before build ===" -ForegroundColor Yellow
Stop-Process -Name "tailscale-remote" -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 200

Write-Host "=== Building Tailscale Remote ===" -ForegroundColor Cyan
cargo build --release
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Write-Host "`nBuild complete! Executable available at:" -ForegroundColor Green
Write-Host " - target\release\tailscale-remote.exe"
