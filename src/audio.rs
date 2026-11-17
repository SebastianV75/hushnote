//! Captura de audio a 16 kHz mono f32: micrófono ("Tú") y audio del sistema ("Ellos") como
//! pistas separadas, con `pw-record` de PipeWire (o `parec` como respaldo). También lee
//! archivos WAV, para importar grabaciones y para las pruebas.
//!
//! La captura del sistema sigue la idea de Hex (`src/meeting/linux.rs`, MIT): `pw-record`
//! con `stream.capture.sink=true` graba el monitor de la salida por defecto.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const RATE: u32 = 16_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Speaker {
    Yo,
    Ellos,
    /// Audio importado de un archivo: no sabemos quién habla.
    Audio,
}

impl Speaker {
    pub fn nombre(self) -> &'static str {
        match self {
            Speaker::Yo => "Tú",
            Speaker::Ellos => "Ellos",
            Speaker::Audio => "Audio",
        }
    }
    pub fn from_nombre(s: &str) -> Speaker {
        match s {
            "Tú" => Speaker::Yo,
            "Ellos" => Speaker::Ellos,
            _ => Speaker::Audio,
        }
    }
}

/// Un bloque de muestras de una pista.
pub struct Block {
    pub speaker: Speaker,
    pub pcm: Vec<f32>,
}

/// Un proceso de captura en marcha; al soltarlo se detiene.
pub struct Capture {
    child: Child,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Capture {
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn has(cmd: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {cmd}")).stdout(Stdio::null()).status()
        .map(|s| s.success()).unwrap_or(false)
}

/// `pw-record` 1.3.81+ necesita `--raw` para escribir PCM sin cabecera a stdout; las versiones
/// anteriores lo rechazan y ya escriben PCM crudo. Leemos su `--help` como hace Hex.
fn pw_record_takes_raw() -> bool {
    Command::new("pw-record").arg("--help").output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("--raw") || String::from_utf8_lossy(&o.stderr).contains("--raw"))
        .unwrap_or(false)
}

fn capture_command(speaker: Speaker) -> Result<Command, String> {
    if has("pw-record") {
        let mut c = Command::new("pw-record");
        c.args(["--rate", "16000", "--channels", "1", "--format", "s16"]);
        if speaker == Speaker::Ellos {
            c.args(["-P", "{ stream.capture.sink=true }"]);
        }
        if pw_record_takes_raw() {
            c.arg("--raw");
        }
        c.arg("-");
        return Ok(c);
    }
    if has("parec") {
        let mut c = Command::new("parec");
        c.args(["--raw", "--format=s16le", "--rate=16000", "--channels=1"]);
        if speaker == Speaker::Ellos {
            c.arg("--device=@DEFAULT_MONITOR@");
        }
        return Ok(c);
    }
    Err("No encontré `pw-record` (PipeWire) ni `parec` para grabar audio. En Arch/Omarchy: `sudo pacman -S pipewire`.".into())
}

/// Empieza a capturar una pista; los bloques (~100 ms) llegan por `tx`.
pub fn start(speaker: Speaker, tx: Sender<Block>) -> Result<Capture, String> {
    let mut child = capture_command(speaker)?
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("no pude iniciar la captura de {}: {e}", speaker.nombre()))?;
    let mut out = child.stdout.take().ok_or("sin stdout")?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::spawn(move || {
        let mut reader = PcmReader::default();
        let mut buf = vec![0u8; 3200]; // 100 ms de s16 mono a 16 kHz
        while !flag.load(Ordering::Relaxed) {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let pcm = reader.push(&buf[..n]);
                    if !pcm.is_empty() && tx.send(Block { speaker, pcm }).is_err() {
                        break;
                    }
                }
            }
        }
    });
    Ok(Capture { child, stop, thread: Some(thread) })
}

/// Convierte bytes s16le en f32, saltando una cabecera WAV si el programa la escribe.
#[derive(Default)]
pub struct PcmReader {
    header: Vec<u8>,
    header_done: bool,
    odd: Option<u8>,
}

impl PcmReader {
    pub fn push(&mut self, mut bytes: &[u8]) -> Vec<f32> {
        if !self.header_done {
            self.header.extend_from_slice(bytes);
            if self.header.len() < 44 {
                return Vec::new();
            }
            self.header_done = true;
            let data = std::mem::take(&mut self.header);
            let start = if data.starts_with(b"RIFF") {
                // Busca el bloque "data" y salta su cabecera de 8 bytes.
                data.windows(4).position(|w| w == b"data").map(|p| p + 8).unwrap_or(44)
            } else {
                0
            };
            return self.samples(&data[start.min(data.len())..]);
        }
        if bytes.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(bytes.len() / 2 + 1);
        if let Some(lo) = self.odd.take() {
            out.push(i16::from_le_bytes([lo, bytes[0]]) as f32 / 32768.0);
            bytes = &bytes[1..];
        }
        out.extend(self.samples(bytes));
        out
    }

    fn samples(&mut self, bytes: &[u8]) -> Vec<f32> {
        let mut chunks = bytes.chunks_exact(2);
        let out = chunks.by_ref().map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        self.odd = chunks.remainder().first().copied();
        out
    }
}

/// Lee un WAV (cualquier tasa, mono o estéreo) como 16 kHz mono f32.
pub fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().filter_map(Result::ok).map(|s| s as f32 / scale).collect()
        }
    };
    let mono: Vec<f32> = raw.chunks(channels).map(|f| f.iter().sum::<f32>() / f.len() as f32).collect();
    Ok(resample(&mono, spec.sample_rate, RATE))
}

/// Remuestreo lineal: suficiente para voz.
pub fn resample(pcm: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || pcm.is_empty() {
        return pcm.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let n = (pcm.len() as f64 / ratio) as usize;
    (0..n)
        .map(|i| {
            let x = i as f64 * ratio;
            let j = x as usize;
            let f = (x - j as f64) as f32;
            let a = pcm[j];
            let b = *pcm.get(j + 1).unwrap_or(&a);
            a + (b - a) * f
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salta_la_cabecera_wav_y_une_bytes_partidos() {
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.extend_from_slice(&[0u8; 20]);
        wav.extend_from_slice(b"data\0\0\0\0");
        wav.extend_from_slice(&16384i16.to_le_bytes());
        wav.extend_from_slice(&(-16384i16).to_le_bytes());
        let mut r = PcmReader::default();
        let mut out = r.push(&wav[..wav.len() - 1]);
        out.extend(r.push(&wav[wav.len() - 1..]));
        assert_eq!(out, vec![0.5, -0.5]);
    }

    #[test]
    fn pcm_crudo_sin_cabecera() {
        let bytes: Vec<u8> = (0..30).flat_map(|_| 8192i16.to_le_bytes()).collect();
        let out = PcmReader::default().push(&bytes);
        assert_eq!(out.len(), 30);
        assert!(out.iter().all(|&s| s == 0.25));
    }

    #[test]
    fn remuestrea_a_16k() {
        let pcm = vec![0.0; 48_000];
        assert_eq!(resample(&pcm, 48_000, 16_000).len(), 16_000);
    }
}
