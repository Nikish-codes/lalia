//! The Lalia daemon: hotkeys → microphone → Gemini → typing, plus the tray,
//! the on-screen indicator, the control socket and live config reloads.
//!
//! Everything funnels into one event loop (`App::handle`), so the state
//! machine lives in one place. Events from a dictation session carry the
//! session's id; anything from an older session is ignored.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::audio::{self, Capture};
use crate::config::{self, Activation, Config, InsertMode, Mode};
use crate::gemini::{self, TranscriptEvent};
use crate::history;
use crate::hotkey::{self, HotkeyEvent, Hotkeys, KeyId};
use crate::inject::{Injector, Joiner};
use crate::ipc;
use crate::notify::{self, Sound};
use crate::overlay::{self, Overlay};
use crate::tray::{self, TrayState};
use crate::{autostart, gui};

/// After the key is released, wait this long for the server to flush.
const FINISH_GRACE: Duration = Duration::from_millis(1500);
/// Quiet period after the last interim result while finishing.
const FINISH_AFTER_INTERIM: Duration = Duration::from_millis(800);
/// Quiet period after a final result while finishing.
const FINISH_AFTER_FINAL: Duration = Duration::from_millis(350);
/// Never wait longer than this after release.
const FINISH_CAP: Duration = Duration::from_millis(4000);
const RECENT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Connecting,
    Listening,
    Finishing,
    Paused,
    NeedsSetup,
}

/// Reply to the `status` command (read by the settings window).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub pid: u32,
    pub phase: Phase,
    pub hotkey: String,
    pub pause_hotkey: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Status,
    Toggle,
    Start,
    Stop,
    Pause,
    Resume,
    TogglePause,
    Reload,
    Quit,
    Settings(String),
    Copy(String),
    SetMode(Mode),
    SetInsert(InsertMode),
    SetActivation(Activation),
    SetOverlay(bool),
    SetAutostart(bool),
}

impl Command {
    /// Parse a control-socket line.
    pub fn parse(line: &str) -> Option<Self> {
        let mut parts = line.trim().splitn(2, ' ');
        let verb = parts.next()?.to_ascii_lowercase();
        let arg = parts.next().unwrap_or("").trim().to_string();
        Some(match verb.as_str() {
            "status" | "ping" => Command::Status,
            "toggle" => Command::Toggle,
            "start" => Command::Start,
            "stop" => Command::Stop,
            "pause" => Command::Pause,
            "resume" => Command::Resume,
            "toggle-pause" => Command::TogglePause,
            "reload" => Command::Reload,
            "quit" => Command::Quit,
            "settings" | "show" => {
                Command::Settings(if arg.is_empty() { "home".into() } else { arg })
            }
            _ => return None,
        })
    }
}

pub enum AppEvent {
    Hotkey(HotkeyEvent),
    Session(u64, SessionEvent),
    Command(Command, Option<oneshot::Sender<String>>),
    InjectError(String),
    ConfigChanged,
}

pub enum SessionEvent {
    Transcript(TranscriptEvent),
    Deadline,
    AudioError(String),
}

struct Session {
    id: u64,
    capture: Option<Capture>,
    audio_tx: Option<UnboundedSender<Vec<i16>>>,
    gemini: JoinHandle<()>,
    forwarder: JoinHandle<()>,
    timer: Option<JoinHandle<()>>,
    joiner: Joiner,
    /// Everything committed so far, joined.
    text: String,
    /// The latest interim result not yet confirmed by a final one.
    pending: String,
    started: Instant,
    finish_started: Option<Instant>,
    ready: bool,
}

struct App {
    cfg: Config,
    tx: UnboundedSender<AppEvent>,
    session: Option<Session>,
    next_id: u64,
    enabled: bool,
    last_error: Option<String>,
    hotkeys: Hotkeys,
    injector: Injector,
    overlay: Overlay,
    tray: Option<tray::Handle>,
    clipboard: Option<arboard::Clipboard>,
    recent: Vec<String>,
}

