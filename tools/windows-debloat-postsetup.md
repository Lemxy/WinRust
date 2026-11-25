# Debloating de Windows y post-setup de ISOs modificadas

Fecha: 2026-11-25. Objetivo: entender cómo las ISOs modificadas (Tiny11, NTLite, tiny11builder) recortan
Windows y cómo inyectan apps en el post-setup, para añadir **WinRust** a la ISO Tiny11 que ya tenemos
(`tiny11-26h2-beta1.iso`, construida con NTLite) sin rehacer las supresiones.

## 1. Qué hace realmente el debloating

Hay tres niveles, y las herramientas serias usan los tres:

| Nivel | Herramienta | Qué toca | Reversible |
|---|---|---|---|
| Paquetes AppX | DISM (`/Remove-ProvisionedAppxPackage`) o NTLite | Store apps preinstaladas (Xbox, Clipchamp, News…) | No vuelven solas; se pueden reinstalar desde Store |
| Componentes/CBS | DISM (`/Remove-Package`, `/Remove-Capability`) o NTLite | WinSxS, .NET, drivers, MMC de 32 bits, idiomas, WinRE | **No** (Core / removals profundos) |
| Configuración | `autounattend.xml`, registro offline, servicios, tareas | OOBE, telemetría, Defender, Update, servicios, tareas | Sí (registro/servicios) |

- **tiny11builder** (`tiny11maker.ps1`): monta el ISO, copia `install.wim`, lo monta con DISM, elimina
  paquetes AppX/CBS, aplica un `autounattend.xml` (compact + sin cuenta Microsoft), exporta con
  compresión de recuperación y reconstruye el ISO con **oscdimg** del ADK. La variante **Core** además
  borra WinSxS, WinRE, Update y Defender: **no es serviceable**.
- **NTLite**: hace lo mismo con una UI y guarda un *preset* (`Auto-saved *.xml`) con la lista exacta de
  componentes, features, servicios y tareas. Deja un `NTLite.log` y un `autounattend.xml` en la raíz.
- **Regla práctica**: las supresiones de paquetes/componentes son **irreversibles**; lo que se puede
  revertir es la configuración (servicios, tareas, registro, asociaciones).

## 2. Anatomía de la ISO que tenemos

Analizada montándola en el host:

```text
D:\
  autounattend.xml        (2 KB, NTLite)
  Auto-saved 7eb3e01a.xml (preset NTLite con las supresiones)
  NTLite.log
  boot\  efi\  sources\  support\  _manifest\
  sources\install.esd     (4,47 GB, índice 1 = Windows 11 Pro)
```

`autounattend.xml` actual, en resumen (es **el mismo que publica tiny11builder** en su rama `winui-preview`,
así que la ISO es NTLite + el answer file de tiny11):

- `oobeSystem` → `HideOnlineAccountScreens=true` (fuerza cuenta local, pero no la crea).
- `windowsPE` → instalación **Compact**, índice 1, sin clave.
- Un `ConfigureChatAutoInstall=false` suelto (fuera de un pass; NTLite/tiny11 lo toleran).
- **No crea usuario local, no ejecuta nada al final.** Por eso el OOBE se queda pidiendo cuenta/red.

El preset confirma que es **Win11 Professional 25H2 x64** construido con NTLite 2025.8, con recortes de
componentes de 32 bits, MMC, OneDrive, etc. Es la base que queremos conservar.

## 3. Dónde se inyecta código en el post-setup

