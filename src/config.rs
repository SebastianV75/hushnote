//! Settings in `~/.config/hushnote/config.json`, created with defaults on first run.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Summaries {
    #[default]
    Local,
    Claude,
    Codex,
    #[serde(rename = "opencode")]
    OpenCode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// GGUF speech model for transcribe.cpp (Parakeet v3 by default, the same one Hex uses).
    pub speech_model: PathBuf,
    /// The meeting's language: "es" or "en". It also picks the speech model (see `models`).
    pub language: String,
    /// Local LLM server with an OpenAI-compatible API (Ollama, llama-server, LM Studio).
    pub llm_url: String,
    pub llm_model: String,
    /// Who writes the summary: the local LLM (default) or a CLI the user is signed in to.
    pub summaries: Summaries,
    /// The model for Claude Code, Codex or OpenCode (e.g. "sonnet", "opencode/big-pickle");
    /// empty = the tool's own default.
    pub summary_cli_model: String,
    /// Folder where meetings are saved as Markdown.
    pub notes_dir: PathBuf,
    /// Also capture the system audio (what the other people say).
    pub system_audio: bool,
    /// Look for a newer release on GitHub once a day.
    pub check_updates: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            speech_model: crate::models::PARAKEET_V3.path(),
            language: "es".into(),
            llm_url: "http://localhost:11434/v1".into(),
            llm_model: crate::ollama::DEFAULT_MODEL.into(),
            summaries: Summaries::Local,
            summary_cli_model: String::new(),
            notes_dir: dirs::document_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Hushnote"),
            system_audio: true,
            check_updates: true,
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
            Ok(text) => {
                let mut config: Config = serde_json::from_str(&text).unwrap_or_else(|e| {
                    eprintln!("hushnote: {} is not valid ({e}); using defaults", path.display());
                    Config::default()
                });
                // Earlier defaults: `qwen3:4b` now names a model that always thinks first.
                if config.llm_model == "qwen3:4b" {
                    config.llm_model = crate::ollama::DEFAULT_MODEL.into();
                }
                // Earlier default: detect the language. The app now offers Spanish and English.
                if config.language.trim().is_empty() {
                    config.language = "es".into();
                }
                config
            }
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
