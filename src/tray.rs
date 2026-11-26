//! System-tray icon: colour shows the state; left-click opens the settings
//! window, middle-click pauses/resumes, and the menu holds the quick toggles.
//!
//! This file describes *what* the tray shows (state, headline, menu) so both
//! platforms offer exactly the same thing. Showing it is platform work:
//! StatusNotifierItem over D-Bus on Linux (`sni.rs`), a notification-area
//! icon on Windows (`windows.rs`).

#[cfg(unix)]
mod sni;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use sni::{remove, spawn, update, Handle};
#[cfg(windows)]
pub use windows::{balloon, remove, spawn, update, Handle};

use crate::app::{Command, Phase};
use crate::config::{Activation, InsertMode, Mode};
use crate::hotkey;
use crate::icon::IconState;

/// What the tray shows; the daemon pushes a fresh copy on every change.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayState {
    pub phase: Phase,
    pub hotkey: String,
    pub pause_hotkey: String,
    pub activation: Activation,
    pub mode: Mode,
    pub insert: InsertMode,
    pub overlay: bool,
    pub autostart: bool,
    pub recent: Vec<String>,
    pub error: Option<String>,
}

/// One entry of the tray menu.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// Greyed-out text (the status line).
    Label(String),
    Action {
        label: String,
        /// Freedesktop icon name (used where the desktop shows menu icons).
        icon: &'static str,
        enabled: bool,
        command: Command,
    },
    Check {
        label: String,
        checked: bool,
        command: Command,
    },
    /// A submenu of mutually exclusive options.
    Radio {
        label: String,
        selected: usize,
        options: Vec<(String, Command)>,
    },
    Submenu {
        label: String,
        icon: &'static str,
        items: Vec<Item>,
    },
    Separator,
}

impl TrayState {
    pub fn icon_state(&self) -> IconState {
        match self.phase {
            Phase::Idle if self.error.is_some() => IconState::Attention,
            Phase::Idle => IconState::Idle,
            Phase::Listening => IconState::Listening,
            Phase::Connecting | Phase::Finishing => IconState::Busy,
            Phase::Paused => IconState::Paused,
            Phase::NeedsSetup => IconState::Attention,
        }
    }

    pub fn headline(&self) -> String {
        let key = hotkey::label(&self.hotkey);
        match self.phase {
            Phase::Idle => match self.activation {
                Activation::Hold => format!("Ready — hold {key} to dictate"),
                Activation::Toggle => format!("Ready — tap {key} to dictate"),
            },
            Phase::Connecting => "Connecting…".into(),
            Phase::Listening => "Listening…".into(),
            Phase::Finishing => "Finishing…".into(),
            Phase::Paused => format!(
                "Paused — tap {} to resume",
                hotkey::label(&self.pause_hotkey)
            ),
            Phase::NeedsSetup => "Setup needed — add your API key".into(),
        }
    }

    /// Tooltip text: the headline, plus the last error if there is one.
    pub fn tooltip(&self) -> String {
        match &self.error {
            Some(e) => format!("{}\nLast error: {e}", self.headline()),
            None => self.headline(),
        }
    }

