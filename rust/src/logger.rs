//! The engine's own log file (PLAN-B6 r2.6, r4.4, r5.3). With `DELTABADGER_LOG_FILE=<path>` (the desktop app sets it),
//! `engine::log` and every `deltabadger:` stderr line (`eline!`) are appended to that file instead of stdout and stderr:
//! the app may be gone while the engine drains, so nobody else can bound the file, and a closed pipe would make
//! `println!` panic mid-drain. The file is renamed to `<path>.1` (replacing the older one) once it passes 10 MB, and the
//! first line of every file it opens is `--- start <DELTABADGER_READY_TOKEN> ---`, so the app finds this start's lines
//! across both files. Write errors are ignored: a full disk drops lines, never the engine's work. A file that can't be
//! opened, or can't be renamed when it is full, takes no more lines. Without the variable,
//! lines go to stdout and stderr exactly as before (Docker, Umbrel: `docker logs`).
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const ROTATE_AT: u64 = 10 * 1024 * 1024;

struct Log { path: PathBuf, file: Option<std::fs::File>, len: u64, limit: u64, marker: String, read_only: bool }

static LOG: OnceLock<Mutex<Log>> = OnceLock::new();
/// While set, no line is written anywhere (`muted`).
static MUTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `f` with every engine line dropped: encryption-reset's judgement runs the takeover on a copy (rotation::owed), whose
/// start-log lines describe nothing that happened.
pub fn muted<T>(f: impl FnOnce() -> T) -> T {
    use std::sync::atomic::Ordering::SeqCst;
    MUTED.store(true, SeqCst);
    let out = f();
    MUTED.store(false, SeqCst);
    out
}

impl Log {
    fn open(&mut self) {
        let append = || std::fs::OpenOptions::new().append(true).create(true).open(&self.path);
        // Tests (debug builds): a file every write fails on, behind the logger.
        self.file = if self.read_only { let _ = append(); std::fs::File::open(&self.path) } else { append() }.ok();
        self.len = std::fs::metadata(&self.path).map_or(0, |m| m.len());
        let marker = std::mem::take(&mut self.marker);
        self.put(&marker);
        self.marker = marker;
    }

    fn put(&mut self, text: &str) {
        // Without a file (it could not be opened, or could not be rotated) lines are dropped. Never stderr instead, whose
        // reader may be gone or stalled: a blocked write would hold the engine.
        if let Some(f) = self.file.as_mut() { if f.write_all(text.as_bytes()).is_ok() { self.len += text.len() as u64 } }
    }

    fn write(&mut self, text: &str) {
        self.put(text);
        if self.file.is_some() && self.len > self.limit {
            self.file = None; // closed before the rename (Windows renames no open file)
            let mut old = self.path.clone().into_os_string();
            old.push(".1");
            // A file that can't be renamed is left as it is, at its size: no line is written past it.
            if std::fs::rename(&self.path, old).is_ok() { self.open(); }
        }
    }
}

/// `serve` and `run`, once, before their first line.
pub fn init(env: &dyn Fn(&str) -> Option<String>) {
    let Some(path) = env("DELTABADGER_LOG_FILE").filter(|p| !p.trim().is_empty()) else { return };
    let token = env("DELTABADGER_READY_TOKEN").unwrap_or_default();
    let test = |name: &str| if cfg!(debug_assertions) { env(name) } else { None };
    let mut log = Log {
        path: PathBuf::from(path), file: None, len: 0,
        // Tests (debug builds): a small size, so a test rotates the file in a few lines.
        limit: test("DELTABADGER_TEST_LOG_ROTATE_BYTES").and_then(|n| n.parse().ok()).unwrap_or(ROTATE_AT),
        marker: format!("--- start {} ---\n", token.trim()),
        read_only: test("DELTABADGER_TEST_LOG_READ_ONLY").is_some_and(|v| v == "1"),
    };
    log.open();
    let _ = LOG.set(Mutex::new(log));
}

fn line(text: &str, stdout: bool) {
    if MUTED.load(std::sync::atomic::Ordering::SeqCst) { return; }
    match LOG.get() {
        Some(log) => log.lock().unwrap_or_else(|e| e.into_inner()).write(&format!("{text}\n")),
        None if stdout => println!("{text}"),
        None => eprintln!("{text}"),
    }
}

/// A line for stdout (`engine::log`), or the log file.
pub fn out(text: &str) { line(text, true) }

/// A line for stderr (`eline!`), or the log file.
pub fn err(text: &str) { line(text, false) }

/// `eprintln!` through the engine's log: stderr as before, or the log file under `DELTABADGER_LOG_FILE`.
#[macro_export]
macro_rules! eline {
    ($($arg:tt)*) => { $crate::logger::err(&format!($($arg)*)) };
}
