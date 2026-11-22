//! Each meeting is a Markdown file in the notes folder, readable in any editor (Obsidian,
//! nvim…). HTML comments mark the sections so the file can be read back.

use crate::audio::Speaker;
use crate::llm::clock;
use crate::session::Line;
use std::path::{Path, PathBuf};

/// What the session is: a meeting (decisions, action items) or a class (concepts, tasks).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Meeting,
    Class,
}

impl Mode {
    /// The value in the file's front matter.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Meeting => "meeting",
            Mode::Class => "class",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Meeting => "Meeting",
            Mode::Class => "Class",
        }
    }

    /// Anything unknown, or missing in files from before classes existed, is a meeting.
    pub fn parse(s: &str) -> Mode {
        if s.trim().eq_ignore_ascii_case("class") { Mode::Class } else { Mode::Meeting }
    }

    pub fn next(self) -> Mode {
        match self {
            Mode::Meeting => Mode::Class,
            Mode::Class => Mode::Meeting,
        }
    }
}

/// A review question and its answer (a flashcard).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Card {
    pub question: String,
    pub answer: String,
}

/// One exchange of "Ask your notes".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    pub question: String,
    pub answer: String,
    /// The subject when the question was asked of all its classes; empty for "this class".
    pub scope: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meeting {
    pub file: PathBuf,
    pub title: String,
    pub mode: Mode,
    /// The course or topic this session belongs to ("Calculus I"); empty when not set.
    pub subject: String,
    /// "YYYY-MM-DD HH:MM"
    pub date: String,
    pub duration_ms: u64,
    pub notes: String,
    pub summary: String,
    /// Who wrote the summary: "Qwen3 4B", "Claude"…
    pub summary_by: String,
    /// Review questions for a class; empty until they are generated.
    pub review: Vec<Card>,
    /// The questions the user asked about the notes, oldest first.
    pub questions: Vec<Turn>,
    pub lines: Vec<Line>,
}

const NOTES: &str = "<!-- hushnote:notes -->";
const SUMMARY: &str = "<!-- hushnote:summary -->";
const REVIEW: &str = "<!-- hushnote:review -->";
const QUESTIONS: &str = "<!-- hushnote:questions -->";
const TRANSCRIPT: &str = "<!-- hushnote:transcript -->";
/// Every section ends where the next marker present begins.
const MARKERS: [&str; 5] = [NOTES, SUMMARY, REVIEW, QUESTIONS, TRANSCRIPT];
/// Separates the question from its scope in a turn's heading: `### Question · all Calculus I classes`.
const SCOPE_SEP: &str = " · all ";
const SCOPE_END: &str = " classes";

impl Meeting {
    pub fn new(dir: &Path, mode: Mode) -> Meeting {
        let now = chrono::Local::now();
        Meeting {
            file: dir.join(format!("{}.md", now.format("%Y-%m-%d_%H%M%S"))),
            title: format!("{} on {}", mode.label(), now.format("%b %-d, %H:%M")),
            mode,
            date: now.format("%Y-%m-%d %H:%M").to_string(),
            ..Default::default()
        }
    }

    pub fn to_markdown(&self) -> String {
        let mut md = format!("---\ntitle: \"{}\"\ndate: {}\nduration: {}\n", self.title.replace('"', "'"), self.date, clock(self.duration_ms));
        md.push_str(&format!("mode: {}\n", self.mode.as_str()));
        let subject = self.subject.split_whitespace().collect::<Vec<_>>().join(" ");
        if !subject.is_empty() {
            md.push_str(&format!("subject: \"{}\"\n", subject.replace('"', "'")));
        }
        if !self.summary_by.is_empty() {
            md.push_str(&format!("summary_by: {}\n", self.summary_by));
        }
        md.push_str(&format!("---\n\n# {}\n\n", self.title));
        md.push_str(&format!("{NOTES}\n## My notes\n\n{}\n\n", self.notes.trim()));
        md.push_str(&format!("{SUMMARY}\n{}\n\n", self.summary.trim()));
        if !self.review.is_empty() {
            md.push_str(&format!("{REVIEW}\n## Review questions\n\n"));
            for c in &self.review {
                md.push_str(&format!("- **Q:** {}\n  **A:** {}\n", indent(&c.question), indent(&c.answer)));
            }
            md.push('\n');
        }
        if !self.questions.is_empty() {
            md.push_str(&format!("{QUESTIONS}\n## Questions I asked\n\n"));
            for t in &self.questions {
                md.push_str(&format!("### {}\n\n{}\n\n", t.heading(), lower_headings(&t.answer)));
            }
        }
        md.push_str(&format!("{TRANSCRIPT}\n## Transcript\n\n"));
        for l in &self.lines {
            md.push_str(&format!("- **[{}] {}:** {}\n", clock(l.t_ms), l.speaker.name(), l.text));
        }
        md
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = self.file.with_extension("md.tmp");
        std::fs::write(&tmp, self.to_markdown()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.file).map_err(|e| format!("{}: {e}", self.file.display()))
    }

    /// The path the session belongs at: `<notes_dir>/<Subject>/<YYYY-MM-DD> <Title>.md`
    /// (no subject folder without a subject), before any ` (2)` suffix.
    pub fn ideal_path(&self, notes_dir: &Path) -> PathBuf {
        let day: String = self.date.chars().take(10).collect();
        let day = if day.len() == 10 && day.chars().all(|c| c.is_ascii_digit() || c == '-') { day } else { chrono::Local::now().format("%Y-%m-%d").to_string() };
        let title = sanitize_name(&self.title, TITLE_MAX);
        let title = if title.is_empty() { "Untitled".to_string() } else { title };
        subject_dir(notes_dir, &self.subject).join(format!("{day} {title}.md"))
    }

