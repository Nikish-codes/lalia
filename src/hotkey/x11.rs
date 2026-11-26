//! Hotkeys on X11.
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

use super::{parse, HotkeyEvent, Key, KeyId};

/// X11 keysyms for a key (several when layouts differ, e.g. AltGr).
fn keysyms(key: Key) -> Vec<u32> {
    match key {
        // F1 = 0xFFBE ... F35 = 0xFFE0
        Key::F(n) => vec![0xFFBE + (n as u32 - 1)],
        Key::Pause => vec![0xFF13],
        Key::ScrollLock => vec![0xFF14],
        Key::Insert => vec![0xFF63],
        Key::Menu => vec![0xFF67],
        Key::RightCtrl => vec![0xFFE4],
        Key::RightAlt => vec![0xFFEA, 0xFE03],
        Key::RightSuper => vec![0xFFEC],
    }
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

    /// Keys are never taken from apps on X11, so pausing changes nothing here.
    pub fn set_paused(&self, _paused: bool) {}
}

/// Resolve a key name to a keycode on the current keyboard layout.
fn keycode(name: &str) -> anyhow::Result<u8> {
    let key = parse(name).ok_or_else(|| anyhow::anyhow!("unknown hotkey '{name}'"))?;
    let (conn, _) = x11rb::connect(None)?;
    let setup = conn.setup();
    let min = setup.min_keycode;
    let count = setup.max_keycode - min + 1;
    let mapping = conn.get_keyboard_mapping(min, count)?.reply()?;
    let per = mapping.keysyms_per_keycode.max(1) as usize;
    keysyms(key)
        .iter()
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
    fn keysym_values() {
        assert_eq!(keysyms(Key::F(1)), vec![0xFFBE]);
        assert_eq!(keysyms(Key::F(12)), vec![0xFFC9]);
        assert_eq!(keysyms(Key::F(35)), vec![0xFFE0]);
        assert!(keysyms(Key::RightAlt).contains(&0xFE03));
    }
}
