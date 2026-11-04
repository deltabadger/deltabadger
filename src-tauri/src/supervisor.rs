//! The engine the app runs, and what its window shows about it (PLAN-B6 §1–§3 as amended by r1.1, r1.4, r1.5, r1.8,
//! r1.11, r2.3, r3.1, r3.3, r4.3, r4.4, r5.3).
//!
//! One child: `deltabadger-engine serve`, with the app's data dir in its environment (`Launch::engine_env`), its stdin a
//! pipe the app closes on every quit, its stdout and stderr captured in `<data>/log/deltabadger-stderr.log`. The engine
//! writes its own lines to `<data>/log/deltabadger.log`; the app adds a few (each start's marker, the data paths, what it
//! showed, how the engine ended). `supervise` runs on its own thread for the app's whole life: it waits out an engine
//! lock someone else holds (D2/D3), starts the engine, follows `/ui-ready` for as long as the child lives, and shows
//! whatever the engine serves, or why it stopped (D4/D5/D7). It never restarts an engine by itself, and it never kills
//! one: a quit only closes the engine's stdin (`stop_engine`), and the engine finishes its tick and exits on its own.
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::secrets;

pub const PORT_BASE: u16 = 3000;
pub const HOST: &str = "127.0.0.1";
const POLL: Duration = Duration::from_millis(500);
const LOCK_POLL: Duration = Duration::from_secs(2);
/// D3's hint joins D2 after this long (r1.8).
const LOCK_HINT_AFTER: Duration = Duration::from_secs(65);
/// D1's second sentence (§3.1).
const STILL_STARTING_AFTER: Duration = Duration::from_secs(60);
/// A page the window shows is replaced by D7 only after this many failed polls in a row (r2.3).
const FAILED_POLLS: u32 = 3;
const DETAIL_LINES: usize = 20;
const STDERR_TAIL: usize = 100;

/// What starts the engine, and the data dir it runs on.
pub struct Launch {
    /// `deltabadger-engine`, next to the app's executable.
    pub exe: PathBuf,
    /// After `serve`: nothing in the app (the unit tests' stub engine).
    pub extra_args: Vec<String>,
    /// The engine's working directory: the app's resources, where the catalogue bundle is (§4.2).
    pub cwd: PathBuf,
    pub data: PathBuf,
    /// Lines for the log once it can be written (how the data dir was chosen).
    pub notes: Vec<String>,
    /// The first start's port, taken without probing it (tests); else the first free one from PORT_BASE.
    pub first_port: Option<u16>,
}

impl Launch {
    pub fn db(&self) -> PathBuf { self.data.join("db") }
    pub fn primary(&self) -> PathBuf { self.db().join("production.sqlite3") }
    pub fn queue(&self) -> PathBuf { self.db().join("production_queue.sqlite3") }
    pub fn tmp(&self) -> PathBuf { self.data.join("tmp") }
    pub fn log_dir(&self) -> PathBuf { self.data.join("log") }
    pub fn log_file(&self) -> PathBuf { self.log_dir().join("deltabadger.log") }
    pub fn stderr_file(&self) -> PathBuf { self.log_dir().join("deltabadger-stderr.log") }
    pub fn secrets(&self) -> PathBuf { self.data.join(".secrets") }
    /// The engine's lock (lease.rs: next to the primary).
    pub fn lock_file(&self) -> PathBuf { self.db().join(".engine.lock") }

