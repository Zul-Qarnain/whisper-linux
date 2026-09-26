use crate::events::{UiEvent, UiEvents};
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Control socket: on Wayland an app cannot grab global shortcuts, so the
/// compositor binds `whisper-flow toggle` to a key and it arrives here.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(dir).join(format!("whisper-flow-{}.sock", unsafe { libc::getuid() }))
}

pub fn send_command(cmd: &str) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path()).context("no whisper-flow daemon is running (start it with `whisper-flow run`)")?;
    writeln!(stream, "{cmd}")?;
    stream.flush()?;
    let mut buf = [0u8; 32];
    let n = std::io::Read::read(&mut stream, &mut buf).unwrap_or(0);
    let reply = String::from_utf8_lossy(&buf[..n]).trim().to_string();
    if !reply.is_empty() && reply != "ok" && reply != "running" {
        bail!("{reply}");
    }
    Ok(())
}

pub struct HotkeyServer {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HotkeyServer {
    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_file(socket_path());
    }
}

pub fn start_server(events: UiEvents) -> Result<HotkeyServer> {
    let path = socket_path();
    if path.exists() {
        // Refuse to start a second daemon: the live one owns the socket.
        if UnixStream::connect(&path).is_ok() {
            bail!("another whisper-flow instance is already running ({})", path.display());
        }
        let _ = std::fs::remove_file(&path); // stale socket from a dead run
    }
    let listener = UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
    listener.set_nonblocking(true).context("set nonblocking")?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let handle = std::thread::Builder::new()
        .name("wf-hotkey-socket".into())
        .spawn(move || {
            let mut buf = [0u8; 64];
            while !stop2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let n = match std::io::Read::read(&mut stream, &mut buf) {
                            Ok(n) => n,
                            Err(_) => continue,
                        };
                        let line = String::from_utf8_lossy(&buf[..n]).trim().trim_end_matches('\0').to_lowercase();
                        let reply = match line.as_str() {
                            "toggle" | "start" | "stop" => {
                                let _ = events.unbounded_send(UiEvent::Toggle);
                                "ok\n"
                            }
                            "settings" => {
                                let _ = events.unbounded_send(UiEvent::OpenSettings);
                                "ok\n"
                            }
                            "quit" => {
                                let _ = events.unbounded_send(UiEvent::QuitApp);
                                "ok\n"
                            }
                            "status" => "running\n",
                            "" => "",
                            other => {
                                let _ = write!(stream, "unknown command: {other}\n");
                                continue;
                            }
                        };
                        let _ = stream.write_all(reply.as_bytes());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(60));
                    }
                    Err(_) => break,
                }
            }
        })
        .context("spawn hotkey socket thread")?;

    Ok(HotkeyServer { stop, handle: Some(handle) })
}
