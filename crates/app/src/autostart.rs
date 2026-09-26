use anyhow::Result;
use std::fs;
use std::path::PathBuf;

fn autostart_file() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("autostart").join("whisper-flow.desktop"))
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    let Some(path) = autostart_file() else { return Ok(()) };
    if enabled {
        let exec = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "whisper-flow".into());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &path,
            format!(
                "[Desktop Entry]\nType=Application\nName=Whisper Flow\nExec={exec} run\nX-GNOME-Autostart-enabled=true\nComment=Voice typing with whisper.cpp\n"
            ),
        )?;
    } else if path.is_file() {
        let _ = fs::remove_file(&path);
    }
    Ok(())
}

pub fn is_enabled() -> bool {
    autostart_file().map(|p| p.is_file()).unwrap_or(false)
}
