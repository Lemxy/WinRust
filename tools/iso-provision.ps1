#Requires -Version 5.1
<#
.SYNOPSIS
    Provisioning de la ISO WinRust: redirecciones de registro reversibles (IFEO, handlers ms-*, asociaciones).

.DESCRIPTION
    Aplica, en un Windows ya instalado, el provisioning descrito en tools\iso-replacement.md:

      IFEO          notepad.exe -> rust-notepad.exe; taskmgr.exe (verificar, ya existe);
                    control.exe / msinfo32.exe solo de forma opcional y documentada.
      URI           ms-settings: -> rust-settings.exe; ms-screenclip:/ms-snippingtool: -> rs-shot.exe --region;
                    ms-photos:/ms-paint: solo si la app propia existe y sabe interpretar el URI.
      Asociaciones  ProgID rust-notepad + "Abrir con" (.txt/.log/.ini/.md/.json/.xml/.csv) + Applications\rust-notepad.exe,
                    sin tocar UserChoice (que esta build protege con UCPD.sys).
      Shell         SOLO lectura: HKCU Winlogon\Shell = rs-shell.exe y tarea rs-overlay deshabilitada.
      Servicios     SOLO lectura: WSearch, AppXSvc, ClipSVC, StateRepository, WaaSMedicSvc, WpnService, cbdhsvc, ucpd.

    Modos:
      -DryRun   (por defecto) imprime el plan y no escribe nada, ni registro ni estado.
      -Apply    escribe los cambios y guarda el valor original de cada uno antes de tocarlo.
      -Restore  devuelve cada valor original y borra las claves que creó este script.

    Estado:
      tools\state\iso-provision.backup.json   valor original de cada valor de registro tocado
      tools\state\journal.json               registro de lo aplicado/restaurado (append por ejecución)

    Garantías de diseño:
      * Nada de disco: no se copian, borran ni parchean binarios del sistema.
      * Idempotente: si un valor ya tiene el destino, no se reescribe ni se duplica en el journal.
      * Reversible: -Restore usa el backup, no una lista hardcodeada.
      * explorer.exe, shell32, WinSxS, Store, LockApp, paquetes XAML y UserChoice no se tocan nunca.

    Elevación: -Apply y -Restore necesitan administrador porque las entradas IFEO viven en HKLM.
    -DryRun funciona sin elevar (solo lee).

.EXAMPLE
    powershell -NoProfile -File tools\iso-provision.ps1
    powershell -NoProfile -File tools\iso-provision.ps1 -IncludeOptional
    powershell -NoProfile -File tools\iso-provision.ps1 -Apply
    powershell -NoProfile -File tools\iso-provision.ps1 -Restore

.NOTES
    Repo: C:\winrust-build. Documento de diseño: tools\iso-replacement.md.
    Checklist de aceptación en VM: tools\iso-test.md.

    Si la máquina tiene la ejecución de scripts restringida (pasa en la build de referencia):
      powershell -NoProfile -ExecutionPolicy Bypass -File tools\iso-provision.ps1
    -Apply escribe HKCU en el perfil del usuario que eleva: ejecútalo como el usuario objetivo,
    no como otra cuenta de administrador.
#>
[CmdletBinding()]
param(
    # Modo por defecto: enseña el plan y no escribe nada.
    [switch]$DryRun,
    # Aplica el plan (requiere elevación).
    [switch]$Apply,
    # Deshace lo aplicado usando tools\state\iso-provision.backup.json (requiere elevación).
    [switch]$Restore,
    # Incluye los pasos marcados como opcionales (IFEO de control.exe/msinfo32, App Paths, ms-photos:).
    [switch]$IncludeOptional,
    # No pide confirmación antes de -Apply.
    [switch]$NoPrompt,
    # Carpeta de estado (backup + journal). Por defecto tools\state junto a este script.
    [string]$StateDir = ''
)

$ErrorActionPreference = 'Stop'

# Al redirigir la salida (CI, pipes), escribe UTF-8 para que los acentos no se corrompan.
if ([Console]::IsOutputRedirected) {
    try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false) } catch {}
}

if (-not $StateDir) {
    $here = if ($PSScriptRoot) { $PSScriptRoot } else { (Split-Path -Parent $MyInvocation.MyCommand.Path) }
    $StateDir = Join-Path $here 'state'
}

# --------------------------------------------------------------------------- constantes

$IfeoRoot  = 'SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options'
$Classes   = 'Software\Classes'
$Winlogon  = 'Software\Microsoft\Windows NT\CurrentVersion\Winlogon'
$BackupFile  = Join-Path $StateDir 'iso-provision.backup.json'
$JournalFile = Join-Path $StateDir 'journal.json'

$script:ProgramFiles = [Environment]::GetFolderPath('ProgramFiles')
if (-not $script:ProgramFiles) { $script:ProgramFiles = 'C:\Program Files' }

# Aplicación -> (carpeta en Program Files, ejecutable). Refleja el instalador real del repo:
#   src\install\explorer.rs (rust-explorer\rust-explorer.exe, rs-shell.exe), topbar.rs (topbar\topbar.exe),
#   tools.rs (rust-settings, rs-shot, photoview), taskmgr.rs (rtaskmgr\rtaskmgr-win32.exe).
$script:Apps = [ordered]@{
    'rs-shell'      = @{ App = 'rust-explorer'; Exe = 'rs-shell.exe' }
    'rust-explorer' = @{ App = 'rust-explorer'; Exe = 'rust-explorer.exe' }
    'topbar'        = @{ App = 'topbar';        Exe = 'topbar.exe' }
    'rust-settings' = @{ App = 'rust-settings'; Exe = 'rust-settings.exe' }
    'rust-control'  = @{ App = 'rust-control';  Exe = 'rust-control.exe' }
    'rs-shot'       = @{ App = 'rs-shot';       Exe = 'rs-shot.exe' }
    'photoview'     = @{ App = 'photoview';     Exe = 'photoview.exe' }
    'rtaskmgr'      = @{ App = 'rtaskmgr';      Exe = 'rtaskmgr-win32.exe' }
    'rust-notepad'  = @{ App = 'rust-notepad';  Exe = 'rust-notepad.exe' }
    'rust-paint'    = @{ App = 'rust-paint';    Exe = 'rust-paint.exe' }
}

$script:AppCache = @{}
$script:RegRoots = @{
    HKLM = [Microsoft.Win32.Registry]::LocalMachine
    HKCU = [Microsoft.Win32.Registry]::CurrentUser
}

# Modo por defecto: DryRun.
if (-not $Apply -and -not $Restore) { $DryRun = $true }
if ($Apply -and $Restore) { throw 'usa -Apply o -Restore, no los dos' }
if ($DryRun -and ($Apply -or $Restore)) { throw 'usa -DryRun solo, o -Apply, o -Restore' }

# --------------------------------------------------------------------------- registro (lectura/escritura)

