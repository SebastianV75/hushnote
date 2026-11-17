//! The window, laid out like OpenCode: a sidebar without a background of its own (Home,
//! Search meetings, New meeting, then the meetings), and a bordered main panel with the live
//! transcript as a chat in a 720 point column, a composer for "My notes", and a footer with the
//! notes folder and the You/Them level meters. After the meeting the summary reads like an
//! assistant's answer: Decisions, Action items (checkboxes, due day on the right) and Open
//! questions.
//!
//! While recording, a capsule overlay at the top centre of the screen shows a red dot, the time
//! and the last phrase (on Wayland it is a layer-shell surface and takes no clicks).

use crate::audio::Speaker;
use crate::config::{self, Config};
use crate::llm::{Llm, clock};
use crate::session::{Event, Line, Recording};
use crate::store::{self, Meeting};
use rune::tok::TextStyle;
use rune::{
    Align, App, AppSpec, Color, Cx, El, Icon, Live, OverlayHandle, OverlaySpec, ScreenEdge, ScrollSpec, TextEdit, Tx,
    Waker, col, el, icon, row, text, text_input,
};
use rune_kit::{Markdown, MarkdownStyle};
use std::collections::HashSet;
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

const AVATARS: [&str; 6] = ["#A8346F", "#1F7A8C", "#8A3FA0", "#B5481E", "#8F2A2A", "#3F6FB5"];

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

const METER_BARS: usize = 5;
const AUTOSAVE_S: f64 = 2.0;
const COLUMN: f32 = 720.0;

const LANGUAGES: [(&str, &str); 8] = [
    ("", "Auto-detect"),
    ("en", "English"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("nl", "Dutch"),
];

fn language_label(code: &str) -> String {
    LANGUAGES.iter().find(|(c, _)| *c == code).map(|(_, l)| l.to_string()).unwrap_or_else(|| code.to_uppercase())
}

fn model_label(path: &Path) -> String {
    let name = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.contains("parakeet") && name.contains("v3") {
        "Parakeet v3".into()
    } else if name.contains("parakeet") && name.contains("v2") {
        "Parakeet v2".into()
    } else if name.contains("parakeet-unified-en") {
        "Parakeet English".into()
    } else if name.contains("whisper-large-v3-turbo") {
        "Whisper turbo".into()
    } else {
        path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "model".into())
    }
}

/// `/home/me/Documents/Hushnote` → `~/Documents/Hushnote`.
fn tilde(path: &Path) -> String {
    match dirs::home_dir() {
        Some(home) if path.starts_with(&home) => format!("~/{}", path.strip_prefix(&home).unwrap().display()),
        _ => path.display().to_string(),
    }
}

/// A meeting's avatar colour, stable for its file.
fn avatar_color(m: &Meeting) -> Color {
    let h = m.file.to_string_lossy().bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    hex(AVATARS[h as usize % AVATARS.len()])
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
    Done(PathBuf, Result<String, String>),
    Title(PathBuf, String),
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
    /// `None` is Home.
    selected: Option<usize>,
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
    /// A message for the footer, and whether it is a warning.
    status: Option<(String, bool)>,
    /// Meetings with something new (a summary arrived while looking elsewhere).
    unseen: HashSet<PathBuf>,
    /// Meetings whose transcription or summary failed.
    warned: HashSet<PathBuf>,
    search: Option<TextEdit>,
    /// When the open meeting changed without being saved yet.
    dirty: Option<f64>,
    /// Scenes for screenshots fill in sample meetings that must never be written to disk.
    demo: bool,
    capsule: Option<OverlayHandle<Capsule>>,
    capsule_shown: (String, String),
}

