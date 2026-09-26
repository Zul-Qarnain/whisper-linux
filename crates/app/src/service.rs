use crate::engine::Engine;
use crate::model_mgr;
use crate::settings::Model;
use crate::system;
use anyhow::Result;
use crossbeam_channel::{unbounded, Sender};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Job {
    Transcribe { id: u64, audio: Vec<f32>, language: Option<String> },
    /// Cancel the job with this id (ignored if that job already finished).
    Cancel { id: u64 },
    SetModel { model: Model, threads: usize },
    Unload,
    Quit,
}

#[derive(Clone, Debug)]
pub enum ServiceEvent {
    Done { id: u64, text: String },
    Failed { id: u64, error: String },
    Loaded { model: Model, load_ms: u64 },
    Unloaded,
}

pub struct InferenceService {
    tx: Sender<Job>,
    cancel: Arc<AtomicBool>,
}

static LAST_JOB_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl InferenceService {
    pub fn spawn(model: Model, threads: usize, events: Sender<ServiceEvent>) -> Arc<Self> {
        let (tx, rx) = unbounded::<Job>();
        let cancel = Arc::new(AtomicBool::new(false));

        std::thread::Builder::new()
            .name("wf-inference".into())
            .spawn({
                let cancel = cancel.clone();
                move || worker(model, threads, &cancel, rx, events)
            })
            .expect("spawn inference thread");

        // Idle watchdog: unloads the model after `idle_unload_secs` without a
        // job so resident memory drops back to ~0 (model is the whole cost).
        let wd_tx = tx.clone();
        std::thread::Builder::new()
            .name("wf-unload-watchdog".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(5));
                let idle_secs = crate::settings::Settings::load().idle_unload_secs;
                if idle_secs == 0 {
                    continue;
                }
                let last = LAST_JOB_MS.load(Ordering::Relaxed);
                if last != 0 && now_ms().saturating_sub(last) > idle_secs * 1000 {
                    let _ = wd_tx.send(Job::Unload);
                    LAST_JOB_MS.store(0, Ordering::Relaxed);
                }
            })
            .expect("spawn watchdog");

        Arc::new(Self { tx, cancel })
    }

    pub fn transcribe(&self, id: u64, audio: Vec<f32>, language: Option<String>) -> Result<()> {
        self.tx.send(Job::Transcribe { id, audio, language })?;
        Ok(())
    }

    pub fn cancel(&self, id: u64) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Job::Cancel { id });
    }

    pub fn set_model(&self, model: Model, threads: usize) -> Result<()> {
        Ok(self.tx.send(Job::SetModel { model, threads })?)
    }

    pub fn unload(&self) -> Result<()> {
        self.cancel.store(true, Ordering::Relaxed); // unblock a running job, then free the model
        let _ = self.tx.send(Job::Unload);
        Ok(())
    }

    pub fn quit(&self) -> Result<()> {
        let _ = self.tx.send(Job::Quit);
        Ok(())
    }
}

fn worker(mut model: Model, mut threads: usize, cancel: &Arc<AtomicBool>, rx: crossbeam_channel::Receiver<Job>, events: Sender<ServiceEvent>) {
    let mut engine: Option<Engine> = None;
    let mut running: Option<u64> = None;

    while let Ok(job) = rx.recv() {
        match job {
            Job::Transcribe { id, audio, language } => {
                LAST_JOB_MS.store(now_ms(), Ordering::Relaxed);
                cancel.store(false, Ordering::Relaxed);
                running = Some(id);

                if engine.is_none() {
                    match model_mgr::path_for(model) {
                        Ok(path) => match Engine::load(&path, threads) {
                            Ok(eng) => {
                                let t = Instant::now();
                                engine = Some(eng);
                                let _ = events.send(ServiceEvent::Loaded { model, load_ms: t.elapsed().as_millis() as u64 });
                            }
                            Err(e) => {
                                let _ = events.send(ServiceEvent::Failed { id, error: e.to_string() });
                                running = None;
                                continue;
                            }
                        },
                        Err(e) => {
                            let _ = events.send(ServiceEvent::Failed { id, error: e.to_string() });
                            running = None;
                            continue;
                        }
                    }
                }

                let result = engine.as_ref().map(|e| e.transcribe(&audio, language.as_deref(), cancel.clone()));
                match result {
                    Some(Ok(text)) => {
                        let _ = events.send(ServiceEvent::Done { id, text });
                    }
                    Some(Err(e)) => {
                        let _ = events.send(ServiceEvent::Failed { id, error: e.to_string() });
                    }
                    None => {}
                }
                running = None;
            }
            Job::Cancel { id } => {
                if running == Some(id) {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            Job::SetModel { model: m, threads: t } => {
                model = m;
                threads = t;
                if engine.take().is_some() {
                    system::malloc_trim();
                }
            }
            Job::Unload => {
                if engine.take().is_some() {
                    system::malloc_trim();
                    let _ = events.send(ServiceEvent::Unloaded);
                }
                LAST_JOB_MS.store(0, Ordering::Relaxed);
            }
            Job::Quit => break,
        }
    }
}
