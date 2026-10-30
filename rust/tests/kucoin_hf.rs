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
    // HF-4: an uncertain cancel reads the order at once; first it cannot be told, then it reads cancelled.
    not_found(&t, &ext);
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, hf_answer(&ext, false, true, "0", "0"));
    let handle = deltabadger::venue::Handle::attributed(&v);
    assert!(reversal::cancel_captured(&o.primary, &handle, bot.id, now).await.unwrap().is_empty());
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(pending(&c), (1, 1), "the cancel entry and the reserve stay");
    // The next pass: the cancel cannot be told again, but the order reads cancelled.
    t.reply(&format!("DELETE /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 503, json!("unavailable"));
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

// ---- HF-4 (D-KUCOIN-HF-RETENTION): obligations, receipts, the spending barrier -------------------------------------

fn obligation(c: &rusqlite::Connection, id: i64) -> Option<Value> { deltabadger::engine::reconcile::obligation(c, id).unwrap() }
fn receipt(c: &rusqlite::Connection, id: i64) -> Option<Value> { deltabadger::engine::reconcile::receipt(c, id).unwrap() }
fn begins(c: &rusqlite::Connection, bot: i64, at: chrono::DateTime<chrono::Utc>) -> bool {
    use deltabadger::engine::{placement, FixedClock};
    let (_d, _o, _b, plan) = engine_setup();
    let b = model::load_bot(c, bot).unwrap();
    let begun = placement::begin_held(c, &b, &plan, None, None, &FixedClock(at)).unwrap();
    let placed = matches!(begun, placement::Begun::Intent(_));
    if placed { placement::drop_intent(c, bot).unwrap(); }
    placed
}
fn not_found(t: &ScriptedTransport, ext: &str) { t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, json!({"code":"400100","msg":"order not exist"})); }
fn at(s: &str) -> chrono::DateTime<chrono::Utc> { s.parse().unwrap() }

/// R02: an HF row KuCoin no longer serves, at ages around 2, 7 and 14 days, is never abandoned or closed from fills; it
/// owes (an obligation) and its bot places nothing.
#[tokio::test(flavor = "current_thread")]
async fn r02_ages_around_2_7_14_days_never_abandon_or_close() {
    use deltabadger::engine::polling;
    let now = at("2026-10-30T12:05:00Z");
    for days in [2, 7, 14] {
        for secs in [-1, 0, 1] {
            let (dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
            let created = now - chrono::Duration::days(days) + chrono::Duration::seconds(secs);
            o.primary.execute("UPDATE transactions SET created_at = ?1", [created.format("%Y-%m-%d %H:%M:%S").to_string()]).unwrap();
            not_found(&t, &ext);
            let r = polling::sweep(&o.primary, &v, &model::load_bot(&o.primary, bot.id).unwrap(), now).await;
            assert!(matches!(r, Err(polling::PollFailure::AwaitingReconciliation(tx, _)) if tx == id), "{days}d{secs:+}s: {r:?}");
            let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
            assert_eq!(row_exec(&c, id).0, Some(0), "{days}d{secs:+}s: still waiting");
            assert!(!t.requests().iter().any(|r| r.path.contains("fills")), "no fills fallback");
            assert!(obligation(&c, id).is_some(), "{days}d{secs:+}s: an obligation");
            assert!(!begins(&c, bot.id, now), "{days}d{secs:+}s: no new spending");
        }
    }
}

/// R02: a resting order a month old is still read, its exact partial fill kept, and owes nothing.
#[tokio::test(flavor = "current_thread")]
async fn r02_an_old_resting_order_is_still_read() {
    use deltabadger::engine::polling;
    let now = at("2026-11-30T12:05:00Z");
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, hf_answer(&ext, true, false, "0.0004", "24"));
    polling::sweep(&o.primary, &v, &model::load_bot(&o.primary, bot.id).unwrap(), now).await.unwrap();
    assert_eq!(row_exec(&o.primary, id), (Some(1), Some(0.0004)));
    assert!(obligation(&o.primary, id).is_none());
    assert!(begins(&o.primary, bot.id, now));
}

/// R02/R06, r1.1, r3.3: the obligation holds the bot's spending wherever an intent is born (DCA, signal) and whatever
/// pair; the tick ends AwaitingReconciliation with no POST; a read that applies ends it; a row moved to another bot (Merge,
/// Split) moves it.
#[tokio::test(flavor = "current_thread")]
async fn r02_r06_an_unreadable_hf_row_holds_new_spending() {
    use deltabadger::engine::{placement, polling, tick, FixedClock};
    let now = at("2026-11-08T12:05:00Z");
    let (dir, _o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    // One answer per read, in order (a lone scripted answer repeats).
    not_found(&t, &ext);
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, hf_answer(&ext, true, false, "0", "0"));
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let outcome = tick::tick(&c, &v, bot.id, &FixedClock(now), &mut Default::default()).await.unwrap();
    assert!(matches!(outcome, tick::TickOutcome::AwaitingReconciliation), "{outcome:?}");
    assert_eq!(posts(&t), 1, "no new order");
    assert!(model::load_bot(&c, bot.id).unwrap().rust_placement().is_none(), "no intent");
    // The signal path refuses too.
    let (_d, _o, _b, plan) = engine_setup();
    let b = model::load_bot(&c, bot.id).unwrap();
    let version = model::credential_version(&c, &b).unwrap();
    assert!(matches!(placement::begin_signal(&c, &b, &plan, None, &version, None, &FixedClock(now)).unwrap(), placement::SignalClaim::Blocked));
    // Another bot owning the row (Merge/Split moved it) holds it; the first bot is free.
    let other = common::seed::insert_bot(&c, &common::seed::Seeded { user_id: 1, exchange_id: bot.exchange_id, btc: 1, quote: 2, ticker_id: 1, api_key_id: 1 }, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    c.execute("UPDATE transactions SET bot_id = ?1 WHERE id = ?2", [other, id]).unwrap();
    assert!(begins(&c, bot.id, now) && !begins(&c, other, now), "the obligation follows its row");
    c.execute("UPDATE transactions SET bot_id = ?1 WHERE id = ?2", [bot.id, id]).unwrap();
    // A read that applies ends it.
    polling::sweep(&c, &v, &model::load_bot(&c, bot.id).unwrap(), now).await.unwrap();
    assert!(obligation(&c, id).is_none() && begins(&c, bot.id, now));
}

/// r1.1, r3.1: lower totals and an unknown state each owe (no longer a silent skip), and hold the spending.
#[tokio::test(flavor = "current_thread")]
async fn r1_lower_totals_and_unknown_states_owe() {
    use deltabadger::engine::polling;
    let now = at("2026-10-30T12:05:00Z");
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    let key = format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR");
    t.reply(&key, 200, hf_answer(&ext, true, false, "0.0004", "24"));
    t.reply(&key, 200, hf_answer(&ext, true, false, "0.0003", "18"));
    polling::sweep(&o.primary, &v, &model::load_bot(&o.primary, bot.id).unwrap(), now).await.unwrap();
    let r = polling::sweep(&o.primary, &v, &model::load_bot(&o.primary, bot.id).unwrap(), now).await;
    assert!(matches!(r, Err(polling::PollFailure::AwaitingReconciliation(..))), "{r:?}");
    assert_eq!(row_exec(&o.primary, id), (Some(1), Some(0.0004)), "the lower answer wrote nothing");
    assert!(!begins(&o.primary, bot.id, now));
    // A limit order KuCoin ended with nothing dealt and no cancel is Unknown: owed, not a failed sweep.
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", true).await;
    let mut unknown = hf_answer(&ext, false, false, "0", "0");
    unknown["data"]["type"] = json!("limit");
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, unknown);
    let r = polling::sweep(&o.primary, &v, &model::load_bot(&o.primary, bot.id).unwrap(), now).await;
    assert!(matches!(r, Err(polling::PollFailure::AwaitingReconciliation(tx, _)) if tx == id), "{r:?}");
    assert!(!begins(&o.primary, bot.id, now));
}

/// r3.2: a failed follow-up read owes before its retry.
#[tokio::test(flavor = "current_thread")]
async fn r3_a_failed_follow_up_read_owes() {
    use deltabadger::engine::polling;
    let now = at("2026-10-30T12:05:00Z");
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    t.network(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), "pre_send", "timeout");
    assert!(matches!(polling::follow_up(&o.primary, &v, bot.id, id, now).await, Err(polling::PollFailure::Transient(_))));
    assert!(obligation(&o.primary, id).is_some());
    assert!(!begins(&o.primary, bot.id, now));
}

