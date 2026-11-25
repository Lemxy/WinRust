#Requires -Version 5.1
<#
.SYNOPSIS
    Talks to the QEMU HMP monitor of the ISO test (TCP 4445): screenshot and keys.

.EXAMPLE
    .\vm-hmp.ps1 -Screen
    .\vm-hmp.ps1 -Cmd "sendkey shift-f10"
    .\vm-hmp.ps1 -Text "reg add hklm\vmsystem\setup\status\childcompletion /v setup.exe /t reg_dword /d 3 /f" -Enter
#>
param(
    [string]$Cmd = '',
    [switch]$Screen,
    [string]$Text = '',
    [switch]$Enter,
    [string]$Out = (Join-Path $env:TEMP 'vm-screen.png'),
    [int]$Port = 4445
)

$ErrorActionPreference = 'Stop'
$ffmpeg = 'C:\Program Files\ffmpeg\ffmpeg-9.1-essentials_build\bin\ffmpeg.exe'

function Hmp([string]$line) {
    $tcp = New-Object Net.Sockets.TcpClient('127.0.0.1', $Port)
    $st = $tcp.GetStream()
    $w = New-Object IO.StreamWriter($st)
    $w.AutoFlush = $true
    $w.WriteLine($line)
    Start-Sleep -Milliseconds 350
    $w.Close()
    $tcp.Close()
}

if ($Cmd) { Hmp $Cmd }

if ($Screen) {
    $ppm = Join-Path $env:TEMP 'vm.ppm'
    if (Test-Path $ppm) { Remove-Item $ppm -Force }
    Hmp "screendump $ppm"
    Start-Sleep -Milliseconds 600
    if (-not (Test-Path $ppm)) { throw 'screendump did not write the PPM (is QEMU running with -monitor tcp:...?)' }
    & $ffmpeg -y -loglevel error -i $ppm $Out
    Write-Output "screen: $Out"
}

# QEMU key names for the characters that are not a plain key.
$named = @{
    ' ' = 'spc'; '.' = 'dot'; ',' = 'comma'; '\' = 'backslash'; '/' = 'slash'; ':' = 'shift-semicolon'
    ';' = 'semicolon'; '-' = 'minus'; '_' = 'shift-minus'; '=' = 'equal'; '+' = 'shift-equal'
    '"' = 'shift-apostrophe'; "'" = 'apostrophe'; '(' = 'shift-9'; ')' = 'shift-0'
    '[' = 'bracket_left'; ']' = 'bracket_right'; '{' = 'shift-bracket_left'; '}' = 'shift-bracket_right'
    '<' = 'shift-comma'; '>' = 'shift-dot'; '?' = 'shift-slash'; '|' = 'shift-backslash'
    '@' = 'shift-2'; '#' = 'shift-3'; '$' = 'shift-4'; '%' = 'shift-5'; '^' = 'shift-6'
    '&' = 'shift-7'; '*' = 'shift-8'; '!' = 'shift-1'; '~' = 'shift-grave_accent'; '`' = 'grave_accent'
}

function KeyName([char]$ch) {
    $s = [string]$ch
    if ($named.ContainsKey($s)) { return $named[$s] }
    if ($ch -cmatch '[A-Z]') { return 'shift-' + $s.ToLower() }
    if ($ch -cmatch '[a-z0-9]') { return $s }
    throw "no key name for '$s'"
}

if ($Text) {
    $tcp = New-Object Net.Sockets.TcpClient('127.0.0.1', $Port)
    $st = $tcp.GetStream()
    $w = New-Object IO.StreamWriter($st)
    $w.AutoFlush = $true
    Start-Sleep -Milliseconds 120
    foreach ($ch in $Text.ToCharArray()) {
        $w.WriteLine('sendkey ' + (KeyName $ch))
        Start-Sleep -Milliseconds 30
    }
    $w.Close()
    $tcp.Close()
}
if ($Enter) { Hmp 'sendkey ret' }
