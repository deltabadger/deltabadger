//! PLAN-conditions-every-bot CE-4: a signal bot's price rule and spending cap. The engine's sequences (§4.2: the gate per
//! call side, the cap's fast path, clamp, guard and cap line, no stop), the signal cells against the Rails-recorded basket
//! twin's gate (§4.1: met_at writes and the gate's decision), and the settings through the page, MCP and REST, which are
//! the one signal settings writer (D-SIG-CONDITIONS, D-SIG-COND-LINE, D-SIG-CAP).
mod common;
use axum::{body::{to_bytes, Body}, http::Request};
use chrono::{DateTime, Utc};
use common::seed::{self, TxSpec};
use deltabadger::engine::signal::{self, Step};
use deltabadger::engine::tick::{PriceCache, TickContext};
use deltabadger::engine::venue_rules::AmountLogic;
use deltabadger::engine::{accounting, model, placement, FixedClock};
use deltabadger::ruby::BigDec;
use deltabadger::venue::fake::{FakeVenue, AddOutcome};
use deltabadger::web;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::Path;
use tower::ServiceExt;

const T0: &str = "2026-09-01T14:00:01Z";
fn t(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn at(s: &str) -> FixedClock { FixedClock(t(s)) }

/// A started Kraken signal bot (BTC/EUR) with a buy rule (fixed `buy` EUR) and a sell rule (fixed 60 EUR).
struct Sig { _dir: tempfile::TempDir, c: Connection, s: seed::Seeded, bot: i64, buy: i64, sell: i64 }

fn sig(buy: &str, settings: Value, transient: Value) -> Sig {
    let dir = common::rails_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let c = o.primary;
    let mut stored = json!({ "base_asset_id": s.btc, "quote_asset_id": s.quote });
    for (k, v) in settings.as_object().unwrap() { stored[k] = v.clone(); }
    c.execute("INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, started_at, created_at, updated_at) \
               VALUES ('Bots::Signal', 1, ?1, ?2, ?3, ?4, '2026-08-25 09:00:00', '2026-08-25 09:00:00', '2026-08-25 09:00:00')",
              params![s.exchange_id, s.user_id, stored.to_string(), transient.to_string()]).unwrap();
    let bot = c.last_insert_rowid();
    let rule = |direction: i64, amount: &str, token: &str| {
        c.execute("INSERT INTO bot_signals (bot_id, direction, amount_type, amount, enabled, token, created_at, updated_at) \
                   VALUES (?1, ?2, 0, ?3, 1, ?4, '2026-08-25 09:00:00', '2026-08-25 09:00:00')", params![bot, direction, amount, token]).unwrap();
        c.last_insert_rowid()
    };
    let (buy, sell) = (rule(0, buy, "tok-buy"), rule(1, "60", "tok-sell"));
    Sig { _dir: dir, c, s, bot, buy, sell }
}

fn priced() -> FakeVenue {
    FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.2", "49995.3").balance_body("ZEUR", "10000", "0")
}
/// `priced` with one BTC to sell, in the one balance body every read gets.
fn holding() -> FakeVenue {
    FakeVenue::from_script(&json!({ "http": { "/0/private/BalanceEx": [{ "error": [], "result": { "ZEUR": { "balance": "10000", "hold_trade": "0" },
                                                                                            "XXBT": { "balance": "1", "hold_trade": "0" } } }] } }))
        .ticker("XXBTZEUR", "49990.1", "50000.2", "49995.3")
}

impl Sig {
    fn call(&self, rule: i64, n: usize) {
        model::append_signal_call(&self.c, self.bot, &model::SignalCall { source: format!("hook:{n}"), signal_id: rule, triggered_at: t(T0) }).unwrap();
    }
    async fn run(&self, venue: &FakeVenue, now: &str) -> Step {
        let prices = PriceCache::default();
        let bot = model::load_bot(&self.c, self.bot).unwrap();
        let cx = TickContext { credential_version: model::credential_version(&self.c, &bot).unwrap(), prices: &prices,
                               process_start: t("2026-09-01T00:00:00Z"), stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
        signal::run_next(&self.c, venue, self.bot, &at(now), &cx).await.unwrap()
    }
    fn calls(&self) -> usize { model::load_bot(&self.c, self.bot).unwrap().signal_calls().unwrap().len() }
    fn transient(&self) -> Value { model::load_bot(&self.c, self.bot).unwrap().transient }
    /// (status, quote_amount, error_messages) of each row.
    fn rows(&self) -> Vec<(i64, String, String)> {
        let mut s = self.c.prepare("SELECT status, CAST(quote_amount AS TEXT), error_messages FROM transactions WHERE bot_id = ?1 ORDER BY id").unwrap();
        s.query_map([self.bot], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get(2)?))).unwrap().map(Result::unwrap).collect()
    }
    fn lines(&self) -> Vec<(String, Value)> {
        let mut s = self.c.prepare("SELECT event, details FROM bot_activity_logs WHERE bot_id = ?1 ORDER BY id").unwrap();
        s.query_map([self.bot], |r| Ok((r.get::<_, String>(0)?, serde_json::from_str(&r.get::<_, String>(1)?).unwrap()))).unwrap().map(Result::unwrap).collect()
    }
    fn status(&self) -> i64 { self.c.query_row("SELECT status FROM bots WHERE id = ?1", [self.bot], |r| r.get(0)).unwrap() }
    /// A REGULAR buy already on the books, counted by the cap enabled before it.
    fn spent(&self, quote: &'static str) {
        seed::insert_tx(&self.c, &self.s, self.bot, &TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("O-{quote}")), order_type: 0,
            amount: Some("0.001"), quote_amount: Some(quote), price: Some("50000"), quote_amount_exec: Some(quote), amount_exec: Some("0.001"),
            created_at: "2026-08-30 10:00:00".into() });
    }
}

