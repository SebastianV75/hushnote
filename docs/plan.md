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
- **Summaries:** a local LLM through an OpenAI-compatible API (`/v1/chat/completions`):
  Ollama, llama-server or LM Studio. Default `http://localhost:11434/v1`, model `qwen3:8b`.
  Prompts are in English; the summary is written in the meeting's language, as
  `## Decisions`, `## Action items` (`- [ ] Owner does the task · due`) and
  `## Open questions`. Long meetings are summarized in parts; `<think>` blocks are filtered.
- **Storage:** one Markdown file per meeting in `~/Documents/Hushnote` (or `~/Hushnote`), with
  sections marked `<!-- hushnote:notes -->`, `<!-- hushnote:summary -->` and
  `<!-- hushnote:transcript -->` so it can be read back.
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
- 12 unit tests (`cargo test`).

Not tested here (no PipeWire, GPU or disk for an LLM in the sandbox): real mic and system
audio, the capsule as a Hyprland overlay, summaries with Ollama, `--features vulkan`.

Next:
1. Silero VAD v6.2 as Hex uses it (`handy-computer/silero-vad-v6.2-gguf`), instead of the
   energy VAD, so silence and noise don't get transcribed (or hallucinated).
2. Try it on Omarchy: audio, capsule, Ollama summary, Vulkan.
3. Later: diarization (Sortformer in transcribe-cpp), importing audio from the UI, a Waybar
   icon and a global shortcut.

## Development environment (instabox sandbox)

- Rust in `RUSTUP_HOME=/data/home/.rustup`, `CARGO_HOME=/data/home/.cargo`; export both before
  running `cargo`.
- The sandbox is XFCE/X11 without PipeWire or a GPU. The UI is checked headless (lavapipe);
  real capture is tried on the user's Omarchy machine.
- Disk is tight (~5 GB free in /data).
- References cloned in `/data/workspace/ref/rune` and `/data/workspace/ref/hex`.
