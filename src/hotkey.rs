//! Global hotkeys: the dictate key and the pause key.
//!
//! This file holds what both platforms share — which keys exist and what
//! they are called. Watching them is platform work:
//!
//! * Linux (X11): poll the keyboard state, never grabbing keys (`x11.rs`).
//! * Windows: a low-level keyboard hook that also keeps the hotkeys away
//!   from the focused app, so F12 does not open DevTools (`windows.rs`).

#[cfg(windows)]
mod windows;
#[cfg(unix)]
mod x11;

#[cfg(windows)]
pub use windows::Hotkeys;
#[cfg(unix)]
pub use x11::Hotkeys;

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

/// A key Lalia can watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// F1–F35 (Windows keyboards stop at F24).
    F(u8),
    Pause,
    ScrollLock,
    Insert,
    Menu,
    RightCtrl,
    RightAlt,
    RightSuper,
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
    ("RightSuper", RIGHT_SUPER),
];

#[cfg(windows)]
const RIGHT_SUPER: &str = "Right Win";
#[cfg(not(windows))]
const RIGHT_SUPER: &str = "Right Super";

/// Human label for a configured key name.
pub fn label(name: &str) -> String {
    let key = parse(name);
    CHOICES
        .iter()
        .find(|(n, _)| key.is_some() && parse(n) == key)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Parse a key name from the config ("F12", "right_ctrl", "Scroll Lock"…).
pub fn parse(name: &str) -> Option<Key> {
    let norm = name
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-', ' '], "");
    if let Some(n) = norm.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=35).contains(&n).then_some(Key::F(n));
    }
    Some(match norm.as_str() {
        "pause" | "break" => Key::Pause,
        "scrolllock" => Key::ScrollLock,
        "insert" => Key::Insert,
        "menu" | "apps" => Key::Menu,
        "rightctrl" | "controlr" | "ctrlr" => Key::RightCtrl,
        "rightalt" | "altr" | "altgr" => Key::RightAlt,
        "rightsuper" | "superr" | "rightwin" | "winr" => Key::RightSuper,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_keys() {
        assert_eq!(parse("F1"), Some(Key::F(1)));
        assert_eq!(parse("f12"), Some(Key::F(12)));
        assert_eq!(parse("F35"), Some(Key::F(35)));
        assert_eq!(parse("F36"), None);
        assert_eq!(parse("F0"), None);
    }

    #[test]
    fn named_keys() {
        assert_eq!(parse("RightCtrl"), Some(Key::RightCtrl));
        assert_eq!(parse("right_ctrl"), Some(Key::RightCtrl));
        assert_eq!(parse("Scroll Lock"), Some(Key::ScrollLock));
        assert_eq!(parse("AltGr"), Some(Key::RightAlt));
        assert_eq!(parse("Space"), None);
    }

    #[test]
    fn every_choice_resolves() {
        for (name, _) in CHOICES {
            assert!(parse(name).is_some(), "{name}");
        }
        assert_eq!(label("rightctrl"), "Right Ctrl");
        assert_eq!(label("F12"), "F12");
        assert_eq!(label("nonsense"), "nonsense");
    }
}
