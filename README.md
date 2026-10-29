# Lalia

[![CI](https://github.com/Nikish-codes/lalia/actions/workflows/ci.yml/badge.svg)](https://github.com/Nikish-codes/lalia/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org)

**System-wide live dictation for Linux (X11).** From Greek **λαλιά** = *"speech, talk"*.

Hold **F12**, speak, and the text is typed straight into **whatever window is
focused** — browser, editor, terminal, chat, anywhere. Release to finish.

It runs on **Gemini 3.5 Transcribe Live** — the exact model and technique
Antigravity's record button uses (the Live API's `BidiGenerateContent` +
`inputAudioTranscription`).

```
F12 held ──► mic ──► 16 kHz PCM ──► Gemini 3.5 Transcribe Live (WebSocket)
                                            │ interim / final transcripts
F12 released ──► xdotool types text ────────┘  into the focused window
```

## Install

### Option A — .deb (one command)

Download the latest `.deb` from the
[**Releases page**](https://github.com/Nikish-codes/lalia/releases), then:

```bash
sudo apt install ./lalia_0.2.0_amd64.deb    # or: sudo dpkg -i lalia_*.deb
lalia set-key        # paste your Gemini API key (AIza… or AQ.Ab…)
lalia                # start it
```

Build the package yourself with `./packaging/build-deb.sh` (output in `dist/`).
The `.deb` installs the `lalia` command, a menu entry, an icon, and a systemd
user unit (disabled by default).

### Option B — from source

```bash
sudo apt install -y xdotool libasound2-dev
cargo build --release
sudo install -m755 target/release/lalia /usr/local/bin/lalia
lalia set-key
```

or just `./install.sh` (installs to `~/.local/bin`).

You need a Gemini API key: <https://aistudio.google.com/apikey>.
The free tier covers this model at **$0**.

## Features

| Feature | How |
|---|---|
| **Push-to-talk or toggle** | `activation = "hold"` (default) or `"toggle"` — tap to start/stop |
| **Pause everything** | Tap **F11** to pause/resume; F12 is inert while paused |
| **Streaming or on-release insert** | Type words as you speak, or paste the whole thing at the end |
| **Verbatim or smart** | `VERBATIM` (raw) or `SMART` (punctuation, casing, filler removal, lists) |
| **Custom vocabulary** | Bias recognition toward names/jargon (`vocabulary = [...]`) |
| **Language hints / auto-detect** | `language = "en-US"` or `""` for auto |
| **Notifications + sound** | Desktop toast and themed sound on start/stop/pause |
| **Start on login** | `lalia autostart enable` (systemd `--user`) |
| **Start paused** | `start_enabled = false` |
| **Single instance** | A second launch exits politely |
| **Configurable hotkeys** | Any `F1`–`F35` for both dictate and pause |

## Usage

```bash
lalia                    # run (hold F12 to dictate)
lalia selftest 8         # capture 8s, print transcripts, type nothing
lalia set-key            # store the API key (0600)
lalia config             # show resolved settings + paths
lalia generate-config    # write ~/.config/lalia/config.toml
lalia autostart enable|disable|status
lalia help
```

## Configuration

`~/.config/lalia/config.toml` (created by `lalia generate-config`):

```toml
hotkey = "F12"           # dictate key
pause_hotkey = "F11"     # pause/resume key
activation = "hold"      # "hold" | "toggle"
insert = "streaming"     # "streaming" | "on_release"
mode = "VERBATIM"        # "VERBATIM" | "SMART"
language = ""            # e.g. "en-US", empty = auto-detect
vocabulary = []          # e.g. ["Kubernetes", "ConnectRPC"]
model = "gemini-3.5-transcribe-live"
type_delay_ms = 2        # raise to 4-6 if an app drops characters
notify = true
sound = true
start_enabled = true     # false = start paused
```

Environment variables override the file (`LALIA_HOTKEY`, `LALIA_MODE`,
`LALIA_VOCAB`, `LALIA_ACTIVATION`, `GEMINI_API_KEY`, …). See `lalia help`.

## Run on login

```bash
lalia autostart enable     # creates ~/.config/systemd/user/lalia.service
lalia autostart status
journalctl --user -u lalia -f
```

## Troubleshooting

* **"hotkey 'F12' not found"** → unusual layout; set another `hotkey`.
* **Text lands in the wrong place** → click the target field first; xdotool types
  into the *focused* window.
* **Characters dropped in some apps** → `type_delay_ms = 6`.
* **Nothing transcribed** → `lalia selftest 8`; if you don't see `ready
  (streaming)`, it's the key/model (the error is printed).
* **Paused by accident** → tap the pause key (F11); you'll get a toast + sound.

## Requirements

Linux with **X11** (verified on Cinnamon). `xdotool` for typing, `libasound2`
for audio, `libnotify-bin` / `libcanberra-gtk3-module` optional for
notifications and sound. **Wayland** is not supported yet (needs the
GlobalShortcuts portal + `wtype`).

## Layout

```
src/hotkey.rs     global F12/F11 detection (X11 keymap polling, no key grab)
src/audio.rs      mic capture (cpal) → downmix + resample → 16 kHz mono PCM16
src/gemini.rs     Live API WebSocket: setup, stream audio, parse interim/final
src/inject.rs     types text into the focused window (xdotool / XTEST)
src/notify.rs     desktop notifications + themed sound
src/autostart.rs  systemd --user "start on login"
src/config.rs     config file + env resolution
src/main.rs       state machine, CLI, pause, single-instance guard
packaging/        .deb build (control, desktop, icon, unit)
```

## License

MIT
