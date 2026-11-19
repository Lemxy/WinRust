# Rebuilds the plugins (cargo only recompiles what changed), refreshes payload\ and runs `plugin-launcher update`: no launcher rebuild.
#   scripts\update-plugin.ps1 rust-search|rtaskmgr-win32|topbar|rust-explorer|wiztree|photoview|debloat
# Build + copy need no admin. The update step does: from a normal shell this script asks for it via a UAC prompt (the user clicks Yes),
# then prints the last lines of %ProgramData%\plugin-launcher\install.log (the elevated window's own output is lost).
param([Parameter(Mandatory)][ValidateSet('rust-search', 'rtaskmgr-win32', 'topbar', 'rust-explorer', 'wiztree', 'photoview', 'debloat')][string]$name)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
& "$PSScriptRoot\build-plugins.ps1" # rebuilds what changed (and what ships with the plugin), refreshes payload\
$exe = "$root\target\release\plugin-launcher.exe"
if (-not (Test-Path $exe)) { throw "build the launcher once first: cargo build --release in $root" }
$log = "$env:ProgramData\plugin-launcher\install.log"
$before = if (Test-Path $log) { @(Get-Content $log -Encoding UTF8).Count } else { 0 }
# Fast path: if the elevated scheduled task `winrust-update-topbar` is registered, trigger it directly without UAC (~1 s).
if ($name -eq 'topbar') {
    cmd /c "schtasks /query /tn winrust-update-topbar >nul 2>&1"
    if ($LASTEXITCODE -eq 0) {
        schtasks /run /tn winrust-update-topbar | Out-Null
        for ($i = 0; $i -lt 15; $i++) {
            Start-Sleep -Milliseconds 300
            $new = if (Test-Path $log) { @(Get-Content $log -Encoding UTF8 | Select-Object -Skip $before) } else { @() }
            if ($new -match "^\[\d+\] topbar: ") { return ($new | Select-Object -Last 2) }
        }
    }
}
$lnk = "$root\target\update-$name.lnk"
$s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk); $s.TargetPath = $exe; $s.Arguments = "update $name"; $s.Save()
Start-Process explorer.exe -ArgumentList "`"$lnk`""
# explorer returns at once: wait for the launcher's verdict in the log (the user may take a while to answer UAC)
$deadline = (Get-Date).AddMinutes(5)
do {
    Start-Sleep 2
    $new = if (Test-Path $log) { @(Get-Content $log -Encoding UTF8 | Select-Object -Skip $before) } else { @() }
} until (($new -match "^\[\d+\] ${name}: ") -or (Get-Date) -gt $deadline)
Remove-Item $lnk -ErrorAction SilentlyContinue
if (-not ($new -match "^\[\d+\] ${name}: ")) { Write-Warning "no result in $log after 5 minutes (UAC declined?)" }
$new | Select-Object -Last 4
