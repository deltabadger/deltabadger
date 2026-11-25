//! The IBKR connect page on Rust (Ibkr plan I3) where it differs from 2.x by design, beside its page parity
//! (the recorded settings_ibkr_* scenarios): a new connection's keys made inline with ffdhe2048 (D-IBKR-DHGROUP), downloads of
//! their public halves, a key whose material makes no session offering Disconnect only (D-IBKR-RECONNECT), and a
//! disconnect that drops the key's account binding and keeps its ledger rows.
mod common;
#[path = "support/settings_fixture.rs"]
mod fixture;
use common::web::{self, Browser, Csrf};
use deltabadger::venue::signers::ibkr as signer;
use deltabadger::web::{App, Config};

const NOW: &str = "2026-09-10T12:00:30.123456Z";

struct Fx { _dir: tempfile::TempDir, c: rusqlite::Connection, app: App, browser: Browser, user: i64, exchange: i64, seed: common::seed::Seeded }

async fn hosted() -> Fx { install(true).await }

async fn install(hosted: bool) -> Fx { install_with(hosted, None).await }

/// The install, on the data API's container (`hosted`, where IBKR is offered) or not; IBKR's key check sent to `ibkr`.
async fn install_with(hosted: bool, ibkr: Option<String>) -> Fx {
    let (dir, opened, seed) = fixture::install();
    deltabadger::store::test_support::migrate(&opened.primary).unwrap();
    let env = |name: &str| match name { "SECRET_KEY_BASE" => Some(web::SECRET.to_string()), "MARKET_DATA_URL" if hosted => Some("http://data-api.invalid".into()), _ => None };
    let app = App::new(Config::from_env(&env).unwrap(), &env, opened.primary, web::TestClock::at(NOW)).unwrap()
        .with_figure_source(deltabadger::web::figure::loading::Source::Disabled).unwrap();
    let app = match ibkr { Some(url) => app.with_venue_key_url(deltabadger::venue::ibkr::IBKR, &url).unwrap(), None => app };
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    c.execute("UPDATE exchanges SET type = 'Exchanges::Ibkr', name = 'Interactive Brokers' WHERE id = ?1", [seed.exchange_id]).unwrap();
    c.execute("DELETE FROM api_keys", []).unwrap();
    c.execute("UPDATE users SET encrypted_password=?1,confirmed_at='2026-01-01 00:00:00',setup_completed=1 WHERE id=?2",
              (deltabadger::crypto::hash_password("Correct-horse-9").unwrap(), seed.user_id)).unwrap();
    let mut browser = Browser::default();
    browser.send(&app, "GET", "/login", None, Csrf::None, &[]).await;
    let signed = browser.send(&app, "POST", "/login", Some(&[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]), Csrf::Form, &[]).await;
    assert_eq!(signed.status, 303);
    Fx { _dir: dir, c, app, browser, user: seed.user_id, exchange: seed.exchange_id, seed }
}

