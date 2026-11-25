#Requires -Version 5.1
<#
.SYNOPSIS
    Adds the WinRust post-setup to the modified Tiny11 ISO (NTLite base) and rebuilds a bootable ISO.

.DESCRIPTION
    Keeps the NTLite removals (install.esd and its preset are untouched) and injects:
      sources\$OEM$\$1\WinRust\apps\*.exe      -> C:\WinRust\apps
      sources\$OEM$\$1\WinRust\tools\*.ps1     -> C:\WinRust\tools
      sources\$OEM$\$$\Setup\Scripts\SetupComplete.cmd -> C:\Windows\Setup\Scripts
      autounattend.xml (merged template: local admin, PostOobe, FirstLogonCommands)

    The ISO is rebuilt with IMAPI2 (BIOS/SeaBIOS boot through boot\etfsboot.com). For a UEFI-bootable
    ISO use oscdimg from the Windows ADK:
      oscdimg -m -o -u2 -udfver102 -bootdata:2#p0,e,b<media>\boot\etfsboot.com#pEF,e,b<media>\efi\microsoft\boot\efisys.bin <media> <out.iso>

.PARAMETER Check
    Validate inputs and print the plan; do not mount, copy or build.

.PARAMETER StageOnly
    Copy the ISO tree and inject the files, but do not build the new ISO.

.EXAMPLE
    .\iso-inject.ps1 -Check
    .\iso-inject.ps1
#>
param(
    [string]$BaseIso = 'I:\tmp\isos\tiny11-26h2-beta1.iso',
    [string]$Payload = 'I:\tmp\vm\payload',
    [string]$WorkDir = 'C:\WinRust-ISO\media',
    [string]$OutIso = 'C:\WinRust-ISO\winrust-tiny11.iso',
    [string]$User = 'rexx',
    [string]$Password = 'winrust',
    [string]$VolumeLabel = 'WINRUST_TINY11',
    [switch]$AutoDisk,
    [switch]$Check,
    [switch]$StageOnly,
    [switch]$KeepWork
)

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$post = Join-Path $here 'iso-postsetup'

function L([string]$m) { Write-Host ("[{0}] {1}" -f (Get-Date -Format 'HH:mm:ss'), $m) }
function Need([string]$p, [string]$what) {
    if (-not (Test-Path $p)) { throw "$what not found: $p" }
}

# --- inputs -------------------------------------------------------------------------------------
Need $BaseIso 'base ISO'
Need $Payload 'payload folder'
Need (Join-Path $Payload 'apps') 'payload apps'
Need (Join-Path $post 'autopost.ps1') 'autopost.ps1'
Need (Join-Path $post 'SetupComplete.cmd') 'SetupComplete.cmd'
Need (Join-Path $post 'autounattend.xml') 'autounattend.xml template'
Need (Join-Path $here 'iso-provision.ps1') 'iso-provision.ps1'
Need (Join-Path $here 'shell-phase2.ps1') 'shell-phase2.ps1'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { throw 'Run this from an elevated PowerShell (Mount-DiskImage + IMAPI2).' }

L "Base ISO : $BaseIso"
L "Payload  : $Payload"
L "Work dir : $WorkDir"
L "Output   : $OutIso"
L "User     : $User"
L "Boot     : BIOS (etfsboot.com); UEFI needs oscdimg (see the header)"
L "Removals : install.esd and the NTLite preset are kept untouched"
if ($Check) { L 'Check mode: nothing is mounted, copied or built.'; exit 0 }

if (Test-Path $WorkDir) {
    L "Removing the previous work dir $WorkDir"
    Remove-Item $WorkDir -Recurse -Force
}
New-Item -ItemType Directory $WorkDir -Force | Out-Null

# --- copy the ISO tree --------------------------------------------------------------------------
$mounted = $null
try {
    L 'Mounting the base ISO...'
    $mounted = Mount-DiskImage -ImagePath $BaseIso -PassThru
    $vol = $mounted | Get-Volume
    if (-not $vol.DriveLetter) { throw 'The mounted ISO has no drive letter.' }
    $src = "$($vol.DriveLetter):\"
    L "Copying $src -> $WorkDir (this reads the whole ISO)..."
    $null = robocopy $src $WorkDir /E /COPY:DAT /R:1 /W:1 /NFL /NDL /NJH /NJS /NP
    if ($LASTEXITCODE -ge 8) { throw "robocopy failed with exit code $LASTEXITCODE" }
} finally {
    if ($mounted) { Dismount-DiskImage -ImagePath $BaseIso -ErrorAction SilentlyContinue | Out-Null; L 'Base ISO dismounted.' }
}

# --- $OEM$ payload ------------------------------------------------------------------------------
$oem1 = Join-Path $WorkDir 'sources\$OEM$\$1\WinRust'
$oemApps = Join-Path $oem1 'apps'
$oemTools = Join-Path $oem1 'tools'
$oemWin = Join-Path $WorkDir 'sources\$OEM$\$$\Setup\Scripts'
New-Item -ItemType Directory $oemApps, $oemTools, $oemWin -Force | Out-Null

$missing = @()
Get-ChildItem (Join-Path $Payload 'apps') -Filter *.exe | ForEach-Object {
    Copy-Item $_.FullName (Join-Path $oemApps $_.Name) -Force
}
foreach ($n in @('topbar.exe', 'rust-explorer.exe', 'rs-shell.exe', 'rust-settings.exe', 'rs-shot.exe', 'rust-control.exe', 'rust-notepad.exe', 'rs-calc.exe', 'rtaskmgr-win32.exe')) {
    if (-not (Test-Path (Join-Path $oemApps $n))) { $missing += $n }
}
L ("apps injected: {0}" -f ((Get-ChildItem $oemApps -Filter *.exe).Count))
if ($missing.Count) { L ("WARNING missing apps: {0}" -f ($missing -join ', ')) }

