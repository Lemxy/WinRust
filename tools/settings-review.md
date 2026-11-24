# Ajustes de WinRust: arquitectura y revisión

## 1. Arquitectura (25-11-2026): los ajustes son su propia app

El hub **ya no vive en la barra**. Antes `topbar` creaba su propia ventana (`settingswin.rs`) en el hilo de la barra;
ahora es un programa independiente, `plugins/rust-settings`, con su ventana y su bucle de mensajes:

- **`plugins/rust-settings/src/hub.rs`** (nuevo): el hub (rail de 7 categorías con icono, tarjetas, doble buffer, frost,
  textos y persistencia idénticos). Es la ventana principal del programa: 960×680 (mínimo 720×560), sin marco,
  arrastrable por la cabecera (`WM_NCHITTEST` → `HTCAPTION`), centrada en el monitor del cursor, `Esc` cierra.
- **`plugins/rust-settings/src/main.rs`**: solo arranca el hub (`hub::run()`). Sus páginas de sistema (pantalla, sonido,
  energía, red, Bluetooth, apps, Windows Update, personalización, acerca de) siguen siendo el mismo código y se ven en
  **la misma ventana**: se entra por *Aplicaciones → System settings (display, sound…)* o con `--system`, y `Esc` vuelve
  al hub. No hay una segunda ventana.
- **`plugins/topbar/src/settings.rs`**: solo el lanzador (`style_menu` / `style_menu_at(page)`). Busca
  `rust-settings.exe` junto a la barra, en `Program Files\rust-settings` y en `plugin-launcher\bin`, y lo lanza con
  `--page …`. Sin él, la barra muestra un aviso y sigue funcionando.
- `plugins/topbar/src/settingswin.rs`: **eliminado** (sin pre-creado, sin fallback `STATIC`, sin trazas del hub dentro
  de la barra).

### Cómo se pide una página

```
rust-settings --page apariencia|escritorio|barra|inicio|idioma|apps|avanzado
rust-settings --system            (o un ms-settings:sound: las páginas del sistema, en la misma ventana)
```

Instancia única: si ya está abierta, la segunda invocación no crea otra ventana; envía
`RegisterWindowMessageW("WinRust.SettingsPage")` con la página en `wParam` y la trae al frente (`fg::force`).

### Cómo se aplica en vivo

Cada cambio, además de escribirse en `look.txt`/`style.txt`/`lang.txt`/marcadores, hace
`PostMessage(HWND_BROADCAST, RegisterWindowMessageW("WinRust.SettingsChanged"))`. La barra lo atiende en su
procedimiento (`SETTINGS_CHANGED`): recarga el historial de uso y hace `relook(true)` (recoloca y repinta todas las
barras). Los cambios de estilo ya se anunciaban por su cuenta (`style::msg()`) y el idioma se relee cada 0,5 s.

### El bug de «el hub no recibe input» (causa real)

`WM_MOUSELEAVE` **no existe en windows-sys 0.61** (por eso `rust-settings` ya tenía su propio `const WM_MOUSELEAVE`).
En el código del hub, `WM_MOUSELEAVE => { … }` sin ese nombre en el ámbito no era una constante sino un **binding
irrefutable**: ese brazo del `match` capturaba **todos** los mensajes siguientes. Como `WM_PAINT`, `WM_ERASEBKGND` y
`WM_MOUSEMOVE` están antes, la ventana se pintaba y el hover funcionaba; `WM_LBUTTONDOWN`, `WM_MOUSEWHEEL`, `WM_KEYDOWN`,
`WM_CLOSE` y `WM_DESTROY` —todos después— se perdían en ese brazo: ni clics, ni arrastre, ni teclado. Ahora el brazo usa
`crate::WM_MOUSELEAVE` y el hub recibe el input.

### Qué se ha ido

El pre-creado de la ventana en la barra, las cuatro recetas de creación y el fallback `STATIC` (la ventana la crea ya el
propio programa), y los contadores de repintado del hub dentro de la barra.

## 2. Ajustes de WinRust (los comparten varias apps)