/// Run the daemon until "Quit". Returns early if another instance runs.
pub async fn run(cfg: Config) -> anyhow::Result<()> {
    let Some(listener) = ipc::claim(&ipc::daemon_socket())? else {
        // Already running: launching Lalia again opens its window.
        let _ = ipc::daemon("settings home");
        eprintln!("[lalia] Lalia is already running — opening its settings window.");
        return Ok(());
    };
    listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(listener)?;

    let version = env!("CARGO_PKG_VERSION");
    println!("lalia {version} — system-wide live dictation (Greek λαλιά, \"speech\")");
    println!("{}", cfg.summary());

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // Hotkeys.
    let (hk_tx, mut hk_rx) = mpsc::unbounded_channel::<HotkeyEvent>();
    let hotkeys = Hotkeys::start(hk_tx)?;
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            while let Some(h) = hk_rx.recv().await {
                if tx.send(AppEvent::Hotkey(h)).is_err() {
                    return;
                }
            }
        });
    }

    // Control socket.
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut line = String::new();
                    if BufReader::new(read).read_line(&mut line).await.is_err() {
                        return;
                    }
                    let reply = match Command::parse(&line) {
                        Some(cmd) => {
                            let (otx, orx) = oneshot::channel();
                            if tx.send(AppEvent::Command(cmd, Some(otx))).is_err() {
                                return;
                            }
                            orx.await.unwrap_or_else(|_| "error".into())
                        }
                        None => format!("error: unknown command '{}'", line.trim()),
                    };
                    let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
                });
            }
        });
    }

    // Config watcher (the settings window and hand edits both land here).
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut last = config::stamp();
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let now = config::stamp();
                if now != last {
                    last = now;
                    if tx.send(AppEvent::ConfigChanged).is_err() {
                        return;
                    }
                }
            }
        });
    }

    let injector = {
        let tx = tx.clone();
        Injector::start(move |e| {
            let _ = tx.send(AppEvent::InjectError(e));
        })
    };
    let overlay = Overlay::start(cfg.overlay, cfg.overlay_position, cfg.overlay_style);
    let _ = history::trim();
    let recent = history::load()
        .into_iter()
        .take(RECENT)
        .map(|e| e.text)
        .collect();

    let mut app = App {
        enabled: cfg.start_enabled,
        cfg,
        tx: tx.clone(),
        session: None,
        next_id: 0,
        last_error: None,
        hotkeys,
        injector,
        overlay,
        tray: None,
        clipboard: None,
        recent,
    };
    app.apply_hotkeys();
    app.tray = tray::spawn(app.tray_state(), tx.clone()).await;

    let key = hotkey::label(&app.cfg.hotkey);
    if !app.cfg.has_key() {
        println!("\nNo API key yet — opening setup.\n");
        gui::open("setup");
    } else if app.enabled {
        println!(
            "\n{} {key} to dictate. {} to pause/resume.\n",
            match app.cfg.activation {
                Activation::Hold => "Hold",
                Activation::Toggle => "Tap",
            },
            hotkey::label(&app.cfg.pause_hotkey)
        );
        if app.tray.is_none() {
            notify::notify(
                "Lalia",
                &format!("Ready — {key} to dictate"),
                app.cfg.notify,
            );
        }
    } else {
        println!(
            "\n  (starting PAUSED — tap {} to enable)\n",
            app.cfg.pause_hotkey
        );
    }

    while let Some(event) = rx.recv().await {
        app.handle(event);
    }
    Ok(())
}

impl App {
    fn phase(&self) -> Phase {
        if !self.cfg.has_key() {
            return Phase::NeedsSetup;
        }
        match &self.session {
            Some(s) if s.finish_started.is_some() => Phase::Finishing,
            Some(s) if s.ready => Phase::Listening,
            Some(_) => Phase::Connecting,
            None if !self.enabled => Phase::Paused,
            None => Phase::Idle,
        }
    }

    fn tray_state(&self) -> TrayState {
        TrayState {
            phase: self.phase(),
            hotkey: self.cfg.hotkey.clone(),
            pause_hotkey: self.cfg.pause_hotkey.clone(),
            activation: self.cfg.activation,
            mode: self.cfg.mode,
            insert: self.cfg.insert,
            overlay: self.cfg.overlay,
            autostart: autostart::is_enabled(),
            recent: self.recent.clone(),
            error: self.last_error.clone(),
        }
    }

