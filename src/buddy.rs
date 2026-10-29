//! "Lali", the little dictation buddy: a round, wobbly character that shows
//! what Lalia is doing while you talk.
//!
//! * listening — it bobs, its mouth moves with your voice and sound rings
//!   ripple out of it; your words unroll in a speech bubble beside it
//! * connecting — dots orbit around it; finishing — "thinking" dots bounce
//! * done — happy ^ ^ eyes and a burst of sparkles; paused — sleepy, with z's
//! * error — > < eyes and a little shake
//!
//! Pure drawing on a tiny-skia pixmap; `overlay.rs` owns the window and calls
//! `Anim::step` + `draw` every frame.

use std::f32::consts::{PI, TAU};

use tiny_skia::{
    FillRule, GradientStop, LineCap, LinearGradient, Paint, Path, PathBuilder, Pixmap, Point, Rect,
    Shader, SpreadMode, Stroke, Transform,
};

use crate::overlay::{
    accent, draw_text, head_fit, measure, pill, rgba, solid, tail_fit, Fonts, Phase,
};

/// Window size in logical pixels (room for rings, sparkles and the bubble).
pub const WIDTH: f32 = 470.0;
pub const HEIGHT: f32 = 136.0;
/// Body radius.
const R: f32 = 28.0;
const GAP: f32 = 16.0;
const BUBBLE_H: f32 = 46.0;
const BUBBLE_MIN: f32 = 150.0;
const BUBBLE_MAX: f32 = 310.0;

type Rgb = (u8, u8, u8);

fn colors(phase: Phase) -> (Rgb, Rgb) {
    match phase {
        Phase::Connecting => ((0xFF, 0xB3, 0x47), (0xFF, 0x6B, 0x9A)),
        Phase::Listening => ((0xFF, 0x5C, 0x8A), (0x8B, 0x5C, 0xF6)),
        Phase::Finishing => ((0x8B, 0x5C, 0xF6), (0x22, 0xC3, 0xE6)),
        Phase::Done | Phase::Resumed => ((0x7C, 0x4D, 0xFF), (0x1F, 0xB6, 0xD9)),
        Phase::Paused => ((0xA3, 0xAA, 0xB8), (0x5B, 0x63, 0x73)),
        Phase::Error => ((0xF8, 0x71, 0x71), (0xFB, 0x92, 0x3C)),
    }
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Everything that moves, advanced once per frame.
pub struct Anim {
    t: f32,
    appear: f32,
    phase: Option<Phase>,
    phase_t: f32,
    from: (Rgb, Rgb),
    level: f32,
    blink_in: f32,
    blink_t: f32,
    rings: Vec<f32>,
    ring_timer: f32,
    gaze: (f32, f32),
    gaze_target: (f32, f32),
    gaze_in: f32,
    caption: f32,
    caption_w: f32,
    seed: u32,
}

impl Default for Anim {
    fn default() -> Self {
        Self {
            t: 0.0,
            appear: 0.0,
            phase: None,
            phase_t: 0.0,
            from: colors(Phase::Listening),
            level: 0.0,
            blink_in: 2.0,
            blink_t: -1.0,
            rings: Vec::new(),
            ring_timer: 0.0,
            gaze: (0.0, 0.0),
            gaze_target: (0.0, 0.0),
            gaze_in: 1.5,
            caption: 0.0,
            caption_w: BUBBLE_MIN,
            seed: 0x9E37_79B9,
        }
    }
}

/// What to show this frame.
pub struct Frame<'a> {
    pub phase: Phase,
    pub label: &'a str,
    pub text: &'a str,
    /// Voice level 0–1 (already smoothed by the caller is fine).
    pub level: f32,
}

impl Anim {
    /// Start over (called when the window appears).
    pub fn reset(&mut self) {
        *self = Self {
            seed: self.seed,
            ..Self::default()
        };
    }

