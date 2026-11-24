#Requires -Version 5.1
<#
.SYNOPSIS
    Freezes the stock Windows Explorer shell and removes Microsoft Edge on a
    Windows 11 (26xxx) machine where WinRust (topbar.exe + rust-explorer.exe)
    is already the shell.

.DESCRIPTION
    Two independent blocks, both reversible and idempotent:

      Explorer : keeps explorer.exe from coming back as a shell/desktop, and can
                 optionally block the image outright through IFEO.
      Edge     : runs Edge's own uninstaller, removes the AppX/provisioned
                 package for all users, blocks EdgeUpdate install policies,
                 disables Edge's update tasks, moves Edge shortcuts to a backup
                 folder and optionally points the shortcut surface at another
                 browser.

    Default mode is -DryRun: it only prints the plan and changes nothing.
    -Apply   writes the changes and records every original value in a JSON file.
    -Restore puts the recorded values back. It does NOT reinstall Edge (that is
             not possible from a backup); the JSON keeps the Edge version and the
             exact command needed to reinstall.

.PARAMETER DryRun
    Default. Show what would change, change nothing, write no state file.

.PARAMETER Apply
    Apply the changes. Requires an elevated (non-SYSTEM) PowerShell.

.PARAMETER Restore
    Revert everything recorded in the state file. Requires elevation.

.PARAMETER ExplorerPolicy
    Minimal (default): NoDesktop, NoTrayContextMenu, NoViewContextMenu + AutoRestartShell=0.
    Strict           : the above plus NoFileMenu, NoRun, NoFind.
    None             : do not touch Explorer policies at all.

.PARAMETER HardFreeze
    Adds an IFEO "Debugger" entry for explorer.exe so the image cannot start at
    all. This is the same mechanism WinRust already uses for taskmgr.exe and
    SearchHost.exe, but it also breaks WinRust's own hand-offs that shell out to
    explorer.exe (opening URLs/Settings, "open file location"). Read the warning
    printed by -Apply before using it.

.PARAMETER NoShellStop
    Do not stop an already running shell explorer.exe.

.PARAMETER ShellCommand
    Optional: set HKLM\...\Winlogon\Shell to this command. Left untouched by
    default. With WinRust the supported route is `rs-shell.exe --enable`, which
    writes the per-user HKCU shell value and is undone by WinRust itself.

.PARAMETER SkipExplorer
    Skip the whole Explorer block.

.PARAMETER SkipEdge
    Skip the whole Edge block.

.PARAMETER DisableEdgeUpdateServices
    Also disable edgeupdate / edgeupdatem / MicrosoftEdgeElevationService.
    Stronger block, but it also stops WebView2 Runtime updates. Off by default.

.PARAMETER ReplaceWith
    Full path to another browser .exe. Removes Edge's shortcut surface, creates
    replacement shortcuts and sets the legacy machine-wide StartMenuInternet
    default. It cannot set the Windows 11 UserChoice hash; the script opens
    Settings > Default apps at the end so the user can confirm (unless -NoPrompt).

.PARAMETER RemoveEdgeUserData
    Move %LOCALAPPDATA%\Microsoft\Edge to the backup folder (it can be several
    GB). Off by default. Other users' profiles are not touched.

.PARAMETER NoPrompt
    Never open the Default apps settings page or ask questions.

.PARAMETER Force
    Skip confirmations and allow overwriting a foreign IFEO Debugger value.

.PARAMETER StatePath
    Where the backup JSON lives. Default: <script dir>\state\explorer-edge.backup.json

.EXAMPLE
    .\explorer-edge.ps1
    Prints the full plan (default dry run).

.EXAMPLE
    .\explorer-edge.ps1 -Apply
    Freezes Explorer (minimal policy set) and removes Edge, recording backups.

.EXAMPLE
    .\explorer-edge.ps1 -Apply -HardFreeze -DisableEdgeUpdateServices -ReplaceWith 'C:\Program Files\Mozilla Firefox\firefox.exe'

.EXAMPLE
    .\explorer-edge.ps1 -Restore

.NOTES
    Design notes / sources are listed at the bottom of this file.
    This script deliberately never touches:
      * WinRust files, processes, tasks (including WinRust-update) or HKCU\Software\rs-shell
      * Microsoft Edge WebView2 (files, AppX, EdgeUpdate GUID F3017226-...)
      * HKLM\...\Policies\System\DisableTaskMgr (it would block WinRust's rtaskmgr IFEO)
      * HKLM\...\Policies\Explorer\NoControlPanel (it would block WinRust's ms-settings hand-off)
      * User data, unless -RemoveEdgeUserData is passed (and then it is moved, not deleted)
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [switch]$DryRun,
    [switch]$Apply,
    [switch]$Restore,

    [ValidateSet('Minimal', 'Strict', 'None')]
    [string]$ExplorerPolicy = 'Minimal',

    [switch]$HardFreeze,
    [switch]$NoShellStop,
    [string]$ShellCommand,
    [switch]$SkipExplorer,

    [switch]$SkipEdge,
    [switch]$DisableEdgeUpdateServices,
    [string]$ReplaceWith,
    [switch]$RemoveEdgeUserData,

    [switch]$NoPrompt,
    [switch]$Force,
    [string]$StatePath
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# --------------------------------------------------------------------------- #
# Mode / script scope
# --------------------------------------------------------------------------- #

$selectedModes = @()
if ($DryRun)  { $selectedModes += 'DryRun' }
if ($Apply)   { $selectedModes += 'Apply' }
if ($Restore) { $selectedModes += 'Restore' }
if ($selectedModes.Count -gt 1) {
    throw 'Use only one of -DryRun, -Apply or -Restore.'
}

$script:Mode = if ($Restore) { 'Restore' } elseif ($Apply) { 'Apply' } else { 'DryRun' }
$script:DryRun = ($script:Mode -eq 'DryRun')
# -WhatIf must never write a state file or touch the registry.
if ($WhatIfPreference) { $script:DryRun = $true }
$script:Cmdlet = $PSCmdlet
$script:ExitCode = 0
$script:LogPath = $null
$script:State = $null
$script:BackupDir = $null

$script:ScriptDir = if ($PSScriptRoot) {
    $PSScriptRoot
} elseif ($MyInvocation.MyCommand.Path) {
    Split-Path -Parent $MyInvocation.MyCommand.Path
} else {
    'C:\winrust-build\tools'
}

if (-not $PSBoundParameters.ContainsKey('StatePath') -or [string]::IsNullOrWhiteSpace($StatePath)) {
    $StatePath = Join-Path $script:ScriptDir 'state\explorer-edge.backup.json'
}
$StatePath = [IO.Path]::GetFullPath($StatePath)

# --------------------------------------------------------------------------- #
# Constants
# --------------------------------------------------------------------------- #

$script:EdgeStableGuid = '56EB18F8-B008-4CBD-B6D2-8C97FE7E9062'
$script:WebViewGuid    = 'F3017226-FE2A-4295-8BDF-00C3A9A7E4C5'  # never touched
$script:PolicyKey      = 'HKLM\SOFTWARE\Policies\Microsoft\EdgeUpdate'
$script:WinlogonKey    = 'HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
$script:ExplorerPolKey = 'HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\Explorer'
$script:IfeoKey        = 'HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\explorer.exe'
$script:IfeoDebugger   = 'cmd.exe /c exit'   # same pattern WinRust uses for SearchHost.exe
$script:EdgeUpdateDevKey = 'HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdateDev'
$script:EdgeClientState  = "HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\ClientState\{$($script:EdgeStableGuid)}"
$script:EdgeUninstallKey = 'HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Microsoft Edge'

# --------------------------------------------------------------------------- #
# Logging
# --------------------------------------------------------------------------- #

function Write-Log {
    param(
        [Parameter(Mandatory = $true)][string]$Text,
        [ValidateSet('Info', 'Ok', 'Warn', 'Error', 'Action', 'Debug')][string]$Level = 'Info'
    )

    $color = switch ($Level) {
        'Ok'     { 'Green' }
        'Warn'   { 'Yellow' }
        'Error'  { 'Red' }
        'Action' { 'Cyan' }
        'Debug'  { 'DarkGray' }
        default  { 'Gray' }
    }
    $prefix = switch ($Level) {
        'Ok'     { '[ ok ]' }
        'Warn'   { '[warn]' }
        'Error'  { '[fail]' }
        'Action' { '[plan]' }
        'Debug'  { '[dbg ]' }
        default  { '[info]' }
    }

    Write-Host ("{0} {1}" -f $prefix, $Text) -ForegroundColor $color

    if (-not $script:DryRun -and $script:LogPath) {
        try {
            $line = '{0} {1,-6} {2}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Level.ToUpperInvariant(), $Text
            Add-Content -LiteralPath $script:LogPath -Value $line -Encoding UTF8
        } catch {
            # Logging must never break the run.
        }
    }
}

function Write-Section {
    param([string]$Title)
    Write-Host ''
    Write-Host ('=' * 78) -ForegroundColor DarkCyan
    Write-Host " $Title" -ForegroundColor White
    Write-Host ('=' * 78) -ForegroundColor DarkCyan
}

function Set-ExitCode {
    param([int]$Code)
    if ($Code -gt $script:ExitCode) { $script:ExitCode = $Code }
}

# --------------------------------------------------------------------------- #
# Pre-flight helpers
# --------------------------------------------------------------------------- #

function Test-IsAdmin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($id)
    return $principal.IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)
}

function Test-IsSystem {
    return ([Security.Principal.WindowsIdentity]::GetCurrent().User.Value -eq 'S-1-5-18')
}