fn capped(limit: Value) -> (Value, Value) {
    (json!({ "quote_amount_limited": true, "quote_amount_limit": limit }), json!({ "quote_amount_limit_enabled_at": "2026-08-01T00:00:00.000Z" }))
}
fn unmet_line(rule: i64) -> (String, Value) { ("signal_condition_unmet".into(), json!({ "signal_id": rule, "limit_type": "price" })) }
fn cap_line(rule: i64) -> (String, Value) { ("signal_quote_amount_limit_reached".into(), json!({ "signal_id": rule })) }

// ---------------------------------------------------------------------------------------------------------------------
// §2c: the gate, per call side.

/// D-SIG-COND-LINE: a buy call its price rule does not let through is consumed with one line: no row, no order, no venue
/// read beyond the rule's price, the bot's status unchanged, no met_at.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_unmet_buy_rule_consumes_the_call_with_its_line_and_nothing_else() {
    let s = sig("100", json!({ "price_limited": true, "price_limit": 40000, "price_limit_value_condition": "below", "price_limit_timing_condition": "while" }), json!({}));
    let v = priced();
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    assert_eq!((s.calls(), v.sent().len(), s.rows().len(), s.status()), (0, 0, 0, 1));
    assert_eq!(s.lines(), vec![unmet_line(s.buy)]);
    assert!(s.transient().get("price_limit_condition_met_at").is_none());
    assert_eq!(v.calls("/0/private/BalanceEx"), 0, "nothing sized");
}

/// The unmet write is fenced to the credentials the price was read with: a key replaced while the read is in flight keeps
/// the call and writes nothing.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_key_replaced_while_an_unmet_rules_price_is_read_keeps_the_call() {
    let s = sig("100", json!({ "price_limited": true, "price_limit": 40000, "price_limit_value_condition": "below", "price_limit_timing_condition": "while" }), json!({}));
    let path = s._dir.path().join("production.sqlite3");
    let cipher = seed::cipher();
    let rotated = std::cell::Cell::new(false);
    let v = priced().on_price(move || {
        if rotated.replace(true) { return; }
        Connection::open(&path).unwrap().execute("UPDATE api_keys SET key = ?1", [cipher.encrypt("rotated-key")]).unwrap();
    });
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Again);
    assert_eq!((s.calls(), v.sent().len(), s.rows().len(), s.status()), (1, 0, 0, 1));
    assert!(s.lines().is_empty(), "nothing consumed");
}

/// An `after` rule latches on the met call (met_at stamped, the order placed) and lets the next call through unread, the
/// price above it by then; no carry is written, ever.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_met_after_rule_latches_on_the_met_call() {
    let s = sig("100", json!({ "price_limited": true, "price_limit": 60000, "price_limit_value_condition": "below", "price_limit_timing_condition": "after" }), json!({}));
    let v = priced().ticker("XXBTZEUR", "69990.1", "70000.2", "69995.3").ticker("XXBTZEUR", "69990.1", "70000.2", "69995.3")
        .next_add(AddOutcome::Accept("OTX-1".into())).next_add(AddOutcome::Accept("OTX-2".into()));
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    assert_eq!(v.sent().len(), 1, "met: placed");
    let met = s.transient()["price_limit_condition_met_at"].clone();
    assert!(met.is_string(), "stamped: {}", s.transient());
    s.call(s.buy, 2);
    assert_eq!(s.run(&v, "2026-09-01T14:00:30Z").await, Step::Ran);
    assert_eq!(v.sent().len(), 2, "latched: placed above the limit");
    assert_eq!(s.transient()["price_limit_condition_met_at"], met, "the latch is kept");
    assert!(s.transient().get("missed_quote_amount").is_none() && s.transient().get("missed_quote_amount_was_set").is_none(), "no carry: {}", s.transient());
}

/// A `while` rule unmet after a met_at clears it, and writes no carry.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_while_rule_unmet_after_its_met_at_clears_it_without_a_carry() {
    let s = sig("100", json!({ "price_limited": true, "price_limit": 40000, "price_limit_value_condition": "below", "price_limit_timing_condition": "while" }),
                json!({ "price_limit_condition_met_at": "2026-09-01T12:00:00.000Z", "missed_quote_amount": "30" }));
    let v = priced();
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    let td = s.transient();
    assert!(td.get("price_limit_condition_met_at").is_some_and(Value::is_null), "cleared: {td}");
    assert_eq!((&td["missed_quote_amount"], td.get("missed_quote_amount_was_set")), (&json!("30"), None), "the carry untouched");
    assert_eq!(s.lines(), vec![unmet_line(s.buy)]);
}

/// The call's side selects the rules: an unmet buy rule never holds a sell call; the sell side's rule does.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn each_call_is_gated_by_its_own_sides_rule() {
    let s = sig("100", json!({ "price_limited": true, "price_limit": 40000, "price_limit_value_condition": "below" }), json!({}));
    let v = holding();
    s.call(s.sell, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    assert_eq!((v.sent().len(), v.sent()[0].side), (1, deltabadger::venue::Side::Sell), "the buy rule does not gate a sell call");
    s.c.execute("UPDATE bots SET settings = json_set(settings, '$.sell_price_limited', json('true'), '$.sell_price_limit', 60000, \
                 '$.sell_price_limit_value_condition', 'above') WHERE id = ?1", [s.bot]).unwrap();
    s.call(s.sell, 2);
    assert_eq!(s.run(&v, "2026-09-01T14:00:30Z").await, Step::Ran);
    assert_eq!(v.sent().len(), 1, "the sell rule holds it");
    assert_eq!(s.lines().last().unwrap(), &unmet_line(s.sell));
}