| Punto | Cuándo | Contexto | Dónde se declara | Uso recomendado |
|---|---|---|---|---|
| `windowsPE` | Antes de aplicar la imagen | WinPE (SYSTEM) | `autounattend.xml` | Particionado, claves, `Compact` |
| `specialize` + `RunSynchronous` | Tras aplicar la imagen, antes de OOBE | SYSTEM | `autounattend.xml` | Registro de máquina, `PostOobe`, drivers |
| **`SetupComplete.cmd`** | Tras OOBE, antes del primer login | **SYSTEM** | `%WINDIR%\Setup\Scripts\SetupComplete.cmd` | Instalar apps, HKLM, servicios |
| **`FirstLogonCommands`** | Primer login del usuario creado | **Usuario admin (elevado)** | `oobeSystem` → `Shell-Setup` | HKCU, accesos, shell del usuario |
| `$OEM$\$1\…` | Durante la instalación | Copia de archivos | `sources\$OEM$\` | Dejar payload en `C:\…` |
| `$OEM$\$$\…` | Durante la instalación | Copia de archivos | `sources\$OEM$\` | Dejar scripts en `C:\Windows\…` |
| `PostOobe` | Tras OOBE | SYSTEM | `HKLM\SYSTEM\Setup\FirstBoot\PostOobe\00` | Forzar `SetupComplete.cmd` si el OEM key lo salta |
| `Windows\Provisioning\Packages` | Al arrancar | Sistema | `$OEM$\$$\Provisioning\Packages` | Provisioning packages (PPKG) |

Detalles que importan:

- **`SetupComplete.cmd` se salta en Pro/Home con clave OEM en BIOS/MSDM** (por diseño). Se puede forzar
  registrando en `specialize`:
  `reg add "HKLM\SYSTEM\Setup\FirstBoot\PostOobe" /v 00 /t REG_SZ /d "cmd /c C:\Windows\Setup\Scripts\SetupComplete.cmd & exit /b 0" /f`
- **No reiniciar dentro de `SetupComplete.cmd`** (Microsoft lo desaconseja: deja el setup a medias).
- `FirstLogonCommands` corre **una sola vez** y, si el usuario es administrador, **elevado**. Es el sitio
  correcto para `HKCU` (shell, asociaciones, ajustes por usuario).
- `$OEM$\$1\x` → `C:\x`; `$OEM$\$$\x` → `C:\Windows\x`. Todo lo que dejemos ahí está disponible cuando
  corren los scripts.
- El `autounattend.xml` de la raíz del ISO es el que lee Setup. Si la ISO ya trae uno (NTLite), hay que
  **fusionar**, no reemplazar, para no perder `Compact`/`HideOnlineAccountScreens`.

## 4. Cómo lo hacen otras ISOs modificadas

El patrón se repite; cambia el nombre del script, no el mecanismo:

| Proyecto | Supresiones | Inyección de apps / ajustes |
|---|---|---|
| **tiny11builder / Tiny11** | DISM offline: AppX y componentes; `autounattend.xml` con `Compact` + `HideOnlineAccountScreens` | No instala apps: solo quita. Para añadir algo se usa `$OEM$` + `SetupComplete.cmd` / `FirstLogonCommands` (lo que hacemos aquí). |
| **NTLite** | Preset `Auto-saved *.xml`: componentes, features, servicios, tareas | Página **Post-Setup**: archivos y comandos con *Before logon* (genera `SetupComplete.cmd`) o *After logon* (genera `FirstLogonCommands`); quedan registrados en el preset. |
| **AtlasOS** | Playbook AME: AppX, servicios, tareas, telemetría | El ISO lleva un playbook y un script que se ejecuta en el primer arranque (tarea/FirstLogon); se reaplica con una nueva versión del playbook. |
| **ReviOS** | Playbook AME similar | Carpeta `Playbook` en el ISO + ejecución en el primer logon; el usuario puede reaplicar/actualizar. |
| **Ghost Spectre / builds “compact”** | Componentes y AppX | `$OEM$` + `SetupComplete.cmd` para runtimes/apps y `FirstLogonCommands` para ajustes de usuario. |

Denominador común:

1. **Payload en `$OEM$`** (archivos que deben existir antes de que corra nada).
2. **Un hook SYSTEM** (`SetupComplete.cmd`, o `RunSynchronous` en `specialize`) para lo de máquina.
3. **Un hook de primer login** (`FirstLogonCommands`) para `HKCU`, accesos y shell del usuario.
4. **Un marcador/estado y un log** para saber qué se aplicó y poder revertir.
5. **Reinicio o cierre de sesión** al final si el cambio es el shell (el shell se lee al iniciar sesión).

Los *provisioning packages* (`$OEM$\$$\Provisioning\Packages\*.ppkg`) son la alternativa “soportada” para
configuración de empresa, pero no sirven para instalar binarios arbitrarios con la flexibilidad de un script.


## 5. Diseño para WinRust en el post-setup

### 5.1 Payload en el ISO

```text
sources\$OEM$\$1\WinRust\
    apps\        topbar.exe, rust-explorer.exe, rs-shell.exe, rs-clip.exe,
                 rust-settings.exe, rust-control.exe, rs-shot.exe, rs-calc.exe,
                 rust-notepad.exe, rtaskmgr-win32.exe
    tools\       autopost.ps1, iso-provision.ps1, shell-phase2.ps1, vm-setup.ps1
