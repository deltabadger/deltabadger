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
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("price_limited", serde_json::json!(true)));
    drop(o);
    let out = cli(dir.path(), &["check"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), format!("deltabadger: this install uses things only the full app runs:\nbot {id} (scheduled): price_limited\n"));
}

fn free_port() -> u16 { std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() }

/// GET /up; `None` when nothing listens.
fn http_up(port: u16) -> Option<String> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok()?;
    write!(stream, "GET /up HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").ok()?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;
    Some(answer)
}

/// A child process killed when the test ends, passed or failed: a failing assertion must not leave a server running.
struct Running(std::process::Child);
impl Drop for Running {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

/// Runs `cmd` to its end. One still running after 20 s is killed and fails the test: it should have refused.
fn finished(cmd: &mut Command) -> std::process::Output {
    use std::io::Read;
    let mut child = Running(cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while child.0.try_wait().unwrap().is_none() {
        assert!(std::time::Instant::now() < deadline, "still running after 20 s: it should have refused");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut out = std::process::Output { status: child.0.wait().unwrap(), stdout: vec![], stderr: vec![] };
    child.0.stderr.take().unwrap().read_to_end(&mut out.stderr).unwrap();
    out
}

#[test]
fn serve_takes_the_install_over_holds_the_one_lock_until_sigterm_stops_both_then_handback_returns_it() {
    let (dir, o, s) = common::install_alpaca(); // seeds an admin user
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2099-01-01 00:00:00")); // never due: no call to Alpaca
    drop(o);
    let port = free_port();
    let mut child = Running(cli(dir.path(), &["serve"]).env("PORT", port.to_string()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout = child.0.stdout.take().unwrap();
    std::thread::spawn(move || for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) { let _ = tx.send(line); });
    let next = || rx.recv_timeout(std::time::Duration::from_secs(20)).expect("a log line within 20 s");
    let took = next();
    assert!(took.contains("took over"), "{took}");
    let running = next();
    assert!(running.contains("running") && running.contains(&format!("port {port}")), "{running}"); // signal handlers are registered before this line
    assert!(http_up(port).is_some_and(|a| a.starts_with("HTTP/1.1 200")), "the web answers in the same process");
    assert_eq!(lease_of(dir.path()).unwrap()["engine"], "rust", "serve claimed the install, as run does");
    for cmd in ["check", "run", "handback", "serve"] {
        let out = cli(dir.path(), &[cmd]).env("PORT", free_port().to_string()).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{cmd}");
        assert!(stderr(&out).contains("another Deltabadger engine is running"), "{cmd}: {}", stderr(&out));
    }
    assert_eq!(lease_of(dir.path()).unwrap()["engine"], "rust", "a refused handback wrote nothing");

    assert!(Command::new("kill").args(["-TERM", &child.0.id().to_string()]).status().unwrap().success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.0.try_wait().unwrap() { break s; }
        assert!(std::time::Instant::now() < deadline, "serve did not exit on SIGTERM");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0));
    assert!(http_up(port).is_none(), "the web stopped with the engine");
    assert_eq!(lease_of(dir.path()).unwrap()["engine"], "rust", "stopping serve is not a handback");
    let out = cli(dir.path(), &["handback"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let row = lease_of(dir.path()).unwrap();
    assert_eq!((row["engine"].as_str(), row["handed_back"].as_bool()), (Some("none"), Some(true)));
}

#[test]
fn serve_refusals_come_before_the_claim() {
    // What the engine refuses: a Kraken bot in this build.
    let (dir, o, s) = common::install();
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    drop(o);
    let out = finished(cli(dir.path(), &["serve"]).env("PORT", free_port().to_string()));
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("Exchanges::Kraken is not connected in this build"), "{}", stderr(&out));
    assert!(lease_of(dir.path()).is_none(), "nothing claimed");

    // What the web refuses: a port already taken, and an install with no admin user.
    let (dir, o, _) = common::install_alpaca();
    drop(o);
    let taken = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let out = finished(cli(dir.path(), &["serve"]).env("PORT", taken.local_addr().unwrap().port().to_string()));
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("cannot listen on port"), "{}", stderr(&out));
    assert!(lease_of(dir.path()).is_none(), "nothing claimed");

    let dir = common::rails_install();
    let out = finished(cli(dir.path(), &["serve"]).env("PORT", free_port().to_string()));
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no admin user"), "{}", stderr(&out));
    assert!(lease_of(dir.path()).is_none(), "nothing claimed");
}

/// `serve`'s eligibility refusal names the bot and the reason in the words `check` uses.
#[test]
fn serve_refuses_an_ineligible_install_in_the_words_check_uses() {
    let (dir, o, s) = common::install_alpaca(); // seeds an admin user
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("price_limited", serde_json::json!(true)));
    drop(o);
    let checked = cli(dir.path(), &["check"]).output().unwrap();
    let served = finished(cli(dir.path(), &["serve"]).env("PORT", free_port().to_string()));
    assert_eq!((checked.status.code(), served.status.code()), (Some(1), Some(1)));
    assert!(stderr(&served).contains(&format!("bot {id} (scheduled): price_limited")), "{}", stderr(&served));
    assert_eq!(stderr(&served), stderr(&checked), "the same words, byte for byte");
    assert!(lease_of(dir.path()).is_none(), "nothing claimed");
}

/// `run` and `serve` with the mail sender as a service of the supervisor, as processes: what a bot is owed goes out
/// through the SMTP server the environment names, the marker goes only once the server took the mail, and no credential
/// is sent in the clear or printed.
#[test]
fn run_and_serve_send_the_mail_a_bot_is_owed_and_neither_send_nor_print_a_credential_in_the_clear() {
    use common::smtp::{self, Behaviour};
    use deltabadger::engine::notice;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for command in ["run", "serve"] {
        let (dir, o, s) = common::install_alpaca();
        // Not due for decades: the engine idles, the mail does not wait for a tick.
        let bot = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2099-01-01 00:00:00"));
        o.primary.execute("UPDATE bots SET label = 'Weekly BTC', transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3",
                          (format!("$.{}", notice::STOPPED), notice::stopped_marker("unauthorized.", chrono::Utc::now()).to_string(), bot)).unwrap();
        drop(o);
        let owed = || notice::all_pending(&rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap()).unwrap().len();
        // The server lives in this test's runtime, so every wait here is a tokio sleep that lets it answer.
        let run_until = |server: &smtp::Fake, credentials: Option<(&str, &str)>, done: &dyn Fn(&smtp::Fake) -> bool| -> (Option<i32>, String) {
            let mut process = cli(dir.path(), &[command]);
            process.stdout(Stdio::piped()).stderr(Stdio::piped()).env("PORT", free_port().to_string())
                .env("SMTP_ADDRESS", "127.0.0.1").env("SMTP_PORT", server.port.to_string()).env("NOTIFICATIONS_SENDER", "bots@example.com");
            if let Some((user, password)) = credentials { process.env("SMTP_USER_NAME", user).env("SMTP_PASSWORD", password); }
            let child = process.spawn().unwrap();
            let reached = rt.block_on(async {
                for _ in 0..400 { if done(server) { return true; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
                false
            });
            // Stopped before anything is asserted, so a failure here leaves no engine running.
            assert!(Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap().success());
            let out = child.wait_with_output().unwrap();
            assert!(reached, "{command}: the sender never got that far");
            (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
        };

        // A user name and a password, and a server that offers no STARTTLS: two attempts, no AUTH on the wire, the
        // marker stays, and the log says why without naming a credential.
        let plain = rt.block_on(smtp::start(Behaviour { auth: true, ..Default::default() }));
        let (code, printed) = run_until(&plain, Some(("alice", "s3cret-pw")), &|server| server.sessions().len() >= 2);
        assert_eq!(code, Some(0), "{command}: {printed}");
        assert!(printed.contains(&format!("[mail] stopped_by_error for bot {bot}: not sent: tls: 127.0.0.1 offers no STARTTLS")), "{command}: {printed}");
        for secret in ["s3cret-pw", "alice", "AGFsaWNlAHMzY3JldC1wdw=="] { assert!(!printed.contains(secret), "{command}: {printed}"); }
        assert!(plain.sessions().iter().all(|s| !s.lines.iter().any(|l| l.starts_with("AUTH"))), "{command}: {:?}", plain.sessions());
        assert_eq!(owed(), 1, "{command}");

        // The next start, the same relay used without credentials (plain text is then allowed, as in Rails): the mail
        // goes out and the marker with it, through eligibility::guard.
        let (code, printed) = run_until(&plain, None, &|server| server.messages().len() == 1 && owed() == 0);
        assert_eq!(code, Some(0), "{command}: {printed}");
        assert!(printed.contains(&format!("[mail] stopped_by_error for bot {bot}: sent")), "{command}: {printed}");
        let mail = &plain.messages()[0];
        assert!(mail.contains("\r\nFrom: bots@example.com\r\nTo: o@example.com\r\n") && mail.contains("\r\nSubject: Weekly BTC has been stopped\r\n"), "{command}: {mail}");
        assert_eq!(plain.sessions().last().unwrap().lines[..2], ["EHLO localhost", "AUTH PLAIN AAA="], "{command}");
    }
}

#[test]
fn check_reports_each_jobs_last_run_and_each_reference_sources_age() {
    let (dir, o, _s) = common::install_alpaca();
    deltabadger::jobs::state::record_success(&o.primary, "sync_alpaca_crypto_from_deltabadger_job", None, "2026-09-01T10:15:04Z".parse().unwrap()).unwrap();
    drop(o);
    let out = cli(dir.path(), &["check"]).env_remove("SECRET_KEY_BASE").output().unwrap(); // check needs no keys, job state included
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout.contains("job sync_alpaca_crypto_from_deltabadger_job: last success 2026-09-01T10:15:04Z"), "{stdout}");
    assert!(stdout.contains("reference Alpaca crypto tickers"), "{stdout}");
    assert!(stdout.contains("reference Indices: no row"), "{stdout}");
}


async fn scheduler_process(command: &str, restart: bool) -> Result<(), Box<dyn std::error::Error>> {
    use rusqlite::OptionalExtension;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/api/v2/listings")).and(query_param("venue", "alpaca_crypto"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": [] })))
        .expect(1..).mount(&server).await;
    let (dir, o, s) = common::install_alpaca();
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2099-01-01 00:00:00"));
    if restart {
        let paths = deltabadger::store::Paths::from_env(&|_| None, dir.path());
        let lock = deltabadger::lease::lock(&paths, chrono::Utc::now()).map_err(|e| format!("{e:?}"))?;
        deltabadger::engine::handover::take_over(&lock, &o, &seed::cipher(), "test", chrono::Utc::now()).map_err(|e| format!("{e:?}"))?;
        o.primary.execute("UPDATE app_configs SET value = json_set(value, '$.last_success_at', '2020-01-01T00:00:00Z', '$.rails_at', NULL, '$.incomplete_since', '2020-01-01T00:00:00Z') WHERE key = ?1",
            ["rust_job.sync_alpaca_crypto_from_deltabadger_job"])?;
        assert!(deltabadger::engine::eligibility::check_install_at(&o.primary, chrono::Utc::now())
            .map_err(|e| format!("{e:?}"))?.problems.iter().any(|m| m.contains("reference data stale")));
    }
    drop(o);
    let mut child = Running(cli(dir.path(), &[command]).env("MARKET_DATA_URL", server.uri()).env("MARKET_DATA_TOKEN", "tok")
        .env("PORT", free_port().to_string()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let state = loop {
        assert!(child.0.try_wait()?.is_none(), "{command} exited before the scheduler ran (restart={restart})");
        let c = rusqlite::Connection::open(dir.path().join("production.sqlite3"))?;
        let row: Option<String> = c.query_row("SELECT value FROM app_configs WHERE key = ?1",
            ["rust_job.sync_alpaca_crypto_from_deltabadger_job"], |r| r.get(0)).optional()?;
        if let Some(v) = row.filter(|v| serde_json::from_str::<serde_json::Value>(v).is_ok_and(|j| j["last_error"].is_string())) { break v; }
        assert!(std::time::Instant::now() < deadline, "the crypto job never recorded its run");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert!(state.contains("degraded listings payload"), "{state}");
    assert!(Command::new("kill").args(["-TERM", &child.0.id().to_string()]).status()?.success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        if let Some(st) = child.0.try_wait()? { break st; }
        assert!(std::time::Instant::now() < deadline, "{command} did not exit on SIGTERM");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(status.code(), Some(0), "the scheduler stops and the engine drains");
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn run_pulls_reference_data_at_start_records_each_jobs_state_and_stops_on_sigterm() -> Result<(), Box<dyn std::error::Error>> {
    scheduler_process("run", false).await
}

#[tokio::test(flavor = "current_thread")]
async fn serve_pulls_reference_data_at_start_and_stops_on_sigterm() -> Result<(), Box<dyn std::error::Error>> {
    scheduler_process("serve", false).await
}

#[tokio::test(flavor = "current_thread")]
async fn run_and_serve_restart_a_stale_engine_owned_install_and_start_the_scheduler() -> Result<(), Box<dyn std::error::Error>> {
    for command in ["run", "serve"] { scheduler_process(command, true).await?; }
    Ok(())
}

#[test]
fn run_and_serve_refuse_stale_reference_data_while_rails_owns_the_install() -> Result<(), Box<dyn std::error::Error>> {
    for command in ["run", "serve"] {
        let (dir, o, s) = common::install_alpaca();
        seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2099-01-01 00:00:00"));
        o.primary.execute("UPDATE exchange_assets SET updated_at = '2020-01-01 00:00:00'", [])?;
        let out = finished(cli(dir.path(), &[command]).env("PORT", free_port().to_string()));
        assert_eq!(out.status.code(), Some(1), "{command}: {}", stderr(&out));
        assert!(stderr(&out).contains("reference data stale"));
        assert!(deltabadger::lease::read(&o.primary, &seed::cipher()).map_err(|e| format!("{e:?}"))?.is_none());
    }
    Ok(())
}