function Split-RegPath {
    param([Parameter(Mandatory)][string]$Path)
    $p = $Path -replace '^([A-Za-z]+):\\', '$1\'
    $i = $p.IndexOf('\')
    if ($i -lt 1) { throw "ruta de registro no válida: $Path" }
    $hive = $p.Substring(0, $i).ToUpperInvariant()
    $sub  = $p.Substring($i + 1).TrimStart('\')
    if (-not $script:RegRoots.ContainsKey($hive)) { throw "hive no soportada: $hive (usa HKLM o HKCU)" }
    [pscustomobject]@{ Hive = $hive; Sub = $sub; Text = "$hive\$sub" }
}

function Read-RegValue {
    param([string]$Path, [string]$Name = '')
    $r = Split-RegPath $Path
    $key = $script:RegRoots[$r.Hive].OpenSubKey($r.Sub, $false)
    if ($null -eq $key) {
        return [pscustomobject]@{ KeyExists = $false; ValueExists = $false; Kind = $null; Value = $null }
    }
    try {
        $names = $key.GetValueNames()
        if (-not ($names -contains $Name)) {
            return [pscustomobject]@{ KeyExists = $true; ValueExists = $false; Kind = $null; Value = $null }
        }
        $kind = [string]$key.GetValueKind($Name)
        $val = $key.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        [pscustomobject]@{ KeyExists = $true; ValueExists = $true; Kind = $kind; Value = $val }
    } finally {
        $key.Close()
    }
}

function Get-KeyState {
    # Devuelve si la clave existe y, si no, la primera clave que falta (la raíz del subárbol creado).
    param([string]$Path)
    $r = Split-RegPath $Path
    $segs = $r.Sub -split '\\'
    $prefix = ''
    $firstMissing = $null
    foreach ($s in $segs) {
        if ($prefix) { $prefix = "$prefix\$s" } else { $prefix = $s }
        $k = $script:RegRoots[$r.Hive].OpenSubKey($prefix, $false)
        if ($null -eq $k) {
            if (-not $firstMissing) { $firstMissing = $prefix }
        } else {
            $k.Close()
        }
    }
    [pscustomobject]@{ KeyExists = (-not $firstMissing); FirstMissing = $firstMissing }
}

function ConvertTo-RegData {
    param([string]$Kind, $Stored)
    switch ($Kind) {
        'Binary' { if ($null -eq $Stored) { return [byte[]]@() } else { return [Convert]::FromBase64String([string]$Stored) } }
        'None'   { if ($null -eq $Stored) { return [byte[]]@() } else { return [Convert]::FromBase64String([string]$Stored) } }
        'DWord'  { return [int]$Stored }
        'QWord'  { return [long]$Stored }
        default  { return [string]$Stored }
    }
}

function ConvertTo-StoredData {
    param([string]$Kind, $Value)
    switch ($Kind) {
        'Binary' { if ($null -eq $Value) { return $null } else { return [Convert]::ToBase64String([byte[]]$Value) } }
        'None'   { if ($null -eq $Value) { return $null } else { return [Convert]::ToBase64String([byte[]]$Value) } }
        default  { return $Value }
    }
}

function Set-RegValue {
    param([string]$Path, [string]$Name, $Stored, [string]$Kind)
    $r = Split-RegPath $Path
    $key = $script:RegRoots[$r.Hive].CreateSubKey($r.Sub)
    try {
        $key.SetValue($Name, (ConvertTo-RegData -Kind $Kind -Stored $Stored), [Microsoft.Win32.RegistryValueKind]$Kind)
    } finally {
        $key.Close()
    }
}

function Remove-RegValue {
    param([string]$Path, [string]$Name)
    $r = Split-RegPath $Path
    $key = $script:RegRoots[$r.Hive].OpenSubKey($r.Sub, $true)
    if ($null -eq $key) { return $false }
    try {
        if (-not (($key.GetValueNames()) -contains $Name)) { return $false }
        $key.DeleteValue($Name, $false)
        return $true
    } finally {
        $key.Close()
    }
}

function Remove-RegTree {
    param([string]$Hive, [string]$Sub)
    try {
        $script:RegRoots[$Hive].DeleteSubKeyTree($Sub, $false)
        return $true
    } catch {
        return $false
    }
}

function Test-RegTreeEmpty {
    # ¿La clave y todo su subárbol no tienen ningún valor? Solo entonces se borra al restaurar.
    param([string]$Hive, [string]$Sub)
    $k = $script:RegRoots[$Hive].OpenSubKey($Sub, $false)
    if ($null -eq $k) { return $true }
    try {
        if ($k.ValueCount -gt 0) { return $false }
        foreach ($n in $k.GetSubKeyNames()) {
            if (-not (Test-RegTreeEmpty -Hive $Hive -Sub "$Sub\$n")) { return $false }
        }
        return $true
    } finally {
        $k.Close()
    }
}

# --------------------------------------------------------------------------- utilidades

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Read-MachineInfo {
    $b = Read-RegValue 'HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion' 'CurrentBuild'
    $d = Read-RegValue 'HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion' 'DisplayVersion'
    $e = Read-RegValue 'HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion' 'EditionID'
    [pscustomobject]@{
        Name    = $env:COMPUTERNAME
        Build   = $(if ($b.ValueExists) { [string]$b.Value } else { '' })
        Version = $(if ($d.ValueExists) { [string]$d.Value } else { '' })
        Edition = $(if ($e.ValueExists) { [string]$e.Value } else { '' })
    }
}

function Resolve-App {
    # Busca el exe de una app en Program Files o junto al instalador (ISO \WinRust\bin, \WinRust\apps o target\release).
    param([string]$Name)
    if ($script:AppCache.ContainsKey($Name)) { return $script:AppCache[$Name] }
    $info = $script:Apps[$Name]
    if (-not $info) { throw "app desconocida: $Name" }
    $candidates = @(
        (Join-Path (Join-Path $script:ProgramFiles $info.App) $info.Exe)
        (Join-Path (Join-Path $PSScriptRoot $info.App) $info.Exe)
        (Join-Path $PSScriptRoot $info.Exe)
        (Join-Path $PSScriptRoot "..\WinRust\bin\$($info.Exe)")
        (Join-Path $PSScriptRoot "..\WinRust\apps\$($info.Exe)")
        (Join-Path $PSScriptRoot "..\target\release\$($info.Exe)")
    )
    $found = $null
    foreach ($c in $candidates) {
        if (Test-Path -LiteralPath $c -PathType Leaf) { $found = (Resolve-Path -LiteralPath $c).Path; break }
    }
    $script:AppCache[$Name] = $found
    $found
}

function Quote-Exe {
    param([string]$Path)
    '"' + $Path + '"'
}

function Format-RegValueText {
    param($Result)
    if (-not $Result.KeyExists) { return '(clave no existe)' }
    if (-not $Result.ValueExists) { return '(no existe)' }
    $v = $Result.Value
    switch ([string]$Result.Kind) {
        'Binary' { return ('(binario, {0} bytes)' -f ([byte[]]$v).Length) }
        'None'   { return '(sin datos)' }
        'DWord'  { return ('0x{0:x8} ({0})' -f [int]$v) }
        'QWord'  { return [string]$v }
        'MultiString' { return (($v -join ' | ')) }
        default {
            if ($null -eq $v) { return '(vacío)' }
            if ([string]$v -eq '') { return '(vacío)' }
            return [string]$v
        }
    }
}

function Shorten-RegKey {
    param([string]$Hive, [string]$Key, [string]$Name)
    $text = "$Hive\$Key"
    $text = $text -replace [regex]::Escape("HKLM\$IfeoRoot"), 'HKLM\...\IFEO'
    $text = $text -replace [regex]::Escape("HKCU\$Classes"), 'HKCU\...\Classes'
    if ($Name) { return "$text [$Name]" }
    return "$text [(Default)]"
}

function Write-JsonFile {
    param([string]$Path, $Data)
    $dir = Split-Path -Parent $Path
    if ($dir -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    $json = $Data | ConvertTo-Json -Depth 14
    $tmp = "$Path.tmp"
    [IO.File]::WriteAllText($tmp, $json, (New-Object System.Text.UTF8Encoding($false)))
    Move-Item -LiteralPath $tmp -Destination $Path -Force
}

function Read-JsonFile {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    $raw = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
    if (-not $raw) { return $null }
    try { return ($raw | ConvertFrom-Json) } catch { throw "JSON no válido en ${Path}: $($_.Exception.Message)" }
}

# --------------------------------------------------------------------------- modelo de pasos

function New-Op {
    param(
        [string]$Id,
        [string]$Section,
        [ValidateSet('Set', 'SetNone', 'Delete', 'EnsureExists')][string]$Mode,
        [string]$Hive,
        [string]$Key,
        [string]$Name = '',
        [string]$App = '',
        [string]$OpArgs = '',
        # Sin tipo: un [string] convertiría el $null por defecto en '' y rompería Get-OpDesired.
        $LiteralValue = $null,
        [switch]$Optional,
        [string]$Note = '',
        [string]$Warn = ''
    )
    $exe = $null
    if ($App) { $exe = Resolve-App $App }
    [pscustomobject]@{
        Id           = $Id
        Section      = $Section
        Mode         = $Mode
        Hive         = $Hive
        Key          = $Key
        Name         = $Name
        App          = $App
        Exe          = $exe
        Args         = $OpArgs
        LiteralValue = $LiteralValue
        Optional     = [bool]$Optional
        Note         = $Note
        Warn         = $Warn
        Kind         = $(if ($Mode -eq 'SetNone') { 'None' } else { 'String' })
        Path         = "$Hive\$Key"
    }
}

function Get-OpDesired {
    param($Op)
    switch ($Op.Mode) {
        'Delete' { return $null }
        'SetNone' { return [byte[]]@() }
        default {
            if ($null -ne $Op.LiteralValue -and [string]$Op.LiteralValue -ne '') { return [string]$Op.LiteralValue }
            if (-not $Op.Exe) { return $null }
            $cmd = Quote-Exe $Op.Exe
            if ($Op.Args) { $cmd = "$cmd $($Op.Args)" }
            return $cmd
        }
    }
}

function Test-SameValue {
    param($Current, $Desired, [string]$Kind)
    if ($null -eq $Desired) { return $false }
    switch ($Kind) {
        'None'   { return ((ConvertTo-StoredData 'None' $Current) -eq (ConvertTo-StoredData 'None' $Desired)) }
        'Binary' { return ((ConvertTo-StoredData 'Binary' $Current) -eq (ConvertTo-StoredData 'Binary' $Desired)) }
        'DWord'  { return ([int]$Current -eq [int]$Desired) }
        default  { return ([string]$Current -ceq [string]$Desired) }
    }
}

function Get-ActionLabel {
    param($Op)
    switch ($Op.Mode) {
        'Delete'       { return 'quitar' }
        'SetNone'      { return 'añadir' }
        'EnsureExists' { return 'verificar' }
        default {
            if ($Op.Section -eq 'IFEO') { return 'redirigir' }
            if ($Op.Section -eq 'URI') { return 'redirigir' }
            return 'definir'
        }
    }
}

function Get-Ops {
    $ops = New-Object System.Collections.Generic.List[object]

    # ------------------------------------------------------------------ 1. IFEO (HKLM)
    # IFEO antepone el "Debugger" y pasa la ruta original como primer argumento; rust-notepad debe tolerarla.
    $ops.Add((New-Op -Id 'ifeo-notepad' -Section 'IFEO' -Mode 'Set' -Hive 'HKLM' `
                -Key "$IfeoRoot\notepad.exe" -Name 'Debugger' -App 'rust-notepad' `
                -Note 'Sustituye cualquier lanzamiento de notepad.exe (ShellExecute, CreateProcess, Win+R). Windows añade la ruta original como 1er argumento: rust-notepad debe ignorarla.' `
                -Warn 'IFEO es T1546.012 para EDR: documentar exclusión. Requiere también la vista WOW6432Node si se lanza desde un proceso de 32 bits.'))

    # control.exe: el plan (§4.4) DESACONSEJA IFEO aquí porque rompe rundll32 shell32.dll,Control_RunDLL
    # y los accesos de las herramientas administrativas. Se ofrece solo con -IncludeOptional.
    $ops.Add((New-Op -Id 'ifeo-control' -Section 'IFEO' -Mode 'Set' -Hive 'HKLM' `
                -Key "$IfeoRoot\control.exe" -Name 'Debugger' -App 'rust-control' -Optional `
                -Note 'redirige control.exe (y los .cpl lanzados por nombre) a rust-control; rust-control debe ignorar la ruta original que IFEO añade como argumento.' `
                -Warn 'CONTRADICE el plan §4.4: rompe rundll32 shell32.dll,Control_RunDLL, los applets .cpl y los accesos de compmgmt/services/devmgmt. Solo si se acepta perder el Panel de control clásico.'))

    # taskmgr.exe YA está redirigido en la máquina de referencia: no duplicar ni reescribir.
    $ops.Add((New-Op -Id 'ifeo-taskmgr' -Section 'IFEO' -Mode 'EnsureExists' -Hive 'HKLM' `
                -Key "$IfeoRoot\taskmgr.exe" -Name 'Debugger' -App 'rtaskmgr' `
                -Note 'Ya existe (rtaskmgr-win32.exe): si hay valor se deja intacto; solo se crea si falta.'))

    # msinfo32.exe: no hay sustituto dedicado (solo la página About de rust-settings); opcional y documentado.
    $ops.Add((New-Op -Id 'ifeo-msinfo32' -Section 'IFEO' -Mode 'Set' -Hive 'HKLM' `
                -Key "$IfeoRoot\msinfo32.exe" -Name 'Debugger' -App 'rust-control' -Optional `
                -Note 'Opcional: msinfo32.exe no tiene página propia; abriría rust-control (sin equivalencia de msinfo32, que debe ignorar el argumento de IFEO).' `
                -Warn 'Pierdes Información del sistema (msinfo32) sin sustituto equivalente; déjalo fuera salvo petición expresa.'))

    # ------------------------------------------------------------------ 2. Handlers de URI (HKCU)
    $ops.Add((New-Op -Id 'uri-ms-settings-command' -Section 'URI' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\ms-settings\shell\open\command" -Name '' -App 'rust-settings' -OpArgs '--system "%1"' `
                -Note 'Sin DelegateExecute: si existe, ShellExecute ignora el comando (ver plan §2.6).' `
                -Warn 'rust-settings todavía NO interpreta --system ni la URI (solo --nightlight): abrirá la ventana principal, no la página pedida. Falta el router winrust-uri.'))

    # El DelegateExecute que hoy usa Windows vive en HKLM; si apareciera en HKCU, se quita.
    $ops.Add((New-Op -Id 'uri-ms-settings-delegate' -Section 'URI' -Mode 'Delete' -Hive 'HKCU' `
                -Key "$Classes\ms-settings\shell\open\command" -Name 'DelegateExecute' `
                -Note 'HKLM\...\ms-settings conserva su DelegateExecute como fallback de otros usuarios/procesos elevados.'))

    $ops.Add((New-Op -Id 'uri-ms-screenclip' -Section 'URI' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\ms-screenclip\shell\open\command" -Name '' -App 'rs-shot' -OpArgs '--region' `
                -Note 'URI que usan Accesibilidad, Configuración y apps de terceros para recortar. rs-shot --region está implementado.'))

    $ops.Add((New-Op -Id 'uri-ms-snippingtool' -Section 'URI' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\ms-snippingtool\shell\open\command" -Name '' -App 'rs-shot' -OpArgs '--region' `
                -Note 'Alias no estándar usado por algunos accesos; en esta LTSC la clave no existe, así que es aditivo.'))

    # ms-photos: photoview.exe existe, pero no parsea argumentos ni URIs (plugins/photoview no lee args).
    $ops.Add((New-Op -Id 'uri-ms-photos' -Section 'URI' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\ms-photos\shell\open\command" -Name '' -App 'photoview' -OpArgs '"%1"' -Optional `
                -Note 'Solo cuando photoview sepa abrir un ms-photos: (hoy no lee argumentos).' `
                -Warn 'photoview.exe existe pero NO interpreta ms-photos: todavía; activarlo ahora abre el visor con una ruta inválida.'))

    # ms-paint: rust-paint.exe no existe; el guard lo deja como "saltado" automáticamente.
    $ops.Add((New-Op -Id 'uri-ms-paint' -Section 'URI' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\ms-paint\shell\open\command" -Name '' -App 'rust-paint' -OpArgs '"%1"' `
                -Note 'Se aplica solo si rust-paint.exe existe; si no, se salta (no se crea un handler roto).'))

    # ------------------------------------------------------------------ 3. Asociaciones (HKCU, sin UserChoice)
    $ops.Add((New-Op -Id 'progid-rust-notepad-name' -Section 'Asociaciones' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\rust-notepad" -Name '' -App 'rust-notepad' -LiteralValue 'WinRust Text' `
                -Note 'Descripción visible en "Abrir con".'))

    $ops.Add((New-Op -Id 'progid-rust-notepad-open' -Section 'Asociaciones' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\rust-notepad\shell\open\command" -Name '' -App 'rust-notepad' -OpArgs '"%1"'))

    $ops.Add((New-Op -Id 'progid-rust-notepad-edit' -Section 'Asociaciones' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\rust-notepad\shell\edit\command" -Name '' -App 'rust-notepad' -OpArgs '"%1"'))

    foreach ($ext in @('.txt', '.log', '.ini', '.md', '.json', '.xml', '.csv')) {
        $ops.Add((New-Op -Id "openwith-$ext" -Section 'Asociaciones' -Mode 'SetNone' -Hive 'HKCU' `
                    -Key "$Classes\$ext\OpenWithProgids" -Name 'rust-notepad' -App 'rust-notepad' `
                    -Note 'Solo añade WinRust a "Abrir con"; no cambia el predeterminado ni toca UserChoice.'))
    }

    $ops.Add((New-Op -Id 'applications-rust-notepad' -Section 'Asociaciones' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\Applications\rust-notepad.exe\shell\open\command" -Name '' -App 'rust-notepad' -OpArgs '"%1"' `
                -Note 'Permite que rust-notepad aparezca como aplicación y que "Abrir con" lo resuelva por nombre.'))

    # Receta §4.3 del plan: el verbo open del ProgID Applications\notepad.exe (al que apunta .txt en esta build).
    $ops.Add((New-Op -Id 'applications-notepad-open' -Section 'Asociaciones' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\Applications\notepad.exe\shell\open\command" -Name '' -App 'rust-notepad' -OpArgs '"%1"' `
                -Note 'No toca UserChoice ni HKCR\.txt: solo redefine el verbo open del ProgID Applications\notepad.exe.'))

    $ops.Add((New-Op -Id 'applications-notepad-edit' -Section 'Asociaciones' -Mode 'Set' -Hive 'HKCU' `
                -Key "$Classes\Applications\notepad.exe\shell\edit\command" -Name '' -App 'rust-notepad' -OpArgs '"%1"'))

    # App Paths: solo afecta a ShellExecute por nombre; opcional para no ampliar la huella en HKLM.
    $ops.Add((New-Op -Id 'apppaths-notepad' -Section 'Asociaciones' -Mode 'Set' -Hive 'HKLM' `
                -Key 'SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\notepad.exe' -Name '' -App 'rust-notepad' -Optional `
                -Note 'Opcional: ShellExecute("notepad.exe") de terceros acaba en rust-notepad. No afecta a CreateProcess con ruta completa.'))

    return $ops
}

# --------------------------------------------------------------------------- comprobaciones (solo lectura)

function New-CheckRow {
    param([string]$What, [string]$Actual, [string]$Expected, [string]$State, [string]$Hint = '')
    [pscustomobject]@{
        'Comprobación' = $What
        'Actual'       = $Actual
        'Esperado'     = $Expected
        'Estado'       = $State
        'Nota'         = $Hint
    }
}

function Get-TaskState {
    param([string]$Name)
    $t = Get-ScheduledTask -TaskName $Name -ErrorAction SilentlyContinue
    if (-not $t) { return $null }
    [string]$t.State
}

function Get-Checks {
    $rows = New-Object System.Collections.Generic.List[object]

    # --- shell del usuario (no se cambia aquí: lo gestiona rs-shell --enable)
    $shellExe = Resolve-App 'rs-shell'
    $shell = Read-RegValue "HKCU\$Winlogon" 'Shell'
    $actual = if ($shell.ValueExists) { [string]$shell.Value } else { '(no definido -> explorer.exe)' }
    $expected = if ($shellExe) { $shellExe } else { 'C:\Program Files\rust-explorer\rs-shell.exe' }
    $ok = $shell.ValueExists -and ($actual.Trim('"') -ieq $expected)
    $rows.Add((New-CheckRow 'HKCU Winlogon\Shell' $actual $expected $(if ($ok) { 'OK' } else { 'AVISO' }) `
                $(if ($ok) { 'shell por usuario correcto' } else { 'no se cambia desde aquí: rs-shell --enable (o shell-phase2.ps1 -Apply)' })))

    # --- tarea rs-overlay (debe estar deshabilitada para que no haya dos barras)
    $ov = Get-TaskState 'rs-overlay'
    if ($null -eq $ov) {
        $rows.Add((New-CheckRow 'Tarea rs-overlay' 'no encontrada' 'Disabled' 'AVISO' 'si no existe, no hay overlay que apagar'))
    } elseif ($ov -eq 'Disabled') {
        $rows.Add((New-CheckRow 'Tarea rs-overlay' $ov 'Disabled' 'OK' ''))
    } else {
        $rows.Add((New-CheckRow 'Tarea rs-overlay' $ov 'Disabled' 'AVISO' 'no se cambia: deshabilítala con shell-phase2.ps1 -Apply'))
    }

    # --- UserChoice (prueba de que no lo tocamos)
    $uc = Read-RegValue 'HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.txt\UserChoice' 'ProgId'
    $rows.Add((New-CheckRow 'UserChoice .txt (solo lectura)' $(if ($uc.ValueExists) { [string]$uc.Value } else { '(no existe)' }) '(sin cambios)' 'INFO' 'este script nunca escribe UserChoice (hash + UCPD.sys)'))

    # --- servicios que el plan exige mantener
    $svc = @(
        @{ Name = 'WSearch';        Expect = 'Disabled'; Why = 'búsqueda Windows fuera; rust-search la sustituye' }
        @{ Name = 'AppXSvc';        Expect = 'Enabled';  Why = 'instalar/actualizar MSIX' }
        @{ Name = 'ClipSVC';        Expect = 'Enabled';  Why = 'licencias/Store' }
        @{ Name = 'StateRepository';Expect = 'Enabled';  Why = 'despliegue MSIX' }
        @{ Name = 'WaaSMedicSvc';   Expect = 'Enabled';  Why = 'auto-reparación de Windows Update' }
        @{ Name = 'WpnService';     Expect = 'Enabled';  Why = 'notificaciones push' }
        @{ Name = 'ucpd';           Expect = 'Running';  Why = 'protección de asociaciones; no se toca' }
    )
    foreach ($s in $svc) {
        $o = Get-Service -Name $s.Name -ErrorAction SilentlyContinue
        if (-not $o) {
            $rows.Add((New-CheckRow "Servicio $($s.Name)" 'no encontrado' $s.Expect 'AVISO' $s.Why))
            continue
        }
        $state = 'OK'
        if ($s.Expect -eq 'Disabled' -and $o.StartType -ne 'Disabled') { $state = 'AVISO' }
        if ($s.Expect -eq 'Enabled' -and $o.StartType -eq 'Disabled') { $state = 'AVISO' }
        if ($s.Expect -eq 'Running' -and $o.Status -ne 'Running') { $state = 'AVISO' }
        $rows.Add((New-CheckRow "Servicio $($s.Name)" "$($o.Status) / $($o.StartType)" $s.Expect $state $s.Why))
    }

    # cbdhsvc es por sesión (cbdhsvc_xxxx): se comprueba por patrón.
    $cb = Get-Service -Name 'cbdhsvc*' -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cb) {
        $rows.Add((New-CheckRow 'Servicio cbdhsvc*' "$($cb.Status) / $($cb.StartType)" 'Enabled' 'OK' 'historial de portapapeles (base de Win+V)'))
    } else {
        $rows.Add((New-CheckRow 'Servicio cbdhsvc*' 'no encontrado' 'Enabled' 'AVISO' 'historial de portapapeles'))
    }

    # "ylogical": el plan lo menciona, pero no existe como servicio en la máquina de referencia (2026-11-24).
    $yl = Get-Service -Name 'ylogical' -ErrorAction SilentlyContinue
    $rows.Add((New-CheckRow 'Servicio ylogical' $(if ($yl) { "$($yl.Status) / $($yl.StartType)" } else { 'no encontrado' }) 'n/a' 'INFO' 'probable errata del plan: no existe en esta build; no se toca nada'))

    # --- tareas de WinRust (solo lectura)
    foreach ($t in @(
        @{ Name = 'topbar';        Expect = 'presente' }
        @{ Name = 'rs-launcher';   Expect = 'presente' }
        @{ Name = 'rs-search-C';   Expect = 'presente' }
        @{ Name = 'WinRust-update';Expect = 'presente' }
    )) {
        $state = Get-TaskState $t.Name
        $rows.Add((New-CheckRow "Tarea $($t.Name)" $(if ($state) { $state } else { 'no encontrada' }) $t.Expect $(if ($state) { 'OK' } else { 'INFO' }) 'solo lectura'))
    }

    $rows
}

# --------------------------------------------------------------------------- estado (backup + journal)

function Initialize-State {
    $machine = Read-MachineInfo
    $existing = Read-JsonFile $BackupFile
    if ($existing) {
        # Conserva el original de ejecuciones anteriores: -Restore siempre vuelve al estado más antiguo conocido.
        $backup = $existing
        $backup.updatedAt = (Get-Date).ToString('o')
        $backup.mode = 'Apply'
        $backup.restoredAt = $null
        if ($null -eq $backup.registry) { $backup | Add-Member -NotePropertyName registry -NotePropertyValue @() -Force }
    } else {
        $backup = [pscustomobject][ordered]@{
            schemaVersion = 1
            script        = 'tools/iso-provision.ps1'
            plan          = 'tools/iso-replacement.md'
            createdAt     = (Get-Date).ToString('o')
            updatedAt     = (Get-Date).ToString('o')
            restoredAt    = $null
            mode          = 'Apply'
            machine       = [pscustomobject]@{ name = $machine.Name; build = $machine.Build; version = $machine.Version; edition = $machine.Edition }
            user          = [Security.Principal.WindowsIdentity]::GetCurrent().Name
            elevated      = (Test-Admin)
            registry      = @()
        }
    }
    Write-JsonFile $BackupFile $backup
    $script:Backup = $backup

    $journal = Read-JsonFile $JournalFile
    if (-not $journal) {
        $journal = [pscustomobject][ordered]@{ schemaVersion = 1; script = 'tools/iso-provision.ps1'; runs = @() }
    }
    $run = [pscustomobject][ordered]@{
        id         = [guid]::NewGuid().ToString()
        mode       = $(if ($Apply) { 'Apply' } else { 'Restore' })
        startedAt  = (Get-Date).ToString('o')
        finishedAt = $null
        user       = [Security.Principal.WindowsIdentity]::GetCurrent().Name
        elevated   = (Test-Admin)
        machine    = [pscustomobject]@{ name = $machine.Name; build = $machine.Build; version = $machine.Version; edition = $machine.Edition }
        result     = 'en curso'
        entries    = @()
    }
    $journal.runs = @($journal.runs) + $run
    Write-JsonFile $JournalFile $journal
    $script:Journal = $journal
    $script:Run = $run
}

function Add-JournalEntry {
    param($Entry)
    $script:Run.entries = @($script:Run.entries) + $Entry
    $idx = $script:Journal.runs.Count - 1
    $script:Journal.runs[$idx] = $script:Run
    Write-JsonFile $JournalFile $script:Journal
}

function Complete-Journal {
    param([string]$Result)
    $script:Run.finishedAt = (Get-Date).ToString('o')
    $script:Run.result = $Result
    $idx = $script:Journal.runs.Count - 1
    $script:Journal.runs[$idx] = $script:Run
    Write-JsonFile $JournalFile $script:Journal
}

function Get-BackupEntry {
    param([string]$Id, [string]$Path, [string]$Name)
    foreach ($e in @($script:Backup.registry | Where-Object { $_ })) {
        if ($e.id -eq $Id -and $e.path -eq $Path -and $e.name -eq $Name) { return $e }
    }
    return $null
}

function Add-BackupEntry {
    param($Entry)
    $existing = Get-BackupEntry $Entry.id $Entry.path $Entry.name
    if ($existing) { return $existing }
    $entryObj = [pscustomobject]$Entry
    $script:Backup.registry = @($script:Backup.registry) + $entryObj
    $script:Backup.updatedAt = (Get-Date).ToString('o')
    Write-JsonFile $BackupFile $script:Backup
    return $entryObj
}

# --------------------------------------------------------------------------- aplicar

function Invoke-OpApply {
    param($Op, $Desired, $Current)

    $keyState = Get-KeyState $Op.Path
    $entry = Add-BackupEntry ([ordered]@{
        id           = $Op.Id
        section      = $Op.Section
        action       = $Op.Mode
        path         = $Op.Path
        name         = $Op.Name
        kind         = $Current.Kind
        value        = (ConvertTo-StoredData -Kind ([string]$Current.Kind) -Value $Current.Value)
        valueExisted = [bool]$Current.ValueExists
        keyExisted   = [bool]$keyState.KeyExists
        firstMissing = $keyState.FirstMissing
        appliedAt    = (Get-Date).ToString('o')
        restored     = $false
    })

    $from = Format-RegValueText $Current
    $journalEntry = [ordered]@{
        at     = (Get-Date).ToString('o')
        id     = $Op.Id
        action = $Op.Mode
        path   = $Op.Path
        name   = $Op.Name
        from   = $from
        to     = $(if ($Op.Mode -eq 'Delete') { '(eliminado)' } else { (Format-RegValueText ([pscustomobject]@{ KeyExists = $true; ValueExists = $true; Kind = $Op.Kind; Value = $Desired })) })
        result = 'pendiente'
    }
    Add-JournalEntry $journalEntry.Clone()

    try {
        if ($Op.Mode -eq 'Delete') {
            Remove-RegValue -Path $Op.Path -Name $Op.Name | Out-Null
        } else {
            Set-RegValue -Path $Op.Path -Name $Op.Name -Stored (ConvertTo-StoredData -Kind $Op.Kind -Value $Desired) -Kind $Op.Kind
        }
        $after = Read-RegValue $Op.Path $Op.Name
        $ok = $false
        if ($Op.Mode -eq 'Delete') {
            $ok = -not $after.ValueExists
        } elseif ($Op.Mode -eq 'SetNone') {
            $ok = $after.ValueExists
        } else {
            $ok = $after.ValueExists -and (Test-SameValue $after.Value $Desired $Op.Kind)
        }
        $journalEntry.result = $(if ($ok) { 'aplicado' } else { 'ERROR: el valor no quedó como se esperaba' })
        Add-JournalEntry $journalEntry.Clone()
        return $(if ($ok) { 'aplicado' } else { 'ERROR' })
    } catch {
        $journalEntry.result = "ERROR: $($_.Exception.Message)"
        Add-JournalEntry $journalEntry.Clone()
        throw
    }
}

# --------------------------------------------------------------------------- restaurar

function Invoke-Restore {
    $backup = Read-JsonFile $BackupFile
    if (-not $backup) { throw "no hay backup en ${BackupFile}: nada que restaurar" }
    $entries = @($backup.registry | Where-Object { $_ })
    if (-not $entries.Count) { throw "el backup ${BackupFile} no contiene entradas de registro" }

    $needsAdmin = ($entries | Where-Object { $_.path -like 'HKLM\*' }).Count -gt 0
    if ($needsAdmin -and -not (Test-Admin)) { throw 'el backup toca HKLM: ejecuta -Restore desde un PowerShell elevado' }

    Initialize-State
    $rows = New-Object System.Collections.Generic.List[object]
    $errors = 0

    # Orden inverso al aplicado: primero lo último que se tocó.
    $reversed = @($entries)
    [array]::Reverse($reversed)

    foreach ($e in $reversed) {
        $status = ''
        $from = '(actual)'
        $r = Split-RegPath $e.path
        try {
            if ($e.valueExisted) {
                Set-RegValue -Path $e.path -Name $e.name -Stored $e.value -Kind $e.kind
                $status = 'restaurado'
            } else {
                if (Remove-RegValue -Path $e.path -Name $e.name) { $status = 'valor añadido eliminado' }
                else { $status = 'sin cambios' }
                if ($e.firstMissing) {
                    # Solo se borra el subárbol creado si sigue vacío; si otro programa añadió datos, se respeta.
                    if (Test-RegTreeEmpty -Hive $r.Hive -Sub $e.firstMissing) {
                        if (Remove-RegTree -Hive $r.Hive -Sub $e.firstMissing) {
                            $status = 'clave creada eliminada'
                        } else {
                            $status = 'ERROR al borrar la clave creada'
                            $errors++
                        }
                    } else {
                        $status = "$status (clave con otros datos: no se borra)"
                    }
                }
            }

            # Comprobación posterior
            if ($e.valueExisted -and $status -eq 'restaurado') {
                $now = Read-RegValue $e.path $e.name
                if (-not ($now.ValueExists -and (Test-SameValue $now.Value (ConvertTo-RegData -Kind $e.kind -Stored $e.value) $e.kind))) {
                    $status = 'ERROR: no coincide con el original'
                    $errors++
                }
            }
        } catch {
            $status = "ERROR: $($_.Exception.Message)"
            $errors++
        }

        Add-JournalEntry ([ordered]@{
            at = (Get-Date).ToString('o'); id = $e.id; action = 'Restore'; path = $e.path; name = $e.name
            from = $from; to = $(if ($e.valueExisted) { 'valor original' } else { '(eliminado)' }); result = $status
        })

        $rows.Add([pscustomobject]@{
            'Acción' = 'restaurar'
            'Clave'  = (Shorten-RegKey -Hive $r.Hive -Key $r.Sub -Name $e.name)
            'Resultado' = $status
        })
    }

    $backup.restoredAt = (Get-Date).ToString('o')
    $backup.mode = 'Restored'
    Write-JsonFile $BackupFile $backup

    Write-Host ''
    Write-Host 'RESTAURACIÓN' -ForegroundColor Cyan
    Write-WideTable $rows @('Acción', 'Clave', 'Resultado')

    if ($errors) {
        Complete-Journal 'con errores'
        Write-Host "Restauración terminada con $errors error(es). Revisa $JournalFile." -ForegroundColor Red
        exit 1
    }
    Complete-Journal 'ok'
    Write-Host 'Restaurado. Cierra sesión y vuelve a entrar para que Explorer vuelva como shell.' -ForegroundColor Green
    Write-Host 'Comprueba: HKCU Shell, ms-settings:, .txt con "Abrir con", y que no queda IFEO de notepad.exe.' -ForegroundColor Yellow
    exit 0
}

# --------------------------------------------------------------------------- plan / salida

function Write-PlanTable {
    param($Rows)
    # Out-String con ancho fijo: evita que la consola recorte las columnas "Valor nuevo"/"Estado".
    $text = $Rows | Format-Table -Property 'Acción', 'Clave', 'Valor anterior', 'Valor nuevo', 'Estado' -AutoSize -Wrap |
        Out-String -Width 240
    $text -split "`r?`n" | Where-Object { $_ -match '\S' } | ForEach-Object { Write-Host $_ }
}

function Write-WideTable {
    param($Rows, [string[]]$Properties)
    $text = $Rows | Format-Table -Property $Properties -AutoSize -Wrap | Out-String -Width 240
    $text -split "`r?`n" | Where-Object { $_ -match '\S' } | ForEach-Object { Write-Host $_ }
}

function Write-Header {
    $machine = Read-MachineInfo
    Write-Host ''
    Write-Host 'WinRust ISO provisioning' -ForegroundColor Cyan
    Write-Host ("  Equipo     : {0}  ({1} {2}, {3})" -f $machine.Name, $machine.Version, $machine.Build, $machine.Edition)
    Write-Host ("  Usuario    : {0}" -f [Security.Principal.WindowsIdentity]::GetCurrent().Name)
    Write-Host ("  Elevado    : {0}" -f $(if (Test-Admin) { 'sí' } else { 'no' }))
    Write-Host ("  Modo       : {0}" -f $(if ($Apply) { 'APPLY (escribe)' } elseif ($Restore) { 'RESTORE' } else { 'DRYRUN (no escribe)' }))
    Write-Host ("  Backup     : {0}" -f $BackupFile)
    Write-Host ("  Journal    : {0}" -f $JournalFile)
    Write-Host ("  Opcionales : {0}" -f $(if ($IncludeOptional) { 'incluidos' } else { 'omitidos (usa -IncludeOptional)' }))
    Write-Host ''
}

function Write-NextSteps {
    Write-Host ''
    Write-Host 'PRÓXIMOS PASOS EN LA ISO (no los ejecuta este script)' -ForegroundColor Cyan
    Write-Host @'
  1) Auditar el install.wim antes de tocar nada (solo lectura):
       dism /Mount-Image /ImageFile:D:\iso\sources\install.wim /Index:1 /MountDir:C:\Mount /ReadOnly
       dism /Image:C:\Mount /Get-Capabilities
       dism /Image:C:\Mount /Get-ProvisionedAppxPackages
       dism /Unmount-Image /MountDir:C:\Mount /Discard

  2) Quitar SOLO capabilities con sustituto probado (LTSC 26300: son Win32, no AppX):
       Microsoft.Windows.Notepad~~~~0.0.1.0        -> requiere rust-notepad (AÚN NO EXISTE)
       Microsoft.Windows.MSPaint~~~~0.0.1.0        -> requiere rust-paint   (AÚN NO EXISTE)
       Microsoft.Windows.SnippingTool~~~~0.0.1.0   -> DEJAR hasta validar ms-screenclip: en VM
       Media.WindowsMediaPlayer~~~~0.0.1.0         -> opcional (sin sustituto propio)
       Microsoft.Windows.PowerShell.ISE~~~~0.0.1.0 -> opcional
       Browser.InternetExplorer~~~~0.0.1.0         -> opcional (comprobar dependencias)
       MathRecognizer~~~~0.0.1.0                   -> opcional
       App.StepsRecorder~~~~0.0.1.0                -> opcional
     Comando: dism /Image:C:\Mount /Remove-Capability /CapabilityName:<nombre>

  3) Retail 24H2+ (solo si aplica; en esta LTSC no hay estas AppX):
       dism /Image:C:\Mount /Remove-AppxProvisionedPackage /PackageName:Microsoft.WindowsNotepad
       (y el handler ms-* de WinRust en el MISMO paso, o el URI queda roto)

  4) NO quitar nunca: Microsoft.WindowsStore, windows.immersivecontrolpanel, LockApp,
     StartMenuExperienceHost, ShellExperienceHost, MicrosoftWindows.Client.CBS/Core,
     Microsoft.UI.Xaml.CBS, VCLibs / UI.Xaml / WindowsAppRuntime / NET.Native, WinSxS, WinRE.

  5) Cerrar la imagen:
       dism /Image:C:\Mount /Cleanup-Image /StartComponentCleanup
       dism /Unmount-Image /MountDir:C:\Mount /Commit
       oscdimg ... (reconstruir ISO)

  6) Tras instalar la ISO: tools\iso-provision.ps1 -DryRun en la VM, luego -Apply,
     y pasar tools\iso-test.md completo antes de dar la imagen por buena.
'@ | Write-Host
}

