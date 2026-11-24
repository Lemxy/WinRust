# Auditoría i18n de WinRust

**Fecha de la instantánea:** 2026-11-24 02:27 (hora local)
**Alcance:** todo el árbol `C:\winrust-build` (workspace Rust), sin compilar.
**Herramienta:** `tools/i18n-check.ps1` (PowerShell 5.1, sin dependencias, solo lectura).

> `common/es.tsv` estaba siendo reescrito por otro proceso durante la auditoría. Las cifras y
> líneas de este informe son una foto fija de las 02:27; al volver a ejecutar el script pueden
> cambiar (sobre todo los duplicados y las entradas muertas).

---

## Resumen ejecutivo

| Métrica | Valor |
|---|---:|
| Ficheros `.rs` (sin `target/`) | 149 |
| Ficheros con `t!`/`tf!` | 71 |
| Llamadas `t!`/`tf!` | **1681** (`t!` 1371, `tf!` 310) |
| Claves inglesas distintas usadas | 1321 |
| Llamadas sin algún idioma (`ja`/`pt`/`zh`) | **69** |
| Marcadores de formato descuadrados | **0** |
| Claves inglesas sin entrada en `es.tsv` | **30** |
| Entradas de `es.tsv` no usadas | **11** |
| Duplicados en `es.tsv` | **70** (61 exactos + 9 solo por mayúsculas) |
| Líneas mal formadas en `es.tsv` | **1** |
| Español idéntico al inglés | 31 (casi todas marcas/unidades) |
| Llamadas no comprobables (textos variables) | 4 |
| Código de salida del script | **1** (hay problemas) |

**Conclusión rápida:** no hay ni un solo desajuste de marcadores de formato y no hay llamadas con
argumentos mal formados. Los problemas reales son (a) 30 claves sin español; muchas son solo
diferencias de mayúsculas o de puntos suspensivos con una entrada que ya existe, y por eso el
español cae al inglés; (b) 69 llamadas a las que les faltan idiomas (casi todas en `topbar`);
(c) un gran bloque duplicado en `common/es.tsv` que además pisa traducciones buenas
("Settings" → "Configuración" en vez de "Ajustes"); y (d) la variante de portugués: el proyecto es
**pt-BR de facto**, no pt-PT.

---

## 1. Cobertura de `t!` / `tf!`

### 1.1. Llamadas a las que les falta algún idioma: 69

La macro no falla: si falta `ja:`, `pt:` o `zh:`, esos idiomas muestran el inglés (ver
`common/src/lib.rs`, ramas 2–4 de `t!`). El español **no** se ve afectado, porque se busca por la
clave inglesa en `es.tsv` con independencia de los demás idiomas.

| Crate | Llamadas incompletas | Detalle |
|---|---:|---|
| `plugins/topbar` | 66 | 44 en `settingswin.rs` (sin `ja/pt/zh`), 16 en `start7.rs`, 3 en `start10.rs`, 3 en `main.rs` (solo falta `zh`) |
| `common` | 2 | `common/src/lib.rs:257-259`, **solo tests** (`#[cfg(test)]`); no es UI |
| `plugins/rust-settings` | 1 | `main.rs:1487` "Wallpaper changed" (solo falta `zh`) |

Casos concretos más visibles de `topbar`:

- `plugins/topbar/src/settingswin.rs:86-174` — toda la ventana de ajustes de la barra: "Appearance",
  "Start menu", "Blur", "Bar", "Language", "Theme", "Position", "Size", "Behavior"… Solo ruso e
  inglés; en japonés, portugués y chino se ve inglés.
- `plugins/topbar/src/start7.rs:166-176,225-297` — "Documents", "Pictures", "Music", "This PC",
  "Control Panel", "Settings", "Devices and printers", "Shut down"…
- `plugins/topbar/src/start10.rs:188-213` — "Recently added", "Pinned", "No apps found".
- `plugins/topbar/src/main.rs:472,2751,2752` — "Task View", "Adjust date and time",
  "Notification settings" (les falta solo `zh`).
- `plugins/rust-settings/src/main.rs:1487` — "Wallpaper changed" (falta `zh`).

