// Updates from GitHub, with no installer: the app is a single exe, so it can replace itself.
//
// Checking: about once a day (while "Check for updates" is on) the latest release is read from the GitHub API. When
// its version is newer than this build, the settings panel shows it with an "Update" button.
//
// Updating: the release's `lyrics-overlay.exe` is downloaded next to the running exe and checked against the SHA-256
// GitHub lists for it. Windows won't let a running exe be overwritten, but it can be renamed: the running one becomes
// `lyrics-overlay.old.exe`, the download takes its name, the new copy is started and this one quits. The next start
// deletes the old file.

use crate::core::{core, notify, Ev};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const REPO: &str = "AhPlace2/lyrics-overlay";
/// The release file the updater installs. Each release must attach the native build under exactly this name.
const ASSET: &str = "lyrics-overlay.exe";
const USER_AGENT: &str = concat!("lyrics-overlay/", env!("CARGO_PKG_VERSION"));
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_SIZE: u64 = 100 * 1024 * 1024;
/// Passed to the new copy, so it waits for this one to quit instead of handing over to it.
pub const UPDATED_ARG: &str = "--updated";

#[derive(Clone)]
struct Release {
    version: String,
    url: String,
    sha256: String,
    page: String,
}

#[derive(Clone)]
enum Status {
    Idle,
    Available(Release),
    Downloading(Release),
    Failed(Release, String),
}

static STATUS: Mutex<Status> = Mutex::new(Status::Idle);
static LAST_CHECK: Mutex<Option<Instant>> = Mutex::new(None);

fn status() -> Status {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_status(s: Status) {
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = s;
    notify();
}

/// What the settings panel shows: null when there is nothing to say.
pub fn panel_value() -> Value {
    match status() {
        Status::Idle => Value::Null,
        Status::Available(r) => json!({ "version": r.version, "state": "available" }),
        Status::Downloading(r) => json!({ "version": r.version, "state": "downloading" }),
        Status::Failed(r, e) => json!({ "version": r.version, "state": "failed", "error": e, "page": r.page }),
    }
}

/// Starts the background checker, and tidies up after an update.
pub fn start() {
    if let Some(old) = old_path() {
        // Right after an update the old copy may still be closing; try for a few seconds.
        std::thread::spawn(move || {
            for _ in 0..20 {
                if !old.exists() || std::fs::remove_file(&old).is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }
    std::thread::Builder::new()
        .name("updates".into())
        .spawn(|| {
            // Not at the very start, so it doesn't compete with the first song and lyrics lookups.
            std::thread::sleep(Duration::from_secs(if test_api().is_some() { 1 } else { 15 }));
            loop {
                let due = LAST_CHECK.lock().unwrap_or_else(|e| e.into_inner()).is_none_or(|t| t.elapsed() >= EVERY);
                if due && core().flag("checkUpdates") {
                    *LAST_CHECK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                    check();
                }
                std::thread::sleep(Duration::from_secs(60 * 10));
            }
        })
        .unwrap();
}

/// Turned on in the settings: check now rather than within the next day.
pub fn check_soon() {
    *LAST_CHECK.lock().unwrap_or_else(|e| e.into_inner()) = None;
    std::thread::spawn(|| {
        *LAST_CHECK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        check();
    });
}

fn check() {
    if matches!(status(), Status::Downloading(_)) {
        return;
    }
    let Ok(r) = latest() else { return };
    let Some(r) = r else { return };
    if !newer(&r.version, env!("CARGO_PKG_VERSION")) {
        return;
    }
    if test_api().is_some() && std::env::var("LO_UPDATE_AUTO").is_ok() && !std::env::args().any(|a| a == UPDATED_ARG) {
        set_status(Status::Available(r));
        install();
        return;
    }
    let was_known = matches!(status(), Status::Available(ref k) if k.version == r.version);
    set_status(Status::Available(r.clone()));
    if !was_known {
        let mut c = core();
        c.events.push(Ev::Toast(format!("Version {} is out. Update in Settings", r.version)));
        drop(c);
        notify();
    }
}

/// The newest release that carries the exe. None when there is no release or it has no exe for us.
fn latest() -> Result<Option<Release>, String> {
    let api = test_api().unwrap_or_else(|| format!("https://api.github.com/repos/{REPO}/releases/latest"));
    let mut res = crate::lyrics::HTTP
        .get(api)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    if res.status() != 200 {
        return Err(format!("GitHub answered {}", res.status()));
    }
    let v: Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
    let version = v["tag_name"].as_str().unwrap_or("").trim_start_matches(['v', 'V']).to_string();
    let page = v["html_url"].as_str().unwrap_or("").to_string();
    let Some(asset) = v["assets"].as_array().and_then(|a| a.iter().find(|a| a["name"].as_str() == Some(ASSET))) else { return Ok(None) };
    let url = asset["browser_download_url"].as_str().unwrap_or("").to_string();
    // GitHub works out each file's SHA-256 itself; without it there is no way to check the download, so no update.
    let sha256 = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).unwrap_or("").to_lowercase();
    if version.is_empty() || !(url.starts_with("https://github.com/") || test_api().is_some()) || sha256.len() != 64 {
        return Ok(None);
    }
    Ok(Some(Release { version, url, sha256, page }))
}

/// "0.2.0" > "0.1.7". Anything after a '-' (like "-beta") counts as older than the plain version.
fn newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> (Vec<u64>, bool) {
        let (main, pre) = s.split_once('-').map(|(m, _)| (m, true)).unwrap_or((s, false));
        (main.split('.').map(|p| p.parse().unwrap_or(0)).collect(), pre)
    };
    let ((mut x, xp), (mut y, yp)) = (parse(a), parse(b));
    let n = x.len().max(y.len());
    x.resize(n, 0);
    y.resize(n, 0);
    x > y || (x == y && yp && !xp)
}

/// Debug builds only: LO_UPDATE_API points the check at a local fake release, for testing the swap.
fn test_api() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var("LO_UPDATE_API").ok()
    } else {
        None
    }
}

