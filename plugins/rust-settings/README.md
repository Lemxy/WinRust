# rust-settings

Windows 11 Settings rebuilt in Rust (Direct2D + DirectWrite, no XAML, no WebView): about 100 pages
laid out like Windows' own, reading and changing the real values.

- System: display (scale, resolution, orientation, refresh rate, HDR, night light), sound (output /
  input devices, volume mixer), notifications, focus, power & battery, storage, multitasking, clipboard, about
- Bluetooth & devices: paired devices with one-click connect, mouse, touchpad, printers, cameras, AutoPlay
- Network & internet: Wi-Fi list + connect, Ethernet, VPN, proxy, airplane mode, known networks
- Personalization: background, colors (accent, dark mode), themes, lock screen, Start, taskbar, fonts
- Apps: installed apps (uninstall asks first), startup apps, default apps
- Accounts, Time & language, Gaming, Accessibility, Privacy & security (per-app permissions), Windows Update

Pages Windows keeps behind its own secure UI (PIN / face sign-in, some admin-only switches) open
Windows' page instead.

`rust-settings.exe [page]` - a page name such as `"Bluetooth & devices"`, or an `ms-settings:` address.
Builds on its own (like rust-search): `cargo build --release` in this folder.
About 1 MB; MIT-licensed, from the Rusty Windows project.
