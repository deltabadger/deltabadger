mod common;
use chrono::{DateTime, Duration, Utc};
use common::scripted::{accepted, not_found, ok, script, venue, POST_SEND, PRE_SEND};
use common::seed::{self, BotSpec};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{basket, model, FixedClock};
use deltabadger::enums::BotStatus;
use deltabadger::store;
use deltabadger::venue::alpaca::AlpacaVenue;
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const T0: &str = "2026-09-01T10:00:00.5Z"; // first tick of a bot started 2026-09-01 10:00: one weekly interval owed
fn one<T: rusqlite::types::FromSql>(o: &store::Opened, sql: &str) -> T { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }
fn notionals(t: &ScriptedTransport) -> Vec<(String, String)> {
    t.posted_orders().iter().map(|o| (o["symbol"].as_str().unwrap().to_string(), o["notional"].as_str().unwrap_or("").to_string())).collect()
}
/// A weekly basket of `quote` USD over BTC, ETH (and SOL) at these weights, on Alpaca paper.
fn basket_bot(quote: f64, weights: &[f64]) -> (tempfile::TempDir, store::Opened, i64, seed::Seeded, Vec<i64>) {
    let (d, o, s) = common::install_alpaca();
    let (eth, sol) = seed::add_eth_sol(&o.primary, &s);
    let assets = [s.btc, eth, sol];
    let pairs: Vec<(i64, f64)> = weights.iter().enumerate().map(|(i, w)| (assets[i], *w)).collect();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(quote, "2026-09-01 10:00:00").weights(&pairs));
    (d, o, id, s, assets.to_vec())
}
async fn run(o: &store::Opened, v: &AlpacaVenue<ScriptedTransport>, id: i64, when: DateTime<Utc>) -> TickOutcome {
    tick::tick(&o.primary, v, id, &FixedClock(when), &mut Attempts::default()).await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn a_basket_tick_places_one_order_per_member_in_weight_order() {
    let (_d, o, id, _, assets) = basket_bot(60.0, &[0.7, 0.3]);
    let t = script(json!({}));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Done { placed: true }));
    // Empty basket: offsets 42 and 18 of 60; each leg is its share of the contribution.
    assert_eq!(notionals(&t), vec![("BTC/USD".into(), "42.00".into()), ("ETH/USD".into(), "18.00".into())]);
    let rows: Vec<i64> = o.primary.prepare("SELECT base_asset_id FROM transactions ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(rows, vec![assets[0], assets[1]]);
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Scheduled);
}

#[tokio::test(flavor = "current_thread")]
async fn a_rejected_middle_leg_stops_the_tick_and_never_sends_the_rest() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.5, 0.3, 0.2]);
    let t = script(json!({ "POST /v2/orders": [accepted("OTX-1"), { "status": 403, "body": { "code": 40310000, "message": "insufficient buying power" } }, accepted("OTX-3")] }));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Rescheduled));
    assert_eq!(notionals(&t).len(), 2, "SOL is never sent");
    let rows: Vec<(i64, Option<String>)> = o.primary.prepare("SELECT status, external_id FROM transactions ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(rows, vec![(0, Some("OTX-1".into())), (1, None)], "leg 1 submitted, leg 2 failed, nothing for leg 3");
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!((b.status, b.last_failure_kind().as_deref()), (BotStatus::Retrying, Some("insufficient_funds")));
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'execution_failed'"), 0, "a failed row this tick: Rails logs nothing");
    assert!(one::<bool>(&o, "SELECT last_end_of_funds_notification IS NOT NULL FROM bots"), "Bot::Failable stamps the shared budget");
}

#[tokio::test(flavor = "current_thread")]
async fn a_pre_send_failure_after_a_placed_leg_reschedules_without_retrying() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.5, 0.3, 0.2]);
    let t = script(json!({ "POST /v2/orders": [accepted("OTX-1"), { "network": "pre_send", "message": PRE_SEND }] }));
    let mut attempts = Attempts::default();
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut attempts).await.unwrap();
    assert!(matches!(out, TickOutcome::Rescheduled), "a retry would replay leg 1: {out:?}");
    assert_eq!(attempts.transient, 0);
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 1);
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none(), "nothing reached Alpaca: the intent is dropped");
    assert_eq!(model::load_bot(&o.primary, id).unwrap().last_failure_kind().as_deref(), Some("transient"));
}