function Get-OsInfo {
    $cv = Get-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -ErrorAction SilentlyContinue
    [pscustomobject]@{
        ProductName = $cv.ProductName
        DisplayVersion = $cv.DisplayVersion
        Build = [int]$cv.CurrentBuild
        Ubr = $cv.UBR
    }
}

function Test-RebootPending {
    return ((Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending') -or
            (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired'))
}

# --------------------------------------------------------------------------- #
# Registry helpers (raw .NET access, so absent keys are easy to detect)
# --------------------------------------------------------------------------- #

function Split-RegPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    $m = [regex]::Match($Path, '^(?<hive>HKLM|HKEY_LOCAL_MACHINE|HKCU|HKEY_CURRENT_USER|HKU|HKEY_USERS)[:\\](?<sub>.+)$', 'IgnoreCase')
    if (-not $m.Success) { throw "Unsupported registry path: $Path" }

    $hive = switch ($m.Groups['hive'].Value.ToUpperInvariant()) {
        'HKLM' { 'HKLM' }
        'HKEY_LOCAL_MACHINE' { 'HKLM' }
        'HKCU' { 'HKCU' }
        'HKEY_CURRENT_USER' { 'HKCU' }
        'HKU' { 'HKU' }
        'HKEY_USERS' { 'HKU' }
    }
    $sub = $m.Groups['sub'].Value.Trim('\')

    [pscustomobject]@{
        Hive = $hive
        Sub = $sub
        Canonical = "$hive\$sub"
    }
}

function Get-RegRoot {
    param([string]$Hive)
    switch ($Hive) {
        'HKLM' { return [Microsoft.Win32.Registry]::LocalMachine }
        'HKCU' { return [Microsoft.Win32.Registry]::CurrentUser }
        'HKU'  { return [Microsoft.Win32.Registry]::Users }
        default { throw "Unsupported hive: $Hive" }
    }
}

function Get-RegValueInfo {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$Name = ''
    )

    $split = Split-RegPath $Path
    $root = Get-RegRoot $split.Hive
    $key = $null
    try {
        $key = $root.OpenSubKey($split.Sub)
        if ($null -eq $key) {
            return [pscustomobject]@{ KeyExists = $false; Exists = $false; Kind = $null; Value = $null }
        }
        if (@($key.GetValueNames()) -notcontains $Name) {
            return [pscustomobject]@{ KeyExists = $true; Exists = $false; Kind = $null; Value = $null }
        }
        return [pscustomobject]@{
            KeyExists = $true
            Exists    = $true
            Kind      = $key.GetValueKind($Name).ToString()
            Value     = $key.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        }
    } finally {
        if ($key) { $key.Close() }
    }
}

function Test-RegValueEqual {
    param($Info, $Value, [string]$Kind)

    if (-not $Info.Exists) { return $false }
    if ($Info.Kind -ne $Kind) { return $false }
    if (($Info.Value -is [array]) -or ($Value -is [array])) {
        return ((@($Info.Value) -join "`n") -eq (@($Value) -join "`n"))
    }
    return ([string]$Info.Value -eq [string]$Value)
}

function ConvertTo-RegValue {
    param($Value, [string]$Kind)

    switch ($Kind) {
        'DWord'       { return [int]$Value }
        'QWord'       { return [int64]$Value }
        'MultiString' { return [string[]]@($Value) }
        'Binary'      { return [byte[]]@($Value) }
        default       { if ($null -eq $Value) { return '' } else { return [string]$Value } }
    }
}

function Save-State {
    if ($script:DryRun -or $null -eq $script:State) { return }

    $dir = Split-Path -Parent $StatePath
    if (-not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }

    $script:State.updatedAt = (Get-Date).ToString('o')
    $json = $script:State | ConvertTo-Json -Depth 12
    $tmp = "$StatePath.tmp"
    [IO.File]::WriteAllText($tmp, $json, (New-Object System.Text.UTF8Encoding($false)))
    Move-Item -LiteralPath $tmp -Destination $StatePath -Force
}

function Backup-RegValue {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$Name = ''
    )

    if ($script:DryRun) { return }

    $canonical = (Split-RegPath $Path).Canonical
    $existing = @($script:State.registry) | Where-Object { $_.path -eq $canonical -and $_.name -eq $Name } | Select-Object -First 1
    if ($existing) { return }

    $info = Get-RegValueInfo -Path $Path -Name $Name
    $entry = [ordered]@{
        path       = $canonical
        name       = $Name
        keyExisted = [bool]$info.KeyExists
        existed    = [bool]$info.Exists
        kind       = $info.Kind
        value      = $info.Value
    }
    [void]$script:State.registry.Add($entry)
    Save-State
}

function Set-RegValue {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$Name = '',
        [Parameter(Mandatory = $true)]$Value,
        [string]$Kind,
        [string]$Why = ''
    )

    $info = Get-RegValueInfo -Path $Path -Name $Name
    if (-not $Kind) {
        $Kind = if ($info.Exists) { $info.Kind } else { 'String' }
    }

    if ($info.Exists -and (Test-RegValueEqual -Info $info -Value $Value -Kind $Kind)) {
        Write-Log "already set: $Path\$Name = '$Value'" -Level Debug
        return
    }

    $target = "$Path\$Name"
    $reason = if ($Why) { " ($Why)" } else { '' }
    if ($script:DryRun) {
        Write-Log "WOULD SET $target = '$Value' [$Kind]$reason" -Level Action
        return
    }
    if (-not $script:Cmdlet.ShouldProcess($target, "Set registry value to '$Value' [$Kind].$reason")) { return }

    Backup-RegValue -Path $Path -Name $Name

    $split = Split-RegPath $Path
    $root = Get-RegRoot $split.Hive
    $key = $root.CreateSubKey($split.Sub)
    try {
        $key.SetValue($Name, (ConvertTo-RegValue -Value $Value -Kind $Kind), [Microsoft.Win32.RegistryValueKind]$Kind)
    } finally {
        $key.Close()
    }
    Write-Log "SET $target = '$Value' [$Kind]" -Level Ok
}

function Remove-RegValue {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$Name = '',
        [string]$Why = ''
    )

    $info = Get-RegValueInfo -Path $Path -Name $Name
    if (-not $info.Exists) { return }

    $target = "$Path\$Name"
    $reason = if ($Why) { " ($Why)" } else { '' }
    if ($script:DryRun) {
        Write-Log "WOULD DELETE $target (was '$($info.Value)')$reason" -Level Action
        return
    }
    if (-not $script:Cmdlet.ShouldProcess($target, "Delete registry value.$reason")) { return }

    Backup-RegValue -Path $Path -Name $Name

    $split = Split-RegPath $Path
    $root = Get-RegRoot $split.Hive
    $key = $root.OpenSubKey($split.Sub, $true)
    try {
        if ($key) { $key.DeleteValue($Name, $false) }
    } finally {
        if ($key) { $key.Close() }
    }
    Write-Log "DELETED $target" -Level Ok
}

function Remove-RegKeyIfEmpty {
    param([Parameter(Mandatory = $true)][string]$Path)

    $split = Split-RegPath $Path
    $root = Get-RegRoot $split.Hive
    $key = $root.OpenSubKey($split.Sub)
    if ($null -eq $key) { return }
    try {
        $empty = (@($key.GetValueNames()).Count -eq 0 -and @($key.GetSubKeyNames()).Count -eq 0)
    } finally {
        $key.Close()
    }
    if (-not $empty) { return }

    if ($script:DryRun) {
        Write-Log "WOULD DELETE empty key $Path" -Level Action
        return
    }
    if (-not $script:Cmdlet.ShouldProcess($Path, 'Delete registry key created by this script (now empty).')) { return }
    try { $root.DeleteSubKey($split.Sub, $false) } catch { }
    Write-Log "DELETED empty key $Path" -Level Ok
}

# --------------------------------------------------------------------------- #
# State file
# --------------------------------------------------------------------------- #

function ConvertTo-StateList {
    param($Value)
    $list = [System.Collections.ArrayList]@()
    foreach ($item in @($Value)) {
        if ($null -ne $item) { [void]$list.Add($item) }
    }
    return ,$list
}

