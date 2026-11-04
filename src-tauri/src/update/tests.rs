//! The agent's state machine (PLAN-B7 r1.2, r1.11, r1.12, r5.2) with a stub app: every transition, and a quit winning at
//! each step of Stopping.
use super::*;
use std::sync::atomic::AtomicUsize;

fn agent() -> (Arc<Agent>, Arc<Mutex<Vec<Snapshot>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    (Arc::new(Agent::new(move |s| sink.lock().unwrap().push(s.clone()))), seen)
}

fn package(version: &str, critical: bool) -> Package { Package { version: version.into(), critical, bytes: Arc::new(vec![1, 2, 3]) } }

fn ready(agent: &Agent, version: &str) {
    assert!(agent.begin_check(false));
    assert_eq!(agent.found(version, false), Next::Download { visible: true });
    agent.downloading(false, None);
    agent.downloaded(package(version, false));
}

#[test]
fn every_change_is_emitted_once_with_a_higher_sequence() {
    let (agent, seen) = agent();
    assert_eq!(agent.snapshot(), Snapshot { seq: 0, state: State::Idle });
    assert!(agent.begin_check(true));
    agent.downloading(true, None);
    agent.downloading(true, Some(10));
    agent.downloading(true, Some(10));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.iter().map(|s| s.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(seen[2].state, State::Downloading { percent: Some(10), critical: true });
    assert_eq!(serde_json::to_value(&seen[2]).unwrap(), serde_json::json!({"seq": 3, "state": "downloading", "percent": 10, "critical": true}));
}

#[test]
fn one_check_or_download_at_a_time() {
    let (agent, _) = agent();
    assert!(agent.begin_check(false));
    assert!(!agent.begin_check(true), "a second check while one runs");
    assert_eq!(agent.found("2.0.0", false), Next::Download { visible: true });
    agent.downloading(false, Some(5));
    assert!(!agent.begin_check(true), "a check while downloading");
    agent.downloaded(package("2.0.0", false));
    assert!(agent.begin_check(true));
}

#[test]
fn background_checks_fail_silently_and_the_buttons_say_so() {
    let (agent, _) = agent();
    assert!(agent.begin_check(false));
    assert_eq!(agent.snapshot().state, State::Idle, "a background check shows nothing");
    agent.check_failed(false);
    assert_eq!(agent.snapshot().state, State::Idle);
    assert!(agent.begin_check(true));
    assert_eq!(agent.snapshot().state, State::Checking);
    agent.check_failed(true);
    assert_eq!(agent.snapshot().state, State::CheckFailed);
    assert!(agent.begin_check(true));
    agent.up_to_date();
    assert_eq!(agent.snapshot().state, State::Idle);
}

#[test]
fn a_failed_check_never_replaces_a_ready_update() {
    let (agent, _) = agent();
    ready(&agent, "2.0.0");
    for button in [false, true] {
        assert!(agent.begin_check(button));
        assert_eq!(agent.snapshot().state, State::Ready { critical: false });
        agent.check_failed(button);
        assert_eq!(agent.snapshot().state, State::Ready { critical: false });
    }
    assert!(agent.begin_check(true));
    agent.up_to_date();
    assert_eq!(agent.snapshot().state, State::Ready { critical: false }, "the package in memory stays installable");
}

#[test]
fn a_newer_release_is_downloaded_quietly_and_replaces_the_ready_one() {
    let (agent, _) = agent();
    ready(&agent, "2.0.0");
    assert!(agent.begin_check(false));
    assert_eq!(agent.found("2.0.0", false), Next::Done, "the same version isn't fetched again");
    assert!(agent.begin_check(false));
    assert_eq!(agent.found("2.1.0", false), Next::Download { visible: false });
    agent.download_failed(Kind::Download, false);
    assert_eq!(agent.snapshot().state, State::Ready { critical: false }, "a quiet failure leaves the ready update");
    assert!(agent.begin_check(false));
    assert_eq!(agent.found("2.1.0", false), Next::Download { visible: false });
    agent.downloaded(package("2.1.0", true));
    assert_eq!(agent.snapshot().state, State::Ready { critical: true });
    assert_eq!(agent.begin_install().unwrap().version, "2.1.0");
}

#[test]
fn failures_deb_and_manual() {
    let (agent, _) = agent();
    assert!(agent.begin_check(false));
    assert_eq!(agent.found("2.0.0", false), Next::Download { visible: true });
    agent.downloading(false, None);
    agent.download_failed(Kind::Verify, true);
    assert_eq!(agent.snapshot().state, State::Failed { kind: Kind::Verify });
    assert!(!agent.retry_installs(), "a verify failure retries by checking again");
    assert!(agent.begin_install().is_err());
    assert!(agent.begin_check(false));
    agent.manual();
    assert_eq!(agent.snapshot().state, State::Manual);
    assert!(agent.begin_check(false));
    assert_eq!(agent.found("2.0.0", true), Next::Done);
    assert_eq!(agent.snapshot().state, State::DebNotice);
    assert!(agent.begin_check(false), "neither leaves a check running");
}

#[test]
fn critical_and_the_kinds_of_plugin_errors() {
    assert!(critical(&serde_json::json!({"version": "2.0.0", "critical": true})));
    for raw in [serde_json::json!({}), serde_json::json!({"critical": "true"}), serde_json::json!({"critical": 1})] {
        assert!(!critical(&raw), "{raw}");
    }
    use tauri_plugin_updater::Error;
    assert_eq!(kind_of(&Error::MissingSignedVersion), Kind::Verify);
    assert_eq!(kind_of(&Error::SignatureUtf8("x".into())), Kind::Verify);
    assert_eq!(kind_of(&Error::Network("x".into())), Kind::Download);
    assert_eq!(kind_of(&Error::ReleaseNotFound), Kind::Download);
}

#[test]
fn test_settings_only_off_the_released_identifier() {
    let env = |name: &str| match name {
        "DELTABADGER_TEST_UPDATE_INTERVAL_SECS" => Some("20".to_string()),
        "DELTABADGER_TEST_UPDATE_READ_TIMEOUT_SECS" => Some("5".to_string()),
        "DELTABADGER_TEST_INSTALL_FAULT" => Some("swap".to_string()),
        _ => None,
    };
    let release = Config::read(crate::RELEASE_IDENTIFIER, false, &env);
    assert!(release.enabled);
    assert_eq!((release.interval, release.read_timeout, release.fault), (Duration::from_secs(43_200), Duration::from_secs(60), None));
    let test = Config::read("com.deltabadger.app.test", true, &env);
    assert!(test.enabled, "a debug test build checks too (the proofs)");
    assert_eq!((test.interval, test.read_timeout, test.fault.as_deref()), (Duration::from_secs(20), Duration::from_secs(5), Some("swap")));
    assert!(!Config::read("com.deltabadger.app.dev", true, &env).enabled, "tauri dev never reaches the feed");
}

/// The app, scripted: where a quit comes in, how long the lock is busy, how the install ends.
struct Stub {
    agent: Arc<Agent>,
    quit_at: &'static str,
    busy_locks: AtomicUsize,
    lock_error: bool,
    install: Mutex<Option<install::Failure>>,
    events: Mutex<Vec<String>>,
}

impl Stub {
    fn new(agent: &Arc<Agent>, quit_at: &'static str) -> Stub {
        Stub { agent: agent.clone(), quit_at, busy_locks: AtomicUsize::new(0), lock_error: false, install: Mutex::new(None), events: Mutex::new(Vec::new()) }
    }
    fn event(&self, e: &str) { self.events.lock().unwrap().push(e.to_string()) }
    fn events(&self) -> Vec<String> { self.events.lock().unwrap().clone() }
}

impl Host for Stub {
    fn show(&self, page: &str) { self.event(&format!("show {page}")) }
    fn stop_and_wait(&self, cancelled: &dyn Fn() -> bool) -> bool {
        self.event("stop");
        if self.quit_at == "child" {
            self.agent.quit();
        }
        !cancelled()
    }
    fn bundle_lock(&self) -> Result<Option<Box<dyn std::any::Any>>, String> {
        self.event("lock");
        if self.lock_error {
            return Err("unhealthy".into());
        }
        if self.busy_locks.load(Ordering::SeqCst) > 0 {
            self.busy_locks.fetch_sub(1, Ordering::SeqCst);
            if self.quit_at == "lock" {
                self.agent.quit();
            }
            return Ok(None);
        }
        if self.quit_at == "locked" {
            self.agent.quit();
        }
        Ok(Some(Box::new(())))
    }
    fn install(&self, _: &Package) -> Result<(), install::Failure> {
        self.event("install");
        assert_eq!(self.agent.snapshot().state, State::Installing);
        if self.quit_at == "installing" {
            assert!(!self.agent.quit(), "a quit can't stop the replacement once it began");
        }
        self.install.lock().unwrap().take().map_or(Ok(()), Err)
    }
    fn respawn(&self) { self.event("respawn") }
    fn relaunch(&self) { self.event("relaunch") }
    fn log(&self, _: &str) {}
}

const FAST: Timing = Timing { poll: Duration::from_millis(1), lock_retry: Duration::from_millis(5), long_after: Duration::from_secs(60) };

fn install_with(stub: &Stub) {
    let package = stub.agent.begin_install().unwrap();
    stub.agent.run_install(stub, &package, &FAST);
}

#[test]
fn the_click_stops_the_engine_takes_the_lock_installs_and_relaunches() {
    let (agent, _) = agent();
    assert!(agent.begin_install().is_err(), "nothing ready");
    ready(&agent, "2.0.0");
    let stub = Stub::new(&agent, "");
    stub.busy_locks.store(2, Ordering::SeqCst);
    install_with(&stub);
    assert_eq!(stub.events(), ["show updating", "stop", "lock", "lock", "lock", "install", "relaunch"]);
    assert_eq!(agent.snapshot().state, State::Installing);
    assert!(agent.begin_install().is_err(), "one install");
}

#[test]
fn a_long_stop_adds_u8() {
    let (agent, _) = agent();
    ready(&agent, "2.0.0");
    let stub = Stub::new(&agent, "");
    stub.busy_locks.store(1, Ordering::SeqCst);
    let package = agent.begin_install().unwrap();
    assert_eq!(agent.snapshot().state, State::Stopping { long: false });
    agent.run_install(&stub, &package, &Timing { long_after: Duration::ZERO, ..FAST });
    assert!(stub.events().contains(&"show updating-long".to_string()));
}

#[test]
fn a_quit_wins_at_every_step_of_stopping() {
    for (step, events) in [
        ("child", vec!["show updating", "stop"]),
        ("lock", vec!["show updating", "stop", "lock"]),
        ("locked", vec!["show updating", "stop", "lock"]),
    ] {
        let (agent, _) = agent();
        ready(&agent, "2.0.0");
        let stub = Stub::new(&agent, step);
        stub.busy_locks.store(if step == "lock" { 1 } else { 0 }, Ordering::SeqCst);
        install_with(&stub);
        assert_eq!(stub.events(), events, "{step}: nothing installed, nothing respawned");
        assert!(agent.quitting());
        assert!(!agent.begin_check(true), "{step}: no check while quitting");
    }
}

#[test]
fn a_quit_before_the_click_refuses_it() {
    let (agent, _) = agent();
    ready(&agent, "2.0.0");
    assert!(agent.quit());
    assert!(agent.begin_install().is_err());
}

#[test]
fn a_failed_install_respawns_the_engine_and_can_be_retried() {
    let (agent, _) = agent();
    ready(&agent, "2.0.0");
    let stub = Stub::new(&agent, "");
    *stub.install.lock().unwrap() = Some(install::Failure { reason: "test".into(), space: false });
    install_with(&stub);
    assert_eq!(stub.events(), ["show updating", "stop", "lock", "install", "respawn"]);
    assert_eq!(agent.snapshot().state, State::Failed { kind: Kind::Install });
    assert!(agent.retry_installs());
    let stub = Stub::new(&agent, "");
    install_with(&stub);
    assert_eq!(stub.events().last().unwrap(), "relaunch");
}

#[test]
fn a_quit_during_a_failed_replacement_still_means_no_respawn() {
    let (agent, _) = agent();
    ready(&agent, "2.0.0");
    let stub = Stub::new(&agent, "installing");
    *stub.install.lock().unwrap() = Some(install::Failure { reason: "test".into(), space: false });
    install_with(&stub);
    assert_eq!(stub.events(), ["show updating", "stop", "lock", "install"], "no respawn after the quit");
    assert!(agent.quitting());
}

#[test]
fn an_older_release_never_replaces_a_newer_one_in_memory() {
    let (agent, _) = agent();
    ready(&agent, "2.2.0");
    assert!(agent.begin_check(true));
    assert_eq!(agent.found("2.1.0", false), Next::Done);
    assert_eq!(agent.snapshot().state, State::Ready { critical: false });
    assert_eq!(agent.begin_install().unwrap().version, "2.2.0");
}

#[test]
fn a_full_disk_is_its_own_failure() {
    let (agent, _) = agent();
    ready(&agent, "2.0.0");
    let stub = Stub::new(&agent, "");
    *stub.install.lock().unwrap() = Some(install::Failure { reason: "full".into(), space: true });
    install_with(&stub);
    assert_eq!(agent.snapshot().state, State::Failed { kind: Kind::Space });
    assert!(!agent.retry_installs(), "a full disk checks and downloads again");
}

#[test]
fn an_unhealthy_lock_means_no_in_app_install() {
    let (agent, _) = agent();
    ready(&agent, "2.0.0");
    let mut stub = Stub::new(&agent, "");
    stub.lock_error = true;
    install_with(&stub);
    assert_eq!(stub.events(), ["show updating", "stop", "lock", "respawn"]);
    assert_eq!(agent.snapshot().state, State::Manual);
}

#[test]
fn a_download_finishing_during_an_install_leaves_the_state_alone() {
    let (agent, _) = agent();
    ready(&agent, "2.0.0");
    assert!(agent.begin_check(false));
    assert_eq!(agent.found("2.1.0", false), Next::Download { visible: false });
    let _ = agent.begin_install().unwrap();
    agent.downloaded(package("2.1.0", false));
    assert_eq!(agent.snapshot().state, State::Stopping { long: false });
}
