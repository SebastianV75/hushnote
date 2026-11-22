//! Summaries. By default from a local LLM over the OpenAI-compatible API
//! (`/v1/chat/completions`), which Ollama, llama-server (llama.cpp) and LM Studio all speak, so
//! nothing leaves the machine. Optionally through a coding agent's CLI the user is already
//! signed in to (Claude Code, Codex, OpenCode), which sends the transcript to that service.
//!
//! Ollama uses a short context by default, so with it a long meeting is summarized in parts and
//! the partial summaries are then merged. The hosted models take a whole meeting at once.

use crate::config::Summaries;
use crate::session::Line;
use crate::store::{Card, Meeting, Mode, Turn};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

const PART_CHARS: usize = 8_000;
/// Far more than any meeting; the hosted models take it in one go.
const CLI_PART_CHARS: usize = 600_000;

const SYSTEM: &str = "You are an assistant that writes clear, concise meeting notes in \
Markdown. Use only what is in the transcript and the user's notes; never invent facts. The \
transcript is automatic and may contain wrong words: interpret them with common sense. \"You\" is \
the user; \"Them\" are the other participants.";

const CLASS_SYSTEM: &str = "You are an assistant that writes clear, accurate class notes for a \
student, in Markdown. Use only what is in the transcript and the student's notes; never invent \
facts, definitions or formulas. The transcript is automatic and may contain wrong words (names of \
terms and symbols especially): interpret them with common sense. \"You\" is the student; \"Them\" \
is the teacher or other students.";

fn system(mode: Mode) -> &'static str {
    match mode {
        Mode::Meeting => SYSTEM,
        Mode::Class => CLASS_SYSTEM,
    }
}

/// The section headings, fixed per language so a small model can't half-translate them.
struct Sections {
    language: &'static str,
    mode: Mode,
    /// The four headings, in the order they are written.
    headings: [&'static str; 4],
    none: &'static str,
    /// How a due day looks, e.g. "Thu or Mar 9".
    due: &'static str,
}

const MEETING_ES: Sections = Sections {
    language: "Spanish",
    mode: Mode::Meeting,
    headings: ["Puntos clave", "Decisiones", "Pendientes", "Preguntas abiertas"],
    none: "Ninguno",
    due: "jue or 9 mar",
};

const MEETING_EN: Sections = Sections {
    language: "English",
    mode: Mode::Meeting,
    headings: ["Key points", "Decisions", "Action items", "Open questions"],
    none: "None",
    due: "Thu or Mar 9",
};

const CLASS_ES: Sections = Sections {
    language: "Spanish",
    mode: Mode::Class,
    headings: ["Temas vistos", "Conceptos clave y definiciones", "Fórmulas o ejemplos", "Tareas y fechas de entrega"],
    none: "Ninguno",
    due: "jue or 9 mar",
};

const CLASS_EN: Sections = Sections {
    language: "English",
    mode: Mode::Class,
    headings: ["Topics covered", "Key concepts and definitions", "Formulas or examples", "Tasks and due dates"],
    none: "None",
    due: "Thu or Mar 9",
};

fn sections(language: &str, mode: Mode) -> &'static Sections {
    match (language == "en", mode) {
        (true, Mode::Meeting) => &MEETING_EN,
        (false, Mode::Meeting) => &MEETING_ES,
        (true, Mode::Class) => &CLASS_EN,
        (false, Mode::Class) => &CLASS_ES,
    }
}

