#Requires -Version 5.1
<#
.SYNOPSIS
    Measures WinRust inside the test VM (run after the unattended install, elevated).

.DESCRIPTION
    Writes C:\WinRust\bench.md with:
      - the VM's CPU/RAM and the shell processes' memory and idle CPU;
      - start-up time of rs-calc, rust-notepad, rust-settings and rust-control (until their window exists);
      - the same numbers are printed to the console and copied to <payload>\bench.md when a payload drive is
        attached (D:\tools or E:\tools).

    Run it at half of the real hardware (the auto-test VM: 3C/6T, 16 GB RAM) to compare with the host's
    docs\PERFORMANCE.md, which was measured on the full machine (6C/12T).
#>
param(
    [int]$IdleSeconds = 10,
    [int]$StartRuns = 3
)

$ErrorActionPreference = 'Continue'
$outDir = 'C:\WinRust'
New-Item -ItemType Directory $outDir -Force | Out-Null
$out = Join-Path $outDir 'bench.md'
$lines = New-Object System.Collections.Generic.List[string]

function L([string]$s) {
    $lines.Add($s) | Out-Null
    Write-Host $s
}

function AppPath([string]$folder, [string]$exe) {
    Join-Path (Join-Path $env:ProgramFiles $folder) $exe
}

function StartTime([string]$exe) {
    if (-not (Test-Path $exe)) { return $null }
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $exe -PassThru
    $limit = [DateTime]::UtcNow.AddSeconds(30)
    while ($p.MainWindowHandle -eq 0 -and -not $p.HasExited -and [DateTime]::UtcNow -lt $limit) {
        Start-Sleep -Milliseconds 15
        $p.Refresh()
    }
    $sw.Stop()
    $ms = [math]::Round($sw.Elapsed.TotalMilliseconds)
    $ok = ($p.MainWindowHandle -ne 0)
    if (-not $p.HasExited) { $p.CloseMainWindow() | Out-Null; Start-Sleep -Milliseconds 400; if (-not $p.HasExited) { $p.Kill() } }
    if ($ok) { return $ms } else { return $null }
}

$os = Get-CimInstance Win32_OperatingSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$cs = Get-CimInstance Win32_ComputerSystem
L '# WinRust benchmark (inside the VM)'
L ''
L ("- Date: {0}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))
L ("- OS: {0} build {1}" -f $os.Caption, $os.BuildNumber)
L ("- CPU: {0} ({1}C/{2}T assigned)" -f $cpu.Name, $cpu.NumberOfCores, $cpu.NumberOfLogicalProcessors)
L ("- RAM: {0} GB" -f [math]::Round($cs.TotalPhysicalMemory / 1GB, 1))
L ''

# --- shell processes -------------------------------------------------------------------------
L '## Shell processes'
L ''
L '| Process | Working set (MB) | CPU (s) | Idle CPU (%) |'
L '|---|---:|---:|---:|'
$names = @('rs-shell', 'topbar', 'rust-explorer')
$procs = @()
foreach ($n in $names) {
    $p = Get-Process $n -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($p) { $procs += [pscustomobject]@{ Name = $n; Proc = $p; Cpu0 = $p.CPU } }
}
Start-Sleep -Seconds $IdleSeconds
foreach ($x in $procs) {
    $x.Proc.Refresh()
    $delta = [math]::Max(0, $x.Proc.CPU - $x.Cpu0)
    $pct = if ($IdleSeconds -gt 0) { [math]::Round($delta / $IdleSeconds * 100, 2) } else { 0 }
    L ("| {0} | {1} | {2} | {3} |" -f $x.Name, [math]::Round($x.Proc.WorkingSet64 / 1MB, 1), [math]::Round($x.Proc.CPU, 1), $pct)
}
L ''

# --- start-up times --------------------------------------------------------------------------
L '## Start-up time (until the window exists)'
L ''
L '| App | Run 1 (ms) | Run 2 (ms) | Run 3 (ms) | Median (ms) |'
L '|---|---:|---:|---:|---:|'
$targets = @(
    @{ Name = 'rs-calc'; Folder = 'rs-calc'; Exe = 'rs-calc.exe' },
    @{ Name = 'rust-notepad'; Folder = 'rust-notepad'; Exe = 'rust-notepad.exe' },
    @{ Name = 'rust-settings'; Folder = 'rust-settings'; Exe = 'rust-settings.exe' },
    @{ Name = 'rust-control'; Folder = 'rust-control'; Exe = 'rust-control.exe' }
)
foreach ($t in $targets) {
    $exe = AppPath $t.Folder $t.Exe
    $runs = @()
    for ($i = 0; $i -lt $StartRuns; $i++) {
        $ms = StartTime $exe
        if ($null -ne $ms) { $runs += $ms }
        Start-Sleep -Milliseconds 300
    }
    $median = if ($runs.Count) { ($runs | Sort-Object)[[int][math]::Floor($runs.Count / 2)] } else { $null }
    $cells = @()
    for ($i = 0; $i -lt 3; $i++) { $cells += if ($i -lt $runs.Count) { "$($runs[$i])" } else { '—' } }
    L ("| {0} | {1} | {2} | {3} | {4} |" -f $t.Name, $cells[0], $cells[1], $cells[2], $(if ($null -ne $median) { $median } else { '—' }))
}
L ''
L '_Half hardware: compare with docs\PERFORMANCE.md on the full host (6C/12T)._'

$lines | Set-Content $out -Encoding UTF8
Write-Host "written: $out"

# best effort: copy to an attached payload drive
foreach ($d in @('D:', 'E:', 'F:')) {
    if (Test-Path (Join-Path $d '\tools')) {
        Copy-Item $out (Join-Path $d '\bench.md') -Force -ErrorAction SilentlyContinue
        Write-Host "copied to $d\bench.md"
        break
    }
}
