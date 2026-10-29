//! The settings window (eframe/egui): first-run setup, settings, history.
//!
//! It runs as its own short-lived process (`lalia gui`) so the tray daemon
//! stays small. Settings are saved to `config.toml` as you change them and
//! the daemon picks them up within a second; live state comes from the
//! daemon's control socket.

use std::collections::HashMap;
use std::io::Write as _;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Layout,
    Margin, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextureHandle, Ui,
};
use egui_phosphor::regular as ph;

use crate::app::{DaemonStatus, Phase};
use crate::config::{self, Activation, Config, InsertMode, Mode, OverlayPosition, OverlayStyle};
use crate::icon::{self, IconState};
use crate::{audio, autostart, gemini, history, hotkey, ipc, overlay};

// ---------------------------------------------------------------------------
// Launching
// ---------------------------------------------------------------------------

/// Open the settings window on `page`, or raise it if it is already open.
pub fn open(page: &str) {
    if ipc::request(&ipc::gui_socket(), &format!("show {page}")).is_ok() {
        return;
    }
    // After a package upgrade our own executable may be gone; fall back to
    // whatever `lalia` is on the PATH.
    let candidates = std::env::current_exe()
        .ok()
        .filter(|p| p.exists())
        .into_iter()
        .chain(std::iter::once(std::path::PathBuf::from("lalia")));
    for exe in candidates {
        if spawn_detached(&exe, &["gui", page]) {
            return;
        }
    }
    eprintln!("[lalia] cannot open the settings window");
}

/// Start `exe args…` in its own process group and reap it when it exits.
fn spawn_detached(exe: &std::path::Path, args: &[&str]) -> bool {
    use std::os::unix::process::CommandExt;
    match std::process::Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .process_group(0)
        .spawn()
    {
        Ok(mut child) => {
            thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

/// Run the settings window (blocks until it is closed).
pub fn run(page: &str) -> anyhow::Result<()> {
    let start = Page::from_slug(page).unwrap_or(Page::Home);
    let Some(listener) = ipc::claim(&ipc::gui_socket())? else {
        ipc::request(&ipc::gui_socket(), &format!("show {}", start.slug()))?;
        return Ok(());
    };

    let logo = icon::render(256, IconState::Idle);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lalia")
            .with_app_id("lalia")
            .with_inner_size([940.0, 680.0])
            .with_min_inner_size([780.0, 540.0])
            .with_icon(Arc::new(egui::IconData {
                rgba: icon::rgba(&logo),
                width: 256,
                height: 256,
            })),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Lalia",
        options,
        Box::new(move |cc| Ok(Box::new(SettingsApp::new(cc, start, listener)))),
    );
    let _ = std::fs::remove_file(ipc::gui_socket());
    result.map_err(|e| anyhow::anyhow!("cannot open the settings window: {e}"))
}

// ---------------------------------------------------------------------------
// Look
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Pal {
    bg: Color32,
    side: Color32,
    card: Color32,
    control: Color32,
    control_hover: Color32,
    input: Color32,
    stroke: Color32,
    text: Color32,
    weak: Color32,
    accent: Color32,
    accent_soft: Color32,
    teal: Color32,
    ok: Color32,
    warn: Color32,
    bad: Color32,
}

fn pal(dark: bool) -> Pal {
    if dark {
        Pal {
            bg: Color32::from_rgb(0x0F, 0x11, 0x18),
            side: Color32::from_rgb(0x13, 0x16, 0x20),
            card: Color32::from_rgb(0x18, 0x1B, 0x26),
            control: Color32::from_rgb(0x23, 0x27, 0x35),
            control_hover: Color32::from_rgb(0x2C, 0x31, 0x42),
            input: Color32::from_rgb(0x11, 0x13, 0x1B),
            stroke: Color32::from_rgb(0x27, 0x2C, 0x3B),
            text: Color32::from_rgb(0xE8, 0xEA, 0xF2),
            weak: Color32::from_rgb(0x98, 0x9F, 0xB4),
            accent: Color32::from_rgb(0x8B, 0x6C, 0xFF),
            accent_soft: Color32::from_rgba_unmultiplied(0x8B, 0x6C, 0xFF, 0x26),
            teal: Color32::from_rgb(0x2B, 0xC4, 0xE4),
            ok: Color32::from_rgb(0x34, 0xD3, 0x99),
            warn: Color32::from_rgb(0xFB, 0xBF, 0x24),
            bad: Color32::from_rgb(0xF8, 0x71, 0x71),
        }
    } else {
        Pal {
            bg: Color32::from_rgb(0xF6, 0xF7, 0xFB),
            side: Color32::from_rgb(0xEC, 0xEE, 0xF6),
            card: Color32::WHITE,
            control: Color32::from_rgb(0xEE, 0xF0, 0xF6),
            control_hover: Color32::from_rgb(0xE3, 0xE6, 0xF0),
            input: Color32::from_rgb(0xF8, 0xF9, 0xFC),
            stroke: Color32::from_rgb(0xE1, 0xE4, 0xEE),
            text: Color32::from_rgb(0x1B, 0x1E, 0x29),
            weak: Color32::from_rgb(0x5F, 0x66, 0x7A),
            accent: Color32::from_rgb(0x6D, 0x4A, 0xFF),
            accent_soft: Color32::from_rgba_unmultiplied(0x6D, 0x4A, 0xFF, 0x1C),
            teal: Color32::from_rgb(0x0E, 0x9F, 0xC0),
            ok: Color32::from_rgb(0x10, 0xA3, 0x6E),
            warn: Color32::from_rgb(0xC2, 0x84, 0x0C),
            bad: Color32::from_rgb(0xDC, 0x3C, 0x3C),
        }
    }
}

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(egui::FontData::from_static(overlay::FONT_REGULAR)),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(egui::FontData::from_static(overlay::FONT_SEMIBOLD)),
    );
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "inter".into());
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    let mut semi: Vec<String> = vec!["inter-semibold".into()];
    semi.extend(
        fonts.families[&FontFamily::Proportional]
            .iter()
            .filter(|f| f.as_str() != "inter")
            .cloned(),
    );
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), semi);
    ctx.set_fonts(fonts);
}

fn install_style(ctx: &egui::Context) {
    for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
        let p = pal(dark);
        ctx.style_mut_of(theme, |style| {
            style.visuals = if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            };
            let v = &mut style.visuals;
            v.panel_fill = p.bg;
            v.window_fill = p.card;
            v.extreme_bg_color = p.input;
            v.faint_bg_color = p.control;
            v.hyperlink_color = p.accent;
            v.selection.bg_fill = p.accent.gamma_multiply(0.45);
            v.selection.stroke = Stroke::new(1.0, p.accent);
            v.window_corner_radius = CornerRadius::same(12);
            v.menu_corner_radius = CornerRadius::same(10);
            v.window_stroke = Stroke::new(1.0, p.stroke);
            let radius = CornerRadius::same(8);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.stroke);
            v.widgets.noninteractive.corner_radius = radius;
            for (w, fill) in [
                (&mut v.widgets.inactive, p.control),
                (&mut v.widgets.hovered, p.control_hover),
                (&mut v.widgets.active, p.control_hover),
                (&mut v.widgets.open, p.control_hover),
            ] {
                w.bg_fill = fill;
                w.weak_bg_fill = fill;
                w.corner_radius = radius;
                w.fg_stroke = Stroke::new(1.0, p.text);
                w.expansion = 0.0;
            }
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.stroke);
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.6));
            v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);

            style.spacing.item_spacing = vec2(10.0, 10.0);
            style.spacing.button_padding = vec2(14.0, 7.0);
            style.spacing.interact_size.y = 32.0;
            style.spacing.combo_width = 220.0;
            use egui::TextStyle;
            style.text_styles = [
                (TextStyle::Heading, semibold(24.0)),
                (TextStyle::Body, FontId::proportional(14.5)),
                (TextStyle::Button, FontId::proportional(14.5)),
                (TextStyle::Small, FontId::proportional(12.0)),
                (TextStyle::Monospace, FontId::monospace(13.0)),
            ]
            .into();
        });
    }
    ctx.set_theme(match std::env::var("LALIA_THEME").as_deref() {
        Ok("light") => egui::ThemePreference::Light,
        Ok("dark") => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    });
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

