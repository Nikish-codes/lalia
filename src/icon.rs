//! The Lalia icon, drawn in code so the app icon, the tray states and the
//! overlay all share one geometry: a λ (from λαλιά) speaking two sound waves,
//! on a rounded tile. `packaging/lalia.svg` is the same drawing as a file.

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LinearGradient, Paint, PathBuilder, Pixmap, Point,
    Shader, SpreadMode, Stroke, Transform,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconState {
    /// Ready, waiting for the hotkey.
    Idle,
    /// Microphone open, words streaming.
    Listening,
    /// Connecting or finishing a session.
    Busy,
    /// Dictation switched off.
    Paused,
    /// Needs attention (no API key, last session failed).
    Attention,
}

pub const BRAND_A: (u8, u8, u8) = (0x7C, 0x4D, 0xFF); // violet
pub const BRAND_B: (u8, u8, u8) = (0x1F, 0xB6, 0xD9); // teal

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

fn palette(state: IconState) -> ((u8, u8, u8), (u8, u8, u8)) {
    match state {
        IconState::Idle | IconState::Attention => (BRAND_A, BRAND_B),
        IconState::Listening => ((0xFF, 0x3D, 0x71), (0xFF, 0x8A, 0x3D)),
        IconState::Busy => ((0xF5, 0x9E, 0x0B), (0xF9, 0x73, 0x16)),
        IconState::Paused => ((0x6B, 0x72, 0x80), (0x37, 0x41, 0x51)),
    }
}

fn rounded_rect(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
    // Cubic corners with k≈0.55 approximate a circle; a slightly larger k
    // gives the softer "squircle" look of modern app icons.
    let k = 0.62 * r;
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
}

/// Arc around (cx, cy) of radius r from angle a0 to a1 (radians, y down).
fn arc(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
    let segs = 4;
    let step = (a1 - a0) / segs as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    pb.move_to(cx + r * a0.cos(), cy + r * a0.sin());
    for i in 0..segs {
        let t0 = a0 + step * i as f32;
        let t1 = t0 + step;
        let (s0, c0) = t0.sin_cos();
        let (s1, c1) = t1.sin_cos();
        pb.cubic_to(
            cx + r * (c0 - k * s0),
            cy + r * (s0 + k * c0),
            cx + r * (c1 + k * s1),
            cy + r * (s1 - k * c1),
            cx + r * c1,
            cy + r * s1,
        );
    }
}

/// The λ glyph as two strokes, in unit coordinates.
fn lambda_path(s: f32) -> tiny_skia::Path {
    let p = |x: f32, y: f32| (x * s, y * s);
    let mut pb = PathBuilder::new();
    let (x, y) = p(0.205, 0.245);
    pb.move_to(x, y);
    let (cx, cy) = p(0.305, 0.200);
    let (x, y) = p(0.355, 0.305);
    pb.quad_to(cx, cy, x, y);
    let (x, y) = p(0.570, 0.775);
    pb.line_to(x, y);
    let (x, y) = p(0.447, 0.505);
    pb.move_to(x, y);
    let (x, y) = p(0.255, 0.775);
    pb.line_to(x, y);
    pb.finish().expect("lambda path")
}

fn white(alpha: f32) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba(1.0, 1.0, 1.0, alpha).unwrap_or(Color::WHITE));
    paint.anti_alias = true;
    paint
}

fn stroke(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        ..Stroke::default()
    }
}

