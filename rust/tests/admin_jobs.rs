//! A25 (PLAN-A25 r4): the admin jobs page at /jobs over Rust's scheduler and engine. Rails-free: the crate's own schema,
//! a scheduler built from the crate's registration functions (never run: no outbound call), the web in-process.
mod common;
use common::seed::{self, BotSpec, Seeded};
use common::web::{Answer, Browser, Csrf};
use deltabadger::jobs::{self, state, Scheduler};
use deltabadger::web::{csrf, session, App};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::rc::Rc;

const NOW: &str = "2026-09-30T12:00:00Z";
type Api = Rc<Option<jobs::data_api::DataApi<deltabadger::venue::http::ReqwestTransport>>>;

struct Fx { dir: tempfile::TempDir, c: Connection, s: Seeded, app: App, scheduler: Scheduler }

fn path(dir: &Path) -> PathBuf { dir.join("primary.sqlite3") }

fn env(n: &str) -> Option<String> { (n == "SECRET_KEY_BASE").then(|| "admin-jobs-secret".to_string()) }

/// Every job the crate registers at a start (main.rs scheduler_service), wrapped as main wraps them.
fn registered(c: &Connection) -> Vec<Box<dyn jobs::Job>> {
    use deltabadger::venue::alpaca::LiveFactory;
    let api: Api = Rc::new(None);
    let mut all = jobs::reference::shared_jobs(api.clone());
    all.push(Box::new(jobs::setup_sync::SeedAndSync::new(&env, jobs::data_api::DataApi::live)));
    all.push(jobs::app_update::job(&env));
    all.extend(deltabadger::sync::jobs::register(c, &LiveFactory::new(), api.clone()).unwrap());
    all.extend(deltabadger::tracker::jobs::register(c, &LiveFactory::new(), api.clone(), deltabadger::tracker::jobs::system_wall()).unwrap());
    all.push(deltabadger::tax::job::job(api.clone(), LiveFactory::new(), deltabadger::tax::job::tmp_dir(&env)));
    all.push(Box::new(deltabadger::sync::binance_repair::HistoryRepair::new(LiveFactory::new())));
    all.push(Box::new(deltabadger::identity::Discovery { venues: LiveFactory::new() }));
    all.extend(jobs::reference::catalogue_jobs(&env, PathBuf::from(jobs::reference::BUNDLE), jobs::reference::LiveFetch::default()));
    all.push(Box::new(deltabadger::sync::jobs::IbkrActivation { venues: LiveFactory::new() }));
    all.push(Box::new(deltabadger::sync::binance_order_links::OrderLinks));
    all.push(Box::new(deltabadger::engine::rails_unrecorded::Report::new(LiveFactory::new())));
    let rates = std::sync::Arc::new(jobs::data_api::RateCache::default());
    all.into_iter().map(|job| jobs::resolve::configured(job, &env, rates.clone())).collect()
}

/// A job registered with a scope no job of this crate makes (text): it must render hidden.
struct Texty(&'static str);
impl jobs::Job for Texty {
    fn spec(&self) -> jobs::Spec { jobs::Spec { name: "texty", scope: Some(self.0.into()), schedule: None, jitter: jobs::schedule::Jitter::NONE, retry: jobs::Retry::None, deadline: jobs::DEADLINE } }
    fn run<'a>(&'a self, _: jobs::Cx<'a>, _: Vec<jobs::Wake>) -> jobs::JobFuture<'a> { Box::pin(async { jobs::Outcome::Done }) }
}

fn fx() -> Fx { fx_with(vec![]) }

fn fx_with(extra: Vec<Box<dyn jobs::Job>>) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let c = Connection::open(path(dir.path())).unwrap();
    deltabadger::store::create_schema(&c).unwrap();
    deltabadger::store::test_support::migrate(&c).unwrap();
    let s = seed::seed_kraken(&c, &seed::cipher());
    c.execute("UPDATE users SET confirmed_at = created_at, setup_completed = 1, wash_sale_enabled = 0", []).unwrap();
    let primary = Connection::open(path(dir.path())).unwrap();
    deltabadger::store::configure(&primary).unwrap();
    let app = App::new(deltabadger::web::Config::from_env(&env).unwrap(), &env, primary, common::web::TestClock::at(NOW)).unwrap();
    let api: Api = Rc::new(None);
    let scheduler = Scheduler::new(Connection::open(path(dir.path())).unwrap(), seed::cipher(), registered(&c).into_iter().chain(extra).collect(), None)
        .with_resolver(jobs::resolve::all(deltabadger::venue::alpaca::LiveFactory::new(), api, deltabadger::tracker::jobs::system_wall()));
    app.attach_jobs(scheduler.wakers()).unwrap();
    Fx { dir, c, s, app, scheduler }
}

