//! `serve` as the desktop app runs it (PLAN-B6 B6-0a): BINDING, the stop on stdin's end, the ready token, and the
//! backup's sync on Windows (D-BACKUP-WINDOWS-SYNC). Rails-free: every install here is one `serve` created on an empty
//! STORAGE_DIR, so this file runs on Linux and Windows in CI. B6-0b: the engine's own log (DELTABADGER_LOG_FILE), the
//! desktop's refusal wording with its phase sentence, the debug hooks for a failing migration and a panic, and the
//! retiring service worker.
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
    /// Every line not read yet, once the process has exited and both pipes are closed.
    fn rest(&self) -> Vec<String> {
        let mut v = Vec::new();
        while let Ok(l) = self.lines.recv_timeout(Duration::from_secs(10)) { v.push(l) }
        v
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
        .env_remove("DELTABADGER_TEST_PAUSE").env_remove("DELTABADGER_PLATFORM").env_remove("DELTABADGER_LOG_FILE")
        .env_remove("ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL");
    // As the desktop app starts it: no console window, so no Ctrl-C reaches it either (PLAN-B6 r1.7).
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut c, 0x0800_0000);
    c
}

fn start(c: &mut Command, port: u16) -> Engine {
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let (tx, lines) = std::sync::mpsc::channel();
    // Each line carries where it came from: "out: " or "err: ".
    for (tag, out) in [("out: ", Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>), ("err: ", Box::new(child.stderr.take().unwrap()))] {
        let tx = tx.clone();
        std::thread::spawn(move || for l in std::io::BufReader::new(out).lines().map_while(Result::ok) { let _ = tx.send(format!("{tag}{l}")); });
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

// ---- B6-0b ----

/// A data dir as the desktop app lays it out (PLAN-B6 §1.2): STORAGE_DIR=<data>/db, the log in <data>/log.
struct Data { dir: tempfile::TempDir }
impl Data {
    fn new() -> Self {
        let d = Data { dir: tempfile::tempdir().unwrap() };
        std::fs::create_dir_all(d.db()).unwrap();
        std::fs::create_dir_all(d.log_dir()).unwrap();
        d
    }
    fn path(&self) -> &Path { self.dir.path() }
    fn db(&self) -> PathBuf { self.path().join("db") }
    fn log_dir(&self) -> PathBuf { self.path().join("log") }
    fn log_file(&self) -> PathBuf { self.log_dir().join("deltabadger.log") }
    fn log(&self) -> String { std::fs::read_to_string(self.log_file()).unwrap_or_default() }
    fn primary(&self) -> PathBuf { self.db().join("production.sqlite3") }
    fn queue(&self) -> PathBuf { self.db().join("production_queue.sqlite3") }
    fn sql(&self, sql: &str) { rusqlite::Connection::open(self.primary()).unwrap().execute_batch(sql).unwrap() }
    /// A copy of this data dir's install (its files, not its folders).
    fn copy(&self) -> Data {
        let to = Data::new();
        for f in std::fs::read_dir(self.db()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_file()) {
            std::fs::copy(&f, to.db().join(f.file_name().unwrap())).unwrap();
        }
        to
    }
    /// Removes a database with its -wal and -shm.
    fn remove(&self, db: &Path) {
        for suffix in ["", "-wal", "-shm"] { let _ = std::fs::remove_file(format!("{}{suffix}", db.display())); }
    }
}

/// `serve` as the desktop app starts it on `data`.
fn desktop(data: &Data, port: u16) -> Command {
    let mut c = command(&data.db(), port);
    c.env("DELTABADGER_PLATFORM", "desktop").env("DELTABADGER_LOG_FILE", data.log_file());
    c
}

/// An install a desktop `serve` made and stopped: Rust's, fully migrated, nothing in it but its seeds.
fn made() -> Data {
    let data = Data::new();
    let port = free_port();
    let mut e = start(&mut desktop(&data, port), port);
    assert_eq!(e.ready(), format!("ready {TOKEN} app"));
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    data
}

/// What a refused start showed: its page, and every stdout ("out: ") and stderr ("err: ") line. Stopped by stdin's end.
fn refused(mut c: Command, port: u16) -> (String, Vec<String>) {
    let mut e = start(&mut c, port);
    assert_eq!(e.ready(), format!("ready {TOKEN} refused"));
    let page = body(&get(port, "/").unwrap()).to_string();
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    (page, e.rest())
}

const FAILING: &str = "20261107000001";

/// A working bot whose stored label is no text: the start's set-aside check cannot name it, so the check fails.
const MALFORMED_BOT: &str = "INSERT INTO bots (type, status, exchange_id, settings, transient_data, label, created_at, updated_at)
    VALUES ('Bots::DcaSingleAsset', 1, (SELECT min(id) FROM exchanges), '{}', '{}', X'FF00', '2026-01-01 00:00:00', '2026-01-01 00:00:00')";

/// One refusal row of PLAN-B6 r2.4 (+ r3.7): how to break the install, the environment it starts with, the operator's
/// text (today's page and stderr everywhere but the desktop), and the desktop's line.
struct Row { name: &'static str, operator: String, line: String, env: Vec<(&'static str, String)> }

/// Breaks a copy of `base` for each row and returns the copy with its row. `<data>`'s paths are the copy's own.
fn rows(base: &Data) -> Vec<(Data, Row)> {
    let mut out = Vec::new();
    let mut row = |name: &'static str, breaks: &dyn Fn(&Data), env: Vec<(&'static str, String)>, operator: &dyn Fn(&Data) -> String, line: &dyn Fn(&Data) -> String| {
        let d = base.copy();
        breaks(&d);
        let r = Row { name, operator: operator(&d), line: line(&d), env };
        out.push((d, r));
    };
    let secrets = |d: &Data| d.path().join(".secrets").display().to_string();
    let log = |d: &Data| d.log_dir().display().to_string();
    let none = |_: &Data| {};
    let wrong = |d: &Data| format!("Deltabadger didn't start: it can't read your data with the keys in {}. If that file was replaced or deleted, quit \
        Deltabadger, put the original file back, and open it again. If you set Deltabadger's keys yourself, they must be the ones this data was made \
        with. Nothing was changed.", secrets(d));
    let incomplete = |d: &Data| format!("Deltabadger didn't start: it doesn't have all the keys it needs to read your data. If you supplied \
        Deltabadger's keys yourself, open it again with the same complete keys. Otherwise, quit Deltabadger, put the original {} file back, and open \
        it again. Nothing was changed.", secrets(d));
    let missing = |d: &Data| format!("Deltabadger didn't start: one of its two data files is missing from {}. If you moved files out of that folder, \
        quit Deltabadger, put them back, and open it again. Nothing was changed.", d.db().display());
    let foreign = |d: &Data| format!("Deltabadger didn't start: the data in {} isn't data this version can open. Nothing was changed. Contact support \
        and send them the files in {}.", d.db().display(), log(d));
    let pair = |pk: &str, salt: &str| vec![("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", pk.to_string()), ("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", salt.to_string())];
    row("wrong pair", &none, pair("another-primary-key", "another-salt"),
        &|_| "can't read this install with this SECRET_KEY_BASE. Use the secret this install was created with.".into(), &wrong);
    row("partial pair", &none, vec![("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", "a-primary-key".into())],
        &|_| "encryption keys: Partial { missing: \"ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT\" }".into(), &incomplete);
    row("keys marked external, none given", &none, vec![("ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL", "true".into())],
        &|_| "encryption keys: ExternalMarkerWithoutKeys".into(), &incomplete);
    row("queue missing", &|d| d.remove(&d.queue()), vec![],
        &|d| format!("the job queue {} is missing, but this install's data is at {}. Put the queue file back, or set QUEUE_DATABASE_PATH to where it \
            is, then start again.", d.queue().display(), d.primary().display()), &missing);
    row("queue in the way", &|d| {
            d.remove(&d.primary());
            rusqlite::Connection::open(d.queue()).unwrap().execute_batch("INSERT INTO solid_queue_processes (kind, last_heartbeat_at, pid, hostname, \
                created_at, name) VALUES ('Worker', '2026-01-01 00:00:00', 1, 'h', '2026-01-01 00:00:00', 'w')").unwrap();
        }, vec![],
        &|d| { let (q, p) = (d.queue().display().to_string(), d.primary().display().to_string());
               format!("can't set up a new install: {q} already has data in it, but {p} doesn't exist. If this is an existing install, restore {p} or \
                   set DATABASE_PATH to it. To start fresh, move {q} out of the way.") }, &missing);
    row("foreign file", &|d| { d.remove(&d.primary()); d.sql("CREATE TABLE notes (x)"); }, vec![],
        &|d| format!("{} is not a Deltabadger database; refusing to touch it", d.primary().display()), &foreign);
    row("unknown migration", &|d| d.sql("INSERT INTO schema_migrations (version) VALUES ('29991231000001')"), vec![],
        &|_| "this data has migrations this build does not know (29991231000001); use a newer build".into(), &foreign);
    row("diverged history", &|d| d.sql("DELETE FROM schema_migrations WHERE version = (SELECT max(version) FROM schema_migrations);
                                        INSERT INTO schema_migrations (version) VALUES ('29991231000001')"), vec![],
        &|_| "this data's migration history differs from this build's; refusing".into(), &foreign);
    // An install Rails owns, two Rust migrations pending, the second failing: backed up, the first applied.
    row("failing migration", &|d| d.sql(&format!("DELETE FROM app_configs WHERE key = '{}'; DROP TABLE estv_crypto_values; DROP TABLE rust_idempotency_orders;
            DELETE FROM rust_schema_migrations WHERE version IN ('20261106000001', '{FAILING}')", deltabadger::lease::KEY)),
        vec![("DELTABADGER_TEST_FAIL_MIGRATION", FAILING.into())],
        &|_| format!("Rust migration {FAILING}_rust_idempotency_orders failed and was rolled back: failed on purpose (DELTABADGER_TEST_FAIL_MIGRATION)\n\
            the migrations before it stay applied; start again once it is fixed"),
        &|d| format!("Deltabadger didn't start: updating your data failed, and the last step was undone. A copy of your data from before the update is \
            in {}. Contact support and send them the files in {}.", d.db().join("backups").display(), log(d)));
    row("DATABASE_URL", &none, vec![("DATABASE_URL", "sqlite3:elsewhere".into())],
        &|_| "DATABASE_URL is set; this build supports only *_DATABASE_PATH".into(),
        &|_| "Deltabadger didn't start: a DATABASE_URL setting on this computer points it elsewhere. Remove it, then open Deltabadger again. Nothing \
            was changed.".into());
    let aside = |phase: &'static str| move |d: &Data| format!("Deltabadger didn't start: it couldn't check your bots before starting them. Nothing was \
        bought or sold.{phase} Quit Deltabadger and open it again. If this keeps happening, contact support and send them the files in {}.", log(d));
    let failed_check = |_: &Data| "Sqlite(InvalidColumnType(0, \"label\", Blob))".to_string();
    row("set-aside failure", &|d| d.sql(MALFORMED_BOT), vec![], &failed_check, &aside(" Nothing was changed."));
    // PLAN-B6 r3.7: an account with no identity key is an encryption reset, committed before the set-aside check.
    row("identity reset, then a set-aside failure", &|d| d.sql(&format!("{MALFORMED_BOT};
            INSERT INTO exchange_accounts (user_id, exchange_id, created_at, updated_at) VALUES (1, (SELECT min(id) FROM exchanges), '2026-01-01', '2026-01-01')")),
        vec![], &failed_check, &aside(""));
    out
}

/// What no desktop line may hold: the operator's terms.
const OPERATOR_TERMS: [&str; 7] = ["SECRET_KEY_BASE", "_PATH", "migration", "{", "Sqlite", "Some(", "Partial"];

/// PLAN-B6 r1.10, r2.4, r3.7, r5.2: each refusal reachable on the desktop, from a crafted data dir, shows its line on the
/// page and nothing else; the operator's text is in the log, never on the page; "Nothing was changed." only while
/// nothing was.
#[cfg(debug_assertions)]
#[test]
fn each_desktop_refusal_shows_its_line_and_logs_the_operators_text() {
    let base = made();
    for (data, row) in rows(&base) {
        let port = free_port();
        let mut c = desktop(&data, port);
        for (k, v) in &row.env { c.env(k, v); }
        let (page, lines) = refused(c, port);
        let name = row.name;
        assert_eq!(page, deltabadger::web::refused::page(&row.line), "{name}");
        for term in OPERATOR_TERMS { assert!(!row.line.contains(term), "{name}: {term} in the line"); }
        assert!(!page.contains(&deltabadger::web::i18n::escape(&row.operator)) && !page.contains(&row.operator), "{name}: the operator's text on the page");
        let log = data.log();
        assert!(log.contains(&format!("deltabadger: {}\ndeltabadger: {}\n", row.line, row.operator)), "{name}: {log}");
        assert!(log.starts_with(&format!("--- start {TOKEN} ---\n")), "{name}: {log}");
        assert!(lines.is_empty(), "{name}: nothing on stdout or stderr: {lines:?}");
    }
}

/// r3.5: of two pending migrations, the first stays applied and the failing second leaves nothing; the backup is made.
#[cfg(debug_assertions)]
#[test]
fn a_failing_migration_leaves_the_one_before_it_applied_and_its_own_work_undone() {
    let base = made();
    let (data, row) = rows(&base).into_iter().find(|(_, r)| r.name == "failing migration").unwrap();
    let port = free_port();
    let mut c = desktop(&data, port);
    for (k, v) in &row.env { c.env(k, v); }
    let (page, _) = refused(c, port);
    assert!(!page.contains("Nothing was changed"), "{page}");
    let c = rusqlite::Connection::open(data.primary()).unwrap();
    let recorded = |v: &str| c.query_row("SELECT count(*) FROM rust_schema_migrations WHERE version = ?1", [v], |r| r.get::<_, i64>(0)).unwrap();
    let table = |t: &str| c.query_row("SELECT count(*) FROM sqlite_master WHERE name = ?1", [t], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!((recorded("20261106000001"), table("estv_crypto_values")), (1, 1), "the first applied");
    assert_eq!((recorded(FAILING), table("rust_idempotency_orders")), (0, 0), "the second rolled back");
    let copies = backups(&data.db());
    assert!(copies.len() == 1 && !partial(&copies[0]), "{copies:?}");
}

/// Docker and Umbrel: the same rows in today's words, byte for byte (the page shows the operator's text, stderr says it).
#[cfg(debug_assertions)]
#[test]
fn the_same_refusals_off_the_desktop_keep_todays_words() {
    let base = made();
    for platform in [None, Some("umbrel"), Some("docker")] {
        for (data, row) in rows(&base) {
            let port = free_port();
            let mut c = command(&data.db(), port);
            if let Some(p) = platform { c.env("DELTABADGER_PLATFORM", p); }
            for (k, v) in &row.env { c.env(k, v); }
            let (page, lines) = refused(c, port);
            let name = row.name;
            assert_eq!(page, deltabadger::web::refused::page(&row.operator), "{platform:?} {name}");
            let stderr: String = lines.iter().filter_map(|l| l.strip_prefix("err: ")).map(|l| format!("{l}\n")).collect();
            assert!(stderr.contains(&format!("deltabadger: {}\n", row.operator)), "{platform:?} {name}: {stderr}");
            assert!(!data.log_file().exists(), "{platform:?} {name}: no log file");
        }
    }
}

/// The desktop's wording needs the folders it names: without DELTABADGER_LOG_FILE, today's words.
#[test]
fn the_desktop_without_its_log_file_keeps_todays_words() {
    let data = Data::new();
    data.sql("CREATE TABLE notes (x)");
    let port = free_port();
    let mut c = desktop(&data, port);
    c.env_remove("DELTABADGER_LOG_FILE");
    let (page, _) = refused(c, port);
    assert_eq!(page, deltabadger::web::refused::page(&format!("{} is not a Deltabadger database; refusing to touch it", data.primary().display())));
}

fn first_line(path: &Path) -> String { std::fs::read_to_string(path).unwrap().lines().next().unwrap_or_default().to_string() }

/// r2.6, r5.3: every line goes to the log file, none to stdout or stderr; each start opens with its marker, and a second
/// start appends.
#[test]
fn the_log_file_takes_every_line_and_each_start_marks_it() {
    let data = Data::new();
    for n in 1..=2 {
        let port = free_port();
        let mut e = start(&mut desktop(&data, port), port);
        assert_eq!(e.ready(), format!("ready {TOKEN} app"));
        e.close_stdin();
        assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
        assert_eq!(e.rest(), Vec::<String>::new(), "start {n}: nothing on stdout or stderr");
        let log = data.log();
        assert_eq!(log.matches(&format!("--- start {TOKEN} ---\n")).count(), n, "{log}");
        for what in ["took over", "running, with the web UI", "stopped on request"] { assert_eq!(log.matches(what).count(), n, "{what}: {log}"); }
    }
    assert_eq!(first_line(&data.log_file()), format!("--- start {TOKEN} ---"));
}

/// r2.6, r5.3: the file is renamed to `.1` once it passes its size during the run (a small one in tests), and every
/// file opened starts with the marker.
#[cfg(debug_assertions)]
#[test]
fn the_log_rotates_during_the_run_and_each_segment_starts_with_the_marker() {
    let data = Data::new();
    let port = free_port();
    let mut c = desktop(&data, port);
    c.env("DELTABADGER_TEST_LOG_ROTATE_BYTES", "200");
    let mut e = start(&mut c, port);
    assert_eq!(e.ready(), format!("ready {TOKEN} app"));
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    let older = data.log_dir().join("deltabadger.log.1");
    for f in [&data.log_file(), &older] { assert_eq!(first_line(f), format!("--- start {TOKEN} ---"), "{}", f.display()); }
    // The last line is in one of the two (it may itself have passed the size, leaving the new file its marker alone).
    let both = format!("{}{}", std::fs::read_to_string(&older).unwrap(), data.log());
    assert!(both.contains("stopped on request"), "{both}");
    let longest = std::fs::read_to_string(&older).unwrap().lines().map(str::len).max().unwrap() as u64 + 1;
    assert!(std::fs::metadata(&older).unwrap().len() <= 200 + longest, "rotated once past the size");
    assert_eq!(std::fs::read_dir(data.log_dir()).unwrap().count(), 2, "the older `.1` replaced");
}

/// r2.6: a log file every write fails on (read-only behind the logger, tests only): lines are dropped, the run and its
/// stop are not, and nothing goes to stdout or stderr instead.
#[cfg(debug_assertions)]
#[test]
fn a_log_that_cannot_be_written_drops_lines_never_the_stop() {
    let data = Data::new();
    let port = free_port();
    let mut c = desktop(&data, port);
    c.env("DELTABADGER_TEST_LOG_READ_ONLY", "1");
    let mut e = start(&mut c, port);
    assert_eq!(e.ready(), format!("ready {TOKEN} app"));
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    assert!(lock_is_free(&data.db()));
    assert_eq!(std::fs::metadata(data.log_file()).unwrap().len(), 0, "every write failed");
    assert_eq!(e.rest(), Vec::<String>::new());
}

/// Without DELTABADGER_LOG_FILE (Docker, Umbrel): the timestamped lines on stdout as before, and no log file.
#[test]
fn without_the_log_variable_lines_stay_on_stdout() {
    let data = Data::new();
    let port = free_port();
    let mut c = desktop(&data, port);
    c.env_remove("DELTABADGER_LOG_FILE");
    let mut e = start(&mut c, port);
    for what in ["set up a new install", "took over", "running, with the web UI"] {
        let l = e.line(what);
        let stamp = l.strip_prefix("out: ").expect("on stdout").split(' ').next().unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(stamp).is_ok() && stamp.len() == 24, "{l}");
    }
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    assert!(e.line("stopped on request").starts_with("out: "));
    assert_eq!(std::fs::read_dir(data.log_dir()).unwrap().count(), 0, "no log file");
}

/// r4.4: a panic's text reaches stderr even when every line goes to the log file (the app keeps stderr for it).
#[cfg(debug_assertions)]
#[test]
fn a_panic_reaches_stderr_with_the_log_file_set() {
    let data = Data::new();
    let port = free_port();
    let mut c = desktop(&data, port);
    c.env("DELTABADGER_TEST_PANIC", "after-handlers");
    let mut e = start(&mut c, port);
    assert_eq!(e.exit_code(Duration::from_secs(60)), Some(101));
    let lines = e.rest();
    assert!(lines.iter().any(|l| l.starts_with("err: ") && l.contains("DELTABADGER_TEST_PANIC at after-handlers")), "{lines:?}");
    assert!(data.log().contains("took over"), "{}", data.log());
    assert!(lock_is_free(&data.db()));
}

/// r4.1: on the desktop, /service-worker.js is the retiring worker, never cached; elsewhere today's worker, byte for byte.
#[test]
fn the_desktop_serves_the_retiring_worker_and_every_other_platform_todays() {
    let today = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../public/service-worker.js")).unwrap();
    for platform in [Some("desktop"), None, Some("umbrel"), Some("docker")] {
        let data = Data::new();
        let port = free_port();
        let mut c = desktop(&data, port);
        match platform { Some(p) => { c.env("DELTABADGER_PLATFORM", p); } None => { c.env_remove("DELTABADGER_PLATFORM"); } }
        let mut e = start(&mut c, port);
        assert_eq!(e.ready(), format!("ready {TOKEN} app"));
        let answer = get(port, "/service-worker.js").unwrap();
        let head = answer.split_once("\r\n\r\n").unwrap().0.to_ascii_lowercase();
        assert!(head.starts_with("http/1.1 200"), "{platform:?}: {head}");
        if platform == Some("desktop") {
            assert_eq!(body(&answer), deltabadger::web::assets::RETIRING_WORKER);
            assert!(head.contains("\r\ncache-control: no-store\r\n"), "{head}");
            for part in ["skipWaiting()", "caches.delete(key)", "self.registration.unregister()", "self.clients.matchAll()", "client.navigate(client.url)"] {
                assert!(body(&answer).contains(part), "{part}");
            }
            assert!(!body(&answer).contains("'fetch'"), "no fetch handler");
        } else {
            assert_eq!(body(&answer).as_bytes(), &today[..], "{platform:?}");
            assert!(head.contains("\r\ncache-control: public, max-age=31536000\r\n"), "{platform:?}: {head}");
        }
        e.close_stdin();
        assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    }
}

/// `check` is the operator's: today's words under DELTABADGER_PLATFORM=desktop too.
#[test]
fn check_keeps_todays_words_on_the_desktop() {
    let data = made();
    let out = Command::new(env!("CARGO_BIN_EXE_deltabadger")).arg("check").env("STORAGE_DIR", data.db()).env("SECRET_KEY_BASE", SECRET)
        .env("DELTABADGER_PLATFORM", "desktop").env("DELTABADGER_LOG_FILE", data.log_file())
        .env("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", "another-primary-key").env("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", "another-salt")
        .env_remove("DATABASE_URL").env_remove("ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL").stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "deltabadger: can't read this install with this SECRET_KEY_BASE. Use the secret this install was created with.\n");
}