#[tokio::test(flavor = "current_thread")]
async fn the_same_failure_on_the_first_leg_retries_the_whole_tick() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.5, 0.3, 0.2]);
    let t = script(json!({ "POST /v2/orders": [{ "network": "pre_send", "message": PRE_SEND }, accepted("OTX-1"), accepted("OTX-2"), accepted("OTX-3")] }));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::RetryAfter(_)));
    assert!(matches!(run(&o, &venue(&t), id, at(T0) + Duration::seconds(3)).await, TickOutcome::Done { placed: true }));
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn an_ambiguous_leg_keeps_its_intent_and_later_legs_wait() {
    let (_d, o, id, _, assets) = basket_bot(60.0, &[0.5, 0.3, 0.2]);
    let t = script(json!({ "POST /v2/orders": [accepted("OTX-1"), { "network": "post_send", "message": POST_SEND }] }));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::AwaitingReconciliation));
    let intent = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap();
    let eth_ticker = model::ticker_for_asset(&o.primary, &model::load_bot(&o.primary, id).unwrap(), assets[1]).unwrap().unwrap();
    assert_eq!((intent["ticker_id"].as_i64(), intent["base_asset_id"].as_i64()), (Some(eth_ticker.id), Some(assets[1])));
    assert_eq!(notionals(&t).len(), 2, "SOL waits");
    assert_eq!(one::<String>(&o, "SELECT event FROM bot_activity_logs ORDER BY id DESC LIMIT 1"), "placement_ambiguous");
}

#[tokio::test(flavor = "current_thread")]
async fn after_a_settled_recovery_the_tick_ends_until_the_next_checkpoint() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.5, 0.3, 0.2]);
    let t = script(json!({
        "POST /v2/orders": [accepted("OTX-1"), { "network": "post_send", "message": POST_SEND }, accepted("OTX-3"), accepted("OTX-4"), accepted("OTX-5")],
        "GET /v2/orders:by_client_order_id": [not_found()],
        // Leg 1 (BTC 30.00) filled by the next checkpoint's sweep.
        "GET /v2/orders/OTX-1": [ok(json!({ "id": "OTX-1", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "buy", "notional": "30",
                                            "qty": null, "filled_qty": "0.00046875", "filled_avg_price": "64000", "limit_price": null }))],
    }));
    let v = venue(&t);
    assert!(matches!(run(&o, &v, id, at(T0)).await, TickOutcome::AwaitingReconciliation));
    // 20 minutes on, Alpaca's own 404 proves leg 2 absent: the intent is settled and that tick ends there.
    assert!(matches!(run(&o, &v, id, at(T0) + Duration::seconds(1200)).await, TickOutcome::Rescheduled));
    assert_eq!(notionals(&t).len(), 2, "no leg until the next checkpoint");
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none());
    // The next checkpoint: 2 × 60 owed − 30 invested = 90. BTC holds 30 of a 120 portfolio, so the unbought ETH and SOL take
    // their shares plus what legs 2 and 3 did not buy: offsets 30, 36, 24 (Rails carries it the same way, accountable.rb:18-73).
    // BTC's order is 90 × (30 / 90): the 32-digit BigDecimal quotient makes it 29.99…97, floored to 29.99 on the wire, as in Rails.
    assert!(matches!(run(&o, &v, id, at("2026-09-08T10:00:00.5Z")).await, TickOutcome::Done { placed: true }));
    assert_eq!(notionals(&t)[2..].to_vec(), vec![("BTC/USD".into(), "29.99".into()), ("ETH/USD".into(), "36.00".into()), ("SOL/USD".into(), "24.00".into())]);
}

