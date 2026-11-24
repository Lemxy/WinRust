# Soporte de Wallpaper Engine en WinRust

> **Estado:** investigación técnica + plan de implementación. No se ha ejecutado ni instalado Wallpaper Engine (WE) durante este trabajo.
> **Máquina de referencia:** Windows 10 Enterprise LTSC 2024, versión 26H2, build **26300.9711** (familia 24H2+, “raised desktop”).
> **Código inspeccionado:** `plugins/rust-explorer/src/main.rs` (módulo `desk`, creación de `Progman`), `plugins/rs-shell/src/main.rs` (wallpaper/autostart).
> **Nota local:** en esta máquina no aparece `wallpaper_engine` en las bibliotecas de Steam conocidas (`C:\Program Files (x86)\Steam`, `G:\SteamLibrary`, etc.), así que las conclusiones sobre WE son estáticas/documentales y el plan incluye una fase 0 de verificación en una máquina con WE instalado.

---

## 0. Resumen ejecutivo

Wallpaper Engine **no inyecta código en Explorer ni necesita que `explorer.exe` sea el proceso padre**. Lo que hace es buscar una jerarquía de ventanas concreta por clase (`Progman`, `SHELLDLL_DefView`, `WorkerW`), pedirle a `Progman` que cree el `WorkerW` de fondo mediante el mensaje no documentado `0x052C`, y reparentar su ventana de render con `SetParent` dentro de ese `WorkerW`. El propio desarrollador de WE lo resumió así en 2017: *“Injection is not happening… all you'd need to do is make sure that a certain window hierarchy exists”* ([Steam, 2017](https://steamcommunity.com/app/431960/discussions/0/144513128845344033/)).

Hay dos topologías relevantes:

- **Clásica (Windows 10 / Windows 11 ≤ 23H2):** `Progman` es una ventana normal; el escritorio de iconos vive en un `WorkerW` **de nivel superior** que contiene `SHELLDLL_DefView`; el `WorkerW` de fondo es el **siguiente hermano de nivel superior** de ese primero.
- **Raised desktop (builds ≥ 26100, incluida 24H2/25H2/26H2):** `Progman` se crea con `WS_EX_NOREDIRECTIONBITMAP`; `SHELLDLL_DefView` es un hijo **layered** de `Progman`; y el `WorkerW` que pinta el fondo es un **hijo de `Progman`, por debajo de `DefView`**. Aquí la composición ya no es GDI: el shell compone ese subárbol por la capa visual (DirectComposition), y una ventana GDI normal no se ve.

El código actual de WinRust ya tiene una aproximación a la topología raised: crea `Progman` con título `rs-desktop`, un `SHELLDLL_DefView` hijo, y al recibir `0x052C` crea un `WorkerW` hijo en el fondo; además hace transparente la lista de iconos con color-key cuando detecta un hijo dentro de ese `WorkerW` (`main.rs`, módulo `desk`, aprox. líneas 1237–1460 y 1572–1576). **Pero no cumple ninguna de las dos topologías completas**:

- Para la clásica falta un `WorkerW` de nivel superior que contenga `SHELLDLL_DefView`/`SysListView32`; WE busca ese hermano de nivel superior y no lo encuentra.
- El título de `Progman` es `rs-desktop`, no el canónico `"Program Manager"`; si WE usa `FindWindowW(L"Progman", L"Program Manager")`, falla antes de mirar la jerarquía.
- Para la raised falta `WS_EX_NOREDIRECTIONBITMAP` en `Progman`, `SHELLDLL_DefView` layered y una superficie compuesta por DComp; con GDI el contenido no se compone.
- `rs-shell` llama a `SPI_SETDESKWALLPAPER` al arrancar y al cambiar el fondo; en la topología raised eso puede destruir el `WorkerW` de WE (ver [FeatherWall](https://raw.githubusercontent.com/RitvikDayal/featherwall/main/docs/architecture.md)).

**Recomendación principal:** no empezar por reescribir el escritorio. La vía más segura y rápida para tener WE detrás de los iconos en 26H2 es usar la **API oficial de línea de comandos de WE** (`-control openWallpaper … -playInWindow`) y reparentar esa ventana dentro del `WorkerW` que WinRust ya crea. Es la única alternativa que no depende de la topología interna de WE y que funciona con el `Progman` actual. Después, si se quiere soporte “nativo” completo (playlists, reglas de pausa, integración de escritorio), se aborda la topología clásica o la raised con DirectComposition, por fases y con feature flag.

---

## 1. Cómo se engancha Wallpaper Engine exactamente

### 1.1 El truco clásico: `0x052C` + `WorkerW`

El mecanismo está bien documentado por la comunidad desde 2014:

1. WE llama a `FindWindowW(L"Progman", NULL)`.
2. Envía a `Progman` el mensaje **no documentado** `0x052C` (a veces llamado `WM_SPAWN_WORKERW`) con `SendMessageTimeoutW`. En Windows 10/11 clásico, eso hace que Explorer cree un `WorkerW` entre la capa de iconos y el fondo estático.
3. WE enumera las ventanas de nivel superior con `EnumWindows`, busca la que tiene un hijo de clase `SHELLDLL_DefView` (la capa de iconos) y toma el **siguiente** `WorkerW` de nivel superior con `FindWindowExW(NULL, hwnd_iconos, L"WorkerW", NULL)`.
4. WE hace `SetParent(ventana_wallpaper, workerw)` y muestra su ventana.

La explicación canónica, con el volcado de Spy++, es la de CodeProject: *“Send 0x052C to Progman. This message directs Progman to spawn a WorkerW behind the desktop icons”* y *“we take its next sibling and assign it to workerw”* ([CodeProject, 2014](https://www.codeproject.com/Articles/856020/Draw-Behind-Desktop-Icons-in-Windows-plus)). La documentación de DynamicWallpaper lo reproduce con `wParam=0xD`, `lParam=0` y `lParam=1` ([dynamicwallpaper.readthedocs.io](https://dynamicwallpaper.readthedocs.io/en/docs/dev/make-wallpaper.html)), y `weebp` usa la misma secuencia ([weebp.c](https://github.com/Francesco149/weebp/blob/master/src/weebp.c)).

### 1.2 La topología raised (24H2 en adelante)

Microsoft confirmó el cambio:

> *“When the desktop is split out from the list view window (aka the ‘raised desktop’) we no longer create multiple top-level HWNDs… the top-level ‘Progman’ window is now created with `WS_EX_NOREDIRECTIONBITMAP` … and the shell DefView child window is a `WS_EX_LAYERED` child window. When the desktop is raised, we create a child `WorkerW` window that is z-ordered under the DefView that will render the wallpaper.”*
> — Microsoft, reproducido en [lively-community/lively#2074](https://github.com/lively-community/lively/issues/2074)

Consecuencias:

- El `WorkerW` ya **no** es un hermano de nivel superior: es un hijo de `Progman`.
- `SHELLDLL_DefView` tampoco está dentro de un `WorkerW`: está directamente bajo `Progman`.
- El flag que distingue la topología es `GetWindowLongPtrW(progman, GWL_EXSTYLE) & WS_EX_NOREDIRECTIONBITMAP` (0x00200000). Lively lo usa exactamente así ([DeepWiki: Desktop Core](https://deepwiki.com/rocksdanister/lively/3.1-desktop-core)).
- En raised, **GDI no compone**: ni `WM_PAINT`, ni `BitBlt`, ni `UpdateLayeredWindow` sirven para el subárbol de `Progman`. FeatherWall documenta que hay que presentar con **DirectComposition + swapchain flip-model** ([FeatherWall architecture](https://raw.githubusercontent.com/RitvikDayal/featherwall/main/docs/architecture.md)). Esto es coherente con que la ventana de contenido de WE tenga `WS_EX_NOREDIRECTIONBITMAP` y `WS_EX_LAYERED` (observación de comunidad con Spy++: [Steam, 2025](https://steamcommunity.com/app/431960/discussions/2/571540483751323871/)).
- `0x052C` sigue funcionando en 24H2, pero el `WorkerW` aparece como hijo de `Progman`, no como hermano ([lively#2074, comentario de 2025](https://github.com/lively-community/lively/issues/2074)). El `wParam=0xD, lParam=0x1` es el que mantiene vivo el `WorkerW` en las builds nuevas; patrones antiguos `(0,0)` pueden liberarlo y Explorer lo destruye ([slab/wallpaper README](https://tangled.org/aesthetic.computer/core/blob/c70b9eac6a5589fb07c5606f96557a41c40017ce/slab/windows/wallpaper/README.md)).

### 1.3 Qué comprueba WE realmente

La mejor evidencia pública del contrato exacto es `weshim.dll` de **we-linux**, un shim que hace funcionar WE bajo Wine/Proton sin Explorer. El shim crea un escritorio falso y engancha las llamadas de WE; su código muestra qué APIs usa WE para engancharse ([weshim.c](https://github.com/zHappySnake/we-linux/blob/main/shim/weshim.c)):

- `FindWindowW(L"Progman", …)`
- `SendMessageTimeoutW(progman, 0x052C, …)`
- `EnumWindows` + `FindWindowExW(…, L"SHELLDLL_DefView", …)` + `FindWindowExW(NULL, after, L"WorkerW", …)`
- `SetParent(ventana_wallpaper, workerw)`
- `SetWindowLongPtrW(GWL_EXSTYLE, …)` (WE pone `WS_EX_TRANSPARENT` para que los clics pasen a los iconos)
- `SystemParametersInfoW` (consulta/actualización del fondo)
- `ShowCursor(FALSE)` (WE oculta el cursor porque en Windows los iconos ya lo proporcionan)
- `CreateWindowExW` para la ventana de render

El shim **no engancha `OpenProcess`, ni una comprobación de `explorer.exe`, ni `GetShellWindow`**. Esto no demuestra al 100% que WE no mire el proceso en alguna versión, pero sí demuestra que **el contrato funcional es la jerarquía de clases**, no la presencia de Explorer: WE funciona bajo Wine con solo fabricar `Progman` + `WorkerW` + `SHELLDLL_DefView`. El propio desarrollador de WE lo confirmó en 2017.

Matices:

- **El título de `Progman` es un riesgo real.** `weshim` no lo llama `rs-desktop`: lo crea con el título canónico **`"Program Manager"`**. Muchísimo código de shell (y probablemente partes de WE) usa `FindWindowW(L"Progman", L"Program Manager")`, no solo la clase; si WE pasa ese título y el nuestro es `rs-desktop`, `FindWindowW` devuelve `NULL` y no hay enganche. No se puede afirmar que WE pase título sin verlo, pero el coste de alinearse es mínimo: crear `Progman` con `"Program Manager"` y cambiar la detección de `rs-shell` (que hoy hace `FindWindowW("Progman", "rs-desktop")`, `rs-shell/src/main.rs:240`) a clase + PID/proceso, o a un marcador propio. Esto debe entrar en la fase 0/2.
- **`GetShellWindow()` sí puede ser un problema.** Si WE (o alguna librería de shell) usa `GetShellWindow()` en vez de `FindWindow("Progman")`, sin Explorer devuelve `NULL` ([Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getshellwindow)). La solución habitual en shells de terceros es llamar a `SetShellWindow(Progman)` / `SetShellWindowEx(Progman, listview)` de `shell32.dll` (exportada pero no documentada; ver la implementación de ReactOS: [SetShellWindowEx](https://reactos.org/archives/public/ros-diffs/2009-January/028360.html)). Es experimental y hay que probarlo; pero es barato y puede evitar fallos de apps que asumen `GetShellWindow() == Progman`.
- **Integridad y sesión:** WE debe correr en la misma sesión interactiva y con el mismo nivel de integridad que el shell. Si `rust-explorer` corre elevado y WE no (o al revés), UIPI puede bloquear `SetParent`/`SetWindowLongPtr`/mensajes. No ejecutar ninguno como administrador (WE además rompe Steamworks si se eleva: [WE Steamworks](https://help.wallpaperengine.io/en/steam/steamworks.html)).
- **DWM:** WE necesita composición DWM activa. En un shell propio no hay que desactivarla.

### 1.4 Qué NO hace WE

- No inyecta DLLs en Explorer (confirmado por el desarrollador).
- No necesita el overlay de Steam.
- No necesita ser hijo de `steam.exe`.
- No “se niega” explícitamente si no detecta Explorer; simplemente no encuentra el host y no pinta. La excepción podría ser `GetShellWindow()`/Steamworks, que hay que validar.

---

## 2. Qué necesita un shell de terceros

### 2.1 Mínimo para la topología clásica

Para que WE se enganche solo, sin CLI:

```
[arriba]  WorkerW  "capa de iconos"   (top-level, visible, cubre el virtual screen)
            └─ SHELLDLL_DefView       (hijo, a ser posible WS_EX_LAYERED)
                 └─ SysListView32     (hijo, los iconos)
          WorkerW  "host de fondo"    (top-level, justo DEBAJO de la capa de iconos)
          Progman                     (top-level, debajo del host; puede estar oculto)
[abajo]
```

Orden Z (de arriba a abajo): **capa de iconos → host de fondo → Progman**. Así `FindWindowExW(NULL, capa_de_iconos, L"WorkerW")` devuelve el host, y `SetParent` de WE deja el wallpaper entre el fondo y los iconos.

No basta con:

- crear `Progman` y un `WorkerW` hijo cualquiera;
- crear un `SHELLDLL_DefView` de tamaño cero y poner el `SysListView32` como hermano en vez de hijo;
- crear el `WorkerW` después de `Progman` pero dejarlo tapado por una `Progman` opaca.

Eso es exactamente lo que hace el código actual de WinRust, y explica por qué WE puede no encontrar el host.

### 2.2 Mínimo para la topología raised

```
Progman  (WS_EX_NOREDIRECTIONBITMAP)
  ├─ SHELLDLL_DefView  (WS_EX_LAYERED, hijos: SysListView32)
  ├─ [superficie de wallpaper: WS_EX_LAYERED, alpha=255, DComp]
  └─ WorkerW           (fondo estático, HWND_BOTTOM)
```

Aquí no se puede usar GDI para la superficie del wallpaper: hay que presentar con D3D11/DirectComposition, y la ventana debe ser `WS_EX_LAYERED` con alpha 255 (Microsoft, vía [lively#2074](https://github.com/lively-community/lively/issues/2074); [FeatherWall](https://raw.githubusercontent.com/RitvikDayal/featherwall/main/docs/architecture.md)). `DefView` debe ser layered y estar por encima. En builds ≥ 26100, `DefView` no siempre renderiza transparente salvo HDR/slideshow; Microsoft dijo que a partir de 27XXX será siempre transparente ([lively#2074, respuesta de Microsoft](https://github.com/lively-community/lively/issues/2074)). Por eso imitar la topología raised sin DComp es una trampa: se pinta en superficies que nunca se componen.

### 2.3 Mensajes que conviene manejar

| Mensaje | Dónde | Qué hacer |
|---|---|---|
| `0x052C` (`WM_SPAWN_WORKERW`) | `Progman` | Crear/garantizar el `WorkerW` host, reafirmar el orden Z y devolver 0. WE lo envía con `SendMessageTimeout`; hay que responder rápido. |
| `WM_SETTINGCHANGE` / `WM_WININICHANGE` (`0x001A`) | `Progman` | Si `wParam == SPI_SETDESKWALLPAPER` (0x0014), recargar el BMP/registro; si `wParam == SPI_SETWORKAREA` (0x002F), recolocar la lista. **No destruir el host.** |
| `WM_DISPLAYCHANGE` (`0x007E`) | `Progman` | Redimensionar `Progman`, capa de iconos y host al virtual screen; reafirmar Z; WE suele reaccionar solo, pero el host debe seguir cubriendo todos los monitores. |
| `WM_DWMCOMPOSITIONCHANGED` (`0x031E`) | `Progman` | Reaplicar `WS_EX_LAYERED`/color-key o el target de DComp. |
| `WM_COMMAND` con `0x7402` | `SHELLDLL_DefView` | Alternar iconos (lo usa “Show desktop icons” y `wallpaper64.exe -control hideIcons/showIcons`). Sin esto, WE puede ocultar/enseñar iconos y no pasar nada. |
| `TaskbarCreated` (mensaje registrado) | ventana oculta del shell | Re-registrar tray/recargar tras reinicio del shell; no es imprescindible para el enganche, sí para la bandeja de WE. |
| `WM_ENDSESSION` / lock | shell | Pausar/cerrar WE limpiamente (`-control pause` / `closeWallpaper`). |

No hace falta “responder” a `WM_SETTINGCHANGE` para que WE se enganche; hace falta sobre todo **no romper** el host cuando `rs-shell` cambia el fondo.

### 2.4 ¿Basta con `Progman` + `WorkerW` + `SHELLDLL_DefView`?

Para la clásica: **sí, si el orden Z y la relación padre-hijo son correctos**. `weshim` lo demuestra: crea `Progman` oculto, un `WorkerW` de iconos con `SHELLDLL_DefView`, y un `WorkerW` host visible; WE encuentra el host y reparenta su ventana. El shim incluso tiene que interceptar `FindWindowExW` porque el host y la capa de iconos son dos `WorkerW` de nivel superior y WE espera “el siguiente después del de iconos” ([weshim.c](https://github.com/zHappySnake/we-linux/blob/main/shim/weshim.c)).

Para la raised: **no basta** con crear los HWND; hay que darles los estilos correctos y presentar por DComp. Si solo se crean las clases, WE puede encontrar el `WorkerW` hijo pero el contenido no se ve (pantalla negra), que es el fallo típico en 24H2+.

---

## 3. Estado actual de WinRust (inspección local)

### 3.1 Lo que ya existe

En `plugins/rust-explorer/src/main.rs`:

- `RegisterClassW` de `Progman`, `WorkerW` y `SHELLDLL_DefView` (aprox. líneas 1935–1940).
- `Progman` se crea como `WS_POPUP` + `WS_EX_TOOLWINDOW`, título `rs-desktop`, cubriendo el virtual screen y en `HWND_BOTTOM` (líneas 1941–1944).
- `SHELLDLL_DefView` se crea como hijo de `Progman`, tamaño 0, con `DefWindowProc` (líneas 1572–1574).
- `SysListView32` se crea como hijo de `Progman` (no de `DefView`), con la lista de iconos (línea 1554).
- Al recibir `0x052C`, `desk::spawn_workerw(h)` crea un `WorkerW` **hijo** de `Progman`, `WS_CHILD | WS_VISIBLE`, tamaño del cliente y `HWND_BOTTOM` (líneas 1414–1421; handler en 1879).
- `desk::check_live` corre cada 2 s y considera “live” si `GetWindow(WORKERW, GW_CHILD) != NULL`; entonces pone `WS_EX_LAYERED` + `LWA_COLORKEY` (color `0x000101`) en la lista (líneas 1425–1438).
- `desk::erase` pinta el color key en `Progman` mientras `LIVE`; si no, pinta el BMP del wallpaper (líneas 1443–1455).
- `WM_SIZE` redimensiona el `WorkerW` hijo (línea 1630).

En `plugins/rs-shell/src/main.rs`:

- `set_wallpaper()` llama a `SystemParametersInfoW(SPI_SETDESKWALLPAPER, …)` (línea 204) al arrancar y otra vez a los 4 s (líneas 224–233).
- El escritorio se localiza por `FindWindowW("Progman", "rs-desktop")` (líneas 240 y 365), así que cambiar el título de `Progman` rompería el supervisor.

### 3.2 Diagnóstico de los huecos

| Requisito de WE | Estado en WinRust | Consecuencia |
|---|---|---|
| `Progman` de nivel superior | Sí | OK |
| `Progman` con título `"Program Manager"` | **No**: título `rs-desktop` | Si WE usa `FindWindow("Progman", "Program Manager")`, no lo encuentra; alinear título y cambiar la detección de `rs-shell` |
| `SHELLDLL_DefView` presente | Sí, pero tamaño 0 y sin `SysListView32` dentro | Muchos enumeradores de shell esperan `DefView → SysListView32`; WE en principio solo necesita la clase, pero es frágil |
| `WorkerW` host de nivel superior, debajo de la capa de iconos | **No**: el `WorkerW` es hijo de `Progman` | La ruta clásica de WE no lo encuentra (`FindWindowEx(NULL, progman, "WorkerW")` no busca hijos) |
| `WorkerW` hijo bajo `Progman` (raised) | Sí, se crea al recibir `0x052C` | La ruta raised puede encontrarlo, pero faltan NRB/DefView layered/DComp para que se vea |
| `Progman` con `WS_EX_NOREDIRECTIONBITMAP` | **No** | WE puede clasificar el escritorio como clásico y no usar la ruta hija; o usar la clásica y fallar |
| `DefView` layered / composición DComp | **No** | Si WE usa la ruta raised, el contenido GDI no se compone |
| Detección de “live” | Solo mira hijos del `WorkerW` propio | Si WE reparenta a `Progman` o a un host de nivel superior, `LIVE` nunca se activa y la lista tapa el wallpaper |
| `SetShellWindow` | No | `GetShellWindow()` devuelve `NULL`; posible fallo de WE o de apps que lo usan |
| `SPI_SETDESKWALLPAPER` seguro | `rs-shell` lo llama sin coordinación | En raised puede destruir el `WorkerW` de WE (ver FeatherWall) |
| Multi-monitor | `Progman` y host cubren el virtual screen | Bien de base; falta validar coordenadas negativas y DPI |
| Reattach tras reinicio de WE | No | Si WE se reinicia o cambia la resolución, el host puede quedar vacío |

**Conclusión:** el código actual es un buen esqueleto para la topología raised, pero no es suficiente para ninguna de las dos. La opción menos arriesgada es no depender de que WE se enganche solo.

---

## 4. Alternativas prácticas

### 4.1 CLI oficial de WE + reparent (recomendada a corto plazo)

WE documenta oficialmente `-control openWallpaper` con `-playInWindow`, `-width`, `-height`, `-x`, `-y`, `-borderless`, `-activate` ([WE CLI docs](https://help.wallpaperengine.io/en/functionality/cli.html)). La idea:

1. Arrancar WE si no está corriendo (`wallpaper64.exe`, o `launcher.exe -run wallpaper64.exe -nobrowse`).
2. Ejecutar, por monitor:
   ```text
   wallpaper64.exe -control openWallpaper -file <project.json|scene.pkg|mp4|index.html>
       -playInWindow "WinRust Wallpaper 0" -width <w> -height <h>
       -x <x> -y <y> -borderless
   ```
3. Esperar a que exista la ventana con ese título (`FindWindowW(NULL, title)` o enumerar por PID).
4. Quitar estilos de ventana superior (`WS_POPUP`, `WS_CAPTION`, `WS_THICKFRAME`) y poner `WS_CHILD`; `SetParent(hwnd, workerw)`.
5. Moverla a `(0,0)` en coordenadas del `WorkerW`, con `SetWindowPos`.
6. Poner `WS_EX_NOACTIVATE` y quitar `WS_EX_APPWINDOW` para que no aparezca en Alt+Tab.
7. Vigilar que siga viva; si WE la cierra, relanzar.

Ventajas:

- Es API oficial y estable dentro de lo que WE documenta.
- No depende de `Progman`/`WorkerW`/`0x052C` para el attachment: nosotros hacemos el `SetParent`.
- Funciona con el `WorkerW` hijo que WinRust ya crea; el color-key actual de la lista sigue sirviendo.
- No requiere inyección ni captura de pantalla.
- Soporta scene, video, web y (probablemente) application wallpapers, porque es el mismo renderer de WE.

Límites (importantes):

- **No soporta playlists en la ventana.** El desarrollador de WE lo confirmó: *“wallpaper windows do not support the playlist functionality”* ([Steam, 2021](https://steamcommunity.com/app/431960/discussions/1/3156454576361774953/)). Para playlists hay que implementar el ciclo fuera (relanzar `openWallpaper` con cada elemento) o usar la ruta nativa.
- **Es una ventana, no la capa de escritorio.** No aparece en la vista previa de “Personalización”, no participa en el cambio de escritorio virtual como el wallpaper nativo y puede recibir foco/input si no se ajusta.
- **Una ventana por monitor.** Hay que gestionar títulos únicos y coordenadas; WE no ofrece un “span” en pop-out.
- **WE debe estar corriendo.** `-control` no arranca la app; si no está, el comando puede lanzarla y comportarse de forma distinta. El propio CLI dice “Make sure Wallpaper Engine is already running”.
- **DPI cross-process.** `SetParent` entre procesos fuerza un reset de DPI awareness del proceso hijo en Windows 10 1703+ ([Microsoft SetParent](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setparent)); WE ya hace `SetParent` contra Explorer, así que está acostumbrado, pero hay que probar 125%/150% y monitores con DPI distinto.
- **La ventana pop-out puede tener `WS_EX_LAYERED`/`WS_EX_NOREDIRECTIONBITMAP`.** Si al reparentar se ve negra, no insistir con GDI: pasar a la ruta nativa o a captura. En la topología clásica WE ya usa una ventana NRB+layered dentro de un `WorkerW` normal, así que hay base para que funcione; hay que verificarlo en 26300.

### 4.2 Enganche nativo clásico (soporte completo, más invasivo)

Consiste en reconstruir el escritorio como Explorer:

- `Progman` pasa a ser una ventana de fondo/oculta (puede seguir siendo el controlador y recibir `0x052C`).
- Se crea un `WorkerW` de nivel superior para los iconos, con `SHELLDLL_DefView` y `SysListView32` dentro.
- Se crea un `WorkerW` host de nivel superior justo debajo de la capa de iconos.
- La capa de iconos debe ser transparente: `WS_EX_LAYERED` + `LWA_COLORKEY` (o DComp). La lista de iconos deja de necesitar el color-key, o se mantiene como capa interna.
- `Progman` queda por debajo del host; puede pintar el BMP estático cuando no hay WE.

Ventajas: WE se engancha solo, con su lógica de multi-monitor, pausa, playlists y perfiles. Es el comportamiento “de Explorer”.

Costes: hay que mover la lista y el `WM_NOTIFY`/drop target al nuevo padre; cambiar `place_desk_list`, `drag_end`, `WM_CONTEXTMENU`, `WM_SIZE`, etc. Es un refactor grande y con riesgo de regresiones en iconos.

### 4.3 Enganche nativo raised (24H2+ “de verdad”)

Si WE en 26300 ignora la ruta clásica y exige la raised:

- `Progman` con `WS_EX_NOREDIRECTIONBITMAP`.
- `SHELLDLL_DefView` `WS_EX_LAYERED`.
- Superficie de wallpaper `WS_EX_LAYERED` con alpha 255, hija de `Progman`, por debajo de `DefView`, presentando por D3D11/DirectComposition.
- `WorkerW` hijo al fondo.

Esto implica reescribir el render del escritorio (y probablemente la lista de iconos) a DComp. No es razonable para el estado actual de `rust-explorer`, que es GDI. Recomendación: **no hacerlo** salvo que la fase 0 demuestre que WE no acepta la ruta clásica en 26300 y que el pop-out no es suficiente.

### 4.4 Captura (DWM thumbnail / PrintWindow / Windows.Graphics.Capture)

- **DWM thumbnail:** `DwmRegisterThumbnail` es eficiente y en vivo, pero **source y destination deben ser ventanas de nivel superior** ([Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmregisterthumbnail)). No sirve para pintar dentro de un `WorkerW` hijo. Habría que tener una capa de iconos de nivel superior y aun así es un parche visual, no integración.
- **PrintWindow:** pide a la ventana que se dibuje en un DC; con contenido D3D/DComp suele devolver negro o requerir `PW_RENDERFULLCONTENT`; es caro (readback CPU) y no da 60 fps.
- **Windows.Graphics.Capture + D3D11 shared texture:** es la captura moderna y eficiente, pero hay que crear un frame pool, presentar en una superficie DComp y lidiar con permisos/protección. Es un proyecto en sí mismo. Solo como último recurso si WE no coopera de ninguna otra forma.
- **Inyección/shim tipo `weshim`:** técnicamente funciona (lo hace we-linux), pero requiere DLL por arquitectura, parchear imports, sobrevivir actualizaciones de WE y puede chocar con anti-cheat/DRM. No recomendado para un shell de usuario final.

### 4.5 Decisión recomendada

| Si el objetivo es… | Vía | Por qué |
|---|---|---|
| Tener WE funcionando **ya**, con el escritorio actual | **4.1 CLI + reparent** | API oficial, sin tocar la topología, sin inyección; límites: sin playlists en ventana y es una ventana, no la capa nativa |
| “Soporte completo” con playlists/perfiles/pausa nativos | **4.2 nativo clásico** | WE se engancha solo y usa toda su lógica de escritorio; requiere refactor de la capa de iconos |
| Solo builds 24H2+ y se acepta reescribir el render | 4.3 raised + DComp | Es la topología “correcta” en 26H2, pero incompatible con el render GDI actual; mucho esfuerzo |
| WE no coopera de ninguna forma | 4.4 captura | Último recurso; límites de rendimiento/calidad y no es integración real |

**Regla práctica:** no invertir en 4.2/4.3 hasta que la fase 0 confirme qué ruta toma WE en 26300 y hasta que 4.1 esté probado en hardware real. La fase 1 entrega valor sin bloquear la fase 2.

---

## 5. Plan por fases para WinRust

### Fase 0 — Diagnóstico controlado (1–2 días)

Objetivo: saber **qué ruta toma WE en la build 26300** antes de escribir código de producción.

1. En una máquina con WE instalado, crear el escritorio WinRust y ejecutar WE manualmente.
2. Con Spy++/WinSpy, volcar:
   - `GetClassName`, `GetWindowText`, `GetParent`, `GetWindow(GW_CHILD/GW_HWNDNEXT)` de `Progman`, `WorkerW`, `SHELLDLL_DefView`.
   - `GetWindowLongPtrW(progman, GWL_EXSTYLE)`: ¿tiene `WS_EX_NOREDIRECTIONBITMAP`?
   - Jerarquía después de lanzar WE: ¿aparece un `WorkerW` nuevo? ¿es hijo de `Progman` o de nivel superior? ¿WE reparentó su ventana a `Progman` o a un `WorkerW`?
   - `GetShellWindow()`: ¿NULL?
   - Probar `FindWindowW(L"Progman", L"Program Manager")` y `FindWindowW(L"Progman", NULL)` contra el `Progman` de WinRust (`rs-desktop`): si la primera devuelve `NULL`, el título es el problema.
3. Instrumentación temporal en `rust-explorer` (solo debug, no producción):
   - log en `WM_SPAWN_WORKERW` de `wParam`/`lParam` y del árbol resultante;
   - log en `check_live` de los hijos de `Progman`/`WorkerW`;
   - log de `WM_SETTINGCHANGE` (`wParam`) y `WM_DISPLAYCHANGE`.
4. Probar también `-control openWallpaper -playInWindow` y ver si la ventana se puede reparentar y si se ve detrás de los iconos con el color-key actual.

Criterio de salida: una tabla “WE en 26300 usa ruta X; el pop-out se ve/no se ve; `GetShellWindow` es NULL/sí”. Sin esto, cualquier implementación nativa es a ciegas.

### Fase 1 — Puente CLI + reparent (3–5 días, recomendada)

**Objetivo:** WE visible detrás de los iconos en el escritorio actual, sin tocar la topología.

Archivos:

- Nuevo `plugins/rust-explorer/src/we.rs` (o submódulo `desk::we` si se quiere mantener todo en `main.rs`).
- `plugins/rust-explorer/src/main.rs`: llamar a `we::start()` en `WM_CREATE` de modo desktop; `we::tick()` en el timer de 2 s; `we::shutdown()` en `WM_DESTROY`.
- `plugins/rust-explorer/Cargo.toml`: no hacen falta crates nuevos; `windows-sys` ya tiene lo necesario (procesos, ventanas, threads). Si se lanza el proceso, añadir `Win32_System_Threading`/`Win32_System_Diagnostics_ToolHelp` ya presentes en el workspace; en el crate está `Win32_System_LibraryLoader`, no `Threading` explícito, pero se puede usar `std::process::Command` para no añadir features.

Lógica:

1. **Descubrir WE:** leer la ruta de Steam (`HKCU\Software\Valve\Steam\SteamPath` + `steamapps\common\wallpaper_engine\wallpaper64.exe`), o una ruta configurada. Si no existe, desactivar el módulo y seguir con el BMP estático.
2. **Asegurar proceso:** si no hay `wallpaper64.exe`/`wallpaper32.exe`, lanzarlo con `std::process::Command` (sin ventana, cwd = carpeta de WE). Si se quiere pasar por Steam, `launcher.exe -run wallpaper64.exe -nobrowse` (los launch options de Steam son `-run wallpaper64.exe -nobrowse`: [SteamDB](https://steamdb.info/app/431960/config/)). No ejecutar como admin.
3. **Leer la wallpaper activa:** `wallpaper64.exe -control getWallpaper` (documentado, aunque en algunas versiones devuelve vacío; tener fallback a `config.json`). Para el MVP, permitir fijar una ruta en ajustes de WinRust.
4. **Abrir pop-out por monitor:** `-control openWallpaper -file <ruta> -playInWindow "WinRust Wallpaper <n>" -width <w> -height <h> -x <x> -y <y> -borderless`.
5. **Esperar ventana:** `FindWindowW(NULL, title)` en bucle de 50 ms hasta 5 s; si falla, log y reintento.
6. **Reparentar:** `attach_popout(hwnd, workerw, x, y, w, h)` (snippet abajo). Ajustar estilos y `WS_EX_NOACTIVATE`.
7. **Vigilar:** cada 2 s, si `IsWindow(hwnd) == 0`, relanzar; si el `WorkerW` se recreó, reattach. No reparentar en cada tick (provoca parpadeo).
8. **Cierre:** en `WM_DESTROY`/logout, `-control closeWallpaper -location "WinRust Wallpaper <n>"`; si se quiere conservar el estado, `-control pause`.

Integración con el color-key actual:

- `desk::check_live` ya detecta un hijo dentro de `WORKERW`; al reparentar el pop-out, se activa `LIVE` y la lista se vuelve transparente. Hay que asegurarse de que el pop-out quede en el fondo del `WorkerW` (`SetWindowPos(..., HWND_BOTTOM, ...)`) y que la lista siga por encima.
- Si el pop-out no se ve pero `LIVE` está activo, probar `WS_EX_LAYERED` + `SetLayeredWindowAttributes(alpha=255)` en el pop-out, o `WS_CLIPCHILDREN` en el `WorkerW`. Si sigue negro, es señal de que la ventana es NRB/DComp y hay que pasar a la fase 2.

Criterio de aceptación:

- Wallpaper scene/video/web visible detrás de los iconos en 26300.
- Iconos clicables, drag & drop y menú contextual intactos.
- Sin entrada en Alt+Tab; sin robo de foco.
- Sobrevive a cambio de resolución y a reinicio de WE.
- CPU/GPU comparable a WE con Explorer (medir con el mismo wallpaper).

### Fase 2 — Enganche nativo clásico detrás de flag (1–2 semanas)

**Objetivo:** que WE se enganche solo, con playlists/perfiles/pausa nativos, sin pop-out.

Feature flag: `--desktop-native-we` o una clave en `%APPDATA%\rust-explorer\desktop.ini`; por defecto **off** hasta que la fase 0/1 valide.

Cambios estructurales:

1. **`Progman`**: mantenerlo como ventana de control de nivel superior, oculta o al fondo; título **`"Program Manager"`** (el canónico; si se prefiere `rs-desktop`, validar antes que WE no pasa título). Al cambiarlo, actualizar `rs-shell` para localizarlo por clase + PID/proceso, no por título (`rs-shell/src/main.rs:240`).
2. **Capa de iconos**: crear un `WorkerW` de nivel superior, `WS_POPUP | WS_VISIBLE`, `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, cubriendo el virtual screen. Dentro:
   - `SHELLDLL_DefView` hijo, tamaño completo, `WS_CHILD | WS_VISIBLE`, proc propio.
   - `SysListView32` hijo de `DefView` (mover la creación y el `WM_NOTIFY`).
3. **Host**: crear un `WorkerW` de nivel superior, `WS_POPUP | WS_VISIBLE`, `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, cubriendo el virtual screen.
4. **Orden Z:** `SetWindowPos(host, icon_layer, …)` para dejarlo justo debajo; `SetWindowPos(progman, host, …)` para dejarlo debajo del host. Reafirmar en `WM_DISPLAYCHANGE`, `WM_WINDOWPOSCHANGED` y `0x052C`.
5. **Transparencia:** la capa de iconos debe dejar ver el host. Opciones:
   - `WS_EX_LAYERED` + `LWA_COLORKEY` en la capa de iconos, con `WM_ERASEBKGND` pintando el key; la lista dentro con `CLR_NONE`.
   - En raised, `DefView` layered (como hace el shell) + una **superficie propia** layered con alpha 255 entre `DefView` y el `WorkerW` de fondo; eso ya implica DComp, no GDI.
   - Si el color-key produce halos en las etiquetas, mantener el color-key actual en la lista y hacer que el host sea el padre del `WorkerW` interno; pero la opción limpia es la capa de iconos transparente.
6. **Lista y notificaciones:** el `SysListView32` ahora pertenece a `DefView`; su `WM_NOTIFY` va a `DefView`, no a `Progman`. Hay que:
   - dar a `DefView` un proc que reenvíe `WM_NOTIFY`, `WM_COMMAND`, `WM_CONTEXTMENU`, `WM_DROPFILES` al `Progman`/controlador;
   - o crear la lista con `SetParent` a `DefView` y subclasificar `DefView` para reenviar.
   - Ajustar `place_desk_list`, `drag_end`, `icons_hidden`, `register_drop` y `bg_menu` para usar la nueva jerarquía.
7. **`0x052C`:** en el handler, garantizar host + orden Z; devolver 0. Aceptar cualquier `wParam`/`lParam` (WE usa `0xD/0x1` en builds nuevas; otros usan `0/0`).
8. **`SetShellWindow`:** llamar a `SetShellWindow(Progman)` (o `SetShellWindowEx(Progman, list)`) vía `GetProcAddress` en `shell32.dll`, con fallback silencioso. Es undocumented; probar que no rompe `rs-shell`.
9. **`check_live`:** considerar live si hay un hijo en el host **o** en `Progman` (según la ruta que use WE); si hay live, no pintar el BMP en `Progman`/host, o pintar solo el fallback.
10. **`rs-shell`:** no llamar a `SPI_SETDESKWALLPAPER` si WE está live. Añadir un chequeo (`FindWindowW("Progman", "rs-desktop")` + un flag compartido, o un mensaje `WM_APP` al desktop) y, si WE está live, solo actualizar el BMP y repintar; si no, el comportamiento actual. En la topología raised, `SPI_SETDESKWALLPAPER` puede destruir el `WorkerW` (FeatherWall), así que esto es importante incluso en la fase 1.

Criterio de aceptación:

- WE se engancha solo al arrancar (sin CLI), en 26300 y en una build clásica si se soporta.
- Playlists/perfiles y reglas de pausa de WE funcionan.
- Iconos, drag & drop, menú contextual y doble clic siguen funcionando.
- `SPI_SETDESKWALLPAPER` no rompe el wallpaper.
- Reinicio de WE, cambio de resolución y lock/unlock dejan el escritorio consistente.

### Fase 3 — Endurecimiento y multi-monitor (3–5 días)

- Multi-monitor: host y capa de iconos al virtual screen; coordenadas negativas (`SM_XVIRTUALSCREEN`), DPI por monitor, `WM_DPICHANGED`.
- HDR: WE puede usar HDR; el color-key GDI no entiende HDR. Si el usuario tiene HDR, validar; si falla, desactivar live wallpaper o usar la ruta DComp.
- Pausa: WE ya pausa con apps maximizadas/fullscreen; validar que el topbar de WinRust no se detecta como “maximized” y que una ventana de `rust-explorer` maximizada sí pausa.
- Lock/UAC: el wallpaper no se ve en la secure desktop; es esperado.
- Reinicio del shell: si `rust-explorer` muere y vuelve, WE puede quedar huérfano; al arrancar, cerrar pop-outs viejos (`closeWallpaper -location`) y relanzar, o reattach si la ventana sigue viva.
- Telemetría local: log en `%LOCALAPPDATA%\rust-explorer\we.log` con attach/reattach/errores, sin datos personales.

---

## 6. Snippets Rust/Win32

### 6.1 Reparent del pop-out de WE (fase 1)

```rust
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, SetParent, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, GWL_STYLE, HWND_BOTTOM, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WS_CAPTION, WS_CHILD, WS_EX_APPWINDOW,
    WS_EX_NOACTIVATE, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
};

/// Hace que una ventana top-level de WE pase a ser hija de `host`.
/// `x,y,w,h` son coordenadas de pantalla (el rectángulo del monitor).
pub unsafe fn attach_popout(hwnd: HWND, host: HWND, x: i32, y: i32, w: i32, h: i32) {
    // 1. Quitar decoración y marcar como child (SetParent no cambia los estilos solo).
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    let style = (style & !(WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU)) | WS_CHILD;
    SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);

    // 2. Fuera de Alt+Tab y sin activarse al mostrarse.
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    let ex = (ex | WS_EX_NOACTIVATE) & !WS_EX_APPWINDOW;
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);

    // 3. Reparentar.
    SetParent(hwnd, host);

    // 4. Pasar de coordenadas de pantalla a coordenadas del host.
    let mut pt = POINT { x, y };
    ScreenToClient(host, &mut pt);

    // 5. Al fondo del host, por debajo de la lista de iconos.
    SetWindowPos(
        hwnd,
        HWND_BOTTOM,
        pt.x,
        pt.y,
        w,
        h,
        SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
    );
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
}
```

> Nota: si el pop-out se ve negro, probar `WS_EX_LAYERED` + `SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA)` antes de descartar la vía. Si sigue negro, la ventana está usando DComp/NRB y el reparent GDI no es suficiente.

### 6.2 Host clásico mínimo (fase 2)

```rust
use windows_sys::Win32::Foundation::{HWND, HINSTANCE, RECT};
use windows_sys::Win32::UI::Controls::LVS_ICON;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_SPAWN_WORKERW: u32 = 0x052C;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Crea la jerarquía clásica: Progman (fondo), WorkerW de iconos + DefView,
/// y WorkerW host justo debajo de la capa de iconos.
unsafe fn create_classic_desktop(inst: HINSTANCE, r: RECT) -> (HWND, HWND, HWND) {
    let ex = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;

    let progman = CreateWindowExW(
        WS_EX_TOOLWINDOW,
        wide("Progman").as_ptr(),
        wide("rs-desktop").as_ptr(),
        WS_POPUP,
        r.left, r.top, r.right - r.left, r.bottom - r.top,
        0 as HWND, 0 as HMENU, inst, std::ptr::null_mut(),
    );

    // Capa de iconos: top-level, transparente (layered color-key o DComp).
    let icons = CreateWindowExW(
        ex,
        wide("WorkerW").as_ptr(),
        std::ptr::null(),
        WS_POPUP | WS_VISIBLE,
        r.left, r.top, r.right - r.left, r.bottom - r.top,
        0 as HWND, 0 as HMENU, inst, std::ptr::null_mut(),
    );

    let defview = CreateWindowExW(
        0,
        wide("SHELLDLL_DefView").as_ptr(),
        std::ptr::null(),
        WS_CHILD | WS_VISIBLE,
        0, 0, r.right - r.left, r.bottom - r.top,
        icons, 0 as HMENU, inst, std::ptr::null_mut(),
    );

    // La lista de iconos debe colgar de DefView, no de Progman.
    let _list = CreateWindowExW(
        0,
        wide("SysListView32").as_ptr(),
        std::ptr::null(),
        WS_CHILD | WS_VISIBLE | LVS_ICON,
        0, 0, r.right - r.left, r.bottom - r.top,
        defview, 0 as HMENU, inst, std::ptr::null_mut(),
    );

    // Host del wallpaper: top-level, justo debajo de la capa de iconos.
    let host = CreateWindowExW(
        ex,
        wide("WorkerW").as_ptr(),
        std::ptr::null(),
        WS_POPUP | WS_VISIBLE,
        r.left, r.top, r.right - r.left, r.bottom - r.top,
        0 as HWND, 0 as HMENU, inst, std::ptr::null_mut(),
    );

    // Orden Z: icons arriba, host debajo, progman al fondo.
    SetWindowPos(host, icons, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    SetWindowPos(progman, host, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);

    (progman, icons, host)
}

/// Handler en el WndProc de Progman.
unsafe fn on_spawn_workerw(progman: HWND, icons: HWND, host: HWND) -> LRESULT {
    // Si el host ya existe, solo reafirmar el orden Z.
    let _ = progman;
    SetWindowPos(host, icons, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    0
}
```

En el `match` del `WndProc`:

```rust
WM_SPAWN_WORKERW if desk() => {
    desk::on_spawn_workerw(h, desk::ICONS.get(), desk::HOST.get());
    0
}
```

(En el código real, `ICONS`/`HOST` irían en el `thread_local!` existente, junto a `WORKERW`.)

### 6.3 Detección de topología (fase 0/2)

```rust
const WS_EX_NOREDIRECTIONBITMAP: u32 = 0x0020_0000;

unsafe fn topology(progman: HWND) -> &'static str {
    let ex = GetWindowLongPtrW(progman, GWL_EXSTYLE) as u32;
    if ex & WS_EX_NOREDIRECTIONBITMAP != 0 {
        "raised" // Progman -> DefView (layered) + WorkerW hijo
    } else {
        "classic" // WorkerW top-level + DefView dentro
    }
}
```

---

## 7. Riesgos y mitigaciones

| Riesgo | Impacto | Mitigación |
|---|---|---|
| `0x052C` es undocumented y puede cambiar | Alto | Aislar en `desk::we`; feature flag; fallback a pop-out/BMP; logs; probar cada build de Windows. |
| WE actualiza y cambia clases/títulos | Medio | Buscar por PID + clase + título del pop-out; no hardcodear un solo class name; tolerar fallo. |
| Color-key `0x000101` produce agujeros | Bajo | Es un color casi imposible en contenido real; si preocupa, usar DComp o una máscara por región. |
| DWM/layered + D3D rendimiento | Medio | Medir GPU/CPU con el mismo wallpaper; evitar reparentar en cada tick; no usar `PrintWindow`. |
| Multi-monitor / DPI | Medio | Host al virtual screen; coordenadas negativas; `WM_DPICHANGED`; probar 125/150/200%. |
| WE pausa con apps maximizadas | Bajo/deseado | Validar que el topbar no se detecta como maximizado; usar “Pause per monitor” de WE. |
| `SPI_SETDESKWALLPAPER` destruye el host en raised | Alto | No llamarlo mientras WE esté live; actualizar BMP y repintar; o reattach después. |
| Ejecutar como admin rompe Steamworks/UIPI | Medio | No elevar WE ni el shell; misma integridad; documentar. |
| HDR | Medio | WE puede renderizar HDR; el color-key GDI no. Si HDR está activo y falla, desactivar live o usar DComp. |
| DRM/redistribución | Bajo | WE es DRM-free y no requiere Steam para wallpapers locales, pero **no** redistribuir sus archivos; el usuario debe tener su licencia. |
| Seguridad/secure desktop | Bajo | El wallpaper no se ve en UAC/lock; es esperado; no intentar saltarlo. |
| Inyección/shim | Alto | No inyectar en WE: frágil, por arquitectura, posible choque con anti-cheat; solo como experimento. |

### DRM / Steam / requisitos

- WE es **DRM-free** en la práctica: se puede ejecutar `wallpaper32.exe`/`wallpaper64.exe` directamente; Steam solo hace falta para Workshop ([WE Steamworks](https://help.wallpaperengine.io/en/steam/steamworks.html), [hilo oficial](https://steamcommunity.com/app/431960/discussions/2/598515617325204711/)). Existe `nosteam.txt` en `wallpaper_engine/bin` para desactivar la integración con Steam ([WE ingame](https://help.wallpaperengine.io/en/steam/ingame.html)).
- **No** hace falta el overlay de Steam ni que el proceso sea hijo de Steam.
- **No** hay un `--silent` documentado. El arranque silencioso se consigue con la opción de autostart de WE (“launch the application quietly in the background”: [WE automatic startup](https://help.wallpaperengine.io/en/functionality/automaticstartup.html)) o lanzando el `.exe` sin UI. Los launch options de Steam son `launcher.exe -run wallpaper64.exe -nobrowse` ([SteamDB](https://steamdb.info/app/431960/config/)).
- No ejecutar como administrador: rompe la comunicación Steamworks ([WE Steamworks](https://help.wallpaperengine.io/en/steam/steamworks.html)).
- WE usa la misma GPU que el escritorio/Explorer; si el shell y WE están en GPUs distintas, el rendimiento se degrada ([WE DWM](https://help.wallpaperengine.io/en/performance/dwm.html)).
- En 24H2 hay un bug conocido: si los iconos del escritorio están ocultos al arrancar, WE puede quedar invisible; requiere las KB de enero de 2025 y, en la práctica, WE 2.5/2.6+ ([aviso de WE](https://steamcommunity.com/app/431960/discussions/2/4702412991285452819/), [anuncios 2.6](https://steamcommunity.com/app/431960/announcements/)). WinRust debería exigir una versión mínima o detectar la build.

---

## 8. Validación recomendada

Matriz mínima antes de dar por bueno el soporte:

1. **Build 26300 (26H2)** y, si se puede, una 23H2/Windows 10 para la ruta clásica.
2. **Tipos de wallpaper:** scene (`project.json`/`scene.pkg`), video (`.mp4`), web (`index.html`), application wallpaper.
3. **Multi-monitor:** 1, 2 y 3 monitores; uno a la izquierda del primario (coordenada negativa); resoluciones y DPI distintos; modo “extender”, no “duplicar”.
4. **Ciclo de vida:** arranque del shell, reinicio de WE, cambio de wallpaper, cambio de resolución, lock/unlock, suspensión/reanudación, cierre de sesión.
5. **Interacción:** clic en icono, doble clic, drag & drop, menú contextual, selección múltiple, renombrar, borrar a papelera.
6. **Rendimiento:** GPU/CPU en idle y con una ventana maximizada; comparar con WE + Explorer en el mismo equipo.
7. **HDR:** activado/desactivado.
8. **Permisos:** shell y WE no elevados; probar también con UAC (el wallpaper no debe verse en la secure desktop, pero debe volver al volver).
9. **Steam:** con Steam abierto, cerrado y con `nosteam.txt`; verificar que no se requiere Workshop para wallpapers locales.

---

## 9. Fuentes

1. CodeProject — *Draw Behind Desktop Icons in Windows 8+* (2014): `0x052C`, `EnumWindows`, `FindWindowEx`, jerarquía clásica. https://www.codeproject.com/Articles/856020/Draw-Behind-Desktop-Icons-in-Windows-plus
2. Wallpaper Engine (Steam) — *Running without explorer.exe* (2017), respuesta del desarrollador: no hay inyección, solo hace falta la jerarquía de ventanas. https://steamcommunity.com/app/431960/discussions/0/144513128845344033/
3. Wallpaper Engine — *Command Line Controls*: `-control openWallpaper`, `-playInWindow`, `-borderless`; WE debe estar corriendo. https://help.wallpaperengine.io/en/functionality/cli.html
4. Wallpaper Engine — *Automatic start-up at boot*: arranque silencioso en segundo plano; modo high-priority como servicio. https://help.wallpaperengine.io/en/functionality/automaticstartup.html
5. Wallpaper Engine — *Steamworks is unavailable*: no ejecutar como admin; Steam debe comunicarse. https://help.wallpaperengine.io/en/steam/steamworks.html
6. Wallpaper Engine — *Steam always shows “in-game”* / `nosteam.txt`. https://help.wallpaperengine.io/en/steam/ingame.html
7. SteamDB — Wallpaper Engine launch options (`launcher.exe -run wallpaper64.exe -nobrowse`). https://steamdb.info/app/431960/config/
8. zHappySnake/we-linux — `weshim.c`: crea `Progman` + `WorkerW` + `SHELLDLL_DefView` y engancha las APIs de WE; demuestra que WE no necesita Explorer. https://github.com/zHappySnake/we-linux/blob/main/shim/weshim.c
9. lively-community/lively#2074 — respuesta de Microsoft sobre raised desktop, `WS_EX_NOREDIRECTIONBITMAP`, `DefView` layered y `WorkerW` hijo; detección por flag. https://github.com/lively-community/lively/issues/2074
10. Lively DeepWiki — *Desktop Core*: topologías clásica/raised, `EnsureWorkerWZOrder`, `SetWindowPos` bajo `DefView`. https://deepwiki.com/rocksdanister/lively/3.1-desktop-core
11. FeatherWall — *architecture.md*: en 24H2+ GDI/BitBlt/UpdateLayeredWindow no se componen; hace falta DirectComposition; `SPI_SETDESKWALLPAPER` destruye el `WorkerW`. https://raw.githubusercontent.com/RitvikDayal/featherwall/main/docs/architecture.md
12. DynamicWallpaper — *Setting a window as wallpaper*: secuencia `0x052C` con `0xD/0` y `0xD/1`; encontrar el `WorkerW` correcto. https://dynamicwallpaper.readthedocs.io/en/docs/dev/make-wallpaper.html
13. weebp — `weebp.c`: implementación C del mismo truco y fallback a `Progman`. https://github.com/Francesco149/weebp/blob/master/src/weebp.c
14. StackOverflow — *Display directly on screen. Work on Windows 10 not on Windows 11*: cita de Microsoft sobre raised desktop. https://stackoverflow.com/questions/79818519/display-directly-on-screen-work-on-windows-10-not-on-windows-11
15. Microsoft Learn — `SetParent`: no cambia `WS_CHILD`/`WS_POPUP`; reset de DPI cross-process. https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setparent
16. Microsoft Learn — `GetShellWindow`: devuelve `NULL` si no hay proceso shell. https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getshellwindow
17. Microsoft Learn — `DwmRegisterThumbnail`: source y destination deben ser top-level; destination del proceso llamador. https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmregisterthumbnail
18. ReactOS — `SetShellWindowEx`/`SetShellWindow` (undocumented, shell32): ejemplo de uso con `Progman` y el listview. https://reactos.org/archives/public/ros-diffs/2009-January/028360.html
19. Wallpaper Engine (Steam) — *Opening a Playlist to a Window* (2021): las ventanas pop-out no soportan playlists. https://steamcommunity.com/app/431960/discussions/1/3156454576361774953/
20. Wallpaper Engine — *Desktops with Integrated and Dedicated GPU Perform Poorly (dwm.exe)*: WE forma parte del escritorio y debe usar la misma GPU. https://help.wallpaperengine.io/en/performance/dwm.html
21. Wallpaper Engine (Steam) — *Windows 11 24H2 - Wallpaper Engine not visible* (2024–2025): bug de iconos ocultos en 24H2; fixes de Microsoft/WE 2.6. https://steamcommunity.com/app/431960/discussions/2/4702412991285452819/
22. Wallpaper Engine — anuncios 2.5/2.6: compatibilidad con los cambios de escritorio de Windows 11 24H2. https://steamcommunity.com/app/431960/announcements/
23. Steam (2025) — observación de que la ventana de contenido de WE usa `WS_EX_NOREDIRECTIONBITMAP` + `WS_EX_LAYERED`. https://steamcommunity.com/app/431960/discussions/2/571540483751323871/
24. slab/wallpaper README — en 24H2 el `0x052C` correcto es `wParam=0xD, lParam=1`; el `(0xD,0)`/`(0,0)` puede destruir el `WorkerW`. https://tangled.org/aesthetic.computer/core/blob/c70b9eac6a5589fb07c5606f96557a41c40017ce/slab/windows/wallpaper/README.md