/// What the notes must look like, with the headings written out in the session's language.
fn format(s: &Sections) -> String {
    let [h0, h1, h2, h3] = s.headings;
    let (none, due) = (s.none, s.due);
    let task_rule = format!(
        "a task list, one line each: `- [ ] Owner does the task · due`. Add ` · due` only when a day \
         or date was said, as a short word such as {due}. \"{none}\" if there were none"
    );
    let body = match s.mode {
        Mode::Meeting => format!(
            "## {h0}\n(3 to 6 bullets with the most important things that were said: facts, requirements, \
             explanations, figures. Not the decisions or the tasks)\n\
             ## {h1}\n(bullets with what was decided or agreed; \"{none}\" if nothing was)\n\
             ## {h2}\n({task_rule})\n\
             ## {h3}\n(bullets; \"{none}\" if there were none)\n\
             Put names, dates, amounts and product names in `inline code` sparingly, only when it helps \
             scanning. If the user took notes, honour them: they are what matters most to the user, so \
             expand them with details from the transcript."
        ),
        Mode::Class => format!(
            "## {h0}\n(bullets with the subjects the class went through, in the order they came up)\n\
             ## {h1}\n(one bullet per concept: `**Term**: its definition or explanation`, as the teacher \
             gave it)\n\
             ## {h2}\n(the formulas, rules and worked examples that were shown; write formulas in `inline \
             code`; \"{none}\" if there were none)\n\
             ## {h3}\n({task_rule}; include homework, readings, exams and projects)\n\
             If the student took notes, honour them: they are what matters most, so expand them with \
             details from the transcript."
        ),
    };
    format!(
        "Write the notes in {lang}, with exactly these four sections and these exact headings:\n{body} \
         Add nothing before or after the sections.",
        lang = s.language,
    )
}

pub struct Llm {
    pub provider: Summaries,
    /// The local server and model.
    pub url: String,
    pub model: String,
    /// The model for a CLI provider; empty = the CLI's own default.
    pub cli_model: String,
    /// The meeting's language, "es" or "en": the notes are written in it.
    pub language: String,
    /// Whether the notes are for a meeting or a class.
    pub mode: Mode,
}

impl Llm {
    pub fn from_config(c: &crate::config::Config) -> Llm {
        Llm {
            provider: c.summaries,
            url: c.llm_url.clone(),
            model: c.llm_model.clone(),
            cli_model: c.summary_cli_model.clone(),
            language: c.language.clone(),
            mode: Mode::Meeting,
        }
    }

    pub fn with_mode(mut self, mode: Mode) -> Llm {
        self.mode = mode;
        self
    }

    /// Who writes the summary, for "Summary by …": "Qwen3 4B", "Claude", "Claude (sonnet)".
    pub fn by(&self) -> String {
        match self.provider {
            Summaries::Local => model_name(&self.model),
            p if self.cli_model.is_empty() => p.label().to_string(),
            p => format!("{} ({})", p.label(), self.cli_model),
        }
    }
}

