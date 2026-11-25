# Checklist de prueba en VM — ISO WinRust + `iso-provision.ps1`

| | |
|---|---|
| Fecha | 2026-11-24 |
| Repo | `C:\winrust-build` |
| Script bajo prueba | `tools\iso-provision.ps1` (DryRun por defecto, `-Apply`, `-Restore`) |
| Diseño de referencia | `tools\iso-replacement.md` |
| Alcance | Probar en una **VM desechable** una ISO limpia + provisioning. No sustituye a la revisión de código. |
| Regla | Cada punto tiene criterio de aceptación y **comando de reversión**. Si algo falla, se revierte ese punto antes de seguir. |

Marcas de resultado: **OK** (pasa), **FALLO** (no cumple), **BLOQ** (no se puede probar: falta un binario o una función), **N/A** (no aplica a esta edición).

> Estado actual de los binarios (2026-11-24): **no existen** `rust-notepad.exe` ni `rust-paint.exe`.
> Todo lo que dependa de ellos debe marcarse **BLOQ**, no **FALLO**. `photoview.exe` existe pero todavía no
> interpreta URIs (`ms-photos:`), y `rust-settings.exe` todavía no interpreta `--system`/la URI.
> Por eso el DryRun del script ya los deja como "saltado" u "omitido (opcional)".

---

## 0. Preparación de la VM

### 0.1 Hardware y snapshot

| # | Requisito | Criterio | Nota |
|---|---|---|---|
| 0.1.1 | VM UEFI + Secure Boot + TPM 2.0 | Arranca con Secure Boot activo; BitLocker disponible | Igual que el equipo objetivo |
| 0.1.2 | 2 discos (opcional) | El segundo disco se ve en `diskpart`/`Get-Disk` | Necesario solo para probar discos/Storage Spaces |
| 0.1.3 | 2 monitores (opcional) | `topbar` dibuja barra en ambos | Si no hay, marcar N/A |
| 0.1.4 | Snapshot **`00-iso-limpia`** antes de tocar nada | Se puede volver a él en < 1 min | Obligatorio |
| 0.1.5 | Snapshot **`01-provisioned`** tras `-Apply` | Punto de retorno para pruebas destructivas (Store, WU, UAC) | Obligatorio |

### 0.2 Cuentas y datos

| # | Requisito | Criterio |
|---|---|---|
| 0.2.1 | Un usuario **administrador local** (`wr-admin`) | Puede elevar y ejecutar `-Apply` |
| 0.2.2 | Un usuario **estándar** (`wr-user`) | Sin privilegios de admin; sirve para UAC y HKLM |
| 0.2.3 | Carpetas con ficheros de prueba `.txt/.log/.ini/.md/.json/.xml/.csv/.png/.pdf` | En `C:\Test\` |
| 0.2.4 | Instaladores de terceros a mano (Steam, Discord, Office, 7-Zip, Firefox/Chrome) | Sin red no se puede probar Store/WU |

### 0.3 Copia del provisioning y política de ejecución

```powershell
# Copia tools\ a la VM (ejemplo: carpeta compartida de Hyper-V)
Copy-Item "\\VBOXSRV\WinRust-share\tools" -Destination C:\WinRust\provisioning -Recurse

# La build de referencia tiene la ejecución de scripts restringida: usa Bypass en la sesión
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass

# Si la carpeta es de solo lectura, manda el estado a una carpeta escribible:
#   .\iso-provision.ps1 -DryRun -StateDir C:\WinRust\state
```

### 0.4 Baseline **antes** de aplicar (obligatorio para comparar)

> Ejecuta este bloque en una consola **elevada** (`dism /Online` lo exige).

```powershell
$b = 'C:\WinRust\baseline'
New-Item -ItemType Directory -Force $b | Out-Null

reg export "HKCU\Software\Classes\ms-settings"   "$b\ms-settings.reg" /y 2>$null
reg export "HKCU\Software\Classes\.txt"          "$b\dot-txt.reg" /y
reg export "HKCU\Software\Classes\Applications"  "$b\applications.reg" /y
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.txt\UserChoice" /v ProgId  > "$b\txt-userchoice.txt"
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.txt\UserChoice" /v Hash    >> "$b\txt-userchoice.txt"
reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options" /s > "$b\ifeo.txt"
reg query "HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon" /v Shell > "$b\shell.txt"
reg export "HKLM\SOFTWARE\Classes\ms-settings" "$b\ms-settings-hklm.reg" /y 2>$null

