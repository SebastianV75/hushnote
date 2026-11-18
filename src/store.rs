//! Each meeting is a Markdown file in the notes folder, readable in any editor (Obsidian,
//! nvim…). HTML comments mark the sections so the file can be read back.

use crate::audio::Speaker;
use crate::llm::clock;
use crate::session::Line;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meeting {
    pub file: PathBuf,
    pub title: String,
    /// "YYYY-MM-DD HH:MM"
    pub date: String,
    pub duration_ms: u64,
    pub notes: String,
    pub summary: String,
    /// Who wrote the summary: "Qwen3 4B", "Claude"…
    pub summary_by: String,
    pub lines: Vec<Line>,
}

const NOTES: &str = "<!-- hushnote:notes -->";
const SUMMARY: &str = "<!-- hushnote:summary -->";
const TRANSCRIPT: &str = "<!-- hushnote:transcript -->";

impl Meeting {
    pub fn new(dir: &Path) -> Meeting {
        let now = chrono::Local::now();
        Meeting {
            file: dir.join(format!("{}.md", now.format("%Y-%m-%d_%H%M%S"))),
            title: format!("Meeting on {}", now.format("%b %-d, %H:%M")),
            date: now.format("%Y-%m-%d %H:%M").to_string(),
            ..Default::default()
        }
    }

    pub fn to_markdown(&self) -> String {
        let mut md = format!("---\ntitle: \"{}\"\ndate: {}\nduration: {}\n", self.title.replace('"', "'"), self.date, clock(self.duration_ms));
        if !self.summary_by.is_empty() {
            md.push_str(&format!("summary_by: {}\n", self.summary_by));
        }
        md.push_str(&format!("---\n\n# {}\n\n", self.title));
        md.push_str(&format!("{NOTES}\n## My notes\n\n{}\n\n", self.notes.trim()));
        md.push_str(&format!("{SUMMARY}\n{}\n\n", self.summary.trim()));
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
                        "summary_by" => m.summary_by = v,
                        _ => {}
                    }
                }
                body = &rest[end + 5..];
            }
        }
        let section = |start: &str, end: Option<&str>| -> String {
            let Some(i) = body.find(start) else { return String::new() };
            let rest = &body[i + start.len()..];
            let j = end.and_then(|e| rest.find(e)).unwrap_or(rest.len());
            rest[..j].trim().to_string()
        };
        m.notes = section(NOTES, Some(SUMMARY)).trim_start_matches("## My notes").trim().to_string();
        m.summary = section(SUMMARY, Some(TRANSCRIPT));
        for line in section(TRANSCRIPT, None).lines() {
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

    pub fn load(file: &Path) -> Option<Meeting> {
        std::fs::read_to_string(file).ok().map(|md| Meeting::parse(file, &md))
    }
}

fn parse_clock(s: &str) -> Option<u64> {
    let parts: Vec<u64> = s.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    let secs = parts.iter().fold(0, |acc, p| acc * 60 + p);
    Some(secs * 1000)
}

/// The saved meetings, newest first.
pub fn list(dir: &Path) -> Vec<Meeting> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files.reverse();
    files.iter().filter_map(|f| Meeting::load(f)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_markdown() {
        let m = Meeting {
            file: PathBuf::from("/tmp/x.md"),
            title: "Planning \"Q1\"".into(),
            date: "2026-11-17 16:30".into(),
            duration_ms: 125_000,
            notes: "- review the budget\n- call Ana".into(),
            summary: "## Summary\nWe talked about the budget.\n\n## Action items\n- [ ] Call Ana".into(),
            summary_by: "Claude".into(),
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
        assert_eq!(back.lines, m.lines);
    }
}
