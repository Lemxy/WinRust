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
| **[doc]** | Documentado por Microsoft (enlace en §12) |
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
   Ver `FORK-CHANGES.md` y `PENDING.md` §4.
2. **Falta la capa de "aplicaciones y diálogos"**: editor de texto, calculadora, Paint/Fotos completos, reproductor,
   visor de imágenes (hay `photoview`), visor de eventos, servicios, dispositivos, discos, usuarios, registro,
   portapapeles (Win+V), emoji (Win+.), Run (hay `rs-launcher`), Alt+Tab (hay uno propio), Task View completo,
   snap layouts (Win+Z), compartir/proyectar, y **los handlers de URI** (`ms-settings:`, `ms-photos:`,
   `ms-paint:`, `ms-screenclip:`…) que hoy siguen apuntando a Windows.
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
   `Microsoft.Windows.SnippingTool`, `Media.WindowsMediaPlayer`…) **[hecho]**. Los paquetes del shell
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
    `rs-launcher` pero hay que cablearlo a Win+R si no lo está ya), Task View completo y el **router de `ms-settings:`**.
13. **Riesgo principal**: que una actualización restaure los componentes (LCU/feature update), que `sfc`/DISM
    "repare" lo que hayamos cambiado en disco (por eso no se toca disco), y que el antivirus/anti-cheat interprete
    IFEO/COM por usuario como manipulación. Mitigación: registro por usuario, journal, `-Restore`, y no tocar
    `WinSxS`/`System32`.
14. **Recuperación**: el shell está en `HKCU` (por usuario), así que un fallo deja al usuario con pantalla vacía pero
    el sistema arranca; `Ctrl+Alt+Del → Task Manager → ejecutar explorer.exe` y `rs-shell --disable` recuperan.
    Un shell en `HKLM` (todos los usuarios) es mucho más peligroso: solo para kiosco/`Shell Launcher` (Ent/Edu/IoT).

---## 1. Punto de partida verificado

### 1.1 Lo que el fork ya hace (con referencia)
| 1 | prueba | `A -> B` | x | y |
| 9 | Notificaciones | `\\.\pipe\winrust-notify` | x | y |
| 2 | Un solo shell | tarea `rs-overlay` deshabilitada | `tools/shell-phase2.ps1:210-216` | `-Restore` |
| 3 | Hosts bloqueados | IFEO `Debugger = cmd.exe /c exit` para `StartMenuExperienceHost.exe`, `SearchApp.exe` | `tools/shell-phase2.ps1:68-78` | `-Restore` |
| 4 | Task Manager | IFEO `taskmgr.exe` | `plugins/rtaskmgr-win32` | uninstall |
| 5 | Carpetas | `HKCU\Software\Classes\Directory\shell\open\command` | registro | borrar |
