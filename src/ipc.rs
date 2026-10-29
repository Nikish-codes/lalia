//! Local control sockets in `$XDG_RUNTIME_DIR/lalia/` (a 0700 directory).
//!
//! `daemon.sock` doubles as the single-instance guard and lets the CLI, the
//! tray and the settings window talk to the running daemon with one-line
//! commands (`status`, `toggle`, `pause`, …). `gui.sock` lets a second
//! "open settings" request raise the existing window instead of opening another.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

pub fn runtime_dir() -> PathBuf {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir).join("lalia"),
        _ => {
            let uid = fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
            PathBuf::from(format!("/tmp/lalia-{uid}"))
        }
    }
}

pub fn daemon_socket() -> PathBuf {
    runtime_dir().join("daemon.sock")
}

pub fn gui_socket() -> PathBuf {
    runtime_dir().join("gui.sock")
}

/// Send one command and return the one-line reply.
pub fn request(path: &std::path::Path, command: &str) -> anyhow::Result<String> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(command.trim().as_bytes())?;
    stream.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(line.trim_end().to_string())
}

/// Ask the running daemon; `None` when it isn't running.
pub fn daemon(command: &str) -> Option<String> {
    request(&daemon_socket(), command).ok()
}

/// Bind `path` unless another live process already listens there.
/// Returns `Ok(None)` when it is taken.
pub fn claim(path: &std::path::Path) -> anyhow::Result<Option<UnixListener>> {
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    if UnixStream::connect(path).is_ok() {
        return Ok(None);
    }
    let _ = fs::remove_file(path); // stale socket from a crash
    Ok(Some(UnixListener::bind(path)?))
}

/// Read one command line from a connection.
pub fn read_command(stream: &UnixStream) -> Option<String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let line = line.trim().to_string();
    (!line.is_empty()).then_some(line)
}
