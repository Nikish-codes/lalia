//! Desktop notifications and soft audio feedback (both optional, non-blocking).

use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy)]
pub enum Sound {
    Start,
    Stop,
    Paused,
    Resumed,
    Error,
}

fn sound_id(s: Sound) -> &'static str {
    match s {
        Sound::Start => "message",
        Sound::Stop => "complete",
        Sound::Paused => "bell",
        Sound::Resumed => "bell",
        Sound::Error => "dialog-error",
    }
}

pub fn notify(summary: &str, body: &str, enabled: bool) {
    if !enabled {
        return;
    }
    let _ = Command::new("notify-send")
        .args([
            "--app-name=Lalia",
            "--expire-time=1600",
            "--icon=audio-input-microphone",
            summary,
            body,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub fn sound(event: Sound, enabled: bool) {
    if !enabled {
        return;
    }
    // canberra plays the desktop's themed event sounds.
    let _ = Command::new("canberra-gtk-play")
        .args(["-i", sound_id(event)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
