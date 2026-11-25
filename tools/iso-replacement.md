# Sustitución de Explorer y del resto de componentes de Windows por WinRust en una ISO modificada

**Documento de diseño. Solo investigación: no se ha modificado el sistema, el registro ni el código del repo.**
La única escritura es este fichero (`tools/iso-replacement.md`).

| | |
|---|---|
| Fecha | 2026-11-24 |
| Repo | `C:\winrust-build` (fork de [Lemxy/WinRust](https://github.com/Lemxy/WinRust)) |
| Máquina de referencia | Windows 10 Enterprise LTSC 2024, 26H2, build **26300.9711**, edición `EnterpriseS` |
| Alcance | Diseño de una ISO "WinRust": qué se puede quitar, cómo se redirige y qué falta por desarrollar |
| Base ya aplicada | `rs-shell` como shell de usuario, `topbar`, `rust-explorer --desktop`, IFEO de los hosts del shell, `rtaskmgr`, `rust-control`, `rust-settings`, `rs-search`, `rs-shot`, `rs-du` |

## Convenciones

| Marca | Significado |
|---|---|
| **[hecho]** | Verificado en este repo (ruta:línea) o en esta máquina con una consulta de solo lectura |
| **[doc]** | Documentado por Microsoft (enlace en la sección 12) |
| **[repo]** | Afirmación que el propio fork documenta (`FORK-CHANGES.md`, `PENDING.md`, `ROADMAP.md`) |
| **[conjetura]** | Razonamiento propio sin verificación directa; **hay que probarlo** antes de diseñar sobre ello |
| **[riesgo]** | Puede romper el arranque, el servicing, la activación o la seguridad |

> Regla de oro del documento: **no se borran binarios del sistema**. Se sustituye *comportamiento* (shell, handlers,
> asociaciones), no *ficheros*. Todo lo que se toca debe tener un `-Restore` y un journal, como los scripts actuales.

---

## 0. Resumen ejecutivo

1. **El fork ya sustituye la capa de "experiencia" del shell**: escritorio (`rust-explorer --desktop`), barra (`topbar`,
   que además crea su propio `Shell_TrayWnd` para recibir `Shell_NotifyIcon`), menú Inicio (3 estilos), búsqueda
   (`rust-search`), notificaciones propias (`winrust-notify` + puente `UserNotificationListener`), Task Manager
   (`rtaskmgr`), Panel de control (`rust-control`), captura (`rs-shot`), limpieza (`rs-du`) y ajustes (`rust-settings`).
   Ver `FORK-CHANGES.md` y `PENDING.md` sección 4.
2. **Falta la capa de "aplicaciones y diálogos"**: editor de texto, calculadora, Paint/Fotos completos, reproductor,
   visor de eventos, servicios, dispositivos, discos, usuarios, registro, portapapeles (Win+V), emoji (Win+.), Run
   (existe `rs-launcher`), Task View completo, snap layouts (Win+Z), compartir/proyectar, y **los handlers de URI**
   (`ms-settings:`, `ms-photos:`, `ms-paint:`, `ms-screenclip:`...) que hoy siguen apuntando a Windows.
3. **La ISO correcta no es "Windows sin Explorer"**, es **Windows completo + provisioning**: se recortan solo
   paquetes/capabilities *soportados* (DISM) y el resto se resuelve **después de instalar** con registro (IFEO, COM,
   handlers, asociaciones). Borrar `explorer.exe`, `shell32.dll`, `SystemSettings` o los paquetes XAML rompe el
   servicing, `sfc`, las actualizaciones y media docena de APIs que WinRust usa a diario.
4. **`explorer.exe` debe seguir en disco y ejecutable**. WinRust ya lo necesita para *hand-offs* (URLs, `ms-settings:`,
   `/select`) y `rust-explorer` usa `IFileOperation` (shell32) e `IShellWindows`; además el propio Explorer hospeda
   servidores COM (`/factory`) que dan soporte a bandeja, namespace y diálogos. El fork ya lo documenta
   (`tools/explorer-edge.ps1`, aviso de `-HardFreeze`).
5. **IFEO es la herramienta adecuada para *hosts* del shell** (`StartMenuExperienceHost`, `SearchHost`,
   `ShellExperienceHost`, `Widgets`, `Cortana`) y para *redirigir ejecutables sueltos* (`taskmgr.exe` ya lo hace,
   `notepad.exe` podría). **No es adecuada para `explorer.exe`** ni para servicios, y tiene dos trampas: la clave que
   se usa depende de la **bitness del proceso que llama** a `CreateProcess` (no del binario lanzado) y EDR la marca
   como T1546.012.
6. **COM es el mecanismo para las clases del shell** (`InprocServer32`/`LocalServer32` en `HKCU\Software\Classes\CLSID`),
   pero **solo funciona en procesos de integridad media**: un proceso elevado ignora el registro por usuario
   (`The COM Elevation Moniker`, MS). Para clases que usan procesos elevados hay que ir a `HKLM` (o a un shim).
7. **Los handlers `ms-*:` se redirigen limpios por usuario**: `HKCU\Software\Classes\ms-settings\shell\open\command`
   con un `(Default)` propio y **sin** `DelegateExecute`. Hoy el handler de Windows es `DelegateExecute =
   {4ed3a719-cea8-4bd9-910d-e252f997afc2}` (`windows.system.launcher.dll`) con el comando vacío **[hecho]**.
   Ojo: el hash `UserChoice` y el driver `UCPD.sys` **no** protegen `ms-settings`, pero sí `http/https/.pdf`
   **[hecho: UCPD en ejecución en esta máquina]**.
8. **Las asociaciones de ficheros no se pueden escribir a mano** en Windows 11 moderno: `UserChoice` tiene hash y
   `UCPD.sys`; el camino soportado es el XML de asociaciones (`DISM /Export-DefaultAppAssociations`) o la UI. Para
   WinRust la vía práctica es `OpenWithProgids` + "Abrir con" propio + registrar ProgIDs propios, **no** pisar
   `UserChoice`.
9. **MSIX/AppX**: en retail (24H2+) Notepad/Paint/Fotos/Calculadora/Reloj/Terminal/Cámara son paquetes; en esta LTSC
   son **capabilities Win32** (`Microsoft.Windows.Notepad`, `Microsoft.Windows.MSPaint`,
   `Microsoft.Windows.SnippingTool`, `Media.WindowsMediaPlayer`...) **[hecho]**. Los paquetes del shell
   (`StartMenuExperienceHost`, `ShellExperienceHost`, `LockApp`, `immersivecontrolpanel`,
   `MicrosoftWindows.Client.CBS/Core`, `Microsoft.UI.Xaml.CBS`) salen como `NonRemovable=True` **[hecho]**: no se
   quitan, se bloquean.
10. **En 26xxx el shell XAML se hospeda en `ShellHost.exe`** (proceso en ejecución que carga `Windows.UI.Xaml.dll` y
    `Microsoft.UI.Xaml.dll`) **[hecho]**. Consecuencia: bloquear `StartMenuExperienceHost.exe` por IFEO puede no ser
    suficiente ni necesario en todas las builds; hay que **verificar por build** qué proceso dibuja qué **[conjetura]**.
11. **La ISO debe ser "clean + provisioning"**, no "borrada". Ventajas: servicing intacto, `sfc`/DISM funcionan,
    actualizaciones no fallan, se puede revertir. Inconveniente: el recorte real de disco es modesto (unos cientos de
    MB, no GB) y el provisioning hay que re-aplicarlo tras algunas actualizaciones.
12. **Trabajo imprescindible que falta**: editor de texto, calculadora, visor/editor de imágenes, reproductor,
    páginas de servicios/dispositivos/discos/eventos/usuarios en `rust-control`, portapapeles, emoji, Run (existe
    `rs-launcher`), Task View completo y el **router de `ms-settings:`**.
13. **Riesgo principal**: que una actualización restaure los componentes (LCU/feature update), que `sfc`/DISM
    "repare" lo que hayamos cambiado en disco (por eso no se toca disco), y que el antivirus/anti-cheat interprete
    IFEO/COM por usuario como manipulación. Mitigación: registro por usuario, journal, `-Restore`, y no tocar
    `WinSxS`/`System32`.
14. **Recuperación**: el shell está en `HKCU` (por usuario), así que un fallo deja al usuario con pantalla vacía pero
    el sistema arranca; `Ctrl+Alt+Del -> Task Manager -> ejecutar explorer.exe` y `rs-shell --disable` recuperan.
    Un shell en `HKLM` (todos los usuarios) es mucho más peligroso: solo para kiosco/`Shell Launcher` (Ent/Edu/IoT).

---## 1. Punto de partida verificado

### 1.1 Lo que el fork ya hace (con referencia)

| # | Cambio | Mecanismo | Dónde está | Reversión |
|---|---|---|---|---|
| 1 | `rs-shell` es el shell del usuario | `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon\Shell` = `C:\Program Files\rust-explorer\rs-shell.exe` | `plugins/rs-shell/src/main.rs:1-18`; `tools/shell-phase2.ps1` | `rs-shell --disable` o `shell-phase2.ps1 -Restore` |
| 2 | Un solo shell (sin pelea con Explorer) | tarea `rs-overlay` deshabilitada | `tools/shell-phase2.ps1:210-216` | `-Restore` |
| 3 | Hosts del shell de Windows bloqueados | IFEO `Debugger = cmd.exe /c exit` para StartMenuExperienceHost, SearchApp, SearchUI, SearchHost, Widgets, Cortana y ShellExperienceHost | `tools/shell-phase2.ps1:68-78` | `-Restore` |
| 4 | Task Manager propio | IFEO `taskmgr.exe` -> `C:\Program Files\rtaskmgr\rtaskmgr-win32.exe` | `plugins/rtaskmgr-win32`; verificado en el registro | uninstall de rtaskmgr |
| 5 | Carpetas abren en `rust-explorer` | `HKCU\Software\Classes\Directory\shell\open\command` y `Folder\...` | verificado en el registro | borrar esas claves |
| 6 | Búsqueda propia | `WSearch` deshabilitado (`Stopped/Disabled`), `rust-search` + `rs-launcher` (Alt+Space) | `plugins/rust-search`; servicio verificado | instalador / `sc config` |
| 7 | Escritorio propio | `rust-explorer --desktop`, clase `Progman`, `WorkerW` hija, `SHELLDLL_DefView` | `plugins/rust-explorer/src/main.rs:1964-2012, 2613-2615` | cerrar el proceso |
| 8 | Barra propia y host de bandeja | `topbar` registra una ventana oculta de clase `Shell_TrayWnd` y recibe `Shell_NotifyIcon` por `WM_COPYDATA`; reemite `TaskbarCreated` | `plugins/topbar/src/main.rs:3584-3587`; `tray.rs:15-40` | cerrar `topbar` |
| 9 | Notificaciones propias | `\\.\pipe\winrust-notify`, toasts + campana + historial; puente `UserNotificationListener` | `FORK-CHANGES.md:76-85`; `tools/notify-bridge.md` | cerrar el puente |
| 10 | Panel de control propio | `rust-control` (categorías, 13 módulos de página) | `plugins/rust-control/src/pages` | n/a |
| 11 | Ajustes propios | `rust-settings` (pantalla, sonido, energía, red, personalización, Bluetooth, apps, Windows Update, About, More) | `plugins/rust-settings/src/main.rs:7, 211-216` | n/a |
| 12 | Captura/grabación | `rs-shot` por hook de teclado (Win+Shift+S, PrintScreen, Win+Alt+R); **sin** handler `ms-screenclip:` | `tools/rs-shot-spec.md` sección 5 | n/a |
| 13 | Limpieza de disco | `rs-du` (sustituye a `cleanmgr`) | `plugins/rs-du` | n/a |
| 14 | Edge fuera | setup.exe + AppX + políticas EdgeUpdate + tareas | `tools/explorer-edge.ps1:1217-1484` | `-Restore` (no reinstala Edge) |
| 15 | Explorer "congelado" sin bloquearlo | políticas `NoDesktop`, `NoTrayContextMenu`, `NoViewContextMenu`, `AutoRestartShell=0` | `tools/explorer-edge.ps1:1070-1106` | `-Restore` |
| 16 | Visor de fotos | `photoview` (WIC) + "Abrir con" | `plugins/photoview` | n/a |
| 17 | Desbloat / purge | servicios, telemetría, AppX | `plugins/debloat`, `plugins/purge` | `--undo`, log |

### 1.2 Estado real de la máquina de referencia (consultas de solo lectura)

Esto importa porque **la ISO objetivo no es retail**: es LTSC. Muchas suposiciones de "Windows 11 moderno" no aplican.

| Hecho observado | Valor | Implicación |
|---|---|---|
| Edición / build | `EnterpriseS` (LTSC 2024), 26H2, **26300.9711** | Sin Widgets/Cortana/Copilot/Photos/Paint/Notepad AppX; el recorte de la ISO es distinto al de Pro |
| `Winlogon\Shell` (HKLM) | `explorer.exe` | El shell global sigue siendo Explorer; el cambio es por usuario |
| `Winlogon\Userinit` | `C:\Windows\system32\userinit.exe,` | `userinit` lanza el shell; no se toca |
| `AutoRestartShell` | `0` | Explorer no se autorrelanza |
| `HKCU\...\Winlogon\Shell` | `C:\Program Files\rust-explorer\rs-shell.exe` | Shell por usuario **[hecho]** |
| IFEO activos | Start/Search/ShellExperienceHost/Widgets/Cortana -> `cmd.exe /c exit`; `taskmgr.exe` -> `rtaskmgr-win32.exe` | Bloqueo por nombre de imagen |
| `explorer.exe` | **`C:\Windows\explorer.exe`** (no en `System32`), **hardlink** a `C:\Windows\WinSxS\...\explorer.exe` | `fsutil hardlink list` lo confirma: editar el fichero edita el store |
| Procesos del shell | `ShellHost.exe` (carga `Windows.UI.Xaml.dll`, `Microsoft.UI.Xaml.dll`), `sihost.exe`, `topbar`, `rust-explorer`, `rs-shell`, `rs-launcher`; **cero `explorer.exe`** | El shell XAML vive en `ShellHost` en 26xxx |
| `ms-settings:` | `(Default)` vacío + `DelegateExecute={4ed3a719-cea8-4bd9-910d-e252f997afc2}` (`windows.system.launcher.dll`) | Punto de redirección limpio por usuario |
| Configuración (AUMID) | `windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel`; exe en `C:\Windows\ImmersiveControlPanel\SystemSettings.exe` | El ejecutable **no** está en `System32` |
| Paquetes del shell | `Microsoft.Windows.StartMenuExperienceHost`, `Microsoft.Windows.ShellExperienceHost`, `Microsoft.LockApp`, `windows.immersivecontrolpanel`, `MicrosoftWindows.Client.CBS/Core`, `Microsoft.UI.Xaml.CBS` -> **`NonRemovable=True`** | No se quitan: se bloquean |
| Store | `Microsoft.WindowsStore` instalado, `NonRemovable=False` | Microsoft **no soporta** quitarlo |
| Apps "de usuario" | Sin Photos/Paint/Notepad/Calculator/Camera/Terminal/Clock AppX | En LTSC son capacidades Win32 |
| Capabilities relevantes | `Microsoft.Windows.Notepad`, `Microsoft.Windows.MSPaint`, `Microsoft.Windows.SnippingTool`, `Media.WindowsMediaPlayer`, `Microsoft.Windows.PowerShell.ISE`, `Browser.InternetExplorer`, `MathRecognizer`, `App.StepsRecorder` -> **Instalado** | Se pueden quitar con `/Remove-Capability` |
| `TextInputHost.exe` | `C:\Windows\SystemApps\SxS\MicrosoftWindows.61869836.InpApp_cw5n1h2txyewy\TextInputHost.exe` | Teclado táctil/emoji/IME vive en un paquete SxS |
| `UCPD.sys` | driver `ucpd` **RUNNING** | `UserChoice` de http/https/.pdf/.docx... protegido |
| Asociación `http` | `UserChoice` -> `FirefoxURL-308046B0AF4A39CB` | No hay handler `HKCR\http\shell\open\command` (IE/Edge fuera) |
| `Shell_TrayWnd` | lo crea `topbar` cuando no hay Explorer | La bandeja de terceros funciona sin Explorer |
| Herramientas | `control.exe`, `mmc.exe`, `regedit.exe` (`C:\Windows`), `notepad.exe`, `mspaint.exe`, `calc.exe` (stub), `SnippingTool.exe`, `osk/magnify/narrator/charmap/dxdiag/msinfo32/resmon/perfmon/cleanmgr/mstsc/msconfig` presentes | Casi todo el arsenal clásico sigue ahí |

### 1.3 Tres niveles de "sustituir" (y por qué solo dos son viables)

| Nivel | Qué significa | Viabilidad | Cómo |
|---|---|---|---|
| **Función** | El usuario obtiene la misma capacidad por otro programa (barra -> `topbar`) | Sí, soportado y ya hecho | Shell de usuario, IFEO de hosts, handlers, asociaciones |
| **Identidad** | El nombre/AUMID/CLSID sigue existiendo, pero detrás está WinRust (p. ej. `ms-settings:` -> `rust-settings`) | Sí, con matices | Handler por usuario, COM por usuario, `DelegateExecute`, ProgID propio |
| **Binario** | `explorer.exe` *es* `rs-shell.exe`; `System32\notepad.exe` *es* Rust | No viable | Hardlinks a WinSxS, WRP, firma, servicing, Defender, actualizaciones |
El nivel binario falla por cinco motivos concretos **[hecho/doc]**:

1. `C:\Windows\explorer.exe` es un **hardlink** al store (`WinSxS`): cualquier escritura altera la copia de servicio.
2. Windows Resource Protection y `sfc` comparan contra el manifiesto de componentes; `DISM /RestoreHealth` y cada LCU
   reemplazan el fichero.
3. La firma Authenticode y el catálogo no cubrirían un binario ajeno; procesos PPL/CI y anti-cheat lo rechazan.
4. Los *app execution aliases* (`%LOCALAPPDATA%\Microsoft\WindowsApps\notepad.exe`, reparse points de 0 bytes)
   resuelven antes que el PATH y tienen su propia lógica (`AppExecutionAliasRedirect` en IFEO) **[conjetura: probar]**.
5. Rompe la reparación: un `sfc /scannow` "arreglaría" el sistema y dejaría el shell a medias.

---

## 2. Mecanismos de reemplazo y cuándo usar cada uno

### 2.1 Tabla comparativa

| Mecanismo | Qué redirige | Alcance | Privilegios | Argumentos | Sirve para AppX | Sirve para servicios | Sobrevive a updates | Reversible | Señal AV/EDR |
|---|---|---|---|---|---|---|---|---|---|
| `Winlogon\Shell` (HKCU) | El shell del usuario | Por usuario | Usuario | Sí (línea de comando) | Lanza .exe, no AUMID directo | No | Sí (valor de usuario) | Sí (`--disable`) | Bajo |
| `Winlogon\Shell` (HKLM) | El shell de todos | Máquina | Admin | Sí | No | No | Sí, pero las updates pueden reescribirlo | Sí (backup) | Medio |
| `Shell Launcher` | Shell por usuario/grupo (Eshell/CustomShellHost) | Máquina, Ent/Edu/IoT | Admin | Sí | Sí (v2) | No | Sí (CSP/GPO) | Sí | Bajo (soportado) |
| `Userinit` | Programas de inicio de sesión | Máquina | Admin | Lista | No | No | Sí | Sí | Alto (persistencia) |
| IFEO `Debugger` | Cualquier imagen por nombre | Máquina | Admin | El binario recibe la ruta original como argumento | Parcial (por exe host) | Sí, pero no recomendado | Sí (HKLM) | Sí (borrar valor) | **Alto (T1546.012)** |
| `App Paths` | Resolución de `ShellExecute`/`CreateProcess` por nombre | Máquina o usuario | Admin / usuario | Sí | No (alias) | No | Sí | Sí | Bajo |
| COM por usuario (`HKCU\Software\Classes\CLSID`) | Clases del shell y handlers `IExecuteCommand` | Usuario | Usuario | El handler decide | Sí, si la activación es por CLSID | No | Sí | Sí | Medio (COM hijacking) |
| COM por máquina (`HKLM`) | Igual, para procesos elevados | Máquina | Admin | Igual | Sí | No | Sí | Sí | Medio |
| Handler de protocolo (`HKCR\ms-*`) | `ShellExecute("ms-settings:...")` | Máquina o usuario | Admin / usuario | URI completa | Sí (es el mecanismo de las apps) | No | Sí | Sí | Bajo |
| Asociación (`ProgID`, `OpenWithProgids`) | "Abrir con" y doble clic | Usuario / máquina | Usuario / admin | `%1` | Sí (AUMID como ProgID) | No | Sí, salvo `UserChoice` | Sí | Bajo |
| `UserChoice` | Asociación por defecto | Usuario | Usuario, **bloqueado** | n/a | Sí | No | Windows lo resetea si el hash no cuadra | No fiable | **Alto (UCPD)** |
| Shims AppCompat (`.sdb`) | Redirección de ejecutables y fixes | Máquina | Admin | Limitado | No | Parcial | **No** (las updates reinstalan) | Sí (desinstalar shim) | Medio |
| Hardlink/symlink en `System32` | El binario en sí | Máquina | TrustedInstaller | n/a | No | Sí | **No** (WRP/servicing lo revierte) | Difícil | **Muy alto** |
| `Remove-AppxProvisionedPackage` | Quitar apps para usuarios nuevos | Imagen | Admin | n/a | Sí | No | Hasta la próxima feature update | No (hace falta source) | Bajo |
| `Remove-Capability` (DISM) | Quitar FODs (Notepad, Paint, WMP...) | Imagen | Admin | n/a | No (son Win32) | No | Hasta la próxima LCU/feature update | No (hace falta FOD ISO) | Bajo |

### 2.2 `Winlogon\Shell` y `Userinit`

**[doc]** `userinit.exe` ejecuta el valor `Shell` de `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon`
(por defecto `explorer.exe`). Un valor ausente o vacío cae en `explorer.exe`. Se admite una lista separada por comas.
**El fork usa la variante por usuario** (`HKCU\...\Winlogon\Shell`), que Winlogon consulta antes que la de máquina
**[repo: `tools/shell-phase2.ps1`; `plugins/rs-shell/src/main.rs:3-4`]**.

| Variante | Cuándo usarla | Riesgo |
|---|---|---|
| `HKCU` | Un usuario con WinRust; el resto con Explorer | Bajo: un fallo afecta a ese usuario y se repara con otro admin |
| `HKLM` | Kiosco, equipo de un solo uso, multi-sesión | Alto: sin shell no hay escritorio para nadie; exige plan de recuperación |
| `Shell Launcher` | Ent/Edu/IoT con requisito de soporte | Bajo, pero **no disponible en Home/Pro**; no soporta apps que lanzan otro proceso y salen **[doc]** |

`Userinit` no debe tocarse: es el punto donde Windows inicia la sesión (incluye `userinit.exe,`); añadir cosas ahí es
persistencia clásica y lo vigilan los EDR. Para "arrancar WinRust al entrar" ya está el shell o una tarea programada.

### 2.3 IFEO (`Image File Execution Options`)

**[doc]** `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\<imagen>\Debugger` hace que
`CreateProcess` **anteponga** el valor a la línea de comando: `"<debugger>" <ruta original> [argumentos]`. Detalles que
importan al diseño:

- Se aplica a **cualquier** lanzamiento de esa imagen, venga de donde venga (incluido `ShellExecute`).
- La clave que se consulta depende de la **bitness del proceso llamador**: un proceso de 32 bits en WOW64 usa
  `HKLM\SOFTWARE\WOW6432Node\...\Image File Execution Options` **[doc]**. Por eso, para redirigir `notepad.exe`
  lanzado desde un instalador de 32 bits, hay que escribir **las dos** claves.
- `CreateProcess` con `DEBUG_PROCESS`/`DEBUG_ONLY_THIS_PROCESS` **se salta** IFEO (por eso funciona un depurador real).
- El valor no se valida: si el "debugger" no existe, el lanzamiento falla con `ERROR_FILE_NOT_FOUND`.
- `cmd.exe /c exit` como valor **no lanza nada y termina**: es el truco que usa el fork para "matar" hosts.
- En imágenes empaquetadas (Notepad AppX) puede haber entradas `UseFilter`, `FilterFullPath` y
  `AppExecutionAliasRedirect` que complican la redirección **[conjetura]**.
- **Riesgos**: MITRE T1546.012 **[doc]**, cuarentena por AV/EDR, y bloquea también los *hand-offs* propios
  (`explorer.exe` es el caso que el fork ya documenta en `tools/explorer-edge.ps1:1827-1840`).

**Regla de diseño**: IFEO solo en `HKLM`, solo para imágenes *host* del shell o para redirigir un ejecutable a un
programa WinRust equivalente, **nunca** para `explorer.exe`, `rundll32.exe`, `dllhost.exe` ni servicios.
### 2.4 `App Paths`, PATH y `Applications\<exe>`

**[doc]** `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\<exe>` permite que `ShellExecute`/`CreateProcess`
encuentren un ejecutable por nombre aunque no esté en PATH (así se lanza `firefox.exe`, `SnippingTool.exe`...). Es una
**redirección por nombre** con dos límites: solo la usan las rutas de resolución de ShellExecute (los programas que
hacen `CreateProcess("notepad.exe")` buscan en PATH, no en App Paths) y no cubre los *app execution aliases*.

Recetas útiles y su alcance:

| Clave | Efecto | Límite |
|---|---|---|
| `HKLM\...\App Paths\notepad.exe` `(Default)` = `C:\Program Files\WinRust\rust-notepad.exe` | `ShellExecute("notepad.exe")` abre el editor de WinRust | No afecta a quien llame con ruta completa ni a `System32\notepad.exe` |
| `HKCU\Software\Classes\Applications\notepad.exe\shell\open\command` | Cambia el verbo `open` del ProgID `Applications\notepad.exe` (que es el `(Default)` de `.txt` en esta máquina **[hecho]**) | Es la vía **más limpia** para `.txt`: no toca `UserChoice` ni `HKCR\.txt` |
| `HKCR\.txt\OpenWithProgids\WinRust.Text` | Añade WinRust a "Abrir con" | No cambia el predeterminado |
| `HKCR\WinRust.Text\shell\open\command` | ProgID propio de WinRust | Solo se usa si el usuario lo elige o si es el predeterminado |

**[hecho]** En esta máquina `HKCR\.txt` = `Applications\notepad.exe` y su comando es
`"C:\Windows\System32\notepad.exe" "%1"`. Por tanto, **la sustitución del Bloc de notas no requiere tocar `UserChoice`**:
basta redefinir `HKCU\Software\Classes\Applications\notepad.exe\shell\open\command` (y opcionalmente
`App Paths`), y el doble clic en `.txt` abrirá el editor de WinRust.

### 2.5 COM: clases del shell, `DelegateExecute` y activación de apps

**Registro por usuario**: `HKCR` es una vista fusionada; `HKCU\Software\Classes` gana sobre `HKLM\Software\Classes`
**[doc: Merged View of HKEY_CLASSES_ROOT]**. Esto permite reemplazar una clase del shell sin admin.

**Límite crítico**: "un proceso elevado por UAC **no carga clases por usuario**" **[doc: The COM Elevation Moniker]**.
Por eso:
- Un `ShellExperienceHost`/`Settings` a integridad media sí respeta `HKCU`.
- `mmc.exe`, `regedit.exe` o cualquier cosa elevada **no**: para esos hay que usar `HKLM` (admin) o un shim.

**Combinaciones que usa el shell**:

| Pieza | Qué es | Cómo se reemplaza |
|---|---|---|
| `InprocServer32` / `LocalServer32` | Implementación de una clase (`{CLSID}`) | `HKCU\Software\Classes\CLSID\{...}\InprocServer32` (DLL propia) o `LocalServer32` (EXE propio) |
| `DelegateExecute` | En un verbo, el CLSID de un objeto `IExecuteCommand` que sustituye a la línea de comando **[doc]** | Poner `DelegateExecute` en el verbo propio; **quitarlo** para volver a `command` |
| `IApplicationActivationManager::ActivateApplication` | Lanza una app UWP por **AUMID** | No se puede redirigir sin cambiar el AUMID: usar handler propio o `ms-*:` |
| `ShellServiceObjectDelayLoad` | Objetos que el shell carga al arrancar (`WebCheck` en esta máquina **[hecho]**) | Añadir/quitar valores; poco útil hoy |
| `RunOnce` | Arranque único | Útil para provisioning, no para reemplazos |

**Clases del shell que interesan** (nombres estables; el CLSID exacto hay que confirmarlo por build):

| Clase | Implementación típica | Uso |
|---|---|---|
| `ImmersiveShell` | `twinui.pcshell.dll` (`{C2F03A33-21F5-47FA-B4BB-156362A2F239}` **[hecho]**) | Servicios internos del shell (ventanas, posiciones). **No tocar** |
| `StartMenuExperienceHost` | `StartDocked.dll` + `StartMenuExperienceHost.exe` (AppX) | Menú Inicio. Bloquear por IFEO; un `LocalServer32` propio es posible pero frágil **[conjetura]** |
| `ShellExperienceHost` | `ShellExperienceHost.exe` (AppX) + `Windows.UI.ActionCenter.dll` etc. | Flyouts. Bloquear por IFEO (ya hecho) |
| `ShellWindows` | `explorer.exe /factory` | `IShellWindows`: lo usa `rust-explorer` (`shellwin.rs`) y "Mostrar en carpeta". **Mantener Explorer** |
| `FileOperation` | `shell32.dll` | `IFileOperation` (copiar, borrar a papelera). **No tocar shell32** |

**Conclusión de diseño**: COM se usa para (a) handlers `IExecuteCommand` propios que Windows invoca (p. ej. un
`DelegateExecute` en un verbo de WinRust), y (b) reemplazar clases de *flyout* si algún día se quiere interceptar
`ShellExperienceHost` sin IFEO. Para el shell en sí, IFEO + `HKCU` shell son más simples y más reversibles.

### 2.6 Handlers de protocolo/URI (`ms-settings:`, `ms-photos:`, `ms-screenclip:`...)

**[hecho]** Hoy: `HKCR\ms-settings\shell\open\command` tiene `(Default)` vacío y
`DelegateExecute={4ed3a719-cea8-4bd9-910d-e252f997afc2}` (`windows.system.launcher.dll`, "Association Launch Execute
Command"). Ese `DelegateExecute` es el que convierte el URI en una activación de la app Configuración por AUMID.

Redirección por usuario (sin admin, sin tocar `HKLM`):

```
HKCU\Software\Classes\ms-settings\shell\open\command
    (Default) = "C:\Program Files\WinRust\rust-settings.exe" "%1"
    ; NO crear DelegateExecute: si existe, ShellExecute ignora el comando
```

Notas:
- Si se deja `DelegateExecute`, Windows sigue lanzando la app empaquetada aunque el `(Default)` sea otro **[doc]**.
- `ShellExecute` respeta `HKCU\Software\Classes`; las apps UWP que usan `Windows.System.Launcher` acaban en el mismo
  resolver del shell, pero **hay que probarlo** con una app real **[conjetura]**.
- Un proceso elevado puede ignorar la clave por usuario (misma regla que COM) si el handler se resuelve en su contexto.
- `UCPD.sys` protege `http`, `https`, `.pdf`, `.htm(l)`, `.doc(x)`, `.xls(x)`, `.ppt(x)` **[doc: kolbicz]**, **no**
  `ms-settings` ni el resto de `ms-*`.
- Para `ms-screenclip:` (Recortes) y `ms-photos:` el mismo patrón; en esta LTSC **no existen** esas claves **[hecho]**,
  así que crearlas es aditivo (no hay nada que romper).

**Estrategia recomendada: un "router"**. Un único `winrust-uri.exe` registrado en *todos* los `ms-*` que WinRust
cubra; el router decide por prefijo (`ms-settings:display` -> página de `rust-settings`; `ms-photos:` -> `photoview`;
`ms-screenclip:` -> `rs-shot --region`) y, si no lo cubre, **delega en Windows** (restaura el comportamiento original
lanzando la app por AUMID). Así la cobertura crece sin bloquear nada.

### 2.7 Asociaciones de ficheros: `ProgID`, `OpenWithProgids`, `UserChoice`

| Mecanismo | Escribible | Efecto | Veredicto |
|---|---|---|---|
| `HKCR\<ext>` `(Default)` | Admin | Asociación "clásica" | Windows 11 la ignora si hay `UserChoice` |
| `HKCR\<ext>\OpenWithProgids` | Admin / usuario | Añade entradas a "Abrir con" | **Recomendado** |
| `HKCU\...\FileExts\<ext>\OpenWithList` | Usuario | Lista de "Abrir con" | Recomendado |
| `HKCU\...\FileExts\<ext>\UserChoice` | **Bloqueado** | Predeterminado real | **No tocar** (hash + ACL + UCPD) |
| XML `DefaultAssociationsConfiguration` | Admin/GPO | Predeterminados en equipo gestionado | Válido en Enterprise/Education; en Pro/Home no |
| `IApplicationAssociationRegistration` | Programa | Consultar, no fijar | Solo lectura |

**[doc]** `UserChoice` guarda `ProgId` + `Hash`; el hash se calcula con la SID, la extensión, el ProgID y entropía de
la máquina, y desde 2024 `UCPD.sys` bloquea la escritura de las claves protegidas (incluidas las de `http/https`).
`UserChoiceLatest`/`HashVersion` es una migración nueva que **ignora** `UserChoice` en algunas ediciones **[conjetura:
comprobar en la build objetivo]**.

**Diseño**: WinRust no debe ser "el predeterminado" por registro. Debe (a) registrarse en `OpenWithProgids`, (b)
ofrecer su propio diálogo "Abrir con" (ya existe en `rust-explorer`), y (c) en una ISO de empresa, usar el XML de
asociaciones. Para `.txt` concretamente existe el atajo de `Applications\notepad.exe` (§2.4).
### 2.8 Menús contextuales (`IContextMenu`, `IExplorerCommand`, verbos)

| Método | Registro | Cuándo usarlo |
|---|---|---|
| Verbo estático con `command` | `HKCR\<clase>\shell\<verbo>\command` | Acciones simples (abrir con WinRust, "Analizar con rs-du") |
| Verbo estático con `DelegateExecute` | `...\shell\<verbo>` + `DelegateExecute={CLSID}` | Cuando hace falta lógica en COM (`IExecuteCommand`) **[doc]** |
| `IExplorerCommand` (Windows 11) | `...\shell\<verbo>` + `ExplorerCommandHandler={CLSID}` | Menús modernos con estado, iconos y submenús |
| `IContextMenu` (shell extension DLL) | `...\shellex\ContextMenuHandlers\<nombre>` | Solo si hace falta inyectar en el menú de Windows; **evitarlo** (bloquea el menú si la DLL falla) |

El menú propio de `rust-explorer` (`plugins/rust-explorer/src/menu.rs`) ya sustituye el menú de Windows en el
explorador de WinRust, con un "More options..." que cae al de shell32. Para el escritorio y la barra, los verbos
propios deben registrarse por **clase** (`Directory`, `Directory\Background`, `Drive`, `*`, `DesktopBackground`) en
`HKCU\Software\Classes` para no pedir admin.

### 2.9 Hardlinks, symlinks y junctions en `System32`: por qué no

| Problema | Detalle |
|---|---|
| Hardlink a WinSxS | **[hecho]** `fsutil hardlink list C:\Windows\explorer.exe` devuelve la copia del store. Editar el archivo edita el componente |
| WRP / `sfc` | Compara con el manifiesto; restaura el binario original y deja un "repair pending" |
| Servicing (CBS) | Cada LCU/feature update reemplaza el payload; el enlace se pierde y el "reemplazo" desaparece sin avisar |
| Firma y CI | Un binario no firmado por Microsoft en `System32` no carga en procesos con CI/PPL; Defender/EDR lo marca |
| Symlinks/junctions | Requieren privilegio `SeCreateSymbolicLink`; rompen el *path resolution* de instaladores y del propio Windows |
| WinRE / recuperación | Un `System32` manipulado puede impedir arrancar o reparar |
| Anti-cheat | Los anti-cheat comprueban integridad de módulos del sistema; un `notepad.exe` distinto puede expulsar al usuario |

**Alternativa soportada**: shims de AppCompat (§2.10) para ejecutables clásicos; handlers/COM para el shell; y
"no tocar disco" para todo lo demás.

### 2.10 Shims de compatibilidad (`.sdb`, AppCompat)

**[doc]** Un shim se aplica por base de datos (`sysmain.sdb` o un `.sdb` propio instalado con `sdbinst.exe`) y puede
redirigir un ejecutable (`RedirectExe`) o aplicar fixes. Ventajas: es un mecanismo **soportado**, no modifica ficheros y
se puede desinstalar. Límites:

- No cubre apps empaquetadas (AppX/MSIX) ni servicios del sistema.
- `RedirectExe` no pasa argumentos de forma fiable y no sirve para reemplazar un host del shell.
- Las actualizaciones de Windows pueden reinstalar/ignorar shims; la base `sysmain.sdb` se reemplaza con el SO.
- Sigue siendo detectable como "AppCompat injection" por algunos EDR.

**Veredicto**: útil como plan B para `notepad.exe`/`calc.exe` en un equipo concreto; **no** como base de la ISO.

### 2.11 MSIX/AppX: quitar, provisionar y sustituir

| Concepto | Cmdlet | Alcance | Notas |
|---|---|---|---|
| Instalado por usuario | `Remove-AppxPackage` | Perfil actual | Falla si `NonRemovable=True` (`0x80073CFA`) |
| Instalado para todos | `Remove-AppxPackage -AllUsers` | Todos los perfiles | Admin; **no** toca la provisión |
| Provisionado (imagen) | `Remove-AppxProvisionedPackage -Online` (o `/Image`) | Usuarios nuevos | Es lo que hay que hacer en la ISO |
| Paquete del sistema | n/a | n/a | `NonRemovable`; solo se puede "bloquear" (IFEO/COM/política) |
| Dependencias | `Get-AppxPackage` + `Dependencies` | n/a | Quitar una framework rompe a sus dependientes (VCLibs, UI.Xaml, WindowsAppRuntime) |

Reglas prácticas:

1. **Nunca** quitar frameworks: `Microsoft.VCLibs.*`, `Microsoft.UI.Xaml.*`, `Microsoft.WindowsAppRuntime.*`,
   `Microsoft.NET.Native.*`, `MicrosoftWindows.Client.CBS/Core`, `Microsoft.UI.Xaml.CBS`.
2. **Nunca** quitar `Microsoft.WindowsStore` (no soportado) ni `windows.immersivecontrolpanel`, `LockApp`,
   `StartMenuExperienceHost`, `ShellExperienceHost` (paquetes del shell).
3. Se pueden quitar (retail) Photos, Paint, Notepad, Calculator, Camera, Clock, Terminal, Media Player, Sound Recorder,
   Solitaire, Weather, News, Clipchamp, Teams, Copilot, DevHome, Xbox (según edición), **después** de tener sustituto.
4. Al quitar una app que implementa un handler (`ms-photos:`, `ms-paint:`), hay que **registrar el handler de WinRust
   en el mismo paso**, o el sistema se queda con un URI roto.
5. En 24H2+ Enterprise/Education existe la política soportada `RemoveDefaultMicrosoftStorePackages` (lista estática +
   lista dinámica por PFN, desde la no-seguridad de abril de 2026) **[doc]**. Es la vía "de empresa" para que las apps
   no vuelvan al iniciar sesión; en Pro/Home no aplica.
6. `Remove-AppxProvisionedPackage` **no** quita la app de perfiles existentes: hay que hacer las dos cosas (provisioned
   + `-AllUsers`) o provisionar antes de crear el usuario.

### 2.12 `ShellHost.exe` y el shell XAML en 24H2+/26xxx

**[hecho]** En esta build hay un único `ShellHost.exe` que carga `Windows.UI.Xaml.dll`, `Microsoft.UI.Xaml.dll`,
`Windows.UI.Immersive.dll` y `Windows.UI.Xaml.Controls.dll`, y arranca con
`"C:\Windows\System32\ShellHost.exe"` sin argumentos. En 24H2 Microsoft documenta `ShellHost.exe` como componente que
"hospeda" partes del shell junto a `StartMenuExperienceHost` **[doc: KB5072911]**. Implicaciones:

- El IFEO sobre `StartMenuExperienceHost.exe` puede quedarse "sin efecto visible" si en esa build el menú se dibuja
  dentro de `ShellHost` **[conjetura]**; hay que comprobar en la build objetivo qué proceso crea la ventana de Inicio.
- Matar `ShellHost` tumba flyouts/XAML de varias apps; **no** es un objetivo válido de bloqueo.
- `ShellHost` depende de los paquetes XAML (`MicrosoftWindows.Client.CBS`, `Microsoft.UI.Xaml.CBS`,
  `MicrosoftWindows.Client.Core`); si se quitan, el shell entero falla (es el bug de KB5072911). **No tocar.**
- La estrategia correcta sigue siendo: **shell de usuario = WinRust** (Explorer nunca arranca) + bloquear los hosts
  que sí se lanzan por nombre, sin intentar "desmantelar" el runtime XAML.

---
## 3. Inventario y cobertura

La tabla maestra pedida se parte en tres para que sea legible (mismas columnas): **shell/UX**, **ajustes y
herramientas**, **apps/servicios/runtime**. "Acción" usa el vocabulario del §2: dejar, IFEO, COM, handler, App Paths,
asociación, quitar AppX, quitar capability.

### 3.1 Tabla maestra (1/3): shell y experiencia de usuario

| Componente | Binario/ruta | Clase/AUMID | Estado en WinRust | Acción propuesta | Riesgo |
|---|---|---|---|---|---|
| Escritorio (iconos, fondo, menú) | `C:\Windows\explorer.exe` (hardlink WinSxS) | `Progman`, `WorkerW`, `SHELLDLL_DefView` | Hecho: `rust-explorer --desktop` | Dejar explorer en disco; shell por usuario; **no** IFEO | Bajo |
| Barra de tareas | `explorer.exe` | `Shell_TrayWnd`, `Shell_SecondaryTrayWnd` | Hecho: `topbar` (multi-monitor) | Dejar | Bajo |
| Bandeja del sistema | `explorer.exe` + `Shell_NotifyIcon` | `Shell_TrayWnd` (host propio de `topbar`) | Hecho (host propio verificado) | Dejar; vigilar orden/topmost frente a Explorer | Bajo |
| Reloj y calendario | `explorer.exe` + `ShellExperienceHost` (`ClockFlyoutExperience.dll`) | n/a | Parcial: reloj + flyout `Cal` de `topbar` (sin cuentas/eventos) | Completar integración de calendario (opcional) | Bajo |
| Menú Inicio | `StartMenuExperienceHost.exe` (AppX) | PFN `Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy` (`App`/`FullTrustApp`) | Hecho: `start7/10/11` | Mantener IFEO; verificar `ShellHost` por build | Medio |
| Búsqueda (UI y servicio) | `SearchHost.exe`, `SearchApp.exe`, `SearchUI.exe`, `WSearch` | PFN `Microsoft.Windows.Search` (retail) | Hecho: `rust-search` + Alt+Space; Win+S a Windows | Cablear Win+S a `rust-search`; mantener WSearch deshabilitado | Medio |
| Action Center / quick settings / flyouts | `ShellExperienceHost.exe` + `Windows.UI.ActionCenter/QuickActions/Clock/Battery/ImeStatus/SharePicker` | PFN `Microsoft.Windows.ShellExperienceHost_cw5n1h2txyewy` | Parcial: `topbar` (quick settings, Wi-Fi, volumen, batería); faltan share, nearby, IME, teclado táctil | Mantener IFEO; desarrollar Share/Cast si se usan | Medio |
| Notificaciones (toasts) | `WpnService` + `ShellExperienceHost` + `explorer.exe` | AUMID de la app; `UserNotificationListener` | Hecho fase 1 (`rs-notify`, toasts, campana, historial); puente nativo | Completar acciones/respuesta de toasts | Medio |
| Centro de notificaciones | `explorer.exe` + `ShellExperienceHost` | n/a | Hecho: panel de 50 entradas en `topbar` | Persistir historial (pendiente) | Bajo |
| Portapapeles (Win+V) | `cbdhsvc` + UI del shell | n/a | **No** | Desarrollar: `AddClipboardFormatListener` + almacén + hotkey | Medio |
| Panel emoji/kaomoji (Win+.) | `TextInputHost.exe` (paquete `InpApp`) | PFN `MicrosoftWindows.61869836.InpApp_cw5n1h2txyewy` | **No** | Desarrollar picker propio o dejar `TextInputHost` | Medio |
| Alt+Tab | `explorer.exe` (AltTab) | n/a | Hecho: `topbar` (`alt_tab`, por monitor) | Dejar | Bajo |
| Task View / escritorios virtuales | `explorer.exe` + `IVirtualDesktopManager` | n/a | Parcial: botón + Win+Ctrl+1..3; sin crear/cerrar escritorios | Completar (API documentada + conjetura para el resto) | Bajo |
| Snap layouts (Win+Z) / Snap Assist | `explorer.exe` / `ShellHost` | n/a | No (solo snap por bordes en `topbar`) | Opcional | Bajo |
| Win+D / Win+E | `explorer.exe` | n/a | Hecho: `rs-shell` | Dejar | Bajo |
| Win+R (Ejecutar) | `explorer.exe` (`RunFileDlg` de shell32) | n/a | Hecho: `rs-launcher` (modo Run, historial) | Verificar que Win+R está cableado sin Explorer | Bajo |
| Win+I (Configuración) | `SystemSettings` (AppX) | AUMID de Configuración | Hecho: `reshell`/`topbar` abren `ms-settings:` | Router `ms-settings:` (§4.1) | Medio |
| Win+X (menú de sistema) | `explorer.exe` | n/a | Hecho: menú de `topbar` | Dejar | Bajo |
| Win+Space (idioma) | `explorer.exe` | n/a | Hecho: `topbar`/`rs-shell` | Dejar | Bajo |
| Win+Shift+S (recorte) | `SnippingTool.exe` | `ms-screenclip:` | Hecho por hook (`rs-shot`); **handler no registrado** | Handler `ms-screenclip:` -> `rs-shot --region` | Medio |
| Win+Alt+R (grabación) | Xbox Game Bar | n/a | Hecho: `rs-shot --record` | Dejar | Bajo |
| Win+L (bloqueo) | `LogonUI.exe` + `LockApp` | PFN `Microsoft.LockApp_cw5n1h2txyewy` | Windows (no se toca) | No tocar | Alto |
| Pantalla de bloqueo / inicio de sesión | `LogonUI.exe`, `LockApp`, `CloudExperienceHost` | n/a | Windows | No tocar (credential provider propio = proyecto aparte) | Alto |
| UAC | `consent.exe` (secure desktop) | n/a | Windows | No tocar | Alto |
| OOBE | `CloudExperienceHost`, paquete OOBE, `msoobe` | n/a | Windows | No tocar; automatizar con `unattend.xml` | Alto |
| Widgets | `Widgets.exe` + `WidgetService` + Web Experience Pack | n/a | Bloqueado (IFEO + servicio); ausente en LTSC | Dejar bloqueado / quitar AppX en retail | Bajo |
| Copilot | `Copilot.exe` / AppX `Microsoft.Copilot` | n/a | Ausente; `debloat` aplica políticas | Quitar AppX + política (retail) | Bajo |
| `shell:AppsFolder` / "Todas las apps" | shell32 + paquetes | n/a | Hecho parcial (el Inicio enumera accesos y AppX) | Dejar | Bajo |
| Diálogos Abrir/Guardar | `comdlg32.dll` / `IFileDialog` (shell32) | n/a | Windows (no dependen de Explorer) | Dejar | Bajo |
| "Abrir con" | `OpenWith.exe` + shell32 | n/a | Hecho propio en `rust-explorer` | Registrar ProgIDs propios; no pisar `UserChoice` | Bajo |
| Menú contextual | shell32 (`IContextMenu`) + `HKCR\*\shell` | n/a | Hecho propio + fallback | Dejar | Bajo |
| Propiedades de archivo/carpeta | shell32 (property sheet) | n/a | Windows | Dejar | Bajo |
| Papelera | shell32 + `$Recycle.Bin` | CLSID `{645FF040-5081-101B-9F08-00AA002F954E}` | Hecho (`IFileOperation`) | Dejar | Bajo |
| Este equipo / unidades / red | `windows.storage.dll`, `NetworkExplorer.dll` | CLSID `{20D04FE0-...}`, `{F02C1A0D-...}` | Hecho (`rust-explorer`) | Dejar | Bajo |
| Jump lists / recientes | `explorer.exe` + `AutomaticDestinations` | n/a | No | Opcional | Bajo |
| Compartir (share sheet) | `SharePickerUI.dll` (ShellExperienceHost) | n/a | No | Opcional / dejar | Bajo |
| Proyectar (Win+P) / Conectar (Win+K) | `DisplaySwitch.exe` / QuickActions (Cast) | n/a | No | Win+P -> `DisplaySwitch.exe`; Win+K opcional | Bajo |
| Teclado táctil | `TabTip.exe` / `TextInputHost.exe` | PFN `InpApp` | No | Dejar (necesario en táctil) | Medio |
| Accesibilidad (Lupa, Narrador, OSK) | `magnify.exe`, `narrator.exe`, `osk.exe` | n/a | No (páginas en `rust-control`) | Dejar | Bajo |
| Fondo de pantalla | `SystemParametersInfo` + `rust-explorer` | n/a | Hecho (multi-monitor) | Dejar | Bajo |
| "Mostrar escritorio" | `topbar` (zona junto al reloj) | n/a | Hecho | Dejar | Bajo |
### 3.2 Tabla maestra (2/3): ajustes y herramientas

| Componente | Binario/ruta | Clase/AUMID | Estado en WinRust | Acción propuesta | Riesgo |
|---|---|---|---|---|---|
| Configuración (Win+I) | `C:\Windows\ImmersiveControlPanel\SystemSettings.exe` | `windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel` | Parcial: `rust-settings` (~10 páginas) + helper que arranca Explorer si hace falta | **No** IFEO todavía; router `ms-settings:` con fallback | Alto |
| Handler `ms-settings:` | `windows.system.launcher.dll` | `DelegateExecute={4ed3a719-...}` | Parcial (páginas cubiertas por `rust-settings`) | `HKCU\Software\Classes\ms-settings\shell\open\command` -> router | Medio |
| Panel de control clásico | `C:\Windows\System32\control.exe` | n/a | Parcial: `rust-control` replica la vista por categorías | Dejar `control.exe`; abrir applets no cubiertos | Medio |
| Applets `.cpl` (17 en esta máquina) | `System32\*.cpl` (`desk`, `sysdm`, `appwiz`, `ncpa`, `powercfg`, `timedate`, `mmsys`, `intl`, `Firewall`, `hdwwiz`, `bthprops`, `joy`, `main`, `telephon`, `TabletPC`, `inetcpl`, `irprops`) | n/a | Parcial: páginas propias para pantalla/sonido/red/energía/fecha | Dejar; reimplementar solo los que se usen a diario | Bajo |
| Administración de equipos | `compmgmt.msc` (`mmc.exe`) | n/a | Enlace desde `rust-control` | Dejar | Bajo |
| Servicios | `services.msc` | n/a | Parcial: página de `rust-control` | Reimplementar con SCM (`OpenSCManager`, `StartService`) | Bajo |
| Administrador de dispositivos | `devmgmt.msc` | n/a | Parcial: lista de `rust-control` | Reimplementar (CfgMgr32/SetupAPI) | Medio |
| Administración de discos | `diskmgmt.msc` | n/a | Parcial: página de `rust-control` | Reimplementar (VDS o IOCTL) | Medio |
| Visor de eventos | `eventvwr.msc` | n/a | Enlace | Reimplementar con `wevtapi` (consulta + detalle) | Medio |
| Programador de tareas | `taskschd.msc` | n/a | No | Opcional (Task Scheduler COM) | Bajo |
| Editor del Registro | `C:\Windows\regedit.exe` | n/a | No | Opcional; si se hace, editor de solo lectura + import/export | Medio |
| Configuración del sistema | `msconfig.exe` | n/a | No | Dejar (arranque seguro, servicios) | Bajo |
| Monitor de recursos | `resmon.exe` | n/a | No | Opcional (ya hay Task Manager) | Bajo |
| Monitor de rendimiento | `perfmon.exe` | n/a | No | Dejar | Bajo |
| Liberador de espacio | `cleanmgr.exe` | n/a | Hecho: `rs-du` | Dejar | Bajo |
| Diagnóstico de DirectX | `dxdiag.exe` | n/a | No | Opcional (página en `rust-control`) | Bajo |
| Información del sistema | `msinfo32.exe` | n/a | Parcial (About) | Opcional | Bajo |
| Mapa de caracteres | `charmap.exe` | n/a | No | Dejar (o integrar en el picker de emoji) | Bajo |
| Teclado en pantalla | `osk.exe` | n/a | No | Dejar | Bajo |
| Lupa | `magnify.exe` | n/a | No (accesibilidad en `rust-control`) | Dejar | Bajo |
| Narrador | `narrator.exe` | n/a | No | Dejar | Bajo |
| Recortes | `SnippingTool.exe` (Win32 en LTSC; AppX `Microsoft.ScreenSketch` en retail) | `ms-screenclip:` | Hecho parcial: `rs-shot` por hotkey | Handler `ms-screenclip:` + `App Paths` | Medio |
| Escritorio remoto | `mstsc.exe` | n/a | No | Dejar | Bajo |
| Bloc de notas | `C:\Windows\System32\notepad.exe` (capability en LTSC; AppX en retail) | `Microsoft.Windows.Notepad~~~~0.0.1.0` | **No** | Desarrollar `rust-notepad` + `Applications\notepad.exe` + `App Paths` | Medio |
| Calculadora | `calc.exe` (stub de la AppX) | `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App` (retail) | Hecho: `rs-calc` (estándar, científica, memoria, historial, CLI) | IFEO de `calc.exe` -> `rs-calc.exe` (ver `plugins/rs-calc/README.md`); el instalador no lo aplica | Medio |
| Paint | `mspaint.exe` (capability en LTSC; AppX `Microsoft.Paint` en retail) | `ms-paint:` (retail) | **No** | Desarrollar `rust-paint` o mantener; handler `ms-paint:` | Medio |
| Fotos | AppX `Microsoft.Windows.Photos` (retail); en LTSC no hay visor moderno | `ms-photos:` | Parcial: `photoview` | Registrar `ms-photos:` -> `photoview`; `OpenWithProgids` de imágenes | Bajo |
| Reproductor multimedia | `wmplayer.exe` (capability `Media.WindowsMediaPlayer`) / AppX Media Player | n/a | **No** | Opcional (`rust-media`) o dejar | Bajo |
| Cámara | AppX `Microsoft.WindowsCamera` | `ms-camera:` | No | Dejar o quitar (retail) | Bajo |
| Reloj / alarmas | AppX `Microsoft.WindowsAlarms` | `ms-clock:` | Parcial (hora en `topbar`; sin alarmas/temporizadores) | Opcional | Bajo |
| Microsoft Store | `Microsoft.WindowsStore` (AppX) | `ms-windows-store:` | No | **Dejar** (no soportado quitarlo; hace falta para apps) | Alto |
| Terminal | AppX `Microsoft.WindowsTerminal` + alias `wt.exe` | n/a | No | Dejar o quitar; `conhost` sigue | Bajo |
| Windows Update | `UsoSvc`, `WaaSMedicSvc`, `TrustedInstaller`, página de Configuración | `ms-settings:windowsupdate` | Parcial (página WU en `rust-settings` vía WUA) | No tocar servicios; dejar la página de Windows como fallback | Alto |
| UAC | `consent.exe` | n/a | Windows | No tocar | Alto |
| Pantalla de bloqueo / login | `LogonUI.exe`, `LockApp` | n/a | Windows | No tocar | Alto |
| OOBE | `CloudExperienceHost`, OOBE AppX | n/a | Windows | `unattend.xml`, no borrar | Alto |
| Apps y características | `ms-settings:appsfeatures` | n/a | Parcial (página Apps de `rust-settings`) | Router + fallback | Medio |
| Impresoras | `ms-settings:printers`, `control printers` | n/a | No | Dejar (spooler + UI de Windows) | Bajo |
| Bluetooth | `ms-settings:bluetooth` | n/a | Hecho (página BT de `rust-settings`) | Dejar | Bajo |
### 3.3 Tabla maestra (3/3): binarios, servicios y runtime del que dependen los demás

| Componente | Binario/ruta | Clase/AUMID | Estado en WinRust | Acción propuesta | Riesgo |
|---|---|---|---|---|---|
| Explorer | `C:\Windows\explorer.exe` (hardlink) | `ShellWindows` (`/factory`), `Progman` | No arranca como shell; se usa para hand-offs | **Dejar en disco y ejecutable**; no IFEO | Alto si se quita |
| shell32 | `System32\shell32.dll` | `FileOperation`, `ShellFolder`, `IContextMenu` | En uso (`IFileOperation`, diálogos, papelera) | No tocar | Muy alto |
| `twinui.pcshell.dll` / ImmersiveShell | `System32\twinui.pcshell.dll` | `{C2F03A33-...}` | En uso por el shell de Windows | No tocar | Muy alto |
| comctl32 / DWM | `comctl32.dll`, `dwm.exe` | n/a | En uso por WinRust (controles, composición) | No tocar | Muy alto |
| `Windows.UI.Xaml` + CBS/Core | `System32\Windows.UI.Xaml.dll`; paquetes `MicrosoftWindows.Client.CBS/Core`, `Microsoft.UI.Xaml.CBS` | n/a | En uso por `ShellHost`, Settings, LockApp, InpApp | No tocar / no quitar | Muy alto |
| ShellHost | `System32\ShellHost.exe` | n/a | Corriendo (hospeda XAML del shell) | No tocar | Alto |
| sihost | `System32\sihost.exe` | n/a | Corriendo (Shell Infrastructure Host) | No tocar | Alto |
| RuntimeBroker | `System32\RuntimeBroker.exe` | n/a | Corriendo (broker de UWP) | No tocar | Alto |
| TextInputHost | `SystemApps\SxS\...InpApp...\TextInputHost.exe` | PFN `InpApp` | Corriendo (IME/táctil/emoji) | Dejar (o reemplazar solo si hay picker propio) | Medio |
| ApplicationFrameHost | `System32\ApplicationFrameHost.exe` | n/a | Corriendo (host de ventanas UWP) | No tocar | Alto |
| WpnService | `svchost` (`WpnService`) | n/a | Corriendo | Mantener (notificaciones de apps) | Medio |
| ClipSVC | `svchost` | n/a | Manual/parado | Mantener (licencias/Store) | Alto |
| AppXSvc / StateRepository | `svchost`, `StateRepository` | n/a | Corriendo | Mantener (despliegue MSIX) | Alto |
| WaaSMedicSvc | `svchost` | n/a | Manual | Mantener (reparación de WU) | Alto |
| DcomLaunch / RPCSS | `svchost` | n/a | Corriendo | No tocar | Muy alto |
| WSearch | `SearchIndexer.exe` | n/a | Deshabilitado | Mantener deshabilitado si no se usa Outlook/búsqueda indexada | Medio |
| cbdhsvc (portapapeles) | `svchost` | n/a | Automático | Mantener (necesario para Win+V futuro) | Bajo |
| UCPD | `System32\drivers\UCPD.sys` | n/a | Corriendo | No tocar | Alto (lo vigila Defender) |
| Defender / BFE / mpssvc | `MsMpEng.exe`, `BFE`, `mpssvc` | n/a | Corriendo | No tocar | Muy alto |
| BitLocker | `BDESVC` | n/a | Según equipo | No tocar | Muy alto |
| ShellHWDetection | `svchost` | n/a | Corriendo | Mantener (autoplay, USB) | Medio |
| Themes / Audiosrv | `svchost` | n/a | Corriendo | Mantener | Alto |
| Frameworks AppX | `Microsoft.VCLibs.*`, `Microsoft.UI.Xaml.*`, `WindowsAppRuntime.*`, `NET.Native.*` | n/a | Instalados | **Nunca quitar** | Muy alto |
| WinRE | `Winre.wim`, `reagentc` | n/a | Habilitado | No tocar; probar recuperación | Alto |
| Activación | `ClipSVC`, `LicenseManager`, `slsvc` | n/a | Corriendo | No tocar | Alto |
| Anti-cheat / DRM | servicios de terceros | n/a | n/a | No tocar; probar juegos | Alto |

### 3.4 Mapa de atajos Win+*

| Atajo | Quién lo implementa en Windows | Estado en WinRust | Acción |
|---|---|---|---|
| Win | Explorer (menú Inicio) | `topbar` (botón) | Dejar |
| Win+S | SearchHost | Windows (limitación conocida en `ROADMAP.md`) | Cablear a `rust-search` |
| Win+E | explorer | `rs-shell` -> `rust-explorer` | Dejar |
| Win+D | explorer | `rs-shell` (`winrust-common`) | Dejar |
| Win+R | explorer / shell32 | `rs-launcher` (Run) | Verificar |
| Win+I | explorer -> `ms-settings:` | `topbar`/`reshell` | Router |
| Win+X | explorer | `topbar` | Dejar |
| Win+Space | explorer | `topbar`/`rs-shell` | Dejar |
| Win+Shift+S | SnippingTool / `ms-screenclip:` | `rs-shot` (hook) | Handler |
| Win+Alt+R | Game Bar | `rs-shot --record` | Dejar |
| Win+V | cbdhsvc + shell | **No** | Desarrollar |
| Win+. | TextInputHost | **No** | Desarrollar |
| Win+Tab | explorer (Task View) | Parcial (`topbar` Task View) | Completar |
| Win+Ctrl+D / F4 / 1..3 | explorer (escritorios virtuales) | Parcial (1..3) | Completar |
| Win+Z | explorer (snap layouts) | No | Opcional |
| Win+A | Action Center | `topbar` quick settings | Dejar |
| Win+N | Centro de notificaciones | `topbar` campana | Dejar |
| Win+P | `DisplaySwitch.exe` | No | Cablear a `DisplaySwitch.exe` |
| Win+K | Cast UI | No | Opcional |
| Win+L | Winlogon | Windows | No tocar |
| Win+1..9 | explorer (barra) | `topbar` | Dejar |

### 3.5 Páginas `ms-settings:` y estrategia de router

`rust-settings` reconoce hoy (tests en `plugins/rust-settings/src/main.rs:2161-2192`): `ms-settings:display`,
`sound`, `personalization`/`colors`/`themes`, `powersleep`/`storagesense`, `network-status`, `about`, y los alias
`more`. Todo lo demás va a la Configuración de Windows (que necesita Explorer como shell; el helper ya lo arranca).

| Grupo de páginas | Cubierto | Acción |
|---|---|---|
| Pantalla, sonido, energía, almacenamiento, red, personalización, about, Bluetooth, apps, Windows Update | Sí | Router -> `rust-settings` |
| Cuentas, privacidad, idioma/región, fecha/hora, accesibilidad, juegos, actualización de apps | Parcial (algunas abren `rust-control` o `ms-settings:`) | Router con fallback |
| Redes avanzadas (VPN, proxy, 802.1X), impresoras, dispositivos, opciones de carpeta, valores predeterminados | No | Fallback a Windows |
| "Administrar Bluetooth device", "Añadir impresora", "Abrir ubicación de la app" | No | Fallback (dependen de Explorer/XAML) |
## 4. Recetas concretas

Todo lo de esta sección es **registro por usuario** salvo donde se indique `HKLM`. Cada receta debe ir acompañada de
una entrada en el journal (valor anterior) para el `-Restore`.

### 4.1 Router de `ms-settings:`

1. Crear `winrust-uri.exe` (nuevo, ~200 líneas) con una tabla `prefijo -> programa/página`.
2. Registrar por usuario **solo** los prefijos cubiertos; el resto se queda con Windows:

```
Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\Software\Classes\ms-settings\shell\open\command]
@="\"C:\Program Files\WinRust\winrust-uri.exe\" \"%1\""

; Importante: NO crear DelegateExecute en esta clave.
```

3. Si el router no entiende el URI, debe lanzar la app de Windows por AUMID
   (`IApplicationActivationManager::ActivateApplication` con
   `windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel`).
4. **No** tocar `HKLM\Software\Classes\ms-settings` hasta que la cobertura sea 100%: es el fallback de todos los
   usuarios y de los procesos elevados.

**Por qué funciona**: el `(Default)` de `ms-settings` está vacío y el `DelegateExecute` de Windows es lo que hace la
activación por AUMID; al definir un comando y no poner `DelegateExecute`, `ShellExecute` ejecuta el comando.

### 4.2 `ms-photos:`, `ms-paint:`, `ms-screenclip:` y `ms-camera:`

Mismo patrón, con un `ProgID`/comando propio y sin `DelegateExecute`:

```
[HKEY_CURRENT_USER\Software\Classes\ms-photos\shell\open\command]
@="\"C:\Program Files\WinRust\photoview.exe\" \"%1\""

[HKEY_CURRENT_USER\Software\Classes\ms-screenclip\shell\open\command]
@="\"C:\Program Files\WinRust\rs-shot.exe\" --region"

[HKEY_CURRENT_USER\Software\Classes\ms-paint\shell\open\command]
@="\"C:\Program Files\WinRust\rust-paint.exe\" \"%1\""
```

- En esta LTSC `ms-photos`/`ms-paint`/`ms-screenclip` **no existen** en `HKCR` **[hecho]**: crearlas es aditivo.
- En retail, si se quita la AppX de Fotos/Paint, **hay que** crear el handler en el mismo paso; si no, el URI queda roto.
- `ms-screenclip:` se usa desde el Centro de accesibilidad, la Configuración y apps de terceros; hoy `rs-shot` solo
  escucha el hotkey, así que este handler es lo que cierra el círculo.

### 4.3 Bloc de notas: `.txt` sin tocar `UserChoice`

**[hecho]** `.txt` -> `Applications\notepad.exe` -> `"C:\Windows\System32\notepad.exe" "%1"`. Receta mínima:

```
[HKEY_CURRENT_USER\Software\Classes\Applications\notepad.exe\shell\open\command]
@="\"C:\Program Files\WinRust\rust-notepad.exe\" \"%1\""

[HKEY_CURRENT_USER\Software\Classes\Applications\notepad.exe\shell\edit\command]
@="\"C:\Program Files\WinRust\rust-notepad.exe\" \"%1\""
```

Complementos recomendados:

| Clave | Para qué |
|---|---|
| `HKCU\Software\Classes\WinRust.Text` + `shell\open\command` | ProgID propio (icono, descripción) |
| `HKCR\.txt\OpenWithProgids` (o `HKCU\Software\Classes\.txt\OpenWithProgids`) | "Abrir con" incluye WinRust |
| `HKLM\...\App Paths\notepad.exe` | `ShellExecute("notepad.exe")` de terceros |
| IFEO `notepad.exe` (las dos bitness) | Último recurso: también captura `CreateProcess` con ruta completa |

**Cuidado con el alias**: en retail `notepad.exe` puede ser un *app execution alias*; si el usuario lo ejecuta desde
`%LOCALAPPDATA%\Microsoft\WindowsApps`, IFEO necesita `AppExecutionAliasRedirect` **[conjetura]**. En LTSC no hay
alias de Notepad (es Win32) **[hecho]**.

### 4.4 `control.exe` y applets

- **Dejar** `control.exe`: es el lanzador de los `.cpl` y muchas páginas de `rust-control` abren applets que solo
  existen ahí (`desk.cpl`, `sysdm.cpl`, `appwiz.cpl`, `ncpa.cpl`, `powercfg.cpl`, `timedate.cpl`, `mmsys.cpl`).
- Para los applets que WinRust cubra, el enlace de `rust-control` debe ir a la página propia, no a `control.exe`.
- Si se quiere interceptar `control.exe` (por ejemplo, para abrir `rust-control` en vez del panel clásico), **no** se
  debe hacer con IFEO global: rompe `rundll32 shell32.dll,Control_RunDLL` y los accesos directos de las herramientas
  administrativas. Mejor un verbo/`App Paths` y dejar el binario.

### 4.5 Configuración: por qué **no** IFEO todavía

`SystemSettings.exe` es el destino de **cientos** de flujos del sistema: "Administrar dispositivos Bluetooth",
"Añadir impresora", "Valores predeterminados", "Abrir ubicación de la app", páginas de privacidad, cuenta, etc.
Bloquearlo antes de tener cobertura completa deja al usuario sin salida y rompe a terceros que abren
`ms-settings:` específicos. Plan:

1. Router con fallback (4.1).
2. Medir qué páginas se usan (telemetría local del router: prefijo + contador).
3. Cuando una página tenga sustituto, añadirla a la tabla del router.
4. Solo al final, y solo en una edición gestionada, plantear IFEO (o COM) para `SystemSettings.exe`.

### 4.6 Inicio, búsqueda y `ShellExperienceHost`

El fork ya los bloquea por IFEO (`tools/shell-phase2.ps1:68-78`). Dos mejoras:

| Mejora | Cómo | Por qué |
|---|---|---|
| Comprobar qué proceso dibuja el Inicio en la build | Spy/Process Explorer sobre la ventana de Inicio en una VM con Explorer | En 26xxx puede estar en `ShellHost` **[conjetura]** |
| Añadir IFEO a `SearchProtocolHost.exe`/`SearchFilterHost.exe` | Solo si `WSearch` se queda deshabilitado | Evita que una app que llama al indexador lo despierte |
| No bloquear `ShellHost.exe` | n/a | Tumba XAML de todo el sistema |
| Vigilar `SearchHost.exe` en retail | IFEO ya existe; en retail el paquete se llama `Microsoft.Windows.Search` | Consistencia |

### 4.7 Explorer: hand-offs que hay que conservar

| Hand-off | Quién lo usa | Qué pasa si se bloquea Explorer |
|---|---|---|
| `ShellExecute("https://...")` / `mailto:` | Navegadores, apps | Sin navegador predeterminado falla; con Firefox funciona **[hecho]** |
| `ShellExecute("ms-settings:...")` | WinRust, apps | El helper de Settings ya no puede arrancar Explorer |
| `explorer.exe /select,<ruta>` | `rtaskmgr`, menús, terceros | "Abrir ubicación" deja de funcionar |
| `SHOpenFolderAndSelectItems` | Navegadores, chat, descargas | Necesita una ventana `IShellWindows` (la de `rust-explorer`) o Explorer |
| `IFileOperation` | `rust-explorer` | Es shell32, no Explorer: sigue funcionando |
| `IApplicationActivationManager` | Lanzar apps UWP | No depende de Explorer, pero algunas apps esperan el shell |

**Conclusión**: el fork acierta al no bloquear `explorer.exe`. La ISO debe mantenerlo y, si se quiere "que no
aparezca", hacerlo por `Winlogon\Shell` (por usuario) y por las políticas de `explorer-edge.ps1`.

### 4.8 Win+R / Ejecutar

`rs-launcher` ya implementa un Run con historial y autocompletado (`ROADMAP.md`, v0.5.3). Lo que falta comprobar:

- Que el hotkey Win+R esté registrado por `rs-shell`/`topbar` (no por Explorer).
- Que `rs-launcher` resuelva `shell:...`, `ms-settings:...`, rutas y comandos de PATH/App Paths.
- Que el fallback (caja de Windows) no se dispare por error cuando Explorer no está.
### 4.9 Bandeja: lo que ya funciona y lo que hay que vigilar

`topbar` registra una clase `Shell_TrayWnd` propia (`plugins/topbar/src/main.rs:3584-3587`) y trata los
`WM_COPYDATA` de `Shell_NotifyIcon` (`tray.rs:35-40`). Con Explorer ausente, las apps encuentran esa ventana primero y
sus iconos aparecen. Puntos delicados:

- **Orden Z**: Explorer puede recrear su `Shell_TrayWnd` y quedar por delante; el código ya lo re-eleva
  (`main.rs:1538`). Si en el futuro se permite Explorer para Settings, hay que reemitir `TaskbarCreated`.
- **Iconos por GUID** (`NOTIFYICON_VERSION_4`): soportados en el parser (`TIcon.guid`).
- **Restricción**: si una app usa `Shell_NotifyIconGetRect` o espera la ventana de Explorer para menús contextuales,
  puede comportarse distinto **[conjetura]**.
- **No** crear un `Shell_TrayWnd` desde dos procesos: solo el líder de `topbar`.

### 4.10 Portapapeles (Win+V) y emoji (Win+.): diseño mínimo

**Portapapeles**

1. `AddClipboardFormatListener(hwnd)` en `topbar` (o en un helper).
2. En `WM_CLIPBOARDUPDATE`, `OpenClipboard` + `GetClipboardData(CF_UNICODETEXT/CF_DIB)` y guardar en un almacén
   circular en memoria (y opcionalmente en `%LOCALAPPDATA%\WinRust\clipboard`).
3. `RegisterHotKey(MOD_WIN, 'V')` -> panel propio (lista, búsqueda, fijar, borrar).
4. Pegar: `SetClipboardData` + `SendInput(Ctrl+V)` a la ventana en primer plano.
5. Límites: no se pueden capturar formatos propietarios (dibujos complejos, HTML con imágenes) sin `OLE`/`IFileDataObject`;
   el historial de Windows (`cbdhsvc`) no se reutiliza. **No** deshabilitar `cbdhsvc`: es lo que hace funcionar el
   historial de Windows si el usuario vuelve a Explorer.

**Emoji**

1. Panel propio con `Segoe UI Emoji` (glifos por rango Unicode) y categorías.
2. Insertar con `SendInput`/`EM_REPLACESEL` o portapapeles + Ctrl+V (más compatible).
3. Alternativa: dejar `TextInputHost` (Win+.) y no implementar nada; funciona sin Explorer si el paquete `InpApp`
   arranca por hotkey **[conjetura]**.
4. Si se implementa, **no** registrar Win+. si `TextInputHost` sigue activo: colisión de hotkeys.

### 4.11 Alt+Tab, Task View y snap

| Función | Estado | Qué falta |
|---|---|---|
| Alt+Tab | `topbar::alt_tab` (por monitor, `main.rs:681`) | Agrupar por app, cerrar con rueda, previews en vivo |
| Task View | Botón + rejilla de ventanas; Win+Tab delega en Windows si Explorer está | Crear/cerrar escritorios virtuales (`IVirtualDesktopManager` + API privada) |
| Snap por arrastre | Hecho en `topbar` | Snap Assist (sugerencias), Win+Z (snap layouts) |
| Escritorios virtuales | Win+Ctrl+1..3 | Nombres, fondo distinto, mover ventanas |

**Nota**: las APIs de escritorios virtuales (`IVirtualDesktopManagerInternal`) son **no documentadas** y cambian entre
builds; si se usan, aislarlas en un módulo con fallback.

### 4.12 UAC, bloqueo, login y OOBE: lo que **no** se toca

| Componente | Por qué no | Qué hacer en su lugar |
|---|---|---|
| `consent.exe` (UAC) | Secure desktop, firma, políticas; sustituirlo es un riesgo de seguridad y de cumplimiento | Dejar; WinRust ya usa `ShellExecute("runas")` |
| `LogonUI` / `LockApp` | Arranque de sesión y bloqueo; un fallo deja el equipo inaccesible | Dejar; personalizar solo con credenciales soportadas (Credential Provider) si algún día hace falta |
| `userinit.exe` | Punto de entrada de la sesión | Dejar |
| OOBE | Requerido para instalar y para `sysprep` | Automatizar con `unattend.xml`, no borrar |

---
## 5. La ISO: qué se puede quitar de verdad

### 5.1 Dos modelos y por qué se elige "clean + provisioning"

| | ISO "borrada" (editar `install.wim`) | ISO "clean + provisioning" (recomendada) |
|---|---|---|
| Qué se hace | Quitar paquetes, capabilities y **ficheros** de `System32`/`WinSxS` | Quitar solo paquetes/capabilities soportados; el resto, registro post-instalación |
| Servicing | Roto o en riesgo: las LCU esperan componentes que ya no están | Intacto |
| `sfc` / `DISM /RestoreHealth` | Fallan o "reparan" lo borrado | Funcionan |
| Actualizaciones | Pueden fallar o reinstalar lo quitado | Se aplican; el provisioning se re-aplica |
| Recuperación (WinRE) | Un `System32` incompleto puede impedir reparar | WinRE intacto |
| Ahorro de disco | Alto (GB) | Modesto (cientos de MB) |
| Reversibilidad | Casi nula | Total (journal + `-Restore`) |
| Riesgo de romper apps | Alto (dependencias ocultas) | Bajo/medio |
| Esfuerzo | Alto y frágil | Medio y mantenible |

**Decisión**: ISO = Windows completo con `install.wim` recortado **solo** por DISM (apps/capabilities prescindibles) +
carpeta `\WinRust\` con binarios, scripts y `unattend.xml`; el shell y los reemplazos se aplican en el **primer inicio
de sesión** con una tarea de provisioning (o en `unattend.xml` para lo que sea de máquina).

### 5.2 DISM: paquetes, capabilities y AppX

| Operación | Comando (offline) | Qué hace | Límite |
|---|---|---|---|
| Quitar una app provisionada | `DISM /Image:C:\Mount /Remove-AppxProvisionedPackage /PackageName:<PFN o nombre>` | No se instala para usuarios nuevos | No afecta a perfiles existentes |
| Quitar una capability | `DISM /Image:C:\Mount /Remove-Capability /CapabilityName:Microsoft.Windows.Notepad~~~~0.0.1.0` | Quita el FOD (Notepad, Paint, WMP, ISE, IE, MathRecognizer...) | No se puede quitar si otro paquete depende; hace falta la FOD ISO para volver |
| Quitar un paquete de actualización | `DISM /Image:C:\Mount /Remove-Package /PackageName:<nombre>` | Quita una LCU/paquete concreto | **Peligroso**: solo paquetes identificados y soportados; no tocar el servicing stack |
| Ver qué hay | `/Get-Packages`, `/Get-Capabilities`, `/Get-ProvisionedAppxPackages` | Inventario | n/a |

Reglas:

1. **Offline** (montar `install.wim` con `/Mount-Image /Index:1 /ReadOnly` para auditar, sin `/ReadOnly` para cambiar)
   es más limpio que `/Online`; el resultado es determinista y no depende de la máquina.
2. **No** quitar paquetes con `Microsoft-Windows-*-Package` que no sean LCU conocidas: el árbol CBS se corrompe.
3. **No** quitar capabilities de idioma que se usen (`Language.Basic`, `Language.Handwriting`, `Language.Speech`,
   `Language.TextToSpeech`): rompen teclado, voz y accesibilidad.
4. Tras quitar una capability, comprobar `DISM /Image:... /Cleanup-Image /StartComponentCleanup` no falla.
5. Documentar cada PFN/capability quitada en el journal de la ISO (nombre + versión + motivo).

### 5.3 Qué quitar, qué dejar y qué es intocable (esta LTSC y retail)

| Elemento | Esta LTSC (26300) | Retail 24H2/25H2 | Decisión |
|---|---|---|---|
| Notepad | capability `Microsoft.Windows.Notepad` (Win32) | AppX `Microsoft.WindowsNotepad` | Quitar **solo** con `rust-notepad` listo |
| Paint | capability `Microsoft.Windows.MSPaint` | AppX `Microsoft.Paint` | Quitar solo con `rust-paint` |
| WordPad | ya no está (24H2) | ya no está | n/a |
| Recortes | capability `Microsoft.Windows.SnippingTool` (Win32) | AppX `Microsoft.ScreenSketch` | Dejar hasta que `ms-screenclip:` esté cableado |
| WMP | capability `Media.WindowsMediaPlayer` | AppX Media Player | Opcional |
| PowerShell ISE | capability | capability | Quitar (opcional) |
| IE | capability `Browser.InternetExplorer` | capability | Quitar (opcional) |
| MathRecognizer / StepsRecorder | capabilities | capabilities | Quitar (opcional) |
| Fotos | no hay (solo `photoview`) | AppX Photos | Quitar + handler `ms-photos:` |
| Calculadora / Cámara / Reloj / Terminal | no hay | AppX | Quitar si hay sustituto o no se usan |
| Store | instalado | instalado | **No quitar** (no soportado) |
| Xbox / Solitaire / Weather / News / Clipchamp / Teams / Copilot / DevHome | no hay (LTSC) | AppX | Quitar (retail) |
| StartMenuExperienceHost / ShellExperienceHost / LockApp / Settings / CBS / Core / UI.Xaml.CBS | `NonRemovable=True` | `NonRemovable=True` | **No tocar** |
| VCLibs / UI.Xaml / WindowsAppRuntime / NET.Native | frameworks | frameworks | **No tocar** |
| `explorer.exe`, `shell32.dll`, `twinui`, `comctl32`, `dwm`, `Windows.UI.Xaml` | sistema | sistema | **No tocar** |
| WinSxS / servicing stack / WinRE | sistema | sistema | **No tocar** |

### 5.4 Servicing: lo que hay que asumir

- Una **LCU** puede reinstalar capabilities que quitamos (Notepad/Paint han vuelto en varias builds) y reescribir
  claves `HKLM` de políticas. Hay que **re-verificar** tras cada actualización.
- Una **feature update** (24H2 -> 25H2) reemplaza `install.wim` en la práctica: el recorte de apps se pierde; el
  provisioning de usuario (HKCU, IFEO, handlers) se conserva si está en una tarea de arranque.
- `sfc /scannow` y `DISM /RestoreHealth` **no** restauran capabilities quitadas (no son componentes protegidos en el
  mismo sentido) pero **sí** restauran binarios de `System32` modificados **[conjetura: verificar con una capability
  concreta]**.
- `Remove-AppxProvisionedPackage` no se puede deshacer sin el paquete original (Store o ISO de FOD).
- La política `RemoveDefaultMicrosoftStorePackages` (Ent/Edu 24H2+, abril 2026) es la vía soportada para que las apps
  no vuelvan; no existe en Pro/Home.

### 5.5 Ediciones

| Edición | Shell Launcher | Política de quitar apps | Notas |
|---|---|---|---|
| Enterprise / LTSC / Education / IoT | Sí | Sí (24H2+ con LCU de abril 2026) | La mejor base para una ISO WinRust "soportada" |
| Pro | No | No | Todo por registro/IFEO; el usuario puede deshacer |
| Home | No | No | Igual; menos componentes que quitar |

### 5.6 Contenido propuesto de la ISO

```
\sources\install.wim                 (recortado con DISM: apps/capabilities prescindibles)
\sources\boot.wim                    (sin tocar)
\WinRust\bin\                        rs-shell.exe, topbar.exe, rust-explorer.exe, rust-settings.exe,
                                     rust-control.exe, rs-shot.exe, rs-du.exe, rtaskmgr-win32.exe,
                                     rust-search.exe, rs-launcher.exe, photoview.exe, rs-notify.exe
\WinRust\provisioning\               iso-provisioning.ps1 (Apply/Restore/DryRun), handlers.reg, ifeo.reg,
                                     associations.reg, state\ (journal)
\WinRust\apps\                       rust-notepad.exe, rust-calc.exe, rust-paint.exe, ... (futuro)
\autounattend.xml                    (opcional: usuario, idioma, OOBE, primera tarea)
\sources\$OEM$\$$\Setup\Scripts\SetupComplete.cmd   (lanza el provisioning de máquina)
```

---
## 6. Dependencias ocultas: qué rompe si se quita

| Dependencia | Quién la usa | Si falta | Veredicto |
|---|---|---|---|
| `explorer.exe` | `SHOpenFolderAndSelectItems`, `ShellExecute` ("abrir carpeta"), `/select`, COM `/factory`, helper de Settings | "Mostrar en carpeta" espera 20 s y falla; Settings no abre | **Intocable en disco**; no bloquear |
| `shell32.dll` | `IFileOperation`, `SHChangeNotify`, `IShellItem`, diálogos, propiedades, papelera, `ShellExecute` | WinRust pierde copiar/borrar/renombrar, diálogos y papelera | **Intocable** |
| `SHOpenFolderAndSelectItems` | Navegadores, chat, descargas, `rtaskmgr` | Necesita una ventana `IShellWindows` o Explorer; `rust-explorer` la implementa (`shellwin.rs`) | **Reemplazable** (hecho), Explorer de respaldo |
| `IFileOperation` | `rust-explorer` (borrar a papelera, copiar) | Sin shell32 no hay operaciones de shell | **Intocable** (es shell32) |
| `ShellExecute` | WinRust entero (`open`, `runas`, `ms-settings:`) | Sin shell32/shell no hay hand-offs | **Intocable** |
| `OpenWith.exe` | "Abrir con" clásico, terceros | WinRust tiene su propio diálogo; el de Windows es prescindible si el propio cubre todo | **Reemplazable** (parcial) |
| `rundll32.exe` | `.cpl`, `Control_RunDLL`, instaladores | Muchos applets y paneles dejan de abrir | **Intocable** |
| `SystemSettings.exe` | Configuración, `ms-settings:`, terceros | Cientos de flujos sin salida | **Reemplazable solo con cobertura completa** |
| `Windows.UI.Xaml` + CBS/Core | ShellHost, Settings, LockApp, InpApp, apps UWP | Shell y apps XAML fallan (KB5072911) | **Intocable** |
| `WaaSMedicSvc` | Reparación de Windows Update | WU puede quedar sin auto-reparación | **Intocable** |
| `ClipSVC` | Licencias, Store, activación de apps | Store y apps licenciadas fallan | **Intocable** si se mantiene Store |
| `AppXSvc` / `StateRepository` | Instalar/actualizar MSIX | No se pueden instalar apps | **Intocable** |
| `ShellHost.exe` | Shell XAML en 24H2+ | Flyouts/Inicio/Settings pueden caer | **Intocable** |
| `RuntimeBroker.exe` | UWP (pickers, notificaciones, cámara) | Apps UWP fallan al pedir capacidades | **Intocable** |
| DCOM (`DcomLaunch`, `RPCSS`) | Todo COM (incluido WinRust) | Nada funciona | **Intocable** |
| `WpnService` | Notificaciones push de apps | Apps sin notificaciones; el puente de WinRust no recibe nada | **Mantener** |
| `TextInputHost.exe` | IME, teclado táctil, emoji, candidatos | Sin entrada CJK/táctil/emoji | **Intocable** (reemplazo total no recomendado) |
| `ApplicationFrameHost.exe` | Ventanas UWP | Apps UWP no dibujan | **Intocable** si hay apps UWP |
| `SearchHost`/`SearchApp`/`SearchUI` | Búsqueda de Windows | WinRust ya los bloquea; `rust-search` cubre archivos | **Reemplazable** (hecho) |
| `StartMenuExperienceHost` | Menú Inicio | WinRust lo sustituye | **Reemplazable** (hecho) |
| `StartDocked.dll` | Implementación del Inicio (COM) | Solo si algo invoca el CLSID del Inicio | **Prescindible** con IFEO |
| `cbdhsvc` | Historial de portapapeles | Win+V de Windows deja de funcionar | **Mantener** (base del futuro Win+V) |
| `UCPD.sys` | Protección de asociaciones | Sin él, `UserChoice` se puede escribir, pero Defender lo protege | **No tocar** |
| `WinRE` / `reagentc` | Recuperación | Sin recuperación ante un fallo de arranque | **No tocar** |
| Anti-cheat / DRM | Juegos | IFEO/COM por usuario puede considerarse manipulación | **Probar**; documentar exclusión |

---
## 7. Lo que WinRust todavía necesita desarrollar (priorizado)

| Prioridad | Componente | Alcance mínimo | Depende de | Esfuerzo | Riesgo |
|---|---|---|---|---|---|
| **Imprescindible** | `rust-notepad` | Abrir/guardar UTF-8/UTF-16/ANSI, buscar/reemplazar, deshacer, imprimir, arrastrar ficheros, "Abrir con" | n/a | M | Bajo |
| **Imprescindible** | Router `winrust-uri` | Tabla `ms-*` -> programa/página + fallback por AUMID + journal | `rust-settings` | S | Medio |
| **Imprescindible** | Provisioning unificado | `iso-provisioning.ps1` con Apply/DryRun/Restore y journal único (sustituye a los 2 scripts actuales) | scripts actuales | M | Bajo |
| **Imprescindible** | Cobertura `ms-settings:` | Las páginas de `rust-settings` + las de `rust-control` (red, fecha, accesibilidad) | router | S | Bajo |
| **Imprescindible** | `ms-screenclip:` | Handler -> `rs-shot --region`; `App Paths` de SnippingTool | `rs-shot` | S | Bajo |
| **Imprescindible** | Alt+Tab/Task View estables sin Explorer | Ventanas, previews, escritorios, multi-monitor | `topbar` | M | Bajo |
| **Deseable** | `rs-calc` | Hecho: científica, memoria, historial, CLI. Falta conversión de unidades | n/a | S | Bajo |
| **Deseable** | `rust-paint` | Bitmap, formas, texto, recorte, guardar PNG/JPG; `ms-paint:` | WIC/GDI+ | M | Bajo |
| **Deseable** | Fotos completa | Galería, zoom, rotar, recortar, EXIF; `ms-photos:` | `photoview` | M | Bajo |
| **Deseable** | Servicios (página) | Listar, iniciar/parar, tipo de inicio, dependencias (SCM) | `rust-control` | S | Bajo |
| **Deseable** | Dispositivos | Árbol por clase, habilitar/deshabilitar, drivers (CfgMgr32/SetupAPI) | `rust-control` | M | Medio |
| **Deseable** | Discos | Volúmenes, formatear, letras, SMART (VDS/IOCTL) | `rust-control` | M | Medio |
| **Deseable** | Visor de eventos | Consultas por log/proveedor/nivel, detalle, exportar (wevtapi) | `rust-control` | M | Bajo |
| **Deseable** | Usuarios | Locales, grupos, crear/borrar, contraseña (NetUser*/LSA) | `rust-control` | M | Medio |
| **Deseable** | Portapapeles (Win+V) | Historial, fijar, buscar, pegar | `topbar` | M | Medio |
| **Deseable** | Emoji (Win+.) | Panel con Segoe UI Emoji, búsqueda, recientes | `topbar` | S | Bajo |
| **Deseable** | Task View completo | Crear/cerrar escritorios, mover ventanas, nombres | API documentada + privada | M | Medio |
| **Deseable** | Run cableado a Win+R | Verificar registro del hotkey y resolución | `rs-launcher` | S | Bajo |
| **Deseable** | Win+P | `DisplaySwitch.exe` o API de proyección | `topbar` | S | Bajo |
| **Opcional** | `rust-media` | Reproducir audio/vídeo (Media Foundation), listas | MF | L | Medio |
| **Opcional** | Editor de registro | Solo lectura + import/export, búsqueda | n/a | M | Alto |
| **Opcional** | Programador de tareas | Listar/crear/borrar (Task Scheduler COM) | `rust-control` | M | Bajo |
| **Opcional** | Snap layouts (Win+Z) | Zonas por monitor, preview | `topbar` | M | Bajo |
| **Opcional** | Share sheet / Cast | Compartir archivos/texto, Miracast | APIs WinRT | L | Medio |
| **Opcional** | Credential provider propio | Login/lock con la identidad de WinRust | LSA | XL | Muy alto |

**Esfuerzo**: S = días, M = 1-2 semanas, L = 3-4 semanas, XL = más.
---

## 8. Riesgos y reversibilidad

### 8.1 Riesgos técnicos

| Riesgo | Probabilidad | Impacto | Mitigación |
|---|---|---|---|
| El shell no arranca (pantalla vacía) | Media | Alto | Shell en HKCU; `explorer.exe` intacto; `Ctrl+Alt+Del` -> `rs-shell --disable`; prueba en VM |
| Una LCU restaura IFEO/valores HKLM | Alta | Medio | Journal + tarea de re-provisioning al inicio; verificación semanal |
| Una feature update reinstala apps quitadas | Alta | Bajo | Política `RemoveDefaultMicrosoftStorePackages` (Ent/Edu) o re-provisioning |
| `sfc`/DISM revierte cambios en disco | Media | Medio | No tocar disco: solo registro |
| AV/EDR bloquea IFEO o COM por usuario | Media | Alto | Documentar exclusión; firmar binarios; evitar `explorer.exe` |
| Anti-cheat rechaza el equipo | Baja/Media | Alto | Probar los juegos objetivo; no tocar `System32`; no inyectar en procesos de terceros |
| Win+V/emoji duplican hotkeys | Media | Bajo | Un solo dueño por hotkey; `RegisterHotKey` con comprobación |
| `ms-settings` mal redirigido deja ajustes inaccesibles | Media | Alto | Router con fallback; no tocar HKLM; probar cada página |
| `rust-explorer` no cubre `IShellWindows` en algún flujo | Media | Medio | Mantener Explorer de respaldo; probar "Mostrar en carpeta" con 5 apps |
| El usuario queda sin acceso a Store/apps | Baja | Medio | No quitar Store; mantener frameworks |
| BitLocker pide recuperación tras cambios de arranque | Baja | Alto | Probar con BitLocker activo; no tocar BCD/arranque |
| WinRE no repara | Baja | Alto | No tocar `Winre.wim`; probar `reagentc /info` |

### 8.2 Lo que las actualizaciones restauran (y cómo sobrevivir)

| Elemento | Lo restaura | Contramedida |
|---|---|---|
| IFEO de hosts del shell | LCU/feature update (a veces) | Re-aplicar en la tarea de provisioning (idempotente) |
| `HKCU\...\Winlogon\Shell` | No (es del usuario) | Nada; sobrevive |
| `HKCU\Software\Classes\ms-*` | No | Nada; sobrevive |
| Handlers `HKLM` | LCU (si toca el componente) | Evitar `HKLM`; usar `HKCU` |
| Capabilities quitadas (Notepad, Paint) | Feature update | Re-quitar o aceptar que vuelvan |
| Apps AppX quitadas | Feature update / Store | Política de empresa o re-quitar |
| Edge | Update orquestado / Store | Políticas + tareas (ya en `explorer-edge.ps1`) |
| Políticas `NoDesktop` etc. | No | Nada |
| `WSearch` deshabilitado | LCU (raro) | Re-aplicar |
| Asociaciones `UserChoice` | Windows (hash inválido) | No tocarlas; usar `OpenWith` |
| Shims `.sdb` | Actualización de `sysmain.sdb` | No usar como base |

### 8.3 Firma, anti-cheat y activación

- **Firma**: firmar los binarios de WinRust (EV o al menos OV) reduce SmartScreen y falsos positivos; no resuelve
  IFEO/COM (eso es comportamiento, no firma).
- **Anti-cheat**: no inyectar DLLs en procesos de terceros, no parchear memoria, no tocar `System32`. IFEO sobre
  `notepad.exe`/`calc.exe` es visible; documentar. Probar Valorant/Fortnite/EAC/BattlEye si es un requisito.
- **Activación**: `ClipSVC`, `LicenseManager`, `slsvc` intactos; no cambiar `EditionID` ni el canal.
- **Store**: si se quita, Microsoft no da soporte y muchas apps dejan de actualizarse; no es un objetivo de este
  diseño.
- **AUMID**: las apps que dependen de un AUMID concreto (Outlook, Teams, WhatsApp) necesitan la app instalada; el
  reemplazo por Win32 no hereda el AUMID.

### 8.4 Recuperación (diseño)

| Escenario | Cómo se recupera |
|---|---|
| Shell de usuario roto | `Ctrl+Alt+Del` -> Task Manager -> `explorer.exe`; luego `rs-shell --disable` |
| IFEO que bloquea Explorer por error | Igual, pero hace falta quitar el valor `Debugger` (requiere admin) |
| `HKCU` del usuario corrupto | Otro admin inicia sesión, `reg load` del hive y borrar `Shell` |
| `HKLM` roto (Shell Launcher/kiosco) | WinRE -> símbolo del sistema -> `reg load`/`reg add`; o restaurar imagen |
| Actualización que rompe el shell | `DISM /Online /Cleanup-Image /RestoreHealth` + `sfc /scannow`; si no, desinstalar la LCU |
| AppX corrupto | `Add-AppxPackage -Register` del manifiesto (KB5072911) |

### 8.5 "Modo restore" unificado

El fork ya tiene dos scripts con estado (`tools/state/explorer-edge.backup.json`,
`tools/state/shell-phase2.backup.json`). Para la ISO conviene **un solo journal**:

```json
{
  "schemaVersion": 2,
  "createdAt": "...",
  "machine": { "name": "...", "build": "26300.9711" },
  "registry": [ { "path": "HKCU\\...", "name": "", "existed": true, "kind": "String", "value": "..." } ],
  "services": [ { "name": "WSearch", "start": 2, "status": "Running" } ],
  "tasks":    [ { "path": "\\", "name": "rs-overlay", "state": "Ready" } ],
  "appx":     [ { "name": "Microsoft.WindowsNotepad", "version": "..." } ],
  "capabilities": [ { "name": "Microsoft.Windows.Notepad~~~~0.0.1.0" } ],
  "files":    [ { "path": "...", "backup": "..." } ]
}
```

Reglas: escribir el journal **antes** de cada cambio, `-Restore` en orden inverso, `-DryRun` por defecto, y una
verificación posterior (`Get-ItemProperty`/`Get-AppxPackage`) que falle ruidosamente si el valor no quedó.
---

## 9. Plan por fases

### F0. Auditoría y congelación (1-2 días)

| Entregable | Detalle |
|---|---|
| Inventario de la build objetivo | `Get-ComputerInfo`, `dism /Get-Capabilities`, `Get-AppxPackage -AllUsers`, `Get-StartApps`, IFEO, servicios, handlers |
| Snapshot de VM | UEFI + Secure Boot + TPM (para BitLocker), dos discos si se prueba Storage Spaces |
| Decisión de edición | LTSC/Enterprise si se quiere Shell Launcher y política de apps; Pro/Home si no |
| Lista de apps que el usuario final necesita | Navegador, Office/Teams, juegos, VPN, impresoras, escáner, cámara, mensajería |
| Criterios de aceptación | Los de la sección 10, acordados antes de tocar nada |

**Gate**: la lista de "apps que deben seguir funcionando" está cerrada y probada en la build limpia.

### F1. Recorte de la ISO (2-4 días)

| Paso | Comando/acción | Verificación |
|---|---|---|
| 1 | Copiar la ISO a una carpeta de trabajo; montar `install.wim` con `DISM /Mount-Image` | `dism /Get-ImageInfo` |
| 2 | Auditar apps/capabilities | `/Get-ProvisionedAppxPackages`, `/Get-Capabilities` |
| 3 | Quitar apps de usuario (retail) y capabilities prescindibles (§5.3) | Lista frente a §5.3 |
| 4 | **No** tocar shell, XAML, Store, frameworks, WinSxS, WinRE | Diff del árbol |
| 5 | Desmontar con `/Commit`; reconstruir ISO con `oscdimg` | Arranque en VM |
| 6 | Instalar en VM y comprobar OOBE, primer inicio, Settings, Store | Checklist 10.1-10.3 |

**Gate**: Windows instala, arranca, abre Settings y Store, y no hay errores de CBS en `dism /Get-Packages`.

### F2. Provisioning del shell (3-5 días)

| Paso | Acción | Reversión |
|---|---|---|
| 1 | Instalar WinRust (`WinRust.exe install all --yes` o el instalador propio de la ISO) | Desinstalador |
| 2 | `rs-shell --enable` (HKCU) | `--disable` |
| 3 | IFEO de hosts del shell + `taskmgr.exe` | journal |
| 4 | `HKCU\Software\Classes\Directory|Folder\shell\open\command` -> `rust-explorer` | journal |
| 5 | Handlers `ms-settings:`/`ms-screenclip:` -> router/`rs-shot` (solo si el router existe) | journal |
| 6 | `WSearch` deshabilitado | journal (start=2) |
| 7 | Edge fuera (fase 1) | `-Restore` |
| 8 | Tarea de re-provisioning al inicio (idempotente) | deshabilitar tarea |

**Gate**: tras reiniciar, hay barra, escritorio, Inicio, búsqueda, bandeja y notificaciones; Settings abre por
fallback; "Mostrar en carpeta" funciona; los archivos se copian/borran.

### F3. Reemplazos nativos (iterativo, 4-8 semanas)

Orden recomendado por dependencia y valor:

1. `rust-notepad` + `.txt` (cierra el agujero más visible).
2. Router `ms-settings:` + páginas que ya existen.
3. `ms-screenclip:` + `App Paths` del Recortes.
4. Páginas de servicios/discos/eventos/usuarios en `rust-control`.
5. `rust-calc`, `rust-paint`/Fotos, `ms-paint:`/`ms-photos:`.
6. Win+V y Win+.
7. Task View completo, Win+P, snap layouts.
8. `rust-media` (opcional).

**Gate por entrega**: prueba de regresión de la sección 10 + revisión de que el fallback sigue vivo.

### F4. Limpieza y endurecimiento (2-3 días)

| Acción | Nota |
|---|---|
| Quitar del ISO las apps ya sustituidas (Notepad/Paint/Fotos/Calc...) | Solo si el sustituto está probado |
| Añadir `OpenWithProgids` de WinRust | Sin tocar `UserChoice` |
| Bloquear reinstalación de Edge/apps | Políticas + tareas |
| Congelar hotkeys (un dueño por atajo) | Evitar duplicados |
| Documentar el "modo restore" | `iso-provisioning.ps1 -Restore` |
| Firmar binarios | Reduce SmartScreen/AV |

**Gate**: `sfc /scannow` limpio, `DISM /RestoreHealth` limpio, Store instala una app, WU aplica una LCU.

### F5. Mantenimiento (continuo)

| Actividad | Frecuencia |
|---|---|
| Re-aplicar provisioning tras LCU/feature update | Cada actualización |
| Revisar IFEO/handlers/servicios | Mensual |
| Probar arranque, shell, Settings, UAC, Store, BitLocker, WinRE | Trimestral |
| Actualizar la ISO con la LCU nueva | Trimestral |
| Revisar que las apps quitadas no han vuelto | Mensual |

---

## 10. Checklist de pruebas de no regresión

### 10.1 Arranque e instalación

- [ ] La ISO arranca en UEFI con Secure Boot.
- [ ] OOBE completa (cuenta local, idioma, red).
- [ ] Primer inicio de sesión sin pantalla negra ni `ShellHost` caído.
- [ ] Reinicio limpio y reinicio tras LCU.
- [ ] `dism /Online /Cleanup-Image /ScanHealth` sin errores.
- [ ] `sfc /scannow` sin violaciones.
- [ ] `reagentc /info` habilitado y WinRE arranca.
- [ ] Con BitLocker (TPM+PIN), el equipo pide la clave y arranca.

### 10.2 Shell

- [ ] Barra visible en el monitor principal y en el secundario.
- [ ] Escritorio con iconos, fondo, menú contextual y "Mostrar escritorio".
- [ ] Inicio abre, busca, lanza apps Win32 y AppX.
- [ ] Búsqueda (Alt+Space) encuentra archivos y apps.
- [ ] Bandeja: iconos de terceros (Steam, OneDrive, impresora, extraíble).
- [ ] Reloj, calendario, volumen, Wi-Fi, batería.
- [ ] Notificaciones propias y nativas (si el puente está activo).
- [ ] Alt+Tab, Task View, Win+D/E/R/I/X/Space/1-9.
- [ ] "Mostrar en carpeta" desde navegador, chat y descargas.
- [ ] Copiar/mover/borrar a papelera/deshacer desde `rust-explorer`.
- [ ] Diálogos Abrir/Guardar en una app Win32 y en una UWP.

### 10.3 Ajustes y herramientas

- [ ] `ms-settings:` cubiertas abren `rust-settings`; el resto abre Windows.
- [ ] Panel de control clásico abre y sus applets funcionan.
- [ ] `services.msc`, `devmgmt.msc`, `diskmgmt.msc`, `eventvwr.msc` abren (aunque sea con mmc).
- [ ] `regedit`, `msconfig`, `resmon`, `perfmon`, `dxdiag`, `msinfo32`, `charmap`, `osk`, `magnify`, `narrator` abren.
- [ ] `cleanmgr` (o `rs-du`) limpia sin romper nada.
- [ ] `notepad`, `calc`, `mspaint`, `mstsc`, Recortes abren (o su sustituto).
- [ ] `ms-screenclip:` lanza `rs-shot`.
- [ ] Store abre, instala y actualiza una app.
- [ ] Windows Update comprueba, descarga e instala.

### 10.4 Seguridad y cuentas

- [ ] UAC pide consentimiento (admin) y credenciales (usuario estándar).
- [ ] Una app elevada funciona (mmc, regedit).
- [ ] Un usuario estándar no puede escribir HKLM ni `System32`.
- [ ] Bloqueo (Win+L) y desbloqueo.
- [ ] Cambio de usuario rápido y cierre de sesión.
- [ ] Dominio/Azure AD (si aplica), GPO aplicadas.
- [ ] Defender activo, actualizado y sin exclusiones raras.

### 10.5 Apps de terceros

- [ ] Navegador (Firefox/Chrome) como predeterminado y abriendo enlaces.
- [ ] Office/Teams/Outlook (incluido el inicio de sesión que necesita Explorer).
- [ ] Steam + un juego con anti-cheat.
- [ ] VPN, impresora, escáner, cámara, auriculares Bluetooth.
- [ ] Apps MSIX (WhatsApp/Spotify) instalan y notifican.
- [ ] Una app que dependa de `SHOpenFolderAndSelectItems`.
- [ ] Una app que abra `ms-settings:` específico (p. ej. Bluetooth).

### 10.6 Recuperación

- [ ] `iso-provisioning.ps1 -Restore` deja el sistema como antes.
- [ ] Tras `-Restore`, Explorer vuelve como shell y todo funciona.
- [ ] Si se borra el perfil, un usuario nuevo recibe el provisioning (o el Windows limpio, según diseño).
- [ ] WinRE -> símbolo del sistema permite reparar el shell.
- [ ] Un `-DryRun` no escribe nada (comparar registro/archivos antes y después).
---

## 11. Preguntas abiertas y experimentos a hacer

| # | Pregunta | Experimento | Si la respuesta es "sí" |
|---|---|---|---|
| 1 | ¿Qué proceso dibuja Inicio/flyouts en 26300? | VM con Explorer; Spy++ sobre la ventana de Inicio; comparar con `ShellHost` | Ajustar la lista de IFEO (posiblemente innecesaria) |
| 2 | ¿`ShellExecute("ms-settings:")` desde un proceso elevado usa `HKCU\Software\Classes`? | Lanzar desde `mmc`/PowerShell elevado y ver qué abre | Si no, mover el handler a `HKLM` para ese caso |
| 3 | ¿`Launcher.LaunchUriAsync` respeta el handler por usuario? | App UWP de prueba que abra `ms-settings:display` | Router viable para apps UWP |
| 4 | ¿Un *app execution alias* de Notepad respeta IFEO? | Retail 24H2: crear IFEO y ejecutar `notepad.exe` desde Win+R y desde `WindowsApps` | Añadir `AppExecutionAliasRedirect`/`FilterFullPath` |
| 5 | ¿`Remove-Capability` de Notepad sobrevive a una LCU? | Quitar, aplicar LCU, comprobar | Re-aplicar en F5 |
| 6 | ¿`sfc` restaura binarios de `System32` modificados? | Modificar una copia en VM y ejecutar `sfc` | Refuerza la regla "no tocar disco" |
| 7 | ¿`WSearch` deshabilitado rompe Outlook/búsqueda de Windows? | Probar Outlook con y sin WSearch | Decidir si se deja deshabilitado |
| 8 | ¿La bandeja propia cubre `NOTIFYICON_VERSION_4` con GUID? | Probar 10 apps (Steam, OneDrive, impresora, extraíble...) | Ajustar `tray.rs` |
| 9 | ¿`SHOpenFolderAndSelectItems` funciona sin ventana abierta? | Probar "Mostrar en carpeta" con carpeta cerrada | Documentar el timeout de 20 s |
| 10 | ¿Alt+Tab propio cubre UWP y ventanas elevadas? | Probar con Settings, Task Manager, juego | Ajustar `topbar` |
| 11 | ¿`UserChoiceLatest`/`HashVersion` está activo en la build? | `reg query` de `SystemProtectedUserData\<SID>\AnyoneRead\AppDefaults` | Asumir que `UserChoice` no es escribible ni con hash |
| 12 | ¿Shell Launcher (LTSC) acepta `rs-shell`? | Configurar XML de Shell Launcher con `rs-shell.exe` | Alternativa soportada a `HKCU` shell |

---

## 12. Fuentes

### 12.1 Documentación de Microsoft

| Tema | Enlace |
|---|---|
| `Winlogon\Shell`, GINA y inicio de sesión | https://learn.microsoft.com/en-us/windows/win32/secauthn/msgina-dll-features |
| Shell Launcher (ediciones, límites, v1/v2) | https://learn.microsoft.com/en-us/windows/configuration/shell-launcher/ |
| IFEO (descripción oficial) | https://learn.microsoft.com/en-us/previous-versions/windows/desktop/xperf/image-file-execution-options |
| IFEO: cómo funciona y bitness del llamador | https://learn.microsoft.com/en-us/archive/blogs/mithuns/image-file-execution-options-ifeo ; https://learn.microsoft.com/en-us/archive/blogs/greggm/inside-image-file-execution-options-debugging |
| IFEO: solo antepone el depurador | https://devblogs.microsoft.com/oldnewthing/20070702-00 |
| MITRE ATT&CK T1546.012 (IFEO injection) | https://attack.mitre.org/techniques/T1546/012/ |
| App Paths y registro de aplicaciones | https://learn.microsoft.com/en-us/windows/win32/shell/app-registration |
| HKCR como vista fusionada | https://learn.microsoft.com/en-us/windows/win32/sysinfo/merged-view-of-hkey-classes-root |
| COM por usuario y elevación | https://learn.microsoft.com/en-us/windows/win32/com/the-com-elevation-moniker ; https://learn.microsoft.com/en-us/previous-versions/bb756926(v=msdn.10) |
| `InprocServer32` / `LocalServer32` | https://learn.microsoft.com/en-us/windows/win32/com/inprocserver32 |
| `IExecuteCommand` y `DelegateExecute` | https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-iexecutecommand ; https://learn.microsoft.com/en-us/windows/win32/shell/shortcut-choose-method |
| `IApplicationActivationManager` | https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-iapplicationactivationmanager |
| Abrir la app de Configuración (`ms-settings:`) | https://learn.microsoft.com/en-us/windows/apps/develop/launch/launch-settings-app |
| Plataforma de apps predeterminadas (UCPD, hash) | https://learn.microsoft.com/en-us/windows/apps/develop/windows-integration/default-apps-platform |
| `SHOpenFolderAndSelectItems` | https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shopenfolderandselectitems |
| `IFileOperation` | https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-ifileoperation |
| `ShellExecute` | https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutew |
| `AddClipboardFormatListener` | https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-addclipboardformatlistener |
| `RegisterHotKey` | https://learn.microsoft.com/en-us/windows/win32/api/winuser/fn-winuser-registerhotkey |
| `fsutil hardlink` | https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/fsutil-hardlink |
| `sfc` | https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/sfc |
| DISM: capabilities | https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/dism-capabilities-package-servicing-command-line-options |
| Features on Demand (Notepad, Paint, WordPad, WMP...) | https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/features-on-demand-non-language-fod |
| `Remove-AppxPackage` y apps no removibles | https://learn.microsoft.com/en-us/powershell/module/appx/remove-appxpackage ; https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/modern-inbox-store-apps-troubleshooting-guidance |
| `Remove-AppxProvisionedPackage` | https://learn.microsoft.com/en-us/powershell/module/dism/remove-appxprovisionedpackage |
| Quitar apps por política (Ent/Edu 24H2+) | https://learn.microsoft.com/en-us/windows/configuration/policy-based-inbox-app-removal/policy-based-inbox-app-removal |
| WordPad eliminado en 24H2 | https://learn.microsoft.com/en-us/windows/whats-new/removed-features |
| Bug XAML del shell (KB5072911, `ShellHost.exe`) | https://support.microsoft.com/en-us/topic/kb5072911-multiple-symptoms-occur-after-provisioning-a-pc-with-a-windows-11-version-24h2-update-d2d30684-4e2b-47f5-9899-a00a8e0acb09 |
| Solución de problemas del menú Inicio | https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/troubleshoot-start-menu-errors |

### 12.2 Blogs y análisis de terceros

| Tema | Enlace |
|---|---|
| `UserChoiceLatest` (nueva protección) | https://kolbi.cz/blog/2025/04/20/userchoicelatest-microsofts-new-protection-for-file-type-associations |
| `UCPD.sys` parte 2 (claves protegidas) | https://kolbi.cz/blog/2025/07/15/ucpd-sys-userchoice-protection-driver-part-2 |
| UCPD y navegador predeterminado | https://www.bleepingcomputer.com/news/microsoft/new-windows-driver-blocks-software-from-changing-default-web-browser/ |
| Cómo funciona UCPD por dentro | https://binary.ninja/2025/03/25/default-browser-upcd.html |
| IFEO como redirección general | https://trainsec.net/library/windows-internals/keyboard-hook-with-with-image-file-execution-options |
| Desinstalar Edge (formas y códigos) | https://woshub.com/uninstall-disable-microsoft-edge/ ; https://github.com/he3als/EdgeRemover |
| Apps de la ISO de 24H2 | https://techcommunity.microsoft.com/blog/windows-itpro-blog/inbox-microsoft-store-apps-update-in-windows-media/4433131 |
| Quitar apps en 24H2/25H2 | https://www.kylereddoch.me/blog/remove-preinstalled-microsoft-store-apps-in-windows-11-24h2-and-25h2 |

### 12.3 Código y documentos del repo

| Tema | Ruta |
|---|---|
| Shell de usuario, Win+D/E, respawn, fallback | `plugins/rs-shell/src/main.rs:1-18, 252-307` |
| Host de bandeja (`Shell_TrayWnd`) y `TaskbarCreated` | `plugins/topbar/src/main.rs:3584-3587`; `plugins/topbar/src/tray.rs:15-40` |
| Alt+Tab propio | `plugins/topbar/src/main.rs:681` |
| Flyouts (Quick, Wifi, Cal, Start, Menu) | `plugins/topbar/src/fly.rs` |
| Escritorio (`Progman`/`WorkerW`) | `plugins/rust-explorer/src/main.rs:1964-2012, 2613-2615` |
| `IShellWindows` (Mostrar en carpeta) | `plugins/rust-explorer/src/shellwin.rs:1-42` |
| `IFileOperation` | `plugins/rust-explorer/src/main.rs:1379-1400` |
| Menú contextual propio | `plugins/rust-explorer/src/menu.rs` |
| Páginas de `ms-settings:` cubiertas | `plugins/rust-settings/src/main.rs:7, 211-216, 2161-2192` |
| Panel de control (categorías) | `plugins/rust-control/src/pages` ; `plugins/rust-control/src/sys.rs:126-135` |
| IFEO de Task Manager | `plugins/rtaskmgr-win32` (y registro de la máquina) |
| Fase 1 (Explorer/Edge, con fuentes) | `tools/explorer-edge.ps1:1804-1875` |
| Fase 2 (shell + IFEO + servicios) | `tools/shell-phase2.ps1:58-79, 179-244` |
| Notificaciones y puente | `tools/notify-bridge.md`; `plugins/rs-notify*`; `FORK-CHANGES.md:76-85` |
| Captura y grabación | `tools/rs-shot-spec.md` (sección 5) |
| Estado y pendientes | `PENDING.md:33-45`; `ROADMAP.md` (limitaciones conocidas) |
| Wallpaper Engine / `Progman` | `tools/wallpaper-engine.md` |

### 12.4 Observaciones locales (solo lectura, 2026-11-24)

- `HKLM\...\Winlogon`: `Shell=explorer.exe`, `Userinit=C:\Windows\system32\userinit.exe,`, `AutoRestartShell=0`.
- `HKCU\...\Winlogon\Shell=C:\Program Files\rust-explorer\rs-shell.exe`.
- IFEO: StartMenuExperienceHost/SearchApp/SearchUI/SearchHost/Widgets/Cortana/ShellExperienceHost -> `cmd.exe /c exit`;
  `taskmgr.exe` -> `C:\Program Files\rtaskmgr\rtaskmgr-win32.exe`.
- `HKCU\Software\Classes\Directory\shell\open\command` y `Folder\...` -> `rust-explorer.exe "%1"`.
- `HKCR\ms-settings\shell\open\command`: `(Default)` vacío, `DelegateExecute={4ed3a719-cea8-4bd9-910d-e252f997afc2}`,
  `windows.system.launcher.dll`.
- `Get-AppxPackage`: `NonRemovable=True` para StartMenuExperienceHost, ShellExperienceHost, LockApp,
  immersivecontrolpanel, Client.CBS/Core, UI.Xaml.CBS.
- `dism /Get-Capabilities`: Notepad, MSPaint, SnippingTool, WindowsMediaPlayer, PowerShell.ISE, InternetExplorer,
  MathRecognizer, StepsRecorder instalados.
- `fsutil hardlink list C:\Windows\explorer.exe`: incluye `C:\Windows\WinSxS\...\explorer.exe`.
- `sc query ucpd`: `RUNNING`; `C:\Windows\System32\drivers\UCPD.sys` presente.
- `ShellHost.exe` en ejecución con `Windows.UI.Xaml.dll` y `Microsoft.UI.Xaml.dll` cargados; sin `explorer.exe`.

---

*Fin del documento. Cualquier cambio en el sistema debe pasar por `iso-provisioning.ps1 -DryRun` primero y por la
checklist de la sección 10 antes de darse por bueno.*