Nota: `common/src/lib.rs:257-259` son aserciones de test (`t!("да","yes")`,
`tf!("{} шт","{} pcs",3)`); conviene excluirlas al contar cobertura de UI.

### 1.2. Marcadores de formato: 0 desajustes

Se compararon los 310 `tf!` entre ruso, inglés, japonés, portugués, chino y la plantilla española
de `es.tsv` (número de huecos y secuencia de especificaciones `{:.0}`, `{:>8}`, etc.). Resultado:
**todos cuadran**.

- `{name}` puede cambiar de orden entre idiomas sin romper nada (`format!` los resuelve por
  nombre); por eso no se marcan como error los reordenamientos de `rtaskmgr-win32/src/app.rs`.
- `t!` no formatea: los `{...}` de un `t!` son texto literal. Por eso no se auditan como marcadores.

**Defecto visible relacionado (no es un marcador):** en
`plugins/rust-search/src/bin/rs-launcher.rs:1461` el ruso, el japonés y el portugués escriben
`"{21B5} выполнить…"` dentro de un `t!`; como `t!` no interpreta `\u{21B5}`, el usuario ve
literalmente `{21B5}` en lugar de `↵`. El inglés y el chino sí usan el carácter. Está en
`rust-search` (fuera de lo que puedo editar): **pendiente**.

### 1.3. Claves inglesas sin entrada en `es.tsv`: 30

La búsqueda es **sensible a mayúsculas**, igual que el `HashMap` de `es.rs`. Por eso muchas
"faltan" aunque exista una entrada parecida: el español cae al inglés y no se nota en los tests.

| # | Fichero:línea | Clave inglesa | Por qué falta |
|---:|---|---|---|
| 1 | `common/src/lib.rs:257` | `yes` | solo test; existe `Yes` |
| 2 | `common/src/lib.rs:259` | `yes` | solo test; existe `Yes` |
| 3 | `src/install/mod.rs:158` | `removed` | existe `Removed` |
| 4 | `src/ui.rs:534` | `Installed` | existe `installed` |
| 5 | `src/ui.rs:642` | `Failed` | solo existe `FAILED` |
| 6 | `src/ui.rs:643` | `Installed` | existe `installed` |
| 7 | `src/ui.rs:798` | `Error: {e}` | existe `error: {e}` |
| 8 | `src/ui.rs:890` | `Error: {e}` | existe `error: {e}` |
| 9 | `plugins/debloat/src/main.rs:614` | `off` | existe `Off` |
| 10 | `plugins/purge/src/main.rs:149` | `removed` | existe `Removed` |
| 11 | `plugins/purge/src/main.rs:192` | `\nThis REMOVES the apps above FOR GOOD, there is no undo. Type YES to continue: ` | entrada 947 sin el `\n` inicial |
| 12 | `plugins/purge/src/main.rs:202` | `\nPress Enter to close.` | entrada 720 sin el `\n` inicial |
| 13 | `plugins/rust-control/src/pages/network.rs:68` | `Loopback` | nueva |
| 14 | `plugins/rust-control/src/pages/network.rs:69` | `Tunnel` | nueva |
| 15 | `plugins/rust-control/src/pages/network.rs:71` | `Type {}` | nueva |
| 16 | `plugins/rust-control/src/pages/security.rs:55` | `netsh exited with {}` | nueva |
| 17 | `plugins/rtaskmgr-win32/src/boost.rs:185` | `off` | existe `Off` |
| 18 | `plugins/rtaskmgr-win32/src/data.rs:354` | `SYSTEM` | existe `System` (otro significado: usuario del proceso) |
| 19 | `plugins/rtaskmgr-win32/src/main.rs:63` | `ERROR: {e}` | existe `error: {e}` |
| 20 | `plugins/rust-explorer/src/main.rs:157` | `Local Disk` | existe `Local disk` |
| 21 | `plugins/rust-explorer/src/main.rs:158` | `Network Drive` | nueva |
| 22 | `plugins/rust-explorer/src/main.rs:1350` | `Could not move to the Recycle Bin.\n\n{}\n\nDelete permanently?` | nueva |
| 23 | `plugins/rust-explorer/src/ui.rs:1298` | `Local Disk` | existe `Local disk` |
| 24 | `plugins/rust-settings/src/bt.rs:315` | `Searching…` | existe `searching.` (minúscula y sin `…`) |
| 25 | `plugins/rust-settings/src/bt.rs:330` | `{} devices found` | nueva |
| 26 | `plugins/rust-settings/src/bt.rs:383` | `Could not remove: {}` | existe `could not remove: {}` |
| 27 | `plugins/rust-settings/src/main.rs:630` | `none` | existe `None` |
| 28 | `plugins/topbar/src/fly.rs:1213` | `Uninstall "{}"?` (comillas rectas) | nueva; existe la frase larga `Uninstall "{}"? Its own uninstaller…` |
| 29 | `plugins/topbar/src/main.rs:2628` | `Start menu…` (U+2026) | existe `Start menu` sin puntos suspensivos |
| 30 | `plugins/topbar/src/settings.rs:138` | `Where it opens and how it looks` | nueva |