#[tokio::test(flavor = "current_thread")]
async fn one_leg_below_minimum_beside_a_placed_leg_is_one_orders_below_minimum_line() {
    let (_d, o, id, _, _) = basket_bot(3.0, &[0.7, 0.3]); // 2.10 and 0.90: ETH under the 1 USD minimum
    let t = script(json!({}));
    run(&o, &venue(&t), id, at(T0)).await;
    assert_eq!(notionals(&t), vec![("BTC/USD".into(), "2.10".into())]);
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'orders_below_minimum'");
    assert_eq!(serde_json::from_str::<Value>(&details).unwrap(), json!({ "count": 1, "bases": "ETH" }));
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions WHERE status = 2"), 0, "no skipped row when something was placed");
}

#[tokio::test(flavor = "current_thread")]
async fn every_leg_below_minimum_writes_a_skipped_row_each() {
    let (_d, o, id, _, _) = basket_bot(1.2, &[0.7, 0.3]); // 0.84 and 0.36
    let t = script(json!({}));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Done { placed: false }));
    assert!(t.posted_orders().is_empty());
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions WHERE status = 2"), 2);
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'order_skipped' AND level = 1"), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_skipped_leg_is_reported_even_when_a_later_leg_fails() {
    let (_d, o, id, _, assets) = basket_bot(4.8, &[0.5, 0.3, 0.2]); // 2.40, 1.44, 0.96
    o.primary.execute("UPDATE tickers SET minimum_quote_size = '5' WHERE base_asset_id = ?1", [assets[1]]).unwrap(); // ETH skipped
    o.primary.execute("UPDATE tickers SET minimum_quote_size = '0.5' WHERE base_asset_id = ?1", [assets[2]]).unwrap(); // SOL sent
    let t = script(json!({ "POST /v2/orders": [accepted("OTX-1"), { "status": 403, "body": { "code": 40310000, "message": "insufficient buying power" } }] }));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Rescheduled));
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'orders_below_minimum'");
    assert_eq!(serde_json::from_str::<Value>(&details).unwrap(), json!({ "count": 1, "bases": "ETH" }), "Rails' ensure reports it on the failure path");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions WHERE status = 1"), 1, "SOL's failed row");
}