    fn rand(&mut self) -> f32 {
        // xorshift32 → [0, 1)
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn current_colors(&self, phase: Phase) -> (Rgb, Rgb) {
        let to = colors(phase);
        let k = smoothstep(0.0, 0.3, self.phase_t);
        (mix(self.from.0, to.0, k), mix(self.from.1, to.1, k))
    }

    pub fn step(&mut self, dt: f32, frame: &Frame, fonts: &Fonts) {
        self.t += dt;
        self.appear = (self.appear + dt / 0.38).min(1.0);
        if self.phase != Some(frame.phase) {
            if let Some(old) = self.phase {
                self.from = self.current_colors(old);
            } else {
                self.from = colors(frame.phase);
            }
            self.phase = Some(frame.phase);
            self.phase_t = 0.0;
        } else {
            self.phase_t += dt;
        }

        let target = frame.level.clamp(0.0, 1.0);
        let k = if target > self.level { 0.5 } else { 0.12 };
        self.level += (target - self.level) * k;

        // Blinks every 2–5 s (twice, sometimes).
        if self.blink_t >= 0.0 {
            self.blink_t += dt;
            if self.blink_t > 0.16 {
                self.blink_t = -1.0;
            }
        } else {
            self.blink_in -= dt;
            if self.blink_in <= 0.0 {
                self.blink_t = 0.0;
                self.blink_in = if self.rand() < 0.2 {
                    0.22
                } else {
                    2.0 + self.rand() * 3.0
                };
            }
        }

        // Eyes wander a little, and look around while connecting.
        self.gaze_in -= dt;
        if self.gaze_in <= 0.0 {
            self.gaze_target = (self.rand() * 2.0 - 1.0, self.rand() * 1.2 - 0.6);
            self.gaze_in = 0.8 + self.rand() * 2.0;
        }
        let target = match frame.phase {
            Phase::Connecting => ((self.t * 2.6).sin(), -0.2),
            Phase::Finishing => (0.7, -0.9),
            Phase::Listening => (self.gaze_target.0 * 0.4, self.gaze_target.1 * 0.3),
            _ => (0.0, 0.0),
        };
        self.gaze.0 += (target.0 - self.gaze.0) * (dt * 10.0).min(1.0);
        self.gaze.1 += (target.1 - self.gaze.1) * (dt * 10.0).min(1.0);

        // Sound rings while listening: more often when louder.
        for r in &mut self.rings {
            *r += dt / 0.95;
        }
        self.rings.retain(|r| *r < 1.0);
        if frame.phase == Phase::Listening && self.level > 0.06 {
            self.ring_timer -= dt;
            if self.ring_timer <= 0.0 {
                self.rings.push(0.0);
                self.ring_timer = 0.55 - 0.35 * self.level;
            }
        } else {
            self.ring_timer = 0.0;
        }

        // Speech bubble.
        let (show, want_w) = bubble_content(frame, fonts);
        let target = if show { 1.0 } else { 0.0 };
        let speed = if show { 5.0 } else { 7.0 };
        self.caption += (target - self.caption) * (dt * speed).min(1.0);
        if show {
            self.caption_w += (want_w - self.caption_w) * (dt * 8.0).min(1.0);
        }
    }
}

/// Whether to show the speech bubble, and how wide it wants to be.
fn bubble_content(frame: &Frame, fonts: &Fonts) -> (bool, f32) {
    let live = matches!(
        frame.phase,
        Phase::Connecting | Phase::Listening | Phase::Finishing
    );
    let show = !frame.text.trim().is_empty() || (!live && !frame.label.is_empty());
    let text_w = measure(fonts.regular, 14.0, frame.text.trim());
    let label_w = measure(fonts.semibold, 10.0, &frame.label.to_uppercase()) * 1.06;
    (
        show,
        (text_w.max(label_w) + 30.0).clamp(BUBBLE_MIN, BUBBLE_MAX),
    )
}

fn fill(pm: &mut Pixmap, path: &Path, paint: &Paint) {
    pm.fill_path(path, paint, FillRule::Winding, Transform::identity(), None);
}

fn stroke_path(pm: &mut Pixmap, path: &Path, color: tiny_skia::Color, width: f32) {
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    pm.stroke_path(path, &solid(color), &stroke, Transform::identity(), None);
}

fn oval(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<Path> {
    PathBuilder::from_oval(Rect::from_xywh(cx - rx, cy - ry, 2.0 * rx, 2.0 * ry)?)
}

/// A soft, wobbly circle drawn through midpoints for smooth curves.
fn blob(cx: f32, cy: f32, r: f32, sx: f32, sy: f32, wobble: impl Fn(f32) -> f32) -> Option<Path> {
    let n = 64;
    let pts: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * TAU;
            let rr = r * (1.0 + wobble(a));
            (cx + rr * a.cos() * sx, cy + rr * a.sin() * sy)
        })
        .collect();
    let mid = |a: (f32, f32), b: (f32, f32)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let mut pb = PathBuilder::new();
    let m0 = mid(pts[0], pts[1]);
    pb.move_to(m0.0, m0.1);
    for i in 1..=n {
        let p = pts[i % n];
        let m = mid(p, pts[(i + 1) % n]);
        pb.quad_to(p.0, p.1, m.0, m.1);
    }
    pb.close();
    pb.finish()
}

