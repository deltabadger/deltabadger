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
    assert!(i.bot().rust_defer_until_us().unwrap().is_none(), "recovery cannot write a deferral after STOP");
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

fn stop_after_leg(c: &rusqlite::Connection, id: i64, restart: bool) {
    common::after_order_commit(c, id, move |writer| {
        writer.execute(if restart {
            "UPDATE bots SET status=4, transient_data=json_set(transient_data,'$.run_generation',1,'$.last_action_job_at','2026-09-01T10:00:01.000Z') WHERE id=?1"
        } else { "UPDATE bots SET status=2 WHERE id=?1" }, [id])?;
        Ok(())
    });
}

/// STOP between committed legs preserves the first order and prevents later submissions on both engines.
#[tokio::test(flavor = "current_thread")]
async fn a_web_stop_between_legs_places_nothing_more() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.5, 0.3, 0.2]);
    let t = script(json!({}));
    stop_after_leg(&o.primary, id, false);
    run_on(&o, &venue(&t), id, at(T0)).await;
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

/// Automation::Statusable::FENCED: a tick whose run was taken by a stop and a restart places nothing more and writes nothing
/// after it, so the restarted run's own claim (executing, its last_action_job_at) survives the old tick.
#[tokio::test(flavor = "current_thread")]
async fn a_tick_fenced_by_a_restart_leaves_the_new_run_alone() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.5, 0.5]);
    let t = script(json!({}));
    stop_after_leg(&o.primary, id, true);
    let out = run_on(&o, &venue(&t), id, at(T0)).await;
    assert!(matches!(out, TickOutcome::Skipped), "{out:?}");
    assert_eq!(notionals(&t).len(), 1, "leg 2 was sized on the old run");
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(b.status, BotStatus::Executing, "no :waiting or :scheduled over the new run's claim");
    assert_eq!(b.transient["last_action_job_at"], "2026-09-01T10:00:01.000Z");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 1, "leg 1 stands");
}

/// Pauses at the clock read before claim or completion, after the preceding work committed.
struct OwnershipClock<'a> { c: &'a rusqlite::Connection, id: i64, after_placement: bool, fired: std::cell::Cell<bool> }
impl deltabadger::engine::Clock for OwnershipClock<'_> {
    fn now(&self) -> DateTime<Utc> {
        let ready: bool = self.c.query_row(if self.after_placement {
            "SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id=?1 AND status=0)"
        } else {
            "SELECT status=1 AND json_extract(transient_data,'$.last_action_job_at') IS NOT NULL FROM bots WHERE id=?1"
        }, [self.id], |r| r.get(0)).unwrap();
        if ready && !self.fired.replace(true) {
            assert!(self.c.is_autocommit(), "the pause must be outside the dispatch transaction");
            self.c.execute("UPDATE bots SET status=4, transient_data=json_set(transient_data,'$.run_generation',1,'$.last_action_job_at','2026-09-01T10:00:01.000Z') WHERE id=?1", [self.id]).unwrap();
        }
        at(T0)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ownership_pre_claim_pause_cannot_adopt_a_restarted_run() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.5, 0.5]);
    let t = script(json!({}));
    let clock = OwnershipClock { c: &o.primary, id, after_placement: false, fired: std::cell::Cell::new(false) };
    let out = tick::tick(&o.primary, &venue(&t), id, &clock, &mut Attempts::default()).await.unwrap();
    assert!(clock.fired.get(), "pause reached");
    assert!(t.posted_orders().is_empty(), "{out:?}: the old $60 tick must not trade");
    let b = model::load_bot(&o.primary,id).unwrap();
    assert_eq!(b.status, BotStatus::Executing);
    assert_eq!(b.transient["last_action_job_at"], "2026-09-01T10:00:01.000Z");
}

#[tokio::test(flavor = "current_thread")]
async fn ownership_final_leg_cannot_complete_over_a_restarted_claim() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[1.0]);
    let t = script(json!({}));
    let clock = OwnershipClock { c: &o.primary, id, after_placement: true, fired: std::cell::Cell::new(false) };
    let out = tick::tick(&o.primary, &venue(&t), id, &clock, &mut Attempts::default()).await.unwrap();
    assert!(clock.fired.get());
    assert_eq!(t.posted_orders().len(),1);
    assert!(matches!(out,TickOutcome::Skipped), "{out:?}");
    assert_eq!(model::load_bot(&o.primary,id).unwrap().status,BotStatus::Executing);
}

