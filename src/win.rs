//! Small helpers shared by the Windows code.

use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

/// A NUL-terminated UTF-16 copy of `s`, as Win32 "W" functions want.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Lalia is a GUI program (no console window pops up when it starts from the
/// Start menu). When it *is* started from a terminal, reconnect to that
/// terminal so `lalia status`, `lalia help` and the daemon's log are visible.
/// Otherwise send what it prints to a log file, so problems can be diagnosed.
pub fn attach_parent_console() {
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            return;
        }
        // No terminal. Keep handles we were given (a pipe, a file, or the
        // log inherited from the Lalia that started us); else open the log.
        let stderr = GetStdHandle(STD_ERROR_HANDLE);
        if !stderr.is_null() && stderr != INVALID_HANDLE_VALUE {
            return;
        }
        let path = log_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Start over once it gets big; it is only for recent troubleshooting.
        let big = std::fs::metadata(&path)
            .map(|m| m.len() > 512 * 1024)
            .unwrap_or(false);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(!big)
            .write(true)
            .truncate(big)
            .open(&path);
        if let Ok(file) = file {
            // The handle lives for the rest of the process.
            let handle = file.into_raw_handle();
            SetStdHandle(STD_OUTPUT_HANDLE, handle.cast());
            SetStdHandle(STD_ERROR_HANDLE, handle.cast());
        }
    }
}

/// Where Lalia's output goes when it has no terminal.
pub fn log_path() -> std::path::PathBuf {
    crate::config::data_dir().join("lalia.log")
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