Get-Service WSearch,AppXSvc,ClipSVC,StateRepository,WaaSMedicSvc,WpnService |
    Select-Object Name,Status,StartType | Out-File "$b\services.txt"
Get-ScheduledTask | Where-Object { $_.TaskName -match 'rs-|topbar|WinRust' } |
    Select-Object TaskName,State | Out-File "$b\tasks.txt"
Get-AppxPackage | Select-Object Name,PackageFullName | Out-File "$b\appx.txt" -Width 400
dism /Online /Get-Capabilities | Out-File "$b\capabilities.txt"
```

**Criterio:** el baseline existe y es legible. Sin baseline no se puede demostrar que `-Restore` y
`UserChoice` quedaron intactos → **no seguir**.

---

## 1. Provisioning: DryRun, Apply, idempotencia, Restore

| # | Prueba | Cómo | Criterio de aceptación | Si falla |
|---|---|---|---|---|
| 1.1 | **DryRun no escribe** | `.\iso-provision.ps1` (sin parámetros). Comparar `reg export` de las claves del baseline y `Get-ChildItem C:\WinRust\state` antes/después | Salida con tabla y "DryRun: no se ha escrito nada"; **no** aparecen `iso-provision.backup.json` ni `journal.json`; hashes de registro iguales | Si escribe: **FALLO de seguridad**. No usar `-Apply`. Revisar `Read-RegValue`/`Get-KeyState` |
| 1.2 | **DryRun muestra las 5 columnas** | Misma ejecución | Se ven `Acción`, `Clave`, `Valor anterior`, `Valor nuevo`, `Estado`; no hay celdas cortadas | FALLO de legibilidad: bajar ancho de tabla o redirigir a fichero |
| 1.3 | **DryRun con opcionales** | `.\iso-provision.ps1 -IncludeOptional` | Aparecen `control.exe`, `msinfo32.exe`, `ms-photos:` y `App Paths` como "se creará", con sus avisos | FALLO: revisar `-Optional`/`-IncludeOptional` |
| 1.4 | **Apply pide confirmación** | `.\iso-provision.ps1 -Apply` y responder `no` | "Cancelado: no se ha escrito nada"; sin backup/journal | FALLO: revisar el flujo de confirmación |
| 1.5 | **Apply crea estado** | `.\iso-provision.ps1 -Apply -NoPrompt` (usuario **administrador**, sesión del usuario objetivo) | Tabla con estados `aplicado`/`sin cambios`; existen `tools\state\iso-provision.backup.json` y `journal.json`; el JSON abre con `Get-Content -Raw \| ConvertFrom-Json` | FALLO: revisar permisos de `tools\state` o `Initialize-State` |
| 1.6 | **Backup completo** | `(Get-Content -Raw ...backup.json \| ConvertFrom-Json).registry` | Cada entrada tiene `id`, `path`, `name`, `kind`, `value`, `valueExisted`, `keyExisted`, `firstMissing` | FALLO: no se puede restaurar con garantías |
| 1.7 | **Journal coherente** | `.runs[-1].entries` | Hay entrada `pendiente` **y** `aplicado` por cada cambio; `result` final `ok` | FALLO de auditoría |
| 1.8 | **Idempotencia** | Ejecutar `-Apply -NoPrompt` otra vez | Todo `sin cambios`; 0 errores; el backup **no** cambia sus valores originales (solo `updatedAt`) | FALLO: `Add-BackupEntry` está sobrescribiendo el original |
| 1.9 | **Restore total** | `.\iso-provision.ps1 -Restore` | Tabla `restaurado`/`clave creada eliminada`; 0 errores; el registro coincide con el baseline | FALLO: no seguir; revisar `Invoke-Restore` y el orden inverso |
| 1.10 | **Restore sin backup** | Renombrar `state` y ejecutar `-Restore` | Error claro "no hay backup en ...: nada que restaurar", sin tocar el registro | FALLO: el script no debe inventar un estado |
| 1.11 | **Restore no toca UserChoice** | Comparar `txt-userchoice.txt` antes/después | `ProgId` y `Hash` idénticos | **FALLO crítico**. Revisar que ninguna receta escriba `FileExts\...\UserChoice` |

### 1.12 Qué debe quedar tras `-Apply` (con los binarios actuales)

| Clave | Valor esperado hoy |
|---|---|
| `HKLM\...\IFEO\notepad.exe\Debugger` | **no existe** (falta `rust-notepad.exe`; paso saltado) |
| `HKLM\...\IFEO\taskmgr.exe\Debugger` | sin cambios: `"C:\Program Files\rtaskmgr\rtaskmgr-win32.exe"` |
| `HKCU\Software\Classes\ms-settings\shell\open\command` | `"C:\Program Files\rust-settings\rust-settings.exe" --system "%1"` |
| `HKCU\...\ms-settings\shell\open\command\DelegateExecute` | **no existe** |
| `HKCU\Software\Classes\ms-screenclip\shell\open\command` | `"C:\Program Files\rs-shot\rs-shot.exe" --region` |
| `HKCU\Software\Classes\ms-snippingtool\shell\open\command` | `"C:\Program Files\rs-shot\rs-shot.exe" --region` |
| `HKCU\Software\Classes\rust-notepad*` y `.txt\OpenWithProgids` | **no se crean** (falta `rust-notepad.exe`) |
| `HKCU\Software\Classes\ms-photos\...` | **no se crea** (opcional y sin soporte de URI en `photoview`) |

Comando de comprobación:

```powershell
reg query "HKCU\Software\Classes\ms-settings\shell\open\command" /v ""
reg query "HKCU\Software\Classes\ms-settings\shell\open\command" /v DelegateExecute   # debe fallar
reg query "HKCU\Software\Classes\ms-screenclip\shell\open\command" /v ""
reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\notepad.exe"  # debe fallar
```

---

## 2. Arranque, login y estabilidad

| # | Prueba | Cómo | Criterio | Si falla |
|---|---|---|---|---|
| 2.1 | Arranque UEFI | Encender desde la ISO | Llega a OOBE sin error de arranque | Volver a `00-iso-limpia`; no es un problema de provisioning |
| 2.2 | OOBE | Cuenta local, idioma, red | OOBE completa y crea el usuario admin | Reinstalar ISO; revisar `unattend.xml` si se usó |
| 2.3 | Primer login | Iniciar sesión | Escritorio y barra en < 15 s; sin pantalla negra permanente | `Ctrl+Alt+Del` → Task Manager → `explorer.exe`; luego `rs-shell.exe --disable`. Snapshot y analizar |
| 2.4 | Reinicio limpio | `shutdown /r /t 0` | Vuelve a escritorio y barra sin pasos manuales | Igual que 2.3 |
| 2.5 | Segundo reinicio tras `-Apply` | Repetir | Idéntico | Igual que 2.3 |
| 2.6 | `ShellHost` vivo | `Get-Process ShellHost,sihost` | Ambos presentes; no hay `explorer.exe` como shell | Si falta `ShellHost`: `-Restore` y revisar que no se quitó ningún paquete XAML |
| 2.7 | Sin pantalla negra al bloquear | `Win+L` y desbloquear | Vuelve al escritorio con barra | `-Restore` + `explorer.exe` |
| 2.8 | `dism /Online /Cleanup-Image /ScanHealth` | Consola elevada | "No se detectaron daños en el almacén" | **FALLO de ISO**: se tocó disco/servicing. Restaurar snapshot |
| 2.9 | `reagentc /info` | Consola elevada | WinRE habilitado | Si no: no continuar, es un fallo de imagen |

**Reversión global de esta sección:** `.\iso-provision.ps1 -Restore` + reinicio. Si el shell no arranca:
`Ctrl+Alt+Del` → Task Manager → *Ejecutar nueva tarea* → `explorer.exe`; después
`& "C:\Program Files\rust-explorer\rs-shell.exe" --disable`.

---

## 3. Shell: barra, escritorio, Inicio, búsqueda, notificaciones, bandeja

| # | Prueba | Cómo | Criterio | Si falla |
|---|---|---|---|---|
| 3.1 | Barra principal | Mirar pantalla | Barra visible, reloj, botón Inicio, ventanas | `tasklist \| findstr topbar`; si no está: `& "C:\Program Files\topbar\topbar.exe"` y revisar tarea `topbar` |
| 3.2 | Barra secundaria | 2 monitores | Barra en ambos, sin solaparse | N/A si un monitor; si falla: cerrar y abrir `topbar` |
| 3.3 | Escritorio | Clic derecho, iconos, fondo | Iconos, fondo, menú contextual y "Mostrar escritorio" funcionan | Reiniciar `rust-explorer --desktop` (o cerrar y abrir `rs-shell`) |
| 3.4 | Menú Inicio | Clic en Inicio; buscar y lanzar una app Win32 y una AppX | Abre, filtra y lanza ambas | `Ctrl+Alt+Del` → `explorer.exe` temporal; revisar IFEO de `StartMenuExperienceHost` |
| 3.5 | Búsqueda propia | `Alt+Space`, buscar un fichero de `C:\Test` | Encuentra archivos y apps | `Get-Process rs` (rust-search); revisar tarea `rs-search-C` |
| 3.6 | Búsqueda de Windows apagada | `Get-Service WSearch` | `Stopped / Disabled` | `sc config WSearch start= disabled` + `net stop WSearch` (o `-Restore`) |
| 3.7 | Notificaciones propias | Provocar un toast (p. ej. `rs-notify.exe`) | Aparece toast y entrada en la campana | Revisar `WinRust-notify`/puente; no bloquea la ISO |
| 3.8 | Centro de notificaciones | Clic en la campana | Panel con historial | Cerrar y reabrir `topbar` |
| 3.9 | Bandeja | Iconos de Steam/Discord/OneDrive/impresora/USB | Los iconos aparecen y su menú abre | Ver 6.1; si no aparecen, comprobar `Shell_TrayWnd` con Spy++ |
| 3.10 | Alt+Tab | `Alt+Tab` con 3 ventanas (incluida una UWP, p. ej. Configuración) | Conmuta y muestra miniaturas | Reiniciar `topbar`; documentar si falla con UWP |
| 3.11 | Task View | `Win+Tab` | Rejilla de ventanas | Reiniciar `topbar` |
| 3.12 | Atajos base | `Win+D`, `Win+E`, `Win+R`, `Win+X`, `Win+Space`, `Win+1..9` | Cada uno hace lo esperado sin Explorer | `rs-shell --disable` y volver a `--enable` |
| 3.13 | "Mostrar en carpeta" | Desde Firefox/Chrome (descarga) y desde Telegram/Discord | Abre la carpeta y selecciona el fichero | Si espera 20 s y falla: es `SHOpenFolderAndSelectItems`; abrir `rust-explorer` y reintentar; documentar app |
| 3.14 | Copiar/mover/borrar/papelera/deshacer | En `rust-explorer` | Operaciones correctas; papelera y `Ctrl+Z` | Revisar `IFileOperation`; no tocar `shell32` |
| 3.15 | Diálogos Abrir/Guardar | En una app Win32 y en una UWP | Abren y guardan | Si falla en UWP: `ApplicationFrameHost`/XAML; `-Restore` |
| 3.16 | **Enviar a** | Clic derecho → Enviar a → Escritorio | Copia/mueve el fichero | Es `shell:sendto` de Explorer: si falla, sustituir por verbo propio; no es bloqueante |
| 3.17 | **Compartir** desde una app | Word/Fotos → Compartir | Abre el panel de compartir o falla de forma controlada | Con `ShellExperienceHost` bloqueado, **se espera que no haya panel nativo**: marcar BLOQ/limitación, no FALLO |
| 3.18 | Teclado táctil / emoji | `Win+.` y teclado táctil | Abre `TextInputHost` | Si no: limitación conocida (falta picker propio); no bloquea la ISO |

**Reversión de shell (si algo de 3.x rompe la sesión):**

```powershell
& "C:\Program Files\rust-explorer\rs-shell.exe" --disable
# y, si hace falta, volver a Explorer como shell por usuario:
reg delete "HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon" /v Shell /f
```

---

## 4. Ajustes y utilidades

| # | Prueba | Cómo | Criterio | Si falla |
|---|---|---|---|---|
| 4.1 | `Win+I` | Pulsar | Abre `rust-settings` (no la app de Windows) | `-Restore` si se tocó el handler; revisar `topbar`/`rs-shell` |
| 4.2 | `ms-settings:display` | `start ms-settings:display` | Abre **ventana de `rust-settings`**; hoy **no** salta a la página (falta `--system`/router) | BLOQ conocido: documentar. Reversión: `reg delete "HKCU\Software\Classes\ms-settings" /f` |
| 4.3 | `ms-settings:sound`, `personalization`, `network-status`, `about`, `bluetooth`, `windowsupdate` | `start ms-settings:<página>` | Las páginas cubiertas por `rust-settings` abren; el resto (p. ej. `printers`) abre la Configuración de Windows | Si **ninguna** abre: `-Restore` y reportar |
| 4.4 | `DelegateExecute` ausente | `reg query "HKCU\...\ms-settings\shell\open\command" /v DelegateExecute` | Falla (valor no existe) | `reg delete "HKCU\Software\Classes\ms-settings\shell\open\command" /v DelegateExecute /f` |
| 4.5 | `Control Panel` clásico | `control.exe`, `control desk.cpl`, `control appwiz.cpl`, `control ncpa.cpl` | Abren los applets | **No** hay IFEO de `control.exe` por defecto; si se aplicó con `-IncludeOptional`, borrar: `reg delete "HKLM\...\Image File Execution Options\control.exe" /v Debugger /f` |
| 4.6 | Panel de control propio | `rust-control.exe` | Categorías y módulos abren | Cerrar y abrir; no bloquea |
| 4.7 | Administrador de tareas | `Ctrl+Shift+Esc` y `taskmgr.exe` | Abre `rtaskmgr-win32` | Si abre el de Windows: comprobar IFEO `taskmgr.exe`; restaurar con `-Restore` |
| 4.8 | Bloc de notas | `notepad.exe` y doble clic en `C:\Test\a.txt` | **Hoy**: debe abrir el Notepad de Windows (el paso está saltado). Con `rust-notepad.exe` presente: debe abrir `rust-notepad` | Si con el exe presente sigue abriendo Windows: revisar IFEO y `Applications\notepad.exe`; `-Restore` |
| 4.9 | Recortes | `Win+Shift+S` | Abre `rs-shot` en modo región (hook) | Reinstalar `rs-shot`; revisar que `topbar` esté vivo |
| 4.10 | URI de recortes | `start ms-screenclip:` | Abre `rs-shot --region` | `reg delete "HKCU\Software\Classes\ms-screenclip" /f` |
| 4.11 | URI `ms-snippingtool:` | `start ms-snippingtool:` | Abre `rs-shot --region` | `reg delete "HKCU\Software\Classes\ms-snippingtool" /f` |
| 4.12 | Ejecutar (Win+R) | `Win+R`, escribir `cmd`, `notepad`, `C:\Test`, `ms-settings:display`, `shell:downloads` | `rs-launcher` resuelve comandos, rutas y URIs | Si aparece la caja de Windows o no abre: revisar hotkey en `rs-shell`; no bloquea si hay alternativa |
| 4.13 | Explorer (carpetas) | `Win+E`, doble clic en una carpeta, unidad de red | Abre `rust-explorer`, no `explorer.exe` | `HKCU\Software\Classes\Directory\shell\open\command` → `-Restore` |
| 4.14 | `msinfo32` | `msinfo32.exe` | **Hoy**: abre el de Windows (paso opcional no aplicado). Si se aplicó: abre `rust-control` | `reg delete "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\msinfo32.exe" /v Debugger /f` |
| 4.15 | `services.msc`, `devmgmt.msc`, `diskmgmt.msc`, `eventvwr.msc` | Ejecutar cada uno | Abren (aunque sea vía `mmc.exe`) | Si no: `-Restore`; comprobar que no se aplicó IFEO de `mmc.exe`/`control.exe` |
| 4.16 | `regedit`, `msconfig`, `resmon`, `perfmon`, `dxdiag`, `charmap`, `osk`, `magnify`, `narrator` | Ejecutar | Abren | No hay redirecciones sobre ellos; si fallan, es problema de ISO |
| 4.17 | `rs-du` / `cleanmgr` | Ejecutar y simular limpieza | Analiza y no rompe nada | Cerrar; no bloquea |
| 4.18 | `ms-screenclip` desde una app | En Configuración → Accesibilidad → Recortes (o app de terceros) | Abre `rs-shot` | Igual que 4.10 |

**Reversión de ajustes/URI:**

```powershell
reg delete "HKCU\Software\Classes\ms-settings"     /f
reg delete "HKCU\Software\Classes\ms-screenclip"   /f
reg delete "HKCU\Software\Classes\ms-snippingtool" /f
reg delete "HKCU\Software\Classes\ms-photos"       /f
reg delete "HKCU\Software\Classes\ms-paint"        /f
# o todo de una vez:
.\iso-provision.ps1 -Restore
```

---

## 5. Asociaciones y `UserChoice`

> Regla: **nunca** se escribe `...\FileExts\<ext>\UserChoice`. El script solo crea ProgID/`OpenWithProgids`
> y redefine `Applications\notepad.exe` cuando `rust-notepad.exe` existe.

| # | Prueba | Cómo | Criterio | Si falla |
|---|---|---|---|---|
| 5.1 | `.txt` sin regresión | Doble clic en `C:\Test\a.txt` | Sin `rust-notepad`, abre el Notepad de Windows (comportamiento esperado). Con el exe, abre `rust-notepad` | Revisar `Applications\notepad.exe\shell\open\command`; `-Restore` |
| 5.2 | `.txt` "Abrir con" | Clic derecho → Abrir con → Elegir otra aplicación | Aparece **WinRust Text** si `rust-notepad.exe` existe y el paso se aplicó | Si no aparece: comprobar `HKCU\Software\Classes\.txt\OpenWithProgids\rust-notepad`; `-Restore` |
| 5.3 | `.md/.json/.xml/.csv/.log/.ini` | Clic derecho → Abrir con | WinRust en la lista (si hay exe) | Igual que 5.2, por extensión |
| 5.4 | `.png` | Doble clic y "Abrir con" | Sin cambios en el predeterminado; `photoview` disponible si está instalado | `-Restore`; no se tocó `.png` en el script |
| 5.5 | `.pdf` | Doble clic | Abre el visor predeterminado del usuario (UCPD protege `.pdf`) | **No tocar**: es `UserChoice`; documentar como limitación, no aplicar |
| 5.6 | `UserChoice` intacto `.txt` | Comparar `reg query ...UserChoice /v ProgId` y `/v Hash` con el baseline | Idénticos byte a byte | **FALLO crítico**: algo escribió `UserChoice`. Restaurar desde el `.reg` del baseline |
| 5.7 | `UserChoice` intacto `.pdf/.png` | Igual | Idénticos | Igual |
| 5.8 | ProgID propio | `reg query "HKCU\Software\Classes\rust-notepad\shell\open\command"` | Comando `"...\rust-notepad.exe" "%1"` (si el exe existe) | `reg delete "HKCU\Software\Classes\rust-notepad" /f` |
| 5.9 | Default `.txt` no cambiado | `assoc .txt` y `reg query "HKCU\Software\Classes\.txt"` | Sin valor `(Default)` nuevo de WinRust; `UserChoice` manda | `-Restore` |
| 5.10 | "Abrir con" de terceros | Una app (p. ej. 7-Zip) abre su propio diálogo | No se rompe por el ProgID nuevo | Quitar el valor `rust-notepad` de `OpenWithProgids` |

**Reversión de asociaciones:**

```powershell
reg delete "HKCU\Software\Classes\rust-notepad" /f
foreach ($e in '.txt','.log','.ini','.md','.json','.xml','.csv') {
  reg delete "HKCU\Software\Classes\$e\OpenWithProgids" /v rust-notepad /f 2>$null
}
reg delete "HKCU\Software\Classes\Applications\rust-notepad.exe" /f
# Applications\notepad.exe: vuelve al comando original con:
reg add "HKCU\Software\Classes\Applications\notepad.exe\shell\open\command" /ve /d '"C:\Windows\System32\notepad.exe" "%1"' /f
# o, mejor, -Restore (usa el valor exacto guardado en el backup)
```

---

## 6. Apps de terceros y sistema

| # | Prueba | Cómo | Criterio | Si falla |
|---|---|---|---|---|
| 6.1 | Steam | Instalar, iniciar sesión, abrir un juego | Arranca; icono en bandeja; sin error de anti-cheat | Desinstalar; documentar el anti-cheat. No es un fallo de provisioning si el juego no soporta el entorno |
| 6.2 | Discord | Instalar, login, notificaciones | Arranca, notifica y su icono está en bandeja | Probar sin `topbar` para aislar |
| 6.3 | Office (Word/Excel) | Instalar, abrir/guardar en `C:\Test` | Abre, guarda, "Abrir con", recientes | Revisar `SHOpenFolderAndSelectItems` y diálogos |
| 6.4 | 7-Zip | Instalar; clic derecho → 7-Zip; comprimir/extraer | Menú contextual y operaciones funcionan | Si el menú no aparece: es del shell de Windows; usar la app; documentar |
| 6.5 | Navegador predeterminado | Firefox/Chrome como predeterminado, abrir `https://` desde otra app | Abre el navegador | `UserChoice` de `http/https` está protegido por UCPD: no forzar por registro |
| 6.6 | UAC (admin) | Abrir `regedit` como admin | Pide consentimiento en escritorio seguro y funciona | `-Restore` si algo del shell interfiere; UAC no se toca |
| 6.7 | UAC (usuario estándar) | Con `wr-user`, abrir `diskmgmt.msc` | Pide credenciales de admin | Igual |
| 6.8 | Usuario estándar no escribe HKLM | `reg add "HKLM\Software\TestWR" /v x /d 1` | Acceso denegado | Fallo de imagen/políticas, no del script |
| 6.9 | Store | Abrir Store, instalar una app gratuita, actualizarla | Instala y actualiza | No quitar Store; revisar `ClipSVC`/`AppXSvc` |
| 6.10 | Windows Update | Buscar, descargar e instalar una LCU/definición | Instala sin error CBS | `DISM /Online /Cleanup-Image /RestoreHealth`; si falla, la ISO tocó servicing |
| 6.11 | `sfc /scannow` | Consola elevada | "No se encontraron infracciones" | **FALLO**: se modificó disco. Restaurar `00-iso-limpia` y revisar F1 |
| 6.12 | Reinicio tras LCU | Reiniciar | Vuelve el shell y el provisioning sigue (los valores HKCU sobreviven) | Re-ejecutar `-DryRun` y luego `-Apply` (idempotente) |
| 6.13 | App MSIX de terceros | WhatsApp/Spotify | Instala y notifica | Revisar `AppXSvc`/`StateRepository` |
| 6.14 | Impresora / escáner | Añadir impresora desde Configuración | Abre la página (fallback a Windows) e imprime | Documentar si el fallback no abre |
| 6.15 | Bluetooth | Emparejar auriculares | Funciona y aparece en bandeja | Revisar página BT de `rust-settings` |
| 6.16 | App que abre `ms-settings:` concreto | p. ej. "Administrar dispositivos Bluetooth" | Abre `rust-settings` o Windows, nunca un error | `-Restore` del handler si hay error |

