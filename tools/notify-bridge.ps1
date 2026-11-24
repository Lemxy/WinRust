# tools\notify-bridge.ps1 - registers the sparse package that gives rs-notify-bridge package identity.
#
# WinRust has no explorer.exe and ShellExperienceHost is blocked, so the toasts Discord/Steam/Serein post never draw.
# rs-notify-bridge reads them through the Windows notification listener and re-posts them on the WinRust bar. On Windows
# 11 build 26300 the listener works unpackaged (measured; see tools\notify-bridge.md), but the supported route and the
# future-proof one is this identity package: MSIX manifest + self-signed certificate + external location.
#
# Default is -DryRun: it prints every step and changes nothing.
#   powershell -ExecutionPolicy Bypass -File tools\notify-bridge.ps1                  plan
#   powershell -ExecutionPolicy Bypass -File tools\notify-bridge.ps1 -Status          read-only report
#   powershell -ExecutionPolicy Bypass -File tools\notify-bridge.ps1 -Register        do it
#   powershell -ExecutionPolicy Bypass -File tools\notify-bridge.ps1 -Unregister      undo it
#
# Everything this script creates has its path and thumbprint in tools\state\notify-bridge.backup.json, so -Unregister can
# remove exactly what -Register added and nothing else. It does not touch topbar, the pipe, explorer or the i18n files.
param(
    [switch]$Register,
    [switch]$Unregister,
    [switch]$Status,
    [switch]$DryRun,
    [switch]$KeepCertificate,
    [switch]$NoStart,
    [string]$Exe,
    [string]$InstallDir
)

$ErrorActionPreference = 'Stop'

$Here        = $PSScriptRoot
$Root        = Split-Path $Here
$ToolsDir    = Join-Path $Here 'notify-bridge'
$StateDir    = Join-Path $Here 'state'
$StateFile   = Join-Path $StateDir 'notify-bridge.backup.json'
$BuildDir    = Join-Path $StateDir 'notify-bridge'
$StageDir    = Join-Path $BuildDir 'stage'
$MsixPath    = Join-Path $BuildDir 'WinRust.NotifyBridge.msix'
$CerPath     = Join-Path $BuildDir 'WinRust.NotifyBridge.cer'
$PackageName = 'WinRust.NotifyBridge'
$AppId       = 'WinRustNotifyBridge'
$TaskName    = 'rs-notify-bridge'
$CertSubject = 'CN=WinRust Notify Bridge'

if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'WinRust\notify-bridge' }
$InstallDir = [System.IO.Path]::GetFullPath($InstallDir)
$InstalledExe = Join-Path $InstallDir 'rs-notify-bridge.exe'

$modes = @($Register, $Unregister, $Status) | Where-Object { $_ }
if ($modes.Count -gt 1) { throw 'use only one of -Register, -Unregister or -Status' }
if ($Register -and $DryRun) { throw 'use either -Register or -DryRun, not both' }
if ($Unregister -and $DryRun) { throw 'use either -Unregister or -DryRun, not both' }
$Apply = [bool]$Register -or [bool]$Unregister
$Plan = -not $Apply -and -not $Status

# AppX deployment runs as SYSTEM and validates against machine stores: a certificate trusted only in CurrentUser is not
# enough (measured: 0x800B0109). Creating the certificate and signing do not need admin, but trusting it does.
$IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if ($Apply -and -not $IsAdmin) {
    throw 'this needs an elevated PowerShell: the self-signed certificate must be trusted in LocalMachine\TrustedPeople ' +
          '(0x800B0109 otherwise). Open PowerShell as administrator and run the same command again.'
}

function Write-Step([string]$text) { Write-Host "  $text" }
function Write-Head([string]$text) { Write-Host ''; Write-Host $text -ForegroundColor Cyan }
function Write-Warn([string]$text) { Write-Host "  ! $text" -ForegroundColor Yellow }

function Read-State {
    if (Test-Path $StateFile) {
        try { return Get-Content $StateFile -Raw -Encoding UTF8 | ConvertFrom-Json } catch { return $null }
    }
    return $null
}