function Initialize-State {
    $dir = Split-Path -Parent $StatePath
    if (-not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }

    if (Test-Path -LiteralPath $StatePath) {
        try {
            $obj = Get-Content -LiteralPath $StatePath -Raw | ConvertFrom-Json
        } catch {
            throw "State file is not valid JSON: $StatePath. Move it away and re-run. $_"
        }

        if ($obj.applied -ne $true) {
            $archive = '{0}.restored-{1}.json' -f $StatePath, (Get-Date -Format 'yyyyMMdd-HHmmss')
            Move-Item -LiteralPath $StatePath -Destination $archive -Force
            Write-Log "Previous (already restored) state archived to $archive" -Level Info
        } else {
            $script:State = [ordered]@{
                schemaVersion = 1
                createdAt     = $obj.createdAt
                updatedAt     = $obj.updatedAt
                applied       = [bool]$obj.applied
                restoredAt    = $obj.restoredAt
                backupDir     = $obj.backupDir
                machine       = $obj.machine
                winrust       = $obj.winrust
                edge          = $obj.edge
                notes         = ConvertTo-StateList $obj.notes
                registry      = ConvertTo-StateList $obj.registry
                services      = ConvertTo-StateList $obj.services
                tasks         = ConvertTo-StateList $obj.tasks
                files         = ConvertTo-StateList $obj.files
                created       = ConvertTo-StateList $obj.created
            }
            $script:BackupDir = $obj.backupDir
            Write-Log "Reusing existing backup: $StatePath" -Level Info
            return
        }
    }

    $winrust = Get-WinRustInfo
    $os = Get-OsInfo

    $script:State = [ordered]@{
        schemaVersion = 1
        createdAt     = (Get-Date).ToString('o')
        updatedAt     = (Get-Date).ToString('o')
        applied       = $true
        restoredAt    = $null
        backupDir     = $null
        machine       = [ordered]@{
            computer = $env:COMPUTERNAME
            user     = [Security.Principal.WindowsIdentity]::GetCurrent().Name
            build    = $os.Build
            ubr      = $os.Ubr
            version  = $os.DisplayVersion
            os       = $os.ProductName
            ps       = $PSVersionTable.PSVersion.ToString()
        }
        winrust       = $winrust
        edge          = [ordered]@{ detected = $null; version = $null; uninstalled = $null }
        notes         = [System.Collections.ArrayList]@()
        registry      = [System.Collections.ArrayList]@()
        services      = [System.Collections.ArrayList]@()
        tasks         = [System.Collections.ArrayList]@()
        files         = [System.Collections.ArrayList]@()
        created       = [System.Collections.ArrayList]@()
    }

    $script:BackupDir = Join-Path $dir ('backup\' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
    $script:State.backupDir = $script:BackupDir
    Save-State
}

function Get-SavedState {
    if (-not (Test-Path -LiteralPath $StatePath)) {
        throw "No state file at $StatePath. Nothing to restore."
    }
    $obj = Get-Content -LiteralPath $StatePath -Raw | ConvertFrom-Json

    $script:State = [ordered]@{
        schemaVersion = 1
        createdAt     = $obj.createdAt
        updatedAt     = $obj.updatedAt
        applied       = [bool]$obj.applied
        restoredAt    = $obj.restoredAt
        backupDir     = $obj.backupDir
        machine       = $obj.machine
        winrust       = $obj.winrust
        edge          = $obj.edge
        notes         = ConvertTo-StateList $obj.notes
        registry      = ConvertTo-StateList $obj.registry
        services      = ConvertTo-StateList $obj.services
        tasks         = ConvertTo-StateList $obj.tasks
        files         = ConvertTo-StateList $obj.files
        created       = ConvertTo-StateList $obj.created
    }
    $script:BackupDir = $obj.backupDir
}

# --------------------------------------------------------------------------- #
# WinRust detection / guards
# --------------------------------------------------------------------------- #

function Get-ProcessPathSafe {
    param([string]$Name)
    $p = Get-Process -Name $Name -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $p) { return $null }
    try { return $p.Path } catch { return $null }
}

function Get-WinRustInfo {
    $topbar = Get-ProcessPathSafe 'topbar'
    $rx     = Get-ProcessPathSafe 'rust-explorer'
    $rs     = Get-ProcessPathSafe 'rs-shell'

    $probe = @()
    if ($env:ProgramFiles) {
        $probe += (Join-Path $env:ProgramFiles 'topbar\topbar.exe')
        $probe += (Join-Path $env:ProgramFiles 'rust-explorer\rust-explorer.exe')
    }
    if (${env:ProgramFiles(x86)}) {
        $probe += (Join-Path ${env:ProgramFiles(x86)} 'topbar\topbar.exe')
    }
    $probe += 'C:\winrust-build\target\release\topbar.exe'
    $probe += 'C:\winrust-build\target\debug\topbar.exe'
    $probe = @($probe | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) })

    $overlay = $null
    try { $overlay = Get-ScheduledTask -TaskName 'rs-overlay' -ErrorAction SilentlyContinue } catch { }
    $updateTask = $null
    try { $updateTask = Get-ScheduledTask -TaskName 'WinRust-update' -ErrorAction SilentlyContinue } catch { }

    $replaceExplorer = $null
    try {
        $v = Get-ItemProperty -Path 'HKCU:\Software\rs-shell' -Name 'ReplaceExplorer' -ErrorAction SilentlyContinue
        if ($v) { $replaceExplorer = [int]$v.ReplaceExplorer }
    } catch { }

    [pscustomobject]@{
        detected          = [bool]($topbar -or $rx -or $rs -or $probe -or $overlay)
        topbar            = $topbar
        rustExplorer      = $rx
        rsShell           = $rs
        probedPaths       = @($probe)
        overlayTaskState  = if ($overlay) { $overlay.State.ToString() } else { $null }
        updateTaskState   = if ($updateTask) { $updateTask.State.ToString() } else { $null }
        replaceExplorer   = $replaceExplorer
        processes         = @(
            (Get-Process -Name topbar, 'rust-explorer', 'rs-shell' -ErrorAction SilentlyContinue |
                Select-Object Id, ProcessName)
        )
    }
}

function Test-WinRustPath {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { return $false }
    return ($Path -match '(?i)\\(topbar|rust-explorer|rs-shell)(\\|$)' -or
            $Path -match '(?i)\\winrust-build(\\|$)')
}

function Assert-NotWinRustPath {
    param([string]$Path, [string]$What)
    if (Test-WinRustPath $Path) {
        throw "Refusing to touch WinRust-owned path ($What): $Path"
    }
}

# --------------------------------------------------------------------------- #
# Processes
# --------------------------------------------------------------------------- #

function Get-ShellExplorerProcesses {
    # Mirrors WinRust's own test: explorer.exe with no path and no /factory host.
    $procs = Get-CimInstance Win32_Process -Filter "Name='explorer.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and ($_.CommandLine -match '(?i)explorer\.exe"?(\s+/NoUACCheck)?\s*$') }
    return @($procs)
}

# Read-only audit: Run/RunOnce values and Startup shortcuts that launch explorer.exe.
# Nothing is removed automatically - these entries may belong to the user.
function Get-ExplorerAutostart {
    $results = @()

    $keys = @(
        'HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run',
        'HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce',
        'HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run',
        'HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce'
    )
    foreach ($path in $keys) {
        $split = Split-RegPath $path
        $root = Get-RegRoot $split.Hive
        $key = $root.OpenSubKey($split.Sub)
        if (-not $key) { continue }
        try {
            foreach ($name in @($key.GetValueNames())) {
                $value = [string]$key.GetValue($name)
                if ($value -match '(?i)(^|\\)explorer\.exe(\s|$|")') {
                    $results += [pscustomobject]@{ Source = "$path\$name"; Command = $value }
                }
            }
        } finally {
            $key.Close()
        }
    }

    $startupDirs = @(
        (Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\StartUp'),
        (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\StartUp')
    )
    foreach ($dir in $startupDirs) {
        if (-not (Test-Path -LiteralPath $dir)) { continue }
        foreach ($lnk in (Get-ChildItem -LiteralPath $dir -Filter '*.lnk' -ErrorAction SilentlyContinue)) {
            $target = Get-ShortcutTarget -Path $lnk.FullName
            if ($target -and ($target -match '(?i)\\explorer\.exe$')) {
                $results += [pscustomobject]@{ Source = $lnk.FullName; Command = $target }
            }
        }
    }

    return @($results)
}

function Get-EdgeProcesses {    param($Edge)
    if (-not $Edge -or -not $Edge.Root) { return @() }

    $root = $Edge.Root
    return @(
        Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
            Where-Object {
                $_.ExecutablePath -and
                $_.ExecutablePath.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) -and
                $_.Name -notmatch '(?i)^msedgewebview2\.exe$' -and
                $_.ExecutablePath -notmatch '(?i)\\EBWebView\\'
            }
    )
}

function Stop-EdgeProcesses {
    param($Edge)

    $procs = Get-EdgeProcesses -Edge $Edge
    if (-not $procs -or $procs.Count -eq 0) {
        Write-Log 'No Edge browser processes running.' -Level Debug
        return
    }

    foreach ($p in $procs) {
        if ($script:DryRun) {
            Write-Log "WOULD STOP Edge process $($p.Name) (PID $($p.ProcessId))" -Level Action
            continue
        }
        if (-not $script:Cmdlet.ShouldProcess("$($p.Name) (PID $($p.ProcessId))", 'Stop Edge browser process')) { continue }
        try {
            Stop-Process -Id $p.ProcessId -Force -ErrorAction Stop
            Write-Log "Stopped $($p.Name) (PID $($p.ProcessId))" -Level Ok
        } catch {
            Write-Log "Could not stop $($p.Name) (PID $($p.ProcessId)): $($_.Exception.Message)" -Level Warn
        }
    }
}

# --------------------------------------------------------------------------- #
# Services / tasks
# --------------------------------------------------------------------------- #

function Get-ServiceStartMode {
    param([string]$Name)
    $info = Get-RegValueInfo -Path "HKLM\SYSTEM\CurrentControlSet\Services\$Name" -Name 'Start'
    if (-not $info.Exists) { return $null }
    return [int]$info.Value
}

function ConvertTo-ScStartName {
    param([int]$Start)
    switch ($Start) {
        0 { 'boot' }
        1 { 'system' }
        2 { 'auto' }
        3 { 'demand' }
        4 { 'disabled' }
        default { 'demand' }
    }
}

function Set-ServiceStartMode {
    param(
        [string]$Name,
        [int]$Start,
        [string]$Why
    )

    $current = Get-ServiceStartMode -Name $Name
    if ($null -eq $current) {
        Write-Log "Service $Name not present; skipping." -Level Debug
        return
    }
    if ($current -eq $Start) {
        Write-Log "Service $Name already at start mode $Start." -Level Debug
        return
    }

    if ($script:DryRun) {
        Write-Log "WOULD SET service $Name start=$Start ($(ConvertTo-ScStartName $Start)) ($Why)" -Level Action
        return
    }
    if (-not $script:Cmdlet.ShouldProcess("service $Name", "Set start mode to $(ConvertTo-ScStartName $Start). $Why")) { return }

    $existing = @($script:State.services) | Where-Object { $_.name -eq $Name } | Select-Object -First 1
    if (-not $existing) {
        $svc = Get-Service -Name $Name -ErrorAction SilentlyContinue
        [void]$script:State.services.Add([ordered]@{
            name        = $Name
            keyExisted  = $true
            start       = $current
            status      = if ($svc) { $svc.Status.ToString() } else { $null }
        })
        Save-State
    }

    if ($Start -eq 4) {
        Stop-Service -Name $Name -Force -ErrorAction SilentlyContinue
    }
    sc.exe config $Name start= (ConvertTo-ScStartName $Start) | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Log "sc.exe config $Name failed with exit code $LASTEXITCODE" -Level Warn
    } else {
        Write-Log "SET service $Name start=$Start ($(ConvertTo-ScStartName $Start))" -Level Ok
    }
}