/// A rejected key while the rule's price is read is the failed row (fail_unsized); any other failed read is unmet.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_rejected_key_at_the_gate_is_a_failed_row_and_a_failed_read_is_unmet() {
    let rule = json!({ "price_limited": true, "price_limit": 60000, "price_limit_value_condition": "below" });
    let s = sig("100", rule.clone(), json!({}));
    let v = FakeVenue::from_script(&json!({ "http": { "/0/public/Ticker": [{ "error": ["EAPI:Invalid key"] }] } }));
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    assert_eq!(s.rows().len(), 1);
    assert_eq!((s.rows()[0].0, s.calls()), (1, 0), "a failed row, the call consumed");
    let s = sig("100", rule, json!({}));
    let v = FakeVenue::from_script(&json!({ "http": { "/0/public/Ticker": [{ "error": ["EGeneral:Internal error"] }] } }));
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    assert_eq!((s.rows().len(), s.lines()), (0, vec![unmet_line(s.buy)]));
}

// ---------------------------------------------------------------------------------------------------------------------
// §2b: the cap.

/// The fast path: stored amounts that prove the cap spent (nothing left; under the venue's quote minimum on a BaseOrQuote
/// venue) consume a buy call with the cap line before any venue read.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_cap_the_stored_amounts_prove_spent_is_answered_without_a_venue_read() {
    for (limit, spent) in [(json!(100), "100"), (json!(100), "99.7")] {
        let (settings, transient) = capped(limit);
        let s = sig("100", settings, transient);
        s.spent(spent);
        let v = priced();
        s.call(s.buy, 1);
        assert_eq!(s.run(&v, T0).await, Step::Ran);
        assert_eq!((v.calls("/0/public/Ticker"), v.calls("/0/private/BalanceEx"), v.sent().len()), (0, 0, 0), "{spent}: no venue read");
        assert_eq!((s.lines(), s.rows().len(), s.status()), (vec![cap_line(s.buy)], 1, 1), "{spent}: the cap line, the bot still running");
    }
}

/// D-SIG-CAP's fast path per market rule (§2b step 1), at the precision floor and the quote minimum, ±1 increment.
#[test]
fn the_fast_path_holds_exactly_where_every_minimum_exceeds_the_remainder() {
    let ticker = |minimum: &str| model::Ticker { id: 1, ticker: "XBTEUR".into(), base_code: "XBT".into(), quote_code: "EUR".into(), base_symbol: "BTC".into(),
        quote_symbol: "EUR".into(), exchange_name: "Kraken".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 8, quote_decimals: 2, price_decimals: 1,
        minimum_base_size: BigDec::parse("0.0001").unwrap(), minimum_quote_size: BigDec::parse(minimum).unwrap(), trading_enabled: true, available: true, crypto: true };
    let d = |s: &str| BigDec::parse(s).unwrap();
    for logic in [AmountLogic::Quote, AmountLogic::BaseOrQuote, AmountLogic::BaseAndQuoteInBase, AmountLogic::Base] {
        let spent = |left: &str, minimum: &str| signal::cap_spent_unpriced(&d(left), &ticker(minimum), logic).unwrap();
        assert!(spent("0", "5") && spent("-1", "0"), "{logic:?}: nothing left");
        assert!(spent("0.009", "0") && !spent("0.01", "0"), "{logic:?}: the precision floor 0.01");
        let under_minimum = logic != AmountLogic::Base;
        assert_eq!((spent("4.99", "5"), spent("5", "5"), spent("5.01", "5")), (under_minimum, false, false), "{logic:?}: the quote minimum");
    }
}

/// Clamped: a remainder above the venue's quote minimum is read and sized; under the venue's whole minimum it is the cap
/// line (no row); above it the order is the remainder.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_binding_cap_clamps_the_buy_to_the_remainder() {
    let (settings, transient) = capped(json!(100));
    let s = sig("100", settings.clone(), transient.clone());
    s.spent("99");
    let v = priced();
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    assert!(v.calls("/0/public/Ticker") >= 1, "1 EUR is over the 0.5 quote minimum: the price decides");
    assert_eq!((v.sent().len(), s.rows().len(), s.lines()), (0, 1, vec![cap_line(s.buy)]), "0.00001999 BTC is under Kraken's 0.00005: the cap line");
    let s = sig("100", settings, transient);
    s.spent("90");
    let v = priced();
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    assert_eq!(v.sent().len(), 1);
    assert_eq!(v.sent()[0].volume, "0.00019999", "10 EUR at the 50000.2 ask, floored to 8 places");
    assert_eq!(s.rows().last().unwrap().1, "10", "the row records the remainder");
}

/// A cap that does not bind leaves Rails' own below-minimum skip: the skipped row and its line.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn under_the_minimum_without_a_binding_cap_is_rails_skipped_row() {
    let (settings, transient) = capped(json!(1000));
    let s = sig("1", settings, transient);
    let v = priced();
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    assert_eq!(s.rows().iter().map(|r| r.0).collect::<Vec<_>>(), vec![2], "a skipped row");
    assert_eq!(s.lines().iter().map(|(e, _)| e.as_str()).collect::<Vec<_>>(), ["order_skipped"]);
}