Buena parte de estas 30 no son traducciones nuevas: son **el mismo texto con otra capitalización o
con `…`**. La solución preferible en los ficheros que sí se pueden tocar es igualar el literal
inglés a la clave que ya existe; en los que no, se añade la línea nueva (sección 6). Las que
realmente necesitan una traducción nueva son las marcadas "nueva", las dos de `purge` con `\n`
inicial y `Failed`, `SYSTEM` y `Searching…`; el resto son variantes de mayúsculas.

No he tocado `es.tsv`; las líneas exactas para añadir están en la sección 6.

### 1.4. Textos visibles sin `t!`

Lista de los más evidentes (sin contar mensajes de depuración ni rutas/identificadores internos):

| Fichero:línea | Texto | Estado |
|---|---|---|
| `plugins/rust-control/src/sys.rs:104` | `"<too long>"` (valor de variable de entorno demasiado larga) | **pendiente** (lo dejo por ser casi inalcanzable) |
| `plugins/rust-control/src/sys.rs:117` | `format!("<{} bytes>", len)` (valor binario) | **pendiente** (idem) |
| `plugins/rust-search/src/main.rs:202` | `"cannot open {drive}: (error {}). Run from an elevated (admin) shell; volume must be NTFS."` | pendiente (fuera de rust-control) |
| `plugins/rs-shell/src/main.rs:449-471` | Mensajes de consola `--dry-run`, `--enable`, `--disable`, `usage:` | pendiente (CLI completo en inglés) |
| `plugins/rs-notify/src/main.rs:67-125` | Errores y `USAGE` de la CLI | pendiente (CLI completo en inglés) |
| `plugins/rust-search/src/bin/rs-launcher.rs:1461` | `{21B5}` literal en ru/ja/pt | pendiente |

En `rust-control` siguen sin `t!` solo las marcas y acrónimos que no se traducen:
`"Windows"`, `"Microsoft Defender"`, `"CPU"`, `"GPU"`, `"Ethernet"`, `"Wi-Fi"`, `"PPP"`, `"—"`.

---

## 2. `common/es.tsv`

### 2.1. Estado del fichero

- 1377 líneas reales (1378 al contar el salto final), **1315 claves únicas sensibles a mayúsculas**
  (1306 si se ignoran mayúsculas).
- Finales de línea **mezclados**: 1062 con CRLF y 315 solo LF. El cargador (`es.rs`, `.lines()`)
  quita el `\r`, así que funciona, pero conviene normalizar a LF para que los diffs sean limpios.
- **Línea 1 mal formada:** contiene solo un tabulador (`\t`), es decir, clave vacía. `es.rs` la
  ignora (`if !en.is_empty()`), pero sobra.

### 2.2. Duplicados: 70 hallazgos = 61 líneas exactas duplicadas + 9 parejas que solo cambian mayúsculas

`es.rs` inserta en un `HashMap`, así que **la última línea gana**. El bloque nuevo que empieza
sobre la línea 1066 duplica muchas claves del bloque antiguo. En 6 líneas exactas el español
también cambia (5 claves distintas):