function Write-State($state) {
    New-Item -ItemType Directory -Force $StateDir | Out-Null
    $json = $state | ConvertTo-Json -Depth 6
    # UTF-8 without a BOM: the file is read by PowerShell and by humans.
    [System.IO.File]::WriteAllText($StateFile, $json, [System.Text.UTF8Encoding]::new($false))
}

function Find-SdkTool([string]$name) {
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $roots = @("${env:ProgramFiles(x86)}\Windows Kits\10\bin", "${env:ProgramFiles}\Windows Kits\10\bin")
    foreach ($root in $roots) {
        if (-not (Test-Path $root)) { continue }
        foreach ($v in (Get-ChildItem $root -Directory | Sort-Object Name -Descending)) {
            $p = Join-Path $v.FullName "x64\$name"
            if (Test-Path $p) { return $p }
        }
    }
    return $null
}

function Find-Exe {
    if ($Exe) {
        if (-not (Test-Path $Exe)) { throw "-Exe: $Exe not found" }
        return (Resolve-Path $Exe).Path
    }
    $candidates = @(
        (Join-Path $Root 'target\release\rs-notify-bridge.exe'),
        (Join-Path $Root 'payload\rs-notify-bridge.exe'),
        (Join-Path $InstallDir 'rs-notify-bridge.exe')
    )
    foreach ($c in $candidates) { if (Test-Path $c) { return $c } }
    throw 'rs-notify-bridge.exe not found. Build it first: cargo build --release -p rs-notify-bridge (or scripts\build-plugins.ps1)'
}

function Find-Certificate {
    Get-ChildItem 'Cert:\CurrentUser\My' |
        Where-Object { $_.Subject -eq $CertSubject -and $_.HasPrivateKey } |
        Sort-Object NotAfter -Descending |
        Select-Object -First 1
}

function Ensure-Certificate([bool]$preview) {
    $cert = Find-Certificate
    if ($cert) {
        Write-Step "certificate found: $($cert.Thumbprint) (expires $($cert.NotAfter.ToString('yyyy-MM-dd')))"
        return $cert
    }
    if ($preview) {
        Write-Step "certificate missing: will create a self-signed code-signing certificate '$CertSubject' in Cert:\CurrentUser\My"
        return $null
    }
    Write-Step "creating self-signed certificate '$CertSubject' (CurrentUser\My, 10 years)"
    return New-SelfSignedCertificate `
        -Type Custom `
        -Subject $CertSubject `
        -KeyUsage DigitalSignature `
        -KeyExportPolicy Exportable `
        -FriendlyName 'WinRust notification bridge (self-signed)' `
        -CertStoreLocation 'Cert:\CurrentUser\My' `
        -NotAfter (Get-Date).AddYears(10) `
        -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
}

function Ensure-Trusted($cert, [bool]$preview) {
    $thumb = $cert.Thumbprint
    $trusted = Get-ChildItem 'Cert:\LocalMachine\TrustedPeople' | Where-Object { $_.Thumbprint -eq $thumb }
    if ($trusted) {
        Write-Step 'certificate already trusted in LocalMachine\TrustedPeople'
        return
    }
    if ($preview) {
        Write-Step 'will export the public certificate and import it into LocalMachine\TrustedPeople (needs admin)'
        return
    }
    New-Item -ItemType Directory -Force $BuildDir | Out-Null
    Export-Certificate -Cert $cert -FilePath $CerPath -Force | Out-Null
    Import-Certificate -FilePath $CerPath -CertStoreLocation 'Cert:\LocalMachine\TrustedPeople' | Out-Null
    Write-Step "trusted: $CerPath -> LocalMachine\TrustedPeople"
}

