//! Updates. Each release on GitHub carries `latest.json`: its version and notes, and a download
//! for each system with a minisign signature. TuplePad checks it once a day (or from Help ▸ Check
//! for Updates…), shows what's new, and on request downloads the update, checks its signature
//! against the public key below, puts it in place of this copy and relaunches.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::app::App;
use crate::UpdateBridge;
use slint::ComponentHandle;

/// The newest release's manifest (GitHub redirects this to the latest release's file).
const FEED: &str = "https://github.com/Baz-Studios-LLC/TuplePad/releases/latest/download/latest.json";
/// Where people can download a release themselves.
pub const RELEASES: &str = "https://github.com/Baz-Studios-LLC/TuplePad/releases/latest";
/// The public half of TuplePad's update key. The secret half stays on the machine that makes
/// releases; `examples/update_sign.rs` signs with it.
const PUBLIC_KEY: &str = "RWRUkTt0Aol6ucGlqBuynj3AMPpwCkjiRJiasn3PcgjY5Hjz+Fiuhb3e";
/// How often TuplePad looks for a new version on its own.
const CHECK_EVERY: Duration = Duration::from_secs(20 * 60 * 60);

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub date: String,
    /// What's new, in Markdown.
    #[serde(default)]
    pub notes: String,
    /// The release's page.
    #[serde(default)]
    pub page: String,
    /// By system: "mac", "windows-x64", "windows-arm64", "linux-x64", "linux-arm64".
    pub downloads: BTreeMap<String, Download>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Download {
    pub url: String,
    pub size: u64,
    /// The minisign signature, whose trusted comment is "TuplePad <version> <system>".
    pub signature: String,
}

impl Manifest {
    pub fn download(&self) -> Option<&Download> {
        self.downloads.get(platform())
    }

    fn page(&self) -> String {
        if self.page.is_empty() { RELEASES.into() } else { self.page.clone() }
    }
}

/// This copy's version name, like "v001".
pub fn current_version() -> &'static str {
    crate::VERSION.trim()
}

/// A version name's number: "v012" is 12.
pub fn version_number(name: &str) -> Option<u32> {
    name.trim().strip_prefix('v')?.parse().ok()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((version_number(candidate), version_number(current)), (Some(a), Some(b)) if a > b)
}

/// This system's name in `latest.json`.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        "windows-arm64"
    } else if cfg!(windows) {
        "windows-x64"
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-x64"
    }
}

/// What a download's signature must say it is.
pub fn trusted_comment(version: &str, platform: &str) -> String {
    format!("TuplePad {version} {platform}")
}

/// Checks a downloaded file against its signature: made with TuplePad's key, for this version
/// and system.
pub fn verify(path: &Path, signature: &str, version: &str, public_key: &str) -> Result<(), String> {
    let refused = "The download didn't pass TuplePad's signature check, so it wasn't installed.";
    let key = minisign_verify::PublicKey::from_base64(public_key).map_err(|_| refused.to_string())?;
    let signature = minisign_verify::Signature::decode(signature).map_err(|_| refused.to_string())?;
    if signature.trusted_comment() != trusted_comment(version, platform()) {
        return Err(refused.into());
    }
    let mut verifier = key.verify_stream(&signature).map_err(|_| refused.to_string())?;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read]);
    }
    verifier.finalize().map_err(|_| refused.to_string())
}

/// What an update replaces.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// The app bundle, on a Mac.
    Bundle(PathBuf),
    /// The AppImage file, on Linux.
    AppImage(PathBuf),
    /// The folder TuplePad's installer put it in, on Windows: the new installer runs over it.
    Installed(PathBuf),
}