/// "qwen3:4b-instruct-2507-q4_K_M" → "Qwen3 4B": the family and the size of an Ollama tag.
pub fn model_name(tag: &str) -> String {
    let (family, variant) = tag.split_once(':').unwrap_or((tag, ""));
    let mut name: String = family.chars().take(1).flat_map(char::to_uppercase).chain(family.chars().skip(1)).collect();
    let size = variant.split('-').next().unwrap_or("");
    if !size.is_empty() && size != "latest" {
        name.push(' ');
        name.push_str(&size.to_uppercase());
    }
    name
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
        let sections = sections(&self.language, self.mode);
        let size = if self.provider == Summaries::Local { PART_CHARS } else { CLI_PART_CHARS };
        let pieces = parts(&transcript, size);
        let material = if pieces.len() <= 1 {
            format!("Transcript:\n{transcript}")
        } else {
            let mut partial = String::new();
            for (i, piece) in pieces.iter().enumerate() {
                let (what, extract) = match self.mode {
                    Mode::Meeting => ("meeting", "the key points, decisions and action items (with owners)"),
                    Mode::Class => (
                        "class",
                        "the topics, every concept with its definition, the formulas and examples, \
                         and the tasks with their due dates",
                    ),
                };
                let prompt = format!(
                    "This is part {} of {} of a {what}. Extract {extract} as bullets, in {}. \
                     Bullets only.\n\n{piece}",
                    i + 1,
                    pieces.len(),
                    sections.language
                );
                let summary = self.chat(system(self.mode), &prompt, |_| {})?;
                partial.push_str(&format!("### Part {}\n{summary}\n\n", i + 1));
            }
            let what = if self.mode == Mode::Class { "class" } else { "meeting" };
            format!("Partial notes of the {what}, in order:\n{partial}")
        };
        let notes = if notes.trim().is_empty() { "(the user took no notes)".to_string() } else { notes.to_string() };
        let prompt = format!("The user's notes:\n{notes}\n\n{material}\n\n{}", format(sections));
        self.chat(system(self.mode), &prompt, on_delta)
    }

    /// A short title for the meeting.
    pub fn title(&self, summary: &str) -> Result<String, String> {
        let what = if self.mode == Mode::Class { "class" } else { "meeting" };
        let prompt = format!(
            "Write a 3 to 6 word title for this {what}, in {}, without quotes or a final period:\n\n{summary}",
            sections(&self.language, self.mode).language
        );
        let t = self.chat(system(self.mode), &prompt, |_| {})?;
        Ok(t.lines().next().unwrap_or("").trim_matches(['"', '#', ' ', '*']).to_string())
    }

    /// 5 to 10 review questions with answers, from the class summary (or, with no summary
    /// yet, the start of the transcript) and the student's notes.
    pub fn review(&self, summary: &str, notes: &str, lines: &[Line]) -> Result<Vec<Card>, String> {
        let material = if summary.trim().is_empty() {
            let size = if self.provider == Summaries::Local { PART_CHARS } else { CLI_PART_CHARS };
            let t = parts(&transcript_text(lines), size).swap_remove(0);
            if t.trim().is_empty() && notes.trim().is_empty() {
                return Err("There is no summary, transcript or notes to write review questions from.".into());
            }
            format!("Transcript of the class (automatic, may contain wrong words):\n{t}")
        } else {
            format!("Summary of the class:\n{}", summary.trim())
        };
        let answer = self.chat(system(self.mode), &review_prompt(&material, notes, &self.language), |_| {})?;
        let cards = parse_review(&answer);
        if cards.is_empty() {
            return Err("The model did not return usable questions. Try again, or pick Claude or Codex.".into());
        }
        Ok(cards)
    }
}

/// Characters of a whole "Ask your notes" prompt (material, chat history and question) that the
/// local model is given: about two parts' worth. The hosted models use the large limit.
pub const LOCAL_ASK_CHARS: usize = PART_CHARS * 2;
/// Earlier turns of the chat sent along, so follow-up questions work.
pub const HISTORY_TURNS: usize = 6;
/// An earlier answer is cut to this many characters when it goes back in the prompt.
const HISTORY_ANSWER_CHARS: usize = 1_200;
/// Room kept for the instructions and the question's framing.
const ASK_OVERHEAD: usize = 600;

/// What a question is asked about.
pub enum Scope<'a> {
    /// The open session: summary, notes, review questions and as much transcript as fits.
    Session(&'a Meeting),
    /// Every class of a subject (the summaries only), in any order.
    Subject { name: &'a str, classes: Vec<&'a Meeting> },
}

/// The first `max` characters, cut at a line break when there is one.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    match cut.rfind('\n') {
        Some(i) if i > 0 => cut[..i].to_string(),
        _ => cut,
    }
}

fn used(s: &str) -> usize {
    s.chars().count()
}

/// The material the answer must come from, at most about `limit` characters.
pub fn ask_context(scope: &Scope, limit: usize) -> String {
    match scope {
        Scope::Session(m) => session_context(m, limit),
        Scope::Subject { name, classes } => subject_context(name, classes, limit),
    }
}

