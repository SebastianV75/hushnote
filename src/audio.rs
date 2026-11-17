//! Audio capture at 16 kHz mono f32: the microphone ("You") and the system audio ("Them") as
//! separate tracks, through PipeWire's `pw-record` (or `parec` as a fallback). Also reads WAV
//! files, for importing recordings and for tests.
//!
//! System capture follows Hex (`src/meeting/linux.rs`, MIT): `pw-record` with
//! `stream.capture.sink=true` records the monitor of the default output.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const RATE: u32 = 16_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Speaker {
    Me,
    Them,
    /// Audio imported from a file: we don't know who is speaking.
    Audio,
}

impl Speaker {
    pub fn name(self) -> &'static str {
        match self {
            Speaker::Me => "You",
            Speaker::Them => "Them",
            Speaker::Audio => "Audio",
        }
    }
    pub fn from_name(s: &str) -> Speaker {
        match s {
            "You" => Speaker::Me,
            "Them" => Speaker::Them,
            _ => Speaker::Audio,
        }
    }
}

/// A block of samples from one track.
pub struct Block {
    pub speaker: Speaker,
    pub pcm: Vec<f32>,
}

/// A running capture process; dropping it stops it.
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
        // The capture runs in its own process group: kill all of it, so a wrapper's children
        // can't keep the pipe open and leave the reader (and the transcript) waiting forever.
        if let Ok(pgid) = i32::try_from(self.child.id()) {
            // SAFETY: kill(2) with a negative pid only sends a signal to that process group.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        }
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

/// `pw-record` 1.3.81+ needs `--raw` to write headerless PCM to stdout; older versions reject
/// the flag and already write raw PCM. We read its `--help`, as Hex does.
fn pw_record_takes_raw() -> bool {
    Command::new("pw-record").arg("--help").output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("--raw") || String::from_utf8_lossy(&o.stderr).contains("--raw"))
        .unwrap_or(false)
}

fn capture_command(speaker: Speaker) -> Result<Command, String> {
    if has("pw-record") {
        let mut c = Command::new("pw-record");
        c.args(["--rate", "16000", "--channels", "1", "--format", "s16"]);
        if speaker == Speaker::Them {
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
        if speaker == Speaker::Them {
            c.arg("--device=@DEFAULT_MONITOR@");
        }
        return Ok(c);
    }
    Err("Found neither `pw-record` (PipeWire) nor `parec` to record audio. On Arch/Omarchy: `sudo pacman -S pipewire`.".into())
}

/// Starts capturing one track; blocks (~100 ms) arrive on `tx`.
pub fn start(speaker: Speaker, tx: Sender<Block>) -> Result<Capture, String> {
    let mut child = capture_command(speaker)?
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start capturing {}: {e}", speaker.name()))?;
    let mut out = child.stdout.take().ok_or("no stdout")?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::spawn(move || {
        let mut reader = PcmReader::default();
        let mut buf = vec![0u8; 3200]; // 100 ms of s16 mono at 16 kHz
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

/// Turns s16le bytes into f32, skipping a WAV header if the program writes one.
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
                // Find the "data" chunk and skip its 8-byte header.
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

/// Reads a WAV (any rate, mono or stereo) as 16 kHz mono f32.
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

/// Linear resampling: good enough for speech.
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
    fn skips_the_wav_header_and_joins_split_bytes() {
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
    fn raw_pcm_without_header() {
        let bytes: Vec<u8> = (0..30).flat_map(|_| 8192i16.to_le_bytes()).collect();
        let out = PcmReader::default().push(&bytes);
        assert_eq!(out.len(), 30);
        assert!(out.iter().all(|&s| s == 0.25));
    }

    #[test]
    fn resamples_to_16k() {
        let pcm = vec![0.0; 48_000];
        assert_eq!(resample(&pcm, 48_000, 16_000).len(), 16_000);
    }
}
