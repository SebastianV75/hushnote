//! Hushnote: notas de reuniones con transcripción y resúmenes 100% locales.
//!
//! - `hushnote` abre la app (Rune). Acepta las banderas de Rune para verificarla sin pantalla:
//!   `hushnote --snap ui.png --scene demo`, `--tree`, `--script`…
//! - `hushnote transcribir <archivo.wav> [--resumir]` prueba el motor sin interfaz.
//! - `hushnote --capsula --snap capsula.png` dibuja sola la cápsula que se ve mientras graba.

mod audio;
mod config;
mod llm;
mod segmenter;
mod session;
mod store;
mod ui;

use config::Config;
use session::Event;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, mpsc};

const AYUDA: &str = "Hushnote: notas de reuniones 100% locales.

Uso:
  hushnote                                 abre la app
  hushnote transcribir <archivo.wav> [--resumir]
                                          transcribe un WAV y, si se pide, lo resume con el LLM local
  hushnote --ayuda                         esta ayuda

Configuración: ~/.config/hushnote/config.json";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("transcribir") => transcribir(&args[1..]),
        Some("--ayuda" | "-h" | "--help" | "ayuda") => {
            println!("{AYUDA}");
            ExitCode::SUCCESS
        }
        _ if args.iter().any(|a| a == "--capsula") => {
            ui::run_capsule(args.into_iter().filter(|a| a != "--capsula").collect());
            ExitCode::SUCCESS
        }
        _ => {
            ui::run(args);
            ExitCode::SUCCESS
        }
    }
}

fn transcribir(args: &[String]) -> ExitCode {
    let resumir = args.iter().any(|a| a == "--resumir");
    let Some(wav) = args.iter().find(|a| !a.starts_with("--")).map(PathBuf::from) else {
        eprintln!("Falta el archivo. Uso: hushnote transcribir <archivo.wav> [--resumir]");
        return ExitCode::from(2);
    };
    let config = Config::load();
    let (tx, rx) = mpsc::channel();
    eprintln!("Modelo: {}", config.modelo_voz.display());
    let started = std::time::Instant::now();
    if let Err(e) = session::transcribe_file(&wav, config.modelo_voz.clone(), config.idioma(), tx, Arc::new(|| {})) {
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
    eprintln!("Transcrito en {:.1} s.", started.elapsed().as_secs_f32());
    if failed && lines.is_empty() {
        return ExitCode::FAILURE;
    }
    if resumir {
        let llm = llm::Llm { url: config.llm_url.clone(), model: config.llm_modelo.clone() };
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
