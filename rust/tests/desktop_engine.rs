//! `serve` as the desktop app runs it (PLAN-B6 B6-0a): BINDING, the stop on stdin's end, the ready token, and the
//! backup's sync on Windows (D-BACKUP-WINDOWS-SYNC). Rails-free: every install here is one `serve` created on an empty
//! STORAGE_DIR, so this file runs on Linux and Windows in CI.
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const SECRET: &str = "desktop-test-secret";
const TOKEN: &str = "0123456789abcdef0123456789abcdef";

struct Engine { child: Child, stdin: Option<ChildStdin>, lines: Receiver<String>, port: u16 }
impl Drop for Engine {
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}

impl Engine {
    /// The next line naming `what` within 60 s (every line before it is skipped).
    fn line(&self, what: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(l) if l.contains(what) => return l,
                Ok(_) => {}
                Err(_) => panic!("no line with {what:?} within 60 s"),
            }
        }
    }
    /// What the desktop app does on quit: close stdin.
    fn close_stdin(&mut self) { drop(self.stdin.take()); }
    fn exit_code(&mut self, within: Duration) -> Option<i32> {
        let deadline = Instant::now() + within;
        loop {
            if let Some(s) = self.child.try_wait().unwrap() { return s.code(); }
            assert!(Instant::now() < deadline, "still running after {within:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    fn ready(&self) -> String {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(a) = get(self.port, "/ui-ready").filter(|a| a.starts_with("HTTP/1.1 200")) { return body(&a).to_string() }
            assert!(Instant::now() < deadline, "/ui-ready never answered 200");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn free_port() -> u16 { std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() }

fn command(dir: &Path, port: u16) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_deltabadger"));
    c.arg("serve").env("STORAGE_DIR", dir).env("SECRET_KEY_BASE", SECRET).env("PORT", port.to_string())
        .env("IBKR_CATALOGUE_URL", "off").env("BINDING", "127.0.0.1").env("DELTABADGER_READY_TOKEN", TOKEN)
        .env("DELTABADGER_STOP_ON_STDIN_EOF", "1")
        .env_remove("DATABASE_URL").env_remove("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY").env_remove("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT")
        .env_remove("DELTABADGER_TEST_PAUSE");
    // As the desktop app starts it: no console window, so no Ctrl-C reaches it either (PLAN-B6 r1.7).
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut c, 0x0800_0000);
    c
}

fn start(c: &mut Command, port: u16) -> Engine {
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let (tx, lines) = std::sync::mpsc::channel();
    for out in [Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>, Box::new(child.stderr.take().unwrap())] {
        let tx = tx.clone();
        std::thread::spawn(move || for l in std::io::BufReader::new(out).lines().map_while(Result::ok) { let _ = tx.send(l); });
    }
    Engine { stdin: child.stdin.take(), child, lines, port }
}

fn get(port: u16, path: &str) -> Option<String> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").ok()?;
    let mut a = String::new();
    s.read_to_string(&mut a).ok()?;
    Some(a)
}

fn body(answer: &str) -> &str { answer.split_once("\r\n\r\n").map_or("", |(_, b)| b) }

fn lock_is_free(dir: &Path) -> bool {
    let lock = deltabadger::store::Paths::from_env(&|_| None, dir).lock_file();
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&lock).unwrap();
    f.try_lock().is_ok()
}

/// The addresses the OS says listen on `port`.
fn listening(port: u16) -> Vec<String> {
    let out = if cfg!(windows) { Command::new("netstat").args(["-an", "-p", "TCP"]).output() }
              else if cfg!(target_os = "macos") { Command::new("lsof").args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN"]).output() }
              else { Command::new("ss").args(["-ltnH"]).output() }.unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    text.lines().filter(|l| !cfg!(windows) || l.contains("LISTENING"))
        .flat_map(|l| l.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|t| t.ends_with(&format!(":{port}"))).collect()
}

/// An install one `serve` made and stopped, with the lease row deleted: an install Rails owns, as far as `serve` can tell,
/// so the next start backs it up before taking it over.
fn rails_owned_install() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut e = start(&mut command(dir.path(), port), port);
    e.line("running, with the web UI");
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(c.execute("DELETE FROM app_configs WHERE key = ?1", [deltabadger::lease::KEY]).unwrap(), 1);
    // One Rust migration pending again, as on an install from before it: its table gone and its row with it.
    c.execute_batch("DROP TABLE rust_idempotency_orders").unwrap();
    assert_eq!(c.execute("DELETE FROM rust_schema_migrations WHERE version = ?1", [PENDING]).unwrap(), 1);
    dir
}

