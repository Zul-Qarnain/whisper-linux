use crate::audio::{self, Recording};
use crate::events::{self, schedule, UiEvent, UiEvents};
use crate::hotkey::{self, HotkeyServer};
use crate::inject;
use crate::model_mgr;
use crate::service::InferenceService;
use crate::service::ServiceEvent;
use crate::settings::{Model, Settings};
use crate::system;
use crate::ui;
use crate::autostart;
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{AnyElement, ElementId, InteractiveElement};
use gpui::{
    div, px, rgb, size, App, AppContext, Application, AsyncApp, Bounds, ClickEvent, Context,
    Entity, FontWeight, Pixels, Point, Render, SharedString, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Recording,
    Flushing,
    Thinking,
    Injecting,
}

pub struct AppState {
    pub settings: Settings,
    pub events: UiEvents,
    pub service: Arc<InferenceService>,
    next_id: Arc<AtomicU64>,
    pub phase: Phase,
    pub status: SharedString,
    pub preview: SharedString,
    pub level: f32,
    pub memory: (u64, u64),
    recording: Option<Recording>,
    job_id: u64,
    pub overlay: Option<WindowHandle<AppState>>,
    pub settings_win: Option<WindowHandle<SettingsView>>,
    hotkey: Option<HotkeyServer>,
}

enum Action {
    None,
    OpenOverlay,
    CloseOverlay,
    CloseOverlaySoon(Duration),
    OpenSettings,
    Quit,
}

impl AppState {
    fn new(settings: Settings, events: UiEvents, service: Arc<InferenceService>, hotkey: HotkeyServer) -> Self {
        Self {
            settings,
            events,
            service,
            next_id: Arc::new(AtomicU64::new(1)),
            phase: Phase::Idle,
            status: "Ready".into(),
            preview: "".into(),
            level: 0.0,
            memory: (0, 0),
            recording: None,
            job_id: 0,
            overlay: None,
            settings_win: None,
            hotkey: Some(hotkey),
        }
    }

