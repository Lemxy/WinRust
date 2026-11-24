# Revisión i18n de las apps de WinRust

**Fecha:** 2026-11-24
**Alcance:** `topbar`, `rust-explorer`, `rust-control`, `rtaskmgr-win32`, `rust-settings`, `rs-du`, `purge`,
`rs-notify`, `rs-notify-bridge` (lectura) y `rs-shot` (traducido).
**Método:** lectura de fuentes + `tools/i18n-check.ps1` (solo lectura) + `cargo check -p rs-shot`.
No se ha compilado el workspace.

> `common/es.tsv` y `common/src/lang.rs` los está tocando otro agente. Aquí solo se leen y se reportan.

---

## 1. De dónde sale el idioma

Todas las apps leen el **mismo** sitio: `%ProgramData%\WinRust\lang.txt`, a través de
`winrust_common` (`lang()`, `lang!()`, `t!`, `tf!`). `lang()` cachea el valor la primera vez que se
usa en el proceso y rellena `Lang::En` si el fichero no existe o no se puede leer. Las apps que
tienen selector de idioma escriben con `winrust_common::set_lang()`, que es también el que escribe
`lang.txt` para el resto.

| App | Cómo obtiene el idioma | Selector propio | Fichero propio | Veredicto |
|---|---|---|---|---|
| `topbar` | `#[macro_use] extern crate winrust_common;`, `use winrust_common as i18n;`; `t!`/`tf!` y `i18n::lang!()` | Sí, pero **central**: `settings.rs::language_page()` y `settingswin.rs` usan `Lang::ALL` + `i18n::set_lang()` | No | Correcto |
| `rust-explorer` | `#[macro_use]` + `use winrust_common::{lang, Lang, t, tf}` | No | No | Correcto |
| `rust-control` | `#[macro_use] extern crate winrust_common;`; `lang!()` para orden de fecha y unidades | No | No | Correcto |
| `rtaskmgr-win32` | `#[macro_use] extern crate winrust_common;` (302 llamadas `t!`/`tf!`) | No | No | Correcto |
| `rust-settings` | `#[macro_use]` + `lang!()` + `winrust_common::set_lang()` | Sí, **central**: `LANGS` + `Op::UiLang` → `set_lang` | No | Correcto (ver nota) |
| `rs-du` | `#[macro_use] extern crate winrust_common;` | No | No | Correcto |
| `purge` | `use winrust_common::{t, tf};` | No | No | Correcto |
| `rs-notify` | Sin texto de UI: recibe `--title/--body` ya traducidos y los pinta la barra | No | No | Correcto |
| `rs-notify-bridge` | Sin texto de UI: reenvía notificaciones a la barra | No | No | Correcto |
| `rs-shot` | `#[macro_use] extern crate winrust_common;`; `lang!()` al arrancar (`winrust_common::lang`); `t!`/`tf!` | **No** | **No** | Correcto |

Ninguna app usa `GetUserDefaultUILanguage`, `GetThreadLocale` ni un fichero JSON/INI de idioma
propio. `topbar/src/lang.rs` **no** es i18n: es el selector de **idioma de entrada** (distribución
de teclado, `HKL`), y no toca `lang.txt`.

### Notas de la auditoría

1. `topbar/src/settings.rs::language_page()` escribe la lista de idiomas a mano
   (`[En, Ru, Ja, Pt, Zh, Es]`) en vez de usar `winrust_common::Lang::ALL`. Hoy coincide, pero si
   cambia el orden en `common` la barra y Ajustes discreparán. (Solo reporte; `topbar` está fuera
   de mi alcance.)
2. `rust-settings` sí usa `Lang::ALL`-equivalente en su `LANGS`, con el mismo orden. Duplicación
   inofensiva.
3. `topbar/src/main.rs:2449-2451` mezcla estilos: `match lang!() { i18n::Lang::Es => "No se pudo
   abrir la aplicación", ... }` en lugar de `t!`. Funciona con el idioma central, pero la clave
   inglesa no está en `es.tsv` (ver §3).
4. Ninguna app relee `lang.txt` en caliente (por diseño). `rust-settings`/`topbar` actualizan su
   propia ventana al cambiar; el resto lo aplica al reiniciar. `rs-shot` es de vida corta y lo lee
   al arrancar, como se pedía.

---

## 2. `rs-shot`

### Cómo lee el idioma

- `main()` llama a `lang!()` antes de nada (que es `winrust_common::lang()`, o sea, la lectura de
  `%ProgramData%\WinRust\lang.txt`) y todos los textos salen de `t!`/`tf!`, que consultan ese mismo
  `winrust_common::lang`.
- No hay lista de idiomas, ni fichero propio, ni copia del idioma: sobrevive a cualquier
  reorganización interna de `common` mientras se mantengan `lang()` y los macros reexportados.