    /// Moves the session to where it belongs ([`Meeting::ideal_path`], made unique against disk
    /// and `taken`). Returns the old path when it moved, `None` when it was already there.
    /// The new file is written completely before the old one is removed; on any error the
    /// session stays at its old path, untouched.
    pub fn relocate(&mut self, notes_dir: &Path, taken: &[PathBuf]) -> Result<Option<PathBuf>, String> {
        let target = unique_path(&self.ideal_path(notes_dir), &self.file, taken);
        if target == self.file {
            return Ok(None);
        }
        let old = std::mem::replace(&mut self.file, target.clone());
        if let Err(e) = self.save() {
            self.file = old;
            let _ = std::fs::remove_file(&target);
            return Err(e);
        }
        match std::fs::remove_file(&old) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                // Don't leave two copies: back out.
                let _ = std::fs::remove_file(&target);
                self.file = old.clone();
                return Err(format!("{}: {e}", old.display()));
            }
        }
        // A subject folder left empty goes away (never the notes folder; fails if not empty).
        if let Some(parent) = old.parent().filter(|p| *p != notes_dir) {
            let _ = std::fs::remove_dir(parent);
        }
        Ok(Some(old))
    }

    pub fn parse(file: &Path, md: &str) -> Meeting {
        let mut m = Meeting { file: file.to_path_buf(), ..Default::default() };
        let mut body = md;
        if let Some(rest) = md.strip_prefix("---\n") {
            if let Some(end) = rest.find("\n---\n") {
                for line in rest[..end].lines() {
                    let Some((k, v)) = line.split_once(':') else { continue };
                    let v = v.trim().trim_matches('"').to_string();
                    match k.trim() {
                        "title" => m.title = v,
                        "date" => m.date = v,
                        "duration" => m.duration_ms = parse_clock(&v).unwrap_or(0),
                        "mode" => m.mode = Mode::parse(&v),
                        "subject" => m.subject = v,
                        "summary_by" => m.summary_by = v,
                        _ => {}
                    }
                }
                body = &rest[end + 5..];
            }
        }
        // A section runs until the next marker of any kind.
        let section = |start: &str| -> String {
            let Some(i) = body.find(start) else { return String::new() };
            let rest = &body[i + start.len()..];
            let j = MARKERS.iter().filter_map(|e| rest.find(e)).min().unwrap_or(rest.len());
            rest[..j].trim().to_string()
        };
        m.notes = section(NOTES).trim_start_matches("## My notes").trim().to_string();
        m.summary = section(SUMMARY);
        m.review = parse_cards(&section(REVIEW));
        m.questions = parse_questions(&section(QUESTIONS));
        for line in section(TRANSCRIPT).lines() {
            // - **[01:05] You:** text
            let Some(rest) = line.strip_prefix("- **[") else { continue };
            let Some((time, rest)) = rest.split_once("] ") else { continue };
            let Some((who, text)) = rest.split_once(":** ") else { continue };
            m.lines.push(Line {
                speaker: Speaker::from_name(who),
                t_ms: parse_clock(time).unwrap_or(0),
                text: text.to_string(),
            });
        }
        if m.title.is_empty() {
            m.title = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        }
        m
    }

    #[cfg(test)]
    pub fn load(file: &Path) -> Option<Meeting> {
        std::fs::read_to_string(file).ok().map(|md| Meeting::parse(file, &md))
    }
}

/// Continuation lines of a multi-line text are indented two spaces, under the bullet; blank
/// lines are dropped so the item stays one block.
fn indent(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n  ")
}

/// Reads back `- **Q:** …` / `  **A:** …` items, with indented continuation lines.
fn parse_cards(section: &str) -> Vec<Card> {
    let mut cards: Vec<Card> = Vec::new();
    let mut in_answer = false;
    for line in section.lines() {
        if let Some(q) = line.strip_prefix("- **Q:**") {
            cards.push(Card { question: q.trim().to_string(), answer: String::new() });
            in_answer = false;
        } else if let Some(a) = line.trim_start().strip_prefix("**A:**").filter(|_| line.starts_with(' ')) {
            if let Some(c) = cards.last_mut() {
                c.answer = a.trim().to_string();
                in_answer = true;
            }
        } else if line.starts_with("  ") && !line.trim().is_empty() {
            if let Some(c) = cards.last_mut() {
                let field = if in_answer { &mut c.answer } else { &mut c.question };
                field.push('\n');
                field.push_str(line.trim());
            }
        }
    }
    cards.retain(|c| !c.question.is_empty() && !c.answer.is_empty());
    cards
}

impl Turn {
    /// The question on one line, plus ` · all <subject> classes` when it was about the subject.
    pub fn heading(&self) -> String {
        let q = self.question.split_whitespace().collect::<Vec<_>>().join(" ");
        let scope = self.scope.split_whitespace().collect::<Vec<_>>().join(" ");
        if scope.is_empty() { q } else { format!("{q}{SCOPE_SEP}{scope}{SCOPE_END}") }
    }
}

/// Headings inside an answer are pushed down to level 4 or deeper, so that the only level-3
/// headings in the section are the turns' own questions.
pub fn lower_headings(answer: &str) -> String {
    answer
        .trim()
        .lines()
        .map(|l| {
            let hashes = l.len() - l.trim_start_matches('#').len();
            if (1..=6).contains(&hashes) && l[hashes..].starts_with(' ') { format!("{}{}", "#".repeat((hashes + 3).min(6)), &l[hashes..]) } else { l.to_string() }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads back the `### question` turns; a turn runs to the next line starting with `### `.
fn parse_questions(section: &str) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut answer = String::new();
    let close = |turns: &mut Vec<Turn>, answer: &mut String| {
        if let Some(t) = turns.last_mut() {
            t.answer = answer.trim().to_string();
        }
        answer.clear();
    };
    for line in section.lines() {
        if let Some(h) = line.strip_prefix("### ") {
            close(&mut turns, &mut answer);
            let h = h.trim();
            let (question, scope) = match h.rsplit_once(SCOPE_SEP) {
                Some((q, s)) if s.ends_with(SCOPE_END) && s.len() > SCOPE_END.len() => (q, &s[..s.len() - SCOPE_END.len()]),
                _ => (h, ""),
            };
            turns.push(Turn { question: question.trim().to_string(), answer: String::new(), scope: scope.trim().to_string() });
        } else if !turns.is_empty() {
            answer.push_str(line);
            answer.push('\n');
        }
    }
    close(&mut turns, &mut answer);
    turns.retain(|t| !t.question.is_empty());
    turns
}

fn parse_clock(s: &str) -> Option<u64> {
    let parts: Vec<u64> = s.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    let secs = parts.iter().fold(0, |acc, p| acc * 60 + p);
    Some(secs * 1000)
}

/// Whether a Markdown file is a Hushnote session: it has one of our section markers, or front
/// matter with a `date:` line. Anything else in the folder (the user's own notes in an
/// Obsidian vault) is not ours.
pub fn is_session(md: &str) -> bool {
    if md.contains("<!-- hushnote:") {
        return true;
    }
    md.strip_prefix("---\n").and_then(|rest| rest.find("\n---\n").map(|end| &rest[..end])).is_some_and(|fm| fm.lines().any(|l| l.trim_start().starts_with("date:")))
}

/// The folder name that holds the exports, which is never a subject folder.
const EXPORTS_DIR: &str = "exports";

/// The saved sessions, newest first by date (ties: the later path first). Reads the notes
/// folder and one level of subfolders (one per subject), skipping `exports/`, hidden folders
/// and files that are not Hushnote sessions.
pub fn list(dir: &Path) -> Vec<Meeting> {
    let mut files: Vec<PathBuf> = Vec::new();
    let collect = |d: &Path, files: &mut Vec<PathBuf>| {
        let Ok(entries) = std::fs::read_dir(d) else { return Vec::new() };
        let mut subdirs = Vec::new();
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if name != EXPORTS_DIR {
                    subdirs.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "md") {
                files.push(path);
            }
        }
        subdirs
    };
    for sub in collect(dir, &mut files) {
        collect(&sub, &mut files);
    }
    let mut found: Vec<Meeting> = files.iter().filter_map(|f| std::fs::read_to_string(f).ok().filter(|md| is_session(md)).map(|md| Meeting::parse(f, &md))).collect();
    found.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.file.cmp(&a.file)));
    found
}

