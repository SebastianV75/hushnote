//! A recording in progress: capture → segmenter → speech model, each on its own thread.
//! The UI receives [`Event`]s on a channel and a `waker` wakes it up.

use crate::audio::{self, Block, Capture, RATE, Speaker};
use crate::segmenter::Segmenter;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use transcribe_cpp::{Model, RunOptions, Session, TimestampKind};

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub speaker: Speaker,
    pub t_ms: u64,
    pub text: String,
}

#[derive(Debug)]
pub enum Event {
    Line(Line),
    Status(String),
    Error(String),
    /// All transcription is done (after stopping).
    Finished,
}

pub type Wake = Arc<dyn Fn() + Send + Sync>;
pub type Levels = Arc<dyn Fn(Speaker, f32) + Send + Sync>;

/// The model loads once and is reused across meetings.
static MODEL: Mutex<Option<(PathBuf, Arc<Model>)>> = Mutex::new(None);

pub fn load_model(path: &Path) -> Result<Arc<Model>, String> {
    let mut slot = MODEL.lock().unwrap();
    if let Some((p, m)) = slot.as_ref() {
        if p == path {
            return Ok(m.clone());
        }
    }
    if !path.exists() {
        return Err(format!(
            "Speech model not found at {}. Download it with `scripts/download-model.sh`.",
            path.display()
        ));
    }
    let model = Arc::new(Model::load(path).map_err(|e| format!("could not load the speech model: {e}"))?);
    *slot = Some((path.to_path_buf(), model.clone()));
    Ok(model)
}

/// Reads the model file once in the background so the kernel keeps it in the page cache and
/// the first meeting doesn't wait on the disk. It costs no memory of the app's own: the cache is
/// reclaimed if anything else needs the RAM. (`posix_fadvise(WILLNEED)` would be the polite way,
/// but some kernels read only the first few MB.)
pub fn prewarm(path: &Path) {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else { return };
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 1 << 20];
        while matches!(file.read(&mut buf), Ok(n) if n > 0) {}
    });
}

struct Job {
    speaker: Speaker,
    t_ms: u64,
    pcm: Vec<f32>,
}

/// Speech from the same person that goes before each phrase. Parakeet v3 picks the language by
/// itself and can't be told which one; on a short phrase alone it sometimes guesses wrong ("Sì,
/// claro") or mishears a word ("lo vemos Mayara"). With a few seconds of what that person said
/// before, it hears that the conversation is in Spanish and gets both right.
const CONTEXT: usize = RATE as usize * 5;
/// The context for a second try when the first one still came out in another alphabet.
const LONG_CONTEXT: usize = RATE as usize * 15;
/// Silence between the context and the phrase, so the two don't run into one word.
const GAP: usize = RATE as usize * 3 / 10;

/// Transcribes `pcm` after the tail of `before`, keeping only the phrase's own words.
fn transcribe_phrase(session: &mut Session, options: &RunOptions, before: &[f32], pcm: &[f32]) -> Result<String, String> {
    let language = options.language.as_deref().unwrap_or("es");
    let mut best: Option<String> = None;
    for context in [CONTEXT, LONG_CONTEXT] {
        let context = &before[before.len().saturating_sub(context)..];
        let mut text = with_context(session, options, context, pcm)?;
        if text.is_empty() && !context.is_empty() {
            // The phrase got absorbed into the context: try it on its own.
            text = with_context(session, options, &[], pcm)?;
        }
        let worse = |b: &String| foreign_letters(&text, language) >= foreign_letters(b, language);
        if !best.as_ref().is_some_and(worse) {
            best = Some(text);
        }
        if best.as_ref().is_some_and(|b| foreign_letters(b, language) == 0) || context.len() == before.len() {
            break;
        }
    }
    Ok(best.unwrap_or_default())
}