#[tokio::test(flavor = "current_thread")]
async fn a_member_that_stopped_trading_is_exited_and_the_rest_reweighted() {
    let (_d, o, id, _, assets) = basket_bot(60.0, &[0.7, 0.3]);
    basket::refresh_composition(&o.primary, &model::load_bot(&o.primary, id).unwrap(), at("2026-09-01T09:00:00Z")).unwrap().unwrap(); // as the save did
    o.primary.execute("UPDATE tickers SET trading_enabled = 0 WHERE base_asset_id = ?1", [assets[1]]).unwrap();
    let t = script(json!({}));
    run(&o, &venue(&t), id, at(T0)).await;
    assert_eq!(notionals(&t), vec![("BTC/USD".into(), "60.00".into())]);
    let eth: (i64, Option<String>) = o.primary.query_row("SELECT in_index, exited_at FROM bot_index_assets WHERE asset_id = ?1", [assets[1]], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(eth, (0, Some("2026-09-01 10:00:00.500000".into())));
    let btc: f64 = one(&o, &format!("SELECT target_allocation FROM bot_index_assets WHERE asset_id = {}", assets[0]));
    assert_eq!(btc, 1.0);
}

#[tokio::test(flavor = "current_thread")]
async fn stale_reference_data_refuses_the_tick_naming_the_source_and_its_age() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.7, 0.3]);
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2026-08-30 08:00:00'", []).unwrap(); // 50 h before T0
    // An all-time-high write moves tickers.updated_at (technically_analyzable.rb:125) without a sync: still stale.
    o.primary.execute("UPDATE tickers SET ath = 70000, ath_updated_at = '2026-09-01 09:59:00', updated_at = '2026-09-01 09:59:00'", []).unwrap();
    let t = script(json!({}));
    match run(&o, &venue(&t), id, at(T0)).await {
        TickOutcome::Stale { source, message } => {
            assert_eq!(source, "Alpaca crypto tickers");
            assert!(message.contains("exchange_assets.updated_at") && message.contains("50h 0m old") && message.contains("49h bound"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(t.requests().is_empty(), "nothing is asked of Alpaca");
    assert!(model::load_bot(&o.primary, id).unwrap().last_action_job_at_us().unwrap().is_none(), "a refused tick writes nothing");
}

// The engine's own handling: a stale bot is refused on every pass without ending the loop, and ticks once Rails refreshed.
struct ScriptedFactory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for ScriptedFactory {
    type V = AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self, _t: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V { venue(&self.0) }
}

#[tokio::test(flavor = "current_thread")]
async fn a_stale_bot_is_refused_without_ending_the_engine_and_ticks_once_refreshed() {
    let dir = common::rails_install();
    let paths = store::Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&paths, at("2026-09-01T00:00:00Z")).unwrap();
    let o = store::open(&paths).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2026-08-30 08:00:00'", []).unwrap();
    let t = script(json!({}));
    let mut e = deltabadger::engine::run::Engine::new(o.primary, ScriptedFactory(t.clone()), seed::cipher(), lock);
    deltabadger::engine::run::step(&mut e, &FixedClock(at(T0))).await.unwrap();
    deltabadger::engine::run::step(&mut e, &FixedClock(at("2026-09-01T10:01:00Z"))).await.unwrap();
    assert!(t.posted_orders().is_empty());
    e.primary.execute("UPDATE exchange_assets SET updated_at = '2026-09-01 10:15:00'", []).unwrap(); // Rails' sync ran
    deltabadger::engine::run::step(&mut e, &FixedClock(at("2026-09-01T10:16:00Z"))).await.unwrap();
    assert_eq!(t.posted_orders().len(), 1);
}

/// The wait for the next checkpoint after a settled intent survives a restart. Engine A's tick sends one order whose reply is lost, then settles the
/// intent (`landed`: found by its client order id; else Alpaca's 404 after the 20-minute margin). Engine B, a fresh process,
/// places nothing before the next weekly checkpoint, and buys at it.
async fn restart_after_settlement(landed: bool) {
    let dir = common::rails_install();
    let paths = store::Paths::from_env(&|_| None, dir.path());
    let o = store::open(&paths).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    drop(o);
    let t = script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }, accepted("OTX-2")] }));
    if !landed { t.reply("GET /v2/orders:by_client_order_id", 404, not_found()["body"].clone()); }
    let engine = |now: &str| {
        let lock = deltabadger::lease::lock(&paths, at(now)).unwrap();
        deltabadger::engine::run::Engine::new(store::open(&paths).unwrap().primary, ScriptedFactory(t.clone()), seed::cipher(), lock)
    };
    let mut a = engine(T0);
    deltabadger::engine::run::step(&mut a, &FixedClock(at(T0))).await.unwrap();
    assert_eq!(t.posted_orders().len(), 1, "the first order, its reply lost");
    let settle_at = if landed {
        let cl = model::load_bot(&a.primary, id).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
        t.reply("GET /v2/orders:by_client_order_id", 200, json!({ "id": "OTX-1", "client_order_id": cl, "status": "filled", "symbol": "BTC/USD",
            "type": "market", "side": "buy", "notional": "60", "qty": null, "filled_qty": "0.0009375", "filled_avg_price": "64000", "limit_price": null }));
        at("2026-09-01T10:00:31Z")
    } else {
        at("2026-09-01T10:20:01Z")
    };
    deltabadger::engine::run::step(&mut a, &FixedClock(settle_at)).await.unwrap();
    let b = model::load_bot(&a.primary, id).unwrap();
    assert!(b.rust_placement().is_none(), "settled");
    assert_eq!(b.rust_defer_until_us().unwrap(), Some(at("2026-09-08T10:00:00Z").timestamp_micros()), "persisted with the settlement");
    drop(a); // the process ends: everything in memory is gone
    let mut b = engine("2026-09-01T10:30:00Z");
    for now in ["2026-09-01T10:30:00Z", "2026-09-03T10:00:00Z", "2026-09-07T23:59:59Z"] {
        deltabadger::engine::run::step(&mut b, &FixedClock(at(now))).await.unwrap();
        assert_eq!(t.posted_orders().len(), 1, "{now}: nothing before the next checkpoint");
    }
    deltabadger::engine::run::step(&mut b, &FixedClock(at("2026-09-08T10:00:01Z"))).await.unwrap();
    assert_eq!(t.posted_orders().len(), 2, "the next checkpoint buys");
    assert!(model::load_bot(&b.primary, id).unwrap().rust_defer_until_us().unwrap().is_none(), "the deferred tick removes the key");
}

