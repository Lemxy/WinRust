#Requires -Version 5.1
<#
.SYNOPSIS
    Boots the WinRust-modified Tiny11 ISO on a fresh disk (unattended install test).

.DESCRIPTION
    Creates a 64 GB qcow2 disk (once), boots the ISO with BIOS/SeaBIOS and a QEMU HMP monitor on
    tcp:127.0.0.1:4446, and leaves the install running. The ISO's autounattend.xml creates the local
    administrator, runs C:\WinRust\tools\autopost.ps1 -Phase Machine (SetupComplete) and -Phase User
    (FirstLogonCommands), and reboots into rs-shell.

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
    [string]$Disk = 'I:\tmp\vm\win11-auto.qcow2',
    [int]$MemoryMB = 4096,
    [int]$HmpPort = 4446
)

$ErrorActionPreference = 'Stop'
$qemuDir = 'C:\Program Files\qemu'
$qemu = Join-Path $qemuDir 'qemu-system-x86_64.exe'
$img = Join-Path $qemuDir 'qemu-img.exe'
if (-not (Test-Path $qemu)) { throw "QEMU not found: $qemu" }

if ($Wipe) {
    if (Test-Path $Disk) { Remove-Item $Disk -Force; "removed $Disk" } else { "no disk at $Disk" }
    exit 0
}

if ($Create -or -not (Test-Path $Disk)) {
    if (Test-Path $Disk) { "disk exists: $Disk" }
    else {
        & $img create -f qcow2 $Disk 64G
        "created $Disk (64 GB sparse)"
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
    '-smp', '2',
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