**Reversión de sistema:** las pruebas 6.x no cambian registro por sí mismas; para volver atrás:
`.\iso-provision.ps1 -Restore` + reinicio. Si se rompió el shell: `explorer.exe` desde Task Manager y
`rs-shell --disable`.

---

## 7. Recuperación y reversión (obligatorio antes de dar la ISO por buena)

| # | Prueba | Cómo | Criterio | Si falla |
|---|---|---|---|---|
| 7.1 | `-Restore` deja el registro como el baseline | `.\iso-provision.ps1 -Restore` y comparar exports del baseline | Sin diferencias en las claves tocadas | **FALLO bloqueante**: no publicar la ISO |
| 7.2 | Tras `-Restore`, Explorer vuelve | Cerrar sesión y entrar | `explorer.exe` como shell, barra de Windows, todo normal | `reg delete "HKCU\...\Winlogon" /v Shell /f` + reinicio |
| 7.3 | Recrear el perfil | Borrar `C:\Users\wr-user` y entrar | Si el provisioning es por usuario, el usuario nuevo no lo tiene (diseño) o se re-aplica con la tarea | Documentar el comportamiento elegido |
| 7.4 | WinRE repara | `reagentc /boottore` o arrancar WinRE | Símbolo del sistema disponible; se puede hacer `reg load` y borrar el `Shell` | Si WinRE no arranca: fallo de ISO (`Winre.wim` tocado) |
| 7.5 | Recuperación de IFEO | Con `notepad.exe` redirigido a un exe inexistente, intentar abrirlo | Falla con error de fichero (comportamiento documentado); se recupera borrando `Debugger` | `reg delete "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\notepad.exe" /v Debugger /f` |
| 7.6 | Journal útil | Leer `journal.json` tras Apply+Restore | Se ve qué se aplicó, cuándo y qué se restauró | Corregir el journal antes de firmar la ISO |

