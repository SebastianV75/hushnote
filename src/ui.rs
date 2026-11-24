//! The window, laid out like OpenCode: a sidebar without a background of its own (Home,
//! Search, New session, New notebook, then the notebooks, one group per subject), and a bordered main panel with the live
//! transcript as a chat in a 720 point column, a composer for "My notes", and a footer with the
//! notes folder and the You/Them level meters. After the meeting the summary reads like an
//! assistant's answer: Key points, Decisions, Action items (checkboxes, due day on the right)
//! and Open questions, with the headings in the meeting's language.
//!
//! While recording, a capsule overlay at the top centre of the screen shows a red dot, the time
//! and the last phrase (on Wayland it is a layer-shell surface and takes no clicks).

use crate::audio::Speaker;
use crate::config::{Config, Summaries};
use crate::export;
use crate::llm::{self, Llm, Scope, clock};
use crate::session::{Event, Line, Recording};
use crate::store::{self, Card, Meeting, Mode, Turn};
use crate::update::{self, Version};
use rune::tok::TextStyle;
use rune::{
    Align, App, AppSpec, Color, Cx, El, Icon, Live, OverlayHandle, OverlaySpec, ScreenEdge, ScrollSpec, TextEdit, Tx,
    Waker, col, el, icon, row, text, text_input,
};
use rune_kit::{Markdown, MarkdownStyle};
use crate::{models, ollama};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

// ---- Look ----------------------------------------------------------------------------------

fn hex(s: &str) -> Color {
    rune::color::hex(s).unwrap_or_default()
}

/// The palette (dark first, as Rune and OpenCode).
struct P;
impl P {
    fn window() -> Color { hex("#0C0C0D") }
    fn panel() -> Color { hex("#141415") }
    fn raised() -> Color { hex("#1D1D1F") }
    fn line() -> Color { hex("#29292C") }
    fn code() -> Color { hex("#232326") }
    fn well() -> Color { hex("#111112") }
    fn text() -> Color { hex("#EDEDEE") }
    fn secondary() -> Color { hex("#8C8C92") }
    fn dim() -> Color { hex("#5C5C62") }
    fn accent() -> Color { hex("#4D8DFF") }
    fn rec() -> Color { hex("#F2564B") }
    fn warn() -> Color { hex("#F2C14E") }
    fn meter_off() -> Color { hex("#38383C") }
    fn clear() -> Color { Color { a: 0.0, ..hex("#141415") } }
}


const FONTS: &[&[u8]] = &[include_bytes!("../fonts/geist.ttf"), include_bytes!("../fonts/geist-mono.ttf")];
const FAMILIES: (&str, &str) = ("Geist", "Geist Mono");

fn sans(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight)
}
fn mono(size: f32) -> TextStyle {
    TextStyle::new(size, 400.0).mono().features("'tnum' 1")
}