/// A spent cap never stops the bot, and never holds a sell call.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_spent_cap_answers_buy_calls_and_sells_keep_working() {
    let (settings, transient) = capped(json!(100));
    let s = sig("100", settings, transient);
    s.spent("100");
    let v = holding();
    s.call(s.buy, 1);
    s.call(s.sell, 2);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    assert_eq!(v.sent().iter().map(|o| o.side).collect::<Vec<_>>(), vec![deltabadger::venue::Side::Sell]);
    assert_eq!((s.status(), s.lines()), (1, vec![cap_line(s.buy)]), "running, the buy call answered by the cap line");
}

/// The polling stop (Bot::QuoteAmountLimitable's after_commit) skips a signal bot: a buy whose fill reaches the cap leaves
/// it running.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_fill_that_reaches_the_cap_never_stops_a_signal_bot() {
    let (settings, transient) = capped(json!(99.99));
    let s = sig("100", settings, transient);
    let v = priced().order("OTX-1", json!({ "status": "closed", "price": "50000.2", "vol": "0.00199979", "vol_exec": "0.00199979", "cost": "99.99", "oflags": "",
                                             "descr": { "ordertype": "market", "price": "0", "type": "buy" } }))
                    .next_add(AddOutcome::Accept("OTX-1".into()));
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    let row: i64 = s.c.query_row("SELECT id FROM transactions WHERE bot_id = ?1", [s.bot], |r| r.get(0)).unwrap();
    deltabadger::engine::polling::refresh(&s.c, &v, s.bot, row, t("2026-09-01T14:00:10Z")).await.unwrap();
    let (exec, key): (Option<f64>, Option<String>) = s.c.query_row("SELECT t.quote_amount_exec, b.stop_message_key FROM transactions t JOIN bots b ON b.id = t.bot_id WHERE t.id = ?1", [row], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(exec, Some(99.99), "filled");
    assert!(accounting::exact_cap_available(&s.c, &model::load_bot(&s.c, s.bot).unwrap()).unwrap().unwrap().is_zero(), "the cap is spent");
    assert_eq!((s.status(), key, model::load_bot(&s.c, s.bot).unwrap().pending_amount_limit_stops().unwrap()), (1, None, None), "no stop");
}

/// The exact cap guard in begin_signal (§2b step 4): a reduction under the venue's minimum is the skipped row, its line and
/// the consumed call, in the claim's transaction; no intent.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_guard_reduction_under_the_minimum_is_one_atomic_skip() {
    let (settings, transient) = capped(json!(100));
    let s = sig("100", settings, transient);
    let bot = model::load_bot(&s.c, s.bot).unwrap();
    let ticker = model::ticker_for(&s.c, &bot).unwrap().unwrap();
    // Sized from 2.5006 EUR at 50000.2: 0.00005001 BTC, one increment over Kraken's 0.00005.
    let price = BigDec::parse("50000.2").unwrap();
    let quote = BigDec::parse("2.5006").unwrap();
    let (plan, below) = deltabadger::engine::amount::size_order(&ticker, deltabadger::venue::Side::Buy, &price, &quote.div(&price).unwrap(), &quote,
        deltabadger::engine::venue_rules::KRAKEN.minimum_logic, deltabadger::engine::amount::TxKind::Regular).unwrap();
    assert!(!below && plan.volume == BigDec::parse("0.00005001").unwrap());
    // Spent meanwhile: 2.4999 EUR left, which two increments fewer meet, under the minimum.
    s.spent("97.5001");
    s.call(s.buy, 1);
    let the_call = model::load_bot(&s.c, s.bot).unwrap().signal_calls().unwrap().remove(0);
    let version = model::credential_version(&s.c, &bot).unwrap();
    let claim = placement::begin_signal(&s.c, &bot, &plan, Some(&the_call), &version, None, &at(T0)).unwrap();
    assert!(matches!(claim, placement::SignalClaim::BelowMinimum), "{claim:?}");
    assert_eq!((s.calls(), s.rows().last().unwrap().0, s.lines().iter().map(|(e, _)| e.as_str()).collect::<Vec<_>>()), (0, 2, vec!["order_skipped"]));
    assert!(s.transient().get("rust_placement").is_none(), "no intent");
}

/// A closed buy that never reported its cost: the cap's spend is unknown, and the buy call is a failed row with Rails' text.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_unknown_spend_is_a_failed_row() {
    let (settings, transient) = capped(json!(100));
    let s = sig("100", settings, transient);
    seed::insert_tx(&s.c, &s.s, s.bot, &TxSpec { status: 0, external_status: Some(2), external_id: Some("O-unknown".into()), order_type: 0,
        amount: Some("0.001"), quote_amount: None, price: None, quote_amount_exec: None, amount_exec: None, created_at: "2026-08-30 10:00:00".into() });
    let v = priced();
    s.call(s.buy, 1);
    assert_eq!(s.run(&v, T0).await, Step::Ran);
    let failed = s.rows().into_iter().find(|r| r.0 == 1).expect("a failed row");
    assert!(failed.2.contains("The amount spent so far is unknown"), "{failed:?}");
    assert_eq!((v.sent().len(), s.calls()), (0, 0));
}