/// A name that is safe as a file or folder name on Linux, macOS and Windows: no
/// `/ \ : * ? " < > |`, no control characters, spaces collapsed, no leading dots (hidden) and
/// no trailing dots or spaces, at most `max` characters. May come back empty.
pub fn sanitize_name(s: &str, max: usize) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control() && !"/\\:*?\"<>|".contains(*c)).collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = collapsed.chars().take(max).collect();
    cut.trim_start_matches('.').trim_end_matches(['.', ' ']).trim_start().to_string()
}

const FOLDER_MAX: usize = 60;
const TITLE_MAX: usize = 80;

/// The folder for a subject: `<notes_dir>/<Subject>`, or the notes folder itself when there is
/// no subject (or it sanitizes to nothing). An existing folder whose name matches loosely
/// (case and spacing ignored, same as `subject_key`) is reused, so "calculus i" and
/// "Calculus I" share one.
pub fn subject_dir(notes_dir: &Path, subject: &str) -> PathBuf {
    let mut name = sanitize_name(subject, FOLDER_MAX);
    if name.is_empty() {
        return notes_dir.to_path_buf();
    }
    if subject_key(&name) == EXPORTS_DIR {
        name.push_str(" notes");
    }
    let key = subject_key(&name);
    let mut existing: Vec<String> = std::fs::read_dir(notes_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && subject_key(n) == key)
        .collect();
    existing.sort();
    notes_dir.join(existing.into_iter().next().unwrap_or(name))
}

/// Picks `path`, or `<stem> (2).md`, `(3)`… when it is taken: by a file on disk or by one of
/// `taken` (other sessions in memory). `current` is the session's own file, which is never a
/// collision with itself.
pub fn unique_path(path: &Path, current: &Path, taken: &[PathBuf]) -> PathBuf {
    let free = |p: &Path| p == current || (!p.exists() && !taken.iter().any(|t| t == p));
    if free(path) {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new(""));
    (2..).map(|n| dir.join(format!("{stem} ({n}).md"))).find(|p| free(p)).unwrap()
}

/// Whether the session at `file` may be moved now: not while it is recording, finishing, or
/// has a summary, review or question running. `busy` lists the files those things work on.
pub fn may_relocate(file: &Path, busy: &[Option<&Path>]) -> bool {
    !busy.iter().flatten().any(|b| *b == file)
}

/// A subject compared loosely: trimmed, inner spaces collapsed, case ignored.
pub fn subject_key(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// What kind of sidebar group a notebook is: a subject, or one of the two catch-alls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    Notebook,
    /// Meetings (no subject).
    Meetings,
    /// Classes without a subject.
    Unsorted,
}

/// A sidebar group: indices into the slice given to [`notebooks`], in display order.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub kind: GroupKind,
    /// The heading: the subject as the most recent session spells it.
    pub title: String,
    /// Stable identity for open/closed state: the `subject_key`, or a reserved value that no
    /// subject key can equal (it starts with a NUL).
    pub key: String,
    pub indices: Vec<usize>,
}

pub const MEETINGS_KEY: &str = "\0meetings";
pub const UNSORTED_KEY: &str = "\0unsorted";

/// Groups the sessions into notebooks.
///
/// - One notebook per subject (matched with `subject_key`: case and spacing ignored), titled
///   with the subject of its most recent session. Notebooks go by their latest activity, the
///   newest first, and inside one the sessions run oldest to newest.
/// - After the notebooks, "Meetings" (sessions with no subject that are meetings) and
///   "Unsorted classes" (classes with no subject), each only when it has something, newest
///   first.
/// - "Recent" is the session date; ties keep the slice order (which is newest first).
pub fn notebooks(meetings: &[Meeting]) -> Vec<Group> {
    let mut recent: Vec<usize> = (0..meetings.len()).collect();
    recent.sort_by(|&a, &b| meetings[b].date.cmp(&meetings[a].date).then(a.cmp(&b)));
    let mut books: Vec<Group> = Vec::new();
    let loose = |kind, title: &str, key: &str| Group { kind, title: title.into(), key: key.into(), indices: Vec::new() };
    let (mut plain, mut unsorted) = (loose(GroupKind::Meetings, "Meetings", MEETINGS_KEY), loose(GroupKind::Unsorted, "Unsorted classes", UNSORTED_KEY));
    for i in recent {
        let m = &meetings[i];
        let key = subject_key(&m.subject);
        if key.is_empty() {
            match m.mode {
                Mode::Class => unsorted.indices.push(i),
                Mode::Meeting => plain.indices.push(i),
            }
        } else if let Some(g) = books.iter_mut().find(|g| g.key == key) {
            g.indices.push(i);
        } else {
            let title = m.subject.split_whitespace().collect::<Vec<_>>().join(" ");
            books.push(Group { kind: GroupKind::Notebook, title, key, indices: vec![i] });
        }
    }
    for g in &mut books {
        g.indices.reverse();
    }
    books.extend([plain, unsorted].into_iter().filter(|g| !g.indices.is_empty()));
    books
}

/// Whether a session belongs to the subject being filtered on (`None` shows everything).
pub fn matches_subject(m: &Meeting, filter: Option<&str>) -> bool {
    filter.is_none_or(|f| subject_key(&m.subject) == subject_key(f))
}

// ---- Notebook covers: previews, tasks, order ---------------------------------------------------