fn session_context(m: &Meeting, limit: usize) -> String {
    let what = if m.mode == Mode::Class { "class" } else { "meeting" };
    let mut out = format!("This {what}: \"{}\" ({})\n", m.title.trim(), m.date);
    // The summary and the notes come first and are never dropped, only cut if they alone are
    // too long.
    let mut left = limit.saturating_sub(used(&out));
    fn add(out: &mut String, left: &mut usize, label: &str, body: &str) {
        if body.trim().is_empty() {
            return;
        }
        let body = clip(body.trim(), left.saturating_sub(used(label) + 4));
        *left = left.saturating_sub(used(&body) + used(label) + 4);
        out.push_str(&format!("\n{label}:\n{body}\n"));
    }
    add(&mut out, &mut left, "Summary", &m.summary);
    add(&mut out, &mut left, "The student's own notes", &m.notes);
    if !m.review.is_empty() {
        let cards: String = m.review.iter().map(|c| format!("Q: {}\nA: {}\n", c.question.trim(), c.answer.trim())).collect();
        if used(&cards) + 100 <= left {
            add(&mut out, &mut left, "Review questions", &cards);
        }
    }
    let transcript = transcript_text(&m.lines);
    if !transcript.trim().is_empty() {
        let label = "Transcript (automatic, may contain wrong words)";
        if used(&transcript) <= left.saturating_sub(used(label) + 4) {
            add(&mut out, &mut left, label, &transcript);
        } else if left > 1_000 {
            let body = clip(&transcript, left - used(label) - 80);
            out.push_str(&format!("\n{label}, cut: only the beginning of the class fits:\n{body}\n"));
        } else {
            out.push_str("\n(The transcript is left out because it does not fit.)\n");
        }
    }
    out
}

fn subject_context(name: &str, classes: &[&Meeting], limit: usize) -> String {
    let mut sorted: Vec<&Meeting> = classes.to_vec();
    sorted.sort_by(|a, b| a.date.cmp(&b.date));
    let blocks: Vec<String> = sorted
        .iter()
        .filter_map(|m| {
            let body = if !m.summary.trim().is_empty() { m.summary.trim() } else { m.notes.trim() };
            (!body.is_empty()).then(|| format!("### {} ({})\n{}\n", m.title.trim(), m.date, body))
        })
        .collect();
    let header = |kept: usize| {
        let mut h = format!("Notes of {} classes of the subject \"{name}\", oldest first. Each starts with the class title and date.\n", kept);
        if kept < blocks.len() {
            h.push_str(&format!("Only the {kept} most recent of {} classes fit here; the older ones are left out.\n", blocks.len()));
        }
        h
    };
    // Newest first until the limit, then back to chronological order.
    let mut kept: Vec<String> = Vec::new();
    let mut size = used(&header(blocks.len())) + 1;
    for b in blocks.iter().rev() {
        if !kept.is_empty() && size + used(b) + 1 > limit {
            break;
        }
        let b = if kept.is_empty() { clip(b, limit.saturating_sub(size)) } else { b.clone() };
        size += used(&b) + 1;
        kept.push(b);
    }
    kept.reverse();
    format!("{}\n{}", header(kept.len()), kept.join("\n"))
}

/// The last turns of the chat as text for the prompt.
pub fn history_text(history: &[Turn]) -> String {
    let from = history.len().saturating_sub(HISTORY_TURNS);
    history[from..]
        .iter()
        .map(|t| format!("Student: {}\nYou: {}\n", t.question.trim(), clip(t.answer.trim(), HISTORY_ANSWER_CHARS)))
        .collect()
}

fn ask_system(language: &str) -> String {
    let lang = sections(language, Mode::Class).language;
    format!(
        "You answer a student's questions about their own class or meeting notes. Answer only with \
         what is in the material you are given; never use outside knowledge and never invent \
         facts. If the material does not cover the question, say so clearly: \"That wasn't \
         covered in these notes.\" (in {lang}). Write the answer in {lang}, in short Markdown. When \
         the material has several classes, say which class (title or date) each point comes from."
    )
}

impl Llm {
    /// The characters the whole prompt may use with this provider.
    fn ask_limit(&self) -> usize {
        if self.provider == Summaries::Local { LOCAL_ASK_CHARS } else { CLI_PART_CHARS }
    }

