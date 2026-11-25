#Requires -Version 5.1
<#
.SYNOPSIS
    Takes one screenshot of the live WinRust session into %TEMP%\opencode\shots (docs/screenshots is filled by hand after
    looking at them). One step per call so each image can be checked before moving on.

.DESCRIPTION
    Window-level capture (PrintWindow, then the screen region when a layered flyout comes back empty), never a full-screen
    grab: the user's other windows are not photographed. Clicks are real mouse events on the bar, because the bar and its
    flyouts read mouse messages; the cursor is put back where it was.

.EXAMPLE
    .\shots.ps1 -Step bar
    .\shots.ps1 -Step start
    .\shots.ps1 -Step list
#>
param(
    [Parameter(Mandatory)][ValidateSet('bar', 'start', 'barmenu', 'notify', 'toast', 'menu', 'explorer', 'thispc', 'control', 'taskmgr', 'settings', 'calc', 'notepad', 'list')]
    [string]$Step,
    [string]$Out = (Join-Path $env:TEMP 'opencode\shots')
)

$ErrorActionPreference = 'Stop'
Add-Type -Path (Join-Path $PSScriptRoot 'WinShot.cs') -ReferencedAssemblies System.Drawing
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
New-Item -ItemType Directory $Out -Force | Out-Null

$primary = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$savedCursor = [WinShot]::Cursor()

function Bars {
    [WinShot]::All() | Where-Object { [WinShot]::ClassOf($_) -eq 'topbar' -and [WinShot]::Visible($_) } |
        ForEach-Object { [pscustomobject]@{ H = $_; R = [WinShot]::RectOf($_) } } | Sort-Object { $_.R[1] }
}

# The bars dodge windows: the pointer has to touch the last pixel row of the screen for the primary one to slide in
# (`pt.y >= m.bottom - 1` in over_bar); once it is up, any point over the bar keeps it there.
function Reveal {
    [WinShot]::Move($primary.X + [int]($primary.Width / 2), $primary.Bottom - 1)
    $end = (Get-Date).AddSeconds(3)
    do {
        Start-Sleep -Milliseconds 150
        if ((Bars | Measure-Object).Count -gt 0) { Start-Sleep -Milliseconds 350; return }
    } while ((Get-Date) -lt $end)
    throw 'no bar came back with the pointer at the edge'
}

function PrimaryBar {
    $b = Bars | Where-Object { $_.R[0] -eq $primary.X } | Select-Object -First 1
    if (-not $b) { throw 'no primary bar after Reveal' }
    $b
}

function Wait-Win([string]$cls, [int]$seconds = 3) {
    $end = (Get-Date).AddSeconds($seconds)
    do {
        $h = [WinShot]::Find($cls, '', 0)
        if ($h -ne [IntPtr]::Zero) { return $h }
        Start-Sleep -Milliseconds 60
    } while ((Get-Date) -lt $end)
    throw "window $cls did not appear"
}

function Save([IntPtr]$h, [string]$name, [int]$pad = 0, [bool]$screen = $false) {
    $p = Join-Path $Out "$name.png"
    if (-not [WinShot]::Shot($h, $p, $screen, $pad)) { throw "capture failed: $name" }
    $r = [WinShot]::RectOf($h)
    "saved $p  ({0}x{1} at {2},{3})" -f $r[2], $r[3], $r[0], $r[1]
}

function Close-Flyout { [WinShot]::Key(0x1B); Start-Sleep -Milliseconds 250 }   # Esc: the flyout has the focus

function App([string]$exe, [string[]]$argl, [string]$name, [int]$pad = 0) {
    $sp = @{ FilePath = $exe; PassThru = $true }
    if ($argl -and $argl.Count -gt 0) { $sp.ArgumentList = $argl }
    $p = Start-Process @sp
    try {
        $h = [IntPtr]::Zero
        $end = (Get-Date).AddSeconds(12)
        do {
            Start-Sleep -Milliseconds 120
            $h = [WinShot]::MainOf([uint32]$p.Id)
        } while ($h -eq [IntPtr]::Zero -and (Get-Date) -lt $end)
        if ($h -eq [IntPtr]::Zero) { throw "no window from $exe" }
        Start-Sleep -Milliseconds 900   # let it paint its first frame
        Save $h $name $pad
    }
    finally {
        if (-not $p.HasExited) { $p.CloseMainWindow() | Out-Null; Start-Sleep -Milliseconds 400; if (-not $p.HasExited) { $p.Kill() } }
    }
}

