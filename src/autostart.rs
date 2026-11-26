//! "Start on login".
//!
//! * Linux: an XDG autostart entry (`autostart/xdg.rs`).
//! * Windows: a value under the user's `Run` registry key
//!   (`autostart/windows.rs`).

#[cfg(windows)]
#[path = "autostart/windows.rs"]
mod sys;
#[cfg(unix)]
#[path = "autostart/xdg.rs"]
mod sys;

pub use sys::{is_enabled, set_enabled};

pub fn enable() -> anyhow::Result<()> {
    set_enabled(true)?;
    println!("Lalia autostart: enabled");
    println!("  entry: {}", sys::location());
    if crate::ipc::daemon("status").is_none() {
        println!("  (it starts at your next login; run `lalia` to start it now)");
    }
    Ok(())
}

pub fn disable() -> anyhow::Result<()> {
    set_enabled(false)?;
    println!("Lalia autostart: disabled");
    Ok(())
}

pub fn status() {
    println!("Lalia autostart");
    println!(
        "  entry   : {} ({})",
        sys::location(),
        if is_enabled() { "enabled" } else { "absent" }
    );
    println!(
        "  running : {}",
        if crate::ipc::daemon("status").is_some() {
            "yes"
        } else {
            "no"
        }
    );
}
