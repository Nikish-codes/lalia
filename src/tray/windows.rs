//! The tray on Windows: a notification-area icon owned by a hidden window on
//! its own thread.
//!
//! Left-click opens the settings window, middle-click pauses, right-click
//! (or the keyboard's menu key) shows the menu built from `TrayState::menu`.
//! Notifications are shown as balloons from the same icon, which Windows 10
//! and 11 present as toasts.

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_NONE, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CheckMenuRadioItem, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    GetSystemMetrics, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SendMessageW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON, HMENU,
    MF_BYPOSITION, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON,
    TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE,
    WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WM_MBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

use super::{Item, TrayState};
use crate::app::{AppEvent, Command};
use crate::icon::{self, IconState};
use crate::win::wide;

/// The icon was clicked (lParam = the mouse message).
const WM_TRAY: u32 = WM_APP + 1;
/// The shared state changed; refresh icon and tooltip.
const WM_REFRESH: u32 = WM_APP + 2;
/// Show the pending balloon.
const WM_BALLOON: u32 = WM_APP + 3;
const ICON_ID: u32 = 1;

struct Shared {
    state: TrayState,
    balloon: Option<(String, String)>,
}

/// The tray window, for `balloon` (0 until the tray exists).
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static SHARED: Mutex<Option<Arc<Mutex<Shared>>>> = Mutex::new(None);

/// Per-thread context used by the window procedure.
struct Ctx {
    shared: Arc<Mutex<Shared>>,
    tx: UnboundedSender<AppEvent>,
    icon: HICON,
    shown: Option<IconState>,
    taskbar_created: u32,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub struct Handle {
    shared: Arc<Mutex<Shared>>,
    hwnd: isize,
}

fn copy_wide(dst: &mut [u16], text: &str) {
    let max = dst.len() - 1;
    let mut n = 0;
    for unit in text.encode_utf16().take(max) {
        dst[n] = unit;
        n += 1;
    }
    // Don't end on half of a surrogate pair.
    if n > 0 && (0xD800..0xDC00).contains(&dst[n - 1]) {
        n -= 1;
    }
    dst[n] = 0;
}

/// An HICON of the Lalia icon in `state`, at the notification-area size.
fn make_icon(state: IconState) -> HICON {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
    let pm = icon::render(size as u32, state);
    // Straight-alpha BGRA, top-down; the 1-bit mask is unused with alpha.
    let mut bgra = icon::rgba(&pm);
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let mask = vec![0u8; ((size as usize).div_ceil(16) * 2) * size as usize];
    unsafe {
        CreateIcon(
            GetModuleHandleW(null()),
            size,
            size,
            1,
            32,
            mask.as_ptr(),
            bgra.as_ptr(),
        )
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = ICON_ID;
    data
}

/// Add (or re-add) the icon and bring it up to date.
fn refresh(hwnd: HWND, ctx: &mut Ctx, add: bool) {
    let Ok(shared) = ctx.shared.lock() else {
        return;
    };
    let state = shared.state.icon_state();
    let tip = format!("Lalia — {}", shared.state.tooltip().replace('\n', " · "));
    drop(shared);

    // A new picture when the state changed; the old one is freed after the swap.
    let old = if ctx.shown != Some(state) || ctx.icon.is_null() {
        let old = ctx.icon;
        ctx.icon = make_icon(state);
        ctx.shown = Some(state);
        old
    } else {
        null_mut()
    };
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = ctx.icon;
    copy_wide(&mut data.szTip, &tip);
    unsafe {
        Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &data);
        if !old.is_null() {
            DestroyIcon(old);
        }
    }
}

/// Append `items` to `menu`, recording each clickable entry's command.
/// Menu ids are the command's index + 1 (0 means "nothing chosen").
unsafe fn fill_menu(menu: HMENU, items: &[Item], commands: &mut Vec<Command>) {
    // "&" marks a keyboard mnemonic in Win32 menus.
    let text = |s: &str| wide(&s.replace('&', "&&"));
    for item in items {
        match item {
            Item::Label(label) => {
                AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, text(label).as_ptr());
            }
            Item::Action {
                label,
                enabled,
                command,
                ..
            } => {
                commands.push(command.clone());
                let flags = if *enabled {
                    MF_STRING
                } else {
                    MF_STRING | MF_GRAYED
                };
                AppendMenuW(menu, flags, commands.len(), text(label).as_ptr());
            }
            Item::Check {
                label,
                checked,
                command,
            } => {
                commands.push(command.clone());
                let flags = if *checked {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                };
                AppendMenuW(menu, flags, commands.len(), text(label).as_ptr());
            }
            Item::Radio {
                label,
                selected,
                options,
            } => {
                let sub = CreatePopupMenu();
                for (option, command) in options {
                    commands.push(command.clone());
                    AppendMenuW(sub, MF_STRING, commands.len(), text(option).as_ptr());
                }
                if !options.is_empty() {
                    CheckMenuRadioItem(
                        sub,
                        0,
                        options.len() as u32 - 1,
                        (*selected).min(options.len() - 1) as u32,
                        MF_BYPOSITION,
                    );
                }
                AppendMenuW(menu, MF_POPUP, sub as usize, text(label).as_ptr());
            }
            Item::Submenu { label, items, .. } => {
                let sub = CreatePopupMenu();
                fill_menu(sub, items, commands);
                AppendMenuW(menu, MF_POPUP, sub as usize, text(label).as_ptr());
            }
            Item::Separator => {
                AppendMenuW(menu, MF_SEPARATOR, 0, null());
            }
        }
    }
}

