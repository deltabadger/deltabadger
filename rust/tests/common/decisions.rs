//! The decision-parity grids of script/rust/decisions.rb: built and recorded by Rails on scratch databases, played by
//! deltabadger::parity::decide on pristine copies (tests/parity.rs, tests/tick_basket.rs).
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

pub fn rails(args: &[&str]) { if let Err(e) = try_rails(args) { panic!("{e}"); } }

pub fn try_rails(args: &[&str]) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let run = |rails_args: &[&str]| -> Result<(), String> {
        let mut cmd = Command::new(root.join("bin/rails"));
        cmd.current_dir(root).args(rails_args).env_remove("DATABASE_URL")
            .env("PROXY_KRAKEN", "http://127.0.0.1:9") // any unscripted real call fails fast instead of trading
            .env("APP_ROOT_URL", "http://localhost:3000") // config/environments/development.rb requires it
            .env("SKIP_TEST_DATABASE", "true"); // schema:load in development also purges the repo's storage/test*.sqlite3
        for db in ["primary", "queue", "cache", "cable"] {
            cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", scratch.path().display()));
        }
        // A Rails run stuck on something never shows up in nextest's hour-long TIMEOUT: kill it after 20 minutes
        // (a whole grid records in under 3) and print what it had written, with every thread's backtrace, which
        // script/rust/decisions.rb prints on SIGUSR1 (ten seconds to do it before the kill).
        let child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("bin/rails runs");
        let pid = child.id();
        let (done, finished) = std::sync::mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            let timed_out = finished.recv_timeout(Duration::from_secs(20 * 60)).is_err();
            if timed_out {
                // An empty USR1 dump (#687) means the main thread is stuck in native code, where Ruby's trap never runs.
                eprintln!("{}", native_state(pid));
                let _ = Command::new("kill").args(["-USR1", &pid.to_string()]).status();
                std::thread::sleep(Duration::from_secs(10));
                let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
            }
            timed_out
        });
        let out = child.wait_with_output().expect("bin/rails runs");
        let _ = done.send(());
        let timed_out = watchdog.join().unwrap();
        let output = || format!("stdout:\n{}\nstderr:\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if timed_out { return Err(format!("bin/rails {rails_args:?} still ran after 20 minutes and was killed\n{}", output())); }
        if out.status.success() { Ok(()) } else { Err(format!("bin/rails {rails_args:?} failed:\n{}", String::from_utf8_lossy(&out.stderr))) }
    };
    // The oracle's own queue/cache/cable databases must exist (ActionJob queries Solid Queue directly,
    // broadcasts write Solid Cable): load every schema into the scratch files first.
    run(&["db:schema:load"])?;
    let mut full = vec!["runner", "script/rust/decisions.rb"];
    full.extend_from_slice(args);
    run(&full)
}

/// A stuck process's native side: gdb's backtraces when it can attach (CI's passwordless sudo, else plain), and from
/// /proc every thread's kernel wait channel and the files it holds open (which SQLite databases, which locks).
fn native_state(pid: u32) -> String {
    let mut out = format!("native state of {pid}:\n");
    let gdb = ["thread apply all bt", "info sharedlibrary"].iter().flat_map(|c| ["-ex", c]).collect::<Vec<_>>();
    for (cmd, pre) in [("sudo", vec!["-n", "timeout", "60", "gdb"]), ("timeout", vec!["60", "gdb"])] {
        let pid = pid.to_string();
        let args = pre.into_iter().chain(["-p", pid.as_str(), "-batch"]).chain(gdb.iter().copied()).collect::<Vec<_>>();
        if let Ok(o) = Command::new(cmd).args(&args).output() {
            let text = String::from_utf8_lossy(&o.stdout);
            if text.contains("Thread") { out.push_str(&text); break; }
            out.push_str(&format!("{cmd} gdb: {}\n", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("")));
        }
    }
    let proc = format!("/proc/{pid}");
    for task in std::fs::read_dir(format!("{proc}/task")).into_iter().flatten().flatten() {
        let read = |f: &str| std::fs::read_to_string(task.path().join(f)).unwrap_or_default().trim().to_string();
        out.push_str(&format!("task {:?} {} wchan={} state={}\n", task.file_name(), read("comm"), read("wchan"),
            read("stat").split(' ').nth(2).unwrap_or("")));
    }
    for fd in std::fs::read_dir(format!("{proc}/fd")).into_iter().flatten().flatten() {
        if let Ok(target) = std::fs::read_link(fd.path()) { out.push_str(&format!("fd {:?} -> {}\n", fd.file_name(), target.display())); }
    }
    out.push_str(&std::fs::read_to_string(format!("{proc}/locks")).unwrap_or_else(|_| std::fs::read_to_string("/proc/locks").unwrap_or_default()));
    out
}

/// `command`'s grid, as `rails(&[command, into])` builds it: built once per test run (super::run_template), and
/// copied into `into` for every test that asks.
pub fn built_grid(command: &str, into: &Path) {
    let template = super::run_template(&format!("decisions-{command}"), super::has_subdirs,
        |out| rails(&[command, out.to_str().unwrap()]));
    super::copy_tree(&template, into);
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}

/// R5: failure bookkeeping preserves unrelated NULL evidence. The recorded grids expose
/// Rails deleting this pre-existing NULL flag; require Rust to preserve it explicitly.
/// Return a comparison copy of Rails; never rewrite either recorded output or any money field.
pub fn r5_cleanup_expected(rails: &serde_json::Value, rust: &serde_json::Value) -> serde_json::Value {
    const KEY: &str = "missed_quote_amount_was_set";
    let mut expected = rails.clone();
    for row in expected["changes"]["bots"].as_array_mut().unwrap() {
        if row["before"]["transient_data"].get(KEY) != Some(&serde_json::Value::Null)
            || !row["after"]["transient_data"].is_object()
            || row["after"]["transient_data"].get(KEY).is_some() { continue; }
        // Rust left the bot row untouched (a swap that started and ended within the run): the flag kept is all Rails changed.
        if let Some(mine) = rust["changes"]["bots"].as_array().unwrap().iter().find(|b| b["id"] == row["id"]) {
            assert_eq!(mine["before"]["transient_data"].get(KEY), Some(&serde_json::Value::Null), "R5 same initial NULL flag");
            assert_eq!(mine["after"]["transient_data"].get(KEY), Some(&serde_json::Value::Null), "R5 failure cleanup preserves unrelated NULL flag");
        }
        row["after"]["transient_data"][KEY] = serde_json::Value::Null;
    }
    // A row whose only change was that flag is no change at all once it is kept.
    expected["changes"]["bots"].as_array_mut().unwrap().retain(|row| row["before"] != row["after"]);
    expected
}