| Clave | Primera línea / valor | Línea posterior / valor | Efecto real |
|---|---:|---:|---|
| `Settings` | 827 · **Ajustes** | 1082 · Configuración | La UI muestra "Configuración" |
| `Settings` | 827 · **Ajustes** | 1308 · Configuración | La UI muestra "Configuración" |
| `Disabled` | 341 · **Desactivado** | 1205 · Deshabilitado | La UI muestra "Deshabilitado" |
| `Start` | 868 · **Inicio** | 1318 · Iniciar | Matiz: sustantivo → verbo |
| `Nothing here yet` | 638 · **Aún no hay nada aquí** | 1091 · Aún no hay nada | Se pierde "aquí" |
| `Buttons on the left, like Windows 10` | 209 · **…como en Windows 10** | 1115 · …como Windows 10 | Se pierde "en" |

Parejas que **solo** difieren en mayúsculas (no son duplicados reales para Rust, pero el encargo
pide señalarlas porque una de las dos suele estar muerta o sin usar):

`Computer Management`/`Computer management`, `Date and Time`/`Date and time`,
`enabled`/`Enabled`, `Filter keys`/`Filter Keys`, `on`/`On`, `on battery`/`On battery`,
`plugged in`/`Plugged in`, `Sticky keys`/`Sticky Keys`, `unknown`/`Unknown`.

Recomendación: al terminar la fusión, conservar **la primera** redacción de los duplicados exactos
(salvo que se decida lo contrario de forma consciente) y borrar el bloque duplicado; para las
parejas de mayúsculas, unificar el literal inglés en el fuente para que solo quede una clave.
Mientras tanto, cualquier prueba visual de "Settings" o "Disabled" dará el valor nuevo, no el
histórico.

### 2.3. Entradas no usadas: 11

Detección conservadora: si una clave aparece como literal en una tabla estática que luego se pasa
a `t!(ru, en, …)`, **no** se cuenta como muerta (el auditor no puede seguir esa clave). Por eso
"File Explorer", "Lock", "Sleep", "Restart" o "Power Options" ya no salen aquí.

| Línea | Clave | Comentario |
|---:|---|---|
| 398 | `english` | Resto de la documentación/plantilla; no se usa |
| 399 | `english {}` | Resto de la documentación/plantilla; no se usa |
| 523 | `Look` | Antigua etiqueta de topbar |
| 524 | `Look of the bar and the WinRust programs` | Antigua etiqueta de topbar |
| 714 | `Position, size, behavior` | Antiguo encabezado |
| 947 | `This REMOVES the apps above FOR GOOD, …: ` | La llamada real lleva `\n` inicial (ver 1.3) |
| 952 | `Tile the windows` | Función retirada |
| 1084 | `Shut down the computer` | La tabla usa "Shut down" |
| 1172 | `Computer management` | Sustituida por "Computer Management" en rust-control |
| 1195 | `Date and time` | Sustituida por "Date and Time" en rust-control |
| 1370 | `Show notifications` | Añadida pero aún no conectada |

### 2.4. Español idéntico al inglés: 31

Casi todas son marcas, acrónimos o unidades: `Edge`, `OneDrive`, `Bluetooth`, `Terminal`,
`Firewall`, `Windows Update`, `Mbps`, `NTSTATUS 0x{:08X}`, etc. No requieren traducción.
Solo `stub` (línea 889) parece un texto de estado que quizá debería traducirse; no es urgente.

---

## 3. Terminología y glosario

### 3.1. Glosario propuesto

Para español se propone el uso de Windows en español (el que ya sigue el proyecto). Para portugués
se dan **las dos variantes**: los datos dicen que el proyecto usa pt-BR, aunque el encargo pedía
pt-PT (ver 3.3).

