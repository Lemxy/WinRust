#Requires -Version 5.1
<#
.SYNOPSIS
    Boots the WinRust-modified Tiny11 ISO on a fresh disk (unattended install test).

.DESCRIPTION
    Creates a 120 GB qcow2 disk on the NVMe (C:), boots the ISO with BIOS/SeaBIOS and a QEMU HMP monitor
    on tcp:127.0.0.1:4446, and leaves the install running. By default the VM gets HALF of the real hardware
    (3C/6T and 16 GB on this Ryzen 5 5500 / 32 GB host), so the performance numbers are taken at half of the
    real machine instead of on the slow data HDD.

    The ISO's autounattend.xml creates the local administrator, runs C:\WinRust\tools\autopost.ps1
    -Phase Machine (SetupComplete) and -Phase User (FirstLogonCommands), and reboots into rs-shell.

    If the ISO was built WITHOUT -AutoDisk, Windows Setup still shows the disk page: press Enter/Next
    once. With -AutoDisk the disk is wiped and the install is fully unattended.

.EXAMPLE
    .\iso-autotest.ps1 -Create
    .\iso-autotest.ps1 -Run
    .\iso-autotest.ps1 -Wipe
#>
param(
    [switch]$Create,
    [switch]$Run,
    [switch]$Wipe,
    [string]$Iso = 'C:\WinRust-ISO\winrust-tiny11.iso',
    [string]$Disk = 'C:\WinRust-ISO\vm\win11-auto.qcow2',
    [int]$MemoryMB = 0,          # 0 = half of the real RAM
    [string]$Smp = '',           # empty = half of the real cores/threads
    [int]$DiskGB = 120,          # ~half of the real 256 GB NVMe
    [int]$HmpPort = 4446
)

$ErrorActionPreference = 'Stop'
$qemuDir = 'C:\Program Files\qemu'
$qemu = Join-Path $qemuDir 'qemu-system-x86_64.exe'
$img = Join-Path $qemuDir 'qemu-img.exe'
if (-not (Test-Path $qemu)) { throw "QEMU not found: $qemu" }

# --- half of the real hardware ---------------------------------------------------------------
$cpuInfo = Get-CimInstance Win32_Processor
$realCores = ($cpuInfo | Measure-Object NumberOfCores -Sum).Sum
$realThreads = ($cpuInfo | Measure-Object NumberOfLogicalProcessors -Sum).Sum
$totalRamMB = [int][math]::Floor((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1MB)
$halfCores = [math]::Max(1, [math]::Floor($realCores / 2))
$halfThreads = [math]::Max(1, [math]::Floor($realThreads / 2))
if ($MemoryMB -le 0) { $MemoryMB = [math]::Max(2048, [math]::Floor($totalRamMB / 2)) }
if (-not $Smp) {
    $perCore = [math]::Floor($halfThreads / $halfCores)
    $Smp = if ($perCore -ge 1 -and $halfCores * $perCore -eq $halfThreads) {
        "sockets=1,cores=$halfCores,threads=$perCore"
    } else {
        "$halfThreads"
    }
}
"real: {0}C/{1}T, {2} GB RAM" -f $realCores, $realThreads, [math]::Round($totalRamMB / 1024)
"VM  : {0}C/{1}T, {2} GB RAM (half; disk {3} GB on C: NVMe)" -f $halfCores, $halfThreads, [math]::Round($MemoryMB / 1024), $DiskGB

if ($Wipe) {
    if (Test-Path $Disk) { Remove-Item $Disk -Force; "removed $Disk" } else { "no disk at $Disk" }
    exit 0
}

if ($Create -or -not (Test-Path $Disk)) {
    if (Test-Path $Disk) { "disk exists: $Disk" }
    else {
        New-Item -ItemType Directory (Split-Path $Disk -Parent) -Force | Out-Null
        & $img create -f qcow2 $Disk "${DiskGB}G"
        "created $Disk ($DiskGB GB sparse)"
    }
    if (-not $Run) { exit 0 }
}

if (-not (Test-Path $Iso)) { throw "ISO not found: $Iso (build it with tools\iso-inject.ps1)" }
if (-not (Test-Path $Disk)) { throw "Disk not found: $Disk (use -Create)" }

$accel = if ((Get-CimInstance Win32_Processor).VirtualizationFirmwareEnabled) { 'whpx' } else { 'tcg' }
$cpu = if ($accel -eq 'whpx') { 'host' } else { 'max' }
"accel: $accel  cpu: $cpu"
"ISO  : $Iso"
"disk : $Disk"
"HMP  : tcp:127.0.0.1:$HmpPort"

$qargs = @(
    '-name', 'WinRust auto test',
    '-accel', $accel,
    '-cpu', $cpu,
    '-smp', $Smp,
    '-m', "$MemoryMB",
    '-rtc', 'base=localtime',
    '-drive', "file=$Disk,if=ide,format=qcow2,cache=writeback",
    '-cdrom', $Iso,
    '-nic', 'user,model=e1000',
    '-vga', 'std',
    '-display', 'sdl',
    '-boot', 'once=d',
    '-monitor', "tcp:127.0.0.1:$HmpPort,server=on,wait=off"
)
& $qemu @qargs