    fn on_event(&mut self, ev: UiEvent) -> Action {
        match ev {
            UiEvent::Toggle => match self.phase {
                Phase::Idle => {
                    self.level = 0.0;
                    self.status = "Listening…".into();
                    self.preview = "".into();
                    self.phase = Phase::Recording;
                    self.recording = Some(audio::start_recording(self.settings.clone(), self.events.clone()));
                    Action::OpenOverlay
                }
                Phase::Recording => {
                    self.phase = Phase::Flushing;
                    self.status = "Working…".into();
                    if let Some(rec) = self.recording.take() {
                        rec.stop(); // joins thread; utterance (if any) already queued
                    }
                    Action::None
                }
                _ => Action::None,
            },
            UiEvent::Level(rms) => {
                self.level = rms;
                Action::None
            }
            UiEvent::Utterance { audio, partial } => {
                self.recording.take(); // endpoint path: thread has exited
                if matches!(self.phase, Phase::Recording | Phase::Flushing) {
                    self.job_id = self.next_id.fetch_add(1, Ordering::Relaxed);
                    let lang = if self.settings.language == "auto" { None } else { Some(self.settings.language.clone()) };
                    if let Err(e) = self.service.transcribe(self.job_id, audio, lang) {
                        self.status = format!("service error: {e}").into();
                        self.phase = Phase::Idle;
                        return Action::CloseOverlaySoon(Duration::from_secs(2));
                    }
                    self.phase = Phase::Thinking;
                    self.status = if partial { "Thinking… (short clip)".into() } else { "Thinking…".into() };
                }
                Action::None
            }
            UiEvent::Service(sev) => match sev {
                ServiceEvent::Done { id, text } => {
                    if id != self.job_id {
                        return Action::None;
                    }
                    self.preview = text.clone().into();
                    if text.trim().is_empty() {
                        self.status = "Nothing heard".into();
                        self.phase = Phase::Idle;
                        return Action::CloseOverlaySoon(Duration::from_millis(900));
                    }
                    self.phase = Phase::Injecting;
                    self.status = "Inserting…".into();
                    inject::inject(text, self.events.clone());
                    Action::None
                }
                ServiceEvent::Failed { id, error } => {
                    if id != self.job_id {
                        return Action::None;
                    }
                    self.status = error.into();
                    self.phase = Phase::Idle;
                    Action::CloseOverlaySoon(Duration::from_secs(2))
                }
                ServiceEvent::Loaded { .. } | ServiceEvent::Unloaded => Action::None,
            },
            UiEvent::Status(s) => {
                if let Some(hint) = s.strip_prefix("copied|") {
                    self.status = format!("Copied — {hint}").into();
                    self.phase = Phase::Idle;
                    Action::CloseOverlaySoon(Duration::from_secs(6))
                } else {
                    self.status = "Inserted".into();
                    self.phase = Phase::Idle;
                    Action::CloseOverlaySoon(Duration::from_millis(800))
                }
            }
            UiEvent::Failed(msg) => {
                self.status = msg.into();
                self.phase = Phase::Idle;
                self.recording.take();
                Action::CloseOverlaySoon(Duration::from_millis(900))
            }
            UiEvent::CloseOverlay => {
                if self.phase == Phase::Idle {
                    Action::CloseOverlay
                } else {
                    Action::None
                }
            }
            UiEvent::OpenSettings => Action::OpenSettings,
            UiEvent::QuitApp => {
                if let Some(rec) = self.recording.take() {
                    rec.stop();
                }
                let _ = self.service.quit();
                if let Some(h) = self.hotkey.take() {
                    h.shutdown();
                }
                Action::Quit
            }
            UiEvent::MemoryTick { rss_kb, hwm_kb } => {
                self.memory = (rss_kb, hwm_kb);
                Action::None
            }
            UiEvent::DownloadProgress { .. } | UiEvent::DownloadDone { .. } | UiEvent::DownloadFailed { .. } => {
                Action::None // handled at App level (targets the settings view entity)
            }
        }
    }
}

pub fn run(settings: Settings) -> anyhow::Result<()> {
    if std::env::var("WHISPER_FLOW_X11").is_ok() {
        // Force gpui onto XWayland: notification-type windows there honor
        // keep-above stacking without stealing focus from the typing app.
        std::env::remove_var("WAYLAND_DISPLAY");
    }
    let (events_tx, mut events_rx) = futures::channel::mpsc::unbounded::<UiEvent>();
    let hotkey = hotkey::start_server(events_tx.clone())?;

    // Bridge thread: service crossbeam channel -> futures channel the gpui loop reads.
    let (svc_tx, svc_rx) = crossbeam_channel::unbounded::<ServiceEvent>();
    let bridge_tx = events_tx.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = svc_rx.recv() {
            if bridge_tx.unbounded_send(UiEvent::Service(ev)).is_err() {
                break;
            }
        }
    });

    let service = InferenceService::spawn(settings.model, settings.effective_threads(), svc_tx);

    Application::new().run(move |cx: &mut App| {
        let state: Entity<AppState> = cx.new(|_cx| AppState::new(settings.clone(), events_tx.clone(), service.clone(), hotkey));

        {
            let tx = events_tx.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(3));
                let (rss, hwm) = system::self_memory();
                if tx.unbounded_send(UiEvent::MemoryTick { rss_kb: rss, hwm_kb: hwm }).is_err() {
                    break;
                }
            });
        }

        let pump_state = state.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(ev) = events_rx.next().await {
                let st = pump_state.clone();
                let download_action = match &ev {
                    UiEvent::DownloadProgress { model, got, total } => Some((model.clone(), Some((*got, *total)), None)),
                    UiEvent::DownloadDone { model } => Some((model.clone(), None, Some(Ok(())))),
                    UiEvent::DownloadFailed { model, error } => Some((model.clone(), None, Some(Err(error.clone())))),
                    _ => None,
                };
                let _ = cx.update(move |app| {
                    let action = st.update(app, |s, _cx| s.on_event(ev));
                    finish(app, &st, action);
                    if let Some((model, prog, done)) = download_action {
                        if let Some(w) = st.read(app).settings_win.clone() {
                            let _ = w.update(app, |v, _window, cx| match (prog, done) {
                                (Some((got, total)), _) => v.download_progress(&model, got, total, cx),
                                (_, Some(Ok(()))) => v.download_done(&model, cx),
                                (_, Some(Err(e))) => v.download_failed(&model, e, cx),
                                _ => v.download_done(&model, cx),
                            });
                        }
                    }
                });
            }
        })
        .detach();

        // Startup toast: confirm the daemon is live and point at the shortcut.
        state.update(cx, |s, _cx| {
            s.status = "Ready — bind a shortcut to `whisper-flow toggle`".into();
        });
        ensure_overlay(cx, &state);
        let tx = state.read(cx).events.clone();
        schedule(tx, Duration::from_secs(5), UiEvent::CloseOverlay);
    });
    Ok(())
}