- Los títulos de ventana (`rs-shot — Screenshot`, `rs-shot — recording`) y los nombres de archivo
  por defecto (`Screenshot_<fecha>.png`, `Recording_<fecha>.mp4`) también se traducen. Las carpetas
  `Pictures\Screenshots` y `Videos\Screen Records` se dejan igual en todos los idiomas para no
  dispersar archivos al cambiar de idioma.
- `--print-strings` imprime una muestra de textos por los macros reales; sirve para comparar
  `lang.txt=en` con `lang.txt=es` sin abrir ventanas.

### Textos que quedan en inglés a propósito

Solo diagnósticos técnicos que no son UI de uso normal: mensajes de API (`MMDeviceEnumerator:`,
`GetNextPacketSize:`, `MFCreateSample:`, …), el log `%TEMP%\rs-shot-wgc.log` y el nombre `ffmpeg`.
Se muestran dentro de «sin audio (…)» o «la grabación por WGC no pudo iniciarse: …», con el texto
de alrededor ya traducido.

---

## 3. Textos sin traducir (solo reporte)

Salida de `tools/i18n-check.ps1` (2026-11-24). No se ha tocado ninguna de estas apps.

### Claves inglesas sin entrada en `es.tsv`

| App | Clave |
|---|---|
| `rs-shot` | 100 claves nuevas (lista TSV al final de la entrega) |
| `purge` | `\r\nThis REMOVES the apps above FOR GOOD, there is no undo. Type YES to continue: ` |
| `purge` | `\r\nPress Enter to close.` |
| `rust-explorer` | `More options…` |
| `topbar` | `Could not open the app` |
| instalador | `{own}: updated {}` (fuera del alcance pedido) |

### Idiomas que faltan en alguna llamada

- Ninguna en `topbar`, `rust-explorer`, `rust-control`, `rtaskmgr-win32`, `rust-settings`, `rs-du`,
  `purge`, `rs-notify` ni `rs-notify-bridge`.
- `common/src/lib.rs:259-260`: solo tests (`t!("…","yes")`, `tf!(…,"{} pcs",3)`), no es UI.
- `rs-shot`: corregido; ya no queda ninguna llamada sin `ja/pt/zh` (los títulos usan la constante
  `TITLE`, porque «rs-shot» es marca y no se traduce).

### Avisos de variante de portugués (pt-BR)

`i18n-check.ps1` espera vocabulario pt-PT y marca el pt-BR que el proyecto usa de facto
(`arquivo`, `tela`, `registro`…). Afecta a `rtaskmgr` (16), `rust-settings` (15), `topbar` (7),
`rs-du` (6), `rust-explorer` (5), `rust-control` (3), `purge` (1). No es texto sin traducir: es la
variante elegida (`README.pt-BR.md`). Decidir si se acepta pt-BR como canónico.

### Otros

- `rust-explorer/src/main.rs:150` y `ui.rs:1326`: `ja: "PC"` sin caracteres japoneses. Es una
  abreviatura aceptada; el checker solo avisa.
- `common/es.tsv`: 21 duplicados, 1 línea mal formada (1352, español vacío) y 16 entradas muertas.
  Lo debe resolver el agente que está reescribiendo el fichero.

---

## 4. Verificación de `rs-shot`

1. **Compilación:** `cargo check -p rs-shot` termina en `Finished` sin errores de `rs-shot`
   (los avisos son de `winrust-common`, preexistentes).
2. **Cobertura:** `tools/i18n-check.ps1` ya no marca llamadas de `rs-shot` sin `ja/pt/zh`; las
   100 claves inglesas nuevas se compararon una a una contra las llamadas `t!`/`tf!` de las fuentes
   y contra `es.tsv` con un extractor ad hoc (107 claves en total, 7 ya existentes, 0 sin español,
   0 marcadores descuadrados).
3. **Prueba manual del cambio de idioma** (requiere un binario; no se compiló aquí):
   ```powershell
   $pd = "$env:TEMP\wr-lang"; New-Item -ItemType Directory -Force "$pd\WinRust" | Out-Null
   $env:ProgramData = $pd
   'en' | Set-Content "$pd\WinRust\lang.txt"
   & .\rs-shot.exe --print-strings > en.txt
   'es' | Set-Content "$pd\WinRust\lang.txt"
   & .\rs-shot.exe --print-strings > es.txt
   Compare-Object (Get-Content en.txt) (Get-Content es.txt)
   ```
   `--print-strings` incluye título del editor, «Copied to the clipboard», «Could not save the
   file», «Stop», «REC», «audio/mute», «Saved: {name}», los nombres base de archivo y la primera
   línea de la ayuda. Con `lang.txt=es` deben salir en español (una vez añadidas las claves del
   bloque TSV a `common/es.tsv`); con `en`, en inglés. Mientras falte una clave, `es::tr` cae al
   inglés por diseño.
   (El binario es de subsistema GUI: redirigir la salida a un fichero, como arriba.)