function Show-Plan {
    $exe = $null
    try { $exe = Find-Exe } catch { Write-Warn $_.Exception.Message }
    $makeappx = Find-SdkTool 'makeappx.exe'
    $signtool = Find-SdkTool 'signtool.exe'
    $cert = Find-Certificate

    Write-Head 'rs-notify-bridge: plan (nothing is changed; add -Register to apply)'
    Write-Step "exe            : $(if ($exe) { $exe } else { '<build rs-notify-bridge first>' })"
    Write-Step "install dir    : $InstallDir  (external location)"
    Write-Step "package        : $PackageName / app id $AppId"
    Write-Step "certificate    : $(if ($cert) { "$($cert.Thumbprint)" } else { "$CertSubject (will be created)" })"
    Write-Step "makeappx       : $(if ($makeappx) { $makeappx } else { '<Windows SDK not found>' })"
    Write-Step "signtool       : $(if ($signtool) { $signtool } else { '<Windows SDK not found>' })"
    Write-Step "msix           : $MsixPath"
    Write-Step "task           : $TaskName (at logon, current user, limited)"
    Write-Step "state          : $StateFile"
    Write-Step ""
    Write-Step 'steps:'
    Write-Step '  1. copy the exe and Assets\ into the install dir'
    Write-Step '  2. stage AppxManifest.xml + Assets\ for makeappx'
    Write-Step '  3. makeappx pack + signtool sign with the local certificate'
    Write-Step '  4. Add-AppxPackage -ExternalLocation <install dir>'
    Write-Step '  5. Register-ScheduledTask rs-notify-bridge at logon and start it'
    Write-Step ''
    Write-Step 'an elevated PowerShell is required: the certificate must be trusted in LocalMachine\TrustedPeople.'
    if (-not $IsAdmin) { Write-Warn 'this session is not elevated; -Register will refuse to run.' }
    if (-not $makeappx -or -not $signtool) {
        Write-Warn 'Windows SDK tools are missing. Install the Windows SDK (makeappx.exe and signtool.exe) or add them to PATH.'
    }
    if (-not $cert) {
        Write-Warn 'A self-signed certificate will be created and trusted machine-wide (admin).'
    }
    Write-Step ''
    Write-Step 'note: the listener already works unpackaged on build 26300; -Register is the identity route and is needed if'
    Write-Step '      a later Windows build enforces the userNotificationListener capability.'
}

