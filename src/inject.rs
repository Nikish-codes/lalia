//! Type text into whatever window currently has focus, using X11 XTEST via
//! `xdotool` (installed by default on most X11 desktops).
//!
//! All typing goes through one worker thread, so segments are typed strictly
//! in the order they were transcribed, and `Joiner` decides the spacing
//! between consecutive segments.

use std::process::Command;
use std::sync::mpsc;
use std::thread;

/// Type `text` at the current cursor position in the focused window.
pub fn type_text(text: &str, delay_ms: u32) -> anyhow::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let status = Command::new("xdotool")
        .args([
            "type",
            "--clearmodifiers",
            "--delay",
            &delay_ms.to_string(),
            "--",
            text,
        ])
        .status()
        .map_err(|e| {
            anyhow::anyhow!("could not run xdotool: {e} (install it: sudo apt install xdotool)")
        })?;

    if !status.success() {
        anyhow::bail!("xdotool exited with status {status}");
    }
    Ok(())
}

/// A single typing queue. Cloning shares the same worker.
#[derive(Clone)]
pub struct Injector {
    tx: mpsc::Sender<(String, u32)>,
}

impl Injector {
    /// Start the worker; failures are reported through `on_error`.
    pub fn start(on_error: impl Fn(String) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<(String, u32)>();
        thread::spawn(move || {
            for (text, delay) in rx {
                if let Err(e) = type_text(&text, delay) {
                    on_error(e.to_string());
                }
            }
        });
        Self { tx }
    }

    /// Queue `text` to be typed after everything queued before it.
    pub fn type_text(&self, text: String, delay_ms: u32) {
        if !text.is_empty() {
            let _ = self.tx.send((text, delay_ms));
        }
    }
}

/// Joins transcript segments with natural spacing.
///
/// Some servers send self-contained segments ("Hello there." / "How are
/// you?"), others send fragments that carry their own leading space ("Hello",
/// " there", "."). We add a space between segments unless punctuation or the
/// server's own spacing says otherwise.
#[derive(Debug, Default)]
pub struct Joiner {
    last: Option<char>,
    /// The server has shown it sends its own leading spaces.
    explicit_spacing: bool,
}

const NO_SPACE_BEFORE: &[char] = &[
    '.', ',', '!', '?', ';', ':', '%', ')', ']', '}', '…', '’', '”', '»',
];
const NO_SPACE_AFTER: &[char] = &['(', '[', '{', '“', '‘', '«', '/', '-'];

impl Joiner {
    /// Returns the text to emit for the next raw segment ("" to emit nothing).
    pub fn push(&mut self, raw: &str) -> String {
        let leading_space = raw.starts_with(char::is_whitespace);
        let body = raw.trim();
        if body.is_empty() {
            return String::new();
        }
        let first = body.chars().next().unwrap_or(' ');
        let out = match self.last {
            None => body.to_string(),
            Some(prev) => {
                if leading_space {
                    self.explicit_spacing = true;
                }
                let glue = prev.is_whitespace()
                    || NO_SPACE_AFTER.contains(&prev)
                    || NO_SPACE_BEFORE.contains(&first)
                    || (self.explicit_spacing && !leading_space);
                if glue {
                    body.to_string()
                } else {
                    format!(" {body}")
                }
            }
        };
        self.last = out.chars().last();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join(parts: &[&str]) -> String {
        let mut j = Joiner::default();
        parts.iter().map(|p| j.push(p)).collect()
    }

    #[test]
    fn segments_get_a_space_between_them() {
        assert_eq!(
            join(&["hello world", "how are you"]),
            "hello world how are you"
        );
        assert_eq!(
            join(&["Hello there.", "How are you?"]),
            "Hello there. How are you?"
        );
    }

    #[test]
    fn first_segment_has_no_leading_space() {
        assert_eq!(join(&["  hi"]), "hi");
    }

    #[test]
    fn punctuation_attaches_to_the_previous_word() {
        assert_eq!(join(&["Hello", ",", "world", "!"]), "Hello, world!");
        assert_eq!(join(&["see", "(below)", "now"]), "see (below) now");
        assert_eq!(join(&["a", "(", "b"]), "a (b");
    }

    #[test]
    fn server_spacing_is_respected() {
        // Fragment style: leading spaces mark word starts, so a fragment
        // without one continues the previous word.
        assert_eq!(join(&["Hel", " lo", " wor", "ld"]), "Hel lo world");
        assert_eq!(join(&["Hello", " there", "."]), "Hello there.");
    }

    #[test]
    fn empty_and_whitespace_segments_are_skipped() {
        assert_eq!(join(&["one", "", "   ", "two"]), "one two");
    }

    #[test]
    fn newlines_inside_segments_survive() {
        assert_eq!(join(&["List:\n- a", "- b"]), "List:\n- a - b");
    }
}
