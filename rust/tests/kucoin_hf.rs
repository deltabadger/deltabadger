//! HF-0 contracts and HF-1 M01–M05 resolver/preparation behavior. HF order execution remains disconnected.
mod common;
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr;

use deltabadger::engine::model::{self, CredentialVersion};
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::kucoin::{KucoinVenue, URL};
use deltabadger::venue::kucoin_account::{AccountMode, Resolver, MODE, OPENED};
use deltabadger::venue::Venue;
use serde_json::json;

fn credentials() -> (rusqlite::Connection, CredentialVersion) {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE api_keys(id INTEGER PRIMARY KEY, key TEXT, secret TEXT, passphrase TEXT, access_token TEXT, rsa_signature_key TEXT, rsa_encryption_key TEXT, dh_param TEXT);
        INSERT INTO api_keys(id,key,secret,passphrase) VALUES(1,'key','secret','passphrase'),(2,'other','secret','passphrase');").unwrap();
    let version = model::credential_version_by_id(&c, 1).unwrap().unwrap();
    (c, version)
}

fn classic(t: &ScriptedTransport, opened: bool) {
    t.reply(&format!("GET {MODE}"), 200, json!({"code":"200000","data":{"selfAccountMode":"CLASSIC"}}));
    t.reply(&format!("GET {OPENED}"), 200, json!({"code":"200000","data":opened}));
}

#[tokio::test(flavor = "current_thread")]
async fn m01_account_fixture_runs_through_version_bound_resolver() {
    let (_db, version) = credentials();
    let f = fixture("account");
    let mut count = 0;
    for c in f["cases"].as_array().unwrap().iter().filter(|c| matches!(c["endpoint"].as_str(), Some("mode" | "opened"))) {
        count += 1;
        let t = ScriptedTransport::default();
        if c["endpoint"] == "opened" {
            t.reply(&format!("GET {MODE}"), 200, json!({"code":"200000","data":{"selfAccountMode":"CLASSIC"}}));
        }
        let path = c["request"]["path"].as_str().unwrap();
        if c["response"]["transport_error"].is_string() {
            t.network(&format!("GET {path}"), "post_send", "secret passphrase untrusted remote text");
        } else {
            t.reply(&format!("GET {path}"), c["response"]["status"].as_u64().unwrap() as u16, c["response"]["body"].clone());
        }
        if c["endpoint"] == "mode" && c["expect"]["mode"] != "Unknown" && c["expect"]["mode"] != "UnsupportedUta" {
            t.reply(&format!("GET {OPENED}"), 200, c["then"]["response"]["body"].clone());
        }
        let answer = Resolver::default().resolve(&t, URL, &version).await;
        assert!(answer.current_for(&Some(version.clone())).is_fresh());
        match c["expect"]["mode"].as_str().unwrap() {
            "Unknown" => {
                let error = format!("{:?}", answer.value.unwrap_err());
                assert!(error.contains("KuCoin account mode could not be determined"), "{c}: {error}");
                assert!(error.contains("order not sent"));
                assert!(!error.contains("secret") && !error.contains("passphrase"));
            }
            mode => assert_eq!(answer.value.unwrap().mode.as_str(), mode, "{}", c["id"]),
        }
        assert!(t.requests().iter().all(|r| r.method == "GET"));
        if c["expect"]["mode"] == "UnsupportedUta" { assert_eq!(t.requests().len(), 1); }
    }
    assert_eq!(count, 3 + 7 + 6, "3 detected modes + 7 mode faults + 6 opened faults");
}

#[tokio::test(flavor = "current_thread")]
async fn m02_cache_shares_only_host_key_version_and_never_reuses_expired_success() {
    let (db, version) = credentials();
    let resolver = Resolver::default();
    let t = ScriptedTransport::default();
    let at = "2026-10-30T12:00:00Z".parse().unwrap();
    t.set_now(at);
    classic(&t, false);
    let (a,b) = tokio::join!(resolver.resolve(&t, URL, &version), resolver.resolve(&t, URL, &version));
    assert_eq!(a.value.unwrap().mode, AccountMode::ClassicLf);
    assert_eq!(b.value.unwrap().mode, AccountMode::ClassicLf);
    assert_eq!(t.requests().len(), 2);
    let other = model::credential_version_by_id(&db, 2).unwrap().unwrap();
    resolver.resolve(&t, URL, &other).await.value.unwrap();
    resolver.resolve(&t, "https://another.example", &version).await.value.unwrap();
    assert_eq!(t.requests().len(), 6);
    t.set_now(at + chrono::Duration::seconds(299));
    resolver.resolve(&t, URL, &version).await.value.unwrap();
    assert_eq!(t.requests().len(), 6);
    t.set_now(at + chrono::Duration::seconds(300));
    let failed = ScriptedTransport::default();
    failed.set_now(at + chrono::Duration::seconds(300));
    failed.network(&format!("GET {MODE}"), "post_send", "timeout");
    assert!(resolver.resolve(&failed, URL, &version).await.value.is_err());
    resolver.resolve(&t, URL, &version).await.value.unwrap();
    assert_eq!(t.requests().len(), 8);
    resolver.invalidate(URL, version.id);
    resolver.resolve(&t, URL, &version).await.value.unwrap();
    assert_eq!(t.requests().len(), 10);
    Resolver::default().resolve(&t, URL, &version).await.value.unwrap();
    assert_eq!(t.requests().len(), 12, "a process restart has no cache");
}

#[tokio::test(flavor = "current_thread")]
async fn m03_replaced_credential_material_requires_detection_and_keeps_old_provenance() {
    let (db, mut version) = credentials();
    let resolver = Resolver::default();
    let t = ScriptedTransport::default();
    classic(&t, false);
    for column in ["key", "secret", "passphrase"] {
        let old = resolver.resolve(&t, URL, &version).await;
        db.execute(&format!("UPDATE api_keys SET {column}='replacement' WHERE id=1"), []).unwrap();
        version = model::credential_version_by_id(&db, 1).unwrap().unwrap();
        assert!(!old.current_for(&Some(version.clone())).is_fresh());
        assert!(!model::produced_is_current(&db, &old).unwrap());
        resolver.resolve(&t, URL, &version).await.value.unwrap();
    }
    assert_eq!(t.requests().len(), 8);
}

#[tokio::test(flavor = "current_thread")]
async fn m04_preparation_keeps_immutable_family_and_hf_trading_disconnected() {
    let (_db, version) = credentials();
    for hf in [false, true] {
        let t = ScriptedTransport::default();
        classic(&t, hf);
        let v = KucoinVenue::new(t.clone(), URL).with_account_context(version.clone(), Resolver::default());
        let prepared = v.prepare().await;
        if hf {
            assert!(format!("{:?}", prepared.unwrap_err()).contains("not connected"));
        } else {
            prepared.unwrap();
            assert_eq!(v.placement_identity().unwrap()["kucoin_account_mode"], "ClassicLf");
        }
        assert_eq!(t.requests().len(), 2);
    }
}

#[derive(Clone)]
struct Hook {
    script: ScriptedTransport,
    path: &'static str,
    action: HookAction,
}
type HookAction = std::rc::Rc<std::cell::RefCell<Option<Box<dyn FnOnce()>>>>;
impl deltabadger::venue::http::Transport for Hook {
    fn now(&self) -> chrono::DateTime<chrono::Utc> { deltabadger::venue::http::Transport::now(&self.script) }
    async fn send(&self, r: &deltabadger::venue::http::HttpRequest) -> Result<deltabadger::venue::http::HttpResponse, deltabadger::venue::http::TransportError> {
        tokio::task::yield_now().await;
        let response = self.script.send(r).await;
        if r.path == self.path { if let Some(action) = self.action.borrow_mut().take() { action(); } }
        response
    }
}
fn hook(t: &ScriptedTransport, path: &'static str, action: impl FnOnce() + 'static) -> Hook {
    Hook { script: t.clone(), path, action: std::rc::Rc::new(std::cell::RefCell::new(Some(Box::new(action)))) }
}

#[tokio::test(flavor = "current_thread")]
async fn m02_single_flight_and_validation_invalidates_inflight_evidence() {
    let (_db, version) = credentials();
    let t = ScriptedTransport::default();
    classic(&t, false);
    let resolver = Resolver::default();
    let yielding = hook(&t, MODE, || {});
    let (a,b) = tokio::join!(resolver.resolve(&yielding, URL, &version), resolver.resolve(&yielding, URL, &version));
    a.value.unwrap(); b.value.unwrap();
    assert_eq!(t.requests().len(), 2, "both futures overlap at the first HTTP read");
    resolver.invalidate(URL, version.id);
    let invalidate = resolver.clone();
    let yielding = hook(&t, MODE, move || invalidate.invalidate(URL, 1));
    assert!(resolver.resolve(&yielding, URL, &version).await.value.is_err());
    resolver.resolve(&t, URL, &version).await.value.unwrap();
    assert_eq!(t.requests().len(), 6, "invalidated in-flight evidence cannot populate the cache");
}