// ---------------------------------------------------------------------------
// Small widgets
// ---------------------------------------------------------------------------

fn card<R>(ui: &mut Ui, p: &Pal, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.stroke))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn card_title(ui: &mut Ui, p: &Pal, icon: &str, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).size(20.0).color(p.accent));
        ui.label(RichText::new(title).font(semibold(16.5)).color(p.text));
    });
    if !subtitle.is_empty() {
        ui.add(egui::Label::new(RichText::new(subtitle).size(13.0).color(p.weak)).wrap());
    }
    ui.add_space(6.0);
}

/// A settings row: title + description on the left, the control on the right.
fn row(ui: &mut Ui, p: &Pal, title: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let left = (ui.available_width() - 250.0).max(220.0);
        ui.allocate_ui_with_layout(vec2(left, 20.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(left);
            ui.label(RichText::new(title).font(semibold(14.5)).color(p.text));
            if !desc.is_empty() {
                ui.add(egui::Label::new(RichText::new(desc).size(12.5).color(p.weak)).wrap());
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
}

fn divider(ui: &mut Ui, p: &Pal) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, p.stroke);
    ui.add_space(4.0);
}

fn toggle(ui: &mut Ui, p: &Pal, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(42.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let bg = lerp_color(p.control_hover, p.accent, t);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(12), bg);
    let x = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), t);
    painter.circle_filled(pos2(x, rect.center().y), 9.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn primary(ui: &mut Ui, p: &Pal, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .font(semibold(14.0))
                .color(Color32::WHITE),
        )
        .fill(p.accent)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(10))
        .min_size(vec2(0.0, 36.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn secondary(ui: &mut Ui, p: &Pal, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(p.text))
            .fill(p.control)
            .stroke(Stroke::new(1.0, p.stroke))
            .corner_radius(CornerRadius::same(10))
            .min_size(vec2(0.0, 36.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn link(ui: &mut Ui, p: &Pal, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(p.accent)).frame(false))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn keycap(ui: &mut Ui, p: &Pal, text: &str) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), semibold(13.0), p.text);
    let size = vec2((galley.size().x + 18.0).max(32.0), 28.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect.translate(vec2(0.0, 2.0)),
        CornerRadius::same(7),
        p.stroke,
    );
    painter.rect_filled(rect, CornerRadius::same(7), p.control);
    painter.rect_stroke(
        rect,
        CornerRadius::same(7),
        Stroke::new(1.0, p.stroke),
        StrokeKind::Inside,
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, p.text);
}

fn choice(
    ui: &mut Ui,
    p: &Pal,
    selected: bool,
    icon: &str,
    title: &str,
    desc: &str,
) -> egui::Response {
    let resp = Frame::new()
        .fill(if selected { p.accent_soft } else { p.card })
        .stroke(Stroke::new(
            if selected { 1.5 } else { 1.0 },
            if selected { p.accent } else { p.stroke },
        ))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(22.0).color(if selected {
                    p.accent
                } else {
                    p.weak
                }));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(RichText::new(title).font(semibold(14.5)).color(p.text));
                    ui.add(egui::Label::new(RichText::new(desc).size(12.5).color(p.weak)).wrap());
                });
            });
        })
        .response
        .interact(Sense::click());
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    p: &Pal,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let mut changed = false;
    Frame::new()
        .fill(p.control)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Keep the options in reading order inside right-aligned rows too.
            let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
            ui.horizontal(|ui| {
                let ordered: Vec<&(T, &str)> = if rtl {
                    options.iter().rev().collect()
                } else {
                    options.iter().collect()
                };
                for (v, label) in ordered {
                    let sel = *value == *v;
                    let text = RichText::new(*label).size(13.5).color(if sel {
                        Color32::WHITE
                    } else {
                        p.text
                    });
                    let btn = egui::Button::new(text)
                        .fill(if sel { p.accent } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(8))
                        .min_size(vec2(0.0, 28.0));
                    if ui.add(btn).clicked() && !sel {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

fn level_meter(ui: &mut Ui, p: &Pal, level: f32) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width().min(420.0), 14.0), Sense::hover());
    let segs = 28;
    let gap = 3.0;
    let w = (rect.width() - gap * (segs - 1) as f32) / segs as f32;
    let lit = (level.clamp(0.0, 1.0) * segs as f32).round() as usize;
    for i in 0..segs {
        let x = rect.left() + i as f32 * (w + gap);
        let r = egui::Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
        let t = i as f32 / (segs - 1) as f32;
        let color = if i < lit {
            if t < 0.6 {
                lerp_color(p.teal, p.accent, t / 0.6)
            } else {
                lerp_color(
                    p.accent,
                    Color32::from_rgb(0xFF, 0x6B, 0x8B),
                    (t - 0.6) / 0.4,
                )
            }
        } else {
            p.control_hover
        };
        ui.painter().rect_filled(r, CornerRadius::same(3), color);
    }
}

fn step_badge(ui: &mut Ui, p: &Pal, n: usize, done: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
    let painter = ui.painter();
    if done {
        painter.circle_filled(rect.center(), 14.0, p.ok);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            ph::CHECK,
            FontId::proportional(15.0),
            Color32::WHITE,
        );
    } else {
        painter.circle_filled(rect.center(), 14.0, p.accent_soft);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            n.to_string(),
            semibold(13.5),
            p.accent,
        );
    }
}

fn pill_label(ui: &mut Ui, color: Color32, text: &str) {
    Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, color);
                ui.label(RichText::new(text).size(12.5).color(color));
            });
        });
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Setup,
    Dictation,
    Feedback,
    History,
    About,
}

const PAGES: [Page; 6] = [
    Page::Home,
    Page::Setup,
    Page::Dictation,
    Page::Feedback,
    Page::History,
    Page::About,
];

impl Page {
    fn slug(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Setup => "setup",
            Page::Dictation => "dictation",
            Page::Feedback => "feedback",
            Page::History => "history",
            Page::About => "about",
        }
    }

    fn from_slug(s: &str) -> Option<Self> {
        PAGES.iter().copied().find(|p| p.slug() == s.trim())
    }

    fn title(self) -> &'static str {
        match self {
            Page::Home => "Home",
            Page::Setup => "Setup",
            Page::Dictation => "Dictation",
            Page::Feedback => "Behaviour",
            Page::History => "History",
            Page::About => "About",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::Home => ph::HOUSE,
            Page::Setup => ph::SPARKLE,
            Page::Dictation => ph::MICROPHONE,
            Page::Feedback => ph::SLIDERS_HORIZONTAL,
            Page::History => ph::CLOCK_COUNTER_CLOCKWISE,
            Page::About => ph::INFO,
        }
    }
}