sources\$OEM$\$$\Setup\Scripts\SetupComplete.cmd
autounattend.xml   (fusionado)
```

`$1` copia a `C:\WinRust\`; `$$` copia `SetupComplete.cmd` a `C:\Windows\Setup\Scripts\`.

### 5.2 Fases

1. **Machine (SYSTEM)** — `SetupComplete.cmd` → `autopost.ps1 -Phase Machine`:
   - copia los `.exe` a `C:\Program Files\<app>\`;
   - escribe `%ProgramData%\WinRust\{lang.txt,dodge.txt,autohide.txt,look.txt}`;
   - ejecuta `iso-provision.ps1 -DryRun` y `-Apply -NoPrompt` (las claves HKLM/IFEO son correctas; las
     HKCU caen en SYSTEM y se repiten en la fase User);
   - ejecuta `shell-phase2.ps1 -Apply -NoPrompt` (IFEO de Start/Search/Widgets/Cortana/ShellExperienceHost,
     tarea `rs-overlay`, WSearch);
   - crea `%ProgramData%\WinRust\handlers-enabled` (para que `rs-shell` registre `ms-settings:` del usuario);
   - escribe `C:\WinRust\autopost-machine.log`.

2. **User (primer login, elevado)** — `FirstLogonCommands` → `autopost.ps1 -Phase User`:
   - repite `iso-provision.ps1 -Apply -NoPrompt` y `shell-phase2.ps1 -Apply -NoPrompt` en el **HKCU del
     usuario** (asociaciones `ms-settings:`/`ms-screenclip:` y `Winlogon\Shell = rs-shell.exe`);
   - escribe `C:\WinRust\autopost-user.log`;
   - programa `shutdown /r /t 30` para que el siguiente arranque ya use `rs-shell` (si no, el primer login
     sigue con Explorer hasta cerrar sesión).

3. **Arranque**: `rs-shell` levanta la barra y el escritorio de WinRust; `topbar` crea la barra; el
   escritorio de `rust-explorer` sustituye al de Explorer.

### 5.3 Answer file (`autounattend.xml`)

El ISO trae un answer file mínimo (idéntico al de tiny11builder). `iso-inject.ps1` escribe uno **completo**
que conserva sus claves (`Compact`, `HideOnlineAccountScreens`) y añade:

- `specialize` → `Microsoft-Windows-Deployment\RunSynchronous` con el `reg add ...PostOobe...`.
- `oobeSystem` → `Microsoft-Windows-Shell-Setup`:
  - `OOBE`: `HideEULAPage`, `HideOnlineAccountScreens`, `ProtectYourPC=3`.
  - `UserAccounts\LocalAccounts\LocalAccount`: `rexx`, grupo Administrators, contraseña `winrust`.
  - `AutoLogon`: `rexx`, `LogonCount=2` (el segundo login tras el reinicio ya arranca `rs-shell`).
  - `FirstLogonCommands`: `powershell -NoProfile -ExecutionPolicy Bypass -File C:\WinRust\tools\autopost.ps1 -Phase User`.

> En builds recientes `HideOnlineAccountScreens` puede ignorarse; la cuenta local del answer file es la
> parte que realmente evita el OOBE roto de esta ISO.

## 6. Construcción del ISO modificado

`tools\iso-inject.ps1` (nuevo):

1. Monta el ISO base (`Mount-DiskImage`) y copia el árbol a un directorio de trabajo en `C:` (SSD).2. Copia `apps`/`tools` a `sources\$OEM$\$1\WinRust\` y `SetupComplete.cmd` a
   `sources\$OEM$\$$\Setup\Scripts\`.
3. Fusiona el `autounattend.xml` de la raíz con los nodos anteriores (XML DOM, preservando NTLite).
4. Reconstruye el ISO:
   - **BIOS (SeaBIOS / QEMU)**: IMAPI2 (`IMAPI2FS.MsftFileSystemImage` + `BootOptions` con
     `boot\etfsboot.com`, `PlatformId=0`, sin emulación). Es lo que usa la VM del test.
   - **UEFI**: IMAPI2 no permite dos entradas El Torito; para UEFI real usar `oscdimg` del ADK:
     `oscdimg -m -o -u2 -udfver102 -bootdata:2#p0,e,b...\boot\etfsboot.com#pEF,e,b...\efi\microsoft\boot\efisys.bin <media> <out.iso>`.
   - **Instalación totalmente desatendida**: `-AutoDisk` añade `DiskConfiguration` y `InstallTo` y **borra el
     disco 0** creando una partición activa NTFS (BIOS/MBR, el caso de la VM). Sin ese flag, Setup pide el
     disco como hasta ahora. En UEFI haría falta GPT con EFI+MSR; no usar `-AutoDisk` ahí.
