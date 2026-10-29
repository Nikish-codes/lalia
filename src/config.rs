//! Configuration for Lalia.
//!
//! Precedence (lowest → highest): built-in defaults → `~/.config/lalia/config.toml`
//! → environment variables. The API key may also live in `~/.config/lalia/key`.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayPosition {
    Bottom,
    Top,
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

    fn parse(s: &str) -> Self {
        if s.trim().eq_ignore_ascii_case("smart") {
            Mode::Smart
        } else {
            Mode::Verbatim
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause_hotkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation: Option<Activation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insert: Option<InsertMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_delay_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_position: Option<OverlayPosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
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
    /// Input device name; empty = system default.
    pub input_device: String,
    pub type_delay_ms: u32,
    /// Floating on-screen indicator while dictating.
    pub overlay: bool,
    pub overlay_position: OverlayPosition,
    pub notify: bool,
    pub sound: bool,
    /// Keep a local history of dictations.
    pub history: bool,
    pub start_enabled: bool,
}

pub const DEFAULT_MODEL: &str = "gemini-3.5-transcribe-live";

pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

pub fn data_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    if let Ok(xdg) = env::var(var) {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("lalia");
        }
    }
    home().join(fallback).join("lalia")
}

pub fn home() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".into()))
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn key_path() -> PathBuf {
    config_dir().join("key")
}