fn exe_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

fn old_path() -> Option<PathBuf> {
    exe_path().map(|p| p.with_file_name("lyrics-overlay.old.exe"))
}

/// The "Update" button: download, check, swap, restart. Runs on its own thread.
pub fn install() {
    let r = match status() {
        Status::Available(r) | Status::Failed(r, _) => r,
        _ => return,
    };
    set_status(Status::Downloading(r.clone()));
    std::thread::spawn(move || {
        if let Err(e) = swap_in(&r) {
            set_status(Status::Failed(r, e));
        }
    });
}

fn swap_in(r: &Release) -> Result<(), String> {
    let exe = exe_path().ok_or("Can't find where the app is")?;
    let new = exe.with_file_name("lyrics-overlay.new.exe");
    let old = old_path().ok_or("Can't find where the app is")?;

    let bytes = download(&r.url)?;
    let got = format!("{:x}", Sha256::digest(&bytes));
    if got != r.sha256 {
        return Err("The download doesn't match the release. Nothing was changed".into());
    }
    std::fs::write(&new, &bytes).map_err(|e| not_writable(&exe, e))?;

    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(&exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(not_writable(&exe, e));
    }
    if let Err(e) = std::fs::rename(&new, &exe) {
        // Put the running copy back so the app still starts next time.
        let _ = std::fs::rename(&old, &exe);
        let _ = std::fs::remove_file(&new);
        return Err(not_writable(&exe, e));
    }
    if let Err(e) = std::process::Command::new(&exe).arg(UPDATED_ARG).spawn() {
        // The new exe is in place; it just didn't start. Keep running this one, the next start uses the new one.
        return Err(format!("Updated, but couldn't restart ({e}). It'll be the new version next time you start it"));
    }
    let mut c = core();
    c.events.push(Ev::Quit);
    drop(c);
    notify();
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let agent = crate::lyrics::agent(120);
    let mut res = agent.get(url).header("User-Agent", USER_AGENT).call().map_err(|e| format!("Download failed: {e}"))?;
    if res.status() != 200 {
        return Err(format!("Download failed: GitHub answered {}", res.status()));
    }
    let mut bytes = Vec::new();
    res.body_mut()
        .as_reader()
        .take(MAX_SIZE)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Download failed: {e}"))?;
    Ok(bytes)
}

fn not_writable(exe: &Path, e: std::io::Error) -> String {
    let dir = exe.parent().map(|d| d.display().to_string()).unwrap_or_default();
    format!("Can't write to {dir} ({e}). Download the new version from GitHub instead")
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn versions() {
        assert!(newer("0.2.0", "0.1.7"));
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("0.1.7", "0.1.7"));
        assert!(!newer("0.1.6", "0.1.7"));
        assert!(newer("0.2.0", "0.2.0-beta"));
        assert!(!newer("0.2.0-beta", "0.2.0"));
    }
}