static HOME: Icon = Icon {
    name: "hn-home",
    view: 16.0,
    body: r#"<rect x="2.5" y="2.5" width="4.25" height="4.25" rx="1" stroke="currentColor"/><rect x="9.25" y="2.5" width="4.25" height="4.25" rx="1" stroke="currentColor"/><rect x="2.5" y="9.25" width="4.25" height="4.25" rx="1" stroke="currentColor"/><rect x="9.25" y="9.25" width="4.25" height="4.25" rx="1" stroke="currentColor"/>"#,
};
static SEARCH: Icon = Icon {
    name: "hn-search",
    view: 16.0,
    body: r#"<circle cx="7.25" cy="7.25" r="4.5" stroke="currentColor"/><path d="M10.6 10.6L13.5 13.5" stroke="currentColor" stroke-linecap="round"/>"#,
};
static NEW: Icon = Icon {
    name: "hn-new",
    view: 16.0,
    body: r#"<path d="M7.5 2.75H4A1.25 1.25 0 0 0 2.75 4V12A1.25 1.25 0 0 0 4 13.25H12A1.25 1.25 0 0 0 13.25 12V8.5" stroke="currentColor" stroke-linecap="round"/><path d="M11.6 2.4A1.3 1.3 0 0 1 13.6 4.4L8.4 9.6L6 10.2L6.6 7.8Z" stroke="currentColor" stroke-linejoin="round"/>"#,
};
static CHEVRON: Icon = Icon {
    name: "hn-chevron",
    view: 16.0,
    body: r#"<path d="M4.5 6.5L8 10L11.5 6.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

static CHEVRON_RIGHT: Icon = Icon {
    name: "hn-chevron-right",
    view: 16.0,
    body: r#"<path d="M6.5 4.5L10 8L6.5 11.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"/>"#,
};
static PLUS: Icon = Icon {
    name: "hn-plus",
    view: 16.0,
    body: r#"<path d="M8 3.5V12.5M3.5 8H12.5" stroke="currentColor" stroke-linecap="round"/>"#,
};
static BOOK: Icon = Icon {
    name: "hn-book",
    view: 16.0,
    body: r#"<path d="M3.5 3.25A1.25 1.25 0 0 1 4.75 2H12.5V11.5H4.75A1.25 1.25 0 0 0 3.5 12.75Z" stroke="currentColor" stroke-linejoin="round"/><path d="M3.5 12.75A1.25 1.25 0 0 0 4.75 14H12.5V11.5" stroke="currentColor" stroke-linejoin="round"/>"#,
};
static CHECK: Icon = Icon {
    name: "hn-check",
    view: 16.0,
    body: r#"<path d="M3.5 8.6L6.6 11.6L12.5 4.8" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

const METER_BARS: usize = 5;
const AUTOSAVE_S: f64 = 2.0;
const COLUMN: f32 = 720.0;
/// The column of Home and of a notebook cover: four cards of [`CARD_W`] across.
const COLUMN_WIDE: f32 = 960.0;
const CARD_W: f32 = 212.0;
const CARD_H: f32 = 172.0;
const CARD_GAP: f32 = 24.0;
/// The sheets of a notebook cover: four across, like the folders.
const SHEET_W: f32 = 212.0;
const SHEET_H: f32 = 204.0;

fn language_label(code: &str) -> String {
    models::LANGUAGES.iter().find(|(c, _)| *c == code).map(|(_, l)| l.to_string()).unwrap_or_else(|| code.to_uppercase())
}

/// `/home/me/Documents/Hushnote` → `~/Documents/Hushnote`.
fn tilde(path: &Path) -> String {
    match dirs::home_dir() {
        Some(home) if path.starts_with(&home) => format!("~/{}", path.strip_prefix(&home).unwrap().display()),
        _ => path.display().to_string(),
    }
}

/// "2026-11-18 09:00" → "18 Nov".
fn short_date(date: &str) -> String {
    chrono::NaiveDate::parse_from_str(date.get(..10).unwrap_or(""), "%Y-%m-%d")
        .map(|d| d.format("%-d %b").to_string())
        .unwrap_or_default()
}

/// "2026-11-17 10:00" → "Tue 17 Nov".
fn pretty_date(date: &str) -> String {
    chrono::NaiveDate::parse_from_str(date.get(..10).unwrap_or(""), "%Y-%m-%d")
        .map(|d| d.format("%a %-d %b").to_string())
        .unwrap_or_else(|_| date.to_string())
}

fn minutes(ms: u64) -> String {
    if ms > 0 && ms < 60_000 {
        return format!("{} s", ms.div_ceil(1000));
    }
    let m = (ms + 30_000) / 60_000;
    if m >= 60 { format!("{} h {:02} min", m / 60, m % 60) } else { format!("{m} min") }
}

// ---- State ---------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum View {
    Transcript,
    Summary,
    /// A class's review questions, one card at a time.
    Review,
    /// A chat about the session's notes, or about all the classes of its subject.
    Ask,
}

/// A question being answered: the answer so far shows in the chat.
struct Asking {
    file: PathBuf,
    question: String,
    /// The subject when it is about all its classes, else empty.
    scope: String,
    partial: String,
}

enum Rec {
    Idle,
    /// Capturing into `file`. `t0` is the app clock at the start; `offset_ms` is where this
    /// stretch starts in the meeting (non-zero when a meeting is resumed).
    Live { handle: Option<Recording>, t0: f64, offset_ms: u64, file: PathBuf },
    /// Capture stopped; the last phrases are still being transcribed.
    Finishing { offset_ms: u64, end_ms: u64, file: PathBuf },
}

enum LlmMsg {
    Delta(PathBuf, String),
    /// The summary, and who wrote it ("Qwen3 4B", "Claude"…).
    Done(PathBuf, Result<String, String>, String),
    Title(PathBuf, String),
    /// The review questions, or why they could not be written.
    Review(PathBuf, Result<Vec<Card>, String>),
    /// A piece of the answer to a question about the file's session.
    AskDelta(PathBuf, String),
    /// The whole answer, or why there is none.
    Asked(PathBuf, Result<String, String>),
    State(ollama::State),
}

#[derive(Clone, Copy, PartialEq)]
enum Fetch {
    Speech,
    Llm,
}

/// A one-click download in progress (a speech model, or the summary model through Ollama).
struct Download {
    what: Fetch,
    have: u64,
    total: u64,
    cancel: Arc<AtomicBool>,
}

enum DlMsg {
    Progress(u64, u64),
    Done(Fetch, Result<String, String>),
}

/// Where an update stands, once the notice's "Update" was pressed.
enum Upgrade {
    Idle,
    Downloading { version: Version, have: u64, total: u64 },
    /// The new binary is in place; it runs after a restart.
    Ready(Version),
    Failed(Version, String),
}

enum UpMsg {
    /// A check ended; `explicit` is "Check now", which reports errors.
    Checked { explicit: bool, result: Result<Version, String> },
    Progress(u64, u64),
    Done(Version, Result<(), String>),
}

/// Everything about updates (see `update`).
struct Updates {
    /// What was found by the last good check, and whether its notice was dismissed.
    state: update::State,
    /// The binary to replace, or why this copy can't update itself. Resolved once, at startup.
    target: Result<PathBuf, String>,
    phase: Upgrade,
    checking: bool,
    /// The outcome of the last "Check now", and whether it is an error.
    note: Option<(String, bool)>,
    /// Whether recording or other work is on (read by the installer right before the swap).
    blocked: Arc<AtomicBool>,
    /// "Restart" was pressed; `poll` relaunches.
    restart: bool,
}

impl Updates {
    fn new() -> Updates {
        Updates {
            state: update::State::load(),
            target: update::install_target(),
            phase: Upgrade::Idle,
            checking: false,
            note: None,
            blocked: Arc::new(AtomicBool::new(false)),
            restart: false,
        }
    }
}

struct Meters {
    me: Vec<Live<Color>>,
    them: Vec<Live<Color>>,
}

impl Meters {
    fn new() -> Meters {
        let bars = || (0..METER_BARS).map(|_| Live::new(P::meter_off())).collect();
        Meters { me: bars(), them: bars() }
    }
}

fn set_meter(bars: &[Live<Color>], level: f32) {
    let lit = (level.clamp(0.0, 1.0) * METER_BARS as f32).ceil() as usize;
    for (i, bar) in bars.iter().enumerate() {
        bar.set(if i < lit { P::accent() } else { P::meter_off() });
    }
}

pub struct Ui {
    config: Config,
    meetings: Vec<Meeting>,
    /// `None` is Home, or a notebook cover when `notebook` is set.
    selected: Option<usize>,
    /// The notebook cover on screen (`store::Group::key`); only with `selected == None`.
    notebook: Option<String>,
    /// The cover is running "Review all": the flashcards of every class of the notebook.
    nb_review: bool,
    notes: TextEdit,
    view: View,
    rec: Rec,
    /// The app clock, in seconds, as of the last poll.
    now: f64,
    events: (Sender<Event>, Receiver<Event>),
    llm: (Sender<LlmMsg>, Receiver<LlmMsg>),
    wake: Waker,
    meters: Meters,
    summarizing: Option<PathBuf>,
    /// The class whose review questions are being written.
    reviewing: Option<PathBuf>,
    /// The card on screen in the review, and whether its answer is showing.
    card: usize,
    revealed: bool,
    /// The question box of the Ask view.
    ask_input: TextEdit,
    /// Whether the Ask scope is all the classes of the subject (when that is possible).
    ask_all: bool,
    asking: Option<Asking>,
    /// A message for the footer, and whether it is a warning.
    status: Option<(String, bool)>,
    /// Meetings with something new (a summary arrived while looking elsewhere).
    unseen: HashSet<PathBuf>,
    /// Meetings whose transcription or summary failed.
    warned: HashSet<PathBuf>,
    /// Sessions whose last save failed: their edits live only in memory until a save works.
    unsaved: HashSet<PathBuf>,
    search: Option<TextEdit>,
    /// When the open meeting changed without being saved yet.
    dirty: Option<f64>,
    /// The subject of the open session was edited since the last save: it may need a new folder.
    subject_touched: bool,
    /// Sessions that should move to their ideal path but are busy (recording, summarizing…).
    relocate_pending: HashSet<PathBuf>,
    /// Scenes for screenshots fill in sample meetings that must never be written to disk.
    demo: bool,
    capsule: Option<OverlayHandle<Capsule>>,
    capsule_shown: (String, String),
    download: Option<Download>,
    dl: (Sender<DlMsg>, Receiver<DlMsg>),
    /// Whether the summary model is there (`None` while it is being checked).
    llm_state: Option<ollama::State>,
    /// Screenshot scenes decide this instead of the disk.
    speech_ready_override: Option<bool>,
    /// What "New session" creates; chosen before the recording starts.
    new_mode: Mode,
    /// The subject box in the header, for the open session.
    subject: TextEdit,
    /// Sidebar groups (`store::Group::key`) the user collapsed; everything starts open.
    closed: HashSet<String>,
    /// The "Notebook name" box of "New notebook", while it is open.
    notebook_input: Option<TextEdit>,
    /// The footer message of the last export and the folder it went to, for "Open folder".
    exported: Option<(String, PathBuf)>,
    updates: Updates,
    up: (Sender<UpMsg>, Receiver<UpMsg>),
}

impl Ui {
    fn new(capsule: Option<OverlayHandle<Capsule>>) -> Ui {
        let config = Config::load();
        let meetings = store::list(&config.notes_dir);
        let notes = TextEdit::new(meetings.first().map(|m| m.notes.clone()).unwrap_or_default());
        let subject = TextEdit::new(meetings.first().map(|m| m.subject.clone()).unwrap_or_default());
        let mut ui = Ui {
            selected: (!meetings.is_empty()).then_some(0),
            notebook: None,
            nb_review: false,
            meetings,
            config,
            notes,
            view: View::Transcript,
            rec: Rec::Idle,
            now: 0.0,
            events: mpsc::channel(),
            llm: mpsc::channel(),
            wake: std::sync::Arc::new(|| {}),
            meters: Meters::new(),
            summarizing: None,
            reviewing: None,
            card: 0,
            revealed: false,
            ask_input: TextEdit::new(""),
            ask_all: false,
            asking: None,
            status: None,
            unseen: HashSet::new(),
            warned: HashSet::new(),
            unsaved: HashSet::new(),
            search: None,
            dirty: None,
            subject_touched: false,
            relocate_pending: HashSet::new(),
            demo: false,
            capsule,
            capsule_shown: Default::default(),
            download: None,
            dl: mpsc::channel(),
            llm_state: None,
            speech_ready_override: None,
            new_mode: Mode::Meeting,
            subject,
            closed: HashSet::new(),
            notebook_input: None,
            exported: None,
            updates: Updates::new(),
            up: mpsc::channel(),
        };
        if let Some(m) = ui.current() {
            ui.view = default_view(m);
        }
        crate::session::prewarm(&ui.config.speech_model);
        ui
    }

    fn speech_ready(&self) -> bool {
        self.speech_ready_override.unwrap_or_else(|| self.config.speech_model.exists())
    }

    /// The catalog entry for the configured model (the language's for a path outside it).
    fn speech_artifact(&self) -> &'static models::Artifact {
        models::for_path(&self.config.speech_model).unwrap_or_else(|| models::for_language(&self.config.language))
    }

    /// Whether summaries can be written now: Ollama has the model, or the CLI is installed.
    fn summaries_ready(&self) -> bool {
        match self.config.summaries {
            Summaries::Local => self.llm_state == Some(ollama::State::Ready),
            p => self.demo || p.installed(),
        }
    }

    /// Whether Home should offer to set up summaries (not while Ollama is still being asked).
    fn summaries_need_setup(&self) -> bool {
        match self.config.summaries {
            Summaries::Local => matches!(self.llm_state, Some(ollama::State::Missing | ollama::State::NotRunning)),
            _ => !self.summaries_ready(),
        }
    }

    /// Downloads the configured speech model (and Silero VAD) in the background.
    fn download_speech(&mut self) {
        if self.download.is_some() || self.demo {
            return;
        }
        let artifact = self.speech_artifact();
        if models::for_path(&self.config.speech_model).is_none() {
            self.config.speech_model = artifact.path();
            self.save_config();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.download = Some(Download { what: Fetch::Speech, have: 0, total: artifact.bytes, cancel: cancel.clone() });
        self.status = None;
        let (tx, wake) = (self.dl.0.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let mut shown = 0u64;
            let result = artifact
                .download(&cancel, |have, total| {
                    if have - shown >= total / 300 || have == total {
                        shown = have;
                        let _ = tx.send(DlMsg::Progress(have, total));
                        wake();
                    }
                })
                .and_then(|_| models::VAD.download(&cancel, |_, _| {}))
                .map(|_| format!("{} is ready. Press New session to start.", artifact.name));
            let _ = tx.send(DlMsg::Done(Fetch::Speech, result));
            wake();
        });
    }

    /// Pulls the summary model through Ollama in the background.
    fn download_llm(&mut self) {
        if self.download.is_some() || self.demo {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.download = Some(Download { what: Fetch::Llm, have: 0, total: 0, cancel });
        let (url, model) = (self.config.llm_url.clone(), self.config.llm_model.clone());
        let (tx, wake) = (self.dl.0.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let mut shown = 0u64;
            let result = ollama::pull(&url, &model, |have, total| {
                if have.abs_diff(shown) >= total / 300 || have == total {
                    shown = have;
                    let _ = tx.send(DlMsg::Progress(have, total));
                    wake();
                }
            })
            .map(|_| format!("{model} is ready for summaries."));
            let _ = tx.send(DlMsg::Done(Fetch::Llm, result));
            wake();
        });
    }

    fn cancel_download(&mut self) {
        if let Some(d) = &self.download {
            d.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Asks the summary server whether it has the model, in the background.
    fn check_llm(&mut self) {
        if self.demo {
            return;
        }
        self.llm_state = None;
        let (url, model) = (self.config.llm_url.clone(), self.config.llm_model.clone());
        let (tx, wake) = (self.llm.0.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let _ = tx.send(LlmMsg::State(ollama::check(&url, &model)));
            wake();
        });
    }

    /// Why the app must not install or restart now: it would interrupt or lose work.
    fn update_blocker(&self) -> Option<&'static str> {
        if !matches!(self.rec, Rec::Idle) {
            Some("Stop the recording first")
        } else if self.summarizing.is_some() || self.reviewing.is_some() || self.asking.is_some() {
            Some("Wait for the summary or answer to finish")
        } else if self.download.is_some() {
            Some("Wait for the download to finish")
        } else {
            None
        }
    }

    fn save_update_state(&mut self) {
        if !self.demo {
            let _ = self.updates.state.save();
        }
    }

    /// Asks GitHub for the latest release in the background. At startup (`explicit` false) it
    /// only happens when the setting is on and the last good check is a day old, and failures
    /// are silent; "Check now" always asks and says what happened.
    fn check_for_updates(&mut self, explicit: bool) {
        if self.demo || self.updates.checking {
            return;
        }
        if !explicit && !(self.config.check_updates && self.updates.state.due(update::now())) {
            return;
        }
        self.updates.checking = true;
        self.updates.note = None;
        let (tx, wake) = (self.up.0.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let _ = tx.send(UpMsg::Checked { explicit, result: update::check_latest() });
            wake();
        });
    }

    /// Downloads, checks and installs `version` in the background.
    fn begin_update(&mut self, version: Version) {
        if self.demo || self.update_blocker().is_some() || matches!(self.updates.phase, Upgrade::Downloading { .. } | Upgrade::Ready(_)) {
            return;
        }
        let Ok(target) = self.updates.target.clone() else { return };
        self.updates.phase = Upgrade::Downloading { version, have: 0, total: 0 };
        let (tx, wake, blocked) = (self.up.0.clone(), self.wake.clone(), self.updates.blocked.clone());
        std::thread::spawn(move || {
            let progress = |have, total| {
                let _ = tx.send(UpMsg::Progress(have, total));
                wake();
            };
            let result = update::install(version, &target, progress, &|| !blocked.load(Ordering::Relaxed));
            let _ = tx.send(UpMsg::Done(version, result));
            wake();
        });
    }

    fn dismiss_update(&mut self) {
        self.updates.state.dismissed = self.updates.state.available.clone();
        self.updates.phase = Upgrade::Idle;
        self.save_update_state();
    }

    fn open_releases(&mut self) {
        if let Err(e) = std::process::Command::new("xdg-open").arg(update::RELEASES_URL).spawn() {
            self.warn(format!("Could not open the browser: {e}"));
        }
    }

    /// Saves what is pending and replaces this process with the new binary (same arguments,
    /// environment and folder), so a service manager sees one process that never exits. It only
    /// returns when the restart was refused or `exec` failed; the reason is in the footer.
    fn relaunch(&mut self) {
        if self.demo || !matches!(self.updates.phase, Upgrade::Ready(_)) {
            return;
        }
        if let Some(reason) = self.update_blocker() {
            self.warn(format!("Can't restart now: {}.", reason.to_lowercase()));
            return;
        }
        let Ok(target) = self.updates.target.clone() else { return };
        if let Some(i) = self.selected {
            self.meetings[i].notes = self.notes.text.clone();
        }
        self.status = None;
        self.save_current();
        self.retry_unsaved();
        if self.status.is_some() || !self.unsaved.is_empty() {
            return; // something did not save: stay, and show why
        }
        use std::os::unix::process::CommandExt;
        let e = std::process::Command::new(&target).args(std::env::args_os().skip(1)).exec();
        self.warn(format!("Could not start the new version: {e}"));
    }

    /// Tries again the sessions whose save failed earlier; forgets those that no longer exist.
    fn retry_unsaved(&mut self) {
        let files: Vec<PathBuf> = self.unsaved.iter().cloned().collect();
        for file in files {
            match self.index_of(&file) {
                Some(i) => self.save(i),
                None => {
                    self.unsaved.remove(&file);
                }
            }
        }
    }

    fn index_of(&self, file: &Path) -> Option<usize> {
        self.meetings.iter().position(|m| m.file == file)
    }

    fn current(&self) -> Option<&Meeting> {
        self.selected.and_then(|i| self.meetings.get(i))
    }

    fn rec_file(&self) -> Option<&Path> {
        match &self.rec {
            Rec::Idle => None,
            Rec::Live { file, .. } | Rec::Finishing { file, .. } => Some(file),
        }
    }

    fn is_live(&self, m: &Meeting) -> bool {
        matches!(&self.rec, Rec::Live { file, .. } if *file == m.file)
    }

    fn elapsed_ms(&self) -> u64 {
        match &self.rec {
            Rec::Live { t0, offset_ms, .. } => offset_ms + ((self.now - t0).max(0.0) * 1000.0) as u64,
            Rec::Finishing { end_ms, .. } => *end_ms,
            Rec::Idle => 0,
        }
    }

    fn warn(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), true));
    }

    fn save(&mut self, i: usize) {
        if self.demo {
            return;
        }
        if let Some(m) = self.meetings.get(i) {
            match m.save() {
                Ok(()) => {
                    let file = m.file.clone();
                    self.unsaved.remove(&file);
                }
                Err(e) => {
                    let file = m.file.clone();
                    self.unsaved.insert(file);
                    self.warn(format!("Could not save the session: {e}"));
                }
            }
        }
    }

    fn save_current(&mut self) {
        if let Some(i) = self.selected {
            self.save(i);
            if std::mem::take(&mut self.subject_touched) {
                let file = self.meetings[i].file.clone();
                self.try_relocate(&file);
            }
        }
        self.dirty = None;
    }

    /// Moves the session at `file` to its ideal path (subject folder, readable name), or marks
    /// it pending when it is busy and tries again later. References by path follow the move.
    fn try_relocate(&mut self, file: &Path) {
        if self.demo {
            return;
        }
        let Some(i) = self.index_of(file) else {
            self.relocate_pending.remove(file);
            return;
        };
        let busy = [self.rec_file(), self.summarizing.as_deref(), self.reviewing.as_deref(), self.asking.as_ref().map(|a| a.file.as_path())];
        if !store::may_relocate(file, &busy) {
            self.relocate_pending.insert(file.to_path_buf());
            return;
        }
        self.relocate_pending.remove(file);
        let taken: Vec<PathBuf> = self.meetings.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, m)| m.file.clone()).collect();
        let notes_dir = self.config.notes_dir.clone();
        match self.meetings[i].relocate(&notes_dir, &taken) {
            Ok(Some(old)) => {
                self.unsaved.remove(&old);
                let new = self.meetings[i].file.clone();
                if self.unseen.remove(&old) {
                    self.unseen.insert(new.clone());
                }
                if self.warned.remove(&old) {
                    self.warned.insert(new);
                }
            }
            Ok(None) => {}
            Err(e) => self.warn(format!("Could not move the session: {e}")),
        }
    }

    /// Retries the moves that were postponed because the session was busy.
    fn retry_relocations(&mut self) {
        if self.relocate_pending.is_empty() {
            return;
        }
        let pending: Vec<PathBuf> = self.relocate_pending.iter().cloned().collect();
        for file in pending {
            self.try_relocate(&file);
        }
    }

    fn go_home(&mut self) {
        self.save_current();
        self.selected = None;
        self.notebook = None;
        self.nb_review = false;
    }

    /// Shows the cover of the notebook `key` (`store::Group::key`).
    fn open_notebook(&mut self, key: &str) {
        self.save_current();
        self.selected = None;
        self.notebook = Some(key.to_string());
        self.nb_review = false;
        self.reset_review();
    }

    /// The group on the cover on screen, if it still has sessions.
    fn cover(&self) -> Option<store::Group> {
        let key = self.notebook.as_ref()?;
        store::notebooks(&self.meetings).into_iter().find(|g| &g.key == key)
    }

    /// The flashcards "Review all" goes through: `(session, card)` pairs of the open cover.
    fn deck(&self) -> Vec<(usize, usize)> {
        self.cover().map(|g| store::review_deck(&self.meetings, &g.indices)).unwrap_or_default()
    }

    fn select(&mut self, i: usize) {
        if self.selected == Some(i) {
            return;
        }
        self.save_current();
        self.selected = Some(i);
        self.notebook = None;
        self.nb_review = false;
        self.notes = TextEdit::new(self.meetings[i].notes.clone());
        self.subject = TextEdit::new(self.meetings[i].subject.clone());
        self.view = default_view(&self.meetings[i]);
        self.reset_review();
        self.ask_all = false;
        self.ask_input = TextEdit::new("");
        let file = self.meetings[i].file.clone();
        self.unseen.remove(&file);
    }

    /// "New session": what the Mode chip says, with no subject.
    fn new_meeting(&mut self) {
        self.new_session(self.new_mode, String::new());
    }

    /// "New notebook": a first class under the typed name.
    fn confirm_notebook(&mut self) {
        let Some(name) = self.notebook_input.as_ref().map(|e| e.text.split_whitespace().collect::<Vec<_>>().join(" ")) else { return };
        if name.is_empty() || !matches!(self.rec, Rec::Idle) {
            return;
        }
        self.notebook_input = None;
        self.new_session(Mode::Class, name);
    }

    /// Creates a session of `mode` in `subject` and starts recording it.
    fn new_session(&mut self, mode: Mode, subject: String) {
        if !matches!(self.rec, Rec::Idle) {
            return;
        }
        if !self.speech_ready() {
            self.selected = None;
            self.notebook = None;
            self.download_speech();
            self.status = Some(("Downloading the speech model first. It only happens once.".into(), false));
            return;
        }
        let mut meeting = Meeting::new(&self.config.notes_dir, mode);
        meeting.subject = subject;
        // New sessions start in their subject's folder, named by date and time until the title arrives.
        if let Some(name) = meeting.file.file_name().map(|n| n.to_os_string()) {
            meeting.file = store::subject_dir(&self.config.notes_dir, &meeting.subject).join(name);
        }
        self.save_current();
        self.meetings.insert(0, meeting);
        self.selected = None;
        self.select(0);
        self.view = View::Transcript;
        self.start_recording();
        if matches!(self.rec, Rec::Idle) {
            // Recording could not start: don't leave an empty meeting behind.
            let gone = self.meetings.remove(0);
            self.unsaved.remove(&gone.file);
            self.selected = None;
        }
    }

    /// Records into the open meeting, after whatever it already has.
    fn start_recording(&mut self) {
        let Some(i) = self.selected else { return };
        if !matches!(self.rec, Rec::Idle) {
            return;
        }
        let (me, them) = (self.meters.me.clone(), self.meters.them.clone());
        let levels = std::sync::Arc::new(move |speaker: Speaker, level: f32| match speaker {
            Speaker::Them => set_meter(&them, level),
            _ => set_meter(&me, level),
        });
        let m = &self.meetings[i];
        let offset_ms = m.duration_ms;
        let file = m.file.clone();
        match Recording::start(
            self.config.speech_model.clone(),
            self.config.language(),
            self.config.system_audio,
            self.events.0.clone(),
            self.wake.clone(),
            levels,
        ) {
            Ok(handle) => {
                self.rec = Rec::Live { handle: Some(handle), t0: self.now, offset_ms, file };
                self.view = View::Transcript;
                self.status = None;
                if let Some(c) = &self.capsule {
                    c.show();
                }
            }
            Err(e) => self.warn(e),
        }
    }

    fn stop_recording(&mut self) {
        let end_ms = self.elapsed_ms();
        if let Rec::Live { handle, offset_ms, file, .. } = std::mem::replace(&mut self.rec, Rec::Idle) {
            if let Some(h) = handle {
                h.stop();
            }
            if let Some(i) = self.index_of(&file) {
                self.meetings[i].duration_ms = end_ms;
                self.save(i);
            }
            self.rec = Rec::Finishing { offset_ms, end_ms, file };
            self.status = Some(("Finishing the transcript…".into(), false));
        }
        set_meter(&self.meters.me, 0.0);
        set_meter(&self.meters.them, 0.0);
        if let Some(c) = &self.capsule {
            c.hide();
        }
    }

    fn summarize(&mut self) {
        let Some(i) = self.selected else { return };
        if self.summarizing.is_some() {
            return;
        }
        let m = &self.meetings[i];
        let (file, notes, lines) = (m.file.clone(), self.notes.text.clone(), m.lines.clone());
        let llm = Llm::from_config(&self.config).with_mode(m.mode);
        let by = llm.by();
        let (tx, wake) = (self.llm.0.clone(), self.wake.clone());
        self.meetings[i].summary.clear();
        self.summarizing = Some(file.clone());
        self.view = View::Summary;
        std::thread::spawn(move || {
            let send = |msg: LlmMsg| {
                let _ = tx.send(msg);
                wake();
            };
            let result = llm.summarize(&notes, &lines, |delta| send(LlmMsg::Delta(file.clone(), delta.to_string())));
            let title = result.as_ref().ok().and_then(|s| llm.title(s).ok()).filter(|t| !t.is_empty());
            send(LlmMsg::Done(file.clone(), result, by));
            if let Some(t) = title {
                send(LlmMsg::Title(file, t));
            }
        });
    }

    /// The subject and the sessions in it, when the open session can be asked about all of
    /// them: it has a subject and at least one other session shares it.
    fn ask_subject(&self) -> Option<(String, Vec<&Meeting>)> {
        let m = self.current()?;
        let name = m.subject.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            return None;
        }
        let all: Vec<&Meeting> = self.meetings.iter().filter(|o| store::matches_subject(o, Some(&name))).collect();
        (all.len() > 1).then_some((name, all))
    }

    /// Sends the question in the box, in the background; the answer streams into the chat.
    fn ask(&mut self) {
        let Some(i) = self.selected else { return };
        let question = self.ask_input.text.trim().to_string();
        let m = &self.meetings[i];
        if self.asking.is_some() || question.is_empty() || (m.summary.trim().is_empty() && m.lines.is_empty()) {
            return;
        }
        let llm = Llm::from_config(&self.config).with_mode(m.mode);
        let history = m.questions.clone();
        let budget = llm.ask_budget(&history, &question);
        let (scope, context) = match self.ask_subject().filter(|_| self.ask_all) {
            Some((name, classes)) => {
                let ctx = llm::ask_context(&Scope::Subject { name: &name, classes }, budget);
                (name, ctx)
            }
            None => (String::new(), llm::ask_context(&Scope::Session(m), budget)),
        };
        let file = m.file.clone();
        let (tx, wake) = (self.llm.0.clone(), self.wake.clone());
        self.asking = Some(Asking { file: file.clone(), question: question.clone(), scope, partial: String::new() });
        self.ask_input = TextEdit::new("");
        self.view = View::Ask;
        std::thread::spawn(move || {
            let send = |msg: LlmMsg| {
                let _ = tx.send(msg);
                wake();
            };
            let result = llm.ask(&context, &history, &question, |d| send(LlmMsg::AskDelta(file.clone(), d.to_string())));
            send(LlmMsg::Asked(file, result));
        });
    }

    fn clear_questions(&mut self) {
        let Some(i) = self.selected else { return };
        if self.asking.is_some() || self.meetings[i].questions.is_empty() {
            return;
        }
        self.meetings[i].questions.clear();
        self.save(i);
    }

    fn reset_review(&mut self) {
        self.card = 0;
        self.revealed = false;
    }

    /// Moves to another card (`delta` of -1 or 1, stopping at the ends); its answer hides again.
    fn step_card(&mut self, delta: isize) {
        let len = if self.nb_review { self.deck().len() } else { self.current().map_or(0, |m| m.review.len()) };
        let to = step(self.card, len, delta);
        if to != self.card {
            self.card = to;
            self.revealed = false;
        }
    }

    /// Writes the class's review questions in the background. The old ones stay until the new
    /// ones arrive.
    fn generate_review(&mut self) {
        let Some(i) = self.selected else { return };
        let m = &self.meetings[i];
        if self.reviewing.is_some() || m.mode != Mode::Class || (m.summary.trim().is_empty() && m.lines.is_empty()) {
            return;
        }
        let (file, summary, notes, lines) = (m.file.clone(), m.summary.clone(), self.notes.text.clone(), m.lines.clone());
        let llm = Llm::from_config(&self.config).with_mode(m.mode);
        let (tx, wake) = (self.llm.0.clone(), self.wake.clone());
        self.reviewing = Some(file.clone());
        self.reset_review();
        self.view = View::Review;
        std::thread::spawn(move || {
            let result = llm.review(&summary, &notes, &lines);
            let _ = tx.send(LlmMsg::Review(file, result));
            wake();
        });
    }

    /// Opens a meeting's Markdown file in the user's default editor.
    /// Writes an export for the open session and says where it went in the footer.
    fn export(&mut self, anki: bool) {
        let Some(m) = self.current() else { return };
        if anki && m.review.is_empty() {
            self.warn("No review questions to export yet. Generate the review first.");
            return;
        }
        if self.demo {
            self.warn("Demo mode: nothing is written to disk.");
            return;
        }
        let dir = &self.config.notes_dir;
        let (path, content) = if anki {
            (export::anki_path(dir, m), export::export_anki_csv(&m.review))
        } else {
            (export::markdown_path(dir, m), export::export_markdown(m))
        };
        match export::write_atomic(&path, &content) {
            Ok(()) => {
                let msg = format!("Exported to {}", tilde(&path));
                self.exported = Some((msg.clone(), export::export_dir(&self.config.notes_dir)));
                self.status = Some((msg, false));
            }
            Err(e) => self.warn(format!("Could not export: {e}")),
        }
    }

    /// "New class" on a cover: a class in this notebook, or a meeting on Meetings.
    fn new_in(&mut self, g: &store::Group) {
        match g.kind {
            store::GroupKind::Notebook => self.new_session(Mode::Class, g.title.clone()),
            store::GroupKind::Unsorted => self.new_session(Mode::Class, String::new()),
            store::GroupKind::Meetings => self.new_session(Mode::Meeting, String::new()),
        }
    }

    /// "Ask this notebook": the newest session with something to ask about, with the scope on
    /// all the classes of the subject.
    fn ask_notebook(&mut self) {
        let Some(g) = self.cover() else { return };
        let askable = |m: &Meeting| !m.summary.trim().is_empty() || !m.lines.is_empty();
        let Some(i) = store::most_recent(&self.meetings, &g.indices, g.indices.len()).into_iter().find(|&i| askable(&self.meetings[i])) else { return };
        self.select(i);
        self.view = View::Ask;
        self.ask_all = true;
    }

    /// Writes the notebook's study document and its Anki CSV, and says where in the footer.
    fn export_notebook(&mut self) {
        let Some(g) = self.cover() else { return };
        if self.demo {
            self.warn("Demo mode: nothing is written to disk.");
            return;
        }
        let order = store::chronological(&self.meetings, &g.indices);
        let sessions: Vec<&Meeting> = order.iter().map(|&i| &self.meetings[i]).collect();
        let dir = &self.config.notes_dir;
        let md = export::notebook_markdown_path(dir, &g.title);
        let mut result = export::write_atomic(&md, &export::export_notebook_markdown(&g.title, &sessions));
        let cards = sessions.iter().any(|m| !m.review.is_empty());
        if result.is_ok() && cards {
            result = export::write_atomic(&export::notebook_anki_path(dir, &g.title), &export::export_notebook_anki_csv(&sessions));
        }
        match result {
            Ok(()) => {
                let what = if cards { "Markdown and Anki CSV" } else { "Markdown (no flashcards yet)" };
                let msg = format!("Exported {} ({what}) to {}", g.title, tilde(&export::export_dir(dir)));
                self.exported = Some((msg.clone(), export::export_dir(dir)));
                self.status = Some((msg, false));
            }
            Err(e) => self.warn(format!("Could not export: {e}")),
        }
    }

    fn open_file(&mut self, file: &std::path::Path) {
        match std::process::Command::new("xdg-open").arg(file).spawn() {
            // Reap it so it doesn't linger as a zombie.
            Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
            Err(e) => self.warn(format!("Couldn't open {}: {e}", file.display())),
        }
    }

    /// Ticks or unticks the `line`th line of a meeting's summary (a `- [ ]` task).
    fn toggle_task(&mut self, i: usize, line: usize) {
        let Some(m) = self.meetings.get_mut(i) else { return };
        let mut lines: Vec<String> = m.summary.lines().map(str::to_string).collect();
        if let Some(l) = lines.get_mut(line) {
            *l = if l.contains("- [ ]") { l.replacen("- [ ]", "- [x]", 1) } else { l.replacen("- [x]", "- [ ]", 1).replacen("- [X]", "- [ ]", 1) };
        }
        m.summary = lines.join("\n");
        self.save(i);
    }

    /// Spanish ↔ English; the speech model follows the language (a custom model path stays).
    fn cycle_language(&mut self) {
        if !matches!(self.rec, Rec::Idle) || self.download.as_ref().is_some_and(|d| d.what == Fetch::Speech) {
            return;
        }
        let at = models::LANGUAGES.iter().position(|(c, _)| *c == self.config.language);
        let code = models::LANGUAGES[at.map_or(0, |i| (i + 1) % models::LANGUAGES.len())].0;
        self.config.language = code.to_string();
        let custom = models::for_path(&self.config.speech_model).is_none() && self.config.speech_model.exists();
        if !custom {
            self.config.speech_model = models::for_language(code).path();
        }
        crate::session::prewarm(&self.config.speech_model);
        self.save_config();
        let a = self.speech_artifact();
        self.status = Some((format!("{} · {}: {}", language_label(code), a.name, a.note), false));
    }

    /// Local → Claude → Codex → OpenCode, skipping the CLIs that aren't installed.
    fn cycle_summaries(&mut self) {
        let all = Summaries::ALL;
        let at = all.iter().position(|p| *p == self.config.summaries).unwrap_or(0);
        let next = (1..=all.len()).map(|k| all[(at + k) % all.len()]).find(|p| self.demo || p.installed()).unwrap_or(Summaries::Local);
        self.config.summaries = next;
        self.save_config();
        if next == Summaries::Local {
            self.check_llm();
        }
        self.status = Some((format!("Summaries · {}: {}", next.label(), next.note()), false));
    }

    fn save_config(&mut self) {
        if !self.demo {
            if let Err(e) = self.config.save() {
                self.warn(format!("Could not save the settings: {e}"));
            }
        }
    }

    /// Takes what the background threads sent. Returns whether anything changed.
    fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.events.1.try_recv() {
            changed = true;
            let offset = match &self.rec {
                Rec::Live { offset_ms, .. } | Rec::Finishing { offset_ms, .. } => *offset_ms,
                Rec::Idle => 0,
            };
            let target = self.rec_file().and_then(|f| self.index_of(f));
            match event {
                Event::Line(mut line) => {
                    line.t_ms += offset;
                    if let Some(i) = target {
                        self.meetings[i].lines.push(line);
                        self.dirty.get_or_insert(self.now);
                    }
                }
                Event::Status(s) => self.status = Some((s, false)),
                Event::Error(e) => {
                    if let Some(f) = self.rec_file().map(Path::to_path_buf) {
                        self.warned.insert(f);
                    }
                    self.warn(e);
                }
                Event::Finished => {
                    self.rec = Rec::Idle;
                    if !self.status.as_ref().is_some_and(|s| s.1) {
                        self.status = None;
                    }
                    if let Some(i) = target {
                        self.save(i);
                        let m = &self.meetings[i];
                        if self.selected == Some(i) && !m.lines.is_empty() && m.summary.is_empty() {
                            self.summarize();
                        }
                    }
                    // Moves postponed while recording can happen now (not if the summary just started).
                    self.retry_relocations();
                }
            }
        }
        while let Ok(msg) = self.up.1.try_recv() {
            changed = true;
            match msg {
                UpMsg::Checked { explicit, result } => {
                    self.updates.checking = false;
                    match result {
                        Ok(latest) => {
                            let current = Version::current();
                            self.updates.state.record(latest, current, update::now());
                            if explicit {
                                self.updates.state.dismissed = None;
                                self.updates.note = Some(if latest > current {
                                    (format!("Hushnote {latest} is available."), false)
                                } else {
                                    ("You're up to date.".to_string(), false)
                                });
                            }
                            self.save_update_state();
                        }
                        Err(e) if explicit => self.updates.note = Some((e, true)),
                        Err(_) => {}
                    }
                }
                UpMsg::Progress(h, t) => {
                    if let Upgrade::Downloading { have, total, .. } = &mut self.updates.phase {
                        (*have, *total) = (h, t);
                    }
                }
                UpMsg::Done(version, result) => match result {
                    Ok(()) => {
                        self.updates.phase = Upgrade::Ready(version);
                        self.updates.state.available = None;
                        self.save_update_state();
                    }
                    Err(e) => self.updates.phase = Upgrade::Failed(version, e),
                },
            }
        }
        while let Ok(msg) = self.dl.1.try_recv() {
            changed = true;
            match msg {
                DlMsg::Progress(have, total) => {
                    if let Some(d) = &mut self.download {
                        (d.have, d.total) = (have, total);
                    }
                }
                DlMsg::Done(what, result) => {
                    self.download = None;
                    match result {
                        Ok(msg) => {
                            self.status = Some((msg, false));
                            if what == Fetch::Llm {
                                self.llm_state = Some(ollama::State::Ready);
                            }
                        }
                        Err(e) => self.warn(e),
                    }
                }
            }
        }
        while let Ok(msg) = self.llm.1.try_recv() {
            changed = true;
            match msg {
                LlmMsg::Delta(file, delta) => {
                    if let Some(i) = self.index_of(&file) {
                        self.meetings[i].summary.push_str(&delta);
                    }
                }
                LlmMsg::Done(file, result, by) => {
                    self.summarizing = None;
                    let i = self.index_of(&file);
                    match result {
                        Ok(summary) => {
                            self.warned.remove(&file);
                            if let Some(i) = i {
                                self.meetings[i].summary = summary;
                                self.meetings[i].summary_by = by;
                                self.save(i);
                                if self.selected != Some(i) {
                                    self.unseen.insert(file);
                                }
                            }
                        }
                        Err(e) => {
                            self.warned.insert(file);
                            self.warn(e);
                        }
                    }
                }
                LlmMsg::Review(file, result) => {
                    self.reviewing = None;
                    let i = self.index_of(&file);
                    match result {
                        Ok(cards) => {
                            self.warned.remove(&file);
                            if let Some(i) = i {
                                self.meetings[i].review = cards;
                                self.save(i);
                                if self.selected == Some(i) {
                                    self.reset_review();
                                } else {
                                    self.unseen.insert(file);
                                }
                            }
                        }
                        Err(e) => {
                            self.warned.insert(file);
                            self.warn(e);
                            // Nothing to show on the review screen: go back to the summary.
                            if self.view == View::Review && self.current().is_some_and(|m| m.review.is_empty()) {
                                self.view = View::Summary;
                            }
                        }
                    }
                }
                LlmMsg::AskDelta(file, delta) => {
                    if let Some(a) = self.asking.as_mut().filter(|a| a.file == file) {
                        a.partial.push_str(&delta);
                    }
                }
                LlmMsg::Asked(file, result) => {
                    let Some(a) = self.asking.take().filter(|a| a.file == file) else { continue };
                    match result {
                        Ok(answer) => {
                            if let Some(i) = self.index_of(&file) {
                                self.meetings[i].questions.push(Turn { question: a.question, answer, scope: a.scope });
                                self.save(i);
                                if self.selected != Some(i) {
                                    self.unseen.insert(file);
                                }
                            }
                        }
                        Err(e) => {
                            // Nothing is lost: the question goes back in the box to try again.
                            if self.selected == self.index_of(&file) && self.ask_input.text.trim().is_empty() {
                                self.ask_input = TextEdit::new(a.question);
                            }
                            self.warn(e);
                        }
                    }
                }
                LlmMsg::State(state) => self.llm_state = Some(state),
                LlmMsg::Title(file, title) => {
                    if let Some(i) = self.index_of(&file) {
                        if self.meetings[i].title.starts_with("Meeting on") || self.meetings[i].title.starts_with("Class on") {
                            self.meetings[i].title = title;
                            self.save(i);
                            self.try_relocate(&file);
                        }
                    }
                }
            }
        }
        changed
    }

    fn update_capsule(&mut self) {
        let Some(c) = &self.capsule else { return };
        if !matches!(self.rec, Rec::Live { .. }) {
            return;
        }
        let time = clock(self.elapsed_ms());
        let last = self
            .rec_file()
            .and_then(|f| self.index_of(f))
            .and_then(|i| self.meetings[i].lines.last())
            .map(|l| l.text.clone())
            .unwrap_or_else(|| "Listening…".into());
        if self.capsule_shown != (time.clone(), last.clone()) {
            self.capsule_shown = (time.clone(), last.clone());
            c.update(move |cap, _| {
                cap.time = time;
                cap.last = last;
            });
        }
    }

    fn matches_search(&self, m: &Meeting) -> bool {
        let Some(q) = self.search.as_ref().map(|e| e.text.trim().to_lowercase()).filter(|q| !q.is_empty()) else {
            return true;
        };
        m.title.to_lowercase().contains(&q)
            || m.subject.to_lowercase().contains(&q)
            || m.notes.to_lowercase().contains(&q)
            || m.summary.to_lowercase().contains(&q)
            || m.lines.iter().any(|l| l.text.to_lowercase().contains(&q))
    }
}

