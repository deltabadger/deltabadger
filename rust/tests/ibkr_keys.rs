//! IBKR keys on Rust (Ibkr plan I2): the key check with IBKR's activation wait, replayed against Rails' recorded verdicts
//! (ibkr_key_verdicts.json); the immutable key binding and bot ownership (D-IBKR-ACCOUNT); the activation job; the
//! pending-activation guards; and the balance sync through `ibkr_conids`.
//!
//! The scripted IBKR answers beyond Rails' recording (ledger and position pages, account lists) are provisional: no IBKR
//! session has been recorded yet (gate G1), so they are the shapes Rails' parser expects, tagged `source: "provisional"`.
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::crypto::Cipher;
use deltabadger::engine::model::credential_version_by_id;
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::FixedClock;
use deltabadger::jobs::{Cx, Db, Job, Outcome};
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::ibkr::{self, Error};
use deltabadger::venue::ibkr_account;
use deltabadger::venue::signers::ibkr::Material;
use deltabadger::web::settings::key_check::{self, IbkrVerdict};
use rusqlite::Connection;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACCOUNTS: &str = "/v1/api/iserver/accounts";
const LST: &str = "/v1/api/oauth/live_session_token";
const INIT: &str = "/v1/api/iserver/auth/ssodh/init";

fn fixture(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()).unwrap()
}

/// Rails' recorded verdicts, their pins checked.
fn verdicts() -> Value {
    use sha2::{Digest, Sha256};
    let recorded = fixture("ibkr_key_verdicts.json");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in recorded["sources"].as_object().unwrap() {
        assert_eq!(&json!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap()))), hash, "{path}: re-record ibkr_key_verdicts.json (script/rust/repin)");
    }
    recorded
}

fn material() -> Material {
    let k = fixture("ibkr_test_keys.json");
    let s = |n: &str| k[n].as_str().unwrap().to_string();
    Material { consumer_key: s("consumer_key"), access_token: s("access_token"), access_token_secret: s("access_token_secret"), signature_key: s("signature_key"),
               encryption_key: s("encryption_key"), dh_param: verdicts()["ibkr"]["dh_param"].as_str().unwrap().into(), realm: s("realm") }
}

/// The DH random Rails' recording froze: the recorded LST answers validate under it. Every Session in this binary mints with it.
fn freeze_dh() { ibkr::fix_dh_random(Some(verdicts()["ibkr"]["dh_random"].as_str().unwrap().into())); }

/// One recorded case's answer as ScriptedTransport replays it.
fn reply(a: &Value) -> Value {
    if a["timeout"] == true { return json!({ "network": "post_send", "message": "Net::ReadTimeout" }) }
    if a["refused"] == true { return json!({ "network": "pre_send", "message": "Connection refused" }) }
    json!({ "status": a["status"], "body": a["body"], "content_type": a["content_type"] })
}

/// The recorded mint and brokerage-session answers (a healthy case's first two).
fn mint_and_init() -> (Value, Value) {
    let v = verdicts();
    let one = v["ibkr"]["cases"].as_array().unwrap().iter().find(|c| c["case"] == "one_account").unwrap().clone();
    (one["responses"][0].clone(), one["responses"][1].clone())
}

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const NOW: &str = "2026-10-24T12:00:00Z";

/// A login under a credential version of its own (a one-row api_keys table).
fn login() -> std::sync::Arc<ibkr::Login> {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE api_keys(id INTEGER PRIMARY KEY,key TEXT,secret TEXT,passphrase TEXT,access_token TEXT,rsa_signature_key TEXT,rsa_encryption_key TEXT,dh_param TEXT);
                     INSERT INTO api_keys(id,key) VALUES(1,'replay');").unwrap();
    std::sync::Arc::new(ibkr::Login::new(material(), credential_version_by_id(&c, 1).unwrap().unwrap()).unwrap())
}