    pub fn menu(&self) -> Vec<Item> {
        let active = matches!(self.phase, Phase::Connecting | Phase::Listening);
        let key = hotkey::label(&self.hotkey);
        let mut items = vec![Item::Label(self.headline())];
        if self.phase == Phase::NeedsSetup {
            items.push(Item::Action {
                label: "Set up Lalia…".into(),
                icon: "dialog-password",
                enabled: true,
                command: Command::Settings("setup".into()),
            });
        } else {
            items.push(Item::Action {
                label: if active {
                    "Stop dictation".into()
                } else {
                    "Start dictation".into()
                },
                icon: if active {
                    "media-playback-stop"
                } else {
                    "media-record"
                },
                enabled: self.phase != Phase::Paused && self.phase != Phase::Finishing,
                command: Command::Toggle,
            });
            items.push(Item::Check {
                label: format!("Paused ({})", hotkey::label(&self.pause_hotkey)),
                checked: self.phase == Phase::Paused,
                command: Command::TogglePause,
            });
        }
        items.push(Item::Separator);
        items.push(Item::Radio {
            label: "Mode".into(),
            selected: (self.mode == Mode::Smart) as usize,
            options: vec![
                (
                    "Verbatim — exactly what you say".into(),
                    Command::SetMode(Mode::Verbatim),
                ),
                (
                    "Smart — punctuation & clean-up".into(),
                    Command::SetMode(Mode::Smart),
                ),
            ],
        });
        items.push(Item::Radio {
            label: "Insert text".into(),
            selected: (self.insert == InsertMode::OnRelease) as usize,
            options: vec![
                (
                    "As I speak".into(),
                    Command::SetInsert(InsertMode::Streaming),
                ),
                (
                    "All at once when I finish".into(),
                    Command::SetInsert(InsertMode::OnRelease),
                ),
            ],
        });
        items.push(Item::Radio {
            label: "Hotkey".into(),
            selected: (self.activation == Activation::Toggle) as usize,
            options: vec![
                (
                    format!("Hold {key} to talk"),
                    Command::SetActivation(Activation::Hold),
                ),
                (
                    format!("Tap {key} to start / stop"),
                    Command::SetActivation(Activation::Toggle),
                ),
            ],
        });
        items.push(Item::Check {
            label: "On-screen indicator".into(),
            checked: self.overlay,
            command: Command::SetOverlay(!self.overlay),
        });
        items.push(Item::Separator);
        if !self.recent.is_empty() {
            items.push(Item::Submenu {
                label: "Copy recent".into(),
                icon: "document-open-recent",
                items: self
                    .recent
                    .iter()
                    .map(|text| Item::Action {
                        label: short(text, 48),
                        icon: "edit-copy",
                        enabled: true,
                        command: Command::Copy(text.clone()),
                    })
                    .collect(),
            });
        }
        items.push(Item::Action {
            label: "History…".into(),
            icon: "document-open-recent",
            enabled: true,
            command: Command::Settings("history".into()),
        });
        items.push(Item::Action {
            label: "Settings…".into(),
            icon: "preferences-system",
            enabled: true,
            command: Command::Settings("home".into()),
        });
        items.push(Item::Check {
            label: "Start on login".into(),
            checked: self.autostart,
            command: Command::SetAutostart(!self.autostart),
        });
        items.push(Item::Separator);
        items.push(Item::Action {
            label: "Quit Lalia".into(),
            icon: "application-exit",
            enabled: true,
            command: Command::Quit,
        });
        items
    }
}

fn short(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(phase: Phase) -> TrayState {
        TrayState {
            phase,
            hotkey: "F12".into(),
            pause_hotkey: "F11".into(),
            activation: Activation::Hold,
            mode: Mode::Smart,
            insert: InsertMode::Streaming,
            overlay: true,
            autostart: false,
            recent: vec!["hello there".into()],
            error: None,
        }
    }

    #[test]
    fn short_labels() {
        assert_eq!(short("hello   world", 48), "hello world");
        let long = "word ".repeat(30);
        let s = short(&long, 20);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= 20);
    }

    #[test]
    fn menu_reflects_state() {
        let menu = state(Phase::Idle).menu();
        assert_eq!(menu[0], Item::Label("Ready — hold F12 to dictate".into()));
        assert!(menu.iter().any(|i| matches!(
            i,
            Item::Radio { label, selected: 1, .. } if label == "Mode"
        )));
        assert!(menu.iter().any(|i| matches!(
            i,
            Item::Check { label, checked: true, command: Command::SetOverlay(false) }
                if label == "On-screen indicator"
        )));
        assert!(menu
            .iter()
            .any(|i| matches!(i, Item::Submenu { label, .. } if label == "Copy recent")));
        assert_eq!(
            menu.last(),
            Some(&Item::Action {
                label: "Quit Lalia".into(),
                icon: "application-exit",
                enabled: true,
                command: Command::Quit
            })
        );

        let setup = state(Phase::NeedsSetup).menu();
        assert!(
            matches!(&setup[1], Item::Action { command: Command::Settings(p), .. } if p == "setup")
        );
        assert_eq!(state(Phase::NeedsSetup).icon_state(), IconState::Attention);
        assert_eq!(state(Phase::Listening).icon_state(), IconState::Listening);
    }
}