Todos en `%ProgramData%\WinRust\` salvo donde se diga. Los escribe el hub; el resto los relee (caché de ~1 s), así que
no hace falta reiniciar.

| Ajuste | Fichero / clave | Lo escribe | Lo lee | En el hub |
|---|---|---|---|---|
| Tema/estilo (8) | `style.txt` | hub (Apariencia) | todas (`style::style()`) | sí |
| Colores propios (7) | `colors.txt` | hub (Apariencia → Colores) | todas (`style::pal()`) | sí, en línea |
| Plantillas | `themes.txt` | hub (Apariencia → Plantillas) | hub | sí, en línea |
| Opacidad de la barra | `bar-opacity.txt` | hub (Barra) | topbar (`apply_opacity`) | sí |
| Lado de la barra | `side.txt` | hub (Barra) | topbar | sí |
| Tamaño de la barra | `bar-size.txt` | hub (Barra) | topbar | sí |
| Auto-ocultar | `autohide.txt` | hub (Barra) | topbar | sí |
| Botones a la izquierda | `align-left.txt` | hub (Barra) | topbar | sí |
| Alt+Tab por monitor | `alttab-monitor.txt` | hub (Barra) | topbar | sí |
| Monitor de la barra | `look.txt` → `bar-monitor` | hub (Barra) | topbar (`bars_sync`) | sí (si hay >1) |
| Blur de la barra + radio | `look.txt` → `bar-blur`, `bar-radius` | hub (Barra) | topbar | sí |
| Blur/opacidad/fuerza del menú | `look.txt` → `menu-blur`, `menu-alpha`, `menu-radius` | hub (Apariencia → Efectos) | topbar | sí |
| Frost en ventanas WinRust | `look.txt` → `win-blur` | hub (Apariencia → Efectos) | `common::frost` | sí |
| Botón buscar / vista de tareas | `look.txt` → `bar-search`, `bar-taskview` | hub (Barra) | topbar | sí |
| Notificaciones | `look.txt` → `notify`, `notify-timeout` | hub (Barra) | topbar, rs-notify | sí |
| Inicio: posición / estilo / escala | `look.txt` → `start-left`, `start-style`, `start-scale` | hub (Inicio) | topbar | sí |
| Inicio: ordenar búsqueda por uso | `look.txt` → `start-ranked` | hub (Inicio → Buscador) | topbar (`fly::filtered`) | sí |
| Historial de apps | `%APPDATA%\topbar-usage.txt` (se borra) | hub (Inicio → Buscador) | `fly::uses()` | sí |
| Idioma | `lang.txt` | hub, installer `--lang` | **todas** (`t!`) | sí |
| Blur heredado | `blur.txt` | nadie (legado) | respaldo de `look::bar_blur` | no (compatibilidad) |

## 3. Escritorio (`rust-explorer` corre el escritorio)

| Ajuste | Fichero / marcador | Lo lee | Efecto |
|---|---|---|---|
| Ocultar iconos | `%APPDATA%\WinRust\desktop-icons-hidden.txt` (existe = ocultos) | `rust-explorer` | **inmediato**: el hub además oculta/muestra el `SysListView32` del escritorio y escribe el marcador |
| Iconos pequeños | `%APPDATA%\WinRust\small-icons` (existe = pequeños) | `rust-explorer` (`apply_icon_size`, al crear el escritorio) | **necesita reiniciar el escritorio** |
| Fondo | `HKCU\Control Panel\Desktop\WallPaper` → `%LOCALAPPDATA%\rs-shell\wallpaper.bmp` | `rust-explorer` (lo pinta al arrancar) | «Elegir imagen…» abre la página de Windows; «Recargar el fondo» reenvía la imagen (`SPI_SETDESKWALLPAPER`); **una imagen nueva necesita cerrar sesión** |
| Papelera / carpeta del escritorio | — | — | accesos inmediatos |
| Doble clic para abrir | **no existe** en `rust-explorer` | — | requeriría tocar `rust-explorer`; no se inventa un marcador |

## 4. Ajustes de cada aplicación

| App | Dónde guarda | En el hub |
|---|---|---|
| **topbar** | tabla 2 + `%APPDATA%\topbar-*.txt` (orden de pines, bandeja, uso, estado) | todo menos el orden de pines/tray |
| **rust-explorer** | `%APPDATA%\WinRust\quick-access.txt`, `rust-explorer-pins.txt`, marcadores del escritorio; `%APPDATA%\rust-explorer\window.txt` | marcadores (tabla 3) + botón «Abrir» |
| **rtaskmgr-win32** | `%APPDATA%\rtaskmgr\settings.ini`; `%ProgramData%\WinRust\profile.txt` | botón «Abrir» |
| **rust-control** | nada propio | botón «Abrir» |
| **rs-shot** | **ninguno**: valores por defecto + flags (`--out`, `--audio`, `--fps`…) | botón «Abrir»; el hub dice que no tiene fichero de ajustes |
| **rust-settings** | este mismo programa: el hub + sus páginas de sistema (`HKCU\...\Themes\Personalize`, `lang.txt`, `look.txt`) | es el hub |
| **rust-search / rs-launcher** | `%LOCALAPPDATA%\WinRust\run-history.txt` (historial) | no |
| **rs-shell** | `%LOCALAPPDATA%\rs-shell\log.txt`, `wallpaper.bmp` | no |

## 5. La ventana

Rail de 7 categorías con icono Segoe MDL2 y nombre (Apariencia, Escritorio, Barra de tareas, Menú Inicio, Idioma,
Aplicaciones, Avanzado), título grande de la página, tarjetas con filas y separadores finos, doble buffer, rueda por
categoría. Un solo nombre en toda la interfaz: clave inglesa `WinRust Settings` (español «Ajustes de WinRust»), también
en el menú de la barra, el del reloj y el del Inicio.

**Apariencia**: 8 temas con muestra · 7 colores (fila + muestra + `#RRGGBB`, el clic abre el diálogo) · plantillas en
línea (aplicar con la fila, borrar con la ✕) · guardar plantilla · efectos (blur del menú, 5 opacidades, 3 fuerzas,
frost de ventanas).
**Escritorio**: mostrar iconos, iconos pequeños, fondo, accesos.
**Barra de tareas**: posición, tamaño, monitor, blur + radio, opacidad, auto-ocultar, botones a la izquierda, Alt+Tab por
monitor, buscar, vista de tareas, notificaciones y duración.
**Menú Inicio**: posición, estilo (11/10/7), escala, ordenar búsqueda por uso, borrar historial, carpeta de apps fijadas.
**Idioma**: 6 idiomas + nota de aplicación inmediata.
**Aplicaciones**: Explorador, Task Manager, Panel de control, rs-shot y las páginas del sistema (misma ventana).
**Avanzado**: restablecer (con confirmación en línea), idioma, versión y rutas, cerrar.