impl Fx {
    async fn page(&mut self) -> String {
        let answer = self.browser.send(&self.app, "GET", "/settings/ibkr_connect", None, Csrf::None, &[]).await;
        assert_eq!(answer.status, 200);
        answer.body
    }
    async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> u16 {
        self.page().await;
        self.browser.send(&self.app, "POST", path, Some(form), Csrf::Form, &[]).await.status
    }
    /// The key's (status, signing, encryption, dh) as stored, decrypted.
    fn key(&self) -> Option<(i64, String, String, String)> {
        use rusqlite::OptionalExtension;
        let row: Option<(i64, String, String, String)> = self.c.query_row("SELECT status, rsa_signature_key, rsa_encryption_key, dh_param FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 AND key_type = 0",
                                                                         (self.user, self.exchange), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional().unwrap();
        row.map(|(s, a, b, d)| (s, self.app.cipher.decrypt(&a).unwrap(), self.app.cipher.decrypt(&b).unwrap(), self.app.cipher.decrypt(&d).unwrap()))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_connection_makes_its_keys_inline_and_serves_their_public_halves() {
    let mut fx = hosted().await;
    assert!(fx.page().await.contains("Start setup"));
    assert_eq!(fx.post("/settings/ibkr_connect", &[]).await, 302);
    let (status, signing, encryption, dh) = fx.key().expect("the key is made");
    assert_eq!(status, 0);
    assert_eq!(dh, signer::ffdhe2048_pem(), "D-IBKR-DHGROUP");
    assert!(signer::usable(&signing, &encryption, &dh));
    assert_ne!(signing, encryption);
    let page = fx.page().await;
    assert!(page.contains("artifact=signing") && page.contains("api_key[access_token]"), "the downloads, the portal steps and the paste form");
    for (artifact, expected) in [("signing", signer::public_pem(&signing).unwrap()), ("encryption", signer::public_pem(&encryption).unwrap()), ("dhparam", dh.clone())] {
        let answer = fx.browser.send(&fx.app, "GET", &format!("/settings/ibkr_connect/download?artifact={artifact}"), None, Csrf::None, &[]).await;
        assert_eq!((answer.status, answer.body.as_str()), (200, expected.as_str()), "{artifact}");
        assert!(!answer.body.contains("PRIVATE"));
    }
    // Starting again keeps the keys the user may have uploaded already.
    fx.browser.send(&fx.app, "POST", "/settings/ibkr_connect", None, Csrf::Header, &[]).await;
    assert_eq!(fx.key().map(|k| k.1), Some(signing));
}

#[tokio::test(flavor = "current_thread")]
async fn a_key_whose_material_makes_no_session_offers_disconnect_only_and_is_never_completed() {
    let mut fx = hosted().await;
    let now = "2026-09-01 00:00:00";
    // A 2.x key whose job never ran (no RSA keys), then one whose material this build cannot read.
    fx.c.execute("INSERT INTO api_keys(user_id, exchange_id, key_type, status, created_at, updated_at) VALUES(?1, ?2, 0, 3, ?3, ?3)", (fx.user, fx.exchange, now)).unwrap();
    for material in [None, Some("not a key")] {
        if let Some(text) = material {
            let e = fx.app.cipher.encrypt(text);
            fx.c.execute("UPDATE api_keys SET rsa_signature_key = ?1, rsa_encryption_key = ?1, dh_param = ?1", [e]).unwrap();
        }
        let page = fx.page().await;
        assert!(page.contains("be used now") && page.contains(">Disconnect</button>"), "{page}");
        assert!(!page.contains("api_key[access_token]") && !page.contains("Start over") && !page.contains("aria-busy"));
        let row = |c: &rusqlite::Connection| c.query_row("SELECT count(*), max(rsa_signature_key), max(dh_param), max(status) FROM api_keys", [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, i64>(3)?))).unwrap();
        let before = row(&fx.c);
        assert_eq!(fx.post("/settings/ibkr_connect", &[]).await, 302);
        assert_eq!(row(&fx.c), before);
        assert!(fx.page().await.contains("be used now"), "a create converts nothing");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn disconnect_drops_the_key_and_its_binding_and_keeps_its_ledger_rows() {
    let mut fx = hosted().await;
    assert_eq!(fx.post("/settings/ibkr_connect", &[]).await, 302);
    let id: i64 = fx.c.query_row("SELECT id FROM api_keys", [], |r| r.get(0)).unwrap();
    deltabadger::venue::ibkr_account::bind(&fx.c, &fx.app.cipher, id, "U1234567", chrono::Utc::now()).unwrap().unwrap();
    fx.c.execute("INSERT INTO account_transactions(user_id, exchange_id, api_key_id, base_amount, base_currency, entry_type, transacted_at, created_at, updated_at) \
                  VALUES(?1, ?2, ?3, 1, 'AAPL', 0, '2026-09-01 00:00:00', '2026-09-01 00:00:00', '2026-09-01 00:00:00')", (fx.user, fx.exchange, id)).unwrap();
    assert_eq!(fx.post("/settings/ibkr_connect", &[("_method", "delete")]).await, 302);
    assert!(fx.key().is_none());
    assert!(deltabadger::venue::ibkr_account::binding(&fx.c, id).unwrap().is_none());
    assert_eq!(fx.c.query_row("SELECT count(*), max(api_key_id) FROM account_transactions", [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))).unwrap(), (1, None));
    assert!(fx.page().await.contains("Start setup"));
}

/// PLAN-B2 r2.5 (B2-6b): a 2.x key row whose Ibkr::GenerateConnectionKeysJob was still queued gets its material at the
/// takeover, as the job would (D-IBKR-DHGROUP: ffdhe2048, not a generated prime): the wizard shows the downloads, not
/// D-IBKR-RECONNECT's Disconnect. A key that has its material already keeps it byte for byte (the job's own guard).
#[tokio::test(flavor = "current_thread")]
async fn a_queued_key_job_fills_a_blank_key_at_takeover_and_never_rotates_one() {
    for blank in [true, false] {
        let mut fx = hosted().await;
        let before = if blank {
            fx.c.execute("INSERT INTO api_keys(user_id, exchange_id, key_type, status, created_at, updated_at) VALUES(?1, ?2, 0, 0, '2026-09-01 00:00:00', '2026-09-01 00:00:00')", (fx.user, fx.exchange)).unwrap();
            assert!(fx.page().await.contains("be used now"), "2.x's job never ran: D-IBKR-RECONNECT");
            None
        } else {
            assert_eq!(fx.post("/settings/ibkr_connect", &[]).await, 302);
            Some(fx.c.query_row("SELECT rsa_signature_key, rsa_encryption_key, dh_param, updated_at FROM api_keys", [], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))).unwrap())
        };
        let key: i64 = fx.c.query_row("SELECT id FROM api_keys", [], |r| r.get(0)).unwrap();
        let paths = deltabadger::store::Paths::from_env(&|_| None, fx._dir.path());
        let o = deltabadger::store::open(&paths).unwrap();
        o.queue.execute("INSERT INTO solid_queue_jobs (queue_name, class_name, arguments, active_job_id, priority, created_at, updated_at) VALUES ('default', 'Ibkr::GenerateConnectionKeysJob', ?1, 'k', 0, '2026-09-01 00:00:00', '2026-09-01 00:00:00')",
                        [serde_json::json!({ "job_class": "Ibkr::GenerateConnectionKeysJob", "arguments": [key] }).to_string()]).unwrap();
        let lock = deltabadger::lease::lock(&paths, chrono::Utc::now()).unwrap();
        let t = deltabadger::engine::handover::take_over(&lock, &o, &fx.app.cipher, "0.2.0", chrono::Utc::now()).unwrap();
        drop(lock);
        assert_eq!(o.queue.query_row("SELECT count(*) FROM solid_queue_jobs", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "the job leaves the queue");
        assert_eq!(t.dropped_jobs, usize::from(!blank), "a key with its material has nothing left to do");
        let (status, signing, encryption, dh) = fx.key().unwrap();
        assert!(signer::usable(&signing, &encryption, &dh));
        assert_eq!(status, 0);
        if let Some(before) = before {
            let after = fx.c.query_row("SELECT rsa_signature_key, rsa_encryption_key, dh_param, updated_at FROM api_keys", [], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))).unwrap();
            assert_eq!(after, before, "never rotated");
        } else {
            assert_eq!(dh, signer::ffdhe2048_pem(), "D-IBKR-DHGROUP");
            assert_ne!(signing, encryption);
        }
        let page = fx.page().await;
        assert!(page.contains("artifact=signing") && page.contains("api_key[access_token]") && !page.contains("be used now"), "the downloads: {page}");
    }
}