function Register-Bridge {
    $exePath = Find-Exe
    $makeappx = Find-SdkTool 'makeappx.exe'
    $signtool = Find-SdkTool 'signtool.exe'
    if (-not $makeappx) { throw 'makeappx.exe not found: install the Windows SDK (makeappx.exe) or add it to PATH' }
    if (-not $signtool) { throw 'signtool.exe not found: install the Windows SDK (signtool.exe) or add it to PATH' }
    if (-not (Test-Path (Join-Path $ToolsDir 'AppxManifest.xml'))) { throw "missing $(Join-Path $ToolsDir 'AppxManifest.xml')" }

    $old = Read-State
    $state = [ordered]@{
        version           = 1
        installedAt       = (Get-Date).ToString('o')
        installDir        = $InstallDir
        exe               = $InstalledExe
        sourceExe         = $exePath
        packageName       = $PackageName
        applicationId     = $AppId
        packageFullName   = $null
        packageFamilyName = $null
        msixPath          = $MsixPath
        certThumbprint    = $null
        certSubject       = $CertSubject
        certCreated       = $false
        taskName          = $TaskName
        taskExisted       = $false
        previous          = $null
    }
    if ($old) { $state.previous = $old }

    Write-Head 'rs-notify-bridge: registering'
    $cert = Ensure-Certificate $false
    $state.certThumbprint = $cert.Thumbprint
    $state.certCreated = if (-not $old -or $old.certThumbprint -ne $cert.Thumbprint) { $true } else { [bool]$old.certCreated }
    Ensure-Trusted $cert $false
    # Save a provisional state before anything is installed: if a later step fails, -Unregister still knows what to undo.
    Write-State $state

    # 1. payload next to the manifest: this directory is the ExternalLocation.
    New-Item -ItemType Directory -Force $InstallDir | Out-Null
    $running = Get-Process rs-notify-bridge -ErrorAction SilentlyContinue
    if ($running) { $running | Stop-Process -Force; Write-Step 'stopped the running bridge' }
    Copy-Item $exePath $InstalledExe -Force
    New-Item -ItemType Directory -Force (Join-Path $InstallDir 'Assets') | Out-Null
    Copy-Item (Join-Path $ToolsDir 'Assets\*') (Join-Path $InstallDir 'Assets') -Force
    Write-Step "payload: $InstalledExe"

    # 2. stage: only the manifest and its assets belong in the package.
    if (Test-Path $StageDir) { Remove-Item $StageDir -Recurse -Force }
    New-Item -ItemType Directory -Force $StageDir | Out-Null
    Copy-Item (Join-Path $ToolsDir 'AppxManifest.xml') $StageDir -Force
    Copy-Item (Join-Path $ToolsDir 'Assets') $StageDir -Recurse -Force
    New-Item -ItemType Directory -Force $BuildDir | Out-Null

    # 3. pack + sign.
    & $makeappx pack /o /nv /h SHA256 /d $StageDir /p $MsixPath
    if ($LASTEXITCODE -ne 0) { throw "makeappx failed with exit code $LASTEXITCODE" }
    & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint $MsixPath
    if ($LASTEXITCODE -ne 0) { throw "signtool failed with exit code $LASTEXITCODE" }
    Write-Step "signed: $MsixPath"

    # The old task, if any, must not keep the exe busy while the package is replaced.
    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($task) {
        $state.taskExisted = $true
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    }

    # 4. register. A same-version registration must be removed first (0x80073CF9 otherwise).
    $existing = Get-AppxPackage -Name $PackageName -ErrorAction SilentlyContinue
    if ($existing) {
        Write-Step "removing the previous $PackageName registration"
        Remove-AppxPackage -Package $existing.PackageFullName -ErrorAction Stop
    }
    Add-AppxPackage -Path $MsixPath -ExternalLocation $InstallDir -ErrorAction Stop
    $pkg = Get-AppxPackage -Name $PackageName -ErrorAction SilentlyContinue
    if (-not $pkg) { throw "Add-AppxPackage returned but $PackageName is not registered" }
    $state.packageFullName = $pkg.PackageFullName
    $state.packageFamilyName = $pkg.PackageFamilyName
    Write-Step "registered: $($pkg.PackageFullName)"
    Write-Step "family    : $($pkg.PackageFamilyName)"

    # 5. start at logon. A scheduled task, like the rest of the project's rs-* tasks; the exe hides its own console.
    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($task) {
        $state.taskExisted = $true
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Step 'replaced the existing rs-notify-bridge task'
    }
    $user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
    $action = New-ScheduledTaskAction -Execute $InstalledExe
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable `
        -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew
    $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Principal $principal `
        -Description 'Forwards Windows notifications to the WinRust bar (rs-notify-bridge)' -Force | Out-Null
    Write-Step "task registered: $TaskName"

    Write-State $state
    if (-not $NoStart) {
        Start-ScheduledTask -TaskName $TaskName
        Write-Step 'task started'
    }
    Write-Head 'done'
    Write-Step "undo with: powershell -ExecutionPolicy Bypass -File tools\notify-bridge.ps1 -Unregister"
}

