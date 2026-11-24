# Revisión de ajustes de WinRust (24-11-2026, 2.ª pasada)

Alcance: todo lo que un usuario puede cambiar, dónde se guarda, quién lo lee y si está en la ventana única de Ajustes
(`plugins/topbar/src/settingswin.rs`). Solo se ha leído el código de las demás apps; `rs-shot` y `rs-notify-bridge` no se
han tocado.

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
| Inicio: ordenar búsqueda por uso | `look.txt` → `start-ranked` | hub (Inicio → Buscador) | topbar (`fly::filtered`) | sí **nuevo** |
| Historial de apps | `%APPDATA%\topbar-usage.txt` (se borra) | hub (Inicio → Buscador) | `fly::uses()` | sí **nuevo** |
| Idioma | `lang.txt` | hub, rust-settings, installer `--lang` | **todas** (`t!`) | sí |
| Blur heredado | `blur.txt` | nadie (legado) | respaldo de `look::bar_blur` | no (compatibilidad) |

## 2. Escritorio (`rust-explorer` corre el escritorio)

| Ajuste | Fichero / marcador | Lo lee | En el hub | Efecto |
|---|---|---|---|---|
| Ocultar iconos | `%APPDATA%\WinRust\desktop-icons-hidden.txt` (existe = ocultos) | `rust-explorer` (desktop) | sí | **inmediato**: el hub además oculta/muestra el `SysListView32` del escritorio (lo mismo que hace su menú) y escribe el marcador para el próximo arranque |
| Iconos pequeños | `%APPDATA%\WinRust\small-icons` (existe = pequeños) | `rust-explorer` (`apply_icon_size`, al crear el escritorio) | sí | **necesita reiniciar el escritorio** (el hub solo escribe el marcador; cambiar la lista de imágenes y la rejilla en caliente desde aquí dejaría el escritorio a medias) |
| Fondo | `HKCU\Control Panel\Desktop\WallPaper` → `%LOCALAPPDATA%\rs-shell\wallpaper.bmp` (lo genera rs-shell al iniciar sesión) | `rust-explorer` (lo pinta una vez al arrancar) | "Elegir una imagen…" abre la página de Windows; "Recargar el fondo" reenvía la imagen a Windows (`SPI_SETDESKWALLPAPER`) y repinta el escritorio | **una imagen nueva necesita cerrar sesión** (quien convierte el JPG/PNG al BMP es rs-shell) |
| Papelera / carpeta del escritorio | — | — | accesos (`shell:RecycleBinFolder`, `%USERPROFILE%\Desktop`) | inmediato |
| Doble clic para abrir | **no existe** en `rust-explorer` (abre con doble clic fijo) | — | no | requeriría tocar `rust-explorer`; no se inventa un marcador que nadie lee |

## 3. Ajustes de cada aplicación

| App | Dónde guarda | En el hub |
|---|---|---|
| **topbar** | tabla 1 + `%APPDATA%\topbar-*.txt` (orden de pines, bandeja, uso, estado de la barra) | todo menos el orden de pines/tray |
| **rust-explorer** | `%APPDATA%\WinRust\quick-access.txt`, `rust-explorer-pins.txt`, `desktop-icons-hidden.txt`, `small-icons`; `%APPDATA%\rust-explorer\window.txt` | marcadores del escritorio (tabla 2) + botón "Abrir" |
| **rtaskmgr-win32** | `%APPDATA%\rtaskmgr\settings.ini` + `history.csv`; `%ProgramData%\WinRust\profile.txt`; `%ProgramData%\winrust-debloat\undo-*.txt` | botón "Abrir" (no tiene `--settings`) |
| **rust-control** | nada propio | botón "Abrir" |
| **rust-settings** | `lang.txt`, `look.txt`, `HKCU\...\Themes\Personalize` (`AppsUseLightTheme`, `SystemUsesLightTheme`, `EnableTransparency`) y sus páginas de sistema | botón "Abrir" |
| **rs-shot** | **ninguno**: valores por defecto + flags (`--out`, `--audio/--no-audio`, `--fps`, `--clipboard`…) | botón "Abrir"; se dice en el hub que no tiene fichero de ajustes |
| **rust-search / rs-launcher** | `%LOCALAPPDATA%\WinRust\run-history.txt` (historial) | no |
| **rs-shell** | `%LOCALAPPDATA%\rs-shell\log.txt`, `wallpaper.bmp` | no (no tiene ajustes de usuario) |

## 4. La ventana única

Rail de 7 categorías con **icono Segoe MDL2 + nombre**, título grande de la página, tarjetas (`head` + filas con
separadores finos), 960×680 (se encoge en monitores bajos, mínimo 720×560), doble buffer, `Esc`/✕, rueda por categoría.
Nada de páginas aparte ni flyouts intermedios: un clic → hub.

