//! A21 F24: AppUpdate and AppUpdate::CheckJob against what Rails recorded (script/rust/record_app_update.rb →
//! fixtures/app_update_vectors.json), and the job's schedule, storage and handover.
mod common;
use chrono::{DateTime, Duration, Utc};
use deltabadger::engine::FixedClock;
use deltabadger::jobs::app_update::{self, Check, Fetch, FetchFuture, Platform, Release};
use deltabadger::jobs::{Cx, Db, Job, Outcome};
use deltabadger::store::{self, Paths};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::cell::RefCell;

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/app_update_vectors.json")).unwrap() }
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn db() -> (tempfile::TempDir, Connection) {
    let dir = common::baseline_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    (dir, o.primary)
}

#[test]
fn pins_are_current() {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in vectors()["sources"].as_object().unwrap() {
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap(), "re-record {path}: script/rust/repin");
    }
    assert_eq!(vectors()["cache_ttl_secs"], json!(app_update::ttl().num_seconds()));
    assert_eq!(vectors()["compose_command"], json!(app_update::COMPOSE_COMMAND));
}

/// The answers record_app_update.rb scripts, in its order: `(case, status, content type, body)`, or None for its timeout.
fn answers() -> Vec<(&'static str, Option<(u16, &'static str, String)>)> {
    let json = "application/json; charset=utf-8";
    vec![
        ("success", Some((200, json, json!({ "tag_name": "v3.0.1", "html_url": "https://evil.invalid/" }).to_string()))),
        ("server_error", Some((500, json, r#"{"message":"oops"}"#.into()))),
        ("timeout", None),
        ("prerelease_tag", Some((200, json, json!({ "tag_name": "v3.1.0-beta.1" }).to_string()))),
        ("nightly_tag", Some((200, json, json!({ "tag_name": "nightly" }).to_string()))),
        ("array_body", Some((200, json, "[]".into()))),
        ("not_json", Some((200, "text/plain", "hello".into()))),
        ("redirect", Some((302, "", String::new()))),
        ("redirect_with_release", Some((302, json, json!({ "tag_name": "v3.0.3" }).to_string()))),
        ("not_found_with_release", Some((404, json, json!({ "tag_name": "v3.0.4" }).to_string()))),
        ("missing_tag", Some((200, json, "{}".into()))),
        ("bare_tag", Some((200, json, json!({ "tag_name": "3.0.2" }).to_string()))),
        ("four_part_tag", Some((200, json, json!({ "tag_name": "v3.0.2.1" }).to_string()))),
    ]
}

/// One scripted answer per run, counting the requests.
struct Scripted { next: RefCell<Option<Result<(u16, String, Vec<u8>), String>>>, calls: RefCell<usize> }
impl Fetch for Scripted {
    fn latest<'a>(&'a self) -> FetchFuture<'a> {
        *self.calls.borrow_mut() += 1;
        let next = self.next.borrow_mut().take().expect("one answer per run");
        Box::pin(async move { next })
    }
}

fn release_json(r: Option<Release>) -> Value { r.map_or(Value::Null, |r| json!({ "version": r.version, "url": r.url })) }

/// refresh! per answer against one cache: what it returns, and what the cache holds after (a failure keeps the last).
#[tokio::test(flavor = "current_thread")]
async fn refresh_matches_rails_answer_by_answer() {
    let recorded = vectors()["refresh"].as_array().unwrap().clone();
    let answers = answers();
    assert_eq!(recorded.iter().map(|r| r["case"].as_str().unwrap()).collect::<Vec<_>>(), answers.iter().map(|a| a.0).collect::<Vec<_>>());
    let (_d, c) = db();
    let db = Db::new(c, common::seed::cipher());
    let now = at("2026-11-03T12:00:00Z");
    for ((name, answer), rails) in answers.into_iter().zip(recorded) {
        let returned = answer.as_ref().and_then(|(s, ct, b)| app_update::release_from(*s, ct, b.as_bytes()));
        assert_eq!(release_json(returned), rails["returned"], "{name}: returned");
        let job = Check { enabled: true, fetch: Scripted { next: RefCell::new(Some(answer.map(|(s, ct, b)| (s, ct.to_string(), b.into_bytes())).ok_or_else(|| "timeout".to_string()))), calls: RefCell::new(0) } };
        let outcome = job.run(Cx { db: db.clone(), clock: &FixedClock(now), wakers: Default::default() }, vec![]).await;
        assert_eq!(outcome, if rails["returned"].is_null() { Outcome::NothingNew } else { Outcome::Done }, "{name}: outcome");
        assert_eq!(*job.fetch.calls.borrow(), 1);
        let cached = db.run(move |c, _| app_update::latest(c, now)).await.unwrap();
        assert_eq!(release_json(cached), rails["cached"], "{name}: cached");
    }
}

/// Clients::Github's request: the one URL, Rails' two headers; a 3xx is an answer, never a second request.
#[tokio::test(flavor = "current_thread")]
async fn the_request_is_rails_and_a_redirect_is_not_followed() {
    use wiremock::{matchers, Mock, MockServer, ResponseTemplate};
    let (github, elsewhere) = (MockServer::start().await, MockServer::start().await);
    Mock::given(matchers::method("GET")).and(matchers::path("/repos/deltabadger/deltabadger/releases/latest"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", format!("{}/x", elsewhere.uri()))
            .set_body_raw(r#"{"tag_name":"v3.0.3"}"#, "application/json")).expect(1).mount(&github).await;
    Mock::given(matchers::any()).respond_with(ResponseTemplate::new(200)).expect(0).mount(&elsewhere).await;
    let fetch = app_update::LiveFetch::at(&format!("{}/repos/deltabadger/deltabadger/releases/latest", github.uri()));
    let (status, content_type, body) = fetch.latest().await.unwrap();
    assert_eq!((status, app_update::release_from(status, &content_type, &body).map(|r| r.version)), (302, Some("3.0.3".to_string())));
    let request = &github.received_requests().await.unwrap()[0];
    let rails = &vectors()["refresh"][0]["requests"][0];
    // WebMock writes the default port out.
    assert_eq!(rails["url"], json!(app_update::LATEST.replacen("api.github.com", "api.github.com:443", 1)));
    for (name, value) in rails["headers"].as_object().unwrap() {
        assert_eq!(request.headers.get(name.as_str()).unwrap().to_str().unwrap(), value.as_str().unwrap(), "{name}");
    }
}

/// The desktop and an opted-out install never ask (Rails: refresh! sends nothing; the job returns before it).
#[tokio::test(flavor = "current_thread")]
async fn desktop_and_opted_out_never_call_github() {
    for case in vectors()["silent"].as_array().unwrap() {
        assert_eq!(case["requests"], json!([]));
        let env = case["env"].clone();
        let enabled = app_update::enabled(&|k| env[k].as_str().map(String::from));
        assert!(!enabled);
        let (_d, c) = db();
        let job = Check { enabled, fetch: Scripted { next: RefCell::new(None), calls: RefCell::new(0) } };
        let outcome = job.run(Cx { db: Db::new(c, common::seed::cipher()), clock: &FixedClock(at("2026-11-03T12:00:00Z")), wakers: Default::default() }, vec![]).await;
        assert_eq!((outcome, *job.fetch.calls.borrow()), (Outcome::NothingNew, 0));
    }
    // The registered job reads the process environment as the scheduler starts it.
    assert!(!app_update::enabled(&|k| (k == "DELTABADGER_PLATFORM").then(|| "desktop".to_string())));
}

#[test]
fn available_is_gem_version_pair_by_pair() {
    for case in vectors()["available"].as_array().unwrap() {
        let (current, latest) = (case["current"].as_str().unwrap(), case["latest"].as_str().unwrap());
        assert_eq!(json!(app_update::available(current, latest)), case["available"], "{current:?} vs {latest:?}");
    }
}

#[test]
fn platform_and_switch_are_rails_per_environment() {
    for case in vectors()["enabled"].as_array().unwrap() {
        let env = |k: &str| match k { "DELTABADGER_PLATFORM" => case["platform_marker"].as_str().map(String::from), "DELTABADGER_UPDATE_CHECK" => case["update_check"].as_str().map(String::from), _ => None };
        let platform = match app_update::platform(&env) { Platform::Desktop => "desktop", Platform::Umbrel => "umbrel", Platform::Docker => "docker", Platform::Unknown => "unknown" };
        assert_eq!((json!(platform), json!(app_update::enabled(&env))), (case["platform"].clone(), case["enabled"].clone()), "{case}");
    }
}

/// The answer outlives a restart (it is a row), is gone exactly 25 h after it was fetched, and a failed run never renews it.
#[tokio::test(flavor = "current_thread")]
async fn the_answer_lasts_twenty_five_hours_from_its_fetch() {
    let (d, c) = db();
    let fetched = at("2026-11-03T00:00:00Z");
    let release = Release { version: "3.0.1".into(), url: "https://github.com/deltabadger/deltabadger/releases/tag/v3.0.1".into() };
    app_update::store(&c, &release, fetched).unwrap();
    drop(c);
    let c = Connection::open(d.path().join("production.sqlite3")).unwrap();
    assert_eq!(app_update::latest(&c, fetched + Duration::hours(25) - Duration::seconds(1)).unwrap(), Some(release.clone()));
    assert_eq!(app_update::latest(&c, fetched + Duration::hours(25)).unwrap(), None);
    let db = Db::new(c, common::seed::cipher());
    let job = Check { enabled: true, fetch: Scripted { next: RefCell::new(Some(Ok((500, "application/json".into(), b"{}".to_vec())))), calls: RefCell::new(0) } };
    assert_eq!(job.run(Cx { db: db.clone(), clock: &FixedClock(fetched + Duration::hours(20)), wakers: Default::default() }, vec![]).await, Outcome::NothingNew);
    let later = fetched + Duration::hours(25);
    assert_eq!(db.run(move |c, _| app_update::latest(c, later)).await.unwrap(), None);
}

/// config/recurring.yml `0 */12 * * *`; a queued Rails check runs once after a takeover.
#[test]
fn schedule_and_handover() {
    use deltabadger::jobs::schedule::Schedule;
    let spec = app_update::spec();
    assert_eq!((spec.name, spec.schedule), ("app_update_check_job", Some(Schedule::EveryHours { every: 12, minute: 0 })));
    let s = spec.schedule.unwrap();
    assert_eq!(s.next_fire(at("2026-11-03T05:00:00Z")), at("2026-11-03T12:00:00Z"));
    assert_eq!(s.next_fire(at("2026-11-03T12:00:00Z")), at("2026-11-04T00:00:00Z"));
    assert!(matches!(deltabadger::engine::handover::rails_work("AppUpdate::CheckJob"), deltabadger::engine::handover::RailsWork::WakeUnscoped("app_update_check_job")));
    assert!(matches!(deltabadger::engine::handover::rails_work("Asset::InferColorFromImageJob"), deltabadger::engine::handover::RailsWork::Wake("infer_color_from_image_job")));
}