/// The card after moving `delta` from `at` in a deck of `len`, stopping at the first and last.
fn step(at: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    at.saturating_add_signed(delta).min(len - 1)
}

fn default_view(m: &Meeting) -> View {
    if m.summary.trim().is_empty() { View::Transcript } else { View::Summary }
}

fn setup(s: &mut Ui, waker: Waker) {
    s.wake = waker;
    s.check_llm();
    s.check_for_updates(false);
}

fn poll(s: &mut Ui, t: f64, tx: &mut Tx) -> bool {
    s.now = t;
    let mut changed = s.drain();
    s.updates.blocked.store(s.update_blocker().is_some(), Ordering::Relaxed);
    if std::mem::take(&mut s.updates.restart) {
        s.relaunch();
        changed = true;
    }
    s.retry_relocations();
    if let Some(since) = s.dirty {
        if t - since >= AUTOSAVE_S && !matches!(s.rec, Rec::Live { .. }) {
            s.save_current();
        } else {
            tx.wake_after(Duration::from_secs_f64(AUTOSAVE_S));
        }
    }
    if matches!(s.rec, Rec::Live { .. }) {
        // The clock ticks once a second; the meters move on their own through `Live`.
        tx.wake_after(Duration::from_millis(250));
        s.update_capsule();
        changed = true;
    }
    changed
}

fn on_close(s: &mut Ui, _: &mut Tx) {
    if let Some(i) = s.selected {
        s.meetings[i].notes = s.notes.text.clone();
    }
    s.stop_recording();
    s.save_current();
}

// ---- View: frame and sidebar ----------------------------------------------------------------

fn view(s: &Ui, _: &mut Cx<Ui>) -> El {
    row()
        .w_full()
        .h_full()
        .fill(P::window())
        .child(sidebar(s))
        .child(main_panel(s))
}

fn nav_item(key: &str, glyph: &'static Icon, label: &str, active: bool, on: impl Fn(&mut Ui) + 'static) -> El {
    row()
        .key(key.to_string())
        .w_full()
        .h(36.0)
        .px(10.0)
        .gap(12.0)
        .center_y()
        .radius(9.0)
        .fill(if active { P::raised() } else { P::clear() })
        .hover(P::raised())
        .role("button")
        .label(label.to_string())
        .on_press(on)
        .child(icon(glyph, 17.0).color(P::secondary()))
        .child(text(label.to_string()).font(sans(14.5, 400.0)).color(P::text()))
}

fn dot(color: Color) -> El {
    el().size(7.0, 7.0).radius(3.5).fill(color)
}

fn sidebar(s: &Ui) -> El {
    let mut nav = col().w_full().gap(2.0);
    nav = nav.child(nav_item("nav-home", &HOME, "Home", s.selected.is_none() && s.notebook.is_none(), |s: &mut Ui| s.go_home()));
    nav = match &s.search {
        Some(edit) => nav.child(
            row()
                .key("nav-search")
                .w_full()
                .h(36.0)
                .px(10.0)
                .gap(12.0)
                .center_y()
                .radius(9.0)
                .fill(P::raised())
                .border(1.0, P::line())
                .child(icon(&SEARCH, 17.0).color(P::secondary()))
                .child(
                    text_input(edit, sans(14.5, 400.0))
                        .key("search-input")
                        .grow(1.0)
                        .min_w(0.0)
                        .color(P::text())
                        .placeholder("Search", P::dim())
                        .caret(P::accent(), Color { a: 0.35, ..P::accent() })
                        .on_edit(|s: &mut Ui| s.search.as_mut().expect("search open"))
                        .on_input_key(|s: &mut Ui, ev: &rune::KeyEvent, _: &mut Tx| {
                            if matches!(ev.key, rune::Key::Escape) {
                                s.search = None;
                                return true;
                            }
                            false
                        }),
                ),
        ),
        None => nav.child(nav_item("nav-search", &SEARCH, "Search", false, |s: &mut Ui| {
            s.search = Some(TextEdit::new(""));
        })),
    };
    let recording = !matches!(s.rec, Rec::Idle);
    let mut new = nav_item("nav-new", &NEW, "New session", false, |s: &mut Ui| s.new_meeting());
    if recording {
        new = new.opacity(0.45);
    }
    nav = nav.child(new);
    nav = nav.child(
        row().w_full().pl(34.0).child(
            chip_button("new-mode", format!("Mode · {}", s.new_mode.label()), true, |s: &mut Ui| s.new_mode = s.new_mode.next())
                .help("What the next session is. Classes get notes with topics, concepts, formulas and tasks."),
        ),
    );
    nav = match &s.notebook_input {
        Some(edit) => nav.child(
            row()
                .key("nav-notebook")
                .w_full()
                .h(36.0)
                .px(10.0)
                .gap(12.0)
                .center_y()
                .radius(9.0)
                .fill(P::raised())
                .border(1.0, P::line())
                .child(icon(&BOOK, 17.0).color(P::secondary()))
                .child(
                    text_input(edit, sans(14.5, 400.0))
                        .key("notebook-input")
                        .grow(1.0)
                        .min_w(0.0)
                        .color(P::text())
                        .placeholder("Notebook name", P::dim())
                        .caret(P::accent(), Color { a: 0.35, ..P::accent() })
                        .on_edit(|s: &mut Ui| s.notebook_input.as_mut().expect("notebook box open"))
                        .on_input_key(|s: &mut Ui, ev: &rune::KeyEvent, _: &mut Tx| match ev.key {
                            rune::Key::Escape => {
                                s.notebook_input = None;
                                true
                            }
                            rune::Key::Enter => {
                                s.confirm_notebook();
                                true
                            }
                            _ => false,
                        }),
                ),
        ),
        None => {
            let mut item = nav_item("nav-notebook", &BOOK, "New notebook", false, |s: &mut Ui| {
                if matches!(s.rec, Rec::Idle) {
                    let mut edit = TextEdit::new("");
                    edit.focused = true;
                    s.notebook_input = Some(edit);
                }
            });
            if recording {
                item = item.opacity(0.45);
            }
            nav.child(item)
        }
    };

    let mut list = col().key("meetings").w_full().gap(2.0);
    let groups = store::notebooks(&s.meetings);
    let mut shown = 0;
    for g in &groups {
        let hits: Vec<usize> = g.indices.iter().copied().filter(|&i| s.matches_search(&s.meetings[i])).collect();
        if hits.is_empty() {
            continue;
        }
        shown += hits.len();
        let holds_selected = s.selected.is_some_and(|i| g.indices.contains(&i));
        let open = holds_selected || !s.closed.contains(&g.key);
        list = list.child(group_header(s, g, hits.len(), open));
        if open {
            for i in hits {
                list = list.child(session_row(s, i));
            }
        }
    }
    if shown == 0 {
        let msg = if s.meetings.is_empty() { "No sessions yet" } else { "Nothing matches" };
        list = list.child(text(msg).font(sans(13.0, 400.0)).color(P::dim()).px(10.0).py(8.0));
    }

    col()
        .key("sidebar")
        .w(272.0)
        .h_full()
        .min_h(0.0)
        .pt(18.0)
        .pb(12.0)
        .pl(16.0)
        .pr(10.0)
        .gap(22.0)
        .child(nav)
        .child(list.scroll(ScrollSpec { stick_bottom: false }).grow(1.0).min_h(0.0))
        .children(update_notice(s))
}

