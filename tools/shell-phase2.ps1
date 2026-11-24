#Requires -Version 5.1
<#
.SYNOPSIS
    Phase 2: WinRust becomes the real shell and the Windows pieces it replaces stay off.

.DESCRIPTION
    Phase 1 (explorer-edge.ps1) froze the stock Explorer and removed Edge, but the session still started
    explorer.exe as the logon shell and the overlay killed it later. This script switches to shell mode and
    blocks the shell components WinRust replaces:

      shell      : rs-shell.exe becomes this user's shell (HKCU Winlogon\Shell, the supported route), and the
                   "rs-overlay" logon task is disabled so two shells do not fight.
      components : Image File Execution Options entries ("cmd.exe /c exit", the same mechanism WinRust already
                   uses for taskmgr.exe and SearchHost.exe) for the Windows Start menu, the search UI, Widgets,
                   Cortana and the Action Center host. explorer.exe itself is NOT blocked: opening a folder,
                   "show in folder" and URL/Settings hand-offs still go through it.
      services   : WidgetService (if present) is disabled. WSearch is left as it is (already disabled by phase 1).

    Default mode is -DryRun: it prints the plan and changes nothing.
    -Apply writes the changes and records every original value in tools\state\shell-phase2.backup.json.
    -Restore puts the recorded values back and re-enables the overlay task.

    The shell change only applies at the next sign-in; the IFEO/service changes apply at once. Recovery from a
    black screen after signing in: Ctrl+Alt+Del -> Task Manager -> File > Run new task -> explorer.exe, then
    run `rs-shell.exe --disable` (or this script with -Restore) from an elevated PowerShell.

.PARAMETER DryRun
    Default. Show what would change, change nothing, write no state file.

.PARAMETER Apply
    Apply the changes. Requires an elevated (non-SYSTEM) PowerShell.

.PARAMETER Restore
    Revert everything recorded in the state file. Requires elevation.

.PARAMETER KeepShellExperienceHost
    Do not block ShellExperienceHost.exe (Action Center / flyouts). WinRust has its own volume, battery and
    notification surfaces, so blocking it is the default.

.PARAMETER NoPrompt
    Do not ask for confirmation before applying.

.EXAMPLE
    .\shell-phase2.ps1
    .\shell-phase2.ps1 -Apply
    .\shell-phase2.ps1 -Restore
#>
param(
    [switch]$DryRun,
    [switch]$Apply,
    [switch]$Restore,
    [switch]$KeepShellExperienceHost,
    [switch]$NoPrompt
)

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------- constants

$Shell        = 'C:\Program Files\rust-explorer\rs-shell.exe'
$StateDir     = Join-Path $PSScriptRoot 'state'
$StateFile    = Join-Path $StateDir 'shell-phase2.backup.json'
$HkcuWinlogon = 'HKCU:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
$IfeoRoot     = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options'
$IfeoBlock    = 'cmd.exe /c exit'

# What WinRust replaces. explorer.exe is deliberately absent (it stays available for hand-offs).
$Components = [ordered]@{
    'StartMenuExperienceHost.exe' = 'Windows Start menu'
    'SearchApp.exe'               = 'Windows search (legacy UI)'
    'SearchUI.exe'                = 'Windows search (Cortana UI)'
    'SearchHost.exe'              = 'Windows search host'
    'Widgets.exe'                 = 'Widgets board'
    'Cortana.exe'                 = 'Cortana'
}
if (-not $KeepShellExperienceHost) {
    $Components['ShellExperienceHost.exe'] = 'Action Center / system flyouts'
}
$Services = @('WidgetService')

# ---------------------------------------------------------------- helpers

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Read-ShellValue {
    if (-not (Test-Path $HkcuWinlogon)) { return $null }
    (Get-ItemProperty -Path $HkcuWinlogon -Name Shell -ErrorAction SilentlyContinue).Shell
}

function Read-Ifeo([string]$exe) {
    $k = Join-Path $IfeoRoot $exe
    if (-not (Test-Path $k)) { return $null }
    (Get-ItemProperty -Path $k -Name Debugger -ErrorAction SilentlyContinue).Debugger
}

function Read-TaskState([string]$name) {
    $t = Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
    if (-not $t) { return $null }
    [string]$t.State
}

function Read-Service([string]$name) {
    Get-Service -Name $name -ErrorAction SilentlyContinue
}

function Write-Step([string]$text) { Write-Host "  $text" }

# ---------------------------------------------------------------- planning

