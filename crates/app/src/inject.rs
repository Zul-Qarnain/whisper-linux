use crate::events::{UiEvent, UiEvents};
use crate::system::{command_exists, session, Session};
use std::process::Command;
use std::time::Duration;

/// Clipboard-paste injection on a worker thread (never the UI thread):
/// save old text -> set ours -> synthesize Ctrl+V -> restore the user's clipboard.
/// If keystroke injection is unavailable, we leave the text on the clipboard
/// and tell the user to paste manually.
pub fn inject(text: String, events: UiEvents) {
    std::thread::Builder::new()
        .name("wf-inject".into())
        .spawn(move || {
            let outcome = do_inject(&text);
            let status = match outcome {
                Ok(()) => "pasted".to_string(),
                Err(reason) => format!("copied|{reason}"),
            };
            let _ = events.unbounded_send(UiEvent::Status(status));
        })
        .expect("spawn inject thread");
}

fn do_inject(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    let saved = clipboard.get_text().ok();
    clipboard.set_text(text.to_string()).map_err(|e| format!("clipboard write failed: {e}"))?;

    std::thread::sleep(Duration::from_millis(150)); // let the offer reach the compositor
    let pasted = match session() {
        Session::Wayland if command_exists("wtype") => send_keys(&["wtype", "-M", "ctrl", "v", "-m", "ctrl"]),
        Session::X11 if command_exists("xdotool") => send_keys(&["xdotool", "key", "ctrl+v"]),
        _ => false,
    };

    if pasted {
        // Give the target app time to read the clipboard before restoring.
        std::thread::sleep(Duration::from_millis(400));
        if let Some(old) = saved {
            let _ = clipboard.set_text(old);
        }
        Ok(())
    } else {
        let hint = match session() {
            Session::Wayland => "install wtype or press Ctrl+V",
            Session::X11 => "install xdotool or press Ctrl+V",
            Session::Unknown => "press Ctrl+V",
        };
        Err(hint.to_string())
    }
}

fn send_keys(argv: &[&str]) -> bool {
    let (prog, args) = argv.split_first().expect("argv non-empty");
    Command::new(prog)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
