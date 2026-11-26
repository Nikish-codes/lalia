//! The tray on Linux: StatusNotifierItem over D-Bus. Works in Cinnamon, KDE,
//! XFCE, MATE and GNOME with the AppIndicator extension (Ubuntu's default).

use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip, TrayMethods};
use tokio::sync::mpsc::UnboundedSender;

use super::{Item, TrayState};
use crate::app::{AppEvent, Command};
use crate::icon;

pub struct LaliaTray {
    state: TrayState,
    tx: UnboundedSender<AppEvent>,
}

impl LaliaTray {
    fn send(&self, cmd: Command) {
        let _ = self.tx.send(AppEvent::Command(cmd, None));
    }
}

fn convert(item: Item) -> MenuItem<LaliaTray> {
    match item {
        Item::Label(label) => StandardItem {
            label,
            enabled: false,
            ..Default::default()
        }
        .into(),
        Item::Action {
            label,
            icon,
            enabled,
            command,
        } => StandardItem {
            label,
            icon_name: icon.into(),
            enabled,
            activate: Box::new(move |t: &mut LaliaTray| t.send(command.clone())),
            ..Default::default()
        }
        .into(),
        Item::Check {
            label,
            checked,
            command,
        } => CheckmarkItem {
            label,
            checked,
            activate: Box::new(move |t: &mut LaliaTray| t.send(command.clone())),
            ..Default::default()
        }
        .into(),
        Item::Radio {
            label,
            selected,
            options,
        } => {
            let commands: Vec<Command> = options.iter().map(|(_, c)| c.clone()).collect();
            SubMenu {
                label,
                submenu: vec![RadioGroup {
                    selected,
                    select: Box::new(move |t: &mut LaliaTray, i| {
                        if let Some(c) = commands.get(i) {
                            t.send(c.clone());
                        }
                    }),
                    options: options
                        .into_iter()
                        .map(|(label, _)| RadioItem {
                            label,
                            ..Default::default()
                        })
                        .collect(),
                }
                .into()],
                ..Default::default()
            }
            .into()
        }
        Item::Submenu { label, icon, items } => SubMenu {
            label,
            icon_name: icon.into(),
            submenu: items.into_iter().map(convert).collect(),
            ..Default::default()
        }
        .into(),
        Item::Separator => MenuItem::Separator,
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
        let st = self.state.icon_state();
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
        ToolTip {
            title: "Lalia — live dictation".into(),
            description: self.state.tooltip(),
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
        self.state.menu().into_iter().map(convert).collect()
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

/// Take the icon away (before the process exits).
pub fn remove(handle: &Handle) {
    // The returned future only waits for the shutdown to finish.
    drop(handle.shutdown());
}