| Inglés | Español (usar siempre) | Portugués actual (pt-BR) | pt-PT (si se migra) | Japonés | Chino |
|---|---|---|---|---|---|
| explorer / file manager | **Explorador de archivos** | Explorador de Arquivos | Explorador de Ficheiros | エクスプローラー | 文件资源管理器 |
| folder | **carpeta** | pasta | pasta | フォルダー | 文件夹 |
| file | **archivo** | arquivo | ficheiro | ファイル | 文件 |
| drive | **unidad** | unidade | unidade | ドライブ | 驱动器 |
| volume | **volumen** | volume | volume | ボリューム | 卷 |
| free space | **espacio libre** | espaço livre | espaço livre | 空き容量 | 可用空间 |
| Recycle Bin | **Papelera de reciclaje** | Lixeira | Lixeira | ごみ箱 | 回收站 |
| Quick access | **Acceso rápido** | Acesso rápido | Acesso rápido | クイック アクセス | 快速访问 |
| This PC | **Este equipo** | Este Computador | Este PC | この PC | 此电脑 |
| tab | **pestaña** | aba | separador | タブ | 选项卡 |
| pinned | **anclado** | fixado | afixado | ピン留め | 已固定 |
| settings | **Ajustes** (ventana) / configuración (genérico) | Configurações | Definições | 設定 | 设置 |
| task manager | **Administrador de tareas** | Gerenciador de tarefas | Gestor de tarefas | タスク マネージャー | 任务管理器 |
| uninstall | **desinstalar** | desinstalar | desinstalar | アンインストール | 卸载 |
| notifications | **notificaciones** | notificações | notificações | 通知 | 通知 |
| blur | **desenfoque** | desfoque | desfoque | ぼかし | 模糊 |
| wallpaper | **fondo de pantalla** | papel de parede | papel de parede | 壁紙 | 壁纸 |
| shortcut | **acceso directo** | atalho | atalho | ショートカット | 快捷方式 |

### 3.2. Español: lo que está bien y las desviaciones

Bien (recuento sobre los valores españoles de `es.tsv`):

- `carpeta` 20 usos, `directorio` 0.
- `unidad` 5 usos; ningún caso de clave inglesa con *drive* traducida como "disco".
- `Explorador de archivos` 1, `gestor de archivos` 0.
- `Papelera de reciclaje` 9, sin variantes cortas.
- `Este equipo` 1; `Acceso rápido` 3 (siempre con "rápido" en minúscula, correcto).
- `Administrador de tareas` 6; `barra de tareas` 2; `fondo de pantalla` 2; `desenfoque` 6.

Desviaciones reales:

1. **`Pinned` → `Fijados`** (`es.tsv:1086`) frente a `Anclado · {}` (`707`), `Anclado · no en
   ejecución` (`708`), `Anclar a Acceso rápido` (`704`) y `Anclar a la barra` (`705`). Debe ser
   **Anclados/Anclado**.
2. **`Settings` → `Configuración`** por el duplicado nuevo (ver 2.2), cuando el proyecto usa
   **Ajustes** en 15 entradas. "Configuración" es válido para "Setup" o "system configuration",
   pero no para la ventana *Settings*.
3. **`Disabled` → `Deshabilitado`** por el duplicado nuevo; el histórico y más natural en Windows
   es **Desactivado**. Lo mismo con la pareja `enabled`/`Enabled`: "activado" frente a
   "Habilitado"; debe quedar **Activado**.
4. **`Start` → `Iniciar`** (verbo) frente a **Inicio** (sustantivo) del bloque antiguo.
5. **`Sticky Keys` → `Teclas de adhesión`** frente a `Sticky keys` → **Teclas pegajosas**. Windows
   usa "Teclas especiales" o "Teclas de filtrado", pero dentro del proyecto hay que elegir una.
   Recomiendo mantener la histórica "Teclas pegajosas".
6. Parejas que solo cambian mayúsculas y no cambian el significado: `on`/`On`, `on battery`/`On
   battery`, `plugged in`/`Plugged in`, `unknown`/`Unknown`, `Filter keys`/`Filter Keys`. No son
   errores de traducción, pero conviene dejar una sola forma por concepto.
7. `Atajos de teclado` (`es.tsv:1339`) frente al término del glosario "acceso directo": aquí
   *shortcut* significa atajo de teclado, así que **es correcto**; no cambiar.

### 3.3. Portugués: el proyecto es pt-BR, no pt-PT

Recuento sobre todos los textos `pt:` de `t!`/`tf!`:

| Pareja | pt-PT | pt-BR |
|---|---:|---:|
| ficheiro / **arquivo** | 0 | 36 |
| ecrã / **tela** | 0 | 20 |
| registo / **registro** | 0 | 18 |
| gestor / **gerenciador** | 0 | 12 |
| palavra-passe / **senha** | 0 | 13 |
| rato / **mouse** | 0 | 5 |
| transferir / **baixar** | 0 | 1 |
| equipa / equipe | 0 | 0 |

No hay ni una sola forma pt-PT. Además `common/src/lib.rs:4` documenta "Portuguese (Brazil)" y
existe `README.pt-BR.md`. Conclusión: **la premisa pt-PT no se cumple**; el estándar real es
pt-BR. Hay dos opciones coherentes:

- **A (recomendada, coste bajo):** declarar pt-BR como canónico, corregir la documentación que
  diga pt-PT y no introducir formas pt-PT aisladas.
- **B (coste alto):** migrar todo el portugués a pt-PT (más de 100 cadenas) en una sola pasada;
  hacerlo solo en algunos ficheros crearía más incoherencia de la que hay.

En `rust-control` he mantenido pt-BR ("Tela", "Editor do Registro", "Gerenciador") por coherencia
con el resto; solo he corregido una mezcla interna (ver 4.2).

### 3.4. Ruso, japonés y chino

- No hay `ja:` ni `zh:` vacíos, ni texto español/portugués colado (0 avisos de acentos).
- No hay ruso sin cirílico.
- `ja: "PC"` en `rust-explorer` (`main.rs:148`, `ui.rs:1326`) es correcto: Windows japonés llama
  "PC" a *This PC*.
- Mezcla de caracteres chinos `账户` / `帐户` para "account" en el resto del repo (p. ej.
  `pages/home.rs` usaba `帐户` y `pages/mod.rs` `账户`). En `rust-control` ya está unificado en
  `账户`.
- El bloque nuevo de `es.tsv` introduce japonés/chino correctos en general; no se han detectado
  traducciones automáticas incoherentes graves, más allá de las variantes ya citadas.

---

## 4. `plugins/rust-control`

### 4.1. Cobertura

- **291 llamadas** `t!`/`tf!`, **236 claves inglesas distintas** (el encargo hablaba de ~235).
- 232 claves ya existían en `es.tsv` con la clave inglesa exacta; **4 son nuevas** y están en la
  lista de la sección 6.
- No hay llamadas con marcadores descuadrados ni argumentos mal formados.
- Todos los textos visibles de las páginas usan `t!`/`tf!` salvo los dos casos casi inalcanzables
  de `sys.rs` (`<too long>`, `<{} bytes>`) y las marcas/acrónimos.

### 4.2. Arreglos aplicados (solo `plugins/rust-control/**`)

1. `src/sys.rs` — `error_text(0)` devolvía `"unknown error"` fijo en inglés; ahora
   `t!("неизвестно", "unknown", …)`.
2. `src/sys.rs` — `format_size`: el separador decimal sigue al idioma (coma en ES/RU/PT, punto en
   EN/JA/ZH) usando `dot!()`. Antes siempre `1.5 GB`.
3. `src/sys.rs` — `format_date`: `YYYYMMDD` se muestra como `DD/MM/YYYY` (ES/PT),
   `DD.MM.YYYY` (RU) o `YYYY-MM-DD` (EN/JA/ZH). Antes siempre ISO.
4. `src/sys.rs` — `duration_text`: las unidades (`d/h/min` → `д/ч/м`, `日/時間/分`,
   `天/小时/分`, `d/h/min`) siguen al idioma. Antes siempre `3d 4h 12m`.
5. `src/pages/system.rs` — `architecture()` devolvía `"unknown"`; ahora `t!(…)`.
6. `src/pages/network.rs` — `if_type_name`: `Loopback`, `Tunnel` y `Type {t}` estaban fijos en
   inglés; ahora `t!`/`tf!`. `Ethernet`, `Wi-Fi` y `PPP` se dejan como nombres técnicos.
7. `src/pages/security.rs` — el error de `netsh` (`netsh exited with {}`) estaba fijo en inglés;
   ahora `tf!` con los cinco idiomas.