/// Render the icon at `size`×`size` pixels.
pub fn render(size: u32, state: IconState) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("icon size");
    let s = size as f32;
    // Small sizes get a slightly larger tile and thicker strokes to stay crisp.
    let inset = if size <= 32 { 0.0 } else { 0.04 * s };
    let weight = if size <= 24 { 0.125 } else { 0.105 };

    let (a, b) = palette(state);
    let mut bg = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    bg.shader = LinearGradient::new(
        Point::from_xy(inset, inset),
        Point::from_xy(s - inset, s - inset),
        vec![
            GradientStop::new(0.0, rgb(a)),
            GradientStop::new(1.0, rgb(b)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(Shader::SolidColor(rgb(a)));
    let mut pb = PathBuilder::new();
    let side = s - 2.0 * inset;
    rounded_rect(&mut pb, inset, inset, side, side, side * 0.235);
    let tile = pb.finish().expect("tile path");
    pm.fill_path(&tile, &bg, FillRule::Winding, Transform::identity(), None);

    // Soft top highlight for depth (skipped at tray sizes where it muddies).
    if size > 32 {
        let mut gloss = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        gloss.shader = LinearGradient::new(
            Point::from_xy(0.0, inset),
            Point::from_xy(0.0, s * 0.55),
            vec![
                GradientStop::new(0.0, Color::from_rgba(1.0, 1.0, 1.0, 0.18).unwrap()),
                GradientStop::new(1.0, Color::from_rgba(1.0, 1.0, 1.0, 0.0).unwrap()),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .unwrap_or(Shader::SolidColor(Color::TRANSPARENT));
        pm.fill_path(
            &tile,
            &gloss,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    let lambda_alpha = if state == IconState::Paused {
        0.75
    } else {
        1.0
    };
    pm.stroke_path(
        &lambda_path(s),
        &white(lambda_alpha),
        &stroke(weight * s),
        Transform::identity(),
        None,
    );

    if state != IconState::Paused {
        let (cx, cy) = (0.575 * s, 0.47 * s);
        let span = 50f32.to_radians();
        for (r, alpha) in [(0.165, 0.95), (0.275, 0.6)] {
            let mut pb = PathBuilder::new();
            arc(&mut pb, cx, cy, r * s, -span, span);
            if let Some(path) = pb.finish() {
                pm.stroke_path(
                    &path,
                    &white(alpha),
                    &stroke(weight * 0.82 * s),
                    Transform::identity(),
                    None,
                );
            }
        }
    } else {
        // Pause bars where the waves would be.
        for x in [0.70, 0.80] {
            let mut pb = PathBuilder::new();
            pb.move_to(x * s, 0.36 * s);
            pb.line_to(x * s, 0.60 * s);
            if let Some(path) = pb.finish() {
                pm.stroke_path(
                    &path,
                    &white(0.95),
                    &stroke(weight * 0.8 * s),
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    if state == IconState::Attention {
        let r = 0.17 * s;
        let (cx, cy) = (s - r - 0.01 * s, s - r - 0.01 * s);
        if let Some(ring) = PathBuilder::from_circle(cx, cy, r) {
            pm.fill_path(
                &ring,
                &white(1.0),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if let Some(dot) = PathBuilder::from_circle(cx, cy, r * 0.72) {
            let mut p = Paint::default();
            p.set_color_rgba8(0xF5, 0x9E, 0x0B, 255);
            p.anti_alias = true;
            pm.fill_path(&dot, &p, FillRule::Winding, Transform::identity(), None);
        }
    }
    pm
}

/// Unpremultiplied RGBA bytes (for egui / PNG).
pub fn rgba(pm: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pm.data().len());
    for px in pm.pixels() {
        let c = px.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

/// ARGB32 in network byte order, as StatusNotifierItem wants.
pub fn argb(pm: &Pixmap) -> Vec<u8> {
    let mut out = rgba(pm);
    for px in out.as_chunks_mut::<4>().0 {
        px.rotate_right(1);
    }
    out
}

/// Write PNGs of every state (used by packaging and for previews).
pub fn export(dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    for size in [16u32, 22, 24, 32, 48, 64, 128, 256, 512] {
        render(size, IconState::Idle).save_png(dir.join(format!("lalia-{size}.png")))?;
    }
    for (name, st) in [
        ("idle", IconState::Idle),
        ("listening", IconState::Listening),
        ("busy", IconState::Busy),
        ("paused", IconState::Paused),
        ("attention", IconState::Attention),
    ] {
        for size in [22u32, 128] {
            render(size, st).save_png(dir.join(format!("state-{name}-{size}.png")))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_state_and_size() {
        for st in [
            IconState::Idle,
            IconState::Listening,
            IconState::Busy,
            IconState::Paused,
            IconState::Attention,
        ] {
            for size in [16, 22, 64, 256] {
                let pm = render(size, st);
                assert_eq!(pm.width(), size);
                // The centre of the tile is opaque.
                let c = pm.pixel(size / 2, size / 2).unwrap();
                assert_eq!(c.alpha(), 255);
            }
        }
    }

    #[test]
    fn argb_is_rotated_rgba() {
        let pm = render(16, IconState::Idle);
        let a = rgba(&pm);
        let b = argb(&pm);
        assert_eq!(b[0], a[3]);
        assert_eq!(b[1], a[0]);
    }
}
