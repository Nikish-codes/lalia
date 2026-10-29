//! Type text into whatever window currently has focus, using X11 XTEST via
//! `xdotool` (installed by default on most X11 desktops).

use std::process::Command;

/// Type `text` at the current cursor position in the focused window.
pub fn type_text(text: &str, delay_ms: u32) -> anyhow::Result<()> {
    if text.trim().is_empty() {
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
        .map_err(|e| anyhow::anyhow!("could not run xdotool: {e} (is it installed?)"))?;

    if !status.success() {
        anyhow::bail!("xdotool exited with status {status}");
    }
    Ok(())
}
