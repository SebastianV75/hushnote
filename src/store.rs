//! Cada reunión es un archivo Markdown en la carpeta de notas: legible en cualquier editor
//! (Obsidian, rune, nvim…). Unos comentarios HTML marcan las secciones para volver a leerlo.

use crate::audio::Speaker;
use crate::llm::clock;
use crate::session::Line;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meeting {
    pub file: PathBuf,
    pub title: String,
    /// "AAAA-MM-DD HH:MM"
    pub date: String,
    pub duration_ms: u64,
    pub notes: String,
    pub summary: String,
    pub lines: Vec<Line>,
}

const NOTAS: &str = "<!-- hushnote:notas -->";
const RESUMEN: &str = "<!-- hushnote:resumen -->";
const TRANSCRIPCION: &str = "<!-- hushnote:transcripcion -->";

impl Meeting {
    pub fn new(dir: &Path) -> Meeting {
        let now = chrono::Local::now();
        Meeting {
            file: dir.join(format!("{}.md", now.format("%Y-%m-%d_%H%M%S"))),
            title: format!("Reunión del {}", now.format("%d/%m %H:%M")),
            date: now.format("%Y-%m-%d %H:%M").to_string(),
            ..Default::default()
        }
    }

    pub fn to_markdown(&self) -> String {
        let mut md = format!(
            "---\ntitulo: \"{}\"\nfecha: {}\nduracion: {}\n---\n\n# {}\n\n",
            self.title.replace('"', "'"),
            self.date,
            clock(self.duration_ms),
            self.title
        );
        md.push_str(&format!("{NOTAS}\n## Mis notas\n\n{}\n\n", self.notes.trim()));
        md.push_str(&format!("{RESUMEN}\n{}\n\n", self.summary.trim()));
        md.push_str(&format!("{TRANSCRIPCION}\n## Transcripción\n\n"));
        for l in &self.lines {
            md.push_str(&format!("- **[{}] {}:** {}\n", clock(l.t_ms), l.speaker.nombre(), l.text));
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
                        "titulo" => m.title = v,
                        "fecha" => m.date = v,
                        "duracion" => m.duration_ms = parse_clock(&v).unwrap_or(0),
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
        m.notes = section(NOTAS, Some(RESUMEN)).trim_start_matches("## Mis notas").trim().to_string();
        m.summary = section(RESUMEN, Some(TRANSCRIPCION));
        for line in section(TRANSCRIPCION, None).lines() {
            // - **[01:05] Tú:** texto
            let Some(rest) = line.strip_prefix("- **[") else { continue };
            let Some((time, rest)) = rest.split_once("] ") else { continue };
            let Some((who, text)) = rest.split_once(":** ") else { continue };
            m.lines.push(Line {
                speaker: Speaker::from_nombre(who),
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

/// Las reuniones guardadas, de la más nueva a la más vieja.
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
    fn ida_y_vuelta_por_markdown() {
        let m = Meeting {
            file: PathBuf::from("/tmp/x.md"),
            title: "Planeación \"Q1\"".into(),
            date: "2026-11-17 16:30".into(),
            duration_ms: 125_000,
            notes: "- revisar presupuesto\n- llamar a Ana".into(),
            summary: "## Resumen\nSe habló del presupuesto.\n\n## Pendientes\n- [ ] Llamar a Ana".into(),
            lines: vec![
                Line { speaker: Speaker::Yo, t_ms: 5_000, text: "Hola, ¿empezamos?".into() },
                Line { speaker: Speaker::Ellos, t_ms: 65_000, text: "Sí: primero el presupuesto.".into() },
            ],
        };
        let back = Meeting::parse(&m.file, &m.to_markdown());
        assert_eq!(back.title, "Planeación 'Q1'");
        assert_eq!(back.date, m.date);
        assert_eq!(back.duration_ms, m.duration_ms);
        assert_eq!(back.notes, m.notes);
        assert_eq!(back.summary, m.summary);
        assert_eq!(back.lines, m.lines);
    }
}