    /// What the child gets on top of the app's own environment (PLAN-B6 §1.2, r1.4, r4.4): the 2.x desktop's paths and
    /// platform, the bind address and the stop on stdin's end, this start's ready token, the engine's log file, and the
    /// keys `secrets::setup_secrets` resolved.
    pub fn engine_env(&self, port: u16, token: &str, keys: &HashMap<String, String>) -> HashMap<String, String> {
        let path = |p: PathBuf| p.to_string_lossy().into_owned();
        let mut env: HashMap<String, String> = [
            ("PORT", port.to_string()),
            ("BINDING", HOST.to_string()),
            // Settings reads this to know updates arrive through check_for_updates, not through anything the user
            // has to run; the refusal page words its lines for the desktop.
            ("DELTABADGER_PLATFORM", "desktop".to_string()),
            ("APP_ROOT_URL", format!("http://{HOST}:{port}")),
            ("STORAGE_DIR", path(self.db())),
            ("DATABASE_PATH", path(self.primary())),
            ("QUEUE_DATABASE_PATH", path(self.queue())),
            ("APP_TMP_DIR", path(self.tmp())),
            ("DELTABADGER_STOP_ON_STDIN_EOF", "1".to_string()),
            ("DELTABADGER_READY_TOKEN", token.to_string()),
            ("DELTABADGER_LOG_FILE", path(self.log_file())),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        env.extend(keys.iter().map(|(k, v)| (k.clone(), v.clone())));
        env
    }
}

/// The engine that runs now, if one does. Shared by the supervisor and every quit.
#[derive(Default)]
pub struct Engine(Mutex<Slot>);

#[derive(Default)]
struct Slot {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    quitting: bool,
    log: Option<PathBuf>,
}

/// How `stop_engine` found the engine.
#[derive(Debug)]
pub enum Stopped {
    NotRunning,
    /// It had already exited.
    Exited(ExitStatus),
    /// Its stdin is closed: it finishes what it is doing and exits on its own.
    Draining,
}

impl Engine {
    fn slot(&self) -> MutexGuard<'_, Slot> { self.0.lock().unwrap_or_else(|e| e.into_inner()) }

    pub fn quitting(&self) -> bool { self.slot().quitting }

    /// The running engine's process id.
    pub fn pid(&self) -> Option<u32> { self.slot().child.as_ref().map(Child::id) }

    /// The engine's exit status once it has exited (the child is then forgotten).
    fn exited(&self) -> Option<ExitStatus> {
        let mut slot = self.slot();
        let status = slot.child.as_mut()?.try_wait().ok()??;
        slot.child = None;
        slot.stdin = None;
        Some(status)
    }
}

/// Every quit (tray Quit, Cmd-Q, Dock Quit, AppleScript, logout; PLAN-B6 r1.1): the engine's stdin is closed and this
/// returns at once. It never waits and never kills: the engine finishes the tick in hand and exits by itself, and the
/// supervisor shows nothing for an exit that was asked for. Also stops a start that has not spawned yet.
pub fn stop_engine(engine: &Engine) -> Stopped {
    let mut slot = engine.slot();
    slot.quitting = true;
    let had_stdin = slot.stdin.take().is_some();
    let stopped = match slot.child.as_mut() {
        None => Stopped::NotRunning,
        Some(child) => match child.try_wait() {
            Ok(Some(status)) => Stopped::Exited(status),
            _ => Stopped::Draining,
        },
    };
    if had_stdin {
        if let Some(log) = &slot.log {
            let _ = note(log, "desktop: quit: the engine's input is closed; it stops on its own");
        }
    }
    stopped
}

/// What the window shows: the launcher's status page with one of these, or an engine page.
pub trait View: Send + Sync {
    /// The status page with `fragment` (`Page::fragment`).
    fn status(&self, fragment: &str);
    /// An engine page.
    fn open(&self, url: &str);
    /// Back to the engine page the window showed before the status page replaced it (r2.3).
    fn back(&self);
}

/// The status page's states (ui/index.html reads them from `location.hash`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    /// D1.
    Starting,
    /// D1 with its second sentence.
    StillStarting,
    /// D2.
    Finishing,
    /// D2 with D3's hint.
    FinishingLong,
    /// D4, with the details.
    Stopped(String),
    /// D5, with the details.
    NotStarted(String),
    /// D7.
    NotResponding,
}