    fn publish(&self) {
        if let Some(t) = &self.tray {
            tray::update(t, self.tray_state());
        }
    }

    fn status_json(&self) -> String {
        serde_json::to_string(&DaemonStatus {
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            phase: self.phase(),
            hotkey: self.cfg.hotkey.clone(),
            pause_hotkey: self.cfg.pause_hotkey.clone(),
            error: self.last_error.clone(),
        })
        .unwrap_or_else(|_| "{}".into())
    }

    fn apply_hotkeys(&mut self) {
        if let Err(e) = self.hotkeys.set(&self.cfg.hotkey, &self.cfg.pause_hotkey) {
            self.error(&format!("{e} — using F12 / F11 instead"));
            let _ = self.hotkeys.set("F12", "F11");
        }
    }

    fn error(&mut self, msg: &str) {
        eprintln!("\n[lalia] error: {msg}");
        let (title, advice) = friendly_error(msg);
        self.last_error = Some(advice.clone());
        notify::sound(Sound::Error, self.cfg.sound);
        if self.overlay.is_enabled() {
            self.overlay
                .flash(overlay::Phase::Error, title, &advice, 4.5);
        }
        notify::notify(&format!("Lalia: {title}"), &advice, self.cfg.notify);
        self.publish();
    }

    fn handle(&mut self, event: AppEvent) {
        match event {
            AppEvent::Hotkey(HotkeyEvent {
                key: KeyId::Pause,
                down: true,
            }) => self.set_enabled(!self.enabled),
            AppEvent::Hotkey(HotkeyEvent {
                key: KeyId::Pause,
                down: false,
            }) => {}
            AppEvent::Hotkey(HotkeyEvent {
                key: KeyId::Main,
                down: true,
            }) => self.main_down(),
            AppEvent::Hotkey(HotkeyEvent {
                key: KeyId::Main,
                down: false,
            }) => {
                if self.cfg.activation == Activation::Hold {
                    self.begin_finish();
                }
            }
            AppEvent::Session(id, ev) => {
                if self.session.as_ref().map(|s| s.id) == Some(id) {
                    self.session_event(ev);
                }
            }
            AppEvent::Command(cmd, reply) => {
                let answer = self.command(cmd);
                if let Some(r) = reply {
                    let _ = r.send(answer);
                }
            }
            AppEvent::InjectError(e) => self.error(&format!("typing failed: {e}")),
            AppEvent::ConfigChanged => self.reload(),
        }
    }

    fn main_down(&mut self) {
        if !self.enabled {
            if self.overlay.is_enabled() {
                self.overlay.flash(
                    overlay::Phase::Paused,
                    "Paused",
                    &format!(
                        "Tap {} to turn dictation back on",
                        hotkey::label(&self.cfg.pause_hotkey)
                    ),
                    1.6,
                );
            }
            return;
        }
        match &self.session {
            None => self.start_session(),
            Some(s) if s.finish_started.is_some() => {
                // Speaking again while the last session is still flushing:
                // wrap it up now (keeping its last words) and start fresh.
                self.finalize();
                self.start_session();
            }
            Some(_) => {
                if self.cfg.activation == Activation::Toggle {
                    self.begin_finish();
                }
            }
        }
    }

    fn set_enabled(&mut self, on: bool) {
        if self.enabled == on {
            return;
        }
        self.enabled = on;
        if !on && self.session.is_some() {
            self.finalize();
        }
        let pause_key = hotkey::label(&self.cfg.pause_hotkey);
        if on {
            notify::sound(Sound::Resumed, self.cfg.sound);
            eprintln!("[lalia] ▶  active");
        } else {
            notify::sound(Sound::Paused, self.cfg.sound);
            eprintln!("[lalia] ⏸  paused");
        }
        if self.overlay.is_enabled() {
            if on {
                self.overlay.flash(
                    overlay::Phase::Resumed,
                    "Dictation on",
                    &format!(
                        "{} {} to talk",
                        self.verb(),
                        hotkey::label(&self.cfg.hotkey)
                    ),
                    1.4,
                );
            } else {
                self.overlay.flash(
                    overlay::Phase::Paused,
                    "Paused",
                    &format!("Dictation is off — tap {pause_key} to resume"),
                    1.6,
                );
            }
        } else if on {
            notify::notify("Lalia active", "Dictation is on", self.cfg.notify);
        } else {
            notify::notify("Lalia paused", "Dictation is off", self.cfg.notify);
        }
        self.publish();
    }