| Categoría | Contenido |
|---|---|
| **Apariencia** | 8 temas con muestra · 7 colores (fila + muestra + `#RRGGBB`; un clic abre el diálogo de color) · plantillas en línea (aplicar con la fila, borrar con la ✕) · guardar plantilla · efectos: blur del menú, 5 opacidades, 3 fuerzas, frost de ventanas |
| **Escritorio** | mostrar iconos, iconos pequeños, aviso de reinicio · fondo: elegir imagen, recargar · accesos: papelera, carpeta del escritorio |
| **Barra de tareas** | posición, tamaño, monitor, blur + radio, opacidad, auto-ocultar, botones a la izquierda, Alt+Tab por monitor, buscar, vista de tareas, notificaciones y duración |
| **Menú Inicio** | posición, estilo (11/10/7), escala, ordenar búsqueda por uso, borrar historial, carpeta de apps fijadas |
| **Idioma** | 6 idiomas + nota de aplicación inmediata |
| **Aplicaciones** | Explorador, Task Manager, Panel de control, rs-shot, rust-settings (con "No instalado") |
| **Avanzado** | restablecer (con confirmación en línea), idioma, versión y rutas, cerrar |

Puntos de entrada (todos al hub): menú de la barra → Apariencia; engranaje del Inicio y fila "Start menu" → Inicio; menú
del reloj de la bandeja → Barra.

## 5. El bug de `CreateWindowExW` (NULL con `GetLastError()=0`)

Lo que se sabe con seguridad (binario en marcha, hook global `EVENT_OBJECT_CREATE`):

- El callback corre, `settingswin::open()` entra (el fantasma del menú desaparece en ~3 ms) y **la ventana nunca llega a
  existir**.
- La clase **sí** se registra: `RegisterClassW -> atom 49705`, y en el segundo intento `GetClassInfoW` ya la encuentra (no
  vuelve a registrar).
- `CreateWindowExW` devuelve `NULL` con `GetLastError()=0`. Ese par es el de un fallo **antes** de crear la ventana de
  verdad: o la clase no se puede usar desde ese proceso, o algo (un hook `WH_CBT` del sistema) aborta la creación. Ni el
  tamaño (`0x0` también crea) ni el `hInstance` ni el estilo lo explican: un proceso nuevo crea esa misma clase y ese
  mismo estilo sin problema, y en el proceso de la barra los menús y los toasts se crean bien.

Por eso el arreglo no apuesta a una sola receta: **se prueban tres, en orden, y se registra cada resultado**. La pista que
ordena la lista: en ese proceso **todas** las ventanas que sí se crean son `WS_EX_LAYERED` (menús, toasts, bandeja) o
`WS_EX_NOACTIVATE` (tips, Alt+Tab, host de la bandeja); la de ajustes era la única "normal" y activable:

1. nuestra clase, `WS_EX_TOOLWINDOW|WS_EX_TOPMOST|WS_EX_LAYERED` (la receta exacta de los menús, que sí se crean),
2. la misma con `hInstance = NULL`,
3. la misma sin `WS_EX_LAYERED` y con `hInstance = NULL`,
4. **último recurso**: una ventana de la clase de sistema `STATIC` (que Windows registra en todo proceso) y
   `SetWindowLongPtrW(GWLP_WNDPROC, proc)` para quedarnos con el procedimiento. La ventana es idéntica por dentro y
   `existing()` la encuentra por el título (`WinRust settings`), no por la clase.

Además: `init` ya no es imprescindible (si falló, `open_at` vuelve a intentarlo), el tamaño se fuerza a ≥400 px, se
registra el `hInstance`, el átomo, el `GetClassInfoW` (instancia y procedimiento) y **la traza de mensajes** que recibe el
procedimiento durante el intento fallido (si está vacía, el procedimiento no llegó a llamarse), y al final
`ShowWindow(SW_SHOW)` explícito además de `SetWindowPos(SWP_SHOWWINDOW)`.

Trazas en `%LOCALAPPDATA%\rs-shell\topbar.log`:

```
settings: class "topbar_settings" registered atom N, hinstance 0x..., last error E
settings: class "topbar_settings" already registered, hinstance 0x..., procedure Some(0x...)
settings: window created (class+layered) at 0x...
settings: CreateWindowExW (class+layered) failed: last error E, hinstance 0x..., size WxH, messages [0x0081 0x0083 ...]
settings: window created as a subclassed STATIC at 0x...
settings: even a STATIC window failed: last error E
settings: opened on category C at X,Y WxH, visible 1
```

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
9. "Ajustes de la barra" no abría nada: §5.
10. Engranaje del Inicio y fila "Start menu" abrían un flyout distinto; el reloj de la bandeja no tenía entrada: los tres
    van al hub.
11. "Doble clic para abrir" y el tamaño de iconos en caliente no tienen API en `rust-explorer`: documentado arriba.

## 7. Qué revisar al compilar

1. `cargo build -p topbar` y mirar el arranque en `topbar.log`: debe salir `window created (...) at 0x...`. Si sale
   `subclassed STATIC`, el hub abre igual y la traza dice qué receta falló y por qué.
2. Clic derecho en la barra → "Ajustes de la barra": la ventana aparece centrada en ese monitor.
3. Engranaje del Inicio → Inicio; reloj de la bandeja → "WinRust settings" → Barra.
4. Apariencia: cambiar de tema, abrir el diálogo de un color, aplicar y borrar una plantilla, mover la opacidad del menú.
5. Escritorio: ocultar/mostrar iconos (debe verse al instante); "iconos pequeños" cambia la marca y avisa del reinicio.
6. Barra: mover la barra a la izquierda y volver, cambiar de monitor con dos pantallas, apagar el blur.
7. Inicio: apagar "ordenar por uso" y buscar una app; "borrar historial" deja el fichero fuera.
8. Idioma: cambia la ventana y la barra en ~1 s, sin reiniciar.
9. `Esc` cierra; reabrir es instantáneo (no se recrea).