/// A notebook's heading: a chevron that opens and closes it, the name (opens the cover, and
/// shows highlighted while the cover is on screen), the count and, on notebooks, a "+" for a
/// new class in it.
fn group_header(s: &Ui, g: &store::Group, count: usize, open: bool) -> El {
    let key = g.key.clone();
    let chevron = el()
        .key(format!("group-{}", g.key))
        .size(26.0, 28.0)
        .radius(7.0)
        .center()
        .hover(P::raised())
        .role("button")
        .label(format!("{} {}", if open { "Collapse" } else { "Expand" }, g.title))
        .on_press(move |s: &mut Ui| {
            if !s.closed.remove(&key) {
                s.closed.insert(key.clone());
            }
        })
        .child(icon(if open { &CHEVRON } else { &CHEVRON_RIGHT }, 14.0).color(P::dim()));
    let on_cover = s.notebook.as_ref() == Some(&g.key);
    let open_key = g.key.clone();
    let name = row()
        .key(format!("group-open-{}", g.key))
        .grow(1.0)
        .min_w(0.0)
        .h(32.0)
        .px(6.0)
        .gap(6.0)
        .center_y()
        .radius(8.0)
        .fill(if on_cover { P::raised() } else { P::clear() })
        .hover(P::raised())
        .role("button")
        .label(format!("{} ({count})", g.title))
        .on_press(move |s: &mut Ui| s.open_notebook(&open_key))
        .child(text(g.title.clone()).font(sans(13.5, 550.0)).color(P::text()).truncate().min_w(0.0).shrink())
        .child(text(format!("({count})")).font(sans(12.5, 400.0)).color(P::dim()));
    let mut r = row().key(format!("group-row-{}", g.key)).w_full().gap(0.0).center_y().child(chevron).child(name);
    if g.kind == store::GroupKind::Notebook {
        let subject = g.title.clone();
        let mut plus = el()
            .key(format!("group-new-{}", g.key))
            .size(26.0, 26.0)
            .radius(7.0)
            .center()
            .hover(P::raised())
            .role("button")
            .label(format!("New class in {}", g.title))
            .help("New class in this notebook")
            .on_press(move |s: &mut Ui| s.new_session(Mode::Class, subject.clone()))
            .child(icon(&PLUS, 15.0).color(P::dim()));
        if !matches!(s.rec, Rec::Idle) {
            plus = plus.opacity(0.45);
        }
        r = r.child(el().w(2.0)).child(plus);
    }
    r
}

/// One session in a notebook: title, the markers (recording, warning, unseen) and the short date.
fn session_row(s: &Ui, i: usize) -> El {
    let m = &s.meetings[i];
    let selected = s.selected == Some(i);
    let marker = if s.rec_file() == Some(m.file.as_path()) {
        Some(P::rec())
    } else if s.warned.contains(&m.file) {
        Some(P::warn())
    } else if s.unseen.contains(&m.file) {
        Some(P::accent())
    } else {
        None
    };
    let mut r = row()
        .key(format!("meeting-{i}"))
        .w_full()
        .h(34.0)
        .pl(26.0)
        .pr(10.0)
        .gap(8.0)
        .center_y()
        .radius(9.0)
        .fill(if selected { P::raised() } else { P::clear() })
        .hover(P::raised())
        .role("button")
        .label(m.title.clone())
        .on_press(move |s: &mut Ui| s.select(i))
        .child(
            text(m.title.clone())
                .font(sans(14.5, if selected { 500.0 } else { 400.0 }))
                .color(if selected { P::text() } else { P::secondary() })
                .truncate()
                .min_w(0.0)
                .grow(1.0),
        );
    if let Some(c) = marker {
        r = r.child(dot(c));
    }
    r.child(text(short_date(&m.date)).font(sans(12.5, 400.0)).color(P::dim()))
}

// ---- View: main panel -------------------------------------------------------------------------

fn main_panel(s: &Ui) -> El {
    let (title, body, footer) = match s.current() {
        Some(m) => {
            let i = s.selected.unwrap();
            match s.view {
                View::Transcript => (m.title.clone(), transcript_view(s, m), footer(s, Some(m))),
                View::Summary => (m.title.clone(), summary_view(s, i, m), footer(s, Some(m))),
                View::Review => (m.title.clone(), review_view(s, m), footer(s, Some(m))),
                View::Ask => (m.title.clone(), ask_view(s, m), footer(s, Some(m))),
            }
        }
        None => match s.cover() {
            Some(g) if s.nb_review => (g.title.clone(), notebook_review_view(s, &g), footer(s, None)),
            Some(g) => (g.title.clone(), notebook_view(s, &g), footer(s, None)),
            None => ("Home".to_string(), home_view(s), footer(s, None)),
        },
    };
    col()
        .key("panel")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .fill(P::panel())
        .border(1.0, P::line())
        .radius(12.0)
        .child(header(s, &title))
        .child(body)
        .child(footer)
        .wrap_in_margin()
}

/// The panel sits 10 points in from the window's top, right and bottom edges.
trait Margin {
    fn wrap_in_margin(self) -> El;
}
impl Margin for El {
    fn wrap_in_margin(self) -> El {
        row().grow(1.0).min_w(0.0).h_full().min_h(0.0).pt(10.0).pr(10.0).pb(10.0).child(self)
    }
}

fn header(s: &Ui, title: &str) -> El {
    let crumbs = row()
        .gap(10.0)
        .center_y()
        .min_w(0.0)
        .shrink()
        .child(text("hushnote").font(sans(15.0, 400.0)).color(P::secondary()))
        .child(text("/").font(sans(15.0, 400.0)).color(P::dim()))
        .child(text(title.to_string()).font(sans(15.0, 550.0)).color(P::text()).truncate().min_w(0.0));
    let mut right = row().gap(18.0).center_y();
    if let Some(m) = s.current() {
        if s.is_live(m) {
            right = right.child(
                row()
                    .key("rec-clock")
                    .gap(8.0)
                    .center_y()
                    .child(dot(P::rec()).size(9.0, 9.0).radius(4.5))
                    .child(text(clock(s.elapsed_ms())).font(mono(14.0)).color(P::text())),
            );
        } else {
            right = right.child(subject_input(s));
            if !m.lines.is_empty() || !m.summary.trim().is_empty() {
                right = right.child(view_switch(s));
            }
            let mut meta = row().gap(14.0);
            if m.duration_ms > 0 {
                meta = meta.child(text(minutes(m.duration_ms)).font(mono(13.0)).color(P::secondary()));
            }
            meta = meta.child(text(pretty_date(&m.date)).font(mono(13.0)).color(P::secondary()));
            right = right.child(meta);
        }
    }
    row()
        .key("header")
        .w_full()
        .h(56.0)
        .px(24.0)
        .gap(16.0)
        .center_y()
        .child(crumbs)
        .child(el().grow(1.0))
        .child(right)
}

/// The subject of the open session, saved with the same autosave as the notes.
fn subject_input(s: &Ui) -> El {
    row().key("subject-box").w(190.0).h(28.0).px(8.0).center_y().radius(6.0).fill(P::raised()).border(1.0, P::line()).child(
        text_input(&s.subject, sans(13.0, 400.0))
            .key("subject-input")
            .grow(1.0)
            .min_w(0.0)
            .color(P::text())
            .placeholder("Subject (e.g. Calculus I)", P::dim())
            .caret(P::accent(), Color { a: 0.35, ..P::accent() })
            .on_edit(|s: &mut Ui| &mut s.subject)
            .on_input_change(|s: &mut Ui, tx: &mut Tx| {
                if let Some(i) = s.selected {
                    s.meetings[i].subject = s.subject.text.clone();
                    s.subject_touched = true;
                    s.dirty.get_or_insert(s.now);
                    tx.wake_after(Duration::from_secs_f64(AUTOSAVE_S));
                }
            }),
    )
}

fn view_switch(s: &Ui) -> El {
    let item = |key: &str, label: &str, v: View| {
        let on = s.view == v;
        text(label.to_string())
            .key(key.to_string())
            .font(sans(13.0, 500.0))
            .color(if on { P::text() } else { P::dim() })
            .px(9.0)
            .py(4.0)
            .radius(5.0)
            .fill(if on { P::raised() } else { P::clear() })
            .role("tab")
            .on_press(move |s: &mut Ui| s.view = v)
    };
    let mut r = row().gap(2.0).child(item("view-transcript", "Transcript", View::Transcript)).child(item("view-summary", "Summary", View::Summary));
    if s.current().is_some_and(|m| !m.review.is_empty()) || s.reviewing.is_some() && s.view == View::Review {
        r = r.child(item("view-review", "Review", View::Review));
    }
    if s.current().is_some_and(|m| !m.summary.trim().is_empty() || !m.lines.is_empty()) {
        r = r.child(item("view-ask", "Ask", View::Ask));
    }
    r
}

/// A scrolling area whose content sits in a centred column at most 720 points wide.
fn column(key: &str, content: El, follow_end: bool) -> El {
    column_of(key, content, follow_end, COLUMN)
}

/// [`column`] with its own maximum width (Home and the notebook covers are wider, for the grids).
fn column_of(key: &str, content: El, follow_end: bool, max: f32) -> El {
    let centred = row()
        .w_full()
        .child(el().grow(1.0))
        .child(content.grow(1000.0).max_w(max).min_w(0.0))
        .child(el().grow(1.0));
    col().w_full().child(centred).scroll(ScrollSpec { stick_bottom: follow_end }).key(key.to_string()).grow(1.0).min_h(0.0).px(24.0)
}

fn transcript_view(s: &Ui, m: &Meeting) -> El {
    let mut lines = col().w_full().gap(22.0).pt(8.0).pb(24.0);
    if m.lines.is_empty() {
        let hint = if s.is_live(m) {
            "Listening. Phrases show up here a moment after they are spoken."
        } else {
            "No transcript yet. Press Record to start."
        };
        lines = lines.child(text(hint).font(sans(14.5, 400.0)).color(P::dim()).wrap().pl(84.0));
    }
    for (i, l) in m.lines.iter().enumerate() {
        lines = lines.child(line_view(i, l));
    }
    let centred_composer = row()
        .w_full()
        .px(24.0)
        .pb(6.0)
        .child(el().grow(1.0))
        .child(composer(s, m).grow(1000.0).max_w(COLUMN).min_w(0.0))
        .child(el().grow(1.0));
    col().w_full().grow(1.0).min_h(0.0).child(column("transcript", lines, true)).child(centred_composer)
}

fn line_view(i: usize, l: &Line) -> El {
    let stamp = text(clock(l.t_ms)).font(mono(12.5)).color(P::dim()).w(84.0).pt(if l.speaker == Speaker::Me { 6.0 } else { 2.0 });
    let body = match l.speaker {
        Speaker::Me => row().grow(1.0).min_w(0.0).child(el().grow(1.0)).child(
            col()
                .max_w(540.0)
                .shrink()
                .min_w(0.0)
                .px(18.0)
                .py(11.0)
                .radius(12.0)
                .fill(P::raised())
                .child(text(l.text.clone()).font(sans(15.0, 400.0)).color(P::text()).wrap()),
        ),
        other => col()
            .grow(1.0)
            .min_w(0.0)
            .gap(5.0)
            .child(text(other.name()).font(sans(12.5, 450.0)).color(P::secondary()))
            .child(text(l.text.clone()).font(sans(15.0, 400.0)).color(P::text()).wrap().w_full()),
    };
    row().key(format!("line-{i}")).w_full().child(stamp).child(body)
}

fn chip_button(key: &str, label: String, chevron: bool, on: impl Fn(&mut Ui) + 'static) -> El {
    let mut r = row()
        .key(key.to_string())
        .h(30.0)
        .px(8.0)
        .gap(5.0)
        .center_y()
        .radius(5.0)
        .hover(P::line())
        .role("button")
        .label(label.clone())
        .on_press(on)
        .child(text(label).font(sans(14.0, 400.0)).color(P::secondary()));
    if chevron {
        r = r.child(icon(&CHEVRON, 14.0).color(P::dim()));
    }
    r
}

fn action_button(key: &str, label: &str, mark: El, on: impl Fn(&mut Ui) + 'static) -> El {
    row()
        .key(key.to_string())
        .h(34.0)
        .px(13.0)
        .gap(9.0)
        .center_y()
        .radius(9.0)
        .fill(P::line())
        .hover(hex("#323236"))
        .role("button")
        .label(label.to_string())
        .on_press(on)
        .child(mark)
        .child(text(label.to_string()).font(sans(14.0, 550.0)).color(P::text()))
}

fn composer(s: &Ui, m: &Meeting) -> El {
    let field = text_input(&s.notes, sans(15.0, 400.0).lh(24.0))
        .key("notes-input")
        .w_full()
        .color(P::text())
        .placeholder("Jot down what matters. Hushnote fills in the rest from the transcript.", P::dim())
        .caret(P::accent(), Color { a: 0.35, ..P::accent() })
        .on_edit(|s: &mut Ui| &mut s.notes)
        .on_input_change(|s: &mut Ui, tx: &mut Tx| {
            if let Some(i) = s.selected {
                s.meetings[i].notes = s.notes.text.clone();
                s.dirty.get_or_insert(s.now);
                tx.wake_after(Duration::from_secs_f64(AUTOSAVE_S));
            }
        });
    let notes = col()
        .key("notes-scroll")
        .w_full()
        .min_h(52.0)
        .max_h(150.0)
        .shrink()
        .scroll(ScrollSpec { stick_bottom: false })
        .child(field);

    let action = match &s.rec {
        Rec::Live { .. } if s.is_live(m) => {
            action_button("stop", "Stop", el().size(10.0, 10.0).radius(2.0).fill(P::rec()), |s: &mut Ui| s.stop_recording())
        }
        Rec::Finishing { .. } if s.rec_file() == Some(m.file.as_path()) => {
            text("Finishing…").font(sans(13.5, 400.0)).color(P::secondary())
        }
        Rec::Idle if !s.speech_ready() => match &s.download {
            Some(d) if d.what == Fetch::Speech => download_progress(d, 150.0),
            _ => {
                let a = s.speech_artifact();
                action_button("download-model", &format!("Download · {}", models::megabytes(a.bytes)), el(), |s: &mut Ui| s.download_speech())
            }
        },
        Rec::Idle => {
            let label = if m.lines.is_empty() { "Record" } else { "Resume" };
            action_button("record", label, dot(P::rec()).size(9.0, 9.0).radius(4.5), |s: &mut Ui| s.start_recording())
        }
        _ => el(),
    };
    let bar = row()
        .w_full()
        .gap(6.0)
        .center_y()
        .child(
            chip_button("language", language_label(&s.config.language), true, |s: &mut Ui| s.cycle_language())
                .help(format!("Transcribed on this machine with {}.", s.speech_artifact().name)),
        )
        .child(
            chip_button("summaries", summaries_label(s), true, |s: &mut Ui| s.cycle_summaries())
                .help(s.config.summaries.note().to_string()),
        )
        .child(el().grow(1.0))
        .child(action);
    col()
        .key("composer")
        .w_full()
        .pt(14.0)
        .pb(10.0)
        .px(16.0)
        .gap(10.0)
        .radius(12.0)
        .fill(P::raised())
        .border(1.0, P::line())
        .child(text("MY NOTES").font(mono(11.5)).color(P::dim()).pl(2.0))
        .child(notes.pl(2.0))
        .child(bar.pl(-6.0))
}

// ---- View: summary --------------------------------------------------------------------------

/// Pieces of a summary: Markdown runs, and task lines drawn as rows of their own so the
/// checkbox toggles and the due day sits on the right.
enum Piece {
    Md(String),
    Task { line: usize, done: bool, text: String, due: Option<String> },
}

fn pieces(summary: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut md = String::new();
    for (n, raw) in summary.lines().enumerate() {
        match store::parse_task(raw) {
            Some((done, text, due)) => {
                if !md.trim().is_empty() {
                    out.push(Piece::Md(std::mem::take(&mut md)));
                }
                md.clear();
                out.push(Piece::Task { line: n, done, text, due });
            }
            None => {
                md.push_str(raw);
                md.push('\n');
            }
        }
    }
    if !md.trim().is_empty() {
        out.push(Piece::Md(md));
    }
    out
}

fn markdown_style() -> MarkdownStyle {
    MarkdownStyle::neutral(true)
        .text(P::text())
        .muted(P::secondary())
        .link(P::accent())
        .inline_code_bg(P::code())
        .code_block(P::well(), P::line())
        .code_block_font(mono(12.5))
        .body_font(sans(15.0, 400.0).lh(26.0))
        .heading_font(1, sans(19.0, 600.0))
        .heading_font(2, sans(17.0, 600.0))
        .heading_font(3, sans(15.5, 600.0))
        .heading_font(4, sans(15.0, 600.0))
        .list(18.0, 6.0)
        .rule(P::line())
        .task_checkbox(16.0, 4.0, 0.0, None, P::accent(), hex("#FFFFFF"), P::clear(), P::secondary())
        .selection(Color { a: 0.35, ..P::accent() })
}

fn task_row(i: usize, line: usize, done: bool, label: &str, due: Option<&str>) -> El {
    let boxed = el()
        .size(17.0, 17.0)
        .radius(4.0)
        .fill(if done { P::accent() } else { P::clear() })
        .border(if done { 0.0 } else { 1.5 }, P::secondary())
        .center()
        .children(done.then(|| icon(&CHECK, 13.0).color(hex("#FFFFFF"))));
    let mut r = row()
        .key(format!("task-{line}"))
        .w_full()
        .gap(14.0)
        .center_y()
        .py(4.0)
        .role("checkbox")
        .checked(done)
        .label(label.to_string())
        .on_press(move |s: &mut Ui| s.toggle_task(i, line))
        .child(boxed)
        .child(text(label.to_string()).font(sans(15.0, 400.0)).color(if done { P::secondary() } else { P::text() }).wrap().grow(1.0).min_w(0.0));
    if let Some(d) = due {
        r = r.child(text(d.to_string()).font(mono(12.5)).color(P::secondary()));
    }
    r
}

