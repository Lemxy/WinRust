# rs-notify-bridge: native Windows notifications on the WinRust bar

WinRust (`rs-shell`) replaces `explorer.exe`, and `ShellExperienceHost` is blocked through IFEO. Consequently the WinRT
toasts every program posts (`Windows.UI.Notifications`) have no renderer: Discord, Steam, Serein and friends "send" a
notification and the user sees nothing. The WinRust bar already has its own notification system, the
`\\.\pipe\winrust-notify` pipe with `winrust_common::notify::NotifyWire`.

`rs-notify-bridge` connects the two. It reads the notifications Windows itself collects through
`Windows.UI.Notifications.Management.UserNotificationListener` and writes each one to the WinRust pipe as a normal
`NotifyWire`, with app name, title, body and (when known) icon. The bar then shows it exactly like a notification raised
by `rs-notify.exe`, and keeps it in its centre.

```
Discord / Steam / Serein
        │  ToastNotificationManager (WinRT)
        ▼
Windows Notification Platform  ──►  wpndatabase.db
        │  (never drawn: no ShellExperienceHost)
        ▼
UserNotificationListener (poll every ~2 s)
        │
        ▼
rs-notify-bridge ── NotifyWire ──► \\.\pipe\winrust-notify ──► topbar: toast + centre
```

Nothing in the pipe protocol changes. The bridge is a separate process; topbar does not know it exists.

---

## Decision: the listener (path 1), not the database (path 2)

The task said the listener needs package identity. **On this machine it does not.** I measured it before writing any
code; the bridge therefore uses path 1 directly, and the database is only a documented, disabled fallback.

### Evidence (Windows 11 26H2, build 26300.9711, 2026-11-24)

1. An unpackaged process can call the API. From a normal `powershell.exe`:

   ```powershell
   Add-Type -TypeDefinition @'
   using System; using System.Runtime.InteropServices; using System.Text;
   public static class Pkg {
     [DllImport("kernel32.dll", CharSet=CharSet.Unicode)]
     static extern int GetCurrentPackageFullName(ref int len, StringBuilder name);
     public static string Id() {
       int len = 0; int hr = GetCurrentPackageFullName(ref len, null);
       if (hr == 15700) return "(no package)";
       var sb = new StringBuilder(len); hr = GetCurrentPackageFullName(ref len, sb);
       return hr + " " + sb;
     }
   }
   '@
   [Pkg]::Id()
   ```

   Result: `(no package)` (error 15700, `APPMODEL_ERROR_NO_PACKAGE`). PowerShell is not packaged, has no sparse
   package and no `userNotificationListener` capability declaration.

2. Access status is already `Allowed`:

   ```powershell
   $t = [Windows.UI.Notifications.Management.UserNotificationListener, Windows.UI.Notifications, ContentType = WindowsRuntime]
   $t::Current.GetAccessStatus()
   ```

   Result: `Allowed`.

3. `GetNotificationsAsync(Toast)` returns real notifications from unpackaged senders:

   ```
   count: 10
   id=12 app=Mozilla Firefox aumid=FirefoxToast-308046B0AF4A39CB
   id=14 app=ArtCraft Launcher aumid=ai.storyteller.artcraft-launcher
   id=1  app=Configuración aumid=windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel
   ...
   ```

4. End-to-end, with the same API the bridge uses. A toast was posted from PowerShell and read back through the
   listener:

   ```powershell
   $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent(
            [Windows.UI.Notifications.ToastTemplateType]::ToastText02)
   $text = $xml.GetElementsByTagName('text')
   $text.Item(0).AppendChild($xml.CreateTextNode('WinRust bridge test')) | Out-Null
   $text.Item(1).AppendChild($xml.CreateTextNode('posted at 13:50:26')) | Out-Null
   $notifier = [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier(
                 'Microsoft.Windows.ShellExperienceHost_cw5n1h2txyewy!App')
   $notifier.Show([Windows.UI.Notifications.ToastNotification]::new($xml))
   ```

   Reading it back gave:

   ```
   FOUND id=32 app=Host de experiencia del shell de Windows
         aumid=Microsoft.Windows.ShellExperienceHost_cw5n1h2txyewy!App
         at=24/11/2026 13:50:26
         text=[WinRust bridge test | posted at 13:50:26]
   ```

   That is title, body, app display name, AUMID, id and creation time, exactly the fields the bridge forwards.

