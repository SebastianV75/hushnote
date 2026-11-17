//! Configuración en `~/.config/hushnote/config.json`. Se crea con valores por defecto la primera vez.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Modelo de voz GGUF para transcribe.cpp (por defecto Parakeet v3, el mismo de Hex).
    pub modelo_voz: PathBuf,
    /// Idioma de la reunión (código ISO, p. ej. "es"); vacío = detectar.
    pub idioma: String,
    /// Servidor LLM local compatible con OpenAI (Ollama, llama-server, LM Studio).
    pub llm_url: String,
    pub llm_modelo: String,
    /// Carpeta donde se guardan las reuniones en Markdown.
    pub carpeta_notas: PathBuf,
    /// Capturar también el audio del sistema (lo que dicen los demás).
    pub audio_sistema: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            modelo_voz: data_dir().join("models").join("parakeet-tdt-0.6b-v3-Q8_0.gguf"),
            idioma: "es".into(),
            llm_url: "http://localhost:11434/v1".into(),
            llm_modelo: "qwen3:8b".into(),
            carpeta_notas: dirs::document_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Hushnote"),
            audio_sistema: true,
        }
    }
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("hushnote")
}

fn config_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("hushnote").join("config.json")
}

impl Config {
    pub fn load() -> Config {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("hushnote: {} no es válido ({e}); uso valores por defecto", path.display());
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

    pub fn idioma(&self) -> Option<String> {
        let idioma = self.idioma.trim();
        (!idioma.is_empty()).then(|| idioma.to_string())
    }
}
