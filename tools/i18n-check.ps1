# i18n-check.ps1 — auditoría de traducciones de WinRust.
#
# Uso:
#   powershell -ExecutionPolicy Bypass -File tools\i18n-check.ps1
#   powershell -ExecutionPolicy Bypass -File tools\i18n-check.ps1 -Root C:\winrust-build
#
# Comprueba, sin modificar nada:
#   - llamadas t!/tf! a las que les falta ja:, pt: o zh:;
#   - marcadores de formato ({}, {0}, {:.0}, {name}...) que no cuadran entre idiomas;
#   - claves inglesas de t!/tf! que no están en common/es.tsv;
#   - entradas de common/es.tsv que ya no usa nadie;
#   - duplicados y líneas mal formadas de common/es.tsv;
#   - textos sospechosos: ja/zh sin caracteres CJK, ru sin cirílico, portugués de
#     Brasil colado en pt:, español colado en otros idiomas;
#   - variantes de terminología española (carpeta/directorio, unidad/disco...).
#
# Código de salida: 0 si no hay ningún problema, 1 si hay alguno.
# No necesita módulos ni dependencias: solo PowerShell 5.1 y .NET.

[CmdletBinding()]
param(
    # Raíz del repositorio (por defecto, la carpeta padre de tools\).
    [string]$Root = '',
    # No contar las entradas muertas como problema (siguen listándose).
    [switch]$UnusedIsWarning
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
# La consola de Windows suele estar en una página de códigos local: forzar UTF-8
# para que los acentos y el cirílico se lean bien al ejecutar el script a mano.
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false) } catch { }

if (-not $Root) {
    # En PowerShell 5.1, con [CmdletBinding()] $PSScriptRoot aún no está definido
    # al evaluar los valores por defecto: se resuelve aquí, ya en el cuerpo.
    $here = $PSScriptRoot
    if (-not $here) { $here = Split-Path -Parent $MyInvocation.MyCommand.Path }
    if (-not $here) { $here = (Get-Location).Path }
    $Root = Split-Path -Parent $here
}