**Conclusion:** on build 26300 the listener works without identity, so the bridge works when copied next to the bar.
To rule out "PowerShell is special", the same probe was run from a **copy of `powershell.exe` renamed and placed in
`%TEMP%`** (different path, no AUMID, no package): `GetAccessStatus()` was `Allowed` and the async call returned the same
ten notifications. This is not a documented guarantee: `uap:Capability`/`rescap:Capability Name="userNotificationListener"`
is the supported declaration for packaged apps, and a future build could start enforcing it. The sparse package is
therefore still delivered and is the recommended installation, even though it is not required today.

### Two manifest facts verified with `makeappx`/`Add-AppxPackage`

- `uap:Capability Name="userNotificationListener"` is **rejected** by the current schema:

  ```
  error C00CE169: App manifest validation error: ... 'userNotificationListener' violates the enumeration constraint of
  'documentsLibrary picturesLibrary videosLibrary ...'
  ```

  `rescap:Capability Name="userNotificationListener"` is accepted by `makeappx pack`. Restricted capabilities may be
  sideloaded without Store approval; approval is only needed to publish in the Store.

- Trusting the self-signed certificate only in `Cert:\CurrentUser\TrustedPeople` is **not enough** on this build:
  `Add-AppxPackage` fails with `0x800B0109` ("the root certificate must be trusted"), because AppX deployment runs as
  SYSTEM and does not see per-user stores. The same package registered successfully after importing the certificate into
  `Cert:\LocalMachine\TrustedPeople` (elevated), producing
  `WinRust.NotifyBridge_1.0.0.0_neutral__nag08cf4bx43m`. `tools\notify-bridge.ps1` therefore requires an elevated
  PowerShell for `-Register`/`-Unregister`.

---

## Files

| Path | What it is |
| --- | --- |
| `plugins/rs-notify-bridge/` | Crate. Console binary `rs-notify-bridge.exe`, edition 2024, workspace member. |
| `plugins/rs-notify-bridge/src/main.rs` | Poll, dedupe, `NotifyWire` conversion, pipe writer, `--once/--all/--verbose/--db`. |
| `plugins/rs-notify-bridge/app.manifest` | Side-by-side manifest with the `<msix>` identity block (publisher/package name/app id). |
| `plugins/rs-notify-bridge/app.rc`, `build.rs` | Embed the icon and the manifest into the exe. |
| `tools/notify-bridge/AppxManifest.xml` | Sparse package manifest: identity, `runFullTrust`, `userNotificationListener`, external content. |
| `tools/notify-bridge/Assets/` | Store/44/150 px logos referenced by the manifest. |
| `tools/notify-bridge.ps1` | Plan / register / unregister / status, certificate, signing, task, state. |
| `tools/state/notify-bridge.backup.json` | Created by `-Register`: every path and thumbprint it added. |
| `Cargo.toml`, `payload.txt` | Workspace member and payload entry. |

No new crates: `windows` 0.62 and `windows-sys` 0.61 (already in the workspace) and `winrust-common`.

---

## Build

```
cargo build --release -p rs-notify-bridge
```

or, with the rest of the payload:

```
powershell -ExecutionPolicy Bypass -File scripts\build-plugins.ps1
```

The bridge does not need to be installed to run (on this build). Copy it anywhere and start it while topbar is running:

```
target\release\rs-notify-bridge.exe --verbose
```

A second copy exits immediately (named mutex `Local\WinRustNotifyBridge`). Started by a logon task, the process hides
its own console window; started from a terminal it shares the terminal's console and stays visible.

### Options