/// What the cap counts: open orders at their quote, a partly filled cancel at what it executed, an unresolved buy intent
/// at its quote.
#[test]
fn the_cap_counts_open_orders_partial_cancels_and_an_unresolved_intent() {
    let (settings, transient) = capped(json!(100));
    let s = sig("100", settings, transient);
    let tx = |external_status, quote: &'static str, exec: Option<&'static str>, id: &str| seed::insert_tx(&s.c, &s.s, s.bot, &TxSpec { status: 0, external_status: Some(external_status),
        external_id: Some(id.into()), order_type: 0, amount: Some("0.001"), quote_amount: Some(quote), price: Some("50000"), quote_amount_exec: exec,
        amount_exec: exec.map(|_| "0.0006"), created_at: "2026-08-30 10:00:00".into() });
    tx(0, "40", None, "O-open");
    tx(3, "50", Some("30"), "O-cancelled");
    let left = || accounting::exact_cap_available(&s.c, &model::load_bot(&s.c, s.bot).unwrap()).unwrap().unwrap();
    assert_eq!(left(), BigDec::parse("30").unwrap(), "100 - 40 open - 30 executed");
    s.c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement', json('{\"quote_amount\":\"29.5\",\"side\":\"buy\"}')) WHERE id = ?1", [s.bot]).unwrap();
    assert_eq!(left(), BigDec::parse("0.5").unwrap(), "the unresolved intent counted");
}

// ---------------------------------------------------------------------------------------------------------------------
// §4.1: the signal cells against the Rails basket twin's gate (grid-conditions, Kraken, both sides).

/// What a gate decided: the call (or tick) went on to trade, was held by the rule, or failed.
#[derive(Debug, PartialEq)]
enum Gate { Pass, Held, Failed }

