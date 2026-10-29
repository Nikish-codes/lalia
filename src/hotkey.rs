//! Global hotkey detection for X11.
//!
//! Rather than grabbing keys (which steals them from apps), we poll the X
//! server's keyboard bitmap ~60x/second. The function keys do nothing on their
//! own in most apps, so this is safe and needs no root or `input` group.

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

/// X11 keysym for an "F<n>" key name (F1 = 0xFFBE ... F35 = 0xFFE0).
fn func_keysym(name: &str) -> Option<u32> {
    let n: u32 = name.strip_prefix(['F', 'f'])?.parse().ok()?;
    (1..=35).contains(&n).then(|| 0xFFBE + (n - 1))
}

fn keysym_for(name: &str) -> u32 {
    func_keysym(name).unwrap_or(0xFFC9) // default F12
}

/// Spawn a background thread watching `key_name`, reporting edges as `KeyId`.
pub fn spawn(key_name: &str, id: KeyId, tx: UnboundedSender<HotkeyEvent>) -> anyhow::Result<()> {
    let keysym = keysym_for(key_name);
    let key_name = key_name.to_string();

    let (conn, _screen) = x11rb::connect(None)?;
    let setup = conn.setup();
    let min = setup.min_keycode;
    let max = setup.max_keycode;
    let count = max - min + 1;
    let mapping = conn.get_keyboard_mapping(min, count)?.reply()?;
    let per = mapping.keysyms_per_keycode as usize;
    let keycode = mapping
        .keysyms
        .chunks(per)
        .position(|chunk| chunk.contains(&keysym))
        .map(|idx| min + idx as u8)
        .ok_or_else(|| anyhow::anyhow!("hotkey '{key_name}' not found on this keyboard layout"))?;
    drop(conn);

    thread::spawn(move || {
        let (conn, _screen) = match x11rb::connect(None) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[lalia] X11 reconnect failed: {e}");
                return;
            }
        };
        let mut was_down = false;
        loop {
            let down = match conn.query_keymap() {
                Ok(cookie) => match cookie.reply() {
                    Ok(reply) => reply.keys[(keycode / 8) as usize] & (1 << (keycode % 8)) != 0,
                    Err(_) => false,
                },
                Err(_) => false,
            };
            if down != was_down {
                was_down = down;
                if tx.send(HotkeyEvent { key: id, down }).is_err() {
                    return; // receiver dropped: shutting down
                }
            }
            thread::sleep(Duration::from_millis(16));
        }
    });

    Ok(())
}
