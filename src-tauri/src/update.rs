//! The update agent (PLAN-B7 §1–§5 as amended through r6): one `UpdateState` for the whole app, checks at start, every
//! 12 hours and on the Settings button, one download at a time, the package kept in memory only (r2.1: nothing staged on
//! disk, nothing installed at start). An install happens on the user's click, or on the user's own Quit (tray item, macOS
//! menu item; r2.1, r6 Q1): the engine is stopped and its exit waited for (never a kill), then the app's bundle lock is
//! taken exclusively, then B7's own replacement step runs (install.rs). The click relaunches after it; the Quit closes
//! the app. Quitting wins at any point before the replacement (r1.2, r5.2).
//!
//! The plugin is used for `check` and `download` only; `download` verifies the signature (r4.6). Its `install()` is never
//! called (r3.1, Q9).
pub mod install;

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Why an update didn't get installed (U5a–U5d).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Download,
    Verify,
    Space,
    Install,
}

/// What the banner and the Settings row show (§5.3). Never a version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum State {
    Idle,
    Checking,
    CheckFailed,
    Downloading { percent: Option<u8>, critical: bool },
    Ready { critical: bool },
    Failed { kind: Kind },
    DebNotice,
    /// This app can't replace itself here (r4.4, r5.3): the download page instead (U14).
    Manual,
    /// The engine is finishing before the install (U7; `long`: U8).
    Stopping { long: bool },
    Installing,
}

/// A state with its sequence number: the page renders the newest one it has seen (r1.12).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub seq: u64,
    #[serde(flatten)]
    pub state: State,
}

/// Why an install runs (r2.1): the click relaunches into the new version; the user's own Quit closes the app after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Click,
    Quit,
}

/// What the user's own Quit does (r2.1, r3.7).
pub enum UserQuit {
    /// An update is ready: install it, then close.
    Install(Package),
    /// The replacement step has begun: it closes the app itself once it is done.
    AfterReplacement,
    /// B6's quit.
    Quit,
}

/// A downloaded, verified package. Memory only (r2.1).
#[derive(Clone)]
pub struct Package {
    pub version: String,
    pub critical: bool,
    pub bytes: Arc<Vec<u8>>,
}

// The install's phase (r1.2, r5.2): one atomic, moved by compare-exchange, so a quit and an install never both win.
const OPEN: u8 = 0;
const STOPPING: u8 = 1;
const INSTALLING: u8 = 2;
const QUITTING: u8 = 3;

struct Inner {
    seq: u64,
    state: State,
    /// A check or a download is under way (one at a time).
    busy: bool,
    package: Option<Package>,
}

pub struct Agent {
    inner: Mutex<Inner>,
    phase: AtomicU8,
    /// A quit came in, whatever the phase was: even one that came too late to stop the replacement step means no
    /// respawn after it.
    quit: AtomicBool,
    /// The replacement step and what follows it (the failure mark, the exit) are under way: an app exit waits for them.
    replacing: AtomicBool,
    emit: Box<dyn Fn(&Snapshot) + Send + Sync>,
}

/// What a check that found an update leads to.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    /// Nothing to download (this version is already in memory, or a .deb).
    Done,
    /// Download it; `visible`: the state follows the download (false while another package is ready: it stays ready).
    Download { visible: bool },
}