impl Page {
    pub fn name(&self) -> &'static str {
        match self {
            Page::Starting => "starting",
            Page::StillStarting => "still-starting",
            Page::Finishing => "finishing",
            Page::FinishingLong => "finishing-long",
            Page::Stopped(_) => "stopped",
            Page::NotStarted(_) => "not-started",
            Page::NotResponding => "not-responding",
        }
    }

    /// `<state>&log=<the log folder>&details=<lines>` (the last two for D4 and D5), percent-encoded.
    pub fn fragment(&self, log_dir: &Path) -> String {
        let mut fragment = self.name().to_string();
        if let Page::Stopped(details) | Page::NotStarted(details) = self {
            fragment.push_str("&log=");
            fragment.push_str(&encode(&log_dir.to_string_lossy()));
            fragment.push_str("&details=");
            fragment.push_str(&encode(details));
        }
        fragment
    }
}

fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Appends one line to the log.
pub fn note(log: &Path, line: &str) -> Result<(), String> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .and_then(|mut f| f.write_all(format!("{line}\n").as_bytes()))
        .map_err(|error| format!("Failed to write {:?}: {error}", log))
}

pub fn find_available_port(start: u16) -> u16 {
    (start..start.saturating_add(100)).find(|port| !answers(*port)).unwrap_or(start)
}

fn answers(port: u16) -> bool {
    port_check::is_port_reachable_with_timeout(format!("{HOST}:{port}"), Duration::from_millis(100))
}

/// Whether another process holds the engine lock now. Probed and released at once: the engine takes it itself
/// (r1.8). A data dir without a lock file has no engine.
pub fn lock_held(lock_file: &Path) -> Result<bool, String> {
    let file = match File::open(lock_file) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("Failed to open {:?}: {error}", lock_file)),
    };
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(error)) => Err(format!("Failed to probe {:?}: {error}", lock_file)),
    }
}

/// Before the first start (r1.11): the folders, and the keys.
fn prepare(launch: &Launch) -> Result<HashMap<String, String>, String> {
    for dir in [launch.data.clone(), launch.db(), launch.tmp(), launch.log_dir()] {
        fs::create_dir_all(&dir).map_err(|error| format!("Failed to create writable directory {:?}: {error}", dir))?;
    }
    secrets::setup_secrets(&launch.data, &launch.primary())
}

/// The supervisor, for the app's whole life (r1.5, r1.11). Returns when a quit came in or once the window shows why
/// the engine isn't running (D4/D5).
pub fn supervise(launch: &Launch, engine: &Engine, view: &dyn View) {
    let log = launch.log_file();
    let show = |page: Page| {
        let _ = note(&log, &format!("desktop: showing {}", page.name()));
        view.status(&page.fragment(&launch.log_dir()));
    };
    let keys = match prepare(launch) {
        Ok(keys) => keys,
        Err(error) => return show(Page::NotStarted(error)),
    };
    for line in &launch.notes {
        let _ = note(&log, line);
    }
    let (mut first_port, mut base, mut retried_port) = (launch.first_port, PORT_BASE, false);
    loop {
        match wait_for_lock(launch, engine, &show) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => return show(Page::NotStarted(error)),
        }
        let port = first_port.take().unwrap_or_else(|| find_available_port(base));
        let token = match secrets::random_hex(16) {
            Ok(token) => token,
            Err(error) => return show(Page::NotStarted(error)),
        };
        match start(launch, engine, port, &token, &keys) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                let _ = note(&log, &format!("desktop: {error}"));
                return show(Page::NotStarted(error));
            }
        }
        let (status, served) = match watch(engine, view, &show, &log, port, &token) {
            Watched::Quit(status) => {
                let _ = note(&log, &format!("desktop: the engine stopped as asked (exit {})", describe(&status)));
                return;
            }
            Watched::Exited { status, served } => (status, served),
        };
        record_exit(launch, &status);
        // Retries only before anything was served, and only for these two (r1.5): another engine holds the lock (wait
        // it out), or someone took the port between the probe and the bind (the next port, once).
        if !served && status.code() == Some(1) {
            match lock_held(&launch.lock_file()) {
                Ok(true) => continue,
                Ok(false) if !retried_port && answers(port) => {
                    retried_port = true;
                    base = port.saturating_add(1);
                    let _ = note(&log, &format!("desktop: port {port} is taken; trying the next one"));
                    continue;
                }
                _ => {}
            }
        }
        let details = this_start(launch, &token);
        return show(if served { Page::Stopped(details) } else { Page::NotStarted(details) });
    }
}

