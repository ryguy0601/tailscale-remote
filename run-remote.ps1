<#
.SYNOPSIS
    Launcher for Tailscale Remote on Windows (auto-builds if needed, supports Startup registration).
.PARAMETER InstallStartup
    Registers Tailscale Remote to run automatically on Windows logon (headless QR/CLI).
.PARAMETER UninstallStartup
    Removes the Windows startup registration.
#>
[CmdletBinding()]
param (
    [switch]$InstallStartup,
    [switch]$UninstallStartup,

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ExtraArgs
)

$ErrorActionPreference = "Stop"

$repoDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $repoDir

$StartupFolder = [Environment]::GetFolderPath("Startup")
$ShortcutPath = Join-Path $StartupFolder "TailscaleRemote.lnk"

if ($UninstallStartup) {
    if (Test-Path $ShortcutPath) {
        Remove-Item -Path $ShortcutPath -Force
        Write-Host "[OK] Removed Tailscale Remote from Windows Startup." -ForegroundColor Green
    } else {
        Write-Host "[*] No startup shortcut found to remove." -ForegroundColor Yellow
    }
    exit 0
}

if ($InstallStartup) {
    $WScriptShell = New-Object -ComObject WScript.Shell
    $Shortcut = $WScriptShell.CreateShortcut($ShortcutPath)
    $Shortcut.TargetPath = "powershell.exe"
    $Shortcut.Arguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$repoDir\run-remote.ps1`" --no-browser"
    $Shortcut.WorkingDirectory = $repoDir
    $Shortcut.WindowStyle = 7
    $Shortcut.Description = "Tailscale Remote (Phone Trackpad, Keyboard & Media Remote)"
    $Shortcut.Save()
    Write-Host "[OK] Tailscale Remote registered to run automatically on Windows Startup!" -ForegroundColor Green
    Write-Host "     Shortcut: $ShortcutPath" -ForegroundColor Cyan
    exit 0
}

$exe = Join-Path $repoDir "target\release\tailscale-remote.exe"
if (-not (Test-Path $exe)) {
    & "$repoDir\build.ps1"
}

& $exe @ExtraArgs
exit $LASTEXITCODE
