//! Lalia — system-wide live dictation for Linux (X11).
//!
//! Greek λαλιά = "speech". Hold the hotkey (default F12), speak, and the text is
//! typed into whatever window is focused. Runs on Gemini 3.5 Transcribe Live —
//! the same model Antigravity's record button uses.

mod audio;
mod autostart;
mod config;
mod gemini;
mod hotkey;
mod inject;
mod notify;

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::JoinHandle;

use config::{Activation, Config, InsertMode};
use gemini::TranscriptEvent;
use hotkey::{HotkeyEvent, KeyId};
use notify::Sound;

enum AppEvent {
    Hotkey(HotkeyEvent),
    Transcript(TranscriptEvent),
    Deadline,
}

struct Session {
    audio_stop: Arc<AtomicBool>,
    audio_tx: Option<UnboundedSender<Vec<i16>>>,
    gemini: JoinHandle<()>,
    forwarder: JoinHandle<()>,
    timer: Option<JoinHandle<()>>,
    accumulated: String,
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("help" | "--help" | "-h") => {
            print_help();
            return Ok(());
        }
        Some("version" | "--version" | "-V") => {
            println!("lalia {VERSION}");
            return Ok(());
        }
        Some("set-key") => return cmd_set_key(),
        Some("config") => return cmd_config(),
        Some("generate-config") => {
            let p = config::write_default_config(true)?;
            println!("Wrote {}", p.display());
            return Ok(());
        }
        Some("autostart") => return cmd_autostart(args.get(1).map(String::as_str)),
        _ => {}
    }

    let cfg = Config::load()?;

    let selftest_pos = args
        .iter()
        .position(|a| a == "--selftest" || a == "selftest");
    let selftest_secs: u64 = selftest_pos
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let inject_enabled = selftest_pos.is_none();

    cfg.require_key()?;

    // Single-instance guard: hold a loopback port for the process lifetime.
    let _instance_guard = match std::net::TcpListener::bind(("127.0.0.1", 51987)) {
        Ok(l) => l,
        Err(_) => {
            eprintln!("[lalia] Lalia is already running.");
            return Ok(());
        }
    };

    println!("lalia {VERSION} — system-wide live dictation (Greek λαλιά, \"speech\")");
    println!("{}", cfg.summary());
    if !cfg.start_enabled {
        println!("\n  (starting PAUSED — tap {} to enable)", cfg.pause_hotkey);
    }
    println!(
        "\n{} {} to dictate. {} to pause/resume.\n",
        match cfg.activation {
            Activation::Hold => "Hold",
            Activation::Toggle => "Tap",
        },
        cfg.hotkey,
        cfg.pause_hotkey,
    );

    let (app_tx, mut app_rx) = mpsc::unbounded_channel::<AppEvent>();

    // Hotkey listeners.
    let (hk_tx, mut hk_rx) = mpsc::unbounded_channel::<HotkeyEvent>();
    hotkey::spawn(&cfg.hotkey, KeyId::Main, hk_tx.clone())?;
    if !cfg.pause_hotkey.eq_ignore_ascii_case(&cfg.hotkey) {
        hotkey::spawn(&cfg.pause_hotkey, KeyId::Pause, hk_tx.clone())?;
    }
    drop(hk_tx);
    {
        let app_tx = app_tx.clone();
        tokio::spawn(async move {
            while let Some(h) = hk_rx.recv().await {
                if app_tx.send(AppEvent::Hotkey(h)).is_err() {
                    return;
                }
            }
        });
    }

    let mut session: Option<Session> = None;
    let mut finishing = false;
    let mut enabled = cfg.start_enabled;

    if selftest_pos.is_some() {
        println!("selftest: capturing for {selftest_secs}s — speak now (no text is typed)…");
        session = Some(start_session(&cfg, &app_tx)?);
        arm_timer(session.as_mut(), selftest_secs * 1000, &app_tx);
    } else if enabled {
        notify::notify(
            "Lalia",
            &format!(
                "Ready — {} {} to dictate",
                if cfg.activation == Activation::Hold {
                    "hold"
                } else {
                    "tap"
                },
                cfg.hotkey
            ),
            cfg.notify,
        );
    } else {
        notify::notify("Lalia", "Paused — tap the pause key to enable", cfg.notify);
    }

    while let Some(event) = app_rx.recv().await {
        match event {
            AppEvent::Hotkey(HotkeyEvent {
                key: KeyId::Pause,
                down: true,
            }) => {
                enabled = !enabled;
                if !enabled {
                    if session.is_some() {
                        finalize(&cfg, session.take(), inject_enabled);
                        finishing = false;
                    }
                    notify::sound(Sound::Paused, cfg.sound);
                    notify::notify("Lalia paused", "Dictation is off", cfg.notify);
                    eprintln!("[lalia] ⏸  paused");
                } else {
                    notify::sound(Sound::Resumed, cfg.sound);
                    notify::notify("Lalia active", "Dictation is on", cfg.notify);
                    eprintln!("[lalia] ▶  active");
                }
            }

            AppEvent::Hotkey(HotkeyEvent {
                key: KeyId::Main,
                down: true,
            }) => {
                if !enabled {
                    continue;
                }
                if session.is_none() {
                    match start_session(&cfg, &app_tx) {
                        Ok(s) => {
                            notify::sound(Sound::Start, cfg.sound);
                            eprintln!("[lalia] ● listening…");
                            session = Some(s);
                            finishing = false;
                        }
                        Err(e) => {
                            notify::notify("Lalia error", &e.to_string(), cfg.notify);
                            eprintln!("[lalia] could not start: {e}");
                        }
                    }
                } else if cfg.activation == Activation::Toggle {
                    begin_finish(session.as_mut());
                    arm_timer(session.as_mut(), 1200, &app_tx);
                    finishing = true;
                }
            }

            AppEvent::Hotkey(HotkeyEvent {
                key: KeyId::Main,
                down: false,
            }) => {
                if cfg.activation == Activation::Hold && !finishing && session.is_some() {
                    begin_finish(session.as_mut());
                    arm_timer(session.as_mut(), 1200, &app_tx);
                    finishing = true;
                }
            }

            AppEvent::Hotkey(HotkeyEvent {
                key: KeyId::Pause,
                down: false,
            }) => {}

            AppEvent::Transcript(TranscriptEvent::Ready) => {
                eprintln!("[lalia] ready (streaming)");
            }
            AppEvent::Transcript(TranscriptEvent::Interim(text)) => {
                eprint!("\r[lalia] … {text}                    ");
            }
            AppEvent::Transcript(TranscriptEvent::Final(text)) => {
                eprint!("\r");
                if let Some(s) = session.as_mut() {
                    let piece = text.trim();
                    if !piece.is_empty() {
                        match cfg.insert {
                            InsertMode::Streaming => {
                                if inject_enabled {
                                    inject_piece(cfg.type_delay_ms, piece);
                                }
                            }
                            InsertMode::OnRelease => {
                                if !s.accumulated.is_empty() && !s.accumulated.ends_with(' ') {
                                    s.accumulated.push(' ');
                                }
                                s.accumulated.push_str(piece);
                            }
                        }
                    }
                }
                eprintln!("[lalia] ✓ {text}");
                if finishing {
                    arm_timer(session.as_mut(), 250, &app_tx);
                }
            }
            AppEvent::Transcript(TranscriptEvent::Error(e)) => {
                notify::notify("Lalia error", &e, cfg.notify);
                notify::sound(Sound::Error, cfg.sound);
                eprintln!("\n[lalia] error: {e}");
                if session.is_some() {
                    finishing = true;
                    finalize(&cfg, session.take(), inject_enabled);
                }
            }
            AppEvent::Transcript(TranscriptEvent::Closed) => {
                if session.is_some() && (finishing || selftest_pos.is_some()) {
                    finalize(&cfg, session.take(), inject_enabled);
                    finishing = false;
                }
            }
            AppEvent::Deadline => {
                if session.is_some() && (finishing || selftest_pos.is_some()) {
                    finalize(&cfg, session.take(), inject_enabled);
                    finishing = false;
                }
            }
        }
    }

    Ok(())
}

