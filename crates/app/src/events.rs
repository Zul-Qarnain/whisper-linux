use crate::service::ServiceEvent;
use futures::channel::mpsc::UnboundedSender;
use std::time::Duration;

#[derive(Debug)]
pub enum UiEvent {
    Toggle,
    OpenSettings,
    QuitApp,
    Level(f32),
    Utterance { audio: Vec<f32>, partial: bool },
    Service(ServiceEvent),
    Status(String),
    Failed(String),
    CloseOverlay,
    MemoryTick { rss_kb: u64, hwm_kb: u64 },
    DownloadProgress { model: String, got: u64, total: u64 },
    DownloadDone { model: String },
    DownloadFailed { model: String, error: String },
}

pub type UiEvents = UnboundedSender<UiEvent>;

pub fn schedule(events: UiEvents, delay: Duration, ev: UiEvent) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let _ = events.unbounded_send(ev);
    });
}