/// What an update would replace, or why this copy can't update itself.
pub fn target() -> Result<Target, String> {
    // Tests install into a copy of their own (debug builds only).
    if cfg!(debug_assertions)
        && let Some(path) = std::env::var_os("TUPLEPAD_UPDATE_TARGET")
    {
        let path = PathBuf::from(path);
        return Ok(if cfg!(target_os = "macos") {
            Target::Bundle(path)
        } else if cfg!(windows) {
            Target::Installed(path)
        } else {
            Target::AppImage(path)
        });
    }
    let exe = std::env::current_exe().and_then(dunce::canonicalize).map_err(|e| e.to_string())?;
    let development = "This copy of TuplePad was built from source, so it can't update itself. Download the new version instead.";
    let target = if cfg!(target_os = "macos") {
        let bundle = exe.ancestors().nth(3).filter(|p| p.extension().is_some_and(|e| e == "app")).ok_or(development)?;
        let shown = bundle.to_string_lossy();
        if shown.contains("/AppTranslocation/") {
            return Err("macOS is running this copy from a temporary place. Move TuplePad to your Applications folder, open it from there, and update again.".into());
        }
        if shown.starts_with("/Volumes/") && !can_write(bundle.parent().unwrap_or(bundle)) {
            return Err("This copy of TuplePad is running from a disk image. Drag it to your Applications folder, open it from there, and update again.".into());
        }
        Target::Bundle(bundle.to_path_buf())
    } else if cfg!(windows) {
        let folder = exe.parent().ok_or(development)?;
        // Inno Setup leaves its uninstaller beside what it installed.
        if !folder.join("unins000.exe").exists() {
            return Err(development.into());
        }
        Target::Installed(folder.to_path_buf())
    } else {
        let image = std::env::var_os("APPIMAGE").map(PathBuf::from).ok_or(
            "This copy of TuplePad came from somewhere that updates it (like your system's package manager), so it can't update itself.",
        )?;
        Target::AppImage(image)
    };
    let folder = match &target {
        Target::Bundle(path) | Target::AppImage(path) => path.parent().map(Path::to_path_buf),
        Target::Installed(folder) => Some(folder.clone()),
    };
    match folder {
        Some(folder) if can_write(&folder) => Ok(target),
        Some(folder) => Err(format!("TuplePad can't replace itself in {}. Download the new version and install it yourself.", folder.display())),
        None => Err(development.into()),
    }
}

fn can_write(folder: &Path) -> bool {
    let probe = folder.join(format!(".tuplepad-write-check-{}", std::process::id()));
    let written = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    written
}

/// Where an update is: nothing yet, looking, found, downloading, …
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading { received: u64, total: u64 },
    Installing,
    /// In place; TuplePad relaunches once it has quit.
    Installed,
    Failed(String),
}

impl Phase {
    pub fn busy(&self) -> bool {
        matches!(self, Phase::Checking | Phase::Downloading { .. } | Phase::Installing)
    }
}

/// What the worker thread and the app share.
#[derive(Default)]
struct Shared {
    phase: Phase,
    manifest: Option<Manifest>,
    /// What to start once TuplePad has quit, after an install.
    relaunch: Option<Relaunch>,
    /// Bumped on every change, so the app knows to redraw.
    generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Relaunch {
    /// Open the new app (a Mac bundle or an AppImage).
    Open(PathBuf),
    /// Run the new installer quietly; it opens TuplePad when it's done.
    Installer(PathBuf),
}

#[derive(Default)]
pub struct UpdateState {
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
    seen: u64,
    /// When to see whether a daily check is due.
    next_look: Option<Instant>,
    /// The check was asked for, so the sheet shows its result even when there's nothing new.
    manual: bool,
    pub sheet_open: bool,
    /// Scripted runs don't relaunch; they note what would have been opened.
    pub relaunched: Option<Relaunch>,
}

impl UpdateState {
    fn set(shared: &Arc<Mutex<Shared>>, change: impl FnOnce(&mut Shared)) {
        if let Ok(mut shared) = shared.lock() {
            change(&mut shared);
            shared.generation += 1;
        }
    }

    fn phase(&self) -> Phase {
        self.shared.lock().map(|s| s.phase.clone()).unwrap_or_default()
    }

    fn manifest(&self) -> Option<Manifest> {
        self.shared.lock().ok().and_then(|s| s.manifest.clone())
    }
}

fn feed() -> String {
    std::env::var("TUPLEPAD_UPDATE_URL").unwrap_or_else(|_| FEED.into())
}

fn public_key() -> String {
    match std::env::var("TUPLEPAD_UPDATE_KEY") {
        Ok(key) if cfg!(debug_assertions) => key,
        _ => PUBLIC_KEY.into(),
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(120)))
        .https_only(!cfg!(debug_assertions))
        .user_agent(format!("TuplePad/{}", current_version()))
        .build()
        .into()
}

fn offline(error: ureq::Error) -> String {
    eprintln!("update: {error}");
    "TuplePad couldn't reach GitHub. Check your internet connection and try again.".into()
}

