# Hushnote: plan and context

A desktop app for meeting notes, in the spirit of Granola: it records the meeting, transcribes
it live and writes notes with decisions, action items and open questions. **Everything stays
on the machine.** Main target: Linux on Omarchy (Hyprland, Wayland, PipeWire).

## Decisions

- **UI: [Rune](https://github.com/anomalyco/rune)** (Rust, GPU, by Anomaly/OpenCode), pinned to
  commit `842c28c2ca5dd1506c1e4a67052db61800aed721` on `dev`. Pre-release: the API may change,
  and they don't take PRs.
  - Layer-shell overlays on Wayland (`App::overlay`, `OverlaySpec`) draw the recording
    capsule, but **on Wayland they take no clicks** (display only).
  - `rune::Recorder` records audio **on macOS only**, so on Linux we capture ourselves.
  - Headless mode (`--snap out.png --scene NAME`, `--script`, `--tree`) checks the UI without
    a display. Script time is virtual (it advances per frame), so it can't drive real-time
    audio: the end-to-end test drives the UI state from a Rust test instead.
  - `rune-kit` (feature `markdown`) renders the summary.
- **Look:** Rune/OpenCode style, dark first. Geist and Geist Mono (OFL, in `fonts/`), palette
  in `src/ui.rs` (`P`), mockups and logo in `assets/` (maintained elsewhere; not edited from
  this side).
- **Transcription: `transcribe-cpp` =0.4.2**, the engine
  [Hex](https://github.com/anomalyco/hex) uses (MIT, also by Anomaly and built with Rune). The
  bindings must match `transcribe-cpp-sys` exactly, hence the `=` pin.
  - The models are Hex's exact pinned artifacts (same repositories, revisions and sha256),
    fetched by `scripts/download-model.sh [v3|english|whisper]` into
    `~/.local/share/hushnote/models/`:
    - **Parakeet TDT 0.6B v3 Q8_0** (default; 25 European languages incl. Spanish),
      `handy-computer/parakeet-tdt-0.6b-v3-gguf` rev `85ac09ea…`, sha256 `5859f779…`.
    - Parakeet Unified English (Hex's pick for English), rev `7e948f21…`, sha256 `4b50b6dd…`.
    - Whisper large-v3-turbo (Hex's pick for other languages), rev `d222c9f6…`,
      sha256 `d5e65f2b…`.
    - Silero VAD v6.2 (1.2 MB, rev `a9b2d61c…`, sha256 `0d871a80…`), downloaded with every
      model, as Hex does.
- **Speech detection (`src/vad.rs`), as Hex's `speech_detector`:** the energy segmenter cuts
  phrases; Silero then drops phrases without speech and trims the rest to it (threshold 0.4,
  min speech 160 ms, 250 ms margin). Optional: without its file, everything is transcribed as
  before. Measured: 30 s of fan noise, hum and clicks took 0.3 s instead of 1.9 s (the model
  never runs), and speech after 6 s of noise is stamped 00:05 instead of 00:00.
  - Feature `vulkan` for the GPU; without it, it runs on the CPU.
- **Why not use Hex directly:** Hex has Meetings with Linux code (pw-record per output, call
  detection with pw-dump), but `src/labs/catalog.rs:242` limits it to macOS, and on Linux its
  summaries only go through OpenCode (usually a cloud model). We borrow ideas and code from Hex
  with attribution (MIT). An issue for anomalyco/hex about Meetings on Linux could be proposed,
  only if the user approves (it would be posted from their account).
- **Audio:** `pw-record` (or `parec` as a fallback). Mic = "You", system monitor
  (`stream.capture.sink=true`) = "Them". `--raw` only with PipeWire ≥ 1.3.81 (detected through
  `--help`). Each capture runs in its own process group, and stopping kills the whole group, so
  a wrapper's children can't keep the pipe open and stall the transcript.
- **Screen awake while recording:** the Wayland `idle-inhibit` protocol (`src/inhibit.rs`), held
  from Record to Stop, so a class with no input is not locked or blanked. logind's `idle`
  inhibitors are not read by Hyprland. It dies with the process; closing the lid still suspends.
- **Summaries:** by default a local LLM through an OpenAI-compatible API
  (`/v1/chat/completions`): Ollama, llama-server or LM Studio. Default
  `http://localhost:11434/v1`, model `qwen3:4b-instruct-2507-q4_K_M` (~2.5 GB, ~3–4 GB of RAM
  only while writing; the smallest that writes reliable notes and task lists in Spanish and
  English). Not plain `qwen3:4b`: that tag now points at the thinking build, which ignores
  `/no_think` and reasons for minutes on a CPU. Older configs with `qwen3:4b` are migrated.
  - Optional, picked in the composer ("Summary · …") or in Home › Set up: **Claude** (the user's
    subscription, `claude -p` with no tools, no settings and no saved session), **Codex**
    (`codex exec` read-only, ephemeral, ignoring the user's config) or **OpenCode** (`opencode run
    --pure --format json`; its free models need no account). These send the transcript to that
    service, and the UI says so. Each runs in an empty temp folder. Only installed CLIs are
    offered. `summary_cli_model` picks their model (empty = their default). Tested here: the
    same Spanish meeting summarized correctly by all three (Claude ~6 s, Codex and OpenCode
    ~18 s).
  Prompts are in English; the summary is written in the language picked in the composer, with
  four sections whose headings the app gives already translated (a 4B model left some in
  English): Key points, Decisions, Action items (`- [ ] Owner does the task · due`) and Open
  questions; in Spanish, Puntos clave, Decisiones, Pendientes and Preguntas abiertas. The
  meeting file records who wrote the summary (`summary_by:` in the front matter). With the local model long meetings are summarized in parts; the hosted
  ones take the whole meeting. `<think>` blocks are filtered.
- **Languages (for now Spanish and English):** the composer's language picker also picks the
  speech model, as Hex recommends: Spanish → Parakeet v3, English → Parakeet Unified English.
  Switching to one that isn't downloaded offers its one-click Download. Parakeet v3 ignores the
  language hint (only prompt-conditioned Parakeet variants read it), and alone a short phrase
  could come out in the wrong language ("Sì, claro") or misheard ("lo vemos Mayara"). So each
  phrase is transcribed after the last 5 s of the same person's speech and only the words past
  that point are kept (word timestamps); if it still has letters Spanish/English don't use, it
  is retried with 15 s. On a conversation of short phrases this fixed every slip seen without
  context, for ~0.25 s more per phrase. Canary 1B, Nemotron 3.5 and Whisper turbo, which do take
  a language, were worse on Spanish with English tech words or far too slow on CPU.
  The app reads the model file once at start (and on a language switch) so the first meeting
  doesn't wait on the disk; that only fills the page cache.
- **One-click setup (Home › Set up), as in Hex:** the speech model downloads from the app
  (resumable, sha256-checked, with progress and Cancel; Silero VAD comes along), and the
  composer offers Download instead of Record while it is missing. The summary model is pulled
  through Ollama's `/api/pull` with progress; if Ollama isn't running, Home says how to start it
  (`sudo pacman -S ollama && sudo systemctl enable --now ollama`). `src/models.rs`,
  `src/ollama.rs`.
- **Storage:** one Markdown file per meeting in `~/Documents/Hushnote` (or `~/Hushnote`), with
  sections marked `<!-- hushnote:notes -->`, `<!-- hushnote:summary -->` and
  `<!-- hushnote:transcript -->` so it can be read back.
- **Release and install:** `scripts/ci.sh` (on this machine, as CI) runs the tests and builds a
  portable release tarball (speech engine pinned to AVX2, never AVX-512); `scripts/install.sh`
  installs it, and `--uninstall [--purge]` removes it. See `docs/install.md`.
- **Settings:** `~/.config/hushnote/config.json` (created on first run; the app saves the model
  and language picked in the composer).

## Status

Done and tested in the sandbox:
- `hushnote transcribe <file.wav> [--summarize]`: Spanish speech transcribed almost perfectly
  with correct timestamps; without an LLM, `--summarize` fails with a clear message.
- The UI (`src/ui.rs`): sidebar (Home, Search meetings, New meeting, meetings with avatar and
  red/blue/yellow dots), main panel with live chat-style transcript, "My notes" composer with
  model/language pickers and Record/Stop, You/Them meters, summary view with clickable tasks and
  due days, Home with the block wordmark, recording capsule. `--snap` scenes: `summary`,
  `transcript`, `recording`, `home`, `empty`; `--capsule` for the capsule.
- End to end through the UI state with a fake `pw-record` playing Spanish speech: New meeting →
  phrases arrive live → Stop → transcript finishes → meeting saved with the notes; the automatic
  summary fails cleanly without an LLM. `cargo test e2e -- --ignored --nocapture` (needs the
  model, the fake `pw-record` on `PATH` and `XDG_CONFIG_HOME` pointing at scratch settings).
- 14 unit tests (`cargo test`); ignored ones for the end-to-end run and the real Silero model.

Not tested here (no PipeWire, GPU or disk for an LLM in the sandbox): real mic and system
audio, the capsule as a Hyprland overlay, summaries with Ollama, `--features vulkan`.

Next:
1. Try it on Omarchy: audio, capsule, Ollama summary, Vulkan.
2. Later: diarization (Sortformer in transcribe-cpp), importing audio from the UI, a Waybar
   icon and a global shortcut.

## Development environment (instabox sandbox)

- Rust in `RUSTUP_HOME=/data/home/.rustup`, `CARGO_HOME=/data/home/.cargo`; export both before
  running `cargo`.
- The sandbox is XFCE/X11 without PipeWire or a GPU. The UI is checked headless (lavapipe);
  real capture is tried on the user's Omarchy machine.
- Disk is tight (~5 GB free in /data).
- References cloned in `/data/workspace/ref/rune` and `/data/workspace/ref/hex`.
