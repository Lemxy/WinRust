#Requires -Version 5.1
<#
.SYNOPSIS
    Lightweight QEMU VM for testing the WinRust ISO.

.DESCRIPTION
    The CPU supports AMD-V but it is disabled in the firmware, so Hyper-V/VirtualBox cannot run a 64-bit guest.
    QEMU with TCG (software emulation) does not need it: slower, but it boots Windows and lets us run the
    provisioning + checklist. Enable "SVM Mode" in the BIOS and this script will use WHPX automatically.

    The VM: 4 GB RAM, 2 vCPU, 64 GB sparse qcow2 disk, IDE disk (no virtio drivers needed),
    e1000 NIC, SDL window. A FAT drive with `payload\` is attached so the WinRust binaries and the
    provisioning scripts are available inside the guest.

.PARAMETER Create
    Create the 64 GB qcow2 disk (once).

.PARAMETER Install
    Boot the Windows ISO (Tiny11 / official). Detach it with -Run afterwards.

.PARAMETER Run
    Boot the installed system.

.PARAMETER Payload
    Attach (or detach) the `payload` FAT drive. On by default in Install/Run.

.PARAMETER Iso
    ISO path. Default: the newest *.iso under I:\tmp\isos.

.PARAMETER MemoryMB
    Guest RAM, 4096 by default.

.PARAMETER Wipe
    Delete the qcow2 disk (asks first unless -Yes).

.EXAMPLE
    .\iso-vm.ps1 -Create
    .\iso-vm.ps1 -Install
    .\iso-vm.ps1 -Run
#>
param(
    [switch]$Create,
    [switch]$Install,
    [switch]$Run,
    [switch]$NoPayload,
    [string]$Iso = "",
    [int]$MemoryMB = 4096,
    [switch]$Wipe,
    [switch]$Yes
)

$ErrorActionPreference = 'Stop'
$root   = 'I:\tmp\vm'
$disk   = Join-Path $root 'win11.qcow2'
$payload = Join-Path $root 'payload'

function Find-Qemu([string]$exe) {
    $c = Get-Command $exe -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    foreach ($p in @("$env:ProgramFiles\qemu\$exe", "${env:ProgramFiles(x86)}\qemu\$exe", 'C:\Program Files\qemu\' + $exe)) {
        if (Test-Path $p) { return $p }
    }
    throw "$exe not found: install QEMU (winget install SoftwareFreedomConservancy.QEMU) or put it in PATH"
}

$qemu = Find-Qemu 'qemu-system-x86_64.exe'
$img  = Find-Qemu 'qemu-img.exe'

if (-not (Test-Path $root)) { New-Item -ItemType Directory $root -Force | Out-Null }

if ($Wipe) {
    if (Test-Path $disk) {
        if (-not $Yes) {
            $a = Read-Host "Delete $disk and lose the installed system? (yes/no)"
            if ($a -notmatch '^(y|yes|s|si|sí)$') { exit 1 }
        }
        Remove-Item $disk -Force
        "disk removed"
    } else { "no disk to remove" }
    exit 0
}

if ($Create -or -not (Test-Path $disk)) {
    if (Test-Path $disk) { "disk already exists: $disk" }
    else {
        & $img create -f qcow2 $disk 64G | Out-Host
        "created $disk (64 GB sparse)"
    }
    if (-not ($Install -or $Run)) { exit 0 }
}

if (-not $Iso) {
    $Iso = Get-ChildItem 'I:\tmp\isos' -Filter *.iso -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if ($Install -and -not ($Iso -and (Test-Path $Iso))) { throw "no ISO found (use -Iso <path>)" }

# WHPX when the firmware exposes AMD-V, TCG otherwise (slow but works without touching the BIOS).
$accel = if ((Get-CimInstance Win32_Processor).VirtualizationFirmwareEnabled) { 'whpx' } else { 'tcg' }
$cpu   = if ($accel -eq 'whpx') { 'host' } else { 'max' }   # TCG needs a model with SSE4/AVX for modern Windows
"accel: $accel  cpu: $cpu"

$args = @(
    '-name', 'WinRust ISO test',
    '-accel', $accel,
    '-cpu', $cpu,
    '-smp', '2',
    '-m', "$MemoryMB",
    '-rtc', 'base=localtime',
    '-drive', "file=$disk,if=ide,format=qcow2,cache=writeback",
    '-nic', 'user,model=e1000',
    '-vga', 'std',
    '-display', 'sdl',
    '-boot', $(if ($Install) { 'once=d' } else { 'c' })
)
if ($Install) { $args += @('-cdrom', $Iso) }
if (-not $NoPayload) {
    if (Test-Path $payload) { $args += @('-drive', "file=fat:rw:$payload,format=raw,media=disk") }
    else { "payload folder missing: $payload (the guest will not see the WinRust files)" }
}

"qemu: $qemu"
"cmd : $qemu $($args -join ' ')"
& $qemu @args