function Get-Plan {
    $plan = [ordered]@{ shell = @(); components = @(); services = @(); tasks = @() }

    $cur = Read-ShellValue
    $plan.shell += [pscustomobject]@{
        Action = 'set'; Target = 'HKCU Winlogon\Shell'; From = $cur; To = $Shell
        Why = 'rs-shell starts the bar, the desktop and the Run keys itself; Explorer never starts'
    }
    $overlay = Read-TaskState 'rs-overlay'
    if ($overlay -and $overlay -ne 'Disabled') {
        $plan.tasks += [pscustomobject]@{ Action = 'disable'; Target = 'rs-overlay'; From = $overlay; To = 'Disabled'; Why = 'the overlay and the shell would both start the bar' }
    }

    foreach ($exe in $Components.Keys) {
        $p = Read-Ifeo $exe
        $why = $Components[$exe]
        if ($null -eq $p) {
            $plan.components += [pscustomobject]@{ Action = 'block'; Target = $exe; From = '(none)'; To = $IfeoBlock; Why = $why }
        } elseif ($p -ne $IfeoBlock) {
            $plan.components += [pscustomobject]@{ Action = 'replace'; Target = $exe; From = $p; To = $IfeoBlock; Why = $why }
        }
    }

    foreach ($svc in $Services) {
        $s = Read-Service $svc
        if ($s -and $s.StartType -ne 'Disabled') {
            $plan.services += [pscustomobject]@{ Action = 'disable'; Target = $svc; From = [string]$s.StartType; To = 'Disabled'; Why = 'Widgets platform (WinRust has no widgets)' }
        }
    }

    $plan
}

function Write-Plan($plan) {
    Write-Host ''
    Write-Host 'SHELL' -ForegroundColor Cyan
    $plan.shell | Format-Table Action, Target, From, To, Why -AutoSize -Wrap | Out-Host
    if ($plan.tasks.Count) {
        Write-Host 'LOGON TASKS' -ForegroundColor Cyan
        $plan.tasks | Format-Table Action, Target, From, To, Why -AutoSize -Wrap | Out-Host
    } else {
        Write-Host 'LOGON TASKS  (nothing to change)' -ForegroundColor Cyan
    }
    Write-Host 'COMPONENTS (IFEO: the image cannot start)' -ForegroundColor Cyan
    if ($plan.components.Count) {
        $plan.components | Format-Table Action, Target, From, To, Why -AutoSize -Wrap | Out-Host
    } else {
        Write-Host '  (nothing to change)'
    }
    Write-Host 'SERVICES' -ForegroundColor Cyan
    if ($plan.services.Count) {
        $plan.services | Format-Table Action, Target, From, To, Why -AutoSize -Wrap | Out-Host
    } else {
        Write-Host '  (nothing to change)'
    }
    Write-Host ''
}

# ---------------------------------------------------------------- apply

function Save-State($state) {
    if (-not (Test-Path $StateDir)) { New-Item -ItemType Directory -Path $StateDir -Force | Out-Null }
    $state | ConvertTo-Json -Depth 8 | Set-Content -Path $StateFile -Encoding UTF8
}

function Invoke-Apply {
    $state = [ordered]@{
        when            = (Get-Date).ToString('o')
        user            = [Security.Principal.WindowsIdentity]::GetCurrent().Name
        shell           = @{ value = (Read-ShellValue) }
        replaceExplorer = (Get-ItemProperty 'HKCU:\Software\rs-shell' -Name ReplaceExplorer -ErrorAction SilentlyContinue).ReplaceExplorer
        tasks           = @{}
        components      = @{}
        services        = @{}
    }
    foreach ($n in @('rs-overlay', 'topbar')) {
        $state.tasks[$n] = (Read-TaskState $n)
    }
    foreach ($exe in $Components.Keys) {
        $state.components[$exe] = @{ debugger = (Read-Ifeo $exe) }
    }
    foreach ($svc in $Services) {
        $s = Read-Service $svc
        if ($s) { $state.services[$svc] = @{ start = [string]$s.StartType; status = [string]$s.Status } }
    }
    Save-State $state

    # 1. the shell (rs-shell writes HKCU Winlogon\Shell itself)
    Write-Step "running $Shell --enable"
    $out = & $Shell --enable 2>&1
    $out | ForEach-Object { Write-Step "  $_" }
    if ($LASTEXITCODE -ne 0) { throw "rs-shell --enable failed with exit code $LASTEXITCODE" }
    $now = Read-ShellValue
    if (-not ($now -and ($now -ieq $Shell))) { throw "HKCU Winlogon\Shell is '$now', expected '$Shell'" }
    Write-Step "shell = $now"

    # 2. one shell only
    if ((Read-TaskState 'rs-overlay') -eq 'Running') {
        Stop-ScheduledTask -TaskName 'rs-overlay' -ErrorAction SilentlyContinue
        Write-Step 'rs-overlay stopped'
    }
    Disable-ScheduledTask -TaskName 'rs-overlay' -ErrorAction SilentlyContinue | Out-Null
    Write-Step 'rs-overlay disabled (rs-shell starts the bar from now on)'

    # 3. components WinRust replaces
    foreach ($exe in $Components.Keys) {
        $k = Join-Path $IfeoRoot $exe
        if (-not (Test-Path $k)) { New-Item -Path $k -Force | Out-Null }
        New-ItemProperty -Path $k -Name Debugger -PropertyType String -Value $IfeoBlock -Force | Out-Null
        Write-Step "blocked $exe ($($Components[$exe]))"
        Get-Process -Name ([IO.Path]::GetFileNameWithoutExtension($exe)) -ErrorAction SilentlyContinue |
            Stop-Process -Force -ErrorAction SilentlyContinue
    }

    # 4. services
    foreach ($svc in $Services) {
        $s = Read-Service $svc
        if ($s) {
            if ($s.Status -eq 'Running') { Stop-Service -Name $svc -Force -ErrorAction SilentlyContinue }
            Set-Service -Name $svc -StartupType Disabled -ErrorAction SilentlyContinue
            Write-Step "disabled $svc"
        }
    }

    Write-Host ''
    Write-Host 'Applied. The shell changes at the next sign-in; the blocks are active now.' -ForegroundColor Green
    Write-Host 'Sign out and back in, then check: bar, desktop icons, Win+E, Start, tray.' -ForegroundColor Yellow
    Write-Host "Backup: $StateFile" -ForegroundColor DarkGray
    Write-Host 'If the session comes up empty: Ctrl+Alt+Del -> Task Manager -> Run new task -> explorer.exe,' -ForegroundColor DarkGray
    Write-Host 'then run this script with -Restore from an elevated PowerShell.' -ForegroundColor DarkGray
}

