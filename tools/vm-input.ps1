#Requires -Version 5.1
<#
.SYNOPSIS
    Drives the QEMU window of the WinRust ISO test: focus, screenshot, text and key combos.

.DESCRIPTION
    Test-only helper (host side). It never runs inside the VM: it focuses the QEMU window and injects keyboard/mouse
    input, so OOBE and the checklist can be driven without a person at the keyboard.

    Examples:
        .\vm-input.ps1 -Shot                 # capture the window to $env:TEMP\qemu-shot.png
        .\vm-input.ps1 -Focus                # bring QEMU to the front and click its centre
        .\vm-input.ps1 -Text "hola"          # type text (Unicode, any layout)
        .\vm-input.ps1 -Keys Enter           # one key
        .\vm-input.ps1 -Keys Win+R           # a combo
        .\vm-input.ps1 -Text "powershell" -Keys Enter -Delay 400
#>
param(
    [switch]$Focus,
    [switch]$Shot,
    [string]$Text = '',
    [string[]]$Keys = @(),
    [int]$Delay = 120,
    [string]$Out = (Join-Path $env:TEMP 'qemu-shot.png')
)

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class VmInput {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool f);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, int x, int y, int d, IntPtr e);
    [DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] p, int size);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int l, t, r, b; }
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Explicit)] public struct INPUTUNION { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public INPUTUNION u; }

    public const uint KEYEVENTF_KEYUP = 0x0002;
    public const uint KEYEVENTF_UNICODE = 0x0004;

    public static void FocusWindow(IntPtr h) {
        uint p; var t = GetWindowThreadProcessId(GetForegroundWindow(), out p); var c = GetCurrentThreadId();
        AttachThreadInput(c, t, true);
        SetForegroundWindow(h);
        AttachThreadInput(c, t, false);
    }
    public static void ClickCenter(IntPtr h) {
        RECT r; GetWindowRect(h, out r);
        int x = (r.l + r.r) / 2, y = (r.t + r.b) / 2;
        SetCursorPos(x, y);
        mouse_event(0x0002, 0, 0, 0, IntPtr.Zero); // left down
        mouse_event(0x0004, 0, 0, 0, IntPtr.Zero); // left up
    }
    public static void Key(ushort vk, bool up) {
        var i = new INPUT { type = 1 };
        i.u.ki.wVk = vk;
        i.u.ki.dwFlags = up ? KEYEVENTF_KEYUP : 0;
        SendInput(1, new[] { i }, Marshal.SizeOf(typeof(INPUT)));
    }
    public static void Char(char ch) {
        var i = new INPUT { type = 1 };
        i.u.ki.wScan = ch;
        i.u.ki.dwFlags = KEYEVENTF_UNICODE;
        SendInput(1, new[] { i }, Marshal.SizeOf(typeof(INPUT)));
        i.u.ki.dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
        SendInput(1, new[] { i }, Marshal.SizeOf(typeof(INPUT)));
    }
}
'@

$p = Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $p) { throw 'QEMU is not running (qemu-system-x86_64 with a window)' }
$h = [IntPtr]$p.MainWindowHandle

$vk = @{
    'enter' = 0x0D; 'return' = 0x0D; 'tab' = 0x09; 'esc' = 0x1B; 'escape' = 0x1B; 'space' = 0x20
    'back' = 0x08; 'backspace' = 0x08; 'delete' = 0x2E; 'del' = 0x2E
    'up' = 0x26; 'down' = 0x28; 'left' = 0x25; 'right' = 0x27
    'home' = 0x24; 'end' = 0x23; 'pgup' = 0x21; 'pgdn' = 0x22
    'win' = 0x5B; 'shift' = 0x10; 'ctrl' = 0x11; 'control' = 0x11; 'alt' = 0x12
    'f10' = 0x79; 'f11' = 0x7A; 'f12' = 0x7B
    'f1' = 0x70; 'f2' = 0x71; 'f3' = 0x72; 'f4' = 0x73; 'f5' = 0x74; 'f6' = 0x75; 'f7' = 0x76; 'f8' = 0x77; 'f9' = 0x78
}
for ($i = 0; $i -le 9; $i++) { $vk["$i"] = 0x30 + $i }
foreach ($c in [char[]]'abcdefghijklmnopqrstuvwxyz') { $vk["$c"] = [int][char]([string]$c).ToUpper() }

if ($Focus) {
    [VmInput]::FocusWindow($h)
    Start-Sleep -Milliseconds 250
    [VmInput]::ClickCenter($h)
    Start-Sleep -Milliseconds 250
}

if ($Shot) {
    $r = New-Object VmInput+RECT
    [void][VmInput]::GetWindowRect($h, [ref]$r)
    $w = $r.r - $r.l; $ht = $r.b - $r.t
    if ($w -le 0 -or $ht -le 0) { throw 'QEMU window has no size' }
    $bmp = New-Object System.Drawing.Bitmap $w, $ht
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.l, $r.t, 0, 0, $bmp.Size)
    $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Write-Output "shot: $Out"
}

if ($Text) {
    foreach ($ch in $Text.ToCharArray()) {
        if ($ch -eq "`n" -or $ch -eq "`r") { [VmInput]::Key(0x0D, $false); [VmInput]::Key(0x0D, $true) }
        else { [VmInput]::Char($ch) }
        Start-Sleep -Milliseconds 12
    }
}

foreach ($combo in $Keys) {
    $names = $combo -split '\+'
    $codes = @()
    foreach ($n in $names) {
        $k = $n.Trim().ToLower()
        if (-not $vk.ContainsKey($k)) { throw "unknown key '$n' (use: $($vk.Keys -join ', '))" }
        $codes += $vk[$k]
    }
    foreach ($c in $codes) { [VmInput]::Key([UInt16]$c, $false) }
    [Array]::Reverse($codes)
    Start-Sleep -Milliseconds $Delay
    foreach ($c in $codes) { [VmInput]::Key([UInt16]$c, $true) }
    Start-Sleep -Milliseconds $Delay
}
