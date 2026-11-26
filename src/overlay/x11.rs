//! The overlay window on X11: an override-redirect window with an empty
//! input shape, so it never takes focus or clicks. With a compositor it has
//! real per-pixel transparency; without one it is cut to the pill's outline.

use tiny_skia::Pixmap;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{self, *};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::config::OverlayPosition;

pub struct Backend {
    conn: RustConnection,
    screen: Screen,
    screen_num: usize,
    msb_first: bool,
}

pub struct Window {
    win: xproto::Window,
    gc: Gcontext,
    depth: u8,
    w: u16,
    h: u16,
}

impl Backend {
    pub fn connect() -> anyhow::Result<Self> {
        let (conn, screen_num) = RustConnection::connect(None)?;
        let screen = conn.setup().roots[screen_num].clone();
        let msb_first = conn.setup().image_byte_order == ImageOrder::MSB_FIRST;
        let _ = conn.randr_query_version(1, 5).map(|c| c.reply());
        Ok(Self {
            conn,
            screen,
            screen_num,
            msb_first,
        })
    }

    fn atom(&self, name: &str) -> anyhow::Result<Atom> {
        Ok(self.conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
    }

    fn argb_visual(&self) -> Option<Visualid> {
        self.screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id)
    }

    /// Per-pixel transparency needs a compositor and a 32-bit visual.
    pub fn transparent(&self) -> bool {
        let composited = self
            .atom(&format!("_NET_WM_CM_S{}", self.screen_num))
            .ok()
            .and_then(|a| self.conn.get_selection_owner(a).ok()?.reply().ok())
            .map(|r| r.owner != x11rb::NONE)
            .unwrap_or(false);
        composited && self.argb_visual().is_some()
    }