fn start_session(cfg: &Config, app_tx: &UnboundedSender<AppEvent>) -> anyhow::Result<Session> {
    let (audio_tx, audio_rx) = mpsc::unbounded_channel::<Vec<i16>>();
    let audio_stop = audio::start(audio_tx.clone())?;

    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<TranscriptEvent>();
    let gemini = gemini::spawn(cfg.clone(), audio_rx, events_tx);

    let forwarder = {
        let app_tx = app_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = events_rx.recv().await {
                if app_tx.send(AppEvent::Transcript(ev)).is_err() {
                    return;
                }
            }
        })
    };

    Ok(Session {
        audio_stop,
        audio_tx: Some(audio_tx),
        gemini,
        forwarder,
        timer: None,
        accumulated: String::new(),
    })
}

fn begin_finish(session: Option<&mut Session>) {
    if let Some(s) = session {
        s.audio_stop.store(true, Ordering::Relaxed);
        s.audio_tx = None; // ends the audio stream → server sends audioStreamEnd
    }
}

fn arm_timer(session: Option<&mut Session>, ms: u64, app_tx: &UnboundedSender<AppEvent>) {
    if let Some(s) = session {
        if let Some(t) = s.timer.take() {
            t.abort();
        }
        let app_tx = app_tx.clone();
        s.timer = Some(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            let _ = app_tx.send(AppEvent::Deadline);
        }));
    }
}