#[tokio::test(flavor = "current_thread")]
async fn a_restart_after_a_placed_settlement_waits_for_the_next_checkpoint() { restart_after_settlement(true).await; }

#[tokio::test(flavor = "current_thread")]
async fn a_restart_after_a_not_placed_settlement_waits_for_the_next_checkpoint() { restart_after_settlement(false).await; }

/// An install at `paths` with a weekly bot of 60 USD over BTC (and ETH, SOL at `weights` when given), its engines built on
/// demand over one shared script, as separate processes would be.
struct Install { _dir: tempfile::TempDir, paths: store::Paths, id: i64, t: ScriptedTransport }
impl Install {
    fn new(weights: &[f64], t: ScriptedTransport) -> Self {
        let dir = common::rails_install();
        let paths = store::Paths::from_env(&|_| None, dir.path());
        let o = store::open(&paths).unwrap();
        let s = seed::seed_alpaca(&o.primary, &seed::cipher());
        let (eth, sol) = seed::add_eth_sol(&o.primary, &s);
        let spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00");
        let spec = if weights.is_empty() { spec } else {
            let assets = [s.btc, eth, sol];
            spec.weights(&weights.iter().enumerate().map(|(i, w)| (assets[i], *w)).collect::<Vec<_>>())
        };
        let id = seed::insert_bot(&o.primary, &s, &spec);
        Self { _dir: dir, paths, id, t }
    }
    fn engine(&self, now: &str) -> deltabadger::engine::run::Engine<ScriptedFactory> {
        let lock = deltabadger::lease::lock(&self.paths, at(now)).unwrap();
        deltabadger::engine::run::Engine::new(store::open(&self.paths).unwrap().primary, ScriptedFactory(self.t.clone()), seed::cipher(), lock)
    }
    fn sql(&self, sql: &str) { store::open(&self.paths).unwrap().primary.execute(sql, []).unwrap(); }
    fn bot(&self) -> model::Bot { model::load_bot(&store::open(&self.paths).unwrap().primary, self.id).unwrap() }
}
async fn step(e: &mut deltabadger::engine::run::Engine<ScriptedFactory>, now: &str) { deltabadger::engine::run::step(e, &FixedClock(at(now))).await.unwrap(); }

/// A settled bot that the user stops and starts fresh (Bot::Lifecycle#start: started_at = now, last_action_job_at = nil)
/// buys at once, as Rails does: the wait was computed under the old start.
#[tokio::test(flavor = "current_thread")]
async fn a_settled_bot_stopped_and_started_fresh_buys_at_once() {
    let i = Install::new(&[], script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }, accepted("OTX-2")] })));
    i.t.reply("GET /v2/orders:by_client_order_id", 404, not_found()["body"].clone());
    let mut e = i.engine(T0);
    step(&mut e, T0).await;
    i.sql("UPDATE bots SET status = 2"); // stopped while its order is unresolved
    step(&mut e, "2026-09-01T10:20:01Z").await; // the idle reconciliation settles it as not placed
    assert!(i.bot().rust_placement().is_none());
    assert!(i.bot().rust_defer_until_us().unwrap().is_some(), "the wait is written for a stopped bot too");
    i.sql("UPDATE bots SET status = 1, started_at = '2026-09-02 09:00:00', transient_data = json_remove(transient_data, '$.last_action_job_at')");
    step(&mut e, "2026-09-02T09:00:01Z").await;
    assert_eq!(i.t.posted_orders().len(), 2, "a fresh start buys at once");
}