const LANGUAGES: &[(&str, &str)] = &[
    ("", "Detect automatically"),
    ("en-US", "English (US)"),
    ("en-GB", "English (UK)"),
    ("en-IN", "English (India)"),
    ("hi-IN", "Hindi"),
    ("bn-IN", "Bengali"),
    ("ta-IN", "Tamil"),
    ("te-IN", "Telugu"),
    ("mr-IN", "Marathi"),
    ("gu-IN", "Gujarati"),
    ("kn-IN", "Kannada"),
    ("ml-IN", "Malayalam"),
    ("ur-PK", "Urdu"),
    ("es-ES", "Spanish (Spain)"),
    ("es-MX", "Spanish (Mexico)"),
    ("fr-FR", "French"),
    ("de-DE", "German"),
    ("it-IT", "Italian"),
    ("pt-BR", "Portuguese (Brazil)"),
    ("pt-PT", "Portuguese (Portugal)"),
    ("nl-NL", "Dutch"),
    ("sv-SE", "Swedish"),
    ("pl-PL", "Polish"),
    ("uk-UA", "Ukrainian"),
    ("ru-RU", "Russian"),
    ("tr-TR", "Turkish"),
    ("el-GR", "Greek"),
    ("ar-SA", "Arabic"),
    ("he-IL", "Hebrew"),
    ("ja-JP", "Japanese"),
    ("ko-KR", "Korean"),
    ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"),
    ("id-ID", "Indonesian"),
    ("vi-VN", "Vietnamese"),
    ("th-TH", "Thai"),
];

#[derive(Debug, Clone, PartialEq)]
enum KeyCheck {
    Idle,
    Checking,
    Ok,
    Failed(String),
}

struct MicTest {
    capture: audio::Capture,
    until: Instant,
}

struct SettingsApp {
    page: Page,
    requested: Arc<Mutex<Option<Page>>>,
    daemon: Arc<Mutex<Option<DaemonStatus>>>,

    cfg: Config,
    saved: Config,
    load_error: Option<String>,
    changed_at: Option<Instant>,
    saved_flash: Option<Instant>,
    file_stamp: (Option<SystemTime>, Option<SystemTime>),
    stamp_checked: Instant,

    key_input: String,
    show_key: bool,
    key_check: Arc<Mutex<KeyCheck>>,
    has_key_file: bool,
    env_key: bool,

    devices: Vec<String>,
    mic: Option<MicTest>,
    mic_level: f32,
    mic_heard: bool,
    mic_error: Option<String>,

    vocab_input: String,
    autostart: bool,
    playground: String,

    history: Vec<history::Entry>,
    history_at: Option<Instant>,
    search: String,
    confirm_clear: bool,
    copied: Option<(u64, Instant)>,

    textures: HashMap<(IconState, u32), TextureHandle>,
}