    fn verb(&self) -> &'static str {
        match self.cfg.activation {
            Activation::Hold => "Hold",
            Activation::Toggle => "Tap",
        }
    }

    fn start_session(&mut self) {
        if !self.cfg.has_key() {
            notify::notify(
                "Lalia needs an API key",
                "Add your Gemini API key to start dictating.",
                self.cfg.notify,
            );
            gui::open("setup");
            return;
        }
        self.next_id += 1;
        let id = self.next_id;
        // Opening the microphone can take a moment; don't stall other tasks.
        let started = tokio::task::block_in_place(|| start_streams(&self.cfg, id, &self.tx));
        match started {
            Ok(session) => {
                notify::sound(Sound::Start, self.cfg.sound);
                eprintln!("[lalia] ● listening…");
                self.last_error = None;
                self.overlay.show(
                    overlay::Phase::Connecting,
                    "Connecting",
                    session.capture.as_ref().map(|c| c.level.clone()),
                );
                self.session = Some(session);
                self.publish();
            }
            Err(e) => self.error(&format!("could not start: {e}")),
        }
    }

    /// Stop the microphone and let the server flush what it heard.
    fn begin_finish(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        if s.finish_started.is_some() {
            return;
        }
        if let Some(c) = s.capture.take() {
            c.stop();
        }
        s.audio_tx = None; // ends the audio stream → client sends audioStreamEnd
        s.finish_started = Some(Instant::now());
        self.arm(FINISH_GRACE);
        self.overlay
            .set_phase(overlay::Phase::Finishing, "Finishing");
        self.publish();
    }

    /// (Re)arm the finish deadline, never beyond `FINISH_CAP` after release.
    fn arm(&mut self, after: Duration) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let after = match s.finish_started {
            Some(t) => after.min(FINISH_CAP.saturating_sub(t.elapsed())),
            None => after,
        };
        if let Some(t) = s.timer.take() {
            t.abort();
        }
        let tx = self.tx.clone();
        let id = s.id;
        s.timer = Some(tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = tx.send(AppEvent::Session(id, SessionEvent::Deadline));
        }));
    }

    fn session_event(&mut self, ev: SessionEvent) {
        let finishing = self
            .session
            .as_ref()
            .is_some_and(|s| s.finish_started.is_some());
        match ev {
            SessionEvent::Transcript(TranscriptEvent::Ready) => {
                eprintln!("[lalia] ready (streaming)");
                if let Some(s) = self.session.as_mut() {
                    s.ready = true;
                }
                if !finishing {
                    self.overlay.set_phase(
                        overlay::Phase::Listening,
                        &format!("Listening · {}", hotkey::label(&self.cfg.hotkey)),
                    );
                }
                self.publish();
            }
            SessionEvent::Transcript(TranscriptEvent::Interim(text)) => {
                eprint!("\r[lalia] … {text}                    ");
                if let Some(s) = self.session.as_mut() {
                    s.pending = text;
                }
                self.show_live_text();
                if finishing {
                    self.arm(FINISH_AFTER_INTERIM);
                }
            }
            SessionEvent::Transcript(TranscriptEvent::Final(text)) => {
                eprint!("\r");
                eprintln!("[lalia] ✓ {text}");
                if let Some(s) = self.session.as_mut() {
                    s.pending.clear();
                }
                self.commit(&text);
                self.show_live_text();
                if finishing {
                    self.arm(FINISH_AFTER_FINAL);
                }
            }
            SessionEvent::Transcript(TranscriptEvent::TurnComplete) => {
                if finishing {
                    self.finalize();
                }
            }
            SessionEvent::Transcript(TranscriptEvent::Error(e)) => {
                self.finalize();
                self.error(&e);
            }
            SessionEvent::Transcript(TranscriptEvent::Closed) | SessionEvent::Deadline => {
                self.finalize();
            }
            SessionEvent::AudioError(e) => {
                self.finalize();
                self.error(&e);
            }
        }
    }

    fn show_live_text(&self) {
        if let Some(s) = &self.session {
            let mut live = s.text.clone();
            if !s.pending.trim().is_empty() {
                if !live.is_empty() {
                    live.push(' ');
                }
                live.push_str(s.pending.trim());
            }
            self.overlay.set_text(&live);
        }
    }

    /// Add a confirmed piece of text to the session (typing it when streaming).
    fn commit(&mut self, raw: &str) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let piece = s.joiner.push(raw);
        if piece.is_empty() {
            return;
        }
        s.text.push_str(&piece);
        if self.cfg.insert == InsertMode::Streaming {
            self.injector.type_text(piece, self.cfg.type_delay_ms);
        }
    }

    /// End the session: keep any unconfirmed words, type, record, tidy up.
    fn finalize(&mut self) {
        let Some(pending) = self.session.as_ref().map(|s| s.pending.clone()) else {
            return;
        };
        if !pending.trim().is_empty() {
            eprintln!("\r[lalia] ✓ {pending} (unconfirmed)");
            self.commit(&pending);
        }
        let Some(mut s) = self.session.take() else {
            return;
        };
        if let Some(t) = s.timer.take() {
            t.abort();
        }
        s.gemini.abort();
        s.forwarder.abort();
        if let Some(c) = s.capture.take() {
            c.stop();
        }
        s.audio_tx = None;

        let text = s.text.trim().to_string();
        if self.cfg.insert == InsertMode::OnRelease && !text.is_empty() {
            self.injector
                .type_text(text.clone(), self.cfg.type_delay_ms);
        }
        let words = text.split_whitespace().count();
        if !text.is_empty() {
            if self.cfg.history {
                let entry = history::Entry::new(&text, s.started.elapsed().as_millis() as u64);
                if let Err(e) = history::append(&entry) {
                    eprintln!("[lalia] could not save history: {e}");
                }
            }
            self.recent.insert(0, text.clone());
            self.recent.truncate(RECENT);
        }
        notify::sound(Sound::Stop, self.cfg.sound);
        if words > 0 {
            self.overlay.flash(
                overlay::Phase::Done,
                &format!("Typed {words} word{}", if words == 1 { "" } else { "s" }),
                &text,
                0.9,
            );
        } else if s.ready {
            self.overlay
                .flash(overlay::Phase::Done, "Nothing heard", "", 0.8);
        } else {
            self.overlay.hide();
        }
        eprintln!("[lalia] ○ done");
        self.publish();
    }

    fn reload(&mut self) {
        match Config::load() {
            Ok(new) => {
                if new == self.cfg {
                    return;
                }
                let keys_changed =
                    new.hotkey != self.cfg.hotkey || new.pause_hotkey != self.cfg.pause_hotkey;
                let had_key = self.cfg.has_key();
                self.cfg = new;
                if keys_changed {
                    self.apply_hotkeys();
                }
                self.overlay.configure(
                    self.cfg.overlay,
                    self.cfg.overlay_position,
                    self.cfg.overlay_style,
                );
                if !had_key && self.cfg.has_key() {
                    self.last_error = None;
                    eprintln!("[lalia] API key added — ready");
                }
                eprintln!("[lalia] settings reloaded");
                self.publish();
            }
            Err(e) => self.error(&e.to_string()),
        }
    }

    fn persist(&mut self, change: impl FnOnce(&mut config::FileConfig)) {
        match config::update_file(change) {
            Ok(()) => self.reload(),
            Err(e) => self.error(&format!("could not save settings: {e}")),
        }
    }

    fn command(&mut self, cmd: Command) -> String {
        match cmd {
            Command::Status => return self.status_json(),
            Command::Toggle => match &self.session {
                Some(s) if s.finish_started.is_none() => self.begin_finish(),
                _ if !self.enabled => return "paused".into(),
                _ => self.main_down_force(),
            },
            Command::Start => {
                if self.session.is_none() {
                    if !self.enabled {
                        return "paused".into();
                    }
                    self.start_session();
                }
            }
            Command::Stop => self.begin_finish(),
            Command::Pause => self.set_enabled(false),
            Command::Resume => self.set_enabled(true),
            Command::TogglePause => self.set_enabled(!self.enabled),
            Command::Reload => self.reload(),
            Command::Quit => {
                self.finalize();
                let _ = std::fs::remove_file(ipc::daemon_socket());
                eprintln!("[lalia] bye");
                // Exit once the reply has gone out.
                tokio::spawn(async {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    std::process::exit(0);
                });
                return "bye".into();
            }
            Command::Settings(page) => gui::open(&page),
            Command::Copy(text) => {
                if self.clipboard.is_none() {
                    self.clipboard = arboard::Clipboard::new().ok();
                }
                match self.clipboard.as_mut().map(|c| c.set_text(text)) {
                    Some(Ok(())) => {}
                    _ => self.error("could not copy to the clipboard"),
                }
            }
            Command::SetMode(m) => self.persist(|f| f.mode = Some(m.as_str().into())),
            Command::SetInsert(i) => self.persist(|f| f.insert = Some(i)),
            Command::SetActivation(a) => self.persist(|f| f.activation = Some(a)),
            Command::SetOverlay(on) => self.persist(|f| f.overlay = Some(on)),
            Command::SetAutostart(on) => {
                if let Err(e) = autostart::set_enabled(on) {
                    self.error(&format!("could not change start-on-login: {e}"));
                }
                self.publish();
            }
        }
        "ok".into()
    }

    /// Start (or restart while flushing) regardless of the activation mode.
    fn main_down_force(&mut self) {
        if self.session.is_some() {
            self.finalize();
        }
        self.start_session();
    }
}