impl Agent {
    pub fn new(emit: impl Fn(&Snapshot) + Send + Sync + 'static) -> Self {
        Agent {
            inner: Mutex::new(Inner { seq: 0, state: State::Idle, busy: false, package: None }),
            phase: AtomicU8::new(OPEN),
            quit: AtomicBool::new(false),
            replacing: AtomicBool::new(false),
            emit: Box::new(emit),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> { self.inner.lock().unwrap_or_else(|e| e.into_inner()) }

    fn set(&self, inner: &mut Inner, state: State) {
        if inner.state == state {
            return;
        }
        inner.seq += 1;
        inner.state = state;
        (self.emit)(&Snapshot { seq: inner.seq, state: inner.state.clone() });
    }

    pub fn snapshot(&self) -> Snapshot {
        let inner = self.lock();
        Snapshot { seq: inner.seq, state: inner.state.clone() }
    }

    /// No install or quit under way. B6-3b's notice poll looks at the engine only then (PLAN-B7 r7).
    pub fn open(&self) -> bool { self.phase.load(Ordering::SeqCst) == OPEN }

    pub fn quitting(&self) -> bool { self.quit.load(Ordering::SeqCst) }

    /// Every quit, before B6's `stop_engine`: from here on nothing installs and nothing respawns. Once the replacement
    /// step has begun it can't be held on macOS (B6 r1.1); the swap is one step, so either app is left (r2.4).
    pub fn quit(&self) -> bool {
        self.quit.store(true, Ordering::SeqCst);
        self.phase.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |p| (p != INSTALLING).then_some(QUITTING)).is_ok()
    }

    /// The user's own Quit (tray item, macOS menu item): with an update ready (or an install failure with its package in
    /// memory) it becomes the Quit install; during the replacement step it waits for it; otherwise B6's quit.
    pub fn user_quit(&self) -> UserQuit {
        if let Ok(package) = self.begin_install() {
            return UserQuit::Install(package);
        }
        if self.quit() { UserQuit::Quit } else { UserQuit::AfterReplacement }
    }

    /// An exit the app can't hold off for long (macOS Exit: Dock Quit, logout): waits up to `limit` for a replacement
    /// step that has begun, so the swap and the failure mark are done before the process ends.
    pub fn wait_for_replacement(&self, limit: Duration) {
        let until = Instant::now() + limit;
        while self.replacing.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A Quit install failed in the previous run (r3.6, `<data>/log/update-failed`): this run starts showing it (U5d),
    /// with no package, so "Try again" checks and downloads again.
    pub fn failed_before(&self, kind: Kind) {
        let mut inner = self.lock();
        self.set(&mut inner, State::Failed { kind });
    }

    /// A check begins. False: one is running already, or an install is under way. A check by the button shows
    /// "Checking…", unless an update is ready (it stays ready while a newer one is looked for).
    pub fn begin_check(&self, button: bool) -> bool {
        let mut inner = self.lock();
        if inner.busy || !self.open() {
            return false;
        }
        inner.busy = true;
        if button && !matches!(inner.state, State::Ready { .. }) {
            self.set(&mut inner, State::Checking);
        }
        true
    }

    /// The check found nothing newer.
    pub fn up_to_date(&self) {
        let mut inner = self.lock();
        inner.busy = false;
        if !self.open() {
            return;
        }
        let state = match &inner.package {
            None if holds_failure(&inner.state) => return,
            None => State::Idle,
            Some(package) if inner.state == State::Checking => State::Ready { critical: package.critical },
            Some(_) => return,
        };
        self.set(&mut inner, state);
    }

    /// The check failed. A start or timer check fails silently (logged); the button's says so — but never in place of a
    /// ready update.
    pub fn check_failed(&self, button: bool) {
        let mut inner = self.lock();
        inner.busy = false;
        if inner.state == State::Checking {
            self.set(&mut inner, if button { State::CheckFailed } else { State::Idle });
        }
    }

    /// The check found `version`.
    pub fn found(&self, version: &str, deb: bool) -> Next {
        let mut inner = self.lock();
        if deb {
            inner.busy = false;
            self.set(&mut inner, State::DebNotice);
            return Next::Done;
        }
        // Only a newer release replaces the one in memory (a feed can go back to an older one).
        match inner.package.as_ref().filter(|p| !newer(version, &p.version)).map(|p| p.critical) {
            Some(critical) => {
                inner.busy = false;
                if inner.state == State::Checking {
                    self.set(&mut inner, State::Ready { critical });
                }
                Next::Done
            }
            None => Next::Download { visible: !matches!(inner.state, State::Ready { .. }) && !holds_failure(&inner.state) },
        }
    }

    /// The app can't replace itself here (the probe failed, r4.4; the lock isn't healthy, r5.3): no download.
    pub fn manual(&self) {
        let mut inner = self.lock();
        inner.busy = false;
        if self.open() {
            self.set(&mut inner, State::Manual);
        }
    }

    pub fn downloading(&self, critical: bool, percent: Option<u8>) {
        let mut inner = self.lock();
        if self.open() {
            self.set(&mut inner, State::Downloading { percent, critical });
        }
    }

    pub fn downloaded(&self, package: Package) {
        let mut inner = self.lock();
        inner.busy = false;
        let critical = package.critical;
        inner.package = Some(package);
        if self.open() && !holds_failure(&inner.state) {
            self.set(&mut inner, State::Ready { critical });
        }
    }

    /// A download failed; `visible` as `Next::Download` said (a quiet download of a newer version leaves the ready one).
    pub fn download_failed(&self, kind: Kind, visible: bool) {
        let mut inner = self.lock();
        inner.busy = false;
        if visible && self.open() {
            self.set(&mut inner, State::Failed { kind });
        }
    }

    /// What "Try again" does (r1.11, r6: no counter): an install failure retries the install of the package in memory;
    /// every other failure checks and downloads again.
    pub fn retry_installs(&self) -> bool {
        let inner = self.lock();
        inner.state == State::Failed { kind: Kind::Install } && inner.package.is_some()
    }

    /// The click (§2.1): from `ready`, or from an install failure with the package still in memory. Moves the phase to
    /// Stopping and returns the package; anything else is refused.
    pub fn begin_install(&self) -> Result<Package, String> {
        let mut inner = self.lock();
        let installable = matches!(inner.state, State::Ready { .. } | State::Failed { kind: Kind::Install });
        let Some(package) = inner.package.clone().filter(|_| installable) else {
            return Err("no update is ready to install".into());
        };
        self.phase
            .compare_exchange(OPEN, STOPPING, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "an install or a quit is under way".to_string())?;
        self.set(&mut inner, State::Stopping { long: false });
        Ok(package)
    }

    fn set_state(&self, state: State) {
        let mut inner = self.lock();
        self.set(&mut inner, state);
    }

    /// Back to normal after an install that didn't happen; false when a quit won meanwhile. The quit flag is read after
    /// the phase moved: a quit that saw the old phase (and so left ending the app to the install) has set the flag by
    /// then, and one that comes later sees OPEN and ends the app itself; either way exactly one side ends it.
    fn reopen(&self, from: u8, state: State) -> bool {
        let mut inner = self.lock();
        if self.phase.compare_exchange(from, OPEN, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return false;
        }
        if self.quitting() {
            self.phase.store(QUITTING, Ordering::SeqCst);
            return false;
        }
        self.set(&mut inner, state);
        true
    }

    /// The install, on its own thread (r5.2): Stopping (own engine's exit, then the bundle lock), then Installing (the
    /// format check and the one commit step), then a relaunch (click) or the app's exit (Quit). Holds no lock while it
    /// waits. A quit that won during Stopping ends the app itself; this returns without installing.
    pub fn run_install(&self, host: &dyn Host, package: &Package, timing: &Timing, intent: Intent) {
        let (page, long_page) = match intent {
            Intent::Click => ("updating", "updating-long"),
            Intent::Quit => ("quitting-update", "quitting-update-long"),
        };
        host.show(page);
        let started = Instant::now();
        let long = std::cell::Cell::new(false);
        let tick = || {
            if !long.get() && started.elapsed() >= timing.long_after {
                long.set(true);
                self.set_state(State::Stopping { long: true });
                host.show(long_page);
            }
            self.quitting()
        };
        host.log("desktop: update: stopping the engine to install the update");
        if !host.stop_and_wait(&tick) {
            return host.log("desktop: update: quit while the engine was stopping; nothing installed");
        }
        host.log("desktop: update: the engine has stopped");
        let lock = loop {
            if tick() {
                return host.log("desktop: update: quit while waiting for the app's lock; nothing installed");
            }
            match host.bundle_lock() {
                Ok(Some(lock)) => break lock,
                Ok(None) => {}
                Err(error) => {
                    host.log(&format!("desktop: update: the app's update lock can't be used ({error}); not installing"));
                    match intent {
                        Intent::Click if self.reopen(STOPPING, State::Manual) => host.respawn(),
                        Intent::Click => {}
                        Intent::Quit => host.exit(false),
                    }
                    return;
                }
            }
            let until = Instant::now() + timing.lock_retry;
            while Instant::now() < until {
                if tick() {
                    break;
                }
                std::thread::sleep(timing.poll);
            }
        };
        // Set before the phase moves, so an app exit that can no longer stop the step always waits for it.
        self.replacing.store(true, Ordering::SeqCst);
        if self.phase.compare_exchange(STOPPING, INSTALLING, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            self.replacing.store(false, Ordering::SeqCst);
            return host.log("desktop: update: quit before installing; nothing installed");
        }
        self.set_state(State::Installing);
        // A quit that came in by now means no relaunch either. On Windows the app's Exit waits for this step
        // (`wait_for_replacement`), so the attempt mark and the installer's launch finish before the process ends.
        // ponytail: a quit in the instant after the installer's launch can't take back its /R; the app it relaunches is
        // the one the click asked for.
        let relaunch = intent == Intent::Click && !self.quitting();
        match host.install(package, relaunch) {
            Ok(()) => {
                let relaunch = relaunch && !self.quitting();
                host.clear_failed_mark();
                host.log(if relaunch { "desktop: update: installed; restarting" } else { "desktop: update: installed; closing" });
                // ponytail: the lock is held until the process exits (the relaunched app's engine waits up to 2 s for it).
                std::mem::forget(lock);
                host.exit(relaunch);
            }
            Err(failure) => {
                host.log(&format!("desktop: update: not installed: {}", failure.reason));
                drop(lock);
                let kind = if failure.space { Kind::Space } else { Kind::Install };
                // A click respawns the engine (r2.2); a Quit, or a quit during the step, ends the app as asked and says so at
                // the next start (r3.6).
                if intent == Intent::Quit || !self.reopen(INSTALLING, State::Failed { kind }) {
                    host.mark_failed(kind);
                    host.exit(false);
                } else {
                    host.respawn();
                }
            }
        }
        self.replacing.store(false, Ordering::SeqCst);
    }
}

/// What the install needs from the app (lib.rs), and from the tests.
pub trait Host {
    /// The status page with this state (ui/index.html: `updating`, `updating-long`, `quitting-update`,
    /// `quitting-update-long`).
    fn show(&self, page: &str);
    /// supervisor::stop_and_wait.
    fn stop_and_wait(&self, cancelled: &dyn Fn() -> bool) -> bool;
    /// The app's bundle lock, taken exclusively: Ok(None) while an engine of this app runs.
    fn bundle_lock(&self) -> Result<Option<Box<dyn std::any::Any>>, String>;
    /// The replacement step; Ok once committed. `relaunch`: the Windows installer starts the app again after it (`/R`).
    fn install(&self, package: &Package, relaunch: bool) -> Result<(), install::Failure>;
    /// The engine started again through B6's start path, unless a quit came in.
    fn respawn(&self);
    /// The app ends; `relaunch`: into the version just installed.
    fn exit(&self, relaunch: bool);
    /// `<data>/log/update-failed` (r3.6): the next start shows U5d once.
    fn mark_failed(&self, kind: Kind);
    /// An install succeeded: the previous run's mark, if not shown yet, is out of date.
    fn clear_failed_mark(&self);
    fn log(&self, line: &str);
}

pub struct Timing {
    pub poll: Duration,
    /// The bundle lock is tried this often (r4.1: 2 s).
    pub lock_retry: Duration,
    /// U8 joins U7 after this long (B3's stop grace period, 60 s).
    pub long_after: Duration,
}

impl Timing {
    pub const APP: Timing =
        Timing { poll: Duration::from_millis(100), lock_retry: Duration::from_secs(2), long_after: Duration::from_secs(60) };
}

/// The agent's settings: production values, shortened by test-only variables on any identifier but the released app's.
#[derive(Clone)]
pub struct Config {
    pub enabled: bool,
    pub interval: Duration,
    pub read_timeout: Duration,
    pub fault: Option<String>,
}

pub const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

impl Config {
    pub fn read(identifier: &str, debug: bool, env: &dyn Fn(&str) -> Option<String>) -> Config {
        let test = identifier != crate::RELEASE_IDENTIFIER;
        let secs = |name: &str| test.then(|| env(name)).flatten().and_then(|v| v.trim().parse::<u64>().ok()).filter(|v| *v > 0);
        Config {
            // Release builds and test builds (§1): `tauri dev` never reaches the production feed.
            enabled: !debug || identifier == crate::TEST_IDENTIFIER,
            interval: Duration::from_secs(secs("DELTABADGER_TEST_UPDATE_INTERVAL_SECS").unwrap_or(12 * 60 * 60)),
            read_timeout: Duration::from_secs(secs("DELTABADGER_TEST_UPDATE_READ_TIMEOUT_SECS").unwrap_or(60)),
            fault: test.then(|| env("DELTABADGER_TEST_INSTALL_FAULT")).flatten().filter(|f| !f.is_empty()),
        }
    }
}

/// Whether this app can replace itself where it is installed, checked before anything is downloaded: macOS the swap
/// probe (r4.4); everywhere the bundle lock healthy (r5.3). Returns the installed app.
pub fn can_install_here(env: &dyn Fn(&str) -> Option<String>) -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().and_then(|e| e.canonicalize()).map_err(|e| format!("no executable: {e}"))?;
    let app = install::installed_app(&exe, env).ok_or("not an installed app")?;
    #[cfg(target_os = "macos")]
    install::swap_probe(&app).map_err(|e| format!("the app's folder can't take a swap: {e}"))?;
    let dir = install::lock_dir(env).ok_or("no folder for the update lock")?;
    install::open_lock(&dir.join(install::lock_name(&app)))?;
    Ok(app)
}

/// An install failure stays shown until the user acts on it: a check finding nothing or a quiet download doesn't replace it.
fn holds_failure(state: &State) -> bool { matches!(state, State::Failed { kind: Kind::Install | Kind::Space }) }

/// The mark a failed Quit install leaves for the next start (r3.6), in `<data>/log/`.
pub const FAILED_MARK: &str = "update-failed";

/// One line, the UTC time and the kind, fsynced (no version).
pub fn write_failed_mark(log_dir: &std::path::Path, kind: Kind, now: std::time::SystemTime) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(log_dir)?;
    let at = chrono::DateTime::<chrono::Utc>::from(now).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let kind = if kind == Kind::Space { "space" } else { "install" };
    let mut file = std::fs::File::create(log_dir.join(FAILED_MARK))?;
    file.write_all(format!("{at} {kind}\n").as_bytes())?;
    file.sync_all()
}

