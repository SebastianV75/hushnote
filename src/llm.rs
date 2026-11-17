//! Summaries. By default from a local LLM over the OpenAI-compatible API
//! (`/v1/chat/completions`), which Ollama, llama-server (llama.cpp) and LM Studio all speak, so
//! nothing leaves the machine. Optionally through a coding agent's CLI the user is already
//! signed in to (Claude Code, Codex, OpenCode), which sends the transcript to that service.
//!
//! Ollama uses a short context by default, so with it a long meeting is summarized in parts and
//! the partial summaries are then merged. The hosted models take a whole meeting at once.

use crate::config::Summaries;
use crate::session::Line;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

const PART_CHARS: usize = 8_000;
/// Far more than any meeting; the hosted models take it in one go.
const CLI_PART_CHARS: usize = 600_000;

const SYSTEM: &str = "You are an assistant that writes clear, concise meeting notes in \
Markdown. Always write in the same language the meeting was held in (the language of the \
transcript), even though these instructions are in English. Use only what is in the transcript \
and the user's notes; never invent facts. The transcript is automatic and may contain wrong \
words: interpret them with common sense. \"You\" is the user; \"Them\" are the other \
participants.";

const FORMAT: &str = "Write the notes with exactly these three sections, with the headings \
translated into the meeting's language:\n\
## Decisions\n(bullets; \"None\" if there were none)\n\
## Action items\n(a task list, one line each: `- [ ] Owner does the task · due`. Add ` · due` only \
when a day or date was said, as a short word such as Thu or Mar 9. \"None\" if there were none)\n\
## Open questions\n(bullets; \"None\" if there were none)\n\
Put names, dates, amounts and product names in `inline code` sparingly, only when it helps \
scanning. If the user took notes, honour them: they are what matters most to the user, so \
expand them with details from the transcript. Add nothing before or after the sections.";

pub struct Llm {
    pub provider: Summaries,
    /// The local server and model.
    pub url: String,
    pub model: String,
    /// The model for a CLI provider; empty = the CLI's own default.
    pub cli_model: String,
}

impl Llm {
    pub fn from_config(c: &crate::config::Config) -> Llm {
        Llm { provider: c.summaries, url: c.llm_url.clone(), model: c.llm_model.clone(), cli_model: c.summary_cli_model.clone() }
    }
}

impl Summaries {
    pub const ALL: [Summaries; 4] = [Summaries::Local, Summaries::Claude, Summaries::Codex, Summaries::OpenCode];

    pub fn label(self) -> &'static str {
        match self {
            Summaries::Local => "Local",
            Summaries::Claude => "Claude",
            Summaries::Codex => "Codex",
            Summaries::OpenCode => "OpenCode",
        }
    }

    /// The command line tool, for the hosted providers.
    pub fn binary(self) -> Option<&'static str> {
        match self {
            Summaries::Local => None,
            Summaries::Claude => Some("claude"),
            Summaries::Codex => Some("codex"),
            Summaries::OpenCode => Some("opencode"),
        }
    }

    /// Whether it can be used right now: the local one always (Ollama is checked apart), a CLI
    /// when it is on `PATH`.
    pub fn installed(self) -> bool {
        self.binary().is_none_or(|b| find_binary(b).is_some())
    }

    /// What it needs, for the setup row.
    pub fn note(self) -> &'static str {
        match self {
            Summaries::Local => "Writes the summary on this machine.",
            Summaries::Claude => "Uses your Claude subscription through Claude Code. The transcript is sent to Anthropic.",
            Summaries::Codex => "Uses your ChatGPT subscription through Codex. The transcript is sent to OpenAI.",
            Summaries::OpenCode => "Uses OpenCode's free models; no account needed. The transcript is sent to OpenCode.",
        }
    }

    pub fn install_hint(self) -> &'static str {
        match self {
            Summaries::Local => "",
            Summaries::Claude => "Install Claude Code and sign in: npm i -g @anthropic-ai/claude-code, then run claude once.",
            Summaries::Codex => "Install Codex and sign in: npm i -g @openai/codex, then codex login.",
            Summaries::OpenCode => "Install OpenCode: npm i -g opencode-ai (or curl -fsSL https://opencode.ai/install | bash).",
        }
    }
}

