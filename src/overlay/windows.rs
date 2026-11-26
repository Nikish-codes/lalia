//! The overlay window on Windows: a layered, click-through, never-activated
//! topmost popup. Frames are pushed with `UpdateLayeredWindow`, which gives
//! real per-pixel transparency on every supported Windows version.
//!
//! Everything here runs on the overlay thread, which also pumps the window's
//! (few) messages via `pump`.

use std::ptr::{null, null_mut};

use tiny_skia::Pixmap;
use windows_sys::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetMonitorInfoW,
    MonitorFromPoint, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, PeekMessageW,
    RegisterClassW, SetWindowPos, TranslateMessage, UpdateLayeredWindow, HWND_TOPMOST, MSG,
    PM_REMOVE, SWP_NOACTIVATE, SWP_SHOWWINDOW, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::config::OverlayPosition;
use crate::win::wide;

pub struct Backend {
    class: Vec<u16>,
}

pub struct Window {
    hwnd: HWND,
    w: i32,
    h: i32,
    /// Memory DC with a top-down 32-bit bitmap we draw each frame into.
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pos: std::cell::Cell<(i32, i32)>,
}

/// Work area (x, y, w, h) and DPI of the monitor under the mouse pointer.
fn pointer_monitor() -> ((i32, i32, i32, i32), u32) {
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let rect = if GetMonitorInfoW(monitor, &mut info) != 0 {
            info.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: 1280,
                bottom: 720,
            }
        };
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) != 0 {
            dpi_x = 96;
        }
        (
            (
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ),
            dpi_x,
        )
    }
}

impl Backend {
    pub fn connect() -> anyhow::Result<Self> {
        let class = wide("LaliaOverlay");
        unsafe {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.hInstance = GetModuleHandleW(null());
            wc.lpszClassName = class.as_ptr();
            // A second registration (never happens today) would just fail.
            RegisterClassW(&wc);
        }
        Ok(Self { class })
    }

    /// Layered windows are always per-pixel transparent.
    pub fn transparent(&self) -> bool {
        true
    }

    /// Scale of the monitor under the pointer (96 dpi = 1.0).
    pub fn scale(&self) -> f32 {
        (pointer_monitor().1 as f32 / 96.0).clamp(1.0, 3.0)
    }

    pub fn create(&self, w: u32, h: u32, _transparent: bool) -> anyhow::Result<Window> {
        let (w, h) = (w as i32, h as i32);
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW,
                self.class.as_ptr(),
                wide("Lalia").as_ptr(),
                WS_POPUP,
                0,
                0,
                w,
                h,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            if hwnd.is_null() {
                anyhow::bail!("cannot create the overlay window");
            }
            let dc = CreateCompatibleDC(null_mut());
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down rows, like our pixmap
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let mut bits: *mut core::ffi::c_void = null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if dc.is_null() || bitmap.is_null() || bits.is_null() {
                DestroyWindow(hwnd);
                anyhow::bail!("cannot create the overlay bitmap");
            }
            let old = SelectObject(dc, bitmap as HGDIOBJ);
            Ok(Window {
                hwnd,
                w,
                h,
                dc,
                bitmap,
                old,
                bits: bits as *mut u8,
                pos: std::cell::Cell::new((0, 0)),
            })
        }
    }

    /// Put the window on the pointer's monitor, `edge` px from the top or
    /// bottom of its work area (so it clears the taskbar), and show it.
    pub fn show(&self, win: &Window, pos: OverlayPosition, edge: i32) -> anyhow::Result<()> {
        let ((mx, my, mw, mh), _) = pointer_monitor();
        let x = mx + (mw - win.w) / 2;
        let y = match pos {
            // The work area already excludes the taskbar: sit a little closer.
            OverlayPosition::Bottom => my + mh - win.h - edge / 2,
            OverlayPosition::Top => my + edge / 2,
        };
        win.pos.set((x, y));
        unsafe {
            SetWindowPos(
                win.hwnd,
                HWND_TOPMOST,
                x,
                y,
                win.w,
                win.h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        Ok(())
    }

    /// Show a frame (premultiplied RGBA, the window's size).
    pub fn present(&self, win: &Window, pm: &Pixmap) -> anyhow::Result<()> {
        let len = (win.w * win.h * 4) as usize;
        if pm.data().len() != len {
            anyhow::bail!("overlay frame has the wrong size");
        }
        unsafe {
            // SAFETY: `bits` points to the DIB section's w*h*4 bytes, which
            // live until `destroy`. GDI wants premultiplied BGRA.
            let dst = std::slice::from_raw_parts_mut(win.bits, len);
            let src = pm.data().as_chunks::<4>().0;
            for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src) {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = s[3];
            }
            let (x, y) = win.pos.get();
            let dst_pos = POINT { x, y };
            let size = SIZE {
                cx: win.w,
                cy: win.h,
            };
            let src_pos = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let ok = UpdateLayeredWindow(
                win.hwnd,
                null_mut(),
                &dst_pos,
                &size,
                win.dc,
                &src_pos,
                0 as COLORREF,
                &blend,
                ULW_ALPHA,
            );
            if ok == 0 {
                anyhow::bail!("cannot update the overlay window");
            }
        }
        Ok(())
    }

    pub fn destroy(&self, win: Window) {
        unsafe {
            DestroyWindow(win.hwnd);
            SelectObject(win.dc, win.old);
            DeleteObject(win.bitmap as HGDIOBJ);
            DeleteDC(win.dc);
        }
    }

    /// Handle pending window messages for this thread.
    pub fn pump(&self) {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