impl Fx {
    /// The tracker's link to IBKR's key (its broken chip, its `+` menu, a sync warning's fix).
    async fn chip(&mut self, key_type: &str) -> (u16, String) {
        let path = format!("/tracker/add_api_key/new?exchange_id={}{key_type}", self.exchange);
        let answer = self.browser.send(&self.app, "GET", &path, None, Csrf::None, &[("Turbo-Frame", "modal")]).await;
        (answer.status, answer.body)
    }
    /// A write posted from the tracker's sheet.
    async fn sheet_post(&mut self, path: &str, form: &[(&str, &str)]) -> common::web::Answer {
        self.chip("").await;
        let mut form = form.to_vec();
        form.push(("sheet", "tracker"));
        self.browser.send(&self.app, "POST", path, Some(&form), Csrf::Form, &[("Turbo-Frame", "modal_content")]).await
    }
    fn sheet_url(&self) -> String { format!("/tracker/add_api_key/new?exchange_id={}", self.exchange) }
}

fn location(answer: &common::web::Answer) -> Option<String> { answer.headers.iter().find(|(k, _)| k == "location").map(|(_, v)| v.clone()) }

/// The sheet closing to the tracker with IBKR's connection card in it, the track's Exchange IBKR.
fn in_sheet(body: &str) -> bool {
    body.contains("class=\"dialog sheet-dialog\"") && body.contains("data-modal--base-close-url-value=\"/tracker\"") && body.contains("sheet__track")
        && body.contains("id=\"ibkr-connect-wizard\"") && body.contains("set-api set-api--connect") && body.contains("Interactive Brokers")
}

/// D-TRACKER-IBKR-SHEET: the tracker's IBKR links open the sheet with the connection in each of its states, whatever key type
/// the link names; the generic key form never reads IBKR's material.
#[tokio::test(flavor = "current_thread")]
async fn the_tracker_ibkr_link_opens_the_sheet_in_every_state() {
    let mut fx = hosted().await;
    let (status, body) = fx.chip("").await;
    assert!(status == 200 && in_sheet(&body) && body.contains("Start setup"), "None: {body}");
    // Started from the sheet: back to the sheet, on the downloads.
    let answer = fx.sheet_post("/settings/ibkr_connect", &[]).await;
    assert_eq!((answer.status, location(&answer)), (302, Some(fx.sheet_url())));
    let (_, body) = fx.chip("").await;
    assert!(in_sheet(&body) && body.contains("artifact=signing") && body.contains("name=\"api_key[access_token]\"") && body.contains("rinput"), "Downloads: {body}");
    let recent = "2026-09-09 00:00:00";
    fx.c.execute("UPDATE api_keys SET status = 3, updated_at = ?1", [recent]).unwrap();
    let (_, body) = fx.chip("&key_type=trading").await;
    assert!(in_sheet(&body) && body.contains("Start over") && body.contains("api_key[access_token]"), "Pending: {body}");
    fx.c.execute("UPDATE api_keys SET status = 3, updated_at = '2026-08-01 00:00:00'", []).unwrap();
    let (_, body) = fx.chip("&key_type=read_only").await;
    assert!(in_sheet(&body) && body.contains("sform__error") && body.contains("Start over"), "Stalled: {body}");
    fx.c.execute("UPDATE api_keys SET status = 1", []).unwrap();
    let (_, body) = fx.chip("").await;
    assert!(in_sheet(&body) && body.contains(">Disconnect</button>") && !body.contains("api_key[access_token]"), "Connected: {body}");
    // Material this build cannot read: Disconnect only, never a decryption failure.
    let unreadable = fx.app.cipher.encrypt("not a key");
    fx.c.execute("UPDATE api_keys SET rsa_signature_key = ?1, rsa_encryption_key = ?1, dh_param = ?1", [&unreadable]).unwrap();
    let (status, body) = fx.chip("&key_type=trading").await;
    assert!(status == 200 && in_sheet(&body) && body.contains("be used now") && body.contains(">Disconnect</button>"), "Unusable: {body}");
}

/// Off the data API's container IBKR is offered only to a user who has its key (as `/settings/ibkr_connect`): with none, the
/// link answers as before.
#[tokio::test(flavor = "current_thread")]
async fn off_the_container_with_no_key_the_ibkr_link_is_not_the_sheet() {
    let mut fx = install(false).await;
    let (_, body) = fx.chip("").await;
    assert!(!body.contains("ibkr-connect-wizard"), "{body}");
}

/// A write from the sheet that does not finish the connection comes back to the sheet with what it earned inside the card
/// (a frame does not show the layout's flash), and it is shown once.
#[tokio::test(flavor = "current_thread")]
async fn an_unfinished_write_comes_back_to_the_sheet_with_its_message_inside() {
    let mut fx = hosted().await;
    fx.sheet_post("/settings/ibkr_connect", &[]).await;
    let answer = fx.sheet_post("/settings/ibkr_connect/activate", &[("api_key[key]", ""), ("api_key[access_token]", "t"), ("api_key[secret]", "s")]).await;
    assert_eq!((answer.status, location(&answer)), (302, Some(fx.sheet_url())));
    let (_, body) = fx.chip("").await;
    let card = &body[body.find("ibkr-connect-wizard").unwrap()..];
    assert!(card.contains("class=\"sform__error\" role=\"alert\""), "{body}");
    let (_, again) = fx.chip("").await;
    assert!(!again.contains("role=\"alert\""), "shown once");
    // The key gone meanwhile (another tab): still back to the sheet, never a page without its frame.
    fx.c.execute("DELETE FROM api_keys", []).unwrap();
    let answer = fx.browser.send(&fx.app, "POST", "/settings/ibkr_connect/activate", Some(&[("api_key[key]", "k"), ("api_key[access_token]", "t"), ("api_key[secret]", "s"), ("sheet", "tracker")]),
                                 Csrf::Header, &[("Turbo-Frame", "modal_content")]).await;
    assert_eq!(answer.status, 302);
    assert!(location(&answer).is_some_and(|l| l.starts_with("/tracker/add_api_key/new")), "{:?}", location(&answer));
}