pub fn find_binary(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

pub fn transcript_text(lines: &[Line]) -> String {
    lines.iter().map(|l| format!("[{}] {}: {}\n", clock(l.t_ms), l.speaker.name(), l.text)).collect()
}

pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) }
}

/// Splits the transcript into pieces of whole lines.
fn parts(text: &str, size: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    for line in text.lines() {
        let last = out.last_mut().unwrap();
        if !last.is_empty() && last.len() + line.len() > size {
            out.push(String::new());
        }
        let last = out.last_mut().unwrap();
        last.push_str(line);
        last.push('\n');
    }
    out
}

/// The answer in `opencode run --format json`: the text of its `text` events.
fn opencode_text(stdout: &str) -> String {
    stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|j| j["type"] == "text")
        .filter_map(|j| j["part"]["text"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strips the `<think>…</think>` blocks some models (Qwen3, DeepSeek-R1) write.
#[derive(Default)]
struct ThinkFilter {
    buf: String,
    thinking: bool,
}

impl ThinkFilter {
    fn push(&mut self, s: &str) -> String {
        self.buf.push_str(s);
        let mut out = String::new();
        loop {
            if self.thinking {
                match self.buf.find("</think>") {
                    Some(i) => {
                        self.buf.drain(..i + "</think>".len());
                        self.thinking = false;
                    }
                    None => return out,
                }
            } else {
                match self.buf.find("<think>") {
                    Some(i) => {
                        out.push_str(&self.buf[..i]);
                        self.buf.drain(..i + "<think>".len());
                        self.thinking = true;
                    }
                    None => {
                        // Keep a possible "<thi" split at the end.
                        let keep = (1.."<think>".len()).rev()
                            .find(|&n| self.buf.ends_with(&"<think>"[..n])).unwrap_or(0);
                        let cut = self.buf.len() - keep;
                        out.push_str(&self.buf[..cut]);
                        self.buf.drain(..cut);
                        return out;
                    }
                }
            }
        }
    }
}

impl Llm {
    fn chat(&self, system: &str, user: &str, mut on_delta: impl FnMut(&str)) -> Result<String, String> {
        if self.provider != Summaries::Local {
            let text = self.run_cli(system, user)?;
            on_delta(&text);
            return Ok(text);
        }
        self.chat_local(system, user, on_delta)
    }

    /// Runs the provider's CLI once, with the prompt on stdin, in an empty folder and without
    /// tools, and returns its answer.
    fn run_cli(&self, system: &str, user: &str) -> Result<String, String> {
        let name = self.provider.label();
        let binary = self.provider.binary().unwrap_or_default();
        let exe = find_binary(binary).ok_or_else(|| format!("{name} isn't installed. {}", self.provider.install_hint()))?;
        let dir = std::env::temp_dir().join(format!("hushnote-{binary}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let answer_file = dir.join("answer.md");
        let mut cmd = Command::new(exe);
        cmd.current_dir(&dir);
        let model = self.cli_model.trim();
        let stdin = match self.provider {
            Summaries::Claude => {
                cmd.args(["-p", "--system-prompt", system, "--tools", "", "--no-session-persistence", "--setting-sources", ""]);
                cmd.args(["--output-format", "text"]);
                if !model.is_empty() {
                    cmd.args(["--model", model]);
                }
                user.to_string()
            }
            Summaries::Codex => {
                // The user's config can carry hooks, MCP servers or a model their plan lacks;
                // a summary needs none of it.
                cmd.args(["exec", "--skip-git-repo-check", "--ephemeral", "--ignore-user-config", "--sandbox", "read-only"]);
                cmd.arg("-o").arg(&answer_file);
                if !model.is_empty() {
                    cmd.args(["-m", model]);
                }
                cmd.arg("-");
                format!("{system}\n\n{user}")
            }
            Summaries::OpenCode => {
                cmd.args(["run", "--pure", "--format", "json"]);
                if !model.is_empty() {
                    cmd.args(["-m", model]);
                }
                cmd.arg("Follow the instructions below. Answer with the text only; don't use tools.");
                format!("{system}\n\n{user}")
            }
            Summaries::Local => unreachable!(),
        };
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Couldn't start {binary}: {e}"))?;
        if let Some(mut pipe) = child.stdin.take() {
            let _ = pipe.write_all(stdin.as_bytes());
        }
        let out = child.wait_with_output().map_err(|e| format!("{binary}: {e}"))?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let answer = match self.provider {
            Summaries::Codex => std::fs::read_to_string(&answer_file).unwrap_or_default(),
            Summaries::OpenCode => opencode_text(&stdout),
            _ => stdout.clone(),
        };
        let _ = std::fs::remove_dir_all(&dir);
        let answer = answer.trim().to_string();
        if !out.status.success() || answer.is_empty() {
            let err = String::from_utf8_lossy(&out.stderr);
            let last = err.lines().chain(stdout.lines()).filter(|l| !l.trim().is_empty()).last().unwrap_or("no answer").trim().to_string();
            return Err(format!("{name} couldn't write the summary: {last}"));
        }
        Ok(answer)
    }

    fn chat_local(&self, system: &str, user: &str, mut on_delta: impl FnMut(&str)) -> Result<String, String> {
        let url = format!("{}/chat/completions", self.url.trim_end_matches('/'));
        // The original Qwen3 thinks before answering unless told not to; notes don't need it, and
        // on a CPU it would take minutes. (The 2507 instruct models never think.)
        let hybrid_qwen3 = self.model.starts_with("qwen3") && !self.model.contains("2507");
        let user = if hybrid_qwen3 { format!("{user}\n/no_think") } else { user.to_string() };
        let body = serde_json::json!({
            "model": self.model,
            "stream": true,
            "temperature": 0.3,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
        });
        let response = ureq::post(&url).send_json(&body).map_err(|e| {
            format!(
                "Could not reach the local LLM at {} ({e}). Is Ollama running? Start it with `sudo systemctl enable --now ollama`, then download {} from Home.",
                self.url, self.model
            )
        })?;
        let reader = BufReader::new(response.into_body().into_reader());
        let mut filter = ThinkFilter::default();
        let mut full = String::new();
        for line in reader.lines() {
            let line = line.map_err(|e| format!("response cut off: {e}"))?;
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else { continue };
            if let Some(delta) = json["choices"][0]["delta"]["content"].as_str() {
                let clean = filter.push(delta);
                if !clean.is_empty() {
                    full.push_str(&clean);
                    on_delta(&clean);
                }
            }
        }
        Ok(full.trim().to_string())
    }

    /// Writes the final notes; `on_delta` receives the text as it arrives.
    pub fn summarize(&self, notes: &str, lines: &[Line], on_delta: impl FnMut(&str)) -> Result<String, String> {
        let transcript = transcript_text(lines);
        if transcript.trim().is_empty() && notes.trim().is_empty() {
            return Err("There is no transcript and no notes to summarize.".into());
        }
        let size = if self.provider == Summaries::Local { PART_CHARS } else { CLI_PART_CHARS };
        let pieces = parts(&transcript, size);
        let material = if pieces.len() <= 1 {
            format!("Transcript:\n{transcript}")
        } else {
            let mut partial = String::new();
            for (i, piece) in pieces.iter().enumerate() {
                let prompt = format!(
                    "This is part {} of {} of a meeting. Extract the key points, decisions and \
                     action items (with owners) as bullets, in the meeting's language. Bullets only.\n\n{piece}",
                    i + 1,
                    pieces.len()
                );
                let summary = self.chat(SYSTEM, &prompt, |_| {})?;
                partial.push_str(&format!("### Part {}\n{summary}\n\n", i + 1));
            }
            format!("Partial notes of the meeting, in order:\n{partial}")
        };
        let notes = if notes.trim().is_empty() { "(the user took no notes)".to_string() } else { notes.to_string() };
        let prompt = format!("The user's notes:\n{notes}\n\n{material}\n\n{FORMAT}");
        self.chat(SYSTEM, &prompt, on_delta)
    }

    /// A short title for the meeting.
    pub fn title(&self, summary: &str) -> Result<String, String> {
        let prompt = format!("Write a 3 to 6 word title for this meeting, in the meeting's language, without quotes or a final period:\n\n{summary}");
        let t = self.chat(SYSTEM, &prompt, |_| {})?;
        Ok(t.lines().next().unwrap_or("").trim_matches(['"', '#', ' ', '*']).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_reasoning_even_when_split() {
        let mut f = ThinkFilter::default();
        let mut out = String::new();
        for piece in ["Hello <th", "ink>thinking", " hard</thi", "nk> world"] {
            out.push_str(&f.push(piece));
        }
        assert_eq!(out, "Hello  world");
    }

    #[test]
    fn splits_on_whole_lines() {
        let line = "x".repeat(100);
        let text: String = (0..200).map(|_| format!("{line}\n")).collect();
        let p = parts(&text, PART_CHARS);
        assert!(p.len() >= 2);
        assert!(p.iter().all(|s| s.len() <= PART_CHARS + 101));
        assert_eq!(p.concat(), text);
    }

    #[test]
    fn opencode_answer_is_its_text_events() {
        let text = serde_json::json!({"type": "text", "part": {"type": "text", "text": "## Decisions\n- Ship it"}});
        let out = format!("{{\"type\":\"step_start\",\"part\":{{}}}}\n{text}\n{{\"type\":\"step_finish\"}}");
        assert_eq!(opencode_text(&out), "## Decisions\n- Ship it");
    }

    /// Writes a real summary through each installed CLI (`HUSHNOTE_CLI=claude,codex,opencode`).
    /// `cargo test cli_summaries -- --ignored --nocapture`
    #[test]
    #[ignore = "uses the network and the user's accounts"]
    fn cli_summaries() {
        let lines = vec![
            Line { speaker: crate::audio::Speaker::Me, t_ms: 0, text: "Bueno, decidimos lanzar la versión dos el jueves.".into() },
            Line { speaker: crate::audio::Speaker::Them, t_ms: 4_000, text: "Perfecto. Yo, Ana, preparo las notas de la versión para el miércoles.".into() },
            Line { speaker: crate::audio::Speaker::Me, t_ms: 9_000, text: "¿Y quién avisa a soporte? Eso no lo sabemos todavía.".into() },
        ];
        let wanted = std::env::var("HUSHNOTE_CLI").unwrap_or_else(|_| "claude,codex,opencode".into());
        for provider in Summaries::ALL.into_iter().filter(|p| wanted.contains(&p.label().to_lowercase())) {
            let llm = Llm { provider, url: String::new(), model: String::new(), cli_model: String::new() };
            let start = std::time::Instant::now();
            let summary = llm.summarize("", &lines, |_| {}).unwrap_or_else(|e| panic!("{e}"));
            let title = llm.title(&summary).unwrap();
            println!("=== {} ({:.1} s) · {title}\n{summary}\n", provider.label(), start.elapsed().as_secs_f32());
            assert!(summary.contains("- [ ]") && summary.matches("## ").count() == 3, "{summary}");
        }
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(65_000), "01:05");
        assert_eq!(clock(3_725_000), "1:02:05");
    }
}
