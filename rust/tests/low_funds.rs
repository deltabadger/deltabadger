//! A bot out of funds on KuCoin (plan LF, Rust-only): KuCoin ends a market buy it could not fund `isActive: false,
//! cancelExist: false` with nothing dealt, which Rails reads `:unknown` forever. Three named divergences:
//! D-KUCOIN-ZERO-FILL-DONE (such an order, once older than ZERO_FILL_SETTLE, is Closed with nothing filled),
//! D-ZERO-FILL-NOTE (one activity line says so, and claims no cause) and D-LOW-FUNDS-STATUS ("Low funds" before the
//! countdown, from the synced balances). Rails-free.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::events::EngineEvent;
use deltabadger::engine::placement::{self, Recovery, Sent};
use deltabadger::engine::polling::{self, FollowUp, PollFailure};
use deltabadger::engine::{amount, model, FixedClock};
use deltabadger::store::{self, Paths};
use deltabadger::venue::binance::Pairs;
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::kucoin::{self, KucoinVenue, URL};
use deltabadger::venue::{OrderStatus, Venue, ZERO_FILL_SETTLE};
use serde_json::{json, Value};

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
/// The case's three runs: one market buy a day, sized in base, each answered done with nothing dealt (ids replaced).
const CASE: [(&str, &str, &str); 3] = [("000000000000000000000c01", "2026-10-21T16:04:58.412Z", "0.00093"),
                                       ("000000000000000000000c02", "2026-10-22T16:04:58.731Z", "0.00094"),
                                       ("000000000000000000000c03", "2026-10-23T16:04:58.208Z", "0.00092")];
/// KuCoin's answer to GET /api/v1/orders/{id} for such an order.
fn answer(id: &str, created: &str, size: &str) -> Value {
    json!({ "id": id, "symbol": "BTC-EUR", "opType": "DEAL", "type": "market", "side": "buy", "price": "0", "size": size, "funds": "0",
            "dealFunds": "0", "dealSize": "0", "fee": "0", "feeCurrency": "EUR", "clientOid": format!("cl-{id}"), "stp": "", "stop": "",
            "isActive": false, "cancelExist": false, "createdAt": at(created).timestamp_millis() })
}
fn env(data: Value) -> Value { json!([{ "status": 200, "body": { "code": "200000", "data": data } }]) }