#[tokio::test(flavor = "current_thread")]
async fn ibkr_key_verdicts_replay_every_recorded_case() {
    freeze_dh();
    let recorded = verdicts();
    let mut replayed = 0;
    for case in recorded["ibkr"]["cases"].as_array().unwrap() {
        let name = case["case"].as_str().unwrap();
        let verdict = &case["verdict"];
        let theirs = match (verdict["kind"].as_str().unwrap(), &verdict["data"]) {
            ("success", Value::Bool(true)) => "correct",
            ("success", d) if d == "pending_activation" => "pending_activation",
            ("failure", _) => "failure",
            other => panic!("{name}: Rails answered {other:?}"),
        };
        if name == "lock_busy" {
            // D-IBKR-LOCK-TIMEOUT: Rails reads IbkrLock::Timeout (no status) as pending activation; here it is no verdict.
            assert_eq!(theirs, "pending_activation", "Rails no longer reads a busy login as pending activation: drop the divergence");
            assert!(matches!(key_check::ibkr_verdict(&Err(Error::LockTimeout)), IbkrVerdict::Failure(_)));
            replayed += 1;
            continue;
        }
        let mut script = serde_json::Map::new();
        for (r, a) in case["requests"].as_array().unwrap().iter().zip(case["responses"].as_array().unwrap()) {
            script.entry(format!("{} {}", r["method"].as_str().unwrap(), r["path"].as_str().unwrap())).or_insert_with(|| json!([])).as_array_mut().unwrap().push(reply(a));
        }
        let t = ScriptedTransport::from_script(&Value::Object(script));
        let clock = FixedClock(at(NOW));
        let ours = key_check::ibkr_check(&ibkr::Session::new(&t, login(), &clock)).await;
        // D-IBKR-ACCOUNT: several accounts are refused; an account that is no U…/DU…/DF… id (a pseudo account, a text
        // answer Rails reads as one account) is no verdict.
        let expected = match name {
            "two_accounts" => IbkrVerdict::Incorrect(ibkr_account::MULTI_ACCOUNT),
            "pseudo_account" | "plain_text_200" => { assert_eq!(theirs, "correct", "{name}"); IbkrVerdict::Failure("IBKR named no account Deltabadger can read".into()) }
            _ => match theirs {
                "correct" => IbkrVerdict::Account(if name == "paper_account" { "DU1234567" } else { "U1234567" }.into()),
                "pending_activation" => IbkrVerdict::PendingActivation,
                _ => { assert!(matches!(ours, IbkrVerdict::Failure(_)), "{name}: {ours:?}, Rails a failure"); ours.clone() }
            },
        };
        assert_eq!(ours, expected, "{name}");
        let sent: Vec<String> = t.requests().iter().map(|r| format!("{} {}", r.method, r.path)).collect();
        let rails: Vec<String> = case["requests"].as_array().unwrap().iter().map(|r| format!("{} {}", r["method"].as_str().unwrap(), r["path"].as_str().unwrap())).collect();
        assert_eq!(sent, rails, "{name}: the request sequence");
        if let Some(init) = t.requests().iter().find(|r| r.path == INIT) { assert_eq!(init.body, Some(json!({ "publish": true, "compete": true }))); }
        replayed += 1;
    }
    assert_eq!(replayed, 24, "a recorded IBKR case was added or dropped");
}

