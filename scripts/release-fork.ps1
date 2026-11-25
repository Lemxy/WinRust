# Builds a fork release (RexxColder/WinRust) without touching git: no commit, no tag, no push, no `gh release create`.
#   powershell -ExecutionPolicy Bypass -File scripts\release-fork.ps1                 # current version + 1 patch
#   powershell -ExecutionPolicy Bypass -File scripts\release-fork.ps1 -Version v0.5.9  # explicit version
#   ... -Repo someone/WinRust                                                          # another fork of the fork
# Bumps Cargo.toml, runs build-all.ps1, copies plugin-launcher.exe to WinRust.exe, and signs manifest.txt with
# %USERPROFILE%\.winrust\update-signing.key (WINRUST_SIGNING_KEY overrides it) when the key exists. Without the key the
# release is still usable by hand, but the auto-updater (src/update) will refuse it: it only installs signed manifests.
# Prints the SHA256 and the exact command that would publish; running it is the user's decision.
param([string]$Version, [string]$Repo = 'RexxColder/WinRust')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
Set-Location $root

$toml = Get-Content "$root\Cargo.toml" -Raw
$cur = [regex]::Match($toml, '(?m)^version = "([^"]+)"').Groups[1].Value
if (-not $Version) {
    $p = $cur.Split('.')
    if ($p.Count -ne 3 -or $p[2] -notmatch '^\d+$') { throw "Cargo.toml version '$cur' is not x.y.z: pass -Version" }
    $Version = 'v{0}.{1}.{2}' -f $p[0], $p[1], ([int]$p[2] + 1)
}
if ($Version -notmatch '^v\d+\.\d+\.\d+$') { throw 'version must look like v0.5.9' }
$number = $Version.TrimStart('v')
Write-Host "release: $Version (Cargo.toml had $cur, repo $Repo)"

# the installer shows CARGO_PKG_VERSION and the updater compares manifests against it
$toml = [regex]::Replace($toml, '(?m)^version = "[^"]*"', "version = `"$number`"", 1)
[IO.File]::WriteAllText("$root\Cargo.toml", $toml)

& "$PSScriptRoot\build-all.ps1"
if ($LASTEXITCODE) { throw 'build-all.ps1 failed' }

$dir = Join-Path $root "target\release\winrust-$Version"
if (Test-Path $dir) { Remove-Item $dir -Recurse -Force }
New-Item -ItemType Directory $dir | Out-Null
$exe = Join-Path $dir 'WinRust.exe'
Copy-Item "$root\target\release\plugin-launcher.exe" $exe -Force
$sha = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
$assets = @($exe)

$key = if ($env:WINRUST_SIGNING_KEY) { $env:WINRUST_SIGNING_KEY } else { Join-Path $env:USERPROFILE '.winrust\update-signing.key' }
$signed = $false
if (Test-Path $key) {
    cargo build --release -p winrust-sign
    if ($LASTEXITCODE) { throw 'winrust-sign build failed' }
    # only WinRust.exe is attached (the plugins ride inside it); the auto-updater accepts a manifest that lists it alone
    $manifest = Join-Path $dir 'manifest.txt'
    [IO.File]::WriteAllText($manifest, (@('winrust-manifest 1', "version $Version", "$sha WinRust.exe") -join "`n") + "`n", (New-Object Text.UTF8Encoding $false))
    & "$root\target\release\winrust-sign.exe" sign $manifest (Join-Path $dir 'manifest.sig')
    if ($LASTEXITCODE) { throw 'signing failed: broken key file?' }
    $assets += @($manifest, (Join-Path $dir 'manifest.sig'))
    $signed = $true
} else {
    Write-Host "release: no signing key at $key - the auto-updater cannot install this build" -ForegroundColor Yellow
}

Write-Host ''
Get-ChildItem $dir | ForEach-Object { '{0,-14} {1,12} bytes  {2}' -f $_.Name, $_.Length, (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower() }
Write-Host ''
Write-Host "WinRust.exe SHA256: $sha"
Write-Host "signed: $signed"

# printed, never run: no tag is pushed and no GitHub release is created here
$list = ($assets | ForEach-Object { '"' + $_ + '"' }) -join ' '
Write-Host ''
Write-Host 'publish with (not run by this script):' -ForegroundColor Cyan
Write-Host "gh release create $Version $list --repo $Repo --title `"WinRust $Version`" --generate-notes"
if (-not $signed) {
    Write-Host 'note: WinRust.exe alone is uploaded; without manifest.txt/manifest.sig the auto-updater stays on the installed build.' -ForegroundColor Yellow
}