fn summary_view(s: &Ui, i: usize, m: &Meeting) -> El {
    let running = s.summarizing.as_deref() == Some(m.file.as_path());
    let mut c = col().w_full().gap(12.0).pt(6.0).pb(28.0);
    if m.summary.trim().is_empty() && !running {
        let how = match s.config.summaries {
            Summaries::Local => format!(
                "Hushnote writes it with a local LLM ({} at {}) when the session ends. Nothing leaves this machine.",
                s.config.llm_model, s.config.llm_url
            ),
            p => format!("Hushnote writes it with {} when the session ends. {}", p.label(), p.note()),
        };
        c = c
            .child(text("No summary yet").font(sans(17.0, 600.0)).color(P::text()))
            .child(text(how).font(sans(15.0, 400.0)).color(P::secondary()).wrap());
        if s.summaries_need_setup() {
            c = c.child(llm_row(s));
        } else if !m.lines.is_empty() || !m.notes.trim().is_empty() {
            let mut buttons = row().pt(6.0).gap(10.0).child(action_button("summarize", "Write summary", el(), |s: &mut Ui| s.summarize()));
            if m.mode == Mode::Class && !m.lines.is_empty() {
                buttons = buttons.child(review_button(s, m));
            }
            c = c.child(buttons);
            if m.mode == Mode::Class && !m.lines.is_empty() {
                c = c.children(review_note(s));
            }
        }
        return column("summary", c, false);
    }
    let style = markdown_style();
    for (n, piece) in pieces(&m.summary).into_iter().enumerate() {
        c = c.child(match piece {
            Piece::Md(src) => Markdown::new(src).render(&style).key(format!("md-{n}")).pt(if n > 0 { 12.0 } else { 0.0 }),
            Piece::Task { line, done, text, due } => task_row(i, line, done, &text, due.as_deref()),
        });
    }
    if running {
        c = c.child(row().gap(8.0).center_y().child(dot(P::accent())).child(text("Writing the summary…").font(sans(13.5, 400.0)).color(P::secondary())));
    } else {
        c = c.child(summary_actions(s, m));
    }
    column("summary", c, running)
}

/// Rewrite the summary, and where it is saved, with a button to open the file.
fn summary_actions(s: &Ui, m: &Meeting) -> El {
    let name = m.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let file = m.file.clone();
    let mut r = row()
        .key("summary-actions")
        .w_full()
        .pt(10.0)
        .gap(10.0)
        .center_y()
        .child(action_button("resummarize", "Rewrite summary", el(), |s: &mut Ui| s.summarize()));
    if m.mode == Mode::Class {
        r = r.child(review_button(s, m));
    }
    let r = r
        .child(el().grow(1.0))
        .child(text(name).font(mono(12.5)).color(P::dim()).truncate().min_w(0.0).shrink())
        .child(chip_button("open-file", "Open file".into(), false, move |s: &mut Ui| s.open_file(&file)));
    let mut c = col().w_full().gap(8.0).child(r).child(export_row(s, m, true));
    if m.mode == Mode::Class {
        c = c.children(review_note(s));
    }
    c
}

/// Export chips, and "Open folder" right after an export. `markdown` is off on the review
/// screen, which only offers the cards.
fn export_row(s: &Ui, m: &Meeting, markdown: bool) -> El {
    let mut r = row().key("export-row").w_full().gap(6.0).center_y().pl(-8.0);
    if markdown {
        r = r.child(chip_button("export-markdown", "Export Markdown".into(), false, |s: &mut Ui| s.export(false)).help("A clean study document: summary and review questions, without the transcript."));
    }
    if !m.review.is_empty() {
        r = r.child(chip_button("export-anki", "Export Anki CSV".into(), false, |s: &mut Ui| s.export(true)).help("Cards as front,back, ready for Anki's File > Import."));
    }
    if let (Some((msg, dir)), Some((now, _))) = (&s.exported, &s.status) {
        if msg == now {
            let dir = dir.clone();
            r = r.child(chip_button("open-exports", "Open folder".into(), false, move |s: &mut Ui| s.open_file(&dir)));
        }
    }
    r
}

// ---- View: review -------------------------------------------------------------------------------

fn review_button(s: &Ui, m: &Meeting) -> El {
    let label = if m.review.is_empty() { "Generate review" } else { "Regenerate review" };
    let key = if m.review.is_empty() { "generate-review" } else { "regenerate-review" };
    if s.reviewing.is_some() {
        return text("Writing review questions…").font(sans(13.5, 400.0)).color(P::secondary());
    }
    action_button(key, label, el(), |s: &mut Ui| s.generate_review())
}

/// A small tip when the local model writes the summary: a long class loses detail with it.
fn review_note(s: &Ui) -> Option<El> {
    (s.config.summaries == Summaries::Local).then(|| {
        row()
            .key("review-note")
            .w_full()
            .gap(2.0)
            .center_y()
            .child(text("For quality review questions, use Claude or Codex:").font(sans(13.0, 400.0)).color(P::dim()).shrink().min_w(0.0))
            .child(chip_button("summaries", summaries_label(s), true, |s: &mut Ui| s.cycle_summaries()).help(s.config.summaries.note().to_string()))
    })
}

// ---- View: ask ------------------------------------------------------------------------------

/// A tip when the local model answers: it sees less of a long class.
fn ask_note(s: &Ui) -> Option<El> {
    (s.config.summaries == Summaries::Local).then(|| {
        text("For better answers, pick Claude or Codex with the summary chip below.").font(sans(13.0, 400.0)).color(P::dim()).wrap()
    })
}

fn turn_view(key: &str, question: &str, scope: &str, answer: El) -> El {
    let mut asked = col().max_w(540.0).shrink().min_w(0.0).px(18.0).py(11.0).radius(12.0).fill(P::raised()).gap(4.0);
    if !scope.is_empty() {
        asked = asked.child(text(format!("All {scope} classes")).font(sans(12.0, 450.0)).color(P::secondary()));
    }
    asked = asked.child(text(question.to_string()).font(sans(15.0, 400.0)).color(P::text()).wrap());
    col()
        .key(key.to_string())
        .w_full()
        .gap(14.0)
        .child(row().w_full().child(el().grow(1.0)).child(asked))
        .child(answer)
}

fn ask_view(s: &Ui, m: &Meeting) -> El {
    let style = markdown_style();
    let thing = if m.mode == Mode::Class { "class" } else { "meeting" };
    let live = s.asking.as_ref().filter(|a| a.file == m.file);
    let mut c = col().w_full().gap(26.0).pt(8.0).pb(24.0);
    if m.questions.is_empty() && live.is_none() {
        c = c.child(
            col()
                .gap(6.0)
                .child(text("Ask your notes").font(sans(17.0, 600.0)).color(P::text()))
                .child(
                    text(format!("Ask about this {thing}. Answers come only from its summary, your notes and the transcript."))
                        .font(sans(15.0, 400.0))
                        .color(P::secondary())
                        .wrap(),
                )
                .children(ask_note(s)),
        );
    }
    for (n, t) in m.questions.iter().enumerate() {
        c = c.child(turn_view(&format!("turn-{n}"), &t.question, &t.scope, Markdown::new(t.answer.clone()).render(&style).key(format!("answer-{n}"))));
    }
    if let Some(a) = live {
        let answer = if a.partial.trim().is_empty() {
            row().gap(8.0).center_y().child(dot(P::accent())).child(text("Thinking…").font(sans(13.5, 400.0)).color(P::secondary()))
        } else {
            row().w_full().child(Markdown::new(a.partial.clone()).render(&style).key("answer-live"))
        };
        c = c.child(turn_view("turn-live", &a.question, &a.scope, answer));
    } else if !m.questions.is_empty() {
        c = c.children(ask_note(s));
    }
    let composer = row().w_full().px(24.0).pb(6.0).child(el().grow(1.0)).child(ask_composer(s, m).grow(1000.0).max_w(COLUMN).min_w(0.0)).child(el().grow(1.0));
    col().w_full().grow(1.0).min_h(0.0).child(column("ask", c, true)).child(composer)
}

fn ask_composer(s: &Ui, m: &Meeting) -> El {
    let thing = if m.mode == Mode::Class { "class" } else { "meeting" };
    let busy = s.asking.is_some();
    let field = text_input(&s.ask_input, sans(15.0, 400.0).lh(24.0))
        .key("ask-input")
        .w_full()
        .color(P::text())
        .placeholder(format!("Ask about this {thing}…"), P::dim())
        .caret(P::accent(), Color { a: 0.35, ..P::accent() })
        .on_edit(|s: &mut Ui| &mut s.ask_input)
        .on_input_key(|s: &mut Ui, ev: &rune::KeyEvent, _: &mut Tx| {
            if matches!(ev.key, rune::Key::Enter) && !ev.mods.shift {
                s.ask();
                return true;
            }
            false
        });
    let input = col().key("ask-scroll").w_full().min_h(40.0).max_h(150.0).shrink().scroll(ScrollSpec { stick_bottom: false }).child(field);
    let mut bar = row().w_full().gap(6.0).center_y();
    if let Some((subject, _)) = s.ask_subject() {
        let label = if s.ask_all { format!("All {subject} classes") } else { format!("This {thing}") };
        bar = bar.child(
            chip_button("ask-scope", label, true, |s: &mut Ui| s.ask_all = !s.ask_all)
                .help("Switch between this session and the summaries of every class in the subject."),
        );
    } else {
        bar = bar.child(text(format!("This {thing}")).font(sans(14.0, 400.0)).color(P::secondary()).px(8.0));
    }
    bar = bar.child(chip_button("summaries", summaries_label(s), true, |s: &mut Ui| s.cycle_summaries()).help(s.config.summaries.note().to_string()));
    if !m.questions.is_empty() && !busy {
        bar = bar.child(chip_button("ask-clear", "Clear".into(), false, |s: &mut Ui| s.clear_questions()).help("Delete the questions and answers saved in this session."));
    }
    bar = bar.child(el().grow(1.0));
    bar = bar.child(if busy {
        text("Thinking…").font(sans(13.5, 400.0)).color(P::secondary())
    } else {
        action_button("ask-send", "Ask", el(), |s: &mut Ui| s.ask())
    });
    col()
        .key("ask-composer")
        .w_full()
        .pt(12.0)
        .pb(10.0)
        .px(16.0)
        .gap(8.0)
        .radius(12.0)
        .fill(P::raised())
        .border(1.0, P::line())
        .child(input.pl(2.0))
        .child(bar.pl(-6.0))
}

fn review_view(s: &Ui, m: &Meeting) -> El {
    let mut c = col().w_full().gap(16.0).pt(6.0).pb(28.0);
    if s.reviewing.as_deref() == Some(m.file.as_path()) {
        c = c.child(row().gap(8.0).center_y().child(dot(P::accent())).child(text("Writing review questions…").font(sans(13.5, 400.0)).color(P::secondary())));
        return column("review", c, false);
    }
    let Some(card) = m.review.get(s.card) else {
        c = c
            .child(text("No review questions yet").font(sans(17.0, 600.0)).color(P::text()))
            .child(text("Hushnote writes 5 to 10 questions with answers from this class's summary.").font(sans(15.0, 400.0)).color(P::secondary()).wrap());
        if !m.summary.trim().is_empty() || !m.lines.is_empty() {
            c = c.child(row().pt(6.0).child(review_button(s, m)));
        }
        return column("review", c.children(review_note(s)), false);
    };
    c = c.child(flashcard(card, s.card, m.review.len(), s.revealed, None)).child(row().pt(10.0).child(review_button(s, m))).child(export_row(s, m, false));
    column("review", c.children(review_note(s)), false)
}

/// One flashcard with its Previous / Next / Show answer controls, shared by a class's review
/// and by "Review all" on a notebook (which names the class each card comes from in `source`).
/// `at` is the card on screen out of `len`; the click handlers act on `Ui::card` and
/// `Ui::revealed`.
fn flashcard(card: &Card, at: usize, len: usize, revealed: bool, source: Option<&str>) -> El {
    let label = |t: &str| text(t.to_string()).font(mono(12.0)).color(P::dim());
    let mut face = col()
        .key("review-card")
        .w_full()
        .min_h(120.0)
        .gap(12.0)
        .px(28.0)
        .py(26.0)
        .radius(14.0)
        .fill(P::raised())
        .border(1.0, P::line())
        .role("button")
        .label(if revealed { "Hide the answer" } else { "Show the answer" })
        .on_press(|s: &mut Ui| s.revealed = !s.revealed);
    if let Some(from) = source {
        face = face.child(text(from.to_string()).font(sans(13.0, 500.0)).color(P::secondary()).truncate().min_w(0.0));
    }
    face = face.child(label("QUESTION")).child(text(card.question.clone()).font(sans(19.0, 600.0)).color(P::text()).wrap().w_full());
    face = if revealed {
        face.child(el().w_full().h(1.0).fill(P::line()))
            .child(label("ANSWER"))
            .child(Markdown::new(card.answer.clone()).render(&markdown_style()).key("review-answer"))
    } else {
        face.child(text("Click to show the answer").font(sans(13.5, 400.0)).color(P::dim()))
    };
    let counter = text(format!("{} / {}", at + 1, len)).font(mono(13.0)).color(P::secondary());
    let controls = row()
        .key("review-controls")
        .w_full()
        .gap(10.0)
        .center_y()
        .child(action_button("review-prev", "Previous", el(), |s: &mut Ui| s.step_card(-1)))
        .child(action_button("review-next", "Next", el(), |s: &mut Ui| s.step_card(1)))
        .child(counter)
        .child(el().grow(1.0))
        .child(action_button("review-reveal", if revealed { "Hide answer" } else { "Show answer" }, el(), |s: &mut Ui| s.revealed = !s.revealed));
    col().w_full().gap(16.0).child(face).child(controls)
}

// ---- View: home and footer -------------------------------------------------------------------

fn wordmark(scale: f32) -> El {
    // The block wordmark from assets/wordmark.svg: "hush" grey, "note" white.
    let svg = include_str!("../assets/wordmark.svg");
    let attr = |tag: &str, name: &str| -> Option<String> {
        let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        Some(tag[start..].split('"').next()?.to_string())
    };
    let (vw, vh) = (390.0, 60.0);
    let mut mark = el().w(vw * scale).h(vh * scale);
    for tag in svg.split("<rect").skip(1) {
        let tag = tag.split("/>").next().unwrap_or("");
        let num = |n: &str| attr(tag, n).and_then(|v| v.parse::<f32>().ok());
        let (Some(x), Some(y), Some(w), Some(h)) = (num("x"), num("y"), num("width"), num("height")) else { continue };
        let fill = attr(tag, "fill").map(|f| hex(&f)).unwrap_or(P::text());
        mark = mark.child(el().absolute(x * scale, y * scale).size(w * scale, h * scale).fill(fill));
    }
    mark
}

fn home_view(s: &Ui) -> El {
    let total_ms: u64 = s.meetings.iter().map(|m| m.duration_ms).sum();
    let open_tasks: usize = s.meetings.iter().map(|m| m.summary.lines().filter(|l| l.trim_start().starts_with("- [ ]")).count()).sum();
    let stat = |label: &str, value: String| {
        col()
            .gap(4.0)
            .child(text(value).font(mono(18.0)).color(P::text()))
            .child(text(label.to_string()).font(sans(13.0, 400.0)).color(P::secondary()))
    };
    let stats = row()
        .gap(36.0)
        .child(stat("sessions", s.meetings.len().to_string()))
        .child(stat("recorded", minutes(total_ms)))
        .child(stat("open action items", open_tasks.to_string()));
    let start = row()
        .gap(8.0)
        .center_y()
        .child(action_button("home-new", "New session", dot(P::rec()).size(9.0, 9.0).radius(4.5), |s: &mut Ui| s.new_meeting()))
        .child(chip_button("home-mode", format!("Mode · {}", s.new_mode.label()), true, |s: &mut Ui| s.new_mode = s.new_mode.next()));
    let intro = text("Meeting notes that never leave your machine. Transcribed live with Parakeet, summarized by a local LLM, saved as Markdown.")
        .font(sans(15.0, 400.0))
        .color(P::secondary())
        .wrap()
        .max_w(560.0);
    let books = !s.meetings.is_empty();
    let mut c = col().w_full().gap(18.0).pt(if books { 40.0 } else { 64.0 }).pb(28.0).child(wordmark(if books { 0.5 } else { 0.62 })).child(intro);
    c = if books {
        c.child(row().w_full().pt(4.0).gap(24.0).center_y().child(stats).child(el().grow(1.0)).child(start))
    } else {
        c.child(stats.pt(8.0)).child(start.pt(10.0))
    };
    if !s.speech_ready() || s.summaries_need_setup() || s.download.is_some() {
        c = c.child(
            col()
                .key("setup")
                .w_full()
                .pt(18.0)
                .gap(8.0)
                .child(text("SET UP").font(mono(11.5)).color(P::dim()).pb(2.0))
                .child(speech_row(s))
                .child(llm_row(s)),
        );
    }
    if books {
        let mut grid = row().key("notebook-grid").w_full().gaps(28.0, CARD_GAP).flex_wrap();
        for g in store::notebooks(&s.meetings) {
            grid = grid.child(folder_card(s, &g));
        }
        c = c.child(col().w_full().pt(22.0).gap(16.0).child(text("NOTEBOOKS").font(mono(11.5)).color(P::dim())).child(grid));
    }
    c = c.child(
        col()
            .key("updates")
            .w_full()
            .pt(if books { 22.0 } else { 18.0 })
            .gap(8.0)
            .child(text("UPDATES").font(mono(11.5)).color(P::dim()).pb(2.0))
            .child(update_row(s)),
    );
    column_of("home", c, false, if books { COLUMN_WIDE } else { COLUMN })
}

// ---- Folders and sheets -----------------------------------------------------------------------------

/// How a sheet of paper looks: `depth` 0 is the one in front; the ones behind it are a shade darker.
struct Paper {
    bg: Color,
    title: Color,
    body: Color,
    faint: Color,
}

fn paper(depth: usize) -> Paper {
    let bg = ["#ECEAE4", "#DEDCD6", "#D0CEC8"][depth.min(2)];
    Paper { bg: hex(bg), title: hex("#171718"), body: hex("#64635E"), faint: hex("#8A8984") }
}

/// What a session says on a sheet: title (bold, two lines at most) and the preview lines.
fn sheet_text(m: &Meeting, look: &Paper, big: bool, lines: usize) -> El {
    let (title_font, body_font) = if big { (sans(14.5, 650.0).lh(19.0), sans(11.5, 400.0).lh(15.0)) } else { (sans(12.5, 650.0).lh(16.0), sans(9.5, 400.0).lh(12.5)) };
    let mut c = col().w_full().min_h(0.0).gap(if big { 3.0 } else { 2.0 });
    c = c.child(text(m.title.clone()).font(title_font).color(look.title).max_lines(2).w_full());
    let mut body = col().w_full().gap(if big { 3.0 } else { 2.0 }).pt(if big { 5.0 } else { 3.0 });
    for l in store::preview_lines(m, lines) {
        body = body.child(text(l).font(body_font).color(look.body).truncate().min_w(0.0).w_full());
    }
    c.child(body)
}