/// D-SIG-CONDITIONS: on each Kraken cell of Rails' condition grid, a signal bot carrying the basket's rule (same side, same
/// stored met_at, the same venue script) and a call of that side writes met_at as Rails' basket does (stamped, kept or
/// cleared) and decides as its gate does: traded ↔ placed, paused ↔ the unmet line, a failed tick ↔ the failed row. Carry,
/// status and check jobs are not compared (a signal bot has none of them).
#[tokio::test(flavor = "current_thread")]
async fn signal_cells_decide_as_the_rails_basket_twins_gate() {
    const CASES: [&str; 15] = ["below_met", "below_unmet", "below_at_limit", "above_met", "above_unmet", "between_reversed_met", "between_unmet",
        "after_stored", "after_unmet", "after_met", "while_stored_met", "while_stored_unmet", "missing_ticker", "server_error", "rejected_key"];
    let built = tempfile::tempdir().unwrap();
    common::decisions::built_grid("grid-conditions", built.path());
    let rails_root = tempfile::tempdir().unwrap();
    let mut names = vec![];
    for side in ["buy", "sell"] {
        for case in CASES {
            let name = format!("cond-kraken-{side}-{case}");
            common::decisions::copy_dir(&built.path().join(&name), &rails_root.path().join(&name));
            names.push((name, side));
        }
    }
    let rust_root = tempfile::tempdir().unwrap();
    for (name, _) in &names { common::decisions::copy_dir(&rails_root.path().join(name), &rust_root.path().join(name)); }
    common::decisions::rails(&["record", rails_root.path().to_str().unwrap()]);
    let mut failures = vec![];
    for (name, side) in &names {
        let rails: Value = serde_json::from_str(&std::fs::read_to_string(rails_root.path().join(name).join("rails.json")).unwrap()).unwrap();
        let (rails_met, rails_gate) = rails_cell(&rails, side);
        let (rust_met, rust_gate) = signal_cell(&rust_root.path().join(name), side).await;
        if (&rails_met, &rails_gate) != (&rust_met, &rust_gate) {
            failures.push(format!("{name}: Rails basket met_at {rails_met:?} gate {rails_gate:?}; signal met_at {rust_met:?} gate {rust_gate:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// met_at as a write: "kept", "stamped" or "cleared".
fn met_write(before: Option<&Value>, after: Option<&Value>) -> &'static str {
    match (before.filter(|v| !v.is_null()), after.filter(|v| !v.is_null())) {
        (None, Some(_)) => "stamped",
        (Some(_), None) => "cleared",
        (a, b) if a == b => "kept",
        _ => "restamped",
    }
}

fn rails_cell(rails: &Value, side: &str) -> (&'static str, Gate) {
    let key = if side == "sell" { "sell_price_limit_condition_met_at" } else { "price_limit_condition_met_at" };
    let bot = rails["changes"]["bots"].as_array().unwrap().iter().find(|b| b["id"] == json!(1));
    let met = bot.map_or("kept", |b| met_write(b["before"]["transient_data"].get(key), b["after"]["transient_data"].get(key)));
    let gate = if !rails["sent"].as_array().unwrap().is_empty() { Gate::Pass }
        else if !rails["checks"].as_array().unwrap().is_empty() { Gate::Held } else { Gate::Failed };
    (met, gate)
}

/// The cell's database and script, with a signal bot beside its basket (stopped): the basket's rule and met_at, a rule of
/// the cell's side (60 EUR, the basket's amount) and one call, run once at the cell's time.
async fn signal_cell(dir: &Path, side: &str) -> (&'static str, Gate) {
    let scenario: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("scenario.json")).unwrap()).unwrap();
    let c = Connection::open(dir.join("production.sqlite3")).unwrap();
    deltabadger::store::test_support::migrate(&c).unwrap();
    let (settings, transient, user, exchange): (String, String, i64, i64) =
        c.query_row("SELECT settings, transient_data, user_id, exchange_id FROM bots WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    let (settings, transient): (Value, Value) = (serde_json::from_str(&settings).unwrap(), serde_json::from_str(&transient).unwrap());
    let base: i64 = settings["allocations"].as_object().unwrap().keys().next().unwrap().parse().unwrap();
    let mut rules = json!({ "base_asset_id": base, "quote_asset_id": settings["quote_asset_id"] });
    for (k, v) in settings.as_object().unwrap() { if k.contains("price_limit") { rules[k] = v.clone(); } }
    let key = if side == "sell" { "sell_price_limit_condition_met_at" } else { "price_limit_condition_met_at" };
    let mut td = json!({});
    if let Some(met) = transient.get(key) { td[key] = met.clone(); }
    c.execute("UPDATE bots SET status = 2 WHERE id = 1", []).unwrap();
    c.execute("INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, started_at, created_at, updated_at) \
               VALUES ('Bots::Signal', 1, ?1, ?2, ?3, ?4, '2026-08-25 09:00:00', '2026-08-25 09:00:00', '2026-08-25 09:00:00')",
              params![exchange, user, rules.to_string(), td.to_string()]).unwrap();
    let bot = c.last_insert_rowid();
    c.execute("INSERT INTO bot_signals (bot_id, direction, amount_type, amount, enabled, token, created_at, updated_at) \
               VALUES (?1, ?2, 0, '60', 1, 'tok-cell', '2026-08-25 09:00:00', '2026-08-25 09:00:00')", params![bot, i64::from(side == "sell")]).unwrap();
    let rule = c.last_insert_rowid();
    let now = scenario["at"].as_str().unwrap();
    model::append_signal_call(&c, bot, &model::SignalCall { source: "hook:cell".into(), signal_id: rule, triggered_at: t(now) }).unwrap();
    // The listing the cell withdraws after it was built (missing_ticker): Rails' grid applies it before the tick.
    if let Some(after) = scenario["ticker_after"].as_object() {
        for (column, value) in after { c.execute(&format!("UPDATE tickers SET {column} = ?1"), [value.as_bool().map_or(0, i64::from)]).unwrap(); }
    }
    let venue = FakeVenue::from_script(&scenario["script"]);
    let prices = PriceCache::default();
    let loaded = model::load_bot(&c, bot).unwrap();
    let cx = TickContext { credential_version: model::credential_version(&c, &loaded).unwrap(), prices: &prices,
                           process_start: t("2026-09-01T00:00:00Z"), stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
    signal::run_next(&c, &venue, bot, &at(now), &cx).await.unwrap();
    let after = model::load_bot(&c, bot).unwrap().transient;
    let met = met_write(td.get(key), after.get(key));
    let failed: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND status = 1)", [bot], |r| r.get(0)).unwrap();
    let unmet: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'signal_condition_unmet')", [bot], |r| r.get(0)).unwrap();
    let gate = if !venue.sent().is_empty() { Gate::Pass } else if unmet { Gate::Held } else if failed { Gate::Failed } else { panic!("{}: no decision", dir.display()) };
    (met, gate)
}

// ---------------------------------------------------------------------------------------------------------------------
// The settings: one writer for the page, MCP and REST.

const NOW: &str = "2026-09-10T12:00:30.123456Z";

/// A stopped Kraken signal bot (BTC/EUR, one buy rule) and an MCP and a REST client allowed update_bot_settings and
/// start_bot.
struct Install { _dir: tempfile::TempDir, app: web::App, c: Connection, user: i64, id: i64, s: seed::Seeded }

fn install() -> Install {
    let dir = common::rails_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let c = o.primary;
    c.execute("INSERT INTO bots (type, status, exchange_id, user_id, label, settings, transient_data, started_at, created_at, updated_at) \
               VALUES ('Bots::Signal', 2, ?1, ?2, 'Signals', ?3, '{}', '2026-08-25 09:00:00', '2026-08-25 09:00:00', '2026-08-25 09:00:00')",
              params![s.exchange_id, s.user_id, json!({ "base_asset_id": s.btc, "quote_asset_id": s.quote }).to_string()]).unwrap();
    let id = c.last_insert_rowid();
    c.execute("INSERT INTO bot_signals (bot_id, direction, amount_type, amount, enabled, token, created_at, updated_at) \
               VALUES (?1, 0, 0, '100', 1, 'tok-settings', '2026-08-25 09:00:00', '2026-08-25 09:00:00')", [id]).unwrap();
    c.execute("UPDATE users SET confirmed_at = created_at, wash_sale_enabled = 0", []).unwrap();
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('CE4','ce4','http://localhost/cb',0,'mcp api','2026-09-10 12:00:00','2026-09-10 12:00:00')", []).unwrap();
    let application = c.last_insert_rowid();
    for (token, scope) in [("ce4-mcp", "mcp"), ("ce4-api", "api")] {
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,'2026-09-10 12:00:00',86400)", (application, s.user_id, token, scope)).unwrap();
    }
    let tools = json!(["update_bot_settings", "start_bot"]).to_string();
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,?3,?3,'2026-09-10 12:00:00','2026-09-10 12:00:00')", (s.user_id, application, &tools)).unwrap();
    let permissions = json!({"tool_permissions":{"update_bot_settings":true,"start_bot":true}}).to_string();
    c.execute("UPDATE users SET mcp_settings = ?1, rest_settings = ?1", [&permissions]).unwrap();
    let app = common::web::app(dir.path(), "engine-test-secret", common::web::TestClock::at(NOW));
    Install { _dir: dir, app, c, user: s.user_id, id, s }
}

#[derive(Clone, Copy, Debug)]
enum Surface { Web, Mcp, Rest }

async fn send(app: &web::App, request: Request<Body>) -> (u16, String) {
    let response = web::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    (status, String::from_utf8(to_bytes(response.into_body(), 4_000_000).await.unwrap().to_vec()).unwrap())
}