/// The newest release's manifest, or None when nothing has been released yet.
fn fetch_manifest() -> Result<Option<Manifest>, String> {
    let mut response = match agent().get(&feed()).call() {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(error) => return Err(offline(error)),
    };
    let text = response.body_mut().with_config().limit(1 << 20).read_to_string().map_err(offline)?;
    serde_json::from_str(&text).map(Some).map_err(|e| {
        eprintln!("update: latest.json: {e}");
        "The update information on GitHub couldn't be read. Try again later.".to_string()
    })
}

fn fetch(download: &Download, to: &Path, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<(), String> {
    let mut response = agent().get(&download.url).call().map_err(offline)?;
    let mut reader = response.body_mut().with_config().limit(download.size + 1).reader();
    let mut file = std::fs::File::create(to).map_err(|e| e.to_string())?;
    let mut buffer = vec![0; 1 << 16];
    let mut received = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(String::new());
        }
        let read = reader.read(&mut buffer).map_err(|e| {
            eprintln!("update: {e}");
            "The download stopped partway. Check your internet connection and try again.".to_string()
        })?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        received += read as u64;
        progress(received);
    }
    if received != download.size {
        return Err("The download stopped partway. Check your internet connection and try again.".into());
    }
    Ok(())
}

/// Puts a verified download in place of this copy, and says what to start afterwards.
fn install(file: &Path, target: &Target, version: &str) -> Result<Relaunch, String> {
    match target {
        Target::Bundle(bundle) => install_bundle(file, bundle, version).map(|_| Relaunch::Open(bundle.clone())),
        Target::AppImage(image) => {
            let staged = image.with_file_name(format!(".TuplePad-update-{}", std::process::id()));
            std::fs::copy(file, &staged).map_err(|e| couldnt_install(&e))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(|e| couldnt_install(&e))?;
            }
            // Renaming over the running AppImage is fine: TuplePad keeps the old file open.
            std::fs::rename(&staged, image).map_err(|e| {
                let _ = std::fs::remove_file(&staged);
                couldnt_install(&e)
            })?;
            Ok(Relaunch::Open(image.clone()))
        }
        Target::Installed(_) => Ok(Relaunch::Installer(file.to_path_buf())),
    }
}

fn couldnt_install(error: &dyn std::fmt::Display) -> String {
    eprintln!("update: {error}");
    format!("TuplePad couldn't put the new version in place ({error}). Download it and install it yourself.")
}