function Write-Notes {
    param($Ops)
    # Deduplicado: siete extensiones comparten la misma nota y no aportan nada repetidas.
    $warns = @($Ops | Where-Object { $_.Warn } | Group-Object Warn)
    if ($warns.Count) {
        Write-Host ''
        Write-Host 'AVISOS' -ForegroundColor Cyan
        foreach ($g in $warns) {
            Write-Host ("  ({0})" -f (($g.Group | ForEach-Object { $_.Id }) -join ', ')) -ForegroundColor DarkGray
            Write-Host ("    {0}" -f $g.Name) -ForegroundColor Yellow
        }
    }
    $notes = @($Ops | Where-Object { $_.Note } | Group-Object Note)
    if ($notes.Count) {
        Write-Host ''
        Write-Host 'NOTAS' -ForegroundColor Cyan
        foreach ($g in $notes) {
            Write-Host ("  ({0}) {1}" -f (($g.Group | ForEach-Object { $_.Id }) -join ', '), $g.Name) -ForegroundColor DarkGray
        }
    }
}

# --------------------------------------------------------------------------- main
# --------------------------------------------------------------------------- main

function Invoke-OpsPass {
    # Recorre los pasos. Sin -Execute solo calcula el plan (no escribe nada).
    param($Ops, [switch]$Execute)

    $rows = New-Object System.Collections.Generic.List[object]
    $counts = [ordered]@{ aplicar = 0; sinCambios = 0; saltados = 0; opcionales = 0; errores = 0 }

    foreach ($op in $Ops) {
        $cur = Read-RegValue $op.Path $op.Name
        $desired = Get-OpDesired $op
        $action = Get-ActionLabel $op
        $keyText = Shorten-RegKey -Hive $op.Hive -Key $op.Key -Name $op.Name
        $fromText = Format-RegValueText $cur
        $toText = $(if ($op.Mode -eq 'Delete') { '(se elimina)' } elseif ($null -eq $desired) { '(nada)' } else { Format-RegValueText ([pscustomobject]@{ KeyExists = $true; ValueExists = $true; Kind = $op.Kind; Value = $desired }) })

        # --- motivos para no tocar
        $skip = $null
        if ($op.Optional -and -not $IncludeOptional) { $skip = 'omitido (opcional)' }
        elseif ($op.App -and -not $op.Exe) { $skip = "saltado: falta $($script:Apps[$op.App].Exe)" }

        $status = ''
        if ($skip) {
            $status = $skip
            if ($op.Optional -and -not $IncludeOptional) { $counts['opcionales'] = $counts['opcionales'] + 1 }
            else { $counts['saltados'] = $counts['saltados'] + 1 }
            $toText = '(nada)'
        } else {
            # --- ¿hace falta escribir?
            $needsWrite = $false
            if ($op.Mode -eq 'Delete') {
                $needsWrite = $cur.ValueExists
            } elseif ($op.Mode -eq 'EnsureExists') {
                $needsWrite = -not ($cur.ValueExists -and [string]$cur.Value)
                if (-not $needsWrite) { $toText = '(se mantiene el actual)' }
            } else {
                $needsWrite = -not ($cur.ValueExists -and (Test-SameValue $cur.Value $desired $op.Kind))
            }

            if (-not $needsWrite) {
                $status = 'sin cambios'
                $counts['sinCambios'] = $counts['sinCambios'] + 1
            } elseif ($Execute) {
                try {
                    $status = Invoke-OpApply -Op $op -Desired $desired -Current $cur
                    if ($status -eq 'ERROR') { $counts['errores'] = $counts['errores'] + 1 }
                    else { $counts['aplicar'] = $counts['aplicar'] + 1 }
                } catch {
                    $status = "ERROR: $($_.Exception.Message)"
                    $counts['errores'] = $counts['errores'] + 1
                }
            } else {
                switch ($op.Mode) {
                    'Delete' { $status = 'se quitará' }
                    'EnsureExists' { $status = 'se creará' }
                    default { $status = $(if ($cur.ValueExists) { 'se reemplazará' } else { 'se creará' }) }
                }
                $counts['aplicar'] = $counts['aplicar'] + 1
            }
        }

        $rows.Add([pscustomobject]@{
            'Acción'         = $action
            'Clave'          = $keyText
            'Valor anterior' = $fromText
            'Valor nuevo'    = $toText
            'Estado'         = $status
        })
    }

    [pscustomobject]@{ Rows = $rows; Counts = $counts }
}

