param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot)
)

$dir = Join-Path $Root 'es-chunks'
$out = Join-Path $Root 'common\es.tsv'

$map = @{}
$problems = New-Object System.Collections.Generic.List[string]

for ($i = 1; $i -le 6; $i++) {
    $srcFile = Join-Path $dir "chunk$i.tsv"
    $esFile  = Join-Path $dir "chunk$i.es.tsv"
    if (-not (Test-Path $esFile)) { $problems.Add("FALTA chunk$i.es.tsv"); continue }

    $src = @{}
    foreach ($l in (Get-Content $srcFile -Encoding utf8)) {
        $p = $l -split "`t", 2
        if ($p.Count -eq 2) { $src[$p[0]] = $p[1] }
    }
    $es = @{}
    foreach ($l in (Get-Content $esFile -Encoding utf8)) {
        $p = $l -split "`t", 2
        if ($p.Count -eq 2) { $es[$p[0]] = $p[1] }
    }

    foreach ($id in $src.Keys) {
        if (-not $es.ContainsKey($id)) { $problems.Add("chunk$i id $id sin traduccion"); continue }
        $en = $src[$id]
        $sp = $es[$id]

        # marcadores de formato deben coincidir
        $enTok = ([regex]::Matches($en, '\{[^{}]*\}') | ForEach-Object { $_.Value } | Sort-Object) -join '|'
        $esTok = ([regex]::Matches($sp, '\{[^{}]*\}') | ForEach-Object { $_.Value } | Sort-Object) -join '|'
        if ($enTok -ne $esTok) { $problems.Add("chunk$i id $id marcadores: EN[$enTok] ES[$esTok]"); continue }

        # secuencias de escape conservadas (las comillas pueden pasar a «» o “”: no son parte de la clave)
        $enEsc = ([regex]::Matches($en, '\\[nrt\\]|\\u\{[0-9A-Fa-f]+\}') | ForEach-Object { $_.Value } | Sort-Object) -join '|'
        $esEsc = ([regex]::Matches($sp, '\\[nrt\\]|\\u\{[0-9A-Fa-f]+\}') | ForEach-Object { $_.Value } | Sort-Object) -join '|'
        if ($enEsc -ne $esEsc) { $problems.Add("chunk$i id $id escapes: EN[$enEsc] ES[$esEsc]"); continue }

        if ($sp -match "`t") { $problems.Add("chunk$i id $id tiene tabulador interno"); continue }

        if ($map.ContainsKey($en) -and $map[$en] -ne $sp) { $problems.Add("id ${id}: '$en' traducido de dos formas: '$($map[$en])' / '$sp'"); continue }
        $map[$en] = $sp
    }
}

$lines = $map.Keys | Sort-Object | ForEach-Object { "$_`t$($map[$_])" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllLines($out, [string[]]$lines, $utf8)
"Entradas en es.tsv : $($map.Count)"
"Problemas          : $($problems.Count)"
$problems | Select-Object -First 30