/// D2 while another process holds the engine lock, re-probed every 2 s, with D3's hint after 65 s (r1.8). False on a
/// quit.
fn wait_for_lock(launch: &Launch, engine: &Engine, show: &dyn Fn(Page)) -> Result<bool, String> {
    let started = Instant::now();
    let mut shown = None;
    loop {
        if engine.quitting() {
            return Ok(false);
        }
        if !lock_held(&launch.lock_file())? {
            return Ok(true);
        }
        let page = if started.elapsed() >= LOCK_HINT_AFTER { Page::FinishingLong } else { Page::Finishing };
        if shown.as_ref() != Some(&page) {
            show(page.clone());
            shown = Some(page);
        }
        let until = Instant::now() + LOCK_POLL;
        while Instant::now() < until && !engine.quitting() {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// One spawn. Before it (r3.3, r4.3, r4.4, r5.3): the previous start's capture kept as `.1`, this start's marker and
/// the data paths in the log, the key file synced. False when a quit came in first.
fn start(launch: &Launch, engine: &Engine, port: u16, token: &str, keys: &HashMap<String, String>) -> Result<bool, String> {
    let log = launch.log_file();
    let capture_path = launch.stderr_file();
    match fs::rename(&capture_path, launch.log_dir().join("deltabadger-stderr.log.1")) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("Failed to keep the previous {:?}: {error}", capture_path))
        }
        _ => {}
    }
    let capture = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&capture_path)
        .map_err(|error| format!("Failed to open {:?}: {error}", capture_path))?;
    let capture_err = capture
        .try_clone()
        .map_err(|error| format!("Failed to open {:?}: {error}", capture_path))?;
    note(&log, &format!("--- start {token} ---"))?;
    note(
        &log,
        &format!("data: {} primary: {} queue: {}", launch.data.display(), launch.primary().display(), launch.queue().display()),
    )?;
    secrets::make_durable(&launch.secrets())?;

    let mut command = Command::new(&launch.exe);
    command
        .arg("serve")
        .args(&launch.extra_args)
        .current_dir(&launch.cwd)
        .envs(launch.engine_env(port, token, keys))
        .stdin(Stdio::piped())
        .stdout(capture)
        .stderr(capture_err);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000); // CREATE_NO_WINDOW

    // Under the slot's lock: a quit either comes before (nothing is spawned) or finds the stdin to close.
    let mut slot = engine.slot();
    if slot.quitting {
        return Ok(false);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start {:?}: {error}", launch.exe))?;
    let _ = note(&log, &format!("desktop: the engine started on port {port} (process {})", child.id()));
    slot.stdin = child.stdin.take();
    slot.child = Some(child);
    slot.log = Some(log);
    Ok(true)
}

enum Watched {
    /// It exited after a quit.
    Quit(ExitStatus),
    Exited { status: ExitStatus, served: bool },
}