/// Show the menu at the pointer and return what was chosen.
fn show_menu(hwnd: HWND, state: &TrayState) -> Option<Command> {
    let mut commands = Vec::new();
    unsafe {
        let menu = CreatePopupMenu();
        fill_menu(menu, &state.menu(), &mut commands);
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu); // also destroys the submenus
        (chosen > 0)
            .then(|| commands.get(chosen as usize - 1).cloned())
            .flatten()
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Decide what to do while holding the context, act after releasing it:
    // the menu runs its own message loop, which re-enters this function.
    enum Todo {
        Nothing,
        Send(Command),
        Menu(TrayState),
        Close,
        Default,
    }
    let todo = CTX.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(ctx) = guard.as_mut() else {
            return Todo::Default;
        };
        match msg {
            WM_TRAY => match lparam as u32 {
                WM_LBUTTONUP => Todo::Send(Command::Settings("home".into())),
                WM_MBUTTONUP => Todo::Send(Command::TogglePause),
                WM_RBUTTONUP | WM_CONTEXTMENU => match ctx.shared.lock() {
                    Ok(shared) => Todo::Menu(shared.state.clone()),
                    Err(_) => Todo::Nothing,
                },
                _ => Todo::Nothing,
            },
            WM_REFRESH => {
                refresh(hwnd, ctx, false);
                Todo::Nothing
            }
            WM_BALLOON => {
                let pending = ctx.shared.lock().ok().and_then(|mut s| s.balloon.take());
                if let Some((title, text)) = pending {
                    let mut data = icon_data(hwnd);
                    data.uFlags = NIF_INFO;
                    data.dwInfoFlags = NIIF_NONE;
                    copy_wide(&mut data.szInfoTitle, &title);
                    copy_wide(&mut data.szInfo, &text);
                    unsafe {
                        Shell_NotifyIconW(NIM_MODIFY, &data);
                    }
                }
                Todo::Nothing
            }
            WM_CLOSE => Todo::Close,
            WM_DESTROY => {
                unsafe {
                    PostQuitMessage(0);
                }
                Todo::Nothing
            }
            // Explorer restarted: our icon is gone, add it again.
            m if m == ctx.taskbar_created && m != 0 => {
                refresh(hwnd, ctx, true);
                Todo::Nothing
            }
            _ => Todo::Default,
        }
    });
    let send = |cmd: Command| {
        CTX.with(|cell| {
            if let Some(ctx) = cell.borrow().as_ref() {
                let _ = ctx.tx.send(AppEvent::Command(cmd, None));
            }
        });
    };
    match todo {
        Todo::Nothing => 0,
        Todo::Send(cmd) => {
            send(cmd);
            0
        }
        Todo::Menu(state) => {
            if let Some(cmd) = show_menu(hwnd, &state) {
                send(cmd);
            }
            0
        }
        Todo::Close => {
            // DestroyWindow sends WM_DESTROY straight back into this function.
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
                DestroyWindow(hwnd);
            }
            0
        }
        Todo::Default => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Put the icon in the notification area.
pub async fn spawn(state: TrayState, tx: UnboundedSender<AppEvent>) -> Option<Handle> {
    let shared = Arc::new(Mutex::new(Shared {
        state,
        balloon: None,
    }));
    let (ready_tx, ready_rx) = mpsc::sync_channel::<isize>(1);
    let thread_shared = shared.clone();
    thread::spawn(move || unsafe {
        let class = wide("LaliaTray");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = GetModuleHandleW(null());
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);
        // A hidden top-level window (message-only windows miss "TaskbarCreated").
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Lalia").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if hwnd.is_null() {
            let _ = ready_tx.send(0);
            return;
        }
        let mut ctx = Ctx {
            shared: thread_shared,
            tx,
            icon: null_mut(),
            shown: None,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
        };
        refresh(hwnd, &mut ctx, true);
        CTX.with(|cell| *cell.borrow_mut() = Some(ctx));
        let _ = ready_tx.send(hwnd as isize);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
    let hwnd =
        tokio::task::block_in_place(|| ready_rx.recv_timeout(Duration::from_secs(5))).unwrap_or(0);
    if hwnd == 0 {
        eprintln!("[lalia] tray icon unavailable");
        return None;
    }
    WINDOW.store(hwnd, Ordering::Relaxed);
    if let Ok(mut global) = SHARED.lock() {
        *global = Some(shared.clone());
    }
    Some(Handle { shared, hwnd })
}

/// Push a new state to the tray (no-op when unchanged).
pub fn update(handle: &Handle, state: TrayState) {
    if let Ok(mut shared) = handle.shared.lock() {
        if shared.state == state {
            return;
        }
        shared.state = state;
    }
    unsafe {
        PostMessageW(handle.hwnd as HWND, WM_REFRESH, 0, 0);
    }
}

/// Take the icon away (before the process exits, so no ghost icon stays).
pub fn remove(handle: &Handle) {
    WINDOW.store(0, Ordering::Relaxed);
    unsafe {
        SendMessageW(handle.hwnd as HWND, WM_CLOSE, 0, 0);
    }
}

/// Show a notification from the tray icon. Returns false without a tray.
pub fn balloon(title: &str, text: &str) -> bool {
    let hwnd = WINDOW.load(Ordering::Relaxed);
    if hwnd == 0 {
        return false;
    }
    let Some(shared) = SHARED.lock().ok().and_then(|s| s.clone()) else {
        return false;
    };
    if let Ok(mut shared) = shared.lock() {
        shared.balloon = Some((title.to_string(), text.to_string()));
    }
    unsafe { PostMessageW(hwnd as HWND, WM_BALLOON, 0, 0) != 0 }
}