/// One MCP tool call: (HTTP status, the text, isError).
async fn mcp(i: &Install, tool: &str, mut arguments: serde_json::Map<String, Value>) -> (u16, Value) {
    let request = |body: Value, session: Option<&str>| {
        let mut r = Request::builder().method("POST").uri("/mcp").header("host", "localhost:3000").header("authorization", "Bearer ce4-mcp")
            .header("content-type", "application/json").header("accept", "application/json, text/event-stream");
        if let Some(session) = session { r = r.header("mcp-session-id", session); }
        r.body(Body::from(body.to_string())).unwrap()
    };
    let response = web::router(i.app.clone()).oneshot(request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"CE4","version":"1"}}}), None)).await.unwrap();
    let session = response.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
    assert_eq!(send(&i.app, request(json!({"jsonrpc":"2.0","method":"notifications/initialized"}), Some(&session))).await.0, 202);
    arguments.insert("bot_id".into(), json!(i.id));
    let (status, body) = send(&i.app, request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":arguments}}), Some(&session))).await;
    let body: Value = serde_json::from_str(&body).unwrap();
    (status, json!({ "text": body["result"]["content"][0]["text"], "error": body["result"]["isError"] }))
}

/// One settings save: the page's post (under `bots_signal`), or the automation dialect over MCP or REST.
async fn save(i: &Install, surface: Surface, web_fields: Value, api_fields: Value) -> (u16, Value) {
    match surface {
        Surface::Web => {
            let token = web::csrf::new_token();
            let session = web::session::SessionData { user: Some((i.user, "x".into())), csrf: Some(token.clone()), ..Default::default() };
            let cookie = web::session::seal(&i.app.keys.session, &session, i.app.now());
            let request = Request::builder().method("PATCH").uri(format!("/bots/{}", i.id)).header("host", "localhost:3000")
                .header("cookie", format!("{}={cookie}", web::session::COOKIE)).header("x-csrf-token", web::csrf::masked(&token))
                .header("content-type", "application/json").header("accept", "text/vnd.turbo-stream.html")
                .body(Body::from(json!({ "bots_signal": web_fields }).to_string())).unwrap();
            let (status, body) = send(&i.app, request).await;
            (status, json!(body))
        }
        Surface::Rest => {
            let request = Request::builder().method("PATCH").uri(format!("/api/v1/bots/{}", i.id)).header("host", "localhost:3000")
                .header("authorization", "Bearer ce4-api").header("content-type", "application/json").header("accept", "application/json")
                .body(Body::from(api_fields.to_string())).unwrap();
            let (status, body) = send(&i.app, request).await;
            (status, serde_json::from_str(&body).unwrap_or(json!(body)))
        }
        Surface::Mcp => mcp(i, "update_bot_settings", api_fields.as_object().unwrap().clone()).await,
    }
}