impl Ui {
    fn new(capsule: Option<OverlayHandle<Capsule>>) -> Ui {
        let config = Config::load();
        let meetings = store::list(&config.notes_dir);
        let notes = TextEdit::new(meetings.first().map(|m| m.notes.clone()).unwrap_or_default());
        let mut ui = Ui {
            selected: (!meetings.is_empty()).then_some(0),
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
            status: None,
            unseen: HashSet::new(),
            warned: HashSet::new(),
            search: None,
            dirty: None,
            demo: false,
            capsule,
            capsule_shown: Default::default(),
        };
        if let Some(m) = ui.current() {
            ui.view = default_view(m);
        }
        if !ui.config.speech_model.exists() {
            ui.status = Some((
                format!("Speech model missing at {}. Run scripts/download-model.sh.", tilde(&ui.config.speech_model)),
                true,
            ));
        }
        ui
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
            if let Err(e) = m.save() {
                self.warn(format!("Could not save the meeting: {e}"));
            }
        }
    }

    fn save_current(&mut self) {
        if let Some(i) = self.selected {
            self.save(i);
        }
        self.dirty = None;
    }

    fn go_home(&mut self) {
        self.save_current();
        self.selected = None;
    }

    fn select(&mut self, i: usize) {
        if self.selected == Some(i) {
            return;
        }
        self.save_current();
        self.selected = Some(i);
        self.notes = TextEdit::new(self.meetings[i].notes.clone());
        self.view = default_view(&self.meetings[i]);
        let file = self.meetings[i].file.clone();
        self.unseen.remove(&file);
    }

    fn new_meeting(&mut self) {
        if !matches!(self.rec, Rec::Idle) {
            return;
        }
        let meeting = Meeting::new(&self.config.notes_dir);
        self.save_current();
        self.meetings.insert(0, meeting);
        self.selected = None;
        self.select(0);
        self.view = View::Transcript;
        self.start_recording();
        if matches!(self.rec, Rec::Idle) {
            // Recording could not start: don't leave an empty meeting behind.
            self.meetings.remove(0);
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
        let llm = Llm { url: self.config.llm_url.clone(), model: self.config.llm_model.clone() };
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
            send(LlmMsg::Done(file.clone(), result));
            if let Some(t) = title {
                send(LlmMsg::Title(file, t));
            }
        });
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

    fn cycle_language(&mut self) {
        let at = LANGUAGES.iter().position(|(c, _)| *c == self.config.language).unwrap_or(0);
        self.config.language = LANGUAGES[(at + 1) % LANGUAGES.len()].0.to_string();
        self.save_config();
    }

    fn cycle_model(&mut self) {
        let mut models: Vec<PathBuf> = std::fs::read_dir(config::data_dir().join("models"))
            .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "gguf") && !p.ends_with(crate::vad::FILE)).collect())
            .unwrap_or_default();
        models.sort();
        if models.len() < 2 {
            self.status = Some((format!("Speech model: {}", tilde(&self.config.speech_model)), false));
            return;
        }
        let at = models.iter().position(|p| *p == self.config.speech_model).map_or(0, |i| i + 1);
        self.config.speech_model = models[at % models.len()].clone();
        self.save_config();
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
                LlmMsg::Done(file, result) => {
                    self.summarizing = None;
                    let i = self.index_of(&file);
                    match result {
                        Ok(summary) => {
                            self.warned.remove(&file);
                            if let Some(i) = i {
                                self.meetings[i].summary = summary;
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
                LlmMsg::Title(file, title) => {
                    if let Some(i) = self.index_of(&file) {
                        if self.meetings[i].title.starts_with("Meeting on") {
                            self.meetings[i].title = title;
                            self.save(i);
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
            || m.notes.to_lowercase().contains(&q)
            || m.summary.to_lowercase().contains(&q)
            || m.lines.iter().any(|l| l.text.to_lowercase().contains(&q))
    }
}

fn default_view(m: &Meeting) -> View {
    if m.summary.trim().is_empty() { View::Transcript } else { View::Summary }
}

fn setup(s: &mut Ui, waker: Waker) {
    s.wake = waker;
}

fn poll(s: &mut Ui, t: f64, tx: &mut Tx) -> bool {
    s.now = t;
    let mut changed = s.drain();
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

fn avatar(m: &Meeting) -> El {
    let initial = m.title.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "·".into());
    el().size(20.0, 20.0)
        .radius(5.0)
        .fill(avatar_color(m))
        .center()
        .child(text(initial).font(sans(11.0, 650.0)).color(hex("#FFFFFF")))
}

fn dot(color: Color) -> El {
    el().size(7.0, 7.0).radius(3.5).fill(color)
}

fn sidebar(s: &Ui) -> El {
    let mut nav = col().w_full().gap(2.0);
    nav = nav.child(nav_item("nav-home", &HOME, "Home", s.selected.is_none(), |s: &mut Ui| s.go_home()));
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
                        .placeholder("Search meetings", P::dim())
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
        None => nav.child(nav_item("nav-search", &SEARCH, "Search meetings", false, |s: &mut Ui| {
            s.search = Some(TextEdit::new(""));
        })),
    };
    let recording = !matches!(s.rec, Rec::Idle);
    let mut new = nav_item("nav-new", &NEW, "New meeting", false, |s: &mut Ui| s.new_meeting());
    if recording {
        new = new.opacity(0.45);
    }
    nav = nav.child(new);

    let mut list = col().key("meetings").w_full().gap(2.0);
    let mut shown = 0;
    for (i, m) in s.meetings.iter().enumerate() {
        if !s.matches_search(m) {
            continue;
        }
        shown += 1;
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
            .h(38.0)
            .px(10.0)
            .gap(12.0)
            .center_y()
            .radius(9.0)
            .fill(if selected { P::raised() } else { P::clear() })
            .hover(P::raised())
            .role("button")
            .label(m.title.clone())
            .on_press(move |s: &mut Ui| s.select(i))
            .child(avatar(m))
            .child(
                text(m.title.clone())
                    .font(sans(14.5, if selected { 500.0 } else { 400.0 }))
                    .color(if selected { P::text() } else { P::secondary() })
                    .truncate()
                    .grow(1.0)
                    .min_w(0.0),
            );
        if let Some(c) = marker {
            r = r.child(dot(c));
        }
        list = list.child(r);
    }
    if shown == 0 {
        let msg = if s.meetings.is_empty() { "No meetings yet" } else { "Nothing matches" };
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
}

// ---- View: main panel -------------------------------------------------------------------------

fn main_panel(s: &Ui) -> El {
    let (title, body, footer) = match s.current() {
        Some(m) => {
            let i = s.selected.unwrap();
            match s.view {
                View::Transcript => (m.title.clone(), transcript_view(s, m), footer(s, Some(m))),
                View::Summary => (m.title.clone(), summary_view(s, i, m), footer(s, Some(m))),
            }
        }
        None => ("Home".to_string(), home_view(s), footer(s, None)),
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
    row().gap(2.0).child(item("view-transcript", "Transcript", View::Transcript)).child(item("view-summary", "Summary", View::Summary))
}

/// A scrolling area whose content sits in a centred column at most 720 points wide.
fn column(key: &str, content: El, follow_end: bool) -> El {
    let centred = row()
        .w_full()
        .child(el().grow(1.0))
        .child(content.grow(1000.0).max_w(COLUMN).min_w(0.0))
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
        .child(chip_button("model", model_label(&s.config.speech_model), true, |s: &mut Ui| s.cycle_model()))
        .child(chip_button("language", language_label(&s.config.language), true, |s: &mut Ui| s.cycle_language()))
        .child(text("Local only").font(sans(14.0, 400.0)).color(P::secondary()).px(8.0).help("Audio, transcript and summary never leave this machine."))
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
        let l = raw.trim_start();
        let task = l.strip_prefix("- [ ] ").map(|t| (false, t)).or_else(|| {
            l.strip_prefix("- [x] ").or_else(|| l.strip_prefix("- [X] ")).map(|t| (true, t))
        });
        match task {
            Some((done, t)) => {
                if !md.trim().is_empty() {
                    out.push(Piece::Md(std::mem::take(&mut md)));
                }
                md.clear();
                let (text, due) = match t.rsplit_once(" · ") {
                    Some((head, due)) if due.chars().count() <= 14 && !due.trim().is_empty() => (head.to_string(), Some(due.trim().to_string())),
                    _ => (t.to_string(), None),
                };
                out.push(Piece::Task { line: n, done, text: text.replace('`', ""), due });
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
        .border(if done { 0.0 } else { 1.5 }, P::secondary());
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
        c = c
            .child(text("No summary yet").font(sans(17.0, 600.0)).color(P::text()))
            .child(
                text(format!(
                    "Hushnote writes it with a local LLM ({} at {}) when the meeting ends. Nothing leaves this machine.",
                    s.config.llm_model, s.config.llm_url
                ))
                .font(sans(15.0, 400.0))
                .color(P::secondary())
                .wrap(),
            );
        if !m.lines.is_empty() || !m.notes.trim().is_empty() {
            c = c.child(row().pt(6.0).child(action_button("summarize", "Write summary", el(), |s: &mut Ui| s.summarize())));
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
        c = c.child(saved_file(m)).child(row().child(action_button("resummarize", "Rewrite summary", el(), |s: &mut Ui| s.summarize())));
    }
    column("summary", c, running)
}

/// Where the summary lives on disk, as the file shows it.
fn saved_file(m: &Meeting) -> El {
    let name = m.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut c = col()
        .key("saved-file")
        .w_full()
        .mt(10.0)
        .px(18.0)
        .py(16.0)
        .gap(8.0)
        .radius(9.0)
        .fill(P::well())
        .border(1.0, P::line())
        .child(text(name).font(mono(12.5)).color(P::secondary()))
        .child(text("<!-- hushnote:summary -->").font(mono(12.5)).color(P::accent()));
    for l in m.summary.lines().filter(|l| !l.trim().is_empty()).take(2) {
        c = c.child(text(l.to_string()).font(mono(12.5)).color(P::text()).truncate());
    }
    c
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
    let c = col()
        .w_full()
        .gap(18.0)
        .pt(64.0)
        .child(wordmark(0.62))
        .child(
            text("Meeting notes that never leave your machine. Transcribed live with Parakeet, summarized by a local LLM, saved as Markdown.")
                .font(sans(15.0, 400.0))
                .color(P::secondary())
                .wrap(),
        )
        .child(
            row()
                .pt(8.0)
                .gap(36.0)
                .child(stat("meetings", s.meetings.len().to_string()))
                .child(stat("recorded", minutes(total_ms)))
                .child(stat("open action items", open_tasks.to_string())),
        )
        .child(row().pt(10.0).child(action_button("home-new", "New meeting", dot(P::rec()).size(9.0, 9.0).radius(4.5), |s: &mut Ui| s.new_meeting())));
    column("home", c, false)
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
        Some(m) if s.view == View::Summary && !m.summary.trim().is_empty() => row()
            .gap(22.0)
            .child(text(format!("Summary by {}", s.config.llm_model)).font(sans(13.5, 400.0)).color(P::secondary()))
            .child(text(minutes(m.duration_ms)).font(sans(13.5, 400.0)).color(P::secondary())),
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
    sync.summary = "## Decisions\n- The beta ships on `Friday`.\n- Maya owns the pricing page copy.\n- Acme gets a live demo on Monday, not a recording.\n\n\
                    ## Action items\n- [ ] Dani fixes the sign-up bug · Thu\n- [ ] Maya sends the pricing copy · Wed\n- [x] Prepare the Acme demo · Mon\n\n\
                    ## Open questions\n- Does the free tier need a usage cap before launch?\n- Who runs the support inbox during the first week?"
        .into();
    vec![
        sync,
        meeting("2026-11-16-acme-kickoff.md", "Acme kickoff call", "2026-11-16 15:00", 32),
        meeting("2026-11-14-design-review.md", "Design review", "2026-11-14 11:30", 58),
        meeting("2026-11-13-maya.md", "1:1 with Maya", "2026-11-13 09:30", 25),
        meeting("2026-11-12-hiring.md", "Hiring debrief", "2026-11-12 17:00", 41),
        meeting("2026-11-10-quarterly.md", "Quarterly planning", "2026-11-10 10:00", 84),
    ]
}

fn scene_demo(s: &mut Ui) {
    s.demo = true;
    s.config = Config { language: "en".into(), ..Config::default() };
    s.config.notes_dir = dirs::home_dir().unwrap_or_default().join("Documents").join("Hushnote");
    s.meetings = demo_meetings();
    s.selected = Some(0);
    s.notes = TextEdit::new(s.meetings[0].notes.clone());
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

fn scene_home(s: &mut Ui) {
    scene_demo(s);
    s.selected = None;
}

fn scene_empty(s: &mut Ui) {
    s.demo = true;
    s.config = Config::default();
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
            ("empty", scene_empty),
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
