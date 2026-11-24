//! Updates from GitHub releases: once a day (at most) the app asks which release is the latest;
//! when it is newer, the notice offers "Update", which downloads the release tarball and its
//! `.sha256`, checks it, takes the binary out and swaps it in for the running one with a rename.
//! Every URL and path is built from the parsed version (`x.y.z`), never from what the API
//! returns, and everything goes over HTTPS. The check is silent when it fails, so working
//! offline is the same as before.

use crate::{config, models};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where to send people whose copy can't update itself.
pub const RELEASES_URL: &str = "https://github.com/SebastianV75/hushnote/releases/latest";

const REPO: &str = "SebastianV75/hushnote";
const LATEST_URL: &str = "https://api.github.com/repos/SebastianV75/hushnote/releases/latest";
const CHECK_EVERY_S: u64 = 24 * 60 * 60;
const MAX_JSON: u64 = 1 << 20;
const MAX_CHECKSUM: u64 = 4096;
/// The tarball is ~18 MB; this is a ceiling against a wrong or hostile response.
const MAX_TARBALL: u64 = 120_000_000;
const MAX_BINARY: u64 = 400_000_000;

// ---- Versions ------------------------------------------------------------------------------

/// `major.minor.patch`, compared number by number (0.10.0 is newer than 0.9.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// "0.2.1" or "v0.2.1"; anything else (pre-release suffixes, missing or extra parts,
    /// signs, leading zeros) is `None`.
    pub fn parse(s: &str) -> Option<Version> {
        let mut parts = s.strip_prefix('v').unwrap_or(s).split('.');
        let mut number = || {
            let p = parts.next()?;
            let ok = !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()) && (p == "0" || !p.starts_with('0'));
            if ok { p.parse().ok() } else { None }
        };
        let v = Version(number()?, number()?, number()?);
        parts.next().is_none().then_some(v)
    }

    pub fn current() -> Version {
        Version::parse(VERSION).expect("Cargo.toml has an x.y.z version")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

fn asset_name(v: Version) -> String {
    format!("hushnote-{v}-linux-x86_64.tar.gz")
}

fn asset_url(v: Version, name: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/v{v}/{name}")
}

/// The version of a `releases/latest` response, if it is a published (not draft, not
/// pre-release) release tagged `vX.Y.Z` that carries the tarball and its checksum.
pub fn parse_release(json: &str) -> Option<Version> {
    let release: serde_json::Value = serde_json::from_str(json).ok()?;
    if release["draft"].as_bool() != Some(false) || release["prerelease"].as_bool() != Some(false) {
        return None;
    }
    let version = Version::parse(release["tag_name"].as_str()?.strip_prefix('v')?)?;
    let tarball = asset_name(version);
    let checksum = format!("{tarball}.sha256");
    let has = |name: &str| release["assets"].as_array().is_some_and(|a| a.iter().any(|x| x["name"].as_str() == Some(name)));
    (has(&tarball) && has(&checksum)).then_some(version)
}

// ---- What is remembered between launches -----------------------------------------------------------

/// `update.json` in the data folder: when the last successful check was, the newer version it
/// found and the one whose notice was dismissed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Seconds since the Unix epoch; 0 = never.
    pub checked_at: u64,
    pub available: Option<String>,
    pub dismissed: Option<String>,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl State {
    pub fn path() -> PathBuf {
        config::data_dir().join("update.json")
    }

    pub fn load() -> State {
        State::load_from(&State::path())
    }

    pub fn load_from(path: &Path) -> State {
        fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&State::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&tmp, json).and_then(|_| fs::rename(&tmp, path)).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A check is due when the last good one was a day ago, or the clock went back.
    pub fn due(&self, now: u64) -> bool {
        now >= self.checked_at.saturating_add(CHECK_EVERY_S) || self.checked_at > now.saturating_add(CHECK_EVERY_S)
    }

    /// The remembered newer version, if it is still newer than `current` (it is not once the
    /// app has been updated).
    pub fn available(&self, current: Version) -> Option<Version> {
        self.available.as_deref().and_then(Version::parse).filter(|v| *v > current)
    }

    /// What the notice offers: the newer version unless its notice was dismissed.
    pub fn notice(&self, current: Version) -> Option<Version> {
        self.available(current).filter(|v| self.dismissed.as_deref() != Some(&v.to_string()))
    }

    /// Records a successful check that found `latest`.
    pub fn record(&mut self, latest: Version, current: Version, now: u64) {
        self.checked_at = now;
        self.available = (latest > current).then(|| latest.to_string());
    }
}

// ---- Asking GitHub ------------------------------------------------------------------------------------

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(timeout))
        .user_agent("hushnote")
        .build()
        .into()
}

