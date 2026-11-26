//! Desktop notifications and soft audio feedback (both optional, non-blocking).
//!
//! * Linux: `notify-send` and the desktop's themed sounds (`canberra-gtk-play`).
//! * Windows: balloons from the tray icon (shown as toasts) and short chimes
//!   synthesized here, because the stock Windows sounds are loud dings.

#[derive(Debug, Clone, Copy)]
pub enum Sound {
    Start,
    Stop,
    Paused,
    Resumed,
    Error,
}

#[cfg(unix)]
mod sys {
    use super::Sound;
    use std::process::{Command, Stdio};

    fn sound_id(s: Sound) -> &'static str {
        match s {
            Sound::Start => "message",
            Sound::Stop => "complete",
            Sound::Paused => "bell",
            Sound::Resumed => "bell",
            Sound::Error => "dialog-error",
        }
    }

    pub fn notify(summary: &str, body: &str) {
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

    pub fn sound(event: Sound) {
        // canberra plays the desktop's themed event sounds.
        let _ = Command::new("canberra-gtk-play")
            .args(["-i", sound_id(event)])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

#[cfg(windows)]
mod sys {
    use super::{chime, Sound};
    use std::sync::OnceLock;
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    pub fn notify(summary: &str, body: &str) {
        crate::tray::balloon(summary, body);
    }

    /// The five chimes as WAV files in memory. They live for the whole
    /// process, as `SND_ASYNC | SND_MEMORY` requires.
    fn wav(event: Sound) -> &'static [u8] {
        static WAVS: OnceLock<[Vec<u8>; 5]> = OnceLock::new();
        let wavs = WAVS.get_or_init(|| {
            [
                chime(&[(660.0, 70), (880.0, 90)]),   // start: rising
                chime(&[(880.0, 70), (660.0, 90)]),   // stop: falling
                chime(&[(523.0, 110)]),               // paused
                chime(&[(784.0, 110)]),               // resumed
                chime(&[(330.0, 110), (262.0, 160)]), // error: low, falling
            ]
        });
        &wavs[match event {
            Sound::Start => 0,
            Sound::Stop => 1,
            Sound::Paused => 2,
            Sound::Resumed => 3,
            Sound::Error => 4,
        }]
    }

    pub fn sound(event: Sound) {
        unsafe {
            PlaySoundW(
                wav(event).as_ptr().cast(),
                std::ptr::null_mut(),
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            );
        }
    }
}

pub fn notify(summary: &str, body: &str, enabled: bool) {
    if enabled {
        sys::notify(summary, body);
    }
}

pub fn sound(event: Sound, enabled: bool) {
    if enabled {
        sys::sound(event);
    }
}

/// A short, quiet chime as a 16-bit mono WAV: the notes `(Hz, ms)` played
/// one after another, each with a soft attack and a fading tail.
#[cfg(any(windows, test))]
fn chime(notes: &[(f32, u32)]) -> Vec<u8> {
    const RATE: u32 = 22_050;
    const VOLUME: f32 = 0.22;
    let mut samples: Vec<i16> = Vec::new();
    for &(freq, ms) in notes {
        let n = (RATE * ms / 1000) as usize;
        let attack = (RATE as usize / 250).max(1); // 4 ms
        for i in 0..n {
            let t = i as f32 / RATE as f32;
            let rise = (i as f32 / attack as f32).min(1.0);
            let fall = (1.0 - i as f32 / n as f32).powf(1.5);
            // A sine with a little second harmonic sounds rounder than a pure tone.
            let wave = (std::f32::consts::TAU * freq * t).sin()
                + 0.25 * (std::f32::consts::TAU * 2.0 * freq * t).sin();
            samples.push((wave / 1.25 * rise * fall * VOLUME * i16::MAX as f32) as i16);
        }
    }
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_is_a_valid_quiet_wav() {
        let wav = chime(&[(660.0, 70), (880.0, 90)]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        let riff = u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff + 8, wav.len());
        let data = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(data + 44, wav.len());
        // 160 ms at 22.05 kHz, 2 bytes per sample.
        assert_eq!(data, (22_050 * 70 / 1000 + 22_050 * 90 / 1000) * 2);
        let peak = wav[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b).unsigned_abs())
            .max()
            .unwrap();
        assert!(peak > 1000 && peak < 9000, "peak {peak}");
        // Starts and ends near silence (no clicks).
        let first = i16::from_le_bytes([wav[44], wav[45]]);
        let last = i16::from_le_bytes([wav[wav.len() - 2], wav[wav.len() - 1]]);
        assert!(first.abs() < 200 && last.abs() < 200);
    }
}
