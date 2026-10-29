//! Microphone capture → 16 kHz mono PCM16, in ~100 ms chunks.
//!
//! The cpal stream is created and kept alive on a dedicated OS thread (cpal
//! streams are not always `Send`), and audio chunks are handed off over a
//! tokio unbounded channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use tokio::sync::mpsc::UnboundedSender;

const TARGET_RATE: f64 = 16_000.0;
const CHUNK_SAMPLES: usize = 1_600; // 100 ms @ 16 kHz

/// Streaming linear resampler from the device rate to 16 kHz.
struct Resampler {
    step: f64, // input samples per output sample
    pos: f64,
    buf: Vec<f32>,
}

impl Resampler {
    fn new(in_rate: f64) -> Self {
        Self {
            step: in_rate / TARGET_RATE,
            pos: 0.0,
            buf: Vec::new(),
        }
    }

    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        if self.buf.is_empty() {
            self.buf.push(input[0]); // left edge for interpolation
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
}

impl Capturer {
    fn new(in_rate: f64, channels: usize, tx: UnboundedSender<Vec<i16>>) -> Self {
        Self {
            rs: Resampler::new(in_rate),
            pending: Vec::with_capacity(CHUNK_SAMPLES * 2),
            tx,
            channels,
            mono: Vec::new(),
            out: Vec::new(),
        }
    }

    fn push_f32(&mut self, data: &[f32]) {
        self.mono.clear();
        let ch = self.channels.max(1);
        for frame in data.chunks(ch) {
            let sum: f32 = frame.iter().sum();
            self.mono.push(sum / ch as f32);
        }
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

/// Start capturing. Returns a stop flag; set `true` to release the microphone.
pub fn start(tx: UnboundedSender<Vec<i16>>) -> anyhow::Result<Arc<AtomicBool>> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no default input device (microphone)"))?;
    let supported = device.default_input_config()?;
    let in_rate = supported.sample_rate().0 as f64;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    thread::spawn(move || {
        let err_fn = |e| eprintln!("[lalia] audio stream error: {e}");

        let stream = match format {
            SampleFormat::F32 => {
                let mut cap = Capturer::new(in_rate, channels, tx);
                device.build_input_stream(
                    &config,
                    move |d: &[f32], _| cap.push_f32(d),
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let mut cap = Capturer::new(in_rate, channels, tx);
                device.build_input_stream(
                    &config,
                    move |d: &[i16], _| cap.push_i16(d),
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let mut cap = Capturer::new(in_rate, channels, tx);
                device.build_input_stream(
                    &config,
                    move |d: &[u16], _| cap.push_u16(d),
                    err_fn,
                    None,
                )
            }
            other => {
                eprintln!("[lalia] unsupported sample format {other:?}");
                return;
            }
        };

        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[lalia] cannot open microphone: {e}");
                return;
            }
        };
        if let Err(e) = stream.play() {
            eprintln!("[lalia] cannot start audio stream: {e}");
            return;
        }
        eprintln!("[lalia] microphone open @ {in_rate} Hz, {channels} ch");

        while !stop_thread.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(40));
        }
        drop(stream);
    });

    Ok(stop)
}
