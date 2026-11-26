//! Local control channel between Lalia's processes.
//!
//! The daemon endpoint doubles as the single-instance guard and lets the CLI,
//! the tray and the settings window talk to the running daemon with one-line
//! commands (`status`, `toggle`, `pause`, …). The GUI endpoint lets a second
//! "open settings" request raise the existing window instead of opening
//! another.
//!
//! On Linux these are Unix sockets in `$XDG_RUNTIME_DIR/lalia/` (a 0700
//! directory); on Windows they are named pipes private to the user.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Daemon,
    Gui,
    /// A private endpoint for this test process.
    #[cfg(test)]
    Test,
}

/// A claimed endpoint, ready to serve requests.
pub struct Server(sys::Listener);

/// Claim `endpoint` unless another live process already serves it.
/// Returns `Ok(None)` when it is taken.
pub fn claim(endpoint: Endpoint) -> anyhow::Result<Option<Server>> {
    Ok(sys::claim(endpoint)?.map(Server))
}

impl Server {
    /// Answer every request with `handler(command)` on background threads.
    pub fn serve(self, handler: impl Fn(String) -> String + Send + Sync + 'static) {
        sys::serve(self.0, std::sync::Arc::new(handler));
    }
}

/// Send one command and return the one-line reply.
pub fn request(endpoint: Endpoint, command: &str) -> anyhow::Result<String> {
    sys::request(endpoint, command.trim())
}

/// Ask the running daemon; `None` when it isn't running.
pub fn daemon(command: &str) -> Option<String> {
    request(Endpoint::Daemon, command).ok()
}

/// Give the endpoint up (on exit).
pub fn release(endpoint: Endpoint) {
    sys::release(endpoint);
}

type Handler = std::sync::Arc<dyn Fn(String) -> String + Send + Sync>;

#[cfg(unix)]
mod sys {
    use super::{Endpoint, Handler};
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    pub type Listener = UnixListener;

    fn runtime_dir() -> PathBuf {
        match std::env::var("XDG_RUNTIME_DIR") {
            Ok(dir) if !dir.is_empty() => PathBuf::from(dir).join("lalia"),
            _ => {
                let uid = fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
                PathBuf::from(format!("/tmp/lalia-{uid}"))
            }
        }
    }

    fn path(endpoint: Endpoint) -> PathBuf {
        runtime_dir().join(match endpoint {
            Endpoint::Daemon => "daemon.sock".to_string(),
            Endpoint::Gui => "gui.sock".to_string(),
            #[cfg(test)]
            Endpoint::Test => format!("test-{}.sock", std::process::id()),
        })
    }

    pub fn request(endpoint: Endpoint, command: &str) -> anyhow::Result<String> {
        let mut stream = UnixStream::connect(path(endpoint))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        stream.write_all(command.as_bytes())?;
        stream.write_all(b"\n")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(line.trim_end().to_string())
    }

    pub fn claim(endpoint: Endpoint) -> anyhow::Result<Option<Listener>> {
        let path = path(endpoint);
        if let Some(dir) = path.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
        if UnixStream::connect(&path).is_ok() {
            return Ok(None);
        }
        let _ = fs::remove_file(&path); // stale socket from a crash
        Ok(Some(UnixListener::bind(&path)?))
    }

    pub fn serve(listener: Listener, handler: Handler) {
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let handler = handler.clone();
                thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() {
                        return;
                    }
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        return;
                    }
                    let reply = handler(line);
                    let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
                });
            }
        });
    }

    pub fn release(endpoint: Endpoint) {
        let _ = fs::remove_file(path(endpoint));
    }
}

#[cfg(windows)]
mod sys {
    use super::{Endpoint, Handler};
    use crate::win::wide;
    use std::io::{BufRead, BufReader, Write};
    use std::thread;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
        HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW,
        PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    /// The single instance of the named pipe. Closing it frees the name.
    pub struct Listener(HANDLE);
    // The handle is only used from the serving thread.
    unsafe impl Send for Listener {}