/// A settled weekly bot switched to hourly follows the hourly schedule: no week-long stall.
#[tokio::test(flavor = "current_thread")]
async fn a_settled_bot_whose_interval_changes_follows_the_new_schedule() {
    let i = Install::new(&[], script(json!({ "POST /v2/orders": [{ "network": "post_send", "message": POST_SEND }, accepted("OTX-2")] })));
    i.t.reply("GET /v2/orders:by_client_order_id", 404, not_found()["body"].clone());
    let mut e = i.engine(T0);
    step(&mut e, T0).await;
    step(&mut e, "2026-09-01T10:20:01Z").await;
    assert_eq!(i.bot().rust_defer_until_us().unwrap(), Some(at("2026-09-08T10:00:00Z").timestamp_micros()));
    i.sql("UPDATE bots SET settings = json_set(settings, '$.interval', 'hour'), settings_changed_at = '2026-09-01 10:30:00'");
    step(&mut e, "2026-09-01T10:30:01Z").await;
    assert_eq!(i.t.posted_orders().len(), 1, "the 10:00 hour was this run's");
    step(&mut e, "2026-09-01T11:00:01Z").await;
    assert_eq!(i.t.posted_orders().len(), 2, "the next hourly checkpoint buys");
}

/// Any rescheduled run waits for the next checkpoint across a restart, not only a settled one: here a basket whose leg 2
/// was rejected, or failed before it left after leg 1 was placed.
async fn restart_after_a_rescheduled_leg(leg2: Value) {
    let i = Install::new(&[0.5, 0.3, 0.2], script(json!({
        "POST /v2/orders": [accepted("OTX-1"), leg2, accepted("OTX-3"), accepted("OTX-4"), accepted("OTX-5")],
        "GET /v2/orders/OTX-1": [ok(json!({ "id": "OTX-1", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "buy", "notional": "30",
                                            "qty": null, "filled_qty": "0.00046875", "filled_avg_price": "64000", "limit_price": null }))],
    })));
    let mut a = i.engine(T0);
    step(&mut a, T0).await;
    assert_eq!(i.t.posted_orders().len(), 2);
    assert_eq!(i.bot().rust_defer_until_us().unwrap(), Some(at("2026-09-08T10:00:00Z").timestamp_micros()), "persisted with the reschedule");
    drop(a);
    let mut b = i.engine("2026-09-01T10:05:00Z");
    for now in ["2026-09-01T10:05:00Z", "2026-09-07T23:59:59Z"] {
        step(&mut b, now).await;
        assert_eq!(i.t.posted_orders().len(), 2, "{now}: nothing before the next checkpoint");
    }
    step(&mut b, "2026-09-08T10:00:01Z").await;
    assert_eq!(i.t.posted_orders().len(), 5, "the next checkpoint buys every member");
}

#[tokio::test(flavor = "current_thread")]
async fn a_restart_after_a_rejected_middle_leg_waits_for_the_next_checkpoint() {
    restart_after_a_rescheduled_leg(json!({ "status": 403, "body": { "code": 40310000, "message": "insufficient buying power" } })).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_restart_after_a_pre_send_failure_waits_for_the_next_checkpoint() {
    restart_after_a_rescheduled_leg(json!({ "network": "pre_send", "message": PRE_SEND })).await;
}

/// A wait the engine cannot read is a refusal (logged once): the bot does not run and the wait is kept, so an unreadable
/// schedule decision never becomes an early buy (RULING-B2A-R10).
#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_stored_wait_refuses_and_is_kept() {
    let i = Install::new(&[], script(json!({})));
    i.sql("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_defer_until', 'not a time')");
    let mut e = i.engine(T0);
    step(&mut e, T0).await;
    step(&mut e, T0).await;
    assert!(i.t.posted_orders().is_empty());
    assert_eq!(i.bot().transient["rust_defer_until"], "not a time", "kept");
}