/// A line of Markdown as plain text: no heading marks, list or checkbox markers, emphasis,
/// backticks or link targets.
pub fn strip_markdown(line: &str) -> String {
    let mut l = line.trim();
    while let Some(rest) = l.strip_prefix('#') {
        l = rest;
    }
    let mut l = l.trim_start();
    for marker in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ ", "> "] {
        if let Some(rest) = l.strip_prefix(marker) {
            l = rest.trim_start();
            break;
        }
    }
    // "1. " and "2) "
    let digits = l.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && digits < 4 && (l[digits..].starts_with(". ") || l[digits..].starts_with(") ")) {
        l = l[digits + 2..].trim_start();
    }
    let mut out = String::with_capacity(l.len());
    let mut rest = l;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("**") || rest.starts_with("__") {
            rest = &rest[2..];
        } else if c == '`' {
            rest = &rest[1..];
        } else if c == '[' {
            // [text](target) keeps the text
            if let Some((text, after)) = rest[1..].split_once("](") {
                if let Some((_, tail)) = after.split_once(')') {
                    out.push_str(text);
                    rest = tail;
                    continue;
                }
            }
            out.push(c);
            rest = &rest[1..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Up to `max` short lines that say what a session is about, for the sheets of a notebook:
/// the bullets of the first section of the summary (Topics covered, Key points…) or, without
/// a summary, the start of the notes, or of the transcript. Markdown is stripped.
pub fn preview_lines(m: &Meeting, max: usize) -> Vec<String> {
    let clean = |lines: Vec<String>| -> Vec<String> { lines.into_iter().map(|l| strip_markdown(&l)).filter(|l| !l.is_empty()).take(max).collect() };
    if !m.summary.trim().is_empty() {
        let mut section: Vec<String> = Vec::new();
        let mut started = false;
        for raw in m.summary.lines() {
            let l = raw.trim();
            if l.starts_with('#') {
                if started {
                    break;
                }
                started = true;
                continue;
            }
            started = true;
            if !l.is_empty() {
                section.push(l.to_string());
            }
        }
        let bullets: Vec<String> = section.iter().filter(|l| l.starts_with("- ") || l.starts_with("* ")).cloned().collect();
        let got = clean(if bullets.is_empty() { section } else { bullets });
        if !got.is_empty() {
            return got;
        }
    }
    if !m.notes.trim().is_empty() {
        let got = clean(m.notes.lines().map(str::to_string).collect());
        if !got.is_empty() {
            return got;
        }
    }
    clean(m.lines.iter().map(|l| l.text.clone()).collect())
}

/// A `- [ ] text · due` line of a summary: whether it is done, the text without backticks, and
/// the due day (the part after the last " · ", when it is short).
pub fn parse_task(raw: &str) -> Option<(bool, String, Option<String>)> {
    let l = raw.trim_start();
    let (done, t) = l.strip_prefix("- [ ] ").map(|t| (false, t)).or_else(|| l.strip_prefix("- [x] ").or_else(|| l.strip_prefix("- [X] ")).map(|t| (true, t)))?;
    let (text, due) = match t.rsplit_once(" · ") {
        Some((head, due)) if due.chars().count() <= 14 && !due.trim().is_empty() => (head.to_string(), Some(due.trim().to_string())),
        _ => (t.to_string(), None),
    };
    Some((done, text.replace('`', ""), due))
}

/// An open task of a session, where to find it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTask {
    /// Index of the session in the slice given.
    pub session: usize,
    /// The line of the summary, for ticking it.
    pub line: usize,
    pub text: String,
    pub due: Option<String>,
}

/// The unchecked tasks of the sessions at `indices`, in the order of `indices` and then of the
/// summary.
pub fn open_tasks(meetings: &[Meeting], indices: &[usize]) -> Vec<OpenTask> {
    let mut out = Vec::new();
    for &i in indices {
        let Some(m) = meetings.get(i) else { continue };
        for (line, raw) in m.summary.lines().enumerate() {
            if let Some((false, text, due)) = parse_task(raw) {
                out.push(OpenTask { session: i, line, text, due });
            }
        }
    }
    out
}

/// The sessions at `indices` oldest first (a notebook reads like a notebook). Equal dates keep
/// the reverse of the slice order, which is newest first.
pub fn chronological(meetings: &[Meeting], indices: &[usize]) -> Vec<usize> {
    let mut v = indices.to_vec();
    v.sort_by(|&a, &b| meetings[a].date.cmp(&meetings[b].date).then(b.cmp(&a)));
    v
}

/// The `n` most recent sessions at `indices`, newest first.
pub fn most_recent(meetings: &[Meeting], indices: &[usize], n: usize) -> Vec<usize> {
    let mut v = chronological(meetings, indices);
    v.reverse();
    v.truncate(n);
    v
}

/// "3 classes · 2 due", "6 sessions" or "1 class": what a notebook's card says under its name.
/// Open tasks are counted for notebooks and unsorted classes, not for Meetings.
pub fn group_summary(meetings: &[Meeting], g: &Group) -> String {
    let n = g.indices.len();
    let all_classes = g.indices.iter().all(|&i| meetings[i].mode == Mode::Class);
    let noun = match (all_classes, n) {
        (true, 1) => "class",
        (true, _) => "classes",
        (false, 1) => "session",
        (false, _) => "sessions",
    };
    let mut out = format!("{n} {noun}");
    if g.kind != GroupKind::Meetings {
        let due = open_tasks(meetings, &g.indices).len();
        if due > 0 {
            out.push_str(&format!(" · {due} due"));
        }
    }
    out
}

/// The flashcards of the sessions at `indices` as `(session, card)` pairs: classes oldest
/// first, and each class's cards in their own order.
pub fn review_deck(meetings: &[Meeting], indices: &[usize]) -> Vec<(usize, usize)> {
    chronological(meetings, indices).into_iter().flat_map(|i| (0..meetings[i].review.len()).map(move |c| (i, c))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_markdown() {
        let m = Meeting {
            file: PathBuf::from("/tmp/x.md"),
            title: "Planning \"Q1\"".into(),
            mode: Mode::Class,
            subject: "Calculus I: \"limits\"".into(),
            date: "2026-11-17 16:30".into(),
            duration_ms: 125_000,
            notes: "- review the budget\n- call Ana".into(),
            summary: "## Summary\nWe talked about the budget.\n\n## Action items\n- [ ] Call Ana".into(),
            summary_by: "Claude".into(),
            review: vec![
                Card { question: "What is a limit?".into(), answer: "The value f(x) approaches.".into() },
                Card { question: "Define the derivative:\nuse the limit".into(), answer: "- slope of the tangent\n- rate of change\n\nExample: f(x)=x^2".into() },
            ],
            questions: Vec::new(),
            lines: vec![
                Line { speaker: Speaker::Me, t_ms: 5_000, text: "Hi, shall we start?".into() },
                Line { speaker: Speaker::Them, t_ms: 65_000, text: "Yes: the budget first.".into() },
            ],
        };
        let back = Meeting::parse(&m.file, &m.to_markdown());
        assert_eq!(back.title, "Planning 'Q1'");
        assert_eq!(back.date, m.date);
        assert_eq!(back.duration_ms, m.duration_ms);
        assert_eq!(back.notes, m.notes);
        assert_eq!(back.summary, m.summary);
        assert_eq!(back.summary_by, m.summary_by);
        assert_eq!(back.mode, Mode::Class);
        assert_eq!(back.subject, "Calculus I: 'limits'");
        assert_eq!(back.lines, m.lines);
        // Blank lines inside an answer are dropped; everything else comes back as written.
        assert_eq!(back.review[0], m.review[0]);
        assert_eq!(back.review[1].question, "Define the derivative:\nuse the limit");
        assert_eq!(back.review[1].answer, "- slope of the tangent\n- rate of change\nExample: f(x)=x^2");
        assert!(!back.summary.contains("Review questions"), "the summary stops at the review marker");
    }

    #[test]
    fn the_review_section_is_only_written_when_there_is_one() {
        let mut m = Meeting { title: "T".into(), ..Default::default() };
        assert!(!m.to_markdown().contains("hushnote:review"));
        m.review.push(Card { question: "q".into(), answer: "a".into() });
        let md = m.to_markdown();
        assert!(md.contains("<!-- hushnote:review -->\n## Review questions\n\n- **Q:** q\n  **A:** a\n"));
        assert!(md.find(REVIEW).unwrap() < md.find(TRANSCRIPT).unwrap());
    }

    /// Files saved before modes existed have no `mode:` line and must load as meetings.
    #[test]
    fn old_files_without_a_mode_load_as_meetings() {
        let old = "---\ntitle: \"Weekly sync\"\ndate: 2026-11-17 10:00\nduration: 47:00\nsummary_by: Claude\n---\n\n# Weekly sync\n\n\
                   <!-- hushnote:notes -->\n## My notes\n\nhi\n\n<!-- hushnote:summary -->\n## Key points\n- x\n\n<!-- hushnote:transcript -->\n## Transcript\n\n\
                   - **[00:05] You:** hello\n";
        let m = Meeting::parse(Path::new("/tmp/old.md"), old);
        assert_eq!(m.mode, Mode::Meeting);
        assert_eq!(m.title, "Weekly sync");
        assert_eq!(m.summary_by, "Claude");
        assert_eq!(m.lines.len(), 1);
        assert!(m.review.is_empty());
        assert_eq!(m.subject, "");
        assert_eq!(m.summary, "## Key points\n- x");
        // An unknown value is not an error either.
        assert_eq!(Mode::parse("workshop"), Mode::Meeting);
        assert_eq!(Mode::parse("class"), Mode::Class);
    }

    #[test]
    fn a_meeting_keeps_its_mode_and_default_title() {
        let dir = Path::new("/tmp");
        let class = Meeting::new(dir, Mode::Class);
        assert!(class.title.starts_with("Class on "));
        assert_eq!(Meeting::parse(&class.file, &class.to_markdown()).mode, Mode::Class);
        let meeting = Meeting::new(dir, Mode::Meeting);
        assert!(meeting.title.starts_with("Meeting on "));
        assert_eq!(Meeting::parse(&meeting.file, &meeting.to_markdown()).mode, Mode::Meeting);
    }

    #[test]
    fn the_subject_line_is_only_written_when_set() {
        let mut m = Meeting { title: "T".into(), ..Default::default() };
        assert!(!m.to_markdown().contains("subject:"));
        m.subject = "   ".into();
        assert!(!m.to_markdown().contains("subject:"));
        m.subject = "  Calculus   I ".into();
        assert!(m.to_markdown().contains("\nsubject: \"Calculus I\"\n"));
        assert_eq!(Meeting::parse(Path::new("/tmp/x.md"), &m.to_markdown()).subject, "Calculus I");
    }

    fn session(subject: &str, mode: Mode, date: &str) -> Meeting {
        Meeting { subject: subject.into(), mode, date: date.into(), title: format!("{subject}@{date}"), ..Default::default() }
    }

    fn titles(meetings: &[Meeting], g: &Group) -> Vec<String> {
        g.indices.iter().map(|&i| meetings[i].date.clone()).collect()
    }

    #[test]
    fn notebooks_group_ignoring_case_and_spacing() {
        let all = [
            session("Calculus I", Mode::Class, "2026-11-21 14:00"),
            session("  calculus   i ", Mode::Class, "2026-11-18 09:00"),
            session("Biology 101", Mode::Class, "2026-11-19 11:00"),
        ];
        let groups = notebooks(&all);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].key, "calculus i");
        assert_eq!(groups[0].title, "Calculus I", "the most recent session's spelling, spaces collapsed");
        assert_eq!(groups[0].indices.len(), 2);
        assert_eq!(groups[1].title, "Biology 101");
    }

    #[test]
    fn notebooks_go_by_recent_activity_and_sessions_run_oldest_first() {
        let all = [
            session("Biology 101", Mode::Class, "2026-11-19 11:00"),
            session("Calculus I", Mode::Class, "2026-11-21 14:00"),
            session("Calculus I", Mode::Class, "2026-11-18 09:00"),
            session("Calculus I", Mode::Class, "2026-11-20 09:00"),
            session("Physics", Mode::Class, "2026-11-22 08:00"),
        ];
        let groups = notebooks(&all);
        let order: Vec<_> = groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(order, ["Physics", "Calculus I", "Biology 101"]);
        assert_eq!(titles(&all, &groups[1]), ["2026-11-18 09:00", "2026-11-20 09:00", "2026-11-21 14:00"]);
    }

    #[test]
    fn meetings_and_unsorted_classes_come_last_newest_first() {
        let all = [
            session("", Mode::Meeting, "2026-11-10 10:00"),
            session("", Mode::Class, "2026-11-12 10:00"),
            session("Calculus I", Mode::Class, "2026-11-01 09:00"),
            session("  ", Mode::Meeting, "2026-11-16 15:00"),
            session("", Mode::Class, "2026-11-13 10:00"),
        ];
        let groups = notebooks(&all);
        let kinds: Vec<_> = groups.iter().map(|g| g.kind).collect();
        assert_eq!(kinds, [GroupKind::Notebook, GroupKind::Meetings, GroupKind::Unsorted]);
        assert_eq!(titles(&all, &groups[1]), ["2026-11-16 15:00", "2026-11-10 10:00"]);
        assert_eq!(groups[2].title, "Unsorted classes");
        assert_eq!(titles(&all, &groups[2]), ["2026-11-13 10:00", "2026-11-12 10:00"]);
        // A group only exists when it has something.
        let only = notebooks(&[session("", Mode::Meeting, "2026-11-10 10:00")]);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].key, MEETINGS_KEY);
        assert!(notebooks(&[]).is_empty());
    }

    #[test]
    fn the_subject_filter_ignores_case_and_spacing() {
        let m = Meeting { subject: " Calculus  I".into(), ..Default::default() };
        assert!(matches_subject(&m, None));
        assert!(matches_subject(&m, Some("calculus i")));
        assert!(!matches_subject(&m, Some("Physics")));
        assert!(!matches_subject(&Meeting::default(), Some("Physics")));
    }

    fn turn(q: &str, a: &str, scope: &str) -> Turn {
        Turn { question: q.into(), answer: a.into(), scope: scope.into() }
    }

    #[test]
    fn questions_round_trip_with_markdown_answers() {
        let m = Meeting {
            title: "T".into(),
            summary: "## Key concepts\n- x".into(),
            review: vec![Card { question: "q".into(), answer: "a".into() }],
            questions: vec![
                turn("What is a limit?", "The value f(x) approaches.", ""),
                turn("List the rules", "# Rules\n\n## Sum\n- add\n- more\n\n### Product\n1. multiply\n2. done\n\nText with `code`.", ""),
                turn("Which classes covered it?", "Limits (Nov 3).", "Calculus I"),
            ],
            lines: vec![Line { speaker: Speaker::Me, t_ms: 5_000, text: "hi".into() }],
            ..Default::default()
        };
        let md = m.to_markdown();
        assert!(md.contains("<!-- hushnote:questions -->\n## Questions I asked\n\n### What is a limit?\n\nThe value f(x) approaches.\n\n"));
        assert!(md.contains("### Which classes covered it? · all Calculus I classes\n\nLimits (Nov 3).\n"));
        assert!(md.find(REVIEW).unwrap() < md.find(QUESTIONS).unwrap());
        assert!(md.find(QUESTIONS).unwrap() < md.find(TRANSCRIPT).unwrap());
        let back = Meeting::parse(Path::new("/tmp/x.md"), &md);
        assert_eq!(back.questions.len(), 3);
        assert_eq!(back.questions[0], m.questions[0]);
        assert_eq!(back.questions[2], m.questions[2]);
        // Headings inside an answer are lowered, never cut the turn, and nothing else changes.
        let a = &back.questions[1].answer;
        assert_eq!(back.questions[1].question, "List the rules");
        assert!(a.starts_with("#### Rules\n\n##### Sum\n- add") && a.contains("###### Product\n1. multiply") && a.ends_with("Text with `code`."));
        // The neighbours are not affected.
        assert_eq!(back.summary, m.summary);
        assert_eq!(back.review, m.review);
        assert_eq!(back.lines, m.lines);
        assert!(!back.summary.contains("Questions I asked") && !back.review[0].answer.contains("Questions"));
        // Writing it again gives the same file.
        assert_eq!(back.to_markdown(), Meeting::parse(Path::new("/tmp/x.md"), &back.to_markdown()).to_markdown());
    }

    #[test]
    fn the_questions_section_is_only_written_when_there_are_turns() {
        let mut m = Meeting { title: "T".into(), ..Default::default() };
        assert!(!m.to_markdown().contains("hushnote:questions"));
        m.questions.push(turn("q", "a", ""));
        assert!(m.to_markdown().contains("hushnote:questions"));
    }

    #[test]
    fn old_files_load_with_an_empty_chat_and_summary_stops_at_questions() {
        let old = "---\ntitle: \"X\"\n---\n\n# X\n\n<!-- hushnote:notes -->\n## My notes\n\nn\n\n<!-- hushnote:summary -->\nS\n\n<!-- hushnote:transcript -->\n## Transcript\n\n";
        assert!(Meeting::parse(Path::new("/tmp/o.md"), old).questions.is_empty());
        // Without a review, the summary still ends at the questions.
        let md = "<!-- hushnote:notes -->\nn\n<!-- hushnote:summary -->\nS\n\n<!-- hushnote:questions -->\n## Questions I asked\n\n### q\n\na\n\n<!-- hushnote:transcript -->\n";
        let m = Meeting::parse(Path::new("/tmp/o.md"), md);
        assert_eq!(m.summary, "S");
        assert_eq!(m.questions, vec![turn("q", "a", "")]);
    }

    /// A scratch folder under the system temp dir, removed on drop.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!("hushnote-test-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn class(subject: &str, title: &str, date: &str) -> Meeting {
        Meeting { title: title.into(), subject: subject.into(), mode: Mode::Class, date: date.into(), notes: "n".into(), ..Default::default() }
    }

    #[test]
    fn names_are_sanitized_for_every_platform() {
        assert_eq!(sanitize_name("a/b\\c:d*e?f\"g<h>i|j", 60), "abcdefghij");
        assert_eq!(sanitize_name("  Calculus   I \t\n", 60), "Calculus I");
        assert_eq!(sanitize_name("Intro to C++...  ", 60), "Intro to C++");
        assert_eq!(sanitize_name("..hidden", 60), "hidden");
        assert_eq!(sanitize_name("a\u{0}b\u{7}c", 60), "abc");
        assert_eq!(sanitize_name(&"x".repeat(200), 60).chars().count(), 60);
        assert_eq!(sanitize_name("abc. . .def", 5), "abc", "cut, then trailing dots and spaces trimmed");
        assert_eq!(sanitize_name("", 60), "");
        assert_eq!(sanitize_name("///", 60), "");
        assert_eq!(sanitize_name("Cálculo diferencial", 60), "Cálculo diferencial");
    }

    #[test]
    fn the_ideal_path_has_a_subject_folder_and_a_readable_name() {
        let tmp = Scratch::new("ideal");
        let dir = &tmp.0;
        let m = class("Calculus I: limits", "Limits & \"epsilon\"", "2026-11-17 16:30");
        assert_eq!(m.ideal_path(dir), dir.join("Calculus I limits").join("2026-11-17 Limits & epsilon.md"));
        let none = class("  ", "Weekly sync", "2026-11-17 10:00");
        assert_eq!(none.ideal_path(dir), dir.join("2026-11-17 Weekly sync.md"));
        let blank = class("???", "", "2026-11-17 10:00");
        assert_eq!(blank.ideal_path(dir), dir.join("2026-11-17 Untitled.md"));
        let long = class("", &"t".repeat(300), "2026-11-17 10:00");
        assert_eq!(long.ideal_path(dir).file_name().unwrap().to_string_lossy().chars().count(), "2026-11-17 .md".len() + 80);
        // A subject can't take the exports folder's name.
        assert_ne!(class("Exports", "T", "2026-11-17 10:00").ideal_path(dir).parent().unwrap(), dir.join("exports"));
    }

    #[test]
    fn subjects_that_differ_only_in_case_share_a_folder() {
        let tmp = Scratch::new("case");
        let dir = &tmp.0;
        assert_eq!(subject_dir(dir, "Calculus I"), dir.join("Calculus I"));
        std::fs::create_dir_all(dir.join("Calculus I")).unwrap();
        assert_eq!(subject_dir(dir, "calculus  i"), dir.join("Calculus I"));
        assert_eq!(subject_dir(dir, "CALCULUS I"), dir.join("Calculus I"));
        assert_eq!(subject_dir(dir, "Physics"), dir.join("Physics"));
        assert_eq!(subject_dir(dir, ""), *dir);
    }

    #[test]
    fn relocate_moves_the_file_and_keeps_the_content() {
        let tmp = Scratch::new("move");
        let dir = &tmp.0;
        let mut m = class("Calculus I", "Class on Nov 17, 16:30", "2026-11-17 16:30");
        m.file = dir.join("2026-11-17_163000.md");
        m.summary = "## Key points\n- x".into();
        m.lines = vec![Line { speaker: Speaker::Me, t_ms: 5_000, text: "hi".into() }];
        m.save().unwrap();
        let before = m.clone();
        m.title = "Limits".into();
        let old = m.relocate(dir, &[]).unwrap().expect("it moved");
        assert_eq!(old, dir.join("2026-11-17_163000.md"));
        assert!(!old.exists(), "the old file is gone");
        assert_eq!(m.file, dir.join("Calculus I").join("2026-11-17 Limits.md"));
        let back = Meeting::load(&m.file).unwrap();
        assert_eq!(back.title, "Limits");
        assert_eq!((back.summary.as_str(), back.lines.clone(), back.subject.as_str()), (before.summary.as_str(), before.lines.clone(), "Calculus I"));
        // Already in place: nothing happens.
        assert_eq!(m.relocate(dir, &[]).unwrap(), None);
        // Dropping the subject brings it back to the root and removes the empty folder.
        m.subject.clear();
        m.relocate(dir, &[]).unwrap().unwrap();
        assert_eq!(m.file, dir.join("2026-11-17 Limits.md"));
        assert!(!dir.join("Calculus I").exists());
    }

    #[test]
    fn relocate_never_collides_with_disk_or_memory() {
        let tmp = Scratch::new("collide");
        let dir = &tmp.0;
        let mut a = class("Calculus I", "Limits", "2026-11-17 16:30");
        a.file = dir.join("a.md");
        a.save().unwrap();
        a.relocate(dir, &[]).unwrap();
        let first = dir.join("Calculus I").join("2026-11-17 Limits.md");
        assert_eq!(a.file, first);
        let mut b = class("calculus i", "Limits", "2026-11-17 18:00");
        b.file = dir.join("b.md");
        b.save().unwrap();
        b.relocate(dir, &[]).unwrap();
        assert_eq!(b.file, dir.join("Calculus I").join("2026-11-17 Limits (2).md"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), a.to_markdown(), "the first one is untouched");
        // Another session in memory that has not been written yet also counts.
        let mut c = class("Calculus I", "Limits", "2026-11-17 19:00");
        c.file = dir.join("c.md");
        c.save().unwrap();
        let held = dir.join("Calculus I").join("2026-11-17 Limits (3).md");
        c.relocate(dir, &[held.clone()]).unwrap();
        assert_eq!(c.file, dir.join("Calculus I").join("2026-11-17 Limits (4).md"));
    }

    #[test]
    fn a_failed_relocate_leaves_the_session_where_it_was() {
        let tmp = Scratch::new("fail");
        let dir = &tmp.0;
        let mut m = class("Calculus I", "Limits", "2026-11-17 16:30");
        m.file = dir.join("old.md");
        m.save().unwrap();
        // A plain file where the subject folder should go makes the move impossible.
        std::fs::write(dir.join("Calculus I"), "in the way").unwrap();
        assert!(m.relocate(dir, &[]).is_err());
        assert_eq!(m.file, dir.join("old.md"));
        assert_eq!(Meeting::load(&dir.join("old.md")).unwrap().title, "Limits");
    }

    #[test]
    fn list_reads_the_root_and_subfolders_and_skips_what_is_not_a_session() {
        let tmp = Scratch::new("list");
        let dir = &tmp.0;
        let write = |rel: &str, md: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, md).unwrap();
        };
        let md = |m: &Meeting| m.to_markdown();
        write("2026-11-01_090000.md", &md(&class("", "Old root", "2026-11-01 09:00")));
        write("Calculus I/2026-11-20 Limits.md", &md(&class("Calculus I", "Limits", "2026-11-20 09:00")));
        write("Calculus I/zzz.md", &md(&class("Calculus I", "Newest", "2026-11-22 09:00")));
        write("exports/2026-11-20 Limits.md", &md(&class("Calculus I", "Export", "2026-11-30 09:00")));
        write(".obsidian/workspace.md", &md(&class("", "Hidden", "2026-12-01 09:00")));
        write(".trash/x.md", &md(&class("", "Trashed", "2026-12-01 09:00")));
        write("My own note.md", "# Shopping\n\n- milk\n");
        write("Calculus I/Plain.md", "---\ntitle: x\n---\n\nno date, no markers\n");
        write("Calculus I/deep/too deep.md", &md(&class("", "Too deep", "2026-12-02 09:00")));
        write("Calculus I/readme.txt", "x");
        let found = list(dir);
        let titles: Vec<_> = found.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["Newest", "Limits", "Old root"]);
    }

    #[test]
    fn list_recognizes_front_matter_or_markers_and_breaks_ties_stably() {
        assert!(is_session("---\ntitle: x\ndate: 2026-11-17 10:00\n---\nbody"));
        assert!(is_session("just text\n<!-- hushnote:notes -->\n"));
        assert!(!is_session("---\ntitle: x\n---\nbody date: 2026"));
        assert!(!is_session("# A note"));
        let tmp = Scratch::new("ties");
        for n in ["a", "b", "c"] {
            let mut m = class("", n, "2026-11-17 10:00");
            m.file = tmp.0.join(format!("{n}.md"));
            m.save().unwrap();
        }
        let one: Vec<_> = list(&tmp.0).into_iter().map(|m| m.title).collect();
        assert_eq!(one, ["c", "b", "a"]);
        assert_eq!(one, list(&tmp.0).into_iter().map(|m| m.title).collect::<Vec<_>>());
    }

    #[test]
    fn an_old_file_named_by_date_and_time_still_loads() {
        let tmp = Scratch::new("old");
        let old = "---\ntitle: \"Weekly sync\"\ndate: 2026-11-17 10:00\nduration: 47:00\n---\n\n# Weekly sync\n\n<!-- hushnote:notes -->\n## My notes\n\nhi\n\n<!-- hushnote:summary -->\nS\n\n<!-- hushnote:transcript -->\n## Transcript\n\n- **[00:05] You:** hello\n";
        std::fs::write(tmp.0.join("2026-11-17_100000.md"), old).unwrap();
        let found = list(&tmp.0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Weekly sync");
        assert_eq!(found[0].lines.len(), 1);
    }

    #[test]
    fn a_session_is_not_moved_while_something_works_on_it() {
        let f = Path::new("/n/a.md");
        let other = Path::new("/n/b.md");
        assert!(may_relocate(f, &[]));
        assert!(may_relocate(f, &[None, Some(other)]));
        assert!(!may_relocate(f, &[Some(other), Some(f)]), "recording, summarizing, review or ask");
    }

    fn nb_session(title: &str, date: &str, summary: &str) -> Meeting {
        Meeting { title: title.into(), date: date.into(), mode: Mode::Class, subject: "Calculus I".into(), summary: summary.into(), ..Default::default() }
    }

    #[test]
    fn markdown_is_stripped_from_preview_lines() {
        assert_eq!(strip_markdown("## **Topics** covered"), "Topics covered");
        assert_eq!(strip_markdown("- **Limit**: the value `f(x)` approaches."), "Limit: the value f(x) approaches.");
        assert_eq!(strip_markdown("- [ ] Problem set 3 · Fri"), "Problem set 3 · Fri");
        assert_eq!(strip_markdown("3. See [the book](http://x.y) now"), "See the book now");
        assert_eq!(strip_markdown("  plain   words "), "plain words");
    }

    #[test]
    fn the_preview_is_the_bullets_of_the_first_section() {
        let m = nb_session("Limits", "2026-11-18 09:00", "## Topics covered\n- Limits at a point.\n- One-sided **limits**.\n- Continuity.\n- Fourth.\n\n## Formulas\n- `x`");
        assert_eq!(preview_lines(&m, 3), ["Limits at a point.", "One-sided limits.", "Continuity."]);
        assert_eq!(preview_lines(&m, 10).len(), 4, "only the first section");
        // Without bullets the prose of the section is used.
        let prose = nb_session("P", "2026-11-18 09:00", "## Summary\nWe agreed on the plan.\nShip on Friday.");
        assert_eq!(preview_lines(&prose, 4), ["We agreed on the plan.", "Ship on Friday."]);
    }

    #[test]
    fn the_preview_falls_back_to_notes_and_then_the_transcript() {
        use crate::audio::Speaker;
        let mut m = nb_session("N", "2026-11-18 09:00", "");
        m.notes = "- first note\n\nsecond **note**".into();
        assert_eq!(preview_lines(&m, 4), ["first note", "second note"]);
        m.notes.clear();
        m.lines = vec![Line { speaker: Speaker::Them, t_ms: 0, text: "Hello everyone.".into() }];
        assert_eq!(preview_lines(&m, 4), ["Hello everyone."]);
        m.lines.clear();
        assert!(preview_lines(&m, 4).is_empty());
    }

    #[test]
    fn due_counts_add_up_per_group_and_skip_meetings() {
        let a = nb_session("A", "2026-11-18 09:00", "## Tasks\n- [ ] One · Fri\n- [x] Two\n- [ ] Three");
        let b = nb_session("B", "2026-11-19 09:00", "## Tasks\n- [ ] Four · Mon");
        let mut meet = Meeting { title: "Sync".into(), date: "2026-11-17 10:00".into(), summary: "- [ ] Task".into(), ..Default::default() };
        let all = vec![b, a, meet.clone()];
        let groups = notebooks(&all);
        assert_eq!(group_summary(&all, &groups[0]), "2 classes · 3 due");
        assert_eq!(group_summary(&all, &groups[1]), "1 session");
        meet.summary.clear();
        let none = vec![nb_session("C", "2026-11-18 09:00", "")];
        assert_eq!(group_summary(&none, &notebooks(&none)[0]), "1 class");
        let tasks = open_tasks(&all, &groups[0].indices);
        assert_eq!(tasks.len(), 3);
        assert_eq!((tasks[0].session, tasks[0].line, tasks[0].text.as_str(), tasks[0].due.as_deref()), (1, 1, "One", Some("Fri")));
    }

    #[test]
    fn sheets_run_oldest_first_and_folders_show_the_newest() {
        let all = vec![
            nb_session("newest", "2026-11-21 09:00", ""),
            nb_session("middle", "2026-11-20 09:00", ""),
            nb_session("tie-b", "2026-11-18 09:00", ""),
            nb_session("tie-a", "2026-11-18 09:00", ""),
            nb_session("oldest", "2026-11-10 09:00", ""),
        ];
        let idx: Vec<usize> = (0..all.len()).collect();
        let order: Vec<_> = chronological(&all, &idx).into_iter().map(|i| all[i].title.as_str()).collect();
        assert_eq!(order, ["oldest", "tie-a", "tie-b", "middle", "newest"]);
        let recent: Vec<_> = most_recent(&all, &idx, 3).into_iter().map(|i| all[i].title.as_str()).collect();
        assert_eq!(recent, ["newest", "middle", "tie-b"]);
    }

    #[test]
    fn the_review_deck_goes_class_by_class_in_order() {
        let card = |q: &str| Card { question: q.into(), answer: "a".into() };
        let mut late = nb_session("late", "2026-11-20 09:00", "");
        late.review = vec![card("l1"), card("l2")];
        let mut early = nb_session("early", "2026-11-10 09:00", "");
        early.review = vec![card("e1")];
        let bare = nb_session("bare", "2026-11-15 09:00", "");
        let all = vec![late, bare, early];
        let deck = review_deck(&all, &[0, 1, 2]);
        assert_eq!(deck, [(2, 0), (0, 0), (0, 1)]);
        let questions: Vec<_> = deck.iter().map(|&(s, c)| all[s].review[c].question.as_str()).collect();
        assert_eq!(questions, ["e1", "l1", "l2"]);
        assert!(review_deck(&all, &[1]).is_empty());
    }
}
