//! Study exports of a session: a clean Markdown document and an Anki-ready CSV of its review
//! cards. They go to an `exports/` folder inside the notes folder; `store::list` only reads
//! the notes folder itself, so exports never show up as sessions.

use crate::llm::clock;
use crate::store::{Card, Meeting, lower_headings, sanitize_name};
use std::path::{Path, PathBuf};

/// The folder the exports are written to.
pub fn export_dir(notes_dir: &Path) -> PathBuf {
    notes_dir.join("exports")
}

fn stem(m: &Meeting) -> String {
    m.file.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "session".into())
}

/// `<notes_dir>/exports/<stem>.md`
pub fn markdown_path(notes_dir: &Path, m: &Meeting) -> PathBuf {
    export_dir(notes_dir).join(format!("{}.md", stem(m)))
}

/// `<notes_dir>/exports/<stem>-anki.csv`
pub fn anki_path(notes_dir: &Path, m: &Meeting) -> PathBuf {
    export_dir(notes_dir).join(format!("{}-anki.csv", stem(m)))
}

/// A study document: title, subject, date, duration, the summary and the review questions.
/// The transcript and the raw notes are left out.
pub fn export_markdown(m: &Meeting) -> String {
    let mut md = format!("# {}\n\n", m.title.trim());
    let subject = m.subject.trim();
    if !subject.is_empty() {
        md.push_str(&format!("**Subject:** {subject}  \n"));
    }
    md.push_str(&format!("**Date:** {}  \n", m.date));
    if m.duration_ms > 0 {
        md.push_str(&format!("**Duration:** {}  \n", clock(m.duration_ms)));
    }
    md.push('\n');
    if !m.summary.trim().is_empty() {
        md.push_str(m.summary.trim());
        md.push_str("\n\n");
    }
    if !m.review.is_empty() {
        md.push_str("## Review questions\n\n");
        for (i, c) in m.review.iter().enumerate() {
            md.push_str(&format!("### {}. {}\n\n", i + 1, c.question.trim().replace('\n', " ")));
            md.push_str(&answer_block(&c.answer));
        }
    }
    if !m.questions.is_empty() {
        md.push_str("## Questions I asked\n\n");
        for t in &m.questions {
            md.push_str(&format!("### {}\n\n{}\n\n", t.heading(), lower_headings(&t.answer)));
        }
    }
    md.trim_end().to_string() + "\n"
}

/// A multi-line answer starts on its own line so its lists and paragraphs render.
fn answer_block(answer: &str) -> String {
    let a = answer.trim();
    if a.contains('\n') { format!("**A:**\n\n{a}\n\n") } else { format!("**A:** {a}\n\n") }
}

/// The cards as a CSV that Anki imports directly (File > Import): `Front,Back`.
///
/// Three header lines (`#separator:comma`, `#html:true`, `#columns:Front,Back`), then one row
/// per card with both fields in double quotes (inner quotes doubled). Since the file declares
/// HTML, `&`, `<` and `>` are escaped and line breaks inside a field become `<br>`, so every
/// card is exactly one line of the file.
pub fn export_anki_csv(cards: &[Card]) -> String {
    let mut csv = String::from("#separator:comma\n#html:true\n#columns:Front,Back\n");
    for c in cards {
        csv.push_str(&format!("{},{}\n", field(&c.question), field(&c.answer)));
    }
    csv
}

fn field(text: &str) -> String {
    let html = text
        .trim()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace("\r\n", "\n")
        .replace('\n', "<br>");
    format!("\"{}\"", html.replace('"', "\"\""))
}

// ---- A whole notebook ----------------------------------------------------------------------------

/// `<notes_dir>/exports/<notebook>.md`
pub fn notebook_markdown_path(notes_dir: &Path, title: &str) -> PathBuf {
    export_dir(notes_dir).join(format!("{}.md", notebook_stem(title)))
}

/// `<notes_dir>/exports/<notebook>-anki.csv`
pub fn notebook_anki_path(notes_dir: &Path, title: &str) -> PathBuf {
    export_dir(notes_dir).join(format!("{}-anki.csv", notebook_stem(title)))
}

fn notebook_stem(title: &str) -> String {
    let s = sanitize_name(title, 60);
    if s.is_empty() { "notebook".into() } else { s }
}

