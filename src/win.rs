//! Small helpers shared by the Windows code.

use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

/// A NUL-terminated UTF-16 copy of `s`, as Win32 "W" functions want.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Lalia is a GUI program (no console window pops up when it starts from the
/// Start menu). When it *is* started from a terminal, reconnect to that
/// terminal so `lalia status`, `lalia help` and the daemon's log are visible.
pub fn attach_parent_console() {
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Show a fatal error in a message box.
pub fn error_box(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(message).as_ptr(),
            wide("Lalia").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wide_is_nul_terminated() {
        assert_eq!(super::wide("hi"), vec![104, 105, 0]);
        assert_eq!(super::wide(""), vec![0]);
    }
}