5. Calcula SHA-256 y deja el ISO en `I:\tmp\isos\winrust-tiny11.iso`.

### 6.1 Prueba automatizada

`tools\iso-autotest.ps1` crea un disco nuevo y arranca el ISO con monitor HMP:

```powershell
# ISO con el post-setup (sin -AutoDisk: hay que pulsar Next en la página del disco)
.\tools\iso-inject.ps1
.\tools\iso-autotest.ps1 -Create
.\tools\iso-autotest.ps1 -Run

# ISO totalmente desatendido (BORRA el disco 0)
.\tools\iso-inject.ps1 -AutoDisk
.\tools\iso-autotest.ps1 -Wipe
.\tools\iso-autotest.ps1 -Create
.\tools\iso-autotest.ps1 -Run
```

En el invitado, el orden es:

1. `SetupComplete.cmd` (SYSTEM) → `C:\WinRust\autopost-machine.log`.
2. `FirstLogonCommands` (usuario `rexx`) → `C:\WinRust\autopost-user.log`.
3. Reinicio → `rs-shell` como shell; barra y escritorio de WinRust.

Comprobar: `C:\WinRust\autopost-*.log`, `HKCU\...\Winlogon\Shell`, los `IFEO` de Start/Search, y que
`rs-shell.exe`/`topbar.exe` estén corriendo.


## 7. Riesgos y reversibilidad

- **Supresiones NTLite**: no se tocan; el ISO conserva el `install.esd` y el preset.
- **Cambios de configuración** (servicios, tareas, IFEO, shell): reversibles con
  `shell-phase2.ps1 -Restore` e `iso-provision.ps1 -Restore` desde el propio Windows instalado.
- **Cuenta local** `rexx`/`winrust`: cámbiala en el answer file antes de distribuir el ISO.
- **`-AutoDisk` es destructivo**: borra el disco 0. Úsalo solo en la VM o en un disco vacío; para UEFI
  necesita un `DiskConfiguration` GPT distinto (EFI+MSR) y el ISO UEFI con oscdimg.
- **`SetupComplete.cmd` saltado por OEM key**: cubierto con el `PostOobe` de `specialize`.
- **UEFI**: el ISO generado con IMAPI2 es BIOS-only; para UEFI usar `oscdimg` (sección 5).
- **OOBE roto de esta ISO beta**: si el OOBE sigue fallando antes de `FirstLogonCommands`, el problema es
  la imagen base (26H2 beta), no el post-setup; la solución limpia es reconstruir con tiny11builder sobre
  un ISO oficial y el mismo preset, o usar una build 25H2 estable.

## 8. Referencias

- Microsoft Learn: *Add a Custom Script to Windows Setup* (SetupComplete.cmd, FirstLogonCommands).
- Microsoft Learn: *FirstLogonCommands* (oobeSystem, elevado si el usuario es admin).
- Deployment Research: *Using an AutoUnattend.xml file to Automate Windows 11 Setup from Media* ($OEM$).
- unattend.tools: *Running PowerShell, cmd and VBS from unattend.xml* (contextos y límites).
- ntdevlabs/tiny11builder: `tiny11maker.ps1`, `autounattend.xml`, `oscdimg`.
- Microsoft Learn: *Oscdimg Command-Line Options* (`-bootdata:2#p0,e,b...#pEF,e,b...`).
- NTLite: preset `Auto-saved *.xml` y `NTLite.log` que acompañan a esta ISO.
