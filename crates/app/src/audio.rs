use crate::events::{UiEvent, UiEvents};
use crate::settings::Settings;
use crate::vad::VoiceActivityDetector;
use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Host, SampleFormat, StreamConfig};
use crossbeam_channel::unbounded as cross_unbounded;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

const TARGET_RATE: usize = 16_000;
const PRE_ROLL_MS: usize = 300;
const MAX_UTTERANCE_SECS: usize = 30;
const CHUNK_MS: usize = 20;

/// One recording session on its own thread; ends on endpoint, stop flag, or cap.
pub struct Recording {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

pub fn input_device_names() -> Vec<String> {
    host().input_devices()
        .map(|d| {
            d.filter_map(|dev| dev.description().ok().map(|desc| desc.name().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn host() -> Host {
    cpal::default_host()
}

fn select_device(settings: &Settings) -> Result<Device> {
    let h = host();
    if let Some(want) = settings.microphone.as_deref() {
        if let Ok(devs) = h.input_devices() {
            for dev in devs {
                if let Ok(desc) = dev.description() {
                    if desc.name() == want {
                        return Ok(dev);
                    }
                }
            }
        }
        bail!("microphone '{want}' not found");
    }
    h.default_input_device().context("no input device (microphone) found")
}

/// Picks 16 kHz mono if the device allows it, else the nearest supported rate.
fn pick_config(device: &Device) -> Result<(StreamConfig, SampleFormat)> {
    let mut best: Option<(StreamConfig, SampleFormat, u32)> = None; // (config, fmt, cost)
    for cfg in device.supported_input_configs().context("query input configs")? {
        let fmt = cfg.sample_format();
        if !matches!(fmt, SampleFormat::F32 | SampleFormat::I16) || cfg.channels() != 1 {
            continue;
        }
        let minr = cfg.min_sample_rate();
        let maxr = cfg.max_sample_rate();
        let want = if minr <= TARGET_RATE as u32 && TARGET_RATE as u32 <= maxr {
            TARGET_RATE as u32
        } else {
            minr
        };
        let cost = want.abs_diff(TARGET_RATE as u32);
        let min_buf = match cfg.buffer_size() {
            cpal::SupportedBufferSize::Range { min, .. } => *min,
            cpal::SupportedBufferSize::Unknown => 512,
        };
        let config = StreamConfig {
            channels: 1,
            sample_rate: want,
            buffer_size: cpal::BufferSize::Fixed(min_buf.clamp(256, 2048)),
        };
        if best.as_ref().map(|(_, _, c)| cost < *c).unwrap_or(true) {
            best = Some((config, fmt, cost));
        }
    }
    best.map(|(c, f, _)| (c, f)).context("no usable mono 16-bit/float input config on this device")
}

pub fn start_recording(settings: Settings, events: UiEvents) -> Recording {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let tx = events.clone();
    let handle = std::thread::Builder::new()
        .name("wf-capture".into())
        .spawn(move || {
            if let Err(e) = capture_loop(&settings, &stop2, &tx) {
                let _ = tx.unbounded_send(UiEvent::Failed(e.to_string()));
            }
        })
        .expect("spawn capture thread");
    Recording { stop, handle: Some(handle) }
}

impl Recording {
    /// Signal early stop (toggle-off) and wait for the thread to flush a partial clip.
    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle {
            let _ = h.join();
        }
    }
}

fn capture_loop(settings: &Settings, stop: &Arc<AtomicBool>, events: &UiEvents) -> Result<()> {
    let device = select_device(settings)?;
    let (config, format) = pick_config(&device)?;
    let raw_rate = config.sample_rate as usize;

    let (raw_tx, raw_rx) = cross_unbounded::<Vec<f32>>();
    let err_tx = events.clone();
    let stream = match format {
        SampleFormat::F32 => device.build_input_stream(
            config.clone(),
            move |data: &[f32], _info| {
                let _ = raw_tx.send(data.to_vec());
            },
            move |e: cpal::Error| {
                let _ = err_tx.unbounded_send(UiEvent::Failed(format!("capture error: {e}")));
            },
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream(
            config.clone(),
            move |data: &[i16], _info| {
                let _ = raw_tx.send(data.iter().map(|s| *s as f32 / 32768.0).collect());
            },
            move |e: cpal::Error| {
                let _ = err_tx.unbounded_send(UiEvent::Failed(format!("capture error: {e}")));
            },
            None,
        )?,
        other => bail!("unsupported sample format {other:?}"),
    };
    stream.play().context("start capture stream")?;

    let pre_roll = PRE_ROLL_MS * TARGET_RATE / 1000;
    let silence_needed = (settings.silence_timeout_ms as usize / CHUNK_MS).max(1);
    let cap = MAX_UTTERANCE_SECS * TARGET_RATE;
    let mut vad = VoiceActivityDetector::new(settings.vad_threshold);
    let mut preroll: Vec<f32> = Vec::new();
    let mut collected: Vec<f32> = Vec::with_capacity(TARGET_RATE * 10);
    let mut resampler = Resampler::new(raw_rate, TARGET_RATE);
    let mut started = false;
    let mut endpointed = false;
    let deadline = std::time::Instant::now() + Duration::from_millis(8000);

    while !stop.load(Ordering::Relaxed) {
        if !started && deadline.elapsed() > std::time::Duration::ZERO {
            drop(stream);
            let _ = events.unbounded_send(UiEvent::Failed("no speech detected".into()));
            return Ok(());
        }
        let Ok(chunk) = raw_rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        let resampled = resampler.push(&chunk);
        if resampled.is_empty() {
            continue;
        }
        let rms = vad.feed(&resampled);
        let _ = events.unbounded_send(UiEvent::Level(rms));

        if !started {
            if vad.speech_active {
                collected.extend_from_slice(&preroll);
                preroll.clear();
                collected.extend_from_slice(&resampled);
                started = true;
            } else {
                preroll.extend_from_slice(&resampled);
                let over = preroll.len().saturating_sub(pre_roll);
                if over > 0 {
                    preroll.drain(..over);
                }
            }
        } else {
            collected.extend_from_slice(&resampled);
            if vad.silence_chunks >= silence_needed || collected.len() >= cap {
                endpointed = true;
                break;
            }
        }
    }
    drop(stream); // stops capture

    if started && collected.len() > TARGET_RATE / 4 {
        let _ = events.unbounded_send(UiEvent::Utterance { audio: collected, partial: !endpointed });
    } else {
        let _ = events.unbounded_send(UiEvent::Failed(if started { "clip too short".into() } else { "no speech detected".into() }));
    }
    Ok(())
}

/// Drift-free linear resampler across small chunks: keeps the absolute read
/// position so chunk boundaries never skip or repeat samples.
pub struct Resampler {
    from_rate: usize,
    to_rate: usize,
    abs_pos: f64,
    consumed: usize,
}

impl Resampler {
    pub fn new(from_rate: usize, to_rate: usize) -> Self {
        Self { from_rate, to_rate, abs_pos: 0.0, consumed: 0 }
    }

    pub fn push(&mut self, chunk: &[f32]) -> Vec<f32> {
        if self.from_rate == self.to_rate {
            return chunk.to_vec();
        }
        if chunk.is_empty() {
            return Vec::new();
        }
        let ratio = self.from_rate as f64 / self.to_rate as f64;
        let start = self.consumed as f64;
        let mut out = Vec::with_capacity((chunk.len() as f64 / ratio) as usize + 2);
        let mut q = self.abs_pos;
        loop {
            let p = q - start;
            if p + 1.0 >= chunk.len() as f64 {
                break;
            }
            let i0 = p as usize;
            let frac = (p - i0 as f64) as f32;
            out.push(chunk[i0] * (1.0 - frac) + chunk[i0 + 1] * frac);
            q += ratio;
        }
        self.abs_pos = q;
        self.consumed += chunk.len();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::Resampler;

    #[test]
    fn exact_ratio_48_to_16() {
        let input: Vec<f32> = (0..4800).map(|i| (i as f64 * 0.01).sin() as f32).collect();
        let mut r = Resampler::new(48_000, 16_000);
        let mut out = Vec::new();
        for chunk in input.chunks(960) {
            out.extend(r.push(chunk));
        }
        let expected = input.len() as f64 * 16_000.0 / 48_000.0;
        assert!((out.len() as f64 - expected).abs() <= 2.0, "got {}", out.len());
    }

    #[test]
    fn fractional_ratio_never_panics_or_drifts() {
        // 44100 -> 16000 with awkward chunk sizes exercises carry across chunks.
        let input: Vec<f32> = (0..44100).map(|i| (i as f64 * 0.001).sin() as f32).collect();
        let mut r = Resampler::new(44_100, 16_000);
        let mut out = Vec::new();
        for chunk in input.chunks(1023) {
            out.extend(r.push(chunk));
        }
        let expected = 44100.0 * 16_000.0 / 44_100.0;
        assert!((out.len() as f64 - expected).abs() <= 2.0, "got {} expected ~{expected}", out.len());
        assert!(out.iter().all(|s| s.is_finite()));
    }
}