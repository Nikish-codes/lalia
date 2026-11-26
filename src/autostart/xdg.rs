//! "Start on login" on Linux via an XDG autostart entry
//! (`~/.config/autostart/lalia.desktop`).
//!
//! The desktop session launches it with the right DISPLAY and D-Bus session
//! (the tray icon needs both), which a systemd user unit doesn't reliably get.
//! Older versions used a systemd unit; enabling or disabling removes it.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn config_home() -> PathBuf {
    match env::var("XDG_CONFIG_HOME") {
        Ok(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
        _ => crate::config::home().join(".config"),
    }
}

fn entry_path() -> PathBuf {
    config_home().join("autostart/lalia.desktop")
}

fn legacy_unit_path() -> PathBuf {
    config_home().join("systemd/user/lalia.service")
}

fn remove_legacy_unit() {
    if legacy_unit_path().exists() {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", "lalia.service"])
            .output();
        let _ = fs::remove_file(legacy_unit_path());
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .output();
    }
}

pub fn is_enabled() -> bool {
    fs::read_to_string(entry_path())
        .map(|s| !s.contains("Hidden=true") && !s.contains("X-GNOME-Autostart-enabled=false"))
        .unwrap_or(false)
}

fn entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Lalia\n\
         GenericName=Voice Dictation\n\
         Comment=Live dictation — hold your hotkey and speak\n\
         Exec={exe}\n\
         Icon=lalia\n\
         Terminal=false\n\
         Categories=Utility;Accessibility;\n\
         X-GNOME-Autostart-enabled=true\n\
         X-GNOME-Autostart-Delay=2\n"
    )
}

/// Write the autostart entry (Lalia will start at the next login).
pub fn set_enabled(on: bool) -> anyhow::Result<()> {
    remove_legacy_unit();
    if on {
        let exe = env::current_exe()?;
        // Prefer the stable command name when it resolves to this binary,
        // so package upgrades keep working.
        let exe = match which("lalia") {
            Some(p) if fs::canonicalize(&p).ok() == fs::canonicalize(&exe).ok() => p,
            _ => exe,
        };
        fs::create_dir_all(entry_path().parent().unwrap_or(&config_home()))?;
        fs::write(entry_path(), entry(&exe.display().to_string()))?;
    } else if entry_path().exists() {
        fs::remove_file(entry_path())?;
    }
    Ok(())
}

fn which(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// Where the setting lives (shown by `lalia autostart status`).
pub fn location() -> String {
    entry_path().display().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn entry_is_a_valid_desktop_file() {
        let e = super::entry("/usr/bin/lalia");
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("\nExec=/usr/bin/lalia\n"));
        assert!(e
            .lines()
            .all(|l| l.is_empty() || l.starts_with('[') || l.contains('=')));
    }
}