8. `src/pages/tools.rs` — la sección decía `"Computer management"` y la entrada
   `"Computer Management"`; unificado en **`"Computer Management"`** (una clave menos duplicada).
9. `src/pages/datetime.rs` — la sección decía `"Date and time"` y el título de página
   `"Date and Time"`; unificado en **`"Date and Time"`**. Además `stamp()` ahora usa
   `sys::format_date`, así que la fecha local/UTC sigue el orden del idioma.
10. `src/pages/home.rs` — el portugués mezclaba pt-PT y pt-BR en la misma frase
    ("cópias de segurança … arquivos … Histórico de Arquivos"); queda **"cópias de backup dos
    arquivos com o Histórico de Arquivos"** (pt-BR coherente).
11. `src/pages/home.rs` — chino `用户帐户` → **`用户账户`**, igual que el resto del crate.

No he modificado `common/es.tsv`, `common/src/es.rs`, `common/src/look.rs`, `plugins/topbar/**`,
`plugins/rust-explorer/**`, `Cargo.toml` ni `payload.txt`. El cargador `es.rs` y las tablas de
idioma (`Lang` en `common/src/lib.rs`) no tenían ningún fallo real, así que no hacía falta tocarlos.

### 4.3. Pendiente dentro de rust-control

- `"<too long>"` y `"<{} bytes>"` de `sys.rs` (valores de registro poco frecuentes).
- `pt: "Tela"` / `"Editor do Registro"` se mantienen a propósito mientras el proyecto sea pt-BR.

---

## 5. Fechas, números y plurales

- **rust-control:** corregido (fechas, decimales y unidades de duración siguen el idioma).
- **`plugins/rtaskmgr-win32`:** ya usaba `dot!()` para los decimales; correcto.
- **`plugins/topbar`:** usa `GetDateFormatW`/`GetTimeFormatW` con `LOCALE_USER_DEFAULT` para el
  reloj; correcto.
- **`plugins/rust-explorer` (pendiente, no editable):** `size_text` en `src/main.rs:504-506`
  escribe siempre `"{:.1} MB"` / `"{:.2} GB"` con punto, y `date_text` en `main.rs:509-517` fija
  `YYYY-MM-DD HH:MM`. En español/ruso/portugués debería usar coma decimal y el orden local.
- **`plugins/rust-search/src/bin/rs-launcher.rs:940-946`:** mismo problema de decimal con punto.
- **Plurales:** las macros no tienen plurales. Abundan fórmulas tipo `item(s)`, `item(ns)`,
  `{} elemento(s)`. No es un fallo de formato, pero conviene una pasada de redacción para evitar
  paréntesis en la UI (p. ej. en `rs-du`, `purge`, `rtaskmgr`).

---

## 6. Entradas para `common/es.tsv`

Las 30 llamadas sin español corresponden a **24 claves únicas** (hay 6 claves repetidas en dos
ficheros distintos). Van con la línea exacta `English<TAB>español` (hay un **tabulador real** entre
las dos columnas). Los `\n` son texto literal, como en el fuente Rust.

```tsv
yes	sí
removed	eliminado
Installed	Instalado
Failed	Fallido
Error: {e}	Error: {e}
off	desactivado
\nThis REMOVES the apps above FOR GOOD, there is no undo. Type YES to continue: 	\nEsto ELIMINA las aplicaciones anteriores DEFINITIVAMENTE; no hay forma de deshacerlo. Escribe YES para continuar: 
\nPress Enter to close.	\nPulsa Enter para cerrar.
Loopback	Bucle invertido
Tunnel	Túnel
Type {}	Tipo {}
netsh exited with {}	netsh terminó con el código {}
SYSTEM	SISTEMA
ERROR: {e}	ERROR: {e}
Local Disk	Disco local
Network Drive	Unidad de red
Could not move to the Recycle Bin.\n\n{}\n\nDelete permanently?	No se pudo mover a la Papelera de reciclaje.\n\n{}\n\n¿Eliminar permanentemente?
Searching…	Buscando…
{} devices found	{} dispositivos encontrados
Could not remove: {}	No se pudo quitar: {}
none	ninguno
Uninstall "{}"?	¿Desinstalar “{}”?
Start menu…	Menú Inicio…
Where it opens and how it looks	Dónde se abre y cómo se ve
```