    /// How many characters the material may take once the chat history and the question are
    /// counted, for `ask_context`.
    pub fn ask_budget(&self, history: &[Turn], question: &str) -> usize {
        let rest = used(&history_text(history)) + used(question) + ASK_OVERHEAD;
        self.ask_limit().saturating_sub(rest).max(1_000)
    }

    /// Answers `question` from `context` (see `ask_context`), after the earlier `history`;
    /// `on_delta` receives the answer as it arrives.
    pub fn ask(&self, context: &str, history: &[Turn], question: &str, on_delta: impl FnMut(&str)) -> Result<String, String> {
        let question = question.trim();
        if question.is_empty() {
            return Err("Type a question first.".into());
        }
        let chat = history_text(history);
        let chat = if chat.is_empty() { String::new() } else { format!("Conversation so far:\n{chat}\n") };
        let prompt = format!("Material:\n{context}\n{chat}The student's question:\n{question}");
        let answer = self.chat(&ask_system(&self.language), &prompt, on_delta)?;
        if answer.trim().is_empty() {
            return Err("The model returned an empty answer. Try again.".into());
        }
        Ok(answer)
    }
}

/// Most cards a review keeps.
pub const MAX_CARDS: usize = 10;

/// The prompt for the review questions: a strict, simple `Q:` / `A:` format.
fn review_prompt(material: &str, notes: &str, language: &str) -> String {
    let lang = sections(language, Mode::Class).language;
    let notes = if notes.trim().is_empty() { "(the student took no notes)" } else { notes.trim() };
    format!(
        "The student's notes:\n{notes}\n\n{material}\n\n\
         Write between 5 and 10 review questions, each with its answer, to help the student study \
         this class. Write them in {lang}. Cover the most important concepts, definitions and \
         formulas. Use only what is in the material above; never invent facts. Each question must \
         make sense on its own. Each answer is one to three sentences.\n\
         Use exactly this format, with the letters Q and A, and nothing before or after:\n\
         Q: the first question\nA: its answer\n\nQ: the second question\nA: its answer"
    )
}

#[derive(PartialEq)]
enum Tag {
    Q,
    A,
}

/// The line without list or numbering marks, bold markers or heading marks.
fn bare(line: &str) -> String {
    let mut l = line.replace("**", "").replace("__", "");
    loop {
        let t = l.trim_start();
        let digits = t.chars().take_while(char::is_ascii_digit).count();
        let next = if let Some(r) = t.strip_prefix(['-', '*', '•', '#', '>']) {
            r
        } else if digits > 0 && t[digits..].starts_with(['.', ')']) {
            &t[digits + 1..]
        } else {
            break;
        };
        l = next.to_string();
    }
    l.trim().to_string()
}

/// `Q: text` or `A: text`, also as Q1:, P:/R:, Pregunta:/Respuesta:, Question:/Answer:.
fn tagged(line: &str) -> Option<(Tag, String)> {
    let l = bare(line);
    let (label, rest) = l.split_once(':')?;
    let label = label.trim().to_lowercase();
    let label = label.trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ');
    match label {
        "q" | "p" | "question" | "pregunta" => Some((Tag::Q, rest.trim().to_string())),
        "a" | "r" | "answer" | "respuesta" => Some((Tag::A, rest.trim().to_string())),
        _ => None,
    }
}

