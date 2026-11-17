//! Settings in `~/.config/hushnote/config.json`, created with defaults on first run.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// GGUF speech model for transcribe.cpp (Parakeet v3 by default, the same one Hex uses).
    pub speech_model: PathBuf,
    /// The meeting's language as an ISO code (e.g. "en", "es"); empty = detect it.
    pub language: String,
    /// Local LLM server with an OpenAI-compatible API (Ollama, llama-server, LM Studio).
    pub llm_url: String,
    pub llm_model: String,
    /// Folder where meetings are saved as Markdown.
    pub notes_dir: PathBuf,
    /// Also capture the system audio (what the other people say).
    pub system_audio: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            speech_model: data_dir().join("models").join("parakeet-tdt-0.6b-v3-Q8_0.gguf"),
            language: String::new(),
            llm_url: "http://localhost:11434/v1".into(),
            llm_model: "qwen3:8b".into(),
            notes_dir: dirs::document_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Hushnote"),
            system_audio: true,
        }
    }
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("hushnote")
}

pub fn config_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("hushnote").join("config.json")
}

impl Config {
    pub fn load() -> Config {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("hushnote: {} is not valid ({e}); using defaults", path.display());
                Config::default()
            }),
            Err(_) => {
                let config = Config::default();
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap_or_default());
                config
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn language(&self) -> Option<String> {
        let language = self.language.trim();
        (!language.is_empty()).then(|| language.to_string())
    }
}