function Get-EdgeUpdateTasks {
    return @(
        Get-ScheduledTask -ErrorAction SilentlyContinue |
            Where-Object { $_.TaskPath -like '\Microsoft\EdgeUpdate\*' }
    )
}

function Disable-EdgeTasks {
    $tasks = Get-EdgeUpdateTasks
    if ($tasks.Count -eq 0) {
        Write-Log 'No Microsoft\EdgeUpdate scheduled tasks found.' -Level Info
        return
    }

    foreach ($t in $tasks) {
        if (Test-WinRustPath $t.TaskName) {
            Write-Log "Skipping WinRust task $($t.TaskName)" -Level Warn
            continue
        }
        if ($t.State.ToString() -eq 'Disabled') {
            Write-Log "Task already disabled: $($t.TaskPath)$($t.TaskName)" -Level Debug
            continue
        }

        if ($script:DryRun) {
            Write-Log "WOULD DISABLE task $($t.TaskPath)$($t.TaskName) (currently $($t.State))" -Level Action
            continue
        }
        if (-not $script:Cmdlet.ShouldProcess("$($t.TaskPath)$($t.TaskName)", 'Disable Edge Update scheduled task')) { continue }

        $existing = @($script:State.tasks) | Where-Object { $_.path -eq $t.TaskPath -and $_.name -eq $t.TaskName } | Select-Object -First 1
        if (-not $existing) {
            [void]$script:State.tasks.Add([ordered]@{
                path  = $t.TaskPath
                name  = $t.TaskName
                state = $t.State.ToString()
            })
            Save-State
        }

        try {
            Disable-ScheduledTask -TaskPath $t.TaskPath -TaskName $t.TaskName -ErrorAction Stop | Out-Null
            Write-Log "DISABLED task $($t.TaskPath)$($t.TaskName)" -Level Ok
        } catch {
            Write-Log "Could not disable task $($t.TaskPath)$($t.TaskName): $($_.Exception.Message)" -Level Warn
        }
    }
}

# --------------------------------------------------------------------------- #
# Files / shortcuts
# --------------------------------------------------------------------------- #

function Move-ToBackup {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$Kind = 'file'
    )

    Assert-NotWinRustPath -Path $Path -What 'backup move'
    if (-not (Test-Path -LiteralPath $Path)) { return }

    if ($script:DryRun) {
        Write-Log "WOULD MOVE $Path -> <backup>\$Kind\..." -Level Action
        return
    }

    $existing = @($script:State.files) | Where-Object { $_.original -eq $Path } | Select-Object -First 1
    if ($existing) { return }

    $destDir = Join-Path $script:BackupDir $Kind
    $dest = Join-Path $destDir ('{0:D3}-{1}' -f (@($script:State.files).Count + 1), (Split-Path $Path -Leaf))

    if (-not $script:Cmdlet.ShouldProcess($Path, "Move to backup ($dest)")) { return }

    New-Item -ItemType Directory -Path $destDir -Force | Out-Null
    try {
        Move-Item -LiteralPath $Path -Destination $dest -Force -ErrorAction Stop
    } catch {
        Write-Log "Could not move $Path to backup: $($_.Exception.Message)" -Level Error
        Set-ExitCode 1
        return
    }
    [void]$script:State.files.Add([ordered]@{
        original = $Path
        backup   = $dest
        kind     = $Kind
    })
    Save-State
    Write-Log "MOVED $Path -> $dest" -Level Ok
}

function Get-ShortcutTarget {
    param([string]$Path)

    $shell = $null
    try {
        $shell = New-Object -ComObject WScript.Shell
        $lnk = $shell.CreateShortcut($Path)
        return $lnk.TargetPath
    } catch {
        return $null
    } finally {
        if ($shell) { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shell) }
    }
}

function Get-EdgeShortcuts {
    $candidates = @(
        (Join-Path $env:PUBLIC 'Desktop\Microsoft Edge.lnk'),
        (Join-Path $env:USERPROFILE 'Desktop\Microsoft Edge.lnk'),
        (Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\Microsoft Edge.lnk'),
        (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Microsoft Edge.lnk'),
        (Join-Path $env:APPDATA 'Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\Microsoft Edge.lnk'),
        (Join-Path $env:APPDATA 'Microsoft\Internet Explorer\Quick Launch\Microsoft Edge.lnk')
    )

    $found = @()
    foreach ($path in ($candidates | Select-Object -Unique)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
        $target = Get-ShortcutTarget -Path $path
        if ($target -and ($target -match '(?i)\\Edge\\Application\\(msedge|msedge_proxy)\.exe$')) {
            $found += $path
        } elseif (-not $target) {
            Write-Log "Shortcut has no readable target; leaving it alone: $path" -Level Warn
        } else {
            Write-Log "Shortcut not pointing at Edge; leaving it alone: $path -> $target" -Level Debug
        }
    }
    return @($found | Select-Object -Unique)
}

function New-ShortcutFile {
    param(
        [string]$Path,
        [string]$Target,
        [string]$Arguments = ''
    )

    if ($script:DryRun) {
        Write-Log "WOULD CREATE shortcut $Path -> $Target" -Level Action
        return
    }
    if (-not $script:Cmdlet.ShouldProcess($Path, "Create shortcut to $Target")) { return }

    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }

    $shell = $null
    try {
        $shell = New-Object -ComObject WScript.Shell
        $lnk = $shell.CreateShortcut($Path)
        $lnk.TargetPath = $Target
        if ($Arguments) { $lnk.Arguments = $Arguments }
        $lnk.Save()
    } catch {
        Write-Log "Could not create shortcut ${Path}: $($_.Exception.Message)" -Level Warn
        return
    } finally {
        if ($shell) { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shell) }
    }

    [void]$script:State.created.Add([ordered]@{ kind = 'shortcut'; path = $Path })
    Save-State
    Write-Log "CREATED shortcut $Path" -Level Ok
}

# --------------------------------------------------------------------------- #
# Edge detection
# --------------------------------------------------------------------------- #

function Get-EdgeInstall {
    $roots = @()
    if (${env:ProgramFiles(x86)}) { $roots += (Join-Path ${env:ProgramFiles(x86)} 'Microsoft\Edge\Application') }
    if ($env:ProgramFiles)        { $roots += (Join-Path $env:ProgramFiles 'Microsoft\Edge\Application') }
    if ($env:LOCALAPPDATA)        { $roots += (Join-Path $env:LOCALAPPDATA 'Microsoft\Edge\Application') }

    $found = @()
    foreach ($root in ($roots | Select-Object -Unique)) {
        if (-not (Test-Path -LiteralPath $root)) { continue }

        $dirs = Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' }

        foreach ($d in $dirs) {
            $setup = Join-Path $d.FullName 'Installer\setup.exe'
            $exe   = Join-Path $d.FullName 'msedge.exe'
            $found += [pscustomobject]@{
                Root        = $root
                Version     = [version]$d.Name
                VersionText = $d.Name
                Dir         = $d.FullName
                Setup       = if (Test-Path -LiteralPath $setup -PathType Leaf) { $setup } else { $null }
                Exe         = $exe
                ExeExists   = (Test-Path -LiteralPath $exe -PathType Leaf)
                Level       = if ($env:LOCALAPPDATA -and $d.FullName.StartsWith($env:LOCALAPPDATA, [StringComparison]::OrdinalIgnoreCase)) { 'user' } else { 'system' }
            }
        }
    }

    return @($found | Sort-Object Version -Descending)
}

function Get-EdgeAppX {
    $packages = @()
    $provisioned = @()

    try {
        $packages = @(Get-AppxPackage -AllUsers -Name 'Microsoft.MicrosoftEdge.Stable' -ErrorAction SilentlyContinue)
    } catch {
        Write-Log "Could not query Edge AppX: $($_.Exception.Message)" -Level Warn
    }
    try {
        $provisioned = @(
            Get-AppxProvisionedPackage -Online -ErrorAction SilentlyContinue |
                Where-Object { $_.DisplayName -eq 'Microsoft.MicrosoftEdge.Stable' }
        )
    } catch {
        Write-Log "Could not query provisioned Edge package: $($_.Exception.Message)" -Level Warn
    }

    [pscustomobject]@{
        Packages    = $packages
        Provisioned = $provisioned
    }
}

function Get-EdgeUninstallString {
    $info = Get-RegValueInfo -Path $script:EdgeUninstallKey -Name 'UninstallString'
    if ($info.Exists) { return [string]$info.Value }
    return $null
}