/// R05, r1.2, r2.5: a settled row keeps its settlement. An open or higher answer afterwards writes nothing and owes; the
/// settled facts again end that; a read KuCoin can no longer serve owes nothing.
#[tokio::test(flavor = "current_thread")]
async fn r05_a_settled_row_takes_only_its_own_facts() {
    use deltabadger::engine::polling;
    let now = at("2026-10-30T12:05:00Z");
    let (dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    let key = format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR");
    for (active, cancelled, size, funds) in [(false, true, "0.0004", "24"), (true, false, "0.0004", "24"), (false, true, "0.0005", "30"), (false, true, "0.0004", "24")] {
        t.reply(&key, 200, hf_answer(&ext, active, cancelled, size, funds));
    }
    polling::sweep(&o.primary, &v, &model::load_bot(&o.primary, bot.id).unwrap(), now).await.unwrap();
    assert_eq!(row_exec(&o.primary, id), (Some(3), Some(0.0004)));
    let settled = receipt(&o.primary, id).unwrap();
    assert_eq!((&settled["status"], &settled["deal_size"], &settled["source"]), (&json!("cancelled"), &json!("0.0004"), &json!("api")));
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    for _ in 0..2 {
        polling::refresh(&c, &v, bot.id, id, now).await.unwrap();
        assert_eq!(row_exec(&c, id), (Some(3), Some(0.0004)), "never reopened or raised");
        assert!(obligation(&c, id).is_some() && !begins(&c, bot.id, now));
    }
    polling::refresh(&c, &v, bot.id, id, now).await.unwrap();
    assert!(obligation(&c, id).is_none(), "its own facts end the obligation");
    assert_eq!(receipt(&c, id).unwrap(), settled, "the receipt is unchanged");
    not_found(&t, &ext);
    for _ in 0..2 { let _ = polling::refresh(&c, &v, bot.id, id, now).await; }
    assert!(obligation(&c, id).is_none() && begins(&c, bot.id, now), "a settled row owes nothing for a missing answer");
}

fn known_fill(c: &rusqlite::Connection, user: i64, exchange: i64, trade: &str, order: &str, size: &str, funds: &str, fee: &str) {
    let raw = json!({"symbol":"BTC-EUR","tradeId":trade,"orderId":order,"side":"buy","size":size,"funds":funds,"fee":fee,"feeCurrency":"EUR","createdAt":1793361600000_i64,"family":"ClassicHf"});
    c.execute("INSERT INTO account_transactions (user_id, exchange_id, base_amount, base_currency, entry_type, transacted_at, created_at, updated_at, raw_data, tx_id) \
               VALUES (?1, ?2, ?3, 'BTC', 0, '2026-10-30 12:00:00', '2026-10-30 12:00:00', '2026-10-30 12:00:00', ?4, ?5)",
              rusqlite::params![user, exchange, size, raw.to_string(), format!("BTC-EUR-{trade}")]).unwrap();
}

/// r1.3, r2.7, r2.8: a terminal answer below the order's known fills (the owner's, on the row's exchange) owes and settles
/// nothing; another user's fill of the same order id is not this order's; fees count as their absolute value.
#[tokio::test(flavor = "current_thread")]
async fn r1_known_fills_bound_a_settlement() {
    use deltabadger::engine::{polling, reconcile};
    let now = at("2026-10-30T12:05:00Z");
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    known_fill(&o.primary, 1, bot.exchange_id, "1", &ext, "0.0003", "18", "-0.018");
    known_fill(&o.primary, 1, bot.exchange_id, "2", &ext, "0.0002", "12", "0.012");
    o.primary.execute_batch("CREATE TEMP TABLE u AS SELECT * FROM users WHERE id = 1; UPDATE u SET id = 2, email = 'two@example.test'; INSERT INTO users SELECT * FROM u;").unwrap();
    known_fill(&o.primary, 2, bot.exchange_id, "3", &ext, "0.9", "9000", "9");
    let k = reconcile::known_fills(&o.primary, 1, bot.exchange_id, &ext).unwrap();
    assert_eq!((k.size.to_s_f(), k.funds.to_s_f(), k.fees["EUR"].to_s_f()), ("0.0005".into(), "30.0".into(), "0.03".into()));
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, hf_answer(&ext, false, true, "0.0004", "24"));
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, hf_answer(&ext, false, true, "0.0005", "30"));
    assert!(polling::sweep(&o.primary, &v, &model::load_bot(&o.primary, bot.id).unwrap(), now).await.is_err());
    assert_eq!(row_exec(&o.primary, id).0, Some(0), "not settled below its known fills");
    assert!(receipt(&o.primary, id).is_none() && obligation(&o.primary, id).is_some());
    polling::sweep(&o.primary, &v, &model::load_bot(&o.primary, bot.id).unwrap(), now).await.unwrap();
    assert_eq!(row_exec(&o.primary, id).0, Some(3));
}

fn reversal_work(c: &rusqlite::Connection, bot: i64, id: i64) {
    use deltabadger::engine::reversal;
    c.execute(&format!("UPDATE bots SET transient_data=json_set(transient_data,'$.{}',json(?1),'$.{}',json(?2)) WHERE id=?3", reversal::CANCELS, reversal::RESERVE_PENDING),
              rusqlite::params![json!([{"id": id, "registered": true, "sent": true}]).to_string(), json!([id]).to_string(), bot]).unwrap();
}
fn work(c: &rusqlite::Connection, bot: i64) -> (Vec<i64>, Vec<i64>) { deltabadger::engine::reversal::work(&model::load_bot(c, bot).unwrap()) }

/// §3 step 3: a proven terminal read ends the reversal's cancel and credits the reserve in the same settlement, also for a
/// Closed answer (today's cancel retry knows only Cancelled).
#[tokio::test(flavor = "current_thread")]
async fn r05_a_terminal_read_ends_the_cancel_and_settles_the_reserve_once() {
    use deltabadger::engine::polling;
    let now = at("2026-10-30T12:05:00Z");
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    reversal_work(&o.primary, bot.id, id);
    let carry = |c: &rusqlite::Connection| model::load_bot(c, bot.id).unwrap().transient["missed_quote_amount"].clone();
    let before = carry(&o.primary);
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, hf_answer(&ext, false, false, "0.0004", "24"));
    polling::follow_up(&o.primary, &v, bot.id, id, now).await.unwrap();
    assert_eq!(row_exec(&o.primary, id).0, Some(2));
    assert_eq!(work(&o.primary, bot.id), (vec![], vec![]), "the cancel and the reserve settled");
    let after = carry(&o.primary);
    assert_ne!(after, before, "the unspent reserve returned");
    polling::refresh(&o.primary, &v, bot.id, id, now).await.unwrap();
    assert_eq!(carry(&o.primary), after, "credited once");
}