---

## 8. Criterios globales (go / no-go)

La ISO se considera **aceptable** solo si:

1. **Ningún FALLO** en las secciones 1, 2, 5 y 7.
2. `UserChoice` de `.txt`, `.png` y `.pdf` **idéntico** al baseline (5.6/5.7).
3. `sfc /scannow` y `dism /Online /Cleanup-Image /ScanHealth` limpios (2.8, 6.11).
4. `-DryRun` se mantiene sin escrituras (1.1) y `-Apply` es idempotente (1.8).
5. `-Restore` devuelve el sistema al baseline y Explorer vuelve como shell (7.1/7.2).
6. Store instala una app y WU aplica una actualización (6.9/6.10).
7. Los puntos **BLOQ** están documentados como pendientes de crate, no ocultos.

### 8.1 Plantilla de resultados

| Sección | OK | FALLO | BLOQ | N/A | Notas |
|---|---|---|---|---|---|
| 1. Provisioning | | | | | |
| 2. Arranque | | | | | |
| 3. Shell | | | | | |
| 4. Ajustes | | | | | |
| 5. Asociaciones | | | | | |
| 6. Apps/sistema | | | | | |
| 7. Recuperación | | | | | |

---

## 9. Anexo A — Diagnóstico rápido

```powershell
# Handler de Configuración
reg query "HKCU\Software\Classes\ms-settings\shell\open\command" /v ""
reg query "HKLM\Software\Classes\ms-settings\shell\open\command" /v DelegateExecute

# Recortes
reg query "HKCU\Software\Classes\ms-screenclip\shell\open\command" /v ""

# IFEO
reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\notepad.exe"
reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe"

# Shell por usuario y tarea
reg query "HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon" /v Shell
Get-ScheduledTask -TaskName rs-overlay | Select-Object TaskName,State

# Asociaciones
reg query "HKCU\Software\Classes\.txt\OpenWithProgids"
reg query "HKCU\Software\Classes\rust-notepad\shell\open\command"
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.txt\UserChoice"

# Servicios que no deben tocarse
Get-Service WSearch,AppXSvc,ClipSVC,StateRepository,WaaSMedicSvc,WpnService,ucpd

# Procesos del shell
Get-Process rs-shell,topbar,rust-explorer,ShellHost,sihost,explorer -ErrorAction SilentlyContinue |
    Select-Object Name,Id,Path
```