/// The previous run's mark, if any: its kind (anything unreadable is an install failure). lib.rs deletes it once the page
/// has been given the state.
pub fn read_failed_mark(log_dir: &std::path::Path) -> Option<Kind> {
    let text = std::fs::read_to_string(log_dir.join(FAILED_MARK)).ok()?;
    Some(if text.split_whitespace().nth(1) == Some("space") { Kind::Space } else { Kind::Install })
}

/// `<data>/updates/attempt` (r4.3; install.rs `write_attempt`), read at every start: the version that runs is the outcome,
/// whatever the installer did. The target running → it installed: the attempt and any older failure mark go. Anything
/// else (PREINSTALL refused, the installer crashed or was cancelled, the file unreadable) → the failure mark (U5d once,
/// as after a failed Quit install, r3.6), written before the attempt is deleted. The line to log, if there was one.
pub fn settle_attempt(data: &std::path::Path, log_dir: &std::path::Path, running: &str, now: std::time::SystemTime) -> Option<String> {
    let path = data.join("updates").join("attempt");
    // Only a missing attempt is no attempt: one that can't be read is a failed install.
    let text = match std::fs::read_to_string(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        read => read.unwrap_or_default(),
    };
    let target = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.get("version")?.as_str().map(str::to_string));
    let line = if target.as_deref() == Some(running) {
        let _ = std::fs::remove_file(log_dir.join(FAILED_MARK));
        "desktop: update: the installer installed this version".to_string()
    } else if let Err(error) = write_failed_mark(log_dir, Kind::Install, now) {
        // The attempt stays: the next start tries again.
        return Some(format!("desktop: update: the last installer didn't install this version; could not write {FAILED_MARK}: {error}"));
    } else {
        "desktop: update: the last installer didn't install this version; shown once".to_string()
    };
    Some(match std::fs::remove_file(&path) {
        // ponytail: an attempt that can't be deleted shows U5d again at the next start; a read-only data dir is broken anyway.
        Err(error) => format!("{line}; could not delete the attempt: {error}"),
        Ok(()) => line,
    })
}