/// A short title and plain-language advice for an error message.
fn friendly_error(raw: &str) -> (&'static str, String) {
    let lower = raw.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    if has(&[
        "api key",
        "api_key",
        "permission denied",
        "unauthenticated",
        "403",
    ]) {
        (
            "API key problem",
            "Gemini rejected the API key. Open Lalia → Setup to add a new one.".into(),
        )
    } else if has(&["microphone", "input device", "unplugged"]) {
        (
            "Microphone unavailable",
            "Can't open the microphone. Check it's connected, or pick another one in Lalia → Dictation."
                .into(),
        )
    } else if has(&[
        "cannot reach gemini",
        "dns",
        "connection refused",
        "timed out",
        "network",
    ]) {
        (
            "Can't reach Gemini",
            "Check your internet connection and try again.".into(),
        )
    } else if has(&["quota", "resource_exhausted", "rate limit", "429"]) {
        (
            "Gemini quota reached",
            "Your API key is out of quota for now. Try again later or use another key.".into(),
        )
    } else if has(&["xdotool"]) {
        (
            "Can't type",
            "Typing needs xdotool: sudo apt install xdotool".into(),
        )
    } else if lower.contains("model") && has(&["not found", "not supported", "unknown"]) {
        (
            "Model unavailable",
            format!(
                "Gemini can't use this model ({raw}). Reset it in Lalia → Dictation → Advanced."
            ),
        )
    } else {
        ("Something went wrong", raw.to_string())
    }
}

