# Revisión de ajustes de WinRust (24-11-2026, 2.ª pasada)

Alcance: todo lo que un usuario puede cambiar, dónde se guarda, quién lo lee y si está en la ventana única de ajustes
(`plugins/topbar/src/settingswin.rs`, «WinRust Settings» / «Ajustes de WinRust»). Solo se ha leído el código de las demás
apps; `rs-shot` y `rs-notify-bridge` no se han tocado.

## 1. Ajustes de WinRust (los comparten varias apps)

Todos en `%ProgramData%\WinRust\` salvo donde se diga. Los escribe la barra; el resto los relee (caché de ~1 s), así que
no hace falta reiniciar.

| Ajuste | Fichero / clave | Lo escribe | Lo lee | En el hub |
|---|---|---|---|---|
| Tema/estilo (8) | `style.txt` | hub (Apariencia) | todas (`style::style()`) | sí |
| Colores propios (7) | `colors.txt` | hub (Apariencia → Colores) | todas (`style::pal()`) | sí, en línea |
| Plantillas | `themes.txt` | hub (Apariencia → Plantillas) | hub | sí, en línea |
| Opacidad de la barra | `bar-opacity.txt` | hub (Barra → Opacidad) | topbar (`apply_opacity`) | sí |
| Lado de la barra | `side.txt` | hub (Barra) | topbar | sí |
| Tamaño de la barra | `bar-size.txt` | hub (Barra) | topbar | sí |
| Auto-ocultar | `autohide.txt` | hub (Barra) | topbar | sí |
| Botones a la izquierda | `align-left.txt` | hub (Barra) | topbar | sí |
| Alt+Tab por monitor | `alttab-monitor.txt` | hub (Barra) | topbar | sí |
| Monitor de la barra | `look.txt` → `bar-monitor` | hub (Barra) | topbar (`bars_sync`) | sí (si hay >1) |
| Blur de la barra + radio | `look.txt` → `bar-blur`, `bar-radius` | hub (Barra) | topbar | sí |
| Blur/opacidad/fuerza del menú | `look.txt` → `menu-blur`, `menu-alpha`, `menu-radius` | hub (Apariencia → Efectos) | topbar (menús y esta ventana) | sí |
| Frost en ventanas WinRust | `look.txt` → `win-blur` | hub (Apariencia → Efectos) | `common::frost` | sí |
| Botón buscar / vista de tareas | `look.txt` → `bar-search`, `bar-taskview` | hub (Barra) | topbar | sí |
| Notificaciones | `look.txt` → `notify`, `notify-timeout` | hub (Barra) | topbar, rs-notify | sí |
| Inicio: posición / estilo / escala | `look.txt` → `start-left`, `start-style`, `start-scale` | hub (Inicio) | topbar | sí |
| Inicio: ordenar búsqueda por uso | `look.txt` → `start-ranked` | hub (Inicio → Buscador) | topbar (`fly::filtered`) | sí |
| Historial de apps | `%APPDATA%\topbar-usage.txt` (se borra) | hub (Inicio → Buscador) | `fly::uses()` | sí |
| Idioma | `lang.txt` | hub, rust-settings, installer `--lang` | **todas** (`t!`) | sí |
| Blur heredado | `blur.txt` | nadie (legado) | respaldo de `look::bar_blur` | no (compatibilidad) |

## 2. Escritorio (`rust-explorer` corre el escritorio)

| Ajuste | Fichero / marcador | Lo lee | En el hub | Efecto |
|---|---|---|---|---|
| Ocultar iconos | `%APPDATA%\WinRust\desktop-icons-hidden.txt` (existe = ocultos) | `rust-explorer` (desktop) | sí | **inmediato**: además oculta/muestra el `SysListView32` del escritorio (lo mismo que hace su menú) y escribe el marcador para el próximo arranque |
| Iconos pequeños | `%APPDATA%\WinRust\small-icons` (existe = pequeños) | `rust-explorer` (`apply_icon_size`, al crear el escritorio) | sí | **necesita reiniciar el escritorio** (solo se escribe el marcador: cambiar a mano la lista de imágenes y la rejilla desde aquí dejaría el escritorio a medias) |
| Fondo | `HKCU\Control Panel\Desktop\WallPaper` → `%LOCALAPPDATA%\rs-shell\wallpaper.bmp` (lo genera rs-shell al iniciar sesión) | `rust-explorer` (lo pinta una vez al arrancar) | «Elegir imagen…» abre la página de Windows; «Recargar el fondo» reenvía la imagen (`SPI_SETDESKWALLPAPER`) y repinta el escritorio | **una imagen nueva necesita cerrar sesión** |
| Papelera / carpeta del escritorio | — | — | accesos (`shell:RecycleBinFolder`, `%USERPROFILE%\Desktop`) | inmediato |
| Doble clic para abrir | **no existe** en `rust-explorer` (abre con doble clic fijo) | — | no | requeriría tocar `rust-explorer`; no se inventa un marcador que nadie lee |

## 3. Ajustes de cada aplicación

| App | Dónde guarda | En el hub |
|---|---|---|
| **topbar** | tabla 1 + `%APPDATA%\topbar-*.txt` (orden de pines, bandeja, uso, estado) | todo menos el orden de pines/tray |
| **rust-explorer** | `%APPDATA%\WinRust\quick-access.txt`, `rust-explorer-pins.txt`, `desktop-icons-hidden.txt`, `small-icons`; `%APPDATA%\rust-explorer\window.txt` | marcadores del escritorio (tabla 2) + botón «Abrir» |
| **rtaskmgr-win32** | `%APPDATA%\rtaskmgr\settings.ini` + `history.csv`; `%ProgramData%\WinRust\profile.txt`; `%ProgramData%\winrust-debloat\undo-*.txt` | botón «Abrir» (no tiene `--settings`) |
| **rust-control** | nada propio | botón «Abrir» |
| **rust-settings** | `lang.txt`, `look.txt`, `HKCU\...\Themes\Personalize` y sus páginas de sistema | botón «Abrir» |
| **rs-shot** | **ninguno**: valores por defecto + flags (`--out`, `--audio/--no-audio`, `--fps`, `--clipboard`…) | botón «Abrir»; el hub dice que no tiene fichero de ajustes |
| **rust-search / rs-launcher** | `%LOCALAPPDATA%\WinRust\run-history.txt` (historial) | no |
| **rs-shell** | `%LOCALAPPDATA%\rs-shell\log.txt`, `wallpaper.bmp` | no (no tiene ajustes de usuario) |

## 4. La ventana única

Rail de 7 categorías con **icono Segoe MDL2 + nombre**, título grande de la página, tarjetas (`head` + filas con
separadores finos), 960×680 (se encoge en monitores bajos, mínimo 720×560), doble buffer, `Esc`/✕, rueda por categoría.
Nada de páginas aparte ni flyouts intermedios: un clic → hub. El nombre es uno solo en toda la interfaz: clave inglesa
`WinRust Settings` (español «Ajustes de WinRust»), también en el menú de la barra, el del reloj y el del Inicio.

| Categoría | Contenido |
|---|---|
| **Apariencia** | 8 temas con muestra · 7 colores (fila + muestra + `#RRGGBB`; un clic abre el diálogo de color) · plantillas en línea (aplicar con la fila, borrar con la ✕) · guardar plantilla · efectos: blur del menú, 5 opacidades, 3 fuerzas, frost de ventanas |
| **Escritorio** | mostrar iconos, iconos pequeños, aviso de reinicio · fondo: elegir imagen, recargar · accesos: papelera, carpeta del escritorio |
| **Barra de tareas** | posición, tamaño, monitor, blur + radio, opacidad, auto-ocultar, botones a la izquierda, Alt+Tab por monitor, buscar, vista de tareas, notificaciones y duración |
| **Menú Inicio** | posición, estilo (11/10/7), escala, ordenar búsqueda por uso, borrar historial, carpeta de apps fijadas |
| **Idioma** | 6 idiomas + nota de aplicación inmediata |
| **Aplicaciones** | Explorador, Task Manager, Panel de control, rs-shot, rust-settings (con «No instalado») |
| **Avanzado** | restablecer (con confirmación en línea), idioma, versión y rutas, cerrar |