/// A .deb is told about updates, never downloads them (§2.4): the same value the plugin picks its install branch by.
fn deb() -> bool {
    #[cfg(target_os = "linux")]
    return tauri::utils::platform::bundle_type() == Some(tauri::utils::config::BundleType::Deb);
    #[cfg(not(target_os = "linux"))]
    false
}

/// One check, and the download it leads to (§1, r1.13). Returns the state after it.
pub async fn check<R: tauri::Runtime>(app: &tauri::AppHandle<R>, agent: &Agent, config: &Config, button: bool, log: &(dyn Fn(&str) + Sync)) -> Snapshot {
    use tauri_plugin_updater::UpdaterExt;
    if !agent.begin_check(button) {
        return agent.snapshot();
    }
    let deb = deb();
    let read_timeout = config.read_timeout;
    let mut builder = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .configure_client(move |client| client.connect_timeout(CONNECT_TIMEOUT).read_timeout(read_timeout));
    if deb {
        builder = builder.target("linux-x86_64-appimage");
    }
    let found = match builder.build() {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    let mut update = match found {
        Ok(Some(update)) => update,
        Ok(None) => {
            agent.up_to_date();
            return agent.snapshot();
        }
        Err(error) => {
            log(&format!("desktop: update: the check failed: {}", chain(&error)));
            agent.check_failed(button);
            return agent.snapshot();
        }
    };
    let critical = critical(&update.raw_json);
    log(&format!("desktop: update: {} is available{}", update.version, if critical { " (critical)" } else { "" }));
    let Next::Download { visible } = agent.found(&update.version, deb) else { return agent.snapshot() };
    if let Err(why) = can_install_here(&|k| std::env::var(k).ok()) {
        log(&format!("desktop: update: can't install in place here: {why}"));
        agent.manual();
        return agent.snapshot();
    }
    if visible {
        agent.downloading(critical, None);
    }
    // The check's 30 s is the whole request's; the download is bounded by its connect and read deadlines instead.
    update.timeout = None;
    let (mut got, mut shown) = (0u64, None);
    let progress = |chunk: usize, total: Option<u64>| {
        got += chunk as u64;
        if let Some(total) = total.filter(|t| *t > 0 && visible) {
            let percent = (got.saturating_mul(100) / total).min(100) as u8;
            if shown != Some(percent) {
                shown = Some(percent);
                agent.downloading(critical, Some(percent));
            }
        }
    };
    match update.download(progress, || {}).await {
        Ok(bytes) => {
            log("desktop: update: downloaded and verified");
            agent.downloaded(Package { version: update.version.clone(), critical, bytes: Arc::new(bytes) });
        }
        Err(error) => {
            let kind = kind_of(&error);
            log(&format!("desktop: update: the download failed ({kind:?}): {}", chain(&error)));
            agent.download_failed(kind, visible);
        }
    }
    agent.snapshot()
}

/// An error with its causes (reqwest's own text leaves out why a request failed).
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// The plugin's errors that mean the package didn't verify (U5b); every other is a failed download (U5a).
pub fn kind_of(error: &tauri_plugin_updater::Error) -> Kind {
    use tauri_plugin_updater::Error::*;
    match error {
        Minisign(_) | Base64(_) | SignatureUtf8(_) | SignedVersionMismatch { .. } | MissingSignedVersion => Kind::Verify,
        _ => Kind::Download,
    }
}

/// `candidate` is a later version than `held` (semver, as the plugin compares); an unparseable one never is.
fn newer(candidate: &str, held: &str) -> bool {
    match (semver::Version::parse(candidate), semver::Version::parse(held)) {
        (Ok(c), Ok(h)) => c > h,
        _ => false,
    }
}

/// `"critical": true` at the top of latest.json (§3); anything else is not critical.
pub fn critical(raw: &serde_json::Value) -> bool { raw.get("critical") == Some(&serde_json::Value::Bool(true)) }

#[cfg(test)]
mod tests;