/// Disconnect from the sheet finishes on the tracker: its warnings rebuilt and the tracker visited. With IBKR history its chip
/// stays, broken, and opens the sheet at the start; with none it is gone. A Disconnect refused (an order settling) keeps the
/// key and says why in the sheet.
#[tokio::test(flavor = "current_thread")]
async fn disconnect_from_the_sheet_finishes_on_the_tracker_unless_refused() {
    for history in [false, true] {
        let mut fx = hosted().await;
        fx.sheet_post("/settings/ibkr_connect", &[]).await;
        fx.c.execute("UPDATE api_keys SET status = 1", []).unwrap();
        let id: i64 = fx.c.query_row("SELECT id FROM api_keys", [], |r| r.get(0)).unwrap();
        if history {
            fx.c.execute("INSERT INTO account_transactions(user_id, exchange_id, api_key_id, base_amount, base_currency, entry_type, transacted_at, created_at, updated_at) \
                          VALUES(?1, ?2, ?3, 1, 'AAPL', 0, '2026-09-01 00:00:00', '2026-09-01 00:00:00', '2026-09-01 00:00:00')", (fx.user, fx.exchange, id)).unwrap();
        }
        let answer = fx.sheet_post("/settings/ibkr_connect", &[("_method", "delete")]).await;
        assert_eq!(answer.status, 200);
        assert!(answer.body.contains("target=\"sync-warnings\"") && answer.body.contains("action=\"redirect\"") && answer.body.contains("/tracker"), "{}", answer.body);
        assert!(fx.key().is_none());
        let tracker = fx.browser.send(&fx.app, "GET", "/tracker", None, Csrf::None, &[]).await;
        assert_eq!(tracker.status, 200);
        let link = format!("/tracker/add_api_key/new?exchange_id={}", fx.exchange);
        if history {
            assert_eq!(fx.c.query_row("SELECT count(*) FROM account_transactions", [], |r| r.get::<_, i64>(0)).unwrap(), 1, "the ledger kept");
            assert!(tracker.body.contains("data-broken=\"true\"") && tracker.body.contains(&link), "{}", tracker.body);
            let (_, body) = fx.chip("").await;
            assert!(in_sheet(&body) && body.contains("Start setup"), "{body}");
        } else {
            assert!(!tracker.body.contains("data-broken=\"true\""), "{}", tracker.body);
        }
    }
    let mut fx = hosted().await;
    fx.sheet_post("/settings/ibkr_connect", &[]).await;
    fx.c.execute("UPDATE api_keys SET status = 1", []).unwrap();
    let bot = common::seed::insert_bot(&fx.c, &fx.seed, &common::seed::BotSpec::weekly(60.0, "2026-09-01 00:00:00"));
    fx.c.execute("INSERT INTO transactions(bot_id,exchange_id,external_id,status,external_status,created_at,updated_at)VALUES(?1,?2,'order-a',0,0,'2026-09-10 00:00:00','2026-09-10 00:00:00')", (bot, fx.exchange)).unwrap();
    let answer = fx.sheet_post("/settings/ibkr_connect", &[("_method", "delete")]).await;
    assert_eq!((answer.status, location(&answer)), (302, Some(fx.sheet_url())));
    assert!(fx.key().is_some(), "the key kept");
    let (_, body) = fx.chip("").await;
    assert!(in_sheet(&body) && body.contains("An order is still being settled."), "{body}");
}

fn fixture(name: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()).unwrap()
}

/// An activation from the sheet that ends connected finishes on the tracker (warnings rebuilt, tracker visited), and the
/// tracker's IBKR chip is healthy then. IBKR's answers are 2.x's recorded session (ibkr_key_verdicts.json, its DH random
/// frozen) for one account.
#[tokio::test(flavor = "current_thread")]
async fn an_activation_from_the_sheet_that_connects_finishes_on_the_tracker() {
    use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};
    let verdicts = fixture("ibkr_key_verdicts.json");
    deltabadger::venue::ibkr::fix_dh_random(Some(verdicts["ibkr"]["dh_random"].as_str().unwrap().into()));
    let one = verdicts["ibkr"]["cases"].as_array().unwrap().iter().find(|c| c["case"] == "one_account").unwrap().clone();
    let server = MockServer::start().await;
    let answer = |a: &serde_json::Value| ResponseTemplate::new(a["status"].as_u64().unwrap() as u16).set_body_raw(a["body"].as_str().unwrap().as_bytes().to_vec(), "application/json");
    Mock::given(method("POST")).and(path("/v1/api/oauth/live_session_token")).respond_with(answer(&one["responses"][0])).mount(&server).await;
    Mock::given(method("POST")).and(path("/v1/api/iserver/auth/ssodh/init")).respond_with(answer(&one["responses"][1])).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1/api/iserver/accounts")).respond_with(answer(&one["responses"][2])).mount(&server).await;
    let mut fx = install_with(true, Some(server.uri())).await;
    fx.app.attach_jobs(Default::default()).unwrap();
    let e = |v: &str| fx.app.cipher.encrypt(v);
    // The connection's own keys, made earlier (the downloads step), as the recorded session signed with them.
    let k = fixture("ibkr_test_keys.json");
    let t = |n: &str| k[n].as_str().unwrap().to_string();
    fx.c.execute("INSERT INTO api_keys(user_id, exchange_id, key_type, status, rsa_signature_key, rsa_encryption_key, dh_param, ibkr_realm, created_at, updated_at) VALUES(?1, ?2, 0, 0, ?3, ?4, ?5, ?6, '2026-09-01 00:00:00', '2026-09-01 00:00:00')",
                 rusqlite::params![fx.user, fx.exchange, e(&t("signature_key")), e(&t("encryption_key")), e(verdicts["ibkr"]["dh_param"].as_str().unwrap()), t("realm")]).unwrap();
    let answer = fx.sheet_post("/settings/ibkr_connect/activate", &[("api_key[key]", &t("consumer_key")), ("api_key[access_token]", &t("access_token")), ("api_key[secret]", &t("access_token_secret"))]).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(answer.body.contains("target=\"sync-warnings\"") && answer.body.contains("action=\"redirect\""), "{}", answer.body);
    assert_eq!(fx.key().map(|k| k.0), Some(1), "connected");
    // IBKR the one venue, healthy: no broken chip asks for its key.
    let tracker = fx.browser.send(&fx.app, "GET", "/tracker", None, Csrf::None, &[]).await;
    assert_eq!(tracker.status, 200);
    assert!(!tracker.body.contains("data-broken=\"true\""), "{}", tracker.body);
    let (_, body) = fx.chip("").await;
    assert!(in_sheet(&body) && body.contains(">Disconnect</button>"), "the sheet shows it connected: {body}");
}