/// Alpaca whose AddOrder runs `hook` first: a test's concurrent writer while the order is in flight.
struct Hooked(AlpacaVenue<ScriptedTransport>, Box<dyn Fn()>);
impl deltabadger::venue::Venue for Hooked {
    fn rules(&self) -> &'static deltabadger::engine::venue_rules::VenueRules { self.0.rules() }
    async fn price(&self, t: &model::Ticker, s: deltabadger::venue::PriceSide) -> Result<deltabadger::ruby::BigDec, deltabadger::venue::VenueError> { self.0.price(t, s).await }
    async fn add_order(&self, o: &deltabadger::venue::NewOrder) -> Result<String, deltabadger::venue::VenueError> { (self.1)(); self.0.add_order(o).await }
    async fn orders(&self, ids: &[String]) -> Result<Vec<deltabadger::venue::OrderState>, deltabadger::venue::VenueError> { self.0.orders(ids).await }
    async fn order_by_client_id(&self, cl: &str, since: DateTime<Utc>) -> Result<Option<deltabadger::venue::OrderState>, deltabadger::venue::VenueError> { self.0.order_by_client_id(cl, since).await }
    async fn fills_from_trades(&self, ids: &[String], since: DateTime<Utc>) -> Result<Vec<deltabadger::venue::OrderState>, deltabadger::venue::VenueError> { self.0.fills_from_trades(ids, since).await }
    async fn balance(&self, a: &str, _all_crypto: bool) -> Result<deltabadger::ruby::BigDec, deltabadger::venue::VenueError> { self.0.balance(a, _all_crypto).await }
}

/// A stop always wins over a tick in progress: the order in hand finishes, nothing more is placed. A listed divergence:
/// Rails' leg loop never re-reads the bot's status, so it places the remaining legs.
#[tokio::test(flavor = "current_thread")]
async fn a_web_stop_between_legs_places_nothing_more() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.5, 0.3, 0.2]);
    let db = o.primary.path().unwrap().to_string();
    let t = script(json!({}));
    let v = Hooked(venue(&t), Box::new(move || {
        rusqlite::Connection::open(&db).unwrap().execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap();
    }));
    run_on(&o, &v, id, at(T0)).await;
    assert_eq!(notionals(&t), vec![("BTC/USD".into(), "30.00".into())], "leg 1 finishes; legs 2 and 3 are never sent");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 1);
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Stopped);
}
async fn run_on<V: deltabadger::venue::Venue>(o: &store::Opened, v: &V, id: i64, when: DateTime<Utc>) -> TickOutcome {
    tick::tick(&o.primary, v, id, &FixedClock(when), &mut Attempts::default()).await.unwrap()
}

/// A retrying bot whose retry time has passed meets stale reference data: the engine rechecks it after a bounded wait instead
/// of folding the expired retry time into its wake (which made every pass come back at once), and ticks it at the recheck
/// once Rails refreshed the catalog.
#[tokio::test(flavor = "current_thread")]
async fn a_stale_retrying_bot_is_rechecked_after_a_bounded_wait_not_in_a_tight_loop() {
    let dir = common::rails_install();
    let paths = store::Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&paths, at("2026-09-01T00:00:00Z")).unwrap();
    let o = store::open(&paths).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 5, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2026-08-30 08:00:00'", []).unwrap();
    let t = script(json!({}));
    let mut e = deltabadger::engine::run::Engine::new(o.primary, ScriptedFactory(t.clone()), seed::cipher(), lock);
    e.inject_stale_retry(id, at("2026-09-01T09:59:00Z").timestamp_micros()); // its retry time has passed
    let us = |s: &str| at(s).timestamp_micros();
    let recheck = deltabadger::engine::run::STALE_RECHECK_US;
    for now in ["2026-09-01T10:00:00.5Z", "2026-09-01T10:00:01.5Z"] { // two consecutive passes
        let wake = deltabadger::engine::run::step(&mut e, &FixedClock(at(now))).await.unwrap();
        assert!(wake >= us(now) + 60_000_000.min(recheck), "pass at {now}: wake {} µs after now, not a tight loop", wake - us(now));
    }
    assert!(t.posted_orders().is_empty());
    e.primary.execute("UPDATE exchange_assets SET updated_at = '2026-09-01 10:01:00'", []).unwrap(); // Rails' sync ran
    deltabadger::engine::run::step(&mut e, &FixedClock(at("2026-09-01T10:02:00Z"))).await.unwrap();
    assert!(t.posted_orders().is_empty(), "not before the recheck");
    deltabadger::engine::run::step(&mut e, &FixedClock(DateTime::from_timestamp_micros(us("2026-09-01T10:00:00.5Z") + recheck).unwrap())).await.unwrap();
    assert_eq!(t.posted_orders().len(), 1, "ticked at the recheck");
}

