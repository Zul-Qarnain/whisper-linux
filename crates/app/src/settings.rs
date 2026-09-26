use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Model {
    TinyEnQ5,
    BaseEnQ5,
    SmallEnQ5,
}

impl Model {
    pub fn id(self) -> &'static str {
        match self {
            Model::TinyEnQ5 => "tiny.en-q5",
            Model::BaseEnQ5 => "base.en-q5",
            Model::SmallEnQ5 => "small.en-q5",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub model: Model,
    pub language: String,
    pub microphone: Option<String>,
    pub start_on_login: bool,
    pub threads: Option<usize>,
    pub idle_unload_secs: u64,
    pub vad_threshold: f32,
    pub silence_timeout_ms: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: Model::TinyEnQ5,
            language: "en".into(),
            microphone: None,
            start_on_login: false,
            threads: None,
            idle_unload_secs: 120,
            vad_threshold: 0.012,
            silence_timeout_ms: 700,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("whisper-flow").join("settings.json")
    }

    pub fn load() -> Self {
        fs::read(Self::path()).ok().and_then(|v| serde_json::from_slice(&v).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn effective_threads(&self) -> usize {
        self.threads.unwrap_or_else(crate::system::recommended_threads).clamp(1, 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
        assert_eq!(s.model, Model::TinyEnQ5);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let back: Settings = serde_json::from_str(r#"{"model":"BaseEnQ5"}"#).unwrap();
        assert_eq!(back.model, Model::BaseEnQ5);
        assert_eq!(back.language, "en");
    }
}
