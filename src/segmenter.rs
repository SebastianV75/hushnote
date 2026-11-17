//! Corta una pista en frases para transcribirlas mientras la reunión sigue: abre un fragmento
//! cuando hay voz, lo cierra tras una pausa (o al llegar al máximo) y guarda un poco de audio
//! anterior para no comerse el inicio de la palabra. Detección por energía con un piso de
//! ruido que se adapta.

use crate::audio::RATE;

const FRAME: usize = (RATE as usize) * 30 / 1000; // 30 ms
const PREROLL: usize = (RATE as usize) * 300 / 1000;
const PAUSE: usize = (RATE as usize) * 700 / 1000;
const MIN_SPEECH: usize = (RATE as usize) * 600 / 1000;
const MAX_CHUNK: usize = (RATE as usize) * 20;
const MIN_CUT: usize = (RATE as usize) * 2;

/// Un fragmento con voz, listo para el modelo. `start` en muestras desde el inicio de la pista.
#[derive(Debug)]
pub struct Chunk {
    pub start: usize,
    pub pcm: Vec<f32>,
}

pub struct Segmenter {
    pending: Vec<f32>,
    /// Muestras vistas antes de `pending`.
    pos: usize,
    floor: f32,
    preroll: Vec<f32>,
    current: Option<Chunk>,
    voiced: usize,
    silence: usize,
    level: f32,
}

impl Default for Segmenter {
    fn default() -> Self {
        Segmenter {
            pending: Vec::new(),
            pos: 0,
            floor: 0.003,
            preroll: Vec::new(),
            current: None,
            voiced: 0,
            silence: 0,
            level: 0.0,
        }
    }
}

impl Segmenter {
    /// Nivel de la última trama, 0..1, para el medidor.
    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn push(&mut self, pcm: &[f32]) -> Vec<Chunk> {
        self.pending.extend_from_slice(pcm);
        let mut out = Vec::new();
        let mut used = 0;
        while self.pending.len() - used >= FRAME {
            let frame = self.pending[used..used + FRAME].to_vec();
            used += FRAME;
            if let Some(chunk) = self.frame(&frame) {
                out.push(chunk);
            }
            self.pos += FRAME;
        }
        self.pending.drain(..used);
        out
    }

    /// Cierra lo que quede al detener la grabación.
    pub fn flush(&mut self) -> Option<Chunk> {
        let rest = std::mem::take(&mut self.pending);
        if let Some(current) = self.current.as_mut() {
            current.pcm.extend_from_slice(&rest);
        }
        self.pos += rest.len();
        self.close()
    }

    fn frame(&mut self, frame: &[f32]) -> Option<Chunk> {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        // dBFS -60..0 → 0..1
        self.level = ((20.0 * rms.max(1e-6).log10() + 60.0) / 60.0).clamp(0.0, 1.0);
        let speech = rms > (self.floor * 3.0).max(0.008);
        if !speech {
            // El piso sigue al ruido de fondo: baja rápido, sube despacio.
            self.floor = if rms < self.floor { self.floor * 0.9 + rms * 0.1 } else { self.floor * 0.995 + rms * 0.005 };
        }

        match self.current.as_mut() {
            None => {
                if speech {
                    let start = self.pos.saturating_sub(self.preroll.len());
                    let mut pcm = std::mem::take(&mut self.preroll);
                    pcm.extend_from_slice(frame);
                    self.current = Some(Chunk { start, pcm });
                    self.voiced = FRAME;
                    self.silence = 0;
                } else {
                    self.preroll.extend_from_slice(frame);
                    let excess = self.preroll.len().saturating_sub(PREROLL);
                    self.preroll.drain(..excess);
                }
                None
            }
            Some(current) => {
                current.pcm.extend_from_slice(frame);
                if speech {
                    self.voiced += FRAME;
                    self.silence = 0;
                } else {
                    self.silence += FRAME;
                }
                let len = current.pcm.len();
                let paused = self.silence >= PAUSE;
                // Al pasar del máximo corta en la próxima trama silenciosa, o a la fuerza a 1.5×.
                let long = len >= MAX_CHUNK && (!speech || len >= MAX_CHUNK * 3 / 2) && len >= MIN_CUT;
                if paused || long { self.close() } else { None }
            }
        }
    }

    fn close(&mut self) -> Option<Chunk> {
        let chunk = self.current.take()?;
        let voiced = std::mem::take(&mut self.voiced);
        self.silence = 0;
        (voiced >= MIN_SPEECH).then_some(chunk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(secs: f32, amp: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        (0..n).map(|i| amp * (i as f32 * 0.07).sin()).collect()
    }

    #[test]
    fn dos_frases_separadas_por_una_pausa_son_dos_fragmentos() {
        let mut s = Segmenter::default();
        let mut audio = tone(1.0, 0.0);
        audio.extend(tone(2.0, 0.3));
        audio.extend(tone(1.0, 0.0));
        audio.extend(tone(1.5, 0.3));
        audio.extend(tone(1.0, 0.0));
        let mut chunks = Vec::new();
        for block in audio.chunks(1600) {
            chunks.extend(s.push(block));
        }
        chunks.extend(s.flush());
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        // El primero empieza ~300 ms antes de la voz (1 s).
        let start = chunks[0].start as f32 / RATE as f32;
        assert!((0.6..1.0).contains(&start), "inicio {start}");
    }

    #[test]
    fn el_silencio_y_los_golpes_cortos_no_se_transcriben() {
        let mut s = Segmenter::default();
        let mut audio = tone(2.0, 0.0);
        audio.extend(tone(0.2, 0.5));
        audio.extend(tone(2.0, 0.0));
        let mut chunks = s.push(&audio);
        chunks.extend(s.flush());
        assert!(chunks.is_empty());
    }

    #[test]
    fn un_monologo_largo_se_corta() {
        let mut s = Segmenter::default();
        let chunks = s.push(&tone(50.0, 0.3));
        assert!(chunks.len() >= 1);
        assert!(chunks.iter().all(|c| c.pcm.len() <= MAX_CHUNK * 3 / 2 + FRAME));
    }
}