| Option | Meaning |
| --- | --- |
| `--once` | One poll, deliver what is fresh, exit. Exit 3 if the bar was not listening. |
| `--all` | Send every notification Windows currently has, ignoring age and the seen file. Implies `--once`. Testing only. |
| `--interval MS` | Poll period (default 2000, minimum 200). |
| `--verbose`, `-v` | Log every poll, every new/skipped notification and every write. |
| `--db [PATH]` | Diagnose the disabled SQLite fallback (see below). Never used by the normal flow. |
| `-h`, `--help` | Usage. |

Exit codes: `0` ok, `1` bad arguments, `2` listener unavailable, `3` no bar with `--once`, `4` access denied,
`5` `--db` could not run.

### Behaviour details

- **Deduplication**: key `AUMID|Id|CreationTime`, last 200 keys in memory and in
  `%LOCALAPPDATA%\WinRust\notify-bridge.seen`. A restart does not replay old notifications.
- **Startup grace**: notifications created in the last 120 s are sent on the first poll (a logon task may start a
  second after Discord); anything older is remembered and skipped. `--all` bypasses this.
- **Pipe down**: notifications are queued (at most 64) and retried every poll, so a topbar restart does not lose them.
  The "bar is not listening" line is printed once per transition, not every 2 s.
- **Wire fields**: `app` = `AppInfo.DisplayInfo.DisplayName` (falls back to the AUMID, then `Windows`); `title` = first
  text element; `body` = the remaining text elements joined by newlines; `icon` = table/config path; `id` =
  `nr-<fnv1a-64 hex>` of the dedup key so the bar replaces rather than stacks duplicates.
- **Timeout**: `timeout_ms` stays 0, so the bar's `notify-timeout` setting applies, like `rs-notify`.
- **Single file per notification**: the bar's server reads one message per connection; that is exactly what is written.

---

## Register / unregister

Prerequisites:

- Windows 10 2004 (19041) or later; this machine is Windows 11 26H2 26300.
- Windows SDK `makeappx.exe` and `signtool.exe` (found under `%ProgramFiles(x86)%\Windows Kits\10\bin\*\x64`, or on
  `PATH`).
- An **elevated** PowerShell for `-Register`/`-Unregister` (machine-wide certificate trust).
- Sideloading of signed MSIX must be allowed (default on Windows 11; developer mode is not required for a signed
  package).

Plan first (this is the default and changes nothing):

```powershell
powershell -ExecutionPolicy Bypass -File tools\notify-bridge.ps1
```

Register:

```powershell
# from an administrator PowerShell, in C:\winrust-build
powershell -ExecutionPolicy Bypass -File tools\notify-bridge.ps1 -Register
```

What `-Register` does, in order:

1. Finds `target\release\rs-notify-bridge.exe` (or `payload\`, or the installed copy; override with `-Exe`).
2. Copies it and `Assets\` to `%LOCALAPPDATA%\WinRust\notify-bridge` (the **external location**; override with
   `-InstallDir`).
3. Finds or creates a self-signed code-signing certificate `CN=WinRust Notify Bridge` in `CurrentUser\My`, exports the
   public `.cer` to `tools\state\notify-bridge\` and imports it into `LocalMachine\TrustedPeople`.
4. Stages `AppxManifest.xml` + `Assets\`, runs `makeappx pack /o /nv /h SHA256`, then
   `signtool sign /fd SHA256 /sha1 <thumbprint>`.
5. Removes an existing `WinRust.NotifyBridge` registration (same version would fail with `0x80073CF9`) and registers
   with `Add-AppxPackage -Path ...\WinRust.NotifyBridge.msix -ExternalLocation <install dir>`.
6. Registers the logon task `rs-notify-bridge` (current user, interactive, limited) and starts it unless `-NoStart`.
   This follows the project's `rs-*` task naming.
7. Writes `tools\state\notify-bridge.backup.json` with install dir, package full/family name, certificate thumbprint,
   task name and whether each pre-existed.

Status:

```powershell
powershell -ExecutionPolicy Bypass -File tools\notify-bridge.ps1 -Status
```

It prints the package, certificate trust, task state/last result, installed exe, running process, topbar, the
`winrust-notify` pipe and the saved state.

Unregister:

```powershell
# from an administrator PowerShell
powershell -ExecutionPolicy Bypass -File tools\notify-bridge.ps1 -Unregister
```

Removes the task, stops the process, removes the package, deletes only the install directory this script created (it
checks for the exe/`Assets\`), and removes the certificate from `LocalMachine\TrustedPeople`, `CurrentUser\TrustedPeople`
and `CurrentUser\My` **only if this script created it**. `-KeepCertificate` keeps it. The state file is kept with an
`unregisteredAt` field; the `.msix` and `.cer` stay in `tools\state\notify-bridge\` for a quick re-register.

Everything is reversible; nothing outside the list above is touched (no topbar, pipe, explorer or i18n files).

---

## Testing

### 1. Funnel check without any app

With topbar running:

```powershell
target\release\rs-notify-bridge.exe --once --all --verbose
```

Every notification currently in the Windows centre is written to the bar, oldest first. On this machine that includes
Firefox, AMD Software, Settings and the "WinRust bridge test" toast from the evidence section. Expect lines like:

```
[   0.12s] poll: 11 notification(s)
[   0.12s] new: Mozilla Firefox / ...
[   0.13s] sent: Mozilla Firefox / ...
```

Then run without `--all` (normal mode) and trigger new notifications.

### 2. A known toast, no app needed

```powershell
powershell -ExecutionPolicy Bypass -File tools\notify-bridge.ps1 -Status   # topbar + pipe should be visible
# in a second window, with the bridge running:
$xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent(
         [Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$t = $xml.GetElementsByTagName('text')
$t.Item(0).AppendChild($xml.CreateTextNode('Prueba')) | Out-Null
$t.Item(1).AppendChild($xml.CreateTextNode('desde PowerShell WinRT')) | Out-Null
$n = [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier(
       'Microsoft.Windows.ShellExperienceHost_cw5n1h2txyewy!App')
$n.Show([Windows.UI.Notifications.ToastNotification]::new($xml))
```

The bar should show "Prueba" / "desde PowerShell WinRT" within ~2 s. A ShellExperienceHost AUMID is used because that
package is registered; Windows itself will not draw the toast (ShellExperienceHost is blocked), which is exactly the
situation the bridge exists for. Using the `ToastGeneric` template also exercises the title/body split.

### 3. Discord / Steam / Serein

- Discord: enable notifications, then have someone message you (or use the "Test notification" in Discord settings).
- Steam: enable "Notify me about…" in Steam settings; a friend message or a download completion posts a toast.
- Serein / launchers: whatever event posts a toast appears in the Windows centre, so it appears on the bar.

Check `--verbose`: the app name comes from `AppInfo.DisplayInfo.DisplayName`, which is localized (e.g. "Configuración"),
and the AUMID is printed nowhere unless you add it to a config file.

### 4. Icons

The built-in table is intentionally small and checked against the file system:

- Discord: newest `%LOCALAPPDATA%\Discord\app-*\Discord.exe`.
- Steam: `%ProgramFiles(x86)%\Steam\steam.exe` or `%ProgramFiles%\Steam\steam.exe`.
- Serein / ArtCraft: a few `%LOCALAPPDATA%\Programs\...` candidates and the first `.exe` in the Serein folder.

Anything else gets no icon (the bar's own glyph). To add an app without recompiling, create
`%LOCALAPPDATA%\WinRust\notify-icons.txt` with one `aumid-prefix=path` per line (case-insensitive substring match), e.g.:

```
discord=C:\Users\me\AppData\Local\Discord\app-1.0.9180\Discord.exe
steam=C:\Program Files (x86)\Steam\steam.exe
```

Comments start with `#`. The value may begin with `%LOCALAPPDATA%`, `%APPDATA%`, `%ProgramFiles%` or
`%ProgramFiles(x86)%` (written as `LOCALAPPDATA/...` etc.). Only files the shell can give an icon for are useful (exe,
`.lnk`); the bar's icon loader (`winrust_common::icon`) takes shell icons, not raw PNGs.

---

## Limitations (current phase)

- **No actions, buttons or replies.** `NotifyWire` has one optional action (`action_label`/`action_cmd`); native toasts
  carry arbitrary arguments and protocol handlers, which cannot be reproduced safely. The bridge sends no action.
- **No dismissal sync.** Dismissing in the WinRust centre does not remove the notification from Windows' centre, and
  vice versa. `UserNotificationListener.RemoveNotification` exists but needs a stable mapping and a decision about who
  owns the notification; deliberately not wired yet.
- **Icons are a table/config only.** `AppInfo.DisplayInfo.GetLogo` returns a WinRT stream; the bar currently loads shell
  icons by path. Writing the logo to a file would show a generic image icon, not the logo, until the bar learns to load
  bitmaps from a path. Hence the exe-path table.
- **No images/hero art/attribution text.** Only text elements are read from the binding; images, footers and progress
  bars are ignored. (Progress can be added later by reading `NotificationBinding` hints.)
- **No grouping.** Each notification is its own toast/centre entry; Windows' per-app grouping is not reproduced.
- **Polling, not events.** `NotificationChanged` needs a message loop and is documented as unreliable for a desktop
  process, so latency is up to `--interval` (2 s). The background-task route is a phase-3 item.
- **History window.** Notifications older than 120 s at startup are skipped even if they are still in the Windows centre;
  only new ones are forwarded (use `--all` to inspect the backlog).
- **Best effort fields.** A malformed toast still forwards what can be read; an empty AUMID/name becomes `Windows`.
  Third-party senders sometimes change the binding template; the code falls back to the first binding with text.
- **Per-user.** The pipe and the notification database are per-session; the bridge runs in the logged-in user's session
  (the `rs-notify-bridge` task is interactive, limited). It does not run as a service.
- **Windows still has no renderer.** The toast is not drawn by Windows in this configuration; the bar is the only place
  it appears. Some apps check `ToastNotifier.Setting` and may disable notifications if Windows reports them off.
- **The API can silently return nothing.** If the user revokes access, `GetNotificationsAsync` returns an empty list
  rather than an error; access is checked at startup and after an API error.

## Fallback: `wpndatabase.db` (documented, not enabled)

Path: `%LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db`, with `-wal` and `-shm` sidecars. On this machine
the main file was 1 MiB with a 238 KiB WAL at the time of measurement.

Why it is not the default:

- It is a live SQLite database owned by the notification platform. Reading it while Windows writes means WAL/shm
  coordination, locks, and a schema that changes between Windows builds.
- There is no SQLite crate in the workspace and the task forbids adding one; the only external option here is
  `sqlite3.exe`, which is **not installed** on this machine (checked with `where.exe sqlite3.exe`).
- The listener already returns the same records with a supported API, so the fallback has no benefit on this build.

What `--db` does today (honest diagnostic, exit 5 if it cannot run):

1. Defaults to the path above or takes an explicit path.
2. Runs `sqlite3 -readonly <db> "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;"`.
3. If a `Notification` table exists, prints `PRAGMA table_info(Notification)`.
4. If `sqlite3.exe` is missing, explains the situation and exits 5.

Exact design for a future fallback (fields marked **to confirm**; this machine has no SQLite tool to inspect the schema,
so they are not claimed as working):

- Copy `wpndatabase.db`, `-wal` and `-shm` to a private temporary directory and open the copy read-only, so a reader
  never blocks or corrupts the platform's writer. Record that this snapshot may lag the live centre.
- Discover tables/columns with `sqlite_master`/`PRAGMA table_info` instead of hard-coding a build-specific schema.
  The public reverse-engineering of the database uses a `Notification` table (one row per toast) and a `Handler` table
  (the AUMID/display name of the sender); **confirm names and columns** with `--db` on the target build.
- The notification body is an XML payload; extract the `<text>` nodes of the `ToastGeneric`/text binding. Reuse the
  same title/body rule as the listener path.
- Use the row's primary key plus arrival/creation timestamp as the dedup key; the listener's numeric `Id` is not the same
  thing.
- Map the SQLite timestamp (likely FILETIME, 100 ns since 1601) to the 120 s startup grace, and the `Handler` AUMID to
  the same icon table.
- Only after that works: keep it behind `--db` forever, never auto-detect. Reading another process's WAL is inherently
  fragile and a supported API exists.

## Phase 3

1. **Events instead of polling**: a packaged COM background task with `UserNotificationChangedTrigger`, or at least
   `NotificationChanged` on a dedicated STA thread, to cut latency and CPU.
2. **Actions and replies**: map toast arguments to a WinRust command only when the target is a known, safe protocol
   (e.g. Discord's `discord://`), or expose a "Open in app" action that activates the sender's AUMID.
3. **Icon pipeline**: read `AppInfo.DisplayInfo.GetLogo`, save a PNG next to `%LOCALAPPDATA%\WinRust\notify-icons\`, and
   teach the bar's toast drawing to load a bitmap path (a `winrust_common` change, coordinated with topbar).
4. **Dismissal sync**: remember `(AUMID, Id)` and call `RemoveNotification` when the user clears the bar's entry; also
   poll removals to drop entries the user dismissed in Windows.
5. **Grouping and replacement**: group by AUMID in the centre, use a stable per-app id for "N new messages" summaries.
6. **Per-app rules**: a settings page (`rust-settings`) with allow/block, icon override, quiet apps, and translated
   strings (`t!`), following the existing settings-window patterns.
7. **Backlog import**: one-time import of the current Windows centre at first run, with a clear "these are old"
   presentation, instead of the 120 s grace.
8. **Better fallback**: if a later build enforces the capability, either require the sparse package or implement the
   `wpndatabase.db` reader with a bundled SQLite (would need a dependency decision).

## Troubleshooting

| Symptom | Cause / fix |
| --- | --- |
| Exit 4, "access denied" | Windows notification access is denied. Allow the app under Settings → Privacy & security → Notifications. A console process cannot show the consent prompt; `RequestAccessAsync` is only tried when the status is `Unspecified`. |
| Exit 2, listener unavailable | The API is not present/activatable on this build. See the fallback section. |
| Exit 3 with `--once` | topbar is not running; `rs-notify-bridge` keeps a queue only in daemon mode. Start topbar and rerun. |
| Toast appears in the bar but not in Windows | Expected: ShellExperienceHost is blocked. The bar is the only renderer. |
| `Add-AppxPackage` 0x800B0109 | The certificate is not trusted machine-wide. `-Register` imports it into `LocalMachine\TrustedPeople`; run it elevated. |
| `Add-AppxPackage` 0x80073CF9 | Same package version already registered. `-Register` removes it first; if a stale registration remains, `-Unregister` then `-Register`. |
| `makeappx` C00CE169 on `userNotificationListener` | The capability must be `rescap:Capability`, not `uap:Capability`; the shipped manifest is already correct. |
| Two bridges? | The named mutex stops a second daemon; `--once` intentionally bypasses it. |
| Duplicate toasts after a restart | The seen file may be missing/read-only; check `%LOCALAPPDATA%\WinRust\notify-bridge.seen`. |

## Verification pending (needs a compile)

These were not tested because this work does not compile:

- `cargo build --release -p rs-notify-bridge` with `windows 0.62`: method names/feature set (`IAsyncOperation::join`,
  `IVectorView::GetAt`, `NotificationBinding::GetTextElements`, `HSTRING::to_string_lossy`).
- Whether a direct launch from the external location actually carries identity with the embedded `<msix>` block. The
  sparse package registers (verified) and deployment succeeds, but `GetCurrentPackageFullName` inside a *built*
  bridge still needs checking. If identity is absent, the listener still works on this build; if a later build needs
  identity, launch via `shell:AppsFolder\WinRust.NotifyBridge_nag08cf4bx43m!WinRustNotifyBridge` instead of the exe.
- End-to-end with the real binary and the bar: `--once --all --verbose`, then a live Discord/Steam toast, dedupe across
  a restart, and the icon table on an actual Discord install.
- The logon task's console hiding: the process hides its own console when it owns it; check that no window flashes at
  login on the target machine.