impl SettingsApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        page: Page,
        listener: std::os::unix::net::UnixListener,
    ) -> Self {
        install_fonts(&cc.egui_ctx);
        install_style(&cc.egui_ctx);

        let requested: Arc<Mutex<Option<Page>>> = Arc::default();
        {
            let requested = requested.clone();
            let ctx = cc.egui_ctx.clone();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    if let Some(line) = ipc::read_command(&stream) {
                        let page = line
                            .strip_prefix("show")
                            .and_then(Page::from_slug)
                            .unwrap_or(Page::Home);
                        if let Ok(mut r) = requested.lock() {
                            *r = Some(page);
                        }
                        let _ = (&stream).write_all(b"ok\n");
                        ctx.request_repaint();
                    }
                }
            });
        }

        let daemon: Arc<Mutex<Option<DaemonStatus>>> = Arc::default();
        {
            let daemon = daemon.clone();
            let ctx = cc.egui_ctx.clone();
            thread::spawn(move || loop {
                let status: Option<DaemonStatus> =
                    ipc::daemon("status").and_then(|s| serde_json::from_str(&s).ok());
                let changed = daemon
                    .lock()
                    .map(|mut d| {
                        let changed = d.as_ref().map(|x| (x.phase, x.pid, x.error.clone()))
                            != status.as_ref().map(|x| (x.phase, x.pid, x.error.clone()));
                        *d = status;
                        changed
                    })
                    .unwrap_or(false);
                if changed {
                    ctx.request_repaint();
                }
                thread::sleep(Duration::from_millis(700));
            });
        }

        let (cfg, load_error) = match config::load_file() {
            Ok(f) => (Config::from_file(f), None),
            Err(e) => (
                Config::from_file(config::FileConfig::default()),
                Some(e.to_string()),
            ),
        };
        let has_key_file = config::read_key_file().is_some();
        let env_key = ["LALIA_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"]
            .iter()
            .any(|v| {
                std::env::var(v)
                    .map(|k| !k.trim().is_empty())
                    .unwrap_or(false)
            });

        Self {
            page,
            requested,
            daemon,
            saved: cfg.clone(),
            cfg,
            load_error,
            changed_at: None,
            saved_flash: None,
            file_stamp: config::stamp(),
            stamp_checked: Instant::now(),
            key_input: String::new(),
            show_key: false,
            key_check: Arc::new(Mutex::new(KeyCheck::Idle)),
            has_key_file,
            env_key,
            devices: audio::input_devices(),
            mic: None,
            mic_level: 0.0,
            mic_heard: false,
            mic_error: None,
            vocab_input: String::new(),
            autostart: autostart::is_enabled(),
            playground: String::new(),
            history: Vec::new(),
            history_at: None,
            search: String::new(),
            confirm_clear: false,
            copied: None,
            textures: HashMap::new(),
        }
    }

    fn icon_tex(&mut self, ctx: &egui::Context, state: IconState, size: u32) -> egui::TextureId {
        self.textures
            .entry((state, size))
            .or_insert_with(|| {
                let pm = icon::render(size, state);
                ctx.load_texture(
                    format!("icon-{state:?}-{size}"),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [size as usize, size as usize],
                        &icon::rgba(&pm),
                    ),
                    egui::TextureOptions::LINEAR,
                )
            })
            .id()
    }

    fn paint_icon(&mut self, ui: &mut Ui, state: IconState, size: f32) {
        let px = (size * ui.ctx().pixels_per_point()).round() as u32;
        let tex = self.icon_tex(&ui.ctx().clone(), state, px);
        let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
        ui.painter().image(
            tex,
            rect,
            egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    fn daemon_status(&self) -> Option<DaemonStatus> {
        self.daemon.lock().ok().and_then(|d| d.clone())
    }

    fn has_key(&self) -> bool {
        self.has_key_file || self.env_key
    }

    /// Per-frame housekeeping: raise requests, autosave, external edits, mic.
    fn tick(&mut self, ctx: &egui::Context) {
        if let Some(page) = self.requested.lock().ok().and_then(|mut r| r.take()) {
            self.page = page;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        // Autosave shortly after the last change.
        if self.cfg != self.saved {
            let since = *self.changed_at.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(350) {
                self.save();
            } else {
                ctx.request_repaint_after(Duration::from_millis(120));
            }
        }

        // Pick up edits made elsewhere (tray menu, text editor).
        if self.stamp_checked.elapsed() > Duration::from_secs(1) {
            self.stamp_checked = Instant::now();
            let stamp = config::stamp();
            if stamp != self.file_stamp {
                self.file_stamp = stamp;
                self.has_key_file = config::read_key_file().is_some();
                if self.cfg == self.saved {
                    if let Ok(f) = config::load_file() {
                        self.cfg = Config::from_file(f);
                        self.saved = self.cfg.clone();
                    }
                }
            }
            self.autostart = autostart::is_enabled();
        }

        if let Some(m) = &self.mic {
            let lvl = (m.capture.level.get() * 3.0).sqrt().min(1.0);
            self.mic_level = if lvl > self.mic_level {
                lvl
            } else {
                self.mic_level * 0.88
            };
            if lvl > 0.15 {
                self.mic_heard = true;
            }
            if Instant::now() >= m.until {
                self.mic = None;
                self.mic_level = 0.0;
            }
            ctx.request_repaint_after(Duration::from_millis(33));
        }
        if matches!(
            self.key_check.lock().map(|k| k.clone()),
            Ok(KeyCheck::Checking)
        ) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn save(&mut self) {
        self.changed_at = None;
        match config::save_file(&self.cfg.to_file()) {
            Ok(_) => {
                self.saved = self.cfg.clone();
                self.file_stamp = config::stamp();
                self.saved_flash = Some(Instant::now());
                self.load_error = None;
                thread::spawn(|| {
                    let _ = ipc::daemon("reload");
                });
            }
            Err(e) => self.load_error = Some(format!("could not save settings: {e}")),
        }
    }

    fn send(&self, cmd: &'static str) {
        let slot = self.daemon.clone();
        thread::spawn(move || {
            let _ = ipc::daemon(cmd);
            if let Some(s) = ipc::daemon("status").and_then(|s| serde_json::from_str(&s).ok()) {
                if let Ok(mut d) = slot.lock() {
                    *d = Some(s);
                }
            }
        });
    }

    fn start_daemon(&self) {
        let exe = std::env::current_exe().unwrap_or_else(|_| "lalia".into());
        if !spawn_detached(&exe, &[]) {
            eprintln!("[lalia] could not start Lalia");
        }
    }

    fn verify_key(&mut self, ctx: &egui::Context) {
        let key = self.key_input.trim().to_string();
        if key.is_empty() {
            return;
        }
        let mut cfg = self.cfg.clone();
        cfg.api_key = key.clone();
        let slot = self.key_check.clone();
        if let Ok(mut k) = slot.lock() {
            *k = KeyCheck::Checking;
        }
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(anyhow::Error::from)
                .and_then(|rt| rt.block_on(gemini::verify(&cfg)));
            let state = match result {
                Ok(()) => match config::set_api_key(&key) {
                    Ok(_) => {
                        let _ = ipc::daemon("reload");
                        KeyCheck::Ok
                    }
                    Err(e) => {
                        KeyCheck::Failed(format!("The key works but could not be saved: {e}"))
                    }
                },
                Err(e) => KeyCheck::Failed(e.to_string()),
            };
            if let Ok(mut k) = slot.lock() {
                *k = state;
            }
            ctx.request_repaint();
        });
    }

    fn start_mic_test(&mut self) {
        self.mic = None;
        self.mic_error = None;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        match audio::start(&self.cfg.input_device, tx, |e| eprintln!("[lalia] {e}")) {
            Ok(capture) => {
                self.mic = Some(MicTest {
                    capture,
                    until: Instant::now() + Duration::from_secs(12),
                })
            }
            Err(e) => self.mic_error = Some(e.to_string()),
        }
    }

    fn load_history(&mut self, force: bool) {
        let stale = self
            .history_at
            .map(|t| t.elapsed() > Duration::from_secs(3))
            .unwrap_or(true);
        if force || stale {
            self.history = history::load();
            self.history_at = Some(Instant::now());
        }
    }

    // --- sidebar -----------------------------------------------------------

    fn sidebar(&mut self, ui: &mut Ui, p: &Pal) {
        ui.horizontal(|ui| {
            self.paint_icon(ui, IconState::Idle, 38.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new("Lalia").font(semibold(19.0)).color(p.text));
                ui.label(RichText::new("λαλιά · speech").size(12.0).color(p.weak));
            });
        });
        ui.add_space(14.0);
        let status = self.daemon_status();
        let (color, text) = match status.as_ref().map(|s| s.phase) {
            None => (p.weak, "Not running"),
            Some(Phase::Idle) => (p.ok, "Ready"),
            Some(Phase::Connecting) => (p.warn, "Connecting…"),
            Some(Phase::Listening) => (Color32::from_rgb(0xFF, 0x5C, 0x7F), "Listening"),
            Some(Phase::Finishing) => (p.warn, "Finishing…"),
            Some(Phase::Paused) => (p.weak, "Paused"),
            Some(Phase::NeedsSetup) => (p.warn, "Setup needed"),
        };
        pill_label(ui, color, text);
        ui.add_space(16.0);

        let setup_done = self.has_key();
        for page in PAGES {
            let selected = self.page == page;
            let (rect, resp) =
                ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
            let hover = resp.hovered();
            if selected || hover {
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::same(10),
                    if selected { p.accent_soft } else { p.control },
                );
            }
            let fg = if selected { p.accent } else { p.text };
            ui.painter().text(
                pos2(rect.left() + 14.0, rect.center().y),
                Align2::LEFT_CENTER,
                page.icon(),
                FontId::proportional(18.0),
                fg,
            );
            ui.painter().text(
                pos2(rect.left() + 42.0, rect.center().y),
                Align2::LEFT_CENTER,
                page.title(),
                if selected {
                    semibold(14.5)
                } else {
                    FontId::proportional(14.5)
                },
                fg,
            );
            if page == Page::Setup && !setup_done {
                ui.painter()
                    .circle_filled(pos2(rect.right() - 16.0, rect.center().y), 4.0, p.warn);
            }
            if resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.page = page;
                if page == Page::History {
                    self.load_history(true);
                }
            }
            ui.add_space(-6.0);
        }

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.label(
                RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                    .size(11.5)
                    .color(p.weak),
            );
            if let Some(t) = self.saved_flash {
                if t.elapsed() < Duration::from_millis(1600) {
                    ui.label(
                        RichText::new(format!("{} Saved", ph::CHECK_CIRCLE))
                            .size(12.5)
                            .color(p.ok),
                    );
                    ui.ctx().request_repaint_after(Duration::from_millis(200));
                }
            }
        });
    }

    fn header(&self, ui: &mut Ui, p: &Pal, title: &str, subtitle: &str) {
        ui.label(RichText::new(title).font(semibold(26.0)).color(p.text));
        ui.add(egui::Label::new(RichText::new(subtitle).size(14.0).color(p.weak)).wrap());
        ui.add_space(14.0);
    }

    fn not_running_banner(&mut self, ui: &mut Ui, p: &Pal) {
        if self.daemon_status().is_some() {
            return;
        }
        Frame::new()
            .fill(p.warn.gamma_multiply(0.12))
            .stroke(Stroke::new(1.0, p.warn.gamma_multiply(0.5)))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(ph::WARNING_CIRCLE).size(20.0).color(p.warn));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(
                            RichText::new("Lalia isn't running")
                                .font(semibold(14.5))
                                .color(p.text),
                        );
                        ui.label(
                            RichText::new(
                                "Start it to put the icon in your tray and enable the hotkey.",
                            )
                            .size(12.5)
                            .color(p.weak),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary(ui, p, &format!("{}  Start Lalia", ph::PLAY)).clicked() {
                            self.start_daemon();
                        }
                    });
                });
            });
        ui.add_space(12.0);
    }

    // --- Home --------------------------------------------------------------

    fn page_home(&mut self, ui: &mut Ui, p: &Pal) {
        self.load_history(false);
        let status = self.daemon_status();
        let key = hotkey::label(&self.cfg.hotkey);
        let pause = hotkey::label(&self.cfg.pause_hotkey);
        let hold = self.cfg.activation == Activation::Hold;

        self.not_running_banner(ui, p);

        let (state, title, sub) = match status.as_ref().map(|s| s.phase) {
            _ if !self.has_key() => (
                IconState::Attention,
                "Let's get you set up".to_string(),
                "Add your Gemini API key and test your microphone — it takes a minute.".to_string(),
            ),
            None => (
                IconState::Paused,
                "Lalia is stopped".to_string(),
                "Start Lalia to dictate into any app.".to_string(),
            ),
            Some(Phase::Paused) => (
                IconState::Paused,
                "Dictation is paused".to_string(),
                format!("Tap {pause} or press Resume to turn it back on."),
            ),
            Some(Phase::Listening) => (
                IconState::Listening,
                "Listening…".to_string(),
                "Your words are being typed into the focused window.".to_string(),
            ),
            Some(Phase::Connecting | Phase::Finishing) => (
                IconState::Busy,
                "Working…".to_string(),
                "Connecting to Gemini or finishing the last words.".to_string(),
            ),
            Some(_) => (
                IconState::Idle,
                "Ready to dictate".to_string(),
                if hold {
                    format!("Hold {key} anywhere, speak, and let go. Your words appear wherever the cursor is.")
                } else {
                    format!("Tap {key} anywhere, speak, then tap {key} again. Your words appear wherever the cursor is.")
                },
            ),
        };

        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                self.paint_icon(ui, state, 72.0);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&title).font(semibold(22.0)).color(p.text));
                    ui.add(egui::Label::new(RichText::new(&sub).size(14.0).color(p.weak)).wrap());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if !self.has_key() {
                            if primary(ui, p, &format!("{}  Start setup", ph::SPARKLE)).clicked() {
                                self.page = Page::Setup;
                            }
                        } else if let Some(s) = &status {
                            if s.phase == Phase::Paused {
                                if primary(ui, p, &format!("{}  Resume", ph::PLAY)).clicked() {
                                    self.send("resume");
                                }
                            } else if secondary(ui, p, &format!("{}  Pause", ph::PAUSE)).clicked() {
                                self.send("pause");
                            }
                        }
                    });
                });
            });
        });
        ui.add_space(12.0);

        card(ui, p, |ui| {
            card_title(ui, p, ph::KEYBOARD, "Shortcuts", "");
            ui.horizontal_wrapped(|ui| {
                keycap(ui, p, &key);
                ui.label(
                    RichText::new(if hold {
                        "Hold to talk"
                    } else {
                        "Tap to start / stop"
                    })
                    .color(p.weak),
                );
                ui.add_space(18.0);
                keycap(ui, p, &pause);
                ui.label(RichText::new("Pause / resume").color(p.weak));
                ui.add_space(18.0);
                ui.label(RichText::new(ph::CURSOR_CLICK).size(17.0).color(p.weak));
                ui.label(RichText::new("Tray icon: quick settings").color(p.weak));
            });
        });
        ui.add_space(12.0);

        let stats = history::stats(&self.history, history::now());
        ui.columns(3, |cols| {
            for (col, (value, label, icon)) in cols.iter_mut().zip([
                (stats.words_today, "words today", ph::SUN),
                (stats.words_week, "words this week", ph::CALENDAR_BLANK),
                (stats.sessions, "dictations", ph::WAVEFORM),
            ]) {
                card(col, p, |ui| {
                    ui.label(RichText::new(icon).size(18.0).color(p.teal));
                    ui.label(
                        RichText::new(value.to_string())
                            .font(semibold(26.0))
                            .color(p.text),
                    );
                    ui.label(RichText::new(label).size(12.5).color(p.weak));
                });
            }
        });
        ui.add_space(12.0);

        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(ph::CLOCK_COUNTER_CLOCKWISE)
                        .size(20.0)
                        .color(p.accent),
                );
                ui.label(RichText::new("Recent").font(semibold(16.5)).color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !self.history.is_empty() && link(ui, p, "See all").clicked() {
                        self.page = Page::History;
                    }
                });
            });
            if self.history.is_empty() {
                ui.label(
                    RichText::new("Nothing yet — your dictations will show up here.")
                        .size(13.0)
                        .color(p.weak),
                );
            }
            let recent: Vec<history::Entry> = self.history.iter().take(3).cloned().collect();
            for e in &recent {
                divider(ui, p);
                self.history_row(ui, p, e, true);
            }
        });
    }

    fn history_row(&mut self, ui: &mut Ui, p: &Pal, e: &history::Entry, compact: bool) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} · {} word{} · {:.1}s",
                    history::ago(e.ts, history::now()),
                    e.words,
                    if e.words == 1 { "" } else { "s" },
                    e.ms as f32 / 1000.0
                ))
                .size(12.0)
                .color(p.weak),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let copied = self.copied.is_some_and(|(ts, at)| {
                    ts == e.ts && at.elapsed() < Duration::from_millis(1500)
                });
                let label = if copied {
                    format!("{} Copied", ph::CHECK)
                } else {
                    format!("{} Copy", ph::COPY)
                };
                if link(ui, p, &label).clicked() {
                    ui.ctx().copy_text(e.text.clone());
                    self.copied = Some((e.ts, Instant::now()));
                }
            });
        });
        let text = if compact && e.text.chars().count() > 220 {
            format!("{}…", e.text.chars().take(219).collect::<String>())
        } else {
            e.text.clone()
        };
        ui.add(
            egui::Label::new(RichText::new(text).color(p.text))
                .wrap()
                .selectable(true),
        );
    }

    // --- Setup -------------------------------------------------------------

    fn page_setup(&mut self, ui: &mut Ui, p: &Pal) {
        self.header(
            ui,
            p,
            "Set up Lalia",
            "Four quick steps. You can come back here any time.",
        );
        let ctx = ui.ctx().clone();
        let check = self
            .key_check
            .lock()
            .map(|k| k.clone())
            .unwrap_or(KeyCheck::Idle);

        // 1. API key
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                step_badge(ui, p, 1, self.has_key());
                ui.label(
                    RichText::new("Connect to Gemini")
                        .font(semibold(16.5))
                        .color(p.text),
                );
            });
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Lalia uses Google's Gemini live transcription. Paste a free API key — it's stored only on this computer (~/.config/lalia/key).",
                    )
                    .size(13.0)
                    .color(p.weak),
                )
                .wrap(),
            );
            ui.add_space(4.0);
            if self.has_key() && check != KeyCheck::Checking {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(ph::CHECK_CIRCLE).size(18.0).color(p.ok));
                    ui.label(
                        RichText::new(if self.has_key_file {
                            "A key is saved. Paste a new one below to replace it."
                        } else {
                            "Using the key from your environment (GEMINI_API_KEY)."
                        })
                        .color(p.text),
                    );
                });
            }
            ui.horizontal(|ui| {
                let field = egui::TextEdit::singleline(&mut self.key_input)
                    .hint_text("AIza… or AQ.Ab…")
                    .password(!self.show_key)
                    .margin(Margin::symmetric(10, 8))
                    .desired_width((ui.available_width() - 210.0).max(200.0));
                let resp = ui.add(field);
                let eye = if self.show_key {
                    ph::EYE_SLASH
                } else {
                    ph::EYE
                };
                if ui
                    .add(
                        egui::Button::new(RichText::new(eye).size(17.0)).min_size(vec2(36.0, 36.0)),
                    )
                    .on_hover_text(if self.show_key { "Hide" } else { "Show" })
                    .clicked()
                {
                    self.show_key = !self.show_key;
                }
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let busy = check == KeyCheck::Checking;
                let clicked = ui
                    .add_enabled_ui(!busy && !self.key_input.trim().is_empty(), |ui| {
                        primary(ui, p, &format!("{}  Verify & save", ph::SHIELD_CHECK))
                    })
                    .inner
                    .clicked();
                if (clicked || enter) && !busy {
                    self.verify_key(&ctx);
                }
            });
            match &check {
                KeyCheck::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Checking with Gemini…").color(p.weak));
                    });
                }
                KeyCheck::Ok => {
                    ui.label(
                        RichText::new(format!("{}  Key works and is saved.", ph::CHECK_CIRCLE))
                            .color(p.ok),
                    );
                    self.has_key_file = true;
                }
                KeyCheck::Failed(e) => {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{}  {e}", ph::X_CIRCLE)).color(p.bad),
                        )
                        .wrap(),
                    );
                    ui.horizontal(|ui| {
                        if link(ui, p, "Save it anyway").clicked() {
                            match config::set_api_key(&self.key_input) {
                                Ok(_) => {
                                    self.has_key_file = true;
                                    if let Ok(mut k) = self.key_check.lock() {
                                        *k = KeyCheck::Idle;
                                    }
                                    self.send("reload");
                                }
                                Err(e) => self.load_error = Some(e.to_string()),
                            }
                        }
                    });
                }
                KeyCheck::Idle => {}
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("No key yet?").size(13.0).color(p.weak));
                ui.hyperlink_to(
                    RichText::new(format!(
                        "Get one free at aistudio.google.com {}",
                        ph::ARROW_SQUARE_OUT
                    ))
                    .size(13.0),
                    "https://aistudio.google.com/apikey",
                );
            });
        });
        ui.add_space(12.0);

        // 2. Microphone
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                step_badge(ui, p, 2, self.mic_heard);
                ui.label(
                    RichText::new("Check your microphone")
                        .font(semibold(16.5))
                        .color(p.text),
                );
            });
            self.mic_controls(ui, p);
        });
        ui.add_space(12.0);

        // 3. Keys
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                step_badge(ui, p, 3, true);
                ui.label(
                    RichText::new("Choose your keys")
                        .font(semibold(16.5))
                        .color(p.text),
                );
            });
            self.key_controls(ui, p);
        });
        ui.add_space(12.0);

        // 4. Try it
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                step_badge(ui, p, 4, !self.playground.trim().is_empty());
                ui.label(RichText::new("Try it").font(semibold(16.5)).color(p.text));
            });
            let key = hotkey::label(&self.cfg.hotkey);
            let how = if self.cfg.activation == Activation::Hold {
                format!(
                    "Click in the box, then hold {key} and say something. Let go when you're done."
                )
            } else {
                format!("Click in the box, tap {key}, say something, then tap {key} again.")
            };
            ui.add(egui::Label::new(RichText::new(how).size(13.0).color(p.weak)).wrap());
            if self.daemon_status().is_none() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Lalia needs to be running for this.").color(p.warn));
                    if link(ui, p, "Start Lalia").clicked() {
                        self.start_daemon();
                    }
                });
            }
            ui.add(
                egui::TextEdit::multiline(&mut self.playground)
                    .hint_text("Your words will appear here…")
                    .desired_rows(4)
                    .margin(Margin::same(12))
                    .desired_width(f32::INFINITY),
            );
            if !self.playground.trim().is_empty() {
                ui.label(
                    RichText::new(format!(
                        "{}  It works! You can dictate into any app now.",
                        ph::CONFETTI
                    ))
                    .color(p.ok),
                );
            }
        });
    }

    fn mic_controls(&mut self, ui: &mut Ui, p: &Pal) {
        row(ui, p, "Microphone", "Which input Lalia listens to.", |ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new(ph::ARROWS_CLOCKWISE).size(16.0))
                        .min_size(vec2(32.0, 32.0)),
                )
                .on_hover_text("Refresh the list")
                .clicked()
            {
                self.devices = audio::input_devices();
            }
            let current = if self.cfg.input_device.is_empty() {
                "System default".to_string()
            } else {
                self.cfg.input_device.clone()
            };
            egui::ComboBox::from_id_salt("mic")
                .width(220.0)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.cfg.input_device,
                        String::new(),
                        "System default",
                    );
                    for d in self.devices.clone() {
                        ui.selectable_value(&mut self.cfg.input_device, d.clone(), d);
                    }
                });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if self.mic.is_some() {
                if secondary(ui, p, &format!("{}  Stop test", ph::STOP)).clicked() {
                    self.mic = None;
                    self.mic_level = 0.0;
                }
            } else if secondary(ui, p, &format!("{}  Test microphone", ph::MICROPHONE)).clicked() {
                self.start_mic_test();
            }
            level_meter(ui, p, self.mic_level);
        });
        if let Some(e) = &self.mic_error {
            ui.add(
                egui::Label::new(RichText::new(format!("{}  {e}", ph::X_CIRCLE)).color(p.bad))
                    .wrap(),
            );
        } else if self.mic.is_some() {
            ui.label(
                RichText::new(if self.mic_heard {
                    "Sounds good — the bar moves when you speak."
                } else {
                    "Say something — the bar should move."
                })
                .size(13.0)
                .color(if self.mic_heard { p.ok } else { p.weak }),
            );
        } else if self.mic_heard {
            ui.label(
                RichText::new(format!("{}  Microphone works.", ph::CHECK_CIRCLE))
                    .size(13.0)
                    .color(p.ok),
            );
        }
    }

    fn key_controls(&mut self, ui: &mut Ui, p: &Pal) {
        row(
            ui,
            p,
            "Dictate key",
            "Function keys and right-hand modifiers work best — they do nothing in most apps.",
            |ui| {
                key_combo(ui, "hotkey", &mut self.cfg.hotkey);
            },
        );
        divider(ui, p);
        row(ui, p, "Pause key", "Turns dictation off and on.", |ui| {
            key_combo(ui, "pause_hotkey", &mut self.cfg.pause_hotkey);
        });
        if hotkey::keysyms(&self.cfg.hotkey) == hotkey::keysyms(&self.cfg.pause_hotkey) {
            ui.label(
                RichText::new("Use two different keys for dictate and pause.")
                    .size(12.5)
                    .color(p.warn),
            );
        }
        divider(ui, p);
        row(ui, p, "How the key works", "", |ui| {
            segmented(
                ui,
                p,
                &mut self.cfg.activation,
                &[
                    (Activation::Hold, "Hold to talk"),
                    (Activation::Toggle, "Tap to start/stop"),
                ],
            );
        });
    }

    // --- Dictation -----------------------------------------------------------

    fn page_dictation(&mut self, ui: &mut Ui, p: &Pal) {
        self.header(
            ui,
            p,
            "Dictation",
            "How Lalia listens and how your words reach the screen.",
        );

        card(ui, p, |ui| {
            card_title(ui, p, ph::MAGIC_WAND, "Transcription style", "");
            ui.columns(2, |cols| {
                if choice(
                    &mut cols[0],
                    p,
                    self.cfg.mode == Mode::Verbatim,
                    ph::QUOTES,
                    "Verbatim",
                    "Exactly what you say, word for word.",
                )
                .clicked()
                {
                    self.cfg.mode = Mode::Verbatim;
                }
                if choice(
                    &mut cols[1],
                    p,
                    self.cfg.mode == Mode::Smart,
                    ph::SPARKLE,
                    "Smart",
                    "Adds punctuation and capitals, drops “um”s, formats lists.",
                )
                .clicked()
                {
                    self.cfg.mode = Mode::Smart;
                }
            });
            ui.add_space(6.0);
            card_title(ui, p, ph::TEXT_AA, "Typing", "");
            ui.columns(2, |cols| {
                if choice(
                    &mut cols[0],
                    p,
                    self.cfg.insert == InsertMode::Streaming,
                    ph::LIGHTNING,
                    "As you speak",
                    "Words appear a phrase at a time — feels instant.",
                )
                .clicked()
                {
                    self.cfg.insert = InsertMode::Streaming;
                }
                if choice(
                    &mut cols[1],
                    p,
                    self.cfg.insert == InsertMode::OnRelease,
                    ph::PACKAGE,
                    "When you finish",
                    "Everything is typed at once — best for chat apps that send on Enter.",
                )
                .clicked()
                {
                    self.cfg.insert = InsertMode::OnRelease;
                }
            });
        });
        ui.add_space(12.0);

        card(ui, p, |ui| {
            card_title(ui, p, ph::KEYBOARD, "Keys", "");
            self.key_controls(ui, p);
        });
        ui.add_space(12.0);

        card(ui, p, |ui| {
            card_title(ui, p, ph::TRANSLATE, "Language & words", "");
            row(
                ui,
                p,
                "Language",
                "A hint helps accuracy; auto-detect handles switching languages.",
                |ui| {
                    let current = self.cfg.language_codes.join(", ");
                    let label = LANGUAGES
                        .iter()
                        .find(|(c, _)| *c == current)
                        .map(|(_, n)| n.to_string())
                        .unwrap_or_else(|| format!("Custom: {current}"));
                    egui::ComboBox::from_id_salt("lang")
                        .width(220.0)
                        .selected_text(label)
                        .height(320.0)
                        .show_ui(ui, |ui| {
                            for (code, name) in LANGUAGES {
                                let sel = current == *code;
                                if ui.selectable_label(sel, *name).clicked() && !sel {
                                    self.cfg.language_codes = if code.is_empty() {
                                        Vec::new()
                                    } else {
                                        vec![code.to_string()]
                                    };
                                }
                            }
                        });
                },
            );
            divider(ui, p);
            ui.label(
                RichText::new("Custom vocabulary")
                    .font(semibold(14.5))
                    .color(p.text),
            );
            ui.add(
                egui::Label::new(
                    RichText::new("Names, products and jargon Lalia should spell right.")
                        .size(12.5)
                        .color(p.weak),
                )
                .wrap(),
            );
            let mut remove = None;
            ui.horizontal_wrapped(|ui| {
                for (i, word) in self.cfg.custom_vocabulary.iter().enumerate() {
                    Frame::new()
                        .fill(p.accent_soft)
                        .corner_radius(CornerRadius::same(16))
                        .inner_margin(Margin {
                            left: 12,
                            right: 4,
                            top: 3,
                            bottom: 3,
                        })
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(word).color(p.text));
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(ph::X).size(12.0).color(p.weak),
                                        )
                                        .frame(false),
                                    )
                                    .on_hover_text("Remove")
                                    .clicked()
                                {
                                    remove = Some(i);
                                }
                            });
                        });
                }
            });
            if let Some(i) = remove {
                self.cfg.custom_vocabulary.remove(i);
            }
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.vocab_input)
                        .hint_text("Add a word and press Enter (commas add several)")
                        .margin(Margin::symmetric(10, 8))
                        .desired_width((ui.available_width() - 90.0).max(160.0)),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (secondary(ui, p, "Add").clicked() || enter)
                    && !self.vocab_input.trim().is_empty()
                {
                    for w in self
                        .vocab_input
                        .split(',')
                        .map(str::trim)
                        .filter(|w| !w.is_empty())
                    {
                        if !self
                            .cfg
                            .custom_vocabulary
                            .iter()
                            .any(|x| x.eq_ignore_ascii_case(w))
                        {
                            self.cfg.custom_vocabulary.push(w.to_string());
                        }
                    }
                    self.vocab_input.clear();
                    resp.request_focus();
                }
            });
        });
        ui.add_space(12.0);

        card(ui, p, |ui| {
            card_title(ui, p, ph::MICROPHONE, "Microphone", "");
            self.mic_controls(ui, p);
        });
        ui.add_space(12.0);

        card(ui, p, |ui| {
            egui::CollapsingHeader::new(
                RichText::new("Advanced").font(semibold(15.0)).color(p.text),
            )
            .id_salt("advanced")
            .show(ui, |ui| {
                row(
                    ui,
                    p,
                    "Typing speed",
                    "Delay between characters. Raise it if an app drops letters.",
                    |ui| {
                        ui.add(
                            egui::Slider::new(&mut self.cfg.type_delay_ms, 0..=12).suffix(" ms"),
                        );
                    },
                );
                divider(ui, p);
                row(
                    ui,
                    p,
                    "Model",
                    "The Gemini Live model used for transcription.",
                    |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.cfg.model)
                                .margin(Margin::symmetric(10, 8))
                                .desired_width(240.0),
                        );
                    },
                );
                if self.cfg.model != config::DEFAULT_MODEL
                    && link(ui, p, "Reset to default model").clicked()
                {
                    self.cfg.model = config::DEFAULT_MODEL.into();
                }
            });
        });
    }

    // --- Behaviour ------------------------------------------------------------

    fn page_feedback(&mut self, ui: &mut Ui, p: &Pal) {
        self.header(
            ui,
            p,
            "Behaviour",
            "What Lalia shows you, and when it runs.",
        );
        card(ui, p, |ui| {
            card_title(ui, p, ph::BROWSER, "On-screen indicator", "");
            row(
                ui,
                p,
                "Show while dictating",
                "A small pill with a level meter and the words as they're heard. It never takes focus.",
                |ui| {
                    toggle(ui, p, &mut self.cfg.overlay);
                },
            );
            if self.cfg.overlay {
                divider(ui, p);
                row(ui, p, "Position", "", |ui| {
                    segmented(
                        ui,
                        p,
                        &mut self.cfg.overlay_position,
                        &[
                            (OverlayPosition::Top, "Top"),
                            (OverlayPosition::Bottom, "Bottom"),
                        ],
                    );
                });
                divider(ui, p);
                row(
                    ui,
                    p,
                    "Style",
                    "Lali is a little animated buddy that reacts to your voice; the pill is quieter. Lali needs desktop effects (a compositor), otherwise the pill is used.",
                    |ui| {
                        segmented(
                            ui,
                            p,
                            &mut self.cfg.overlay_style,
                            &[(OverlayStyle::Buddy, "Lali"), (OverlayStyle::Pill, "Pill")],
                        );
                    },
                );
            }
        });
        ui.add_space(12.0);
        card(ui, p, |ui| {
            card_title(ui, p, ph::BELL, "Sounds & notifications", "");
            row(
                ui,
                p,
                "Sounds",
                "A soft cue when dictation starts, stops or pauses.",
                |ui| {
                    toggle(ui, p, &mut self.cfg.sound);
                },
            );
            divider(ui, p);
            row(
                ui,
                p,
                "Notifications",
                "Desktop notifications for errors (and pause/resume when the indicator is off).",
                |ui| {
                    toggle(ui, p, &mut self.cfg.notify);
                },
            );
        });
        ui.add_space(12.0);
        card(ui, p, |ui| {
            card_title(ui, p, ph::POWER, "Startup & privacy", "");
            row(
                ui,
                p,
                "Start on login",
                "Lalia waits quietly in the tray.",
                |ui| {
                    let mut on = self.autostart;
                    if toggle(ui, p, &mut on).changed() {
                        match autostart::set_enabled(on) {
                            Ok(()) => self.autostart = on,
                            Err(e) => self.load_error = Some(e.to_string()),
                        }
                    }
                },
            );
            divider(ui, p);
            row(
                ui,
                p,
                "Start paused",
                "Begin with dictation off until you press the pause key.",
                |ui| {
                    let mut paused = !self.cfg.start_enabled;
                    if toggle(ui, p, &mut paused).changed() {
                        self.cfg.start_enabled = !paused;
                    }
                },
            );
            divider(ui, p);
            row(
                ui,
                p,
                "Keep history",
                "Save what you dictate on this computer so you can copy it again.",
                |ui| {
                    toggle(ui, p, &mut self.cfg.history);
                },
            );
        });
    }

    // --- History -------------------------------------------------------------

    fn page_history(&mut self, ui: &mut Ui, p: &Pal) {
        self.load_history(false);
        self.header(
            ui,
            p,
            "History",
            "Everything you've dictated, newest first. Stored only on this computer.",
        );
        if !self.cfg.history {
            ui.label(
                RichText::new(format!(
                    "{}  History is off — turn it on in Behaviour.",
                    ph::INFO
                ))
                .color(p.warn),
            );
            ui.add_space(6.0);
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(format!("{}  Search", ph::MAGNIFYING_GLASS))
                    .margin(Margin::symmetric(10, 8))
                    .desired_width((ui.available_width() - 170.0).max(200.0)),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.history.is_empty() {
                    return;
                }
                if self.confirm_clear {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Delete all").color(Color32::WHITE))
                                .fill(p.bad)
                                .corner_radius(CornerRadius::same(10))
                                .min_size(vec2(0.0, 36.0)),
                        )
                        .clicked()
                    {
                        if let Err(e) = history::clear() {
                            self.load_error = Some(e.to_string());
                        }
                        self.confirm_clear = false;
                        self.load_history(true);
                    }
                    if link(ui, p, "Cancel").clicked() {
                        self.confirm_clear = false;
                    }
                } else if secondary(ui, p, &format!("{}  Clear", ph::TRASH)).clicked() {
                    self.confirm_clear = true;
                }
            });
        });
        ui.add_space(6.0);

        let q = self.search.trim().to_lowercase();
        let entries: Vec<history::Entry> = self
            .history
            .iter()
            .filter(|e| q.is_empty() || e.text.to_lowercase().contains(&q))
            .take(200)
            .cloned()
            .collect();
        if entries.is_empty() {
            card(ui, p, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(18.0);
                    ui.label(RichText::new(ph::WAVEFORM).size(40.0).color(p.weak));
                    ui.label(
                        RichText::new(if q.is_empty() {
                            "Nothing here yet"
                        } else {
                            "No matches"
                        })
                        .font(semibold(16.0))
                        .color(p.text),
                    );
                    ui.label(
                        RichText::new(if q.is_empty() {
                            "Dictate something and it will show up here."
                        } else {
                            "Try a different word."
                        })
                        .color(p.weak),
                    );
                    ui.add_space(18.0);
                });
            });
        }
        for e in &entries {
            card(ui, p, |ui| self.history_row(ui, p, e, false));
            ui.add_space(8.0);
        }
    }

    // --- About ---------------------------------------------------------------

    fn page_about(&mut self, ui: &mut Ui, p: &Pal) {
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                self.paint_icon(ui, IconState::Idle, 88.0);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new("Lalia").font(semibold(28.0)).color(p.text));
                    ui.label(
                        RichText::new(format!(
                            "Version {} · system-wide live dictation",
                            env!("CARGO_PKG_VERSION")
                        ))
                        .color(p.weak),
                    );
                    ui.label(
                        RichText::new("From Greek λαλιά — “speech, talk”.")
                            .italics()
                            .color(p.weak),
                    );
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.hyperlink_to(
                    format!("{} GitHub", ph::GITHUB_LOGO),
                    "https://github.com/Nikish-codes/lalia",
                );
                ui.add_space(12.0);
                ui.hyperlink_to(
                    format!("{} Get an API key", ph::KEY),
                    "https://aistudio.google.com/apikey",
                );
                ui.add_space(12.0);
                ui.hyperlink_to(
                    format!("{} Report a problem", ph::BUG),
                    "https://github.com/Nikish-codes/lalia/issues",
                );
            });
        });
        ui.add_space(12.0);
        card(ui, p, |ui| {
            card_title(
                ui,
                p,
                ph::FOLDER,
                "Files",
                "Everything stays on this computer.",
            );
            for (label, path) in [
                ("Settings", config::config_path()),
                ("API key", config::key_path()),
                ("History", history::path()),
            ] {
                row(ui, p, label, &path.display().to_string(), |ui| {
                    if link(ui, p, &format!("{} Open folder", ph::FOLDER_OPEN)).clicked() {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                            let dir = dir.to_path_buf();
                            thread::spawn(move || {
                                let _ = std::process::Command::new("xdg-open").arg(dir).status();
                            });
                        }
                    }
                });
            }
        });
        ui.add_space(12.0);
        card(ui, p, |ui| {
            card_title(
                ui,
                p,
                ph::TERMINAL_WINDOW,
                "Command line",
                "Bind `lalia toggle` to any shortcut in your desktop's keyboard settings.",
            );
            for (cmd, what) in [
                ("lalia toggle", "start / stop dictation"),
                ("lalia pause · resume", "turn dictation off / on"),
                ("lalia selftest 8", "test mic + key, types nothing"),
                ("lalia status", "state as JSON"),
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(cmd).monospace().color(p.text));
                    ui.label(RichText::new(format!("— {what}")).color(p.weak));
                });
            }
        });
    }
}

