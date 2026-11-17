//! Silero voice activity detection (v6.2, the file Hex uses), to keep audio without speech away
//! from the speech model, which tends to invent text for it ("Thank you.", "Gracias.").
//!
//! It works as Hex's `speech_detector` (MIT): the energy segmenter still cuts the audio into
//! phrases, and each phrase is checked here before it is transcribed. A phrase with no speech is
//! dropped; one with speech is trimmed to it, with a margin. The detector is optional: while
//! its file is missing (or it fails), every phrase is transcribed as before.

use crate::audio::RATE;
use crate::config;
use std::ops::Range;
use std::path::PathBuf;
use transcribe_cpp::{Backend, Model, ModelOptions, VadOptions, VadSession};

pub const FILE: &str = "silero-vad-v6.2-F32.gguf";

/// Speech shorter than this is ignored. Silero's default of 250 ms drops a short "Ok" or "Sí";
/// 160 ms keeps those and still rejects clicks, keys and room noise (Hex's value).
const MIN_SPEECH_MS: i32 = 160;

/// Speech probability that counts as speech. Silero's default of 0.5 misses quiet speech;
/// 0.4 hears it and still rejects the silence in a recorded meeting (Hex's value).
const THRESHOLD: f64 = 0.4;

/// Audio kept on each side of the speech, so the model hears the words' edges.
const MARGIN: usize = RATE as usize / 4;

pub fn path() -> PathBuf {
    config::data_dir().join("models").join(FILE)
}

pub enum Verdict {
    /// The samples to transcribe.
    Speech(Range<usize>),
    /// Nothing worth transcribing.
    Silence,
}

pub struct Detector {
    session: VadSession,
}

impl Detector {
    /// Loads the detector if its file is installed. On the CPU it loads in tens of milliseconds.
    pub fn load() -> Option<Detector> {
        let path = path();
        if !path.exists() {
            return None;
        }
        let model = Model::load_with(&path, &ModelOptions { backend: Backend::Cpu, ..Default::default() }).ok()?;
        Some(Detector { session: model.vad_session().ok()? })
    }

    /// Where the speech is in 16 kHz mono `pcm`, or `None` if the detector failed (then the
    /// caller transcribes everything, as without a detector).
    pub fn check(&mut self, pcm: &[f32]) -> Option<Verdict> {
        let options = VadOptions { threshold: THRESHOLD, min_speech_ms: MIN_SPEECH_MS, ..Default::default() };
        let segments = self.session.run(pcm, &options).ok()?.segments;
        Some(match bounds(segments.iter().map(|s| (s.start_sample, s.end_sample)), pcm.len()) {
            Some(speech) => Verdict::Speech(widen(pcm.len(), speech, MARGIN)),
            None => Verdict::Silence,
        })
    }
}

/// The span of `segments` (first speech sample, first sample after it) in a clip of `len`
/// samples, clamped to the clip; `None` if there is no speech inside it.
fn bounds(segments: impl Iterator<Item = (i64, i64)>, len: usize) -> Option<Range<usize>> {
    let (mut first, mut last) = (i64::MAX, i64::MIN);
    for (start, end) in segments {
        first = first.min(start);
        last = last.max(end);
    }
    if first > last {
        return None;
    }
    let clamp = |v: i64| usize::try_from(v.max(0)).unwrap_or(usize::MAX).min(len);
    let (start, end) = (clamp(first), clamp(last));
    (start < end).then_some(start..end)
}

/// `speech` widened by `margin` samples on each side, clamped to a clip of `len` samples.
fn widen(len: usize, speech: Range<usize>, margin: usize) -> Range<usize> {
    let start = speech.start.min(len).saturating_sub(margin);
    let end = speech.end.saturating_add(margin).min(len);
    start..end.max(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_bound_the_speech_inside_the_clip() {
        assert_eq!(bounds([(1_000, 4_000), (9_000, 12_000)].into_iter(), 16_000), Some(1_000..12_000));
        assert_eq!(bounds([(12_000, 20_000)].into_iter(), 16_000), Some(12_000..16_000));
        assert_eq!(bounds([].into_iter(), 16_000), None);
        assert_eq!(bounds([(16_000, 18_000)].into_iter(), 16_000), None);
    }

    #[test]
    fn the_margin_stops_at_the_clip_edges() {
        assert_eq!(widen(48_000, 16_000..32_000, 4_000), 12_000..36_000);
        assert_eq!(widen(48_000, 2_000..4_000, 4_000), 0..8_000);
        assert_eq!(widen(48_000, 44_000..47_000, 4_000), 40_000..48_000);
    }

    /// With the real detector: silence and noise are rejected, speech is kept.
    /// `cargo test vad -- --ignored` (needs scripts/download-model.sh vad).
    #[test]
    #[ignore = "needs the Silero VAD file"]
    fn real_detector_rejects_silence_and_keeps_speech() {
        let mut d = Detector::load().expect("Silero VAD installed");
        assert!(matches!(d.check(&vec![0.0; RATE as usize * 2]), Some(Verdict::Silence)));
        let mut seed = 1u32;
        let noise: Vec<f32> = (0..RATE * 2)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 * 0.02 - 0.01
            })
            .collect();
        assert!(matches!(d.check(&noise), Some(Verdict::Silence)), "faint noise is not speech");
        let wav = std::env::var("HUSHNOTE_VAD_WAV").expect("HUSHNOTE_VAD_WAV: a 16 kHz WAV with speech");
        let speech = crate::audio::read_wav(std::path::Path::new(&wav)).unwrap();
        let mut padded = vec![0.0; RATE as usize * 2];
        padded.extend_from_slice(&speech);
        padded.extend(vec![0.0; RATE as usize * 2]);
        match d.check(&padded) {
            Some(Verdict::Speech(r)) => {
                eprintln!("speech {:.2}..{:.2} s of {:.2} s", r.start as f32 / RATE as f32, r.end as f32 / RATE as f32, padded.len() as f32 / RATE as f32);
                assert!(r.start > RATE as usize, "the leading silence is trimmed");
                assert!(r.end < padded.len() - RATE as usize, "the trailing silence is trimmed");
            }
            _ => panic!("speech not heard"),
        }
    }
}
