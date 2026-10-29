//! Microphone capture → 16 kHz mono PCM16, in ~100 ms chunks.
//!
//! The cpal stream is created and kept alive on a dedicated OS thread (cpal
//! streams are not always `Send`), and audio chunks are handed off over a
//! tokio unbounded channel. `start` waits until the stream is actually
//! running, so a missing or busy microphone is reported to the caller instead
//! of silently producing no audio.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use tokio::sync::mpsc::UnboundedSender;

const TARGET_RATE: f64 = 16_000.0;
const CHUNK_SAMPLES: usize = 1_600; // 100 ms @ 16 kHz
const OPEN_TIMEOUT: Duration = Duration::from_secs(4);

/// Latest input level (peak of the last callback, 0.0–1.0) as f32 bits.
#[derive(Clone, Default)]
pub struct Level(Arc<AtomicU32>);

impl Level {
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// A running capture. Dropping it (or calling `stop`) releases the microphone.
pub struct Capture {
    stop: Arc<AtomicBool>,
    pub level: Level,
}

impl Capture {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Streaming linear resampler from the device rate to 16 kHz.
struct Resampler {
    step: f64, // input samples per output sample
    pos: f64,
    buf: Vec<f32>,
    primed: bool,
}

impl Resampler {
    fn new(in_rate: f64) -> Self {
        Self {
            step: in_rate / TARGET_RATE,
            pos: 0.0,
            buf: Vec::new(),
            primed: false,
        }
    }

    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        if !self.primed {
            // Left edge for interpolation — only at the very start. (Doing it
            // whenever the buffer ran empty duplicated samples and skewed timing.)
            self.buf.push(input[0]);
            self.primed = true;
        }
        self.buf.extend_from_slice(input);

        while self.pos + 1.0 < self.buf.len() as f64 {
            let i = self.pos.floor() as usize;
            let f = (self.pos - i as f64) as f32;
            out.push(self.buf[i] * (1.0 - f) + self.buf[i + 1] * f);
            self.pos += self.step;
        }

        let drop = (self.pos.floor() as usize).min(self.buf.len());
        if drop > 0 {
            self.buf.drain(0..drop);
            self.pos -= drop as f64;
        }
    }
}

/// Owns the resampler + chunk buffer for one capture session.
struct Capturer {
    rs: Resampler,
    pending: Vec<i16>,
    tx: UnboundedSender<Vec<i16>>,
    channels: usize,
    mono: Vec<f32>,
    out: Vec<f32>,
    level: Level,
}

impl Capturer {
    fn new(in_rate: f64, channels: usize, tx: UnboundedSender<Vec<i16>>, level: Level) -> Self {
        Self {
            rs: Resampler::new(in_rate),
            pending: Vec::with_capacity(CHUNK_SAMPLES * 2),
            tx,
            channels,
            mono: Vec::new(),
            out: Vec::new(),
            level,
        }
    }