impl Fx {
    /// Publishes the listing (as the running scheduler does each turn) and returns its (name, scope) slots.
    fn slots(&mut self) -> Vec<(String, Option<String>)> { self.scheduler.slots(common::web::at(NOW)).unwrap() }

    /// A browser signed in as `user`.
    fn browser(&self, user: i64) -> Browser {
        let hash: String = self.c.query_row("SELECT encrypted_password FROM users WHERE id = ?1", [user], |r| r.get(0)).unwrap();
        let token = csrf::new_token();
        let data = session::SessionData { user: Some((user, hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
        Browser { cookie: Some(session::seal(&self.app.keys.session, &data, self.app.now())), page: Some(format!("<meta name=\"csrf-token\" content=\"{}\">", csrf::masked(&token))) }
    }

    fn member(&self) -> i64 {
        self.c.execute("INSERT INTO users (email, encrypted_password, name, admin, confirmed_at, setup_completed, created_at, updated_at) \
                        VALUES ('member@example.com', 'x', 'Member', 0, '2026-01-01', 1, '2026-01-01', '2026-01-01')", []).unwrap();
        self.c.last_insert_rowid()
    }

    async fn page(&self, b: &mut Browser) -> Answer { b.get(&self.app, "/jobs").await }

    async fn run_now(&self, b: &mut Browser, job: &str, scope: &str, csrf: Csrf) -> Answer {
        b.send(&self.app, "POST", "/jobs", Some(&[("job", job), ("scope", scope)]), csrf, &[]).await
    }

    fn pending(&self) -> Vec<(String, Option<String>)> { self.app.job_wakers().unwrap().pending() }
}

fn t(s: &str) -> chrono::DateTime<chrono::Utc> { s.parse().unwrap() }

/// Every slot the scheduler holds is a row of the page, the late-resolved scope included; nothing else is.
#[tokio::test(flavor = "current_thread")]
async fn the_page_lists_every_registered_slot() {
    let mut f = fx();
    let user = f.s.user_id.to_string();
    f.app.job_wakers().unwrap().wake(deltabadger::tracker::jobs::TRACKER_LEDGER, Some(&user), None);
    let slots = f.slots();
    assert!(slots.len() > 15, "{slots:?}");
    assert!(slots.contains(&(deltabadger::tracker::jobs::TRACKER_LEDGER.into(), Some(user.clone()))), "the late scope resolved");
    let mut b = f.browser(f.s.user_id);
    let page = f.page(&mut b).await;
    assert_eq!(page.status, 200, "{}", page.body);
    for (name, scope) in &slots {
        let row = format!("<td>{name}</td><td>{}</td>", scope.as_deref().unwrap_or(""));
        assert_eq!(page.body.matches(&row).count(), 1, "{name} {scope:?}: one row\n{}", page.body);
    }
    assert_eq!(page.body.matches("<tr>\n      <td>").count(), slots.len(), "one row per slot, no other");
    // The scope factory is listed without a button; a reference job has one.
    let factory = page.body.split("<td>credential_scope_factory</td>").nth(1).unwrap().split("</tr>").next().unwrap();
    assert!(!factory.contains("<form"), "{factory}");
    let stocks = page.body.split(&format!("<td>{}</td>", jobs::reference::STOCKS)).nth(1).unwrap().split("</tr>").next().unwrap();
    assert!(stocks.contains("Run now") && stocks.contains("daily at 10:00 UTC"), "{stocks}");
}

/// Failing reads the latest error against the latest refresh; the error is its time and a fixed phrase only.
#[tokio::test(flavor = "current_thread")]
async fn a_failing_job_shows_its_error_time_and_a_phrase_never_its_text() {
    let mut f = fx();
    let job = jobs::reference::STOCKS;
    let row = |body: &str| body.split(&format!("<td>{job}</td>")).nth(1).unwrap().split("</tr>").next().unwrap().to_string();
    f.slots();
    let mut b = f.browser(f.s.user_id);
    state::record_success(&f.c, job, None, t("2026-09-29T10:00:00Z")).unwrap();
    state::record_error(&f.c, job, None, t("2026-09-30T10:00:00Z"), "data-api 503: upstream said nope").unwrap();
    state::record_error(&f.c, job, None, t("2026-09-30T11:00:00Z"), "data-api 503: upstream said nope again").unwrap();
    let r = row(&f.page(&mut b).await.body);
    assert!(r.contains("2026-09-30 11:00:00 UTC") && r.contains("failed — diagnostic text omitted") && !r.contains("upstream"), "{r}");
    // A run that refreshed nothing does not end the failure.
    state::record_run(&f.c, job, None, t("2026-09-30T11:30:00Z")).unwrap();
    let r = row(&f.page(&mut b).await.body);
    assert!(r.contains("<td>2026-09-30 11:30:00 UTC</td><td>2026-09-29 10:00:00 UTC</td>") && r.contains("failed —"), "{r}");
    // A run dropped at its deadline, its message cut at 200 characters.
    let long = format!("dropped past its 600s deadline {}", "x".repeat(300));
    state::record_error(&f.c, job, None, t("2026-09-30T11:40:00Z"), &long).unwrap();
    assert!(row(&f.page(&mut b).await.body).contains("<td>timed out</td>"));
    // Recovered: the latest error's time only.
    state::record_success(&f.c, job, None, t("2026-09-30T11:50:00Z")).unwrap();
    let r = row(&f.page(&mut b).await.body);
    assert!(r.contains("2026-09-30 11:40:00 UTC") && !r.contains("timed out") && !r.contains("failed"), "{r}");
}

/// Secrets seeded into every slot's stored error, and into scopes, never reach the response, attributes included.
#[tokio::test(flavor = "current_thread")]
async fn seeded_secrets_in_errors_and_scopes_never_appear() {
    let hidden = ["hunter2", "member@example.com", "tok3n-scope"];
    let mut f = fx_with(hidden.iter().map(|s| Box::new(Texty(s)) as Box<dyn jobs::Job>).collect());
    let secrets = ["AKIAREALKEY1234567890", "hunter2", "Bearer shortkey", r#"{"password":"x9"}"#, "member@example.com",
                   r#"{"p":"c2VjcmV0","h":{"iv":"aXY="}}"#, "https://data.example/x?token=tok3n", "venue said: invalid signature for key 42"];
    let slots = f.slots();
    for (i, secret) in secrets.iter().enumerate() {
        for (name, scope) in &slots {
            state::record_error(&f.c, name, scope.as_deref(), t("2026-09-30T11:00:00Z") + chrono::Duration::seconds(i as i64), secret).unwrap();
        }
        let mut b = f.browser(f.s.user_id);
        let page = f.page(&mut b).await;
        assert_eq!(page.status, 200);
        let whole = format!("{:?}\n{}", page.headers, page.body);
        assert!(!whole.contains(secret), "{secret} leaked:\n{}", page.body);
        for scope in hidden { assert!(!whole.contains(scope), "scope {scope} leaked"); }
        assert!(!whole.contains("venue said") && !whole.contains("data.example"));
    }
    let mut b = f.browser(f.s.user_id);
    let page = f.page(&mut b).await;
    assert_eq!(page.body.matches("(scope hidden)").count(), hidden.len(), "{}", page.body);
    // No email at all in the page's own part, the admin's included: owners are ids.
    assert!(!page.body.split("<h1>Jobs</h1>").nth(1).unwrap().contains('@'));
}

/// Run now wakes exactly the listed slot; anything else is 422 and wakes nothing.
#[tokio::test(flavor = "current_thread")]
async fn run_now_wakes_exactly_that_job_and_scope() {
    let mut f = fx_with(vec![Box::new(Texty("hidden-scope"))]);
    let wakers = f.app.job_wakers().unwrap();
    wakers.wake(jobs::reference::ASSET_DATA, Some("4242"), None);
    f.slots();
    let mut b = f.browser(f.s.user_id);
    let posted = f.run_now(&mut b, jobs::reference::STOCKS, "", Csrf::Header).await;
    assert_eq!((posted.status, posted.header("location")), (303, Some("/jobs")), "{}", posted.body);
    assert_eq!(f.pending(), [(jobs::reference::STOCKS.to_string(), None)]);
    let posted = f.run_now(&mut b, jobs::reference::ASSET_DATA, "4242", Csrf::Header).await;
    assert_eq!(posted.status, 303);
    assert_eq!(f.pending()[1], (jobs::reference::ASSET_DATA.to_string(), Some("4242".to_string())));
    let before = f.pending();
    for (job, scope) in [("no_such_job", ""), (jobs::reference::STOCKS, "7"), (jobs::reference::ASSET_DATA, "9999"),
                         ("texty", "hidden-scope"), ("credential_scope_factory", ""), ("", "")] {
        let refused = f.run_now(&mut b, job, scope, Csrf::Header).await;
        assert_eq!(refused.status, 422, "{job} {scope}");
    }
    assert_eq!(f.pending(), before, "nothing woken");
}

/// CSRF and the admin gate come before any wake.
#[tokio::test(flavor = "current_thread")]
async fn a_forged_or_non_admin_post_wakes_nothing() {
    let mut f = fx();
    f.slots();
    let mut admin = f.browser(f.s.user_id);
    let forged = f.run_now(&mut admin, jobs::reference::STOCKS, "", Csrf::None).await;
    assert_eq!((forged.status, forged.header("location")), (302, Some("/")), "the pipeline's unverified-request answer");
    let member = f.member();
    let mut b = f.browser(member);
    assert_eq!(f.run_now(&mut b, jobs::reference::STOCKS, "", Csrf::Header).await.status, 404);
    let mut out = Browser::default();
    assert_eq!(out.get(&f.app, "/login").await.status, 200);
    let signed_out = f.run_now(&mut out, jobs::reference::STOCKS, "", Csrf::Header).await;
    assert_eq!((signed_out.status, signed_out.header("location")), (302, Some("/login")));
    assert!(f.pending().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn who_sees_it() {
    let mut f = fx();
    f.slots();
    let mut admin = f.browser(f.s.user_id);
    assert_eq!(f.page(&mut admin).await.status, 200);
    let member = f.member();
    let mut b = f.browser(member);
    let page = f.page(&mut b).await;
    assert_eq!(page.status, 404);
    assert!(!page.body.contains("Jobs</h1>"));
    let mut out = Browser::default();
    let page = out.get(&f.app, "/jobs").await;
    assert_eq!((page.status, page.header("location")), (302, Some("/login")));
    for browser in [&mut admin, &mut b, &mut out] {
        for p in ["/jobs/x", "/jobs/applications/1/jobs", "/jobs.json"] {
            let answer = browser.get(&f.app, p).await;
            assert_eq!(answer.status, if p == "/jobs.json" { 501 } else { 404 }, "{p}: {}", answer.body);
        }
        assert_eq!(browser.get(&f.app, "/de/jobs").await.status, 501, "/de/jobs stays the refusal");
    }
}

/// The engine section: one read over bots, row-local about malformed data; another user's bot carries no link or name.
#[tokio::test(flavor = "current_thread")]
async fn the_engine_section_lists_bots_by_what_holds_them() {
    let mut f = fx();
    f.slots();
    let spec = || BotSpec::weekly(20.0, "2026-09-01 10:00:00");
    let executing = seed::insert_bot(&f.c, &f.s, &spec());
    let broken = seed::insert_bot(&f.c, &f.s, &spec());
    let retrying = seed::insert_bot(&f.c, &f.s, &spec());
    let intent = seed::insert_bot(&f.c, &f.s, &spec());
    let aside = seed::insert_bot(&f.c, &f.s, &spec());
    let member = f.member();
    f.c.execute("UPDATE bots SET status = 4 WHERE id = ?1", [executing]).unwrap();
    f.c.execute("UPDATE bots SET status = 5, transient_data = '{not json' WHERE id = ?1", [broken]).unwrap();
    f.c.execute("UPDATE bots SET status = 5, user_id = ?2, label = 'Secret Savings' WHERE id = ?1", params![retrying, member]).unwrap();
    f.c.execute("UPDATE bots SET status = 2, transient_data = json_set(coalesce(transient_data, '{}'), '$.rust_placement', json('{\"x\":1}')) WHERE id = ?1", [intent]).unwrap();
    f.c.execute("UPDATE bots SET status = 2, transient_data = json_set(coalesce(transient_data, '{}'), '$.rust_set_aside', json('{\"reasons\":[\"r\"]}')) WHERE id = ?1", [aside]).unwrap();
    let mut b = f.browser(f.s.user_id);
    let page = f.page(&mut b).await;
    assert_eq!(page.status, 200, "{}", page.body);
    let engine = page.body.split("<h2>Engine</h2>").nth(1).unwrap();
    let section = |title: &str| engine.split(&format!("<h3>{title}")).nth(1).unwrap().split("<h3>").next().unwrap().to_string();
    assert!(section("Executing").contains(&format!("<a href=\"/bots/{executing}\">bot #{executing}</a>")));
    let retrying_rows = section("Retrying");
    assert!(retrying_rows.contains(&format!("<td>bot #{retrying}</td><td>user #{member}</td>")), "{retrying_rows}");
    assert!(retrying_rows.contains(&format!("bot #{broken}")), "a malformed row keeps its status group");
    assert!(section("Order unresolved").contains(&format!("bot #{intent}")) && section("Set aside").contains(&format!("bot #{aside}")));
    assert!(engine.contains("Unreadable: 1."), "{engine}");
    assert!(!page.body.contains("Secret Savings") && !page.body.contains("member@example.com"));
}

#[tokio::test(flavor = "current_thread")]
async fn the_eleventh_run_now_in_a_minute_is_throttled() {
    let mut f = fx();
    f.slots();
    let mut b = f.browser(f.s.user_id);
    for _ in 0..10 { assert_eq!(f.run_now(&mut b, jobs::reference::STOCKS, "", Csrf::Header).await.status, 303); }
    assert_eq!(f.run_now(&mut b, jobs::reference::STOCKS, "", Csrf::Header).await.status, 429);
    let _ = &f.dir;
}