# --------------------------------------------------------------------------- #
# Explorer block
# --------------------------------------------------------------------------- #

function Set-ExplorerPolicies {
    $winrust = Get-WinRustInfo
    $trusted = ($winrust.detected -or $Force)

    if ($ExplorerPolicy -eq 'None') {
        Write-Log 'ExplorerPolicy=None: no Explorer policy values will be written.' -Level Info
    } elseif (-not $trusted) {
        Write-Log 'WinRust is not detected: skipping Explorer policies (NoDesktop without a replacement desktop means a blank screen). Use -Force to apply anyway.' -Level Warn
        Set-ExitCode 1
    } else {
        $values = [ordered]@{
            NoDesktop         = 1
            NoTrayContextMenu = 1
            NoViewContextMenu = 1
        }
        if ($ExplorerPolicy -eq 'Strict') {
            $values['NoFileMenu'] = 1
            $values['NoRun']      = 1
            $values['NoFind']     = 1
        }

        Write-Log "Applying Explorer policy set '$ExplorerPolicy' (explorer.exe only; WinRust reads none of these)." -Level Info
        foreach ($name in $values.Keys) {
            Set-RegValue -Path $script:ExplorerPolKey -Name $name -Value $values[$name] -Kind 'DWord' `
                -Why 'hide the stock Explorer desktop/menus if explorer.exe ever starts'
        }
    }

    if ($trusted) {
        # Windows should not resurrect explorer.exe after it is killed. WinRust usually
        # sets this itself; setting it again is idempotent and is recorded for -Restore.
        Set-RegValue -Path $script:WinlogonKey -Name 'AutoRestartShell' -Value 0 -Kind 'DWord' `
            -Why 'do not restart explorer.exe automatically'
    } else {
        Write-Log 'Skipping AutoRestartShell=0 as well (no WinRust shell detected).' -Level Warn
    }
}

function Stop-ShellExplorer {
    if ($NoShellStop) {
        Write-Log 'NoShellStop set: leaving any running shell explorer.exe alone.' -Level Info
        return
    }

    $shellProcs = Get-ShellExplorerProcesses
    if ($shellProcs.Count -eq 0) {
        Write-Log 'No shell explorer.exe is running.' -Level Info
        return
    }

    $winrust = Get-WinRustInfo
    $uiAlive = [bool]($winrust.topbar -or $winrust.rustExplorer -or
        (Get-Process -Name topbar, 'rust-explorer' -ErrorAction SilentlyContinue))

    if (-not $uiAlive -and -not $Force) {
        Write-Log 'Refusing to stop explorer.exe: topbar/rust-explorer is not running, the desktop would be empty. Use -Force only if you have another shell ready.' -Level Error
        Set-ExitCode 1
        return
    }

    foreach ($p in $shellProcs) {
        if ($script:DryRun) {
            Write-Log "WOULD STOP shell explorer.exe (PID $($p.ProcessId))" -Level Action
            continue
        }
        if (-not $script:Cmdlet.ShouldProcess("explorer.exe (PID $($p.ProcessId))", 'Stop the stock shell process (folder windows are not touched)')) { continue }

        try {
            Stop-Process -Id $p.ProcessId -Force -ErrorAction Stop
            Write-Log "Stopped shell explorer.exe (PID $($p.ProcessId))" -Level Ok
        } catch {
            Write-Log "Could not stop explorer.exe (PID $($p.ProcessId)): $($_.Exception.Message)" -Level Warn
        }
    }
}

function Set-ExplorerIfeo {
    if (-not $HardFreeze) {
        Write-Log 'HardFreeze not requested: explorer.exe can still be launched on demand.' -Level Info
        return
    }

    $winrust = Get-WinRustInfo
    if (-not $winrust.detected -and -not $Force) {
        Write-Log 'Refusing -HardFreeze: WinRust is not detected and blocking explorer.exe would leave no shell. Use -Force only with another shell ready.' -Level Error
        Set-ExitCode 1
        return
    }

    $info = Get-RegValueInfo -Path $script:IfeoKey -Name 'Debugger'
    if ($info.Exists -and ([string]$info.Value -ne $script:IfeoDebugger) -and -not $Force) {
        Write-Log "IFEO Debugger for explorer.exe already exists and is not ours: '$($info.Value)'. Refusing to overwrite (use -Force to replace it)." -Level Error
        Set-ExitCode 1
        return
    }

    Write-Log 'WARNING: HardFreeze makes every CreateProcess(explorer.exe) a no-op.' -Level Warn
    Write-Log '  * WinRust hands URLs, ms-settings: pages and some /select calls to explorer.exe; those will silently stop working.' -Level Warn
    Write-Log '  * EDR/AV may classify an IFEO Debugger on explorer.exe as T1546.012 (IFEO injection).' -Level Warn
    Write-Log '  * Settings > Apps, "Open file location" and similar shell hand-offs will fail while this is set.' -Level Warn

    Set-RegValue -Path $script:IfeoKey -Name 'Debugger' -Value $script:IfeoDebugger -Kind 'String' `
        -Why 'block explorer.exe from starting at all (same pattern as WinRust SearchHost/taskmgr)'
}

function Set-ExplorerShellCommand {
    if ([string]::IsNullOrWhiteSpace($ShellCommand)) {
        Write-Log 'Winlogon Shell left untouched (WinRust runs rs-shell --overlay and expects explorer.exe there).' -Level Info
        return
    }

    if (-not (Test-ShellCommand -Command $ShellCommand)) {
        Write-Log "ShellCommand does not resolve to an executable: $ShellCommand" -Level Error
        Set-ExitCode 2
        return
    }

    $winrust = Get-WinRustInfo
    if ($winrust.detected) {
        Write-Log 'WinRust detected: its supported shell switch is "rs-shell.exe --enable" (HKCU Winlogon\Shell).' -Level Warn
        Write-Log 'You passed -ShellCommand explicitly, so HKLM Winlogon\Shell will be changed.' -Level Warn
    }

    Set-RegValue -Path $script:WinlogonKey -Name 'Shell' -Value $ShellCommand -Kind 'String' `
        -Why 'replace the interactive shell command'
}

function Test-ShellCommand {
    param([string]$Command)

    $first = $Command.Trim()
    if ($first.StartsWith('"')) {
        $first = ($first.Substring(1) -split '"', 2)[0]
    } else {
        $first = ($first -split '\s+', 2)[0]
    }

    if ([IO.Path]::IsPathRooted($first)) {
        return (Test-Path -LiteralPath $first -PathType Leaf)
    }
    return [bool](Get-Command $first -ErrorAction SilentlyContinue)
}

# --------------------------------------------------------------------------- #
# Edge block
# --------------------------------------------------------------------------- #

function Set-EdgeUpdatePolicy {
    Write-Log 'Setting EdgeUpdate install blocks (documented as domain-joined only; best effort elsewhere).' -Level Info

    Set-RegValue -Path $script:PolicyKey -Name 'InstallDefault' -Value 0 -Kind 'DWord' `
        -Why 'block Microsoft Edge installation by default'
    Set-RegValue -Path $script:PolicyKey -Name "Install{$($script:EdgeStableGuid)}" -Value 0 -Kind 'DWord' `
        -Why 'block the Edge Stable channel installation'
    Set-RegValue -Path $script:PolicyKey -Name "Update{$($script:EdgeStableGuid)}" -Value 0 -Kind 'DWord' `
        -Why 'do not update Edge if it ever comes back'

    Write-Log 'WebView2 policy (Install/Update F3017226-...) intentionally left untouched.' -Level Debug
}

function Remove-EdgePackage {
    param([string]$Version)

    $appx = Get-EdgeAppX
    if ($appx.Packages.Count -eq 0 -and $appx.Provisioned.Count -eq 0) {
        Write-Log 'No Microsoft.MicrosoftEdge.Stable AppX/provisioned package found.' -Level Info
        return
    }

    foreach ($pkg in $appx.Packages) {
        $target = $pkg.PackageFullName
        if ($script:DryRun) {
            Write-Log "WOULD REMOVE AppX $target (all users)" -Level Action
            continue
        }
        if (-not $script:Cmdlet.ShouldProcess($target, 'Remove Edge AppX package for all users')) { continue }
        try {
            Remove-AppxPackage -Package $target -AllUsers -ErrorAction Stop
            Write-Log "REMOVED AppX $target" -Level Ok
        } catch {
            Write-Log "Could not remove AppX $target (Windows treats Edge as a system component): $($_.Exception.Message)" -Level Warn
        }
    }

    foreach ($prov in $appx.Provisioned) {
        $target = $prov.PackageName
        if ($script:DryRun) {
            Write-Log "WOULD REMOVE provisioned package $target" -Level Action
            continue
        }
        if (-not $script:Cmdlet.ShouldProcess($target, 'Remove provisioned Edge package')) { continue }
        try {
            Remove-AppxProvisionedPackage -Online -PackageName $target -ErrorAction Stop | Out-Null
            Write-Log "REMOVED provisioned package $target" -Level Ok
        } catch {
            Write-Log "Could not remove provisioned package ${target}: $($_.Exception.Message)" -Level Warn
        }
    }
}

function Invoke-EdgeSetupUninstall {
    param($Edge, [string]$Version)

    if (-not $Edge.Setup) {
        Write-Log "No setup.exe found for Edge $Version; cannot run the official uninstaller." -Level Warn
        return $false
    }

    # Edge's own setup refuses to uninstall unless these hints are present.
    Set-RegValue -Path $script:EdgeUpdateDevKey -Name 'AllowUninstall' -Value '' -Kind 'String' `
        -Why 'Edge setup checks this before allowing an uninstall'
    Remove-RegValue -Path $script:EdgeClientState -Name 'experiment_control_labels' `
        -Why 'experiment labels can make Edge refuse to uninstall'

    $level = if ($Edge.Level -eq 'user') { '--user-level' } else { '--system-level' }
    $attempts = @(
        @{
            Note = 'documented force uninstall'
            Args = @('--uninstall', $level, '--verbose-logging', '--force-uninstall')
        },
        @{
            Note = 'channel form used by EdgeRemover'
            Args = @('--uninstall', '--msedge', '--channel=stable', $level, '--verbose-logging', '--force-uninstall')
        },
        @{
            Note = 'uninstall string form'
            Args = @('--uninstall', '--msedge', '--channel=stable', $level, '--verbose-logging')
        }
    )

    foreach ($attempt in $attempts) {
        if (-not (Test-Path -LiteralPath $Edge.Exe -PathType Leaf)) { return $true }

        if ($script:DryRun) {
            Write-Log ("WOULD RUN: `"{0}`" {1}   ({2})" -f $Edge.Setup, ($attempt.Args -join ' '), $attempt.Note) -Level Action
            return $true
        }

        $desc = "$($Edge.Setup) $($attempt.Args -join ' ')"
        if (-not $script:Cmdlet.ShouldProcess($desc, "Uninstall Microsoft Edge $Version ($($attempt.Note))")) { continue }

        Write-Log "Running Edge uninstaller ($($attempt.Note))..." -Level Info
        try {
            $proc = Start-Process -FilePath $Edge.Setup -ArgumentList $attempt.Args `
                -WorkingDirectory (Split-Path -Parent $Edge.Setup) -WindowStyle Hidden -PassThru
            Wait-Process -Id $proc.Id -Timeout 300 -ErrorAction SilentlyContinue
            if (-not $proc.HasExited) {
                Write-Log 'setup.exe did not exit after 5 minutes (possible invisible prompt); stopping it.' -Level Warn
                Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
            } else {
                Write-Log "setup.exe exit code: $($proc.ExitCode)" -Level Info
                if ($proc.ExitCode -notin @(0, 19, 3010)) {
                    Write-Log "Uninstaller returned $($proc.ExitCode); will verify and try the next form." -Level Warn
                }
            }
        } catch {
            Write-Log "setup.exe failed: $($_.Exception.Message)" -Level Warn
        }

        # setup.exe may hand off to a child process and return early.
        for ($i = 0; $i -lt 60; $i++) {
            if (-not (Test-Path -LiteralPath $Edge.Exe -PathType Leaf)) { break }
            Start-Sleep -Seconds 1
        }
        if (-not (Test-Path -LiteralPath $Edge.Exe -PathType Leaf)) {
            Write-Log "Edge $Version removed." -Level Ok
            return $true
        }
    }

    return (-not (Test-Path -LiteralPath $Edge.Exe -PathType Leaf))
}

function Set-ReplacementBrowser {
    param([string]$ExePath)

    $full = [IO.Path]::GetFullPath($ExePath)
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) {
        Write-Log "ReplaceWith target does not exist: $full" -Level Error
        Set-ExitCode 2
        return
    }
    if ([IO.Path]::GetExtension($full) -ne '.exe') {
        Write-Log "ReplaceWith must point at an .exe: $full" -Level Error
        Set-ExitCode 2
        return
    }

    $sig = Get-AuthenticodeSignature -LiteralPath $full -ErrorAction SilentlyContinue
    if (-not $sig -or $sig.Status -ne 'Valid') {
        Write-Log "Replacement browser is not validly signed ($(if ($sig) { $sig.Status } else { 'unknown' })): $full" -Level Warn
    }

    $display = (Get-Item -LiteralPath $full).VersionInfo.FileDescription
    if ([string]::IsNullOrWhiteSpace($display)) {
        $display = [IO.Path]::GetFileNameWithoutExtension($full)
    }
    $safeDisplay = ($display -replace '[\\/:*?"<>|]', '-').Trim()
    if ([string]::IsNullOrWhiteSpace($safeDisplay)) { $safeDisplay = 'Browser' }

    # 1. Legacy machine-wide default. Windows 11 still asks the user for the real
    #    UserChoice hash, but this keeps the "default browser" surface coherent.
    $clientName = $null
    $smiRoot = 'HKLM\SOFTWARE\Clients\StartMenuInternet'
    if (Test-Path -LiteralPath 'HKLM:\SOFTWARE\Clients\StartMenuInternet') {
        foreach ($client in (Get-ChildItem -LiteralPath 'HKLM:\SOFTWARE\Clients\StartMenuInternet' -ErrorAction SilentlyContinue)) {
            $cmdInfo = Get-RegValueInfo -Path "HKLM\SOFTWARE\Clients\StartMenuInternet\$($client.PSChildName)\shell\open\command" -Name ''
            if ($cmdInfo.Exists -and ([string]$cmdInfo.Value -like "*$full*")) {
                $clientName = $client.PSChildName
                break
            }
        }
    }
    if ($clientName) {
        Set-RegValue -Path $smiRoot -Name '' -Value $clientName -Kind 'String' `
            -Why 'legacy machine-wide default browser hint'
    } else {
        Write-Log "No StartMenuInternet client registered for $full; only shortcuts will be created." -Level Info
    }

    # 2. Replace the shortcut surface Edge used to own.
    $shortcutTargets = @(
        (Join-Path $env:PUBLIC "Desktop\$safeDisplay.lnk"),
        (Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\$safeDisplay.lnk")
    )
    foreach ($target in $shortcutTargets) {
        if (Test-Path -LiteralPath $target) {
            Write-Log "Replacement shortcut already exists; leaving it: $target" -Level Debug
            continue
        }
        New-ShortcutFile -Path $target -Target $full
    }

    # 3. UserChoice cannot be written reliably (hash + tamper checks). Ask the user.
    if (-not $script:DryRun -and -not $NoPrompt) {
        Write-Log 'Opening Settings > Default apps. Pick the replacement browser for http/https/.htm/.html.' -Level Info
        if ($script:Cmdlet.ShouldProcess('ms-settings:defaultapps', 'Open Default apps settings')) {
            try { Start-Process 'ms-settings:defaultapps' | Out-Null } catch { }
        }
    }
}

function Invoke-EdgeBlock {
    Write-Section 'Microsoft Edge'

    $installs = @(Get-EdgeInstall)
    $installed = @($installs | Where-Object { $_.ExeExists })
    $appx = Get-EdgeAppX
    $uninstallString = Get-EdgeUninstallString
    $edge = if ($installed.Count -gt 0) { $installed[0] } elseif ($installs.Count -gt 0) { $installs[0] } else { $null }
    $present = ($installed.Count -gt 0 -or $appx.Packages.Count -gt 0 -or $appx.Provisioned.Count -gt 0)

    if (-not $present) {
        Write-Log 'Microsoft Edge is not installed (no msedge.exe, no AppX). Reinstall blocks will still be applied.' -Level Ok
    } else {
        Write-Log "Detected Edge version: $(if ($edge) { $edge.VersionText } else { 'unknown (AppX only)' })" -Level Info
        if ($uninstallString) { Write-Log "Registered uninstall string: $uninstallString" -Level Debug }

        if ($script:State -and $script:Mode -eq 'Apply' -and -not $script:DryRun) {
            $script:State.edge.detected = if ($edge) { $edge.VersionText } else { 'appx-only' }
            $script:State.edge.version = if ($edge) { $edge.VersionText } else { $null }
            Save-State
        }

        # Stop the browser before blocking or uninstalling anything.
        Stop-EdgeProcesses -Edge $edge
    }

    # Reinstall block: always applied, even if Edge is already gone.
    Set-EdgeUpdatePolicy
    Disable-EdgeTasks
    if ($DisableEdgeUpdateServices) {
        foreach ($svc in @('edgeupdate', 'edgeupdatem', 'MicrosoftEdgeElevationService')) {
            Set-ServiceStartMode -Name $svc -Start 4 -Why 'block Edge/EdgeUpdate from reinstalling the browser'
        }
    } else {
        Write-Log 'Edge Update services left alone (they also service WebView2). Use -DisableEdgeUpdateServices for a harder block.' -Level Info
    }

    if ($present) {
        if ($edge) {
            $removed = Invoke-EdgeSetupUninstall -Edge $edge -Version $edge.VersionText
            if (-not $removed) {
                Write-Log 'Edge is still present after all uninstall forms. See the risks section: on 24H2+ setup.exe may refuse when it is not launched by an approved parent process.' -Level Error
                Set-ExitCode 1
            }
        }

        Remove-EdgePackage -Version $(if ($edge) { $edge.VersionText } else { $null })
    }

    # Shortcuts are cleaned even if Edge was already uninstalled (dead links).
    $shortcuts = Get-EdgeShortcuts
    if ($shortcuts.Count -eq 0) {
        Write-Log 'No Microsoft Edge shortcuts found in the usual locations.' -Level Info
    } else {
        foreach ($lnk in $shortcuts) {
            Move-ToBackup -Path $lnk -Kind 'shortcuts'
        }
    }

    if ($RemoveEdgeUserData) {
        $userData = Join-Path $env:LOCALAPPDATA 'Microsoft\Edge'
        if (Test-Path -LiteralPath $userData) {
            Write-Log 'MoveEdgeUserData: moving the current user profile to the backup folder (this can take a while).' -Level Warn
            Move-ToBackup -Path $userData -Kind 'userdata'
        }
    } else {
        Write-Log 'Edge user data left in place (use -RemoveEdgeUserData to move it to the backup folder).' -Level Info
    }

    if ($ReplaceWith) {
        Set-ReplacementBrowser -ExePath $ReplaceWith
    }
}

# --------------------------------------------------------------------------- #
# Report
# --------------------------------------------------------------------------- #

function Show-Plan {
    Write-Section 'Pre-flight'
    Write-Log "Mode: $($script:Mode)" -Level Info

    $os = Get-OsInfo
    Write-Log "OS: $($os.ProductName) $($os.DisplayVersion) build $($os.Build).$($os.Ubr)" -Level Info
    if ($os.Build -lt 22000) {
        Write-Log "This script targets Windows 11 (22000+). Build $($os.Build) is untested." -Level Warn
    }

    $admin = Test-IsAdmin
    Write-Log "Elevated: $admin$(if (Test-IsSystem) { ' (running as SYSTEM - not recommended)' })" -Level Info

    $winrust = Get-WinRustInfo
    if ($winrust.detected) {
        Write-Log 'WinRust detected. It will not be modified.' -Level Ok
        Write-Log "  topbar          : $(if ($winrust.topbar) { $winrust.topbar } else { 'not running / unknown path' })" -Level Info
        Write-Log "  rust-explorer   : $(if ($winrust.rustExplorer) { $winrust.rustExplorer } else { 'not running / unknown path' })" -Level Info
        Write-Log "  rs-overlay task : $($winrust.overlayTaskState)" -Level Info
        Write-Log "  WinRust-update  : $($winrust.updateTaskState) (never touched)" -Level Info
        Write-Log "  ReplaceExplorer : $($winrust.replaceExplorer)" -Level Info
    } else {
        Write-Log 'WinRust not detected. Stopping explorer.exe is refused unless -Force is used.' -Level Warn
    }

    $shell = Get-RegValueInfo -Path $script:WinlogonKey -Name 'Shell'
    $auto  = Get-RegValueInfo -Path $script:WinlogonKey -Name 'AutoRestartShell'
    Write-Log "Winlogon Shell = '$($shell.Value)' ; AutoRestartShell = '$($auto.Value)'" -Level Info

    $shellProcs = Get-ShellExplorerProcesses
    Write-Log "Running shell explorer.exe processes: $($shellProcs.Count)" -Level Info

    $userinit = Get-RegValueInfo -Path $script:WinlogonKey -Name 'Userinit'
    if ($userinit.Exists -and ([string]$userinit.Value -match '(?i)explorer\.exe')) {
        Write-Log "Winlogon Userinit launches explorer.exe: '$($userinit.Value)' (left untouched; review it manually)." -Level Warn
    }

    $autostart = Get-ExplorerAutostart
    if ($autostart.Count -eq 0) {
        Write-Log 'No Run/RunOnce/Startup entry launches explorer.exe.' -Level Info
    } else {
        foreach ($entry in $autostart) {
            Write-Log "Autostart entry launches explorer.exe (left untouched): $($entry.Source) -> $($entry.Command)" -Level Warn
        }
    }

    if (Test-RebootPending) {
        Write-Log 'A reboot is pending. Apply first, then reboot before judging the result.' -Level Warn
    }

    if (-not $SkipEdge) {
        $installs = Get-EdgeInstall
        $appx = Get-EdgeAppX
        $installed = @($installs | Where-Object { $_.ExeExists })
        if ($installed.Count -gt 0) {
            Write-Log "Edge installed: $($installed[0].VersionText) at $($installed[0].Dir)" -Level Info
        } elseif ($appx.Packages.Count -gt 0) {
            Write-Log 'Edge: no msedge.exe, but the AppX package is still registered.' -Level Warn
        } else {
            Write-Log 'Edge: not present.' -Level Ok
        }
        Write-Log "Edge AppX packages: $($appx.Packages.Count); provisioned: $($appx.Provisioned.Count)" -Level Info

        $pinfo = Get-RegValueInfo -Path $script:PolicyKey -Name 'InstallDefault'
        Write-Log "EdgeUpdate InstallDefault currently: $(if ($pinfo.Exists) { $pinfo.Value } else { '<not set>' })" -Level Info

        $orchestrator = 'HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Orchestrator\UScheduler\EdgeUpdate'
        if (Test-Path -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Orchestrator\UScheduler\EdgeUpdate') {
            Write-Log "Windows Update still has an EdgeUpdate orchestrator entry ($orchestrator); a feature update may reinstall Edge. Left untouched." -Level Warn
        }
    }
}

function Show-Summary {
    Write-Section 'Result'

    if ($script:DryRun) {
        if ($script:Mode -eq 'Restore') {
            Write-Log 'Dry run / -WhatIf: nothing was restored.' -Level Ok
        } else {
            Write-Log 'Dry run only. Nothing was changed. Re-run with -Apply to apply.' -Level Ok
            Write-Log "Backup would be written to: $StatePath" -Level Info
        }
        return
    }

    if ($script:Mode -eq 'Restore') {
        Write-Log "Restored from: $StatePath" -Level Ok
        Write-Log 'Edge was not reinstalled (the uninstaller cannot be undone). Reinstall it from https://www.microsoft.com/edge if needed.' -Level Info
        return
    }

    Write-Log "Backup written to: $StatePath" -Level Ok
    if ($script:BackupDir) { Write-Log "Moved files are under: $($script:BackupDir)" -Level Info }

    $shellProcs = Get-ShellExplorerProcesses
    Write-Log "Shell explorer.exe processes now: $($shellProcs.Count)" -Level Info

    if (-not $SkipEdge) {
        $left = @(Get-EdgeInstall | Where-Object { $_.ExeExists })
        if ($left.Count -gt 0) {
            Write-Log "Edge still present: $($left[0].VersionText). Check the log and the exit code." -Level Warn
        } else {
            Write-Log 'msedge.exe not present.' -Level Ok
        }
    }

    Write-Log 'Sign out and back in (or reboot) to confirm the shell comes up without Explorer.' -Level Info
    Write-Log 'Recovery: Ctrl+Alt+Del > Task Manager > Run new task > explorer.exe' -Level Info
}

# --------------------------------------------------------------------------- #
# Restore
# --------------------------------------------------------------------------- #

function Invoke-Restore {
    Write-Section 'Restore'
    Get-SavedState

    if ($script:State.machine -and $script:State.machine.computer -and
        $script:State.machine.computer -ne $env:COMPUTERNAME) {
        Write-Log "State was created on '$($script:State.machine.computer)', this is '$env:COMPUTERNAME'. Restoring foreign backups is not recommended." -Level Warn
        if (-not $Force) {
            throw 'Refusing to restore a backup from another computer (use -Force if you are sure).'
        }
    }

    if ($script:State.applied -ne $true) {
        Write-Log 'This state file is already marked as restored. Re-applying it is safe (idempotent).' -Level Info
    }

    # 1. Registry values, reverse order.
    $registry = @($script:State.registry)
    [array]::Reverse($registry)
    foreach ($e in $registry) {
        $target = "$($e.path)\$($e.name)"
        if ($e.existed) {
            if ($script:DryRun) {
                Write-Log "WOULD RESTORE $target = '$($e.value)' [$($e.kind)]" -Level Action
            } elseif ($script:Cmdlet.ShouldProcess($target, "Restore original value '$($e.value)' [$($e.kind)]")) {
                $split = Split-RegPath $e.path
                $root = Get-RegRoot $split.Hive
                $key = $root.CreateSubKey($split.Sub)
                try {
                    $key.SetValue($e.name, (ConvertTo-RegValue -Value $e.value -Kind $e.kind), [Microsoft.Win32.RegistryValueKind]$e.kind)
                } finally {
                    $key.Close()
                }
                Write-Log "RESTORED $target = '$($e.value)'" -Level Ok
            }
        } else {
            if ($script:DryRun) {
                Write-Log "WOULD DELETE $target (did not exist before)" -Level Action
            } elseif ($script:Cmdlet.ShouldProcess($target, 'Delete value that did not exist before -Apply')) {
                $split = Split-RegPath $e.path
                $root = Get-RegRoot $split.Hive
                $key = $root.OpenSubKey($split.Sub, $true)
                try { if ($key) { $key.DeleteValue($e.name, $false) } } finally { if ($key) { $key.Close() } }
                Write-Log "DELETED $target" -Level Ok
            }
        }
    }

    # Remove keys this script created, but only if they are empty now.
    $createdKeys = @($registry | Where-Object { $_.keyExisted -eq $false } | ForEach-Object { $_.path } | Select-Object -Unique)
    foreach ($k in $createdKeys) { Remove-RegKeyIfEmpty -Path $k }

    # 2. Services.
    foreach ($s in @($script:State.services)) {
        if ($null -eq $s.start) { continue }
        if ($script:DryRun) {
            Write-Log "WOULD RESTORE service $($s.name) start=$($s.start)" -Level Action
            continue
        }
        if (-not $script:Cmdlet.ShouldProcess("service $($s.name)", "Restore start mode $($s.start)")) { continue }

        sc.exe config $s.name start= (ConvertTo-ScStartName ([int]$s.start)) | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Log "Could not restore service $($s.name) (it may have been removed with Edge)." -Level Warn
            continue
        }
        if ($s.status -eq 'Running') {
            Start-Service -Name $s.name -ErrorAction SilentlyContinue
        }
        Write-Log "RESTORED service $($s.name) start=$($s.start)" -Level Ok
    }

    # 3. Tasks: re-enable only those we disabled.
    foreach ($t in @($script:State.tasks)) {
        if ($script:DryRun) {
            Write-Log "WOULD ENABLE task $($t.path)$($t.name) (was $($t.state))" -Level Action
            continue
        }
        if (-not $script:Cmdlet.ShouldProcess("$($t.path)$($t.name)", 'Re-enable scheduled task')) { continue }
        try {
            Enable-ScheduledTask -TaskPath $t.path -TaskName $t.name -ErrorAction Stop | Out-Null
            Write-Log "ENABLED task $($t.path)$($t.name)" -Level Ok
        } catch {
            Write-Log "Could not re-enable task $($t.path)$($t.name): $($_.Exception.Message)" -Level Warn
        }
    }

    # 4. Files moved to the backup folder.
    foreach ($f in @($script:State.files)) {
        if (-not (Test-Path -LiteralPath $f.backup)) { continue }
        if (Test-Path -LiteralPath $f.original) {
            Write-Log "Original path already exists, leaving backup in place: $($f.original)" -Level Warn
            continue
        }
        if ($script:DryRun) {
            Write-Log "WOULD MOVE BACK $($f.backup) -> $($f.original)" -Level Action
            continue
        }
        if (-not $script:Cmdlet.ShouldProcess($f.original, 'Move backup back to its original path')) { continue }

        $parent = Split-Path -Parent $f.original
        if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
        Move-Item -LiteralPath $f.backup -Destination $f.original -Force
        Write-Log "MOVED BACK $($f.original)" -Level Ok
    }

    # 5. Shortcuts we created for -ReplaceWith.
    foreach ($c in @($script:State.created)) {
        if (-not (Test-Path -LiteralPath $c.path)) { continue }
        if ($script:DryRun) {
            Write-Log "WOULD DELETE created $($c.kind): $($c.path)" -Level Action
            continue
        }
        if (-not $script:Cmdlet.ShouldProcess($c.path, 'Delete item created by -Apply')) { continue }
        Remove-Item -LiteralPath $c.path -Force -ErrorAction SilentlyContinue
        Write-Log "DELETED created $($c.kind): $($c.path)" -Level Ok
    }

    if (-not $script:DryRun) {
        $script:State.applied = $false
        $script:State.restoredAt = (Get-Date).ToString('o')
        Save-State
    }

    if (-not (Get-Process -Name topbar, 'rust-explorer' -ErrorAction SilentlyContinue)) {
        Write-Log 'WinRust is not running. If you also removed the IFEO block, start the desktop with: explorer.exe' -Level Info
    } else {
        Write-Log 'WinRust is running; the stock Explorer shell was not started on purpose.' -Level Info
    }
}