/// r2.4: a terminal HF row settled before receipts takes its own facts once: its receipt, and what it still owed.
#[tokio::test(flavor = "current_thread")]
async fn r2_a_terminal_row_without_a_receipt_settles_what_it_still_owes() {
    use deltabadger::engine::polling;
    let now = at("2026-10-30T12:05:00Z");
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    for (size, funds) in [("0.0004", "24"), ("0.0005", "30"), ("0.0004", "24")] {
        t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, hf_answer(&ext, false, false, size, funds));
    }
    polling::follow_up(&o.primary, &v, bot.id, id, now).await.unwrap();
    // As HF-3 left it: no receipt, and a reversal's work still open.
    deltabadger::app_config::delete(&o.primary, &format!("{}{id}", deltabadger::engine::reconcile::RECEIPT)).unwrap();
    reversal_work(&o.primary, bot.id, id);
    polling::refresh(&o.primary, &v, bot.id, id, now).await.unwrap();
    assert!(obligation(&o.primary, id).is_some() && receipt(&o.primary, id).is_none(), "other facts: a conflict");
    assert_eq!(work(&o.primary, bot.id).1, vec![id]);
    polling::refresh(&o.primary, &v, bot.id, id, now).await.unwrap();
    assert!(obligation(&o.primary, id).is_none() && receipt(&o.primary, id).is_some());
    assert_eq!(work(&o.primary, bot.id), (vec![], vec![]));
}

// ---- HF-4: `deltabadger reconcile-order` (R03–R05) ------------------------------------------------------------------

/// A v1 evidence bundle for row `id`'s HF order, from its record (`order_record`).
fn bundle(c: &rusqlite::Connection, id: i64, state: &str, base: &str, quote: &str, fees: Value, source: Value) -> Value {
    let rec = deltabadger::venue::kucoin::order_record(c, id).unwrap().unwrap();
    let ext: String = c.query_row("SELECT external_id FROM transactions WHERE id=?1", [id], |r| r.get(0)).unwrap();
    json!({ "version": 1, "transaction_id": id,
            "account": { "host": rec["host"], "key_fingerprint": rec["key_fingerprint"] },
            "order": { "id": ext, "symbol": rec["symbol"], "side": if rec["sell"] == true { "sell" } else { "buy" },
                       "type": if rec["limit"] == true { "limit" } else { "market" }, "family": "ClassicHf" },
            "terminal": { "state": state, "at": "2026-10-30T12:01:00Z" },
            "executed": { "base": base, "quote": quote }, "fees": fees, "source": source })
}
fn attested(raw: &str) -> Value {
    json!({ "kind": "exchange_order_record", "note": "KuCoin order history, exported", "sha256": hex::encode(Sha256::digest(raw.as_bytes())), "raw": raw })
}
fn eur(amount: &str) -> Value { json!([{ "currency": "EUR", "amount": amount }]) }
fn reconcile(c: &rusqlite::Connection, id: i64, b: &Value) -> Result<deltabadger::engine::reconcile::Operator, deltabadger::engine::EngineError> {
    deltabadger::engine::reconcile::by_operator(c, id, &b.to_string(), at("2026-10-30T13:00:00Z"))
}
/// Everything a refused reconciliation must leave as it was.
fn snapshot(c: &rusqlite::Connection) -> String {
    let q = |sql: &str| -> Vec<String> { let mut s = c.prepare(sql).unwrap(); s.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap).collect() };
    [q("SELECT json_array(id, status, external_status, amount_exec, quote_amount_exec, price, error_messages) FROM transactions ORDER BY id"),
     q("SELECT json_array(id, status, transient_data) FROM bots ORDER BY id"),
     q("SELECT json_array(key, value) FROM app_configs WHERE key NOT LIKE 'rust_job%' ORDER BY key"),
     q("SELECT json_array(id, event) FROM bot_activity_logs ORDER BY id")].concat().join("\n")
}