fn row(i: &Install) -> Value {
    let (settings, transient, changed, updated): (String, String, Option<String>, String) = i.c.query_row(
        "SELECT settings, transient_data, settings_changed_at, updated_at FROM bots WHERE id = ?1", [i.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    json!({ "settings": serde_json::from_str::<Value>(&settings).unwrap(), "transient": serde_json::from_str::<Value>(&transient).unwrap(), "changed": changed, "updated": updated })
}

/// The same change on each surface, each on a fresh install: the rows are equal.
async fn on_every_surface(web: Value, api: Value) -> (Vec<(u16, Value)>, Value) {
    let (mut saved, mut rows) = (vec![], vec![]);
    for surface in [Surface::Web, Surface::Mcp, Surface::Rest] {
        let i = install();
        saved.push(save(&i, surface, web.clone(), api.clone()).await);
        rows.push(row(&i));
    }
    assert_eq!(rows[0], rows[1], "web and MCP write the same row");
    assert_eq!(rows[0], rows[2], "web and REST write the same row");
    (saved, rows.remove(0))
}

#[tokio::test(flavor = "current_thread")]
async fn both_sides_price_rules_and_the_cap_save_the_same_row_on_the_page_mcp_and_rest() {
    let (saved, row) = on_every_surface(
        json!({ "price_limited": "1", "price_limit_mode": "restrict", "price_limit_value_condition": "below", "price_limit": "45000",
                "sell_price_limited": "1", "sell_price_limit_mode": "start", "sell_price_limit_value_condition": "above", "sell_price_limit": "60000",
                "quote_amount_limited": "1", "quote_amount_limit": "300" }),
        json!({ "price_limited": true, "price_limit_mode": "restrict", "price_limit_value_condition": "below", "price_limit": 45000,
                "sell_price_limited": true, "sell_price_limit_mode": "start", "sell_price_limit_value_condition": "above", "sell_price_limit": 60000,
                "quote_amount_limited": true, "quote_amount_limit": 300 })).await;
    assert_eq!(saved[0].0, 200, "{}", saved[0].1);
    let updated = "quote_amount_limited, quote_amount_limit, price_limited, price_limit_mode, price_limit_value_condition, price_limit, \
                   sell_price_limited, sell_price_limit_mode, sell_price_limit_value_condition, sell_price_limit";
    assert_eq!(saved[1].1["text"], json!(format!("Bot 'Signals' settings updated: {updated}.")), "{}", saved[1].1);
    assert_eq!((saved[2].0, saved[2].1["data"]["updated"].as_array().map(Vec::len)), (200, Some(10)), "{}", saved[2].1);
    let s = &row["settings"];
    assert_eq!((&s["price_limited"], &s["price_limit"], &s["price_limit_timing_condition"], &s["price_limit_action"]), (&json!(true), &json!(45000.0), &json!("while"), &json!("pause")));
    assert_eq!((&s["sell_price_limited"], &s["sell_price_limit"], &s["sell_price_limit_timing_condition"], &s["sell_price_limit_value_condition"]),
               (&json!(true), &json!(60000.0), &json!("after"), &json!("above")));
    assert_eq!((&s["quote_amount_limited"], &s["quote_amount_limit"]), (&json!(true), &json!(300.0)));
    assert!(s.get("price_limit_in_ticker_id").is_none(), "no subject: {s}");
    for key in ["price_limit_enabled_at", "sell_price_limit_enabled_at", "quote_amount_limit_enabled_at"] {
        assert_eq!(row["transient"][key], json!("2026-09-10T12:00:30.123Z"), "{key}");
    }
    assert_eq!(row["changed"], json!("2026-09-10 12:00:30.123456"), "a settings change moves settings_changed_at (Automation::Configurable)");
}

/// What a signal bot has nothing to act on, or no rule for, is refused by name; a rule not built yet is the guard's refusal;
/// a running bot's settings are refused as every bot's are. Nothing is written.
#[tokio::test(flavor = "current_thread")]
async fn the_api_refuses_what_a_signal_bot_does_not_take() {
    for (fields, text) in [
        (json!({ "price_limited": true, "price_limit_mode": "flip" }), "flip applies to basket bots only."),
        (json!({ "sell_price_limit_mode": "flip" }), "flip applies to basket bots only."),
        (json!({ "price_limit_in_ticker_id": 1 }), "price_limit_in_ticker_id applies to basket bots only."),
        (json!({ "quote_amount": 50 }), "quote_amount applies to DCA bots only."),
        (json!({ "rebalance_enabled": true }), "rebalance_enabled applies to basket and index bots only."),
        (json!({ "base_amount_limited": true }), "base_amount_limited applies to basket bots only."),
        (json!({ "price_drop_limited": true }), "This app can't run that yet: price_drop_limited on a signal bot"),
    ] {
        let i = install();
        let before = row(&i);
        let (_, answer) = save(&i, Surface::Mcp, json!({}), fields.clone()).await;
        assert_eq!((&answer["text"], &answer["error"]), (&json!(text), &json!(true)), "{fields}");
        assert_eq!(row(&i), before, "{fields}: nothing written");
    }
    let i = install();
    i.c.execute("UPDATE bots SET status = 1 WHERE id = ?1", [i.id]).unwrap();
    let (_, answer) = save(&i, Surface::Mcp, json!({}), json!({ "price_limited": true })).await;
    assert_eq!(answer["text"], json!("Bot must be stopped before updating settings. Current status: scheduled."));
    // The name and the venue are taken as Rails' signal bot takes them.
    let i = install();
    let (_, answer) = save(&i, Surface::Mcp, json!({}), json!({ "label": "Renamed" })).await;
    assert_eq!(answer["text"], json!("Bot 'Renamed' settings updated: label."));
}

/// D-SIG-CAP: a start with the cap spent is refused, on every surface the start goes through (start_errors).
#[tokio::test(flavor = "current_thread")]
async fn a_start_with_the_cap_spent_is_refused() {
    let i = install();
    i.c.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true'), '$.quote_amount_limit', 100), \
                 transient_data = json_set(transient_data, '$.quote_amount_limit_enabled_at', '2026-08-01T00:00:00.000Z') WHERE id = ?1", [i.id]).unwrap();
    seed::insert_tx(&i.c, &i.s, i.id, &TxSpec { status: 0, external_status: Some(2), external_id: Some("O-100".into()), order_type: 0, amount: Some("0.002"),
        quote_amount: Some("100"), price: Some("50000"), quote_amount_exec: Some("100"), amount_exec: Some("0.002"), created_at: "2026-08-30 10:00:00".into() });
    let (_, answer) = mcp(&i, "start_bot", serde_json::Map::new()).await;
    assert_eq!(answer["error"], json!(true), "{answer}");
    assert!(answer["text"].as_str().unwrap().starts_with("Failed to start bot 'Signals': Settings"), "{answer}");
    // D-START-CAP-COPY: Rails has no text for this refusal (its missing-translation text); this one is written in every locale.
    assert!(answer["text"].as_str().unwrap().contains("The whole amount has been invested: raise the limit or turn it off to start the bot."), "{answer}");
    let token = web::csrf::new_token();
    let session = web::session::SessionData { user: Some((i.user, "x".into())), csrf: Some(token.clone()), ..Default::default() };
    let cookie = web::session::seal(&i.app.keys.session, &session, i.app.now());
    let request = Request::builder().method("PATCH").uri(format!("/pl/bots/{}/start", i.id)).header("host", "localhost:3000")
        .header("cookie", format!("{}={cookie}", web::session::COOKIE)).header("x-csrf-token", web::csrf::masked(&token))
        .header("accept", "text/vnd.turbo-stream.html").body(Body::empty()).unwrap();
    let (_, page) = send(&i.app, request).await;
    assert!(page.contains("Cała kwota została zainwestowana: zwiększ limit lub go wyłącz, aby uruchomić bota."), "{page}");
    assert!(!page.contains("Translation missing"), "{page}");
    let status: i64 = i.c.query_row("SELECT status FROM bots WHERE id = ?1", [i.id], |r| r.get(0)).unwrap();
    assert_eq!(status, 2, "not started");
    // Raised: the start goes through.
    i.c.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limit', 1000) WHERE id = ?1", [i.id]).unwrap();
    let (_, answer) = mcp(&i, "start_bot", serde_json::Map::new()).await;
    assert_eq!(answer["text"], json!("Bot 'Signals' started successfully."), "{answer}");
}