    impl Drop for Listener {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    /// `\\.\pipe\lalia-<user>-daemon` — one per user, so several people on
    /// one machine each get their own Lalia.
    fn name(endpoint: Endpoint) -> String {
        let user: String = std::env::var("USERNAME")
            .unwrap_or_default()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect();
        let role = match endpoint {
            Endpoint::Daemon => "daemon".to_string(),
            Endpoint::Gui => "gui".to_string(),
            #[cfg(test)]
            Endpoint::Test => format!("test-{}", std::process::id()),
        };
        format!(r"\\.\pipe\lalia-{user}-{role}")
    }

    pub fn request(endpoint: Endpoint, command: &str) -> anyhow::Result<String> {
        let name = name(endpoint);
        let mut tries = 0;
        let mut pipe = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name)
            {
                Ok(f) => break f,
                // The server is answering someone else: wait for our turn.
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && tries < 20 => {
                    tries += 1;
                    unsafe {
                        WaitNamedPipeW(wide(&name).as_ptr(), 500);
                    }
                }
                Err(e) => return Err(e.into()),
            }
        };
        pipe.write_all(command.as_bytes())?;
        pipe.write_all(b"\n")?;
        pipe.flush()?;
        let mut line = String::new();
        BufReader::new(pipe).read_line(&mut line)?;
        Ok(line.trim_end().to_string())
    }

    pub fn claim(endpoint: Endpoint) -> anyhow::Result<Option<Listener>> {
        let handle = unsafe {
            CreateNamedPipeW(
                wide(&name(endpoint)).as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let code = unsafe { GetLastError() };
            // Somebody already owns the name: another instance is running.
            if code == ERROR_ACCESS_DENIED || code == ERROR_PIPE_BUSY {
                return Ok(None);
            }
            anyhow::bail!("cannot create the control pipe (error {code})");
        }
        Ok(Some(Listener(handle)))
    }

    fn read_line(pipe: HANDLE) -> Option<String> {
        let mut line = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            let mut got = 0u32;
            let ok = unsafe {
                ReadFile(
                    pipe,
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                    &mut got,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || got == 0 {
                break;
            }
            line.extend_from_slice(&buf[..got as usize]);
            if line.contains(&b'\n') || line.len() > 8192 {
                break;
            }
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.lines().next().unwrap_or("").trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    pub fn serve(listener: Listener, handler: Handler) {
        thread::spawn(move || {
            // Move the whole listener in (it is `Send`; its raw handle is not),
            // keeping the pipe open for as long as this thread serves it.
            let listener = listener;
            let pipe = listener.0;
            loop {
                let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0
                    || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                if !connected {
                    thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                if let Some(line) = read_line(pipe) {
                    let reply = format!("{}\n", handler(line));
                    let mut written = 0u32;
                    unsafe {
                        WriteFile(
                            pipe,
                            reply.as_ptr(),
                            reply.len() as u32,
                            &mut written,
                            std::ptr::null_mut(),
                        );
                        FlushFileBuffers(pipe);
                    }
                }
                unsafe {
                    DisconnectNamedPipe(pipe);
                }
            }
        });
    }

    pub fn release(_endpoint: Endpoint) {
        // The pipe name disappears with the process.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_serve_request_round_trip() {
        let server = claim(Endpoint::Test)
            .expect("claim works")
            .expect("the test endpoint is free");
        // A second claim sees the first one and backs off.
        assert!(claim(Endpoint::Test).expect("claim works").is_none());

        server.serve(|line| format!("echo {line}"));
        assert_eq!(request(Endpoint::Test, "hello").unwrap(), "echo hello");
        assert_eq!(
            request(Endpoint::Test, "  two words \n").unwrap(),
            "echo two words"
        );
        // Several clients one after another (the Windows pipe serves one at a time).
        let clients: Vec<_> = (0..4)
            .map(|i| std::thread::spawn(move || request(Endpoint::Test, &format!("n{i}")).unwrap()))
            .collect();
        for (i, c) in clients.into_iter().enumerate() {
            assert_eq!(c.join().unwrap(), format!("echo n{i}"));
        }
        release(Endpoint::Test);
    }
}
