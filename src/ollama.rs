//! One-click setup of the local summary model through Ollama's own API: whether the server is
//! up and has the model (`/api/tags`), and pulling it with progress (`/api/pull`).

use std::io::{BufRead, BufReader};

/// The default summary model: Qwen3 4B Instruct 2507 (~2.5 GB), the smallest that writes
/// reliable notes and task lists in Spanish and English. The instruct build answers right away;
/// plain `qwen3:4b` is now the thinking build, which reasons for minutes on a CPU first.
pub const DEFAULT_MODEL: &str = "qwen3:4b-instruct-2507-q4_K_M";

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// No server answered at the configured URL.
    NotRunning,
    /// The server is up but doesn't have the model.
    Missing,
    Ready,
}

/// `http://localhost:11434/v1` → `http://localhost:11434`.
fn base(llm_url: &str) -> String {
    llm_url.trim_end_matches('/').trim_end_matches("/v1").to_string()
}

/// `qwen3` means `qwen3:latest`.
fn same_model(a: &str, b: &str) -> bool {
    let full = |s: &str| if s.contains(':') { s.to_string() } else { format!("{s}:latest") };
    full(a) == full(b)
}

pub fn check(llm_url: &str, model: &str) -> State {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(3)))
        .build()
        .into();
    let Ok(response) = agent.get(&format!("{}/api/tags", base(llm_url))).call() else {
        return State::NotRunning;
    };
    let Ok(json) = response.into_body().read_json::<serde_json::Value>() else {
        // Something answers, but not Ollama (llama-server, LM Studio): trust the config.
        return State::Ready;
    };
    let has = json["models"].as_array().is_some_and(|ms| {
        ms.iter().any(|m| m["name"].as_str().or(m["model"].as_str()).is_some_and(|n| same_model(n, model)))
    });
    if has { State::Ready } else { State::Missing }
}

/// Pulls `model`; `progress` gets the bytes so far and the total of the layer being fetched.
pub fn pull(llm_url: &str, model: &str, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
    let response = ureq::post(&format!("{}/api/pull", base(llm_url)))
        .send_json(serde_json::json!({ "model": model, "stream": true }))
        .map_err(|e| format!("Couldn't reach Ollama: {e}"))?;
    let reader = BufReader::new(response.into_body().into_with_config().limit(u64::MAX).reader());
    for line in reader.lines() {
        let line = line.map_err(|e| format!("Download interrupted ({e}). Try again to resume it."))?;
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if let Some(e) = json["error"].as_str() {
            return Err(format!("Ollama: {e}"));
        }
        if let (Some(done), Some(total)) = (json["completed"].as_u64(), json["total"].as_u64()) {
            progress(done, total);
        }
        if json["status"].as_str() == Some("success") {
            return Ok(());
        }
    }
    Err("Ollama stopped before the model was ready.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_names() {
        assert_eq!(base("http://localhost:11434/v1"), "http://localhost:11434");
        assert_eq!(base("http://localhost:11434/v1/"), "http://localhost:11434");
        assert!(same_model("qwen3:4b", "qwen3:4b"));
        assert!(same_model("llama3.2", "llama3.2:latest"));
        assert!(!same_model("qwen3:8b", "qwen3:4b"));
    }

    #[test]
    fn nothing_listening_is_not_running() {
        assert_eq!(check("http://127.0.0.1:9/v1", "qwen3:4b"), State::NotRunning);
    }

    /// Against a server speaking Ollama's API (`HUSHNOTE_OLLAMA`, e.g. http://127.0.0.1:11434/v1):
    /// missing → pull with progress → ready, and a pull error comes back as an error.
    #[test]
    #[ignore = "needs an Ollama server"]
    fn pull_makes_the_model_ready() {
        let url = std::env::var("HUSHNOTE_OLLAMA").expect("HUSHNOTE_OLLAMA");
        assert_eq!(check(&url, "qwen3:4b"), State::Missing);
        let mut seen = Vec::new();
        pull(&url, "qwen3:4b", |done, total| seen.push((done, total))).unwrap();
        assert!(seen.len() >= 2 && seen.last().unwrap().0 == seen.last().unwrap().1);
        assert_eq!(check(&url, "qwen3:4b"), State::Ready);
        assert!(pull(&url, "nope:1b", |_, _| {}).unwrap_err().contains("does not exist"));
    }
}