if (-not (Test-Path -LiteralPath $Root)) {
    Write-Host "No existe la raíz: $Root" -ForegroundColor Red
    exit 1
}
$Root = (Resolve-Path -LiteralPath $Root).Path.TrimEnd('\')
$TsvPath = Join-Path $Root 'common\es.tsv'
if (-not (Test-Path -LiteralPath $TsvPath)) {
    Write-Host "No existe common\es.tsv en $Root" -ForegroundColor Red
    exit 1
}

# ─────────────────────────── utilidades de Rust ───────────────────────────

# Deshace los escapes de un literal Rust igual que common/src/es.rs, para poder
# comparar la clave de es.tsv con el texto inglés de la llamada.
function ConvertFrom-RustEscapes {
    param([string]$Text)
    $out = New-Object System.Text.StringBuilder
    $i = 0
    while ($i -lt $Text.Length) {
        $c = $Text[$i]
        if ($c -ne '\') { [void]$out.Append($c); $i++; continue }
        $i++
        if ($i -ge $Text.Length) { [void]$out.Append('\'); break }
        $e = $Text[$i]
        switch -CaseSensitive ($e) {
            'n' { [void]$out.Append("`n"); $i++ }
            'r' { [void]$out.Append("`r"); $i++ }
            't' { [void]$out.Append("`t"); $i++ }
            '\' { [void]$out.Append('\'); $i++ }
            '"' { [void]$out.Append('"'); $i++ }
            "'" { [void]$out.Append("'"); $i++ }
            '0' { [void]$out.Append([char]0); $i++ }
            'u' {
                $i++
                if ($i -lt $Text.Length -and $Text[$i] -eq '{') {
                    $i++
                    $hex = New-Object System.Text.StringBuilder
                    while ($i -lt $Text.Length -and $Text[$i] -ne '}') { [void]$hex.Append($Text[$i]); $i++ }
                    if ($i -lt $Text.Length) { $i++ }
                    $code = 0
                    if ([int]::TryParse($hex.ToString(), [System.Globalization.NumberStyles]::HexNumber, $null, [ref]$code)) {
                        [void]$out.Append([char]::ConvertFromUtf32($code))
                    } else {
                        [void]$out.Append('\u{').Append($hex.ToString()).Append('}')
                    }
                } else {
                    [void]$out.Append('\u')
                }
            }
            default { [void]$out.Append('\'); [void]$out.Append($e); $i++ }
        }
    }
    return $out.ToString()
}

# Devuelve el valor de un literal de cadena Rust (normal o raw), o $null si el
# argumento no es un literal simple.
function Get-RustLiteral {
    param([string]$Arg)
    $s = $Arg.Trim()
    if ($s.Length -lt 2) { return $null }

    # Cadena raw: r"..." o r#"..."# (también br#"..."#).
    $m = [regex]::Match($s, '^(?:b?r)(?<h>#*)"(?<body>[\s\S]*)"\k<h>$')
    if ($m.Success) { return $m.Groups['body'].Value }

    # Cadena normal: "..."; se exige que no queden caracteres detrás.
    if ($s[0] -eq '"') {
        $sb = New-Object System.Text.StringBuilder
        $i = 1
        $closed = $false
        while ($i -lt $s.Length) {
            $c = $s[$i]
            if ($c -eq '\') {
                if ($i + 1 -lt $s.Length) { [void]$sb.Append($c).Append($s[$i + 1]); $i += 2; continue }
                return $null
            }
            if ($c -eq '"') { $closed = ($i -eq $s.Length - 1); break }
            [void]$sb.Append($c)
            $i++
        }
        if ($closed) { return (ConvertFrom-RustEscapes $sb.ToString()) }
    }
    return $null
}

# Índice del paréntesis que cierra el que está en $Open, ignorando cadenas,
# caracteres y comentarios. Devuelve -1 si no aparece.
function Find-CloseParen {
    param([string]$Text, [int]$Open)
    $n = $Text.Length
    $depth = 0
    $i = $Open
    while ($i -lt $n) {
        $c = $Text[$i]
        if ($c -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '/') {
            $i = $Text.IndexOf("`n", $i)
            if ($i -lt 0) { return -1 }
            continue
        }
        if ($c -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '*') {
            $d = 1; $i += 2
            while ($i -lt $n -and $d -gt 0) {
                if ($Text[$i] -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '*') { $d++; $i += 2 }
                elseif ($Text[$i] -eq '*' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '/') { $d--; $i += 2 }
                else { $i++ }
            }
            continue
        }
        if ($c -eq '"') {
            $i++
            while ($i -lt $n) {
                if ($Text[$i] -eq '\') { $i += 2; continue }
                if ($Text[$i] -eq '"') { $i++; break }
                $i++
            }
            continue
        }
        if ($c -eq 'r' -or $c -eq 'b') {
            $j = $i
            if ($Text[$j] -eq 'b' -and $j + 1 -lt $n -and $Text[$j + 1] -eq 'r') { $j++ }
            if ($j -lt $n -and $Text[$j] -eq 'r') {
                $k = $j + 1; $h = 0
                while ($k -lt $n -and $Text[$k] -eq '#') { $h++; $k++ }
                if ($k -lt $n -and $Text[$k] -eq '"') {
                    $end = $Text.IndexOf('"' + ('#' * $h), $k + 1)
                    if ($end -lt 0) { return -1 }
                    $i = $end + 1 + $h
                    continue
                }
            }
        }
        if ($c -eq "'") {
            # Carácter literal ('x', '\n') frente a un tiempo de vida ('a).
            if ($i + 2 -lt $n -and $Text[$i + 1] -eq '\' -and $Text[$i + 3] -eq "'") { $i += 4; continue }
            if ($i + 2 -lt $n -and $Text[$i + 2] -eq "'") { $i += 3; continue }
            $i++
            continue
        }
        if ($c -eq '(') { $depth++ }
        elseif ($c -eq ')') { $depth--; if ($depth -eq 0) { return $i } }
        $i++
    }
    return -1
}

# Divide los argumentos de una macro por comas de primer nivel.
function Split-MacroArgs {
    param([string]$Text, [int]$Start, [int]$End)
    $args = New-Object System.Collections.Generic.List[string]
    $sb = New-Object System.Text.StringBuilder
    $n = $End
    $depth = 0
    $i = $Start
    while ($i -lt $n) {
        $c = $Text[$i]
        if ($c -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '/') {
            $i = $Text.IndexOf("`n", $i)
            if ($i -lt 0 -or $i -ge $n) { break }
            [void]$sb.Append(' ')
            continue
        }
        if ($c -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '*') {
            $d = 1; $i += 2
            while ($i -lt $n -and $d -gt 0) {
                if ($Text[$i] -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '*') { $d++; $i += 2 }
                elseif ($Text[$i] -eq '*' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '/') { $d--; $i += 2 }
                else { $i++ }
            }
            [void]$sb.Append(' ')
            continue
        }
        if ($c -eq '"') {
            $j = $i
            $i++
            while ($i -lt $n) {
                if ($Text[$i] -eq '\') { $i += 2; continue }
                if ($Text[$i] -eq '"') { $i++; break }
                $i++
            }
            [void]$sb.Append($Text.Substring($j, [Math]::Min($i, $n) - $j))
            continue
        }
        if ($c -eq 'r' -or $c -eq 'b') {
            $j = $i
            $k = $i
            if ($Text[$k] -eq 'b' -and $k + 1 -lt $n -and $Text[$k + 1] -eq 'r') { $k++ }
            if ($k -lt $n -and $Text[$k] -eq 'r') {
                $q = $k + 1; $h = 0
                while ($q -lt $n -and $Text[$q] -eq '#') { $h++; $q++ }
                if ($q -lt $n -and $Text[$q] -eq '"') {
                    $endq = $Text.IndexOf('"' + ('#' * $h), $q + 1)
                    if ($endq -lt 0) { $endq = $n - 1 }
                    $i = $endq + 1 + $h
                    [void]$sb.Append($Text.Substring($j, [Math]::Min($i, $n) - $j))
                    continue
                }
            }
        }
        if ($c -eq "'") {
            if ($i + 2 -lt $n -and $Text[$i + 1] -eq '\' -and $Text[$i + 3] -eq "'") {
                [void]$sb.Append($Text.Substring($i, 4)); $i += 4; continue
            }
            if ($i + 2 -lt $n -and $Text[$i + 2] -eq "'") {
                [void]$sb.Append($Text.Substring($i, 3)); $i += 3; continue
            }
        }
        if ($c -eq '(' -or $c -eq '[' -or $c -eq '{') { $depth++ }
        elseif ($c -eq ')' -or $c -eq ']' -or $c -eq '}') { $depth-- }
        if ($c -eq ',' -and $depth -eq 0) {
            $args.Add($sb.ToString().Trim())
            [void]$sb.Clear()
            $i++
            continue
        }
        [void]$sb.Append($c)
        $i++
    }
    $tail = $sb.ToString().Trim()
    if ($tail.Length -gt 0) { $args.Add($tail) }
    return ,$args
}

# Busca las llamadas t!/tf! que están en código (no en comentarios ni cadenas).
function Get-RustCalls {
    param([string]$Text)
    $calls = New-Object System.Collections.Generic.List[object]
    $n = $Text.Length
    $i = 0
    while ($i -lt $n) {
        $c = $Text[$i]

        if ($c -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '/') {
            $i = $Text.IndexOf("`n", $i)
            if ($i -lt 0) { break }
            continue
        }
        if ($c -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '*') {
            $d = 1; $i += 2
            while ($i -lt $n -and $d -gt 0) {
                if ($Text[$i] -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '*') { $d++; $i += 2 }
                elseif ($Text[$i] -eq '*' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '/') { $d--; $i += 2 }
                else { $i++ }
            }
            continue
        }
        if ($c -eq '"') {
            $i++
            while ($i -lt $n) {
                if ($Text[$i] -eq '\') { $i += 2; continue }
                if ($Text[$i] -eq '"') { $i++; break }
                $i++
            }
            continue
        }
        if ($c -eq "'") {
            if ($i + 2 -lt $n -and $Text[$i + 1] -eq '\' -and $Text[$i + 3] -eq "'") { $i += 4; continue }
            if ($i + 2 -lt $n -and $Text[$i + 2] -eq "'") { $i += 3; continue }
            $i++
            continue
        }
        if ($c -eq 'r' -or $c -eq 'b') {
            $j = $i
            if ($Text[$j] -eq 'b' -and $j + 1 -lt $n -and $Text[$j + 1] -eq 'r') { $j++ }
            if ($j -lt $n -and $Text[$j] -eq 'r') {
                $k = $j + 1; $h = 0
                while ($k -lt $n -and $Text[$k] -eq '#') { $h++; $k++ }
                if ($k -lt $n -and $Text[$k] -eq '"') {
                    $end = $Text.IndexOf('"' + ('#' * $h), $k + 1)
                    if ($end -lt 0) { break }
                    $i = $end + 1 + $h
                    continue
                }
            }
        }

        if ([char]::IsLetter($c) -or $c -eq '_') {
            $start = $i
            while ($i -lt $n -and ([char]::IsLetterOrDigit($Text[$i]) -or $Text[$i] -eq '_')) { $i++ }
            $ident = $Text.Substring($start, $i - $start)
            if ($ident -eq 't' -or $ident -eq 'tf') {
                $j = $i
                while ($j -lt $n -and [char]::IsWhiteSpace($Text[$j])) { $j++ }
                if ($j -lt $n -and $Text[$j] -eq '!') {
                    $j++
                    while ($j -lt $n -and [char]::IsWhiteSpace($Text[$j])) { $j++ }
                    if ($j -lt $n -and $Text[$j] -eq '(') {
                        $close = Find-CloseParen $Text $j
                        if ($close -gt $j) {
                            $calls.Add([pscustomobject]@{
                                Index = $start
                                Kind  = $ident
                                Open  = $j
                                Close = $close
                                Args  = (Split-MacroArgs $Text ($j + 1) $close)
                            })
                            # Se sigue dentro del paréntesis para encontrar macros anidadas.
                            $i = $j + 1
                            continue
                        }
                    }
                }
            }
            continue
        }
        $i++
    }
    return ,$calls
}

# Marcadores de formato de una plantilla, normalizados por tipo:
#   {} / {0} / {1} -> "pos"
#   {name}         -> "name:name"
#   {:.0} / {:>8}  -> "spec:...".
function Get-FormatShapes {
    param([string]$Text)
    $shapes = New-Object System.Collections.Generic.List[string]
    $i = 0
    while ($i -lt $Text.Length) {
        $c = $Text[$i]
        if ($c -eq '{') {
            if ($i + 1 -lt $Text.Length -and $Text[$i + 1] -eq '{') { $i += 2; continue }
            $j = $Text.IndexOf('}', $i + 1)
            if ($j -lt 0) { $shapes.Add('sin-cerrar'); break }
            $inner = $Text.Substring($i + 1, $j - $i - 1).Trim()
            $colon = $inner.IndexOf(':')
            if ($colon -ge 0) {
                $spec = $inner.Substring($colon + 1).Trim()
                $shapes.Add('spec:' + $spec)
            } else {
                # {}, {0}, {name}: para el relleno español son el mismo hueco posicional.
                $shapes.Add('pos')
            }
            $i = $j + 1
            continue
        }
        if ($c -eq '}' -and $i + 1 -lt $Text.Length -and $Text[$i + 1] -eq '}') { $i += 2; continue }
        $i++
    }
    return ,$shapes
}

# Recoge todos los literales de cadena del fichero (fuera de comentarios). Sirve
# para no declarar muerta una entrada que se pasa a t! desde una tabla estática
# (t!(ru, en, ...) con variables), donde el auditor no puede seguir la clave.
function Get-RustStrings {
    param([string]$Text)
    $list = New-Object System.Collections.Generic.List[string]
    $n = $Text.Length
    $i = 0
    while ($i -lt $n) {
        $c = $Text[$i]
        if ($c -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '/') {
            $i = $Text.IndexOf("`n", $i)
            if ($i -lt 0) { break }
            continue
        }
        if ($c -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '*') {
            $d = 1; $i += 2
            while ($i -lt $n -and $d -gt 0) {
                if ($Text[$i] -eq '/' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '*') { $d++; $i += 2 }
                elseif ($Text[$i] -eq '*' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '/') { $d--; $i += 2 }
                else { $i++ }
            }
            continue
        }
        if ($c -eq '"') {
            $start = $i
            $i++
            while ($i -lt $n) {
                if ($Text[$i] -eq '\') { $i += 2; continue }
                if ($Text[$i] -eq '"') { $i++; break }
                $i++
            }
            $lit = Get-RustLiteral $Text.Substring($start, $i - $start)
            if ($null -ne $lit -and $lit.Length -gt 0) { $list.Add($lit) }
            continue
        }
        if ($c -eq 'r' -or $c -eq 'b') {
            $start = $i
            $j = $i
            if ($Text[$j] -eq 'b' -and $j + 1 -lt $n -and $Text[$j + 1] -eq 'r') { $j++ }
            if ($j -lt $n -and $Text[$j] -eq 'r') {
                $k = $j + 1; $h = 0
                while ($k -lt $n -and $Text[$k] -eq '#') { $h++; $k++ }
                if ($k -lt $n -and $Text[$k] -eq '"') {
                    $end = $Text.IndexOf('"' + ('#' * $h), $k + 1)
                    if ($end -lt 0) { break }
                    $i = $end + 1 + $h
                    $lit = Get-RustLiteral $Text.Substring($start, $i - $start)
                    if ($null -ne $lit -and $lit.Length -gt 0) { $list.Add($lit) }
                    continue
                }
            }
        }
        if ($c -eq "'") {
            if ($i + 2 -lt $n -and $Text[$i + 1] -eq '\' -and $Text[$i + 3] -eq "'") { $i += 4; continue }
            if ($i + 2 -lt $n -and $Text[$i + 2] -eq "'") { $i += 3; continue }
            $i++
            continue
        }
        $i++
    }
    return ,$list
}

function Get-LineNumber {
    param([string]$Text, [int]$Index)
    if ($Index -le 0) { return 1 }
    return ([regex]::Matches($Text.Substring(0, $Index), "`n")).Count + 1
}

function Get-CrateName {
    param([string]$RelativePath)
    $parts = $RelativePath -split '\\'
    if ($parts.Length -ge 2 -and $parts[0] -eq 'plugins') { return "plugins/$($parts[1])" }
    if ($parts[0] -eq 'src') { return 'instalador/src' }
    return $parts[0]
}

# Abrevia un texto para las líneas de informe.
function Format-Short {
    param([string]$Text)
    if ($null -eq $Text) { return '(sin texto)' }
    $s = $Text -replace "`r", '\r' -replace "`n", '\n' -replace "`t", '\t'
    if ($s.Length -gt 90) { return $s.Substring(0, 87) + '...' }
    return $s
}

# Palabras del portugués de Brasil que no se usan en pt-PT.
function Get-PtBrHits {
    param([string]$Text)
    $hits = New-Object System.Collections.Generic.List[string]
    $words = @(
        'arquivo', 'arquivos', 'tela', 'telas', 'celular', 'registro', 'registros',
        'equipe', 'equipes', 'ônibus', 'banheiro', 'geladeira', 'carteira de identidade',
        'mousepad', 'planilha'
    )
    foreach ($w in $words) {
        if ($Text -match ('(?i)(?<![a-zà-ÿ])' + [regex]::Escape($w) + '(?![a-zà-ÿ])')) { $hits.Add($w) }
    }
    return ,$hits
}

# ─────────────────────────── carga de common/es.tsv ───────────────────────────

$tsvText = [System.IO.File]::ReadAllText($TsvPath, [System.Text.Encoding]::UTF8)
$tsvLines = $tsvText -split "`n"
# Las claves de es.tsv distinguen mayúsculas (HashMap de Rust), así que los
# diccionarios del script tienen que ser Ordinal, no los hashtables de PowerShell.
$spanish = [System.Collections.Generic.Dictionary[string, string]]::new([System.StringComparer]::Ordinal)
$spanishRaw = [System.Collections.Generic.Dictionary[string, int]]::new([System.StringComparer]::Ordinal)
$tsvProblems = New-Object System.Collections.Generic.List[object]
$duplicates = New-Object System.Collections.Generic.List[object]
$seen = [System.Collections.Generic.Dictionary[string, int]]::new([System.StringComparer]::Ordinal)
$seenLower = @{}
$lineNo = 0
foreach ($rawLine in $tsvLines) {
    $lineNo++
    $line = $rawLine
    $hadCr = $false
    if ($line.EndsWith("`r")) { $line = $line.Substring(0, $line.Length - 1); $hadCr = $true }
    if ($line.Contains("`r")) {
        $tsvProblems.Add([pscustomobject]@{ Line = $lineNo; Kind = 'CR interno'; Detail = 'la línea contiene un retorno de carro que no es el final de línea' })
        continue
    }
    if ($line.Length -eq 0) {
        # La última línea vacía es normal al terminar el fichero con salto.
        if ($lineNo -lt $tsvLines.Length) {
            $tsvProblems.Add([pscustomobject]@{ Line = $lineNo; Kind = 'línea vacía'; Detail = '' })
        }
        continue
    }
    $tabs = ([regex]::Matches($line, "`t")).Count
    if ($tabs -eq 0) {
        $tsvProblems.Add([pscustomobject]@{ Line = $lineNo; Kind = 'sin tabulador'; Detail = $line })
        continue
    }
    if ($tabs -gt 1) {
        $tsvProblems.Add([pscustomobject]@{ Line = $lineNo; Kind = 'más de un tabulador'; Detail = $line })
        continue
    }
    $pair = $line.Split("`t")
    $en = $pair[0]
    $es = $pair[1]
    if ($en.Length -eq 0) {
        $tsvProblems.Add([pscustomobject]@{ Line = $lineNo; Kind = 'clave vacía'; Detail = $line })
        continue
    }
    if ($es.Length -eq 0) {
        $tsvProblems.Add([pscustomobject]@{ Line = $lineNo; Kind = 'español vacío'; Detail = $en })
    }
    $key = ConvertFrom-RustEscapes $en
    $value = ConvertFrom-RustEscapes $es
    if ($seen.ContainsKey($key)) {
        $first = $seen[$key]
        $same = ($spanish[$key] -ceq $value)
        $duplicates.Add([pscustomobject]@{ Line = $lineNo; Key = $key; First = $first; Spanish = $value; Type = if ($same) { 'exacta' } else { 'exacta, español distinto' } })
    } else {
        $seen[$key] = $lineNo
        $spanish[$key] = $value
        $spanishRaw[$key] = $lineNo
        $lower = $key.ToLowerInvariant()
        if ($seenLower.ContainsKey($lower)) {
            $duplicates.Add([pscustomobject]@{ Line = $lineNo; Key = $key; First = $seenLower[$lower]; Spanish = $value; Type = 'solo mayúsculas' })
        } else {
            $seenLower[$lower] = $lineNo
        }
    }
}
$crlfCount = 0
foreach ($l in $tsvLines) { if ($l.EndsWith("`r")) { $crlfCount++ } }
$lfOnly = ($tsvLines.Length - $crlfCount - 1)

# ─────────────────────────── recorrido de los .rs ───────────────────────────

$files = @(Get-ChildItem -LiteralPath $Root -Recurse -Filter '*.rs' -File |
    Where-Object { $_.FullName -notmatch '\\target\\' } |
    Sort-Object FullName)

$calls = New-Object System.Collections.Generic.List[object]
$usedKeys = [System.Collections.Generic.Dictionary[string, bool]]::new([System.StringComparer]::Ordinal)
$allLiterals = [System.Collections.Generic.Dictionary[string, bool]]::new([System.StringComparer]::Ordinal)
$parsedFiles = 0
foreach ($f in $files) {
    $text = [System.IO.File]::ReadAllText($f.FullName, [System.Text.Encoding]::UTF8)
    if ($text -notmatch '(?<![A-Za-z0-9_])(t|tf)!') { continue }
    $parsedFiles++
    foreach ($lit in (Get-RustStrings $text)) {
        if ($lit.Length -gt 0) { $allLiterals[$lit] = $true }
    }
    $rel = $f.FullName.Substring($Root.Length).TrimStart('\')
    $crate = Get-CrateName $rel
    foreach ($call in (Get-RustCalls $text)) {
        $line = Get-LineNumber $text $call.Index
        $positional = New-Object System.Collections.Generic.List[object]
        $named = @{}
        $unknownLabels = New-Object System.Collections.Generic.List[string]
        foreach ($arg in $call.Args) {
            $m = [regex]::Match($arg, '^(ja|pt|zh|es|ru|en)\s*:\s*(?<rest>[\s\S]*)$')
            if ($m.Success) {
                $label = $m.Groups[1].Value
                $rest = $m.Groups['rest'].Value
                if ($label -eq 'ja' -or $label -eq 'pt' -or $label -eq 'zh') {
                    if ($named.ContainsKey($label)) {
                        $unknownLabels.Add("$label repetido")
                    }
                    $named[$label] = $rest
                } else {
                    $unknownLabels.Add("etiqueta $label no admitida por la macro")
                }
            } else {
                $positional.Add($arg)
            }
        }

        $ru = $null; $en = $null
        if ($positional.Count -ge 1) { $ru = Get-RustLiteral $positional[0] }
        if ($positional.Count -ge 2) { $en = Get-RustLiteral $positional[1] }
        # En tf! los argumentos posicionales a partir del tercero son los valores de
        # formato; en t! no existen, así que tres o más sí son un error.
        if ($call.Kind -eq 't' -and $positional.Count -gt 2) {
            $unknownLabels.Add("$($positional.Count) argumentos posicionales en t! (solo admite ruso e inglés)")
        }

        $dynamic = New-Object System.Collections.Generic.List[string]
        if ($null -eq $en) { $dynamic.Add('inglés no literal') }
        if ($null -eq $ru) { $dynamic.Add('ruso no literal') }

        $langs = @{}
        $langs['ru'] = $ru
        $langs['en'] = $en
        foreach ($k in @('ja', 'pt', 'zh')) {
            if ($named.ContainsKey($k)) {
                $langs[$k] = Get-RustLiteral $named[$k]
                if ($null -eq $langs[$k]) { $dynamic.Add("$k no literal") }
            } else {
                $langs[$k] = $null
            }
        }

        $missing = New-Object System.Collections.Generic.List[string]
        foreach ($k in @('ja', 'pt', 'zh')) {
            if (-not $named.ContainsKey($k)) { $missing.Add($k) }
        }

        if ($null -ne $en -and $en.Length -gt 0) { $usedKeys[$en] = $true }
        if ($null -ne $en -and $en.Length -eq 0) { $unknownLabels.Add('inglés vacío') }
        if ($null -ne $ru -and $ru.Length -eq 0) { $unknownLabels.Add('ruso vacío') }
        foreach ($k in @('ja', 'pt', 'zh')) {
            if ($named.ContainsKey($k) -and $null -ne $langs[$k] -and $langs[$k].Length -eq 0) {
                $unknownLabels.Add("$k vacío")
            }
        }

        # Marcadores de formato: solo tf! los interpreta (t! devuelve el texto tal cual).
        # Se compara la secuencia de especificaciones y el número de huecos; los
        # nombres de {name} pueden cambiar de orden porque format! los resuelve por nombre.
        $markerProblem = $null
        if ($call.Kind -eq 'tf' -and $null -ne $en) {
            $enShapes = Get-FormatShapes $en
            foreach ($k in @('ru', 'ja', 'pt', 'zh')) {
                if ($null -eq $langs[$k]) { continue }
                $shapes = Get-FormatShapes $langs[$k]
                $a = ($enShapes -join '|')
                $b = ($shapes -join '|')
                if ($a -ne $b) {
                    $markerProblem = "en=[$a] $k=[$b]"
                    break
                }
            }
        }

        # El español se rellena con es::fill: sus huecos también deben cuadrar.
        $markerEs = $null
        if ($call.Kind -eq 'tf' -and $null -ne $en -and $spanish.ContainsKey($en)) {
            $a = ((Get-FormatShapes $en) -join '|')
            $b = ((Get-FormatShapes $spanish[$en]) -join '|')
            if ($a -ne $b) { $markerEs = "en=[$a] es=[$b]" }
        }

        $calls.Add([pscustomobject]@{
            File        = $rel
            Line        = $line
            Crate       = $crate
            Kind        = $call.Kind
            English     = $en
            Russian     = $ru
            Ja          = $langs['ja']
            Pt          = $langs['pt']
            Zh          = $langs['zh']
            HasJa       = $named.ContainsKey('ja')
            HasPt       = $named.ContainsKey('pt')
            HasZh       = $named.ContainsKey('zh')
            Missing     = $missing
            Labels      = $unknownLabels
            Dynamic     = $dynamic
            Markers     = $markerProblem
            MarkersEs   = $markerEs
        })
    }
}

# ─────────────────────────── análisis ───────────────────────────

$findings = New-Object System.Collections.Generic.List[object]
function Add-Finding {
    param([string]$Level, [string]$Kind, [string]$Crate, [string]$File, [int]$Line, [string]$Detail)
    $findings.Add([pscustomobject]@{
        Level = $Level; Kind = $Kind; Crate = $Crate; File = $File; Line = $Line; Detail = $Detail
    })
}

foreach ($c in $calls) {
    if ($c.Missing.Count -gt 0) {
        Add-Finding 'AVISO' 'idioma que falta' $c.Crate $c.File $c.Line ("{0}! sin {1}: {2}" -f $c.Kind, ($c.Missing -join ', '), (Format-Short $c.English))
    }
    if ($c.Dynamic.Count -gt 0) {
        Add-Finding 'INFO' 'dinámico' $c.Crate $c.File $c.Line (($c.Dynamic -join ', ') + ' (no comprobable)')
    }
    if ($c.Labels.Count -gt 0) {
        Add-Finding 'ERROR' 'argumentos' $c.Crate $c.File $c.Line (($c.Labels -join '; ') + ': ' + (Format-Short $c.English))
    }
    if ($null -ne $c.Markers) {
        Add-Finding 'ERROR' 'marcadores' $c.Crate $c.File $c.Line ($c.Markers + ': ' + (Format-Short $c.English))
    }
    if ($null -ne $c.MarkersEs) {
        Add-Finding 'ERROR' 'marcadores ES' $c.Crate $c.File $c.Line ($c.MarkersEs + ': ' + (Format-Short $c.English))
    }
    if ($null -ne $c.English -and $c.English.Length -gt 0 -and -not $spanish.ContainsKey($c.English)) {
        Add-Finding 'ERROR' 'clave sin español' $c.Crate $c.File $c.Line (Format-Short $c.English)
    }

    # Idioma ausente con forma de traducción automática o de texto copiado.
    if ($null -ne $c.Ja -and $c.Ja.Length -gt 0 -and $c.Ja -ne $c.English) {
        if ($c.Ja -notmatch '[\u3040-\u30ff\u4e00-\u9fff]' -and $c.Ja -match '[A-Za-z]') {
            Add-Finding 'AVISO' 'ja sin japonés' $c.Crate $c.File $c.Line (Format-Short $c.Ja)
        }
        if ($c.Ja -match '[áéíóúñ¿¡ãõç]') {
            Add-Finding 'AVISO' 'ja con acentos ES/PT' $c.Crate $c.File $c.Line (Format-Short $c.Ja)
        }
    }
    if ($null -ne $c.Zh -and $c.Zh.Length -gt 0 -and $c.Zh -ne $c.English) {
        if ($c.Zh -notmatch '[\u4e00-\u9fff]' -and $c.Zh -match '[A-Za-z]') {
            Add-Finding 'AVISO' 'zh sin chino' $c.Crate $c.File $c.Line (Format-Short $c.Zh)
        }
        if ($c.Zh -match '[áéíóúñ¿¡ãõç]') {
            Add-Finding 'AVISO' 'zh con acentos ES/PT' $c.Crate $c.File $c.Line (Format-Short $c.Zh)
        }
    }
    if ($null -ne $c.Russian -and $c.Russian.Length -gt 0 -and $c.Russian -ne $c.English) {
        if ($c.Russian -notmatch '[\u0400-\u04ff]' -and $c.Russian -match '[A-Za-z]') {
            Add-Finding 'AVISO' 'ru sin cirílico' $c.Crate $c.File $c.Line (Format-Short $c.Russian)
        }
        if ($c.Russian -match '[áéíóúñ¿¡ãõç]') {
            Add-Finding 'AVISO' 'ru con acentos ES/PT' $c.Crate $c.File $c.Line (Format-Short $c.Russian)
        }
    }
    if ($null -ne $c.Pt -and $c.Pt.Length -gt 0) {
        $br = Get-PtBrHits $c.Pt
        if ($br.Count -gt 0) {
            Add-Finding 'AVISO' 'pt-BR' $c.Crate $c.File $c.Line (($br -join ', ') + ': ' + (Format-Short $c.Pt))
        }
    }
}

# Entradas de es.tsv que ningún t!/tf! usa. Se descartan las que aparecen como
# literal en una tabla estática (t!(ru, en, ...) con variables): no se puede
# seguir la clave, pero el texto sí se usa.
$unused = New-Object System.Collections.Generic.List[object]
foreach ($key in $spanish.Keys) {
    if (-not $usedKeys.ContainsKey($key) -and -not $allLiterals.ContainsKey($key)) {
        $unused.Add([pscustomobject]@{ Key = $key; Line = $spanishRaw[$key]; Spanish = $spanish[$key] })
    }
}

# Español idéntico al inglés (posible traducción pendiente).
$identical = New-Object System.Collections.Generic.List[object]
foreach ($key in $spanish.Keys) {
    $es = $spanish[$key]
    if ($key -eq $es -and $key -match '[A-Za-z]{4,}' -and $key -notmatch '^[\d\s\.\,\:\-\+\%\(\)\[\]\/\\]+$') {
        $identical.Add([pscustomobject]@{ Key = $key; Line = $spanishRaw[$key]; Spanish = $es })
    }
}

# Terminología: se comprueba la entrada inglesa concreta, no toda la tabla, para
# no confundir "disk" (disco) con "drive" (unidad) ni "directory" con "folder".
$termSpecs = @(
    @{ Name = 'folder -> carpeta'; Key = '(?i)\bfolders?\b'; Bad = '(?i)\bdirectorios?\b'; Hint = 'folder = carpeta, nunca directorio' },
    @{ Name = 'drive -> unidad'; Key = '(?i)\bdrives?\b'; Bad = '(?i)\bdisco'; Hint = 'drive = unidad; disk = disco' },
    @{ Name = 'Explorer -> Explorador de archivos'; Key = '(?i)explorer|file manager'; Bad = '(?i)gestor de archivos'; Hint = 'Explorador de archivos' },
    @{ Name = 'Recycle Bin -> Papelera de reciclaje'; Key = '(?i)recycle bin'; Bad = '(?i)(?<!de reciclaje )\bpapelera\b(?! de reciclaje)'; Hint = 'Papelera de reciclaje' },
    @{ Name = 'This PC -> Este equipo'; Key = '^This PC$'; Bad = '(?i)\bmi pc\b'; Hint = 'Este equipo' },
    @{ Name = 'Quick access -> Acceso rápido'; Key = '(?i)quick access'; Bad = 'Acceso Rápido'; Hint = 'Acceso rápido (minúscula en rápido)' },
    @{ Name = 'taskbar -> barra de tareas'; Key = '(?i)task ?bar'; Bad = '(?i)(?<!de tareas )\bbarra\b(?! de tareas)'; Hint = 'barra de tareas' },
    @{ Name = 'wallpaper -> fondo de pantalla'; Key = '(?i)wallpaper'; Bad = '(?i)papel tapiz'; Hint = 'fondo de pantalla' },
    @{ Name = 'shortcut -> acceso directo'; Key = '(?i)shortcut'; Bad = '(?i)\batajo'; Hint = 'acceso directo' },
    @{ Name = 'pinned -> anclado'; Key = '(?i)\bpinned\b'; Bad = '(?i)\bfijado'; Hint = 'anclado' },
    @{ Name = 'blur -> desenfoque'; Key = '(?i)\bblur'; Bad = '(?i)difuminad'; Hint = 'desenfoque' },
    @{ Name = 'volume -> volumen'; Key = '(?i)\bvolume\b'; Bad = '(?i)\bvolumen de'; Hint = 'volumen' },
    @{ Name = 'notifications -> notificaciones'; Key = '(?i)notifications?\b'; Bad = '(?i)\bavisos?\b'; Hint = 'notificaciones' },
    @{ Name = 'Task Manager -> Administrador de tareas'; Key = '(?i)task manager'; Bad = '(?i)monitor de (rendimiento|recursos)'; Hint = 'Administrador de tareas' }
)
$termStats = @{}
foreach ($g in $termSpecs) {
    $bad = 0
    $badLines = New-Object System.Collections.Generic.List[string]
    foreach ($key in $spanish.Keys) {
        if ($key -notmatch $g.Key) { continue }
        $es = $spanish[$key]
        if ($es -ceq $key) { continue } # sin traducir: se informa en otra sección
        if ($es -cmatch $g.Bad) {
            $bad++
            $badLines.Add("línea $($spanishRaw[$key]): '$key' -> '$es'")
        }
    }
    $termStats[$g.Name] = [pscustomobject]@{ Bad = $bad; BadLines = $badLines; Hint = $g.Hint }
}

# Recuento de variantes de portugués (PT-PT frente a PT-BR) en los textos pt:.
$ptTerms = @(
    @{ Name = 'ficheiro / arquivo'; A = 'ficheiro'; B = 'arquivo' },
    @{ Name = 'ecrã / tela'; A = 'ecrã'; B = 'tela' },
    @{ Name = 'registo / registro'; A = 'registo'; B = 'registro' },
    @{ Name = 'rato / mouse'; A = 'rato'; B = 'mouse' },
    @{ Name = 'equipa / equipe'; A = 'equipa'; B = 'equipe' },
    @{ Name = 'transferir / baixar'; A = 'transferir'; B = 'baixar' },
    @{ Name = 'palavra-passe / senha'; A = 'palavra-passe'; B = 'senha' },
    @{ Name = 'gestor / gerenciador'; A = 'gestor'; B = 'gerenciador' }
)
$ptTexts = @($calls | Where-Object { $null -ne $_.Pt -and $_.Pt.Length -gt 0 })
$ptStats = @{}
foreach ($t in $ptTerms) {
    $a = 0; $b = 0
    foreach ($c in $ptTexts) {
        $a += ([regex]::Matches($c.Pt, ('(?i)(?<![a-zà-ÿ])' + [regex]::Escape($t.A) + '(s?)(?![a-zà-ÿ])'))).Count
        $b += ([regex]::Matches($c.Pt, ('(?i)(?<![a-zà-ÿ])' + [regex]::Escape($t.B) + '(s?)(?![a-zà-ÿ])'))).Count
    }
    $ptStats[$t.Name] = [pscustomobject]@{ Pt = $a; Br = $b }
}

# ─────────────────────────── salida ───────────────────────────

$errorCount = @($findings | Where-Object { $_.Level -eq 'ERROR' }).Count
$warnCount = @($findings | Where-Object { $_.Level -eq 'AVISO' }).Count
$infoCount = @($findings | Where-Object { $_.Level -eq 'INFO' }).Count
$missingKeyCount = @($findings | Where-Object { $_.Kind -eq 'clave sin español' }).Count
$markerCount = @($findings | Where-Object { $_.Kind -eq 'marcadores' }).Count
$markerEsCount = @($findings | Where-Object { $_.Kind -eq 'marcadores ES' }).Count
$argCount = @($findings | Where-Object { $_.Kind -eq 'argumentos' }).Count
$missingLangCount = @($findings | Where-Object { $_.Kind -eq 'idioma que falta' }).Count
$missingAll = @($calls | Where-Object { $_.Missing.Count -eq 3 }).Count
$missingOnlyZh = @($calls | Where-Object { $_.Missing.Count -eq 1 -and $_.Missing[0] -eq 'zh' }).Count
$dynamicCount = @($calls | Where-Object { $_.Dynamic.Count -gt 0 }).Count

Write-Host ''
Write-Host 'WinRust i18n check' -ForegroundColor Cyan
Write-Host "Raíz: $Root"
Write-Host ("Ficheros .rs: {0} (con t!/tf!: {1})" -f $files.Count, $parsedFiles)
Write-Host ("Llamadas t!/tf!: {0}  (t!: {1}, tf!: {2})" -f $calls.Count, @($calls | Where-Object { $_.Kind -eq 't' }).Count, @($calls | Where-Object { $_.Kind -eq 'tf' }).Count)
$distinctUsed = @($usedKeys.Keys).Count
Write-Host ("Claves inglesas distintas usadas: {0}" -f $distinctUsed)
Write-Host ("es.tsv: {0} líneas ({1} con CRLF, {2} solo LF), {3} claves únicas" -f $tsvLines.Length, $crlfCount, $lfOnly, $spanish.Count)
Write-Host ''
Write-Host 'Resumen' -ForegroundColor Cyan
Write-Host ("  Errores:                         {0}" -f $errorCount)
Write-Host ("  Avisos:                          {0}" -f $warnCount)
Write-Host ("  Llamadas sin algún idioma:       {0}  (sin ja/pt/zh: {1}; solo sin zh: {2})" -f $missingLangCount, $missingAll, $missingOnlyZh)
Write-Host ("  Marcadores descuadrados:         {0} en macros + {1} en español" -f $markerCount, $markerEsCount)
Write-Host ("  Argumentos raros:                {0}" -f $argCount)
Write-Host ("  Claves inglesas sin español:     {0}" -f $missingKeyCount)
Write-Host ("  Entradas de es.tsv no usadas:    {0}" -f $unused.Count)
Write-Host ("  Duplicados en es.tsv:            {0}" -f $duplicates.Count)
Write-Host ("  Líneas mal formadas en es.tsv:   {0}" -f $tsvProblems.Count)
Write-Host ("  Español idéntico al inglés:      {0} (revisar: pueden ser marcas)" -f $identical.Count)
Write-Host ("  Llamadas no comprobables:        {0} (textos variables)" -f $dynamicCount)
Write-Host ''

$crates = @($calls | Group-Object Crate | Sort-Object Name)
foreach ($group in $crates) {
    $crateFindings = @($findings | Where-Object { $_.Crate -eq $group.Name -and $_.Level -ne 'INFO' })
    if ($crateFindings.Count -eq 0) { continue }
    $distinct = @($group.Group | Where-Object { $null -ne $_.English } | ForEach-Object { $_.English } | Sort-Object -Unique).Count
    Write-Host ("== {0} ({1} llamadas, {2} claves) ==" -f $group.Name, $group.Count, $distinct) -ForegroundColor Yellow
    foreach ($kind in @('argumentos', 'marcadores', 'marcadores ES', 'idioma que falta', 'clave sin español', 'ja sin japonés', 'zh sin chino', 'ru sin cirílico', 'ja con acentos ES/PT', 'zh con acentos ES/PT', 'ru con acentos ES/PT', 'pt-BR')) {
        $items = @($crateFindings | Where-Object { $_.Kind -eq $kind })
        if ($items.Count -eq 0) { continue }
        $color = if ($kind -in @('argumentos', 'marcadores', 'marcadores ES', 'clave sin español')) { 'Red' } else { 'DarkYellow' }
        Write-Host ("  -- {0}: {1}" -f $kind, $items.Count) -ForegroundColor $color
        foreach ($it in $items) {
            Write-Host ("     {0}:{1}  {2}" -f $it.File, $it.Line, $it.Detail)
        }
    }
    Write-Host ''
}

if ($duplicates.Count -gt 0) {
    Write-Host '== Duplicados en common/es.tsv ==' -ForegroundColor Yellow
    foreach ($d in $duplicates) {
        Write-Host ("  línea {0} (primera en {1}, {2}): {3}" -f $d.Line, $d.First, $d.Type, (Format-Short $d.Key))
    }
    Write-Host ''
}
if ($tsvProblems.Count -gt 0) {
    Write-Host '== Líneas mal formadas en common/es.tsv ==' -ForegroundColor Yellow
    foreach ($p in $tsvProblems) {
        Write-Host ("  línea {0}: {1}  {2}" -f $p.Line, $p.Kind, (Format-Short $p.Detail))
    }
    Write-Host ''
}

Write-Host ("== Entradas de es.tsv no usadas: {0} ==" -f $unused.Count) -ForegroundColor Yellow
foreach ($u in ($unused | Sort-Object Line)) {
    Write-Host ("  línea {0}: {1}" -f $u.Line, (Format-Short $u.Key))
}
Write-Host ''

Write-Host ("== Español idéntico al inglés: {0} ==" -f $identical.Count) -ForegroundColor Yellow
foreach ($u in ($identical | Sort-Object Line)) {
    Write-Host ("  línea {0}: {1}" -f $u.Line, (Format-Short $u.Key))
}
Write-Host ''

Write-Host '== Terminología española ==' -ForegroundColor Yellow
foreach ($name in ($termStats.Keys | Sort-Object)) {
    $s = $termStats[$name]
    $mark = if ($s.Bad -gt 0) { ' <-- revisar' } else { '' }
    Write-Host ("  {0}: {1} variante(s){2}   ({3})" -f $name, $s.Bad, $mark, $s.Hint)
    foreach ($line in ($s.BadLines | Select-Object -First 8)) { Write-Host ("     {0}" -f $line) }
}
Write-Host ''

Write-Host '== Portugués: pt-PT frente a pt-BR (textos pt: de t!/tf!) ==' -ForegroundColor Yellow
foreach ($name in ($ptStats.Keys | Sort-Object)) {
    $s = $ptStats[$name]
    Write-Host ("  {0}: PT={1}  BR={2}" -f $name, $s.Pt, $s.Br)
}
Write-Host ''

$problems = $errorCount + $duplicates.Count + $tsvProblems.Count + $missingLangCount
if (-not $UnusedIsWarning) { $problems += $unused.Count }
Write-Host ("Resultado: {0} problema(s)" -f $problems) -ForegroundColor $(if ($problems -gt 0) { 'Red' } else { 'Green' })
if ($problems -gt 0) { exit 1 }
exit 0