# --------------------------------------------------------------------------- #
# Main
# --------------------------------------------------------------------------- #

try {
    if (-not $script:DryRun) {
        $stateDir = Split-Path -Parent $StatePath
        if (-not (Test-Path -LiteralPath $stateDir)) {
            New-Item -ItemType Directory -Path $stateDir -Force | Out-Null
        }
        $script:LogPath = Join-Path $stateDir 'explorer-edge.log'
    }

    Write-Section 'WinRust explorer-edge'
    Write-Log "Script : $($MyInvocation.MyCommand.Path)" -Level Info
    Write-Log "Mode   : $($script:Mode)$(if ($WhatIfPreference) { ' (-WhatIf: forced dry run)' })" -Level Info
    Write-Log "State  : $StatePath" -Level Info

    if (-not $script:DryRun) {
        if (-not (Test-IsAdmin)) {
            throw 'This mode needs an elevated PowerShell (Run as administrator).'
        }
        if (Test-IsSystem) {
            throw 'Do not run this as SYSTEM/TrustedInstaller; Edge setup misbehaves. Use a normal admin account.'
        }
    }

    Show-Plan

    if ($script:Mode -eq 'Restore') {
        Invoke-Restore
        Show-Summary
    } else {
        if ($script:Mode -eq 'Apply' -and -not $script:DryRun) {
            Initialize-State
        }

        if (-not $SkipExplorer) {
            Write-Section 'Explorer shell'
            Set-ExplorerPolicies
            Set-ExplorerIfeo
            Set-ExplorerShellCommand
            Stop-ShellExplorer
        } else {
            Write-Log 'SkipExplorer set: Explorer block skipped.' -Level Info
        }

        if (-not $SkipEdge) {
            Invoke-EdgeBlock
        } else {
            Write-Log 'SkipEdge set: Edge block skipped.' -Level Info
        }

        Show-Summary
    }
} catch {
    Write-Log $_.Exception.Message -Level Error
    Set-ExitCode 2
} finally {
    if (-not $script:DryRun -and $script:State -and $script:Mode -eq 'Apply') {
        try { Save-State } catch { }
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    exit $script:ExitCode
}

<#
SOURCES / DESIGN NOTES
======================

Explorer as a shell
  * Winlogon "Shell" is the value userinit.exe executes; a comma-separated list is
    allowed and a null/missing value falls back to explorer.exe.
    https://learn.microsoft.com/en-us/windows/win32/secauthn/msgina-dll-features
  * Supported replacement for Enterprise/Education/IoT: Shell Launcher (it
    replaces explorer.exe with Eshell/CustomShellHost and handles the shell exit).
    Not available on Home/Pro.
    https://learn.microsoft.com/en-us/windows/configuration/shell-launcher/
  * On this machine Shell is still explorer.exe on purpose: WinRust runs
    rs-shell.exe --overlay from the rs-overlay logon task, sets
    HKLM\...\Winlogon\AutoRestartShell=0 and kills the shell explorer.exe; it
    hands the session back to Explorer if topbar dies. Changing Shell here would
    break that recovery path, so this script only writes Shell when -ShellCommand
    is passed explicitly. WinRust's own supported switch is `rs-shell --enable`
    (per-user HKCU Winlogon\Shell), see plugins/rs-shell/src/main.rs.
  * "NoDesktop" policy (ADMX_Desktop) hides icons/context menu of the Explorer
    desktop only; WinRust's rust-explorer desktop is unaffected.
    https://learn.microsoft.com/en-us/windows/client-management/mdm/policy-csp-admx-desktop

IFEO on explorer.exe
  * IFEO "Debugger" is prepended to the command line by CreateProcess; it is a
    debugging feature, not a supported blocking mechanism.
    https://learn.microsoft.com/en-us/previous-versions/windows/desktop/xperf/image-file-execution-options
  * The kernel does not validate the debugger; a wrong path makes the target fail
    with ERROR_FILE_NOT_FOUND, and every launch of that image name is affected.
    https://devblogs.microsoft.com/oldnewthing/20051219-11/
  * Security products and MITRE treat IFEO Debugger injection as T1546.012, so an
    entry on explorer.exe can be flagged/quarantined by EDR/AV.
    https://attack.mitre.org/techniques/T1546/012/
  * WinRust itself uses exactly `cmd.exe /c exit` for SearchHost.exe and an IFEO
    Debugger for taskmgr.exe, so the pattern is consistent with this machine --
    but it also blocks WinRust's own explorer.exe hand-offs (URLs, ms-settings:,
    /select). Hence -HardFreeze is opt-in and off by default.