fn key_combo(ui: &mut Ui, id: &str, value: &mut String) {
    egui::ComboBox::from_id_salt(id)
        .width(180.0)
        .selected_text(hotkey::label(value))
        .height(320.0)
        .show_ui(ui, |ui| {
            for (name, label) in hotkey::CHOICES {
                let sel = hotkey::keysyms(value) == hotkey::keysyms(name);
                if ui.selectable_label(sel, *label).clicked() && !sel {
                    *value = name.to_string();
                }
            }
        });
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.tick(&ctx);
        let p = pal(ui.visuals().dark_mode);

        egui::Panel::left("nav")
            .exact_size(232.0)
            .resizable(false)
            .frame(Frame::new().fill(p.side).inner_margin(Margin {
                left: 16,
                right: 16,
                top: 20,
                bottom: 14,
            }))
            .show(ui, |ui| self.sidebar(ui, &p));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg))
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        Frame::new()
                            .inner_margin(Margin {
                                left: 32,
                                right: 32,
                                top: 28,
                                bottom: 32,
                            })
                            .show(ui, |ui| {
                                let width = ui.available_width().min(760.0);
                                ui.set_max_width(width);
                                if let Some(e) = self.load_error.clone() {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!("{}  {e}", ph::WARNING))
                                                .color(p.bad),
                                        );
                                        if link(ui, &p, "Dismiss").clicked() {
                                            self.load_error = None;
                                        }
                                    });
                                    ui.add_space(8.0);
                                }
                                match self.page {
                                    Page::Home => self.page_home(ui, &p),
                                    Page::Setup => self.page_setup(ui, &p),
                                    Page::Dictation => self.page_dictation(ui, &p),
                                    Page::Feedback => self.page_feedback(ui, &p),
                                    Page::History => self.page_history(ui, &p),
                                    Page::About => self.page_about(ui, &p),
                                }
                            });
                    });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.cfg != self.saved {
            self.save();
        }
    }
}
