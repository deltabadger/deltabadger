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

/// Codex r1: a full log file that can't be renamed (a folder named `<log>.1` is in the way) takes no more lines; the run
/// and its stop go on.
#[cfg(debug_assertions)]
#[test]
fn a_full_log_that_cannot_be_rotated_stops_growing() {
    let data = Data::new();
    let older = data.log_dir().join("deltabadger.log.1");
    std::fs::create_dir(&older).unwrap();
    std::fs::write(older.join("kept"), "").unwrap();
    let port = free_port();
    let mut c = desktop(&data, port);
    c.env("DELTABADGER_TEST_LOG_ROTATE_BYTES", "200");
    let mut e = start(&mut c, port);
    assert_eq!(e.ready(), format!("ready {TOKEN} app"));
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    let log = data.log();
    let longest = log.lines().map(str::len).max().unwrap() + 1;
    assert!(log.len() <= 200 + longest, "{} bytes: {log}", log.len());
    assert_eq!(log.matches("--- start").count(), 1, "{log}");
    assert!(older.join("kept").exists());
    assert_eq!(e.rest(), Vec::<String>::new());
}

/// Codex r1: a log file that can't be opened drops every line; nothing goes to stdout or stderr instead.
#[test]
fn a_log_file_that_cannot_be_opened_drops_lines_and_writes_nothing_else() {
    let data = Data::new();
    let port = free_port();
    let mut c = desktop(&data, port);
    c.env("DELTABADGER_LOG_FILE", data.path().join("no-such-folder").join("deltabadger.log"));
    let mut e = start(&mut c, port);
    assert_eq!(e.ready(), format!("ready {TOKEN} app"));
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    assert_eq!(e.rest(), Vec::<String>::new());
    assert!(!data.path().join("no-such-folder").exists());
}

/// Codex r1 (r4.1): the refusal page answers /service-worker.js with the retiring worker on the desktop, so a reopen into
/// a refusal removes the old worker; elsewhere it stays the refusal page.
#[test]
fn the_refusal_page_serves_the_retiring_worker_on_the_desktop_only() {
    for desktop_platform in [true, false] {
        let data = Data::new();
        data.sql("CREATE TABLE notes (x)");
        let port = free_port();
        let mut c = desktop(&data, port);
        if !desktop_platform { c.env_remove("DELTABADGER_PLATFORM"); }
        let mut e = start(&mut c, port);
        assert_eq!(e.ready(), format!("ready {TOKEN} refused"));
        let answer = get(port, "/service-worker.js").unwrap();
        let head = answer.split_once("\r\n\r\n").unwrap().0.to_ascii_lowercase();
        if desktop_platform {
            assert!(head.starts_with("http/1.1 200") && head.contains("\r\ncache-control: no-store\r\n"), "{head}");
            assert_eq!(body(&answer), deltabadger::web::assets::RETIRING_WORKER);
        } else {
            assert!(head.starts_with("http/1.1 503"), "{head}");
            assert_eq!(body(&answer), body(&get(port, "/").unwrap()), "the refusal page, as before");
        }
        e.close_stdin();
        assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    }
}

// ---- B7-0: the copy before a desktop update migrates, the installed app's update lock, two more barriers ----

/// The two newest Rust migrations and how to make each pending again on a made install (its table gone, its row
/// with it), as on an install from the build before it. The newest migration must be here, or these tests would stop
/// testing "an update brings a migration" the day one is added.
const UNDO: [(&str, &str); 2] = [("20261107000001", "DROP TABLE rust_idempotency_orders"), ("20261108000001", "DROP TABLE rust_rails_unrecorded")];

#[test]
fn the_undo_table_covers_the_newest_migration() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let newest = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).max().unwrap();
    assert!(newest.starts_with(UNDO[1].0), "add the newest migration ({newest}) to UNDO, with how to undo it");
}

/// `data` as the build before the newest `n` migrations left it.
fn pending(data: &Data, n: usize) {
    for (version, undo) in &UNDO[UNDO.len() - n..] {
        data.sql(&format!("{undo}; DELETE FROM rust_schema_migrations WHERE version = '{version}'"));
    }
}

fn recorded(db: &Path, version: &str) -> bool {
    rusqlite::Connection::open(db.join("production.sqlite3")).unwrap()
        .query_row("SELECT count(*) FROM rust_schema_migrations WHERE version = ?1", [version], |r| r.get::<_, i64>(0)).unwrap() == 1
}