/// Moves every Markdown heading down one level (to at most `######`), leaving fenced code alone.
fn demote_headings(md: &str) -> String {
    let mut fenced = false;
    let mut out = Vec::new();
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if !fenced && (1..6).contains(&hashes) && line[hashes..].starts_with(' ') {
            out.push(format!("#{line}"));
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

/// One study document for a notebook: its name, then each session (given in the order they
/// should read) with its date, summary and review questions. No transcripts, no raw notes and
/// no chat history.
pub fn export_notebook_markdown(title: &str, sessions: &[&Meeting]) -> String {
    let mut md = format!("# {}\n\n", title.trim());
    let n = sessions.len();
    md.push_str(&format!("{n} {}\n", if n == 1 { "session" } else { "sessions" }));
    for m in sessions {
        md.push_str(&format!("\n---\n\n## {}\n\n**Date:** {}  \n", m.title.trim(), m.date));
        if m.duration_ms > 0 {
            md.push_str(&format!("**Duration:** {}  \n", clock(m.duration_ms)));
        }
        md.push('\n');
        if !m.summary.trim().is_empty() {
            md.push_str(&demote_headings(m.summary.trim()));
            md.push_str("\n\n");
        }
        if !m.review.is_empty() {
            md.push_str("### Review questions\n\n");
            for (i, c) in m.review.iter().enumerate() {
                md.push_str(&format!("#### {}. {}\n\n", i + 1, c.question.trim().replace('\n', " ")));
                md.push_str(&answer_block(&c.answer));
            }
        }
    }
    md.trim_end().to_string() + "\n"
}

/// An Anki tag from a name: tags are separated by spaces, so inner spaces become underscores.
fn tag(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join("_")
}

/// The cards of every session as one Anki CSV: `Front,Back,Tags`. Each card is tagged with its
/// class's title (`Limits_and_continuity`) so the deck can be filtered by class in Anki; the
/// header declares the tags column (`#tags column:3`). Same quoting and HTML escaping as
/// [`export_anki_csv`].
pub fn export_notebook_anki_csv(sessions: &[&Meeting]) -> String {
    let mut csv = String::from("#separator:comma\n#html:true\n#columns:Front,Back,Tags\n#tags column:3\n");
    for m in sessions {
        let t = tag(&m.title);
        for c in &m.review {
            csv.push_str(&format!("{},{},{}\n", field(&c.question), field(&c.answer), field(&t)));
        }
    }
    csv
}

/// Writes through a temporary file and renames it, like `Meeting::save`.
pub fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, content).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Speaker;
    use crate::session::Line;

    fn class() -> Meeting {
        Meeting {
            file: PathBuf::from("/notes/2026-11-17_103000.md"),
            title: "Limits".into(),
            subject: "Calculus I".into(),
            date: "2026-11-17 10:30".into(),
            duration_ms: 3_725_000,
            notes: "my private scribbles".into(),
            summary: "## Key concepts\n- A limit is a value.".into(),
            review: vec![
                Card { question: "What is a limit?".into(), answer: "The value f(x) approaches.".into() },
                Card { question: "Define the derivative".into(), answer: "- slope\n- rate of change".into() },
            ],
            lines: vec![Line { speaker: Speaker::Me, t_ms: 1000, text: "secret transcript words".into() }],
            ..Default::default()
        }
    }

    #[test]
    fn markdown_is_a_clean_study_document() {
        let md = export_markdown(&class());
        assert!(md.starts_with("# Limits\n"));
        assert!(md.contains("**Subject:** Calculus I"));
        assert!(md.contains("**Date:** 2026-11-17 10:30"));
        assert!(md.contains("**Duration:** 1:02:05"));
        assert!(md.contains("## Key concepts\n- A limit is a value."));
        assert!(md.contains("## Review questions"));
        assert!(md.contains("### 1. What is a limit?\n\n**A:** The value f(x) approaches."));
        assert!(md.contains("### 2. Define the derivative\n\n**A:**\n\n- slope\n- rate of change"));
        assert!(!md.contains("secret transcript words") && !md.contains("Transcript"));
        assert!(!md.contains("my private scribbles"));
        assert!(!md.contains("hushnote:"));
    }

    #[test]
    fn markdown_includes_the_questions_when_there_are_any() {
        use crate::store::Turn;
        let mut m = class();
        assert!(!export_markdown(&m).contains("Questions I asked"));
        m.questions = vec![
            Turn { question: "What is a limit?".into(), answer: "## Short\n- a value".into(), scope: String::new() },
            Turn { question: "Where did we see it?".into(), answer: "Nov 3.".into(), scope: "Calculus I".into() },
        ];
        let md = export_markdown(&m);
        assert!(md.contains("## Questions I asked\n\n### What is a limit?\n\n##### Short\n- a value\n\n"));
        assert!(md.contains("### Where did we see it? · all Calculus I classes\n\nNov 3.\n"));
        assert!(md.find("## Review questions").unwrap() < md.find("## Questions I asked").unwrap());
        assert!(!md.contains("hushnote:") && !md.contains("secret transcript"));
    }

    #[test]
    fn markdown_skips_what_is_missing() {
        let m = Meeting { title: "Sync".into(), date: "2026-11-17 10:00".into(), ..Default::default() };
        let md = export_markdown(&m);
        assert!(!md.contains("Subject") && !md.contains("Duration") && !md.contains("Review questions"));
        assert!(md.contains("# Sync"));
    }

    #[test]
    fn anki_csv_has_headers_and_one_quoted_row_per_card() {
        let cards = vec![
            Card { question: "Say \"hi\", then <b> & more".into(), answer: "line one\nline two\r\nline three".into() },
            Card { question: "Plain".into(), answer: "Answer".into() },
        ];
        let csv = export_anki_csv(&cards);
        assert_eq!(
            csv,
            "#separator:comma\n#html:true\n#columns:Front,Back\n\
             \"Say \"\"hi\"\", then &lt;b&gt; &amp; more\",\"line one<br>line two<br>line three\"\n\
             \"Plain\",\"Answer\"\n"
        );
        assert_eq!(csv.lines().count(), 3 + cards.len());
        assert_eq!(export_anki_csv(&[]), "#separator:comma\n#html:true\n#columns:Front,Back\n");
    }

    #[test]
    fn exports_never_land_in_the_notes_root() {
        let m = class();
        let root = Path::new("/notes");
        let (md, csv) = (markdown_path(root, &m), anki_path(root, &m));
        assert_eq!(md, Path::new("/notes/exports/2026-11-17_103000.md"));
        assert_eq!(csv, Path::new("/notes/exports/2026-11-17_103000-anki.csv"));
        assert_ne!(md.parent(), Some(root));
        assert_ne!(csv.parent(), Some(root));
    }

    #[test]
    fn written_exports_are_not_listed_as_sessions() {
        let dir = std::env::temp_dir().join(format!("hushnote-export-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = class();
        m.file = dir.join("2026-11-17_103000.md");
        m.save().unwrap();
        let path = markdown_path(&dir, &m);
        write_atomic(&path, &export_markdown(&m)).unwrap();
        write_atomic(&anki_path(&dir, &m), &export_anki_csv(&m.review)).unwrap();
        assert!(path.is_file() && anki_path(&dir, &m).is_file());
        assert!(!path.with_extension("md.tmp").exists());
        let listed = crate::store::list(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file, m.file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_notebook_exports_every_session_without_transcripts() {
        let mut a = class();
        a.title = "Limits".into();
        let mut b = class();
        b.title = "Derivatives".into();
        b.date = "2026-11-20 09:00".into();
        b.summary = "## Topics\n- Power rule\n\n```\n# not a heading\n```".into();
        b.review.clear();
        let md = export_notebook_markdown("Calculus I", &[&a, &b]);
        assert!(md.starts_with("# Calculus I\n\n2 sessions\n"));
        assert!(md.find("## Limits").unwrap() < md.find("## Derivatives").unwrap());
        assert!(md.contains("**Date:** 2026-11-17 10:30") && md.contains("**Date:** 2026-11-20 09:00"));
        assert!(md.contains("### Key concepts\n- A limit is a value."), "summary headings move down one level");
        assert!(md.contains("### Topics\n- Power rule") && md.contains("```\n# not a heading\n```"));
        assert!(md.contains("### Review questions\n\n#### 1. What is a limit?\n\n**A:** The value f(x) approaches."));
        assert_eq!(md.matches("### Review questions").count(), 1, "the class with no cards has none");
        assert!(!md.contains("secret transcript words") && !md.contains("my private scribbles"));
        assert!(export_notebook_markdown("Empty", &[]).contains("0 sessions"));
    }

    #[test]
    fn the_notebook_anki_csv_tags_each_card_with_its_class() {
        let mut a = class();
        a.title = "Limits and continuity".into();
        let mut b = class();
        b.title = "Derivatives".into();
        b.review = vec![Card { question: "Power rule?".into(), answer: "n x^(n-1)".into() }];
        let csv = export_notebook_anki_csv(&[&a, &b]);
        assert!(csv.starts_with("#separator:comma\n#html:true\n#columns:Front,Back,Tags\n#tags column:3\n"));
        assert!(csv.contains("\"What is a limit?\",\"The value f(x) approaches.\",\"Limits_and_continuity\"\n"));
        assert!(csv.ends_with("\"Power rule?\",\"n x^(n-1)\",\"Derivatives\"\n"));
        assert_eq!(csv.lines().count(), 4 + 3);
        let named = notebook_markdown_path(Path::new("/notes"), "Calculus I");
        assert_eq!(named, Path::new("/notes/exports/Calculus I.md"));
        assert_eq!(notebook_anki_path(Path::new("/notes"), "A/B"), Path::new("/notes/exports/AB-anki.csv"));
    }
}