## 6. Duplicados e incoherencias

1. Lista de idiomas: el origen es `common/src/lang.rs` (`Lang::ALL`); hub e instalador lo usan.
2. Los ajustes de la barra viven en 10 ficheros (`style.txt`, `side.txt`, `bar-size.txt`, `autohide.txt`,
   `align-left.txt`, `alttab-monitor.txt`, `colors.txt`, `bar-opacity.txt`, `themes.txt`, `look.txt`).
3. `start-scale` se guarda como índice 0/1/2 y se lee como 90/100/115.
4. `notify-timeout` cicla 3/5/8/10 aunque el fichero admite 1..60.
5. `menu-alpha` ofrece 90..35 % pero el fondo del hub usa un mínimo de 60 %.
6. «Doble clic para abrir» y el tamaño de iconos en caliente no tienen API en `rust-explorer`: documentado arriba.

## 7. Qué revisar al compilar

1. `cargo check -p rust-settings` y `-p topbar`: ambos compilan (solo avisos).
2. `rust-settings` a secas: el hub centrado en el monitor del cursor; clics en filas, rueda, arrastre por la cabecera,
   `Esc` cierra, el primer clic actúa.
3. `rust-settings --page barra`: abre directamente en la barra; lanzarlo otra vez **no** abre una segunda ventana, cambia
   la página de la que ya está.
4. Barra: clic derecho → «WinRust Settings» abre en Apariencia; el reloj en Barra; el Inicio en Inicio. Sin
   `rust-settings.exe`, aviso y la barra sigue viva.
5. Cambiar de tema o de lado de la barra en el hub: la barra se recoloca y repinta al instante (`WinRust.SettingsChanged`).
6. Aplicaciones → «System settings (display, sound…)»: las páginas de sistema en la misma ventana; `Esc` vuelve al hub.
7. Idioma: cambia el hub y la barra en ~1 s, sin reiniciar.
