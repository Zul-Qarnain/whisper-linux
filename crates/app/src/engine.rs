use anyhow::{bail, Context, Result};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Backend-swappable ASR seam (spec: "Whisper backend can later be replaced").
pub trait AsrBackend {
    fn transcribe(&self, audio: &[f32], language: Option<&str>, cancel: Arc<AtomicBool>) -> Result<String>;
}

pub struct Engine {
    context: WhisperContext,
    threads: usize,
}

impl Engine {
    pub fn load(path: &Path, threads: usize) -> Result<Self> {
        let path = path.to_str().context("non-utf8 model path")?;
        let context = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .with_context(|| format!("load whisper model {path}"))?;
        Ok(Self { context, threads: threads.clamp(1, 8) })
    }

    /// `audio` is 16 kHz mono f32. The attention window is sized to the clip so
    /// short dictation doesn't pay for a full 30 s context (RAM + latency).
    pub fn transcribe(&self, audio: &[f32], language: Option<&str>, cancel: Arc<AtomicBool>) -> Result<String> {
        let mut state = self.context.create_state().context("create whisper state")?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads as i32);
        params.set_translate(false);
        params.set_language(language);
        params.set_detect_language(false);
        params.set_no_timestamps(true);
        params.set_no_context(true);
        params.set_single_segment(true);
        params.set_max_tokens(64);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.0);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_token_timestamps(false);
        params.set_audio_ctx(((audio.len() / 160) as i32 + 2).clamp(512, 1500));

        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let flag = cancel.clone();
        params.set_abort_callback_safe(move || flag.load(Ordering::Relaxed));

        state.full(params, audio).context("run whisper inference")?;
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }

        let mut text = String::new();
        for segment in state.as_iter() {
            if let Ok(s) = segment.to_str_lossy() {
                text.push_str(&s);
                text.push(' ');
            }
        }
        Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

impl AsrBackend for Engine {
    fn transcribe(&self, audio: &[f32], language: Option<&str>, cancel: Arc<AtomicBool>) -> Result<String> {
        Engine::transcribe(self, audio, language, cancel)
    }
}
