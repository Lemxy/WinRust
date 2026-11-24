param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot),
    [string]$Out  = (Join-Path (Split-Path -Parent $PSScriptRoot) 'es-strings.tsv')
)

# Extrae el literal ingles de cada t!(...) / tf!(...) del proyecto.
# Salida: english<TAB>archivo:linea por cada aparicion.

function Split-Args([string]$s, [int]$from, [int]$to) {
    $args = New-Object System.Collections.Generic.List[string]
    $depth = 0; $i = $from; $start = $from; $inStr = $false
    while ($i -lt $to) {
        $c = $s[$i]
        if ($inStr) {
            if ($c -eq '\') { $i += 2; continue }
            if ($c -eq '"') { $inStr = $false }
            $i++; continue
        }
        switch ($c) {
            '"' { $inStr = $true }
            '(' { $depth++ }
            ')' { $depth-- }
            '[' { $depth++ }
            ']' { $depth-- }
            '{' { $depth++ }
            '}' { $depth-- }
            ',' { if ($depth -eq 0) { $args.Add($s.Substring($start, $i - $start)); $start = $i + 1 } }
        }
        $i++
    }
    if ($start -lt $to) { $args.Add($s.Substring($start, $to - $start)) }
    return ,$args
}

$files = Get-ChildItem $Root -Recurse -Include *.rs -File | Where-Object { $_.FullName -notmatch '\\target\\' }
$rx = [regex]'(?<![A-Za-z0-9_])(tf|t)!\('

$rows = New-Object System.Collections.Generic.List[string]
$unparsed = 0
$calls = 0

foreach ($f in $files) {
    $text = [System.IO.File]::ReadAllText($f.FullName)
    $pos = 0
    while ($true) {
        $m = $rx.Match($text, $pos)
        if (-not $m.Success) { break }
        $t = $m.Index
        $open = $t + $m.Length - 1   # indice del '('
        $calls++

        $depth = 0; $i = $open; $inStr = $false; $close = -1
        while ($i -lt $text.Length) {
            $c = $text[$i]
            if ($inStr) {
                if ($c -eq '\') { $i += 2; continue }
                if ($c -eq '"') { $inStr = $false }
                $i++; continue
            }
            if ($c -eq '"') { $inStr = $true; $i++; continue }
            if ($c -eq '(') { $depth++ }
            elseif ($c -eq ')') { $depth--; if ($depth -eq 0) { $close = $i; break } }
            $i++
        }
        if ($close -lt 0) { $unparsed++; $pos = $open + 1; continue }

        $args = Split-Args $text ($open + 1) $close
        if ($args.Count -ge 2) {
            $en = $args[1].Trim() -replace '^(ja|pt|zh|es)\s*:\s*', ''
            if ($en.StartsWith('"')) {
                $j = 1; $sb = New-Object System.Text.StringBuilder; $ok = $false
                while ($j -lt $en.Length) {
                    $c = $en[$j]
                    if ($c -eq '\') { [void]$sb.Append($en[$j]); [void]$sb.Append($en[$j + 1]); $j += 2; continue }
                    if ($c -eq '"') { $ok = $true; break }
                    [void]$sb.Append($c); $j++
                }
                if ($ok) {
                    $lit = $sb.ToString()
                    $line = ($text.Substring(0, $t).Split("`n").Count)
                    $rel = $f.FullName.Substring($Root.Length).TrimStart('\')
                    if ($lit -notmatch '^\s*$') { $rows.Add("$lit`t$rel`:$line") }
                } else { $unparsed++ }
            } else { $unparsed++ }
        } else { $unparsed++ }
        # sigue dentro de la llamada para capturar t!/tf! anidados
        $pos = $open + 1
    }
}

$rows | Sort-Object -Unique | Set-Content -Path $Out -Encoding utf8
$uniq = ($rows | ForEach-Object { ($_ -split "`t")[0] } | Sort-Object -Unique).Count
"Llamadas vistas      : $calls"
"Extraidas            : $($rows.Count)"
"Sin parsear          : $unparsed"
"Cadenas unicas (en)  : $uniq"
"Archivo              : $Out"
