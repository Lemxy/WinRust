# Revisión de ajustes de WinRust (24-11-2026)

Alcance: todo lo que un usuario puede cambiar y dónde se guarda, quién lo lee y si está en la ventana única de
Ajustes (`plugins/topbar/src/settingswin.rs`). Solo se ha leído el código de las demás apps (`rs-shot` y
`rs-notify-bridge` están en manos de otro agente y no se han tocado).

## 1. Ajustes de la barra y del look (los comparten varias apps)

Todos viven en `%ProgramData%\WinRust\`. La barra es quien los escribe; el resto los relee (cada valor tiene caché de
~1 s), así que no hace falta reiniciar nada.

| Ajuste | Fichero / clave | Lo escribe | Lo lee | ¿En el hub? |
|---|---|---|---|---|
| Tema/estilo (8) | `style.txt` (nombre) | hub (Aspecto), installer | todas (`style::style()`) | sí (Aspecto) |
| Colores propios (7) | `colors.txt` (`bg=RRGGBB;...`) | hub (Aspecto → Colores) | todas (`style::pal()`) | sí (Aspecto → Colores) |
| Transparencia de la barra | `bar-opacity.txt` | hub (Aspecto) | topbar (`apply_opacity`) | sí (Aspecto, 5 valores) |
| Plantillas de look | `themes.txt` | hub (Aspecto → Plantillas) | hub | sí (Aspecto → Plantillas) |
| Lado de la barra | `side.txt` | hub (Barra), flyout de emergencia | topbar | sí (Barra) |
| Tamaño de la barra | `bar-size.txt` | hub (Barra), flyout | topbar | sí (Barra) |
| Auto-ocultar | `autohide.txt` | hub (Barra), flyout | topbar | sí (Barra) |
| Botones a la izquierda | `align-left.txt` | hub (Barra), flyout | topbar | sí (Barra) |
| Alt+Tab por monitor | `alttab-monitor.txt` | hub (Barra), flyout | topbar | sí (Barra) |
| Monitor de la barra | `look.txt` → `bar-monitor` | hub (Barra) | topbar (`bars_sync`) | sí (solo si hay >1 monitor) |
| Blur de la barra | `look.txt` → `bar-blur` | hub (Barra), flyout antiguo | topbar (`blur::on`) | sí (Barra) |
| Radio del blur de la barra | `look.txt` → `bar-radius` (4/8/14) | hub (Barra) | topbar | sí (Barra) |
| Blur del menú | `look.txt` → `menu-blur` | hub (Barra) | topbar, menús de rust-explorer, frost de rust-settings | sí (Barra) |
| Opacidad del menú | `look.txt` → `menu-alpha` (35–100) | hub (Barra) | topbar (menús y esta ventana) | sí (Barra) |
| Fuerza del blur del menú | `look.txt` → `menu-radius` (12/18/26) | hub (Barra) | topbar | sí (Barra) |
| Botón buscar | `look.txt` → `bar-search` | hub (Barra), flyout | topbar | sí (Barra) |
| Botón Vista de tareas | `look.txt` → `bar-taskview` | hub (Barra), flyout | topbar | sí (Barra) |
| Notificaciones | `look.txt` → `notify` | hub (Barra) | topbar (`notify`) | sí (Barra) |
| Duración de notificación | `look.txt` → `notify-timeout` (3/5/8/10 al pulsar) | hub (Barra) | topbar, rs-notify | sí (Barra) |
| Frost en ventanas WinRust | `look.txt` → `win-blur` (off por defecto) | hub (Aspecto) | `common::frost` en explorer, settings, rtaskmgr | sí (Aspecto) **nuevo** |
| Inicio: posición | `look.txt` → `start-left` | hub (Inicio) | topbar | sí (Inicio) |
| Inicio: estilo 7/10/11 | `look.txt` → `start-style` | hub (Inicio) | topbar (`start7/10/11`) | sí (Inicio) |
| Inicio: escala | `look.txt` → `start-scale` (0/1/2 → 90/100/115 %) | hub (Inicio) | topbar | sí (Inicio) |
| Idioma | `lang.txt` (`en/ru/ja/pt/zh/es`) | hub, rust-settings, installer `--lang`, botón de idioma de la barra | **todas** (`t!` → `lang()`) | sí (Idioma) |
| Blur de la barra (heredado) | `blur.txt` (`0`/`1`) | nadie (legado) | `look::bar_blur` como respaldo | no (solo compatibilidad) |

Ficheros propios de la barra en `%APPDATA%\` (no son ajustes de usuario, no tienen página): `topbar-pin-order.txt`,
`topbar-tray.txt`, `topbar-usage.txt`, `topbar-start.txt`, `topbar-taskbar-state.txt`, `topbar-taskband\unpinned.txt`,
`topbar-explorer-pin.txt`.

## 2. Ajustes de cada aplicación

| App | Dónde guarda | Quién lo lee | En el hub |
|---|---|---|---|
| **topbar** (barra) | ver tabla 1 + `%APPDATA%\topbar-*.txt` | topbar | sí (todo menos el orden de pines/tray) |
| **rust-explorer** | `%APPDATA%\WinRust\quick-access.txt`, `rust-explorer-pins.txt`, `desktop-icons-hidden.txt`; `%APPDATA%\rust-explorer\window.txt` (tamaño/posición de ventana) | rust-explorer | **no**: botón "Abrir" (sus opciones se cambian dentro: Ctrl+M, Ctrl+H, menú de la barra lateral) |
| **rtaskmgr-win32** | `%APPDATA%\rtaskmgr\settings.ini` (vista, orden, tamaño, bandeja, tema, ETW, temperatura GPU, reglas) + `history.csv`; `%ProgramData%\WinRust\profile.txt` (perfil de rendimiento); `%ProgramData%\winrust-debloat\undo-*.txt` | rtaskmgr | **no**: botón "Abrir" (no tiene `--settings`) |
| **rust-control** | nada propio (lee el sistema) | — | **no es necesario**: botón "Abrir" |
| **rust-settings** | `lang.txt`, `look.txt` (frost), `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize` (`AppsUseLightTheme`, `SystemUsesLightTheme`, `EnableTransparency`), páginas de pantalla/sonido/red/energía | rust-settings | **no**: botón "Abrir" (es la app de sistema; el hub no duplica sus 15 páginas) |
| **rs-shot** | **ningún fichero**: todo son valores por defecto (audio ON, `Videos\Screen Records`) y flags de línea de comandos (`--out`, `--audio/--no-audio`, `--fps`, `--clipboard`...) | rs-shot | **no se puede sin tocar su crate**: el hub muestra "Abrir" (barra de captura) y lo dice en la página Apps |
| **rust-search / rs-launcher** | `%LOCALAPPDATA%\WinRust\run-history.txt` (historial, no es un ajuste) | rs-launcher | no |
| **rs-shell** | `%LOCALAPPDATA%\rs-shell\log.txt` (log) | rs-shell | no (no tiene ajustes de usuario) |
| **installer** | `lang.txt`, registro de plugins/atajos | installer | idioma: misma clave que el hub |

## 3. Duplicados e incoherencias encontrados

1. **Lista de idiomas en 4 sitios**: `common::lib.rs` (origen), `settings.rs` (`[En, Ru, ...]`), `settingswin.rs`
   (`Lang::ALL` ya), `rust-settings` (`LANGS`), installer (`src/ui.rs::LANGS`). → **corregido**: el origen es
   `common/src/lang.rs` (`Lang::ALL`) y el hub, el instalador y el botón de idioma lo usan; `rust-settings` conserva
   su copia (fuera de alcance: es un cambio de una línea, `LANGS = winrust_common::Lang::ALL`).
2. **Dos páginas de idioma**: el hub y `rust-settings`. El hub es el canónico; la de `rust-settings` sigue existiendo y
   su texto dice "reinicia los programas" (ya no hace falta: `lang.txt` se relee cada 0,5 s).
3. **Los ajustes de la barra están repartidos en 10 ficheros** (`style.txt`, `side.txt`, `bar-size.txt`,
   `autohide.txt`, `align-left.txt`, `alttab-monitor.txt`, `colors.txt`, `bar-opacity.txt`, `themes.txt`, `look.txt`).
   Funciona, pero unificar todo en `look.txt` obligaría a tocar todas las apps: queda anotado, no se ha hecho.
4. **`bar-monitor` frente al monitor de cada app**: solo la barra tiene "monitor" propio; el explorador, rtaskmgr,
   etc. se colocan con `MonitorFromWindow`. No es un bug, pero "Todas/1/2…" solo afecta a la barra.
5. **`start-scale`**: se guarda como índice 0/1/2 pero el getter devuelve 90/100/115. El hub escribe el índice y
   compara con el porcentaje; documentado aquí para que no se "arregle" a medias.
6. **`notify-timeout`**: el hub solo cicla 3/5/8/10 s aunque `look.txt` acepta 1..60 (`rs-notify --timeout`). A
   propósito: nada de listas largas; el valor libre sigue funcionando por fichero.
7. **`menu-alpha`**: el hub ofrece 90/75/67/50/35 %, pero el fondo de esta ventana usa un mínimo de 60 % para que el
   texto se lea. Es una decisión de diseño, no un valor duplicado.
8. **`win-blur` no tenía interruptor en ningún sitio** (solo se podía editar el fichero). → **corregido**: fila en
   Aspecto.
9. **"Ajustes de la barra" (primer elemento del menú contextual) no abría nada**: ver §5.
10. **El engranaje del menú Inicio y la fila "Start menu" abrían una página de flyout distinta** de la ventana de
    Ajustes; el reloj de la bandeja no tenía entrada a los ajustes. → **corregido**: los tres abren el hub (Inicio y
    Barra respectivamente).
11. **La transparencia solo se aplicaba a la barra líder**: `apply_opacity` se llamaba con `leader_bar()`, así que con
    `bar-monitor=all` la otra barra se quedaba opaca. → **corregido**: el hub pasa por `relook()`, que coloca y repinta
    cada barra (`place()` aplica la opacidad a todas).

## 4. La ventana única (`settingswin`)

Rail de categorías + contenido, 900×640 (mínimo exigido 720×560; se encoge si el monitor es más bajo), sin marcos,
doble buffer, `Esc`/✕ cierran, rueda para desplazar la categoría, hover con el acento del proyecto y todo se aplica
en vivo (escribe los mismos ficheros de siempre).

- **Aspecto**: 8 temas con muestra de color, colores propios (diálogo de color del sistema), plantillas (aplicar,
  guardar, borrar), transparencia de la barra, frost en ventanas WinRust.
- **Barra**: posición, tamaño, monitor (Todas/1/2…), blur de barra, blur/opacidad/fuerza del menú, auto-ocultar,
  botones a la izquierda, Alt+Tab por monitor, botones buscar/vista de tareas, notificaciones (on/off y duración).
- **Inicio**: posición, estilo (11/10/7), escala.
- **Idioma**: selector único de 6 idiomas, un solo origen (`lang.txt`), aplicado a todas las apps al instante.
- **Aplicaciones**: Explorador, Task Manager, Panel de control, rs-shot y rust-settings; se abre la app (y se marca
  "No instalado" si no está). Las opciones propias de cada una siguen en su ventana: el hub no las duplica.
- **Avanzado**: reset total (estilo + colores + `look.txt` + barras; las plantillas se conservan), atajo a Idioma,
  versión y las rutas de los ficheros, cerrar.

## 5. El bug de "Ajustes de la barra"

Comprobado sobre el binario instalado (`C:\Program Files\topbar\topbar.exe`, mismo SHA-256 que `target\release`):

- El menú y el mapeo de índices están bien: la fila 2 ("Personalización") lanza `ms-settings` y lo escribe en
  `topbar.log`; la fila 0 sí ejecuta su callback (el "fantasma" del menú desaparece en ~3 ms, frente a los ~150 ms
  del cierre normal, que es exactamente lo que hace `settingswin::open()` al entrar).
- La ventana **no llega a crearse**: un hook `EVENT_OBJECT_CREATE/DESTROY` de todo el sistema no ve ningún
  `topbar_settings` en ningún momento, y el proceso no gasta CPU (no llega ni a capturar el fondo).
- `open()` se comía el fallo: `RegisterClassW` y `CreateWindowExW` no se comprobaban y un `HWND` nulo solo hacía
  `return`.

Arreglo: la clase y la ventana se crean **al arrancar la barra** (`settingswin::init(hi)`, el mismo patrón que los
otros cinco popups de la barra, con el mismo `hInstance`), el fallo se registra con `GetLastError`, se reintenta una
vez y, si aun así no hay ventana, se avisa con un mensaje y se cae al flyout corto de la barra en vez de no hacer
nada. Trazas nuevas en `%LOCALAPPDATA%\rs-shell\topbar.log`:

```
settings: RegisterClassW -> atom N
settings: RegisterClassW failed (E), after UnregisterClassW atom N (last error E)
settings: window ready at 0x...
settings: CreateWindowExW failed (last error E)
settings: no window yet, creating one now
settings: the window could not be created (last error E): ...
settings: opened on category C at X,Y WxH (visible 1)
settings: the settings window is not available, showing the short bar page instead
settings: lang.txt could not be written (run the installer as administrator to apply it everywhere)
```

## 6. Qué revisar al compilar

1. `cargo build -p topbar` → sin errores; warnings nuevos solo de imports si alguno sobra.
2. `%LOCALAPPDATA%\rs-shell\topbar.log`: al arrancar deben aparecer las dos primeras líneas de §5 con `atom` distinto
   de 0 y un `HWND` distinto de 0.
3. Clic derecho en la barra → "Ajustes de la barra": la ventana sale centrada en el monitor de esa barra.
4. Engranaje del menú Inicio y fila "Start menu" → misma ventana, categoría Inicio. Reloj de la bandeja → "WinRust
   settings" → misma ventana, categoría Barra.
5. Cambiar idioma en el hub: los textos de la ventana cambian solos y la barra también en ~1 s (sin reiniciar).
6. `Esc` cierra; volver a abrir no recrea la ventana (un `ShowWindow`).
7. "Restablecer todo" deja `look.txt` y los `*.txt` de estilo en sus valores por defecto y la barra sigue en pie.
