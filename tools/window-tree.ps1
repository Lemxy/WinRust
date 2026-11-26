# window-tree.ps1 - what a wallpaper app (Wallpaper Engine, Lively) sees of this desktop.
#
# Dumps every top-level window of class Progman / WorkerW / SHELLDLL_DefView and their whole child tree:
# class, title, rectangle, styles, visibility, owner process. Also prints GetShellWindow().
#
#   powershell -ExecutionPolicy Bypass -File tools\window-tree.ps1
#   powershell -ExecutionPolicy Bypass -File tools\window-tree.ps1 -Root 1234     # one hwnd instead of the classes
#
# Read-only: no window is moved, shown, hidden or sent a message.
param([long]$Root = 0)

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class WT {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumProc cb, IntPtr lp);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr h, int i);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern IntPtr GetShellWindow();
    public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

function WinClass($h) { $b = New-Object Text.StringBuilder 256; [void][WT]::GetClassName($h, $b, 256); $b.ToString() }
function WinTitle($h) { $b = New-Object Text.StringBuilder 512; [void][WT]::GetWindowText($h, $b, 512); $b.ToString() }

function dump($h, [int]$depth) {
    $r = New-Object WT+RECT
    [void][WT]::GetWindowRect($h, [ref]$r)
    $procId = [uint32]0
    [void][WT]::GetWindowThreadProcessId($h, [ref]$procId)
    $proc = "?"
    try { $proc = (Get-Process -Id $procId -ErrorAction Stop).ProcessName } catch { }
    $style = ([int64][WT]::GetWindowLongPtr($h, -16)) -band 0xFFFFFFFFL
    $ex = ([int64][WT]::GetWindowLongPtr($h, -20)) -band 0xFFFFFFFFL
    $vis = "hidden"
    if ([WT]::IsWindowVisible($h)) { $vis = "visible" }
    $t = WinTitle $h
    if ($t.Length -gt 60) { $t = $t.Substring(0, 57) + "..." }
    $pad = "  " * $depth
    $line = $pad + (WinClass $h) + ' "' + $t + '"  ' + $r.Left + "," + $r.Top + " " + ($r.Right - $r.Left) + "x" + ($r.Bottom - $r.Top)
    $line += "  style=0x" + $style.ToString("X8") + " ex=0x" + $ex.ToString("X8") + "  " + $vis
    $line += "  pid=" + $procId + " " + $proc
    $line
    $kids = New-Object System.Collections.ArrayList
    [void][WT]::EnumChildWindows($h, { param($c, $l) [void]$kids.Add($c); $true }, [IntPtr]::Zero)
    foreach ($c in $kids) {
        # EnumChildWindows is recursive: print only the direct children here, the recursion does the rest
        if ([WT]::GetParent($c) -eq $h) { dump $c ($depth + 1) }
    }
}

$shell = [WT]::GetShellWindow()
if ($shell -ne [IntPtr]::Zero) {
    "GetShellWindow() = 0x" + ([int64]$shell).ToString("X") + ' "' + (WinTitle $shell) + '"'
} else {
    "GetShellWindow() = NULL"
}
""
if ($Root -ne 0) {
    dump ([IntPtr]$Root) 0
} else {
    $tops = New-Object System.Collections.ArrayList
    [void][WT]::EnumWindows({ param($h, $l)
        if (@("Progman", "WorkerW", "SHELLDLL_DefView") -contains (WinClass $h)) { [void]$tops.Add($h) }
        $true }, [IntPtr]::Zero)
    if ($tops.Count -eq 0) { "no Progman / WorkerW / SHELLDLL_DefView top-level window" }
    foreach ($h in $tops) { dump $h 0; "" }
}

