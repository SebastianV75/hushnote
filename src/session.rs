//! A recording in progress: capture → segmenter → speech model, each on its own thread.
//! The UI receives [`Event`]s on a channel and a `waker` wakes it up.

use crate::audio::{self, Block, Capture, RATE, Speaker};
use crate::segmenter::Segmenter;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use transcribe_cpp::{Model, RunOptions};

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

struct Job {
    speaker: Speaker,
    t_ms: u64,
    pcm: Vec<f32>,
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
        let options = RunOptions { language, ..Default::default() };
        for job in rx {
            match session.run(&job.pcm, &options) {
                Ok(t) => {
                    let text = t.text.trim().to_string();
                    if !text.is_empty() {
                        send(Event::Line(Line { speaker: job.speaker, t_ms: job.t_ms, text }));
                    }
                }
                Err(e) => send(Event::Error(format!("transcription: {e}"))),
            }
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