fn with_context(session: &mut Session, options: &RunOptions, context: &[f32], pcm: &[f32]) -> Result<String, String> {
    if context.is_empty() {
        return session.run(pcm, options).map(|t| t.text.trim().to_string()).map_err(|e| e.to_string());
    }
    let mut audio = Vec::with_capacity(context.len() + GAP + pcm.len());
    audio.extend_from_slice(context);
    audio.resize(context.len() + GAP, 0.0);
    audio.extend_from_slice(pcm);
    let start_ms = ((context.len() + GAP) * 1000 / RATE as usize) as i64;
    let t = session.run(&audio, options).map_err(|e| e.to_string())?;
    // A word belongs to the phrase if most of it is after the gap.
    let words: Vec<&str> = t.words.iter().filter(|w| (w.t0_ms + w.t1_ms) / 2 >= start_ms).map(|w| w.text.trim()).collect();
    Ok(capitalize(&words.join(" ")))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Letters that Spanish or English text doesn't use: a phrase in the wrong language ("Sì",
/// Cyrillic, Japanese…).
fn foreign_letters(text: &str, language: &str) -> usize {
    let ours = |c: char| c.is_ascii() || (language == "es" && "áéíóúüñÁÉÍÓÚÜÑ".contains(c));
    text.chars().filter(|c| c.is_alphabetic() && !ours(*c)).count()
}

/// Thread that transcribes chunks in order; sends `Finished` when its channel closes.
fn spawn_transcriber(model_path: PathBuf, language: Option<String>, events: Sender<Event>, wake: Wake) -> Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    std::thread::spawn(move || {
        let send = |e: Event| {
            let _ = events.send(e);
            wake();
        };
        let session = load_model(&model_path).and_then(|m| m.session().map_err(|e| e.to_string()));
        let mut session = match session {
            Ok(s) => s,
            Err(e) => {
                send(Event::Error(e));
                for _ in rx {} // drain the queue so nobody blocks
                send(Event::Finished);
                return;
            }
        };
        // Word times tell which words belong to the phrase and which to the context before it.
        let options = RunOptions { language: language.clone(), timestamps: TimestampKind::Word, ..Default::default() };
        let mut detector = crate::vad::Detector::load();
        let mut heard: HashMap<Speaker, Vec<f32>> = HashMap::new();
        for mut job in rx {
            // Silero drops phrases without speech and trims the rest (see vad.rs).
            match detector.as_mut().and_then(|d| d.check(&job.pcm)) {
                Some(crate::vad::Verdict::Silence) => continue,
                Some(crate::vad::Verdict::Speech(r)) if r != (0..job.pcm.len()) => {
                    job.t_ms += r.start as u64 * 1000 / RATE as u64;
                    job.pcm = job.pcm[r].to_vec();
                }
                _ => {}
            }
            let before = heard.entry(job.speaker).or_default();
            match transcribe_phrase(&mut session, &options, before, &job.pcm) {
                Ok(text) => {
                    if !text.is_empty() {
                        send(Event::Line(Line { speaker: job.speaker, t_ms: job.t_ms, text }));
                    }
                }
                Err(e) => send(Event::Error(format!("transcription: {e}"))),
            }
            before.extend_from_slice(&job.pcm);
            let excess = before.len().saturating_sub(LONG_CONTEXT);
            before.drain(..excess);
        }
        send(Event::Finished);
    });
    tx
}

/// Thread that hands audio blocks to one segmenter per track and queues the chunks.
fn spawn_splitter(blocks: Receiver<Block>, jobs: Sender<Job>, levels: Levels) {
    std::thread::spawn(move || {
        let mut segmenters: HashMap<Speaker, Segmenter> = HashMap::new();
        let job = |speaker: Speaker, c: crate::segmenter::Chunk| Job {
            speaker,
            t_ms: c.start as u64 * 1000 / RATE as u64,
            pcm: c.pcm,
        };
        for block in blocks {
            let seg = segmenters.entry(block.speaker).or_default();
            for chunk in seg.push(&block.pcm) {
                let _ = jobs.send(job(block.speaker, chunk));
            }
            levels(block.speaker, seg.level());
        }
        for (speaker, mut seg) in segmenters {
            if let Some(chunk) = seg.flush() {
                let _ = jobs.send(job(speaker, chunk));
            }
            levels(speaker, 0.0);
        }
        // Dropping `jobs` lets the transcriber finish the queue and send `Finished`.
    });
}

pub struct Recording {
    captures: Vec<Capture>,
    // Dropped on stop, to close the block channel.
    blocks: Option<Sender<Block>>,
}