/// Open the microphone and the Gemini stream for session `id`.
fn start_streams(cfg: &Config, id: u64, tx: &UnboundedSender<AppEvent>) -> anyhow::Result<Session> {
    let (audio_tx, audio_rx) = mpsc::unbounded_channel::<Vec<i16>>();
    let capture = {
        let tx = tx.clone();
        audio::start(&cfg.input_device, audio_tx.clone(), move |e| {
            let _ = tx.send(AppEvent::Session(id, SessionEvent::AudioError(e)));
        })?
    };

    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<TranscriptEvent>();
    let gemini = gemini::spawn(cfg.clone(), audio_rx, events_tx);
    let forwarder = {
        let tx = tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = events_rx.recv().await {
                if tx
                    .send(AppEvent::Session(id, SessionEvent::Transcript(ev)))
                    .is_err()
                {
                    return;
                }
            }
        })
    };

    Ok(Session {
        id,
        capture: Some(capture),
        audio_tx: Some(audio_tx),
        gemini,
        forwarder,
        timer: None,
        joiner: Joiner::default(),
        text: String::new(),
        pending: String::new(),
        started: Instant::now(),
        finish_started: None,
        ready: false,
    })
}

/// `lalia selftest N`: capture N seconds, print transcripts, type nothing.
pub async fn selftest(cfg: Config, secs: u64) -> anyhow::Result<()> {
    cfg.require_key()?;
    println!("selftest: capturing for {secs}s — speak now (no text is typed)…");
    let (audio_tx, audio_rx) = mpsc::unbounded_channel::<Vec<i16>>();
    let capture = audio::start(&cfg.input_device, audio_tx.clone(), |e| {
        eprintln!("[lalia] {e}")
    })?;
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<TranscriptEvent>();
    let task = gemini::spawn(cfg, audio_rx, events_tx);

    let mut joiner = Joiner::default();
    let mut text = String::new();
    let mut pending = String::new();
    let stop_at = tokio::time::Instant::now() + Duration::from_secs(secs);
    let mut audio_tx = Some(audio_tx);
    let mut deadline = stop_at + FINISH_CAP;
    loop {
        let wait_until = if audio_tx.is_some() {
            stop_at
        } else {
            deadline
        };
        let ev = tokio::select! {
            ev = events_rx.recv() => ev,
            _ = tokio::time::sleep_until(wait_until) => {
                if audio_tx.take().is_some() {
                    capture.stop();
                    println!("\nselftest: stopped recording, waiting for the last words…");
                    deadline = tokio::time::Instant::now() + FINISH_GRACE;
                    continue;
                }
                break;
            }
        };
        let Some(ev) = ev else { break };
        match ev {
            TranscriptEvent::Ready => eprintln!("[lalia] ready (streaming)"),
            TranscriptEvent::Interim(t) => {
                eprint!("\r[lalia] … {t}                    ");
                pending = t;
                if audio_tx.is_none() {
                    deadline = tokio::time::Instant::now() + FINISH_AFTER_INTERIM;
                }
            }
            TranscriptEvent::Final(t) => {
                eprintln!("\r[lalia] ✓ {t}");
                pending.clear();
                text.push_str(&joiner.push(&t));
                if audio_tx.is_none() {
                    deadline = tokio::time::Instant::now() + FINISH_AFTER_FINAL;
                }
            }
            TranscriptEvent::TurnComplete if audio_tx.is_none() => break,
            TranscriptEvent::TurnComplete => {}
            TranscriptEvent::Error(e) => eprintln!("\n[lalia] error: {e}"),
            TranscriptEvent::Closed => break,
        }
    }
    task.abort();
    if !pending.trim().is_empty() {
        text.push_str(&joiner.push(&pending));
    }
    println!(
        "\nselftest result: {}",
        if text.is_empty() {
            "(nothing heard)"
        } else {
            &text
        }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(Command::parse("status\n"), Some(Command::Status));
        assert_eq!(Command::parse("TOGGLE"), Some(Command::Toggle));
        assert_eq!(Command::parse("toggle-pause"), Some(Command::TogglePause));
        assert_eq!(
            Command::parse("settings history"),
            Some(Command::Settings("history".into()))
        );
        assert_eq!(
            Command::parse("settings"),
            Some(Command::Settings("home".into()))
        );
        assert_eq!(Command::parse("rm -rf"), None);
        assert_eq!(Command::parse(""), None);
    }

    #[test]
    fn errors_get_plain_advice() {
        assert_eq!(
            friendly_error("API key not valid. Please pass a valid API key.").0,
            "API key problem"
        );
        assert_eq!(
            friendly_error("could not start: cannot open microphone: busy").0,
            "Microphone unavailable"
        );
        assert_eq!(
            friendly_error("cannot reach Gemini: dns error").0,
            "Can't reach Gemini"
        );
        let (title, advice) = friendly_error("weird");
        assert_eq!((title, advice.as_str()), ("Something went wrong", "weird"));
    }

    #[test]
    fn status_round_trips() {
        let s = DaemonStatus {
            version: "1".into(),
            pid: 1,
            phase: Phase::NeedsSetup,
            hotkey: "F12".into(),
            pause_hotkey: "F11".into(),
            error: None,
        };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"needs_setup\""));
        let back: DaemonStatus = serde_json::from_str(&text).unwrap();
        assert_eq!(back.phase, Phase::NeedsSetup);
    }
}