fn copies(db: &Path, prefix: &str) -> Vec<PathBuf> {
    backups(db).into_iter().filter(|b| b.file_name().unwrap().to_string_lossy().starts_with(prefix)).collect()
}

/// Starts `c`, waits for the app, stops it by stdin's end.
fn serve_once(mut c: Command, port: u16) {
    let mut e = start(&mut c, port);
    assert_eq!(e.ready(), format!("ready {TOKEN} app"));
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
}

/// PLAN-B7 §4, r2.5: a desktop install Rust already owns, with a migration this build brings, is copied once before
/// the migration; the copy holds the data as it was before it; a start with nothing to migrate copies nothing.
#[test]
fn a_desktop_update_that_migrates_copies_the_data_first_and_only_then() {
    let data = made();
    assert!(backups(&data.db()).is_empty(), "a new install has nothing to copy");
    pending(&data, 1);
    let port = free_port();
    serve_once(desktop(&data, port), port);
    let made = copies(&data.db(), deltabadger::store::UPDATE_COPY);
    assert_eq!(made.len(), 1, "{:?}", backups(&data.db()));
    assert!(copies(&data.db(), deltabadger::store::TAKEOVER_COPY).is_empty());
    assert!(!partial(&made[0]) && made[0].join("digest").exists());
    assert!(!recorded(&made[0], UNDO[1].0), "the copy is from before the migration");
    assert!(recorded(&data.db(), UNDO[1].0), "then migrated");
    assert!(data.log().contains("Saved a copy of your data to"), "{}", data.log());
    serve_once(desktop(&data, port), port);
    assert_eq!(backups(&data.db()), made, "nothing to migrate: no copy");
}

/// Off the desktop (Docker, Umbrel, hosted) an update migrates as before: no copy (PLAN-B7 Q4).
#[test]
fn off_the_desktop_an_update_makes_no_copy() {
    let data = made();
    pending(&data, 1);
    let port = free_port();
    serve_once(command(&data.db(), port), port);
    assert!(recorded(&data.db(), UNDO[1].0));
    assert!(backups(&data.db()).is_empty(), "{:?}", backups(&data.db()));
}

/// A takeover from Rails takes its own copy, and only that one, though it migrates too.
#[test]
fn a_takeover_on_the_desktop_takes_only_its_own_copy() {
    let dir = rails_owned_install();
    let port = free_port();
    let mut c = command(dir.path(), port);
    c.env("DELTABADGER_PLATFORM", "desktop");
    serve_once(c, port);
    assert_eq!(copies(dir.path(), deltabadger::store::TAKEOVER_COPY).len(), 1, "{:?}", backups(dir.path()));
    assert!(copies(dir.path(), deltabadger::store::UPDATE_COPY).is_empty(), "{:?}", backups(dir.path()));
}

/// An update whose second migration fails on every start: the copy from before the first attempt stays, whatever the
/// retries add; a retry on unchanged data adds nothing (PLAN-B7 r2.5).
#[cfg(debug_assertions)]
#[test]
fn a_failing_update_keeps_the_copy_from_before_its_first_attempt() {
    let data = made();
    pending(&data, 2);
    let port = free_port();
    for _ in 0..3 {
        let mut c = desktop(&data, port);
        c.env("DELTABADGER_TEST_FAIL_MIGRATION", UNDO[1].0);
        let (page, _) = refused(c, port);
        assert!(page.contains("updating your data failed"), "{page}");
        assert!(page.contains("A copy of your data from before the update is in"), "{page}");
    }
    let made = copies(&data.db(), deltabadger::store::UPDATE_COPY);
    assert_eq!(made.len(), 2, "attempt 1 copied, attempt 2 copied what attempt 1 changed, attempt 3 changed nothing: {made:?}");
    assert!(!recorded(&made[0], UNDO[0].0) && !recorded(&made[0], UNDO[1].0), "the first copy holds the data before the update");
    assert!(recorded(&made[1], UNDO[0].0) && !recorded(&made[1], UNDO[1].0));
}