/// The migration `rails_owned_install` leaves pending.
const PENDING: &str = "20261107000001";

fn migrated(dir: &Path) -> bool {
    rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap()
        .query_row("SELECT count(*) FROM rust_schema_migrations WHERE version = ?1", [PENDING], |r| r.get::<_, i64>(0)).unwrap() == 1
}

fn lease(dir: &Path) -> Option<String> {
    rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap()
        .query_row("SELECT value FROM app_configs WHERE key = ?1", [deltabadger::lease::KEY], |r| r.get(0)).ok()
}

fn backups(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir.join("backups")).map(|d| d.map(|e| e.unwrap().path()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn partial(copy: &Path) -> bool { copy.to_string_lossy().ends_with(".partial") }

#[test]
fn the_desktop_engine_listens_on_loopback_answers_with_its_token_and_stops_when_stdin_closes() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut e = start(&mut command(dir.path(), port), port);
    assert!(e.line("running, with the web UI").contains(&format!("port {port}")));
    assert_eq!(e.ready(), format!("ready {TOKEN} app"));
    assert!(get(port, "/health-check").unwrap().starts_with("HTTP/1.1 200"));
    let addrs = listening(port);
    assert!(!addrs.is_empty() && addrs.iter().all(|a| a == &format!("127.0.0.1:{port}")), "{addrs:?}");
    assert!(!lock_is_free(dir.path()), "the engine holds its lock");
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(10)), Some(0));
    e.line("stopped on request");
    assert!(lock_is_free(dir.path()));
}

/// Docker and Umbrel set neither variable: the engine listens on every address, stdin's end stops nothing, and
/// `/ui-ready` answers `ready`.
#[test]
fn without_the_desktop_variables_stdin_is_not_a_stop_and_the_bind_is_every_address() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut c = command(dir.path(), port);
    c.env_remove("BINDING").env_remove("DELTABADGER_READY_TOKEN").env_remove("DELTABADGER_STOP_ON_STDIN_EOF");
    let mut e = start(&mut c, port);
    e.line("running, with the web UI");
    assert_eq!(e.ready(), "ready");
    let addrs = listening(port);
    assert!(addrs.iter().any(|a| a != &format!("127.0.0.1:{port}")), "every address: {addrs:?}");
    e.close_stdin();
    std::thread::sleep(Duration::from_secs(3));
    assert!(e.child.try_wait().unwrap().is_none(), "stdin's end is no stop without DELTABADGER_STOP_ON_STDIN_EOF");
    assert!(get(port, "/health-check").unwrap().starts_with("HTTP/1.1 200"));
}

#[test]
fn an_unparseable_binding_is_refused_before_anything_is_opened() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let out = command(dir.path(), port).env("BINDING", "localhost").stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("BINDING must be an IP address"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing created");
}

/// The refusal page: `ready <token> refused`, and stdin's end stops it with 0.
#[test]
fn the_refusal_page_answers_refused_and_stops_when_stdin_closes() {
    let dir = tempfile::tempdir().unwrap();
    rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap().execute_batch("CREATE TABLE notes (x)").unwrap();
    let port = free_port();
    let mut e = start(&mut command(dir.path(), port), port);
    assert_eq!(e.ready(), format!("ready {TOKEN} refused"));
    assert!(get(port, "/").unwrap().contains("Deltabadger didn't start"));
    assert_eq!(listening(port), vec![format!("127.0.0.1:{port}")]);
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(10)), Some(0));
}

/// C7, the wait for a just-stopped 2.x: `ready <token> starting`, and stdin's end stops it with 0, nothing claimed.
#[test]
fn the_starting_page_answers_starting_and_stops_when_stdin_closes() {
    let dir = rails_owned_install();
    let at = deltabadger::codec::format_time(chrono::Utc::now() - chrono::Duration::seconds(30));
    rusqlite::Connection::open(dir.path().join("production_queue.sqlite3")).unwrap().execute(
        "INSERT INTO solid_queue_processes (kind, last_heartbeat_at, pid, hostname, created_at, name) VALUES ('Worker', ?1, 1, 'h', ?1, 'w')", [at]).unwrap();
    let port = free_port();
    let mut e = start(&mut command(dir.path(), port), port);
    e.line("before taking over");
    assert_eq!(e.ready(), format!("ready {TOKEN} starting"));
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(10)), Some(0));
    assert_eq!(lease(dir.path()), None, "nothing claimed");
}