fn finish(app: &mut App, state: &Entity<AppState>, action: Action) {
    match action {
        Action::None => {}
        Action::OpenOverlay => ensure_overlay(app, state),
        Action::CloseOverlay => close_overlay(app, state),
        Action::CloseOverlaySoon(d) => {
            let tx = state.read(app).events.clone();
            events::schedule(tx, d, UiEvent::CloseOverlay);
        }
        Action::OpenSettings => open_settings(app, state),
        Action::Quit => {
            close_overlay(app, state);
            app.quit();
        }
    }
    redraw(app, state);
}

fn redraw(app: &mut App, state: &Entity<AppState>) {
    if let Some(w) = state.read(app).overlay.clone() {
        let _ = w.update(app, |_, _window, cx| cx.notify());
    }
    if let Some(w) = state.read(app).settings_win.clone() {
        let _ = w.update(app, |_, _window, cx| cx.notify());
    }
}

fn ensure_overlay(app: &mut App, state: &Entity<AppState>) {
    if state.read(app).overlay.is_some() {
        return;
    }
    let bounds = overlay_bounds(app);
    let st = state.clone();
    if let Ok(handle) = app.open_window(overlay_options(bounds), move |_window, _app| st.clone()) {
        state.update(app, |s, _cx| s.overlay = Some(handle));
    }
}

fn close_overlay(app: &mut App, state: &Entity<AppState>) {
    let handle = state.update(app, |s, _cx| s.overlay.take());
    if let Some(h) = handle {
        let _ = h.update(app, |_, window, _cx| window.remove_window());
    }
}

fn overlay_bounds(app: &App) -> Bounds<Pixels> {
    let (w, h) = app
        .primary_display()
        .map(|d| (d.bounds().size.width, d.bounds().size.height))
        .unwrap_or_else(|| (px(1920.), px(1080.)));
    let pill_w = px(300.);
    Bounds::new(Point::new(w / 2.0 - pill_w / 2.0, h - px(130.)), size(pill_w, px(74.)))
}

fn overlay_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::new("Whisper Flow")),
            appears_transparent: true,
            ..Default::default()
        }),
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(gpui::WindowDecorations::Client),
        kind: WindowKind::PopUp,
        focus: false,
        show: true,
        is_resizable: false,
        is_movable: false,
        is_minimizable: false,
        app_id: Some("whisper-flow".into()),
        ..Default::default()
    }
}

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let dot = match self.phase {
            Phase::Recording => ui::REC,
            Phase::Flushing | Phase::Thinking => ui::WARN,
            Phase::Injecting => ui::ACCENT,
            Phase::Idle => ui::OK,
        };
        let preview: SharedString = if self.preview.is_empty() {
            "".into()
        } else {
            self.preview.chars().take(72).collect::<String>().into()
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2()
            .w_full()
            .h_full()
            .rounded(px(18.))
            .bg(gpui::rgb(ui::SURFACE))
            .border_1()
            .border_color(gpui::rgb(ui::BORDER))
            .text_color(gpui::rgb(ui::TEXT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_size(px(9.)).text_color(gpui::rgb(dot)).child("●"))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight(600.0))
                            .child(self.status.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(ui::level_meter(self.level, 0.12))
                    .when(!preview.is_empty(), |d| {
                        d.child(div().text_size(px(11.)).text_color(gpui::rgb(ui::MUTED)).child(preview))
                    }),
            )
    }
}

