use crate::settings::Model;
use anyhow::{bail, Context, Result};
use std::{fs, io::{Read, Write}, path::PathBuf};

const HF: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub model: Model,
    pub file: &'static str,
    pub size_bytes: u64,
    pub est_peak_mb: u32,
    pub over_budget: bool,
}

impl ModelInfo {
    pub fn url(&self) -> String {
        format!("{HF}/{}", self.file)
    }
}

pub fn catalog() -> Vec<ModelInfo> {
    vec![
        ModelInfo { model: Model::TinyEnQ5, file: "ggml-tiny.en-q5_1.bin", size_bytes: 32_166_155, est_peak_mb: 130, over_budget: false },
        ModelInfo { model: Model::BaseEnQ5, file: "ggml-base.en-q5_1.bin", size_bytes: 59_721_011, est_peak_mb: 190, over_budget: false },
        ModelInfo { model: Model::SmallEnQ5, file: "ggml-small.en-q5_1.bin", size_bytes: 190_098_681, est_peak_mb: 340, over_budget: true },
    ]
}

pub fn info_for(model: Model) -> ModelInfo {
    catalog().into_iter().find(|i| i.model == model).expect("model in catalog")
}

pub fn models_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir().context("no XDG data dir")?.join("whisper-flow").join("models");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn path_for(model: Model) -> Result<PathBuf> {
    Ok(models_dir()?.join(info_for(model).file))
}

pub fn is_downloaded(model: Model) -> bool {
    path_for(model).map(|p| p.is_file()).unwrap_or(false)
}

pub fn remove(model: Model) -> Result<()> {
    let path = path_for(model)?;
    if path.is_file() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

/// Streams the model to `<file>.part`, verifies length, then atomically renames.
pub fn download(model: Model, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    let info = info_for(model);
    let dest = models_dir()?.join(info.file);
    if dest.is_file() && dest.metadata().map(|m| m.len() == info.size_bytes).unwrap_or(false) {
        return Ok(dest);
    }
    let part = dest.with_file_name(format!("{}.part", info.file));
    let url = info.url();

    let mut resp = ureq::get(&url).call().with_context(|| format!("request to {url}"))?;
    if !resp.status().is_success() {
        bail!("HTTP {} downloading {url}", resp.status());
    }

    let mut file = fs::File::create(&part)?;
    let mut got = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    let mut reader = resp.body_mut().as_reader();
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        got += n as u64;
        progress(got, info.size_bytes);
    }
    drop(file);
    if got != info.size_bytes {
        let _ = fs::remove_file(&part);
        bail!("download truncated: got {got} of {} bytes", info.size_bytes);
    }
    fs::rename(&part, &dest)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_urls_match_files() {
        let cat = catalog();
        assert_eq!(cat.len(), 3);
        for info in &cat {
            assert!(info.url().ends_with(info.file));
            assert!(info.file.starts_with("ggml-"));
        }
        assert_eq!(cat.iter().filter(|i| i.model == Model::TinyEnQ5).count(), 1);
    }

    #[test]
    fn budget_flags() {
        let small = info_for(Model::SmallEnQ5);
        assert!(small.over_budget);
        assert!(!info_for(Model::TinyEnQ5).over_budget);
    }
}