/// stdin's end before anything is shown: at once, exit 0, and the next start serves.
#[test]
fn stdin_closed_from_the_start_stops_before_the_claim_and_the_next_start_serves() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut first = start(&mut command(dir.path(), port), port);
    first.close_stdin(); // before the engine has read anything
    assert_eq!(first.exit_code(Duration::from_secs(30)), Some(0));
    first.line("stopped on request");
    assert!(lock_is_free(dir.path()));
    let mut e = start(&mut command(dir.path(), port), port);
    e.line("took over");
    e.line("running, with the web UI");
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(10)), Some(0));
}

/// PLAN-B6 r2.5, r3.6: stdin's end at each start barrier. The phase under way finishes, the next is not begun: before the
/// backup nothing is copied; after it the copy and the migrations complete and the claim is not made. Every case exits
/// 0 with the lock free, and the next start takes the install over and serves.
#[cfg(debug_assertions)]
#[test]
fn stdin_closed_at_each_start_barrier_stops_before_the_claim() {
    for (barrier, copied) in [("before-backup", false), ("after-backup", true), ("after-migrate", true), ("before-claim", true)] {
        let dir = rails_owned_install();
        let release = dir.path().join("release");
        let port = free_port();
        let mut c = command(dir.path(), port);
        c.env("DELTABADGER_TEST_PAUSE", barrier).env("DELTABADGER_TEST_RELEASE", &release);
        let mut e = start(&mut c, port);
        e.line(&format!("paused at {barrier}"));
        e.close_stdin();
        std::thread::sleep(Duration::from_millis(300)); // the latch is set by its own thread
        std::fs::write(&release, "").unwrap();
        assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0), "{barrier}");
        assert!(lock_is_free(dir.path()), "{barrier}");
        assert_eq!(lease(dir.path()), None, "{barrier}: nothing claimed");
        let made = backups(dir.path());
        assert_eq!(made.len(), usize::from(copied), "{barrier}: {made:?}");
        assert!(made.iter().all(|b| !partial(b) && b.join("digest").exists()), "{barrier}: a complete copy: {made:?}");
        assert_eq!(migrated(dir.path()), copied, "{barrier}: the migrations run with the copy, both or neither");
        std::fs::remove_file(&release).unwrap();
        let mut next = start(&mut command(dir.path(), port), port);
        next.line("took over");
        next.line("running, with the web UI");
        assert!(migrated(dir.path()), "{barrier}");
        next.close_stdin();
        assert_eq!(next.exit_code(Duration::from_secs(10)), Some(0), "{barrier}: the next start");
    }
}

/// After the engine's handlers are in place, stdin's end is the engine's own stop: claimed, then "stopped on request".
#[cfg(debug_assertions)]
#[test]
fn stdin_closed_once_the_engine_runs_stops_it_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let release = dir.path().join("release");
    let port = free_port();
    let mut c = command(dir.path(), port);
    c.env("DELTABADGER_TEST_PAUSE", "after-handlers").env("DELTABADGER_TEST_RELEASE", &release);
    let mut e = start(&mut c, port);
    e.line("took over");
    e.line("paused at after-handlers");
    e.close_stdin();
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&release, "").unwrap();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    e.line("stopped on request");
    assert!(lease(dir.path()).is_some(), "claimed");
    assert!(lock_is_free(dir.path()));
}

/// D-BACKUP-WINDOWS-SYNC: the copy before a takeover fsyncs every file it wrote; on Windows that needs a handle with write
/// access. Made, with its digest, and no `.partial` left.
#[test]
fn the_backup_before_a_takeover_completes_on_every_os() {
    let dir = rails_owned_install();
    let paths = deltabadger::store::Paths::from_env(&|_| None, dir.path());
    match deltabadger::store::backup(&paths, chrono::Utc::now()).unwrap() {
        deltabadger::store::Backup::Made(copy) => {
            use sha2::Digest;
            let digest: String = ["production.sqlite3", "production_queue.sqlite3"].iter()
                .map(|f| format!("{}  {f}\n", hex::encode(sha2::Sha256::digest(std::fs::read(copy.join(f)).unwrap())))).collect();
            assert_eq!(std::fs::read_to_string(copy.join("digest")).unwrap(), digest);
        }
        other => panic!("{other:?}"),
    }
    assert!(!backups(dir.path()).iter().any(|b| partial(b)), "no .partial: {:?}", backups(dir.path()));
}