fn csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn parse_bool(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Environment lookup; tests pass a fake one.
pub type EnvFn<'a> = &'a dyn Fn(&str) -> Option<String>;

pub fn process_env(name: &str) -> Option<String> {
    env::var(name).ok()
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn read_key(env: EnvFn) -> Option<String> {
    for var in ["LALIA_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"] {
        if let Some(k) = env(var) {
            if !k.trim().is_empty() {
                return Some(k.trim().to_string());
            }
        }
    }
    read_key_file()
}

/// The key stored by `lalia set-key` (ignores the environment).
pub fn read_key_file() -> Option<String> {
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
    // Create with 0600 from the start so the key is never world-readable.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        writeln!(f, "{}", key.trim())?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    fs::write(&path, format!("{}\n", key.trim()))?;
    Ok(path)
}

/// Read `config.toml` (missing file = all defaults).
pub fn load_file() -> anyhow::Result<FileConfig> {
    match fs::read_to_string(config_path()) {
        Ok(text) => toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("invalid {}: {e}", config_path().display())),
        Err(_) => Ok(FileConfig::default()),
    }
}

/// Write `config.toml` atomically (used by the settings window and tray).
pub fn save_file(file: &FileConfig) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(config_dir())?;
    let path = config_path();
    let body = toml::to_string_pretty(file)?;
    let text = format!(
        "# Lalia configuration — https://github.com/Nikish-codes/lalia\n\
         # Edited by the Lalia settings window; `lalia generate-config` writes a\n\
         # commented version. Environment variables override these values.\n\n{body}"
    );
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Load, change and save `config.toml` in one step.
pub fn update_file(change: impl FnOnce(&mut FileConfig)) -> anyhow::Result<()> {
    let mut file = load_file()?;
    change(&mut file);
    save_file(&file)?;
    Ok(())
}

/// Modification times of the config and key files, to notice edits.
pub fn stamp() -> (Option<SystemTime>, Option<SystemTime>) {
    let m = |p: PathBuf| fs::metadata(p).and_then(|m| m.modified()).ok();
    (m(config_path()), m(key_path()))
}

impl Config {
    /// Defaults ← config.toml ← environment.
    pub fn load() -> anyhow::Result<Self> {
        Ok(Self::resolve(load_file()?, &process_env))
    }

    /// Defaults ← config.toml, ignoring the environment (what the settings
    /// window edits).
    pub fn from_file(file: FileConfig) -> Self {
        Self::resolve(file, &no_env)
    }

    pub fn resolve(file: FileConfig, env: EnvFn) -> Self {
        let activation = env("LALIA_ACTIVATION")
            .and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
                "toggle" | "tap" => Some(Activation::Toggle),
                "hold" => Some(Activation::Hold),
                _ => None,
            })
            .or(file.activation)
            .unwrap_or(Activation::Hold);

        let insert = env("LALIA_INSERT")
            .and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
                "on_release" | "release" => Some(InsertMode::OnRelease),
                "streaming" => Some(InsertMode::Streaming),
                _ => None,
            })
            .or(file.insert)
            .unwrap_or(InsertMode::Streaming);

        let overlay_position = env("LALIA_OVERLAY_POSITION")
            .and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
                "top" => Some(OverlayPosition::Top),
                "bottom" => Some(OverlayPosition::Bottom),
                _ => None,
            })
            .or(file.overlay_position)
            .unwrap_or(OverlayPosition::Bottom);

        let flag = |var: &str, file_value: Option<bool>, default: bool| {
            env(var)
                .map(|v| parse_bool(&v))
                .or(file_value)
                .unwrap_or(default)
        };
        let text = |var: &str, file_value: Option<String>, default: &str| {
            env(var)
                .or(file_value)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| default.to_string())
        };

        Config {
            api_key: read_key(env).unwrap_or_default(),
            model: text("LALIA_MODEL", file.model, DEFAULT_MODEL),
            mode: Mode::parse(&env("LALIA_MODE").or(file.mode).unwrap_or_default()),
            language_codes: env("LALIA_LANG")
                .map(|s| csv(&s))
                .or_else(|| file.language.as_deref().map(csv))
                .unwrap_or_default(),
            custom_vocabulary: env("LALIA_VOCAB")
                .map(|s| csv(&s))
                .or(file.vocabulary)
                .unwrap_or_default()
                .into_iter()
                .map(|w| w.trim().to_string())
                .filter(|w| !w.is_empty())
                .collect(),
            hotkey: text("LALIA_HOTKEY", file.hotkey, "F12"),
            pause_hotkey: text("LALIA_PAUSE_HOTKEY", file.pause_hotkey, "F11"),
            activation,
            insert,
            input_device: env("LALIA_INPUT_DEVICE")
                .or(file.input_device)
                .unwrap_or_default()
                .trim()
                .to_string(),
            type_delay_ms: env("LALIA_TYPE_DELAY_MS")
                .and_then(|v| v.trim().parse().ok())
                .or(file.type_delay_ms)
                .unwrap_or(2),
            overlay: flag("LALIA_OVERLAY", file.overlay, true),
            overlay_position,
            notify: flag("LALIA_NOTIFY", file.notify, true),
            sound: flag("LALIA_SOUND", file.sound, true),
            history: flag("LALIA_HISTORY", file.history, true),
            start_enabled: flag("LALIA_START_ENABLED", file.start_enabled, true),
        }
    }

    /// Every setting written out explicitly (the API key is stored separately).
    pub fn to_file(&self) -> FileConfig {
        FileConfig {
            hotkey: Some(self.hotkey.clone()),
            pause_hotkey: Some(self.pause_hotkey.clone()),
            activation: Some(self.activation),
            insert: Some(self.insert),
            mode: Some(self.mode.as_str().to_string()),
            language: Some(self.language_codes.join(", ")),
            vocabulary: Some(self.custom_vocabulary.clone()),
            model: Some(self.model.clone()),
            input_device: Some(self.input_device.clone()),
            type_delay_ms: Some(self.type_delay_ms),
            overlay: Some(self.overlay),
            overlay_position: Some(self.overlay_position),
            notify: Some(self.notify),
            sound: Some(self.sound),
            history: Some(self.history),
            start_enabled: Some(self.start_enabled),
        }
    }

    pub fn has_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    pub fn require_key(&self) -> anyhow::Result<()> {
        if !self.has_key() {
            anyhow::bail!(
                "no API key. Run `lalia set-key`, or set GEMINI_API_KEY, or write it to {}",
                key_path().display()
            );
        }
        Ok(())
    }

    pub fn summary(&self) -> String {
        format!(
            "  model        : {}\n  mode         : {}\n  hotkey       : {} ({})\n  pause hotkey : {}\n  insert       : {:?}\n  language     : {}\n  vocabulary   : {}\n  microphone   : {}\n  overlay      : {}\n  notifications: {}\n  sound        : {}\n  history      : {}\n  start active : {}",
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
            if self.input_device.is_empty() {
                "system default"
            } else {
                &self.input_device
            },
            if self.overlay {
                format!("{:?}", self.overlay_position).to_lowercase()
            } else {
                "off".to_string()
            },
            self.notify,
            self.sound,
            self.history,
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
    fs::write(&path, DEFAULT_CONFIG)?;
    Ok(path)
}

const DEFAULT_CONFIG: &str = r#"# Lalia configuration — https://github.com/Nikish-codes/lalia  (Greek λαλιά = "speech")
# Precedence: these values < environment variables at runtime.
# Every setting here can also be changed in the settings window (`lalia gui`).

# Hold F12 to talk (push-to-talk). Release to finish.
# F1–F35, or: Pause, ScrollLock, Insert, Menu, RightCtrl, RightAlt, RightSuper.
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

# Microphone name as listed in the settings window; empty = system default.
input_device = ""

# Per-character typing delay in ms. Raise to 4-6 if an app drops characters.
type_delay_ms = 2

# Floating indicator with a level meter and live text while you dictate.
overlay = true
overlay_position = "bottom"   # "bottom" | "top"

# Desktop notifications (errors, pause when the overlay is off) and sounds.
notify = true
sound = true

# Keep a local history of what you dictated (~/.local/share/lalia).
history = true

# Start listening (not paused) when Lalia launches.
start_enabled = true
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults() {
        let c = Config::resolve(FileConfig::default(), &env_of(&[("LALIA_KEY", "k")]));
        assert_eq!(c.hotkey, "F12");
        assert_eq!(c.pause_hotkey, "F11");
        assert_eq!(c.activation, Activation::Hold);
        assert_eq!(c.insert, InsertMode::Streaming);
        assert_eq!(c.mode, Mode::Verbatim);
        assert_eq!(c.model, DEFAULT_MODEL);
        assert!(c.overlay && c.notify && c.sound && c.history && c.start_enabled);
        assert_eq!(c.api_key, "k");
    }

    #[test]
    fn default_config_template_parses() {
        let file: FileConfig = toml::from_str(DEFAULT_CONFIG).unwrap();
        let c = Config::resolve(file, &env_of(&[]));
        assert_eq!(c.overlay_position, OverlayPosition::Bottom);
        assert!(c.language_codes.is_empty());
        assert_eq!(c.type_delay_ms, 2);
    }

    #[test]
    fn file_then_env_precedence() {
        let file: FileConfig = toml::from_str(
            r#"
            hotkey = "F9"
            activation = "toggle"
            insert = "on_release"
            mode = "smart"
            language = "en-US, hi-IN"
            vocabulary = ["Kubernetes", " ", "gRPC"]
            notify = false
            "#,
        )
        .unwrap();
        let c = Config::resolve(file.clone(), &env_of(&[]));
        assert_eq!(c.hotkey, "F9");
        assert_eq!(c.activation, Activation::Toggle);
        assert_eq!(c.insert, InsertMode::OnRelease);
        assert_eq!(c.mode, Mode::Smart);
        assert_eq!(c.language_codes, vec!["en-US", "hi-IN"]);
        assert_eq!(c.custom_vocabulary, vec!["Kubernetes", "gRPC"]);
        assert!(!c.notify);

        let c = Config::resolve(
            file,
            &env_of(&[
                ("LALIA_HOTKEY", "F8"),
                ("LALIA_ACTIVATION", "HOLD"),
                ("LALIA_NOTIFY", "yes"),
                ("LALIA_VOCAB", "a, b"),
            ]),
        );
        assert_eq!(c.hotkey, "F8");
        assert_eq!(c.activation, Activation::Hold);
        assert!(c.notify);
        assert_eq!(c.custom_vocabulary, vec!["a", "b"]);
    }

    #[test]
    fn round_trips_through_toml() {
        let c = Config::resolve(
            toml::from_str("mode = \"SMART\"\noverlay_position = \"top\"").unwrap(),
            &env_of(&[]),
        );
        let text = toml::to_string_pretty(&c.to_file()).unwrap();
        let back = Config::resolve(toml::from_str(&text).unwrap(), &env_of(&[]));
        assert_eq!(c, back);
    }

    #[test]
    fn helpers() {
        assert_eq!(csv(" a, ,b ,"), vec!["a", "b"]);
        assert!(parse_bool(" On "));
        assert!(!parse_bool("nope"));
        assert_eq!(Mode::parse("Smart"), Mode::Smart);
        assert_eq!(Mode::parse("anything"), Mode::Verbatim);
    }
}
