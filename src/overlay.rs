//! The on-screen indicator: Lali the buddy, or a small pill, near the bottom
//! (or top) of the screen while you dictate.
//!
//! The window never takes focus or clicks — typing keeps going to the window
//! you were in. This file holds the state, the frame loop and the pill
//! drawing; `buddy.rs` draws Lali; the window itself is platform work
//! (`overlay/x11.rs`, `overlay/windows.rs`). Drawing uses tiny-skia and
//! ab_glyph on its own thread.

use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, PixmapPaint, Point,
    Rect, Shader, SpreadMode, Stroke, Transform,
};
#[cfg(windows)]
#[path = "overlay/windows.rs"]
mod backend;
#[cfg(unix)]
#[path = "overlay/x11.rs"]
mod backend;

use crate::audio::Level;
use crate::buddy;
use crate::config::{OverlayPosition, OverlayStyle};
use crate::icon::{self, IconState};

pub static FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
pub static FONT_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Listening,
    Finishing,
    Done,
    Paused,
    Resumed,
    Error,
}

struct Model {
    enabled: bool,
    position: OverlayPosition,
    style: OverlayStyle,
    visible: bool,
    phase: Phase,
    label: String,
    text: String,
    level: Option<Level>,
    hide_at: Option<Instant>,
}

/// Handle to the overlay thread. Every method is cheap and never blocks.
#[derive(Clone)]
pub struct Overlay {
    model: Arc<Mutex<Model>>,
    wake: mpsc::Sender<()>,
}

impl Overlay {
    pub fn start(enabled: bool, position: OverlayPosition, style: OverlayStyle) -> Self {
        let model = Arc::new(Mutex::new(Model {
            enabled,
            position,
            style,
            visible: false,
            phase: Phase::Connecting,
            label: String::new(),
            text: String::new(),
            level: None,
            hide_at: None,
        }));
        let (wake, rx) = mpsc::channel();
        let m = model.clone();
        thread::spawn(move || {
            if let Err(e) = run(m, rx) {
                eprintln!("[lalia] overlay disabled: {e}");
            }
        });
        Self { model, wake }
    }

    fn update(&self, f: impl FnOnce(&mut Model)) {
        if let Ok(mut m) = self.model.lock() {
            f(&mut m);
        }
        let _ = self.wake.send(());
    }

    pub fn configure(&self, enabled: bool, position: OverlayPosition, style: OverlayStyle) {
        self.update(|m| {
            m.enabled = enabled;
            m.position = position;
            m.style = style;
        });
    }

    pub fn is_enabled(&self) -> bool {
        self.model.lock().map(|m| m.enabled).unwrap_or(false)
    }

    /// Show the pill for a new session.
    pub fn show(&self, phase: Phase, label: &str, level: Option<Level>) {
        self.update(|m| {
            m.visible = true;
            m.phase = phase;
            m.label = label.to_string();
            m.text.clear();
            m.level = level;
            m.hide_at = None;
        });
    }

    pub fn set_phase(&self, phase: Phase, label: &str) {
        self.update(|m| {
            m.phase = phase;
            m.label = label.to_string();
        });
    }

    pub fn set_text(&self, text: &str) {
        self.update(|m| m.text = text.replace(['\n', '\r'], " "));
    }

    /// Show a message for `secs`, then hide.
    pub fn flash(&self, phase: Phase, label: &str, text: &str, secs: f32) {
        self.update(|m| {
            m.visible = true;
            m.phase = phase;
            m.label = label.to_string();
            m.text = text.replace(['\n', '\r'], " ");
            m.level = None;
            m.hide_at = Some(Instant::now() + Duration::from_secs_f32(secs));
        });
    }

    pub fn hide(&self) {
        self.update(|m| {
            m.visible = false;
            m.level = None;
        });
    }
}

// ---------------------------------------------------------------------------
// The window and its frame loop
// ---------------------------------------------------------------------------

const PILL_W: f32 = 440.0;
const PILL_H: f32 = 60.0;
const SHADOW: f32 = 16.0;
const MARGIN_EDGE: f32 = 72.0;