// ---------------------------------------------------------------- settings

struct ModelRowUi {
    progress: Option<f32>,
    downloaded: bool,
}

impl Clone for ModelRowUi {
    fn clone(&self) -> Self {
        Self { progress: self.progress, downloaded: self.downloaded }
    }
}

pub struct SettingsView {
    state: Entity<AppState>,
    downloads: HashMap<String, ModelRowUi>,
    message: SharedString,
}

impl SettingsView {
    fn new(state: Entity<AppState>) -> Self {
        let mut downloads = HashMap::new();
        for info in model_mgr::catalog() {
            downloads.insert(
                info.model.id().to_string(),
                ModelRowUi { progress: None, downloaded: model_mgr::is_downloaded(info.model) },
            );
        }
        Self { state, downloads, message: "".into() }
    }

    fn download_progress(&mut self, model: &str, got: u64, total: u64, cx: &mut Context<Self>) {
        if let Some(row) = self.downloads.get_mut(model) {
            row.progress = Some(got as f32 / total.max(1) as f32);
        }
        cx.notify();
    }

    fn download_done(&mut self, model: &str, cx: &mut Context<Self>) {
        if let Some(row) = self.downloads.get_mut(model) {
            row.progress = None;
            row.downloaded = true;
        }
        self.message = format!("{model} ready — pick \"use\" to switch").into();
        cx.notify();
    }

    fn download_failed(&mut self, model: &str, error: String, cx: &mut Context<Self>) {
        if let Some(row) = self.downloads.get_mut(model) {
            row.progress = None;
        }
        self.message = format!("{model}: {error}").into();
        cx.notify();
    }

    fn set_model(&self, cx: &mut Context<Self>, info_model: Model) {
        let state = self.state.clone();
        let (threads, cur) = {
            let s = state.read(cx);
            (s.settings.effective_threads(), s.settings.model)
        };
        if cur == info_model {
            return;
        }
        state.update(cx, |s, _cx| {
            s.settings.model = info_model;
            let _ = s.settings.save();
            let _ = s.service.set_model(info_model, threads);
        });
    }

    fn cycle_language(&self, cx: &mut Context<Self>, lang: &str) {
        let state = self.state.clone();
        let v = lang.to_string();
        state.update(cx, |s, _cx| {
            s.settings.language = v;
            let _ = s.settings.save();
        });
    }

    fn model_by_id(id: &str) -> Option<Model> {
        match id {
            "tiny.en-q5" => Some(Model::TinyEnQ5),
            "base.en-q5" => Some(Model::BaseEnQ5),
            "small.en-q5" => Some(Model::SmallEnQ5),
            _ => None,
        }
    }

    fn start_download(&mut self, cx: &mut Context<Self>, id: String) {
        let Some(m) = Self::model_by_id(&id) else { return };
        if let Some(row) = self.downloads.get_mut(&id) {
            row.progress = Some(0.0);
        }
        let ev = self.state.read(cx).events.clone();
        let label = id.clone();
        std::thread::spawn(move || {
            let evp = ev.clone();
            match model_mgr::download(m, |got, total| {
                let _ = evp.unbounded_send(UiEvent::DownloadProgress { model: label.clone(), got, total });
            }) {
                Ok(_) => {
                    let _ = ev.unbounded_send(UiEvent::DownloadDone { model: label });
                }
                Err(e) => {
                    let _ = ev.unbounded_send(UiEvent::DownloadFailed { model: label, error: e.to_string() });
                }
            }
        });
    }

