# rust-control

A complete Control Panel for WinRust, drawn with the shared WinRust dark style. The window has its own
hand-drawn chrome, a 200 px sidebar, a breadcrumb bar, a live search field, rounded cards and a status
bar. Painting is double buffered.

The first page is the classic **Category** view: eight categories in two columns, each with a coloured
icon, a title and 1–3 links. `View by: Category / Icons` switches to a flat list of all applets. Links
open a page inside rust-control when one exists; the rest open the matching Windows page
(`ms-settings:`, `control.exe /name …`, or a `.cpl`/`.msc`) and the status bar says what happened.

## Pages

| Page | What it reads | What can be changed |
| --- | --- | --- |
| System | Windows edition, version, build (UBR), architecture, computer name, user, uptime, CPU (`CentralProcessor\0`), RAM (`GlobalMemoryStatusEx`), GPUs (display class / `Video` registry, `DriverDesc`), environment variables (registry) | nothing |
| Programs | `HKLM`/`HKCU` …`Uninstall\*`: name, publisher, version, size, install date | **Uninstall** starts the program's own `UninstallString` after a confirmation |
| Services | SCM `EnumServicesStatusExW`, status and `QueryServiceConfigW` start type | **Start**, **Stop**, and Automatic / Manual / Disabled through `ChangeServiceConfigW` (confirmations, administrator required) |
| Devices | `CM_Get_Device_ID_ListW` + `CM_Locate_DevNodeW` + `CM_Get_DevNode_PropertyW` (CfgMgr32; name, class, manufacturer, status) | opens `devmgmt.msc` |
| Network | `GetAdaptersAddresses`: name, description, MAC, IPv4, IPv6, gateway, DNS, link state | opens `ms-settings:network` |
| User Accounts | `NetUserEnum` (name, description, flags) and `NetLocalGroupEnum` | opens `ms-settings:otherusers` |
| Power | `PowerEnumerate` / `PowerReadFriendlyName` / `PowerGetActiveScheme`, `GetSystemPowerStatus` | activates a plan with `PowerSetActiveScheme` after a confirmation |
| Date and Time | `GetLocalTime`, `GetSystemTime`, `TimeZoneKeyName`, `W32Time` parameters | opens `ms-settings:dateandtime` and `timedate.cpl` |
| Accessibility | `HKCU\Control Panel\Accessibility` flags, Magnifier `RunningState` | opens `ms-settings:easeofaccess` and `magnify.exe` |
| Security | Defender registry values + WinDefend service, firewall profiles parsed from `netsh advfirewall show allprofiles` | opens `windowsdefender:` and the classic firewall page |
| Administrative Tools | none (static list) | `compmgmt.msc`, `diskmgmt.msc`, `eventvwr.msc`, `services.msc`, `devmgmt.msc`, `taskschd.msc`, `regedit`, `msinfo32` |

The search box filters the rows of the active page (or the home categories and applets). `Ctrl+F` focuses
it, `Esc` clears it, `F5` refreshes the page, `↑`/`↓` change page.

## Limitations

- **Administrator rights are not requested automatically.** rust-control runs as the current user
  (`asInvoker`). Starting/stopping services and changing start types report this in the status bar and do
  nothing until the program is started as administrator. Uninstallers and MMC consoles may raise their own
  UAC prompt.
- Defender is read from the registry and the `WinDefend` service, not from WMI. Tamper protection can hide
  those values; the page then says so and offers Windows Security. WMI (`MSFT_MpComputerStatus`) was not
  needed.
- The firewall page parses the localised text output of `netsh`; only the three state lines are used, in
  the order Windows prints them (domain, private, public). If the output changes, the page shows a warning
  instead of guessing.
- Device enumeration uses CfgMgr32 rather than SetupAPI; it lists present devices in the same tree Device
  Manager shows, without device icons or driver update actions.
- Environment variables are shown as stored in the registry (unexpanded `REG_EXPAND_SZ` values included).
- Spanish strings live in `common/es.tsv` (English is the key). The home page, the window chrome and all
  page labels are translated; a missing translation falls back to English by design.
- The app is not code-signed and this crate was not compiled as part of the change.