/// A copy that can't be made stops the start before anything is migrated (C5b, as before a takeover).
#[test]
fn an_update_whose_copy_fails_is_refused_before_it_migrates() {
    let data = made();
    pending(&data, 1);
    std::fs::write(data.db().join("backups"), "").unwrap(); // a file where the copies' folder goes
    let port = free_port();
    let (page, _) = refused(desktop(&data, port), port);
    assert!(page.contains("save a copy of your data to"), "{page}");
    assert!(!recorded(&data.db(), UNDO[1].0), "not migrated");
}

/// A crash in the middle of the copy: the next start makes it complete, with no `.partial` left.
#[cfg(debug_assertions)]
#[test]
fn an_update_copy_cut_short_is_made_again_at_the_next_start() {
    let data = made();
    pending(&data, 1);
    let port = free_port();
    let mut c = desktop(&data, port);
    c.env("DELTABADGER_TEST_ABORT", "mid-backup");
    let mut e = start(&mut c, port);
    assert_ne!(e.exit_code(Duration::from_secs(60)), Some(0), "aborted");
    assert!(!recorded(&data.db(), UNDO[1].0));
    serve_once(desktop(&data, port), port);
    let all = backups(&data.db());
    assert!(all.len() == 1 && !partial(&all[0]) && all[0].join("digest").exists(), "{all:?}");
}

/// PLAN-B7 r4.7: a stop that arrives in a tick that has begun waits for that tick; the engine then stops on request.
#[cfg(debug_assertions)]
#[test]
fn a_stop_during_a_tick_waits_for_the_tick() {
    let data = made();
    let release = data.path().join("release");
    let port = free_port();
    // Without the log file: the barrier's line is on stdout.
    let mut c = command(&data.db(), port);
    c.env("DELTABADGER_TEST_PAUSE", "in-tick").env("DELTABADGER_TEST_RELEASE", &release);
    let mut e = start(&mut c, port);
    e.line("paused at in-tick");
    e.close_stdin();
    std::thread::sleep(Duration::from_secs(2));
    assert!(e.child.try_wait().unwrap().is_none(), "the tick in hand is not cut short");
    std::fs::write(&release, "").unwrap();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    e.line("stopped on request");
}

/// PLAN-B7 r4.7: after the engine has stopped, the port is free while the services still drain.
#[cfg(debug_assertions)]
#[test]
fn the_port_is_free_while_the_services_drain() {
    let data = made();
    let release = data.path().join("release");
    let port = free_port();
    // Without the log file: the barrier's line is on stdout.
    let mut c = command(&data.db(), port);
    c.env("DELTABADGER_TEST_PAUSE", "after-listener-drop").env("DELTABADGER_TEST_RELEASE", &release);
    let mut e = start(&mut c, port);
    assert_eq!(e.ready(), format!("ready {TOKEN} app"));
    e.close_stdin();
    e.line("paused at after-listener-drop");
    assert!(get(port, "/ui-ready").is_none(), "nothing answers on the port");
    assert!(e.child.try_wait().unwrap().is_none(), "still draining");
    std::fs::write(&release, "").unwrap();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
}