    fn remove_model(&mut self, cx: &mut Context<Self>, id: String) {
        let Some(m) = Self::model_by_id(&id) else { return };
        let _ = model_mgr::remove(m);
        let state = self.state.clone();
        state.update(cx, |s, _cx| {
            if s.settings.model == m {
                let _ = s.service.unload();
            }
        });
        if let Some(row) = self.downloads.get_mut(&id) {
            row.progress = None;
            row.downloaded = false;
        }
        self.message = format!("{id} removed").into();
    }
}

fn open_settings(app: &mut App, state: &Entity<AppState>) {
    if state.read(app).settings_win.is_some() {
        return;
    }
    let st = state.clone();
    let bounds = Bounds::centered(None, size(px(580.), px(470.)), app);
    let built = app.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Whisper Flow".into()), ..Default::default() }),
            app_id: Some("whisper-flow-settings".into()),
            ..Default::default()
        },
        move |_, app2| app2.new(|_cx| SettingsView::new(st.clone())),
    );
    match built {
        Ok(handle) => state.update(app, |s, _cx| s.settings_win = Some(handle)),
        Err(e) => state.update(app, |s, _cx| s.status = format!("settings window failed: {e}").into()),
    }
    app.activate(true);
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (model, language, memory, autostart_on, mic) = {
            let st = self.state.read(cx);
            (
                st.settings.model,
                st.settings.language.clone(),
                st.memory,
                st.settings.start_on_login,
                st.settings.microphone.clone(),
            )
        };

        let mic_names_hint = mic.clone().unwrap_or_else(|| "system default".into());
        let mic_label: SharedString = mic_names_hint.into();
        let mic_pill = ui::pill("mic", "microphone", false, false, false, cx, |this, _e, _w, cx| {
            let state = this.state.clone();
            let names = audio::input_device_names();
            state.update(cx, |s, _cx| {
                if !names.is_empty() {
                    let next = match s.settings.microphone.as_ref().and_then(|c| names.iter().position(|n| n == c)) {
                        Some(i) => names.get((i + 1) % names.len()).cloned(),
                        None => names.first().cloned(),
                    };
                    s.settings.microphone = next;
                }
                let _ = s.settings.save();
            });
            this.message = audio::input_device_names().join(" · ").into();
            cx.notify();
        });
        let capture_row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(mic_pill)
            .child(div().text_size(px(11.)).text_color(gpui::rgb(ui::MUTED)).child(mic_label));

        let autostart_pill = ui::pill(
            if autostart_on { "autostart" } else { "autostart-off" },
            if autostart_on { "start on login ✓" } else { "start on login" },
            false,
            false,
            autostart_on,
            cx,
            |this, _e, _w, cx| {
                let state = this.state.clone();
                let now = state.update(cx, |s, _cx| {
                    s.settings.start_on_login = !s.settings.start_on_login;
                    let _ = s.settings.save();
                    s.settings.start_on_login
                });
                let _ = autostart::set_enabled(now);
                cx.notify();
            },
        );
        let hint_pill = ui::pill("hint", "how to trigger", false, false, false, cx, |this, _e, _w, cx| {
            this.message = "bind a keyboard shortcut to `whisper-flow toggle` (KDE: System Settings → Shortcuts → Custom)".into();
            cx.notify();
        });
        let quit_pill = ui::pill("quit", "quit", false, true, false, cx, |this, _e, _w, cx| {
            let ev = this.state.read(cx).events.clone();
            let _ = ev.unbounded_send(UiEvent::QuitApp);
        });
        let system_row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(autostart_pill)
            .child(hint_pill)
            .child(quit_pill);

        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .size_full()
            .bg(gpui::rgb(ui::BG))
            .text_color(gpui::rgb(ui::TEXT))
            // header
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_color(gpui::rgb(ui::ACCENT)).font_weight(FontWeight(700.)).child("◆"))
                            .child(div().text_size(px(15.)).font_weight(FontWeight(600.)).child("Whisper Flow")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(ui::gauge(memory.1 as f32 / 204_800.0, px(90.)))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(gpui::rgb(ui::MUTED))
                                    .child(format!("peak {} / 200 MB", memory.1 / 1024)),
                            ),
                    ),
            )
            // model section
            .child(ui::section_label("MODEL · offline, quantized"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(model_mgr::catalog().into_iter().map(|info| {
                        let id = info.model.id().to_string();
                        let row_ui = self
                            .downloads
                            .get(&id)
                            .cloned()
                            .unwrap_or(ModelRowUi { progress: None, downloaded: false });
                        let selected = model == info.model;
                        let info_model = info.model;
                        let id_btn = id.clone();

                        let action = if let Some(p) = row_ui.progress {
                            div()
                                .id(ElementId::Name(format!("prog-{id}").into()))
                                .h(px(5.))
                                .w(px(86.))
                                .rounded(px(3.))
                                .bg(gpui::rgb(ui::BG))
                                .child(div().h(px(5.)).w(px((p * 86.0).max(2.0))).rounded(px(3.)).bg(gpui::rgb(ui::OK)))
                                .into_any_element()
                        } else if row_ui.downloaded {
                            if selected {
                                div().text_size(px(11.)).text_color(gpui::rgb(ui::MUTED)).child("active").into_any_element()
                            } else {
                                ui::pill(&id_btn, "use", true, false, false, cx, move |this, _e, _w, cx| {
                                    this.set_model(cx, info_model);
                                    cx.notify();
                                })
                                .into_any_element()
                            }
                        } else {
                            let id_arg = id_btn.clone();
                            ui::pill(&id_arg, "download", true, false, false, cx, move |this, _e, _w, cx| {
                                this.start_download(cx, id_btn.clone());
                                cx.notify();
                            })
                            .into_any_element()
                        };

                        let meta = format!(
                            "{} · ~{} MB peak{}",
                            format_size(info.size_bytes),
                            info.est_peak_mb,
                            if info.over_budget { " · over budget" } else { "" }
                        );
                        div()
                            .id(ElementId::Name(format!("row-{id}").into()))
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded(px(10.))
                            .bg(gpui::rgb(if selected { ui::ACCENT_DIM } else { ui::SURFACE }))
                            .when(selected, |d| d.border_1().border_color(gpui::rgb(ui::ACCENT)))
                            .when(!selected, |d| d.border_1().border_color(gpui::rgb(ui::BORDER)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight(600.0))
                                            .child(id.clone()),
                                    )
                                    .child(div().text_size(px(10.5)).text_color(gpui::rgb(ui::MUTED)).child(meta)),
                            )
                            .child(div().flex_1())
                            .child(action)
                    })),
            )
            // language
            .child(ui::section_label("LANGUAGE"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(["en", "auto"].into_iter().map(|l| {
                        let lv = l.to_string();
                        let active = language == lv;
                        ui::pill(
                            &format!("lang-{lv}"),
                            lv.clone(),
                            false,
                            false,
                            active,
                            cx,
                            move |this, _e, _w, cx| {
                                this.cycle_language(cx, &lv);
                                cx.notify();
                            },
                        )
                    })),
            )
            // capture
            .child(ui::section_label("CAPTURE"))
            .child(capture_row)
            // system
            .child(ui::section_label("SYSTEM"))
            .child(system_row)
            .when(!self.message.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(gpui::rgb(ui::WARN))
                        .child(self.message.clone()),
                )
            })
    }
}

fn format_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 100.0 {
        format!("{:.0} MB", mb)
    } else {
        format!("{:.1} MB", mb)
    }
}
