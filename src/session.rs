//! Una grabación en marcha: captura → segmentador → modelo de voz, cada uno en su hilo.
//! La interfaz recibe [`Event`]s por un canal y un `waker` la despierta.

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
    /// Toda la transcripción terminó (después de detener).
    Finished,
}

pub type Wake = Arc<dyn Fn() + Send + Sync>;
pub type Levels = Arc<dyn Fn(Speaker, f32) + Send + Sync>;

/// El modelo se carga una vez y se reutiliza entre reuniones.
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
            "No encuentro el modelo de voz en {}. Descárgalo con `scripts/descargar-modelo.sh`.",
            path.display()
        ));
    }
    let model = Arc::new(Model::load(path).map_err(|e| format!("no pude cargar el modelo: {e}"))?);
    *slot = Some((path.to_path_buf(), model.clone()));
    Ok(model)
}

struct Job {
    speaker: Speaker,
    t_ms: u64,
    pcm: Vec<f32>,
}

/// Hilo que transcribe fragmentos en orden; manda `Finished` cuando se cierra su canal.
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
                for _ in rx {} // vacía la cola para que nadie se bloquee
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
                Err(e) => send(Event::Error(format!("transcripción: {e}"))),
            }
        }
        send(Event::Finished);
    });
    tx
}

/// Hilo que reparte los bloques de audio a un segmentador por pista y encola los fragmentos.
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
        // Al soltar `jobs`, el transcriptor termina la cola y avisa `Finished`.
    });
}

pub struct Recording {
    captures: Vec<Capture>,
    // Se suelta al detener, para cerrar el canal de bloques.
    blocks: Option<Sender<Block>>,
}

impl Recording {
    /// Graba el micrófono y, si se pide, el audio del sistema.
    pub fn start(
        model: PathBuf,
        language: Option<String>,
        system_audio: bool,
        events: Sender<Event>,
        wake: Wake,
        levels: Levels,
    ) -> Result<Recording, String> {
        let jobs = spawn_transcriber(model, language, events.clone(), wake.clone());
        let (btx, brx) = mpsc::channel();
        spawn_splitter(brx, jobs, levels);
        let mut captures = vec![audio::start(Speaker::Yo, btx.clone())?];
        if system_audio {
            match audio::start(Speaker::Ellos, btx.clone()) {
                Ok(c) => captures.push(c),
                Err(e) => {
                    let _ = events.send(Event::Status(format!("Solo micrófono: {e}")));
                    wake();
                }
            }
        }
        Ok(Recording { captures, blocks: Some(btx) })
    }

    /// Detiene la captura; la transcripción pendiente termina sola y llega `Finished`.
    pub fn stop(mut self) {
        for c in self.captures.drain(..) {
            c.stop();
        }
        self.blocks.take();
    }
}

/// Transcribe un archivo WAV con la misma tubería (para importar audio).
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
