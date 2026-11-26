//! Hotkeys on Windows: a low-level keyboard hook.
//!
//! The hook sees every key before the focused app does. For our two keys it
//! reports the press/release and then swallows the event, so holding F12 to
//! dictate does not also open DevTools or "Save As". While Lalia is paused the
//! dictate key is passed through untouched (only the pause key stays ours).
//!
//! Windows calls the hook on the thread that installed it, which therefore
//! runs a message loop and does nothing else; the callback only touches
//! atomics and a channel, so it always returns immediately.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, OnceLock};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, HC_ACTION,
    KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use super::{parse, HotkeyEvent, Key, KeyId};

static MAIN_VK: AtomicU32 = AtomicU32::new(0);
static PAUSE_VK: AtomicU32 = AtomicU32::new(0);
static MAIN_DOWN: AtomicBool = AtomicBool::new(false);
static PAUSE_DOWN: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static EVENTS: OnceLock<UnboundedSender<HotkeyEvent>> = OnceLock::new();

/// Virtual-key code for a key.
fn vk(key: Key) -> Option<u32> {
    Some(match key {
        Key::F(n @ 1..=24) => 0x70 + (n as u32 - 1), // VK_F1 … VK_F24
        Key::F(_) => return None,
        Key::Pause => 0x13,      // VK_PAUSE
        Key::ScrollLock => 0x91, // VK_SCROLL
        Key::Insert => 0x2D,     // VK_INSERT
        Key::Menu => 0x5D,       // VK_APPS
        Key::RightCtrl => 0xA3,  // VK_RCONTROL
        Key::RightAlt => 0xA5,   // VK_RMENU
        Key::RightSuper => 0x5C, // VK_RWIN
    })
}

fn resolve(name: &str) -> anyhow::Result<u32> {
    let key = parse(name).ok_or_else(|| anyhow::anyhow!("unknown hotkey '{name}'"))?;
    vk(key).ok_or_else(|| anyhow::anyhow!("hotkey '{name}' does not exist on Windows keyboards"))
}

/// Report a press or release once (key repeat sends many "down"s).
fn edge(state: &AtomicBool, key: KeyId, down: bool) {
    if state.swap(down, Ordering::Relaxed) != down {
        if let Some(tx) = EVENTS.get() {
            let _ = tx.send(HotkeyEvent { key, down });
        }
    }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for WH_KEYBOARD_LL with HC_ACTION, lparam points to a
        // KBDLLHOOKSTRUCT that is valid for the duration of the call.
        let vk = unsafe { (*(lparam as *const KBDLLHOOKSTRUCT)).vkCode };
        let message = wparam as u32;
        let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let up = message == WM_KEYUP || message == WM_SYSKEYUP;
        if vk != 0 && (down || up) {
            if vk == MAIN_VK.load(Ordering::Relaxed) {
                // Remember whether this press was ours, so its release is
                // treated the same way even if pause flips in between.
                let ours = if down {
                    !PAUSED.load(Ordering::Relaxed)
                } else {
                    MAIN_DOWN.load(Ordering::Relaxed)
                };
                if ours {
                    edge(&MAIN_DOWN, KeyId::Main, down);
                    return 1;
                }
            } else if vk == PAUSE_VK.load(Ordering::Relaxed) {
                edge(&PAUSE_DOWN, KeyId::Pause, down);
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Handle to the hook thread.
#[derive(Clone)]
pub struct Hotkeys;

impl Hotkeys {
    /// Install the hook (watching nothing until `set`).
    pub fn start(tx: UnboundedSender<HotkeyEvent>) -> anyhow::Result<Self> {
        if EVENTS.set(tx).is_err() {
            anyhow::bail!("hotkeys are already running");
        }
        let (ready_tx, ready_rx) = mpsc::sync_channel::<bool>(1);
        thread::spawn(move || unsafe {
            let hook = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(hook),
                GetModuleHandleW(std::ptr::null()),
                0,
            );
            let _ = ready_tx.send(!hook.is_null());
            if hook.is_null() {
                return;
            }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(true) => Ok(Self),
            _ => anyhow::bail!("Windows refused the keyboard hook"),
        }
    }

    /// Watch `main` (dictate) and `pause`. Errors leave the old keys in place.
    pub fn set(&self, main: &str, pause: &str) -> anyhow::Result<()> {
        let main_vk = resolve(main)?;
        let pause_vk = resolve(pause)?;
        MAIN_DOWN.store(false, Ordering::Relaxed);
        PAUSE_DOWN.store(false, Ordering::Relaxed);
        MAIN_VK.store(main_vk, Ordering::Relaxed);
        PAUSE_VK.store(
            if pause_vk == main_vk { 0 } else { pause_vk },
            Ordering::Relaxed,
        );
        Ok(())
    }

    /// While paused the dictate key goes to apps as usual.
    pub fn set_paused(&self, paused: bool) {
        PAUSED.store(paused, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_keys() {
        assert_eq!(vk(Key::F(1)), Some(0x70));
        assert_eq!(vk(Key::F(12)), Some(0x7B));
        assert_eq!(vk(Key::F(24)), Some(0x87));
        assert_eq!(vk(Key::F(25)), None);
        assert_eq!(vk(Key::RightCtrl), Some(0xA3));
        assert!(resolve("F30").is_err());
        assert!(resolve("nope").is_err());
    }

    #[test]
    fn every_choice_exists_on_windows() {
        for (name, _) in crate::hotkey::CHOICES {
            assert!(resolve(name).is_ok(), "{name}");
        }
    }
}