/// The card of a notebook on Home: a folder with its newest sessions' sheets sticking out of
/// it. Everything is absolutely placed inside one fixed box, in paint order (the folder's back
/// and tab, the sheets from the oldest to the newest, then the front of the folder over their
/// lower half). Sheets behind the first are nudged and tilted a few degrees (`rotate`).
fn folder_card(s: &Ui, g: &store::Group) -> El {
    let (w, h) = (CARD_W, CARD_H);
    let (back, front, edge) = (hex("#222226"), hex("#2A2A2E"), hex("#38383E"));
    let key = g.key.clone();
    let summary = store::group_summary(&s.meetings, g);
    let mut card = el()
        .key(format!("folder-{}", g.key))
        .w(w)
        .h(h)
        .hover_group()
        .role("button")
        .label(format!("{}, {}", g.title, summary))
        .on_press(move |s: &mut Ui| s.open_notebook(&key))
        .child(el().absolute(0.0, 0.0).size(92.0, 34.0).radius(11.0).fill(back))
        .child(el().absolute(0.0, 16.0).size(w, h - 16.0).radius(13.0).fill(back));
    // (x, y, width, tilt in radians) of the newest sheet and of the two behind it.
    let spots = [(16.0, 32.0, w - 32.0, 0.0), (12.0, 28.0, w - 24.0, 0.05), (20.0, 26.0, w - 40.0, -0.045)];
    let recent = store::most_recent(&s.meetings, &g.indices, 3);
    for (rank, &i) in recent.iter().enumerate().rev() {
        let (x, y, sw, tilt) = spots[rank];
        let look = paper(rank);
        card = card.child(
            el()
                .absolute(x, y)
                .size(sw, 120.0)
                .radius(7.0)
                .fill(look.bg)
                .rotate(tilt)
                .clip_rounded(7.0)
                .child(if rank == 0 { sheet_text(&s.meetings[i], &look, false, 3).px(11.0).pt(11.0) } else { el() }),
        );
    }
    card = card
        .child(
            col()
                .absolute(0.0, 100.0)
                .size(w, h - 100.0)
                .radius(13.0)
                .fill(front)
                .border(1.0, edge)
                .px(16.0)
                .pt(14.0)
                .gap(3.0)
                .child(text(g.title.clone()).font(sans(14.5, 600.0)).color(P::text()).truncate().min_w(0.0).w_full())
                .child(text(summary).font(sans(12.5, 400.0)).color(P::secondary()).truncate().min_w(0.0).w_full()),
        )
        .child(el().absolute(0.0, 0.0).size(w, h).radius(13.0).border(1.5, Color { a: 0.55, ..P::accent() }).group_opacity(0.0, 1.0));
    card
}

/// A session as a page of the notebook: title, date, a short preview and what it has (flashcards,
/// questions). Pressing it opens the session.
fn session_sheet(s: &Ui, i: usize) -> El {
    let m = &s.meetings[i];
    let look = paper(0);
    let mut have = Vec::new();
    if !m.review.is_empty() {
        have.push(format!("{} flashcard{}", m.review.len(), if m.review.len() == 1 { "" } else { "s" }));
    }
    if !m.questions.is_empty() {
        have.push(format!("{} question{}", m.questions.len(), if m.questions.len() == 1 { "" } else { "s" }));
    }
    col()
        .key(format!("sheet-{i}"))
        .w(SHEET_W)
        .h(SHEET_H)
        .px(14.0)
        .pt(14.0)
        .pb(12.0)
        .gap(4.0)
        .radius(8.0)
        .fill(look.bg)
        .hover(Color { a: 0.07, ..hex("#000000") })
        .shadow(2.0, 10.0, Color { a: 0.35, ..hex("#000000") })
        .clip_rounded(8.0)
        .role("button")
        .label(m.title.clone())
        .on_press(move |s: &mut Ui| s.select(i))
        .child(text(short_date(&m.date)).font(mono(11.0)).color(look.faint))
        .child(sheet_text(m, &look, true, 5))
        .child(el().grow(1.0))
        .child(text(have.join(" · ")).font(sans(11.5, 450.0)).color(look.faint).truncate().min_w(0.0).w_full())
}

// ---- View: notebook cover ---------------------------------------------------------------------------

/// An [`action_button`] with no mark, so its label sits centred.
fn plain_button(key: &str, label: &str, on: impl Fn(&mut Ui) + 'static) -> El {
    row()
        .key(key.to_string())
        .h(34.0)
        .px(14.0)
        .center_y()
        .radius(9.0)
        .fill(P::line())
        .hover(hex("#323236"))
        .role("button")
        .label(label.to_string())
        .on_press(on)
        .child(text(label.to_string()).font(sans(14.0, 550.0)).color(P::text()))
}

fn disabled_button(key: &str, label: &str, tip: &str) -> El {
    plain_button(key, label, |_: &mut Ui| {}).opacity(0.45).help(tip.to_string())
}

fn notebook_view(s: &Ui, g: &store::Group) -> El {
    let order = store::chronological(&s.meetings, &g.indices);
    let cards: usize = g.indices.iter().map(|&i| s.meetings[i].review.len()).sum();
    let idle = matches!(s.rec, Rec::Idle);
    let mut c = col().w_full().gap(26.0).pt(12.0).pb(32.0);

    let mut sub = store::group_summary(&s.meetings, g);
    if cards > 0 {
        sub.push_str(&format!(" · {cards} flashcards"));
    }
    c = c.child(col().gap(6.0).child(text(g.title.clone()).font(sans(26.0, 650.0)).color(P::text()).truncate().min_w(0.0)).child(text(sub).font(sans(14.0, 400.0)).color(P::secondary())));

    // Actions.
    let mut actions = row().key("notebook-actions").w_full().gap(10.0).center_y().flex_wrap();
    let group = g.clone();
    let new_label = if g.kind == store::GroupKind::Meetings { "New session" } else { "New class" };
    let mut new = action_button("nb-new", new_label, dot(P::rec()).size(9.0, 9.0).radius(4.5), move |s: &mut Ui| s.new_in(&group));
    if !idle {
        new = new.opacity(0.45);
    }
    actions = actions.child(new);
    actions = actions.child(if cards > 0 {
        plain_button("nb-review", "Review all", |s: &mut Ui| {
            s.nb_review = true;
            s.reset_review();
        })
    } else {
        disabled_button("nb-review", "No flashcards yet", "Review questions are generated in each class, from its Review tab.")
    });
    let askable = g.indices.iter().any(|&i| !s.meetings[i].summary.trim().is_empty() || !s.meetings[i].lines.is_empty());
    if g.kind == store::GroupKind::Notebook {
        actions = actions.child(if askable {
            plain_button("nb-ask", "Ask this notebook", |s: &mut Ui| s.ask_notebook())
                .help("Ask a question about all the classes of this notebook.")
        } else {
            disabled_button("nb-ask", "Ask this notebook", "Record or write something in a class first.")
        });
    }
    actions = actions.child(plain_button("nb-export", "Export notebook", |s: &mut Ui| s.export_notebook()).help("A Markdown study document and an Anki CSV with every class, in the exports folder."));
    if let (Some((msg, dir)), Some((now, _))) = (&s.exported, &s.status) {
        if msg == now {
            let dir = dir.clone();
            actions = actions.child(chip_button("open-exports", "Open folder".into(), false, move |s: &mut Ui| s.open_file(&dir)));
        }
    }
    c = c.child(actions);

    // Due soon: open tasks, newest class first.
    let mut newest = order.clone();
    newest.reverse();
    let tasks = store::open_tasks(&s.meetings, &newest);
    if !tasks.is_empty() {
        const SHOWN: usize = 6;
        let mut due = col().key("due-soon").w_full().gap(2.0);
        for t in tasks.iter().take(SHOWN) {
            due = due.child(cover_task_row(t, &s.meetings[t.session].title));
        }
        if tasks.len() > SHOWN {
            due = due.child(text(format!("and {} more in the classes", tasks.len() - SHOWN)).font(sans(13.0, 400.0)).color(P::dim()).pt(4.0));
        }
        c = c.child(col().w_full().gap(10.0).child(text("DUE SOON").font(mono(11.5)).color(P::dim())).child(due));
    }

    // The pages, oldest first.
    let mut sheets = row().key("sheets").w_full().gaps(CARD_GAP, CARD_GAP).flex_wrap();
    for &i in &order {
        sheets = sheets.child(session_sheet(s, i));
    }
    let label = if g.kind == store::GroupKind::Meetings { "SESSIONS" } else { "CLASSES" };
    c = c.child(col().w_full().gap(14.0).child(text(label).font(mono(11.5)).color(P::dim())).child(sheets));
    column_of("notebook", c, false, COLUMN_WIDE)
}

/// An open task on a cover: a check that ticks it in its class's file, the task, the class it
/// comes from and its due day.
fn cover_task_row(t: &store::OpenTask, source: &str) -> El {
    let (i, line) = (t.session, t.line);
    let boxed = el().size(17.0, 17.0).radius(4.0).border(1.5, P::secondary());
    let mut r = row()
        .key(format!("due-{i}-{line}"))
        .w_full()
        .gap(14.0)
        .center_y()
        .py(5.0)
        .role("checkbox")
        .checked(false)
        .label(t.text.clone())
        .on_press(move |s: &mut Ui| s.toggle_task(i, line))
        .child(boxed)
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .gap(10.0)
                .center_y()
                .child(text(t.text.clone()).font(sans(15.0, 400.0)).color(P::text()).truncate().min_w(0.0).shrink())
                .child(text(source.to_string()).font(sans(13.0, 400.0)).color(P::dim()).truncate().min_w(0.0).shrink()),
        );
    if let Some(d) = &t.due {
        r = r.child(text(d.clone()).font(mono(12.5)).color(P::secondary()));
    }
    r
}

/// "Review all": the flashcards of every class of the notebook, oldest class first, each
/// naming its class.
fn notebook_review_view(s: &Ui, g: &store::Group) -> El {
    let deck = s.deck();
    let mut c = col().w_full().gap(16.0).pt(6.0).pb(28.0);
    c = c.child(row().pl(-8.0).child(chip_button("review-back", format!("Back to {}", g.title), false, |s: &mut Ui| {
        s.nb_review = false;
        s.reset_review();
    })));
    let at = s.card.min(deck.len().saturating_sub(1));
    match deck.get(at) {
        Some(&(session, card)) => {
            let m = &s.meetings[session];
            c = c.child(flashcard(&m.review[card], at, deck.len(), s.revealed, Some(&m.title)));
        }
        None => {
            c = c.child(text("No flashcards yet").font(sans(17.0, 600.0)).color(P::text()));
        }
    }
    column("notebook-review", c, false)
}

/// Bytes so far and a thin bar, for a download.
fn download_progress(d: &Download, width: f32) -> El {
    let frac = if d.total > 0 { (d.have as f32 / d.total as f32).clamp(0.0, 1.0) } else { 0.0 };
    let label = if d.total > 0 {
        format!("{} / {}", models::megabytes(d.have), models::megabytes(d.total))
    } else {
        "Starting…".to_string()
    };
    row()
        .key("download")
        .gap(12.0)
        .center_y()
        .child(
            el().w(width)
                .h(4.0)
                .radius(2.0)
                .fill(P::line())
                .child(el().w(width * frac).h(4.0).radius(2.0).fill(P::accent())),
        )
        .child(text(label).font(mono(12.5)).color(P::secondary()))
        .child(chip_button("cancel-download", "Cancel".into(), false, |s: &mut Ui| s.cancel_download()))
}

/// The sidebar's notice: a new version is out (Update, Later), it is installing, it is ready
/// (Restart) or it failed (Retry). `None` when there is nothing to say.
fn update_notice(s: &Ui) -> Option<El> {
    let u = &s.updates;
    let blocker = s.update_blocker();
    let width = 214.0;
    let card = |title: String, detail: Option<(String, bool)>, bar: Option<El>, buttons: El| {
        let mut c = col()
            .key("update-notice")
            .w_full()
            .px(12.0)
            .py(10.0)
            .gap(8.0)
            .radius(9.0)
            .fill(P::raised())
            .border(1.0, P::line())
            .child(row().gap(8.0).center_y().child(dot(P::accent())).child(text(title).font(sans(13.5, 500.0)).color(P::text()).wrap().min_w(0.0)));
        if let Some((msg, warn)) = detail {
            c = c.child(text(msg).font(sans(12.5, 400.0)).color(if warn { P::warn() } else { P::secondary() }).wrap());
        }
        if let Some(bar) = bar {
            c = c.child(bar);
        }
        c.child(buttons)
    };
    let buttons = || row().gap(6.0).center_y();
    let primary = |key: &str, label: &str, on: fn(&mut Ui)| match blocker {
        Some(reason) => disabled_button(key, label, reason),
        None => plain_button(key, label, on),
    };
    let reason = blocker.map(|r| (r.to_string(), false));
    Some(match &u.phase {
        Upgrade::Downloading { version, have, total } => {
            let frac = if *total > 0 { (*have as f32 / *total as f32).clamp(0.0, 1.0) } else { 0.0 };
            let label = if *total > 0 && have >= total { "Installing…".to_string() } else if *total > 0 {
                format!("{} / {}", models::megabytes(*have), models::megabytes(*total))
            } else {
                "Starting…".to_string()
            };
            let bar = el().key("update-progress").w(width).h(4.0).radius(2.0).fill(P::line()).child(el().w(width * frac).h(4.0).radius(2.0).fill(P::accent()));
            card(format!("Updating to {version}"), Some((label, false)), Some(bar), buttons())
        }
        Upgrade::Ready(version) => card(
            format!("Hushnote {version} is installed"),
            Some((reason.map_or("Restart to finish updating.".to_string(), |r| r.0), false)),
            None,
            buttons().child(primary("update-restart", "Restart", |s: &mut Ui| {
                s.updates.restart = true;
                (s.wake)();
            })),
        ),
        Upgrade::Failed(version, msg) => {
            let version = *version;
            card(
                format!("Couldn't update to {version}"),
                Some((msg.clone(), true)),
                None,
                buttons()
                    .child(primary("update-retry", "Retry", |s: &mut Ui| {
                        if let Some(v) = s.updates.state.available(Version::current()) {
                            s.updates.phase = Upgrade::Idle;
                            s.begin_update(v);
                        }
                    }))
                    .child(chip_button("update-dismiss", "Dismiss".into(), false, |s: &mut Ui| s.dismiss_update())),
            )
        }
        Upgrade::Idle => {
            let version = u.state.notice(Version::current())?;
            let later = chip_button("update-later", "Later".into(), false, |s: &mut Ui| s.dismiss_update());
            match &u.target {
                Ok(_) => card(
                    format!("Hushnote {version} is available"),
                    reason,
                    None,
                    buttons()
                        .child(primary("update-install", "Update", |s: &mut Ui| {
                            if let Some(v) = s.updates.state.available(Version::current()) {
                                s.begin_update(v);
                            }
                        }))
                        .child(later),
                ),
                Err(why) => card(
                    format!("Hushnote {version} is available"),
                    Some((format!("This copy can't update itself ({why}). Install it from the releases page."), false)),
                    None,
                    buttons().child(plain_button("update-releases", "Releases page", |s: &mut Ui| s.open_releases())).child(later),
                ),
            }
        }
    })
}

/// Home's "Updates" row: the version, the automatic check and "Check now".
fn update_row(s: &Ui) -> El {
    let u = &s.updates;
    let detail = match (&u.note, u.checking) {
        (Some((msg, _)), _) => msg.clone(),
        (None, true) => "Checking…".to_string(),
        _ => match u.state.available(Version::current()) {
            Some(v) => format!("Hushnote {v} is available."),
            None if s.config.check_updates => "Looks for a new version on GitHub once a day.".to_string(),
            None => "Automatic checks are off.".to_string(),
        },
    };
    let busy = u.checking || matches!(u.phase, Upgrade::Downloading { .. });
    let check = if busy {
        disabled_button("update-check", "Check now", "Busy")
    } else {
        plain_button("update-check", "Check now", |s: &mut Ui| s.check_for_updates(true))
    };
    let toggle = chip_button(
        "update-auto",
        format!("Check automatically · {}", if s.config.check_updates { "On" } else { "Off" }),
        false,
        |s: &mut Ui| {
            s.config.check_updates = !s.config.check_updates;
            s.save_config();
        },
    );
    setup_row("update-row", format!("Hushnote {}", update::VERSION), detail, row().gap(6.0).center_y().child(toggle).child(check))
}

fn setup_row(key: &str, title: String, detail: String, right: El) -> El {
    row()
        .key(key.to_string())
        .w_full()
        .px(16.0)
        .py(12.0)
        .gap(16.0)
        .center_y()
        .radius(9.0)
        .border(1.0, P::line())
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(3.0)
                .child(text(title).font(sans(14.5, 500.0)).color(P::text()))
                .child(text(detail).font(sans(13.0, 400.0)).color(P::secondary()).wrap()),
        )
        .child(right)
}

fn ready() -> El {
    row().gap(7.0).center_y().child(dot(P::accent())).child(text("Ready").font(mono(12.5)).color(P::secondary()))
}

fn speech_row(s: &Ui) -> El {
    let a = s.speech_artifact();
    let right = match &s.download {
        Some(d) if d.what == Fetch::Speech => download_progress(d, 140.0),
        _ if s.speech_ready() => ready(),
        _ => action_button("setup-speech", &format!("Download · {}", models::megabytes(a.bytes)), el(), |s: &mut Ui| s.download_speech()),
    };
    setup_row("setup-speech-row", format!("Transcription · {}", a.name), format!("{}. Runs on this machine.", a.note), right)
}

/// "Local", "Claude"… for the composer's summary picker.
fn summaries_label(s: &Ui) -> String {
    format!("Summary · {}", s.config.summaries.label())
}

