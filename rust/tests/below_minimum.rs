//! D-BELOW-MIN-REJECTION (plan OM3b): a venue's definitive "order too small" answer to a scheduled DCA buy leg is the
//! pre-check's skip (tick::record_skipped), never a failed row. Every case runs from a scripted HTTP answer through the real
//! adapter to the tick's outcome: Alpaca directly; the other venues' AddOrder through `Through`, which reads prices and
//! balances from the seeded Alpaca account and sends the order through the adapter under test, under that adapter's rules.
//! Out of scope, unchanged: sells, signal orders and REBALANCE legs (placement::skips_below_minimum). Rails-free: the Rails
//! side of the divergence is tests/parity.rs's `rejected_below_minimum` scenarios.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::scripted::{accepted, not_found, script, venue, POST_SEND};
use common::seed::{self, BotSpec};
use deltabadger::engine::model::{self, Ticker};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::venue_rules::{self, VenueRules};
use deltabadger::engine::{amount, placement, FixedClock};
use deltabadger::enums::BotStatus;
use deltabadger::ruby::BigDec;
use deltabadger::store;
use deltabadger::venue::alpaca::AlpacaVenue;
use deltabadger::venue::binance::{self, BinanceVenue, Pair, Pairs};
use deltabadger::venue::bitvavo::{self, BitvavoVenue};
use deltabadger::venue::bybit::{self, BybitVenue};
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::hyperliquid::{self, HyperliquidVenue};
use deltabadger::venue::kraken::KrakenVenue;
use deltabadger::venue::mexc::{self, MexcVenue};
use deltabadger::venue::{NewOrder, OrderState, PriceSide, Venue, VenueError};
use serde_json::{json, Value};
use std::cell::RefCell;

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const T0: &str = "2026-09-01T10:00:00.5Z"; // a weekly bot started 2026-09-01 10:00: one interval owed
const T1: &str = "2026-09-08T10:00:00.5Z"; // its next checkpoint
fn one<T: rusqlite::types::FromSql>(o: &store::Opened, sql: &str) -> T { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }
fn http(status: u16, body: Value) -> Value { json!({ "status": status, "body": body }) }
const ALPACA_MIN: &str = "cost basis must be >= minimal amount of order 10";
fn alpaca_min() -> Value { http(403, json!({ "code": 40310000, "message": ALPACA_MIN })) }