if ($Restore) {
    Write-Header
    Invoke-Restore
    exit 0
}

$ops = Get-Ops

if ($Apply -and -not (Test-Admin)) {
    throw 'IFEO vive en HKLM (y App Paths opcional también): ejecuta -Apply desde un PowerShell elevado'
}

Write-Header

if ($Apply) {
    # Pre-paso de solo lectura para poder confirmar antes de tocar nada.
    $pre = Invoke-OpsPass -Ops $ops
    $changes = @($pre.Rows | Where-Object { $_.Estado -like 'se *' })
    if (-not $NoPrompt) {
        Write-Host 'CAMBIOS PROPUESTOS' -ForegroundColor Cyan
        if ($changes.Count) {
            foreach ($r in $changes) { Write-Host ("  - {0}: {1}" -f $r.Acción, $r.Clave) }
        } else {
            Write-Host '  (ninguno: todo está ya como se espera)'
        }
        Write-Host ''
        Write-Host 'Recuerda: HKCU se escribe en el perfil del usuario que eleva la sesión.' -ForegroundColor Yellow
        Write-Host 'Ejecuta como el usuario objetivo (Run as administrator sobre su propia sesión), no como otro admin.' -ForegroundColor Yellow
        $answer = Read-Host '¿Aplicar el plan? (si/no)'
        if ($answer -notmatch '^(s|si|sí|y|yes)$') {
            Write-Host 'Cancelado: no se ha escrito nada.'
            exit 1
        }
    }

    Initialize-State   # backup de los valores originales + journal de la ejecución
    $pass = Invoke-OpsPass -Ops $ops -Execute
} else {
    $pass = Invoke-OpsPass -Ops $ops
}