Microsoft Edge removal (2025-2026)
  * Edge's own setup.exe is still the working non-GUI route:
      setup.exe --uninstall --system-level --verbose-logging --force-uninstall
    plus the channel form
      --uninstall --msedge --channel=stable --system-level --verbose-logging --force-uninstall
    https://woshub.com/uninstall-disable-microsoft-edge/
    https://github.com/he3als/EdgeRemover (v1.9.5, maintained; also clears
    EdgeUpdateDev\AllowUninstall and ClientState experiment_control_labels)
  * On 24H2+ setup.exe can exit 532 when it is not started by an approved parent
    process; retries/alternate forms and a reboot are the usual workarounds.
    https://learn.microsoft.com/en-us/answers/questions/2407091/
  * Microsoft's supported consumer path is Settings > Installed apps > Uninstall
    and only appears in the EEA (DMA). Outside the EEA the button is greyed out.
    https://learn.microsoft.com/en-us/deployedge/microsoft-edge-update-policies#uninstall
  * WebView2 is a separate product and must be kept; removing it breaks Teams,
    Outlook, widgets and parts of the shell.
    https://learn.microsoft.com/en-us/microsoft-edge/webview2/

EdgeUpdate policies
  * InstallDefault=0 and Install{56EB18F8-B008-4CBD-B6D2-8C97FE7E9062}=0 are the
    documented reinstall blocks, but Microsoft documents them as applying only to
    AD domain-joined devices. On a home/non-domain PC they are best effort.
    https://learn.microsoft.com/en-us/deployedge/microsoft-edge-update-policies#installdefault
  * UpdateDefault/Update are domain-only too; edge://policy shows "configured but
    will be ignored because this device isn't domain joined" when that is the case.
    https://www.kapilarya.com/fix-update-policies-are-configured-but-will-be-ignored-because-this-device-isnt-domain-joined
  * For a stronger block: disable the Microsoft\EdgeUpdate scheduled tasks and
    (optionally, -DisableEdgeUpdateServices) the edgeupdate/edgeupdatem services.
    Note that the services also update WebView2, so disabling them freezes
    WebView2 security updates as well.
    https://learn.microsoft.com/en-us/answers/questions/2282128/
  * DoNotUpdateToEdgeWithChromium is the 2020 Edge-Legacy blocker and is not used
    here; it does not stop the Chromium browser from being reinstalled.
#>