/// The engine copied into an installed app's layout (PLAN-B7 r4.1): macOS `<x>.app/Contents/MacOS/`, Windows a folder
/// with `Deltabadger.exe`, Linux an AppImage's mount (`APPDIR`, with `APPIMAGE` naming the file).
struct App { dir: tempfile::TempDir, exe: PathBuf, identity: PathBuf, env: Vec<(&'static str, PathBuf)> }
impl App {
    fn new() -> Self { Self::with_engine("deltabadger-engine") }
    /// The same layout with the engine file named `name`.
    fn with_engine(name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (exe, identity, env) = if cfg!(target_os = "macos") {
            let app = root.join("Deltabadger.app");
            (app.join("Contents/MacOS").join(name), app, vec![])
        } else if cfg!(windows) {
            let app = root.join("Deltabadger");
            std::fs::create_dir_all(&app).unwrap();
            std::fs::write(app.join("Deltabadger.exe"), "").unwrap();
            (app.join(format!("{name}.exe")), app, vec![])
        } else {
            let mount = root.join("mount");
            let image = root.join("Deltabadger.AppImage");
            std::fs::write(&image, "").unwrap();
            (mount.join("usr/bin").join(name), image.clone(), vec![("APPDIR", mount), ("APPIMAGE", image)])
        };
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        // A link where the file system allows one (the debug engine is large), else a copy.
        std::fs::hard_link(env!("CARGO_BIN_EXE_deltabadger"), &exe).or_else(|_| std::fs::copy(env!("CARGO_BIN_EXE_deltabadger"), &exe).map(|_| ())).unwrap();
        let locks = root.join("locks");
        let mut env = env;
        env.push(("DELTABADGER_TEST_BUNDLE_LOCK_DIR", locks));
        App { dir, exe, identity, env }
    }
    fn locks(&self) -> PathBuf { self.dir.path().canonicalize().unwrap().join("locks") }
    fn lock_file(&self) -> PathBuf {
        let env: Vec<(String, String)> = self.env.iter().map(|(k, v)| (k.to_string(), v.display().to_string())).collect();
        deltabadger::bundle::lock_file(&self.identity, &|k| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())).unwrap()
    }
    /// `serve` on `data` as the desktop app starts it, from this app.
    fn serve(&self, data: &Data, port: u16) -> Command {
        let c = desktop(data, port);
        let program = c.get_program().to_owned();
        assert_eq!(Path::new(&program), Path::new(env!("CARGO_BIN_EXE_deltabadger")));
        let mut from_app = Command::new(&self.exe);
        from_app.args(c.get_args());
        for (k, v) in c.get_envs() { match v { Some(v) => { from_app.env(k, v); } None => { from_app.env_remove(k); } } }
        for (k, v) in &self.env { from_app.env(k, v); }
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut from_app, 0x0800_0000);
        from_app
    }
}

fn exclusive(lock: &Path) -> std::io::Result<std::fs::File> {
    let f = deltabadger::bundle::open(lock)?;
    f.try_lock().map_err(|e| match e { std::fs::TryLockError::Error(e) => e, std::fs::TryLockError::WouldBlock => std::io::Error::other("held") })?;
    Ok(f)
}

/// PLAN-B7 r3.2: an engine of an installed app holds the app's lock while it runs, so an update can't take it.
#[test]
fn an_engine_of_an_installed_app_holds_its_update_lock_while_it_runs() {
    let app = App::new();
    let data = Data::new();
    let port = free_port();
    let mut e = start(&mut app.serve(&data, port), port);
    assert_eq!(e.ready(), format!("ready {TOKEN} app"));
    assert!(exclusive(&app.lock_file()).is_err(), "the update can't take the lock while the engine runs");
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    assert!(exclusive(&app.lock_file()).is_ok(), "free once it stopped");
}

/// PLAN-B7 r3.2: while an update holds the lock, an engine of that app doesn't start; it says why and exits.
#[test]
fn an_engine_does_not_start_while_its_app_is_being_updated() {
    let app = App::new();
    let _update = exclusive(&app.lock_file()).unwrap();
    let data = Data::new();
    let port = free_port();
    let mut e = start(&mut app.serve(&data, port), port);
    assert_eq!(e.exit_code(Duration::from_secs(10)), Some(1));
    assert!(e.rest().iter().any(|l| l.contains(deltabadger::bundle::BEING_UPDATED)));
    assert!(!data.primary().exists(), "nothing was created");
}

/// Outside an installed app (Docker, a .deb, a build tree) there is no update lock.
#[test]
fn an_engine_outside_an_installed_app_takes_no_update_lock() {
    let app = App::new(); // only for its lock folder
    let data = Data::new();
    let port = free_port();
    let mut c = desktop(&data, port);
    c.env("DELTABADGER_TEST_BUNDLE_LOCK_DIR", app.locks());
    serve_once(c, port);
    assert!(!app.locks().exists(), "no lock folder made");
}

/// PLAN-B7 r3.3: on Windows the installer's mark keeps every engine of the app from starting during the copy.
#[cfg(windows)]
#[test]
fn an_engine_does_not_start_while_the_installer_copies_its_app() {
    let app = App::new();
    std::fs::write(app.identity.join(".updating"), "").unwrap();
    let data = Data::new();
    let port = free_port();
    let mut e = start(&mut app.serve(&data, port), port);
    assert_eq!(e.exit_code(Duration::from_secs(10)), Some(1));
    assert!(e.rest().iter().any(|l| l.contains(deltabadger::bundle::BEING_UPDATED)));
}

