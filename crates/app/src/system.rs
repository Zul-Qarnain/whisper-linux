use std::fs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    X11,
    Wayland,
    Unknown,
}

pub fn session() -> Session {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        Session::Wayland
    } else if std::env::var_os("DISPLAY").is_some() {
        Session::X11
    } else {
        match std::env::var("XDG_SESSION_TYPE").as_deref() {
            Ok("x11") => Session::X11,
            Ok("wayland") => Session::Wayland,
            _ => Session::Unknown,
        }
    }
}

pub fn cpu_cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

pub fn recommended_threads() -> usize {
    let cores = cpu_cores();
    if cores >= 8 {
        4
    } else {
        cores.clamp(1, 4)
    }
}

pub fn available_memory_mb() -> Option<u64> {
    fs::read_to_string("/proc/meminfo").ok()?.lines().find_map(|line| {
        line.strip_prefix("MemAvailable:")?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
    }).map(|kb| kb / 1024)
}

/// (VmRSS kB, VmHWM peak-RSS kB) for this process.
pub fn self_memory() -> (u64, u64) {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    let mut rss = 0u64;
    let mut hwm = 0u64;
    for line in status.lines() {
        if let Some(v) = line.strip_prefix("VmRSS:") {
            rss = v.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("VmHWM:") {
            hwm = v.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
        }
    }
    (rss, hwm)
}

pub fn malloc_trim() {
    unsafe { libc::malloc_trim(0) };
}

pub fn command_exists(command: &str) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("command -v {command}")])
        .stdout(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