Observaciones:

- `yes`, `removed`, `Installed`, `Error: {e}`, `ERROR: {e}`, `off`, `Local Disk`,
  `Could not remove: {}` y `none` son variantes de mayúsculas de claves que ya existen (`Yes`,
  `Removed`, `installed`, `error: {e}`, `Off`, `Local disk`, `could not remove: {}`, `None`).
  Si se prefiere no engordar `es.tsv`, la alternativa es **igualar el literal en el fuente** a la
  clave existente; en ese caso no hace falta añadir la línea. Mientras los fuentes no se toquen,
  la línea nueva es lo que evita que el español caiga al inglés.
- La línea de `\nThis REMOVES…` lleva **espacio final** después de `continuar: `, igual que la
  clave inglesa.
- Las claves `\nThis…` y `\nPress…` llevan `\n` inicial; no las fusiones con las líneas 947 y 720
  existentes (esas no lo llevan). La alternativa es quitar el `\n` inicial de los literales en
  `plugins/purge/src/main.rs:192,202` y reutilizar las entradas actuales.
- `SYSTEM` va en mayúsculas a propósito: es el nombre de usuario que se muestra para los procesos
  del sistema (Windows en español muestra "SISTEMA").
- `Uninstall "{}"?` usa comillas rectas en inglés; el valor español puede usar comillas
  tipográficas.
- `Start menu…` y `Searching…` usan el carácter `…` (U+2026), no tres puntos.

Además, para el bloque duplicado de `es.tsv` (sección 2.2), los valores que hay que **conservar**
en los duplicados exactos son los históricos: `Settings=Ajustes`, `Disabled=Desactivado`,
`Start=Inicio`, `Nothing here yet=Aún no hay nada aquí` y
`Buttons on the left…=Botones a la izquierda, como en Windows 10`.

---

## 7. Pendiente en ficheros que no podía editar

1. `common/es.tsv` (otro agente): eliminar duplicados conservando los valores históricos,
   normalizar CRLF/LF, borrar la línea 1 vacía y añadir las 24 entradas de la sección 6.
2. `plugins/topbar/**`: completar `ja/pt/zh` en `settingswin.rs` (44), `start7.rs` (16),
   `start10.rs` (3) y los 3 `zh` de `main.rs`; añadir las 3 claves faltantes de `es.tsv`.
3. `plugins/purge/src/main.rs`: añadir las 2 claves con `\n` inicial (o quitar el `\n`).
4. `plugins/rust-explorer/**` (lo está tocando el usuario): las 4 claves faltantes equivalentes a
   3 únicas (`Local Disk` ×2, `Network Drive`, `Could not move…`), y `size_text`/`date_text` con
   formato local.
5. `plugins/rust-settings`: en `bt.rs` faltan `Searching…`, `{} devices found`, `Could not remove:
   {}` y en `main.rs` falta `none`; además el `zh` de "Wallpaper changed" (`main.rs:1487`).
6. `plugins/rust-search`: `{21B5}` literal en `rs-launcher.rs:1461` y el error en inglés de
   `main.rs:202`.
7. `plugins/rs-shell` y `plugins/rs-notify`: CLIs completas en inglés; si se consideran UI,
   necesitan `t!` (son programas de consola, no ventanas).

---

## 8. Cómo ejecutar la auditoría

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\i18n-check.ps1
```

- Sin dependencias; PowerShell 5.1; no escribe ningún fichero.
- Devuelve **1** si hay problemas (errores, duplicados, líneas mal formadas, idiomas que faltan o
  entradas muertas) y **0** si todo está limpio.
- Opciones: `-Root <ruta>` para auditar otra copia y `-UnusedIsWarning` para que las entradas
  muertas se listen pero no hagan fallar el código de salida.
- El informe por crate incluye, para cada llamada: `fichero:línea`, tipo (`t!`/`tf!`), idioma que
  falta, marcador descuadrado o clave sin español. Al final lista duplicados, líneas mal formadas,
  entradas muertas, español idéntico al inglés, terminología y el recuento pt-PT/pt-BR.