    /// Desktop scale from `Xft.dpi` (96 dpi = 1.0).
    pub fn scale(&self) -> f32 {
        let dpi = self
            .conn
            .get_property(
                false,
                self.screen.root,
                AtomEnum::RESOURCE_MANAGER,
                AtomEnum::STRING,
                0,
                1 << 16,
            )
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| {
                let text = String::from_utf8_lossy(&r.value).to_string();
                text.lines()
                    .find_map(|l| l.strip_prefix("Xft.dpi:"))
                    .and_then(|v| v.trim().parse::<f32>().ok())
            })
            .unwrap_or(96.0);
        (dpi / 96.0).clamp(1.0, 3.0)
    }

    /// Geometry (x, y, w, h) of the monitor under the mouse pointer.
    fn pointer_monitor(&self) -> (i32, i32, i32, i32) {
        let screen = &self.screen;
        let full = (
            0,
            0,
            screen.width_in_pixels as i32,
            screen.height_in_pixels as i32,
        );
        let pointer = self
            .conn
            .query_pointer(screen.root)
            .ok()
            .and_then(|c| c.reply().ok());
        let monitors = self
            .conn
            .randr_get_monitors(screen.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.monitors)
            .unwrap_or_default();
        let rect = |m: &x11rb::protocol::randr::MonitorInfo| {
            (m.x as i32, m.y as i32, m.width as i32, m.height as i32)
        };
        if let Some(p) = pointer {
            let (px, py) = (p.root_x as i32, p.root_y as i32);
            if let Some(m) = monitors.iter().find(|m| {
                let (x, y, w, h) = rect(m);
                px >= x && px < x + w && py >= y && py < y + h
            }) {
                return rect(m);
            }
        }
        monitors
            .iter()
            .find(|m| m.primary)
            .or(monitors.first())
            .map(rect)
            .unwrap_or(full)
    }

    /// Create the (still hidden) window. Without transparency it is shaped
    /// to a capsule, which is what the pill style draws.
    pub fn create(&self, w: u32, h: u32, transparent: bool) -> anyhow::Result<Window> {
        let conn = &self.conn;
        let screen = &self.screen;
        let (w, h) = (w as u16, h as u16);
        let visual = if transparent {
            self.argb_visual()
        } else {
            None
        };

        let win = conn.generate_id()?;
        let (depth, visual_id, colormap) = match visual {
            Some(v) => {
                let cmap = conn.generate_id()?;
                conn.create_colormap(ColormapAlloc::NONE, cmap, screen.root, v)?;
                (32, v, cmap)
            }
            None => (
                screen.root_depth,
                screen.root_visual,
                screen.default_colormap,
            ),
        };
        let aux = CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(0)
            .border_pixel(0)
            .colormap(colormap);
        conn.create_window(
            depth,
            win,
            screen.root,
            0,
            0,
            w,
            h,
            0,
            WindowClass::INPUT_OUTPUT,
            visual_id,
            &aux,
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"Lalia",
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"lalia-overlay\0Lalia\0",
        )?;
        if let (Ok(kind), Ok(notification)) = (
            self.atom("_NET_WM_WINDOW_TYPE"),
            self.atom("_NET_WM_WINDOW_TYPE_NOTIFICATION"),
        ) {
            conn.change_property32(
                PropMode::REPLACE,
                win,
                kind,
                AtomEnum::ATOM,
                &[notification],
            )?;
        }

        // Click-through: an empty input region.
        if conn.xfixes_query_version(5, 0).is_ok() {
            let region = conn.generate_id()?;
            conn.xfixes_create_region(region, &[])?;
            conn.xfixes_set_window_shape_region(win, shape::SK::INPUT, 0, 0, region)?;
            conn.xfixes_destroy_region(region)?;
        }
        // Without a compositor, cut the window to the rounded pill.
        if visual.is_none() {
            let r = h as f32 / 2.0;
            let rects: Vec<Rectangle> = (0..h)
                .map(|row| {
                    let dy = (row as f32 + 0.5 - r).abs();
                    let inset = (r - (r * r - dy * dy).max(0.0).sqrt()).ceil() as i16;
                    Rectangle {
                        x: inset,
                        y: row as i16,
                        width: (w as i16 - 2 * inset).max(0) as u16,
                        height: 1,
                    }
                })
                .collect();
            conn.shape_rectangles(
                shape::SO::SET,
                shape::SK::BOUNDING,
                ClipOrdering::UNSORTED,
                win,
                0,
                0,
                &rects,
            )?;
        }

        let gc = conn.generate_id()?;
        conn.create_gc(gc, win, &CreateGCAux::new())?;
        Ok(Window {
            win,
            gc,
            depth,
            w,
            h,
        })
    }

    /// Put the window on the pointer's monitor, `edge` px from the top or
    /// bottom, and show it.
    pub fn show(&self, win: &Window, pos: OverlayPosition, edge: i32) -> anyhow::Result<()> {
        let (mx, my, mw, mh) = self.pointer_monitor();
        let x = mx + (mw - win.w as i32) / 2;
        let y = match pos {
            OverlayPosition::Bottom => my + mh - win.h as i32 - edge,
            OverlayPosition::Top => my + edge * 6 / 10,
        };
        self.conn.configure_window(
            win.win,
            &ConfigureWindowAux::new()
                .x(x)
                .y(y)
                .stack_mode(StackMode::ABOVE),
        )?;
        self.conn.map_window(win.win)?;
        Ok(())
    }

    /// Show a frame (premultiplied RGBA, the window's size).
    pub fn present(&self, win: &Window, pm: &Pixmap) -> anyhow::Result<()> {
        let mut data = Vec::with_capacity(pm.data().len());
        for px in pm.data().as_chunks::<4>().0 {
            let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
            if self.msb_first {
                data.extend_from_slice(&[a, r, g, b]);
            } else {
                data.extend_from_slice(&[b, g, r, a]);
            }
        }
        let row_bytes = win.w as usize * 4;
        let max = self
            .conn
            .maximum_request_bytes()
            .saturating_sub(64)
            .max(row_bytes);
        let rows_per = (max / row_bytes).max(1);
        let mut y = 0usize;
        while y < win.h as usize {
            let rows = rows_per.min(win.h as usize - y);
            self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                win.win,
                win.gc,
                win.w,
                rows as u16,
                0,
                y as i16,
                0,
                win.depth,
                &data[y * row_bytes..(y + rows) * row_bytes],
            )?;
            y += rows;
        }
        self.conn.flush()?;
        Ok(())
    }

    pub fn destroy(&self, win: Window) {
        let _ = self.conn.destroy_window(win.win);
        let _ = self.conn.flush();
    }

    /// X11 needs no event pumping for this window.
    pub fn pump(&self) {}
}