struct OwnershipDuringPrice { inner: ScriptedTransport, db: String, id: i64 }
impl deltabadger::venue::http::Transport for OwnershipDuringPrice {
    async fn send(&self, r: &deltabadger::venue::http::HttpRequest) -> Result<deltabadger::venue::http::HttpResponse, deltabadger::venue::http::TransportError> {
        if r.path.ends_with("/quotes") {
            rusqlite::Connection::open(&self.db).unwrap().execute("UPDATE bots SET status=4, transient_data=json_set(transient_data,'$.run_generation',1,'$.last_action_job_at','2026-09-01T10:00:01.000Z') WHERE id=?1", [self.id]).unwrap();
        }
        self.inner.send(r).await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ownership_fenced_tick_skips_failing_balance_and_failure_writes() {
    let (_d, o, id, _, _) = basket_bot(60.0, &[0.5, 0.5]);
    let t = script(json!({"GET /v2/account":[{"status":200,"body":"not json"}]}));
    let v = AlpacaVenue::new(OwnershipDuringPrice { inner:t.clone(),db:o.primary.path().unwrap().into(),id }, deltabadger::venue::alpaca::Urls::for_passphrase(Some("paper")));
    let out=run_on(&o,&v,id,at(T0)).await;
    assert!(matches!(out,TickOutcome::Skipped),"{out:?}");
    assert_eq!(model::load_bot(&o.primary,id).unwrap().status,BotStatus::Executing);
    assert!(!t.requests().iter().any(|r|r.path=="/v2/account"),"a fenced tick never reads balances");
}

struct OwnershipDuringVenueCreation { inner: ScriptedTransport, db: String, id: i64 }
impl deltabadger::venue::VenueFactory for OwnershipDuringVenueCreation {
    type V = AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self, _: &str, _: Option<deltabadger::crypto::Credentials>) -> Self::V {
        // The engine already read its due bot. STOP + resume now creates a different scheduled run.
        rusqlite::Connection::open(&self.db).unwrap().execute(
            "UPDATE bots SET status=1, transient_data=json_set(transient_data,'$.run_generation',1) WHERE id=?1", [self.id]).unwrap();
        venue(&self.inner)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ownership_engine_keeps_the_run_selected_before_venue_creation() {
    use deltabadger::engine::run::{self, Engine};
    let (d, o, id, _, _) = basket_bot(60.0, &[1.0]);
    let t = script(json!({}));
    let factory = OwnershipDuringVenueCreation { inner: t.clone(), db:o.primary.path().unwrap().into(), id };
    let paths=store::Paths::from_env(&|_|None,d.path());
    let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
    let mut engine=Engine::new(o.primary,factory,seed::cipher(),lock);
    run::step(&mut engine,&FixedClock(at(T0))).await.unwrap();
    assert!(t.posted_orders().is_empty(),"the pass cannot adopt the run created while building its venue");
    let bot=model::load_bot(&engine.primary,id).unwrap();
    assert_eq!(bot.status,BotStatus::Scheduled);
    assert!(bot.last_action_job_at_us().unwrap().is_none());
}

// ---- 6b T4: a wider basket's DCA-out (Bot::Composition::OrderSetter#get_sell_orders_data) ----

const SELL_AT: &str = "2026-09-01T10:00:01Z";
/// A 50/50 BTC/ETH basket selling for `quote` USD a week, holding 0.01 BTC (640 USD) and 0.02 ETH (50 USD).
fn basket_seller(quote: f64) -> (tempfile::TempDir, store::Opened, i64, seed::Seeded, i64) {
    let (d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00").weights(&[(s.btc, 0.5), (eth, 0.5)])
        .with("direction", json!("selling")).with("sell_denomination", json!("quote")).with("sell_quote_amount", json!(quote));
    let id = seed::insert_bot(&o.primary, &s, &spec);
    let buy = |n: &str, amount: &str, value: &str, price: &str| json!({ "status": 0, "external_status": 2, "external_id": n, "order_type": 0, "amount": amount,
        "amount_exec": amount, "quote_amount": value, "quote_amount_exec": value, "price": price, "created_at": "2026-09-01 09:00:00" });
    seed::insert_row(&o.primary, &s, id, s.btc, &buy("OB-BTC", "0.01", "640", "64000"));
    seed::insert_row(&o.primary, &s, id, eth, &buy("OB-ETH", "0.02", "50", "2500"));
    (d, o, id, s, eth)
}
fn position(symbol: &str, qty: &str, available: &str) -> Value { json!({ "asset_class": "crypto", "symbol": symbol, "qty": qty, "qty_available": available }) }
/// Bids 64000 and 2500, both held free unless `positions` says otherwise.
fn selling(positions: Value, over: Value) -> ScriptedTransport {
    let mut m = json!({ "GET /v1beta3/crypto/us/latest/quotes": [ok(json!({ "quotes": { "BTC/USD": { "ap": 64010, "bp": 64000 }, "ETH/USD": { "ap": 2501, "bp": 2500 } } }))],
                        "GET /v2/positions": [ok(positions)] });
    for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
    script(m)
}
fn sells(t: &ScriptedTransport) -> Vec<(String, String, String)> {
    t.posted_orders().iter().map(|o| (o["symbol"].as_str().unwrap().into(), o["side"].as_str().unwrap().into(), o["qty"].as_str().unwrap_or("").into())).collect()
}
fn positions_read(t: &ScriptedTransport) -> usize { t.requests().iter().filter(|r| r.method == "GET" && r.path == "/v2/positions").count() }
fn both_free() -> Value { json!([position("BTCUSD", "0.01", "0.01"), position("ETHUSD", "0.02", "0.02")]) }

#[tokio::test(flavor = "current_thread")]
async fn a_basket_sells_its_withdrawal_out_of_the_most_overweight_member_only() {
    let (_d, o, id, _, _) = basket_seller(100.0);
    let t = selling(both_free(), json!({}));
    assert!(matches!(run(&o, &venue(&t), id, at(SELL_AT)).await, TickOutcome::Done { placed: true }));
    // 690 held, 590 left after the withdrawal: BTC sits 345 over its half, ETH 245 under. 100 / 64000 of BTC.
    assert_eq!(sells(&t), vec![("BTC/USD".into(), "sell".into(), "0.001562500".into())]);
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions WHERE side = 1 AND status = 0"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_top_member_under_the_floor_hands_the_withdrawal_to_the_next() {
    let (_d, o, id, _, _) = basket_seller(100.0);
    // Only 0.00001 BTC is free (Alpaca's floor is 0.000027 here): ETH sells what it holds.
    let t = selling(json!([position("BTCUSD", "0.01", "0.00001"), position("ETHUSD", "0.02", "0.02")]), json!({}));
    assert!(matches!(run(&o, &venue(&t), id, at(SELL_AT)).await, TickOutcome::Done { placed: true }));
    assert_eq!(sells(&t), vec![("ETH/USD".into(), "sell".into(), "0.020000000".into())], "min(100 / 2500, 0.02 held)");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions WHERE status = 2"), 0, "a placed order: no skipped row");
}

#[tokio::test(flavor = "current_thread")]
async fn a_member_free_below_what_the_bot_holds_sells_what_is_free_and_the_rest_moves_on() {
    let (_d, o, id, _, _) = basket_seller(100.0);
    let t = selling(json!([position("BTCUSD", "0.01", "0.001"), position("ETHUSD", "0.02", "0.02")]), json!({}));
    run(&o, &venue(&t), id, at(SELL_AT)).await;
    // 0.001 BTC is 64 USD; the 36 left is 0.0144 ETH.
    assert_eq!(sells(&t), vec![("BTC/USD".into(), "sell".into(), "0.001000000".into()), ("ETH/USD".into(), "sell".into(), "0.014400000".into())]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_resting_sell_counts_against_the_member_it_sells() {
    let (_d, o, id, s, _) = basket_seller(100.0);
    seed::insert_row(&o.primary, &s, id, s.btc, &json!({ "status": 0, "external_status": 1, "external_id": "OREST", "order_type": 1, "side": 1, "amount": "0.009",
        "amount_exec": "0", "quote_amount_exec": "0", "price": "65000", "created_at": "2026-09-01 09:30:00" }));
    let rest = ok(json!({ "id": "OREST", "status": "new", "symbol": "BTC/USD", "asset_class": "crypto", "side": "sell", "type": "limit", "order_type": "limit",
                          "qty": "0.009", "filled_qty": "0", "limit_price": "65000", "time_in_force": "gtc" }));
    let t = selling(both_free(), json!({ "GET /v2/orders/OREST": [rest] }));
    run(&o, &venue(&t), id, at(SELL_AT)).await;
    // BTC is worth 64 net of the 0.009 resting and holds 0.001 beyond it: 0.001 sells, and 36 USD of ETH.
    assert_eq!(sells(&t), vec![("BTC/USD".into(), "sell".into(), "0.001000000".into()), ("ETH/USD".into(), "sell".into(), "0.014400000".into())]);
}

#[tokio::test(flavor = "current_thread")]
async fn every_member_under_the_floor_reports_the_first_once() {
    let (_d, o, id, _, _) = basket_seller(0.5);
    let t = selling(both_free(), json!({}));
    assert!(matches!(run(&o, &venue(&t), id, at(SELL_AT)).await, TickOutcome::Done { placed: false }));
    assert!(sells(&t).is_empty());
    assert_eq!(one::<String>(&o, "SELECT base FROM transactions WHERE status = 2"), "BTC", "one skipped row: the top-ranked member's");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'order_skipped'"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn holdings_the_venue_cannot_price_retry_before_anything_is_sold() {
    let (_d, o, id, _, _) = basket_seller(100.0);
    let t = selling(both_free(), json!({ "GET /v1beta3/crypto/us/latest/trades": [ok(json!({ "trades": { "BTC/USD": { "p": 64000 } } }))] }));
    assert!(matches!(run(&o, &venue(&t), id, at(SELL_AT)).await, TickOutcome::RetryAfter(_)), "ETH held and unpriced: no valuation");
    assert!(sells(&t).is_empty());
    assert_eq!(positions_read(&t), 0, "no balance is read");
}

#[tokio::test(flavor = "current_thread")]
async fn a_basket_with_nothing_held_skips_without_reading_balances() {
    let (_d, o, id, _, _) = basket_seller(100.0);
    o.primary.execute("DELETE FROM transactions", []).unwrap();
    let t = selling(both_free(), json!({}));
    assert!(matches!(run(&o, &venue(&t), id, at(SELL_AT)).await, TickOutcome::Done { placed: false }));
    assert!(sells(&t).is_empty());
    assert_eq!((positions_read(&t), one::<i64>(&o, "SELECT count(*) FROM transactions")), (0, 0));
}

#[tokio::test(flavor = "current_thread")]
async fn a_price_this_build_cannot_hold_is_no_valuation_and_retries() {
    let (_d, o, id, _, _) = basket_seller(100.0);
    // Finite to Ruby's BigDecimal (Rails values it), beyond `dec`'s limits here: never a guessed ranking.
    let t = selling(both_free(), json!({ "GET /v1beta3/crypto/us/latest/trades": [ok(json!({ "trades": { "BTC/USD": { "p": 64000 }, "ETH/USD": { "p": "1e1000000" } } }))] }));
    assert!(matches!(run(&o, &venue(&t), id, at(SELL_AT)).await, TickOutcome::RetryAfter(_)));
    assert!(sells(&t).is_empty());
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'execution_failed'"), 0);
}

// ---- 6b T3: Bot::Rebalancer#live_free_balance caps a merged seller at what it bought on this venue ----

/// A 1.0 `asset` buy a merge brought in from another venue.
fn foreign_buy(o: &store::Opened, s: &seed::Seeded, id: i64, asset: i64, amount: &str, price: &str) {
    o.primary.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Binance', 'Binance', '0.1', '0.1', '2026-08-01', '2026-08-01')", []).unwrap();
    let binance = o.primary.last_insert_rowid();
    let row = seed::insert_row(&o.primary, s, id, asset, &json!({ "status": 0, "external_status": 2, "external_id": "OFOREIGN", "order_type": 0, "amount": amount,
        "amount_exec": amount, "price": price, "created_at": "2026-09-01 08:00:00" }));
    o.primary.execute("UPDATE transactions SET exchange_id = ?1 WHERE id = ?2", rusqlite::params![binance, row]).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn a_merged_crypto_seller_sells_no_more_than_its_net_local_units() {
    let (_d, o, id, s, _) = basket_seller(6000.0);
    foreign_buy(&o, &s, id, s.btc, "1.0", "60000");
    let t = selling(json!([position("BTCUSD", "0.5", "0.5"), position("ETHUSD", "0.02", "0.02")]), json!({}));
    run(&o, &venue(&t), id, at(SELL_AT)).await;
    // 6000 / 64000 = 0.09375 BTC wanted, 0.5 free, but only the 0.01 bought here is this venue's to sell; 5360 is left for ETH.
    assert_eq!(sells(&t), vec![("BTC/USD".into(), "sell".into(), "0.010000000".into()), ("ETH/USD".into(), "sell".into(), "0.020000000".into())]);
}

/// A 50/50 AAPL/ETH basket selling 6000 USD: 1 AAPL and 0.02 ETH bought here, plus (`merged`) 100 AAPL a merge brought in;
/// Alpaca holds 50 AAPL free. Returns what was sent and the request paths.
async fn share_seller(merged: bool) -> (Vec<(String, String, String)>, Vec<String>) {
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    // The venue lists it (exchange.assets), so a position names it.
    o.primary.execute("INSERT INTO exchange_assets (exchange_id, asset_id, available, created_at, updated_at) VALUES (?1, ?2, 1, '2026-09-01 09:00:00', '2026-09-01 09:00:00')",
                      rusqlite::params![s.exchange_id, aapl]).unwrap();
    let spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00").weights(&[(aapl, 0.5), (eth, 0.5)])
        .with("direction", json!("selling")).with("sell_denomination", json!("quote")).with("sell_quote_amount", json!(6000.0));
    let id = seed::insert_bot(&o.primary, &s, &spec);
    let buy = |n: &str, amount: &str, price: &str| json!({ "status": 0, "external_status": 2, "external_id": n, "order_type": 0, "amount": amount,
        "amount_exec": amount, "price": price, "created_at": "2026-09-01 09:00:00" });
    seed::insert_row(&o.primary, &s, id, aapl, &buy("OB-AAPL", "1", "180"));
    seed::insert_row(&o.primary, &s, id, eth, &buy("OB-ETH", "0.02", "2500"));
    if merged { foreign_buy(&o, &s, id, aapl, "100", "180"); }
    seed::fresh_stock_jobs(&o.primary, at(SELL_AT));
    let open = json!({ "timestamp": "2026-09-01T06:00:01-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" });
    let t = selling(json!([{ "asset_class": "us_equity", "symbol": "AAPL", "qty": "50", "qty_available": "50" }, position("ETHUSD", "0.02", "0.02")]), json!({
        "GET /v2/clock": [ok(open)],
        "GET /v2/stocks/snapshots": [ok(json!({ "AAPL": { "latestTrade": { "p": 187.41 } } }))],
        "GET /v2/stocks/AAPL/quotes/latest": [ok(json!({ "quote": { "ap": 187.43, "bp": 187.4 } }))],
        "GET /v2/stocks/AAPL/trades/latest": [ok(json!({ "trade": { "p": 187.41 } }))],
    }));
    let out = run(&o, &venue(&t), id, at(SELL_AT)).await;
    let paths = std::iter::once(format!("{out:?}")).chain(t.requests().iter().map(|r| format!("{} {}", r.method, r.path))).collect();
    (sells(&t), paths)
}

#[tokio::test(flavor = "current_thread")]
async fn a_share_bought_here_sells_from_the_top_of_the_ranking() {
    let (sold, paths) = share_seller(false).await;
    // The 1 share held here (187.4 USD), then ETH for the rest of the withdrawal.
    assert_eq!(sold.iter().map(|o| o.0.as_str()).collect::<Vec<_>>(), vec!["AAPL", "ETH/USD"], "{paths:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_merged_seller_of_a_share_sells_none_of_it_here() {
    let (sold, paths) = share_seller(true).await;
    // AAPL ranks first and 50 shares are free, but a sum of fills is not what a share holds after a split: none sells here.
    assert_eq!(sold, vec![("ETH/USD".into(), "sell".into(), "0.020000000".into())], "{paths:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_resting_sell_lowers_its_members_rank() {
    let (_d, o, id, s, _) = basket_seller(10.0);
    seed::insert_row(&o.primary, &s, id, s.btc, &json!({ "status": 0, "external_status": 1, "external_id": "OREST", "order_type": 1, "side": 1, "amount": "0.0095",
        "amount_exec": "0", "quote_amount_exec": "0", "price": "65000", "created_at": "2026-09-01 09:30:00" }));
    let rest = ok(json!({ "id": "OREST", "status": "new", "symbol": "BTC/USD", "asset_class": "crypto", "side": "sell", "type": "limit", "order_type": "limit",
                          "qty": "0.0095", "filled_qty": "0", "limit_price": "65000", "time_in_force": "gtc" }));
    let t = selling(both_free(), json!({ "GET /v2/orders/OREST": [rest] }));
    run(&o, &venue(&t), id, at(SELL_AT)).await;
    // BTC is worth 32 USD net of its resting 0.0095 (640 gross), under ETH's 50: ETH is the most overweight. 10 / 2500.
    assert_eq!(sells(&t), vec![("ETH/USD".into(), "sell".into(), "0.004000000".into())]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_wider_basket_sells_its_quote_sentence_whatever_denomination_is_stored() {
    let (_d, o, id, _, _) = basket_seller(100.0);
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.sell_denomination', 'base', '$.sell_amount', 0.01) WHERE id = ?1", [id]).unwrap();
    let t = selling(both_free(), json!({}));
    run(&o, &venue(&t), id, at(SELL_AT)).await;
    // Bots::DcaMultiAsset#sell_denomination: 'quote' unless one_asset?. 100 / 64000 of BTC, never 0.01 BTC.
    assert_eq!(sells(&t), vec![("BTC/USD".into(), "sell".into(), "0.001562500".into())]);
}

// Slice 7a: the price rule's gate, between the restatement stand-down and the sweep (dca_multi_asset.rb:34-44).

/// A one-asset BTC basket of 60 USD a week, with a buy-side price rule `below <limit>`; BTC trades at 64000.
fn ruled(limit: f64, transient: Value) -> (tempfile::TempDir, store::Opened, i64, seed::Seeded) {
    let (d, o, s) = common::install_alpaca();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("price_limited", json!(true)).with("price_limit", json!(limit))
        .with("price_limit_value_condition", json!("below"));
    spec.transient = transient;
    let id = seed::insert_bot(&o.primary, &s, &spec);
    (d, o, id, s)
}
fn events(o: &store::Opened) -> Vec<(String, Value)> {
    o.primary.prepare("SELECT event, details FROM bot_activity_logs ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, serde_json::from_str(&r.get::<_, String>(1)?).unwrap()))).unwrap().map(Result::unwrap).collect()
}
fn transient(o: &store::Opened, id: i64) -> Value { model::load_bot(&o.primary, id).unwrap().transient }

#[tokio::test(flavor = "current_thread")]
async fn an_unmet_price_rule_pauses_the_bot_without_sweeping_its_orders() {
    let (_d, o, id, s) = ruled(60_000.0, json!({}));
    seed::insert_tx(&o.primary, &s, id, &seed::TxSpec { status: 0, external_status: Some(0), external_id: Some("OTX-OPEN".into()), order_type: 0, amount: None,
        quote_amount: Some("60"), price: Some("64000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-08-25 10:00:01".into() });
    let t = script(json!({}));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Paused));
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Waiting, "waiting for its check, never back to scheduled");
    assert!(t.posted_orders().is_empty());
    assert!(!t.requests().iter().any(|r| r.path.starts_with("/v2/orders/")), "a paused bot sweeps nothing");
    let ev: Vec<String> = events(&o).into_iter().map(|(e, d)| format!("{e} {d}")).collect();
    assert_eq!(ev, vec![r#"limit_paused {"limit_type":"price"}"#.to_string(), "reschedule_disabled {}".into()]);
}

/// A read that could not be made is not "unmet": the pause keeps met_at and the carry (price_limitable.rb:202-206); only a
/// read price that misses the rule captures the carry and clears met_at, in one write.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_read_pauses_without_touching_met_at_or_the_carry() {
    let met = json!({ "price_limit_condition_met_at": "2026-08-31T10:00:00.000Z" });
    let (_d, o, id, _) = ruled(60_000.0, met.clone());
    let t = script(json!({ "GET /v1beta3/crypto/us/latest/trades": [{ "status": 500, "body": { "code": 50010000, "message": "internal server error" } }] }));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Paused));
    let td = transient(&o, id);
    assert_eq!((td.get("price_limit_condition_met_at"), td.get("missed_quote_amount")), (Some(&json!("2026-08-31T10:00:00.000Z")), None), "{td}");

    let (_d, o, id, _) = ruled(60_000.0, met);
    assert!(matches!(run(&o, &venue(&script(json!({}))), id, at(T0)).await, TickOutcome::Paused));
    let td = transient(&o, id);
    // One interval owed since the start (met before it): 60.0, captured as Rails stores it, and met_at cleared.
    assert_eq!((td.get("price_limit_condition_met_at"), td.get("missed_quote_amount"), td.get("missed_quote_amount_was_set")),
               (Some(&Value::Null), Some(&json!(60.0)), Some(&json!(true))), "{td}");
}

/// A rejected key escapes the gate (limit_checkable.rb:29-33): the tick fails as invalid_key instead of parking the bot.
#[tokio::test(flavor = "current_thread")]
async fn a_rejected_key_fails_the_tick_instead_of_pausing() {
    let (_d, o, id, _) = ruled(60_000.0, json!({}));
    let t = script(json!({ "GET /v1beta3/crypto/us/latest/trades": [{ "status": 401, "body": { "code": 40110000, "message": "unauthorized." } }] }));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Rescheduled));
    let b = model::load_bot(&o.primary, id).unwrap();
    assert_eq!((b.status, b.last_failure_kind().as_deref()), (BotStatus::Retrying, Some("invalid_key")));
}

/// Met for the first time, off the grid: met_at is stamped and the tick sizes from the row the stamp left, so it buys the
/// carry alone (no interval since met_at); with no carry it buys nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_first_satisfaction_sizes_from_the_stamped_row() {
    let (_d, o, id, _) = ruled(70_000.0, json!({ "missed_quote_amount": "30" }));
    let t = script(json!({}));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Done { placed: true }));
    assert_eq!(notionals(&t), vec![("BTC/USD".into(), "30.00".into())]);
    assert_eq!(transient(&o, id)["price_limit_condition_met_at"], json!("2026-09-01T10:00:00.500Z"));

    let (_d, o, id, _) = ruled(70_000.0, json!({}));
    let t = script(json!({}));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Done { placed: false }));
    assert!(t.posted_orders().is_empty(), "nothing owed since the condition was met");
}

/// Bot::Fundable wraps the condition (fundable.rb:5-17): a pause still checks the quote balance, and a failed balance read
/// leaves the pause a pause.
#[tokio::test(flavor = "current_thread")]
async fn a_pause_still_runs_the_funds_check() {
    let (_d, o, id, _) = ruled(60_000.0, json!({}));
    let t = script(json!({ "GET /v2/account": [ok(json!({ "cash": "0", "non_marginable_buying_power": "0" }))] }));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Paused));
    assert!(one::<bool>(&o, "SELECT last_end_of_funds_notification IS NOT NULL FROM bots"), "out of funds while paused");
    assert!(transient(&o, id).get(tick::FUNDS_MAIL_PENDING).is_some());