/// R03: an operator's bundle settles an existing HF row with no intent, no new row and no request: a cancelled partial buy
/// (its reversal's cancel ended and its reserve credited once), a filled sell, a market buy's proven zero fill.
#[tokio::test(flavor = "current_thread")]
async fn r03_operator_settles_filled_sell_cancelled_partial_buy_and_zero_fill() {
    use deltabadger::engine::reconcile::Operator;
    // Cancelled partial buy.
    let (dir, o, bot, t, _v, id, _ext) = placed("KEY-A", false).await;
    reversal_work(&o.primary, bot.id, id);
    let sent = t.requests().len();
    let rows: i64 = o.primary.query_row("SELECT count(*) FROM transactions", [], |r| r.get(0)).unwrap();
    let b = bundle(&o.primary, id, "cancelled", "0.0004", "24", eur("0.024"), attested("order HF1 cancelled, dealt 0.0004 for 24 EUR"));
    assert_eq!(reconcile(&o.primary, id, &b).unwrap(), Operator::Reconciled);
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(row_exec(&c, id), (Some(3), Some(0.0004)));
    assert_eq!(work(&c, bot.id), (vec![], vec![]), "cancel ended, reserve credited");
    let r = receipt(&c, id).unwrap();
    assert_eq!((&r["source"], &r["fees"], &r["status"]), (&json!("operator"), &eur("0.024"), &json!("cancelled")));
    assert!(r["sha256"].is_string() && r["evidence"]["order"]["id"] == "HF1");
    assert_eq!(t.requests().len(), sent, "no request");
    assert_eq!(c.query_row("SELECT count(*) FROM transactions", [], |r| r.get::<_, i64>(0)).unwrap(), rows, "no new row");
    assert!(model::load_bot(&c, bot.id).unwrap().rust_placement().is_none(), "no intent");
    assert_eq!(c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event='placement_ambiguous' AND json_extract(details,'$.resolution')='reconciled'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    // Filled sell.
    let (_dir, o, _bot, _t, _v, id, _ext) = placed("KEY-A", false).await;
    let key = format!("{}{id}", deltabadger::venue::kucoin::ORDER_RECORD);
    let mut rec = deltabadger::venue::kucoin::order_record(&o.primary, id).unwrap().unwrap();
    rec["sell"] = json!(true);
    deltabadger::app_config::set_plain(&o.primary, &key, &rec.to_string(), at("2026-10-30T12:00:00Z")).unwrap();
    o.primary.execute("UPDATE transactions SET side = 1 WHERE id = ?1", [id]).unwrap();
    assert_eq!(reconcile(&o.primary, id, &bundle(&o.primary, id, "closed", "0.001", "60", eur("0.06"), attested("sold"))).unwrap(), Operator::Reconciled);
    assert_eq!(row_exec(&o.primary, id), (Some(2), Some(0.001)));
    // A market buy's proven zero fill: the existing zero-fill reading (the row fails with the venue's refusal); replay is a no-op.
    let (_dir, o, _bot, _t, _v, id, _ext) = placed("KEY-A", false).await;
    let b = bundle(&o.primary, id, "closed", "0", "0", json!([]), attested("done, nothing dealt"));
    assert_eq!(reconcile(&o.primary, id, &b).unwrap(), Operator::Reconciled);
    let (status, external): (i64, i64) = o.primary.query_row("SELECT status, external_status FROM transactions WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((status, external), (1, 2));
    assert_eq!(reconcile(&o.primary, id, &b).unwrap(), Operator::AlreadyReconciled);
}

/// R03: a retained order-detail answer and a full-order execution list are evidence; each must agree with the bundle.
#[tokio::test(flavor = "current_thread")]
async fn r03_api_order_detail_and_execution_list_bundles() {
    use deltabadger::engine::reconcile::Operator;
    let (_dir, o, _bot, _t, _v, id, ext) = placed("KEY-A", false).await;
    let mut detail = hf_answer(&ext, false, true, "0.0004", "24");
    detail["data"]["fee"] = json!("-0.024"); detail["data"]["feeCurrency"] = json!("EUR");
    let raw = detail.to_string();
    let source = json!({ "kind": "api_order_detail", "sha256": hex::encode(Sha256::digest(raw.as_bytes())), "raw": raw });
    assert!(reconcile(&o.primary, id, &bundle(&o.primary, id, "cancelled", "0.0004", "24", eur("0.03"), source.clone())).is_err(), "fees differ from the detail");
    assert_eq!(reconcile(&o.primary, id, &bundle(&o.primary, id, "cancelled", "0.0004", "24", eur("0.024"), source)).unwrap(), Operator::Reconciled);
    let (_dir, o, _bot, _t, _v, id, _ext) = placed("KEY-A", false).await;
    let raw = "fill 1; fill 2";
    let list = |coverage: Option<&str>| { let mut s = json!({ "kind": "execution_list", "sha256": hex::encode(Sha256::digest(raw.as_bytes())), "raw": raw,
        "fills": [{"id":"1","base":"0.0003","quote":"18","fee":"0.018","fee_currency":"EUR"},{"id":"2","base":"0.0001","quote":"6","fee":"0.006","fee_currency":"EUR"}] });
        if let Some(cov) = coverage { s["coverage"] = json!(cov); } s };
    assert!(reconcile(&o.primary, id, &bundle(&o.primary, id, "closed", "0.0004", "24", eur("0.024"), list(None))).is_err(), "a trade list without full-order coverage");
    assert_eq!(reconcile(&o.primary, id, &bundle(&o.primary, id, "closed", "0.0004", "24", eur("0.024"), list(Some("full_order")))).unwrap(), Operator::Reconciled);
}

/// R04: every incomplete, contradicting or foreign bundle is refused, naming why, with nothing written.
#[tokio::test(flavor = "current_thread")]
async fn r04_refusals_write_nothing() {
    let (_dir, o, bot, _t, _v, id, ext) = placed("KEY-A", false).await;
    reversal_work(&o.primary, bot.id, id);
    known_fill(&o.primary, 1, bot.exchange_id, "1", &ext, "0.0002", "12", "0.02");
    let good = bundle(&o.primary, id, "cancelled", "0.0004", "24", eur("0.024"), attested("x"));
    let with = |path: &str, v: Value| { let mut b = good.clone(); *b.pointer_mut(path).unwrap() = v; b };
    let ack = json!({"code":"200000","data":{"orderId": ext}}).to_string();
    let mut cases: Vec<(&str, Value)> = vec![
        ("version", with("/version", json!(2))),
        ("another transaction", with("/transaction_id", json!(id + 1))),
        ("another host", with("/account/host", json!("https://api.example.test"))),
        ("another key", with("/account/key_fingerprint", json!("00"))),
        ("another order", with("/order/id", json!("HF2"))),
        ("another symbol", with("/order/symbol", json!("ETH-EUR"))),
        ("another side", with("/order/side", json!("sell"))),
        ("another type", with("/order/type", json!("limit"))),
        ("classic family", with("/order/family", json!("Classic"))),
        ("no terminal proof", with("/terminal/state", json!("open"))),
        ("missing totals", with("/executed/base", json!(""))),
        ("negative totals", with("/executed/quote", json!("-24"))),
        ("one total zero", with("/executed/quote", json!("0"))),
        ("bad time", with("/terminal/at", json!("yesterday"))),
        ("bad hash", with("/source/sha256", json!("00"))),
        ("trade-only CSV", with("/source/kind", json!("trade_csv"))),
        ("a cancel acknowledgement as the detail", with("/source", json!({"kind":"api_order_detail","sha256":hex::encode(Sha256::digest(ack.as_bytes())),"raw":ack}))),
        ("fewer than the known fills' fees", with("/fees", eur("0.01"))),
        ("below the known fills", bundle(&o.primary, id, "cancelled", "0.0001", "6", eur("0.024"), attested("x"))),
        ("a zero below the known fills", with("/executed", json!({"base":"0","quote":"0"}))),
    ];
    let mut unknown = good.clone(); unknown["extra"] = json!(1);
    cases.push(("an unknown field", unknown));
    cases.push(("a duplicate fee currency", with("/fees", json!([{"currency":"EUR","amount":"0.01"},{"currency":"EUR","amount":"0.014"}]))));
    let before = snapshot(&o.primary);
    for (name, b) in &cases {
        let r = reconcile(&o.primary, id, b);
        assert!(r.is_err(), "{name}: {r:?}");
        assert_eq!(snapshot(&o.primary), before, "{name}: nothing written");
    }
    // A cancelled zero is a limit order's proven zero, a closed zero a market one's; a closed zero on a limit row is refused.
    let (_dir, o, _bot, _t, _v, id, _ext) = placed("KEY-A", true).await;
    let before = snapshot(&o.primary);
    assert!(reconcile(&o.primary, id, &bundle(&o.primary, id, "closed", "0", "0", json!([]), attested("x"))).is_err());
    assert_eq!(snapshot(&o.primary), before);
    // A classic row has no recorded route.
    let (_dir, o, bot, _t, _v, _id, _ext) = placed("KEY-A", false).await;
    let classic = common::seed::insert_row(&o.primary, &common::seed::Seeded { user_id: 1, exchange_id: bot.exchange_id, btc: 1, quote: 2, ticker_id: 1, api_key_id: 1 }, bot.id, 1,
                                           &json!({"external_id": "CLASSIC1", "created_at": "2026-10-30 12:00:00"}));
    let mut b = bundle(&o.primary, _id, "closed", "0.001", "60", eur("0"), attested("x"));
    b["transaction_id"] = json!(classic); b["order"]["id"] = json!("CLASSIC1");
    let err = format!("{:?}", reconcile(&o.primary, classic, &b).unwrap_err());
    assert!(err.contains("not an HF order with a recorded route"), "{err}");
}

/// R04: `resolve-placement` settles only an intent: for an HF row that is no longer one, it refuses.
#[tokio::test(flavor = "current_thread")]
async fn r04_resolve_placement_refuses_an_hf_row() {
    use deltabadger::engine::placement::{self, OperatorResolution};
    let (_dir, o, bot, _t, _v, id, ext) = placed("KEY-A", false).await;
    let before = snapshot(&o.primary);
    assert!(placement::resolve_by_operator(&o.primary, bot.id, OperatorResolution::Placed(ext), at("2026-10-30T13:00:00Z")).is_err());
    assert_eq!(snapshot(&o.primary), before);
    assert_eq!(row_exec(&o.primary, id).0, Some(0));
}

/// R05, r1.6: the same bundle again is a no-op; other facts or fees are a conflict with nothing written; an API settlement
/// takes an operator's fees once; a poll answer that contradicts an operator's settlement changes nothing and owes.
#[tokio::test(flavor = "current_thread")]
async fn r05_replay_conflict_and_fee_supplement() {
    use deltabadger::engine::{polling, reconcile::Operator};
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    reversal_work(&o.primary, bot.id, id);
    let b = bundle(&o.primary, id, "cancelled", "0.0004", "24", eur("0.024"), attested("x"));
    assert_eq!(reconcile(&o.primary, id, &b).unwrap(), Operator::Reconciled);
    let carry = model::load_bot(&o.primary, bot.id).unwrap().transient["missed_quote_amount"].clone();
    let settled = snapshot(&o.primary);
    assert_eq!(reconcile(&o.primary, id, &b).unwrap(), Operator::AlreadyReconciled);
    assert_eq!(snapshot(&o.primary), settled, "a replay writes nothing");
    for other in [bundle(&o.primary, id, "cancelled", "0.0005", "30", eur("0.024"), attested("x")), bundle(&o.primary, id, "cancelled", "0.0004", "24", eur("0.03"), attested("x"))] {
        assert!(format!("{:?}", reconcile(&o.primary, id, &other).unwrap_err()).contains("conflict"));
        assert_eq!(snapshot(&o.primary), settled);
    }
    assert_eq!(model::load_bot(&o.primary, bot.id).unwrap().transient["missed_quote_amount"], carry, "credited once");
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, hf_answer(&ext, false, true, "0.0005", "30"));
    let _ = polling::refresh(&o.primary, &v, bot.id, id, at("2026-10-30T13:05:00Z")).await;
    assert_eq!(row_exec(&o.primary, id), (Some(3), Some(0.0004)));
    assert!(obligation(&o.primary, id).is_some());
    // Settled by the API (no fees), then the operator's equal facts: the fees join the receipt, nothing else.
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    t.reply(&format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, hf_answer(&ext, false, true, "0.0004", "24"));
    polling::follow_up(&o.primary, &v, bot.id, id, at("2026-10-30T12:05:00Z")).await.unwrap();
    assert!(receipt(&o.primary, id).unwrap()["fees"].is_null());
    assert_eq!(reconcile(&o.primary, id, &bundle(&o.primary, id, "cancelled", "0.0004", "24", eur("0.024"), attested("x"))).unwrap(), Operator::AlreadyReconciled);
    assert_eq!(receipt(&o.primary, id).unwrap()["fees"], eur("0.024"));
    assert_eq!(c_count(&o.primary, "placement_ambiguous"), 0, "no settlement line for a supplement");
}
fn c_count(c: &rusqlite::Connection, event: &str) -> i64 { c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event=?1", [event], |r| r.get(0)).unwrap() }

/// r1.8: a reconciled buy that spends the quote cap stops the bot and owes its mail, in the settlement's transaction; a
/// replay owes nothing again.
#[tokio::test(flavor = "current_thread")]
async fn r1_a_reconciled_fill_that_spends_the_cap_stops_the_bot() {
    use deltabadger::engine::reconcile::Operator;
    let (_dir, o, bot, _t, _v, id, _ext) = placed("KEY-A", false).await;
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true'), '$.quote_amount_limit', 20), \
                       transient_data = json_set(transient_data, '$.quote_amount_limit_enabled_at', '2026-09-01T00:00:00.000Z') WHERE id = ?1", [bot.id]).unwrap();
    let b = bundle(&o.primary, id, "closed", "0.0004", "24", eur("0.024"), attested("x"));
    assert_eq!(reconcile(&o.primary, id, &b).unwrap(), Operator::Reconciled);
    assert_eq!(model::load_bot(&o.primary, bot.id).unwrap().status, deltabadger::enums::BotStatus::Stopped);
    let stops = c_count(&o.primary, "stopped");
    assert_eq!(reconcile(&o.primary, id, &b).unwrap(), Operator::AlreadyReconciled);
    assert_eq!(c_count(&o.primary, "stopped"), stops);
}

/// R03/R04 through the binary: usage, a refused bundle (exit 1, the reason, nothing written), then the settlement and its
/// replay. The engine's lock is taken, as `resolve-placement` takes it.
#[tokio::test(flavor = "current_thread")]
async fn reconcile_order_cli() {
    let (dir, o, _bot, _t, _v, id, _ext) = placed("KEY-A", false).await;
    let run = |args: &[&str]| std::process::Command::new(env!("CARGO_BIN_EXE_deltabadger")).args(args).env("STORAGE_DIR", dir.path())
        .env_remove("DATABASE_URL").env_remove("PRIMARY_DATABASE_URL").env_remove("QUEUE_DATABASE_URL").output().unwrap();
    for args in [vec!["reconcile-order"], vec!["reconcile-order", "x", "--evidence", "f"], vec!["reconcile-order", "1", "--proof", "f"]] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage: deltabadger reconcile-order"), "{args:?}");
    }
    let file = dir.path().join("evidence.json");
    let good = bundle(&o.primary, id, "cancelled", "0.0004", "24", eur("0.024"), attested("x"));
    let mut bad = good.clone(); bad["order"]["symbol"] = json!("ETH-EUR");
    std::fs::write(&file, bad.to_string()).unwrap();
    let before = snapshot(&o.primary);
    let out = run(&["reconcile-order", &id.to_string(), "--evidence", file.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("order.symbol is not the order's"), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(snapshot(&o.primary), before);
    std::fs::write(&file, good.to_string()).unwrap();
    for want in ["reconciled", "already reconciled"] {
        let out = run(&["reconcile-order", &id.to_string(), "--evidence", file.to_str().unwrap()]);
        assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).trim()), (Some(0), want), "{}", String::from_utf8_lossy(&out.stderr));
    }
    assert_eq!(row_exec(&o.primary, id).0, Some(3));
}