/// The latest published release. Blocks (a few seconds at most): call it off the UI thread.
pub fn check_latest() -> Result<Version, String> {
    let response = agent(Duration::from_secs(8))
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Couldn't reach github.com: {e}"))?;
    let json = response
        .into_body()
        .with_config()
        .limit(MAX_JSON)
        .read_to_string()
        .map_err(|e| format!("Couldn't read the answer from github.com: {e}"))?;
    parse_release(&json).ok_or_else(|| "GitHub has no usable release yet.".to_string())
}

// ---- Where the app can replace itself ----------------------------------------------------------------

/// The installed binary this process is running, resolved once at startup (after a
/// replacement `/proc/self/exe` reads "… (deleted)"), or why it can't be replaced: a build
/// inside a cargo `target/` folder, or a folder the user can't write to.
pub fn install_target() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("can't tell where the app is: {e}"))?;
    check_target(&exe)
}

pub fn check_target(exe: &Path) -> Result<PathBuf, String> {
    let exe = exe.canonicalize().map_err(|e| format!("can't tell where the app is: {e}"))?;
    if exe.components().any(|c| c.as_os_str() == "target") {
        return Err("this is a development build".into());
    }
    if !fs::symlink_metadata(&exe).is_ok_and(|m| m.is_file()) {
        return Err("the app is not a regular file".into());
    }
    let dir = exe.parent().ok_or("the app has no folder")?;
    let writable = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes())
        .is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0);
    if !writable {
        return Err(format!("{} is not writable for you", dir.display()));
    }
    Ok(exe)
}

// ---- Installing ----------------------------------------------------------------------------------------

/// A private folder for one update (mode 0700, unpredictable name), removed when dropped.
pub struct WorkDir(PathBuf);

