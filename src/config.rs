//! Configuration for Lalia.
//!
//! Precedence (lowest → highest): built-in defaults → `~/.config/lalia/config.toml`
//! → environment variables. The API key may also live in `~/.config/lalia/key`.

use std::env;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    Hold,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertMode {
    /// Type each finalized segment into the focused window as it arrives.
    Streaming,
    /// Accumulate everything and type it once when the hotkey is released.
    OnRelease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Verbatim,
    Smart,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Verbatim => "VERBATIM",
            Mode::Smart => "SMART",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FileConfig {
    pub hotkey: Option<String>,
    pub pause_hotkey: Option<String>,
    pub activation: Option<Activation>,
    pub insert: Option<InsertMode>,
    pub mode: Option<String>,
    pub language: Option<String>,
    pub vocabulary: Option<Vec<String>>,
    pub model: Option<String>,
    pub type_delay_ms: Option<u32>,
    pub notify: Option<bool>,
    pub sound: Option<bool>,
    pub start_enabled: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: String,
    pub model: String,
    pub mode: Mode,
    pub language_codes: Vec<String>,
    pub custom_vocabulary: Vec<String>,
    pub hotkey: String,
    pub pause_hotkey: String,
    pub activation: Activation,
    pub insert: InsertMode,
    pub type_delay_ms: u32,
    pub notify: bool,
    pub sound: bool,
    pub start_enabled: bool,
}

pub fn config_dir() -> PathBuf {
    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("lalia");
        }
    }
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config/lalia")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn key_path() -> PathBuf {
    config_dir().join("key")
}

fn csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn env_bool(name: &str) -> Option<bool> {
    env::var(name).ok().map(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn read_key() -> Option<String> {
    if let Ok(k) = env::var("LALIA_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    if let Ok(k) = env::var("GEMINI_API_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    if let Ok(k) = env::var("GOOGLE_API_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    let contents = fs::read_to_string(key_path()).ok()?;
    contents
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Store an API key in `~/.config/lalia/key` with 0600 permissions.
pub fn set_api_key(key: &str) -> anyhow::Result<PathBuf> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let path = key_path();
    fs::write(&path, format!("{}\n", key.trim()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }
    Ok(path)
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let file: FileConfig = match fs::read_to_string(config_path()) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| anyhow::anyhow!("invalid {}: {e}", config_path().display()))?,
            Err(_) => FileConfig::default(),
        };

        let activation = env::var("LALIA_ACTIVATION")
            .ok()
            .and_then(|v| match v.as_str() {
                "toggle" | "tap" => Some(Activation::Toggle),
                "hold" => Some(Activation::Hold),
                _ => None,
            })
            .or(file.activation)
            .unwrap_or(Activation::Hold);

        let insert = env::var("LALIA_INSERT")
            .ok()
            .and_then(|v| match v.as_str() {
                "on_release" | "release" => Some(InsertMode::OnRelease),
                "streaming" => Some(InsertMode::Streaming),
                _ => None,
            })
            .or(file.insert)
            .unwrap_or(InsertMode::Streaming);

        let mode = match env::var("LALIA_MODE").ok().or(file.mode) {
            Some(m) if m.eq_ignore_ascii_case("smart") => Mode::Smart,
            _ => Mode::Verbatim,
        };

        let language_codes = env::var("LALIA_LANG")
            .ok()
            .map(|s| csv(&s))
            .or_else(|| file.language.as_deref().map(csv))
            .unwrap_or_default();

        let custom_vocabulary = env::var("LALIA_VOCAB")
            .ok()
            .map(|s| csv(&s))
            .or(file.vocabulary)
            .unwrap_or_default();

        Ok(Config {
            api_key: read_key().unwrap_or_default(),
            model: env::var("LALIA_MODEL")
                .ok()
                .or(file.model)
                .unwrap_or_else(|| "gemini-3.5-transcribe-live".into()),
            mode,
            language_codes,
            custom_vocabulary,
            hotkey: env::var("LALIA_HOTKEY")
                .ok()
                .or(file.hotkey)
                .unwrap_or_else(|| "F12".into()),
            pause_hotkey: env::var("LALIA_PAUSE_HOTKEY")
                .ok()
                .or(file.pause_hotkey)
                .unwrap_or_else(|| "F11".into()),
            activation,
            insert,
            type_delay_ms: env::var("LALIA_TYPE_DELAY_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .or(file.type_delay_ms)
                .unwrap_or(2),
            notify: env_bool("LALIA_NOTIFY").or(file.notify).unwrap_or(true),
            sound: env_bool("LALIA_SOUND").or(file.sound).unwrap_or(true),
            start_enabled: env_bool("LALIA_START_ENABLED")
                .or(file.start_enabled)
                .unwrap_or(true),
        })
    }

    pub fn require_key(&self) -> anyhow::Result<()> {
        if self.api_key.trim().is_empty() {
            anyhow::bail!(
                "no API key. Run `lalia set-key`, or set GEMINI_API_KEY, or write it to {}",
                key_path().display()
            );
        }
        Ok(())
    }

    pub fn summary(&self) -> String {
        format!(
            "  model        : {}\n  mode         : {}\n  hotkey       : {} ({})\n  pause hotkey : {}\n  insert       : {:?}\n  language     : {}\n  vocabulary   : {}\n  notifications: {}\n  sound        : {}\n  start active : {}",
            self.model,
            self.mode.as_str(),
            self.hotkey,
            match self.activation {
                Activation::Hold => "hold to talk",
                Activation::Toggle => "tap to toggle",
            },
            self.pause_hotkey,
            self.insert,
            if self.language_codes.is_empty() {
                "auto-detect".to_string()
            } else {
                self.language_codes.join(", ")
            },
            if self.custom_vocabulary.is_empty() {
                "(none)".to_string()
            } else {
                self.custom_vocabulary.join(", ")
            },
            self.notify,
            self.sound,
            self.start_enabled,
        )
    }
}

/// Write a documented default config if none exists.
pub fn write_default_config(force: bool) -> anyhow::Result<PathBuf> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let path = config_path();
    if path.exists() && !force {
        return Ok(path);
    }
    let text = r#"# Lalia configuration — https://github.com/  (Greek λαλιά = "speech")
# Precedence: these values < environment variables at runtime.

# Hold F12 to talk (push-to-talk). Release to finish.
hotkey = "F12"

# Tap F11 to pause/resume Lalia entirely (dictation off).
pause_hotkey = "F11"

# "hold" = push-to-talk, "toggle" = tap the hotkey to start/stop.
activation = "hold"

# "streaming" = type words as you speak (feels instant)
# "on_release" = type the whole thing when you let go
insert = "streaming"

# "VERBATIM" = raw words.  "SMART" = punctuation, casing, filler removal.
mode = "VERBATIM"

# BCP-47 hint, or leave empty for automatic language detection.
language = ""

# Bias recognition toward names / jargon (up to ~100 terms works best).
vocabulary = []

# Model id (the same one Antigravity's record button uses).
model = "gemini-3.5-transcribe-live"

# Per-character typing delay in ms. Raise to 4-6 if an app drops characters.
type_delay_ms = 2

# Desktop notifications and a soft sound on state changes.
notify = true
sound = true

# Start listening (not paused) when Lalia launches.
start_enabled = true
"#;
    fs::write(&path, text)?;
    Ok(path)
}