$counts = $pass.Counts
$avisosPlan = @($ops | Where-Object { $_.Warn })

Write-Host 'REDIRECCIONES' -ForegroundColor Cyan
Write-PlanTable $pass.Rows

Write-Host ''
Write-Host 'COMPROBACIONES (solo lectura: no se cambia nada)' -ForegroundColor Cyan
$checks = Get-Checks
Write-WideTable $checks @('Comprobación', 'Actual', 'Esperado', 'Estado', 'Nota')

Write-Host ''
Write-Host 'RESUMEN' -ForegroundColor Cyan
Write-Host ("  a aplicar/aplicados : {0}" -f $counts['aplicar'])
Write-Host ("  sin cambios         : {0}" -f $counts['sinCambios'])
Write-Host ("  saltados (sin app)  : {0}" -f $counts['saltados'])
Write-Host ("  omitidos (opcional) : {0}" -f $counts['opcionales'])
Write-Host ("  errores             : {0}" -f $counts['errores'])
$avisosCheck = @($checks | Where-Object { $_.Estado -eq 'AVISO' })
Write-Host ("  avisos del plan     : {0}   |  avisos de comprobación: {1}" -f $avisosPlan.Count, $avisosCheck.Count)

Write-Notes $ops
Write-NextSteps

if ($Apply) {
    Write-Host ''
    if ($counts['errores']) {
        Complete-Journal 'con errores'
        Write-Host "Aplicado con $($counts['errores']) error(es). Revisa $JournalFile." -ForegroundColor Red
        Write-Host 'Para volver atrás: .\tools\iso-provision.ps1 -Restore' -ForegroundColor Yellow
        exit 1
    }
    Complete-Journal 'ok'
    Write-Host 'Aplicado. Cierra sesión y vuelve a entrar para probar (algunas cachés del shell no se refrescan).' -ForegroundColor Green
    Write-Host "Backup: $BackupFile" -ForegroundColor DarkGray
    Write-Host "Journal: $JournalFile" -ForegroundColor DarkGray
    Write-Host 'Reversión total: .\tools\iso-provision.ps1 -Restore' -ForegroundColor Yellow
    exit 0
}

Write-Host ''
Write-Host 'DryRun: no se ha escrito nada (ni registro ni estado). Usa -Apply para aplicar.' -ForegroundColor Yellow
exit 0