# ---------------------------------------------------------------- restore

function Invoke-Restore {
    if (-not (Test-Path $StateFile)) { throw "no state file at $StateFile" }
    $state = Get-Content -Raw -Path $StateFile | ConvertFrom-Json

    Write-Step "disabling rs-shell as the shell"
    & $Shell --disable | ForEach-Object { Write-Step "  $_" }

    if ($state.shell.value) {
        Set-ItemProperty -Path $HkcuWinlogon -Name Shell -Value $state.shell.value
        Write-Step "HKCU Shell restored to $($state.shell.value)"
    } else {
        Remove-ItemProperty -Path $HkcuWinlogon -Name Shell -ErrorAction SilentlyContinue
        Write-Step 'HKCU Shell removed (normal shell)'
    }

    foreach ($exe in $state.components.PSObject.Properties.Name) {
        $k = Join-Path $IfeoRoot $exe
        $prev = $state.components.$exe.debugger
        if ($prev) {
            New-ItemProperty -Path $k -Name Debugger -PropertyType String -Value $prev -Force | Out-Null
            Write-Step "restored $exe -> $prev"
        } else {
            Remove-ItemProperty -Path $k -Name Debugger -ErrorAction SilentlyContinue
            Write-Step "unblocked $exe"
        }
    }

    foreach ($svc in $state.services.PSObject.Properties.Name) {
        $prev = $state.services.$svc.start
        if ($prev) {
            Set-Service -Name $svc -StartupType $prev -ErrorAction SilentlyContinue
            Write-Step "service $svc -> $prev"
        }
    }

    foreach ($name in @('rs-overlay', 'topbar')) {
        $prev = $state.tasks.$name
        if ($prev -and $prev -ne 'Disabled') {
            Enable-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue | Out-Null
            Write-Step "task $name enabled"
        }
    }
    Start-ScheduledTask -TaskName 'rs-overlay' -ErrorAction SilentlyContinue
    Write-Step 'rs-overlay started (overlay mode again)'

    Write-Host ''
    Write-Host 'Restored. Sign out and back in to get explorer.exe as the shell again.' -ForegroundColor Green
}

# ---------------------------------------------------------------- main

if ($Restore) {
    if (-not (Test-Admin)) { throw 'run this from an elevated PowerShell' }
    Write-Host 'RESTORE' -ForegroundColor Cyan
    Invoke-Restore
    exit 0
}

if ($Apply -and $DryRun) { throw 'use either -Apply or -DryRun, not both' }

$plan = Get-Plan
Write-Plan $plan

if (-not $Apply) {
    Write-Host 'Dry run: nothing was changed. Run with -Apply to write it.' -ForegroundColor Yellow
    exit 0
}

if (-not (Test-Admin)) { throw 'run this from an elevated PowerShell' }
if (-not (Test-Path $Shell)) { throw "rs-shell.exe not found at $Shell" }

if (-not $NoPrompt) {
    $answer = Read-Host 'Apply the plan above? (yes/no)'
    if ($answer -notmatch '^(y|yes|s|si|sí)$') { Write-Host 'Cancelled.'; exit 1 }
}

Write-Host ''
Write-Host 'APPLY' -ForegroundColor Cyan
Invoke-Apply