struct Kc { dir: tempfile::TempDir, o: store::Opened, s: seed::Seeded, bot: i64 }
fn install(status: i64) -> Kc {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    o.primary.execute_batch("UPDATE exchanges SET type = 'Exchanges::Kucoin', name = 'KuCoin'; UPDATE tickers SET ticker = 'BTC-EUR', base = 'BTC'").unwrap();
    o.primary.execute("UPDATE api_keys SET passphrase = ?1", [seed::cipher().encrypt("p")]).unwrap();
    for asset in [s.btc, s.quote] {
        o.primary.execute("INSERT INTO exchange_assets (exchange_id, asset_id, created_at, updated_at) VALUES (?1, ?2, '2026-01-01', '2026-01-01')", [s.exchange_id, asset]).unwrap();
    }
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00");
    spec.status = status;
    let bot = seed::insert_bot(&o.primary, &s, &spec);
    Kc { dir, o, s, bot }
}
impl Kc {
    fn waiting(&self, ext: &str, created: &str) -> i64 {
        let created = at(created).format("%Y-%m-%d %H:%M:%S").to_string();
        seed::insert_tx(&self.o.primary, &self.s, self.bot, &TxSpec { status: 0, external_status: Some(0), external_id: Some(ext.into()), order_type: 0,
            amount: Some("0.00093"), quote_amount: None, price: None, quote_amount_exec: None, amount_exec: None, created_at: created })
    }
    fn bot(&self) -> model::Bot { model::load_bot(&self.o.primary, self.bot).unwrap() }
    /// (external_status, amount_exec, quote_amount_exec) of the row.
    fn row(&self, ext: &str) -> (i64, f64, Option<f64>) {
        self.o.primary.query_row("SELECT external_status, coalesce(amount_exec, 0), quote_amount_exec FROM transactions WHERE external_id = ?1", [ext],
                                 |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap()
    }
    fn notes(&self) -> Vec<Value> {
        let mut s = self.o.primary.prepare("SELECT details FROM bot_activity_logs WHERE event = ?1 ORDER BY id").unwrap();
        s.query_map([polling::ZERO_FILL_EVENT], |r| r.get::<_, String>(0)).unwrap().map(|d| serde_json::from_str(&d.unwrap()).unwrap()).collect()
    }
}
fn pairs() -> Pairs {
    let mut p = Pairs::default();
    for code in ["BTC", "EUR"] { p.assets.insert(code.into(), code.into()); }
    p
}
fn kv(t: &ScriptedTransport) -> KucoinVenue<ScriptedTransport> {
    let creds = deltabadger::crypto::Credentials { key: "test-key".into(), secret: "dGVzdC1zZWNyZXQ=".into(), passphrase: Some("p".into()), redaction_values: vec![] };
    KucoinVenue::new(t.clone(), URL).with_identity(&creds).with_pairs(pairs())
}
fn gets(t: &ScriptedTransport, id: &str) -> usize { t.requests().iter().filter(|r| r.path == format!("/api/v1/orders/{id}")).count() }

// ---- A. D-KUCOIN-ZERO-FILL-DONE

#[test]
fn only_explicit_terminal_evidence_on_a_market_order_is_kept_and_it_settles_strictly_past_five_minutes() {
    let (id, created, size) = CASE[0];
    let base = answer(id, created, size);
    let parse = |raw: &Value| kucoin::parse_order(id, raw, &Pairs::default());
    let s = parse(&base).unwrap();
    let created = at(created);
    assert_eq!((s.status, s.zero_fill_at, s.settles_at()), (OrderStatus::Unknown, Some(created), Some(created + ZERO_FILL_SETTLE)), "parsed as Rails parses it");
    assert_eq!(s.clone().settled(created + Duration::minutes(4)).status, OrderStatus::Unknown, "young");
    assert_eq!(s.clone().settled(created + ZERO_FILL_SETTLE).status, OrderStatus::Unknown, "exactly five minutes is not older");
    let done = s.clone().settled(created + ZERO_FILL_SETTLE + Duration::milliseconds(1));
    assert_eq!((done.status, done.amount_exec.is_zero(), done.closed_empty()), (OrderStatus::Closed, true, true));
    assert_eq!(s.settled(created - Duration::hours(1)).status, OrderStatus::Unknown, "a createdAt in the future of the poll");
    // Anything short of that evidence keeps Rails' outcome: Unknown with nothing kept to settle.
    let edit = |f: &dyn Fn(&mut Value)| { let mut raw = base.clone(); f(&mut raw); raw };
    let unkept: Vec<(&str, Value)> = vec![
        ("isActive null", edit(&|r| r["isActive"] = Value::Null)), ("isActive absent", edit(&|r| { r.as_object_mut().unwrap().remove("isActive"); })),
        ("cancelExist null", edit(&|r| r["cancelExist"] = Value::Null)), ("cancelExist absent", edit(&|r| { r.as_object_mut().unwrap().remove("cancelExist"); })),
        ("dealSize null", edit(&|r| r["dealSize"] = Value::Null)), ("dealSize absent", edit(&|r| { r.as_object_mut().unwrap().remove("dealSize"); })),
        ("dealFunds null", edit(&|r| r["dealFunds"] = Value::Null)), ("dealFunds absent", edit(&|r| { r.as_object_mut().unwrap().remove("dealFunds"); })),
        ("funds dealt with no size", edit(&|r| r["dealFunds"] = json!("0.5"))), ("a limit order", edit(&|r| r["type"] = json!("limit"))),
        ("createdAt absent", edit(&|r| { r.as_object_mut().unwrap().remove("createdAt"); })), ("createdAt a string", edit(&|r| r["createdAt"] = json!("1792598698412"))),
        ("createdAt a fraction", edit(&|r| r["createdAt"] = json!(1.5))), ("createdAt garbage", edit(&|r| r["createdAt"] = json!("abc"))),
    ];
    for (label, raw) in unkept {
        let s = parse(&raw).unwrap();
        assert_eq!((s.status, s.zero_fill_at), (OrderStatus::Unknown, None), "{label}");
        assert_eq!(s.settled(created + Duration::days(1)).status, OrderStatus::Unknown, "{label}");
    }
    // Truthy flags, a fill, and unreadable amounts are as before.
    for (label, raw, want) in [("active", edit(&|r| r["isActive"] = json!(true)), OrderStatus::Open), ("active, truthy", edit(&|r| r["isActive"] = json!("yes")), OrderStatus::Open),
                               ("cancelled", edit(&|r| r["cancelExist"] = json!(true)), OrderStatus::Cancelled),
                               ("partially filled", edit(&|r| { r["dealSize"] = json!("0.0004"); r["dealFunds"] = json!("25.6"); }), OrderStatus::Closed)] {
        let s = parse(&raw).unwrap();
        assert_eq!((s.status, s.zero_fill_at, s.clone().settled(created + Duration::days(1)).closed_empty()), (want, None, false), "{label}");
    }
    for field in ["dealSize", "dealFunds"] { assert!(parse(&edit(&|r| r[field] = json!("garbage"))).is_err(), "{field}: an unreadable amount is never a zero"); }
}

#[tokio::test(flavor = "current_thread")]
async fn the_recorded_answers_settle_past_the_window_with_one_line_and_stay_unknown_inside_it() {
    for (id, created, size) in CASE {
        let k = install(1);
        k.waiting(id, created);
        let t = ScriptedTransport::from_script(&json!({ format!("GET /api/v1/orders/{id}"): env(answer(id, created, size)) }));
        let unknown = Err(PollFailure::General(format!("Order {id} status is unknown.")));
        for young in [Duration::minutes(4), ZERO_FILL_SETTLE] {
            assert_eq!(polling::sweep(&k.o.primary, &kv(&t), &k.bot(), at(created) + young).await, unknown, "{id}: inside the window the sweep raises, as Rails");
        }
        assert_eq!((k.row(id).0, k.notes().len()), (0, 0), "{id}: nothing written while young");
        let later = at(created) + ZERO_FILL_SETTLE + Duration::seconds(1);
        assert_eq!(polling::sweep(&k.o.primary, &kv(&t), &k.bot(), later).await, Ok(()));
        assert_eq!(k.row(id), (2, 0.0, Some(0.0)), "{id}: closed with nothing filled");
        assert_eq!(k.notes(), [json!({ "order_id": id, "exchange": "KuCoin" })]);
        // A later sweep has nothing to ask or raise; a refresh of the closed row, a follow-up, a restart's poll write no second line.
        let asked = gets(&t, id);
        assert_eq!(polling::sweep(&k.o.primary, &kv(&t), &k.bot(), later + Duration::days(1)).await, Ok(()));
        assert_eq!(gets(&t, id), asked, "no longer waiting");
        let tx: i64 = k.o.primary.query_row("SELECT id FROM transactions WHERE external_id = ?1", [id], |r| r.get(0)).unwrap();
        polling::refresh(&k.o.primary, &kv(&t), k.bot, tx, later + Duration::days(1)).await.unwrap();
        assert_eq!(gets(&t, id), asked + 1, "the refresh asked again");
        assert_eq!(polling::follow_up(&k.o.primary, &kv(&t), k.bot, tx, later + Duration::days(2)).await, Ok(FollowUp::Done));
        assert_eq!((k.row(id).0, k.notes().len()), (2, 1), "{id}: one line, whatever re-reads the row");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_follow_up_asks_again_while_young_and_an_old_limit_order_stays_unknown() {
    let (id, created, size) = CASE[1];
    let k = install(1);
    let tx = k.waiting(id, created);
    let t = ScriptedTransport::from_script(&json!({ format!("GET /api/v1/orders/{id}"): env(answer(id, created, size)) }));
    let settles = at(created) + ZERO_FILL_SETTLE;
    assert_eq!(polling::follow_up(&k.o.primary, &kv(&t), k.bot, tx, at(created) + Duration::seconds(5)).await, Ok(FollowUp::SettlesAt(settles)), "young: asked again, never Done");
    assert_eq!((k.row(id).0, k.notes().len()), (0, 0));
    assert_eq!(polling::follow_up(&k.o.primary, &kv(&t), k.bot, tx, settles + Duration::seconds(1)).await, Ok(FollowUp::Done));
    assert_eq!((k.row(id), k.notes().len()), ((2, 0.0, Some(0.0)), 1));
    // The window measures order age, which only a market order settles within: a limit order answering the same stays Unknown.
    let k = install(1);
    let tx = k.waiting(id, created);
    let mut limit = answer(id, created, size);
    limit["type"] = json!("limit");
    limit["price"] = json!("64000");
    let t = ScriptedTransport::from_script(&json!({ format!("GET /api/v1/orders/{id}"): env(limit) }));
    let unknown = || PollFailure::General(format!("Order {id} status is unknown."));
    assert_eq!(polling::follow_up(&k.o.primary, &kv(&t), k.bot, tx, at(created) + Duration::days(3)).await, Err(unknown()));
    assert_eq!(polling::sweep(&k.o.primary, &kv(&t), &k.bot(), at(created) + Duration::days(3)).await, Err(unknown()));
    assert_eq!((k.row(id).0, k.notes().len()), (0, 0));
}

struct Factory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for Factory {
    type V = KucoinVenue<ScriptedTransport>;
    fn for_bot(&self, _t: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V { kv(&self.0) }
}

/// Placed, then the bot stopped before the window: no sweep runs, and the follow-up chain alone settles the row. With the
/// venue's clock two hours ahead the eight asks run out first, and one more just past the window settles it.
#[tokio::test(flavor = "current_thread")]
async fn a_stopped_bots_follow_up_chain_settles_the_row_past_the_window() {
    for (skew, asks) in [(Duration::zero(), 5), (Duration::hours(2), 9)] {
        let (id, placed, size) = CASE[2];
        let k = install(2);
        k.waiting(id, placed);
        let created = at(placed) + skew;
        let t = ScriptedTransport::from_script(&json!({ format!("GET /api/v1/orders/{id}"): env(answer(id, &created.to_rfc3339(), size)) }));
        let paths = Paths::from_env(&|_| None, k.dir.path());
        let lock = deltabadger::lease::lock(&paths, at(placed)).unwrap();
        let mut e = deltabadger::engine::run::Engine::new(store::open(&paths).unwrap().primary, Factory(t.clone()), seed::cipher(), lock);
        let mut now = at(placed) + Duration::seconds(5);
        for _ in 0..500 {
            let wake = deltabadger::engine::run::step(&mut e, &FixedClock(now)).await.unwrap();
            if k.row(id).0 == 2 { break; }
            now = DateTime::from_timestamp_micros(wake).unwrap().max(now + Duration::milliseconds(1));
        }
        assert_eq!((k.row(id), k.notes().len()), ((2, 0.0, Some(0.0)), 1), "skew {skew}: settled by its follow-ups");
        assert!(now > created + ZERO_FILL_SETTLE, "skew {skew}: only past the window");
        assert_eq!(gets(&t, id), asks, "skew {skew}: the asks it took");
    }
}

// ---- B. D-ZERO-FILL-NOTE

/// A lost placement reply whose order the recovery finds done with nothing dealt: one line when it is old enough; a young one
/// is recorded waiting, and its own settlement writes the line later. A second recovery writes nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_recovered_zero_fill_writes_one_line_when_it_settles() {
    for young in [false, true] {
        let k = install(1);
        let bot = k.bot();
        let ticker = model::ticker_for(&k.o.primary, &bot).unwrap().unwrap();
        let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &deltabadger::ruby::BigDec::from_i64(60), &deltabadger::ruby::BigDec::from_i64(64_000),
                                                         &deltabadger::engine::venue_rules::KUCOIN).unwrap() else { panic!() };
        let placed = at(CASE[0].1);
        let sent = ScriptedTransport::from_script(&json!({ "POST /api/v1/orders": [{ "network": "post_send", "message": "read timeout" }] }));
        let intent = placement::begin(&k.o.primary, &bot, &plan, &FixedClock(placed)).unwrap();
        let identity = kv(&sent).placement_identity().unwrap();
        k.o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement.host', ?1, '$.rust_placement.key_fingerprint', ?2) WHERE id = ?3",
                            rusqlite::params![identity["host"].as_str(), identity["key_fingerprint"].as_str(), k.bot]).unwrap();
        assert!(matches!(placement::send_recorded(&k.o.primary, &kv(&sent), &bot, &intent, &FixedClock(placed)).await.unwrap(), Sent::Ambiguous(_)));
        let mut found = answer(CASE[0].0, CASE[0].1, CASE[0].2);
        found["clientOid"] = json!(intent.cl_ord_id);
        let t = ScriptedTransport::from_script(&json!({ format!("GET /api/v1/order/client-order/{}", intent.cl_ord_id): env(found.clone()),
                                                        format!("GET /api/v1/orders/{}", CASE[0].0): env(found) }));
        let now = placed + if young { Duration::minutes(2) } else { Duration::minutes(10) };
        assert!(matches!(placement::recover(&k.o.primary, &kv(&t), &k.bot(), &FixedClock(now)).await.unwrap(), Recovery::Recorded(_)), "young {young}");
        assert_eq!((k.row(CASE[0].0).0, k.notes().len()), if young { (0, 0) } else { (2, 1) }, "young {young}");
        assert!(matches!(placement::recover(&k.o.primary, &kv(&t), &k.bot(), &FixedClock(now)).await.unwrap(), Recovery::NoIntent));
        polling::sweep(&k.o.primary, &kv(&t), &k.bot(), placed + Duration::minutes(10)).await.unwrap();
        assert_eq!((k.row(CASE[0].0).0, k.notes().len()), (2, 1), "young {young}: one line in all");
    }
}

/// The line is written in the settlement's own transaction: a settlement that rolls back leaves no line and the row waiting,
/// and the retry that commits leaves exactly one.
#[tokio::test(flavor = "current_thread")]
async fn a_rolled_back_settlement_leaves_no_line_and_its_retry_exactly_one() {
    let (id, created, size) = CASE[0];
    let k = install(1);
    k.waiting(id, created);
    k.o.primary.execute_batch("CREATE TABLE fail_notes (on_ INTEGER); INSERT INTO fail_notes VALUES (1);
        CREATE TRIGGER fail_note BEFORE INSERT ON bot_activity_logs WHEN (SELECT on_ FROM fail_notes) = 1 BEGIN SELECT RAISE(ABORT, 'disk full'); END;").unwrap();
    let t = ScriptedTransport::from_script(&json!({ format!("GET /api/v1/orders/{id}"): env(answer(id, created, size)) }));
    let later = at(created) + Duration::minutes(6);
    assert!(polling::sweep(&k.o.primary, &kv(&t), &k.bot(), later).await.is_err());
    assert_eq!((k.row(id).0, k.notes().len()), (0, 0), "rolled back whole");
    k.o.primary.execute("UPDATE fail_notes SET on_ = 0", []).unwrap();
    polling::sweep(&k.o.primary, &kv(&t), &k.bot(), later).await.unwrap();
    assert_eq!((k.row(id).0, k.notes().len()), (2, 1));
}

/// With the zero fill settled the run succeeds again, so Bot::Fundable's check after it runs as Rails' does after any
/// successful run: a low balance stamps the funds marker (its unchanged once-a-day budget).
#[tokio::test(flavor = "current_thread")]
async fn after_the_settlement_a_successful_run_with_a_low_balance_stamps_the_funds_marker() {
    let (id, created, size) = CASE[0];
    let k = install(1);
    k.waiting(id, created);
    let book = env(json!({ "time": 1792598698412u64, "sequence": "1", "bids": [["63999.9", "1.5"]], "asks": [["64000.1", "0.3"]] }));
    let accounts = env(json!([{ "id": "a1", "currency": "BTC", "type": "trade", "balance": "0.5", "available": "0.5", "holds": "0" },
                              { "id": "a2", "currency": "EUR", "type": "trade", "balance": "1", "available": "1", "holds": "0" }]));
    let t = ScriptedTransport::from_script(&json!({ format!("GET /api/v1/orders/{id}"): env(answer(id, created, size)),
        "GET /api/v1/market/orderbook/level2_20": book, "GET /api/v1/market/allTickers": env(json!({ "time": 1, "ticker": [{ "symbol": "BTC-EUR", "last": "64000" }] })),
        "POST /api/v1/orders": env(json!({ "orderId": "000000000000000000000c04" })), "GET /api/v1/accounts": accounts,
        "GET /api/v1/fills": [{ "status": 200, "body": { "code": "200000", "data": { "currentPage": 1, "pageSize": 500, "totalNum": 0, "totalPage": 0, "items": [] } } }] }));
    let now = at(created) + Duration::days(1);
    deltabadger::engine::tick::tick(&k.o.primary, &kv(&t), k.bot, &FixedClock(now), &mut Default::default()).await.unwrap();
    assert_eq!(k.row(id).0, 2, "the sweep settled the zero fill");
    assert!(t.requests().iter().any(|r| r.method == "POST"), "the run placed its order");
    let (stamped, marker): (Option<String>, Option<String>) = k.o.primary.query_row(
        &format!("SELECT last_end_of_funds_notification, json_extract(transient_data, '$.{}') FROM bots WHERE id = ?1", deltabadger::engine::tick::FUNDS_MAIL_PENDING),
        [k.bot], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(stamped.is_some() && marker.is_some(), "Fundable stamped the low balance: {stamped:?} {marker:?}");
}

// ---- C. D-LOW-FUNDS-STATUS

use deltabadger::jobs::{state, Cx, Db, Job, Outcome, Scheduler};
use deltabadger::sync::jobs::BALANCE_SYNC;
use std::sync::{Arc, Mutex};

const NOW: &str = "2026-10-23T16:10:00Z";
fn creds() -> deltabadger::crypto::Credentials {
    deltabadger::crypto::Credentials { key: "test-key".into(), secret: "dGVzdC1zZWNyZXQ=".into(), passphrase: Some("p".into()), redaction_values: vec![] }
}
fn accounts(eur: Option<&str>) -> Value {
    let mut rows = vec![json!({ "id": "a1", "currency": "BTC", "type": "trade", "balance": "0.5", "available": "0.5", "holds": "0" })];
    if let Some(eur) = eur { rows.push(json!({ "id": "a2", "currency": "EUR", "type": "trade", "balance": eur, "available": eur, "holds": "0" })); }
    json!({ "code": "200000", "data": rows })
}
impl Kc {
    fn db(&self) -> Db { Db::new(store::open(&Paths::from_env(&|_| None, self.dir.path())).unwrap().primary, seed::cipher()) }
    /// One complete KuCoin balance sync of the bot's key, EUR as given (absent: KuCoin dropped a zero balance).
    async fn synced(&self, eur: Option<&str>) {
        let t = ScriptedTransport::from_script(&json!({ "GET /api/v1/accounts": [{ "status": 200, "body": accounts(eur) }] }));
        let done = deltabadger::sync::balances::sync_kucoin(&self.db(), t, &deltabadger::sync::balances::NoPrices, self.s.api_key_id, &creds(), &FixedClock(at(NOW))).await;
        assert!(matches!(done, Ok(Ok(_))), "{:?}", done.map(|d| d.err().map(|f| f.error)));
    }
    fn app(&self) -> deltabadger::web::App { common::web::app(self.dir.path(), common::web::SECRET, common::web::TestClock::at(NOW)) }
    fn bar(&self) -> String { bar_at(self.dir.path(), self.bot) }
}
/// The bot's status bar as the stream redraws it after a run.
fn bar_at(dir: &std::path::Path, bot: i64) -> String {
    let c = store::open(&Paths::from_env(&|_| None, dir)).unwrap().primary;
    let app = common::web::app(dir, common::web::SECRET, common::web::TestClock::at(NOW));
    let messages = deltabadger::web::bot::live::render(&c, &app, &EngineEvent::TickEnded { bot_id: bot }).unwrap();
    messages.into_iter().map(|(_, html)| html).find(|html| html.contains("target=\"status_bar_")).expect("a status bar")
}
/// The countdown's text before its timer, and whether it reads green.
fn prefix(bar: &str) -> (String, bool) {
    let div = bar.split("data-controller=\"countdown\"").next().unwrap();
    let green = div.rsplit("<div").next().unwrap().contains("text-success");
    let after = bar.split("data-marquee-target=\"text\">").last().unwrap();
    (after.split("<div style").next().unwrap().trim().to_string(), green)
}

#[tokio::test(flavor = "current_thread")]
async fn low_funds_shows_only_from_a_complete_current_sync() {
    let low = ("Low funds".to_string(), false);
    let none = (String::new(), true);
    // Never synced: nothing to say.
    let k = install(1);
    assert_eq!(prefix(&k.bar()), none, "never synced");
    for (eur, want) in [(Some("1"), &low), (Some("1000"), &none), (None, &low)] {
        let k = install(1);
        k.synced(eur).await;
        let rows: i64 = k.o.primary.query_row("SELECT count(*) FROM account_balances WHERE asset_id = ?1", [k.s.quote], |r| r.get(0)).unwrap();
        assert_eq!(rows, i64::from(eur.is_some()), "EUR {eur:?}: a zero balance has no row");
        assert_eq!(&prefix(&k.bar()), want, "EUR {eur:?}: buffer 60 / week × 3 days = 25.71");
    }
    // Interrupted: before its writes (the stamp turns incomplete) or before its fetch (the job's marker), or a replaced key.
    let k = install(1);
    k.synced(Some("1")).await;
    let origin = deltabadger::sync::cache::balance_origin_key(k.s.user_id, k.s.exchange_id);
    k.o.primary.execute("UPDATE app_configs SET value = json_set(value, '$.complete', json('false')) WHERE key = ?1", [&origin]).unwrap();
    assert_eq!(prefix(&k.bar()), none, "write phase interrupted");
    let k = install(1);
    k.synced(Some("1")).await;
    state::mark_incomplete(&k.o.primary, BALANCE_SYNC, Some(&k.s.api_key_id.to_string()), at(NOW)).unwrap();
    assert_eq!(prefix(&k.bar()), none, "fetch phase interrupted");
    let k = install(1);
    k.synced(Some("1")).await;
    k.o.primary.execute("UPDATE api_keys SET key = ?1", [seed::cipher().encrypt("replaced-key")]).unwrap();
    assert_eq!(prefix(&k.bar()), none, "replaced key");
}

#[tokio::test(flavor = "current_thread")]
async fn retrying_keeps_its_explanation_market_closed_comes_first_and_a_seller_never_shows_it() {
    let k = install(5);
    k.synced(Some("1")).await;
    assert_eq!(prefix(&k.bar()), ("Next try in".to_string(), false), "retrying: its own words only");
    let k = install(1);
    k.synced(Some("1")).await;
    k.o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.waiting_for_market_open', json('true')) WHERE id = ?1", [k.bot]).unwrap();
    model::log_activity(&k.o.primary, k.bot, "market_closed", model::Level::Info, json!({ "next_market_open_at": "2026-10-26T13:30:00.000Z" }), at(NOW)).unwrap();
    assert_eq!(prefix(&k.bar()), ("The market is closed now. · Low funds".to_string(), false), "bot.status.market_closed, then Low funds");
    let k = install(1);
    k.o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.direction', 'selling') WHERE id = ?1", [k.bot]).unwrap();
    k.synced(Some("1")).await;
    assert!(!k.bar().contains("Low funds"), "a seller spends base");
}

#[tokio::test(flavor = "current_thread")]
async fn the_bot_page_and_its_tile_say_it_too() {
    let k = install(1);
    k.synced(Some("1")).await;
    let app = k.app();
    for (key, value) in [("market_data_provider", "deltabadger"), ("market_data_url", "http://127.0.0.1:1"), ("market_data_token", "synthetic-token")] {
        deltabadger::app_config::set(&k.o.primary, &app.cipher, key, value, app.now()).unwrap();
    }
    k.o.primary.execute("UPDATE users SET confirmed_at = '2026-01-01 00:00:00'", []).unwrap();
    seed::insert_bot(&k.o.primary, &k.s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")); // a dashboard of one bot opens the bot
    model::log_activity(&k.o.primary, k.bot, polling::ZERO_FILL_EVENT, model::Level::Info, json!({ "order_id": CASE[0].0, "exchange": "KuCoin" }), at(NOW)).unwrap();
    let hash: String = k.o.primary.query_row("SELECT encrypted_password FROM users WHERE id = ?1", [k.s.user_id], |r| r.get(0)).unwrap();
    use deltabadger::web::session;
    let data = session::SessionData { user: Some((k.s.user_id, hash.chars().take(29).collect())), csrf: Some(deltabadger::web::csrf::new_token()), ..Default::default() };
    for path in [format!("/bots/{}", k.bot), "/bots".to_string()] {
        let mut browser = common::web::Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None };
        let page = browser.send(&app, "GET", &path, None, common::web::Csrf::None, &[]).await;
        assert_eq!(page.status, 200, "{path}: {:?}", page.header("location"));
        let bar = page.body.split(&format!("id=\"status_bar_bots_dca_multi_asset_{}\"", k.bot)).nth(1).unwrap_or_else(|| panic!("{path}: no status bar"));
        assert_eq!(prefix(bar.split("</div>\n</div>").next().unwrap()), ("Low funds".to_string(), false), "{path}");
    }
    // D-ZERO-FILL-NOTE in the bot's activity, as its orders frame reads it.
    let mut browser = common::web::Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None };
    let feed = browser.send(&app, "GET", &format!("/bots/{}.turbo_stream", k.bot), None, common::web::Csrf::None, &[("Turbo-Frame", "orders_pagination")]).await;
    assert!(feed.body.contains(&format!("Order {} closed on KuCoin with nothing filled.", CASE[0].0)), "{}", feed.body);
}

/// The nightly balance job as the scheduler runs it, with the venue's read substituted: BalanceSync itself marks the job
/// incomplete and tells the web before it fetches (its refused fetch writes nothing), then sync_kucoin writes the case's low
/// balance completely, and the run ends Done under BalanceSync's own credential version, so the scheduler records it.
struct LowSync { key: i64 }
impl Job for LowSync {
    fn spec(&self) -> deltabadger::jobs::Spec { deltabadger::sync::jobs::BalanceSync::new(NoVenues, std::rc::Rc::new(deltabadger::sync::balances::NoPrices), self.key).spec() }
    fn run<'a>(&'a self, cx: Cx<'a>, wakes: Vec<deltabadger::jobs::Wake>) -> deltabadger::jobs::JobFuture<'a> { Box::pin(async move { self.run_attributed(cx, wakes).await.value }) }
    fn run_attributed<'a>(&'a self, cx: Cx<'a>, wakes: Vec<deltabadger::jobs::Wake>) -> deltabadger::jobs::AttributedJobFuture<'a> {
        Box::pin(async move {
            let (db, clock) = (cx.db.clone(), cx.clock);
            let marked = deltabadger::sync::jobs::BalanceSync::new(NoVenues, std::rc::Rc::new(deltabadger::sync::balances::NoPrices), self.key).run_attributed(cx, wakes).await;
            assert!(matches!(marked.value, Outcome::Failed(_)), "the substituted fetch is refused: {:?}", marked.value);
            let t = ScriptedTransport::from_script(&json!({ "GET /api/v1/accounts": [{ "status": 200, "body": accounts(Some("1")) }] }));
            let done = deltabadger::sync::balances::sync_kucoin(&db, t, &deltabadger::sync::balances::NoPrices, self.key, &creds(), clock).await;
            assert!(matches!(done, Ok(Ok(_))));
            marked.map(|_| Outcome::Done)
        })
    }
}
struct Idle(&'static str, Option<String>);
impl Job for Idle {
    fn spec(&self) -> deltabadger::jobs::Spec {
        deltabadger::jobs::Spec { name: self.0, scope: self.1.clone(), schedule: None, jitter: deltabadger::jobs::schedule::Jitter::NONE,
                                  retry: deltabadger::jobs::Retry::None, deadline: deltabadger::jobs::DEADLINE }
    }
    fn run<'a>(&'a self, _: Cx<'a>, _: Vec<deltabadger::jobs::Wake>) -> deltabadger::jobs::JobFuture<'a> { Box::pin(async { Outcome::Done }) }
}
struct NoVenues;
impl deltabadger::sync::jobs::Connect for NoVenues {
    type T = deltabadger::venue::http::ReqwestTransport;
    fn connect(&self, c: &deltabadger::crypto::Credentials) -> deltabadger::venue::alpaca::AlpacaVenue<Self::T> {
        deltabadger::sync::jobs::Connect::connect(&deltabadger::venue::alpaca::LiveFactory::new(), c)
    }
}

/// The stream follows the evidence, each change told to the web (Notifications::balances_changed) after its commit: shown
/// after a complete low sync; hidden once the next balance job marks itself incomplete before it fetches; shown again once
/// the scheduler records that job's success.
#[tokio::test(flavor = "current_thread")]
async fn the_status_bar_follows_the_balance_jobs_from_the_scheduler_to_the_stream() {
    let k = install(1);
    k.synced(Some("1")).await;
    assert_eq!(prefix(&k.bar()).0, "Low funds");
    let told: Arc<Mutex<Vec<(i64, bool, String)>>> = Default::default();
    let (dir, bot, heard) = (k.dir.path().to_path_buf(), k.bot, told.clone());
    let notifications = deltabadger::jobs::notifications::Notifications::new(|_, _| Ok(())).with_balances(move |key| {
        let c = store::open(&Paths::from_env(&|_| None, &dir)).unwrap().primary;
        let incomplete = state::read(&c, BALANCE_SYNC, Some(&key.to_string())).unwrap().incomplete_since.is_some();
        heard.lock().unwrap().push((key, incomplete, prefix(&bar_at(&dir, bot)).0));
    });
    let c = store::open(&Paths::from_env(&|_| None, k.dir.path())).unwrap().primary;
    // The failed fetch wakes the user's tracker, as BalanceSync does: a job that does nothing here.
    let scheduler = Scheduler::new(c, seed::cipher(), vec![Box::new(LowSync { key: k.s.api_key_id })], None).with_notifications(notifications)
        .with_resolver(Box::new(|name, scope| Ok(Box::new(Idle(Box::leak(name.to_string().into_boxed_str()), scope.map(str::to_string))) as Box<dyn Job>)));
    scheduler.wakers().wake(BALANCE_SYNC, Some(&k.s.api_key_id.to_string()), None);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let clock = FixedClock(at(NOW));
    let watch = told.clone();
    let (ended, ()) = tokio::join!(scheduler.run(stopped, &clock), async {
        for _ in 0..2000 { if watch.lock().unwrap().len() >= 2 { break; } tokio::time::sleep(std::time::Duration::from_millis(5)).await; }
        let _ = stop.send(true);
    });
    ended.expect("the scheduler ran until stopped");
    let key = k.s.api_key_id;
    assert_eq!(*told.lock().unwrap(), [(key, true, String::new()), (key, false, "Low funds".to_string())], "hidden during the fetch, shown after the recorded success");
    assert_eq!(prefix(&k.bar()), ("Low funds".to_string(), false));
}

/// The balance check reads the key the sync scheduler reads (sync::reading_keys_of): a trading key whose last sync lacked
/// a permission gives way to the read-only key, so that key's synced balances are the current ones.
#[test]
fn the_reading_key_is_the_schedulers_pick() {
    let (_dir, opened, seed) = common::install();
    let c = opened.primary;
    c.execute("INSERT INTO exchanges(type,name,created_at,updated_at) VALUES('Exchanges::Bitget','Bitget','2026-01-01','2026-01-01')", []).unwrap();
    let exchange = c.last_insert_rowid();
    let key = |kind: i64, error: Option<&str>| {
        c.execute("INSERT INTO api_keys(user_id,exchange_id,key,secret,key_type,status,last_sync_error,created_at,updated_at) VALUES(?1,?2,'k','s',?3,1,?4,'2026-01-01','2026-01-01')",
                  rusqlite::params![seed.user_id, exchange, kind, error]).unwrap();
        c.last_insert_rowid()
    };
    let trading = key(0, None);
    let read_only = key(2, None);
    assert_eq!(deltabadger::sync::cache::reading_key(&c, seed.user_id, exchange).unwrap(), Some(trading));
    c.execute("UPDATE api_keys SET last_sync_error='Incorrect permissions' WHERE id=?1", [trading]).unwrap();
    assert_eq!(deltabadger::sync::cache::reading_key(&c, seed.user_id, exchange).unwrap(), Some(read_only));
}