/// Unpacks the new app beside the old one, checks it's TuplePad at the right version, and swaps
/// them (putting the old one back if the swap fails).
fn install_bundle(zip: &Path, bundle: &Path, version: &str) -> Result<(), String> {
    let folder = bundle.parent().ok_or("no folder")?;
    let staging = folder.join(format!(".TuplePad-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let result = (|| {
        let unpacked = std::process::Command::new("/usr/bin/ditto").arg("-x").arg("-k").arg(zip).arg(&staging).status().map_err(|e| couldnt_install(&e))?;
        if !unpacked.success() {
            return Err(couldnt_install(&"the download couldn't be unpacked"));
        }
        let new_app = std::fs::read_dir(&staging)
            .map_err(|e| couldnt_install(&e))?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| couldnt_install(&"there was no app in the download"))?;
        let plist = new_app.join("Contents/Info.plist");
        if plist_value(&plist, "CFBundleIdentifier").as_deref() != Some("com.bazstudios.TuplePad")
            || plist_value(&plist, "CFBundleShortVersionString").as_deref() != Some(version)
        {
            return Err(couldnt_install(&"the download isn't the TuplePad it should be"));
        }
        let old = folder.join(format!(".TuplePad-old-{}.app", std::process::id()));
        std::fs::rename(bundle, &old).map_err(|e| couldnt_install(&e))?;
        if let Err(error) = std::fs::rename(&new_app, bundle) {
            let _ = std::fs::rename(&old, bundle);
            return Err(couldnt_install(&error));
        }
        // The running copy keeps working from memory; its files can go.
        let _ = std::fs::remove_dir_all(&old);
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn plist_value(plist: &Path, key: &str) -> Option<String> {
    let output = std::process::Command::new("/usr/bin/plutil").args(["-extract", key, "raw", "-o", "-"]).arg(plist).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Starts what opens the new version once this process has ended.
fn relaunch_after_exit(relaunch: &Relaunch) -> std::io::Result<()> {
    let pid = std::process::id().to_string();
    match relaunch {
        #[cfg(windows)]
        Relaunch::Installer(setup) => {
            let quoted = setup.to_string_lossy().replace('\'', "''");
            let script = format!(
                "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Start-Process -FilePath '{quoted}' -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/relaunch=1'"
            );
            std::process::Command::new("powershell").args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script]).spawn()?;
        }
        #[cfg(not(windows))]
        Relaunch::Installer(_) => {}
        Relaunch::Open(path) => {
            // `open` starts apps without this process's environment, so a data folder moved
            // with TUPLEPAD_DATA_DIR is handed on.
            let open = if cfg!(target_os = "macos") { "exec /usr/bin/open ${3:+--env \"TUPLEPAD_DATA_DIR=$3\"} \"$2\"" } else { "exec \"$2\"" };
            let script = format!("while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; {open}");
            let data = std::env::var_os("TUPLEPAD_DATA_DIR").unwrap_or_default();
            std::process::Command::new("/bin/sh")
                .args(["-c", &script, "sh", &pid])
                .arg(path)
                .arg(data)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
        }
    }
    Ok(())
}

/// Release notes for the sheet: Markdown's markers turned into plain text.
pub fn plain_notes(notes: &str) -> String {
    notes
        .lines()
        .map(|line| {
            let line = line.trim_end();
            let bare = line.trim_start_matches('#').trim_start();
            if line.starts_with('#') {
                bare.to_string()
            } else if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
                format!("•  {item}")
            } else {
                line.to_string()
            }
        })
        .map(|line| line.replace("**", "").replace('`', ""))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl App {
    /// Starts the daily checks (not in scripted runs, which check only when told to).
    pub fn start_update_checks(&self) {
        if !self.is_scripted() {
            self.update.borrow_mut().next_look = Some(Instant::now() + Duration::from_secs(8));
        }
        // Say so once after an update.
        let current = current_version().to_string();
        let previous = std::mem::replace(&mut self.settings.borrow_mut().last_version, current.clone());
        if !previous.is_empty() && is_newer(&current, &previous) {
            self.show_toast(&format!("TuplePad was updated to {current}"));
        }
    }

    /// From the 60 ms tick: a daily check when it's due, and whatever the worker changed.
    pub fn update_tick(&self) {
        let due = {
            let mut state = self.update.borrow_mut();
            match state.next_look {
                Some(when) if Instant::now() >= when => {
                    state.next_look = Some(Instant::now() + Duration::from_secs(60 * 60));
                    let settings = self.settings.borrow();
                    settings.check_updates && unix_now() - settings.last_update_check >= CHECK_EVERY.as_secs() as i64
                }
                _ => false,
            }
        };
        if due {
            self.check_for_updates(false);
        }
        let changed = {
            let mut state = self.update.borrow_mut();
            let generation = state.shared.lock().map(|s| s.generation).unwrap_or(0);
            std::mem::replace(&mut state.seen, generation) != generation
        };
        if changed {
            self.update_changed();
        }
    }

    /// Help ▸ Check for Updates… (manual) or the daily check.
    pub fn check_for_updates(&self, manual: bool) {
        let mut state = self.update.borrow_mut();
        if manual {
            state.manual = true;
            state.sheet_open = true;
        }
        // A download under way, or an update already found, just shows.
        match state.phase() {
            Phase::Downloading { .. } | Phase::Installing | Phase::Installed | Phase::Checking => {
                drop(state);
                self.refresh_update();
                return;
            }
            _ => {}
        }
        self.settings.borrow_mut().last_update_check = unix_now();
        let shared = state.shared.clone();
        UpdateState::set(&shared, |s| s.phase = Phase::Checking);
        drop(state);
        self.refresh_update();
        std::thread::spawn(move || {
            let result = fetch_manifest();
            UpdateState::set(&shared, |s| match result {
                Ok(Some(manifest)) if is_newer(&manifest.version, current_version()) && manifest.download().is_some() => {
                    s.manifest = Some(manifest);
                    s.phase = Phase::Available;
                }
                Ok(_) => s.phase = Phase::UpToDate,
                Err(message) => s.phase = Phase::Failed(message),
            });
        });
    }

    /// The worker moved on: tell the person, and relaunch once installed.
    fn update_changed(&self) {
        let (phase, manifest, manual) = {
            let state = self.update.borrow();
            (state.phase(), state.manifest(), state.manual)
        };
        match &phase {
            // Debug builds can install straight away, to test relaunching.
            Phase::Available if !manual && cfg!(debug_assertions) && std::env::var_os("TUPLEPAD_UPDATE_INSTALL").is_some() => self.install_update(),
            Phase::Available if !manual => {
                if let Some(manifest) = &manifest
                    && self.settings.borrow().skipped_update != manifest.version
                {
                    self.show_toast(&format!("TuplePad {} is available", manifest.version));
                }
            }
            // Daily checks that find nothing, or fail, stay quiet.
            Phase::UpToDate | Phase::Failed(_) if !manual => {
                if let Phase::Failed(message) = &phase {
                    eprintln!("update check: {message}");
                }
                UpdateState::set(&self.update.borrow().shared, |s| s.phase = Phase::Idle);
            }
            Phase::Installed => {
                let relaunch = self.update.borrow().shared.lock().ok().and_then(|s| s.relaunch.clone());
                if let Some(relaunch) = relaunch {
                    self.relaunch(relaunch);
                }
            }
            _ => {}
        }
        self.refresh_update();
    }

    /// Quits so the new version can start (scripted runs only note it).
    fn relaunch(&self, relaunch: Relaunch) {
        if self.is_scripted() {
            self.update.borrow_mut().relaunched = Some(relaunch);
            return;
        }
        if let Err(error) = relaunch_after_exit(&relaunch) {
            UpdateState::set(&self.update.borrow().shared, |s| s.phase = Phase::Failed(format!("TuplePad couldn't relaunch ({error}). Quit and open it again.")));
            return;
        }
        self.hide_settings();
        self.shut_down();
        let _ = slint::quit_event_loop();
    }

    /// Install and Relaunch: download, check, put in place, relaunch.
    pub fn install_update(&self) {
        let state = self.update.borrow();
        let Some(manifest) = state.manifest() else { return };
        if state.phase().busy() {
            return;
        }
        let target = match target() {
            Ok(target) => target,
            Err(_) => {
                drop(state);
                self.open_release_page();
                return;
            }
        };
        let Some(download) = manifest.download().cloned() else { return };
        state.cancel.store(false, Ordering::Relaxed);
        let (shared, cancel) = (state.shared.clone(), state.cancel.clone());
        UpdateState::set(&shared, |s| s.phase = Phase::Downloading { received: 0, total: download.size });
        drop(state);
        std::thread::spawn(move || {
            let folder = std::env::temp_dir().join(format!("TuplePad-update-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&folder);
            let name = download.url.rsplit('/').next().filter(|n| !n.is_empty() && !n.contains("..")).unwrap_or("TuplePad-update");
            let file = folder.join(name);
            let total = download.size;
            let last = std::cell::Cell::new(0);
            let result = fetch(&download, &file, &cancel, &|received| {
                // Redraw about every percent.
                if received - last.get() >= total / 100 || received == total {
                    last.set(received);
                    UpdateState::set(&shared, |s| s.phase = Phase::Downloading { received, total });
                }
            })
            .and_then(|_| {
                UpdateState::set(&shared, |s| s.phase = Phase::Installing);
                verify(&file, &download.signature, &manifest.version, &public_key())
            })
            .and_then(|_| install(&file, &target, &manifest.version));
            // An installer runs from the download, so that stays until it has.
            if !matches!(result, Ok(Relaunch::Installer(_))) {
                let _ = std::fs::remove_dir_all(&folder);
            }
            UpdateState::set(&shared, |s| match result {
                Ok(relaunch) => {
                    s.relaunch = Some(relaunch);
                    s.phase = Phase::Installed;
                }
                // Cancelled.
                Err(message) if message.is_empty() => s.phase = Phase::Available,
                Err(message) => s.phase = Phase::Failed(message),
            });
        });
    }

    pub fn cancel_update(&self) {
        let state = self.update.borrow();
        if matches!(state.phase(), Phase::Downloading { .. }) {
            state.cancel.store(true, Ordering::Relaxed);
            return;
        }
        drop(state);
        self.close_update_sheet();
    }

    pub fn close_update_sheet(&self) {
        {
            let mut state = self.update.borrow_mut();
            state.sheet_open = false;
            state.manual = false;
            let phase = state.phase();
            if matches!(phase, Phase::UpToDate | Phase::Failed(_) | Phase::Checking) {
                // A failed install still has its update to offer.
                let back = if state.manifest().is_some() && !matches!(phase, Phase::Checking) { Phase::Available } else { Phase::Idle };
                UpdateState::set(&state.shared, |s| s.phase = back);
            }
        }
        self.refresh_update();
    }

    pub fn skip_update(&self) {
        if let Some(manifest) = self.update.borrow().manifest() {
            self.settings.borrow_mut().skipped_update = manifest.version;
        }
        self.close_update_sheet();
    }

    pub fn show_update_sheet(&self) {
        self.update.borrow_mut().sheet_open = true;
        self.refresh_update();
    }

    pub fn open_release_page(&self) {
        let page = self.update.borrow().manifest().map(|m| m.page()).unwrap_or_else(|| RELEASES.into());
        if self.is_scripted() {
            *self.revealed.borrow_mut() = Some(PathBuf::from(page));
            return;
        }
        crate::platform::open_url(&page);
    }

    /// Brings the sheet, the status bar's pill and Settings up to date.
    pub fn refresh_update(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let bridge = ui.global::<UpdateBridge>();
        let state = self.update.borrow();
        let phase = state.phase();
        let manifest = state.manifest();
        let current = current_version();
        let skipped = manifest.as_ref().is_some_and(|m| m.version == self.settings.borrow().skipped_update);
        let installable = target();
        let version = manifest.as_ref().map(|m| m.version.clone()).unwrap_or_default();
        let size = manifest.as_ref().and_then(|m| m.download()).map(|d| megabytes(d.size)).unwrap_or_default();
        let (kind, title, detail) = match &phase {
            Phase::Idle | Phase::Checking => (0, "Checking for Updates".to_string(), "Looking for a newer TuplePad…".to_string()),
            Phase::UpToDate => (1, "TuplePad Is Up to Date".into(), format!("{current} is the newest version.")),
            Phase::Available => (
                2,
                format!("TuplePad {version} Is Available"),
                match &installable {
                    Ok(_) => format!("You have {current}. The download is {size}, and your documents, unsaved changes too, open again after the relaunch."),
                    Err(reason) => format!("You have {current}. {reason}"),
                },
            ),
            Phase::Downloading { received, total } => (3, "Updating TuplePad".into(), format!("Downloading {} of {}", megabytes(*received), megabytes(*total))),
            Phase::Installing | Phase::Installed => (4, "Updating TuplePad".into(), "Installing…".into()),
            Phase::Failed(message) => (5, "Couldn't Update TuplePad".into(), message.clone()),
        };
        bridge.set_open(state.sheet_open);
        bridge.set_kind(kind);
        bridge.set_title(title.into());
        bridge.set_detail(detail.into());
        bridge.set_notes(manifest.as_ref().map(|m| plain_notes(&m.notes)).unwrap_or_default().into());
        bridge.set_installable(installable.is_ok());
        bridge.set_progress(match phase {
            Phase::Downloading { received, total } if total > 0 => received as f32 / total as f32,
            Phase::Installing | Phase::Installed => 1.0,
            _ => 0.0,
        });
        // The status bar's pill: an update to install, or one under way.
        let pill = match &phase {
            Phase::Available if !skipped || state.manual => format!("Update to {version}"),
            Phase::Downloading { received, total } if *total > 0 => format!("Updating · {}%", received * 100 / total),
            Phase::Installing | Phase::Installed => "Updating…".into(),
            _ => String::new(),
        };
        bridge.set_pill(pill.into());
        drop(state);
        self.refresh_settings_updates();
    }

    /// The Updates row in Settings ▸ General.
    pub fn refresh_settings_updates(&self) {
        let Some(window) = self.settings_window.borrow().as_ref().map(|w| w.clone_strong()) else { return };
        let state = self.update.borrow();
        let status = match state.phase() {
            Phase::Checking => "Checking…".to_string(),
            Phase::Available | Phase::Downloading { .. } | Phase::Installing | Phase::Installed => {
                format!("{} is available", state.manifest().map(|m| m.version).unwrap_or_default())
            }
            _ => match self.settings.borrow().last_update_check {
                0 => "Not checked yet".into(),
                when => match chrono::DateTime::from_timestamp(when, 0) {
                    Some(time) => {
                        let local = time.with_timezone(&chrono::Local);
                        let today = chrono::Local::now().date_naive() == local.date_naive();
                        let day = if today { "today".to_string() } else { local.format("%B %-d").to_string() };
                        format!("Up to date · checked {day} at {}", local.format("%-I:%M %p"))
                    }
                    None => String::new(),
                },
            },
        };
        window.set_update_status(status.into());
    }

    pub fn update_dump(&self) -> serde_json::Value {
        let state = self.update.borrow();
        let phase = state.phase();
        let (name, error) = match &phase {
            Phase::Idle => ("idle", None),
            Phase::Checking => ("checking", None),
            Phase::UpToDate => ("upToDate", None),
            Phase::Available => ("available", None),
            Phase::Downloading { .. } => ("downloading", None),
            Phase::Installing => ("installing", None),
            Phase::Installed => ("installed", None),
            Phase::Failed(message) => ("failed", Some(message.clone())),
        };
        let ui = self.ui.upgrade();
        serde_json::json!({
            "phase": name,
            "error": error,
            "version": state.manifest().map(|m| m.version),
            "sheet": state.sheet_open,
            "pill": ui.as_ref().map(|ui| ui.global::<UpdateBridge>().get_pill().to_string()),
            "title": ui.as_ref().map(|ui| ui.global::<UpdateBridge>().get_title().to_string()),
            "detail": ui.as_ref().map(|ui| ui.global::<UpdateBridge>().get_detail().to_string()),
            "installable": ui.as_ref().map(|ui| ui.global::<UpdateBridge>().get_installable()),
            "skipped": self.settings.borrow().skipped_update,
            "relaunch": match &state.relaunched {
                Some(Relaunch::Open(path)) | Some(Relaunch::Installer(path)) => Some(path.to_string_lossy().to_string()),
                None => None,
            },
        })
    }

    pub fn update_busy(&self) -> bool {
        self.update.borrow().phase().busy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(version_number("v001"), Some(1));
        assert_eq!(version_number(" v012\n"), Some(12));
        assert_eq!(version_number("1.0"), None);
        assert!(is_newer("v002", "v001"));
        assert!(!is_newer("v001", "v001"));
        assert!(!is_newer("v001", "v010"));
        assert!(!is_newer("nonsense", "v001"));
    }

    #[test]
    fn manifest_and_notes() {
        let manifest: Manifest = serde_json::from_str(
            r###"{"version": "v002", "notes": "## What's new\n- **Faster** search\n* `tuplepad` command", "downloads": {"mac": {"url": "https://x/a.zip", "size": 10, "signature": "s"}, "linux-x64": {"url": "https://x/b", "size": 11, "signature": "t"}, "windows-x64": {"url": "https://x/c.exe", "size": 12, "signature": "u"}}}"###,
        )
        .unwrap();
        assert!(manifest.download().is_some());
        assert_eq!(manifest.page(), RELEASES);
        assert_eq!(plain_notes(&manifest.notes), "What's new\n•  Faster search\n•  tuplepad command");
    }

    #[test]
    fn signatures() {
        let folder = std::env::temp_dir().join(format!("tuplepad-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("download.zip");
        std::fs::write(&file, b"the new TuplePad").unwrap();
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let key = pair.pk.to_base64();
        let sign = |comment: &str| {
            minisign::sign(Some(&pair.pk), &pair.sk, std::fs::File::open(&file).unwrap(), Some(comment), None).unwrap().into_string()
        };
        let good = sign(&trusted_comment("v002", platform()));
        assert_eq!(verify(&file, &good, "v002", &key), Ok(()));
        // Another version's or another system's signature isn't enough.
        assert!(verify(&file, &good, "v003", &key).is_err());
        assert!(verify(&file, &sign(&trusted_comment("v002", "plan9")), "v002", &key).is_err());
        // Nor is another key's, or a changed file.
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap().pk.to_base64();
        assert!(verify(&file, &good, "v002", &other).is_err());
        std::fs::write(&file, b"the new TuplePad, changed").unwrap();
        assert!(verify(&file, &good, "v002", &key).is_err());
        assert!(verify(&file, "not a signature", "v002", &key).is_err());
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn the_built_in_key_reads() {
        assert!(minisign_verify::PublicKey::from_base64(PUBLIC_KEY).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn appimage_swap() {
        let folder = std::env::temp_dir().join(format!("tuplepad-appimage-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let (image, download) = (folder.join("TuplePad.AppImage"), folder.join("new"));
        std::fs::write(&image, b"old").unwrap();
        std::fs::write(&download, b"new").unwrap();
        assert_eq!(install(&download, &Target::AppImage(image.clone()), "v002"), Ok(Relaunch::Open(image.clone())));
        assert_eq!(std::fs::read(&image).unwrap(), b"new");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&image).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
