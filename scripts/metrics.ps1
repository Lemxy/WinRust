#Requires -Version 5.1
<#
.SYNOPSIS
    Measures the installed WinRust shell and writes docs/metrics.md (raw) plus the tables for the README.

.DESCRIPTION
    Everything is measured on the live session with the same method for every row:

      - binary size   : the file in Program Files
      - RAM           : WorkingSet64 after the process has been idle for IdleSeconds
      - CPU (idle)    : TotalProcessorTime delta over IdleSeconds, as a percentage of ONE logical core
      - window visible: Stopwatch from CreateProcess to MainWindowHandle != 0, median of Runs
      - flyout        : a real click on the bar, Stopwatch from the click to the flyout window being visible

    Nothing is installed or changed; the apps it opens are closed again and the cursor is put back.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\metrics.ps1
    powershell -ExecutionPolicy Bypass -File scripts\metrics.ps1 -Runs 5 -IdleSeconds 10
#>
param(
    [int]$Runs = 3,
    [int]$IdleSeconds = 5,
    [string]$Out = ''
)

$ErrorActionPreference = 'Stop'
[Threading.Thread]::CurrentThread.CurrentCulture = 'en-US' # the README tables are English: 41.0, not 41,0
$root = Split-Path $PSScriptRoot -Parent
if (-not $Out) { $Out = Join-Path $root 'docs\PERFORMANCE.md' }
Add-Type -Path (Join-Path $root 'tools\WinShot.cs') -ReferencedAssemblies System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$pf = $env:ProgramFiles
$exe = @{
    'topbar'        = Join-Path $pf 'topbar\topbar.exe'
    'rs-shell'      = Join-Path $pf 'rust-explorer\rs-shell.exe'
    'desktop'       = Join-Path $pf 'rust-explorer\rust-explorer.exe'
    'rs-clip'       = Join-Path $pf 'rust-explorer\rs-clip.exe'
    'reshell'       = Join-Path $pf 'topbar\reshell.exe'
    'rs-calc'       = Join-Path $pf 'rs-calc\rs-calc.exe'
    'rust-notepad'  = Join-Path $pf 'rust-notepad\rust-notepad.exe'
    'rust-settings' = Join-Path $pf 'rust-settings\rust-settings.exe'
    'rust-control'  = Join-Path $pf 'rust-control\rust-control.exe'
    'rtaskmgr'      = Join-Path $pf 'rtaskmgr\rtaskmgr-win32.exe'
    'photoview'     = Join-Path $pf 'photoview\photoview.exe'
    'wiztree'       = Join-Path $pf 'wiztree\wiztree.exe'
    'rs-shot'       = Join-Path $pf 'rs-shot\rs-shot.exe'
    'rs-notify'     = Join-Path $pf 'topbar\rs-notify.exe'
    'rs.exe'        = Join-Path $pf 'rs-search\rs.exe'
    'rs-launcher'   = Join-Path $pf 'rs-search\rs-launcher.exe'
}

$lines = New-Object System.Collections.Generic.List[string]
function L([string]$s) { $lines.Add($s) | Out-Null; Write-Host $s }
function Med([double[]]$v) { if (-not $v -or $v.Count -eq 0) { return $null }; $s = @($v | Sort-Object); $s[[int][math]::Floor(($s.Count - 1) / 2)] }
function F1($v) { if ($null -eq $v) { '-' } else { '{0:N1}' -f [double]$v } }
function F0($v) { if ($null -eq $v) { '-' } else { '{0:N0}' -f [double]$v } }

$primary = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$savedCursor = [WinShot]::Cursor()

# The bar dodges windows: the pointer has to touch the last pixel row of the screen for it to slide in.
function Reveal {
    [WinShot]::Move($primary.X + [int]($primary.Width / 2), $primary.Bottom - 1)
    $end = (Get-Date).AddSeconds(2)
    do {
        Start-Sleep -Milliseconds 120
        if (Find-Bar -ne [IntPtr]::Zero) { Start-Sleep -Milliseconds 250; return }
    } while ((Get-Date) -lt $end)
}
function Find-Bar {
    foreach ($h in [WinShot]::All()) {
        if ([WinShot]::ClassOf($h) -eq 'topbar' -and [WinShot]::Visible($h)) {
            $r = [WinShot]::RectOf($h)
            if ($r[0] -eq $primary.X) { return $h }
        }
    }
    [IntPtr]::Zero
}