try {
    switch ($Step) {
        'list' {
            foreach ($b in Bars) { "bar h={0} {1},{2} {3}x{4}" -f $b.H, $b.R[0], $b.R[1], $b.R[2], $b.R[3] }
            foreach ($t in 'topbar_fly', 'topbar_notify', 'topbar_toast', 'WinRustMenu', 'WinRustControl') {
                $h = [WinShot]::Find($t, '', 0)
                if ($h -ne [IntPtr]::Zero) { $r = [WinShot]::RectOf($h); "{0} h={1} {2},{3} {4}x{5}" -f $t, $h, $r[0], $r[1], $r[2], $r[3] }
            }
        }
        'bar' {
            Reveal
            $bars = Bars
            if ($bars.Count -lt 1) { throw 'no bar visible' }
            # each monitor's bar, side by side in one image: the widest one rules, the others are padded
            $w = [int](($bars | ForEach-Object { $_.R[2] } | Measure-Object -Maximum).Maximum)
            $h = [int](($bars | ForEach-Object { $_.R[3] } | Measure-Object -Sum).Sum) + 8 * ($bars.Count - 1)
            $bmp = New-Object Drawing.Bitmap $w, $h
            $g = [Drawing.Graphics]::FromImage($bmp)
            $g.Clear([Drawing.Color]::FromArgb(255, 24, 24, 28))
            $y = 0
            foreach ($b in $bars) {
                $one = Join-Path $Out ("_bar" + $b.R[0] + ".png")
                [WinShot]::Shot($b.H, $one, $true, 0) | Out-Null
                $img = [Drawing.Image]::FromFile($one)
                $g.DrawImage($img, 0, $y, $b.R[2], $b.R[3])
                $img.Dispose()
                Remove-Item $one -Force
                $y += $b.R[3] + 8
            }
            $g.Dispose()
            $p = Join-Path $Out 'bar.png'
            $bmp.Save($p, [Drawing.Imaging.ImageFormat]::Png)
            $bmp.Dispose()
            "saved $p ($w x $h, $($bars.Count) bar(s))"
        }
        'start' {
            Reveal
            $b = PrimaryBar
            [WinShot]::Click($b.R[0] + 22, $b.R[1] + [int]($b.R[3] / 2), $false)
            $h = Wait-Win 'topbar_fly'
            Start-Sleep -Milliseconds 450   # its open animation
            [WinShot]::Move(2, 2)           # no hover highlight in the shot, and no bar strip in the pad
            Start-Sleep -Milliseconds 150
            Save $h 'start' 0 $true
            Close-Flyout
        }
        'barmenu' {
            Reveal
            $b = PrimaryBar
            [WinShot]::Click($b.R[0] + [int]($b.R[2] / 3), $b.R[1] + [int]($b.R[3] / 2), $true)
            $h = Wait-Win 'topbar_fly'
            Start-Sleep -Milliseconds 400
            Save $h 'bar-menu' 14 $true
            Close-Flyout
        }
        'notify' {
            Reveal
            $b = PrimaryBar
            [WinShot]::Click($b.R[0] + $b.R[2] - 46, $b.R[1] + [int]($b.R[3] / 2), $false)
            $h = Wait-Win 'topbar_notify'
            Start-Sleep -Milliseconds 450
            Save $h 'notifications' 16 $true
            Close-Flyout
        }
        'toast' {
            $n = 'C:\Program Files\topbar\rs-notify.exe'
            & $n --app 'Steam' --title 'Download complete' --body 'Blur — 5.4 GB of 5.4 GB' --icon 'C:\Program Files\photoview\photoview.exe' --timeout 12000
            & $n --app 'WinRust' --title 'Update installed' --body 'v0.5.9 — the shell restarts at the next sign-in' --timeout 12000
            Start-Sleep -Milliseconds 900
            $h = [WinShot]::Find('topbar_toast', '', 0)
            if ($h -eq [IntPtr]::Zero) { throw 'no toast' }
            Start-Sleep -Milliseconds 350
            Save $h 'toasts' 18 $true
        }
        'menu' {
            # right-click on the first desktop icon
            [WinShot]::Move($primary.X + 40, 46)
            Start-Sleep -Milliseconds 120
            [WinShot]::Click($primary.X + 40, 46, $true)
            $h = Wait-Win 'WinRustMenu'
            Start-Sleep -Milliseconds 400
            Save $h 'context-menu' 12 $true
            Close-Flyout
        }
        'explorer' {
            App 'C:\Program Files\rust-explorer\rust-explorer.exe' @('C:\winrust-build\plugins') 'explorer'
        }
        'thispc' {
            App 'C:\Program Files\rust-explorer\rust-explorer.exe' @() 'this-pc'
        }
        'control' {
            App 'C:\Program Files\rust-control\rust-control.exe' @() 'control-panel'
        }
        'taskmgr' {
            App 'C:\Program Files\rtaskmgr\rtaskmgr-win32.exe' @() 'task-manager'
        }
        'settings' {
            App 'C:\Program Files\rust-settings\rust-settings.exe' @() 'settings'
        }
        'calc' {
            App 'C:\Program Files\rs-calc\rs-calc.exe' @() 'calc'
        }
        'notepad' {
            $f = Join-Path $env:TEMP 'winrust-shot.txt'
            Set-Content $f "WinRust`n`nThe shell is Rust: bar, desktop, Start menu, file manager, settings,`nnotifications and the screenshot tool.`n`nThis window is rust-notepad." -Encoding UTF8
            App 'C:\Program Files\rust-notepad\rust-notepad.exe' @($f) 'notepad'
        }
    }
}
finally {
    [WinShot]::Move($savedCursor[0], $savedCursor[1])
}
