//! Hushnote: meeting notes with transcription and summaries that stay 100% on your machine.
//!
//! - `hushnote` opens the app (Rune). It takes Rune's flags for checking the UI without a
//!   display: `hushnote --snap ui.png --scene demo`, `--tree`, `--script`…
//! - `hushnote transcribe <file.wav> [--summarize]` tries the engine without the UI.
//! - `hushnote --capsule --snap capsule.png` draws the recording capsule on its own.

mod audio;
mod config;
mod export;
mod llm;
mod models;
mod ollama;
mod segmenter;
mod session;
mod store;
mod ui;
mod vad;

use config::Config;
use session::Event;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, mpsc};

const HELP: &str = "Hushnote: meeting notes that stay on your machine.

Usage:
  hushnote                                  open the app
  hushnote transcribe <file.wav> [--summarize]
                                            transcribe a WAV file and, if asked, summarize it
                                            with the local LLM
  hushnote --help                           show this help

Settings: ~/.config/hushnote/config.json";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("transcribe") => transcribe(&args[1..]),
        Some("--help" | "-h" | "help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        _ if args.iter().any(|a| a == "--capsule") => {
            ui::run_capsule(args.into_iter().filter(|a| a != "--capsule").collect());
            ExitCode::SUCCESS
        }
        _ => {
            ui::run(args);
            ExitCode::SUCCESS
        }
    }
}

fn transcribe(args: &[String]) -> ExitCode {
    let summarize = args.iter().any(|a| a == "--summarize");
    let Some(wav) = args.iter().find(|a| !a.starts_with("--")).map(PathBuf::from) else {
        eprintln!("Missing the file. Usage: hushnote transcribe <file.wav> [--summarize]");
        return ExitCode::from(2);
    };
    let config = Config::load();
    let (tx, rx) = mpsc::channel();
    eprintln!("Model: {}", config.speech_model.display());
    let started = std::time::Instant::now();
    if let Err(e) = session::transcribe_file(&wav, config.speech_model.clone(), config.language(), tx, Arc::new(|| {})) {
        eprintln!("hushnote: {e}");
        return ExitCode::FAILURE;
    }
    let mut lines = Vec::new();
    let mut failed = false;
    for event in rx {
        match event {
            Event::Line(line) => {
                println!("[{}] {}", llm::clock(line.t_ms), line.text);
                lines.push(line);
            }
            Event::Status(s) => eprintln!("{s}"),
            Event::Error(e) => {
                eprintln!("hushnote: {e}");
                failed = true;
            }
            Event::Finished => break,
        }
    }
    eprintln!("Transcribed in {:.1} s.", started.elapsed().as_secs_f32());
    if failed && lines.is_empty() {
        return ExitCode::FAILURE;
    }
    if summarize {
        let llm = llm::Llm::from_config(&config);
        println!();
        let result = llm.summarize("", &lines, |delta| {
            print!("{delta}");
            let _ = std::io::stdout().flush();
        });
        println!();
        if let Err(e) = result {
            eprintln!("hushnote: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