/// The window while it is on screen (it is destroyed when hidden, so a
/// changed DPI, monitor or compositor is picked up the next time).
struct Surface {
    win: backend::Window,
    /// Per-pixel transparency (else an opaque pill-shaped window).
    transparent: bool,
    /// Drawing the animated buddy (else the pill).
    buddy: bool,
    w: u32,
    h: u32,
}

struct Snapshot {
    enabled: bool,
    position: OverlayPosition,
    style: OverlayStyle,
    visible: bool,
    phase: Phase,
    label: String,
    text: String,
    level: f32,
}

fn run(model: Arc<Mutex<Model>>, wake: mpsc::Receiver<()>) -> anyhow::Result<()> {
    let backend = backend::Backend::connect()?;
    let regular = FontRef::try_from_slice(FONT_REGULAR)?;
    let semibold = FontRef::try_from_slice(FONT_SEMIBOLD)?;

    let mut surface: Option<Surface> = None;
    let mut scale = 1.0;
    let mut fade = 0.0f32;
    let mut smooth_level = 0.0f32;
    let mut last = Instant::now();
    let started = Instant::now();
    let mut icon_cache: Option<(IconState, u32, Pixmap)> = None;
    let mut anim = buddy::Anim::default();
    let fonts = Fonts {
        regular: &regular,
        semibold: &semibold,
    };

    loop {
        let timeout = match surface.as_ref() {
            Some(s) if s.buddy => Duration::from_millis(20),
            Some(_) => Duration::from_millis(33),
            None => Duration::from_millis(400),
        };
        match wake.recv_timeout(timeout) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
        while wake.try_recv().is_ok() {}
        backend.pump();

        let snap = {
            let mut m = model
                .lock()
                .map_err(|_| anyhow::anyhow!("overlay state poisoned"))?;
            if m.hide_at.is_some_and(|t| Instant::now() >= t) {
                m.visible = false;
                m.hide_at = None;
            }
            Snapshot {
                enabled: m.enabled,
                position: m.position,
                style: m.style,
                visible: m.visible,
                phase: m.phase,
                label: m.label.clone(),
                text: m.text.clone(),
                level: m.level.as_ref().map(Level::get).unwrap_or(0.0),
            }
        };
        let want = snap.enabled && snap.visible;

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32().min(0.1);
        last = now;

        if want && surface.is_none() {
            scale = backend.scale();
            let transparent = backend.transparent();
            // The buddy needs real transparency; otherwise use the pill.
            let buddy = snap.style == OverlayStyle::Buddy && transparent;
            let pad = if transparent { SHADOW } else { 0.0 };
            let (w, h) = if buddy {
                (buddy::WIDTH * scale, buddy::HEIGHT * scale)
            } else {
                ((PILL_W + 2.0 * pad) * scale, (PILL_H + 2.0 * pad) * scale)
            };
            let (w, h) = (w.round() as u32, h.round() as u32);
            let win = backend.create(w, h, transparent)?;
            // The buddy's window has room for rings around it, so it sits closer.
            let edge = if buddy {
                MARGIN_EDGE * 0.55
            } else {
                MARGIN_EDGE
            };
            backend.show(&win, snap.position, (edge * scale) as i32)?;
            anim.reset();
            surface = Some(Surface {
                win,
                transparent,
                buddy,
                w,
                h,
            });
        }
        let Some(s) = surface.as_ref() else { continue };

        if s.transparent {
            let out = if s.buddy { 0.24 } else { 0.18 };
            fade = if want {
                (fade + dt / 0.12).min(1.0)
            } else {
                (fade - dt / out).max(0.0)
            };
        } else {
            fade = if want { 1.0 } else { 0.0 };
        }
        if fade <= 0.0 && !want {
            if let Some(s) = surface.take() {
                backend.destroy(s.win);
            }
            continue;
        }

        let loud = (snap.level * 3.0).sqrt().min(1.0);
        smooth_level = if loud > smooth_level {
            loud
        } else {
            smooth_level * 0.85
        };

        let mut pm = Pixmap::new(s.w, s.h).ok_or_else(|| anyhow::anyhow!("overlay pixmap"))?;
        if s.buddy {
            let frame = buddy::Frame {
                phase: snap.phase,
                label: &snap.label,
                text: &snap.text,
                level: smooth_level,
            };
            anim.step(dt, &frame, &fonts);
            buddy::draw(&mut pm, &fonts, &anim, &frame, scale, fade, !want);
            backend.present(&s.win, &pm)?;
            continue;
        }
        let state = match snap.phase {
            Phase::Listening => IconState::Listening,
            Phase::Connecting | Phase::Finishing => IconState::Busy,
            Phase::Paused => IconState::Paused,
            Phase::Error => IconState::Attention,
            Phase::Done | Phase::Resumed => IconState::Idle,
        };
        let icon_px = (36.0 * scale).round() as u32;
        if icon_cache.as_ref().map(|c| (c.0, c.1)) != Some((state, icon_px)) {
            icon_cache = Some((state, icon_px, icon::render(icon_px, state)));
        }
        let icon_pm = &icon_cache.as_ref().expect("icon cache").2;
        draw(
            &mut pm,
            &fonts,
            &snap,
            icon_pm,
            DrawParams {
                scale,
                pad: if s.transparent { SHADOW } else { 0.0 },
                alpha: fade,
                level: smooth_level,
                t: started.elapsed().as_secs_f32(),
                shadow: s.transparent,
            },
        );
        backend.present(&s.win, &pm)?;
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

pub struct Fonts<'a> {
    pub regular: &'a FontRef<'a>,
    pub semibold: &'a FontRef<'a>,
}

struct DrawParams {
    scale: f32,
    pad: f32,
    alpha: f32,
    level: f32,
    t: f32,
    shadow: bool,
}

pub(crate) fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba8(r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

pub(crate) fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

/// A capsule (fully rounded ends) at (x, y) of size w×h.
pub(crate) fn pill(x: f32, y: f32, w: f32, h: f32) -> Option<tiny_skia::Path> {
    Rect::from_xywh(x, y, w, h)?;
    let r = (h / 2.0).min(w / 2.0);
    let k = 0.5523 * r; // cubic approximation of a quarter circle
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

pub(crate) fn accent(phase: Phase) -> (u8, u8, u8) {
    match phase {
        Phase::Listening => (0xFF, 0x6B, 0x8B),
        Phase::Connecting | Phase::Finishing => (0xFB, 0xBF, 0x24),
        Phase::Done | Phase::Resumed => (0x34, 0xD3, 0x99),
        Phase::Paused => (0x9C, 0xA3, 0xAF),
        Phase::Error => (0xF8, 0x71, 0x71),
    }
}

/// Lay out `text` and return its advance width.
pub(crate) fn measure(font: &FontRef, px: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut prev = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            w += scaled.kern(p, id);
        }
        w += scaled.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Draw `text` with its baseline starting at `(x, y)`; returns the advance width.
pub(crate) fn draw_text(
    pm: &mut Pixmap,
    font: &FontRef,
    px: f32,
    (x, y): (f32, f32),
    text: &str,
    (color, alpha): ((u8, u8, u8), f32),
) -> f32 {
    let scaled = font.as_scaled(PxScale::from(px));
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let mut caret = x;
    let mut prev = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            caret += scaled.kern(p, id);
        }
        let glyph = id.with_scale_and_position(px, ab_glyph::point(caret, y));
        caret += scaled.h_advance(id);
        prev = Some(id);
        let Some(outlined) = font.outline_glyph(glyph) else {
            continue;
        };
        let b = outlined.px_bounds();
        outlined.draw(|gx, gy, cov| {
            let px_x = b.min.x as i32 + gx as i32;
            let px_y = b.min.y as i32 + gy as i32;
            if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                return;
            }
            let a = (cov * alpha).clamp(0.0, 1.0);
            if a <= 0.0 {
                return;
            }
            let i = ((px_y * w + px_x) * 4) as usize;
            let src = [
                color.0 as f32 * a,
                color.1 as f32 * a,
                color.2 as f32 * a,
                255.0 * a,
            ];
            for k in 0..4 {
                let dst = data[i + k] as f32;
                data[i + k] = (src[k] + dst * (1.0 - a)).round().min(255.0) as u8;
            }
        });
    }
    caret - x
}

