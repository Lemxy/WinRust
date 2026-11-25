#Requires -Version 5.1
<#
.SYNOPSIS
    WinRust post-setup for the modified Tiny11 ISO: installs the apps and turns rs-shell into the shell.

.DESCRIPTION
    Runs from C:\WinRust\tools inside the freshly installed Windows.

    -Phase Machine : from SetupComplete.cmd (SYSTEM, after OOBE, before the first logon). Installs the apps
                     into Program Files, writes the shared ProgramData settings and applies the machine-level
                     provisioning (HKLM/IFEO) and the shell-phase2 machine blocks.
    -Phase User    : from FirstLogonCommands (the first admin user, elevated). Applies the per-user HKCU
                     provisioning and makes rs-shell the shell of that user, then reboots once so the next
                     logon starts WinRust.

    Everything is logged to C:\WinRust\autopost-<phase>.log. Revert with:
        C:\WinRust\tools\iso-provision.ps1 -Restore
        C:\WinRust\tools\shell-phase2.ps1 -Restore
#>
param(
    [ValidateSet('Machine', 'User')]
    [string]$Phase = 'Machine',
    [switch]$NoReboot
)

$ErrorActionPreference = 'Continue'
$root = Split-Path $PSScriptRoot -Parent          # C:\WinRust
$apps = Join-Path $root 'apps'
$tools = $PSScriptRoot
$log = Join-Path $root ("autopost-{0}.log" -f $Phase.ToLower())

function L([string]$m) {
    $line = "[{0}] [{1}] {2}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Phase, $m
    $line | Out-Host
    try { Add-Content -Path $log -Value $line -Encoding UTF8 } catch { }
}

function Run([string]$title, [scriptblock]$what) {
    L "=== $title ==="
    try { & $what 2>&1 | ForEach-Object { L "$_" } } catch { L "ERROR: $_" }
}

L "WinRust auto post-setup on $env:COMPUTERNAME, phase $Phase"
L "root=$root apps=$apps tools=$tools"

# --- shared settings (machine-wide) -------------------------------------------------------------
$pd = Join-Path $env:ProgramData 'WinRust'
New-Item -ItemType Directory $pd -Force | Out-Null
Set-Content (Join-Path $pd 'lang.txt') 'es' -Encoding ascii
Set-Content (Join-Path $pd 'dodge.txt') '1' -Encoding ascii
Set-Content (Join-Path $pd 'autohide.txt') '0' -Encoding ascii
if (-not (Test-Path (Join-Path $pd 'look.txt'))) {
    Set-Content (Join-Path $pd 'look.txt') "bar-monitor=-1`nbar-blur=1`n" -Encoding ascii
}
# lets rs-shell register ms-settings:/ms-screenclip: for the user at logon
New-Item -ItemType File (Join-Path $pd 'handlers-enabled') -Force | Out-Null
L "settings written to $pd (handlers-enabled marker included)"

if ($Phase -eq 'Machine') {
    # --- programs, each in its own folder under Program Files ----------------------------------
    $map = [ordered]@{
        'topbar.exe'         = 'topbar'
        'rust-explorer.exe'  = 'rust-explorer'
        'rs-shell.exe'       = 'rust-explorer'
        'rs-clip.exe'        = 'rust-explorer'
        'rs-notify.exe'      = 'rust-explorer'
        'rust-settings.exe'  = 'rust-settings'
        'rust-control.exe'   = 'rust-control'
        'rs-shot.exe'        = 'rs-shot'
        'rs-calc.exe'        = 'rs-calc'
        'rust-notepad.exe'   = 'rust-notepad'
        'rtaskmgr-win32.exe' = 'rtaskmgr'
    }
    foreach ($k in $map.Keys) {
        $src = Join-Path $apps $k
        if (-not (Test-Path $src)) { L "MISSING $src"; continue }
        $dir = Join-Path $env:ProgramFiles $map[$k]
        New-Item -ItemType Directory $dir -Force | Out-Null
        Copy-Item $src (Join-Path $dir $k) -Force
        L "installed $k -> $dir"
    }

    # --- machine-level provisioning and shell blocks (HKCU here is SYSTEM; the User phase fixes it)
    $prov = Join-Path $tools 'iso-provision.ps1'
    if (Test-Path $prov) {
        Run 'provisioning DryRun' { & powershell -NoProfile -ExecutionPolicy Bypass -File $prov -DryRun }
        Run 'provisioning Apply' { & powershell -NoProfile -ExecutionPolicy Bypass -File $prov -Apply -NoPrompt }
    } else { L "MISSING $prov" }

    $phase2 = Join-Path $tools 'shell-phase2.ps1'
    if (Test-Path $phase2) {
        Run 'shell phase 2 DryRun' { & powershell -NoProfile -ExecutionPolicy Bypass -File $phase2 }
        Run 'shell phase 2 Apply' { & powershell -NoProfile -ExecutionPolicy Bypass -File $phase2 -Apply -NoPrompt }
    } else { L "MISSING $phase2" }

    L 'Machine phase done.'
    exit 0
}

# --- User phase ---------------------------------------------------------------------------------
$prov = Join-Path $tools 'iso-provision.ps1'
if (Test-Path $prov) {
    Run 'user provisioning DryRun' { & powershell -NoProfile -ExecutionPolicy Bypass -File $prov -DryRun }
    Run 'user provisioning Apply' { & powershell -NoProfile -ExecutionPolicy Bypass -File $prov -Apply -NoPrompt }
} else { L "MISSING $prov" }

$phase2 = Join-Path $tools 'shell-phase2.ps1'
if (Test-Path $phase2) {
    Run 'user shell phase 2 DryRun' { & powershell -NoProfile -ExecutionPolicy Bypass -File $phase2 }
    Run 'user shell phase 2 Apply' { & powershell -NoProfile -ExecutionPolicy Bypass -File $phase2 -Apply -NoPrompt }
} else { L "MISSING $phase2" }

# Report the shell that the next logon will start.
$shell = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Winlogon' -Name Shell -ErrorAction SilentlyContinue).Shell
L "HKCU Winlogon Shell = $shell"
L 'User phase done.'

if (-not $NoReboot) {
    L 'Rebooting in 30 s so the next logon starts rs-shell (Explorer is still the shell of this session).'
    & shutdown /r /t 30 /c "WinRust instalado: reinicio para arrancar el shell de WinRust"
}