function Unregister-Bridge {
    $state = Read-State
    $thumb = if ($state -and $state.certThumbprint) { $state.certThumbprint } else { $null }
    if ($state -and $state.installDir) {
        $InstallDir = $state.installDir
        $InstalledExe = Join-Path $InstallDir 'rs-notify-bridge.exe'
    }

    Write-Head 'rs-notify-bridge: unregistering'

    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($task) {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Step "removed task $TaskName"
    } else {
        Write-Step 'task not present'
    }

    $running = Get-Process rs-notify-bridge -ErrorAction SilentlyContinue
    if ($running) { $running | Stop-Process -Force; Write-Step 'stopped the running bridge' }

    $pkg = Get-AppxPackage -Name $PackageName -ErrorAction SilentlyContinue
    if ($pkg) {
        Remove-AppxPackage -Package $pkg.PackageFullName -ErrorAction Stop
        Write-Step "removed package $($pkg.PackageFullName)"
    } else {
        Write-Step 'package not registered'
    }

    if (Test-Path $InstallDir) {
        # Only remove the folder this script owns: it must contain the exe or the Assets\ it copied.
        if ((Test-Path $InstalledExe) -or (Test-Path (Join-Path $InstallDir 'Assets'))) {
            Remove-Item $InstallDir -Recurse -Force
            Write-Step "removed $InstallDir"
        } else {
            Write-Warn "$InstallDir does not look like the bridge folder; left it alone"
        }
    }
    if (Test-Path $StageDir) { Remove-Item $StageDir -Recurse -Force }

    $createdHere = if ($state -and $null -ne $state.certCreated) { [bool]$state.certCreated } else { $false }
    if (-not $KeepCertificate -and $thumb -and $createdHere) {
        foreach ($store in @('Cert:\LocalMachine\TrustedPeople', 'Cert:\CurrentUser\TrustedPeople', 'Cert:\CurrentUser\My')) {
            $c = Get-ChildItem $store | Where-Object { $_.Thumbprint -eq $thumb }
            if ($c) {
                Remove-Item (Join-Path $store $thumb) -Force
                Write-Step "removed certificate $thumb from $store"
            }
        }
    } elseif ($KeepCertificate) {
        Write-Step "certificate kept ($thumb)"
    } elseif ($thumb) {
        Write-Step "certificate was already there before this script; left it ($thumb, use -KeepCertificate to silence this)"
    }

    if ($state) {
        # Copy to a plain ordered dictionary first: Add-Member on a PSCustomObject would not survive ConvertTo-Json.
        $out = [ordered]@{}
        foreach ($p in $state.PSObject.Properties) { $out[$p.Name] = $p.Value }
        $out['unregisteredAt'] = (Get-Date).ToString('o')
        Write-State $out
    }
    Write-Head 'done'
    Write-Step "state kept in $StateFile for the next run"
}

function Show-Status {
    $state = Read-State
    Write-Head 'rs-notify-bridge: status'

    $pkg = Get-AppxPackage -Name $PackageName -ErrorAction SilentlyContinue
    if ($pkg) {
        Write-Step "package        : $($pkg.PackageFullName)"
        Write-Step "family         : $($pkg.PackageFamilyName)"
        Write-Step "installed at   : $($pkg.InstallLocation)"
    } else {
        Write-Step 'package        : not registered'
    }

    $cert = Find-Certificate
    if ($cert) {
        $trusted = Get-ChildItem 'Cert:\LocalMachine\TrustedPeople' | Where-Object { $_.Thumbprint -eq $cert.Thumbprint }
        Write-Step "certificate    : $($cert.Thumbprint) expires $($cert.NotAfter.ToString('yyyy-MM-dd')) trusted=$([bool]$trusted)"
    } else {
        Write-Step "certificate    : none for '$CertSubject'"
    }

    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($task) {
        $info = Get-ScheduledTaskInfo -TaskName $TaskName
        Write-Step "task           : $TaskName state=$($task.State) last=$($info.LastRunTime) result=$($info.LastTaskResult)"
    } else {
        Write-Step 'task           : not registered'
    }

    Write-Step "exe installed  : $(if (Test-Path $InstalledExe) { $InstalledExe } else { 'no' })"
    if ($state) {
        Write-Step "state          : $StateFile"
        Write-Step "built msix     : $(if (Test-Path $MsixPath) { $MsixPath } else { 'no' })"
    } else {
        Write-Step "state          : no $StateFile yet"
    }

    $proc = Get-Process rs-notify-bridge -ErrorAction SilentlyContinue
    Write-Step "process        : $(if ($proc) { "running (pid $($proc.Id))" } else { 'not running' })"

    $bar = Get-Process topbar -ErrorAction SilentlyContinue
    Write-Step "topbar         : $(if ($bar) { 'running' } else { 'not running' })"
    $pipes = @()
    try { $pipes = @([System.IO.Directory]::GetFiles('\\.\pipe\') | Where-Object { $_ -like '*winrust-notify*' }) } catch { }
    Write-Step "notify pipe    : $(if ($pipes.Count) { $pipes[0] } else { 'not found' })"
    Write-Step 'test one toast : rs-notify.exe --app Test --title Hola --body "desde el bridge"'
    Write-Step 'test all now   : cargo run --release -p rs-notify-bridge -- --once --all --verbose'
}

if ($Status) { Show-Status; return }
if ($Plan) { Show-Plan; return }
if ($Register) { Register-Bridge; return }
if ($Unregister) { Unregister-Bridge; return }
