//! Summaries from a local LLM over the OpenAI-compatible API (`/v1/chat/completions`), which
//! Ollama, llama-server (llama.cpp) and LM Studio all speak. Nothing leaves the machine.
//!
//! Ollama uses a short context by default, so a long meeting is summarized in parts and the
//! partial summaries are then merged.

use crate::session::Line;
use std::io::{BufRead, BufReader};

const PART_CHARS: usize = 8_000;

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
    pub url: String,
    pub model: String,
}

pub fn transcript_text(lines: &[Line]) -> String {
    lines.iter().map(|l| format!("[{}] {}: {}\n", clock(l.t_ms), l.speaker.name(), l.text)).collect()
}

pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) }
}

/// Splits the transcript into pieces of whole lines.
fn parts(text: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    for line in text.lines() {
        let last = out.last_mut().unwrap();
        if !last.is_empty() && last.len() + line.len() > PART_CHARS {
            out.push(String::new());
        }
        let last = out.last_mut().unwrap();
        last.push_str(line);
        last.push('\n');
    }
    out
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
        let url = format!("{}/chat/completions", self.url.trim_end_matches('/'));
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
                "Could not reach the local LLM at {} ({e}). Is Ollama running? Try `ollama serve` and `ollama pull {}`.",
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
        let pieces = parts(&transcript);
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
        let p = parts(&text);
        assert!(p.len() >= 2);
        assert!(p.iter().all(|s| s.len() <= PART_CHARS + 101));
        assert_eq!(p.concat(), text);
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(65_000), "01:05");
        assert_eq!(clock(3_725_000), "1:02:05");
    }
}