impl WorkDir {
    /// Creates `root/update-<pid>-<nanos>` and clears what earlier, interrupted updates left in `root`.
    pub fn create(root: &Path) -> Result<WorkDir, String> {
        fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
        if let Ok(entries) = fs::read_dir(root) {
            for e in entries.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("update-")) {
                let _ = fs::remove_dir_all(e.path());
            }
        }
        let path = root.join(format!("update-{}-{}", std::process::id(), nanos()));
        fs::DirBuilder::new().mode(0o700).create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(WorkDir(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// Downloads and installs `version` over `target`. `progress` gets the bytes of the tarball so
/// far and its total (0 when unknown); `allow` is asked right before the swap, and the update
/// stops there if it says no. On any error `target` is as it was.
pub fn install(version: Version, target: &Path, progress: impl FnMut(u64, u64), allow: &dyn Fn() -> bool) -> Result<(), String> {
    let work = WorkDir::create(&config::data_dir().join("update"))?;
    let name = asset_name(version);
    let tarball = work.path().join(&name);
    download(&asset_url(version, &name), &tarball, MAX_TARBALL, progress)?;
    let checksum = download_text(&asset_url(version, &format!("{name}.sha256")))?;
    apply(&tarball, &checksum, version, work.path(), target, allow)
}

fn start_download(url: &str, timeout: Duration) -> Result<ureq::http::Response<ureq::Body>, String> {
    agent(timeout).get(url).call().map_err(|e| format!("Couldn't download the update: {e}"))
}

fn download(url: &str, dest: &Path, max: u64, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
    let response = start_download(url, Duration::from_secs(900))?;
    let total = response.body().content_length().unwrap_or(0);
    if total > max {
        return Err("The update is larger than expected; it was not downloaded.".into());
    }
    let mut file = File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut body = response.into_body().into_with_config().limit(max + 1).reader();
    let mut buf = vec![0u8; 1 << 16];
    let (mut have, mut shown) = (0u64, 0u64);
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("The download was interrupted ({e})."))?;
        if n == 0 {
            break;
        }
        have += n as u64;
        if have > max {
            return Err("The update is larger than expected; it was not downloaded.".into());
        }
        file.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", dest.display()))?;
        if have - shown >= 1 << 18 {
            shown = have;
            progress(have, total.max(have));
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    progress(have, have);
    Ok(())
}

fn download_text(url: &str) -> Result<String, String> {
    start_download(url, Duration::from_secs(30))?
        .into_body()
        .with_config()
        .limit(MAX_CHECKSUM)
        .read_to_string()
        .map_err(|e| format!("Couldn't download the update's checksum: {e}"))
}

/// The sha256 for `name` in a `sha256sum` file (`<64 hex>  <name>`), lowercase.
pub fn parse_sha256(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        let ok = file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then(|| hash.to_ascii_lowercase())
    })
}

/// Checks `tarball` against `checksum_text`, takes `hushnote-<v>-linux-x86_64/hushnote` out of
/// it into `work` and swaps it in for `target`. Nothing touches `target` before the last step.
pub fn apply(tarball: &Path, checksum_text: &str, version: Version, work: &Path, target: &Path, allow: &dyn Fn() -> bool) -> Result<(), String> {
    let name = asset_name(version);
    let expected = parse_sha256(checksum_text, &name).ok_or("The update's checksum file is not valid.")?;
    if models::sha256_file(tarball)? != expected {
        return Err("The update didn't match its checksum, so it was not installed.".into());
    }
    let member = format!("hushnote-{version}-linux-x86_64/hushnote");
    let out = work.join("out");
    fs::DirBuilder::new().mode(0o700).create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let run = Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(&out)
        .arg("--no-same-owner")
        .arg(&member)
        .output()
        .map_err(|e| format!("Couldn't run tar: {e}"))?;
    if !run.status.success() {
        return Err(format!("Couldn't unpack the update: {}", String::from_utf8_lossy(&run.stderr).trim()));
    }
    let binary = out.join(&member);
    let meta = fs::symlink_metadata(&binary).map_err(|_| "The update doesn't contain the app.")?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_BINARY {
        return Err("The update doesn't contain a valid app.".into());
    }
    let mut magic = [0u8; 4];
    if File::open(&binary).and_then(|mut f| f.read_exact(&mut magic)).is_err() || &magic != b"\x7fELF" {
        return Err("The update doesn't contain a valid app.".into());
    }
    replace(&binary, target, allow)
}

/// Removes the `.hushnote-update-<pid>-<n>` copies that an interrupted update left in `dir`,
/// except those of a process that is still running (its update may be in progress).
fn remove_stale_copies(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(".hushnote-update-")) else { continue };
        let alive = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()).is_some_and(|pid| Path::new(&format!("/proc/{pid}")).exists());
        if !alive && entry.file_type().is_ok_and(|t| t.is_file()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Copies `src` next to `target` (same folder, so the rename can't cross filesystems), makes it
/// executable, flushes it and renames it over `target`.
fn replace(src: &Path, target: &Path, allow: &dyn Fn() -> bool) -> Result<(), String> {
    let dir = target.parent().ok_or("The app has no folder.")?;
    remove_stale_copies(dir);
    let tmp = dir.join(format!(".hushnote-update-{}-{}", std::process::id(), nanos()));
    let result = (|| -> Result<(), String> {
        let io = |e: std::io::Error| format!("{}: {e}", tmp.display());
        let mut input = File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
        let mut out = OpenOptions::new().write(true).create_new(true).mode(0o700).open(&tmp).map_err(io)?;
        std::io::copy(&mut input, &mut out).map_err(io)?;
        out.set_permissions(fs::Permissions::from_mode(0o755)).map_err(io)?;
        out.sync_all().map_err(io)?;
        drop(out);
        if !allow() {
            return Err("Recording or other work started, so the update was not installed. Try again when it ends.".into());
        }
        fs::rename(&tmp, target).map_err(|e| format!("{}: {e}", target.display()))
    })();
    match result {
        Ok(()) => {
            let _ = File::open(dir).and_then(|d| d.sync_all());
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hushnote-test-{name}-{}-{}", std::process::id(), nanos()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A tarball shaped like a release, holding `binary`; returns it and its sha256sum file.
    fn fake_release(dir: &Path, version: Version, binary: &[u8]) -> (PathBuf, String) {
        let name = asset_name(version);
        let root = format!("hushnote-{version}-linux-x86_64");
        let src = dir.join("src");
        fs::create_dir_all(src.join(&root)).unwrap();
        fs::write(src.join(&root).join("hushnote"), binary).unwrap();
        fs::write(src.join(&root).join("install.sh"), "#!/bin/sh\n").unwrap();
        let tarball = dir.join(&name);
        let status = Command::new("tar").arg("-czf").arg(&tarball).arg("-C").arg(&src).arg(&root).status().unwrap();
        assert!(status.success());
        let checksum = format!("{}  {name}\n", sha(&fs::read(&tarball).unwrap()));
        (tarball, checksum)
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(v("0.10.0") > v("0.9.0"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.2.2") > v("0.2.1"));
        assert_eq!(v("v0.2.1"), Version(0, 2, 1));
        assert_eq!(v("0.2.1").to_string(), "0.2.1");
        assert_eq!(Version::current().to_string(), VERSION);
    }

    #[test]
    fn malformed_versions_are_rejected() {
        for bad in ["", "v", "1", "1.2", "1.2.3.4", "1.2.x", "1.2.3-rc1", "1.2.3+b", "-1.2.3", "+1.2.3", "01.2.3", "1..3", "1.2.3 ", "vv1.2.3", "99999999.0.0", "latest"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
    }

    fn release(tag: &str, draft: bool, prerelease: bool, assets: &[&str]) -> String {
        let assets: Vec<_> = assets.iter().map(|n| serde_json::json!({ "name": n, "browser_download_url": "https://evil.example/x" })).collect();
        serde_json::json!({ "tag_name": tag, "draft": draft, "prerelease": prerelease, "assets": assets }).to_string()
    }

    #[test]
    fn release_json_is_parsed_and_filtered() {
        let both = ["hushnote-0.3.0-linux-x86_64.tar.gz", "hushnote-0.3.0-linux-x86_64.tar.gz.sha256"];
        assert_eq!(parse_release(&release("v0.3.0", false, false, &both)), Some(v("0.3.0")));
        assert_eq!(parse_release(&release("v0.3.0", true, false, &both)), None, "draft");
        assert_eq!(parse_release(&release("v0.3.0", false, true, &both)), None, "pre-release");
        assert_eq!(parse_release(&release("0.3.0", false, false, &both)), None, "no v");
        assert_eq!(parse_release(&release("v0.3.0-rc1", false, false, &both)), None, "suffix");
        assert_eq!(parse_release(&release("nightly", false, false, &both)), None, "not a version");
        assert_eq!(parse_release(&release("v0.3.0", false, false, &both[..1])), None, "no checksum");
        assert_eq!(parse_release(&release("v0.3.0", false, false, &["hushnote-0.4.0-linux-x86_64.tar.gz", "hushnote-0.4.0-linux-x86_64.tar.gz.sha256"])), None, "assets of another version");
        assert_eq!(parse_release("not json"), None);
        assert_eq!(parse_release("{}"), None);
        assert_eq!(parse_release("[]"), None);
    }

    #[test]
    fn sha256_files_are_parsed() {
        let hash = "a".repeat(64);
        let name = "hushnote-0.3.0-linux-x86_64.tar.gz";
        assert_eq!(parse_sha256(&format!("{hash}  {name}\n"), name), Some(hash.clone()));
        assert_eq!(parse_sha256(&format!("{hash} *{name}"), name), Some(hash.clone()), "binary mode marker");
        assert_eq!(parse_sha256(&format!("{}  {name}", hash.to_uppercase()), name), Some(hash.clone()), "lowercased");
        assert_eq!(parse_sha256(&format!("{}  other.tar.gz\n{hash}  {name}\n", "b".repeat(64)), name), Some(hash.clone()), "picks the right line");
        assert_eq!(parse_sha256(&format!("{hash}  other.tar.gz"), name), None, "another file");
        assert_eq!(parse_sha256(&format!("{hash}  ../{name}"), name), None, "another path");
        assert_eq!(parse_sha256(&format!("{}  {name}", "a".repeat(63)), name), None, "short hash");
        assert_eq!(parse_sha256(&format!("{}  {name}", "g".repeat(64)), name), None, "not hex");
        assert_eq!(parse_sha256("", name), None);
    }

    #[test]
    fn the_check_is_due_once_a_day() {
        let day = CHECK_EVERY_S;
        let mut s = State::default();
        assert!(s.due(1_000_000), "never checked");
        s.record(v("0.2.1"), v("0.2.1"), 1_000_000);
        assert!(!s.due(1_000_000));
        assert!(!s.due(1_000_000 + day - 1));
        assert!(s.due(1_000_000 + day));
        assert!(s.due(500_000 - day - 1), "the clock went back more than a day");
        assert!(!s.due(999_000), "the clock went back a little");
    }

    #[test]
    fn the_known_version_is_remembered_dismissed_and_forgotten_once_installed() {
        let mut s = State::default();
        s.record(v("0.2.2"), v("0.2.1"), 10);
        assert_eq!(s.notice(v("0.2.1")), Some(v("0.2.2")));
        s.dismissed = Some("0.2.2".into());
        assert_eq!(s.notice(v("0.2.1")), None);
        assert_eq!(s.available(v("0.2.1")), Some(v("0.2.2")), "still known, only the notice is hidden");
        s.record(v("0.2.3"), v("0.2.1"), 20);
        assert_eq!(s.notice(v("0.2.1")), Some(v("0.2.3")), "a newer one is announced again");
        assert_eq!(s.available(v("0.2.3")), None, "after updating");
        s.record(v("0.2.0"), v("0.2.1"), 30);
        assert_eq!(s.available(v("0.2.1")), None, "never a downgrade");
    }

    #[test]
    fn the_state_survives_a_restart_and_a_broken_file() {
        let dir = scratch("state");
        let path = dir.join("data").join("update.json");
        assert_eq!(State::load_from(&path), State::default());
        let mut s = State::default();
        s.record(v("0.2.2"), v("0.2.1"), 42);
        s.save_to(&path).unwrap();
        assert_eq!(State::load_from(&path), s);
        fs::write(&path, "{ nope").unwrap();
        assert_eq!(State::load_from(&path), State::default());
        fs::write(&path, r#"{"checked_at": 7}"#).unwrap();
        assert_eq!(State::load_from(&path).checked_at, 7, "missing fields take defaults");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn old_config_files_load_with_updates_on() {
        let old = r#"{ "language": "en", "llm_model": "qwen3:4b" }"#;
        let config: config::Config = serde_json::from_str(old).unwrap();
        assert!(config.check_updates);
        assert_eq!(config.language, "en");
        let off: config::Config = serde_json::from_str(r#"{ "check_updates": false }"#).unwrap();
        assert!(!off.check_updates);
        assert!(config::Config::default().check_updates);
    }

    #[test]
    fn leftover_update_copies_are_removed_unless_their_process_is_alive() {
        let dir = scratch("stale");
        let dead = dir.join(".hushnote-update-4294967295-1");
        let alive = dir.join(format!(".hushnote-update-{}-1", std::process::id()));
        let nameless = dir.join(".hushnote-update-old");
        let folder = dir.join(".hushnote-update-4294967294-1");
        let other = dir.join("hushnote");
        for f in [&dead, &alive, &nameless, &other] {
            fs::write(f, "x").unwrap();
        }
        fs::create_dir(&folder).unwrap();
        let src = dir.join("new");
        fs::write(&src, "new").unwrap();
        replace(&src, &other, &|| true).unwrap();
        assert!(!dead.exists() && !nameless.exists(), "leftovers are gone");
        assert!(alive.exists() && folder.is_dir() && other.is_file(), "a live process's copy, folders and the binary stay");
        assert_eq!(fs::read_to_string(&other).unwrap(), "new");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_good_tarball_replaces_the_binary_atomically() {
        let dir = scratch("good");
        let version = v("0.9.0");
        let new = b"\x7fELF the new app".to_vec();
        let (tarball, checksum) = fake_release(&dir, version, &new);
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let target = bin.join("hushnote");
        fs::write(&target, b"\x7fELF the old app").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        let before = fs::metadata(&target).unwrap();

        let work = WorkDir::create(&dir.join("work")).unwrap();
        apply(&tarball, &checksum, version, work.path(), &target, &|| true).unwrap();

        assert_eq!(fs::read(&target).unwrap(), new);
        let after = fs::metadata(&target).unwrap();
        assert_eq!(after.permissions().mode() & 0o777, 0o755);
        assert_ne!(std::os::unix::fs::MetadataExt::ino(&before), std::os::unix::fs::MetadataExt::ino(&after), "a new file was renamed in");
        let left: Vec<_> = fs::read_dir(&bin).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("hushnote")], "no temp files next to the binary");
        let work_path = work.path().to_path_buf();
        drop(work);
        assert!(!work_path.exists(), "the work folder is removed");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_bad_checksum_leaves_the_binary_untouched() {
        let dir = scratch("bad");
        let version = v("0.9.0");
        let (tarball, _) = fake_release(&dir, version, b"\x7fELF the new app");
        let target = dir.join("hushnote");
        fs::write(&target, b"\x7fELF the old app").unwrap();
        let work = WorkDir::create(&dir.join("work")).unwrap();

        let wrong = format!("{}  {}\n", "0".repeat(64), asset_name(version));
        let err = apply(&tarball, &wrong, version, work.path(), &target, &|| true).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        let err = apply(&tarball, "garbage", version, work.path(), &target, &|| true).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert_eq!(fs::read(&target).unwrap(), b"\x7fELF the old app");
        assert_eq!(fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".hushnote")).count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_tarball_without_the_app_or_with_junk_is_refused() {
        let dir = scratch("junk");
        let version = v("0.9.0");
        let target = dir.join("hushnote");
        fs::write(&target, b"\x7fELF the old app").unwrap();
        // The binary is not an executable.
        let (tarball, checksum) = fake_release(&dir, version, b"#!/bin/sh\n");
        let work = WorkDir::create(&dir.join("work")).unwrap();
        assert!(apply(&tarball, &checksum, version, work.path(), &target, &|| true).is_err());
        // The tarball is for another version: its folder has another name.
        let (tarball, checksum) = fake_release(&dir, v("0.8.0"), b"\x7fELF x");
        let renamed = dir.join(asset_name(version));
        fs::rename(&tarball, &renamed).unwrap();
        let checksum = checksum.replace(&asset_name(v("0.8.0")), &asset_name(version));
        let work2 = WorkDir::create(&dir.join("work2")).unwrap();
        assert!(apply(&renamed, &checksum, version, work2.path(), &target, &|| true).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"\x7fELF the old app");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn work_that_starts_before_the_swap_stops_the_update() {
        let dir = scratch("busy");
        let version = v("0.9.0");
        let (tarball, checksum) = fake_release(&dir, version, b"\x7fELF the new app");
        let target = dir.join("hushnote");
        fs::write(&target, b"\x7fELF the old app").unwrap();
        let work = WorkDir::create(&dir.join("work")).unwrap();
        assert!(apply(&tarball, &checksum, version, work.path(), &target, &|| false).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"\x7fELF the old app");
        assert_eq!(fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".hushnote")).count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_work_folder_is_private_and_clears_old_ones() {
        let dir = scratch("work");
        let old = dir.join("update-1-1");
        fs::create_dir_all(old.join("out")).unwrap();
        let work = WorkDir::create(&dir).unwrap();
        assert!(!old.exists());
        assert_eq!(fs::metadata(work.path()).unwrap().permissions().mode() & 0o777, 0o700);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_an_installed_writable_binary_can_update_itself() {
        let dir = scratch("target");
        let dev = dir.join("target").join("release");
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("hushnote"), "x").unwrap();
        assert!(check_target(&dev.join("hushnote")).unwrap_err().contains("development"));
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("hushnote"), "x").unwrap();
        assert_eq!(check_target(&bin.join("hushnote")).unwrap(), bin.join("hushnote").canonicalize().unwrap());
        assert!(check_target(&bin.join("missing")).is_err());
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o555)).unwrap();
        let readonly = check_target(&bin.join("hushnote"));
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        // (root can write anywhere, so only check the refusal when the folder really is read-only)
        if unsafe { libc::geteuid() } != 0 {
            assert!(readonly.unwrap_err().contains("not writable"));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    /// Downloads the published release for real and installs it over a stand-in binary:
    /// `HUSHNOTE_TEST_DIR=<dir> HUSHNOTE_TEST_VERSION=0.2.1 cargo test published_release -- --ignored --nocapture`
    /// (point XDG_DATA_HOME at scratch space too). Leaves `<dir>/hushnote` for the caller to compare.
    #[test]
    #[ignore = "needs the network"]
    fn published_release_installs_over_an_older_binary() {
        let dir = PathBuf::from(std::env::var("HUSHNOTE_TEST_DIR").expect("HUSHNOTE_TEST_DIR"));
        let version = v(&std::env::var("HUSHNOTE_TEST_VERSION").unwrap_or_else(|_| "0.2.1".into()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("hushnote");
        fs::write(&target, b"\x7fELF the old app").unwrap();
        let latest = check_latest().unwrap();
        eprintln!("latest: {latest}");
        assert!(latest >= version);
        let mut last = (0, 0);
        install(version, &target, |have, total| last = (have, total), &|| true).unwrap();
        eprintln!("downloaded {} of {} bytes; installed {}", last.0, last.1, target.display());
        assert!(fs::metadata(&target).unwrap().len() > 1_000_000);
    }
}
