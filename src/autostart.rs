//! "Start on login" support via a systemd --user service.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn systemd_user_dir() -> PathBuf {
    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("systemd/user");
        }
    }
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config/systemd/user")
}

fn unit_path() -> PathBuf {
    systemd_user_dir().join("lalia.service")
}

fn run(cmd: &str, args: &[&str]) -> String {
    match Command::new(cmd).args(args).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s.trim().to_string()
        }
        Err(e) => format!("({cmd} failed: {e})"),
    }
}

fn display_value() -> String {
    env::var("DISPLAY").unwrap_or_else(|_| ":0".into())
}

pub fn enable() -> anyhow::Result<()> {
    let exe = env::current_exe()?;
    let dir = systemd_user_dir();
    fs::create_dir_all(&dir)?;
    let unit = format!(
        "[Unit]\n\
         Description=Lalia — system-wide live dictation\n\
         After=graphical-session.target\n\
         PartOf=graphical-session.target\n\n\
         [Service]\n\
         Type=simple\n\
         Environment=DISPLAY={display}\n\
         ExecStart={exe}\n\
         Restart=on-failure\n\
         RestartSec=2\n\n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        display = display_value(),
        exe = exe.display()
    );
    fs::write(unit_path(), unit)?;
    run("systemctl", &["--user", "daemon-reload"]);
    let out = run("systemctl", &["--user", "enable", "--now", "lalia.service"]);
    println!("Lalia autostart: enabled");
    println!("  unit: {}", unit_path().display());
    if !out.is_empty() {
        println!("  {out}");
    }
    Ok(())
}

pub fn disable() -> anyhow::Result<()> {
    let out = run(
        "systemctl",
        &["--user", "disable", "--now", "lalia.service"],
    );
    if unit_path().exists() {
        fs::remove_file(unit_path()).ok();
    }
    run("systemctl", &["--user", "daemon-reload"]);
    println!("Lalia autostart: disabled");
    if !out.is_empty() {
        println!("  {out}");
    }
    Ok(())
}

pub fn status() {
    let enabled = run("systemctl", &["--user", "is-enabled", "lalia.service"]);
    let active = run("systemctl", &["--user", "is-active", "lalia.service"]);
    println!("Lalia autostart");
    println!(
        "  unit file : {} ({})",
        unit_path().display(),
        if unit_path().exists() {
            "present"
        } else {
            "absent"
        }
    );
    println!("  enabled   : {enabled}");
    println!("  running   : {active}");
}