foreach ($f in @('autopost.ps1', 'iso-provision.ps1', 'shell-phase2.ps1')) {
    $srcFile = if ($f -eq 'autopost.ps1') { Join-Path $post $f } else { Join-Path $here $f }
    Copy-Item $srcFile (Join-Path $oemTools $f) -Force
}
Copy-Item (Join-Path $post 'SetupComplete.cmd') (Join-Path $oemWin 'SetupComplete.cmd') -Force
L "tools and SetupComplete.cmd injected"

# --- answer file --------------------------------------------------------------------------------
$aa = Join-Path $WorkDir 'autounattend.xml'
if (Test-Path $aa) { Set-ItemProperty $aa -Name IsReadOnly -Value $false }
$tpl = Get-Content (Join-Path $post 'autounattend.xml') -Raw
$tpl = $tpl.Replace('{{USER}}', $User).Replace('{{PASSWORD}}', $Password)
if ($AutoDisk) {
    # Destructive: wipes disk 0 and creates one active NTFS partition (BIOS/MBR; the QEMU test).
    $diskConfig = @'
<DiskConfiguration>
        <Disk wcm:action="add">
          <DiskID>0</DiskID>
          <WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action="add"><Order>1</Order><Type>Primary</Type><Extend>true</Extend></CreatePartition>
          </CreatePartitions>
          <ModifyPartitions>
            <ModifyPartition wcm:action="add"><Order>1</Order><PartitionID>1</PartitionID><Format>NTFS</Format><Label>Windows</Label><Letter>C</Letter><Active>true</Active></ModifyPartition>
          </ModifyPartitions>
        </Disk>
        <WillShowUI>OnError</WillShowUI>
      </DiskConfiguration>
'@
    $installTo = @'
<InstallTo>
            <DiskID>0</DiskID>
            <PartitionID>1</PartitionID>
          </InstallTo>
'@
    L 'AutoDisk: disk 0 WILL BE WIPED (BIOS/MBR, one active NTFS partition)'
    L 'AutoDisk: for UEFI/GPT this needs EFI+MSR partitions; do not use this mode there.'
} else {
    $diskConfig = ''
    $installTo = ''
}
$tpl = $tpl.Replace('<!-- {{DISKCONFIG}} -->', $diskConfig).Replace('<!-- {{INSTALLTO}} -->', $installTo)
Set-Content $aa $tpl -Encoding UTF8
L 'autounattend.xml written (local admin + PostOobe + FirstLogonCommands)'

if ($StageOnly) {
    L "Staged only: $WorkDir (no ISO built)."
    exit 0
}

# --- rebuild the ISO (IMAPI2, BIOS boot) ---------------------------------------------------------
if (Test-Path $OutIso) { Remove-Item $OutIso -Force }
$outDir = Split-Path $OutIso -Parent
New-Item -ItemType Directory $outDir -Force | Out-Null
$bootFile = Join-Path $WorkDir 'boot\etfsboot.com'
Need $bootFile 'BIOS boot sector (boot\etfsboot.com)'

Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;

public static class IsoStream
{
    public static void Copy(object comStream, string path)
    {
        var src = (IStream)comStream;
        using (var fs = new FileStream(path, FileMode.Create, FileAccess.Write, FileShare.None))
        {
            var buf = new byte[65536];
            IntPtr read = Marshal.AllocHGlobal(4);
            try
            {
                int n;
                do
                {
                    src.Read(buf, buf.Length, read);
                    n = Marshal.ReadInt32(read);
                    if (n > 0) fs.Write(buf, 0, n);
                } while (n > 0);
            }
            finally { Marshal.FreeHGlobal(read); }
        }
    }
}
'@

L 'Creating the IMAPI2 file system (UDF 1.02)...'
$fs = New-Object -ComObject IMAPI2FS.MsftFileSystemImage
$fs.FileSystemsToCreate = 4      # FsiFileSystemUDF
$fs.UDFRevision = 0x102
$fs.VolumeName = $VolumeLabel
$fs.FreeMediaBlocks = 0

$boot = New-Object -ComObject IMAPI2FS.BootOptions
$bs = New-Object -ComObject ADODB.Stream
$bs.Type = 1
$bs.Open()
$bs.LoadFromFile($bootFile)
$boot.BootImage = $bs
$boot.PlatformId = 0             # BIOS
$boot.Emulation = 0              # no emulation
$boot.Manufacturer = 'WinRust'
$fs.BootOptions = $boot

L 'Adding the media tree (this takes a few minutes)...'
$null = $fs.Root.AddTree($WorkDir, $false)
L 'Building the image...'
$img = $fs.CreateResultImage()
[IsoStream]::Copy($img.ImageStream, $OutIso)
$bs.Close()
L "ISO written: $OutIso"

$h = Get-FileHash $OutIso -Algorithm SHA256
$gb = [math]::Round((Get-Item $OutIso).Length / 1GB, 2)
L ("SHA256 {0}  ({1} GB)" -f $h.Hash, $gb)
if (-not $KeepWork) { L "Work dir kept for inspection: $WorkDir (delete it to free space)" }
