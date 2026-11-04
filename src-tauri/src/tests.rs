//! The launcher without a window (PLAN-B6 §9 B6-1): the supervisor runs with a recording view, against the real
//! sidecar (binaries/deltabadger-engine-<triple>, which tauri-build requires anyway) or a stub engine. The stub is this
//! test binary run again on `stub_engine` with a script in DELTABADGER_STUB, so every case runs on every OS.
use super::*;
use crate::secrets::hooks;
use crate::supervisor::{lock_held, supervise, this_start};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn temporary_directory(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock should be after the Unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("deltabadger-{label}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// What the window was asked to show, in order: `status:<fragment>`, `open:<url>`, `back`.
#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl View for Recorder {
    fn status(&self, fragment: &str) { self.0.lock().unwrap().push(format!("status:{fragment}")) }
    fn open(&self, url: &str) { self.0.lock().unwrap().push(format!("open:{url}")) }
    fn back(&self) { self.0.lock().unwrap().push("back".into()) }
}

impl Recorder {
    fn events(&self) -> Vec<String> { self.0.lock().unwrap().clone() }
    fn last(&self) -> String { self.events().last().cloned().unwrap_or_default() }
    fn states(&self) -> Vec<String> {
        self.events()
            .iter()
            .map(|e| e.strip_prefix("status:").map_or(e.split(':').next().unwrap().to_string(), |f| f.split('&').next().unwrap().to_string()))
            .collect()
    }
    fn wait_for(&self, what: &str, within: Duration) {
        let deadline = Instant::now() + within;
        while !self.events().iter().any(|e| e.contains(what)) {
            assert!(Instant::now() < deadline, "no {what:?} within {within:?}: {:?}", self.events());
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn free_port() -> u16 { TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() }

/// A launch of the stub engine with `script` on a fresh data dir.
fn stub(label: &str, script: &str) -> Launch {
    let data = temporary_directory(label);
    let script_file = data.join("stub-script");
    fs::write(&script_file, script).unwrap();
    Launch {
        exe: std::env::current_exe().unwrap(),
        // `serve` matches no test; with --exact only the stub runs.
        extra_args: vec!["tests::stub_engine".into(), "--exact".into(), "--nocapture".into(), "--test-threads=1".into()],
        cwd: data.clone(),
        data: data.join("data"),
        notes: Vec::new(),
        first_port: Some(free_port()),
    }
}

fn sidecar() -> PathBuf {
    let exe = format!("deltabadger-engine-{}{}", env!("DESKTOP_TARGET_TRIPLE"), std::env::consts::EXE_SUFFIX);
    Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries").join(exe)
}

fn engine(label: &str) -> Launch {
    let data = temporary_directory(label);
    Launch { exe: sidecar(), extra_args: Vec::new(), cwd: data.clone(), data: data.join("data"), notes: Vec::new(), first_port: Some(free_port()) }
}

fn log(launch: &Launch) -> String { fs::read_to_string(launch.log_file()).unwrap_or_default() }

fn starts(launch: &Launch) -> usize { log(launch).lines().filter(|l| l.starts_with("desktop: the engine started")).count() }

fn details(event: &str) -> String {
    let encoded = event.split("&details=").nth(1).unwrap_or("");
    let bytes = encoded.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(u8::from_str_radix(&encoded[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

/// The stub engine, when run with DELTABADGER_STUB_SCRIPT naming a file of its steps (`;`-separated; a no-op test otherwise):
/// `serve:<state>:<ms>` answers `/ui-ready` with this start's token (exits 1 if the port can't be bound, as `serve`
/// does); `slow:<state>:<ms>:<delay ms>` the same with the first answer delayed; `token:<token>` answers with another
/// token from then on; `gap:<ms>` nothing listening; `say:<text>` a line on stderr; `exit:<code>`; `panic`; `stdin`
/// waits for stdin's end, then exits 0.
#[test]
fn stub_engine() {
    // Only in a child the supervisor started: it alone has a ready token (the script variable is process-wide).
    let (Ok(script), Ok(_)) = (std::env::var("DELTABADGER_STUB_SCRIPT"), std::env::var("DELTABADGER_READY_TOKEN")) else { return };
    let script = fs::read_to_string(script).unwrap();
    let port: u16 = std::env::var("PORT").unwrap().parse().unwrap();
    let mut token = std::env::var("DELTABADGER_READY_TOKEN").unwrap();
    let serve = |body: &str, ms: u64, delay_first: u64| {
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|e| {
            eprintln!("stub: cannot bind port {port}: {e}");
            std::process::exit(1)
        });
        listener.set_nonblocking(true).unwrap();
        let (end, mut delay) = (Instant::now() + Duration::from_millis(ms), delay_first);
        while Instant::now() < end {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request);
                    std::thread::sleep(Duration::from_millis(std::mem::take(&mut delay)));
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    };
    for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let args: Vec<&str> = step.split(':').collect();
        let number = |i: usize| args[i].parse::<u64>().unwrap();
        match args[0] {
            "serve" => serve(&format!("ready {token} {}", args[1]), number(2), 0),
            "slow" => serve(&format!("ready {token} {}", args[1]), number(2), number(3)),
            "token" => token = args[1].to_string(),
            "gap" => std::thread::sleep(Duration::from_millis(number(1))),
            "say" => eprintln!("{}", args[1..].join(":")),
            "exit" => std::process::exit(number(1) as i32),
            "panic" => panic!("stub engine panicked"),
            "stdin" => {
                let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
                std::process::exit(0)
            }
            other => panic!("unknown stub step {other}"),
        }
    }
    std::process::exit(0)
}

/// `supervise` with the stub running `script`; returns when the supervisor does.
fn run_stub(label: &str, script: &str) -> (Launch, Recorder) {
    let launch = stub(label, script);
    let view = Recorder::default();
    with_stub_script(&launch, || supervise(&launch, &Engine::default(), &view));
    (launch, view)
}

/// The stub reads its script from the file named by DELTABADGER_STUB_SCRIPT, which every child inherits; the tests that
/// set it take turns.
static STUB_ENV: Mutex<()> = Mutex::new(());

fn with_stub_script<T>(launch: &Launch, f: impl FnOnce() -> T) -> T {
    let _turn = STUB_ENV.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("DELTABADGER_STUB_SCRIPT", launch.cwd.join("stub-script"));
    let out = f();
    std::env::remove_var("DELTABADGER_STUB_SCRIPT");
    out
}

// ---- §1.2: the environment ----

#[test]
fn the_engine_gets_exactly_the_desktop_environment() {
    let launch = engine("env");
    let keys = HashMap::from([
        ("SECRET_KEY_BASE".to_string(), "k".to_string()),
        ("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY".to_string(), "p".to_string()),
        ("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT".to_string(), "s".to_string()),
    ]);
    let env = launch.engine_env(3007, "tok", &keys);
    let data = |p: &str| launch.data.join(p).to_string_lossy().into_owned();
    let expected: HashMap<String, String> = [
        ("PORT", "3007".to_string()),
        ("BINDING", "127.0.0.1".to_string()),
        ("DELTABADGER_PLATFORM", "desktop".to_string()),
        ("APP_ROOT_URL", "http://127.0.0.1:3007".to_string()),
        ("STORAGE_DIR", data("db")),
        ("DATABASE_PATH", launch.data.join("db").join("production.sqlite3").to_string_lossy().into_owned()),
        ("QUEUE_DATABASE_PATH", launch.data.join("db").join("production_queue.sqlite3").to_string_lossy().into_owned()),
        ("APP_TMP_DIR", data("tmp")),
        ("DELTABADGER_STOP_ON_STDIN_EOF", "1".to_string()),
        ("DELTABADGER_READY_TOKEN", "tok".to_string()),
        ("DELTABADGER_LOG_FILE", launch.data.join("log").join("deltabadger.log").to_string_lossy().into_owned()),
        ("SECRET_KEY_BASE", "k".to_string()),
        ("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", "p".to_string()),
        ("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", "s".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(env, expected);
    // None of the Rails rows (§1.2 D).
    for gone in ["RAILS_ENV", "PIDFILE", "BOOTSNAP_CACHE_DIR", "CACHE_DATABASE_PATH", "CABLE_DATABASE_PATH", "BUNDLE_GEMFILE", "SOLID_QUEUE_IN_PUMA"] {
        assert!(!env.contains_key(gone), "{gone}");
    }
}

// ---- r2.2, r4.3: the data dir ----

#[test]
fn only_the_released_identifier_runs_on_the_production_data_dir() {
    let tauri_dir = |id: &str| Ok(PathBuf::from(format!("/apps/{id}")));
    // Released: the 2.x dir, the override ignored and said so.
    let (dir, notes) = choose_data_dir(RELEASE_IDENTIFIER, Some("/elsewhere"), false, tauri_dir(RELEASE_IDENTIFIER)).unwrap();
    if cfg!(windows) {
        assert!(dir.ends_with("Deltabadger"), "{dir:?}");
    } else {
        assert_eq!(dir, PathBuf::from("/apps/com.deltabadger.app"));
    }
    assert_eq!(notes, vec!["desktop: DELTABADGER_DESKTOP_DATA_DIR is ignored by the released app"]);
    // A test build: the override, else its own identifier's dir; never the production one.
    let test = "com.deltabadger.app.test";
    assert_eq!(choose_data_dir(test, Some("/scenario/x"), false, tauri_dir(test)).unwrap().0, PathBuf::from("/scenario/x"));
    let (own, _) = choose_data_dir(test, None, false, tauri_dir(test)).unwrap();
    assert_eq!(own, PathBuf::from("/apps/com.deltabadger.app.test"));
    assert!(!own.ends_with("Deltabadger"));
    // A relative override is refused rather than read against some working directory.
    assert!(choose_data_dir(test, Some("relative"), false, tauri_dir(test)).is_err());
    // Debug builds: the repository's storage/desktop-dev, whatever the identifier.
    let (dev, _) = choose_data_dir(RELEASE_IDENTIFIER, None, true, tauri_dir(RELEASE_IDENTIFIER)).unwrap();
    assert!(dev.ends_with(Path::new("storage").join("desktop-dev")), "{dev:?}");
}

#[test]
fn a_test_build_never_touches_the_production_dir() {
    // Sentinels where the released app keeps its data; the test identifier's run leaves them as they were.
    let root = temporary_directory("sentinels");
    let production = root.join("com.deltabadger.app");
    let sentinel = production.join("db").join("production.sqlite3");
    fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    fs::write(&sentinel, b"sentinel").unwrap();
    let before = fs::metadata(&sentinel).unwrap().modified().unwrap();
    let (data, _) = choose_data_dir("com.deltabadger.app.test", Some(root.join("scenario").to_str().unwrap()), false, Ok(production.clone())).unwrap();
    let mut launch = stub("sentinel-run", "exit:0");
    launch.data = data;
    let view = Recorder::default();
    with_stub_script(&launch, || supervise(&launch, &Engine::default(), &view));
    assert!(launch.data.join(".secrets").is_file());
    assert_eq!(fs::read(&sentinel).unwrap(), b"sentinel");
    assert_eq!(fs::metadata(&sentinel).unwrap().modified().unwrap(), before);
    assert!(!production.join("db").join(".engine.lock").exists());
    assert!(!production.join(".secrets").exists());
    assert_eq!(fs::read_dir(production.join("db")).unwrap().count(), 1);
}

// ---- r1.8: the lock probe ----

#[test]
fn the_lock_probe_sees_another_holder_and_releases_its_own() {
    let dir = temporary_directory("lock");
    let lock = dir.join(".engine.lock");
    assert!(!lock_held(&lock).unwrap(), "no file, no engine");
    assert!(!lock.exists(), "the probe creates nothing");
    let holder = fs::OpenOptions::new().write(true).create(true).truncate(false).open(&lock).unwrap();
    assert!(!lock_held(&lock).unwrap());
    // The probe let go: the holder can take it.
    holder.try_lock().unwrap();
    assert!(lock_held(&lock).unwrap());
    holder.unlock().unwrap();
    assert!(!lock_held(&lock).unwrap());
}

#[test]
fn a_held_lock_shows_d2_then_the_engine_starts_once_it_is_free() {
    let launch = stub("d2", "serve:app:1500;exit:0");
    fs::create_dir_all(launch.db()).unwrap();
    let holder = fs::OpenOptions::new().write(true).create(true).truncate(false).open(launch.lock_file()).unwrap();
    holder.try_lock().unwrap();
    let view = Arc::new(Recorder::default());
    let engine = Arc::new(Engine::default());
    with_stub_script(&launch, || {
        std::thread::scope(|s| {
            let (v, e) = (view.clone(), engine.clone());
            let l = &launch;
            s.spawn(move || supervise(l, &e, &*v));
            view.wait_for("status:finishing", Duration::from_secs(10));
            std::thread::sleep(Duration::from_secs(3));
            assert_eq!(starts(&launch), 0, "nothing spawned while the lock is held");
            holder.unlock().unwrap();
        })
    });
    assert_eq!(view.states(), ["finishing", "starting", "open", "stopped"]);
}

#[test]
fn a_quit_while_the_lock_is_held_starts_nothing() {
    let launch = stub("d2-quit", "exit:0");
    fs::create_dir_all(launch.db()).unwrap();
    let holder = fs::OpenOptions::new().write(true).create(true).truncate(false).open(launch.lock_file()).unwrap();
    holder.try_lock().unwrap();
    let view = Arc::new(Recorder::default());
    let engine = Arc::new(Engine::default());
    std::thread::scope(|s| {
        let (v, e) = (view.clone(), engine.clone());
        let l = &launch;
        let supervisor = s.spawn(move || supervise(l, &e, &*v));
        view.wait_for("status:finishing", Duration::from_secs(10));
        assert!(matches!(stop_engine(&engine), supervisor::Stopped::NotRunning));
        supervisor.join().unwrap();
    });
    assert_eq!(starts(&launch), 0);
    assert_eq!(view.states(), ["finishing"]);
}

#[test]
fn an_exit_1_while_another_engine_took_the_lock_goes_back_to_waiting() {
    // The engine lost the race for the lock (it exits 1, Locked): the window waits (D2) instead of failing; once the
    // lock is free the engine starts again.
    let launch = stub("locked", "gap:1500;exit:1");
    let view = Arc::new(Recorder::default());
    let engine = Arc::new(Engine::default());
    with_stub_script(&launch, || {
        std::thread::scope(|s| {
            let (v, e) = (view.clone(), engine.clone());
            let l = &launch;
            s.spawn(move || supervise(l, &e, &*v));
            let deadline = Instant::now() + Duration::from_secs(10);
            while engine.pid().is_none() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            }
            let holder = fs::OpenOptions::new().write(true).create(true).truncate(false).open(launch.lock_file()).unwrap();
            holder.try_lock().unwrap();
            view.wait_for("status:finishing", Duration::from_secs(10));
            holder.unlock().unwrap();
        })
    });
    assert_eq!(starts(&launch), 2);
    assert_eq!(view.states(), ["starting", "finishing", "starting", "not-started"]);
}

// ---- r1.4: only this start's engine ----

#[test]
fn an_answer_without_this_starts_token_is_never_shown() {
    let (launch, view) = run_stub("token", "token:0123456789abcdef0123456789abcdef;serve:app:2500;exit:3");
    assert!(!view.events().iter().any(|e| e.starts_with("open:")), "{:?}", view.events());
    assert!(view.last().starts_with("status:not-started"), "{}", view.last());
    assert_eq!(starts(&launch), 1);
}

#[test]
fn a_port_taken_by_someone_else_is_never_shown_and_the_next_port_is_tried_once() {
    // Something takes the chosen port between the probe and the bind and answers as an engine would; the engine can't
    // bind (exit 1), and the next port is tried.
    let launch = stub("port", "serve:app:1500;exit:0");
    let squatter = TcpListener::bind(("127.0.0.1", launch.first_port.unwrap())).unwrap();
    let taken = launch.first_port.unwrap();
    std::thread::spawn(move || {
        for stream in squatter.incoming().flatten() {
            let mut stream = stream;
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nready app");
        }
    });
    let view = Recorder::default();
    with_stub_script(&launch, || supervise(&launch, &Engine::default(), &view));
    let opens: Vec<String> = view.events().into_iter().filter(|e| e.starts_with("open:")).collect();
    assert_eq!(opens.len(), 1, "{:?}", view.events());
    assert!(!opens[0].contains(&format!(":{taken}/")), "{opens:?}");
    assert!(log(&launch).contains(&format!("port {taken} is taken")));
    assert_eq!(starts(&launch), 2);
}

// ---- r1.5: supervision for the child's whole life ----

#[test]
fn the_page_is_shown_once_at_its_launch_url() {
    let (launch, view) = run_stub("once", "serve:app:3000;exit:0");
    let token = log(&launch).lines().find_map(|l| l.strip_prefix("--- start ").map(|t| t.trim_end_matches(" ---").to_string())).unwrap();
    let opens: Vec<String> = view.events().into_iter().filter(|e| e.starts_with("open:")).collect();
    assert_eq!(opens, vec![format!("open:http://127.0.0.1:{}/?launch={token}", launch.first_port.unwrap())]);
}

#[test]
fn an_exit_0_nobody_asked_for_after_a_page_is_d4() {
    let (launch, view) = run_stub("exit0", "serve:app:1500;say:last words;exit:0");
    assert_eq!(view.states(), ["starting", "open", "stopped"]);
    assert!(details(&view.last()).contains("last words"));
    assert!(log(&launch).contains("--- the engine stopped (exit 0); its last output: ---\n"));
}

#[test]
fn a_late_exit_1_is_d4_and_never_retried() {
    let (launch, view) = run_stub("late1", "serve:refused:1500;exit:1");
    assert_eq!(view.states(), ["starting", "open", "stopped"]);
    assert_eq!(starts(&launch), 1);
}

#[test]
fn an_exit_1_before_any_page_is_d5_with_this_starts_lines() {
    let (launch, view) = run_stub("early1", "say:deltabadger: cannot read the install;exit:1");
    assert_eq!(view.states(), ["starting", "not-started"]);
    assert!(details(&view.last()).contains("deltabadger: cannot read the install"), "{}", view.last());
    assert!(view.last().contains(&format!("&log={}", supervisor_encode(&launch.log_dir()))));
    assert_eq!(starts(&launch), 1);
}

fn supervisor_encode(path: &Path) -> String {
    Page::NotStarted(String::new()).fragment(path).split("&log=").nth(1).unwrap().split("&details").next().unwrap().to_string()
}

#[test]
fn a_panic_is_d4_with_the_panic_in_the_details_and_the_log() {
    let (launch, view) = run_stub("panic", "serve:app:1500;panic");
    assert_eq!(view.states(), ["starting", "open", "stopped"]);
    assert!(details(&view.last()).contains("stub engine panicked"), "{}", view.last());
    let log = log(&launch);
    assert!(log.contains("--- the engine stopped (exit 101); its last output: ---"), "{log}");
    assert!(log.contains("stub engine panicked"));
}

#[test]
fn a_killed_engine_is_d4() {
    let launch = stub("killed", "serve:app:30000");
    let view = Arc::new(Recorder::default());
    let engine = Arc::new(Engine::default());
    with_stub_script(&launch, || {
        std::thread::scope(|s| {
            let (v, e) = (view.clone(), engine.clone());
            let l = &launch;
            s.spawn(move || supervise(l, &e, &*v));
            view.wait_for("open:", Duration::from_secs(20));
            let pid = engine.pid().unwrap().to_string();
            #[cfg(unix)]
            let killed = std::process::Command::new("kill").args(["-9", &pid]).status().unwrap();
            #[cfg(windows)]
            let killed = std::process::Command::new("taskkill").args(["/F", "/PID", &pid]).status().unwrap();
            assert!(killed.success());
        })
    });
    assert_eq!(view.states(), ["starting", "open", "stopped"]);
    if cfg!(unix) {
        assert!(log(&launch).contains("--- the engine stopped (exit signal 9)"), "{}", log(&launch));
    }
}

#[test]
fn the_starting_page_then_a_gap_then_the_app_shows_d1_then_the_app() {
    // C7's wait (starting), then more than 5 s with nothing listening while the engine binds, then the app.
    let launch = stub("c7", "serve:starting:1500;gap:5500;serve:app:1500;stdin");
    let view = Arc::new(Recorder::default());
    let engine = Arc::new(Engine::default());
    with_stub_script(&launch, || {
        std::thread::scope(|s| {
            let (v, e) = (view.clone(), engine.clone());
            let l = &launch;
            let supervisor = s.spawn(move || supervise(l, &e, &*v));
            view.wait_for("open:", Duration::from_secs(30));
            stop_engine(&engine);
            supervisor.join().unwrap();
        })
    });
    assert_eq!(view.states(), ["starting", "open"], "nothing after the quit");
    assert!(log(&launch).contains("desktop: the engine stopped as asked (exit 0)"));
}

// ---- r2.3: losing the engine after a page ----

#[test]
fn one_slow_answer_never_replaces_the_page() {
    let (_, view) = run_stub("slow", "serve:app:2000;slow:app:3000:1500;exit:0");
    assert_eq!(view.states(), ["starting", "open", "stopped"]);
}

#[test]
fn three_failed_polls_after_a_page_show_d7_and_the_exit_d4() {
    let (_, view) = run_stub("d7", "serve:app:1500;gap:4000;exit:2");
    assert_eq!(view.states(), ["starting", "open", "not-responding", "stopped"]);
}

#[test]
fn the_same_engine_answering_again_brings_its_page_back() {
    let (_, view) = run_stub("d7-back", "serve:app:1500;gap:4000;serve:app:2000;exit:0");
    assert_eq!(view.states(), ["starting", "open", "not-responding", "back", "stopped"]);
}

// ---- r4.4, r5.3: the details ----

#[test]
fn the_details_are_this_starts_lines_across_a_rotation_and_never_an_earlier_starts() {
    let launch = engine("details");
    fs::create_dir_all(launch.log_dir()).unwrap();
    let rotated = launch.log_dir().join("deltabadger.log.1");
    // The start's first marker rotated away: .1 begins with this start's rotation marker, after an earlier run's lines.
    fs::write(&rotated, "--- start old ---\nold line\n--- start new ---\nnew 1\nnew 2\n").unwrap();
    fs::write(launch.log_file(), "--- start new ---\nnew 3\n--- the engine stopped (exit 2); its last output: ---\nboom\n").unwrap();
    assert_eq!(this_start(&launch, "new"), "new 1\nnew 2\nnew 3\n--- the engine stopped (exit 2); its last output: ---\nboom");
    // Only the current file holds the marker.
    fs::write(&rotated, "--- start old ---\nold line\n").unwrap();
    assert_eq!(this_start(&launch, "new"), "new 3\n--- the engine stopped (exit 2); its last output: ---\nboom");
    // At most the last 20 lines.
    let many: String = (0..30).map(|i| format!("line {i}\n")).collect();
    fs::write(launch.log_file(), format!("--- start new ---\n{many}")).unwrap();
    let shown = this_start(&launch, "new");
    assert_eq!(shown.lines().count(), 20);
    assert!(shown.starts_with("line 10\n") && shown.ends_with("line 29"));
}

#[test]
fn each_start_keeps_the_previous_starts_output() {
    let (launch, _) = run_stub("capture-1", "say:first run;exit:1");
    let view = Recorder::default();
    with_stub_script(&launch, || {
        fs::write(launch.cwd.join("stub-script"), "say:second run;exit:1").unwrap();
        supervise(&launch, &Engine::default(), &view)
    });
    let previous = fs::read_to_string(launch.log_dir().join("deltabadger-stderr.log.1")).unwrap();
    let current = fs::read_to_string(launch.stderr_file()).unwrap();
    assert!(previous.contains("first run") && !previous.contains("second run"));
    assert!(current.contains("second run") && !current.contains("first run"));
    // The second start's details hold nothing from the first.
    assert!(!details(&view.last()).contains("first run"));
    // The data paths line: at each launch, and again before each start.
    assert_eq!(log(&launch).matches(&format!("data: {} primary: ", launch.data.display())).count(), 4);
}

#[test]
fn the_page_fragment_is_encoded_and_names_the_log_folder() {
    let fragment = Page::Stopped("a b&c=d\n#é".into()).fragment(Path::new("/data/log"));
    assert_eq!(fragment, "stopped&log=%2Fdata%2Flog&details=a%20b%26c%3Dd%0A%23%C3%A9");
    assert_eq!(Page::Finishing.fragment(Path::new("/x")), "finishing");
}

// ---- r1.11: failures before the log ----

#[test]
fn a_folder_where_the_key_file_goes_is_d5_and_nothing_starts() {
    let launch = stub("secrets-dir", "exit:0");
    fs::create_dir_all(launch.secrets()).unwrap();
    let view = Recorder::default();
    with_stub_script(&launch, || supervise(&launch, &Engine::default(), &view));
    assert_eq!(view.states(), ["not-started"]);
    assert!(details(&view.last()).contains(".secrets"), "{}", view.last());
    assert_eq!(starts(&launch), 0);
}

#[cfg(unix)]
fn read_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o500)).unwrap();
}

#[cfg(unix)]
#[test]
fn an_unwritable_data_dir_is_d5_and_nothing_starts() {
    let mut launch = stub("ro-data", "exit:0");
    read_only(&launch.cwd.join("parent"));
    launch.data = launch.cwd.join("parent").join("data");
    let view = Recorder::default();
    with_stub_script(&launch, || supervise(&launch, &Engine::default(), &view));
    assert_eq!(view.states(), ["not-started"]);
    assert!(details(&view.last()).contains("Failed to create writable directory"), "{}", view.last());
}

#[cfg(unix)]
#[test]
fn an_unwritable_log_folder_is_d5_and_nothing_starts() {
    let launch = stub("ro-log", "exit:0");
    read_only(&launch.log_dir());
    let view = Recorder::default();
    with_stub_script(&launch, || supervise(&launch, &Engine::default(), &view));
    assert_eq!(view.states(), ["not-started"]);
    assert!(details(&view.last()).contains("deltabadger"), "{}", view.last());
    assert!(!launch.stderr_file().exists());
}

#[test]
fn a_directory_sync_that_fails_is_d5_and_nothing_starts() {
    let launch = stub("sync-fails", "exit:0");
    let view = Recorder::default();
    // The first launch writes the file; the sync before the spawn fails.
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let counter = calls.clone();
    hooks::set(move |stage, _| {
        if stage == hooks::Stage::SyncDir {
            counter.set(counter.get() + 1);
            if counter.get() == 2 {
                return Err(std::io::Error::other("injected"));
            }
        }
        Ok(())
    });
    with_stub_script(&launch, || supervise(&launch, &Engine::default(), &view));
    hooks::clear();
    assert_eq!(view.states(), ["not-started"]);
    assert!(details(&view.last()).contains("injected"));
    assert_eq!(starts(&launch), 0);
}

// ---- r2.1, r3.3: .secrets ----

fn sha_mode_mtime(path: &Path) -> (Vec<u8>, u32, SystemTime) {
    let meta = fs::metadata(path).unwrap();
    #[cfg(unix)]
    let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777;
    #[cfg(not(unix))]
    let mode = 0;
    (fs::read(path).unwrap(), mode, meta.modified().unwrap())
}

#[test]
fn the_key_file_is_private_before_its_first_byte() {
    let dir = temporary_directory("secrets-mode");
    let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
    let record = seen.clone();
    hooks::set(move |stage, path| {
        if stage == hooks::Stage::Created {
            let meta = fs::metadata(path).unwrap();
            #[cfg(unix)]
            let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777;
            #[cfg(not(unix))]
            let mode = 0o600;
            *record.borrow_mut() = Some((mode, meta.len()));
        }
        Ok(())
    });
    secrets::setup_secrets(&dir, &dir.join("db/production.sqlite3")).unwrap();
    hooks::clear();
    assert_eq!(*seen.borrow(), Some((0o600, 0)));
    #[cfg(unix)]
    assert_eq!(sha_mode_mtime(&dir.join(".secrets")).1, 0o600);
}

#[test]
fn a_failed_write_leaves_no_temporary_file() {
    let dir = temporary_directory("secrets-fail");
    hooks::set(|stage, _| if stage == hooks::Stage::Created { Err(std::io::Error::other("disk full")) } else { Ok(()) });
    let result = secrets::setup_secrets(&dir, &dir.join("db/production.sqlite3"));
    hooks::clear();
    assert!(result.unwrap_err().contains("disk full"));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "neither the file nor a temporary one");
}

#[test]
fn the_key_file_and_its_folder_are_synced_when_written() {
    let dir = temporary_directory("secrets-sync");
    hooks::clear();
    secrets::setup_secrets(&dir, &dir.join("db/production.sqlite3")).unwrap();
    let synced = hooks::synced_dirs();
    hooks::clear();
    if cfg!(unix) {
        assert_eq!(synced, vec![dir.clone()]);
    } else {
        assert!(synced.is_empty(), "Windows leaves the entry to the journal (r3.2)");
    }
}

#[test]
fn a_launch_after_a_write_cut_short_completes_the_sync_before_the_engine_starts() {
    // The writer stopped between the link and the directory sync (a crash): the next launch syncs before it spawns.
    let launch = stub("secrets-cut", "exit:1");
    fs::create_dir_all(&launch.data).unwrap();
    hooks::set(|stage, _| if stage == hooks::Stage::Linked { Err(std::io::Error::other("cut short")) } else { Ok(()) });
    assert!(secrets::setup_secrets(&launch.data, &launch.primary()).is_err());
    hooks::clear();
    let written = sha_mode_mtime(&launch.secrets());
    let order = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let record = order.clone();
    let log_file = launch.log_file();
    hooks::set(move |stage, path| {
        if stage == hooks::Stage::SyncDir {
            record.borrow_mut().push((path.to_path_buf(), fs::read_to_string(&log_file).unwrap_or_default().contains("desktop: the engine started")));
        }
        Ok(())
    });
    let view = Recorder::default();
    with_stub_script(&launch, || supervise(&launch, &Engine::default(), &view));
    hooks::clear();
    if cfg!(unix) {
        assert_eq!(*order.borrow(), vec![(launch.data.clone(), false)], "synced once, before the spawn");
    }
    assert_eq!(starts(&launch), 1);
    assert_eq!(sha_mode_mtime(&launch.secrets()), written);
}

#[test]
fn the_key_file_is_unchanged_across_launches() {
    let launch = stub("secrets-three", "exit:1");
    let view = Recorder::default();
    with_stub_script(&launch, || supervise(&launch, &Engine::default(), &view));
    let first = sha_mode_mtime(&launch.secrets());
    std::thread::sleep(Duration::from_millis(20));
    for _ in 0..2 {
        with_stub_script(&launch, || supervise(&launch, &Engine::default(), &view));
        assert_eq!(sha_mode_mtime(&launch.secrets()), first);
    }
    assert_eq!(starts(&launch), 3);
}

#[test]
fn racing_writers_adopt_the_first_file() {
    let dir = temporary_directory("secrets-race");
    let target = dir.join(".secrets");
    let racer = target.clone();
    // Another launch publishes its file while this one is writing.
    hooks::set(move |stage, _| {
        if stage == hooks::Stage::Created {
            fs::write(&racer, "SECRET_KEY_BASE=theirs\n").unwrap();
        }
        Ok(())
    });
    let resolved = secrets::setup_secrets(&dir, &dir.join("db/production.sqlite3")).unwrap();
    hooks::clear();
    assert_eq!(resolved.get("SECRET_KEY_BASE").map(String::as_str), Some("theirs"));
    assert_eq!(fs::read_to_string(&target).unwrap(), "SECRET_KEY_BASE=theirs\n");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
}

// ---- the real sidecar ----

#[test]
fn stop_engine_closes_the_real_engines_input_and_it_stops_on_request() {
    let launch = engine("real");
    let view = Arc::new(Recorder::default());
    let engine = Arc::new(Engine::default());
    std::thread::scope(|s| {
        let (v, e) = (view.clone(), engine.clone());
        let l = &launch;
        let supervisor = s.spawn(move || supervise(l, &e, &*v));
        view.wait_for("open:", Duration::from_secs(120));
        assert!(matches!(stop_engine(&engine), supervisor::Stopped::Draining));
        let deadline = Instant::now() + Duration::from_secs(30);
        while !supervisor.is_finished() {
            assert!(Instant::now() < deadline, "the engine did not stop within 30 s");
            std::thread::sleep(Duration::from_millis(50));
        }
        supervisor.join().unwrap();
    });
    let log = log(&launch);
    assert!(log.contains("stopped on request"), "{log}");
    assert!(log.contains("desktop: the engine stopped as asked (exit 0)"), "{log}");
    assert!(!log.contains("the engine stopped (exit"), "no unexpected exit: {log}");
    assert!(!lock_held(&launch.lock_file()).unwrap());
    assert_eq!(view.states(), ["starting", "open"]);
    assert!(launch.primary().is_file());
}

#[test]
fn a_download_goes_to_the_downloads_folder_under_a_free_name() {
    let downloads = temporary_directory("downloads");
    let first = download_destination(&downloads, Path::new("deltabadger-api.md"));
    assert_eq!(first, downloads.join("deltabadger-api.md"));
    fs::write(&first, "").unwrap();
    assert_eq!(download_destination(&downloads, Path::new("/elsewhere/deltabadger-api.md")), downloads.join("deltabadger-api (1).md"));
    fs::write(downloads.join("deltabadger-api (1).md"), "").unwrap();
    assert_eq!(download_destination(&downloads, Path::new("deltabadger-api.md")), downloads.join("deltabadger-api (2).md"));
    assert_eq!(download_destination(&downloads, Path::new("")), downloads.join("download"));
}

// The desktop chrome hangs off html.tauri, and this script is the only thing that sets it:
// the web layouts ship no tauri asset, and the page policy forbids an inline block. If this
// string loses the class, the packaged app silently paints as a browser page.
#[test]
fn the_initialization_script_marks_the_document_as_desktop() {
    assert!(INITIALIZATION_SCRIPT.contains("window.__IS_TAURI__ = true;"));
    assert!(INITIALIZATION_SCRIPT.contains("window.__TAURI_INTERNALS__ = true;"));
    assert!(INITIALIZATION_SCRIPT.contains("classList.add('tauri')"));
    // The null-documentElement path is the Windows one, and it is the path that cannot be
    // exercised from here — so what it hangs on is pinned instead.
    assert!(INITIALIZATION_SCRIPT.contains("new MutationObserver"));
    assert!(!INITIALIZATION_SCRIPT.contains("DOMContentLoaded"));
}
