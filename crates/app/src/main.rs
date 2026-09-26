mod app;
mod audio;
mod autostart;
mod engine;
mod events;
mod hotkey;
mod inject;
mod model_mgr;
mod service;
mod settings;
mod system;
mod ui;
mod vad;
mod wav;

use anyhow::{bail, Context, Result};
use futures::executor::block_on;
use futures::StreamExt;
use settings::Model;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("run") => app::run(settings::Settings::load()),
        Some("toggle") | Some("start") | Some("stop") => hotkey::send_command(&args[0]),
        Some("settings") => hotkey::send_command("settings"),
        Some("quit") => hotkey::send_command("quit"),
        Some("transcribe-file") => transcribe_file(&args[1..]),
        Some("record") => record(&args[1..]),
        Some("models") => models(&args[1..]),
        Some("serve-demo") => serve_demo(&args[1..]),
        Some(other) => {
            eprintln!(
                "unknown command: {other}\nusage: whisper-flow [run | toggle | settings | quit | transcribe-file <wav> [--model <id>] | record <out.wav> [--transcribe] | models <list|download <id>|remove <id>>]"
            );
            bail!("unknown command");
        }
    }
}

fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn record(args: &[String]) -> Result<()> {
    let out = args.first().filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "record.wav".into());
    let transcribe = args.iter().any(|a| a == "--transcribe");
    let settings = settings::Settings::load();
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<events::UiEvent>();
    println!("recording — speak now (endpoint at {} ms silence, max 30 s)", settings.silence_timeout_ms);
    let rec = audio::start_recording(settings.clone(), tx);
    let clip = loop {
        match block_on(rx.next()) {
            Some(events::UiEvent::Level(rms)) => print!("\r  level {rms:.3}   "),
            Some(events::UiEvent::Utterance { audio, .. }) => {
                println!();
                break audio;
            }
            Some(events::UiEvent::Failed(msg)) => {
                println!();
                rec.stop();
                bail!("capture: {msg}");
            }
            Some(_) => {}
            None => {
                rec.stop();
                bail!("capture stream closed");
            }
        }
    };
    rec.stop();
    wav::write_wav_pcm16(&PathBuf::from(&out), &clip, wav::TARGET_RATE)?;
    println!("  wrote {out} ({:.1} s)", clip.len() as f64 / 16000.0);
    if transcribe {
        let path = model_mgr::path_for(settings.model)?;
        let engine = engine::Engine::load(&path, settings.effective_threads())?;
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let lang = if settings.language == "auto" { None } else { Some(settings.language.as_str()) };
        let t = Instant::now();
        let text = engine.transcribe(&clip, lang, cancel)?;
        let (_, hwm) = system::self_memory();
        println!("[mem] peak RSS {hwm} kB | {:.2}s\ntext: {text}", t.elapsed().as_secs_f64());
    }
    Ok(())
}

fn model_from_id(id: &str) -> Result<Model> {
    Ok(match id {
        "tiny.en-q5" | "tiny" => Model::TinyEnQ5,
        "base.en-q5" | "base" => Model::BaseEnQ5,
        "small.en-q5" | "small" => Model::SmallEnQ5,
        _ => bail!("unknown model id: {id}"),
    })
}

fn transcribe_file(args: &[String]) -> Result<()> {
    let Some(path) = args.first() else {
        bail!("usage: whisper-flow transcribe-file <wav> [--model <id>] [--threads <n>]");
    };
    let model_id = parse_flag(args, "--model").unwrap_or_else(|| "tiny.en-q5".into());
    let model = model_from_id(&model_id)?;
    let threads: usize = parse_flag(args, "--threads")
        .map(|s| s.parse().context("--threads"))
        .transpose()?
        .unwrap_or_else(system::recommended_threads);

    let samples = wav::read_wav_16k_mono(&PathBuf::from(path))?;
    let secs = samples.len() as f64 / 16000.0;
    let model_path = model_mgr::path_for(model)?;
    if !model_path.is_file() {
        bail!("model not downloaded: {} — run `whisper-flow models download {}`", model_path.display(), model.id());
    }

    let (rss0, _) = system::self_memory();
    let t = Instant::now();
    let engine = engine::Engine::load(&model_path, threads)?;
    let (rss1, _) = system::self_memory();
    println!(
        "[mem] idle RSS {rss0} kB -> loaded RSS {rss1} kB ({:.2}s load)",
        t.elapsed().as_secs_f64()
    );

    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let t = Instant::now();
    let text = engine.transcribe(&samples, Some("en"), cancel)?;
    let (_, hwm) = system::self_memory();
    println!("[mem] peak RSS {hwm} kB | {:.2}s decode of {secs:.1}s audio | {threads} threads", t.elapsed().as_secs_f64());
    println!("text: {text}");
    Ok(())
}

fn models(args: &[String]) -> Result<()> {    match args.first().map(String::as_str) {
        Some("list") | None => {
            for info in model_mgr::catalog() {
                let have = model_mgr::is_downloaded(info.model);
                println!(
                    "{:<12} {:>3} MB file  est peak {:>3} MB {:<4} {}",
                    info.model.id(),
                    info.size_bytes / 1_048_576,
                    info.est_peak_mb,
                    if have { "[ok]" } else { "[--]" },
                    if info.over_budget { "(exceeds 200 MB budget)" } else { "" }
                );
            }
            Ok(())
        }
        Some("download") => {
            let id = args.get(1).context("usage: models download <id>")?;
            let model = model_from_id(id)?;
            let mut last = std::time::Instant::now();
            let path = model_mgr::download(model, |got, total| {
                if last.elapsed().as_millis() > 250 || got == total {
                    print!("\r  {:.1} / {:.1} MB ({:.0}%)", got as f64 / 1e6, total as f64 / 1e6, got as f64 * 100.0 / total as f64);
                    last = std::time::Instant::now();
                }
            })?;
            println!("\n  saved {}", path.display());
            Ok(())
        }
        Some("remove") => {
            let id = args.get(1).context("usage: models remove <id>")?;
            model_mgr::remove(model_from_id(id)?)
        }
        Some(other) => bail!("unknown models command: {other}"),
    }
}

/// Hidden harness: exercise the persistent service + injection like the daemon does.
fn serve_demo(args: &[String]) -> Result<()> {
    let path = PathBuf::from(args.first().map(String::as_str).unwrap_or("tests/fixtures/jfk.wav"));
    let samples = wav::read_wav_16k_mono(&path)?;
    let (tx, rx) = crossbeam_channel::unbounded::<service::ServiceEvent>();
    let svc = service::InferenceService::spawn(settings::Model::TinyEnQ5, 4, tx);
    let h = std::thread::spawn(move || {
        for ev in rx {
            match ev {
                service::ServiceEvent::Loaded { model, load_ms } => println!("loaded {model:?} in {load_ms}ms"),
                service::ServiceEvent::Done { id, text } => {
                    println!("done {id}: {text}");
                    let (etx, _) = futures::channel::mpsc::unbounded();
                    inject::inject(text, etx);
                }
                service::ServiceEvent::Failed { id, error } => println!("failed {id}: {error}"),
                service::ServiceEvent::Unloaded => println!("unloaded"),
            }
        }
    });
    svc.transcribe(1, samples, Some("en".into()))?;
    std::thread::sleep(std::time::Duration::from_secs(8));
    let (_, hwm) = system::self_memory();
    println!("peak kB {hwm}");
    svc.quit()?;
    h.join().unwrap();
    Ok(())
}