function Idle($p, [int]$sec = $IdleSeconds) {
    $p.Refresh(); $c0 = $p.TotalProcessorTime.TotalSeconds
    Start-Sleep -Seconds $sec
    $p.Refresh()
    [pscustomobject]@{ Ram = $p.WorkingSet64 / 1MB; Cpu = [math]::Max(0, ($p.TotalProcessorTime.TotalSeconds - $c0) / $sec * 100) }
}

# ---------------------------------------------------------------- 1. the machine
$os = Get-CimInstance Win32_OperatingSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$cs = Get-CimInstance Win32_ComputerSystem
$gpu = (Get-CimInstance Win32_VideoController | Select-Object -First 1).Name
$mons = [System.Windows.Forms.Screen]::AllScreens
$mtext = ($mons | ForEach-Object { '{0}x{1}{2}' -f $_.Bounds.Width, $_.Bounds.Height, $(if ($_.Primary) { ' (primary)' } else { '' }) }) -join ' + '

L '# WinRust metrics'
L ''
L ('Measured on {0:yyyy-MM-dd HH:mm} on the machine below. Method at the end; `scripts\metrics.ps1` reproduces every row.' -f (Get-Date))
L ''
L '| | |'
L '|---|---|'
L ('| CPU | {0} ({1}C / {2}T) |' -f $cpu.Name.Trim(), $cpu.NumberOfCores, $cpu.NumberOfLogicalProcessors)
L ('| RAM | {0} GB |' -f [math]::Round($cs.TotalPhysicalMemory / 1GB))
L ('| GPU | {0} |' -f $gpu)
L ('| OS | {0} (build {1}) |' -f $os.Caption, $os.BuildNumber)
L ('| Displays | {0} |' -f $mtext)
L '| Build | release (GNU), rtaskmgr with MSVC |'
L ''

# ---------------------------------------------------------------- 2. binaries
L '## Binaries'
L ''
L '| Program | File | Size |'
L '|---|---|---:|'
$total = 0.0
foreach ($k in @('topbar', 'rs-shell', 'desktop', 'rs-clip', 'reshell', 'rs-calc', 'rust-notepad', 'rust-settings', 'rust-control', 'rtaskmgr', 'photoview', 'wiztree', 'rs-shot', 'rs-notify', 'rs.exe', 'rs-launcher')) {
    $f = $exe[$k]
    if (Test-Path $f) {
        $mb = (Get-Item $f).Length / 1MB
        $total += $mb
        L ('| {0} | `{1}` | {2:N2} MB |' -f $k, (Split-Path $f -Leaf), $mb)
    }
}
L ('| **Total** | | **{0:N1} MB** |' -f $total)
L ''

