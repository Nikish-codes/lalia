//! "Start on login" on Windows: a `Lalia` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, which Windows runs
//! when the user signs in. No administrator rights needed.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

use crate::win::wide;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Lalia";

/// Where the setting lives (shown by `lalia autostart status`).
pub fn location() -> String {
    format!(r"HKCU\{RUN_KEY}\{VALUE}")
}

/// The command stored under `value` in the Run key, if any.
fn stored(value: &str) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            wide(value).as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

pub fn is_enabled() -> bool {
    stored(VALUE).is_some_and(|cmd| !cmd.trim().is_empty())
}

/// The command line that starts this Lalia.
fn command() -> anyhow::Result<String> {
    Ok(format!("\"{}\"", std::env::current_exe()?.display()))
}

/// Add or remove the `Run` value (Lalia will start at the next sign-in).
pub fn set_enabled(on: bool) -> anyhow::Result<()> {
    write(VALUE, if on { Some(command()?) } else { None })
}

/// Set `value` in the Run key to `command`, or delete it.
fn write(value: &str, command: Option<String>) -> anyhow::Result<()> {
    unsafe {
        let mut key: HKEY = null_mut();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        );
        if status != ERROR_SUCCESS {
            anyhow::bail!("cannot open the Run registry key (error {status})");
        }
        let status = if let Some(command) = command {
            let data = wide(&command);
            RegSetValueExW(
                key,
                wide(value).as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        } else {
            match RegDeleteValueW(key, wide(value).as_ptr()) {
                // Already absent is fine.
                windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                other => other,
            }
        };
        RegCloseKey(key);
        if status != ERROR_SUCCESS {
            anyhow::bail!("cannot change start-on-login (error {status})");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_value_round_trip() {
        // A value of our own, so the test never touches a real Lalia setting.
        let value = format!("LaliaTest{}", std::process::id());
        assert_eq!(stored(&value), None);
        write(&value, Some(r#""C:\Program Files\Lalia\lalia.exe""#.into())).unwrap();
        assert_eq!(
            stored(&value).as_deref(),
            Some(r#""C:\Program Files\Lalia\lalia.exe""#)
        );
        write(&value, None).unwrap();
        assert_eq!(stored(&value), None);
        // Deleting what is not there is fine.
        write(&value, None).unwrap();
        assert!(command().unwrap().starts_with('"'));
    }
}