/// Reads the model's `Q:` / `A:` blocks into cards. Forgiving with numbering, markdown,
/// Spanish labels, multi-line answers and chatter around the list; drops cards that lack
/// a question or an answer and keeps at most [`MAX_CARDS`].
pub fn parse_review(text: &str) -> Vec<Card> {
    let mut cards: Vec<Card> = Vec::new();
    let mut cur: Option<(String, Option<String>)> = None; // question, answer
    let mut blank = false;
    fn close(cur: &mut Option<(String, Option<String>)>, cards: &mut Vec<Card>) {
        if let Some((q, Some(a))) = cur.take() {
            if !q.trim().is_empty() && !a.trim().is_empty() {
                cards.push(Card { question: q.trim().to_string(), answer: a.trim().to_string() });
            }
        }
    }
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            blank = true;
            continue;
        }
        if line.chars().all(|c| matches!(c, '-' | '*' | '_' | '=' | ' ')) {
            continue; // a separator
        }
        match tagged(line) {
            Some((Tag::Q, rest)) => {
                close(&mut cur, &mut cards);
                cur = Some((rest, None));
            }
            Some((Tag::A, rest)) => {
                if let Some((_, a)) = &mut cur {
                    if a.is_none() {
                        *a = Some(rest);
                    } else {
                        a.as_mut().unwrap().push_str(&format!("\n{rest}"));
                    }
                }
            }
            None => {
                // After a blank line only a list item or an indented line continues the block;
                // a plain paragraph is the model talking to the reader.
                let listed = bare(line) != line.replace("**", "");
                if blank && !listed && !raw.starts_with([' ', '\t']) {
                    close(&mut cur, &mut cards);
                } else if let Some((q, a)) = &mut cur {
                    let field = a.as_mut().unwrap_or(q);
                    if !field.is_empty() {
                        field.push('\n');
                    }
                    field.push_str(line);
                }
            }
        }
        blank = false;
    }
    close(&mut cur, &mut cards);
    cards.truncate(MAX_CARDS);
    cards
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
            let llm = Llm { provider, url: String::new(), model: String::new(), cli_model: String::new(), language: "es".into(), mode: Mode::Meeting };
            let start = std::time::Instant::now();
            let summary = llm.summarize("", &lines, |_| {}).unwrap_or_else(|e| panic!("{e}"));
            let title = llm.title(&summary).unwrap();
            println!("=== {} ({:.1} s) · {title}\n{summary}\n", provider.label(), start.elapsed().as_secs_f32());
            assert!(summary.contains("- [ ]") && summary.matches("## ").count() == 4, "{summary}");
            assert!(["## Puntos clave", "## Decisiones", "## Pendientes", "## Preguntas abiertas"].iter().all(|h| summary.contains(h)), "{summary}");
        }
    }

    #[test]
    fn names_the_summary_model() {
        assert_eq!(model_name("qwen3:4b-instruct-2507-q4_K_M"), "Qwen3 4B");
        assert_eq!(model_name("llama3.2:3b"), "Llama3.2 3B");
        assert_eq!(model_name("mistral"), "Mistral");
    }

    #[test]
    fn headings_follow_the_language() {
        let es = format(sections("es", Mode::Meeting));
        assert!(es.contains("## Pendientes") && es.contains("in Spanish") && !es.contains("## Action items"));
        assert!(format(sections("en", Mode::Meeting)).contains("## Action items"));
    }

    #[test]
    fn meeting_sections_are_unchanged() {
        assert_eq!(sections("en", Mode::Meeting).headings, ["Key points", "Decisions", "Action items", "Open questions"]);
        let p = format(sections("en", Mode::Meeting));
        assert_eq!(p.matches("\n## ").count(), 4);
        assert!(!p.contains("Topics covered"));
    }

    #[test]
    fn class_sections_in_both_languages() {
        let en = sections("en", Mode::Class);
        assert_eq!(en.headings, ["Topics covered", "Key concepts and definitions", "Formulas or examples", "Tasks and due dates"]);
        let p = format(en);
        for h in en.headings {
            assert!(p.contains(&format!("## {h}\n")), "{h}");
        }
        assert_eq!(p.matches("\n## ").count(), 4);
        assert!(p.contains("- [ ] Owner does the task · due"), "class tasks reuse the task format");
        assert!(!p.contains("Decisions") && !p.contains("Open questions"));

        let es = sections("es", Mode::Class);
        assert_eq!(es.headings[3], "Tareas y fechas de entrega");
        let p = format(es);
        assert!(p.contains("in Spanish") && p.contains("## Temas vistos") && !p.contains("## Topics covered"));
    }

    #[test]
    fn parse_review_reads_a_clean_answer() {
        let c = parse_review("Q: What is a limit?\nA: The value f(x) approaches.\n\nQ: And a derivative?\nA: A rate of change.");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], Card { question: "What is a limit?".into(), answer: "The value f(x) approaches.".into() });
        assert_eq!(c[1].answer, "A rate of change.");
    }

    #[test]
    fn parse_review_survives_messy_output() {
        let messy = "A: stray answer\nSure! Here are your questions:\n\n\
            1. **Q:** What is entropy?\n   **A:** A measure of disorder.\n   It never decreases in a closed system.\n\n\
            2) Pregunta: ¿Qué es la energía?\n   Respuesta: La capacidad de hacer trabajo.\n\n\
            ---\n\
            - P: ¿Qué dice la segunda ley?\n- R: La entropía no disminuye.\n\n\
            **Question 4**: Define heat\n**Answer**: Energy in transit.\n\n\
            Q: Incomplete question with no answer\n\n\
            Q: List answer?\nA: Three things:\n- one\n\n- two\n\nHope this helps! Good luck.";
        let c = parse_review(messy);
        let qs: Vec<&str> = c.iter().map(|c| c.question.as_str()).collect();
        assert_eq!(qs, ["What is entropy?", "¿Qué es la energía?", "¿Qué dice la segunda ley?", "Define heat", "List answer?"]);
        assert_eq!(c[0].answer, "A measure of disorder.\nIt never decreases in a closed system.");
        assert_eq!(c[1].answer, "La capacidad de hacer trabajo.");
        assert_eq!(c[2].answer, "La entropía no disminuye.");
        assert_eq!(c[3].answer, "Energy in transit.");
        assert_eq!(c[4].answer, "Three things:\n- one\n- two", "the closing chatter is not part of the answer");
    }

    #[test]
    fn parse_review_drops_incomplete_cards_and_caps_at_ten() {
        assert!(parse_review("Q: Only a question\nQ: Another\nA:\n").is_empty());
        assert!(parse_review("A: no question here").is_empty());
        assert!(parse_review("").is_empty());
        assert!(parse_review("I could not write questions.").is_empty());
        let many: String = (1..=14).map(|i| format!("Q: q{i}?\nA: a{i}\n\n")).collect();
        let c = parse_review(&many);
        assert_eq!(c.len(), MAX_CARDS);
        assert_eq!(c[9].question, "q10?");
    }

    #[test]
    fn the_review_prompt_asks_for_five_to_ten_questions_in_the_language() {
        let es = review_prompt("Summary of the class:\n- x", "", "es");
        assert!(es.contains("between 5 and 10 review questions") && es.contains("in Spanish"));
        assert!(es.contains("Q: the first question\nA: its answer") && es.contains("(the student took no notes)"));
        let en = review_prompt("Summary of the class:\n- x", "my note", "en");
        assert!(en.contains("in English") && !en.contains("Spanish") && en.contains("my note"));
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(65_000), "01:05");
        assert_eq!(clock(3_725_000), "1:02:05");
    }

    fn class(title: &str, date: &str, summary: &str) -> Meeting {
        Meeting { title: title.into(), date: date.into(), summary: summary.into(), mode: Mode::Class, ..Default::default() }
    }

    #[test]
    fn session_context_has_summary_notes_and_review_then_the_transcript() {
        let mut m = class("Limits", "2026-11-03 10:00", "SUMMARY-TEXT");
        m.notes = "MY-NOTE".into();
        m.review = vec![Card { question: "What is a limit?".into(), answer: "A value.".into() }];
        m.lines = vec![Line { speaker: crate::audio::Speaker::Me, t_ms: 1000, text: "spoken words".into() }];
        let c = ask_context(&Scope::Session(&m), CLI_PART_CHARS);
        for part in ["Limits", "SUMMARY-TEXT", "MY-NOTE", "What is a limit?", "spoken words"] {
            assert!(c.contains(part), "missing {part}");
        }
        assert!(c.find("SUMMARY-TEXT") < c.find("spoken words"));
    }

    #[test]
    fn a_long_transcript_is_cut_for_the_local_model_but_not_for_the_cli() {
        let mut m = class("Limits", "2026-11-03 10:00", "SUMMARY-TEXT");
        m.notes = "MY-NOTE".into();
        m.lines = (0..2000).map(|i| Line { speaker: crate::audio::Speaker::Them, t_ms: i * 1000, text: format!("sentence number {i}") }).collect();
        let local = ask_context(&Scope::Session(&m), LOCAL_ASK_CHARS);
        assert!(used(&local) <= LOCAL_ASK_CHARS, "{}", used(&local));
        assert!(local.contains("SUMMARY-TEXT") && local.contains("MY-NOTE"));
        assert!(local.contains("sentence number 0") && !local.contains("sentence number 1999") && local.contains("cut"));
        let cli = ask_context(&Scope::Session(&m), CLI_PART_CHARS);
        assert!(cli.contains("sentence number 1999") && !cli.contains("cut"));
        // With almost no room the transcript is dropped, the summary and notes stay.
        let tiny = ask_context(&Scope::Session(&m), 300);
        assert!(tiny.contains("SUMMARY-TEXT") && tiny.contains("MY-NOTE") && !tiny.contains("sentence number 0"));
    }

    #[test]
    fn subject_context_is_chronological_without_transcripts() {
        let mut a = class("Second", "2026-11-10 10:00", "sum two");
        a.lines = vec![Line { speaker: crate::audio::Speaker::Me, t_ms: 0, text: "TRANSCRIPT-WORDS".into() }];
        let b = class("First", "2026-11-03 10:00", "sum one");
        let c = ask_context(&Scope::Subject { name: "Calculus I", classes: vec![&a, &b] }, CLI_PART_CHARS);
        assert!(c.find("First (2026-11-03 10:00)").unwrap() < c.find("Second (2026-11-10 10:00)").unwrap());
        assert!(c.contains("Calculus I") && !c.contains("TRANSCRIPT-WORDS") && !c.contains("left out"));
    }

    #[test]
    fn subject_context_keeps_the_most_recent_classes_that_fit() {
        let big = "x".repeat(600);
        let ms: Vec<Meeting> = (1..=5).map(|d| class(&format!("Class {d}"), &format!("2026-11-0{d} 10:00"), &big)).collect();
        let c = ask_context(&Scope::Subject { name: "Physics", classes: ms.iter().collect() }, 1_900);
        assert!(c.contains("most recent") && c.contains("left out"));
        assert!(c.contains("Class 5") && c.contains("Class 4") && !c.contains("Class 1"));
        assert!(c.find("Class 4") < c.find("Class 5"));
        assert!(used(&c) <= 1_900);
    }

    #[test]
    fn only_the_last_turns_go_back_in_the_prompt() {
        let turns: Vec<Turn> = (0..10).map(|i| Turn { question: format!("question {i}"), answer: "a".repeat(5_000), scope: String::new() }).collect();
        let h = history_text(&turns);
        assert!(!h.contains("question 3") && h.contains("question 4") && h.contains("question 9"));
        assert!(used(&h) < HISTORY_TURNS * (HISTORY_ANSWER_CHARS + 60));
        let local = Llm { provider: Summaries::Local, url: String::new(), model: String::new(), cli_model: String::new(), language: "en".into(), mode: Mode::Class };
        assert!(local.ask_budget(&turns, "q") < LOCAL_ASK_CHARS - used(&h));
        assert!(ask_system("es").contains("Spanish") || ask_system("es").contains("español") || ask_system("es").contains("wasn't"));
    }
}