/// PLAN-B7 r5.3: whoever creates the lock folder and file makes them usable by every account, whatever its umask.
#[cfg(unix)]
#[test]
fn the_update_lock_is_made_for_every_account_whatever_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    let app = App::new();
    let data = Data::new();
    let port = free_port();
    let c = app.serve(&data, port);
    let mut sh = Command::new("sh");
    sh.arg("-c").arg("umask 077; exec \"$0\" \"$@\"").arg(c.get_program()).args(c.get_args());
    for (k, v) in c.get_envs() { match v { Some(v) => { sh.env(k, v); } None => { sh.env_remove(k); } } }
    serve_once(sh, port);
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode(&app.locks()), 0o1777);
    assert_eq!(mode(&app.lock_file()), 0o666);
}

/// PLAN-B7 r5.3: a lock file the engine can't open (left unusable) never stops it: it runs without the lock and says so.
#[cfg(unix)]
#[test]
fn an_unusable_update_lock_never_stops_the_engine() {
    use std::os::unix::fs::PermissionsExt;
    let app = App::new();
    std::fs::create_dir_all(app.locks()).unwrap();
    std::fs::write(app.lock_file(), "").unwrap();
    std::fs::set_permissions(app.lock_file(), std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(app.lock_file()).is_ok() { return } // root reads anything: nothing to test
    let data = Data::new();
    let port = free_port();
    let mut e = start(&mut app.serve(&data, port), port);
    assert_eq!(e.ready(), format!("ready {TOKEN} app"));
    e.close_stdin();
    assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
    assert!(e.rest().iter().chain(std::iter::once(&data.log())).any(|l| l.contains("running without it")), "said so");
}

/// Codex B7-0 P2-1: only the app's engine takes the app's lock. On Windows the engine built on its own is
/// `deltabadger.exe`, the app's launcher name on a case-insensitive file system; it is not the app's engine.
#[test]
fn an_engine_not_named_as_the_apps_engine_takes_no_update_lock() {
    let app = App::with_engine("deltabadger");
    let _update = exclusive(&app.lock_file()).unwrap(); // would stop the app's own engine
    let data = Data::new();
    let port = free_port();
    serve_once(app.serve(&data, port), port);
}

/// Codex B7-0 P2-2: a lock file or folder that is a symlink is never followed; the engine runs without the lock.
#[cfg(unix)]
#[test]
fn a_symlinked_update_lock_is_not_followed() {
    for folder in [false, true] {
        let app = App::new();
        let elsewhere = app.dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        if folder {
            std::os::unix::fs::symlink(&elsewhere, app.locks()).unwrap();
        } else {
            std::fs::create_dir_all(app.locks()).unwrap();
            std::fs::write(elsewhere.join("target"), "").unwrap();
            std::os::unix::fs::symlink(elsewhere.join("target"), app.lock_file()).unwrap();
        }
        let data = Data::new();
        let port = free_port();
        let mut e = start(&mut app.serve(&data, port), port);
        assert_eq!(e.ready(), format!("ready {TOKEN} app"), "folder: {folder}");
        e.close_stdin();
        assert_eq!(e.exit_code(Duration::from_secs(30)), Some(0));
        assert!(e.rest().iter().any(|l| l.contains("running without it")), "folder: {folder}");
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), usize::from(!folder), "nothing made through the link (folder: {folder})");
    }
}

/// Codex B7-0 P2-3: an engine that waited while its app was replaced doesn't run the old app's code beside the new one.
#[cfg(unix)]
#[test]
fn an_engine_that_waited_out_a_replacement_of_its_app_stops() {
    let app = App::new();
    let update = exclusive(&app.lock_file()).unwrap();
    let data = Data::new();
    let port = free_port();
    let mut e = start(&mut app.serve(&data, port), port);
    std::thread::sleep(Duration::from_millis(500)); // the engine is waiting for its share
    // The update's one step: the installed app's path now names another file or folder.
    let old = app.identity.with_extension("old");
    std::fs::rename(&app.identity, &old).unwrap();
    if cfg!(target_os = "macos") { std::fs::create_dir(&app.identity).unwrap() } else { std::fs::write(&app.identity, "").unwrap() }
    drop(update);
    assert_eq!(e.exit_code(Duration::from_secs(10)), Some(1));
    assert!(e.rest().iter().any(|l| l.contains(deltabadger::bundle::BEING_UPDATED)));
    assert!(!data.primary().exists(), "nothing was created");
}
