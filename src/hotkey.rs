//! Global hotkey detection for X11.
//!
//! Rather than grabbing keys (which steals them from apps), we poll the X
//! server's keyboard bitmap ~60x/second. The keys we offer do nothing on their
//! own in most apps, so this is safe and needs no root or `input` group.
//!
//! One thread watches every configured key; `Hotkeys::set` swaps the keys
//! live when the settings change.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyId {
    /// The main dictate key.
    Main,
    /// The pause/resume key.
    Pause,
}

#[derive(Debug, Clone, Copy)]
pub struct HotkeyEvent {
    pub key: KeyId,
    pub down: bool,
}

/// Keys offered in the settings window: (config name, label).
pub const CHOICES: &[(&str, &str)] = &[
    ("F1", "F1"),
    ("F2", "F2"),
    ("F3", "F3"),
    ("F4", "F4"),
    ("F5", "F5"),
    ("F6", "F6"),
    ("F7", "F7"),
    ("F8", "F8"),
    ("F9", "F9"),
    ("F10", "F10"),
    ("F11", "F11"),
    ("F12", "F12"),
    ("F13", "F13"),
    ("F14", "F14"),
    ("F15", "F15"),
    ("F16", "F16"),
    ("F17", "F17"),
    ("F18", "F18"),
    ("F19", "F19"),
    ("F20", "F20"),
    ("Pause", "Pause / Break"),
    ("ScrollLock", "Scroll Lock"),
    ("Insert", "Insert"),
    ("Menu", "Menu"),
    ("RightCtrl", "Right Ctrl"),
    ("RightAlt", "Right Alt / AltGr"),
    ("RightSuper", "Right Super"),
];

/// Human label for a configured key name.
pub fn label(name: &str) -> String {
    let norm = normalize(name);
    CHOICES
        .iter()
        .find(|(n, _)| normalize(n) == norm)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| name.to_string())
}

fn normalize(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .replace(['_', '-', ' '], "")
}

/// X11 keysyms for a key name (several when layouts differ, e.g. AltGr).
pub fn keysyms(name: &str) -> Option<Vec<u32>> {
    let norm = normalize(name);
    if let Some(n) = norm.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        // F1 = 0xFFBE ... F35 = 0xFFE0
        return (1..=35).contains(&n).then(|| vec![0xFFBE + (n - 1)]);
    }
    let syms = match norm.as_str() {
        "pause" | "break" => vec![0xFF13],
        "scrolllock" => vec![0xFF14],
        "insert" => vec![0xFF63],
        "menu" => vec![0xFF67],
        "rightctrl" | "controlr" | "ctrlr" => vec![0xFFE4],
        "rightalt" | "altr" | "altgr" => vec![0xFFEA, 0xFE03],
        "rightsuper" | "superr" => vec![0xFFEC],
        _ => return None,
    };
    Some(syms)
}

type Watched = Vec<(KeyId, u8)>;

/// Handle to the polling thread.
#[derive(Clone)]
pub struct Hotkeys {
    keys: Arc<Mutex<Watched>>,
}

impl Hotkeys {
    /// Start the polling thread (watching nothing until `set`).
    pub fn start(tx: UnboundedSender<HotkeyEvent>) -> anyhow::Result<Self> {
        let (conn, _) = x11rb::connect(None)
            .map_err(|e| anyhow::anyhow!("cannot connect to the X server: {e}"))?;
        let keys: Arc<Mutex<Watched>> = Arc::default();
        let watched = keys.clone();
        thread::spawn(move || {
            let mut state: Vec<(KeyId, u8, bool)> = Vec::new();
            loop {
                let wanted = watched.lock().map(|k| k.clone()).unwrap_or_default();
                if wanted.len() != state.len()
                    || wanted
                        .iter()
                        .zip(&state)
                        .any(|(a, b)| a.0 != b.0 || a.1 != b.1)
                {
                    // Keys changed: forget old edges (a held key is reported once).
                    state = wanted.iter().map(|&(id, kc)| (id, kc, false)).collect();
                }
                let reply = conn.query_keymap().ok().and_then(|c| c.reply().ok());
                let Some(reply) = reply else {
                    thread::sleep(Duration::from_millis(500));
                    continue;
                };
                for (id, kc, was_down) in &mut state {
                    let down = reply.keys[(*kc / 8) as usize] & (1 << (*kc % 8)) != 0;
                    if down != *was_down {
                        *was_down = down;
                        if tx.send(HotkeyEvent { key: *id, down }).is_err() {
                            return; // receiver dropped: shutting down
                        }
                    }
                }
                thread::sleep(Duration::from_millis(16));
            }
        });
        Ok(Self { keys })
    }

    /// Watch `main` (dictate) and `pause`. Errors leave the old keys in place.
    pub fn set(&self, main: &str, pause: &str) -> anyhow::Result<()> {
        let main_kc = keycode(main)?;
        let mut watched = vec![(KeyId::Main, main_kc)];
        let pause_kc = keycode(pause)?;
        if pause_kc != main_kc {
            watched.push((KeyId::Pause, pause_kc));
        }
        if let Ok(mut k) = self.keys.lock() {
            *k = watched;
        }
        Ok(())
    }
}

/// Resolve a key name to a keycode on the current keyboard layout.
pub fn keycode(name: &str) -> anyhow::Result<u8> {
    let syms = keysyms(name).ok_or_else(|| anyhow::anyhow!("unknown hotkey '{name}'"))?;
    let (conn, _) = x11rb::connect(None)?;
    let setup = conn.setup();
    let min = setup.min_keycode;
    let count = setup.max_keycode - min + 1;
    let mapping = conn.get_keyboard_mapping(min, count)?.reply()?;
    let per = mapping.keysyms_per_keycode.max(1) as usize;
    syms.iter()
        .find_map(|sym| {
            mapping
                .keysyms
                .chunks(per)
                .position(|chunk| chunk.contains(sym))
        })
        .map(|idx| min + idx as u8)
        .ok_or_else(|| anyhow::anyhow!("hotkey '{name}' not found on this keyboard layout"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_keys() {
        assert_eq!(keysyms("F1"), Some(vec![0xFFBE]));
        assert_eq!(keysyms("f12"), Some(vec![0xFFC9]));
        assert_eq!(keysyms("F35"), Some(vec![0xFFE0]));
        assert_eq!(keysyms("F36"), None);
        assert_eq!(keysyms("F0"), None);
    }

    #[test]
    fn named_keys() {
        assert_eq!(keysyms("RightCtrl"), Some(vec![0xFFE4]));
        assert_eq!(keysyms("right_ctrl"), Some(vec![0xFFE4]));
        assert_eq!(keysyms("Scroll Lock"), Some(vec![0xFF14]));
        assert!(keysyms("RightAlt").unwrap().contains(&0xFE03));
        assert_eq!(keysyms("Space"), None);
    }

    #[test]
    fn every_choice_resolves() {
        for (name, _) in CHOICES {
            assert!(keysyms(name).is_some(), "{name}");
        }
        assert_eq!(label("rightctrl"), "Right Ctrl");
        assert_eq!(label("F12"), "F12");
    }
}