impl Recording {
    /// Records the microphone and, if asked, the system audio.
    pub fn start(
        model: PathBuf,
        language: Option<String>,
        system_audio: bool,
        events: Sender<Event>,
        wake: Wake,
        levels: Levels,
    ) -> Result<Recording, String> {
        // Capture first: if there is no way to record, don't load the model for nothing.
        let (btx, brx) = mpsc::channel();
        let mut captures = vec![audio::start(Speaker::Me, btx.clone())?];
        if system_audio {
            match audio::start(Speaker::Them, btx.clone()) {
                Ok(c) => captures.push(c),
                Err(e) => {
                    let _ = events.send(Event::Status(format!("Microphone only: {e}")));
                    wake();
                }
            }
        }
        let jobs = spawn_transcriber(model, language, events, wake);
        spawn_splitter(brx, jobs, levels);
        Ok(Recording { captures, blocks: Some(btx) })
    }

    /// Stops capturing; pending transcription finishes on its own and `Finished` arrives.
    pub fn stop(mut self) {
        for c in self.captures.drain(..) {
            c.stop();
        }
        self.blocks.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_other_alphabets() {
        assert_eq!(foreign_letters("Sí, claro, mañana a las tres. ¿Vale?", "es"), 0);
        assert_eq!(foreign_letters("Sì, claro.", "es"), 1);
        assert_eq!(foreign_letters("ありがとう", "es"), 5);
        assert_eq!(foreign_letters("Привет", "en"), 6);
        assert_eq!(foreign_letters("Mañana", "en"), 1);
    }

    #[test]
    fn capitalizes_the_first_letter() {
        assert_eq!(capitalize("sí, claro."), "Sí, claro.");
        assert_eq!(capitalize(""), "");
    }

    /// Real Spanish speech through the speech model: the whole recording, and its last seconds as
    /// a short phrase after the rest as context. Neither may come out with letters Spanish
    /// doesn't use, and the words in `HUSHNOTE_TEST_WORDS` (comma separated, optional) must be
    /// in the whole transcript. The audio isn't in the repo (it's someone's voice):
    /// `HUSHNOTE_TEST_AUDIO=/path/clip.wav HUSHNOTE_TEST_WORDS=tarea,exámenes \
    ///  cargo test --release real_speech -- --ignored --nocapture`
    #[test]
    #[ignore = "needs the speech model and HUSHNOTE_TEST_AUDIO"]
    fn real_speech_comes_out_in_spanish() {
        let path = std::env::var("HUSHNOTE_TEST_AUDIO").expect("set HUSHNOTE_TEST_AUDIO to a WAV with Spanish speech");
        let pcm = audio::read_wav(Path::new(&path)).unwrap();
        let model = crate::models::for_language("es").path();
        let mut session = load_model(&model).unwrap().session().unwrap();
        let options = RunOptions { language: Some("es".into()), timestamps: TimestampKind::Word, ..Default::default() };

        let whole = transcribe_phrase(&mut session, &options, &[], &pcm).unwrap();
        eprintln!("whole:        {whole}");
        assert!(!whole.is_empty(), "nothing was transcribed");
        assert_eq!(foreign_letters(&whole, "es"), 0, "letters Spanish doesn't use: {whole}");
        let lower = whole.to_lowercase();
        for word in std::env::var("HUSHNOTE_TEST_WORDS").unwrap_or_default().split(',').map(str::trim).filter(|w| !w.is_empty()) {
            assert!(lower.contains(&word.to_lowercase()), "missing \"{word}\" in: {whole}");
        }

        let cut = pcm.len().saturating_sub(RATE as usize * 4);
        let (before, phrase) = pcm.split_at(cut);
        let alone = transcribe_phrase(&mut session, &options, &[], phrase).unwrap();
        let with = transcribe_phrase(&mut session, &options, before, phrase).unwrap();
        eprintln!("last 4 s alone:        {alone}");
        eprintln!("last 4 s with context: {with}");
        assert!(!with.is_empty(), "the phrase got lost with its context");
        assert_eq!(foreign_letters(&with, "es"), 0, "letters Spanish doesn't use: {with}");
    }
}

/// Transcribes a WAV file through the same pipeline (for importing audio).
pub fn transcribe_file(path: &Path, model: PathBuf, language: Option<String>, events: Sender<Event>, wake: Wake) -> Result<(), String> {
    let pcm = audio::read_wav(path)?;
    let jobs = spawn_transcriber(model, language, events, wake);
    let (btx, brx) = mpsc::channel();
    spawn_splitter(brx, jobs, Arc::new(|_, _| {}));
    for block in pcm.chunks(RATE as usize / 10) {
        let _ = btx.send(Block { speaker: Speaker::Audio, pcm: block.to_vec() });
    }
    Ok(())
}