fn llm_row(s: &Ui) -> El {
    let switch = chip_button("setup-llm-switch", "Switch".into(), true, |s: &mut Ui| s.cycle_summaries());
    let provider = s.config.summaries;
    if provider != Summaries::Local {
        let (detail, right) = if s.summaries_ready() {
            (provider.note().to_string(), ready())
        } else {
            (provider.install_hint().to_string(), chip_button("setup-llm-check", "Check again".into(), false, |_: &mut Ui| {}))
        };
        return setup_row("setup-llm-row", format!("Summaries · {}", provider.label()), detail, row().gap(6.0).center_y().child(right).child(switch));
    }
    let model = s.config.llm_model.clone();
    let size = if model == ollama::DEFAULT_MODEL { " · 2.5 GB" } else { "" };
    let (detail, right) = match (&s.llm_state, &s.download) {
        (_, Some(d)) if d.what == Fetch::Llm => ("Downloading through Ollama.".to_string(), download_progress(d, 140.0)),
        (Some(ollama::State::Ready), _) => ("Writes the summary on this machine.".to_string(), ready()),
        (Some(ollama::State::Missing), _) => (
            "Writes the summary on this machine, through Ollama. Or switch to Claude, Codex or OpenCode.".to_string(),
            action_button("setup-llm", &format!("Download{size}"), el(), |s: &mut Ui| s.download_llm()),
        ),
        (Some(ollama::State::NotRunning), _) => (
            "Needs Ollama: sudo pacman -S ollama && sudo systemctl enable --now ollama. Or switch to Claude, Codex or OpenCode.".to_string(),
            chip_button("setup-llm-check", "Check again".into(), false, |s: &mut Ui| s.check_llm()),
        ),
        (None, _) => ("Checking Ollama…".to_string(), el()),
    };
    setup_row("setup-llm-row", format!("Summaries · {}", ollama_name(&model)), detail, row().gap(6.0).center_y().child(right).child(switch))
}

/// `qwen3:4b-instruct-2507-q4_K_M` → `qwen3:4b-instruct-2507`, for display.
fn ollama_name(model: &str) -> &str {
    model.strip_suffix("-q4_K_M").unwrap_or(model)
}

fn meter(label: &str, bars: &[Live<Color>]) -> El {
    let mut b = row().gap(2.0).center_y();
    for (i, bar) in bars.iter().enumerate() {
        b = b.child(el().key(format!("{}-{i}", label.to_lowercase())).size(3.0, 13.0).radius(1.0).fill(P::meter_off()).live_fill(bar));
    }
    row().gap(9.0).center_y().child(text(label.to_string()).font(sans(13.5, 400.0)).color(P::secondary())).child(b)
}

fn footer(s: &Ui, m: Option<&Meeting>) -> El {
    let left = match &s.status {
        Some((msg, warn)) => row()
            .gap(8.0)
            .center_y()
            .min_w(0.0)
            .shrink()
            .child(dot(if *warn { P::warn() } else { P::secondary() }))
            .child(text(msg.clone()).font(sans(13.0, 400.0)).color(if *warn { P::warn() } else { P::secondary() }).truncate().min_w(0.0)),
        None => row().child(text(tilde(&s.config.notes_dir)).font(mono(12.5)).color(P::secondary())),
    };
    let right = match m {
        Some(m) if s.is_live(m) => row().key("meters").gap(22.0).center_y().child(meter("You", &s.meters.me)).child(meter("Them", &s.meters.them)),
        Some(m) if s.view == View::Summary && !m.summary.trim().is_empty() => {
            let mut r = row().gap(22.0);
            if !m.summary_by.is_empty() {
                r = r.child(text(format!("Summary by {}", m.summary_by)).font(sans(13.5, 400.0)).color(P::secondary()));
            }
            r.child(text(minutes(m.duration_ms)).font(sans(13.5, 400.0)).color(P::secondary()))
        }
        Some(m) if s.view == View::Ask => row().child(text(format!("{} questions", m.questions.len())).font(sans(13.5, 400.0)).color(P::secondary())),
        Some(m) if s.view == View::Review => row().child(text(format!("{} questions", m.review.len())).font(sans(13.5, 400.0)).color(P::secondary())),
        Some(m) => row().child(text(format!("{} lines · {}", m.lines.len(), minutes(m.duration_ms))).font(sans(13.5, 400.0)).color(P::secondary())),
        None => row(),
    };
    row().key("footer").w_full().h(46.0).px(24.0).gap(16.0).center_y().child(left).child(el().grow(1.0)).child(right)
}

// ---- Recording capsule (overlay) --------------------------------------------------------------

pub struct Capsule {
    time: String,
    last: String,
}

const CAPSULE_SIZE: (f32, f32) = (400.0, 52.0);

fn capsule_view(c: &Capsule, _: &mut Cx<Capsule>) -> El {
    el().w_full().h_full().center().child(
        row()
            .key("capsule")
            .h(38.0)
            .max_w(CAPSULE_SIZE.0 - 8.0)
            .px(14.0)
            .gap(11.0)
            .center_y()
            .radius(12.0)
            .fill(P::raised())
            .border(1.0, P::line())
            .child(dot(P::rec()).size(9.0, 9.0).radius(4.5))
            .child(text(c.time.clone()).font(mono(13.5)).color(P::text()))
            .child(el().w(1.0).h(14.0).fill(P::line()))
            .child(text(c.last.clone()).font(sans(13.5, 400.0)).color(P::secondary()).truncate().shrink().min_w(0.0)),
    )
}

fn capsule_spec() -> OverlaySpec<Capsule> {
    OverlaySpec {
        name: "hushnote-capsule",
        size: CAPSULE_SIZE,
        edge: ScreenEdge::Top,
        margin: 10.0,
        align: Align::Center,
        inset: 0.0,
        interactive: false,
        view: capsule_view,
    }
}

fn capsule_demo() -> Capsule {
    Capsule { time: "12:41".into(), last: "Yes, I'll send a draft on Wednesday".into() }
}

/// Draws the capsule alone (for `--snap`; the real one is an overlay of the main app).
pub fn run_capsule(args: Vec<String>) {
    let spec = AppSpec { fonts: FONTS, families: FAMILIES, ..AppSpec::new("Hushnote capsule", CAPSULE_SIZE, capsule_demo, capsule_view) };
    App::new(spec).args(args).run();
}

// ---- Scenes for headless screenshots -----------------------------------------------------------

fn demo_meetings() -> Vec<Meeting> {
    let line = |speaker, m: u64, s: u64, t: &str| Line { speaker, t_ms: (m * 60 + s) * 1000, text: t.into() };
    let dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("Documents").join("Hushnote");
    let meeting = |file: &str, title: &str, date: &str, mins: u64| Meeting {
        file: dir.join(file),
        title: title.into(),
        date: date.into(),
        duration_ms: mins * 60_000,
        ..Default::default()
    };
    let mut sync = meeting("2026-11-17-product-sync.md", "Weekly product sync", "2026-11-17 10:00", 47);
    sync.notes = "Beta: Friday\nPricing page still needs copy, ask Dani about the sign-up bug".into();
    sync.lines = vec![
        line(Speaker::Them, 12, 8, "Onboarding is basically done. We just need to fix the sign-up bug."),
        line(Speaker::Me, 12, 19, "Good. Can we still ship the beta on Friday?"),
        line(Speaker::Them, 12, 27, "I think so, if Dani can look at it tomorrow morning."),
        line(Speaker::Me, 12, 38, "Okay, then Friday it is. Maya, can you own the pricing copy?"),
        line(Speaker::Them, 12, 41, "Yes, I'll send a draft on Wednesday"),
    ];
    sync.summary = "## Key points\n- Onboarding is done except for the sign-up bug.\n- The pricing page still has no copy.\n\n\
                    ## Decisions\n- The beta ships on `Friday`.\n- Maya owns the pricing page copy.\n- Acme gets a live demo on Monday, not a recording.\n\n\
                    ## Action items\n- [ ] Dani fixes the sign-up bug · Thu\n- [ ] Maya sends the pricing copy · Wed\n- [x] Prepare the Acme demo · Mon\n\n\
                    ## Open questions\n- Does the free tier need a usage cap before launch?\n- Who runs the support inbox during the first week?"
        .into();
    sync.summary_by = "Qwen3 4B".into();
    vec![
        sync,
        meeting("2026-11-16-acme-kickoff.md", "Acme kickoff call", "2026-11-16 15:00", 32),
        meeting("2026-11-14-design-review.md", "Design review", "2026-11-14 11:30", 58),
        meeting("2026-11-13-maya.md", "1:1 with Maya", "2026-11-13 09:30", 25),
        meeting("2026-11-12-hiring.md", "Hiring debrief", "2026-11-12 17:00", 41),
        meeting("2026-11-10-quarterly.md", "Quarterly planning", "2026-11-10 10:00", 84),
    ]
}

/// Sample classes for the class-mode scenes: three of Calculus I and two of Biology 101.
fn demo_classes() -> Vec<Meeting> {
    let dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("Documents").join("Hushnote");
    let class = |file: &str, title: &str, subject: &str, date: &str, mins: u64| Meeting {
        file: dir.join(file),
        title: title.into(),
        mode: Mode::Class,
        subject: subject.into(),
        date: date.into(),
        duration_ms: mins * 60_000,
        summary_by: "Qwen3 4B".into(),
        ..Default::default()
    };
    let card = |q: &str, a: &str| Card { question: q.into(), answer: a.into() };
    let turn = |q: &str, a: &str, scope: &str| Turn { question: q.into(), answer: a.into(), scope: scope.into() };

    let mut limits = class("2026-11-18-limits-and-continuity.md", "Limits and continuity", "Calculus I", "2026-11-18 09:00", 52);
    limits.notes = "Midterm covers chapters 1-3\nOffice hours Wednesday 3pm".into();
    limits.summary = "## Topics covered\n- Limits of a function at a point and at infinity.\n- One-sided limits and when a limit exists.\n- Continuity and the three types of discontinuity.\n\n\
        ## Key concepts and definitions\n- **Limit**: the value `f(x)` approaches as `x` gets close to `a`.\n- **Continuous at a**: the limit exists, `f(a)` exists and they are equal.\n- **Removable discontinuity**: the limit exists but differs from `f(a)`.\n\n\
        ## Formulas or examples\n- `lim x->0 sin(x)/x = 1`\n- `lim x->2 (x^2 - 4)/(x - 2) = 4`, after factoring the numerator.\n- Squeeze theorem: if `g(x) <= f(x) <= h(x)` and both bounds share a limit, so does `f`.\n\n\
        ## Tasks and due dates\n- [ ] Problem set 3, exercises 1-12 · Fri\n- [ ] Read section 2.5 on continuity · Wed\n- [x] Sign up for the midterm review session"
        .into();
    limits.review = vec![
        card("What three conditions make f continuous at a?", "The limit of f(x) as x -> a exists, f(a) is defined, and the two are equal."),
        card("What is lim x->0 sin(x)/x?", "1. It is proved with the squeeze theorem."),
        card("When does a two-sided limit exist?", "When both one-sided limits exist and are equal."),
        card("What is a removable discontinuity?", "A point where the limit exists but is not equal to f(a), or f(a) is undefined."),
        card("State the squeeze theorem.", "If g(x) <= f(x) <= h(x) near a and g and h tend to L, then f tends to L."),
        card("Evaluate lim x->2 (x^2 - 4)/(x - 2).", "Factor to (x + 2)(x - 2)/(x - 2) = x + 2, so the limit is 4."),
        card("Name the three types of discontinuity.", "Removable, jump and infinite."),
    ];
    limits.questions = vec![
        turn(
            "Why is sin(x)/x not defined at 0 but still has a limit?",
            "The function is undefined at `x = 0` because the denominator is zero, but a limit only looks at values near the point. As `x` gets close to 0 from either side, `sin(x)/x` gets close to 1, so the limit exists and the discontinuity is removable.",
            "",
        ),
        turn(
            "What did the professor say about the midterm?",
            "The midterm covers chapters 1 to 3. Office hours are on Wednesday at 3pm, and problem set 3 is due on Friday.",
            "",
        ),
    ];
    limits.lines = vec![Line { speaker: Speaker::Them, t_ms: 61_000, text: "Today we start with limits, and then we talk about continuity.".into() }];

    let mut derivs = class("2026-11-20-derivatives.md", "Derivatives and the power rule", "Calculus I", "2026-11-20 09:00", 55);
    derivs.summary = "## Topics covered\n- The derivative as a limit of difference quotients.\n- Power, sum and constant multiple rules.\n- Tangent lines.\n\n\
        ## Key concepts and definitions\n- **Derivative**: the instantaneous rate of change, or the slope of the tangent line.\n- **Differentiable** implies continuous, but not the other way around.\n\n\
        ## Formulas or examples\n- `f'(x) = lim h->0 (f(x+h) - f(x))/h`\n- `d/dx x^n = n x^(n-1)`\n- `d/dx (3x^4 - 2x) = 12x^3 - 2`\n\n\
        ## Tasks and due dates\n- [ ] Problem set 4, exercises 1-10 · Fri\n- [ ] Quiz on limits · Tue"
        .into();
    derivs.review = vec![
        card("Define the derivative using a limit.", "f'(x) = lim h->0 (f(x+h) - f(x))/h"),
        card("What is the power rule?", "d/dx x^n = n x^(n-1)"),
    ];

    let mut chain = class("2026-11-21-the-chain-rule.md", "The chain rule", "Calculus I", "2026-11-21 14:00", 48);
    chain.summary = "## Topics covered\n- Differentiating composite functions.\n- Product and quotient rules, with examples.\n\n\
        ## Key concepts and definitions\n- **Composite function**: a function applied to the result of another, `f(g(x))`.\n\n\
        ## Formulas or examples\n- `d/dx f(g(x)) = f'(g(x)) * g'(x)`\n- `d/dx sin(x^2) = 2x cos(x^2)`\n\n\
        ## Tasks and due dates\n- [ ] Practice exam, chain rule section · Mon"
        .into();

    let mut bio = class("2026-11-19-cell-respiration.md", "Cellular respiration", "Biology 101", "2026-11-19 11:00", 50);
    bio.summary = "## Topics covered\n- Glycolysis, the Krebs cycle and the electron transport chain.\n\n\
        ## Key concepts and definitions\n- **ATP**: the cell's energy currency.\n\n\
        ## Formulas or examples\n- `C6H12O6 + 6 O2 -> 6 CO2 + 6 H2O + ATP`\n\n\
        ## Tasks and due dates\n- [ ] Lab report 2 · Thu"
        .into();
    let mut photo = class("2026-11-12-photosynthesis.md", "Photosynthesis", "biology 101", "2026-11-12 11:00", 46);
    photo.summary = "## Topics covered\n- Light reactions and the Calvin cycle.\n\n\
        ## Key concepts and definitions\n- **Chlorophyll**: the pigment that absorbs light in the chloroplast.\n\n\
        ## Formulas or examples\n- `6 CO2 + 6 H2O + light -> C6H12O6 + 6 O2`\n\n\
        ## Tasks and due dates\n- [x] Lab report 1"
        .into();
    vec![limits, derivs, chain, bio, photo]
}

fn scene_demo(s: &mut Ui) {
    s.demo = true;
    s.speech_ready_override = Some(true);
    s.llm_state = Some(ollama::State::Ready);
    s.config = Config { language: "en".into(), speech_model: models::PARAKEET_EN.path(), ..Config::default() };
    s.config.notes_dir = dirs::home_dir().unwrap_or_default().join("Documents").join("Hushnote");
    s.meetings = demo_meetings();
    s.selected = Some(0);
    s.notes = TextEdit::new(s.meetings[0].notes.clone());
    s.subject = TextEdit::new(s.meetings[0].subject.clone());
    s.view = View::Summary;
    s.status = None;
    s.unseen.insert(s.meetings[2].file.clone());
    s.warned.insert(s.meetings[4].file.clone());
}

fn scene_transcript(s: &mut Ui) {
    scene_demo(s);
    s.view = View::Transcript;
}

fn scene_recording(s: &mut Ui) {
    scene_demo(s);
    s.view = View::Transcript;
    let m = &mut s.meetings[0];
    m.summary.clear();
    m.duration_ms = 0;
    s.rec = Rec::Live { handle: None, t0: -761.0, offset_ms: 0, file: m.file.clone() };
    s.notes.focused = true;
    set_meter(&s.meters.me, 0.45);
    set_meter(&s.meters.them, 0.85);
}

/// The first Calculus I class of the demo data, open on its summary.
fn scene_class(s: &mut Ui) {
    scene_demo(s);
    s.meetings.extend(demo_classes());
    let i = s.meetings.iter().position(|m| m.title == "Limits and continuity").unwrap_or(0);
    s.selected = Some(i);
    s.notes = TextEdit::new(s.meetings[i].notes.clone());
    s.subject = TextEdit::new(s.meetings[i].subject.clone());
    s.new_mode = Mode::Class;
    s.view = View::Summary;
}

fn scene_review(s: &mut Ui) {
    scene_class(s);
    s.view = View::Review;
    s.card = 2;
    s.revealed = false;
}

fn scene_review_revealed(s: &mut Ui) {
    scene_review(s);
    s.revealed = true;
}

fn scene_ask(s: &mut Ui) {
    scene_class(s);
    s.view = View::Ask;
}

/// The notebooks sidebar: Calculus I open with a class selected, Biology 101 closed, Meetings open.
fn scene_notebooks(s: &mut Ui) {
    scene_class(s);
    s.closed.insert(store::subject_key("Biology 101"));
}

/// Home with the notebooks grid: the demo meetings and classes, nothing open.
fn scene_home(s: &mut Ui) {
    scene_demo(s);
    s.meetings.extend(demo_classes());
    s.selected = None;
}

/// The cover of Calculus I.
fn scene_notebook(s: &mut Ui) {
    scene_home(s);
    s.new_mode = Mode::Class;
    s.notebook = Some(store::subject_key("Calculus I"));
}

/// "Review all" on the cover of Calculus I, on its fourth card with the answer hidden.
fn scene_notebook_review(s: &mut Ui) {
    scene_notebook(s);
    s.nb_review = true;
    s.card = 3;
}

/// First run: nothing downloaded yet, the speech model on its way.
fn scene_setup(s: &mut Ui) {
    scene_empty(s);
    s.speech_ready_override = Some(false);
    s.llm_state = Some(ollama::State::Missing);
    s.download = Some(Download { what: Fetch::Speech, have: 312_000_000, total: models::PARAKEET_V3.bytes, cancel: Arc::new(AtomicBool::new(false)) });
}

