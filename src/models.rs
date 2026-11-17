//! The speech models Hushnote can download in one click: the exact files Hex pins (same
//! repositories, revisions and sha256), plus Silero VAD, which comes with every model.
//! Downloads resume from a `.part` file and are checked before they are renamed into place.

use crate::config;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Artifact {
    pub name: &'static str,
    /// What it is good for, for the picker.
    pub note: &'static str,
    pub repo: &'static str,
    pub rev: &'static str,
    pub file: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
}

pub const PARAKEET_V3: Artifact = Artifact {
    name: "Parakeet v3",
    note: "25 European languages, incl. Spanish",
    repo: "handy-computer/parakeet-tdt-0.6b-v3-gguf",
    rev: "85ac09ea12fc4b1112fa76810059364bc6adc9de",
    file: "parakeet-tdt-0.6b-v3-Q8_0.gguf",
    sha256: "5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7",
    bytes: 739_508_576,
};

/// Hex's pick for English: more accurate on it than v3.
pub const PARAKEET_EN: Artifact = Artifact {
    name: "Parakeet English",
    note: "English only, the most accurate for it",
    repo: "handy-computer/parakeet-unified-en-0.6b-gguf",
    rev: "7e948f21b7bdbac698d3318db9d350f1096f3b6c",
    file: "parakeet-unified-en-0.6b-Q8_0.gguf",
    sha256: "4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795",
    bytes: 731_357_568,
};

/// The models the app offers, one per language it supports for now. (Whisper large-v3-turbo,
/// Hex's pick for other languages, stays in `scripts/download-model.sh`.)
pub const SPEECH: [Artifact; 2] = [PARAKEET_V3, PARAKEET_EN];

/// The languages the app offers, with the model each one uses, as Hex recommends: Parakeet
/// Unified English for English, Parakeet v3 for Spanish.
pub const LANGUAGES: [(&str, &str); 2] = [("es", "Spanish"), ("en", "English")];

pub fn for_language(code: &str) -> &'static Artifact {
    if code == "en" { &PARAKEET_EN } else { &PARAKEET_V3 }
}

pub const VAD: Artifact = Artifact {
    name: "Silero VAD",
    note: "skips audio without speech",
    repo: "handy-computer/silero-vad-v6.2-gguf",
    rev: "a9b2d61c20ae4ddf3644edb34969e8185a485448",
    file: crate::vad::FILE,
    sha256: "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6",
    bytes: 1_241_056,
};

pub fn dir() -> PathBuf {
    config::data_dir().join("models")
}

impl Artifact {
    pub fn path(&self) -> PathBuf {
        dir().join(self.file)
    }

    /// Downloaded (checked by size; the checksum was checked when it was downloaded).
    pub fn installed(&self) -> bool {
        std::fs::metadata(self.path()).is_ok_and(|m| m.len() == self.bytes)
    }

    pub fn url(&self) -> String {
        format!("https://huggingface.co/{}/resolve/{}/{}", self.repo, self.rev, self.file)
    }

    /// Downloads the file, resuming a partial download, and checks its sha256. `progress`
    /// gets the bytes so far and the total. Returns early with an error if `cancel` is set.
    pub fn download(&self, cancel: &AtomicBool, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
        let dest = self.path();
        if self.installed() {
            return Ok(dest);
        }
        std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
        let part = dest.with_extension("gguf.part");
        let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have > self.bytes {
            have = 0;
        }
        if have < self.bytes {
            let mut request = ureq::get(&self.url());
            if have > 0 {
                request = request.header("Range", &format!("bytes={have}-"));
            }
            let response = request.call().map_err(|e| format!("Couldn't reach huggingface.co: {e}"))?;
            if have > 0 && response.status() != 206 {
                have = 0; // the server ignored the range: start over
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(have > 0)
                .truncate(have == 0)
                .open(&part)
                .map_err(|e| format!("{}: {e}", part.display()))?;
            let mut body = response.into_body().into_with_config().limit(self.bytes + 1).reader();
            let mut buf = vec![0u8; 1 << 16];
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Download canceled.".into());
                }
                let n = body.read(&mut buf).map_err(|e| format!("Download interrupted ({e}). Try again to resume it."))?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", part.display()))?;
                have += n as u64;
                progress(have, self.bytes);
            }
            file.flush().map_err(|e| e.to_string())?;
        }
        if sha256_file(&part)? != self.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{} came out corrupted; it was deleted. Try again.", self.name));
        }
        std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
        Ok(dest)
    }
}

/// The artifact for a configured model path, if it is one of the catalog's.
pub fn for_path(path: &std::path::Path) -> Option<&'static Artifact> {
    let file = path.file_name()?.to_str()?;
    SPEECH.iter().find(|a| a.file == file)
}

fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// "312 MB", "1.2 MB".
pub fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb < 10.0 { format!("{mb:.1} MB") } else { format!("{mb:.0} MB") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_paths_are_recognised() {
        assert_eq!(for_path(&PARAKEET_V3.path()).map(|a| a.name), Some("Parakeet v3"));
        assert!(for_path(std::path::Path::new("/x/custom.gguf")).is_none());
        assert_eq!(megabytes(VAD.bytes), "1.2 MB");
        assert_eq!(megabytes(PARAKEET_V3.bytes), "740 MB");
        assert_eq!(for_language("en").name, "Parakeet English");
        assert_eq!(for_language("es").name, "Parakeet v3");
    }

    /// Downloads Silero VAD (1.2 MB) into a scratch data dir, interrupted once and resumed.
    /// `cargo test download -- --ignored`
    #[test]
    #[ignore = "uses the network"]
    fn download_resumes_and_checks_the_checksum() {
        let tmp = std::env::temp_dir().join(format!("hushnote-dl-{}", std::process::id()));
        // SAFETY: the test sets the variable before any thread reads it.
        unsafe { std::env::set_var("XDG_DATA_HOME", &tmp) };
        let cancel = AtomicBool::new(false);
        let err = VAD.download(&cancel, |have, _| {
            if have > 300_000 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert!(err.is_err(), "the first try is canceled");
        let part = VAD.path().with_extension("gguf.part");
        let partial = std::fs::metadata(&part).unwrap().len();
        assert!(partial > 0 && partial < VAD.bytes);
        let cancel = AtomicBool::new(false);
        let mut first = None;
        let path = VAD.download(&cancel, |have, _| {
            first.get_or_insert(have);
        })
        .unwrap();
        assert!(first.unwrap() > partial, "it resumed instead of starting over");
        assert!(VAD.installed() && path.exists() && !part.exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