/// The end of `text` that fits in `max` px, prefixed with "…" when cut.
pub(crate) fn tail_fit(font: &FontRef, px: f32, text: &str, max: f32) -> String {
    if measure(font, px, text) <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0;
    while lo < chars.len() {
        let candidate: String = std::iter::once('…')
            .chain(chars[lo..].iter().copied())
            .collect();
        if measure(font, px, &candidate) <= max {
            return candidate;
        }
        lo += 1;
    }
    String::new()
}

/// The start of `text` that fits in `max` px, suffixed with "…" when cut.
pub(crate) fn head_fit(font: &FontRef, px: f32, text: &str, max: f32) -> String {
    if measure(font, px, text) <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        let candidate: String = chars[..n]
            .iter()
            .collect::<String>()
            .trim_end()
            .chars()
            .chain(std::iter::once('…'))
            .collect();
        if measure(font, px, &candidate) <= max {
            return candidate;
        }
        n -= 1;
    }
    String::new()
}

fn draw(pm: &mut Pixmap, fonts: &Fonts, snap: &Snapshot, icon_pm: &Pixmap, p: DrawParams) {
    let s = p.scale;
    let a = p.alpha;
    let pad = p.pad * s;
    let (x0, y0, w, h) = (pad, pad, PILL_W * s, PILL_H * s);

    if p.shadow {
        // Soft shadow: many faint layers so no bands show.
        for i in 0..12 {
            let grow = (i as f32 + 1.0) * 1.1 * s;
            if let Some(path) = pill(
                x0 - grow,
                y0 - grow + 2.5 * s,
                w + 2.0 * grow,
                h + 2.0 * grow,
            ) {
                pm.fill_path(
                    &path,
                    &solid(rgba(0, 0, 0, 0.028 * a)),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
    if let Some(path) = pill(x0, y0, w, h) {
        pm.fill_path(
            &path,
            &solid(rgba(0x16, 0x17, 0x20, 0.95 * a)),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let stroke = Stroke {
            width: 1.0 * s,
            ..Stroke::default()
        };
        pm.stroke_path(
            &path,
            &solid(rgba(255, 255, 255, 0.09 * a)),
            &stroke,
            Transform::identity(),
            None,
        );
    }

    // Icon with a halo that breathes with the voice.
    let (ar, ag, ab) = accent(snap.phase);
    let icon_size = icon_pm.width() as f32;
    let ix = x0 + 12.0 * s;
    let iy = y0 + (h - icon_size) / 2.0;
    if snap.phase == Phase::Listening {
        let halo = icon_size * (0.56 + 0.1 * p.level);
        if let Some(c) = PathBuilder::from_circle(ix + icon_size / 2.0, iy + icon_size / 2.0, halo)
        {
            pm.fill_path(
                &c,
                &solid(rgba(ar, ag, ab, (0.10 + 0.25 * p.level) * a)),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    pm.draw_pixmap(
        ix as i32,
        iy as i32,
        icon_pm.as_ref(),
        &PixmapPaint {
            opacity: a,
            ..PixmapPaint::default()
        },
        Transform::identity(),
        None,
    );

    // Text column.
    let meter_w = 52.0 * s;
    let tx = ix + icon_size + 12.0 * s;
    let live = matches!(
        snap.phase,
        Phase::Connecting | Phase::Listening | Phase::Finishing
    );
    let max_w = if live {
        x0 + w - meter_w - 18.0 * s - tx
    } else {
        x0 + w - 24.0 * s - tx
    };
    let label = snap.label.to_uppercase();
    draw_text(
        pm,
        fonts.semibold,
        10.5 * s,
        (tx, y0 + 23.0 * s),
        &label,
        ((ar, ag, ab), a),
    );
    let (body, color, body_alpha) = if snap.text.trim().is_empty() {
        let hint = match snap.phase {
            Phase::Connecting => "Connecting to Gemini…",
            Phase::Listening => "Speak now…",
            Phase::Finishing => "Finishing up…",
            _ => "",
        };
        (hint.to_string(), (0x9C, 0xA3, 0xAF), 0.9)
    } else {
        (snap.text.trim().to_string(), (0xF3, 0xF4, 0xF8), 1.0)
    };
    let body_px = 15.0 * s;
    // Live text keeps its newest words; messages keep their beginning.
    let fitted = if live {
        tail_fit(fonts.regular, body_px, &body, max_w)
    } else {
        head_fit(fonts.regular, body_px, &body, max_w)
    };
    draw_text(
        pm,
        fonts.regular,
        body_px,
        (tx, y0 + 44.0 * s),
        &fitted,
        (color, a * body_alpha),
    );

    // Level meter / activity bars.
    if !live {
        return;
    }
    let bars = 5;
    let bar_w = 4.0 * s;
    let gap = 4.5 * s;
    let total = bars as f32 * bar_w + (bars - 1) as f32 * gap;
    let bx0 = x0 + w - 18.0 * s - total;
    let cy = y0 + h / 2.0;
    let max_h = 26.0 * s;
    let min_h = 4.0 * s;
    for i in 0..bars {
        let fi = i as f32;
        let k = match snap.phase {
            Phase::Listening => {
                let wobble = 0.55 + 0.45 * ((p.t * 9.0 + fi * 1.7).sin() * 0.5 + 0.5);
                let shape = 1.0 - ((fi - 2.0).abs() / 3.0);
                (p.level * wobble * (0.55 + 0.45 * shape)).min(1.0)
            }
            Phase::Connecting | Phase::Finishing => {
                0.25 + 0.25 * ((p.t * 6.0 - fi * 0.9).sin() * 0.5 + 0.5)
            }
            _ => 0.0,
        };
        let bh = min_h + (max_h - min_h) * k;
        let x = bx0 + fi * (bar_w + gap);
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        paint.shader = LinearGradient::new(
            Point::from_xy(x, cy - max_h / 2.0),
            Point::from_xy(x, cy + max_h / 2.0),
            vec![
                GradientStop::new(0.0, rgba(ar, ag, ab, a)),
                GradientStop::new(
                    1.0,
                    rgba(icon::BRAND_A.0, icon::BRAND_A.1, icon::BRAND_A.2, a),
                ),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .unwrap_or(Shader::SolidColor(rgba(ar, ag, ab, a)));
        if let Some(path) = pill(x, cy - bh / 2.0, bar_w, bh) {
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// Render the pill to a PNG (used to preview the design without X11).
pub fn preview(
    path: &std::path::Path,
    phase: Phase,
    label: &str,
    text: &str,
) -> anyhow::Result<()> {
    let regular = FontRef::try_from_slice(FONT_REGULAR)?;
    let semibold = FontRef::try_from_slice(FONT_SEMIBOLD)?;
    let scale = 2.0;
    let w = ((PILL_W + 2.0 * SHADOW) * scale) as u32;
    let h = ((PILL_H + 2.0 * SHADOW) * scale) as u32;
    let mut pm = Pixmap::new(w, h).ok_or_else(|| anyhow::anyhow!("pixmap"))?;
    pm.fill(Color::from_rgba8(0xE9, 0xEC, 0xF2, 255));
    let state = match phase {
        Phase::Listening => IconState::Listening,
        Phase::Connecting | Phase::Finishing => IconState::Busy,
        Phase::Paused => IconState::Paused,
        _ => IconState::Idle,
    };
    let icon_pm = icon::render((36.0 * scale) as u32, state);
    draw(
        &mut pm,
        &Fonts {
            regular: &regular,
            semibold: &semibold,
        },
        &Snapshot {
            enabled: true,
            position: OverlayPosition::Bottom,
            style: OverlayStyle::Pill,
            visible: true,
            phase,
            label: label.to_string(),
            text: text.to_string(),
            level: 0.7,
        },
        &icon_pm,
        DrawParams {
            scale,
            pad: SHADOW,
            alpha: 1.0,
            level: 0.7,
            t: 0.4,
            shadow: true,
        },
    );
    pm.save_png(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_fit_keeps_the_end() {
        let font = FontRef::try_from_slice(FONT_REGULAR).unwrap();
        let text = "the quick brown fox jumps over the lazy dog";
        let full = measure(&font, 15.0, text);
        let cut = tail_fit(&font, 15.0, text, full / 2.0);
        assert!(cut.starts_with('…'));
        assert!(cut.ends_with("lazy dog"));
        assert!(measure(&font, 15.0, &cut) <= full / 2.0);
        assert_eq!(tail_fit(&font, 15.0, "short", 500.0), "short");
        let head = head_fit(&font, 15.0, text, full / 2.0);
        assert!(head.starts_with("the quick") && head.ends_with('…'));
    }
}