/// A newer release is known: the notice in the sidebar and the Updates row on Home.
fn scene_update(s: &mut Ui) {
    scene_home(s);
    s.updates.state = update::State { checked_at: 1, available: Some("0.2.2".into()), dismissed: None };
    s.updates.target = Ok(PathBuf::from("/home/me/.local/bin/hushnote"));
}

fn scene_updating(s: &mut Ui) {
    scene_update(s);
    s.updates.phase = Upgrade::Downloading { version: Version(0, 2, 2), have: 9_300_000, total: 17_700_000 };
}

fn scene_updated(s: &mut Ui) {
    scene_update(s);
    s.updates.phase = Upgrade::Ready(Version(0, 2, 2));
}

fn scene_update_failed(s: &mut Ui) {
    scene_update(s);
    s.updates.phase = Upgrade::Failed(Version(0, 2, 2), "The update didn't match its checksum, so it was not installed.".into());
}

fn scene_update_busy(s: &mut Ui) {
    scene_update(s);
    scene_recording(s);
    s.updates.state = update::State { checked_at: 1, available: Some("0.2.2".into()), dismissed: None };
    s.updates.target = Ok(PathBuf::from("/home/me/.local/bin/hushnote"));
}

fn scene_update_manual(s: &mut Ui) {
    scene_update(s);
    s.updates.target = Err("this is a development build".into());
    s.updates.note = Some(("You're up to date.".into(), false));
}

fn scene_empty(s: &mut Ui) {
    s.demo = true;
    s.config = Config::default();
    s.config.notes_dir = dirs::home_dir().unwrap_or_default().join("Documents").join("Hushnote");
    s.speech_ready_override = Some(true);
    s.llm_state = Some(ollama::State::Ready);
    s.meetings.clear();
    s.selected = None;
    s.status = None;
}

pub fn run(args: Vec<String>) {
    let spec = AppSpec {
        min_size: Some((860.0, 560.0)),
        fonts: FONTS,
        families: FAMILIES,
        setup: Some(setup),
        poll: Some(poll),
        scenes: &[
            ("demo", scene_demo),
            ("summary", scene_demo),
            ("transcript", scene_transcript),
            ("recording", scene_recording),
            ("home", scene_home),
            ("class", scene_class),
            ("review", scene_review),
            ("review-revealed", scene_review_revealed),
            ("ask", scene_ask),
            ("notebooks", scene_notebooks),
            ("notebook", scene_notebook),
            ("notebook-review", scene_notebook_review),
            ("empty", scene_empty),
            ("setup", scene_setup),
            ("update", scene_update),
            ("updating", scene_updating),
            ("updated", scene_updated),
            ("update-failed", scene_update_failed),
            ("update-busy", scene_update_busy),
            ("update-manual", scene_update_manual),
        ],
        ..AppSpec::new("Hushnote", (1280.0, 840.0), || Ui::new(None), view)
    };
    let mut app = App::new(spec).on_close(on_close);
    let capsule = app.overlay(capsule_spec(), capsule_demo());
    app.init(move || Ui::new(Some(capsule))).args(args).run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_splits_into_markdown_and_tasks_with_due_days() {
        let p = pieces("## Decisions\n- Ship it.\n\n## Action items\n- [ ] Dani fixes the bug · Thu\n- [x] Prepare the demo\n\n## Open questions\n- Cap?");
        assert_eq!(p.len(), 4);
        assert!(matches!(&p[0], Piece::Md(s) if s.contains("## Action items")));
        assert!(matches!(&p[1], Piece::Task { line: 4, done: false, text, due: Some(d) } if text == "Dani fixes the bug" && d == "Thu"));
        assert!(matches!(&p[2], Piece::Task { done: true, due: None, .. }));
        assert!(matches!(&p[3], Piece::Md(s) if s.contains("Open questions")));
    }

    #[test]
    fn restart_waits_for_sessions_whose_save_failed() {
        let dir = std::env::temp_dir().join(format!("hushnote-test-unsaved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("blocked");
        std::fs::write(&blocker, "a file where a folder should be").unwrap();
        let mut ui = Ui::new(None);
        ui.meetings = vec![Meeting::new(&dir, Mode::Meeting)];
        ui.meetings[0].file = blocker.join("a.md");
        ui.meetings[0].title = "Edited".into();
        ui.selected = None; // Home: nothing selected, so a plain save of "the open one" skips it
        ui.save(0);
        assert!(ui.unsaved.contains(&blocker.join("a.md")));
        ui.updates.phase = Upgrade::Ready(Version::current());
        ui.updates.target = Ok(dir.join("missing").join("hushnote"));
        ui.relaunch();
        assert!(ui.status.as_ref().is_some_and(|s| s.1), "the error is shown");
        assert!(!ui.unsaved.is_empty(), "refused to restart with edits that are not on disk");
        // Once the disk works again, the retry saves it and the restart goes on (here `exec` fails
        // on the missing binary, which leaves the app running).
        std::fs::remove_file(&blocker).unwrap();
        ui.relaunch();
        assert!(ui.unsaved.is_empty());
        assert!(std::fs::read_to_string(blocker.join("a.md")).unwrap().contains("Edited"));
        assert!(ui.status.as_ref().unwrap().0.contains("Could not start the new version"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stepping_through_cards_stops_at_the_ends() {
        assert_eq!(step(0, 8, -1), 0);
        assert_eq!(step(0, 8, 1), 1);
        assert_eq!(step(7, 8, 1), 7);
        assert_eq!(step(7, 8, -1), 6);
        assert_eq!(step(0, 0, 1), 0);
    }

    #[test]
    fn changing_card_hides_the_answer() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        ui.meetings = demo_meetings();
        ui.selected = Some(0);
        ui.meetings[0].review = (1..=3).map(|i| Card { question: format!("q{i}"), answer: format!("a{i}") }).collect();
        ui.revealed = true;
        ui.step_card(1);
        assert_eq!((ui.card, ui.revealed), (1, false));
        ui.revealed = true;
        ui.step_card(-1);
        ui.revealed = true;
        ui.step_card(-1); // already at the first card: stays put, and the answer stays up
        assert_eq!((ui.card, ui.revealed), (0, true));
        ui.step_card(1);
        ui.step_card(1);
        assert_eq!(ui.card, 2);
        ui.select(1);
        assert_eq!((ui.card, ui.revealed), (0, false));
    }

    #[test]
    fn asking_needs_a_subject_shared_by_two_sessions() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        ui.meetings = demo_meetings();
        ui.selected = Some(0);
        assert!(ui.ask_subject().is_none(), "no subject");
        ui.meetings[0].subject = "Calculus I".into();
        assert!(ui.ask_subject().is_none(), "alone in its subject");
        ui.meetings[1].subject = " calculus  i".into();
        let (name, all) = ui.ask_subject().unwrap();
        assert_eq!((name.as_str(), all.len()), ("Calculus I", 2));
    }

    #[test]
    fn answers_are_kept_and_a_failed_question_goes_back_in_the_box() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        ui.meetings = demo_meetings();
        ui.selected = Some(0);
        let file = ui.meetings[0].file.clone();
        let asking = |q: &str| Asking { file: file.clone(), question: q.into(), scope: String::new(), partial: String::new() };
        ui.asking = Some(asking("When is the beta?"));
        ui.llm.0.send(LlmMsg::AskDelta(file.clone(), "On ".into())).unwrap();
        ui.llm.0.send(LlmMsg::Asked(file.clone(), Ok("On Friday.".into()))).unwrap();
        ui.drain();
        assert!(ui.asking.is_none());
        assert_eq!(ui.meetings[0].questions.len(), 1);
        assert_eq!(ui.meetings[0].questions[0].answer, "On Friday.");
        ui.asking = Some(asking("Who owns pricing?"));
        ui.llm.0.send(LlmMsg::Asked(file, Err("model offline".into()))).unwrap();
        ui.drain();
        assert_eq!(ui.meetings[0].questions.len(), 1, "the history is kept");
        assert_eq!(ui.ask_input.text, "Who owns pricing?");
        assert!(ui.status.as_ref().unwrap().1);
        ui.clear_questions();
        assert!(ui.meetings[0].questions.is_empty());
    }

    #[test]
    fn demo_classes_group_into_notebooks_after_the_meetings_are_added() {
        let mut all = demo_meetings();
        all.extend(demo_classes());
        let groups = store::notebooks(&all);
        let names: Vec<_> = groups.iter().map(|g| (g.title.as_str(), g.indices.len())).collect();
        assert_eq!(names, [("Calculus I", 3), ("Biology 101", 2), ("Meetings", 6)]);
    }

    #[test]
    fn a_notebook_cover_opens_reviews_all_cards_and_asks_the_subject() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        ui.meetings = demo_meetings();
        ui.meetings.extend(demo_classes());
        ui.selected = Some(0);
        ui.open_notebook(&store::subject_key("Calculus I"));
        assert!(ui.selected.is_none() && ui.cover().is_some_and(|g| g.title == "Calculus I"));
        let deck = ui.deck();
        assert_eq!(deck.len(), 9, "7 + 2 flashcards");
        // Oldest class first: Limits (18 Nov) before Derivatives (20 Nov).
        assert_eq!(ui.meetings[deck[0].0].title, "Limits and continuity");
        assert_eq!(ui.meetings[deck[8].0].title, "Derivatives and the power rule");
        ui.nb_review = true;
        ui.revealed = true;
        ui.step_card(1);
        assert_eq!((ui.card, ui.revealed), (1, false));
        for _ in 0..20 {
            ui.step_card(1);
        }
        assert_eq!(ui.card, 8, "stops at the last card of the whole deck");
        ui.nb_review = false;
        ui.ask_notebook();
        let m = ui.current().unwrap();
        assert_eq!(m.title, "The chain rule", "the newest session of the notebook");
        assert!(ui.view == View::Ask && ui.ask_all && ui.notebook.is_none());
        ui.go_home();
        assert!(ui.notebook.is_none() && ui.selected.is_none());
    }

    #[test]
    fn exporting_a_notebook_in_demo_mode_writes_nothing() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        ui.meetings = demo_classes();
        ui.open_notebook(&store::subject_key("Calculus I"));
        ui.export_notebook();
        assert!(ui.exported.is_none() && ui.status.as_ref().is_some_and(|s| s.1));
    }

    #[test]
    fn an_empty_notebook_name_keeps_the_box_open() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        ui.notebook_input = Some(TextEdit::new("   "));
        ui.confirm_notebook();
        assert!(ui.notebook_input.is_some(), "nothing was created and the box stays");
    }

    #[test]
    fn search_finds_sessions_by_subject() {
        let mut ui = Ui::new(None);
        ui.meetings = demo_meetings();
        ui.meetings[3].subject = "Organic Chemistry".into();
        ui.search = Some(TextEdit::new("chemis"));
        let hits: Vec<_> = ui.meetings.iter().filter(|m| ui.matches_search(m)).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "Organic Chemistry");
    }

    #[test]
    fn a_busy_session_moves_later_and_its_markers_follow() {
        let dir = std::env::temp_dir().join(format!("hushnote-ui-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ui = Ui::new(None);
        ui.config.notes_dir = dir.clone();
        let mut m = Meeting::new(&dir, Mode::Class);
        m.subject = "Calculus I".into();
        m.title = "Limits".into();
        m.save().unwrap();
        let old = m.file.clone();
        ui.meetings = vec![m];
        ui.unseen.insert(old.clone());
        ui.warned.insert(old.clone());
        ui.summarizing = Some(old.clone());
        ui.try_relocate(&old);
        assert!(old.exists() && ui.relocate_pending.contains(&old), "postponed while summarizing");
        ui.retry_relocations();
        assert_eq!(ui.meetings[0].file, old, "still busy");
        ui.summarizing = None;
        ui.retry_relocations();
        let new = ui.meetings[0].file.clone();
        assert_ne!(new, old);
        assert!(new.exists() && !old.exists());
        assert!(ui.relocate_pending.is_empty());
        assert!(ui.unseen.contains(&new) && ui.warned.contains(&new) && !ui.unseen.contains(&old));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relocating_in_demo_mode_writes_nothing() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        ui.meetings = demo_meetings();
        let before = ui.meetings[0].file.clone();
        ui.try_relocate(&before);
        assert_eq!(ui.meetings[0].file, before);
        assert!(ui.relocate_pending.is_empty());
    }

    #[test]
    fn exporting_in_demo_mode_writes_nothing() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        ui.meetings = demo_meetings();
        ui.selected = Some(0);
        ui.export(false);
        assert!(ui.exported.is_none() && ui.status.as_ref().is_some_and(|s| s.1));
        ui.export(true); // no cards: a clear message, nothing exported
        assert!(ui.status.as_ref().unwrap().0.contains("No review questions"));
    }

    #[test]
    fn ticking_a_task_rewrites_its_line() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        ui.meetings = demo_meetings();
        let line = ui.meetings[0].summary.lines().position(|l| l.contains("Dani")).unwrap();
        ui.toggle_task(0, line);
        assert!(ui.meetings[0].summary.contains("- [x] Dani fixes the sign-up bug · Thu"));
        ui.toggle_task(0, line);
        assert!(ui.meetings[0].summary.contains("- [ ] Dani fixes the sign-up bug · Thu"));
    }

    #[test]
    fn updates_wait_while_recording_or_summarizing_or_downloading() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        assert_eq!(ui.update_blocker(), None);
        ui.rec = Rec::Live { handle: None, t0: 0.0, offset_ms: 0, file: PathBuf::from("/x.md") };
        assert!(ui.update_blocker().is_some());
        ui.rec = Rec::Finishing { offset_ms: 0, end_ms: 0, file: PathBuf::from("/x.md") };
        assert!(ui.update_blocker().is_some());
        ui.rec = Rec::Idle;
        ui.summarizing = Some(PathBuf::from("/x.md"));
        assert!(ui.update_blocker().is_some());
        ui.summarizing = None;
        ui.download = Some(Download { what: Fetch::Llm, have: 0, total: 0, cancel: Arc::new(AtomicBool::new(false)) });
        assert!(ui.update_blocker().is_some());
    }

    #[test]
    fn the_update_notice_can_be_dismissed_and_never_starts_while_blocked() {
        let mut ui = Ui::new(None);
        ui.demo = true; // nothing is written or downloaded
        ui.updates.state = update::State { checked_at: 1, available: Some("99.0.0".into()), dismissed: None };
        assert!(update_notice(&ui).is_some());
        ui.rec = Rec::Live { handle: None, t0: 0.0, offset_ms: 0, file: PathBuf::from("/x.md") };
        ui.begin_update(Version(99, 0, 0));
        assert!(matches!(ui.updates.phase, Upgrade::Idle));
        ui.rec = Rec::Idle;
        ui.dismiss_update();
        assert!(update_notice(&ui).is_none());
        ui.updates.state.available = Some("99.0.1".into());
        assert!(update_notice(&ui).is_some(), "a newer version is announced again");
    }

    #[test]
    fn no_update_check_runs_in_demo_mode_or_when_off_or_when_not_due() {
        let mut ui = Ui::new(None);
        ui.demo = true;
        ui.check_for_updates(true);
        assert!(!ui.updates.checking);
        ui.demo = false;
        ui.config.check_updates = false;
        ui.check_for_updates(false);
        assert!(!ui.updates.checking, "off");
        ui.config.check_updates = true;
        ui.updates.state.checked_at = update::now();
        ui.check_for_updates(false);
        assert!(!ui.updates.checking, "checked less than a day ago");
    }

    /// The whole recording path through the window's state, in real time: New meeting starts
    /// capturing, phrases arrive while recording, Stop finishes the transcript and the meeting
    /// is saved as Markdown with the notes. Needs the speech model and a `pw-record` on `PATH`
    /// that plays speech (see docs/plan.md), and `XDG_CONFIG_HOME` pointing at a config whose
    /// `notes_dir` is scratch space:
    /// `cargo test --release e2e -- --ignored --nocapture`
    #[test]
    #[ignore = "needs the speech model and a fake pw-record"]
    fn e2e_record_stop_and_save() {
        let started = std::time::Instant::now();
        let mut ui = Ui::new(None);
        ui.config.llm_url = "http://127.0.0.1:9/v1".into(); // no LLM here: the summary must fail cleanly
        let tick = |ui: &mut Ui| {
            ui.now = started.elapsed().as_secs_f64();
            ui.drain();
        };
        ui.new_meeting();
        assert!(matches!(ui.rec, Rec::Live { .. }), "recording did not start: {:?}", ui.status);
        let file = ui.current().unwrap().file.clone();
        ui.notes = TextEdit::new("- fecha de lanzamiento");
        ui.meetings[0].notes = ui.notes.text.clone();

        let record_s = std::env::var("HUSHNOTE_E2E_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(26.0);
        let mut live_lines = 0;
        while started.elapsed().as_secs_f64() < record_s {
            std::thread::sleep(Duration::from_millis(100));
            tick(&mut ui);
            let n = ui.meetings[0].lines.len();
            if n != live_lines {
                live_lines = n;
                let l = ui.meetings[0].lines.last().unwrap();
                eprintln!("[{:5.1}s live] [{}] {}: {}", started.elapsed().as_secs_f64(), clock(l.t_ms), l.speaker.name(), l.text);
            }
        }
        assert!(live_lines > 0, "no phrase arrived while recording");
        ui.stop_recording();
        assert!(matches!(ui.rec, Rec::Finishing { .. }));
        while !matches!(ui.rec, Rec::Idle) {
            assert!(started.elapsed().as_secs() < record_s as u64 + 60, "the transcript never finished");
            std::thread::sleep(Duration::from_millis(100));
            tick(&mut ui);
        }
        // The summary starts on its own and fails without an LLM, with a warning and a yellow dot.
        while ui.summarizing.is_some() {
            assert!(started.elapsed().as_secs() < record_s as u64 + 90, "the summary never ended");
            std::thread::sleep(Duration::from_millis(100));
            tick(&mut ui);
        }
        eprintln!("status: {:?}", ui.status);
        assert!(ui.warned.contains(&file));

        let saved = Meeting::load(&file).expect("the meeting was saved");
        eprintln!("saved {} ({} lines, {} ms)", file.display(), saved.lines.len(), saved.duration_ms);
        assert_eq!(saved.notes.trim(), "- fecha de lanzamiento");
        assert_eq!(saved.lines.len(), ui.meetings[0].lines.len());
        assert!(saved.lines.len() >= live_lines);
        assert!(saved.duration_ms >= 20_000);
        assert!(saved.lines.windows(2).all(|w| w[0].t_ms <= w[1].t_ms));
    }
}