/// The seeded Alpaca account's reads; `adapter`'s AddOrder and rules. `hook` runs once, as the order goes out.
struct Through<A> { data: AlpacaVenue<ScriptedTransport>, adapter: A, hook: RefCell<Option<Box<dyn FnOnce()>>> }
impl<A: Venue> Venue for Through<A> {
    fn rules(&self) -> &'static VenueRules { self.adapter.rules() }
    async fn price(&self, t: &Ticker, s: PriceSide) -> Result<BigDec, VenueError> { self.data.price(t, s).await }
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        let hook = self.hook.borrow_mut().take();
        if let Some(h) = hook { h(); }
        self.adapter.add_order(o).await
    }
    async fn orders(&self, ids: &[String]) -> Result<Vec<OrderState>, VenueError> { self.data.orders(ids).await }
    async fn orders_identified<F>(&self, ids: &[String], f: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> { self.data.orders_identified(ids, f).await }
    async fn order_identified<F>(&self, id: &str, f: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> { self.data.order_identified(id, f).await }
    async fn order_by_client_id(&self, id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> { self.data.order_by_client_id(id, symbol, since).await }
    async fn fills_from_trades(&self, ids: &[String], since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> { self.data.fills_from_trades(ids, since).await }
    async fn balance(&self, s: &str, crypto: bool) -> Result<BigDec, VenueError> { self.data.balance(s, crypto).await }
    async fn settled_quote_balance<F>(&self, t: &Ticker, catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<deltabadger::sync::balances::Catalog, VenueError> { self.data.settled_quote_balance(t, catalog).await }
    async fn positions(&self) -> Result<std::collections::HashMap<String, BigDec>, VenueError> { self.data.positions().await }
}
fn through<A: Venue>(adapter: A) -> Through<A> { Through { data: venue(&script(json!({}))), adapter, hook: RefCell::new(None) } }

/// A weekly basket of `quote` USD over BTC, ETH (and SOL) at these weights.
fn basket_bot(quote: f64, weights: &[f64], spec: impl FnOnce(BotSpec) -> BotSpec) -> (tempfile::TempDir, store::Opened, i64, Vec<i64>) {
    let (d, o, s) = common::install_alpaca();
    let (eth, sol) = seed::add_eth_sol(&o.primary, &s);
    let assets = [s.btc, eth, sol];
    let pairs: Vec<(i64, f64)> = weights.iter().enumerate().map(|(i, w)| (assets[i], *w)).collect();
    let id = seed::insert_bot(&o.primary, &s, &spec(BotSpec::weekly(quote, "2026-09-01 10:00:00").weights(&pairs)));
    (d, o, id, assets.to_vec())
}
async fn run<V: Venue>(o: &store::Opened, v: &V, id: i64, when: &str) -> TickOutcome {
    tick::tick(&o.primary, v, id, &FixedClock(at(when)), &mut Attempts::default()).await.unwrap()
}
fn rows(o: &store::Opened) -> Vec<(i64, Option<String>)> {
    o.primary.prepare("SELECT status, external_id FROM transactions ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
fn events(o: &store::Opened) -> Vec<String> {
    o.primary.prepare("SELECT event FROM bot_activity_logs ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

#[derive(Debug, PartialEq)]
enum Want { Skipped, Failed, Ambiguous }

/// A one-asset $60 bot's first tick, its AddOrder answered through `v`'s adapter. Returns what the tick did.
async fn first_tick<A: Venue>(v: &Through<A>, prep: &str) -> (Want, TickOutcome, Vec<(i64, Option<String>)>, Vec<String>) {
    let (_d, o, id, _) = basket_bot(60.0, &[1.0], |b| b);
    if !prep.is_empty() { o.primary.execute(prep, []).unwrap(); }
    let out = run(&o, v, id, T0).await;
    let (rows, events) = (rows(&o), events(&o));
    let intent = model::load_bot(&o.primary, id).unwrap().rust_placement().is_some();
    let want = match (&rows[..], intent) {
        ([(2, None)], false) => Want::Skipped,
        ([(1, None)], false) => Want::Failed,
        ([], true) => Want::Ambiguous,
        other => panic!("unexpected end state {other:?} after {out:?}; events {events:?}"),
    };
    (want, out, rows, events)
}

/// Asserts each answer's verdict; a skip is a skipped row and its order_skipped line, nothing failed, nothing kept.
async fn cases<A: Venue>(name: &str, make: impl Fn(Value) -> Through<A>, prep: &str, answers: Vec<(Value, Want)>) {
    for (answer, want) in answers {
        let v = make(answer.clone());
        let (got, out, _, events) = first_tick(&v, prep).await;
        assert_eq!(got, want, "{name} {answer}: {out:?} {events:?}");
        if want == Want::Skipped {
            assert!(matches!(out, TickOutcome::Done { placed: false }), "{name} {answer}: {out:?}");
            assert_eq!(events.iter().filter(|e| *e == "order_skipped").count(), 1, "{name} {answer}: {events:?}");
            assert!(!events.iter().any(|e| e == "execution_failed"), "{name} {answer}: {events:?}");
        }
    }
}

fn binance_like(rules: &'static VenueRules) -> impl Fn(Value) -> Through<BinanceVenue<ScriptedTransport>> {
    move |answer| through(BinanceVenue::new(ScriptedTransport::from_script(&json!({ "POST /api/v3/order": [answer] })), binance::URL, "s3cr3t").with_rules(rules))
}

#[tokio::test(flavor = "current_thread")]
async fn binance_and_binance_us_min_notional_is_a_skip_and_nothing_else_is() {
    for rules in [&venue_rules::BINANCE, &venue_rules::BINANCE_US] {
        cases(rules.name, binance_like(rules), "", vec![
            (http(400, json!({ "code": -1013, "msg": "Filter failure: MIN_NOTIONAL" })), Want::Skipped),
            // NOTIONAL's range also has a maximum; LOT_SIZE is too high, too low or off the step.
            (http(400, json!({ "code": -1013, "msg": "Filter failure: NOTIONAL" })), Want::Failed),
            (http(400, json!({ "code": -1013, "msg": "Filter failure: LOT_SIZE" })), Want::Failed),
            (http(400, json!({ "code": -2010, "msg": "Account has insufficient balance for requested action." })), Want::Failed),
            // The minimum's text under another code, or under no code at all, proves nothing.
            (http(400, json!({ "code": -1100, "msg": "Filter failure: MIN_NOTIONAL" })), Want::Failed),
            (http(400, json!({ "msg": "Filter failure: MIN_NOTIONAL" })), Want::Failed),
            // A 5xx may have placed the order, whatever it says.
            (http(503, json!({ "code": -1013, "msg": "Filter failure: MIN_NOTIONAL" })), Want::Ambiguous),
        ]).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mexc_30002_is_a_skip_and_nothing_else_is() {
    let make = |answer: Value| through(MexcVenue::new(ScriptedTransport::from_script(&json!({ "POST /api/v3/order": [answer] })), mexc::URL, "s3cr3t"));
    cases("MEXC", make, "", vec![
        (http(400, json!({ "code": 30002, "msg": "The minimum transaction volume cannot be less than :1USDT" })), Want::Skipped),
        (http(400, json!({ "code": 30003, "msg": "The maximum transaction volume cannot be greater than :2000000USDT" })), Want::Failed),
        (http(400, json!({ "code": 30004, "msg": "Insufficient position" })), Want::Failed),
        (http(400, json!({ "msg": "The minimum transaction volume cannot be less than :1USDT" })), Want::Failed),
        (http(500, json!({ "code": 30002, "msg": "The minimum transaction volume cannot be less than :1USDT" })), Want::Ambiguous),
    ]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn bybit_lower_limits_are_a_skip_and_nothing_else_is() {
    let make = |answer: Value| through(BybitVenue::new(ScriptedTransport::from_script(&json!({ format!("POST {}", bybit::PLACE_PATH): [answer] })), bybit::URL));
    let code = |c: i64, m: &str| http(200, json!({ "retCode": c, "retMsg": m, "result": {} }));
    cases("Bybit", make, "", vec![
        (code(170140, "Order value exceeded lower limit"), Want::Skipped),
        (code(170136, "Order quantity lower than the minimum."), Want::Skipped),
        (http(400, json!({ "retCode": 170140, "retMsg": "Order value exceeded lower limit" })), Want::Skipped),
        (code(170137, "Order volume decimal too long"), Want::Failed),
        (code(170131, "Insufficient balance."), Want::Failed),
        (code(170141, "Order value exceeded lower limit"), Want::Failed), // the text under another code
        // 10016 is Bybit's server error: the order may stand.
        (code(10016, "Order value exceeded lower limit"), Want::Ambiguous),
    ]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn bitvavo_217_and_212_are_a_skip_and_nothing_else_is() {
    let make = |answer: Value| through(BitvavoVenue::new(ScriptedTransport::from_script(&json!({ "POST /v2/order": [answer] })), bitvavo::URL));
    let err = |status: u16, code: i64, m: &str| http(status, json!({ "errorCode": code, "error": m }));
    cases("Bitvavo", make, "", vec![
        (err(400, 217, "Minimum order size in quote currency is 5 EUR."), Want::Skipped),
        (err(400, 212, "amount is below the minimum for this market."), Want::Skipped),
        (err(400, 210, "amount exceeds the maximum for this market."), Want::Failed),
        (err(400, 216, "You do not have sufficient balance to complete this operation."), Want::Failed),
        (http(400, json!({ "error": "Minimum order size in quote currency is 5 EUR." })), Want::Failed),
        (err(500, 217, "Minimum order size in quote currency is 5 EUR."), Want::Ambiguous),
    ]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn kraken_order_minimum_is_a_skip_and_nothing_else_is() {
    let make = |answer: Value| through(KrakenVenue::new(ScriptedTransport::from_script(&json!({ "POST /0/private/AddOrder": [http(200, answer)] }))));
    cases("Kraken", make, "", vec![
        (json!({ "error": ["EOrder:Order minimum not met"] }), Want::Skipped),
        (json!({ "error": ["EOrder:Orders limit exceeded"] }), Want::Failed),
        (json!({ "error": ["EGeneral:Invalid arguments:volume"] }), Want::Failed),
        // Mixed: one message that is not the minimum keeps it a failure.
        (json!({ "error": ["EOrder:Order minimum not met", "EOrder:Insufficient funds"] }), Want::Failed),
        // add_outcome_unknown first: Kraken may have placed it.
        (json!({ "error": ["EGeneral:Internal error", "EOrder:Order minimum not met"] }), Want::Ambiguous),
    ]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn hyperliquid_minimum_value_is_a_skip_and_nothing_else_is() {
    let pairs = || Pairs { by_symbol: [("BTC/USD".to_string(), Pair { base: "BTC".into(), quote: "USD".into(), price_decimals: 2 })].into(), assets: [("USD".to_string(), "USD".to_string())].into() };
    let meta = http(200, json!({ "tokens": [{ "name": "BTC", "index": 0 }, { "name": "USD", "index": 1 }], "universe": [{ "name": "@1", "tokens": [0, 1], "index": 1 }] }));
    let make = |answer: Value| through(HyperliquidVenue::new(ScriptedTransport::from_script(&json!({ "POST /info spotMeta": [meta.clone()], "POST /exchange": [http(200, answer)] })), hyperliquid::URL, "0xACCOUNT").with_pairs(pairs()));
    let status = |e: &str| json!({ "status": "ok", "response": { "type": "order", "data": { "statuses": [{ "error": e }] } } });
    // Hyperliquid prices to 5 significant figures within 8 − base_decimals places.
    cases("Hyperliquid", make, "UPDATE tickers SET base_decimals = 5", vec![
        (status("Order must have minimum value of $10."), Want::Skipped),
        (status("Order must have minimum value of 10 USD."), Want::Skipped),
        (status("Order has invalid size."), Want::Failed),
        (status("Insufficient spot balance asset=10001"), Want::Failed),
        // The venue's `err`: refused, but not in the status words the table names.
        (json!({ "status": "err", "response": "Order must have minimum value of $10." }), Want::Failed),
        (json!({ "status": "unknown", "response": "Order must have minimum value of $10." }), Want::Ambiguous),
    ]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn alpaca_minimal_amount_is_a_skip_and_nothing_else_is() {
    for (answer, want) in [
        (alpaca_min(), Want::Skipped),
        (http(403, json!({ "code": 40310000, "message": "insufficient buying power" })), Want::Failed),
        (http(422, json!({ "code": 40010001, "message": "qty must be > 0" })), Want::Failed),
        (http(500, json!({ "message": ALPACA_MIN })), Want::Ambiguous),
    ] {
        let (_d, o, id, _) = basket_bot(60.0, &[1.0], |b| b);
        let t = script(json!({ "POST /v2/orders": [answer.clone()] }));
        run(&o, &venue(&t), id, T0).await;
        let intent = model::load_bot(&o.primary, id).unwrap().rust_placement().is_some();
        let got = match (&rows(&o)[..], intent) { ([(2, None)], false) => Want::Skipped, ([(1, None)], false) => Want::Failed, ([], true) => Want::Ambiguous, other => panic!("{other:?}") };
        assert_eq!(got, want, "{answer}");
        assert_eq!(t.posted_orders().len(), 1, "{answer}: sent once");
    }
}

#[test]
fn the_classifier_matches_only_the_cited_answers() {
    let m = |s: &str| vec![s.to_string()];
    assert!(placement::below_minimum("Exchanges::Mexc", Some("30002"), &m("anything")));
    assert!(!placement::below_minimum("Exchanges::Mexc", None, &m("The minimum transaction volume cannot be less than :")));
    assert!(!placement::below_minimum("Exchanges::Binance", Some("30002"), &m("Filter failure: MIN_NOTIONAL")), "MEXC's code is not Binance's");
    assert!(!placement::below_minimum("Exchanges::Kucoin", None, &m("Order size below the minimum requirement.")), "uncited venues are never classified");
    assert!(!placement::below_minimum("Exchanges::Alpaca", None, &[]), "no message proves nothing");
    assert!(placement::below_minimum("Exchanges::Alpaca", None, &m(ALPACA_MIN)));
}

/// Leg 2 of 3 refused as too small: legs 1 and 3 stand, one orders_below_minimum line, no failed and no skipped row, and the
/// tick ends as one that placed.
#[tokio::test(flavor = "current_thread")]
async fn accepted_rejected_accepted_reports_the_middle_leg_as_below_minimum() {
    let (_d, o, id, _) = basket_bot(60.0, &[0.5, 0.3, 0.2], |b| b);
    let t = script(json!({ "POST /v2/orders": [accepted("OTX-1"), alpaca_min(), accepted("OTX-3")] }));
    assert!(matches!(run(&o, &venue(&t), id, T0).await, TickOutcome::Done { placed: true }));
    assert_eq!(t.posted_orders().len(), 3);
    assert_eq!(rows(&o), vec![(0, Some("OTX-1".into())), (0, Some("OTX-3".into()))]);
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'orders_below_minimum'");
    assert_eq!(serde_json::from_str::<Value>(&details).unwrap(), json!({ "count": 1, "bases": "ETH" }));
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!((b.status, b.last_failure_kind()), (BotStatus::Scheduled, None));
    assert!(b.rust_placement().is_none());
}

/// The same bot with its legs skipped by the pre-check (each ticker's minimum over its leg) and by the venue: the same rows,
/// events, failure state and status; and at the next checkpoint the same pending budget, which is twice the contribution
/// (nothing spent, nothing credited twice: no carry is written).
#[tokio::test(flavor = "current_thread")]
async fn an_all_rejected_run_is_the_pre_checks_skip_through_the_next_checkpoint() {
    let mut seen = vec![];
    for venue_said_so in [false, true] {
        let (_d, o, id, _) = basket_bot(60.0, &[0.7, 0.3], |b| b.transient("last_failure_kind", json!("transient")));
        if !venue_said_so { o.primary.execute("UPDATE tickers SET minimum_quote_size = '100'", []).unwrap(); }
        let refused = if venue_said_so { vec![alpaca_min(), alpaca_min()] } else { vec![] };
        let t = script(json!({ "POST /v2/orders": refused.into_iter().chain([accepted("OTX-1"), accepted("OTX-2")]).collect::<Vec<_>>() }));
        let v = venue(&t);
        assert!(matches!(run(&o, &v, id, T0).await, TickOutcome::Done { placed: false }));
        assert_eq!(t.posted_orders().len(), if venue_said_so { 2 } else { 0 });
        let skipped: Vec<(i64, String, String)> = o.primary.prepare("SELECT status, CAST(quote_amount AS TEXT), coalesce(CAST(amount AS TEXT), '') FROM transactions ORDER BY id").unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get(2)?))).unwrap().map(Result::unwrap).collect();
        let b = model::load_bot(&o.primary, id).unwrap();
        assert!(b.rust_placement().is_none());
        assert!(b.transient.get("missed_quote_amount").is_none(), "no explicit carry: {}", b.transient);
        let state = (skipped, events(&o), b.status, b.last_failure_kind());
        // The next checkpoint, minimums back to the seeded 1 USD: two contributions owed, nothing invested.
        o.primary.execute("UPDATE tickers SET minimum_quote_size = '1'", []).unwrap();
        let pending = amount::pending_quote_amount(&o.primary, &model::load_bot(&o.primary, id).unwrap(), at(T1).timestamp_micros()).unwrap();
        assert_eq!(pending, BigDec::from_i64(120), "venue_said_so={venue_said_so}");
        assert!(matches!(run(&o, &v, id, T1).await, TickOutcome::Done { placed: true }));
        let sent: Vec<String> = t.posted_orders().iter().rev().take(2).map(|o| o["notional"].as_str().unwrap().to_string()).collect();
        seen.push((state, sent));
    }
    assert_eq!(seen[0].0 .3, None, "a successful skip clears the failure state");
    assert_eq!(seen[0], seen[1], "the venue's skip is the pre-check's");
    assert_eq!(seen[1].1, vec!["36.00".to_string(), "84.00".to_string()], "120 owed at 70/30");
}

/// A lost reply first: the intent waits, recovery proves the order absent, and the next checkpoint's below-minimum answer is
/// a skip with nothing failed.
#[tokio::test(flavor = "current_thread")]
async fn a_timeout_then_recovery_then_a_rejection_skips() {
    let (_d, o, id, _) = basket_bot(60.0, &[1.0], |b| b);
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }, alpaca_min()],
                           "GET /v2/orders:by_client_order_id": [not_found()] }));
    let v = venue(&t);
    assert!(matches!(run(&o, &v, id, T0).await, TickOutcome::AwaitingReconciliation));
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_some());
    assert!(matches!(run(&o, &v, id, &(at(T0) + Duration::seconds(1200)).to_rfc3339()).await, TickOutcome::Rescheduled));
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none());
    assert!(matches!(run(&o, &v, id, T1).await, TickOutcome::Done { placed: false }));
    assert_eq!(rows(&o), vec![(2, None)]);
    assert_eq!(t.posted_orders().len(), 2);
    assert_eq!(t.posted_orders()[1]["notional"], "120.00", "the absent order's budget is still owed");
}

/// Settled in one transaction with the answer: a restarted process finds no intent to recover, and the bot's schedule is the
/// one the pre-check's skip leaves (last_action_job_at, no deferral).
#[tokio::test(flavor = "current_thread")]
async fn a_restart_after_the_rejection_settled_finds_nothing_to_recover() {
    let mut seen = vec![];
    for venue_said_so in [false, true] {
        let (_d, o, id, _) = basket_bot(60.0, &[1.0], |b| b);
        if !venue_said_so { o.primary.execute("UPDATE tickers SET minimum_quote_size = '100'", []).unwrap(); }
        let t = script(json!({ "POST /v2/orders": [alpaca_min()] }));
        assert!(matches!(run(&o, &venue(&t), id, T0).await, TickOutcome::Done { placed: false }));
        // A fresh process: a new venue, recovery first.
        let bot = model::load_bot(&o.primary, id).unwrap();
        let later = FixedClock(at(T0) + Duration::seconds(60));
        assert!(matches!(placement::recover(&o.primary, &venue(&t), &bot, &later).await.unwrap(), placement::Recovery::NoIntent));
        seen.push((bot.status, bot.transient.get("last_action_job_at").cloned(), bot.transient.get("rust_defer_until").cloned(), rows(&o)));
    }
    assert_eq!(seen[0], seen[1]);
}

/// A stop that lands while the refused order is in flight: nothing failed is recorded, the intent is gone and the bot stays
/// stopped, exactly as for an accepted order.
#[tokio::test(flavor = "current_thread")]
async fn a_stop_during_placement_leaves_the_bot_stopped_with_nothing_failed() {
    let (_d, o, id, _) = basket_bot(60.0, &[1.0], |b| b);
    let db = o.primary.path().unwrap().to_string();
    let v = through(KrakenVenue::new(ScriptedTransport::from_script(&json!({ "POST /0/private/AddOrder": [http(200, json!({ "error": ["EOrder:Order minimum not met"] }))] }))));
    *v.hook.borrow_mut() = Some(Box::new(move || { rusqlite::Connection::open(db).unwrap().execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap(); }));
    let _ = tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await;
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(b.status, BotStatus::Stopped);
    assert!(b.rust_placement().is_none());
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions WHERE status = 1"), 0);
}

/// Credentials replaced while the refused order is in flight: the answer is not settled under the new key, exactly as a
/// plain rejection's row is not (record_rejected_in): no row of either kind, and the dispatched intent stays for recovery.
#[tokio::test(flavor = "current_thread")]
async fn a_credential_change_during_placement_settles_nothing_as_a_rejection_would() {
    let mut seen = vec![];
    for answer in [json!({ "error": ["EOrder:Order minimum not met"] }), json!({ "error": ["EOrder:Insufficient funds"] })] {
        let (_d, o, id, _) = basket_bot(60.0, &[1.0], |b| b);
        let db = o.primary.path().unwrap().to_string();
        let v = through(KrakenVenue::new(ScriptedTransport::from_script(&json!({ "POST /0/private/AddOrder": [http(200, answer)] }))));
        *v.hook.borrow_mut() = Some(Box::new(move || {
            rusqlite::Connection::open(db).unwrap().execute("UPDATE api_keys SET key = ?1", [seed::cipher().encrypt("ANOTHER")]).unwrap();
        }));
        let result = tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await;
        let b = model::load_bot(&o.primary, id).unwrap();
        seen.push((format!("{:?}", result.map(|_| ()).map_err(|e| format!("{e:?}"))), rows(&o), b.rust_placement().is_some()));
    }
    assert_eq!(seen[0], seen[1]);
    assert_eq!(seen[0].1, vec![]);
}