fn engine_setup() -> (tempfile::TempDir, deltabadger::store::Opened, model::Bot, deltabadger::engine::amount::OrderPlan) {
    use common::seed::{self, BotSpec};
    use deltabadger::engine::amount::{self, Sizing};
    use deltabadger::ruby::BigDec;
    let dir = common::rails_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    o.primary.execute_batch("UPDATE exchanges SET type='Exchanges::Kucoin',name='KuCoin'; UPDATE tickers SET ticker='BTC-EUR',base='BTC'").unwrap();
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(50_000), &deltabadger::engine::venue_rules::KUCOIN).unwrap() else { panic!() };
    (dir, o, bot, plan)
}

fn no_placement(c: &rusqlite::Connection, bot: i64) {
    assert!(model::load_bot(c, bot).unwrap().rust_placement().is_none());
    assert_eq!(c.query_row("SELECT count(*) FROM transactions", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn m01_detection_refusal_has_no_intent_transaction_balance_or_placement_effect() {
    use deltabadger::engine::{tick, FixedClock};
    let f = fixture("account");
    let mut count = 0;
    for c in f["cases"].as_array().unwrap().iter().filter(|c| c["expect"]["mode"] == "Unknown" || matches!(c["id"].as_str(), Some("detect_hf" | "detect_uta"))) {
        count += 1;
        let (dir, o, bot, _) = engine_setup();
        let version = model::credential_version(&o.primary, &bot).unwrap().unwrap();
        let t = ScriptedTransport::default();
        let at = "2026-10-30T12:00:00Z".parse().unwrap();
        t.set_now(at);
        if c["endpoint"] == "opened" { t.reply(&format!("GET {MODE}"), 200, json!({"code":"200000","data":{"selfAccountMode":"CLASSIC"}})); }
        let path = c["request"]["path"].as_str().unwrap();
        if c["response"]["transport_error"].is_string() { t.network(&format!("GET {path}"), "post_send", "secret passphrase"); }
        else { t.reply(&format!("GET {path}"), c["response"]["status"].as_u64().unwrap() as u16, c["response"]["body"].clone()); }
        if c["id"] == "detect_hf" { t.reply(&format!("GET {OPENED}"), 200, c["then"]["response"]["body"].clone()); }
        let venue = KucoinVenue::new(t.clone(), URL).with_account_context(version, Resolver::default());
        tick::tick(&o.primary, &venue, bot.id, &FixedClock(at), &mut Default::default()).await.unwrap();
        no_placement(&o.primary, bot.id);
        assert!(t.requests().iter().all(|r| r.path == MODE || r.path == OPENED));
        // The persisted refusal survives restart and contains no credential material.
        let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        no_placement(&c, bot.id);
        assert_eq!(c.query_row("SELECT count(*) FROM account_balances", [], |r| r.get::<_,i64>(0)).unwrap(), 0, "no synthetic balances");
        let transient: String = c.query_row("SELECT details FROM bot_activity_logs WHERE bot_id=?1 AND event='execution_failed' ORDER BY id DESC LIMIT 1", [bot.id], |r| r.get(0)).unwrap();
        assert!(transient.contains("KuCoin"), "refusal is visible: {transient}");
        assert!(!transient.contains("secret") && !transient.contains("passphrase"));
    }
    assert_eq!(count, 7 + 6 + 2, "13 unknown-mode faults + HF + UTA");
}

#[tokio::test(flavor = "current_thread")]
async fn m04_expired_preparation_refreshes_before_dispatch_and_never_switches_family() {
    use deltabadger::engine::{placement, FixedClock};
    for changed in [false, true] {
        let (dir, o, bot, plan) = engine_setup();
        let version = model::credential_version(&o.primary, &bot).unwrap().unwrap();
        let t = ScriptedTransport::default();
        let at = "2026-10-30T12:00:00Z".parse().unwrap();
        t.set_now(at);
        classic(&t, false);
        t.reply(&format!("GET {OPENED}"), 200, json!({"code":"200000","data":changed}));
        t.reply("POST /api/v1/orders", 200, json!({"code":"200000","data":{"orderId":"accepted"}}));
        let v = KucoinVenue::new(t.clone(), URL).with_account_context(version, Resolver::default());
        v.prepare().await.unwrap();
        t.set_now(at + chrono::Duration::seconds(300));
        let clock = FixedClock(at + chrono::Duration::seconds(300));
        let placement::Begun::Intent(intent) = placement::begin_held(&o.primary, &bot, &plan, v.placement_identity(), None, &clock).unwrap() else { panic!() };
        let raw = model::load_bot(&o.primary, bot.id).unwrap().rust_placement().unwrap();
        assert_eq!(raw["kucoin_account_mode"], "ClassicLf");
        assert_eq!(raw["cl_ord_id"], intent.cl_ord_id);
        let sent = placement::send_recorded(&o.primary, &v, &bot, &intent, &clock).await.unwrap();
        if changed {
            assert!(matches!(sent, placement::Sent::NotSent(_)));
            no_placement(&o.primary, bot.id);
        } else {
            assert!(matches!(sent, placement::Sent::Accepted(_)));
            let requests = t.requests();
            let post = requests.iter().find(|r| r.method == "POST").unwrap();
            assert_eq!(post.body.as_ref().unwrap()["clientOid"], intent.cl_ord_id);
        }
        assert_eq!(t.requests().len(), if changed { 4 } else { 5 });
        let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM transactions", [], |r| r.get::<_,i64>(0)).unwrap(), if changed { 0 } else { 1 });
    }
}

#[tokio::test(flavor = "current_thread")]
async fn m03_rotation_during_expired_preparation_cannot_dispatch_or_relabel_intent() {
    use deltabadger::engine::{placement, FixedClock};
    let (dir, o, bot, plan) = engine_setup();
    let version = model::credential_version(&o.primary, &bot).unwrap().unwrap();
    let t = ScriptedTransport::default();
    let at = "2026-10-30T12:00:00Z".parse().unwrap();
    t.set_now(at);
    classic(&t, false);
    let resolver = Resolver::default();
    let v = KucoinVenue::new(t.clone(), URL).with_account_context(version.clone(), resolver.clone());
    v.prepare().await.unwrap();
    t.set_now(at + chrono::Duration::seconds(300));
    let clock = FixedClock(at + chrono::Duration::seconds(300));
    let placement::Begun::Intent(intent) = placement::begin_held(&o.primary, &bot, &plan, v.placement_identity(), None, &clock).unwrap() else { panic!() };
    let writer = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let h = hook(&t, MODE, move || { writer.execute("UPDATE api_keys SET secret='replacement'", []).unwrap(); });
    let v = KucoinVenue::new(h, URL).with_account_context(version, resolver);
    assert!(matches!(placement::send_recorded(&o.primary, &v, &bot, &intent, &clock).await, Err(deltabadger::engine::EngineError::CredentialsChanged)));
    assert!(t.requests().iter().all(|r| r.method == "GET"));
    assert_eq!(model::load_bot(&o.primary, bot.id).unwrap().rust_placement().unwrap()["cl_ord_id"], intent.cl_ord_id);
    assert_eq!(o.primary.query_row("SELECT count(*) FROM transactions", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn m04_failed_refresh_never_clears_an_earlier_ambiguous_dispatch() {
    use deltabadger::engine::{placement, FixedClock};
    let (_dir, o, bot, plan) = engine_setup();
    let version = model::credential_version(&o.primary, &bot).unwrap().unwrap();
    let t = ScriptedTransport::default();
    let at = "2026-10-30T12:00:00Z".parse().unwrap();
    t.set_now(at);
    classic(&t, false);
    t.network("POST /api/v1/orders", "post_send", "lost reply");
    let resolver = Resolver::default();
    let v = KucoinVenue::new(t.clone(), URL).with_account_context(version.clone(), resolver.clone());
    v.prepare().await.unwrap();
    let placement::Begun::Intent(intent) = placement::begin_held(&o.primary, &bot, &plan, v.placement_identity(), None, &FixedClock(at)).unwrap() else { panic!() };
    assert!(matches!(placement::send_recorded(&o.primary, &v, &bot, &intent, &FixedClock(at)).await.unwrap(), placement::Sent::Ambiguous(_)));
    resolver.invalidate(URL, version.id);
    let fail = ScriptedTransport::default();
    fail.network(&format!("GET {MODE}"), "post_send", "timeout");
    let v = KucoinVenue::new(fail.clone(), URL).with_account_context(version, resolver);
    assert!(matches!(placement::send_recorded(&o.primary, &v, &bot, &intent, &FixedClock(at)).await.unwrap(), placement::Sent::Ambiguous(_)));
    assert_eq!(fail.requests().len(), 1);
    assert_eq!(model::load_bot(&o.primary, bot.id).unwrap().rust_placement().unwrap()["cl_ord_id"], intent.cl_ord_id);
    assert_eq!(o.primary.query_row("SELECT count(*) FROM transactions", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn m04_signal_rebalance_liquidation_and_redeploy_prepare_before_sizing() {
    use deltabadger::engine::{liquidation, rebalance, redeploy, signal, tick, FixedClock};
    for operation in ["signal", "rebalance", "liquidation", "redeploy"] {
        let (dir, o, bot, plan) = engine_setup();
        let at = "2026-10-30T12:00:00Z".parse().unwrap();
        let t = ScriptedTransport::default();
        t.set_now(at);
        t.network(&format!("GET {MODE}"), "post_send", "timeout");
        let version = model::credential_version(&o.primary, &bot).unwrap().unwrap();
        let venue = KucoinVenue::new(t.clone(), URL).with_account_context(version.clone(), Resolver::default());
        let prices = tick::PriceCache::default();
        let cx = tick::TickContext { credential_version: Some(version), prices: &prices, process_start: chrono::DateTime::<chrono::Utc>::MIN_UTC,
            stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
        if operation == "signal" {
            o.primary.execute("UPDATE bots SET type='Bots::Signal',settings=json_set(settings,'$.base_asset_id',?1) WHERE id=?2", [plan.ticker.base_asset_id, bot.id]).unwrap();
            o.primary.execute("INSERT INTO bot_signals(bot_id,direction,amount_type,amount,enabled,token,created_at,updated_at) VALUES(?1,0,0,60,1,'hf1','2026-10-30','2026-10-30')", [bot.id]).unwrap();
            let signal_id = o.primary.last_insert_rowid();
            model::append_signal_call(&o.primary, bot.id, &model::SignalCall { source: "hf1".into(), signal_id, triggered_at: at }).unwrap();
            assert_eq!(signal::run_next(&o.primary, &venue, bot.id, &FixedClock(at), &cx).await.unwrap(), signal::Step::Ran);
        } else {
            o.primary.execute("UPDATE bots SET settings=json_set(settings,'$.rebalance_enabled',json('true')) WHERE id=?1", [bot.id]).unwrap();
            let result = match operation {
                "rebalance" => rebalance::run(&o.primary, &venue, bot.id, &FixedClock(at), &cx).await.map(|_| ()),
                "liquidation" => liquidation::run(&o.primary, &venue, bot.id, &json!([["BTC", plan.ticker.base_asset_id]]), &FixedClock(at), &cx).await,
                "redeploy" => redeploy::run(&o.primary, &venue, bot.id, &FixedClock(at), &cx).await,
                _ => unreachable!(),
            };
            assert!(format!("{:?}", result.unwrap_err()).contains("KuCoin account mode could not be determined"), "{operation}");
        }
        assert_eq!(t.requests().len(), 1, "{operation}: no price, balance or placement requests");
        no_placement(&o.primary, bot.id);
        no_placement(&rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap(), bot.id);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn m03_detection_and_balance_races_preserve_stored_balances_and_watermark() {
    use deltabadger::engine::FixedClock;
    use deltabadger::sync::{self, balances};
    // An HF account's balances are read since HF-3 (`trade_hf`, kucoin_hf_sync.rs): no longer a refusal to race.
    for phase in ["detection", "balance", "failure", "ledger", "checkpoint"] {
        let (dir, o, bot, plan) = engine_setup();
        let version = model::credential_version(&o.primary, &bot).unwrap().unwrap();
        let key = version.id;
        o.primary.execute("INSERT INTO account_balances(user_id,exchange_id,asset_id,free,locked,synced_at,created_at,updated_at) VALUES(?1,?2,?3,17,3,'2026-10-29','2026-10-29','2026-10-29')", rusqlite::params![bot.user_id, bot.exchange_id, plan.ticker.base_asset_id]).unwrap();
        let credentials = sync::validation_material(&o.primary, &common::seed::cipher(), key).unwrap().0;
        let t = ScriptedTransport::default();
        classic(&t, false);
        t.reply("GET /api/v1/accounts", 200, json!({"code":"200000","data":[{"currency":"BTC","available":"99","holds":"0"}]}));
        for endpoint in [sync::kucoin_ledger::FILLS_PATH, sync::kucoin_ledger::DEPOSITS_PATH, sync::kucoin_ledger::WITHDRAWALS_PATH] {
            t.reply(&format!("GET {endpoint}"), 200, json!({"code":"200000","data":{"currentPage":1,"totalPage":1,"totalNum":0,"items":[]}}));
        }
        let writer = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        let path = match phase { "balance" => "/api/v1/accounts", "checkpoint" => sync::kucoin_ledger::FILLS_PATH, _ => MODE };
        let rotate = matches!(phase, "detection" | "balance" | "ledger" | "checkpoint");
        let h = hook(&t, path, move || { if rotate { writer.execute("UPDATE api_keys SET passphrase='replacement'", []).unwrap(); } });
        let db = deltabadger::jobs::Db::new(rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap(), common::seed::cipher());
        if phase == "checkpoint" {
            assert!(sync::kucoin_ledger::sync(&db, h, key, &credentials, &FixedClock("2026-10-30T12:00:00Z".parse().unwrap()), std::time::Duration::ZERO).await.is_err());
        } else if phase == "failure" || phase == "ledger" {
            // Use a separate transport so the fault is the first response, not a queued retry.
            let fail = ScriptedTransport::default();
            fail.network(&format!("GET {MODE}"), "post_send", "secret passphrase");
            let h = Hook { script: fail.clone(), ..h };
            let result = sync::kucoin_ledger::sync(&db, h, key, &credentials, &FixedClock("2026-10-30T12:00:00Z".parse().unwrap()), std::time::Duration::ZERO).await;
            if phase == "ledger" { assert!(result.is_err()); } else { assert!(result.unwrap().is_err()); }
            assert_eq!(fail.requests().len(), 1);
        } else {
            let result = balances::sync_kucoin(&db, h, &balances::NoPrices, key, &credentials, &FixedClock("2026-10-30T12:00:00Z".parse().unwrap())).await;
            if rotate { assert!(result.is_err()); } else { assert!(result.unwrap().is_err()); }
        }
        db.settled().await;
        let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        assert_eq!(c.query_row("SELECT free,locked FROM account_balances", [], |r| Ok((r.get::<_,f64>(0)?,r.get::<_,f64>(1)?))).unwrap(), (17.0,3.0));
        assert_eq!(c.query_row("SELECT last_synced_at FROM api_keys WHERE id=?1", [key], |r| r.get::<_,Option<String>>(0)).unwrap(), None);
        no_placement(&c, bot.id);
    }
}

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kucoin_hf") }
fn fixture(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root().join(format!("{name}.json"))).unwrap()).unwrap()
}
fn case<'a>(f: &'a Value, id: &str) -> &'a Value {
    f["cases"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("missing {id}"))
}
fn hmac(secret: &str, input: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(input.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

#[test]
fn kucoin_hf_provenance_and_generated_assets_are_pinned() {
    let m = fixture("manifest");
    assert_eq!(m["schema_version"], 1);
    assert_eq!(m["sdk_revision"].as_str().unwrap().len(), 40);
    assert_eq!(m["oracle"], "official-docs-and-sdk; synthetic variants are not exchange recordings");
    for (name, hash) in m["generated_sha256"].as_object().unwrap() {
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root().join(name)).unwrap())), hash.as_str().unwrap(), "regenerate {name}");
    }
    let recorder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../script/rust/record_kucoin_hf.py");
    assert_eq!(hex::encode(Sha256::digest(std::fs::read(recorder).unwrap())), m["recorder_sha256"]);
    for source in m["sources"].as_object().unwrap().values() {
        assert!(source["url"].as_str().unwrap().starts_with("https://"));
        assert_eq!(source["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(source["fetched_date"], "2026-10-30");
    }
    let mut ids = BTreeSet::new();
    let mut total = 0;
    for name in ["account", "orders", "fills", "errors", "signing"] {
        let f = fixture(name);
        for c in f["cases"].as_array().unwrap() {
            total += 1;
            assert!(ids.insert(c["id"].as_str().unwrap().to_owned()));
            assert_eq!(c["provenance"]["kind"], "synthetic-variant");
            assert!(!c["provenance"]["note"].as_str().unwrap().is_empty());
            for source in c["provenance"]["sources"].as_array().unwrap() {
                assert!(m["sources"].get(source.as_str().unwrap()).is_some());
            }
            assert!(c["expect"].is_object());
        }
        assert_eq!(f["cases"].as_array().unwrap().len() as u64, m["case_counts"][name].as_u64().unwrap());
    }
    assert_eq!(total, m["case_counts"]["total"].as_u64().unwrap(), "sum of account + orders + fills + errors + signing cases");
    assert_eq!(total, 21 + 34 + 10 + 24 + 10, "21 account + 34 orders + 10 fills + 24 errors + 10 signing = 99 cases");
}

#[test]
fn kucoin_hf_endpoint_contract_covers_every_planned_route() {
    let m = fixture("manifest");
    let endpoints = m["endpoints"].as_object().unwrap();
    for (id, method, path) in [
        ("mode", "GET", "/api/ua/v2/account/mode"),
        ("opened", "GET", "/api/v1/hf/accounts/opened"),
        ("balances", "GET", "/api/v1/accounts"),
        ("apikey", "GET", "/api/v1/user/api-key"),
        ("place", "POST", "/api/v1/hf/orders"),
        ("status_id", "GET", "/api/v1/hf/orders/{orderId}"),
        ("status_client", "GET", "/api/v1/hf/orders/client-order/{clientOid}"),
        ("fills", "GET", "/api/v1/hf/fills"),
        ("cancel", "DELETE", "/api/v1/hf/orders/{orderId}"),
    ] {
        assert_eq!(endpoints[id]["method"], method);
        assert_eq!(endpoints[id]["path"], path);
        assert!(m["sources"].get(id).is_some());
    }
    assert_eq!(endpoints.len(), 9);
    for id in ["status_id", "status_client", "fills", "cancel"] {
        assert!(endpoints[id]["required_query"].as_array().unwrap().contains(&Value::from("symbol")));
    }
    assert_eq!(endpoints["balances"]["query"], serde_json::json!([["type", "trade_hf"]]));
}

#[test]
fn kucoin_hf_source_examples_remain_separate_from_coherent_scenarios() {
    let a = fixture("account");
    assert_eq!(a["examples"]["opened"]["body"]["data"], false);
    assert_eq!(a["examples"]["balances"]["body"]["data"][0]["type"], "trade");
    let o = fixture("orders");
    // The published request/response IDs differ; do not silently repair the source example.
    assert_ne!(o["examples"]["status_id"]["body"]["data"]["id"], "6717422bd51c29000775ea03");
    assert_ne!(o["examples"]["status_client"]["body"]["data"]["clientOid"], "5c52e11203aa677f33e493fb");
    let f = fixture("fills");
    let item = &f["examples"]["fills"]["body"]["data"]["items"][0];
    assert_eq!(item["tradeId"].as_u64(), Some(11_029_373_945_659_392));
    assert_ne!(item["id"], item["tradeId"]);
    assert!(item["createdAt"].as_u64().unwrap() > 1_728_692_138_000, "published request bounds do not contain its response");
}

#[test]
fn kucoin_hf_detection_contract_never_guesses_or_synthesizes_zero() {
    let f = fixture("account");
    for (id, mode) in [("detect_classic", "ClassicLf"), ("detect_hf", "ClassicHf"), ("detect_uta", "UnsupportedUta")] {
        assert_eq!(case(&f, id)["expect"]["mode"], mode);
    }
    for c in f["cases"].as_array().unwrap() {
        assert_eq!(c["expect"]["placement_posts"], 0);
        if c["expect"]["mode"] == "Unknown" || c["id"].as_str().unwrap().starts_with("balances_fail") {
            assert_eq!(c["expect"]["preserve_stored_balances"], true);
            assert_eq!(c["expect"]["placement_intents_created"], 0);
        }
    }
    assert_eq!(case(&f, "balances_holds")["expect"]["available"], "25.12345678");
    assert_eq!(case(&f, "balances_holds")["expect"]["tracker_total"], "26.66759503");
    assert_eq!(case(&f, "balances_locked")["expect"], case(&f, "balances_holds")["expect"]);
    assert_eq!(case(&f, "permissions_general_only")["expect"]["trading_allowed"], false);
}

#[test]
fn kucoin_hf_wire_cases_preserve_intent_uuid_and_sizing_inputs() {
    let f = fixture("orders");
    for side in ["buy", "sell"] {
        for shape in ["market_size", "market_funds", "limit"] {
            let c = case(&f, &format!("place_{side}_{shape}"));
            let b = &c["request"]["body"];
            assert_eq!(b["clientOid"], f["identity"]["clientOid"]);
            assert_eq!(b["side"], side);
            assert_eq!(b["symbol"], "BTC-USDT");
            assert_eq!(b.get("price").is_some(), shape == "limit");
            assert_eq!(b.get("funds").is_some(), shape == "market_funds");
            assert_eq!(b.get("size").is_some(), shape != "market_funds");
            assert_eq!(c["expect"]["transaction_rows_delta"], 1);
            assert_eq!(c["expect"]["placement_posts"], 1);
        }
    }
}

#[test]
fn kucoin_hf_status_cancel_and_identity_contract_is_conservative() {
    let f = fixture("orders");
    for id in ["status_active_before_book", "status_active_in_book"] {
        assert_eq!(case(&f, id)["expect"]["status"], "Open");
        assert_eq!(case(&f, id)["response"]["body"]["data"]["active"], true);
    }
    for id in ["status_missing_active", "status_string_active", "status_inactive_in_book", "status_missing_execution", "status_conflicting_execution"] {
        assert_eq!(case(&f, id)["expect"]["status"], "Unresolved");
        assert_eq!(case(&f, id)["expect"]["reserve_credit"], "0");
    }
    let partial = &case(&f, "status_cancelled_partial")["expect"];
    assert_eq!(partial["status"], "Cancelled");
    assert_eq!(partial["gross_base"], "0.000004");
    assert_eq!(partial["gross_quote"], "0.2708704");
    assert_eq!(case(&f, "status_price_round_half_up")["expect"]["price"], "67717.7");
    for (id, status) in [("status_zero_59s", "Unknown"), ("status_zero_60s", "Unknown"), ("status_zero_after_60s", "Closed"), ("status_zero_limit", "Unknown")] {
        assert_eq!(case(&f, id)["expect"]["status"], status);
    }
    for c in f["cases"].as_array().unwrap().iter().filter(|c| c["id"].as_str().unwrap().starts_with("cancel_")) {
        assert_eq!(c["request"]["query"], serde_json::json!([["symbol", "BTC-USDT"]]));
        assert_eq!(c["expect"]["reserve_credit"], "0");
        assert_eq!(c["expect"]["terminal_proven"], false);
    }
    for id in ["lookup_wrong_id", "lookup_wrong_client_oid", "lookup_wrong_symbol", "lookup_wrong_side", "lookup_wrong_type", "lookup_missing"] {
        assert_eq!(case(&f, id)["expect"]["intent"], "retained");
        assert_eq!(case(&f, id)["expect"]["placement_posts"], 0);
    }
}

#[test]
fn kucoin_hf_placement_classification_pins_database_obligations() {
    let mut f = fixture("errors");
    // P1: a server error never refuses, whatever code it carries.
    let mut served = f["cases"].as_array().unwrap().iter().find(|c| c["id"] == "reject_200004").unwrap().clone();
    served["id"] = json!("reject_200004_at_503"); served["response"]["status"] = json!(503);
    served["expect"]["classification"] = json!("Ambiguous"); served["expect"]["intent"] = json!("retained");
    f["cases"].as_array_mut().unwrap().push(served);
    for c in f["cases"].as_array().unwrap() {
        let e = &c["expect"];
        assert_eq!(e["transaction_rows_delta"], 0);
        assert_eq!(e["reserve_credit"], "0");
        match e["classification"].as_str().unwrap() {
            "Ambiguous" => { assert_eq!(e["intent"], "retained"); assert_eq!(e["replacement_posts"], 0); }
            "Rejected" | "NotSent" => assert_eq!(e["intent"], "cleared"),
            x => panic!("unexpected classification {x}"),
        }
    }
    for id in ["duplicate_client_oid", "unknown_code", "missing_code", "success_missing_id", "success_empty_id", "success_conflicting_client_oid", "truncated_success", "lost_reply", "generic_503", "internal_200", "internal_500", "busy_200", "busy_503"] {
        assert_eq!(case(&f, id)["expect"]["classification"], "Ambiguous", "{id}");
    }
    for code in ["400002", "400003", "400004", "400005", "400006", "400007", "400100", "200004", "429000"] {
        assert_eq!(case(&f, &format!("reject_{code}"))["expect"]["classification"], "Rejected");
    }
}

#[test]
fn kucoin_hf_paging_fixture_requires_returned_integer_cursor_and_empty_terminator() {
    let f = fixture("fills");
    let c = case(&f, "pages_boundary_and_large_ids");
    let pages = c["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 3);
    assert_eq!(pages[0]["response"]["body"]["data"]["items"].as_array().unwrap().len(), 100);
    assert_eq!(pages[1]["response"]["body"]["data"]["items"].as_array().unwrap().len(), 3);
    assert!(pages[2]["response"]["body"]["data"]["items"].as_array().unwrap().is_empty());
    let mut identities = BTreeSet::new();
    for (i, page) in pages.iter().enumerate() {
        let q = page["request"]["query"].as_array().unwrap();
        assert!(!q.iter().any(|p| p[0] == "orderId"));
        if i > 0 {
            let cursor = pages[i - 1]["response"]["body"]["data"]["lastId"].as_u64().unwrap();
            assert!(cursor > (1 << 53));
            assert!(q.contains(&serde_json::json!(["lastId", cursor.to_string()])));
        }
        for item in page["response"]["body"]["data"]["items"].as_array().unwrap() {
            assert_ne!(item["id"], item["tradeId"]);
            identities.insert(format!("{}-{}", item["symbol"].as_str().unwrap(), item["tradeId"].as_u64().unwrap()));
        }
    }
    assert_eq!(identities.len(), 102, "100 + 3 - one repeated boundary fill");
    assert_eq!(c["expect"]["unique_fills"], 102);
    for c in f["cases"].as_array().unwrap().iter().filter(|c| c["expect"]["coverage"] == "incomplete") {
        assert_eq!(c["expect"]["watermark"], "unchanged");
        assert_eq!(c["expect"]["visible_api_rows_delta"], 0);
        assert_eq!(c["expect"]["csv_rows_deleted"], 0);
    }
}

#[test]
fn kucoin_hf_fill_amounts_fees_and_symbol_identity_are_exact() {
    use rust_decimal::Decimal;
    let f = fixture("fills");
    let c = case(&f, "pages_boundary_and_large_ids");
    let mut seen = BTreeSet::new();
    let mut sides = BTreeMap::new();
    let mut totals = [Decimal::ZERO; 3];
    for page in c["pages"].as_array().unwrap() {
        for item in page["response"]["body"]["data"]["items"].as_array().unwrap() {
            let id = item["tradeId"].as_u64().unwrap();
            if !seen.insert(id) { continue; }
            let order = item["orderId"].as_str().unwrap();
            let side = item["side"].as_str().unwrap();
            assert_eq!(*sides.entry(order).or_insert(side), side, "one order cannot change side");
            let decimal = |field: &str| Decimal::from_str(item[field].as_str().unwrap()).unwrap();
            assert_eq!(decimal("price") * decimal("size"), decimal("funds"));
            assert_eq!(item["feeCurrency"], "USDT");
            for (i, field) in ["size", "funds", "fee"].iter().enumerate() { totals[i] += decimal(field); }
        }
    }
    assert_eq!(sides.len(), 2, "separate buy and sell orders, each represented by partial fills");
    for (i, want) in ["0.00102", "69.071952", "0.069071952"].iter().enumerate() {
        assert_eq!(totals[i], Decimal::from_str(want).unwrap());
        assert_eq!(c["expect"]["gross_totals"][i], *want);
    }
    let twins = case(&f, "symbol_scoped_trade_identity");
    assert_eq!(twins["items"][0]["tradeId"], twins["items"][1]["tradeId"]);
    assert_ne!(twins["expect"]["ledger_ids"][0], twins["expect"]["ledger_ids"][1]);
    for (i, item) in twins["items"].as_array().unwrap().iter().enumerate() {
        assert_eq!(twins["expect"]["ledger_ids"][i], format!("{}-{}", item["symbol"].as_str().unwrap(), item["tradeId"].as_u64().unwrap()));
    }
}

#[test]
fn kucoin_hf_independent_hmac_vectors_include_delete_query_and_exact_body() {
    let f = fixture("signing");
    let secret = f["credentials"]["secret"].as_str().unwrap();
    let passphrase = f["credentials"]["passphrase"].as_str().unwrap();
    let mut methods = BTreeSet::new();
    for c in f["cases"].as_array().unwrap() {
        let r = &c["request"];
        let method = r["method"].as_str().unwrap();
        methods.insert(method);
        let query = r["query"].as_array().unwrap();
        let raw_query = query.iter().map(|p| format!("{}={}", p[0].as_str().unwrap(), p[1].as_str().unwrap())).collect::<Vec<_>>().join("&");
        let path = r["path"].as_str().unwrap();
        let endpoint = if query.is_empty() { path.to_owned() } else { format!("{path}?{raw_query}") };
        let body = r["body_bytes"].as_str().unwrap();
        let input = format!("{}{method}{endpoint}{body}", f["timestamp"].as_str().unwrap());
        assert_eq!(input, c["expect"]["prehash"]);
        assert_eq!(hmac(secret, &input), c["expect"]["headers"]["KC-API-SIGN"]);
        assert_eq!(hmac(secret, passphrase), c["expect"]["headers"]["KC-API-PASSPHRASE"]);
        let mut url = reqwest::Url::parse(&format!("https://api.kucoin.com{path}")).unwrap();
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query.iter().map(|p| (p[0].as_str().unwrap(), p[1].as_str().unwrap())));
        }
        assert_eq!(url.as_str(), c["expect"]["transport_url"].as_str().unwrap());
        if method == "DELETE" {
            assert!(raw_query.contains("symbol=BTC-USDT"));
            assert_ne!(hmac(secret, &format!("{}DELETE{path}", f["timestamp"].as_str().unwrap())), c["expect"]["headers"]["KC-API-SIGN"]);
        }
        if c["id"] == "sign_get_unescaped_query" {
            assert_ne!(format!("https://api.kucoin.com{endpoint}"), url.as_str());
        }
    }
    assert_eq!(methods, BTreeSet::from(["DELETE", "GET", "POST"]));
}

/// An account-mode refusal is Rust's own text: it is the key check's log line (and the validator job's stored reason),
/// while a venue's diagnostic keeps omitting its body.
#[test]
fn an_account_mode_refusal_is_the_key_checks_log_line() {
    use deltabadger::web::settings::validator::Diagnostic;
    let refusal = Diagnostic { text: deltabadger::venue::kucoin_account::UTA.into(), status: None, body: String::new(), refusal: true };
    assert_eq!(refusal.log_text(), deltabadger::venue::kucoin_account::UTA);
    let venue = Diagnostic { text: "venue text".into(), status: Some(500), body: r#"{"code":7}"#.into(), refusal: false };
    assert!(!venue.log_text().contains("venue text"));
}

/// O03: every HF signing vector through the real transport. The sent URL (form-encoded, in the given order), the exact
/// body bytes and every signed header match the independent HMAC expectations; classic signing is untouched (tests/kucoin.rs).
#[tokio::test(flavor = "current_thread")]
async fn o03_the_transport_signs_every_hf_vector() {
    use deltabadger::venue::http::{self as wire, HttpRequest, Transport};
    use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};
    let fixture: Value = serde_json::from_str(include_str!("fixtures/kucoin_hf/signing.json")).unwrap();
    let k = &fixture["credentials"];
    let at = chrono::DateTime::from_timestamp_millis(fixture["timestamp"].as_str().unwrap().parse().unwrap()).unwrap();
    for c in fixture["cases"].as_array().unwrap() {
        let server = MockServer::start().await;
        Mock::given(any()).respond_with(ResponseTemplate::new(200).set_body_string(r#"{"code":"200000","data":{}}"#)).mount(&server).await;
        let t = wire::ReqwestTransport::kucoin(wire::client(), k["key"].as_str().unwrap().into(), k["secret"].as_str().unwrap().into(), k["passphrase"].as_str().unwrap().into())
            .with_clock(std::sync::Arc::new(deltabadger::engine::FixedClock(at)));
        let q = &c["request"];
        let method: &'static str = match q["method"].as_str().unwrap() { "GET" => "GET", "POST" => "POST", "DELETE" => "DELETE", m => panic!("{m}") };
        let query = q["query"].as_array().unwrap().iter()
            .map(|p| (&*Box::leak(p[0].as_str().unwrap().to_string().into_boxed_str()), p[1].as_str().unwrap().to_string())).collect();
        let body = q["body_bytes"].as_str().filter(|b| !b.is_empty()).map(|b| serde_json::from_str(b).unwrap());
        t.send(&HttpRequest { method, base: server.uri(), path: q["path"].as_str().unwrap().into(), query, body, not_after: None }).await.unwrap();
        let got = server.received_requests().await.unwrap();
        let (got, id) = (&got[0], c["id"].as_str().unwrap());
        let want_url = reqwest::Url::parse(c["expect"]["transport_url"].as_str().unwrap()).unwrap();
        assert_eq!((got.url.path(), got.url.query()), (want_url.path(), want_url.query()), "{id}: the URL sent");
        assert_eq!(String::from_utf8(got.body.clone()).unwrap(), q["body_bytes"].as_str().unwrap(), "{id}: the body sent");
        for (name, value) in c["expect"]["headers"].as_object().unwrap() {
            assert_eq!(got.headers.get(name.as_str()).map(|h| h.to_str().unwrap()), value.as_str(), "{id}: {name}");
        }
    }
}

/// O04–O06, r1.2, r3.2: every HF status answer, read against its own order record, then settled at the case's clock.
/// Unresolved is unreadable: the row waits, nothing is settled.
#[test]
fn o04_o06_hf_status_answers_read_strictly() {
    use deltabadger::venue::{binance::{Pair, Pairs}, kucoin::{parse_hf_order, HfOrder}, OrderStatus};
    let mut pairs = Pairs::default();
    pairs.by_symbol.insert("BTC-USDT".into(), Pair { base: "BTC".into(), quote: "USDT".into(), price_decimals: 1 });
    let fixture: Value = serde_json::from_str(include_str!("fixtures/kucoin_hf/orders.json")).unwrap();
    let record = |d: &Value| HfOrder { id: d["id"].as_str().unwrap().into(), symbol: d["symbol"].as_str().unwrap().into(),
                                       sell: d["side"] == "sell", limit: d["type"] == "limit" };
    let mut seen = 0;
    for c in fixture["cases"].as_array().unwrap().iter().filter(|c| c["id"].as_str().unwrap().starts_with("status_")) {
        let (id, data, want) = (c["id"].as_str().unwrap(), &c["response"]["body"]["data"], &c["expect"]);
        let now = chrono::DateTime::from_timestamp_millis(c["given"]["now_ms"].as_i64().unwrap()).unwrap();
        let read = parse_hf_order(data, &record(data), &pairs).map(|s| s.settled(now));
        match want["status"].as_str().unwrap() {
            "Unresolved" => assert!(read.is_err(), "{id}: {read:?}"),
            status => {
                let s = read.unwrap_or_else(|e| panic!("{id}: {e:?}"));
                let expected = match status { "Open" => OrderStatus::Open, "Closed" => OrderStatus::Closed, "Cancelled" => OrderStatus::Cancelled, "Unknown" => OrderStatus::Unknown, o => panic!("{o}") };
                assert_eq!(s.status, expected, "{id}");
                assert_eq!(s.amount_exec.to_s_f(), deltabadger::ruby::BigDec::parse(want["gross_base"].as_str().unwrap()).unwrap().to_s_f(), "{id}: base");
                assert_eq!(s.quote_amount_exec.unwrap().to_s_f(), deltabadger::ruby::BigDec::parse(want["gross_quote"].as_str().unwrap()).unwrap().to_s_f(), "{id}: quote");
                if let Some(p) = want["price"].as_str() { assert_eq!(s.price.unwrap().to_s_f(), p, "{id}: price"); }
            }
        }
        seen += 1;
    }
    assert_eq!(seen, 14);
    // r1.2: totals that disagree, or are negative, and an inactive order still in the book, are never a state.
    let base = fixture["cases"].as_array().unwrap().iter().find(|c| c["id"] == "status_filled").unwrap()["response"]["body"]["data"].clone();
    for (key, value) in [("dealFunds", json!("0")), ("dealSize", json!("0")), ("dealSize", json!("-0.00001")), ("dealFunds", json!("-1")), ("inOrderBook", json!(true)),
                         ("active", json!("false")), ("inOrderBook", Value::Null), ("cancelExist", json!(0))] {
        let mut d = base.clone(); d[key] = value.clone();
        assert!(parse_hf_order(&d, &record(&base), &pairs).is_err(), "{key}={value}");
    }
    let mut cancelled = base.clone(); cancelled["cancelExist"] = json!(true); cancelled["inOrderBook"] = json!(true);
    assert!(parse_hf_order(&cancelled, &record(&base), &pairs).is_err(), "cancelled yet in the book");
    // r3.2: an answer for another id, symbol, side or type is not this order, whatever its state.
    for (key, value) in [("id", json!("other")), ("symbol", json!("ETH-USDT")), ("side", json!("sell")), ("type", json!("market"))] {
        let mut d = base.clone(); d[key] = value.clone();
        assert!(parse_hf_order(&d, &record(&base), &pairs).is_err(), "{key}={value}");
    }
}

/// An HF-admitted venue (D-KUCOIN-HF-ORDERS, `allow_hf` until HF-5) prepared on a detected ClassicHf account.
async fn hf_venue(t: &ScriptedTransport, version: CredentialVersion) -> KucoinVenue<ScriptedTransport> {
    classic(t, true);
    let v = KucoinVenue::new(t.clone(), URL).with_account_context(version, Resolver::default()).allow_hf();
    v.prepare().await.unwrap();
    v
}
fn submitted(c: &rusqlite::Connection) -> i64 { c.query_row("SELECT count(*) FROM transactions WHERE status = 0", [], |r| r.get(0)).unwrap() }
fn posts(t: &ScriptedTransport) -> usize { t.requests().iter().filter(|r| r.method == "POST").count() }

/// O01, r1.8: without `allow_hf` a ClassicHf account is refused before any intent; with it the intent records the HF
/// family, the POST goes to the HF route with the intent's UUID, and the accepted order is one row.
#[tokio::test(flavor = "current_thread")]
async fn o01_an_hf_order_dispatches_only_when_admitted_and_is_recorded_once() {
    use deltabadger::engine::{placement, FixedClock};
    let at: chrono::DateTime<chrono::Utc> = "2026-10-30T12:00:00Z".parse().unwrap();
    let clock = FixedClock(at);
    // Not admitted: the production gate.
    let (_dir, o, bot, _plan) = engine_setup();
    let t = ScriptedTransport::default(); t.set_now(at); classic(&t, true);
    let v = KucoinVenue::new(t.clone(), URL).with_account_context(model::credential_version(&o.primary, &bot).unwrap().unwrap(), Resolver::default());
    assert!(v.prepare().await.is_err(), "HF stays unconnected without allow_hf");
    // Admitted.
    let (dir, o, bot, plan) = engine_setup();
    let t = ScriptedTransport::default(); t.set_now(at);
    let v = hf_venue(&t, model::credential_version(&o.primary, &bot).unwrap().unwrap()).await;
    let placement::Begun::Intent(intent) = placement::begin_held(&o.primary, &bot, &plan, v.placement_identity(), None, &clock).unwrap() else { panic!() };
    assert_eq!(model::load_bot(&o.primary, bot.id).unwrap().rust_placement().unwrap()["kucoin_account_mode"], "ClassicHf");
    t.reply("POST /api/v1/hf/orders", 200, json!({"code":"200000","data":{"orderId":"670fd33bf9406e0007ab3945","clientOid":intent.cl_ord_id}}));
    let sent = placement::send_recorded(&o.primary, &v, &bot, &intent, &clock).await.unwrap();
    assert!(matches!(sent, placement::Sent::Accepted(ref id) if id == "670fd33bf9406e0007ab3945"), "{sent:?}");
    let post = t.requests().into_iter().find(|r| r.method == "POST").unwrap();
    assert_eq!((post.path.as_str(), &post.body.as_ref().unwrap()["clientOid"]), ("/api/v1/hf/orders", &json!(intent.cl_ord_id)));
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(submitted(&c), 1);
    assert_eq!(c.query_row("SELECT external_id FROM transactions", [], |r| r.get::<_, String>(0)).unwrap(), "670fd33bf9406e0007ab3945", "the raw orderId");
    assert!(model::load_bot(&c, bot.id).unwrap().rust_placement().is_none(), "the intent became the row");
    // An intent sized on a classic account is never sent on the HF one (and the reverse).
    let (_dir, o, bot, plan) = engine_setup();
    let t = ScriptedTransport::default(); t.set_now(at); classic(&t, false);
    let lf = KucoinVenue::new(t.clone(), URL).with_account_context(model::credential_version(&o.primary, &bot).unwrap().unwrap(), Resolver::default()).allow_hf();
    lf.prepare().await.unwrap();
    let placement::Begun::Intent(intent) = placement::begin_held(&o.primary, &bot, &plan, lf.placement_identity(), None, &clock).unwrap() else { panic!() };
    let t2 = ScriptedTransport::default(); t2.set_now(at);
    let hf = hf_venue(&t2, model::credential_version(&o.primary, &bot).unwrap().unwrap()).await;
    assert!(matches!(placement::send_recorded(&o.primary, &hf, &bot, &intent, &clock).await.unwrap(), placement::Sent::NotSent(_)));
    assert_eq!(posts(&t) + posts(&t2), 0);
}

/// O02, r1.1, r2.3: every HF placement answer in errors.json, through the engine. A refusal clears the intent with no
/// submitted row (the failed row every venue writes); NotSent writes nothing; Ambiguous keeps the intent, and a later
/// tick, after reopening the store, sends no second POST.
#[tokio::test(flavor = "current_thread")]
async fn o02_every_hf_placement_answer_has_its_database_effect() {
    use deltabadger::engine::{placement, tick, FixedClock};
    let at: chrono::DateTime<chrono::Utc> = "2026-10-30T12:00:00Z".parse().unwrap();
    let clock = FixedClock(at);
    let mut f = fixture("errors");
    // P1: a server error never refuses, whatever code it carries.
    let mut served = f["cases"].as_array().unwrap().iter().find(|c| c["id"] == "reject_200004").unwrap().clone();
    served["id"] = json!("reject_200004_at_503"); served["response"]["status"] = json!(503);
    served["expect"]["classification"] = json!("Ambiguous"); served["expect"]["intent"] = json!("retained");
    f["cases"].as_array_mut().unwrap().push(served);
    for c in f["cases"].as_array().unwrap() {
        let (id, want) = (c["id"].as_str().unwrap(), &c["expect"]);
        let (dir, o, bot, plan) = engine_setup();
        let t = ScriptedTransport::default(); t.set_now(at);
        let v = hf_venue(&t, model::credential_version(&o.primary, &bot).unwrap().unwrap()).await;
        let placement::Begun::Intent(intent) = placement::begin_held(&o.primary, &bot, &plan, v.placement_identity(), None, &clock).unwrap() else { panic!() };
        let r = &c["response"];
        match r["transport_error"].as_str() {
            Some("NotSent") => { t.network("POST /api/v1/hf/orders", "pre_send", "refused"); }
            Some(_) => { t.network("POST /api/v1/hf/orders", "post_send", "reset"); }
            None if r["raw_body"].is_string() => { t.reply("POST /api/v1/hf/orders", r["status"].as_u64().unwrap() as u16, r["raw_body"].clone()); }
            None => { t.reply("POST /api/v1/hf/orders", r["status"].as_u64().unwrap() as u16, r["body"].clone()); }
        }
        let sent = placement::send_recorded(&o.primary, &v, &bot, &intent, &clock).await.unwrap();
        let class = match sent { placement::Sent::Rejected(_) => "Rejected", placement::Sent::NotSent(_) => "NotSent", placement::Sent::Ambiguous(_) => "Ambiguous", ref s => panic!("{id}: {s:?}") };
        assert_eq!(class, want["classification"], "{id}");
        let c2 = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        assert_eq!(submitted(&c2), 0, "{id}: no order row");
        if want["reschedule"] == true {
            assert_eq!(c2.query_row("SELECT count(*) FROM transactions", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "{id}: a safe rejection writes no row at all");
        }
        assert_eq!(model::load_bot(&c2, bot.id).unwrap().rust_placement().is_some(), want["intent"] == "retained", "{id}: the intent");
        if want["intent"] == "retained" {
            // Recovery asks the HF clientOid route; a miss never proves the order absent.
            t.reply(&format!("GET /api/v1/hf/orders/client-order/{}?symbol=BTC-EUR", intent.cl_ord_id), 200, json!({"code":"400100","msg":"order not exist"}));
            tick::tick(&c2, &v, bot.id, &FixedClock(at + chrono::Duration::seconds(30)), &mut Default::default()).await.unwrap();
            assert_eq!(posts(&t), 1, "{id}: never a replacement POST");
            assert!(model::load_bot(&c2, bot.id).unwrap().rust_placement().is_some(), "{id}: still held");
        }
    }
}

fn creds(key: &str) -> deltabadger::crypto::Credentials {
    deltabadger::crypto::Credentials { ibkr: None, key: key.into(), secret: "s".into(), passphrase: Some("p".into()), redaction_values: vec![] }
}
/// An admitted HF venue sending with key `key`, after one accepted HF order for the seeded bot: (dir, store, bot, transport,
/// venue, transaction id, external id).
async fn placed(key: &str, limit_plan: bool) -> (tempfile::TempDir, deltabadger::store::Opened, model::Bot, ScriptedTransport, KucoinVenue<ScriptedTransport>, i64, String) {
    use deltabadger::engine::{placement, FixedClock};
    let at: chrono::DateTime<chrono::Utc> = "2026-10-30T12:00:00Z".parse().unwrap();
    let (dir, o, bot, mut plan) = engine_setup();
    plan.limit = limit_plan;
    let t = ScriptedTransport::default(); t.set_now(at); classic(&t, true);
    let v = KucoinVenue::new(t.clone(), URL).with_identity(&creds(key)).with_account_context(model::credential_version(&o.primary, &bot).unwrap().unwrap(), Resolver::default()).allow_hf();
    v.prepare().await.unwrap();
    let placement::Begun::Intent(intent) = placement::begin_held(&o.primary, &bot, &plan, v.placement_identity(), None, &FixedClock(at)).unwrap() else { panic!() };
    t.reply("POST /api/v1/hf/orders", 200, json!({"code":"200000","data":{"orderId":"HF1","clientOid":intent.cl_ord_id}}));
    assert!(matches!(placement::send_recorded(&o.primary, &v, &bot, &intent, &FixedClock(at)).await.unwrap(), placement::Sent::Accepted(_)));
    let id = o.primary.query_row("SELECT id FROM transactions WHERE external_id='HF1'", [], |r| r.get(0)).unwrap();
    (dir, o, bot, t, v, id, "HF1".into())
}
fn hf_answer(id: &str, active: bool, cancelled: bool, size: &str, funds: &str) -> Value {
    json!({"code":"200000","data":{"id":id,"symbol":"BTC-EUR","type":"market","side":"buy","price":"0","size":"0.001","funds":"60",
        "dealSize":size,"dealFunds":funds,"active":active,"inOrderBook":false,"cancelExist":cancelled,"createdAt":1793361600000_i64}})
}
fn row_exec(c: &rusqlite::Connection, id: i64) -> (Option<i64>, Option<f64>) {
    c.query_row("SELECT external_status, amount_exec FROM transactions WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
}

/// O08, r1.3, r1.5, r2.1: the record is written with the row; polls read an HF row by its stored symbol on the HF route
/// even after the bot's pair changes; a classic row beside it keeps the classic route and its fills fallback; an HF row
/// the venue cannot read is never sent to the fills fallback nor abandoned, however old; another key never reads it.
#[tokio::test(flavor = "current_thread")]
async fn o08_hf_rows_are_read_by_their_record_never_the_bots_current_pair() {
    use deltabadger::engine::polling;
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    let record = deltabadger::venue::kucoin::order_record(&o.primary, id).unwrap().unwrap();
    assert_eq!((&record["family"], &record["symbol"], &record["sell"], &record["limit"], &record["host"]), (&json!("ClassicHf"), &json!("BTC-EUR"), &json!(false), &json!(false), &json!(URL)));
    assert!(record["key_fingerprint"].is_string());
    o.primary.execute_batch("UPDATE tickers SET ticker='ETH-EUR'").unwrap();
    // A classic row beside it, and both rows old enough to be abandoned if missing.
    common::seed::insert_row(&o.primary, &common::seed::Seeded { user_id: 1, exchange_id: bot.exchange_id, btc: 1, quote: 2, ticker_id: 1, api_key_id: 1 }, bot.id, 1,
                             &json!({"external_id": "CLASSIC1", "created_at": "2026-09-01 10:00:00"}));
    o.primary.execute("UPDATE transactions SET created_at='2026-09-01 10:00:00'", []).unwrap();
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, json!({"code":"400100","msg":"order not exist"}));
    t.reply("GET /api/v1/orders/CLASSIC1", 200, json!({"code":"400100","msg":"order not exist"}));
    let now = "2026-10-30T12:05:00Z".parse().unwrap();
    let _ = polling::sweep(&o.primary, &v, &model::load_bot(&o.primary, bot.id).unwrap(), now).await;
    let asked: Vec<String> = t.requests().iter().map(|r| format!("{} {}{}", r.method, r.path, r.query.iter().map(|(k, v)| format!("?{k}={v}")).collect::<String>())).collect();
    assert!(asked.contains(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR")), "{asked:?}");
    assert!(!asked.iter().any(|a| a.contains("ETH-EUR")), "never the bot's current pair: {asked:?}");
    assert!(!asked.iter().any(|a| a.starts_with("GET /api/v1/orders/HF1")), "never the classic route for an HF row");
    assert!(!asked.iter().any(|a| a.contains("/fills") && a.contains("HF1")), "no fills fallback for an HF row: {asked:?}");
    assert_eq!(row_exec(&o.primary, id).0, Some(0), "the HF row still waits, never abandoned");
    // Another key's venue does not read the HF row at all.
    let t2 = ScriptedTransport::default(); t2.set_now(now); classic(&t2, true);
    let other = KucoinVenue::new(t2.clone(), URL).with_identity(&creds("KEY-B")).with_account_context(model::credential_version(&o.primary, &bot).unwrap().unwrap(), Resolver::default()).allow_hf();
    other.prepare().await.unwrap();
    t2.reply("GET /api/v1/orders/CLASSIC1", 200, json!({"code":"400100","msg":"order not exist"}));
    let _ = polling::sweep(&o.primary, &other, &model::load_bot(&o.primary, bot.id).unwrap(), now).await;
    assert!(!t2.requests().iter().any(|r| r.path.contains("HF1")), "another key never asks about this order");
}

/// r3.1, r4: an HF row's dealt totals never go down, compared as exact decimals (the row's columns are f64).
#[tokio::test(flavor = "current_thread")]
async fn r3_hf_dealt_totals_never_go_down_and_compare_exactly() {
    use deltabadger::engine::polling;
    let now: chrono::DateTime<chrono::Utc> = "2026-10-30T12:05:00Z".parse().unwrap();
    let (dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    let key = format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR");
    macro_rules! sweep { ($c:expr) => { polling::sweep($c, &v, &model::load_bot($c, bot.id).unwrap(), now) } }
    // One answer per sweep, in order (a scripted key repeats its last answer).
    for (active, cancelled, size, funds) in [(true, false, "0.0004", "0.99999999999999999"), (false, true, "0", "0"),
                                             (false, true, "0.0004", "0.99999999999999998"), (false, true, "0.0004", "0.99999999999999999")] {
        t.reply(&key, 200, hf_answer(&ext, active, cancelled, size, funds));
    }
    let _ = sweep!(&o.primary).await;
    assert_eq!(row_exec(&o.primary, id).0, Some(1), "open with a partial fill");
    // Lower totals, also after reopening the store: nothing applied, the row stays open.
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let _ = sweep!(&c).await;
    assert_eq!(row_exec(&c, id), (Some(1), Some(0.0004)));
    let _ = sweep!(&c).await;
    assert_eq!(row_exec(&c, id).0, Some(1), "one unit lower at the 17th decimal is refused");
    // The identical exact total (1.0 in the f64 column) settles.
    let _ = sweep!(&c).await;
    assert_eq!(row_exec(&c, id).0, Some(3), "cancelled with what it dealt");
}

/// O07: every HF cancel answer, through the venue: only 200000 naming this order is accepted (a request, not a state);
/// a 4xx with a code is the venue's refusal; everything else cannot be told.
#[tokio::test(flavor = "current_thread")]
async fn o07_every_hf_cancel_answer_is_classified() {
    use deltabadger::venue::{OrderRef, VenueError};
    let f = fixture("orders");
    for c in f["cases"].as_array().unwrap().iter().filter(|c| c["id"].as_str().unwrap().starts_with("cancel_")) {
        let (_dir, o, _bot, t, v, id, ext) = placed("KEY-A", false).await;
        let key = format!("DELETE /api/v1/hf/orders/{ext}?symbol=BTC-EUR");
        let r = &c["response"];
        let mut body = r["body"].clone();
        if body["data"]["orderId"] == "670fd33bf9406e0007ab3945" { body["data"]["orderId"] = json!(ext); }
        match r["transport_error"].as_str() {
            Some(_) => { t.network(&key, "post_send", "reset"); }
            None if r["raw_body"].is_string() => { t.reply(&key, r["status"].as_u64().unwrap() as u16, r["raw_body"].clone()); }
            None => { t.reply(&key, r["status"].as_u64().unwrap() as u16, body); }
        }
        let got = v.cancel_with(&OrderRef::load(&o.primary, id, &ext).unwrap()).await;
        let class = match got { Ok(()) => "acknowledged", Err(VenueError::Rejected(_)) => "rejected", Err(_) => "uncertain" };
        assert_eq!(class, c["expect"]["cancellation"], "{}", c["id"]);
    }
}

/// r1.7: an HF cancel that cannot be told keeps the reversal's cancel entry (the placement barrier) and its reserve;
/// a later pass that reads the order cancelled settles it once.
#[tokio::test(flavor = "current_thread")]
async fn r1_7_an_uncertain_hf_cancel_keeps_its_obligation() {
    use deltabadger::engine::reversal;
    let now: chrono::DateTime<chrono::Utc> = "2026-10-30T12:05:00Z".parse().unwrap();
    let (dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    o.primary.execute(&format!("UPDATE bots SET transient_data=json_set(transient_data,'$.{}',json(?1),'$.{}',json(?2)) WHERE id=?3", reversal::CANCELS, reversal::RESERVE_PENDING),
                      rusqlite::params![json!([{"id": id, "registered": true}]).to_string(), json!([id]).to_string(), bot.id]).unwrap();
    let pending = |c: &rusqlite::Connection| { let b = model::load_bot(c, bot.id).unwrap(); (b.transient[reversal::CANCELS].as_array().map_or(0, Vec::len), b.transient[reversal::RESERVE_PENDING].as_array().map_or(0, Vec::len)) };
    t.network(&format!("DELETE /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), "post_send", "reset");
    let handle = deltabadger::venue::Handle::attributed(&v);
    assert!(reversal::cancel_captured(&o.primary, &handle, bot.id, now).await.unwrap().is_empty());
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(pending(&c), (1, 1), "the cancel entry and the reserve stay");
    // The next pass: the cancel cannot be told again, but the order reads cancelled.
    t.reply(&format!("DELETE /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 503, json!("unavailable"));
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, hf_answer(&ext, false, true, "0", "0"));
    assert_eq!(reversal::cancel_captured(&c, &handle, bot.id, now).await.unwrap(), vec![id]);
    assert_eq!(pending(&c).0, 0, "the entry is settled once");
}

/// r1.4: recovery of an HF intent takes an answer only for an order of the intent's side and type; a market answer for a
/// limit intent leaves it unresolved, and the matching answer recovers one row with no second POST.
#[tokio::test(flavor = "current_thread")]
async fn r1_4_hf_recovery_checks_the_order_type() {
    use deltabadger::engine::{placement, tick, FixedClock};
    let at: chrono::DateTime<chrono::Utc> = "2026-10-30T12:00:00Z".parse().unwrap();
    let (dir, o, bot, mut plan) = engine_setup();
    plan.limit = true;
    let t = ScriptedTransport::default(); t.set_now(at);
    let v = hf_venue(&t, model::credential_version(&o.primary, &bot).unwrap().unwrap()).await;
    let placement::Begun::Intent(intent) = placement::begin_held(&o.primary, &bot, &plan, v.placement_identity(), None, &FixedClock(at)).unwrap() else { panic!() };
    t.reply("POST /api/v1/hf/orders", 503, json!({"code":"230005","msg":"The system is busy, please try again later"}));
    assert!(matches!(placement::send_recorded(&o.primary, &v, &bot, &intent, &FixedClock(at)).await.unwrap(), placement::Sent::Ambiguous(_)));
    let key = format!("GET /api/v1/hf/orders/client-order/{}?symbol=BTC-EUR", intent.cl_ord_id);
    let answer = |kind: &str| json!({"code":"200000","data":{"id":"HF9","clientOid":intent.cl_ord_id,"symbol":"BTC-EUR","type":kind,"side":"buy","price":"50000","size":"0.001","funds":"50",
        "dealSize":"0","dealFunds":"0","active":true,"inOrderBook":true,"cancelExist":false,"createdAt":1793361600000_i64}});
    t.reply(&key, 200, answer("market"));
    t.reply(&key, 200, answer("limit"));
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    tick::tick(&c, &v, bot.id, &FixedClock(at + chrono::Duration::seconds(30)), &mut Default::default()).await.unwrap();
    assert!(model::load_bot(&c, bot.id).unwrap().rust_placement().is_some(), "another type is not this order");
    assert_eq!(submitted(&c), 0);
    tick::tick(&c, &v, bot.id, &FixedClock(at + chrono::Duration::seconds(60)), &mut Default::default()).await.unwrap();
    assert_eq!(c.query_row("SELECT count(*) FROM transactions WHERE status=0 AND external_id='HF9'", [], |r| r.get::<_, i64>(0)).unwrap(), 1, "recovered once");
    assert_eq!(posts(&t), 1, "no second POST");
    assert!(deltabadger::venue::kucoin::order_record(&c, c.query_row("SELECT id FROM transactions WHERE external_id='HF9'", [], |r| r.get(0)).unwrap()).unwrap().is_some(), "the recovered row has its record");
}

/// One HF order's read keeps a retryable failure (the follow-up is retried); a bulk read leaves that row excluded.
#[tokio::test(flavor = "current_thread")]
async fn a_single_hf_read_keeps_a_retryable_failure() {
    use deltabadger::venue::OrderRef;
    let (_dir, o, _bot, t, v, id, ext) = placed("KEY-A", false).await;
    let order = OrderRef::load(&o.primary, id, &ext).unwrap();
    t.network(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), "pre_send", "timeout");
    assert!(matches!(v.order_with(&order, |_, _, _| Ok(true)).await, Err(deltabadger::venue::VenueError::Transient(_))));
    assert_eq!(v.orders_with(std::slice::from_ref(&order), |_, _, _| Ok(true)).await.unwrap(), (vec![], vec![ext.clone()]));
}
