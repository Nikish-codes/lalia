//! Local history of dictations, one JSON object per line in
//! `~/.local/share/lalia/history.jsonl`. Never leaves the machine.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config;

const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Unix seconds when the dictation finished.
    pub ts: u64,
    pub text: String,
    pub words: usize,
    /// How long the hotkey was held / the session ran.
    pub ms: u64,
}

impl Entry {
    pub fn new(text: &str, ms: u64) -> Self {
        Self {
            ts: now(),
            text: text.to_string(),
            words: text.split_whitespace().count(),
            ms,
        }
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn path() -> PathBuf {
    config::data_dir().join("history.jsonl")
}

pub fn append(entry: &Entry) -> anyhow::Result<()> {
    fs::create_dir_all(config::data_dir())?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path())?;
    writeln!(f, "{}", serde_json::to_string(entry)?)?;
    Ok(())
}

/// All entries, newest first.
pub fn load() -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(path()) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    entries.reverse();
    entries
}

/// Keep only the newest `MAX_ENTRIES`.
pub fn trim() -> anyhow::Result<()> {
    let entries = load();
    if entries.len() <= MAX_ENTRIES {
        return Ok(());
    }
    write_all(&entries[..MAX_ENTRIES])
}

/// Replace the file with `newest_first`.
pub fn write_all(newest_first: &[Entry]) -> anyhow::Result<()> {
    fs::create_dir_all(config::data_dir())?;
    let mut body = String::new();
    for e in newest_first.iter().rev() {
        body.push_str(&serde_json::to_string(e)?);
        body.push('\n');
    }
    let tmp = path().with_extension("jsonl.tmp");
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path())?;
    Ok(())
}

pub fn clear() -> anyhow::Result<()> {
    match fs::remove_file(path()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// "just now", "5 min ago", "yesterday", "3 days ago"…
pub fn ago(ts: u64, now: u64) -> String {
    let d = now.saturating_sub(ts);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => {
            let h = d / 3600;
            format!("{h} hour{} ago", if h == 1 { "" } else { "s" })
        }
        86_400..=172_799 => "yesterday".into(),
        _ if d < 14 * 86_400 => format!("{} days ago", d / 86_400),
        _ if d < 60 * 86_400 => format!("{} weeks ago", d / (7 * 86_400)),
        _ => format!("{} months ago", d / (30 * 86_400)),
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Stats {
    pub words_today: usize,
    pub words_week: usize,
    pub sessions: usize,
    pub words_total: usize,
}

pub fn stats(entries: &[Entry], now: u64) -> Stats {
    let mut s = Stats {
        sessions: entries.len(),
        ..Stats::default()
    };
    for e in entries {
        let age = now.saturating_sub(e.ts);
        s.words_total += e.words;
        if age < 86_400 {
            s.words_today += e.words;
        }
        if age < 7 * 86_400 {
            s.words_week += e.words;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        assert_eq!(ago(100, 100), "just now");
        assert_eq!(ago(0, 300), "5 min ago");
        assert_eq!(ago(0, 3600), "1 hour ago");
        assert_eq!(ago(0, 7300), "2 hours ago");
        assert_eq!(ago(0, 90_000), "yesterday");
        assert_eq!(ago(0, 3 * 86_400), "3 days ago");
        assert_eq!(ago(0, 21 * 86_400), "3 weeks ago");
        assert_eq!(ago(200, 100), "just now");
    }

    #[test]
    fn counts_words_and_stats() {
        let mut e = Entry::new("one two  three", 1200);
        assert_eq!(e.words, 3);
        e.ts = 1_000_000;
        let mut old = Entry::new("four five", 800);
        old.ts = 1_000_000 - 3 * 86_400;
        let s = stats(&[e, old], 1_000_100);
        assert_eq!(
            s,
            Stats {
                words_today: 3,
                words_week: 5,
                sessions: 2,
                words_total: 5
            }
        );
    }
}
