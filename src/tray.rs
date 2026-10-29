//! System-tray icon (StatusNotifierItem over D-Bus): works in Cinnamon, KDE,
//! XFCE, MATE and GNOME with the AppIndicator extension (Ubuntu's default).
//!
//! The icon changes colour with the state; left-click opens the settings
//! window, middle-click pauses/resumes, and the menu holds the quick toggles.

use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip, TrayMethods};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::{AppEvent, Command, Phase};
use crate::config::{Activation, InsertMode, Mode};
use crate::hotkey;
use crate::icon::{self, IconState};

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

pub struct LaliaTray {
    pub state: TrayState,
    tx: UnboundedSender<AppEvent>,
}

impl LaliaTray {
    fn send(&self, cmd: Command) {
        let _ = self.tx.send(AppEvent::Command(cmd, None));
    }

    fn icon_state(&self) -> IconState {
        match self.state.phase {
            Phase::Idle if self.state.error.is_some() => IconState::Attention,
            Phase::Idle => IconState::Idle,
            Phase::Listening => IconState::Listening,
            Phase::Connecting | Phase::Finishing => IconState::Busy,
            Phase::Paused => IconState::Paused,
            Phase::NeedsSetup => IconState::Attention,
        }
    }

    fn headline(&self) -> String {
        let key = hotkey::label(&self.state.hotkey);
        match self.state.phase {
            Phase::Idle => match self.state.activation {
                Activation::Hold => format!("Ready — hold {key} to dictate"),
                Activation::Toggle => format!("Ready — tap {key} to dictate"),
            },
            Phase::Connecting => "Connecting…".into(),
            Phase::Listening => "Listening…".into(),
            Phase::Finishing => "Finishing…".into(),
            Phase::Paused => format!(
                "Paused — tap {} to resume",
                hotkey::label(&self.state.pause_hotkey)
            ),
            Phase::NeedsSetup => "Setup needed — add your API key".into(),
        }
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

impl ksni::Tray for LaliaTray {
    fn id(&self) -> String {
        "lalia".into()
    }

    fn title(&self) -> String {
        "Lalia".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let st = self.icon_state();
        [22u32, 32, 48, 64]
            .iter()
            .map(|&size| {
                let pm = icon::render(size, st);
                Icon {
                    width: size as i32,
                    height: size as i32,
                    data: icon::argb(&pm),
                }
            })
            .collect()
    }

    fn tool_tip(&self) -> ToolTip {
        let mut description = self.headline();
        if let Some(e) = &self.state.error {
            description.push_str(&format!("\nLast error: {e}"));
        }
        ToolTip {
            title: "Lalia — live dictation".into(),
            description,
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::Settings("home".into()));
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::TogglePause);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let st = &self.state;
        let active = matches!(st.phase, Phase::Connecting | Phase::Listening);
        let mut items: Vec<MenuItem<Self>> = vec![StandardItem {
            label: self.headline(),
            enabled: false,
            ..Default::default()
        }
        .into()];
        if st.phase == Phase::NeedsSetup {
            items.push(
                StandardItem {
                    label: "Set up Lalia…".into(),
                    icon_name: "dialog-password".into(),
                    activate: Box::new(|t: &mut Self| t.send(Command::Settings("setup".into()))),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            items.push(
                StandardItem {
                    label: if active {
                        "Stop dictation".into()
                    } else {
                        "Start dictation".into()
                    },
                    icon_name: if active {
                        "media-playback-stop".into()
                    } else {
                        "media-record".into()
                    },
                    enabled: st.phase != Phase::Paused && st.phase != Phase::Finishing,
                    activate: Box::new(|t: &mut Self| t.send(Command::Toggle)),
                    ..Default::default()
                }
                .into(),
            );
            items.push(
                CheckmarkItem {
                    label: format!("Paused ({})", hotkey::label(&st.pause_hotkey)),
                    checked: st.phase == Phase::Paused,
                    activate: Box::new(|t: &mut Self| t.send(Command::TogglePause)),
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(MenuItem::Separator);

        items.push(
            SubMenu {
                label: "Mode".into(),
                submenu: vec![RadioGroup {
                    selected: (st.mode == Mode::Smart) as usize,
                    select: Box::new(|t: &mut Self, i| {
                        t.send(Command::SetMode(if i == 1 {
                            Mode::Smart
                        } else {
                            Mode::Verbatim
                        }))
                    }),
                    options: vec![
                        RadioItem {
                            label: "Verbatim — exactly what you say".into(),
                            ..Default::default()
                        },
                        RadioItem {
                            label: "Smart — punctuation & clean-up".into(),
                            ..Default::default()
                        },
                    ],
                }
                .into()],
                ..Default::default()
            }
            .into(),
        );
        items.push(
            SubMenu {
                label: "Insert text".into(),
                submenu: vec![RadioGroup {
                    selected: (st.insert == InsertMode::OnRelease) as usize,
                    select: Box::new(|t: &mut Self, i| {
                        t.send(Command::SetInsert(if i == 1 {
                            InsertMode::OnRelease
                        } else {
                            InsertMode::Streaming
                        }))
                    }),
                    options: vec![
                        RadioItem {
                            label: "As I speak".into(),
                            ..Default::default()
                        },
                        RadioItem {
                            label: "All at once when I finish".into(),
                            ..Default::default()
                        },
                    ],
                }
                .into()],
                ..Default::default()
            }
            .into(),
        );
        items.push(
            SubMenu {
                label: "Hotkey".into(),
                submenu: vec![RadioGroup {
                    selected: (st.activation == Activation::Toggle) as usize,
                    select: Box::new(|t: &mut Self, i| {
                        t.send(Command::SetActivation(if i == 1 {
                            Activation::Toggle
                        } else {
                            Activation::Hold
                        }))
                    }),
                    options: vec![
                        RadioItem {
                            label: format!("Hold {} to talk", hotkey::label(&st.hotkey)),
                            ..Default::default()
                        },
                        RadioItem {
                            label: format!("Tap {} to start / stop", hotkey::label(&st.hotkey)),
                            ..Default::default()
                        },
                    ],
                }
                .into()],
                ..Default::default()
            }
            .into(),
        );
        items.push(
            CheckmarkItem {
                label: "On-screen indicator".into(),
                checked: st.overlay,
                activate: Box::new(|t: &mut Self| {
                    let on = !t.state.overlay;
                    t.send(Command::SetOverlay(on))
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);

        if !st.recent.is_empty() {
            let recent: Vec<MenuItem<Self>> = st
                .recent
                .iter()
                .map(|text| {
                    let full = text.clone();
                    StandardItem {
                        label: short(text, 48),
                        icon_name: "edit-copy".into(),
                        activate: Box::new(move |t: &mut Self| t.send(Command::Copy(full.clone()))),
                        ..Default::default()
                    }
                    .into()
                })
                .collect();
            items.push(
                SubMenu {
                    label: "Copy recent".into(),
                    icon_name: "document-open-recent".into(),
                    submenu: recent,
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(
            StandardItem {
                label: "History…".into(),
                icon_name: "document-open-recent".into(),
                activate: Box::new(|t: &mut Self| t.send(Command::Settings("history".into()))),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Settings…".into(),
                icon_name: "preferences-system".into(),
                activate: Box::new(|t: &mut Self| t.send(Command::Settings("home".into()))),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            CheckmarkItem {
                label: "Start on login".into(),
                checked: st.autostart,
                activate: Box::new(|t: &mut Self| {
                    let on = !t.state.autostart;
                    t.send(Command::SetAutostart(on))
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Quit Lalia".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| t.send(Command::Quit)),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

pub type Handle = ksni::Handle<LaliaTray>;

/// Register the tray icon. Works even if the panel starts after Lalia.
pub async fn spawn(state: TrayState, tx: UnboundedSender<AppEvent>) -> Option<Handle> {
    let tray = LaliaTray { state, tx };
    match tray.assume_sni_available(true).spawn().await {
        Ok(handle) => Some(handle),
        Err(e) => {
            eprintln!("[lalia] tray icon unavailable: {e}");
            None
        }
    }
}

/// Push a new state to the tray (no-op when unchanged).
pub fn update(handle: &Handle, state: TrayState) {
    let handle = handle.clone();
    tokio::spawn(async move {
        handle
            .update(move |t| {
                if t.state != state {
                    t.state = state;
                }
            })
            .await;
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn short_labels() {
        assert_eq!(super::short("hello   world", 48), "hello world");
        let long = "word ".repeat(30);
        let s = super::short(&long, 20);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= 20);
    }
}