## 5. Por qué no se abría y por qué se congelaba

El arranque lo dejó escrito en `topbar.log`:

```
settings: class "topbar_settings" registered atom 49637, hinstance 0x7ff763370000, last error 0
settings: CreateWindowExW (class+layered) failed: last error 0, size 960x680, messages [0x0081 0x0082]
settings: CreateWindowExW (...) failed: last error 0, size 960x680, messages [0x0081 0x0082]
settings: window created as a subclassed STATIC at 0xe06a8
```

`0x0081` es `WM_NCCREATE` y `0x0082` es `WM_NCDESTROY`, y entre ellos **no** hay `WM_NCCALCSIZE` ni `WM_CREATE`: el
procedimiento de ventana contestó **FALSO** a `WM_NCCREATE` (lo dejaba a `DefWindowProcW`), y en ese caso Windows aborta
la creación y devuelve `NULL` **sin fijar el último error** — exactamente el `GetLastError()==0` que se veía. La clase
estaba bien registrada; el fallo era la respuesta a ese mensaje.

Arreglo: el procedimiento contesta él mismo `WM_NCCREATE => 1`, `WM_CREATE => 0` y `WM_NCCALCSIZE => 0` (ventana sin
marco: el área de cliente es toda la ventana), y se quitó la traza de mensajes que tocaba un `thread_local`/`RefCell`
**dentro** del procedimiento durante la creación. Como último recurso sigue estando la ventana de clase `STATIC`
subclasificada, ahora **sin `WS_EX_LAYERED`**: una ventana laminada pintada con `WM_PAINT` puede no actualizarse (o
repintarse sin fin), que es lo que se veía como «congelada».