#[tokio::test(flavor = "current_thread")]
async fn an_owned_restart_keeps_ticks_refused_until_a_complete_scheduler_refresh() -> Result<(), Box<dyn std::error::Error>> {
    use deltabadger::engine::{handover, run};
    use deltabadger::jobs::{reference, state};
    let (dir, o, s) = common::install_alpaca();
    let paths = store::Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&paths, at(T0)).map_err(|e| format!("{e:?}"))?;
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    handover::take_over(&lock, &o, &seed::cipher(), "test", at(T0)).map_err(|e| format!("{e:?}"))?;
    state::record_success(&o.primary, reference::ALPACA_CRYPTO, None, at("2026-08-01T00:00:00Z"))?;
    o.primary.execute("UPDATE app_configs SET value = json_set(value, '$.rails_at', NULL) WHERE key = ?1", [state::key(reference::ALPACA_CRYPTO, None)])?;
    handover::take_over(&lock, &o, &seed::cipher(), "test", at(T0)).map_err(|e| format!("{e:?}"))?;
    let t = script(json!({}));
    let mut engine = run::Engine::new(o.primary, ScriptedFactory(t.clone()), seed::cipher(), lock);
    for time in [T0, "2026-09-01T10:01:00Z"] {
        run::step(&mut engine, &FixedClock(at(time))).await.map_err(|e| format!("{e:?}"))?;
    }
    assert!(t.requests().is_empty(), "stale bots make no venue request after restart");
    // A partial refresh still cannot permit a tick. The scheduler's complete-success record is the freshness boundary.
    state::mark_incomplete(&engine.primary, reference::ALPACA_CRYPTO, None, at("2026-09-01T10:02:00Z"))?;
    engine.primary.execute("UPDATE exchange_assets SET updated_at = '2026-09-01 10:02:00'", [])?;
    run::step(&mut engine, &FixedClock(at("2026-09-01T10:03:00Z"))).await.map_err(|e| format!("{e:?}"))?;
    assert!(t.requests().is_empty());
    state::record_success(&engine.primary, reference::ALPACA_CRYPTO, None, at("2026-09-01T10:04:00Z"))?;
    run::step(&mut engine, &FixedClock(at("2026-09-01T10:05:00Z"))).await.map_err(|e| format!("{e:?}"))?;
    assert_eq!(t.posted_orders().len(), 1);
    Ok(())
}

struct ChangeDuringPrice { inner: ScriptedTransport, db: String }
impl deltabadger::venue::http::Transport for ChangeDuringPrice {
    async fn send(&self, r: &deltabadger::venue::http::HttpRequest) -> Result<deltabadger::venue::http::HttpResponse, deltabadger::venue::http::TransportError> {
        if r.path.ends_with("/quotes") {
            // ETH's ticker was read with the whole basket before the first (BTC) price await.
            rusqlite::Connection::open(&self.db).unwrap().execute("UPDATE tickers SET price_decimals = 4 WHERE ticker = 'ETH/USD'", []).unwrap();
        }
        self.inner.send(r).await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_changed_other_member_ticker_fences_the_whole_sized_basket() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.7, 0.3]);
    let t = script(json!({}));
    let transport = ChangeDuringPrice { inner: t.clone(), db: o.primary.path().unwrap().into() };
    let v = AlpacaVenue::new(transport, deltabadger::venue::alpaca::Urls::for_passphrase(Some("paper")));
    let out = tick::tick(&o.primary, &v, id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: false }), "{out:?}");
    assert!(t.posted_orders().is_empty(), "even the unchanged BTC leg was sized from the changed basket");
    assert!(model::load_bot(&o.primary, id).unwrap().rust_placement().is_none());
}