enum Ready {
    Starting,
    /// `app` or `refused`: a page to show.
    Page(&'static str),
}

/// `/ui-ready` of this start's engine: only an answer with its own token counts (r1.4).
fn ready(client: &reqwest::blocking::Client, port: u16, token: &str) -> Option<Ready> {
    let response = client.get(format!("http://{HOST}:{port}/ui-ready")).send().ok()?;
    if response.status() != reqwest::StatusCode::OK {
        return None;
    }
    let body = response.text().ok()?;
    let mut words = body.trim().split(' ');
    if words.next()? != "ready" || words.next()? != token {
        return None;
    }
    let state = match words.next()? {
        "starting" => Ready::Starting,
        "app" => Ready::Page("app"),
        "refused" => Ready::Page("refused"),
        _ => return None,
    };
    words.next().is_none().then_some(state)
}

/// Follows one child for its whole life (r1.5, r2.3, r3.1): `starting` is D1 on the status page; `app` or `refused`
/// is shown once, at `/?launch=<token>` (never a page a service worker cached); once something was shown, three
/// failed polls in a row put D7 in its place until the same child answers again.
fn watch(engine: &Engine, view: &dyn View, show: &dyn Fn(Page), log: &Path, port: u16, token: &str) -> Watched {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(750))
        .no_proxy()
        .build()
        .ok();
    let started = Instant::now();
    let (mut served, mut failed) = (false, 0_u32);
    // The status page the window shows (None: an engine page), and the engine page it showed last.
    let mut status = Some(Page::Starting);
    let mut opened: Option<&'static str> = None;
    show(Page::Starting);
    loop {
        if let Some(exit) = engine.exited() {
            return if engine.quitting() { Watched::Quit(exit) } else { Watched::Exited { status: exit, served } };
        }
        let put = |page: Page, status: &mut Option<Page>| {
            if status.as_ref() != Some(&page) {
                show(page.clone());
                *status = Some(page);
            }
        };
        match client.as_ref().and_then(|c| ready(c, port, token)) {
            Some(Ready::Starting) => {
                failed = 0;
                let long = !served && started.elapsed() >= STILL_STARTING_AFTER;
                put(if long { Page::StillStarting } else { Page::Starting }, &mut status);
            }
            Some(Ready::Page(state)) => {
                failed = 0;
                served = true;
                if status.is_some() || opened != Some(state) {
                    if opened == Some(state) {
                        let _ = note(log, &format!("desktop: showing the engine's {state} page again"));
                        view.back();
                    } else {
                        let _ = note(log, &format!("desktop: showing the engine's {state} page"));
                        view.open(&format!("http://{HOST}:{port}/?launch={token}"));
                        opened = Some(state);
                    }
                    status = None;
                }
            }
            None if served => {
                failed += 1;
                if failed >= FAILED_POLLS && status.is_none() {
                    put(Page::NotResponding, &mut status);
                }
            }
            None => {
                if status == Some(Page::Starting) && started.elapsed() >= STILL_STARTING_AFTER {
                    put(Page::StillStarting, &mut status);
                }
            }
        }
        thread::sleep(POLL);
    }
}

fn describe(status: &ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    status.code().map_or_else(|| "unknown".to_string(), |code| code.to_string())
}

/// After an exit the app didn't ask for (r4.4): the log gets how it ended and the last of the engine's own output (a
/// panic, a loader error), so the one file holds the failure.
fn record_exit(launch: &Launch, status: &ExitStatus) {
    let capture = fs::read(launch.stderr_file()).unwrap_or_default();
    let capture = String::from_utf8_lossy(&capture);
    let lines: Vec<&str> = capture.lines().collect();
    let tail = &lines[lines.len().saturating_sub(STDERR_TAIL)..];
    let mut block = format!("--- the engine stopped (exit {}); its last output: ---", describe(status));
    for line in tail {
        block.push('\n');
        block.push_str(line);
    }
    let _ = note(&launch.log_file(), &block);
}

/// This start's last lines (r4.4, r5.3): what follows its marker in `deltabadger.log.1` and in `deltabadger.log`, in
/// that order, from the files that hold the marker. The engine writes the marker first in every file it opens, so a
/// start rotated out of its first file is still found; an earlier start's lines are never taken.
pub fn this_start(launch: &Launch, token: &str) -> String {
    let marker = format!("--- start {token} ---");
    let mut lines = Vec::new();
    let mut rotated = launch.log_file().into_os_string();
    rotated.push(".1");
    for file in [PathBuf::from(rotated), launch.log_file()] {
        let Ok(bytes) = fs::read(&file) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        if let Some(at) = text.find(&marker) {
            lines.extend(text[at..].lines().filter(|line| *line != marker).map(str::to_string));
        }
    }
    lines[lines.len().saturating_sub(DETAIL_LINES)..].join("\n")
}