El congelamiento de la barra y del hub tenía dos ingredientes, y los dos están fuera:

- la ventana era la `STATIC` + `WS_EX_LAYERED` del último recurso, la única combinación que no se usa en ninguna otra
  ventana del proceso;
- `paint` construía las filas **tres veces** por repintado (`clamp_scroll` + el dibujo + la barra de desplazamiento) y
  cada `rows()` lee los ficheros del look; ahora se construyen una vez y solo se rehacen si hubo que recortar el scroll.

Además, para que un bucle se vea en vez de sufrirse: `count_paint()` cuenta los repintados por segundo y escribe
`settings: N repaints in M ms (X/s): a repaint loop` cuando pasa de 30/s, y `settings: a paint took N ms` cuando un
repintado tarda más de 50 ms. No hay ningún `SetTimer` en todo el módulo, y `WM_MOUSEMOVE` no hace test de posición ni
repinta si el puntero no se ha movido.

### El «congelamiento» del 19:25 (barra que «desaparece»)

Comprobado contra el proceso vivo (pid 8168, hub abierto desde las 19:25:28):

```
ps> EnumWindows -> topbar 0,734 1360x34 vis=True / topbar 1360,866 1440x34 vis=True
                -> topbar_settings 1600,110 960x680 vis=True
ps> FindWindowW('topbar', $null)      -> 0        <-- el «no existe»
ps> FindWindowW('topbar', 'topbar')   -> 984744   <-- sí existe
ps> SendMessageTimeout(bar) -> 0 ms; RedrawWindow(bar) -> 12 ms
ps> GDI 155, USER 65, CPU 0.02 s / 2 s
```

Las barras **no se destruían**: desde PowerShell `FindWindow(class, NULL)` recibe `""` en vez de `NULL` para el título,
y la barra tiene título `"topbar"`, así que no casa (`FindWindow(class, "topbar")` la encuentra al instante); el
`SendMessageTimeout` fallaba porque el HWND era 0. El proceso estaba sano y ocioso.

Lo que sí estaba mal, y se ha corregido:

| Problema | Arreglo |
|---|---|
| El hub salía en el monitor equivocado (`cur_bar()` es la barra del **último evento de primer plano**, no la que abrió el menú) | `CURRENT.set(hwnd)` al pulsar en una barra + `place()` usa `fly::menu_monitor()` (el rect de la barra del menú), luego un `cur_bar()` válido y, como último recurso, **el monitor del cursor** |
| `SetForegroundWindow` podía fallar y el hub quedaba solo «encima» pero sin foco: el primer clic solo activaba la ventana y `Esc` no hacía nada (parece colgado) | `crate::force_fg()` (el mismo truco de los flyouts: `AttachThreadInput` + Alt) |
| `kill_ghost()` destruía el HWND guardado sin comprobar qué era (un handle reciclado podía ser la barra) | solo destruye si la clase es `topbar_fly` |
| Si un `bars_sync` destruyera barras y su creación fallara en silencio, el shell se quedaba sin barra | el procedimiento de la barra contesta `WM_NCCREATE => 1` y `WM_CREATE => 0`; un `create_bar` fallido se registra |
| No había forma de ver si una barra se iba | `bar: WM_DESTROY hwnd=… leader=…, left: …`, `bar: WM_CLOSE …` y `bar: bars_sync A -> B`, además de `settings: … bars A -> B` en cada apertura |

## 6. Duplicados e incoherencias

1. Lista de idiomas: el origen es `common/src/lang.rs` (`Lang::ALL`); el hub y el instalador lo usan, `rust-settings`
   conserva una copia local (fuera de alcance, cambio de una línea).
2. Dos páginas de idioma (hub y `rust-settings`); el texto de `rust-settings` aún pide reiniciar, ya no hace falta
   (`lang.txt` se relee cada 0,5 s).
3. Los ajustes de la barra viven en 10 ficheros (`style.txt`, `side.txt`, `bar-size.txt`, `autohide.txt`,
   `align-left.txt`, `alttab-monitor.txt`, `colors.txt`, `bar-opacity.txt`, `themes.txt`, `look.txt`).
4. `apply_opacity` solo se aplicaba a la barra líder: corregido (el hub pasa por `relook()` → `place()` por barra).
5. `start-scale` se guarda como índice 0/1/2 pero se lee como 90/100/115.
6. `notify-timeout` cicla 3/5/8/10 aunque el fichero admite 1..60 (`rs-notify --timeout`).
7. `menu-alpha` ofrece 90..35 % pero el fondo del hub usa un mínimo de 60 % para que el texto se lea.
8. `win-blur` no tenía interruptor: ahora en Apariencia → Efectos.
9. El nombre de los ajustes estaba repartido («Bar settings», «WinRust settings», «Start menu…»): ahora una sola clave.
10. «Doble clic para abrir» y el tamaño de iconos en caliente no tienen API en `rust-explorer`: documentado arriba.

## 7. Qué revisar al compilar

1. `cargo build -p topbar`; en el arranque, `topbar.log` debe decir `window created (own class) at 0x...` (ya **sin**
   pasar por la `STATIC`). Si apareciera `window created as a subclassed STATIC`, el hub abre igual y el log dice por qué.
2. Clic derecho en la barra → «WinRust Settings»: la ventana aparece centrada en ese monitor.
3. Engranaje del Inicio y menú del Inicio → «WinRust Settings»; reloj de la bandeja → «WinRust Settings».
4. Apariencia: cambiar de tema, abrir el diálogo de un color, aplicar y borrar una plantilla, mover la opacidad del menú.
5. Escritorio: ocultar/mostrar iconos (al instante); «iconos pequeños» cambia la marca y avisa del reinicio.
6. Barra: mover a la izquierda y volver, cambiar de monitor con dos pantallas, apagar el blur.
7. Inicio: apagar «ordenar por uso» y buscar una app; «borrar historial» deja el fichero fuera.
8. Idioma: cambia la ventana y la barra en ~1 s, sin reiniciar.
9. Abrir el hub y mover el ratón por encima 10 s: la barra sigue respondiendo y en el log no aparece ninguna línea de
   `repaints/s` ni de `paint took`.
10. `Esc` cierra; reabrir es instantáneo (no se recrea).
