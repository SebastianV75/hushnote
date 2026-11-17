//! Resúmenes con un LLM local por la API compatible con OpenAI (`/v1/chat/completions`), que
//! hablan Ollama, llama-server (llama.cpp) y LM Studio. Nada sale de la máquina.
//!
//! Ollama usa un contexto corto por defecto, así que una reunión larga se resume por partes y
//! luego se unen los resúmenes parciales.

use crate::session::Line;
use std::io::{BufRead, BufReader};

const PART_CHARS: usize = 8_000;

const SYSTEM: &str = "Eres un asistente que escribe notas de reuniones en español, claras y \
concisas, en Markdown. Usa solo lo que está en la transcripción y en las notas del usuario; \
no inventes datos. La transcripción es automática y puede tener errores de palabras: \
interprétalos con sentido común. \"Tú\" es el usuario; \"Ellos\" son los demás participantes.";

const FORMAT: &str = "Escribe las notas con exactamente estas secciones:\n\
## Resumen\n(2 a 4 frases)\n\
## Puntos clave\n(viñetas)\n\
## Decisiones\n(viñetas; \"Ninguna\" si no hubo)\n\
## Pendientes\n(lista de tareas `- [ ] tarea — responsable` si se sabe; \"Ninguno\" si no hubo)\n\
Si el usuario tomó notas, respétalas: son lo que más le importa, complétalas con detalles de \
la transcripción. No añadas nada antes ni después de las secciones.";

pub struct Llm {
    pub url: String,
    pub model: String,
}

pub fn transcript_text(lines: &[Line]) -> String {
    lines.iter().map(|l| format!("[{}] {}: {}\n", clock(l.t_ms), l.speaker.nombre(), l.text)).collect()
}

pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) }
}

/// Parte la transcripción en trozos de líneas completas.
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

/// Quita bloques `<think>…</think>` que algunos modelos (Qwen3, DeepSeek-R1) escriben.
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
                        // Guarda un posible "<thi" partido al final.
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
                "No pude hablar con el LLM local en {} ({e}). ¿Está corriendo Ollama? Prueba `ollama serve` y `ollama pull {}`.",
                self.url, self.model
            )
        })?;
        let reader = BufReader::new(response.into_body().into_reader());
        let mut filter = ThinkFilter::default();
        let mut full = String::new();
        for line in reader.lines() {
            let line = line.map_err(|e| format!("respuesta cortada: {e}"))?;
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

    /// Escribe las notas finales; `on_delta` recibe el texto mientras llega.
    pub fn summarize(&self, notes: &str, lines: &[Line], on_delta: impl FnMut(&str)) -> Result<String, String> {
        let transcript = transcript_text(lines);
        if transcript.trim().is_empty() && notes.trim().is_empty() {
            return Err("No hay transcripción ni notas que resumir.".into());
        }
        let pieces = parts(&transcript);
        let material = if pieces.len() <= 1 {
            format!("Transcripción:\n{transcript}")
        } else {
            let mut partial = String::new();
            for (i, piece) in pieces.iter().enumerate() {
                let prompt = format!(
                    "Esta es la parte {} de {} de una reunión. Extrae en viñetas los puntos clave, \
                     decisiones y pendientes con sus responsables. Solo viñetas.\n\n{piece}",
                    i + 1,
                    pieces.len()
                );
                let summary = self.chat(SYSTEM, &prompt, |_| {})?;
                partial.push_str(&format!("### Parte {}\n{summary}\n\n", i + 1));
            }
            format!("Notas parciales de la reunión, en orden:\n{partial}")
        };
        let notes = if notes.trim().is_empty() { "(el usuario no tomó notas)".to_string() } else { notes.to_string() };
        let prompt = format!("Notas del usuario:\n{notes}\n\n{material}\n\n{FORMAT}");
        self.chat(SYSTEM, &prompt, on_delta)
    }

    /// Un título corto para la reunión.
    pub fn title(&self, summary: &str) -> Result<String, String> {
        let prompt = format!("Escribe un título de 3 a 6 palabras para esta reunión, sin comillas ni punto final:\n\n{summary}");
        let t = self.chat(SYSTEM, &prompt, |_| {})?;
        Ok(t.lines().next().unwrap_or("").trim_matches(['"', '#', ' ', '*']).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtra_el_razonamiento_aunque_llegue_partido() {
        let mut f = ThinkFilter::default();
        let mut out = String::new();
        for piece in ["Hola <th", "ink>pienso", " mucho</thi", "nk> mundo"] {
            out.push_str(&f.push(piece));
        }
        assert_eq!(out, "Hola  mundo");
    }

    #[test]
    fn parte_por_lineas_completas() {
        let line = "x".repeat(100);
        let text: String = (0..200).map(|_| format!("{line}\n")).collect();
        let p = parts(&text);
        assert!(p.len() >= 2);
        assert!(p.iter().all(|s| s.len() <= PART_CHARS + 101));
        assert_eq!(p.concat(), text);
    }

    #[test]
    fn reloj() {
        assert_eq!(clock(65_000), "01:05");
        assert_eq!(clock(3_725_000), "1:02:05");
    }
}
