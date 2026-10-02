mod common;
use common::seed::{self, BotSpec};
use std::io::BufRead;
use std::path::Path;
use std::process::{Command, Stdio};

fn cli(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_deltabadger"));
    c.args(args).env("STORAGE_DIR", dir).env("SECRET_KEY_BASE", "engine-test-secret")
        .env_remove("DATABASE_URL").env_remove("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY").env_remove("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT");
    c
}
fn lease_of(dir: &Path) -> Option<serde_json::Value> {
    deltabadger::lease::read(&rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(), &seed::cipher()).unwrap()
}
fn stderr(out: &std::process::Output) -> String { String::from_utf8_lossy(&out.stderr).to_string() }

#[test]
fn resolve_placement_refuses_a_database_url_before_touching_any_database() {
    for var in ["DATABASE_URL", "PRIMARY_DATABASE_URL", "QUEUE_DATABASE_URL"] {
        let dir = tempfile::tempdir().unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_deltabadger"))
            .args(["resolve-placement", "1", "--not-placed"])
            .env("STORAGE_DIR", dir.path())
            .env(var, "sqlite3:/elsewhere.sqlite3")
            .output()
            .unwrap();
        assert!(!out.status.success(), "{var}: must exit non-zero");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(var) && err.contains("*_DATABASE_PATH"), "{var}: unclear message: {err}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "{var}: no lock or database file may be created");
    }
}

#[test]
fn run_and_handback_refuse_a_database_url_and_a_missing_secret_before_touching_anything() {
    for cmd in ["run", "handback"] {
        let dir = tempfile::tempdir().unwrap();
        let out = cli(dir.path(), &[cmd]).env("QUEUE_DATABASE_URL", "sqlite3:/elsewhere.sqlite3").output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{cmd}");
        assert!(stderr(&out).contains("QUEUE_DATABASE_URL"), "{cmd}: {}", stderr(&out));
        let out = cli(dir.path(), &[cmd]).env_remove("SECRET_KEY_BASE").output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{cmd}");
        assert!(stderr(&out).contains("SECRET_KEY_BASE"), "{cmd}: {}", stderr(&out));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "{cmd}: no lock or database file may be created");
    }
}

#[test]
fn run_refuses_a_kraken_bot_a_live_key_and_an_unreadable_key_and_claims_nothing() {
    let (dir, o, s) = common::install();
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    drop(o);
    let out = cli(dir.path(), &["run"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("Exchanges::Kraken is not connected in this build"), "{}", stderr(&out));
    assert!(lease_of(dir.path()).is_none(), "nothing claimed");

    let (dir, o, s) = common::install_alpaca();
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE api_keys SET passphrase = ?1", [seed::cipher().encrypt("live")]).unwrap();
    drop(o);
    let out = cli(dir.path(), &["run"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("live Alpaca trading is not enabled"), "{}", stderr(&out));
    assert!(lease_of(dir.path()).is_none());

    let (dir, o, s) = common::install_alpaca();
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    drop(o);
    let out = cli(dir.path(), &["run"]).env("SECRET_KEY_BASE", "another-instance").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("api key unreadable"), "{}", stderr(&out));
    assert!(lease_of(dir.path()).is_none());
}

#[test]
fn run_refuses_a_stopped_bot_whose_orders_it_could_not_poll() {
    // Stopped bots are not "eligible", but the loop still polls their outstanding orders: preflight checks them too.
    let open = |c: &rusqlite::Connection, s: &seed::Seeded, bot: i64| seed::insert_tx(c, s, bot, &seed::TxSpec { status: 0, external_status: Some(1),
        external_id: Some("OPEN-STOPPED".into()), order_type: 1, amount: Some("0.001"), quote_amount: None, price: Some("50000"),
        quote_amount_exec: Some("0"), amount_exec: Some("0"), created_at: "2026-09-01 10:00:01".into() });

    let (dir, o, s) = common::install(); // Kraken
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    open(&o.primary, &s, bot);
    drop(o);
    let out = cli(dir.path(), &["run"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains(&format!("bot {bot}: Exchanges::Kraken is not connected in this build")), "{}", stderr(&out));
    assert!(lease_of(dir.path()).is_none());

    let (dir, o, s) = common::install_alpaca();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    open(&o.primary, &s, bot);
    o.primary.execute("UPDATE api_keys SET passphrase = ?1", [seed::cipher().encrypt("live")]).unwrap();
    drop(o);
    let out = cli(dir.path(), &["run"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains(&format!("bot {bot}: live Alpaca trading is not enabled")), "{}", stderr(&out));
    assert!(lease_of(dir.path()).is_none());
}

#[test]
fn run_takes_over_and_stops_cleanly_on_sigterm_then_handback_returns_the_install() {
    let (dir, o, s) = common::install_alpaca();
    // Not due for decades: the loop takes over and idles without calling Alpaca.
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2099-01-01 00:00:00"));
    drop(o);
    let mut child = cli(dir.path(), &["run"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let took = lines.next().expect("a log line").unwrap();
    assert!(took.contains("took over"), "{took}");
    let running = lines.next().expect("a log line").unwrap();
    assert!(running.contains("running"), "{running}"); // signal handlers are registered before this line
    assert_eq!(lease_of(dir.path()).unwrap()["engine"], "rust");
    assert!(Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap().success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() { break s; }
        assert!(std::time::Instant::now() < deadline, "run did not exit on SIGTERM");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0));
    let wrong = cli(dir.path(), &["handback"]).env("SECRET_KEY_BASE", "another-instance").output().unwrap();
    assert_eq!(wrong.status.code(), Some(1), "a wrong secret is refused before anything is written");
    assert_eq!(lease_of(dir.path()).unwrap()["engine"], "rust", "the row is untouched and still Rust's");
    let out = cli(dir.path(), &["handback"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let row = lease_of(dir.path()).unwrap();
    assert_eq!((row["engine"].as_str(), row["handed_back"].as_bool()), (Some("none"), Some(true)));
}

#[test]
fn check_names_each_ineligible_bot_and_its_reason() {
    let (dir, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limited", serde_json::json!(true)));
    drop(o);
    let out = cli(dir.path(), &["check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), format!("deltabadger: this install uses things only the full app runs:\nbot {id} (scheduled): quote_amount_limited\n"));
}