/// An installed instance whose seeded venue is IBKR and whose seeded key is an IBKR key of `status`.
struct Fx { _dir: tempfile::TempDir, c: Connection, cipher: Cipher, s: seed::Seeded, path: std::path::PathBuf }
impl Fx {
    fn new(status: i64) -> Self {
        let (dir, o, s) = common::install();
        let cipher = seed::cipher();
        let c = o.primary;
        c.execute("UPDATE exchanges SET type = 'Exchanges::Ibkr', name = 'Interactive Brokers' WHERE id = ?1", [s.exchange_id]).unwrap();
        let fx = Self { path: dir.path().join("production.sqlite3"), _dir: dir, c, cipher, s };
        fx.set_key(fx.s.api_key_id, status);
        fx
    }
    fn set_key(&self, id: i64, status: i64) { self.set_key_with(&self.cipher, id, status) }
    fn set_key_with(&self, cipher: &Cipher, id: i64, status: i64) {
        let m = material();
        let e = |v: &str| cipher.encrypt(v);
        self.c.execute("UPDATE api_keys SET key=?1, secret=?2, access_token=?3, rsa_signature_key=?4, rsa_encryption_key=?5, dh_param=?6, ibkr_realm=?7, status=?8, updated_at='2026-10-01 00:00:00' WHERE id=?9",
                       rusqlite::params![e(&m.consumer_key), e(&m.access_token_secret), e(&m.access_token), e(&m.signature_key), e(&m.encryption_key), e(&m.dh_param), m.realm, status, id]).unwrap();
    }
    fn cx<'a>(&self, clock: &'a FixedClock) -> Cx<'a> { Cx { db: Db::new(Connection::open(&self.path).unwrap(), self.cipher.clone()), clock, wakers: Default::default() } }
    fn key(&self) -> (i64, String) { self.c.query_row("SELECT status, updated_at FROM api_keys WHERE id = ?1", [self.s.api_key_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() }
    fn bindings(&self) -> Vec<(i64, String, String)> {
        let mut s = self.c.prepare("SELECT api_key_id, account_ref, account_id_enc FROM ibkr_bindings ORDER BY api_key_id").unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    }
    fn bound_account(&self, key: i64) -> Option<String> { ibkr_account::binding(&self.c, key).unwrap().and_then(|b| b.account_id(&self.cipher)) }
}

/// IBKR at a local server: the recorded mint and brokerage session, then `accounts`.
async fn ibkr_server(accounts: Value) -> MockServer {
    let server = MockServer::start().await;
    let (mint, init) = mint_and_init();
    let answer = |a: &Value| ResponseTemplate::new(a["status"].as_u64().unwrap() as u16).set_body_raw(a["body"].as_str().unwrap().as_bytes().to_vec(), "application/json");
    Mock::given(method("POST")).and(path(LST)).respond_with(answer(&mint)).mount(&server).await;
    Mock::given(method("POST")).and(path(INIT)).respond_with(answer(&init)).mount(&server).await;
    Mock::given(method("GET")).and(path(ACCOUNTS)).respond_with(ResponseTemplate::new(200).set_body_json(accounts)).mount(&server).await;
    server
}

#[derive(Clone)]
struct Local(String);
impl deltabadger::sync::jobs::Connect for Local {
    type T = deltabadger::venue::http::ReqwestTransport;
    fn connect(&self, c: &deltabadger::crypto::Credentials) -> deltabadger::venue::alpaca::AlpacaVenue<Self::T> {
        deltabadger::venue::alpaca::AlpacaVenue::new(deltabadger::venue::http::ReqwestTransport::new(deltabadger::venue::http::client(), c.key.clone(), c.secret.clone()),
                                                     deltabadger::venue::alpaca::Urls { trading: self.0.clone(), data: self.0.clone() })
    }
    fn key_check_egress(&self, _: &str, _: Option<String>) -> Result<(String, reqwest::Client), String> { Ok((self.0.clone(), reqwest::Client::builder().no_proxy().build().unwrap())) }
}

async fn validate(fx: &Fx, server: &MockServer) -> Outcome {
    let clock = FixedClock(at(NOW));
    deltabadger::sync::jobs::Validator { venues: Local(server.uri()), key_id: fx.s.api_key_id }.run(fx.cx(&clock), vec![]).await
}

#[tokio::test(flavor = "current_thread")]
async fn the_first_correct_check_binds_the_key_and_a_later_account_is_refused_with_nothing_written() {
    freeze_dh();
    let fx = Fx::new(0);
    // Account A.
    let a = ibkr_server(json!({ "accounts": ["U1234567"], "selectedAccount": "U1234567", "source": "provisional" })).await;
    assert_eq!(validate(&fx, &a).await, Outcome::Done);
    assert_eq!(fx.key().0, 1);
    let bound = fx.bindings();
    assert_eq!(bound.len(), 1);
    assert_eq!(fx.bound_account(fx.s.api_key_id).as_deref(), Some("U1234567"));
    assert_eq!(bound[0].1.len(), 16, "account_ref is an HMAC prefix");
    assert!(!bound[0].1.contains("1234567") && !bound[0].2.contains("1234567"), "the id is stored encrypted only");
    // The same account again: nothing moves.
    assert_eq!(validate(&fx, &a).await, Outcome::Done);
    assert_eq!(fx.bindings(), bound);
    // The login now names account B: refused (incorrect), the binding as it was. Checked through a fresh connection (a
    // restart keeps the binding: it is a row, not memory).
    let b = ibkr_server(json!({ "accounts": ["U7654321"], "source": "provisional" })).await;
    let restarted = Fx { c: Connection::open(&fx.path).unwrap(), ..fx };
    assert!(matches!(validate(&restarted, &b).await, Outcome::Failed(_) | Outcome::Done));
    assert_eq!(restarted.key().0, 2, "another account is refused");
    assert_eq!(restarted.bindings(), bound, "and nothing of the binding moves");
    // The binding is never updated, whatever writes.
    assert!(restarted.c.execute("UPDATE ibkr_bindings SET account_ref = 'x'", []).is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn a_login_with_several_accounts_is_refused_and_binds_nothing() {
    freeze_dh();
    let fx = Fx::new(0);
    let server = ibkr_server(json!({ "accounts": ["U1234567", "U7654321"], "selectedAccount": "U1234567", "source": "provisional" })).await;
    validate(&fx, &server).await;
    assert_eq!(fx.key().0, 2);
    assert!(fx.bindings().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_passive_recheck_of_a_waiting_key_keeps_its_activation_clock() {
    freeze_dh();
    let fx = Fx::new(3);
    let before = fx.key();
    let server = ibkr_server(json!({ "accounts": [], "source": "provisional" })).await;
    assert_eq!(validate(&fx, &server).await, Outcome::Done);
    assert_eq!(fx.key(), before, "ApiKey#update_status!: still pending, updated_at untouched");
    // A pending_validation key that turns out to wait for activation moves (status changed, Active Record's touch).
    fx.c.execute("UPDATE api_keys SET status = 0", []).unwrap();
    validate(&fx, &server).await;
    let after = fx.key();
    assert_eq!(after.0, 3);
    assert_ne!(after.1, before.1);
}

#[tokio::test(flavor = "current_thread")]
async fn the_activation_job_marks_an_activated_key_correct_and_writes_nothing_else() {
    use deltabadger::sync::jobs::{IbkrActivation, IBKR_ACTIVATION};
    freeze_dh();
    let fx = Fx::new(3);
    let job = |uri: String| IbkrActivation { venues: Local(uri) };
    assert_eq!(job(String::new()).spec().name, IBKR_ACTIVATION);
    assert_eq!(job(String::new()).spec().schedule, Some(deltabadger::jobs::schedule::Schedule::EveryHours { every: 6, minute: 17 }));
    let clock = FixedClock(at(NOW));
    // Still waiting: nothing written.
    let waiting = ibkr_server(json!({ "accounts": [], "source": "provisional" })).await;
    let before = fx.key();
    assert_eq!(job(waiting.uri()).run(fx.cx(&clock), vec![]).await, Outcome::Done);
    assert_eq!(fx.key(), before);
    // IBKR down: logged, nothing written.
    let down = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(500)).mount(&down).await;
    job(down.uri()).run(fx.cx(&clock), vec![]).await;
    assert_eq!(fx.key(), before);
    // Activated: correct, bound.
    let active = ibkr_server(json!({ "accounts": ["U1234567"], "source": "provisional" })).await;
    assert_eq!(job(active.uri()).run(fx.cx(&clock), vec![]).await, Outcome::Done);
    assert_eq!(fx.key().0, 1);
    assert_eq!(fx.bound_account(fx.s.api_key_id).as_deref(), Some("U1234567"));
    // No pending key: nothing to do.
    assert_eq!(job(active.uri()).run(fx.cx(&clock), vec![]).await, Outcome::NothingNew);
}

#[test]
fn a_bot_belongs_to_one_account_and_a_reconnect_to_another_account_refuses_it() {
    let fx = Fx::new(1);
    let now = at(NOW);
    let old_bot = seed::insert_bot(&fx.c, &fx.s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    // Not bound yet: the bot cannot start, and belongs to nothing.
    assert!(ibkr_account::check_bot_account(&fx.c, &fx.cipher, old_bot).unwrap().is_err());
    ibkr_account::assign_bot_account(&fx.c, old_bot, now).unwrap();
    assert_eq!(fx.c.query_row("SELECT count(*) FROM ibkr_bot_accounts", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    // Bound to A: the bot belongs to A from its first start.
    ibkr_account::bind(&fx.c, &fx.cipher, fx.s.api_key_id, "U1234567", now).unwrap().unwrap();
    assert_eq!(ibkr_account::check_bot_account(&fx.c, &fx.cipher, old_bot).unwrap(), Ok(()));
    ibkr_account::assign_bot_account(&fx.c, old_bot, now).unwrap();
    assert_eq!(ibkr_account::bot_account(&fx.c, &fx.cipher, old_bot).unwrap(), Ok("U1234567".into()), "reads and cancels go to the bound account");
    let a_ref = ibkr_account::binding(&fx.c, fx.s.api_key_id).unwrap().unwrap().account_ref;
    // Delete A (its binding goes) and connect B with the same holdings: a fresh binding, another namespace.
    ibkr_account::forget(&fx.c, fx.s.api_key_id).unwrap();
    fx.c.execute("DELETE FROM api_keys WHERE id = ?1", [fx.s.api_key_id]).unwrap();
    fx.c.execute("INSERT INTO api_keys (user_id, exchange_id, key_type, status, created_at, updated_at) VALUES (?1, ?2, 0, 1, '2026-10-01', '2026-10-01')",
                 [fx.s.user_id, fx.s.exchange_id]).unwrap();
    let b_key = fx.c.last_insert_rowid();
    fx.set_key(b_key, 1);
    ibkr_account::bind(&fx.c, &fx.cipher, b_key, "U7654321", now).unwrap().unwrap();
    let b_ref = ibkr_account::binding(&fx.c, b_key).unwrap().unwrap().account_ref;
    assert_ne!(a_ref, b_ref, "a fresh namespace for B");
    // The old bot is refused; its ownership never changes.
    let refused = ibkr_account::check_bot_account(&fx.c, &fx.cipher, old_bot).unwrap().unwrap_err();
    assert!(refused.starts_with("This bot belongs to IBKR account U1234567"), "{refused}");
    assert_eq!(ibkr_account::bot_account(&fx.c, &fx.cipher, old_bot).unwrap(), Err(refused.clone()), "no read or cancel for the old bot");
    ibkr_account::assign_bot_account(&fx.c, old_bot, now).unwrap();
    assert_eq!(fx.c.query_row("SELECT account_ref FROM ibkr_bot_accounts WHERE bot_id = ?1", [old_bot], |r| r.get::<_, String>(0)).unwrap(), a_ref);
    assert!(fx.c.execute("UPDATE ibkr_bot_accounts SET account_ref = ?1", [&b_ref]).is_err(), "never updated");
    // A new bot on B belongs to B and may start.
    let new_bot = seed::insert_bot(&fx.c, &fx.s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    ibkr_account::assign_bot_account(&fx.c, new_bot, now).unwrap();
    assert_eq!(ibkr_account::check_bot_account(&fx.c, &fx.cipher, new_bot).unwrap(), Ok(()));
    // A bot on another venue is never asked about.
    fx.c.execute("UPDATE exchanges SET type = 'Exchanges::Kraken'", []).unwrap();
    assert_eq!(ibkr_account::check_bot_account(&fx.c, &fx.cipher, old_bot).unwrap(), Ok(()));
}

#[tokio::test(flavor = "current_thread")]
async fn a_tick_on_a_key_awaiting_activation_reaches_no_venue_and_writes_nothing() {
    let (_d, o, s) = common::install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE api_keys SET status = 3", []).unwrap();
    let v = deltabadger::venue::fake::FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0");
    let clock = FixedClock(at("2026-09-01T10:00:00.5Z"));
    let outcome = tick::tick(&o.primary, &v, id, &clock, &mut Attempts::default()).await.unwrap();
    assert!(matches!(outcome, TickOutcome::Stale { source: "api_key_pending_activation", .. }), "{outcome:?}");
    assert_eq!(v.calls("/0/public/Ticker") + v.calls("/0/private/BalanceEx") + v.sent().len(), 0);
    let transient: String = o.primary.query_row("SELECT transient_data FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert!(!transient.contains("last_action_job_at"), "the checkpoint is not claimed");
    // Activated: the bot trades.
    o.primary.execute("UPDATE api_keys SET status = 1", []).unwrap();
    let v = v.next_add(deltabadger::venue::fake::AddOutcome::Accept("OTX-1".into()));
    assert!(matches!(tick::tick(&o.primary, &v, id, &clock, &mut Attempts::default()).await.unwrap(), TickOutcome::Done { placed: true }));
}

#[tokio::test(flavor = "current_thread")]
async fn a_rebalance_on_a_key_awaiting_activation_is_not_resumable() {
    let (_d, o, s) = common::install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("rebalance_enabled", json!(true)));
    o.primary.execute("UPDATE api_keys SET status = 3", []).unwrap();
    let v = deltabadger::venue::fake::FakeVenue::new();
    let clock = FixedClock(at("2026-09-01T10:00:00Z"));
    let prices = tick::PriceCache::default();
    let cx = tick::TickContext { credential_version: None, prices: &prices, process_start: at("2026-09-01T00:00:00Z"), stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
    let outcome = deltabadger::engine::rebalance::run(&o.primary, &v, id, &clock, &cx).await.unwrap();
    assert!(matches!(outcome, deltabadger::engine::rebalance::Outcome::Skipped("not_resumable")), "{outcome:?}");
    assert_eq!(v.sent().len(), 0);
}

/// Adds AAPL (conid 265598) and IBM (conid 8314) as the IBKR venue's stocks, and USD cash; EUR cash is the seed's.
fn ibkr_catalogue(fx: &Fx) -> (i64, i64, i64, i64) {
    let c = &fx.c;
    let asset = |ext: &str, symbol: &str, category: &str| {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, ?3, '2026-01-01', '2026-01-01')", [ext, symbol, category]).unwrap();
        c.last_insert_rowid()
    };
    let (aapl, ibm, usd) = (asset("AAPL.US", "AAPL", "Stock"), asset("IBM.US", "IBM", "Stock"), asset("usd", "USD", "Currency"));
    let eur: i64 = c.query_row("SELECT id FROM assets WHERE external_id = 'EUR.FOREX'", [], |r| r.get(0)).unwrap();
    for id in [aapl, ibm, usd, eur] {
        c.execute("INSERT INTO exchange_assets (exchange_id, asset_id, created_at, updated_at) SELECT ?1, ?2, '2026-01-01', '2026-01-01' WHERE NOT EXISTS (SELECT 1 FROM exchange_assets WHERE exchange_id = ?1 AND asset_id = ?2)",
                  [fx.s.exchange_id, id]).unwrap();
    }
    for (conid, id) in [(265598, aapl), (8314, ibm)] { c.execute("INSERT INTO ibkr_conids (conid, asset_id, created_at) VALUES (?1, ?2, '2026-01-01')", [conid, id]).unwrap(); }
    (aapl, ibm, usd, eur)
}

async fn sync_balances(fx: &Fx, t: ScriptedTransport) -> Result<deltabadger::sync::balances::Summary, deltabadger::sync::Failure> {
    let db = Db::new(Connection::open(&fx.path).unwrap(), fx.cipher.clone());
    let credentials = deltabadger::sync::credentials(&fx.c, &fx.cipher, fx.s.api_key_id).unwrap();
    let version = credential_version_by_id(&fx.c, fx.s.api_key_id).unwrap().unwrap();
    let login = ibkr::login(&version, credentials.ibkr.as_deref().unwrap()).unwrap();
    deltabadger::sync::balances::sync_ibkr(&db, t, login, &deltabadger::sync::balances::NoPrices, fx.s.api_key_id, &credentials, &FixedClock(at(NOW))).await.unwrap()
}

fn scripted(accounts: Value, pages: &[Value]) -> ScriptedTransport {
    let (mint, init) = mint_and_init();
    let t = ScriptedTransport::default();
    t.reply_typed(&format!("POST {LST}"), 200, "application/json", mint["body"].as_str().unwrap());
    t.reply_typed(&format!("POST {INIT}"), 200, "application/json", init["body"].as_str().unwrap());
    t.reply(&format!("GET {ACCOUNTS}"), 200, accounts);
    t.reply("GET /v1/api/portfolio/U1234567/ledger", 200, json!({
        "USD": { "cashbalance": 100.5, "currency": "USD" }, "EUR": { "cashbalance": 20, "currency": "EUR" },
        "GBP": { "cashbalance": 7, "currency": "GBP" }, "BASE": { "cashbalance": 999, "currency": "BASE" }, "source": "provisional" }));
    for (i, page) in pages.iter().enumerate() { t.reply(&format!("GET /v1/api/portfolio/U1234567/positions/{i}"), 200, page.clone()); }
    t
}

#[tokio::test(flavor = "current_thread")]
async fn balances_map_every_position_page_by_conid_and_cash_by_currency() {
    freeze_dh();
    let fx = Fx::new(1);
    let (aapl, ibm, usd, eur) = ibkr_catalogue(&fx);
    ibkr_account::bind(&fx.c, &fx.cipher, fx.s.api_key_id, "U1234567", at(NOW)).unwrap().unwrap();
    let pages = [json!([{ "conid": 265598, "position": 10.0, "ticker": "AAPL", "source": "provisional" }, { "conid": 999, "position": 3.0, "ticker": "IBM" }]),
                 json!([{ "conid": "8314", "position": 2.0, "ticker": "IBM" }]), json!([])];
    let t = scripted(json!({ "accounts": ["U1234567"] }), &pages);
    sync_balances(&fx, t.clone()).await.unwrap();
    let mut s = fx.c.prepare("SELECT asset_id, free, locked FROM account_balances WHERE exchange_id = ?1 ORDER BY asset_id").unwrap();
    let rows: Vec<(i64, f64, f64)> = s.query_map([fx.s.exchange_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
    let mut expected = vec![(aapl, 10.0, 0.0), (ibm, 2.0, 0.0), (usd, 100.5, 0.0), (eur, 20.0, 0.0)];
    expected.sort_by_key(|r| r.0);
    // The unknown conid 999 is never matched by its symbol (IBM); GBP has no asset; BASE is the account's total.
    assert_eq!(rows, expected);
    assert!(t.requests().iter().any(|r| r.path.ends_with("/positions/2")), "every page, until an empty one");
}

#[tokio::test(flavor = "current_thread")]
async fn balances_of_a_login_that_names_another_account_fail_with_nothing_written() {
    freeze_dh();
    let fx = Fx::new(1);
    ibkr_catalogue(&fx);
    ibkr_account::bind(&fx.c, &fx.cipher, fx.s.api_key_id, "U1234567", at(NOW)).unwrap().unwrap();
    let t = scripted(json!({ "accounts": ["U7654321"] }), &[json!([])]);
    let failed = sync_balances(&fx, t.clone()).await.unwrap_err();
    assert!(failed.error.contains("no longer names the account"), "{}", failed.error);
    assert_eq!(fx.c.query_row("SELECT count(*) FROM account_balances", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    assert!(!t.requests().iter().any(|r| r.path.contains("/portfolio/")), "the other account is never read");
    assert_eq!(fx.key().0, 1, "IBKR names no invalid-key words: the key stays");
}

#[tokio::test(flavor = "current_thread")]
async fn mcp_get_exchange_balances_reads_the_bound_ibkr_account() {
    use deltabadger::web::mcp::{reads, tools::Called};
    freeze_dh();
    let fx = Fx::new(1);
    ibkr_catalogue(&fx);
    let server = ibkr_server(json!({ "accounts": ["U1234567"], "source": "provisional" })).await;
    Mock::given(method("GET")).and(path("/v1/api/portfolio/U1234567/ledger"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "USD": { "cashbalance": 100.5 }, "BASE": { "cashbalance": 999 } }))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1/api/portfolio/U1234567/positions/0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "conid": 265598, "position": 10.0, "source": "provisional" }]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1/api/portfolio/U1234567/positions/1")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&server).await;
    let app = common::web::app(fx._dir.path(), common::web::SECRET, common::web::TestClock::at(NOW))
        .with_venue_key_url(ibkr::IBKR, &server.uri()).unwrap();
    fx.set_key_with(&app.cipher, fx.s.api_key_id, 1);
    let args = json!({ "exchange_name": "Interactive Brokers" });
    // Not bound yet: no account to read.
    let unbound = reads::plan(&fx.c, &app, fx.s.user_id, "get_exchange_balances", &args).unwrap();
    assert!(matches!(&unbound, Called::Done(v) if v.to_string().contains("has not named its account yet")));
    ibkr_account::bind(&fx.c, &app.cipher, fx.s.api_key_id, "U1234567", at(NOW)).unwrap().unwrap();
    let Called::Fetch(fetch) = reads::plan(&fx.c, &app, fx.s.user_id, "get_exchange_balances", &args).unwrap() else { panic!("a venue read") };
    let answer = reads::finish(&fx.c, reads::fetch(&app, fetch).await).unwrap().to_string();
    assert!(answer.contains("Interactive Brokers Balances") && answer.contains("AAPL: 10") && answer.contains("USD: 100.5"), "{answer}");
    assert!(!answer.contains("999"), "BASE is the account's total, not a balance: {answer}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_correct_key_without_a_binding_is_checked_and_bound_by_its_first_sync() {
    freeze_dh();
    let fx = Fx::new(1);
    let (aapl, ..) = ibkr_catalogue(&fx);
    // A key Rails left correct: no binding. The sync checks it once (the binding check), then reads.
    assert!(fx.bindings().is_empty());
    let t = scripted(json!({ "accounts": ["U1234567"] }), &[json!([{ "conid": 265598, "position": 4.0, "source": "provisional" }]), json!([])]);
    sync_balances(&fx, t).await.unwrap();
    assert_eq!(fx.bound_account(fx.s.api_key_id).as_deref(), Some("U1234567"));
    assert_eq!(fx.key().0, 1);
    assert_eq!(fx.c.query_row("SELECT free FROM account_balances WHERE asset_id = ?1", [aapl], |r| r.get::<_, f64>(0)).unwrap(), 4.0);
}

#[tokio::test(flavor = "current_thread")]
async fn an_unbound_correct_key_that_cannot_bind_is_marked_as_the_check_marks_it_and_nothing_else_is_written() {
    freeze_dh();
    let fx = Fx::new(1);
    ibkr_catalogue(&fx);
    let t = scripted(json!({ "accounts": ["U1234567", "U7654321"] }), &[json!([])]);
    let failed = sync_balances(&fx, t.clone()).await.unwrap_err();
    assert!(failed.error.contains("more than one account"), "{}", failed.error);
    assert_eq!(fx.key().0, 2, "several accounts: incorrect, as the key check marks it");
    assert!(fx.bindings().is_empty());
    assert_eq!(fx.c.query_row("SELECT count(*) FROM account_balances", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(fx.c.query_row("SELECT last_sync_error FROM api_keys WHERE id = ?1", [fx.s.api_key_id], |r| r.get::<_, Option<String>>(0)).unwrap(), None);
    assert!(!t.requests().iter().any(|r| r.path.contains("/portfolio/")), "no account is read");
}

#[test]
fn unbound_correct_ibkr_keys_are_validated_at_start() {
    let fx = Fx::new(1);
    assert_eq!(deltabadger::sync::jobs::unbound_ibkr_keys(&fx.c).unwrap(), [fx.s.api_key_id]);
    ibkr_account::bind(&fx.c, &fx.cipher, fx.s.api_key_id, "U1234567", at(NOW)).unwrap().unwrap();
    assert!(deltabadger::sync::jobs::unbound_ibkr_keys(&fx.c).unwrap().is_empty());
    // The validation a wake asks for is the key check that binds.
    let factory = deltabadger::sync::jobs::register(&fx.c, &Local(String::new()), std::rc::Rc::new(deltabadger::sync::balances::NoPrices)).unwrap();
    let spawned = factory.iter().find_map(|j| j.spawn(deltabadger::sync::jobs::API_KEY_VALIDATOR, Some("1"))).expect("the scope factory spawns the validator");
    assert_eq!(spawned.spec().name, deltabadger::sync::jobs::API_KEY_VALIDATOR);
}

#[tokio::test(flavor = "current_thread")]
async fn an_activated_key_wakes_its_balance_sync() {
    use deltabadger::sync::jobs::{IbkrActivation, BALANCE_SYNC};
    freeze_dh();
    let fx = Fx::new(3);
    let clock = FixedClock(at(NOW));
    let active = ibkr_server(json!({ "accounts": ["U1234567"], "source": "provisional" })).await;
    let cx = fx.cx(&clock);
    let wakers = cx.wakers.clone();
    assert_eq!(IbkrActivation { venues: Local(active.uri()) }.run(cx, vec![]).await, Outcome::Done);
    assert!(wakers.pending().iter().any(|(job, scope)| job == BALANCE_SYNC && scope.as_deref() == Some(&fx.s.api_key_id.to_string())),
            "the scope factory registers the key's balance sync without a restart");
}

#[tokio::test(flavor = "current_thread")]
async fn negative_balances_are_live_answers_but_never_stored() {
    use deltabadger::web::mcp::{reads, tools::Called};
    freeze_dh();
    let fx = Fx::new(1);
    let (aapl, ..) = ibkr_catalogue(&fx);
    // A short position and a margin loan (provisional shapes).
    let ledger = json!({ "USD": { "cashbalance": -50.25 }, "EUR": { "cashbalance": 0 }, "source": "provisional" });
    let positions = json!([{ "conid": 265598, "position": -3.0, "source": "provisional" }, { "conid": 8314, "position": 2.0 }]);
    let server = ibkr_server(json!({ "accounts": ["U1234567"] })).await;
    Mock::given(method("GET")).and(path("/v1/api/portfolio/U1234567/ledger")).respond_with(ResponseTemplate::new(200).set_body_json(ledger.clone())).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1/api/portfolio/U1234567/positions/0")).respond_with(ResponseTemplate::new(200).set_body_json(positions.clone())).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1/api/portfolio/U1234567/positions/1")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&server).await;
    let app = common::web::app(fx._dir.path(), common::web::SECRET, common::web::TestClock::at(NOW)).with_venue_key_url(ibkr::IBKR, &server.uri()).unwrap();
    fx.set_key_with(&app.cipher, fx.s.api_key_id, 1);
    ibkr_account::bind(&fx.c, &app.cipher, fx.s.api_key_id, "U1234567", at(NOW)).unwrap().unwrap();
    let Called::Fetch(fetch) = reads::plan(&fx.c, &app, fx.s.user_id, "get_exchange_balances", &json!({ "exchange_name": "Interactive Brokers" })).unwrap() else { panic!() };
    let answer = reads::finish(&fx.c, reads::fetch(&app, fetch).await).unwrap().to_string();
    // BotApi::Exchanges::Balances#build_balances drops only zero balances.
    assert!(answer.contains("AAPL: -3") && answer.contains("USD: -50.25") && answer.contains("IBM: 2") && !answer.contains("EUR"), "{answer}");
    // The persisted sync keeps holdings only.
    fx.set_key(fx.s.api_key_id, 1);
    let t = ScriptedTransport::default();
    let (mint, init) = mint_and_init();
    t.reply_typed(&format!("POST {LST}"), 200, "application/json", mint["body"].as_str().unwrap());
    t.reply_typed(&format!("POST {INIT}"), 200, "application/json", init["body"].as_str().unwrap());
    t.reply(&format!("GET {ACCOUNTS}"), 200, json!({ "accounts": ["U1234567"] }));
    t.reply("GET /v1/api/portfolio/U1234567/ledger", 200, ledger);
    t.reply("GET /v1/api/portfolio/U1234567/positions/0", 200, positions);
    t.reply("GET /v1/api/portfolio/U1234567/positions/1", 200, json!([]));
    fx.c.execute("DELETE FROM ibkr_bindings", []).unwrap();
    ibkr_account::bind(&fx.c, &fx.cipher, fx.s.api_key_id, "U1234567", at(NOW)).unwrap().unwrap();
    sync_balances(&fx, t).await.unwrap();
    let stored: Vec<i64> = fx.c.prepare("SELECT asset_id FROM account_balances").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert!(!stored.contains(&aapl) && stored.len() == 1, "{stored:?}");
}

/// D-IBKR-INCONCLUSIVE: a check with no verdict (IBKR down, a timeout) never moves an IBKR key's status. Rails'
/// update_status! writes pending_validation, which takes a correct key out of every sync and of the startup validation
/// for good (nothing schedules that validator again).
#[tokio::test(flavor = "current_thread")]
async fn an_inconclusive_startup_validation_keeps_a_correct_unbound_key_correct_until_a_verdict_binds_it() {
    freeze_dh();
    let fx = Fx::new(1);
    let before = fx.key();
    let down = MockServer::start().await;
    let (mint, init) = mint_and_init();
    let answer = |a: &Value| ResponseTemplate::new(a["status"].as_u64().unwrap() as u16).set_body_raw(a["body"].as_str().unwrap().as_bytes().to_vec(), "application/json");
    Mock::given(method("POST")).and(path(LST)).respond_with(answer(&mint)).mount(&down).await;
    Mock::given(method("POST")).and(path(INIT)).respond_with(answer(&init)).mount(&down).await;
    Mock::given(method("GET")).and(path(ACCOUNTS)).respond_with(ResponseTemplate::new(503)).mount(&down).await;
    validate(&fx, &down).await;
    assert_eq!(fx.key(), before, "a 503 writes nothing: still correct");
    // A restart: still listed for the startup validation; the next one binds it.
    let restarted = Fx { c: Connection::open(&fx.path).unwrap(), ..fx };
    assert_eq!(deltabadger::sync::jobs::unbound_ibkr_keys(&restarted.c).unwrap(), [restarted.s.api_key_id]);
    let up = ibkr_server(json!({ "accounts": ["U1234567"], "source": "provisional" })).await;
    assert_eq!(validate(&restarted, &up).await, Outcome::Done);
    assert_eq!(restarted.key().0, 1);
    assert_eq!(restarted.bound_account(restarted.s.api_key_id).as_deref(), Some("U1234567"));
    assert!(deltabadger::sync::jobs::unbound_ibkr_keys(&restarted.c).unwrap().is_empty());
}