# ---------------------------------------------------------------- 3. the running shell
L '## Idle cost of the shell'
L ''
L ('RAM: `WorkingSet64` after {0} s of no work. CPU: `TotalProcessorTime` delta over the same window, as a percentage of ONE logical core ({1} cores).' -f $IdleSeconds, $cpu.NumberOfLogicalProcessors)
L ''
L '| Process | RAM | CPU (1 core) |'
L '|---|---:|---:|'
$desktopProc = Get-CimInstance Win32_Process -Filter "Name='rust-explorer.exe'" | Where-Object { $_.CommandLine -match '--desktop' } | Select-Object -First 1
$sumRam = 0.0; $sumCpu = 0.0
$groups = @(
    @{ name = 'Bar'; procs = @(Get-Process topbar -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$pf\*" }) },
    @{ name = 'Desktop'; procs = @(if ($desktopProc) { Get-Process -Id $desktopProc.ProcessId -ErrorAction SilentlyContinue }) },
    @{ name = 'Supervisor'; procs = @(Get-Process rs-shell -ErrorAction SilentlyContinue) },
    @{ name = 'Clipboard (Win+V)'; procs = @(Get-Process rs-clip -ErrorAction SilentlyContinue) },
    @{ name = 'Desktop hotkeys'; procs = @(Get-Process reshell -ErrorAction SilentlyContinue) }
)
foreach ($g in $groups) {
    $ps = @($g.procs | Where-Object { $_ })
    if ($ps.Count -eq 0) { L ('| {0} | not running | - |' -f $g.name); continue }
    if ($g.name -eq 'Bar') {
        # the blur only runs while the bar is on screen: measure it where the user sees it, pointer on the bar
        Reveal
        $bh = Find-Bar
        if ($bh -ne [IntPtr]::Zero) {
            $br = [WinShot]::RectOf($bh)
            [WinShot]::Move($br[0] + [int]($br[2] / 2), $br[1] + [int]($br[3] / 2))
            Start-Sleep -Milliseconds 400
        }
    }
    $ram = 0.0; $pct = 0.0
    foreach ($p in $ps) { $i = Idle $p; $ram += $i.Ram; $pct += $i.Cpu }
    $sumRam += $ram; $sumCpu += $pct
    L ('| {0} | {1:N1} MB | {2:N2} % |' -f $g.name, $ram, $pct)
}
L ('| **Shell total** | **{0:N1} MB** | **{1:N2} %** |' -f $sumRam, $sumCpu)
L ''

# ---------------------------------------------------------------- 4. cold start of every app
L '## Cold start and idle cost of each app'
L ''
L ('`CreateProcess` -> `MainWindowHandle != 0`, median of {0} runs; RAM and CPU {1} s after the window appeared. Each app is closed afterwards.' -f $Runs, $IdleSeconds)
L ''
L '| App | Window visible (ms) | RAM | CPU (1 core) |'
L '|---|---:|---:|---:|'
$image = Join-Path $root 'assets\th.jpg'
$note = Join-Path $env:TEMP 'winrust-metrics.txt'
Set-Content $note 'WinRust metrics - a plain text file to measure rust-notepad.' -Encoding UTF8
$apps = @(
    @{ name = 'rs-calc'; label = 'rs-calc'; args = @(); kill = 'rs-calc' },
    @{ name = 'rust-notepad'; label = 'rust-notepad'; args = @($note); kill = 'rust-notepad' },
    @{ name = 'rust-settings'; label = 'rust-settings'; args = @(); kill = 'rust-settings' },
    @{ name = 'rust-control'; label = 'rust-control'; args = @(); kill = 'rust-control' },
    @{ name = 'rtaskmgr'; label = 'Task Manager'; args = @(); kill = 'rtaskmgr-win32' },
    @{ name = 'photoview'; label = 'photoview'; args = @($image); kill = 'photoview' },
    @{ name = 'desktop'; label = 'File manager (new window)'; args = @('C:\'); kill = $null }
)
foreach ($a in $apps) {
    $f = $exe[$a.name]
    if (-not (Test-Path $f)) { continue }
    if ($a.kill) { Stop-Process -Name $a.kill -Force -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 300 }
    $times = @(); $ram = $null; $pct = $null
    for ($i = 0; $i -lt $Runs; $i++) {
        $sp = @{ FilePath = $f; PassThru = $true }
        if ($a.args.Count -gt 0) { $sp.ArgumentList = $a.args }
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $p = Start-Process @sp
        $limit = (Get-Date).AddSeconds(25)
        do {
            Start-Sleep -Milliseconds 10
            $p.Refresh()
            if ($p.HasExited) { break }
        } while ($p.MainWindowHandle -eq 0 -and (Get-Date) -lt $limit)
        $sw.Stop()
        if (-not $p.HasExited -and $p.MainWindowHandle -ne 0) {
            $times += $sw.Elapsed.TotalMilliseconds
            if ($null -eq $ram) { $idle = Idle $p; $ram = $idle.Ram; $pct = $idle.Cpu }
        }
        if (-not $p.HasExited) {
            $p.CloseMainWindow() | Out-Null
            Start-Sleep -Milliseconds 350
            if (-not $p.HasExited) { $p.Kill() }
        }
        Start-Sleep -Milliseconds 250
    }
    L ('| {0} | {1} | {2} | {3} |' -f $a.label, (F0 (Med $times)), $(if ($null -ne $ram) { (F1 $ram) + ' MB' } else { '-' }), $(if ($null -ne $pct) { '{0:N2} %' -f $pct } else { '-' }))
}
L ''

# ---------------------------------------------------------------- 5. flyouts (real clicks)
L '## Opening the UI'
L ''
L ('Real mouse click on the bar -> first visible frame of the window it opens, median of {0}. The bar dodges windows, so the pointer touches the screen edge first.' -f $Runs)
L ''
L '| Action | Latency (ms) |'
L '|---|---:|'
function WaitFly([string]$cls, [int]$ms = 2500) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    do {
        if ([WinShot]::Find($cls, '', 0) -ne [IntPtr]::Zero) { $sw.Stop(); return $sw.Elapsed.TotalMilliseconds }
        Start-Sleep -Milliseconds 4
    } while ($sw.ElapsedMilliseconds -lt $ms)
    $null
}
function Measure-Flyout([string]$cls, [string]$where) {
    $t = @()
    for ($i = 0; $i -lt $Runs; $i++) {
        Reveal
        $r = [WinShot]::RectOf((Find-Bar))
        $x = switch ($where) {
            'start' { $r[0] + 22 }
            'menu'  { $r[0] + [int]($r[2] / 3) }
            'bell'  { $r[0] + $r[2] - 220 }
        }
        [WinShot]::Click($x, $r[1] + [int]($r[3] / 2), ($where -eq 'menu'))
        $ms = WaitFly $cls
        if ($null -ne $ms) { $t += $ms }
        [WinShot]::Key(0x1B)
        Start-Sleep -Milliseconds 320
    }
    Med $t
}
L ('| Start menu | {0} |' -f (F1 (Measure-Flyout 'topbar_fly' 'start')))
L ('| Bar menu (right click) | {0} |' -f (F1 (Measure-Flyout 'topbar_fly' 'menu')))
L ('| Notification centre (bell) | {0} |' -f (F1 (Measure-Flyout 'topbar_notify' 'bell')))
L ''

# ---------------------------------------------------------------- 6. notifications end to end
L '## Notifications'
L ''
$t = @()
for ($i = 0; $i -lt 10; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $null = & $exe['rs-notify'] --app 'Metrics' --title 'Ping' --body 'latency' --timeout 1200
    $null = WaitFly 'topbar_toast' 2000
    $t += $sw.Elapsed.TotalMilliseconds
    Start-Sleep -Milliseconds 1500
}
L ('`rs-notify` process start -> toast on screen: **{0} ms** (median of 10, range {1}-{2} ms).' -f (F1 (Med $t)), (F0 ($t | Measure-Object -Minimum).Minimum), (F0 ($t | Measure-Object -Maximum).Maximum))
L ''

# ---------------------------------------------------------------- 7. search
L '## File search'
L ''
L '| Search | Median (ms) | Hits |'
L '|---|---:|---:|'
$rs = $exe['rs.exe']
$rt = @(); $rh = 0
for ($i = 0; $i -lt $Runs; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $hits = @(& $rs winrust 2>$null)
    $sw.Stop(); $rt += $sw.Elapsed.TotalMilliseconds; $rh = $hits.Count
}
L ('| `rs.exe winrust` (MFT) | {0} | {1} |' -f (F0 (Med $rt)), $rh)
try {
    $c = New-Object -ComObject ADODB.Connection
    $c.Open("Provider=Search.CollatorDSO;Extended Properties='Application=Windows';")
    $wt = @(); $wh = 0
    for ($i = 0; $i -lt $Runs; $i++) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $r = $c.Execute("SELECT System.ItemPath FROM SystemIndex WHERE System.FileName LIKE '%winrust%'")
        $k = 0
        while (-not $r.EOF) { $k++; $r.MoveNext() }
        $sw.Stop(); $wt += $sw.Elapsed.TotalMilliseconds; $wh = $k
    }
    L ('| Windows Search (same term) | {0} | {1} |' -f (F0 (Med $wt)), $wh)
} catch { L '| Windows Search | not available | - |' }
L ''

# ---------------------------------------------------------------- 8. the supervisor brings the shell back
L '## Recovery: the supervisor restarts the shell'
L ''
L ('`rs-shell` watches the bar and the desktop. Each number is the time from killing the process to its window being visible again, median of 2. Killing them is safe: the supervisor restarts them, which is exactly what this measures.')
L ''
L '| Piece | Back after |'
L '|---|---:|'
function T_Kill([string]$name, [string]$cls, [string]$args) {
    $t = @()
    for ($i = 0; $i -lt 2; $i++) {
        $procs = @(Get-CimInstance Win32_Process -Filter "Name='$name'" | Where-Object { $args -eq '' -or $_.CommandLine -match $args })
        foreach ($p in $procs) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue }
        Start-Sleep -Milliseconds 300
        $sw = [Diagnostics.Stopwatch]::StartNew()
        do {
            Start-Sleep -Milliseconds 100
            $up = $false
            foreach ($h in [WinShot]::All()) {
                if ([WinShot]::ClassOf($h) -eq $cls -and [WinShot]::Visible($h)) { $up = $true; break }
            }
        } while (-not $up -and $sw.Elapsed.TotalSeconds -lt 30)
        $sw.Stop()
        if ($up) { $t += $sw.Elapsed.TotalMilliseconds }
        Start-Sleep -Milliseconds 800
    }
    Med $t
}
L ('| Bar (`topbar`) | {0:N2} s |' -f ((T_Kill 'topbar.exe' 'topbar' '') / 1000))
L ('| Desktop (`rust-explorer --desktop`) | {0:N2} s |' -f ((T_Kill 'rust-explorer.exe' 'Progman' '--desktop') / 1000))
L ''

# ---------------------------------------------------------------- 9. installer
L '## Installer'
L ''
$inst = Join-Path $pf 'WinRust\WinRust.exe'
if (Test-Path $inst) {
    L ('`WinRust.exe` with every plugin embedded: **{0:N1} MB**; the installed programs above add up to {1:N1} MB.' -f ((Get-Item $inst).Length / 1MB), $total)
    L ''
}

L '## Method'
L ''
L '- Binary size: the installed file on disk.'
L ('- RAM: `Process.WorkingSet64` after the process has been idle for {0} s (never during startup).' -f $IdleSeconds)
L ('- CPU: `TotalProcessorTime` delta over {0} s of idle, as a percentage of ONE logical core ({1} cores available).' -f $IdleSeconds, $cpu.NumberOfLogicalProcessors)
L ('- Window visible: `Stopwatch` from `CreateProcess` to `MainWindowHandle != 0`, median of {0}; the first run of an app is discarded only when it fails to show a window.' -f $Runs)
L '- Flyouts: a real click on the bar, `Stopwatch` from the click to the flyout window (class `topbar_fly` / `topbar_notify`) being visible, median of 3.'
L '- Notifications: `rs-notify` process start to the toast window being visible, median of 10.'
L '- Search: `rs.exe <term>` to process exit vs the same term through Windows Search (ADO), median of 3.'
L '- Recovery: the supervisor (`rs-shell`) is watched while the bar or the desktop is killed, median of 2.'
L '- The bar idle CPU is the live blur: `bar-blur=0` in `%ProgramData%\WinRust\look.txt` trades it for ~0 %.'
L ''
L '_Measured on a real desktop with the fork look (Win7-style Start, dodge windows, blur on)._'

[WinShot]::Move($savedCursor[0], $savedCursor[1])
$lines | Set-Content $Out -Encoding UTF8
Write-Host "written: $Out"