    fn push_f32(&mut self, data: &[f32]) {
        self.mono.clear();
        let ch = self.channels.max(1);
        let mut peak = 0f32;
        for frame in data.chunks(ch) {
            let sum: f32 = frame.iter().sum();
            let v = sum / ch as f32;
            peak = peak.max(v.abs());
            self.mono.push(v);
        }
        self.level.set(peak.min(1.0));
        self.out.clear();
        self.rs.push(&self.mono, &mut self.out);
        for &s in &self.out {
            self.pending
                .push((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
        }
        while self.pending.len() >= CHUNK_SAMPLES {
            let chunk: Vec<i16> = self.pending.drain(0..CHUNK_SAMPLES).collect();
            let _ = self.tx.send(chunk);
        }
    }

    fn push_i16(&mut self, data: &[i16]) {
        let f: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
        self.push_f32(&f);
    }

    fn push_u16(&mut self, data: &[u16]) {
        let f: Vec<f32> = data
            .iter()
            .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0)
            .collect();
        self.push_f32(&f);
    }
}

/// Names of the available input devices (for the settings window).
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    names.dedup();
    names
}

fn find_device(name: &str) -> anyhow::Result<cpal::Device> {
    let host = cpal::default_host();
    if !name.trim().is_empty() {
        if let Ok(mut devices) = host.input_devices() {
            if let Some(d) = devices.find(|d| d.name().map(|n| n == name).unwrap_or(false)) {
                return Ok(d);
            }
        }
        eprintln!("[lalia] microphone '{name}' not found; using the default");
    }
    host.default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no microphone found (no default input device)"))
}

/// Start capturing from `device` ("" = default). Returns once audio is
/// flowing, or with the reason it could not start. Errors that happen later
/// (device unplugged…) go to `on_error`.
pub fn start(
    device: &str,
    tx: UnboundedSender<Vec<i16>>,
    on_error: impl Fn(String) + Send + Sync + 'static,
) -> anyhow::Result<Capture> {
    let device = find_device(device)?;
    let supported = device
        .default_input_config()
        .map_err(|e| anyhow::anyhow!("cannot read microphone settings: {e}"))?;
    let in_rate = supported.sample_rate().0 as f64;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let level = Level::default();
    let level_thread = level.clone();
    let (ready_tx, ready_rx) = std_mpsc::sync_channel::<Result<(), String>>(1);
    let on_error = Arc::new(on_error);

    thread::spawn(move || {
        let err_fn = {
            let on_error = on_error.clone();
            move |e: cpal::StreamError| on_error(format!("microphone error: {e}"))
        };
        let cap = Capturer::new(in_rate, channels, tx, level_thread);
        let stream = match format {
            SampleFormat::F32 => {
                let mut cap = cap;
                device.build_input_stream(
                    &config,
                    move |d: &[f32], _| cap.push_f32(d),
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let mut cap = cap;
                device.build_input_stream(
                    &config,
                    move |d: &[i16], _| cap.push_i16(d),
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let mut cap = cap;
                device.build_input_stream(
                    &config,
                    move |d: &[u16], _| cap.push_u16(d),
                    err_fn,
                    None,
                )
            }
            other => {
                let _ = ready_tx.send(Err(format!("unsupported sample format {other:?}")));
                return;
            }
        };
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("cannot open microphone: {e}")));
                return;
            }
        };
        if let Err(e) = stream.play() {
            let _ = ready_tx.send(Err(format!("cannot start microphone: {e}")));
            return;
        }
        eprintln!("[lalia] microphone open @ {in_rate} Hz, {channels} ch");
        let _ = ready_tx.send(Ok(()));

        while !stop_thread.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(20));
        }
        drop(stream);
    });

    match ready_rx.recv_timeout(OPEN_TIMEOUT) {
        Ok(Ok(())) => Ok(Capture { stop, level }),
        Ok(Err(e)) => anyhow::bail!(e),
        Err(_) => {
            stop.store(true, Ordering::Relaxed);
            anyhow::bail!(
                "the microphone did not start within {}s",
                OPEN_TIMEOUT.as_secs()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resample(rate: f64, input: &[f32], block: usize) -> Vec<f32> {
        let mut rs = Resampler::new(rate);
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            rs.push(chunk, &mut out);
        }
        out
    }

    #[test]
    fn downsamples_48k_to_16k() {
        let input = vec![0.25f32; 48_000];
        let out = resample(48_000.0, &input, 480);
        assert!((out.len() as i64 - 16_000).abs() <= 2, "{}", out.len());
        assert!(out.iter().all(|&s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    fn block_size_does_not_change_output() {
        let input: Vec<f32> = (0..44_100).map(|i| (i as f32 * 0.01).sin()).collect();
        let a = resample(44_100.0, &input, 441);
        let b = resample(44_100.0, &input, 1024);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < 1e-5);
        }
    }

    #[test]
    fn capturer_downmixes_chunks_and_tracks_level() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let level = Level::default();
        let mut cap = Capturer::new(16_000.0, 2, tx, level.clone());
        // Stereo frames alternating between 0.5 and silence on both channels.
        let mut data = Vec::new();
        for i in 0..3_200 {
            let v = if i % 2 == 0 { 0.5 } else { 0.0 };
            data.extend_from_slice(&[v, v]);
        }
        cap.push_f32(&data);
        let chunk = rx.try_recv().expect("one 100 ms chunk");
        assert_eq!(chunk.len(), CHUNK_SAMPLES);
        assert!((level.get() - 0.5).abs() < 1e-6);
        assert!(chunk.iter().any(|&s| s > 16_000));
    }
}