## 10. Anexo B — Qué falta de los crates (bloqueantes de la checklist)

| Falta | Consecuencia en esta checklist | Puntos afectados |
|---|---|---|
| `rust-notepad.exe` | No se crean ProgID/`OpenWithProgids` ni IFEO de `notepad.exe`; `.txt` sigue con Notepad de Windows | 1.12, 4.8, 5.1, 5.2, 5.3 |
| `rust-paint.exe` | `ms-paint:` no se registra (el guard lo salta) | 1.12 |
| Router `winrust-uri` / flag `--system` en `rust-settings` | `ms-settings:<página>` abre la ventana principal, no la página | 4.2, 4.3, 6.16 |
| Soporte de URI en `photoview` (`ms-photos:`) | El handler queda opcional y sin aplicar | 4.x, 5.4 |
| `rs-shot` como handler (ya existe `--region`) | **No bloquea**: el handler de `ms-screenclip:` ya se puede probar | 4.9–4.11 |
| Picker propio de emoji/portapapeles | `Win+.`/`Win+V` siguen siendo de Windows o no existen | 3.18 |
| Share sheet propio | "Compartir" no tiene panel nativo con `ShellExperienceHost` bloqueado | 3.17 |

**Regla:** un punto BLOQ no invalida la ISO, pero **sí** impide quitar la capability/AppX correspondiente en
DISM (`Microsoft.Windows.Notepad`, `Microsoft.Windows.MSPaint`, `Microsoft.ScreenSketch`...).

---

*Fin. Cualquier cambio en el sistema debe pasar por `iso-provision.ps1 -DryRun` y por esta checklist antes de
darse por bueno. La reversión global es siempre `iso-provision.ps1 -Restore` + reinicio.*