/// #710 slice 2: the connect page activates its exchange's account's key (number 1, inserted directly beside a number 2 whose
/// key is older), and no other account's key takes its credentials.
#[tokio::test(flavor = "current_thread")]
async fn an_activation_targets_the_exchanges_account_and_leaves_another_accounts_key() {
    use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};
    let verdicts = fixture("ibkr_key_verdicts.json");
    deltabadger::venue::ibkr::fix_dh_random(Some(verdicts["ibkr"]["dh_random"].as_str().unwrap().into()));
    let one = verdicts["ibkr"]["cases"].as_array().unwrap().iter().find(|c| c["case"] == "one_account").unwrap().clone();
    let server = MockServer::start().await;
    let answer = |a: &serde_json::Value| ResponseTemplate::new(a["status"].as_u64().unwrap() as u16).set_body_raw(a["body"].as_str().unwrap().as_bytes().to_vec(), "application/json");
    Mock::given(method("POST")).and(path("/v1/api/oauth/live_session_token")).respond_with(answer(&one["responses"][0])).mount(&server).await;
    Mock::given(method("POST")).and(path("/v1/api/iserver/auth/ssodh/init")).respond_with(answer(&one["responses"][1])).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1/api/iserver/accounts")).respond_with(answer(&one["responses"][2])).mount(&server).await;
    let mut fx = install_with(true, Some(server.uri())).await;
    fx.app.attach_jobs(Default::default()).unwrap();
    let e = |v: &str| fx.app.cipher.encrypt(v);
    let k = fixture("ibkr_test_keys.json");
    let t = |n: &str| k[n].as_str().unwrap().to_string();
    let account = |number: i64| {
        fx.c.execute("INSERT INTO exchange_accounts(user_id, exchange_id, number, created_at, updated_at) VALUES(?1, ?2, ?3, '2026-01-01', '2026-01-01')", (fx.user, fx.exchange, number)).unwrap();
        fx.c.last_insert_rowid()
    };
    let (_, second) = (account(1), account(2));
    let insert = |token: Option<String>, status: i64| {
        fx.c.execute("INSERT INTO api_keys(user_id, exchange_id, key_type, status, access_token, rsa_signature_key, rsa_encryption_key, dh_param, ibkr_realm, created_at, updated_at) VALUES(?1, ?2, 0, ?3, ?4, ?5, ?6, ?7, ?8, '2026-09-01 00:00:00', '2026-09-01 00:00:00')",
                     rusqlite::params![fx.user, fx.exchange, status, token, e(&t("signature_key")), e(&t("encryption_key")), e(verdicts["ibkr"]["dh_param"].as_str().unwrap()), t("realm")]).unwrap();
        fx.c.last_insert_rowid()
    };
    let other = insert(Some(e("account-2-token")), 2);
    fx.c.execute("INSERT INTO exchange_account_keys(api_key_id, account_id) VALUES(?1, ?2)", [other, second]).unwrap();
    let own = insert(None, 0);
    let row = |c: &rusqlite::Connection, id: i64| -> (i64, Option<String>) { c.query_row("SELECT status, access_token FROM api_keys WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() };
    let before = row(&fx.c, other);
    let answer = fx.sheet_post("/settings/ibkr_connect/activate", &[("api_key[key]", &t("consumer_key")), ("api_key[access_token]", &t("access_token")), ("api_key[secret]", &t("access_token_secret"))]).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let (status, token) = row(&fx.c, own);
    assert_eq!((status, fx.app.cipher.decrypt(&token.unwrap()).unwrap()), (1, t("access_token")), "account 1's key connected");
    assert_eq!(row(&fx.c, other), before, "account 2's key untouched");
}

/// Rule S4 (Codex branch r4, ibkr.rs:245/250), a single-account install: Disconnect removes only the key it read. An
/// activation that commits new credentials into the row between Disconnect's read and its removal keeps the activated key.
#[tokio::test(flavor = "current_thread")]
async fn s7a_s4_a_disconnect_never_removes_a_key_activated_after_it_read_it() {
    let mut fx = hosted().await;
    fx.sheet_post("/settings/ibkr_connect", &[]).await;
    fx.c.execute("UPDATE api_keys SET status = 3", []).unwrap();
    fx.page().await;
    let (path, cipher) = (fx._dir.path().join("production.sqlite3"), fx.app.cipher.clone());
    let (ready, go) = std::sync::mpsc::channel();
    let activation = std::thread::spawn(move || {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("BEGIN IMMEDIATE").unwrap();
        c.execute("UPDATE api_keys SET status = 1, access_token = ?1, updated_at = 'activated'", [cipher.encrypt("activated-token")]).unwrap();
        ready.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        c.execute_batch("COMMIT").unwrap();
    });
    go.recv().unwrap();
    fx.browser.send(&fx.app, "POST", "/settings/ibkr_connect", Some(&[("_method", "delete")]), Csrf::Form, &[]).await;
    activation.join().unwrap();
    let status: Option<i64> = { use rusqlite::OptionalExtension; fx.c.query_row("SELECT status FROM api_keys", [], |r| r.get(0)).optional().unwrap() };
    assert_eq!(status, Some(1), "the activated key is kept");
}

// ---- #710 slice 7b: a second IBKR account ----------------------------------------------------------------------------

/// IBKR at a local server for the 7b cases: 2.x's recorded mint and brokerage session, then a login that opens `account`
/// (the recorded `one_account` session with only its unsigned `/iserver/accounts` answer varied, PLAN-710-7b §5.0).
async fn s7b_server(server: &wiremock::MockServer, account: &str) {
    use wiremock::{matchers::{method, path}, Mock, ResponseTemplate};
    let verdicts = fixture("ibkr_key_verdicts.json");
    deltabadger::venue::ibkr::fix_dh_random(Some(verdicts["ibkr"]["dh_random"].as_str().unwrap().into()));
    let one = verdicts["ibkr"]["cases"].as_array().unwrap().iter().find(|c| c["case"] == "one_account").unwrap().clone();
    let answer = |a: &serde_json::Value| ResponseTemplate::new(a["status"].as_u64().unwrap() as u16).set_body_raw(a["body"].as_str().unwrap().as_bytes().to_vec(), "application/json");
    server.reset().await;
    Mock::given(method("POST")).and(path("/v1/api/oauth/live_session_token")).respond_with(answer(&one["responses"][0])).mount(server).await;
    Mock::given(method("POST")).and(path("/v1/api/iserver/auth/ssodh/init")).respond_with(answer(&one["responses"][1])).mount(server).await;
    Mock::given(method("GET")).and(path("/v1/api/iserver/accounts")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "accounts": [account], "selectedAccount": account }))).mount(server).await;
}
fn test_keys() -> impl Fn(&str) -> String { let k = fixture("ibkr_test_keys.json"); move |n: &str| k[n].as_str().unwrap().to_string() }
/// A key on account `account` (None: number 1 by its absent membership) with the recorded session's whole material.
fn s7b_key(fx: &Fx, account: Option<i64>, status: i64) -> i64 {
    let (e, t) = (|v: &str| fx.app.cipher.encrypt(v), test_keys());
    let verdicts = fixture("ibkr_key_verdicts.json");
    fx.c.execute("INSERT INTO api_keys(user_id, exchange_id, key_type, status, key, secret, access_token, rsa_signature_key, rsa_encryption_key, dh_param, ibkr_realm, created_at, updated_at)
                  VALUES(?1, ?2, 0, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, '2026-09-01 00:00:00', '2026-09-01 00:00:00')",
                 rusqlite::params![fx.user, fx.exchange, status, e(&t("consumer_key")), e(&t("access_token_secret")), e(&t("access_token")), e(&t("signature_key")), e(&t("encryption_key")),
                                   e(verdicts["ibkr"]["dh_param"].as_str().unwrap()), t("realm")]).unwrap();
    let id = fx.c.last_insert_rowid();
    if let Some(a) = account { fx.c.execute("INSERT INTO exchange_account_keys(api_key_id, account_id) VALUES(?1, ?2)", [id, a]).unwrap(); }
    id
}
fn s7b_fenced(fx: &Fx, key: i64) -> bool {
    fx.c.query_row(&format!("SELECT {} FROM api_keys k WHERE k.id = ?1", deltabadger::identity::fenced_sql(&fx.c, "k").unwrap()), [key], |r| r.get(0)).unwrap()
}
fn s7b_bound(fx: &Fx, key: i64) -> Option<String> { deltabadger::venue::ibkr_account::binding(&fx.c, key).unwrap().and_then(|b| b.account_id(&fx.app.cipher)) }
fn s7b_keys(fx: &Fx) -> Vec<(i64, Option<i64>)> {
    fx.c.prepare("SELECT k.id, m.account_id FROM api_keys k LEFT JOIN exchange_account_keys m ON m.api_key_id = k.id ORDER BY k.id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
#[derive(Clone)]
struct S7bVenues(String);
impl deltabadger::sync::jobs::Connect for S7bVenues {
    type T = deltabadger::venue::http::ReqwestTransport;
    fn connect(&self, c: &deltabadger::crypto::Credentials) -> deltabadger::venue::alpaca::AlpacaVenue<Self::T> {
        deltabadger::venue::alpaca::AlpacaVenue::new(deltabadger::venue::http::ReqwestTransport::new(deltabadger::venue::http::client(), c.key.clone(), c.secret.clone()),
                                                     deltabadger::venue::alpaca::Urls { trading: self.0.clone(), data: self.0.clone() })
    }
    fn key_check_egress(&self, _: &str, _: Option<String>) -> Result<(String, reqwest::Client), String> { Ok((self.0.clone(), reqwest::Client::builder().no_proxy().build().unwrap())) }
}
async fn s7b_activation_job(fx: &Fx, uri: &str) -> deltabadger::jobs::Outcome {
    use deltabadger::jobs::Job;
    let clock = deltabadger::engine::FixedClock(NOW.parse().unwrap());
    let cx = deltabadger::jobs::Cx { db: deltabadger::jobs::Db::new(rusqlite::Connection::open(fx._dir.path().join("production.sqlite3")).unwrap(), fx.app.cipher.clone()), clock: &clock, wakers: Default::default() };
    deltabadger::sync::jobs::IbkrActivation { venues: S7bVenues(uri.into()) }.run(cx, vec![]).await
}

/// #710 7b §6 acceptance, at the web and job level (IBKR bots stay off: G3): account 1's login, unchanged, starts opening
/// U7654321. Its key is fenced at the first read that sees it (the resubmit's), never refused; the resubmit asks
/// `key_other`; "Add anyway" makes account 2 bound to U7654321 (key_elsewhere). When the login opens U1234567 again, the
/// activation job lifts account 1's fence and fences account 2's copy, with no user action.
#[tokio::test(flavor = "current_thread")]
async fn s7b_ibkr_unchanged_credentials_another_account_reported() {
    let server = wiremock::MockServer::start().await;
    s7b_server(&server, "U1234567").await;
    let mut fx = install_with(true, Some(server.uri())).await;
    fx.app.attach_jobs(Default::default()).unwrap();
    let t = test_keys();
    let one = s7b_key(&fx, None, 0);
    // The first connection activates and binds U1234567 (account 1's proven identity).
    let posted = [("api_key[key]", t("consumer_key")), ("api_key[access_token]", t("access_token")), ("api_key[secret]", t("access_token_secret"))];
    let form: Vec<(&str, &str)> = posted.iter().map(|(k, v)| (*k, v.as_str())).collect();
    assert_eq!(fx.post("/settings/ibkr_connect/activate", &form).await, 302);
    assert_eq!(s7b_bound(&fx, one).as_deref(), Some("U1234567"));
    // The login opens another account now. The scheduled check is the first to see it: account 1's key is fenced (never
    // condemned), and its page offers the resubmit on its own keys.
    s7b_server(&server, "U7654321").await;
    {
        use deltabadger::jobs::Job;
        let clock = deltabadger::engine::FixedClock(NOW.parse().unwrap());
        let cx = deltabadger::jobs::Cx { db: deltabadger::jobs::Db::new(rusqlite::Connection::open(fx._dir.path().join("production.sqlite3")).unwrap(), fx.app.cipher.clone()), clock: &clock, wakers: Default::default() };
        deltabadger::sync::jobs::Validator { venues: S7bVenues(server.uri()), key_id: one }.run(cx, vec![]).await;
    }
    assert!(s7b_fenced(&fx, one), "fenced by the check's read");
    let page = fx.page().await;
    assert!(page.contains("opens a different account than this one") && page.contains("api_key_question"), "the broken line and the resubmit: {page}");
    // The resubmit asks first, writing nothing.
    let asked = fx.browser.send(&fx.app, "POST", "/settings/ibkr_connect/activate", Some(&form), Csrf::Form, &[]).await;
    assert_eq!(asked.status, 422, "{}", asked.body);
    assert!(asked.body.contains("opens another Interactive Brokers account than") && asked.body.contains("Add anyway"), "key_other: {}", asked.body);
    assert_eq!(s7b_keys(&fx).len(), 1, "nothing saved");
    assert!(s7b_fenced(&fx, one), "account 1's key stays fenced");
    let status: i64 = fx.c.query_row("SELECT status FROM api_keys WHERE id = ?1", [one], |r| r.get(0)).unwrap();
    assert_eq!(status, 1, "never condemned: fenced");
    // Add anyway: account 2, bound to U7654321.
    let value = asked.body.split("value=\"").find_map(|rest| rest.split('"').next().filter(|v| v.starts_with("n!w"))).expect("the Add anyway answer").to_string();
    let mut answered = form.clone();
    answered.push(("key_answer", &value));
    assert!(fx.page().await.contains("api_key_question"), "the fenced page asks in its slot");
    let added = fx.browser.send(&fx.app, "POST", "/settings/ibkr_connect/activate", Some(&answered), Csrf::Form, &[]).await;
    assert_eq!(added.status, 302, "{}", added.body);
    let keys = s7b_keys(&fx);
    assert_eq!(keys.len(), 2, "{keys:?}");
    let (two, second) = (keys[1].0, keys[1].1.expect("on its own account"));
    assert_eq!(fx.c.query_row("SELECT number FROM exchange_accounts WHERE id = ?1", [second], |r| r.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(s7b_bound(&fx, two).as_deref(), Some("U7654321"));
    assert_eq!(s7b_bound(&fx, one).as_deref(), Some("U1234567"), "account 1's binding never moves");
    assert!(s7b_fenced(&fx, one) && !s7b_fenced(&fx, two));
    let page = fx.browser.send(&fx.app, "GET", &format!("/settings/ibkr_connect?account_id={second}"), None, Csrf::None, &[]).await;
    assert!(page.body.contains("Interactive Brokers²") || page.body.contains("belongs to"), "{}", page.body);
    // The login opens U1234567 again: the scheduled check lifts account 1's fence and fences account 2's copy.
    s7b_server(&server, "U1234567").await;
    assert_eq!(s7b_activation_job(&fx, &server.uri()).await, deltabadger::jobs::Outcome::Done);
    assert!(!s7b_fenced(&fx, one), "account 1 is back, no restart, no user action");
    assert!(s7b_fenced(&fx, two), "account 2's copy now reads another account");
}

/// #710 7b r6: with two live accounts an unqualified Disconnect (2.x's form) writes nothing; the account's page carries it.
#[tokio::test(flavor = "current_thread")]
async fn s7b_ibkr_unqualified_disconnect_with_two_accounts_writes_nothing() {
    let mut fx = hosted().await;
    let account = |number: i64| {
        fx.c.execute("INSERT INTO exchange_accounts(user_id, exchange_id, number, created_at, updated_at) VALUES(?1, ?2, ?3, '2026-01-01', '2026-01-01')", (fx.user, fx.exchange, number)).unwrap();
        fx.c.last_insert_rowid()
    };
    let (_, second) = (account(1), account(2));
    let (one, two) = (s7b_key(&fx, None, 1), s7b_key(&fx, Some(second), 1));
    assert_eq!(fx.post("/settings/ibkr_connect", &[("_method", "delete")]).await, 302);
    assert_eq!(s7b_keys(&fx).iter().map(|k| k.0).collect::<Vec<_>>(), [one, two], "nothing removed");
    let page = fx.browser.send(&fx.app, "GET", &format!("/settings/ibkr_connect?account_id={second}"), None, Csrf::None, &[]).await;
    assert!(page.body.contains(&format!("name=\"account_id\" value=\"{second}\"")), "the account's page carries it: {}", page.body);
    let deleted = fx.browser.send(&fx.app, "POST", "/settings/ibkr_connect", Some(&[("_method", "delete"), ("account_id", &second.to_string())]), Csrf::Form, &[]).await;
    assert_eq!(deleted.status, 302, "{}", deleted.body);
    assert_eq!(s7b_keys(&fx).iter().map(|k| k.0).collect::<Vec<_>>(), [one], "the named account's key only");
    // A forged account is a 404.
    let forged = fx.browser.send(&fx.app, "GET", "/settings/ibkr_connect?account_id=999999", None, Csrf::None, &[]).await;
    assert_eq!(forged.status, 404);
}

/// #710 7b rules 3 and 11 (D-IBKR-REGISTRATION): the tracker's IBKR tile beside a connected IBKR account opens a fresh
/// connection: a registration, no account and no `api_keys` row, until IBKR names its account; then it is the key of a new
/// account 2 (the login opens U7654321), and the registration is gone. Account 1 is untouched.
#[tokio::test(flavor = "current_thread")]
async fn s7b_ibkr_registration_activates_as_new_account() {
    let server = wiremock::MockServer::start().await;
    s7b_server(&server, "U1234567").await;
    let mut fx = install_with(true, Some(server.uri())).await;
    fx.app.attach_jobs(Default::default()).unwrap();
    let one = s7b_key(&fx, None, 1);
    let first = deltabadger::identity::ensure_account(&fx.c, fx.user, fx.exchange, chrono::Utc::now()).unwrap();
    deltabadger::venue::ibkr_account::bind(&fx.c, &fx.app.cipher, one, "U1234567", chrono::Utc::now()).unwrap().unwrap();
    let accounts = |c: &rusqlite::Connection| c.query_row("SELECT count(*) FROM exchange_accounts", [], |r| r.get::<_, i64>(0)).unwrap();
    let before = accounts(&fx.c);
    // Start a fresh connection (a tile's `registration=new`).
    fx.page().await;
    let started = fx.browser.send(&fx.app, "POST", "/settings/ibkr_connect", Some(&[("registration", "new")]), Csrf::Header, &[]).await;
    assert_eq!(started.status, 302);
    let (reg, revision): (i64, i64) = fx.c.query_row("SELECT id, revision FROM ibkr_registrations", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((s7b_keys(&fx).len(), accounts(&fx.c)), (1, before), "no key row and no account yet");
    let page = fx.browser.send(&fx.app, "GET", &format!("/settings/ibkr_connect?registration={reg}"), None, Csrf::None, &[]).await;
    assert!(page.body.contains(&format!("artifact=signing&amp;registration={reg}")) && page.body.contains("name=\"revision\""), "{}", page.body);
    // The registration's own keys signed the recorded session: put the recorded ones in their place.
    let (e, t) = (|v: &str| fx.app.cipher.encrypt(v), test_keys());
    fx.c.execute("UPDATE ibkr_registrations SET rsa_signature_key = ?1, rsa_encryption_key = ?2, dh_param = ?3, ibkr_realm = ?4",
                 rusqlite::params![e(&t("signature_key")), e(&t("encryption_key")), e(fixture("ibkr_key_verdicts.json")["ibkr"]["dh_param"].as_str().unwrap()), t("realm")]).unwrap();
    // A login of another IBKR account (other credentials, same recorded session): it becomes account 2.
    s7b_server(&server, "U7654321").await;
    let (reg_s, rev_s) = (reg.to_string(), revision.to_string());
    let form = [("api_key[key]", t("consumer_key")), ("api_key[access_token]", t("access_token")), ("api_key[secret]", t("access_token_secret"))];
    let mut posted: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
    posted.extend([("registration", reg_s.as_str()), ("revision", rev_s.as_str())]);
    let registration_page = format!("/settings/ibkr_connect?registration={reg}");
    fx.browser.send(&fx.app, "GET", &registration_page, None, Csrf::None, &[]).await;
    let activated = fx.browser.send(&fx.app, "POST", "/settings/ibkr_connect/activate", Some(&posted), Csrf::Form, &[]).await;
    assert!(matches!(activated.status, 302 | 422), "{}", activated.body);
    if activated.status == 422 {
        // The login's material is account 1's: the question first (its copy is fenced), then Add anyway.
        let value = activated.body.split("value=\"").find_map(|rest| rest.split('"').next().filter(|v| v.starts_with("n!w"))).expect("Add anyway").to_string();
        posted.push(("key_answer", &value));
        fx.browser.send(&fx.app, "GET", &registration_page, None, Csrf::None, &[]).await;
        let added = fx.browser.send(&fx.app, "POST", "/settings/ibkr_connect/activate", Some(&posted), Csrf::Form, &[]).await;
        assert_eq!(added.status, 302, "{}", added.body);
    }
    let keys = s7b_keys(&fx);
    assert_eq!(keys.len(), 2, "{keys:?}");
    let second = keys[1].1.expect("its own account");
    assert_ne!(second, first);
    assert_eq!(s7b_bound(&fx, keys[1].0).as_deref(), Some("U7654321"));
    assert_eq!(fx.c.query_row("SELECT count(*) FROM ibkr_registrations", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "the registration became the key");
    assert_eq!(s7b_bound(&fx, one).as_deref(), Some("U1234567"), "account 1 untouched");
}