// ---- HF-4: reversal cancels of HF rows end only in a settlement (r1.4, r2.2) ----------------------------------------

/// r1.4: an acknowledged HF cancel is a request: the entry and the reserve stay while the order reads open; the follow-up
/// read of its cancelled partial settles both once, with the exact execution.
#[tokio::test(flavor = "current_thread")]
async fn r1_4_an_acknowledged_hf_cancel_waits_for_the_orders_final_state() {
    use deltabadger::engine::{polling, reversal};
    let now = at("2026-10-30T12:05:00Z");
    let (dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    reversal_work(&o.primary, bot.id, id);
    o.primary.execute(&format!("UPDATE bots SET transient_data=json_set(transient_data,'$.{}',json(?1)) WHERE id=?2", reversal::CANCELS),
                      rusqlite::params![json!([{"id": id, "registered": true, "sent": false}]).to_string(), bot.id]).unwrap();
    t.reply(&format!("DELETE /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 200, json!({"code":"200000","data":{"orderId": ext}}));
    let key = format!("GET /api/v1/hf/orders/{ext}?symbol=BTC-EUR");
    t.reply(&key, 200, hf_answer(&ext, true, false, "0.0002", "12"));
    t.reply(&key, 200, hf_answer(&ext, false, true, "0.0004", "24"));
    let handle = deltabadger::venue::Handle::attributed(&v);
    assert_eq!(reversal::cancel_captured(&o.primary, &handle, bot.id, now).await.unwrap(), vec![id], "polled after its cancel");
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(work(&c, bot.id), (vec![id], vec![id]), "an acknowledgement settles nothing");
    polling::follow_up(&c, &v, bot.id, id, now).await.unwrap();
    assert_eq!(work(&c, bot.id), (vec![id], vec![id]), "still open: nothing settles");
    let carry = model::load_bot(&c, bot.id).unwrap().transient["missed_quote_amount"].clone();
    polling::follow_up(&c, &v, bot.id, id, now).await.unwrap();
    assert_eq!(row_exec(&c, id), (Some(3), Some(0.0004)));
    assert_eq!(work(&c, bot.id), (vec![], vec![]), "settled once");
    assert_ne!(model::load_bot(&c, bot.id).unwrap().transient["missed_quote_amount"], carry);
}

/// r2.2: once a cancel may have gone out (lost, or acknowledged), a refused retry with the order unreadable keeps the
/// entry and the reserve: the refusal does not tell what the first request did.
#[tokio::test(flavor = "current_thread")]
async fn r2_2_a_refused_retry_never_drops_an_earlier_cancel() {
    use deltabadger::engine::reversal;
    let now = at("2026-10-30T12:05:00Z");
    for first in ["lost", "acknowledged"] {
        let (dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
        reversal_work(&o.primary, bot.id, id);
        o.primary.execute(&format!("UPDATE bots SET transient_data=json_set(transient_data,'$.{}',json(?1)) WHERE id=?2", reversal::CANCELS),
                          rusqlite::params![json!([{"id": id, "registered": true, "sent": false}]).to_string(), bot.id]).unwrap();
        let del = format!("DELETE /api/v1/hf/orders/{ext}?symbol=BTC-EUR");
        if first == "lost" { t.network(&del, "post_send", "reset"); } else { t.reply(&del, 200, json!({"code":"200000","data":{"orderId": ext}})); }
        t.reply(&del, 400, json!({"code":"400100","msg":"order not exist"}));
        not_found(&t, &ext);
        let handle = deltabadger::venue::Handle::attributed(&v);
        let _ = reversal::cancel_captured(&o.primary, &handle, bot.id, now).await.unwrap();
        let _ = reversal::cancel_captured(&o.primary, &handle, bot.id, now).await.unwrap();
        let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        assert_eq!(work(&c, bot.id), (vec![id], vec![id]), "{first}: entry and reserve kept");
        assert_eq!(c_count(&c, "order_cancel_failed"), 0, "{first}: not a failed cancel");
    }
}

/// The venue refusing the FIRST HF cancel keeps today's reading (a failed cancel: the order may still be live).
#[tokio::test(flavor = "current_thread")]
async fn a_refused_first_hf_cancel_is_a_failed_cancel() {
    use deltabadger::engine::reversal;
    let now = at("2026-10-30T12:05:00Z");
    let (_dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    reversal_work(&o.primary, bot.id, id);
    o.primary.execute(&format!("UPDATE bots SET transient_data=json_set(transient_data,'$.{}',json(?1)) WHERE id=?2", reversal::CANCELS),
                      rusqlite::params![json!([{"id": id, "registered": true, "sent": false}]).to_string(), bot.id]).unwrap();
    t.reply(&format!("DELETE /api/v1/hf/orders/{ext}?symbol=BTC-EUR"), 400, json!({"code":"400100","msg":"order not exist"}));
    let handle = deltabadger::venue::Handle::attributed(&v);
    assert!(reversal::cancel_captured(&o.primary, &handle, bot.id, now).await.unwrap().is_empty());
    assert_eq!(work(&o.primary, bot.id), (vec![], vec![]));
    assert_eq!(c_count(&o.primary, "order_cancel_failed"), 1);
}

// ---- HF-4: recovery and takeover (R01, R06) ------------------------------------------------------------------------

/// An HF intent whose POST answered 230005 (ambiguous), on the seeded bot: (dir, store, bot, transport, venue, intent).
async fn ambiguous_intent(key: &str) -> (tempfile::TempDir, deltabadger::store::Opened, model::Bot, ScriptedTransport, KucoinVenue<ScriptedTransport>, deltabadger::engine::placement::Intent) {
    use deltabadger::engine::{placement, FixedClock};
    let t0 = at("2026-10-30T12:00:00Z");
    let (dir, o, bot, plan) = engine_setup();
    let t = ScriptedTransport::default(); t.set_now(t0); classic(&t, true);
    let v = KucoinVenue::new(t.clone(), URL).with_identity(&creds(key)).with_account_context(model::credential_version(&o.primary, &bot).unwrap().unwrap(), Resolver::default()).allow_hf();
    v.prepare().await.unwrap();
    let placement::Begun::Intent(intent) = placement::begin_held(&o.primary, &bot, &plan, v.placement_identity(), None, &FixedClock(t0)).unwrap() else { panic!() };
    t.reply("POST /api/v1/hf/orders", 503, json!({"code":"230005","msg":"The system is busy, please try again later"}));
    assert!(matches!(placement::send_recorded(&o.primary, &v, &bot, &intent, &FixedClock(t0)).await.unwrap(), placement::Sent::Ambiguous(_)));
    (dir, o, bot, t, v, *intent)
}

/// R01: misses, other orders' answers and failures past 1,200 s and past 7 days, each after reopening the store, never
/// clear the intent, write a row or send a second POST.
#[tokio::test(flavor = "current_thread")]
async fn r01_misses_past_1200s_and_7_days_never_clear_or_repost() {
    use deltabadger::engine::{tick, FixedClock};
    let (dir, _o, bot, t, v, intent) = ambiguous_intent("KEY-A").await;
    let key = format!("GET /api/v1/hf/orders/client-order/{}?symbol=BTC-EUR", intent.cl_ord_id);
    let mut other_symbol = json!({"code":"200000","data":{"id":"HF9","clientOid":intent.cl_ord_id,"symbol":"ETH-EUR","type":"market","side":"buy","price":"0","size":"0.001","funds":"60",
        "dealSize":"0","dealFunds":"0","active":true,"inOrderBook":false,"cancelExist":false,"createdAt":1793361600000_i64}});
    let mut other_client = other_symbol.clone();
    other_client["data"]["symbol"] = json!("BTC-EUR"); other_client["data"]["clientOid"] = json!("someone-else");
    other_symbol["data"]["clientOid"] = json!(intent.cl_ord_id);
    t.reply(&key, 200, json!({"code":"400100","msg":"order not exist"}));
    t.reply(&key, 200, other_symbol);
    t.reply(&key, 200, other_client);
    t.network(&key, "post_send", "reset");
    t.reply(&key, 200, json!({"code":"400100","msg":"order not exist"}));
    for after in [chrono::Duration::seconds(1201), chrono::Duration::seconds(1300), chrono::Duration::seconds(1400), chrono::Duration::days(7), chrono::Duration::days(8)] {
        let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        tick::tick(&c, &v, bot.id, &FixedClock(at("2026-10-30T12:00:00Z") + after), &mut Default::default()).await.unwrap();
        assert_eq!(model::load_bot(&c, bot.id).unwrap().rust_placement().unwrap()["cl_ord_id"], intent.cl_ord_id, "{after}: the intent stays");
        assert_eq!(submitted(&c), 0, "{after}: no row");
        assert_eq!(posts(&t), 1, "{after}: never a second POST");
    }
}

/// R06, r1.7: recovery asks the intent's own family under its own key, whatever the account's mode is now: after a
/// ClassicHf → ClassicLf change the HF clientOid route still recovers it; a replacement key asks nowhere.
#[tokio::test(flavor = "current_thread")]
async fn r06_mode_or_key_change_never_redirects_recovery() {
    use deltabadger::engine::{placement, FixedClock};
    let later = FixedClock(at("2026-10-30T12:30:00Z"));
    let (_dir, o, bot, _t, _v, intent) = ambiguous_intent("KEY-A").await;
    let version = model::credential_version(&o.primary, &bot).unwrap().unwrap();
    // A replacement key: no lookup anywhere, the intent stays.
    let t = ScriptedTransport::default(); t.set_now(later.0); classic(&t, true);
    let other = KucoinVenue::new(t.clone(), URL).with_identity(&creds("KEY-B")).with_account_context(version.clone(), Resolver::default()).allow_hf();
    assert!(matches!(placement::recover(&o.primary, &other, &model::load_bot(&o.primary, bot.id).unwrap(), &later).await.unwrap(), placement::Recovery::Pending));
    assert!(!t.requests().iter().any(|r| r.path.contains("orders")), "no order lookup under another key");
    // The same key, the account now classic: the HF route of the intent, which finds it.
    let t = ScriptedTransport::default(); t.set_now(later.0); classic(&t, false);
    let same = KucoinVenue::new(t.clone(), URL).with_identity(&creds("KEY-A")).with_account_context(version, Resolver::default()).allow_hf();
    t.reply(&format!("GET /api/v1/hf/orders/client-order/{}?symbol=BTC-EUR", intent.cl_ord_id), 200,
            json!({"code":"200000","data":{"id":"HF9","clientOid":intent.cl_ord_id,"symbol":"BTC-EUR","type":"market","side":"buy","price":"0","size":"0.001","funds":"60",
                   "dealSize":"0","dealFunds":"0","active":true,"inOrderBook":false,"cancelExist":false,"createdAt":1793361600000_i64}}));
    assert!(matches!(placement::recover(&o.primary, &same, &model::load_bot(&o.primary, bot.id).unwrap(), &later).await.unwrap(), placement::Recovery::Recorded(_)));
    assert!(!t.requests().iter().any(|r| r.path.starts_with("/api/v1/orders")), "never the classic route for an HF intent");
    let id: i64 = o.primary.query_row("SELECT id FROM transactions WHERE external_id='HF9'", [], |r| r.get(0)).unwrap();
    assert!(deltabadger::venue::kucoin::order_record(&o.primary, id).unwrap().is_some());
}

/// R06: after a shutdown longer than 7 days, an HF intent and an HF row KuCoin no longer serves both hold the bot: no
/// POST, the intent kept, the row owed; `reconcile-order` settles the row; the intent still blocks until it is recovered.
#[tokio::test(flavor = "current_thread")]
async fn r06_shutdown_over_7_days_reconciles_before_spending() {
    use deltabadger::engine::{placement, tick, FixedClock};
    let (dir, o, bot, t, v, id, ext) = placed("KEY-A", false).await;
    let t0 = at("2026-10-30T12:00:00Z");
    let (_d, _o2, _b, plan) = engine_setup();
    let placement::Begun::Intent(intent) = placement::begin_held(&o.primary, &model::load_bot(&o.primary, bot.id).unwrap(), &plan, v.placement_identity(), None, &FixedClock(t0)).unwrap() else { panic!() };
    t.network("POST /api/v1/hf/orders", "post_send", "reset");
    assert!(matches!(placement::send_recorded(&o.primary, &v, &bot, &intent, &FixedClock(t0)).await.unwrap(), placement::Sent::Ambiguous(_)));
    t.reply(&format!("GET /api/v1/hf/orders/client-order/{}?symbol=BTC-EUR", intent.cl_ord_id), 200, json!({"code":"400100","msg":"order not exist"}));
    not_found(&t, &ext);
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    for days in [8, 9] {
        tick::tick(&c, &v, bot.id, &FixedClock(t0 + chrono::Duration::days(days)), &mut Default::default()).await.unwrap();
        assert_eq!(posts(&t), 2, "day {days}: the first order and the ambiguous one only");
        assert!(model::load_bot(&c, bot.id).unwrap().rust_placement().is_some());
        assert_eq!(row_exec(&c, id).0, Some(0));
    }
    // The row: the operator's evidence.
    deltabadger::engine::reconcile::by_operator(&c, id, &bundle(&c, id, "closed", "0.001", "60", eur("0.06"), attested("filled")).to_string(), t0 + chrono::Duration::days(9)).unwrap();
    assert_eq!(row_exec(&c, id).0, Some(2));
    // The intent still holds the bot.
    tick::tick(&c, &v, bot.id, &FixedClock(t0 + chrono::Duration::days(10)), &mut Default::default()).await.unwrap();
    assert_eq!(posts(&t), 2);
    assert!(model::load_bot(&c, bot.id).unwrap().rust_placement().is_some());
}

// ---- HF-4: rows adopted without provenance (R07) -------------------------------------------------------------------

fn origin(c: &rusqlite::Connection, id: i64) -> Option<String> {
    deltabadger::app_config::get_plain(c, &format!("{}{id}", deltabadger::engine::reconcile::ORIGIN)).unwrap()
        .and_then(|v| serde_json::from_str::<Value>(&v).ok()).and_then(|v| v["origin"].as_str().map(str::to_string))
}
fn open_row(c: &rusqlite::Connection, bot: &model::Bot, ext: &str, created: &str, extra: Value) -> i64 {
    let mut row = json!({"external_id": ext, "created_at": created, "external_status": 0});
    for (k, v) in extra.as_object().unwrap() { row[k] = v.clone(); }
    common::seed::insert_row(c, &common::seed::Seeded { user_id: 1, exchange_id: bot.exchange_id, btc: 1, quote: 2, ticker_id: 1, api_key_id: 1 }, bot.id, 1, &row)
}

/// R07, r2.6: each claim marks the open KuCoin rows it finds without a record or a mark: classic when Rust placed them
/// (created in a Rust ownership window), unproven otherwise. A mark is never written twice, so a known-classic row stays
/// known through restarts and a Rails window.
#[tokio::test(flavor = "current_thread")]
async fn r07_takeover_marks_rows_by_who_placed_them() {
    use deltabadger::{engine::handover, lease};
    let (dir, o, bot, _plan) = engine_setup();
    let l = lease::lock(&deltabadger::store::Paths::from_env(&|_| None, dir.path()), at("2026-10-30T10:00:00Z")).unwrap();
    let rails = open_row(&o.primary, &bot, "A1", "2026-10-01 10:00:00", json!({}));
    let closed = open_row(&o.primary, &bot, "A2", "2026-10-01 10:00:00", json!({"external_status": 2}));
    handover::take_over(&l, &o, &common::seed::cipher(), "test", at("2026-10-30T10:00:00Z")).unwrap();
    assert_eq!((origin(&o.primary, rails).as_deref(), origin(&o.primary, closed)), (Some("unproven"), None));
    let rust = open_row(&o.primary, &bot, "B1", "2026-10-30 11:00:00", json!({}));
    handover::take_over(&l, &o, &common::seed::cipher(), "test", at("2026-10-30T11:30:00Z")).unwrap();
    assert_eq!(origin(&o.primary, rust).as_deref(), Some("classic"), "placed while Rust owned the install");
    assert_eq!(origin(&o.primary, rails).as_deref(), Some("unproven"));
    // Handed back at 12:00; Rails places C1; Rust takes over again.
    let tx = model::immediate(&o.primary).unwrap();
    lease::hand_back(&l, &tx, &common::seed::cipher(), at("2026-10-30T12:00:00Z")).unwrap();
    deltabadger::app_config::set_plain(&tx, lease::HANDED_BACK_AT, "2026-10-30T12:00:00.000Z", at("2026-10-30T12:00:00Z")).unwrap();
    tx.commit().unwrap();
    let by_rails = open_row(&o.primary, &bot, "C1", "2026-10-30 12:30:00", json!({}));
    handover::take_over(&l, &o, &common::seed::cipher(), "test", at("2026-10-30T13:00:00Z")).unwrap();
    assert_eq!(origin(&o.primary, by_rails).as_deref(), Some("unproven"), "placed in the Rails window");
    assert_eq!(origin(&o.primary, rust).as_deref(), Some("classic"), "never re-marked");
}

fn classic_order(id: &str, symbol: &str, active: bool, size: &str, funds: &str) -> Value {
    json!({"code":"200000","data":{ "id": id, "symbol": symbol, "opType": "DEAL", "type": "market", "side": "buy", "price": "0", "size": "0", "funds": "60",
        "dealFunds": funds, "dealSize": size, "fee": "0.06", "feeCurrency": "EUR", "clientOid": "x", "isActive": active, "cancelExist": false, "createdAt": 1790000000000u64 }})
}
fn adopted(opened: bool) -> (tempfile::TempDir, deltabadger::store::Opened, model::Bot, ScriptedTransport, KucoinVenue<ScriptedTransport>) {
    let (dir, o, bot, _plan) = engine_setup();
    // The venue's code for the pair differs from the asset's symbol (KuCoin's BCHSV for the BSV asset, here XBT for BTC).
    o.primary.execute_batch("UPDATE tickers SET ticker = 'XBT-EUR'").unwrap();
    let t = ScriptedTransport::default(); t.set_now(at("2026-10-30T12:00:00Z")); classic(&t, opened);
    let v = KucoinVenue::new(t.clone(), URL).with_identity(&creds("KEY-A")).with_account_context(model::credential_version(&o.primary, &bot).unwrap().unwrap(), Resolver::default()).allow_hf();
    (dir, o, bot, t, v)
}
fn mark_unproven(c: &rusqlite::Connection, id: i64) {
    deltabadger::app_config::set_plain(c, &format!("{}{id}", deltabadger::engine::reconcile::ORIGIN), r#"{"origin":"unproven"}"#, at("2026-10-30T10:00:00Z")).unwrap();
}

async fn sweep(c: &rusqlite::Connection, v: &KucoinVenue<ScriptedTransport>, bot: i64) -> Result<(), deltabadger::engine::polling::PollFailure> {
    deltabadger::engine::polling::sweep(c, v, &model::load_bot(c, bot).unwrap(), at("2026-11-20T12:00:00Z")).await
}

/// R07, r1.5, r2.3, r4: an adopted row's route is proven only by a positive answer naming it on that route, on the venue's
/// own ticker for its pair, in every account mode; until then it holds its bot and is never abandoned or cancelled.
#[tokio::test(flavor = "current_thread")]
async fn r07_adopted_rows_need_a_positive_route() {
    use deltabadger::engine::polling;
    let now = at("2026-11-20T12:00:00Z");
    // A classic account; the classic route names it: classic from now on, today's path.
    let (_d, o, bot, t, v) = adopted(false);
    let id = open_row(&o.primary, &bot, "A1", "2026-10-01 10:00:00", json!({}));
    mark_unproven(&o.primary, id);
    t.reply("GET /api/v1/orders/A1", 200, classic_order("A1", "XBT-EUR", true, "0", "0"));
    sweep(&o.primary, &v, bot.id).await.unwrap();
    assert_eq!(origin(&o.primary, id).as_deref(), Some("classic"));
    assert_eq!(row_exec(&o.primary, id).0, Some(1));
    // A classic account; nothing names it: held, never fills-closed or abandoned (it is 50 days old).
    let (_d, o, bot, t, v) = adopted(false);
    let id = open_row(&o.primary, &bot, "A1", "2026-10-01 10:00:00", json!({}));
    mark_unproven(&o.primary, id);
    t.reply("GET /api/v1/orders/A1", 404, json!({"code":"400100","msg":"order not exist."}));
    assert!(matches!(sweep(&o.primary, &v, bot.id).await, Err(polling::PollFailure::AwaitingReconciliation(tx, _)) if tx == id));
    assert_eq!((row_exec(&o.primary, id).0, origin(&o.primary, id).as_deref()), (Some(0), Some("unproven")));
    assert!(!t.requests().iter().any(|r| r.path.contains("fills")), "no fills fallback");
    assert!(!begins(&o.primary, bot.id, now));
    let r = deltabadger::venue::OrderRef::load(&o.primary, id, "A1").unwrap();
    assert!(matches!(v.cancel_with(&r).await, Err(deltabadger::venue::VenueError::Ambiguous(_))), "never cancelled on a guessed route");
    assert!(!t.requests().iter().any(|r| r.method == "DELETE"));
    // An HF account; the HF route names it on the venue's ticker: its record, with the row's execution as the floor.
    let (_d, o, bot, t, v) = adopted(true);
    let id = open_row(&o.primary, &bot, "A1", "2026-10-01 10:00:00", json!({"amount_exec": "0.0004", "quote_amount_exec": "24"}));
    mark_unproven(&o.primary, id);
    t.reply("GET /api/v1/orders/A1", 404, json!({"code":"400100","msg":"order not exist."}));
    let mut hf = hf_answer("A1", true, false, "0.0004", "24");
    hf["data"]["symbol"] = json!("XBT-EUR");
    let mut lower = hf.clone(); lower["data"]["dealSize"] = json!("0.0003"); lower["data"]["dealFunds"] = json!("18");
    t.reply("GET /api/v1/hf/orders/A1?symbol=XBT-EUR", 200, hf.clone());
    t.reply("GET /api/v1/hf/orders/A1?symbol=XBT-EUR", 200, hf);
    t.reply("GET /api/v1/hf/orders/A1?symbol=XBT-EUR", 200, lower);
    sweep(&o.primary, &v, bot.id).await.unwrap();
    let rec = deltabadger::venue::kucoin::order_record(&o.primary, id).unwrap().unwrap();
    assert_eq!((&rec["family"], &rec["symbol"], &rec["deal_size"]), (&json!("ClassicHf"), &json!("XBT-EUR"), &json!("0.0004")));
    assert_eq!(origin(&o.primary, id), None, "the record is its origin");
    let classic_reads = t.requests().iter().filter(|r| r.path == "/api/v1/orders/A1").count();
    assert!(sweep(&o.primary, &v, bot.id).await.is_err(), "a lower answer than the row's own execution owes");
    assert_eq!(t.requests().iter().filter(|r| r.path == "/api/v1/orders/A1").count(), classic_reads, "the HF route from now on");
    assert_eq!(row_exec(&o.primary, id), (Some(1), Some(0.0004)));
    // An HF account; neither route names it: held. A row whose pair maps to no ticker asks nowhere and is held.
    let (_d, o, bot, t, v) = adopted(true);
    let id = open_row(&o.primary, &bot, "A1", "2026-10-01 10:00:00", json!({}));
    mark_unproven(&o.primary, id);
    t.reply("GET /api/v1/orders/A1", 404, json!({"code":"400100","msg":"order not exist."}));
    t.reply("GET /api/v1/hf/orders/A1?symbol=XBT-EUR", 200, json!({"code":"400100","msg":"order not exist"}));
    assert!(sweep(&o.primary, &v, bot.id).await.is_err());
    assert_eq!(origin(&o.primary, id).as_deref(), Some("unproven"));
    o.primary.execute("UPDATE transactions SET base_asset_id = NULL WHERE id = ?1", [id]).unwrap();
    let before = t.requests().len();
    assert!(sweep(&o.primary, &v, bot.id).await.is_err());
    assert_eq!(t.requests().iter().skip(before).filter(|r| r.path.contains("A1")).count(), 0, "an unmapped pair asks nowhere");
}
