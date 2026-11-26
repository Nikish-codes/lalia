//! Lalia — system-wide live dictation for Windows and Linux (X11).
//!
//! Greek λαλιά = "speech". Hold the hotkey (default F12), speak, and the text is
//! typed into whatever window is focused. Runs on Gemini 3.5 Transcribe Live —
//! the same model Antigravity's record button uses.

// A GUI program on Windows: no console window when started from the Start
// menu (see `win::attach_parent_console` for terminal use).
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

#[cfg(not(any(unix, windows)))]
compile_error!("Lalia supports Linux (X11) and Windows.");

mod app;
mod audio;
mod autostart;
mod buddy;
mod config;
mod gemini;
mod gui;
mod history;
mod hotkey;
mod icon;
mod inject;
mod ipc;
mod notify;
mod overlay;
mod tray;
#[cfg(windows)]
mod win;

use std::io::Read;

use config::Config;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    #[cfg(windows)]
    win::attach_parent_console();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("Error: {e}");
        // Started from the Start menu there is no terminal to read that in.
        #[cfg(windows)]
        if matches!(
            args.first().map(String::as_str),
            None | Some("gui" | "settings")
        ) {
            win::error_box(&e.to_string());
        }
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> anyhow::Result<()> {
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0) {
        Some("help" | "--help" | "-h") => {
            print_help();
            Ok(())
        }
        Some("version" | "--version" | "-V") => {
            println!("lalia {VERSION}");
            Ok(())
        }
        Some("set-key") => cmd_set_key(),
        Some("config") => cmd_config(),
        Some("generate-config") => {
            let p = config::write_default_config(true)?;
            println!("Wrote {}", p.display());
            Ok(())
        }
        Some("autostart") => cmd_autostart(arg(1)),
        Some("gui" | "settings") => gui::run(arg(1).unwrap_or("home")),
        Some(
            cmd @ ("toggle" | "start" | "stop" | "pause" | "resume" | "toggle-pause" | "status"
            | "quit"),
        ) => cmd_control(cmd),
        Some("selftest" | "--selftest") => {
            let secs = arg(1).and_then(|s| s.parse().ok()).unwrap_or(8);
            runtime()?.block_on(app::selftest(Config::load()?, secs))
        }
        // Developer helpers: type text after a delay (end-to-end tests), icon
        // PNGs and overlay previews.
        Some("type-text") => {
            let delay = arg(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            std::thread::sleep(std::time::Duration::from_millis(delay));
            inject::type_text(arg(1).unwrap_or(""), 2)
        }
        Some("render-icons") => icon::export(std::path::Path::new(arg(1).unwrap_or("icons"))),
        Some("render-buddy") => buddy::film(std::path::Path::new(arg(1).unwrap_or("buddy"))),
        Some("render-overlay") => overlay::preview(
            std::path::Path::new(arg(1).unwrap_or("overlay.png")),
            overlay::Phase::Listening,
            "Listening · F12",
            "so the idea is that you just hold the key and talk",
        ),
        None => runtime()?.block_on(app::run(Config::load()?)),
        Some(other) => {
            eprintln!("lalia: unknown command '{other}'\n");
            print_help();
            std::process::exit(2);
        }
    }
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

fn cmd_set_key() -> anyhow::Result<()> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        eprintln!("Paste your Gemini API key (AIza… or AQ.Ab…), then Enter and Ctrl-D:");
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
    println!(
        "Saved key to {}{}",
        path.display(),
        if cfg!(unix) { " (0600)" } else { "" }
    );
    Ok(())
}

fn cmd_config() -> anyhow::Result<()> {
    let cfg = Config::load()?;
    println!("lalia {VERSION}");
    println!("  config file : {}", config::config_path().display());
    println!("  config dir  : {}", config::config_dir().display());
    println!("  history     : {}", history::path().display());
    #[cfg(windows)]
    println!("  log         : {}", win::log_path().display());
    println!(
        "  key present : {}",
        if cfg.has_key() { "yes" } else { "no" }
    );
    println!(
        "  running     : {}",
        if ipc::daemon("status").is_some() {
            "yes"
        } else {
            "no"
        }
    );
    println!("{}", cfg.summary());
    Ok(())
}

fn cmd_control(cmd: &str) -> anyhow::Result<()> {
    match ipc::daemon(cmd) {
        Some(reply) => {
            println!("{reply}");
            Ok(())
        }
        None => anyhow::bail!("Lalia is not running (start it with `lalia`)"),
    }
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
    (none)              Run Lalia in the tray (hold F12 to dictate).
                        If it is already running, opens the settings window.
    gui [PAGE]          Open the settings window
                        (home | setup | dictation | feedback | history | about)
    toggle              Start/stop dictation in the running Lalia — bind this
                        to any shortcut in your desktop's keyboard settings
    start | stop        Start / stop dictation
    pause | resume      Turn dictation off / on (same as F11)
    status              Print the running Lalia's state as JSON
    quit                Quit the running Lalia
    selftest [SECS]     Capture the mic for SECS and print transcripts (types nothing)
    set-key             Read an API key from stdin and store it
    config              Show the resolved configuration and paths
    generate-config     (Re)write a documented default config.toml
    autostart enable|disable|status
                        Start Lalia automatically at login
    help | version

ENVIRONMENT (override config.toml):
    GEMINI_API_KEY / LALIA_KEY   API key (AIza… or AQ.Ab…)
    LALIA_HOTKEY                 e.g. F12, RightCtrl, Pause
    LALIA_PAUSE_HOTKEY           e.g. F11
    LALIA_ACTIVATION             hold | toggle
    LALIA_INSERT                 streaming | on_release
    LALIA_MODE                   VERBATIM | SMART
    LALIA_LANG                   e.g. en-US (empty = auto)
    LALIA_VOCAB                  comma-separated hot-words
    LALIA_MODEL                  default gemini-3.5-transcribe-live
    LALIA_INPUT_DEVICE           microphone name (empty = default)
    LALIA_OVERLAY                true | false (on-screen indicator)
    LALIA_OVERLAY_POSITION       bottom | top
    LALIA_NOTIFY / LALIA_SOUND   true | false
    LALIA_HISTORY                true | false (keep local history)
    LALIA_START_ENABLED          start active (true) or paused (false)

Config file: {}",
        config::config_path().display()
    );
}