fn star(cx: f32, cy: f32, r: f32) -> Option<Path> {
    // Four-pointed sparkle.
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy - r);
    pb.quad_to(cx, cy, cx + r, cy);
    pb.quad_to(cx, cy, cx, cy + r);
    pb.quad_to(cx, cy, cx - r, cy);
    pb.quad_to(cx, cy, cx, cy - r);
    pb.close();
    pb.finish()
}

/// Draw one frame. `alpha` fades everything; `leaving` shrinks it away.
pub fn draw(
    pm: &mut Pixmap,
    fonts: &Fonts,
    anim: &Anim,
    frame: &Frame,
    scale: f32,
    alpha: f32,
    leaving: bool,
) {
    let s = scale;
    let a = alpha;
    let phase = frame.phase;
    let t = anim.t;
    let pt = anim.phase_t;
    let lvl = if phase == Phase::Listening {
        anim.level
    } else {
        0.0
    };
    let (c1, c2) = anim.current_colors(phase);

    let pop = if leaving {
        0.65 + 0.35 * alpha
    } else {
        ease_out_back(anim.appear)
    };
    let r = R * s * pop;

    // Layout: the buddy slides left as the bubble unrolls.
    let cap = ease_out(anim.caption);
    let bubble_w = anim.caption_w * s;
    let group_w = 2.0 * R * s + cap * (GAP * s + bubble_w);
    let mut cx = (WIDTH * s - group_w) / 2.0 + R * s;
    let bob = match phase {
        Phase::Paused => (t * 1.4).sin() * 1.2 * s,
        _ => (t * 2.4).sin() * 2.0 * s,
    };
    let hop = if matches!(phase, Phase::Done | Phase::Resumed) {
        // A happy little hop when finished.
        -(pt * PI / 0.45).sin().max(0.0) * 7.0 * s * (1.0 - smoothstep(0.0, 0.45, pt))
    } else {
        0.0
    };
    if phase == Phase::Error {
        cx += (pt * 42.0).sin() * 3.5 * s * (1.0 - smoothstep(0.0, 0.55, pt));
    }
    let cy = HEIGHT * s / 2.0 + bob + hop;

    // Ground shadow.
    if let Some(p) = oval(cx, HEIGHT * s / 2.0 + R * s + 9.0 * s, r * 0.75, r * 0.14) {
        fill(pm, &p, &solid(rgba(0, 0, 0, 0.16 * a * pop.min(1.0))));
    }

    // Sound rings.
    for &ring in &anim.rings {
        let e = ease_out(ring);
        let rr = r * (1.04 + 0.85 * e);
        if let Some(p) = oval(cx, cy, rr, rr) {
            let fade = (1.0 - ring).powf(1.6);
            let rc = mix(c1, (255, 255, 255), 0.35);
            stroke_path(
                pm,
                &p,
                rgba(rc.0, rc.1, rc.2, 0.6 * fade * a),
                (2.4 * (1.0 - ring) + 0.8) * s,
            );
        }
    }

    // Orbiting dots while connecting.
    if phase == Phase::Connecting {
        for i in 0..3 {
            let ang = t * 4.2 + i as f32 * TAU / 3.0;
            let (x, y) = (cx + ang.cos() * r * 1.38, cy + ang.sin() * r * 1.38);
            if let Some(p) = oval(x, y, 2.8 * s, 2.8 * s) {
                fill(pm, &p, &solid(rgba(c2.0, c2.1, c2.2, 0.9 * a)));
            }
        }
    }

    // Body: squash and stretch with the voice, wobble a little always.
    let sx = 1.0 + 0.07 * lvl + 0.015 * (t * 2.4).cos();
    let sy = 1.0 - 0.05 * lvl - 0.015 * (t * 2.4).cos();
    let amp = 0.018 + 0.07 * lvl;
    if let Some(body) = blob(cx, cy, r, sx, sy, |ang| {
        amp * (0.6 * (3.0 * ang + t * 3.1).sin() + 0.4 * (5.0 * ang - t * 4.3).sin())
    }) {
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        paint.shader = LinearGradient::new(
            Point::from_xy(cx - r, cy - r),
            Point::from_xy(cx + r, cy + r),
            vec![
                GradientStop::new(0.0, rgba(c1.0, c1.1, c1.2, a)),
                GradientStop::new(1.0, rgba(c2.0, c2.1, c2.2, a)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .unwrap_or(Shader::SolidColor(rgba(c1.0, c1.1, c1.2, a)));
        fill(pm, &body, &paint);
    }
    // Glossy highlight.
    if let Some(p) = oval(cx - 0.36 * r, cy - 0.5 * r, 0.3 * r, 0.15 * r) {
        pm.fill_path(
            &p,
            &solid(rgba(255, 255, 255, 0.38 * a)),
            FillRule::Winding,
            Transform::from_rotate_at(-28.0, cx - 0.36 * r, cy - 0.5 * r),
            None,
        );
    }

    // Face.
    let ink = (0x23, 0x19, 0x42);
    let (gx, gy) = anim.gaze;
    let eye_y = cy - 0.08 * r + gy * 0.07 * r - lvl * 0.04 * r;
    let eyes = [cx - 0.33 * r + gx * 0.09 * r, cx + 0.33 * r + gx * 0.09 * r];
    let line = 0.085 * r;
    match phase {
        Phase::Done | Phase::Resumed => {
            for ex in eyes {
                let mut pb = PathBuilder::new();
                pb.move_to(ex - 0.12 * r, eye_y + 0.04 * r);
                pb.quad_to(ex, eye_y - 0.16 * r, ex + 0.12 * r, eye_y + 0.04 * r);
                if let Some(p) = pb.finish() {
                    stroke_path(pm, &p, rgba(ink.0, ink.1, ink.2, a), line);
                }
            }
        }
        Phase::Paused => {
            for ex in eyes {
                let mut pb = PathBuilder::new();
                pb.move_to(ex - 0.12 * r, eye_y);
                pb.quad_to(ex, eye_y + 0.12 * r, ex + 0.12 * r, eye_y);
                if let Some(p) = pb.finish() {
                    stroke_path(pm, &p, rgba(ink.0, ink.1, ink.2, 0.85 * a), line * 0.9);
                }
            }
        }
        Phase::Error => {
            for (i, ex) in eyes.into_iter().enumerate() {
                let d = if i == 0 { 1.0 } else { -1.0 };
                let mut pb = PathBuilder::new();
                pb.move_to(ex - d * 0.1 * r, eye_y - 0.1 * r);
                pb.line_to(ex + d * 0.08 * r, eye_y);
                pb.line_to(ex - d * 0.1 * r, eye_y + 0.1 * r);
                if let Some(p) = pb.finish() {
                    stroke_path(pm, &p, rgba(ink.0, ink.1, ink.2, a), line);
                }
            }
        }
        _ => {
            let open = if anim.blink_t >= 0.0 {
                let k = (anim.blink_t / 0.16 * PI).sin();
                1.0 - 0.9 * k
            } else {
                1.0
            };
            let big = 1.0 + 0.12 * lvl;
            for ex in eyes {
                if let Some(p) = oval(ex, eye_y, 0.115 * r * big, 0.16 * r * big * open) {
                    fill(pm, &p, &solid(rgba(ink.0, ink.1, ink.2, a)));
                }
                if open > 0.5 {
                    if let Some(p) = oval(ex + 0.04 * r, eye_y - 0.06 * r, 0.042 * r, 0.042 * r) {
                        fill(pm, &p, &solid(rgba(255, 255, 255, 0.95 * a)));
                    }
                }
            }
        }
    }

    // Cheeks.
    for ex in [cx - 0.55 * r, cx + 0.55 * r] {
        if let Some(p) = oval(ex + gx * 0.05 * r, cy + 0.14 * r, 0.13 * r, 0.08 * r) {
            fill(pm, &p, &solid(rgba(0xFF, 0x8F, 0xB1, 0.6 * a)));
        }
    }

    // Mouth.
    let mx = cx + gx * 0.06 * r;
    let my = cy + 0.26 * r;
    match phase {
        Phase::Listening => {
            let w = 0.09 * r + 0.05 * r * lvl;
            let h = 0.035 * r + 0.17 * r * lvl;
            if let Some(p) = oval(mx, my + h * 0.3, w, h) {
                fill(pm, &p, &solid(rgba(ink.0, ink.1, ink.2, a)));
            }
        }
        Phase::Done | Phase::Resumed => {
            let mut pb = PathBuilder::new();
            pb.move_to(mx - 0.17 * r, my - 0.03 * r);
            pb.line_to(mx + 0.17 * r, my - 0.03 * r);
            pb.cubic_to(
                mx + 0.17 * r,
                my + 0.2 * r,
                mx - 0.17 * r,
                my + 0.2 * r,
                mx - 0.17 * r,
                my - 0.03 * r,
            );
            pb.close();
            if let Some(p) = pb.finish() {
                fill(pm, &p, &solid(rgba(ink.0, ink.1, ink.2, a)));
            }
            if let Some(p) = oval(mx, my + 0.09 * r, 0.07 * r, 0.04 * r) {
                fill(pm, &p, &solid(rgba(0xFF, 0x7A, 0x9C, a)));
            }
        }
        Phase::Paused => {
            if let Some(p) = oval(mx, my + 0.02 * r, 0.045 * r, 0.035 * r) {
                fill(pm, &p, &solid(rgba(ink.0, ink.1, ink.2, 0.8 * a)));
            }
        }
        Phase::Error => {
            let mut pb = PathBuilder::new();
            pb.move_to(mx - 0.15 * r, my + 0.05 * r);
            pb.quad_to(mx - 0.075 * r, my - 0.03 * r, mx, my + 0.05 * r);
            pb.quad_to(mx + 0.075 * r, my + 0.13 * r, mx + 0.15 * r, my + 0.05 * r);
            if let Some(p) = pb.finish() {
                stroke_path(pm, &p, rgba(ink.0, ink.1, ink.2, a), line * 0.8);
            }
        }
        _ => {
            let mut pb = PathBuilder::new();
            pb.move_to(mx - 0.1 * r, my);
            pb.quad_to(mx, my + 0.1 * r, mx + 0.1 * r, my);
            if let Some(p) = pb.finish() {
                stroke_path(pm, &p, rgba(ink.0, ink.1, ink.2, a), line * 0.8);
            }
        }
    }

    // Thinking dots while finishing.
    if phase == Phase::Finishing {
        for i in 0..3 {
            let bounce = (t * 7.0 - i as f32 * 0.8).sin().max(0.0) * 0.16 * r;
            let x = cx + 0.62 * r + i as f32 * 0.24 * r;
            let y = cy - 1.18 * r - bounce - i as f32 * 0.06 * r;
            if let Some(p) = oval(x, y, 0.075 * r, 0.075 * r) {
                fill(pm, &p, &solid(rgba(c1.0, c1.1, c1.2, 0.95 * a)));
            }
        }
    }

    // Sparkles bursting out when done.
    if matches!(phase, Phase::Done | Phase::Resumed) && pt < 0.9 {
        let e = ease_out(pt / 0.9);
        for i in 0..6 {
            let ang = -PI / 2.0 + i as f32 * TAU / 6.0 + 0.3;
            let dist = r * (1.05 + 0.75 * e);
            let size = r * 0.3 * (1.0 - e * 0.85) * if i % 2 == 0 { 1.0 } else { 0.7 };
            if let Some(p) = star(cx + ang.cos() * dist, cy + ang.sin() * dist, size) {
                let color = if i % 2 == 0 {
                    (0xFF, 0xE5, 0x8A)
                } else {
                    (255, 255, 255)
                };
                fill(
                    pm,
                    &p,
                    &solid(rgba(color.0, color.1, color.2, a * (1.0 - e * 0.6))),
                );
            }
        }
    }

    // Floating z's while paused.
    if phase == Phase::Paused {
        for i in 0..3 {
            let p = (t * 0.45 + i as f32 / 3.0) % 1.0;
            let x = cx + 0.7 * r + p * 0.55 * r;
            let y = cy - 0.7 * r - p * 0.95 * r;
            let size = (9.0 + 6.0 * p) * s * pop;
            let fade = (p * PI).sin();
            draw_text(
                pm,
                fonts.semibold,
                size,
                (x, y),
                "z",
                ((0x9C, 0xA3, 0xAF), a * fade),
            );
        }
    }

    // Speech bubble with the words (or a message).
    if anim.caption > 0.02 {
        let bw = bubble_w * cap;
        let bh = BUBBLE_H * s;
        let bx = cx + R * s * pop + GAP * s;
        let by = HEIGHT * s / 2.0 - bh / 2.0;
        let ca = a * smoothstep(0.0, 0.4, anim.caption);
        // Tail pointing at the buddy.
        let mut pb = PathBuilder::new();
        pb.move_to(bx + 2.0 * s, by + bh / 2.0 - 7.0 * s);
        pb.line_to(bx - 8.0 * s, by + bh / 2.0 + 2.0 * s);
        pb.line_to(bx + 2.0 * s, by + bh / 2.0 + 7.0 * s);
        pb.close();
        let bubble_fill = solid(rgba(0x16, 0x17, 0x20, 0.94 * ca));
        if let Some(p) = pb.finish() {
            fill(pm, &p, &bubble_fill);
        }
        if let Some(p) = rounded(bx, by, bw.max(bh * 0.5), bh, 15.0 * s) {
            fill(pm, &p, &bubble_fill);
            stroke_path(pm, &p, rgba(255, 255, 255, 0.08 * ca), 1.0 * s);
        }
        let ta = ca * smoothstep(0.55, 1.0, anim.caption);
        if ta > 0.01 {
            let pad = 14.0 * s;
            let max_w = bubble_w - 2.0 * pad;
            let acc = accent(phase);
            let label = frame.label.to_uppercase();
            draw_text(
                pm,
                fonts.semibold,
                10.0 * s,
                (bx + pad, by + 18.0 * s),
                &label,
                (acc, ta),
            );
            let live = matches!(
                phase,
                Phase::Connecting | Phase::Listening | Phase::Finishing
            );
            let text = frame.text.trim();
            let body = if live {
                tail_fit(fonts.regular, 14.0 * s, text, max_w)
            } else {
                head_fit(fonts.regular, 14.0 * s, text, max_w)
            };
            draw_text(
                pm,
                fonts.regular,
                14.0 * s,
                (bx + pad, by + 36.0 * s),
                &body,
                ((0xF3, 0xF4, 0xF8), ta),
            );
        }
    }
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 2.0 * r {
        return pill(x, y, w, h);
    }
    let k = 0.5523 * r;
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

/// Render a little film of every state as PNG frames (for previews and docs).
pub fn film(dir: &std::path::Path) -> anyhow::Result<()> {
    use ab_glyph::FontRef;
    std::fs::create_dir_all(dir)?;
    let regular = FontRef::try_from_slice(crate::overlay::FONT_REGULAR)?;
    let semibold = FontRef::try_from_slice(crate::overlay::FONT_SEMIBOLD)?;
    let fonts = Fonts {
        regular: &regular,
        semibold: &semibold,
    };
    let script: &[(f32, Phase, &str, &str)] = &[
        (0.9, Phase::Connecting, "Connecting", ""),
        (1.2, Phase::Listening, "Listening · F12", ""),
        (1.0, Phase::Listening, "Listening · F12", "so the idea is"),
        (
            1.6,
            Phase::Listening,
            "Listening · F12",
            "so the idea is that you just hold the key and talk",
        ),
        (
            0.8,
            Phase::Finishing,
            "Finishing",
            "so the idea is that you just hold the key and talk",
        ),
        (
            1.4,
            Phase::Done,
            "Typed 11 words",
            "So the idea is that you just hold the key and talk.",
        ),
        (
            1.6,
            Phase::Paused,
            "Paused",
            "Dictation is off — tap F11 to resume",
        ),
    ];
    let fps = 25.0;
    let scale = 2.0;
    let mut anim = Anim::default();
    let mut n = 0;
    let mut clock = 0.0f32;
    for &(secs, phase, label, text) in script {
        let frames = (secs * fps) as usize;
        for _ in 0..frames {
            clock += 1.0 / fps;
            // A fake voice: syllable-like bursts.
            let voice = if phase == Phase::Listening {
                ((clock * 9.0).sin() * 0.5 + 0.5) * ((clock * 2.3).sin() * 0.5 + 0.6).min(1.0)
            } else {
                0.0
            };
            let frame = Frame {
                phase,
                label,
                text,
                level: voice,
            };
            anim.step(1.0 / fps, &frame, &fonts);
            let mut pm = Pixmap::new((WIDTH * scale) as u32, (HEIGHT * scale) as u32)
                .ok_or_else(|| anyhow::anyhow!("pixmap"))?;
            draw(&mut pm, &fonts, &anim, &frame, scale, 1.0, false);
            pm.save_png(dir.join(format!("frame-{n:03}.png")))?;
            n += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::FontRef;

    #[test]
    fn every_phase_draws_something() {
        let regular = FontRef::try_from_slice(crate::overlay::FONT_REGULAR).unwrap();
        let semibold = FontRef::try_from_slice(crate::overlay::FONT_SEMIBOLD).unwrap();
        let fonts = Fonts {
            regular: &regular,
            semibold: &semibold,
        };
        for phase in [
            Phase::Connecting,
            Phase::Listening,
            Phase::Finishing,
            Phase::Done,
            Phase::Paused,
            Phase::Resumed,
            Phase::Error,
        ] {
            let mut anim = Anim::default();
            let frame = Frame {
                phase,
                label: "Label",
                text: "some words",
                level: 0.6,
            };
            for _ in 0..30 {
                anim.step(1.0 / 30.0, &frame, &fonts);
            }
            let mut pm = Pixmap::new(WIDTH as u32, HEIGHT as u32).unwrap();
            draw(&mut pm, &fonts, &anim, &frame, 1.0, 1.0, false);
            let painted = pm.pixels().iter().filter(|p| p.alpha() > 0).count();
            assert!(painted > 2000, "{phase:?} painted {painted}");
        }
    }

    #[test]
    fn easing_lands_on_one() {
        assert!((ease_out_back(1.0) - 1.0).abs() < 1e-5);
        assert!(ease_out_back(0.6) > 1.0, "overshoots for the pop");
        assert_eq!(ease_out(2.0), 1.0);
    }
}