fn finalize(cfg: &Config, session: Option<Session>, inject_enabled: bool) {
    let Some(mut s) = session else { return };
    if let Some(t) = s.timer.take() {
        t.abort();
    }
    s.gemini.abort();
    s.forwarder.abort();
    s.audio_stop.store(true, Ordering::Relaxed);
    s.audio_tx = None;

    if inject_enabled && cfg.insert == InsertMode::OnRelease {
        let text = s.accumulated.trim().to_string();
        if !text.is_empty() {
            inject_piece(cfg.type_delay_ms, &text);
        }
    }
    notify::sound(Sound::Stop, cfg.sound);
    eprintln!("[lalia] ○ done");
}

fn inject_piece(delay_ms: u32, text: &str) {
    let text = text.to_string();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = inject::type_text(&text, delay_ms) {
            eprintln!("[lalia] injection failed: {e}");
        }
    });
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

fn cmd_set_key() -> anyhow::Result<()> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        eprintln!("Paste your Gemini API key (AIza… or AQ.Ab…), then Enter:");
    }
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let key = buf
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if key.is_empty() {
        anyhow::bail!("no key provided");
    }
    let path = config::set_api_key(key)?;
    println!("Saved key to {} (0600)", path.display());
    Ok(())
}

fn cmd_config() -> anyhow::Result<()> {
    let cfg = Config::load()?;
    println!("lalia {VERSION}");
    println!("  config file : {}", config::config_path().display());
    println!("  config dir  : {}", config::config_dir().display());
    println!(
        "  key present : {}",
        if cfg.api_key.is_empty() { "no" } else { "yes" }
    );
    println!("{}", cfg.summary());
    Ok(())
}

fn cmd_autostart(action: Option<&str>) -> anyhow::Result<()> {
    match action {
        Some("enable" | "on") => autostart::enable(),
        Some("disable" | "off") => autostart::disable(),
        Some("status") | None => {
            autostart::status();
            Ok(())
        }
        Some(other) => {
            anyhow::bail!("unknown autostart action '{other}' (use enable|disable|status)")
        }
    }
}

fn print_help() {
    println!(
        "lalia {VERSION} — system-wide live dictation (Greek λαλιά = \"speech\")

USAGE:
    lalia [COMMAND] [OPTIONS]

COMMANDS:
    (none)              Run Lalia (hold F12 to dictate)
    selftest [SECS]     Capture the mic for SECS and print transcripts (types nothing)
    set-key             Read an API key from stdin and store it (0600)
    config              Show the resolved configuration and paths
    generate-config     (Re)write a documented default config.toml
    autostart enable|disable|status
                        Start Lalia automatically at login (systemd --user)
    help | version

ENVIRONMENT (override config.toml):
    GEMINI_API_KEY / LALIA_KEY   API key (AIza… or AQ.Ab…)
    LALIA_HOTKEY                 e.g. F12
    LALIA_PAUSE_HOTKEY           e.g. F11
    LALIA_ACTIVATION             hold | toggle
    LALIA_INSERT                 streaming | on_release
    LALIA_MODE                   VERBATIM | SMART
    LALIA_LANG                   e.g. en-US (empty = auto)
    LALIA_VOCAB                  comma-separated hot-words
    LALIA_MODEL                  default gemini-3.5-transcribe-live
    LALIA_NOTIFY / LALIA_SOUND   true | false
    LALIA_START_ENABLED          start active (true) or paused (false)

Config file: ~/.config/lalia/config.toml"
    );
}
