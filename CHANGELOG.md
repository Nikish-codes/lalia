# Changelog

## 0.4.0 — now on Windows

Lalia now runs on **Windows 10 and 11** with everything the Linux version has.

### Download

| | |
|---|---|
| **Windows** | `lalia-0.4.0-windows-x64-setup.exe` — installer for your account (no administrator prompt). Or `lalia-0.4.0-windows-x64.zip` to run without installing. |
| **Linux (X11)** | `lalia_0.4.0_amd64.deb` — `sudo apt install ./lalia_0.4.0_amd64.deb` |

The Windows files are not code-signed yet: if SmartScreen says "Windows
protected your PC", choose **More info → Run anyway**.

### New on Windows

- **Hold F12, speak, let go** — the text is typed into whatever app has focus,
  in any language (Unicode keystrokes, independent of keyboard layout).
- **The hotkeys stay yours**: while Lalia is on, F12 and F11 don't reach the
  focused app, so holding F12 doesn't open browser DevTools. Pause Lalia and F12
  works normally again.
- **Notification-area icon** with the same menu as on Linux; left-click opens
  settings, middle-click pauses.
- **Lali**, the animated buddy, in a click-through, always-on-top transparent
  window that never takes focus; sharp on high-DPI screens.
- **Settings window** drawn with Direct3D (no OpenGL drivers needed).
- **Start on login** (Settings, tray, or the installer), soft built-in chimes,
  notifications from the tray icon.
- **Installer**: Start menu shortcut, optional desktop shortcut and
  start-on-login, clean uninstall from *Installed apps*. Your settings, API key
  and history stay in `%APPDATA%\lalia` and `%LOCALAPPDATA%\lalia`.

### Changed (both platforms)

- The tray menu, key names and control channel are now shared code with a
  small platform half each, so Linux and Windows behave the same.
- Audio capture updated to cpal 0.18.
- Releases are built by GitHub Actions on real Linux and Windows machines when
  a version tag is pushed; the Windows build is driven end-to-end there
  (named pipe, keyboard hook, typing into Notepad, install/uninstall).

### Known limits on Windows

- Windows does not let Lalia type into apps that run as administrator.
- Commands like `lalia status` print after the terminal prompt returns (Lalia
  is a windowed program); pipe them to wait: `lalia status | more`.

## 0.3.1 — Meet Lali

- Lali, a tiny animated buddy, appears while you dictate (`overlay_style`).

## 0.3.0 — Tray icon, settings window, on-screen indicator

- Tray icon, settings window with guided setup, on-screen indicator, history.
- Fixes: missing spaces between phrases, out-of-order typing, dropped last
  words, silent microphone and API-key failures.

## 0.2.0

- First release: system-wide live dictation for Linux (X11).