    let (_d, o, id, _) = ruled(60_000.0, json!({}));
    let t = script(json!({ "GET /v2/account": [{ "status": 500, "body": { "code": 50010000, "message": "internal server error" } }] }));
    assert!(matches!(run(&o, &venue(&t), id, at(T0)).await, TickOutcome::Paused));
    assert!(t.requests().iter().any(|r| r.path == "/v2/account"), "the balance was read");
    assert_eq!(model::load_bot(&o.primary, id).unwrap().status, BotStatus::Waiting);
}

/// P1-T0: buying baskets of two and three members on Kraken (EUR) and Binance (USDT), replayed against Rails' own decisions
/// (script/rust/decisions.rb grid-venue-basket): each member priced and placed on its own pair (Kraken's XBTEUR), sized with
/// its own decimals, a leg under its own minimum skipped and carried to the next checkpoint, a member that stops trading
/// exited, one unpriced member failing the whole tick. Every scenario's bot is one the engine admits.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn kraken_binance_and_kucoin_baskets_decide_as_rails() {
    use common::decisions::{built_grid, copy_dir, r5_cleanup_expected, rails};
    use deltabadger::engine::eligibility;
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    built_grid("grid-venue-basket", rails_root.path());
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 44, "the venue basket grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);
    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rust_dir = rust_root.path().join(&name);
        let admitted = {
            let c = rusqlite::Connection::open(rust_dir.join("production.sqlite3")).unwrap();
            eligibility::check_install(&c).unwrap()
        };
        if !admitted.problems.is_empty() || admitted.eligible.len() != 1 { failures.push(format!("{name}: refused {:?}", admitted.problems)); continue; }
        // The Smart Intervals floor of a basket of several members is its members' largest venue minimum, as Rails computes it.
        {
            use deltabadger::web::bot::{start, Bot as WebBot, For};
            let c = rusqlite::Connection::open(rust_dir.join("production.sqlite3")).unwrap();
            let scenario: Value = serde_json::from_str(&std::fs::read_to_string(rust_dir.join("scenario.json")).unwrap()).unwrap();
            let (user, id): (i64, i64) = c.query_row("SELECT user_id, id FROM bots", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            let floor = start::smart_interval_minimum(&WebBot::find(&c, user, id, For::Settings, "en").unwrap().unwrap());
            let mine = json!({ "value": floor.value.to_f(), "reason": format!("{:?}", floor.reason).to_lowercase() });
            if mine != scenario["rails_floor"] { failures.push(format!("{name}: Smart Intervals floor {mine}, Rails {}", scenario["rails_floor"])); }
        }
        let rails_out: Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_out = deltabadger::parity::decide(&rust_dir).await.unwrap();
        // Rails drops a NULL missed_quote_amount_was_set on a failed run; the engine keeps it (R5, as every grid compares).
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
        // Rails' side must show what each cell exists for, or an equal comparison proves nothing.
        let sent = rails_out["sent"].as_array().unwrap();
        let rows = rails_out["changes"]["transactions"].as_array().unwrap();
        let events = rails_out["changes"]["bot_activity_logs"].to_string();
        let pairs: Vec<&str> = sent.iter().filter_map(|o| o["pair"].as_str().or_else(|| o["symbol"].as_str())).collect();
        let case = name.split_once('-').unwrap().1.split_once('-').unwrap().1;
        let pinned = match case {
            c if c.starts_with("tiers-first_tick") => pairs.len() == 3 && pairs[0].starts_with(if name.contains("kraken") { "XBT" } else { "BTC" }),
            c if c.starts_with("w70-first_tick") => pairs.len() == 2,
            c if c.starts_with("tiers-one_below-") && c != "tiers-one_below-next" => pairs.len() == 2 && events.contains("orders_below_minimum"),
            "tiers-one_below-next" => pairs.len() == 5 && pairs[4] == "SOLEUR",
            "tiers-below_carried" => pairs.len() == 3,
            "w70-all_below" => sent.is_empty() && rows.iter().filter(|r| r["after"]["status"] == 2).count() == 2,
            "tiers-price_missing-middle" => sent.is_empty(),
            "w70-untradable-one" => pairs.len() == 1,
            "tiers-unavailable-one" => pairs.len() == 2,
            // Plan CB T5: a parked member is bought at its cap weight only when every member has a cap.
            "market_cap-first_tick" | "market_cap-parked" => pairs.len() == 3,
            "market_cap-missing_cap" | "market_cap-zero_cap" => pairs.len() == 2,
            _ => true,
        };
        if !pinned { failures.push(format!("{name}: Rails no longer shows what this cell is for: {rails_out}")); }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
}
