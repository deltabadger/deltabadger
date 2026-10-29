mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, TxSpec};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{eligibility, index, model, FixedClock};
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};
use rusqlite::OptionalExtension;

const SYMBOLS: [&str; 5] = ["AAA", "BBB", "CCC", "DDD", "EEE"];
const T0: &str = "2026-09-01T14:00:00.5Z";
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
/// Five Alpaca stocks ranked AAA..EEE in data-api's ND100 row by caps 5, 3, 2, 1 and 0.5 trillion, and an index bot over it
/// (top 3, pure market cap) with a fresh ledger sync: (dir, store, seed, bot, [(asset, ticker)] in SYMBOLS order).
fn universe(num_coins: i64, flattening: f64, hold_all: bool) -> (tempfile::TempDir, store::Opened, seed::Seeded, i64, Vec<(i64, i64)>) {
    let (d, o, s) = common::install_alpaca();
    let members: Vec<(i64, i64)> = SYMBOLS.iter().map(|sym| seed::add_alpaca_stock(&o.primary, &s, sym)).collect();
    seed::insert_index(&o.primary, "nasdaq-100", &["AAA.US", "BBB.US", "CCC.US", "DDD.US", "EEE.US"],
                       &json!({ "AAA.US": 5e12, "BBB.US": 3e12, "CCC.US": 2e12, "DDD.US": 1e12, "EEE.US": 5e11 }));
    let id = seed::index_bot(&o.primary, &s, "nasdaq-100", num_coins, flattening, hold_all);
    seed::fresh_stock_jobs(&o.primary, at(T0));
    (d, o, s, id, members)
}
/// Every symbol quoted at an ask of `asks[i]` (0 = unpriced), an open clock, five accepted orders, a funded account.
fn script(asks: [f64; 5]) -> ScriptedTransport { script_with(asks, json!({})) }
/// `script` with more scripted paths.
fn script_with(asks: [f64; 5], extra: Value) -> ScriptedTransport {
    let mut m = json!({
        "GET /v2/clock": ok(json!({ "timestamp": "2026-09-01T10:00:00.5-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" })),
        "POST /v2/orders": (1..=5).map(|n| json!({ "status": 200, "body": { "id": format!("OTX-{n}"), "status": "accepted" } })).collect::<Vec<_>>(),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([])),
    });
    for (sym, ask) in SYMBOLS.iter().zip(asks) {
        m[format!("GET /v2/stocks/{sym}/quotes/latest")] = ok(json!({ "quote": { "ap": ask, "bp": ask } }));
    }
    for (k, v) in extra.as_object().unwrap() { m[k] = v.clone(); }
    ScriptedTransport::from_script(&m)
}
fn venue(t: &ScriptedTransport) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(t.clone(), Urls::for_passphrase(Some("paper"))) }
fn posted_symbols(t: &ScriptedTransport) -> Vec<String> { t.posted_orders().iter().map(|b| b["symbol"].as_str().unwrap().to_string()).collect() }
fn in_index(o: &store::Opened, bot: i64) -> Vec<(String, f64)> {
    let mut s = o.primary.prepare("SELECT a.symbol, m.target_allocation FROM bot_index_assets m JOIN assets a ON a.id = m.asset_id \
                                   WHERE m.bot_id = ?1 AND m.in_index = 1 ORDER BY m.target_allocation DESC, m.id").unwrap();
    s.query_map([bot], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
async fn tick_until_settled(o: &store::Opened, t: &ScriptedTransport, id: i64, start: DateTime<Utc>) -> TickOutcome {
    let (mut now, mut attempts) = (start, Attempts::default());
    loop {
        match tick::tick(&o.primary, &venue(t), id, &FixedClock(now), &mut attempts).await.unwrap() {
            TickOutcome::RetryAfter(d) => now += Duration::from_std(d).unwrap(),
            other => return other,
        }
    }
}
fn quote_reads(t: &ScriptedTransport, sym: &str) -> usize { t.requests().iter().filter(|r| r.path == format!("/v2/stocks/{sym}/quotes/latest")).count() }

#[test]
fn every_recorded_rails_blend_is_reproduced() {
    let cases = common::vectors()["index_blends"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 12);
    for c in cases {
        let caps: Vec<f64> = c["caps"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let want: Vec<f64> = c["weights"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        for (got, want) in index::blend(&caps, c["flattening"].as_f64().unwrap()).iter().zip(want) { assert!((got-want).abs() <= 2.0*f64::EPSILON, "{c}: {got} != {want}"); }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_fresh_index_bot_probes_and_buys_the_top_three_by_cap_weight() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(in_index(&o, id), vec![("AAA".into(), 0.5), ("BBB".into(), 0.3), ("CCC".into(), 0.2)], "caps 5/3/2 of 10");
    assert_eq!(posted_symbols(&t), vec!["AAA", "BBB", "CCC"], "in target_allocation DESC order");
    let notionals: Vec<Value> = t.posted_orders().iter().map(|b| b["notional"].clone()).collect();
    assert_eq!(notionals, vec![json!("30.00"), json!("18.00"), json!("12.00")], "2c's split over empty holdings (its vectors pin the arithmetic)");
    for sym in ["AAA", "BBB", "CCC"] { assert_eq!(quote_reads(&t, sym), 1, "{sym}: the probe's price is the one Step 1 reuses (5 s cache)"); }
    assert_eq!(quote_reads(&t, "DDD"), 0, "the walk stops at N");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unpriced_newcomer_gives_its_seat_to_the_next_name() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    let t = script([100.0, 200.0, 0.0, 400.0, 500.0]);
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert_eq!(in_index(&o, id).into_iter().map(|(s, _)| s).collect::<Vec<_>>(), vec!["AAA", "BBB", "DDD"]);
    assert_eq!(posted_symbols(&t), vec!["AAA", "BBB", "DDD"]);
}

#[tokio::test(flavor = "current_thread")]
async fn an_unpriced_incumbent_stalls_the_buy_and_nothing_is_placed() {
    let (_d, o, _s, id, m) = universe(3, 0.0, false);
    for (i, w) in [0.5, 0.3, 0.2].into_iter().enumerate() { seed::insert_member(&o.primary, id, m[i].0, m[i].1, w, true, "2026-08-01 00:00:00", None); }
    let t = script([100.0, 200.0, 0.0, 400.0, 500.0]);
    let out = tick_until_settled(&o, &t, id, at(T0)).await;
    assert!(matches!(out, TickOutcome::Rescheduled), "retry_on ×4, then the next checkpoint: {out:?}");
    assert!(t.posted_orders().is_empty(), "Step 1 prices every member before any order");
    let error: String = o.primary.query_row("SELECT json_extract(details, '$.error') FROM bot_activity_logs WHERE event = 'execution_retrying'", [], |r| r.get(0)).unwrap();
    assert_eq!(error, "No price for CCC: Wrong ask price for CCC: 0.0", "an incumbent is never probed: it keeps its seat and stalls the tick");
    assert_eq!(in_index(&o, id).len(), 3, "CCC is still a member");
}

#[tokio::test(flavor = "current_thread")]
async fn a_leaver_is_marked_out_and_never_sold() {
    let (_d, o, s, id, m) = universe(3, 0.0, false);
    for (i, w) in [0.5, 0.3, 0.2].into_iter().enumerate() {
        seed::insert_member(&o.primary, id, m[i].0, m[i].1, w, true, "2026-08-01 00:00:00", None);
        seed::insert_stock_tx(&o.primary, &s, id, m[i].0, SYMBOLS[i], &TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("OC-{i}")),
            order_type: 0, amount: Some("0.1"), quote_amount: Some("10"), price: Some("100"), quote_amount_exec: Some("10"), amount_exec: Some("0.1"),
            created_at: "2026-08-01 14:00:01".into() });
    }
    o.primary.execute("UPDATE indices SET top_coins = '[\"AAA.US\",\"BBB.US\",\"DDD.US\",\"EEE.US\"]'", []).unwrap(); // CCC left the index
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    let (in_idx, exited): (bool, Option<String>) = o.primary.query_row(
        "SELECT in_index, exited_at FROM bot_index_assets WHERE asset_id = ?1", [m[2].0], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((in_idx, exited.as_deref()), (false, Some("2026-09-01 14:00:00.500000")), "marked out at the tick (update_all, updated_at kept)");
    assert!(!posted_symbols(&t).contains(&"CCC".to_string()), "a quitter is not bought");
    assert!(t.posted_orders().iter().all(|b| b["side"] == "buy"), "and never sold");
    assert_eq!(in_index(&o, id).into_iter().map(|(s, _)| s).collect::<Vec<_>>(), vec!["AAA", "BBB", "DDD"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_reentrant_keeps_its_entered_at() {
    let (_d, o, _s, id, m) = universe(3, 0.0, false);
    seed::insert_member(&o.primary, id, m[2].0, m[2].1, 0.2, false, "2026-08-01 00:00:00", Some("2026-08-15 10:30:00"));
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    let row: (bool, String, Option<String>) = o.primary.query_row(
        "SELECT in_index, entered_at, exited_at FROM bot_index_assets WHERE asset_id = ?1", [m[2].0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!(row, (true, "2026-08-01 00:00:00".into(), None), "entered_at ||= now; exited_at cleared");
    assert_eq!(quote_reads(&t, "CCC"), 1, "an exited row is not an incumbent: it is probed like a newcomer");
}

#[tokio::test(flavor = "current_thread")]
async fn a_missing_index_fails_the_tick_as_rails_does() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    o.primary.execute("DELETE FROM indices", []).unwrap();
    o.primary.execute("INSERT INTO indices (external_id, source, created_at, updated_at) VALUES ('other', 'deltabadger', '2026-01-01 00:00:00', '2099-01-01 00:00:00')", []).unwrap();
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Rescheduled), "{out:?}");
    let error: String = o.primary.query_row("SELECT json_extract(details, '$.error') FROM bot_activity_logs WHERE event = 'execution_failed'", [], |r| r.get(0)).unwrap();
    assert_eq!(error, "Index not found");
    assert!(t.posted_orders().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn hold_all_buys_the_whole_universe_with_flattened_weights() {
    let (_d, o, _s, id, _) = universe(3, 0.5, true);
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    let members = in_index(&o, id);
    assert_eq!(members.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), SYMBOLS.to_vec(), "the bounded universe, not num_coins");
    let want = index::blend(&[5e12, 3e12, 2e12, 1e12, 5e11], 0.5);
    for ((sym, got), w) in members.iter().zip(want) { assert!((got - w).abs() < 5e-7, "{sym}: {got} vs {w} (decimal(10,6))"); }
    assert_eq!(t.posted_orders().len(), 5);
}

/// D-IDX-CONDITIONS (reverses #742): an index bot runs its buy-side price rule with `pause`, on the ticker its settings name
/// (D-IDX-SUBJECT: here a member, AAA, last traded at 150). Unmet, the tick pauses it (`waiting`, `limit_paused`, no order),
/// the decorated started_at is nil and nothing is owed; met, it stamps met_at and buys as an unflagged bot. Its sell-side
/// switches have nothing to act on and stay inert.
#[tokio::test(flavor = "current_thread")]
async fn an_index_bot_runs_its_buy_price_rule() {
    let run = |limit: Option<i64>, met_at: Option<&'static str>| async move {
        let (d, o, _s, id, members) = universe(3, 0.0, false);
        if let Some(met_at) = met_at {
            o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.price_limit_condition_met_at', ?1) WHERE id = ?2", (met_at, id)).unwrap();
        }
        if let Some(limit) = limit {
            o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit', ?1, '$.price_limit_value_condition', 'below', \
                               '$.price_limit_in_ticker_id', ?2, '$.sell_price_limited', json('true'), '$.sell_price_limit', 1) WHERE id = ?3", (limit, members[0].1, id)).unwrap();
        }
        let bot = model::load_bot(&o.primary, id).unwrap();
        assert!(eligibility::bot_reasons(&o.primary, &bot).unwrap().is_empty(), "limit {limit:?}");
        let t = script_with([100.0, 200.0, 300.0, 400.0, 500.0], json!({ "GET /v2/stocks/AAA/trades/latest": ok(json!({ "trade": { "p": 150 } })) }));
        let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
        let after = model::load_bot(&o.primary, id).unwrap();
        let paused: i64 = o.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'limit_paused' AND json_extract(details, '$.limit_type') = 'price'", [id], |r| r.get(0)).unwrap();
        (format!("{out:?}"), t.posted_orders(), after, paused, model::condition_paused_type(&o.primary, &model::load_bot(&o.primary, id).unwrap()).unwrap().is_some(), d, o)
    };
    let plain = run(None, None).await;
    assert_eq!(plain.0, "Done { placed: true }");
    let unmet = run(Some(100), None).await; // 150 is not below 100
    assert_eq!((unmet.0.as_str(), unmet.1.len(), unmet.2.status, unmet.3, unmet.4), ("Paused", 0, deltabadger::enums::BotStatus::Waiting, 1, true));
    assert_eq!(unmet.2.condition_started_us().unwrap(), None, "unmet: started_at reads nil");
    let pending = deltabadger::engine::accounting::pending_quote_amount(&unmet.6.primary, &unmet.2, at("2026-09-15T14:00:00Z").timestamp_micros()).unwrap();
    assert!(pending.is_zero(), "nothing owed while unmet: {pending:?}");
    // Met for the first time at this tick: met_at is stamped now, so the window opens now and nothing is owed yet (as on a
    // Rails basket: the interval count from a window opened after the last checkpoint is 0).
    let fresh = run(Some(1000), None).await;
    assert_eq!((fresh.0.as_str(), fresh.1.len(), fresh.3), ("Done { placed: false }", 0, 0));
    assert_eq!(fresh.2.transient["price_limit_condition_met_at"], json!("2026-09-01T14:00:00.500Z"), "met_at stamped");
    // Met since before the start: the window is the bot's own, and it buys as the unflagged bot.
    let met = run(Some(1000), Some("2026-08-31T00:00:00.000Z")).await;
    assert_eq!((met.0.as_str(), &met.1, met.3), (plain.0.as_str(), &plain.1, 0), "met: the orders of the unflagged bot");
}

/// A subject that is not one of the venue's tradable pairs at the bot's quote (here none stored) is unavailable: the gate pauses.
#[tokio::test(flavor = "current_thread")]
async fn an_index_rule_without_a_tradable_subject_pauses() {
    for subject in [None, Some(999_999)] {
        let (_d, o, _s, id, _) = universe(3, 0.0, false);
        o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit', 1000000) WHERE id = ?1", [id]).unwrap();
        if let Some(subject) = subject { o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.price_limit_in_ticker_id', ?1) WHERE id = ?2", (subject, id)).unwrap(); }
        let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
        let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
        assert!(matches!(out, TickOutcome::Paused), "{subject:?}: {out:?}");
        assert!(t.posted_orders().is_empty(), "{subject:?}");
    }
}

/// D-IDX-CAP (reverses #752): an index bot's spending cap is read as a basket's: the cap readers see it, the tick's amount is
/// cut to what is left, and a remainder no leg of which the venue takes stops the bot (D-CAP-REMAINDER-STOP): one `stopped`
/// line with the amount-limit key, the amount-limit mail owed, no order. Values from fixture F (case "vi_cap_1").
#[tokio::test(flavor = "current_thread")]
async fn an_index_bot_applies_its_spending_cap() {
    use deltabadger::engine::amount;
    let f = &common::conditions_fixture()["index_cap_one"];
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.quote_amount_limited', json('true'), '$.quote_amount_limit', ?1), \
                       transient_data = json_set(transient_data, '$.quote_amount_limit_enabled_at', '2026-08-01T00:00:00.000Z') WHERE id = ?2", (f["cap"].as_i64().unwrap(), id)).unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(bot.quote_amount_limited());
    assert_eq!(amount::exact_cap_available(&o.primary, &bot).unwrap(), Some(deltabadger::ruby::BigDec::parse(f["remainder"].as_str().unwrap()).unwrap()));
    assert!(!amount::quote_amount_limit_reached(&o.primary, &bot).unwrap(), "1 USD is over the 0.01 precision floor");
    assert!(eligibility::bot_reasons(&o.primary, &bot).unwrap().is_empty());
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Skipped), "{out:?}");
    assert!(t.posted_orders().is_empty());
    let after = model::load_bot(&o.primary, id).unwrap();
    let key: Option<String> = o.primary.query_row("SELECT stop_message_key FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!((after.status, key.as_deref()), (deltabadger::enums::BotStatus::Stopped, Some(tick::AMOUNT_SPENT)));
    let stopped: i64 = o.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'stopped'", [id], |r| r.get(0)).unwrap();
    assert_eq!(stopped, 1);
    assert!(after.transient.get(deltabadger::engine::notice::LIMIT).is_some(), "the amount-limit mail is owed");
    let skipped: Vec<String> = o.primary.prepare("SELECT CAST(quote_amount AS TEXT) FROM transactions WHERE bot_id = ?1 AND status = 2 ORDER BY id").unwrap()
        .query_map([id], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(skipped, f["skipped"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect::<Vec<_>>());
}

#[test]
fn an_index_bot_is_admitted_on_a_top_or_category_index_of_any_source() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    let reasons = |o: &store::Opened| eligibility::bot_reasons(&o.primary, &model::load_bot(&o.primary, id).unwrap()).unwrap();
    assert_eq!(reasons(&o), Vec::<String>::new());
    // Any source (MarketData.get_top_coins' find_by has no source filter), and a missing row is the tick's Failure.
    o.primary.execute("UPDATE indices SET source = 'coingecko'", []).unwrap();
    assert_eq!(reasons(&o), Vec::<String>::new());
    o.primary.execute("DELETE FROM indices", []).unwrap();
    assert_eq!(reasons(&o), Vec::<String>::new());
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.index_type', 'top')", []).unwrap();
    assert_eq!(reasons(&o), Vec::<String>::new());
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.index_type', 'custom')", []).unwrap();
    assert!(reasons(&o).iter().any(|r| r.starts_with("index_type")), "{:?}", reasons(&o));
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.index_type', 'category', '$.num_coins', 'ten', '$.allocation_flattening', 1.5)", []).unwrap();
    let r = reasons(&o);
    assert!(r.iter().any(|r| r.starts_with("num_coins")) && r.iter().any(|r| r.starts_with("allocation_flattening")), "{r:?}");
}


#[test]
fn stock_and_index_intents_recheck_status_members_settings_and_every_ticker() {
    use deltabadger::engine::{amount, basket, placement, venue_rules::ALPACA};
    use deltabadger::ruby::BigDec;
    for index in [false,true] {
        for mutation in ["status", "restart", "rerun", "settings", "member", "ticker", "split", "instrument", "unavailable", "provider"] {
            let (_d,o,s)=common::install_alpaca();
            let (asset,ticker)=seed::add_alpaca_stock(&o.primary,&s,"AAPL");
            seed::insert_index(&o.primary,"nasdaq-100",&["AAPL.US"],&json!({"AAPL.US":3e12}));
            let id=if index {seed::index_bot(&o.primary,&s,"nasdaq-100",1,0.0,false)} else {
                seed::insert_bot(&o.primary,&s,&seed::BotSpec::weekly(60.0,"2026-09-01 14:00:00").weights(&[(asset,1.0)]))
            };
            basket::write_members(&o.primary,id,&[(asset,ticker,1.0)],at(T0)).unwrap();
            seed::fresh_stock_jobs(&o.primary,at(T0));
            // Claimed by a tick, as the engine sizes it.
            o.primary.execute("UPDATE bots SET status=4, transient_data=json_set(transient_data,'$.last_action_job_at','2026-09-10T12:00:00.000Z') WHERE id=?1",[id]).unwrap();
            let bot=model::load_bot(&o.primary,id).unwrap();
            let ticker=model::ticker_by_id(&o.primary,s.exchange_id,ticker).unwrap().unwrap();
            let composition=placement::composition_snapshot(&o.primary,&bot).unwrap();
            let amount::Sizing::Place(plan)=amount::size(&bot,&ticker,&BigDec::from_i64(60),&BigDec::from_i64(100),&ALPACA).unwrap() else {panic!("sized")};
            let reconciled=deltabadger::engine::splits::snapshot(&o.primary,&bot).unwrap();
            let writer=rusqlite::Connection::open(o.primary.path().unwrap()).unwrap();
            match mutation {
                "status"=>{writer.execute("UPDATE bots SET status=2 WHERE id=?1",[id]).unwrap();},
                // Stopped and started again: working, but not the run that sized the order.
                "restart"=>{writer.execute("UPDATE bots SET status=1 WHERE id=?1",[id]).unwrap();},
                "rerun"=>{writer.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.last_action_job_at','2026-09-10T12:00:01.000Z') WHERE id=?1",[id]).unwrap();},
                "settings"=>{writer.execute("UPDATE bots SET settings=json_set(settings,'$.num_coins',2) WHERE id=?1",[id]).unwrap();},
                "member"=>{writer.execute("UPDATE bot_index_assets SET target_allocation=0.5 WHERE bot_id=?1",[id]).unwrap();},
                "instrument"=>{writer.execute("UPDATE assets SET instrument_type='option' WHERE id=?1",[asset]).unwrap();},
                "unavailable"=>{writer.execute("UPDATE tickers SET available=0 WHERE id=?1",[ticker.id]).unwrap();},
                "provider"=>{ if !index {continue;} writer.execute("DELETE FROM app_configs WHERE key='market_data_provider'",[]).unwrap();},
                "ticker"=>{writer.execute("UPDATE tickers SET base_decimals=8 WHERE id=?1",[ticker.id]).unwrap();},
                _=>{writer.execute("UPDATE bots SET restatement_generation=restatement_generation+1 WHERE id=?1",[id]).unwrap();},
            }
            let begun=placement::begin_unless_changed(&o.primary,&bot,&plan,&[&ticker],&composition,&reconciled,&FixedClock(at(T0))).unwrap();
            // Losing the run fences the tick; any other change only skips the order.
            if matches!(mutation,"status"|"restart"|"rerun") { assert!(matches!(begun,placement::Begun::Fenced),"index={index}, {mutation}"); }
            else { assert!(matches!(begun,placement::Begun::Changed),"index={index}, {mutation}"); }
            assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
        }
    }
}


#[tokio::test(flavor="current_thread")]
async fn unsupported_newcomers_never_publish_or_trade() {
    for instrument in ["option", "tokenized_equity"] {
        let (_d,o,_s,id,m)=universe(3,0.0,false);
        o.primary.execute("UPDATE assets SET instrument_type=?1 WHERE id=?2",rusqlite::params![instrument,m[0].0]).unwrap();
        assert!(eligibility::check_install(&o.primary).unwrap().refusal().is_ok());
        let t=script([100.0;5]);
        tick::tick(&o.primary,&venue(&t),id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
        assert!(t.posted_orders().is_empty());
        assert!(in_index(&o,id).is_empty());
        assert!(eligibility::check_install(&o.primary).unwrap().refusal().is_ok());
    }
}

#[test]
fn index_constituent_reclassification_is_guarded_even_without_allocations() {
    use deltabadger::jobs::import::{in_transaction,Touched};
    for active in [true,false] {
        let (_d,o,_s,id,m)=universe(3,0.0,false);
        seed::insert_member(&o.primary,id,m[0].0,m[0].1,1.0,active,"2026-08-01 00:00:00",None);
        let result=in_transaction(&o.primary,&seed::cipher(),"index reclassification",|tx| {
            tx.execute("UPDATE assets SET instrument_type='tokenized_equity' WHERE id=?1",[m[0].0]).map_err(|e|e.to_string())?;
            Ok(((),Touched::Assets(vec![m[0].0])))
        });
        assert!(result.is_err(),"guard must reject in/out constituent reclassification");
        let instrument:String=o.primary.query_row("SELECT instrument_type FROM assets WHERE id=?1",[m[0].0],|r|r.get(0)).unwrap();
        assert_eq!(instrument,"stock");
        assert!(eligibility::check_install(&o.primary).unwrap().refusal().is_ok());
    }
}

#[tokio::test(flavor="current_thread")]
async fn switching_or_removing_the_provider_invalidates_index_admission_and_freshness() {
    use deltabadger::{app_config,engine::staleness};
    for change in ["coingecko","removed","token_removed","url_changed"] {
        let (_d,o,_s,id,_)=universe(3,0.0,false);
        match change {
            "coingecko"=>app_config::set(&o.primary,&seed::cipher(),"market_data_provider","coingecko",at(T0)).unwrap(),
            "removed"=>{o.primary.execute("DELETE FROM app_configs WHERE key='market_data_provider'",[]).unwrap();},
            "token_removed"=>{o.primary.execute("DELETE FROM app_configs WHERE key='market_data_token'",[]).unwrap();},
            _=>app_config::set(&o.primary,&seed::cipher(),"market_data_url","https://changed.invalid",at(T0)).unwrap(),
        }
        let bot=model::load_bot(&o.primary,id).unwrap();
        if change!="url_changed" { assert!(!eligibility::bot_reasons(&o.primary,&bot).unwrap().is_empty(),"{change}"); }
        assert!(staleness::stale(&o.primary,&bot,at(T0)).unwrap().is_some(),"{change}");
        let t=script([100.0;5]);
        tick::tick(&o.primary,&venue(&t),id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
        assert!(t.posted_orders().is_empty(),"{change}");
    }
}

#[tokio::test(flavor="current_thread")]
async fn duplicate_ranked_assets_use_the_recorded_rails_asset_map_blend() {
    let (_d,o,_s,id,_)=universe(3,0.5,false);
    o.primary.execute("UPDATE indices SET top_coins='[\"AAA.US\",\"AAA.US\",\"BBB.US\"]',weights='{\"AAA.US\":100,\"BBB.US\":10}'",[]).unwrap();
    let t=script([100.0;5]);
    tick::tick(&o.primary,&venue(&t),id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
    let recorded=&common::vectors()["index_duplicate_blend"];
    assert_eq!(in_index(&o,id),vec![("AAA".into(),recorded[0]["weight"].as_f64().unwrap()),("BBB".into(),recorded[2]["weight"].as_f64().unwrap())]);
}

#[tokio::test(flavor="current_thread")]
async fn a_split_for_another_held_asset_during_positions_ends_the_tick() {
    use deltabadger::venue::http::{Transport,HttpRequest,HttpResponse,TransportError};
    struct DuringPositions { script:ScriptedTransport, db:rusqlite::Connection, bot:i64, row:i64 }
    impl Transport for DuringPositions {
        async fn send(&self, req:&HttpRequest)->Result<HttpResponse,TransportError> {
            if req.path=="/v2/positions" {
                tokio::task::yield_now().await;
                // B's incorrect old split arrives while reconciliation is checking A's old split.
                self.db.execute("UPDATE account_transactions SET raw_data=json_set(raw_data,'$.corporate_action','split') WHERE id=?1",[self.row]).unwrap();
                self.db.execute("UPDATE bots SET restatement_generation=restatement_generation+1 WHERE id=?1",[self.bot]).unwrap();
            }
            self.script.send(req).await
        }
    }
    let (_d,o,s,id,m)=universe(3,0.0,false);
    for (i,sym) in ["AAA","BBB"].iter().enumerate() {
        seed::insert_stock_tx(&o.primary,&s,id,m[i].0,sym,&TxSpec{status:0,external_status:Some(2),external_id:Some(format!("held-{sym}")),order_type:0,amount:Some("10"),quote_amount:Some("1000"),price:Some("100"),amount_exec:Some("10"),quote_amount_exec:Some("1000"),created_at:"2026-08-01 14:00:00".into()});
    }
    let mut b=0;
    for sym in ["AAA","BBB"] {
        let row=seed::insert_split(&o.primary,&s,sym,"2026-08-05 00:00:00",Some("3:2"));
        o.primary.execute("UPDATE account_transactions SET base_amount=5,raw_data=?1 WHERE id=?2",rusqlite::params![json!({"corporate_action":if sym=="AAA" {"split"} else {"pending"},"qty":"-10","split_ratio":"3:2","merged_activity_ids":["a","b"]}).to_string(),row]).unwrap();
        b=row;
    }
    let scripted=script([100.0;5]);
    // Override positions only, keeping all other requests visible in the underlying script.
    struct Positions { inner:DuringPositions }
    impl Transport for Positions {
        async fn send(&self,req:&HttpRequest)->Result<HttpResponse,TransportError> {
            let reply=self.inner.send(req).await?;
            if req.path=="/v2/positions" {return Ok(HttpResponse{status:200,body:json!([{"symbol":"AAA","asset_class":"us_equity","qty":"15"},{"symbol":"BBB","asset_class":"us_equity","qty":"30"}]).to_string()});}
            Ok(reply)
        }
    }
    let transport=Positions{inner:DuringPositions{script:scripted.clone(),db:rusqlite::Connection::open(o.primary.path().unwrap()).unwrap(),bot:id,row:b}};
    let v=AlpacaVenue::new(transport,Urls::for_passphrase(Some("paper")));
    let out=tick::tick(&o.primary,&v,id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
    assert!(matches!(out,TickOutcome::Done{placed:false}),"{out:?}");
    assert!(scripted.posted_orders().is_empty());
    assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
    let reason:String=o.primary.query_row("SELECT json_extract(details,'$.reason') FROM bot_activity_logs WHERE event='dca_skipped_restatement' ORDER BY id DESC LIMIT 1",[],|r|r.get(0)).unwrap();
    assert!(reason.contains("changed during reconciliation"),"{reason}");
    let out=tick::tick(&o.primary,&v,id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
    assert!(matches!(out,TickOutcome::Done{placed:false}),"next pass re-reconciles: {out:?}");
    assert!(model::load_bot(&o.primary,id).unwrap().transient["rust_split_hold"]["reason"].as_str().unwrap().contains("BBB"));
}

#[tokio::test(flavor="current_thread")]
async fn a_long_split_history_stands_down_with_a_visible_reason() {
    let (_d,o,s,id,m)=universe(1,0.0,false);
    seed::insert_stock_tx(&o.primary,&s,id,m[0].0,"AAA",&TxSpec{status:0,external_status:Some(2),external_id:Some("old-fill".into()),order_type:0,amount:Some("1"),quote_amount:Some("100"),price:Some("100"),amount_exec:Some("1"),quote_amount_exec:Some("100"),created_at:"2000-01-01 00:00:00".into()});
    for i in 0..1500 {
        let day=(at("2000-01-02T00:00:00Z")+Duration::days(i)).format("%Y-%m-%d %H:%M:%S").to_string();
        seed::insert_split(&o.primary,&s,"AAA",&day,Some("3:2"));
    }
    let t=script([100.0;5]);
    let out=tick::tick(&o.primary,&venue(&t),id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
    assert!(matches!(out,TickOutcome::Rescheduled),"{out:?}");
    assert!(t.posted_orders().is_empty());
    assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
    let error:String=o.primary.query_row("SELECT json_extract(details,'$.error') FROM bot_activity_logs WHERE event='execution_failed'",[],|r|r.get(0)).unwrap();
    assert!(error.contains("split walk refused"),"{error}");
}

#[test]
fn encrypted_provider_configuration_and_environment_precedence_match_rails() {
    use deltabadger::{app_config,engine::{provider,staleness}};
    let (_d,o,_s,id,_)=universe(1,0.0,false);
    for (key,value) in [("market_data_provider","coingecko"),("market_data_url","https://encrypted.invalid"),("market_data_token","encrypted-script-token")] {
        app_config::set(&o.primary,&seed::cipher(),key,value,at(T0)).unwrap();
    }
    provider::bind(&o.primary,&seed::cipher(),&|k|(k=="MARKET_DATA_URL").then(||"https://environment.invalid".into())).unwrap();
    let config=provider::config(&o.primary).unwrap().unwrap();
    assert_eq!(config.url,"https://encrypted.invalid","database credential rows win; env URL selects provider");
    assert_eq!(config.token,"encrypted-script-token");
    let bot=model::load_bot(&o.primary,id).unwrap();
    assert!(eligibility::bot_reasons(&o.primary,&bot).unwrap().is_empty());
    assert!(staleness::stale(&o.primary,&bot,at(T0)).unwrap().is_some());
    assert!(matches!(staleness::verdict(&o.primary,&bot,at(T0)).unwrap(),staleness::Verdict::Unknown(_)),"scheduler can start to refresh");
    seed::fresh_stock_jobs(&o.primary,at(T0));
    assert!(staleness::stale(&o.primary,&bot,at(T0)).unwrap().is_none());
    app_config::set(&o.primary,&seed::cipher(),"market_data_token","",at(T0)).unwrap();
    assert!(provider::config(&o.primary).unwrap().is_none(),"a blank db token does not fall back to environment");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stock_only_index_ignores_stale_unrelated_crypto_feeds() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    let old = at(T0) - Duration::days(7);
    for job in [deltabadger::jobs::reference::ALPACA_CRYPTO, deltabadger::jobs::reference::ASSETS] {
        deltabadger::app_config::set_plain(&o.primary,
            &deltabadger::jobs::state::key(job, None),
            &json!({"last_success_at":old.to_rfc3339(),"rails_at":old.to_rfc3339()}).to_string(), old).unwrap();
    }
    let t = script([100.0; 5]);
    let out = tick_until_settled(&o, &t, id, at(T0)).await;
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(posted_symbols(&t), ["AAA", "BBB", "CCC"]);
}

/// A crypto venue's index bot sizes from the index rows and the crypto market caps, so it waits on them as an Alpaca one
/// does; a basket bot on that venue does not, unless it is weighted by market cap (plan CB T5): then it waits on the caps only.
#[test]
fn a_crypto_venue_index_bot_waits_on_stale_indices_and_market_caps() {
    for venue in ["Kraken", "Binance", "Kucoin"] { crypto_venue_index_staleness(venue); }
}

fn crypto_venue_index_staleness(venue: &str) {
    use deltabadger::engine::staleness;
    let (_d, o, s) = common::install();
    let c = &o.primary;
    c.execute("UPDATE exchanges SET type = 'Exchanges::' || ?1, name = ?1", [venue]).unwrap();
    seed::insert_index(c, "layer-1", &["bitcoin"], &json!({}));
    let index = seed::index_bot(c, &s, "layer-1", 2, 0.0, false);
    let basket = seed::insert_bot(c, &s, &seed::BotSpec::weekly(60.0, "2026-09-01 14:00:00"));
    let market_cap = seed::insert_bot(c, &s, &seed::BotSpec::weekly(60.0, "2026-09-01 14:00:00").with("weighting", json!("market_cap")));
    let now = at(T0);
    for (job, label) in [(deltabadger::jobs::reference::INDICES, "Indices"), (deltabadger::jobs::reference::ASSETS, "Crypto assets")] {
        seed::fresh_stock_jobs(c, now);
        deltabadger::jobs::state::record_success(c, job, None, now - Duration::days(7)).unwrap();
        let stale = staleness::stale(c, &model::load_bot(c, index).unwrap(), now).unwrap().expect(label);
        assert_eq!(stale.source, label, "{venue}");
        let weighted = staleness::stale(c, &model::load_bot(c, market_cap).unwrap(), now).unwrap();
        assert_eq!(weighted.map(|s| s.source), (label == "Crypto assets").then_some(label), "{venue} {label}: a market-cap basket");
        // The door also trusts Rails' own refresh stamp: it refuses once that is as old.
        let old = (now - Duration::days(7)).format("%Y-%m-%d %H:%M:%S").to_string();
        c.execute(if label == "Indices" { "UPDATE indices SET updated_at = ?1" } else { "UPDATE assets SET updated_at = ?1 WHERE category = 'Cryptocurrency'" }, [&old]).unwrap();
        assert!(matches!(staleness::verdict(c, &model::load_bot(c, index).unwrap(), now).unwrap(), staleness::Verdict::Stale(_)), "{venue} {label}: the door refuses too");
        c.execute("UPDATE indices SET updated_at = ?1", [&at(T0).format("%Y-%m-%d %H:%M:%S").to_string()]).unwrap();
        c.execute("UPDATE assets SET updated_at = ?1 WHERE category = 'Cryptocurrency'", [&at(T0).format("%Y-%m-%d %H:%M:%S").to_string()]).unwrap();
        assert!(staleness::stale(c, &model::load_bot(c, basket).unwrap(), now).unwrap().is_none(), "{label}: a basket bot does not size from it");
    }
    seed::fresh_stock_jobs(c, now);
    assert!(staleness::stale(c, &model::load_bot(c, index).unwrap(), now).unwrap().is_none());
}

/// Slice 6b: an index bot's swap sell whose reply was lost, then the index itself changes (data-api's sync) before the engine
/// restarts. The restarted evaluation settles the lost order by its client order id first, and only then refreshes the
/// membership (before_rebalance): the swap resumes on the order it sent, never a second sell under the new composition.
#[tokio::test(flavor = "current_thread")]
async fn an_index_change_between_a_lost_swap_reply_and_a_restart_resumes_the_sent_sell() {
    use deltabadger::engine::rebalance::{self, Outcome};
    use deltabadger::engine::tick::{PriceCache, TickContext};
    let (_d, o, s, id, m) = universe(3, 0.0, false);
    o.primary.execute("UPDATE bots SET status = 2, settings = json_set(settings, '$.rebalance_enabled', json('true')) WHERE id = ?1", [id]).unwrap();
    // Listed on the venue as the data-api sync lists them, so a position names its asset (the sell's free-balance cap).
    o.primary.execute("INSERT INTO exchange_assets (exchange_id, asset_id, available, created_at, updated_at) \
                       SELECT exchange_id, base_asset_id, 1, created_at, updated_at FROM tickers WHERE quote = 'USD' AND ticker NOT LIKE '%/%'", []).unwrap();
    for (i, (sym, qty, quote)) in [("AAA", "10", "1000"), ("BBB", "3", "300")].into_iter().enumerate() {
        seed::insert_stock_tx(&o.primary, &s, id, m[i].0, sym, &TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("held-{sym}")), order_type: 0,
            amount: Some(qty), quote_amount: Some(quote), price: Some("100"), amount_exec: Some(qty), quote_amount_exec: Some(quote), created_at: "2026-08-01 14:00:00".into() });
    }
    let snapshots: serde_json::Map<String, Value> = SYMBOLS.iter().map(|s| (s.to_string(), json!({ "latestTrade": { "p": 100 } }))).collect();
    let mut script = json!({
        "GET /v2/clock": [{ "status": 200, "body": { "timestamp": "2026-09-01T10:00:00.5-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" } }],
        // AAA holds 1000 of 1300 against a 50% target: 350 over, 3.5 shares at the bid; the reply is lost.
        "POST /v2/orders": [{ "network": "post_send", "message": "Faraday::TimeoutError: Net::ReadTimeout" }],
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([{ "symbol": "AAA", "asset_class": "us_equity", "qty": "10", "qty_available": "10" },
                                      { "symbol": "BBB", "asset_class": "us_equity", "qty": "3", "qty_available": "3" }])),
        "GET /v2/stocks/snapshots": ok(Value::Object(snapshots)),
        "GET /v1beta3/crypto/us/latest/trades": ok(json!({ "trades": {} })),
    });
    for sym in SYMBOLS { script[format!("GET /v2/stocks/{sym}/quotes/latest")] = ok(json!({ "quote": { "ap": 100, "bp": 100 } })); }
    let t = ScriptedTransport::from_script(&script);
    let evaluate = |when: &'static str, started: DateTime<Utc>| {
        let (o, t) = (&o, &t);
        async move {
            let prices = PriceCache::default();
            let bot = model::load_bot(&o.primary, id).unwrap();
            let cx = TickContext { credential_version: model::credential_version(&o.primary, &bot).unwrap(), prices: &prices,
                                   process_start: started, stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
            rebalance::run(&o.primary, &venue(t), id, &FixedClock(at(when)), &cx).await.unwrap()
        }
    };
    assert_eq!(evaluate("2026-09-01T16:30:00Z", DateTime::<Utc>::MIN_UTC).await, Outcome::Skipped("awaiting_reconciliation"));
    assert_eq!(in_index(&o, id), vec![("AAA".into(), 0.5), ("BBB".into(), 0.3), ("CCC".into(), 0.2)]);
    let cl = model::load_bot(&o.primary, id).unwrap().rust_placement().unwrap()["cl_ord_id"].as_str().unwrap().to_string();
    assert_eq!(model::load_bot(&o.primary, id).unwrap().transient["rebalance_pending"]["phase"], "selling");
    // The index changes: DDD overtakes CCC.
    o.primary.execute("UPDATE indices SET top_coins = '[\"AAA.US\",\"BBB.US\",\"DDD.US\",\"CCC.US\",\"EEE.US\"]', \
                       weights = json_set(weights, '$.\"DDD.US\"', 2.5e12)", []).unwrap();
    // The venue had the order all along, resting.
    let found = json!({ "id": "OTX-9", "client_order_id": cl, "status": "new", "symbol": "AAA", "asset_class": "us_equity", "type": "market", "side": "sell",
                        "qty": "3.5", "notional": null, "filled_qty": "0", "filled_avg_price": null, "limit_price": null });
    t.reply("GET /v2/orders:by_client_order_id", 200, found.clone());
    t.reply("GET /v2/orders/OTX-9", 200, found);
    // A restart four hours on: the recovery comes first, then the refresh, then the swap reads its own sell.
    assert_eq!(evaluate("2026-09-01T20:30:00Z", at("2026-09-01T20:00:00Z")).await, Outcome::Skipped("waiting_for_sell"));
    assert_eq!(t.posted_orders().len(), 1, "never a second sell");
    let (sell, kind): (i64, String) = o.primary.query_row("SELECT id, transaction_type FROM transactions WHERE external_id = 'OTX-9'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(kind, "REBALANCE");
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(bot.rust_placement().is_none());
    assert_eq!(bot.transient["rebalance_pending"], json!({ "phase": "selling", "sell_transaction_id": sell, "buy_transaction_id": null, "remaining_quote_amount": null, "buy_attempted": false }));
    assert_eq!(in_index(&o, id).iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), ["AAA", "BBB", "DDD"], "the refresh ran after the recovery");
}

/// A stopped index bot rebalancing AAA (10 shares bought at 100) and BBB (3), with AAA's split recorded at `ratio` (a trusted
/// row: 10 shares out, `delta` net in), and the script the swap reads: an open clock, AAA's position at `aaa`.
fn split_swap(ratio: &str, delta: &str, aaa: &str) -> (tempfile::TempDir, store::Opened, i64, i64, ScriptedTransport) {
    let (d, o, s, id, m) = universe(3, 0.0, false);
    o.primary.execute("UPDATE bots SET status = 2, settings = json_set(settings, '$.rebalance_enabled', json('true')) WHERE id = ?1", [id]).unwrap();
    o.primary.execute("INSERT INTO exchange_assets (exchange_id, asset_id, available, created_at, updated_at) \
                       SELECT exchange_id, base_asset_id, 1, created_at, updated_at FROM tickers WHERE quote = 'USD' AND ticker NOT LIKE '%/%'", []).unwrap();
    for (i, (sym, qty, quote)) in [("AAA", "10", "1000"), ("BBB", "3", "300")].into_iter().enumerate() {
        seed::insert_stock_tx(&o.primary, &s, id, m[i].0, sym, &TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("held-{sym}")), order_type: 0,
            amount: Some(qty), quote_amount: Some(quote), price: Some("100"), amount_exec: Some(qty), quote_amount_exec: Some(quote), created_at: "2026-08-01 14:00:00".into() });
    }
    let row = seed::insert_split(&o.primary, &s, "AAA", "2026-08-05 00:00:00", Some(ratio));
    o.primary.execute("UPDATE account_transactions SET base_amount = ?1, raw_data = ?2 WHERE id = ?3",
                      rusqlite::params![delta, json!({ "corporate_action": "split", "qty": "-10", "split_ratio": ratio, "merged_activity_ids": ["a", "b"] }).to_string(), row]).unwrap();
    let snapshots: serde_json::Map<String, Value> = SYMBOLS.iter().map(|s| (s.to_string(), json!({ "latestTrade": { "p": 100 } }))).collect();
    let mut script = json!({
        "GET /v2/clock": ok(json!({ "timestamp": "2026-09-01T10:00:00.5-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" })),
        "POST /v2/orders": ok(json!({ "id": "OTX-1", "status": "accepted" })),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([{ "symbol": "AAA", "asset_class": "us_equity", "qty": aaa, "qty_available": aaa },
                                      { "symbol": "BBB", "asset_class": "us_equity", "qty": "3", "qty_available": "3" }])),
        "GET /v2/stocks/snapshots": ok(Value::Object(snapshots)),
        "GET /v1beta3/crypto/us/latest/trades": ok(json!({ "trades": {} })),
    });
    for sym in SYMBOLS { script[format!("GET /v2/stocks/{sym}/quotes/latest")] = ok(json!({ "quote": { "ap": 100, "bp": 100 } })); }
    (d, o, id, row, ScriptedTransport::from_script(&script))
}
async fn evaluate_swap<T: deltabadger::venue::http::Transport>(o: &store::Opened, v: &AlpacaVenue<T>, id: i64) -> deltabadger::engine::rebalance::Outcome {
    use deltabadger::engine::tick::{PriceCache, TickContext};
    let prices = PriceCache::default();
    let bot = model::load_bot(&o.primary, id).unwrap();
    let cx = TickContext { credential_version: model::credential_version(&o.primary, &bot).unwrap(), prices: &prices,
                           process_start: DateTime::<Utc>::MIN_UTC, stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
    deltabadger::engine::rebalance::run(&o.primary, v, id, &FixedClock(at("2026-09-01T16:30:00Z")), &cx).await.unwrap()
}

/// A rebalance sizes neither leg on a split the venue disagrees with: a recorded 3:2 on 10 shares restates AAA to 15 while
/// the venue holds 30, so the reconciliation tick::execute applies holds the swap (unreconciled, it sold 6 of the 30).
#[tokio::test(flavor = "current_thread")]
async fn a_swap_holds_on_a_split_the_venue_disagrees_with() {
    use deltabadger::engine::rebalance::Outcome;
    let (_d, o, id, _, t) = split_swap("3:2", "5", "30");
    assert_eq!(evaluate_swap(&o, &venue(&t), id).await, Outcome::Skipped("split_hold"));
    assert!(t.posted_orders().is_empty());
    assert!(model::load_bot(&o.primary, id).unwrap().transient.get("rebalance_pending").is_none_or(Value::is_null));
    // Reconciled (the venue's 30 is the 3:1 the row should have said), the same swap sells.
    let (_d, o, id, _, t) = split_swap("3:1", "20", "30");
    assert!(matches!(evaluate_swap(&o, &venue(&t), id).await, Outcome::Placed(_)));
    assert_eq!(posted_symbols(&t), ["AAA"]);
}

/// A split correction landing while a leg is sized (3:1 corrected to 3:2 during a positions read) is caught: the snapshot is
/// taken before the reconciliation and the valuation, and the intent's fence compares the row against it. Captured after the
/// reads, the stale 30-share plan was accepted.
#[tokio::test(flavor = "current_thread")]
async fn a_split_correction_during_the_swaps_reads_is_never_sized_through() {
    use deltabadger::engine::rebalance::Outcome;
    use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
    struct Correcting { script: ScriptedTransport, db: rusqlite::Connection, row: i64, bot: i64, at_read: usize, reads: std::cell::Cell<usize> }
    impl Transport for Correcting {
        async fn send(&self, req: &HttpRequest) -> Result<HttpResponse, TransportError> {
            if req.path == "/v2/positions" {
                self.reads.set(self.reads.get() + 1);
                if self.reads.get() == self.at_read {
                    self.db.execute("UPDATE account_transactions SET base_amount = 5, raw_data = json_set(raw_data, '$.split_ratio', '3:2') WHERE id = ?1", [self.row]).unwrap();
                    self.db.execute("UPDATE bots SET restatement_generation = restatement_generation + 1 WHERE id = ?1", [self.bot]).unwrap();
                }
            }
            self.script.send(req).await
        }
    }
    for at_read in [1, 2] {
        let (_d, o, id, row, t) = split_swap("3:1", "20", "30");
        let v = AlpacaVenue::new(Correcting { script: t.clone(), db: rusqlite::Connection::open(o.primary.path().unwrap()).unwrap(), row, bot: id, at_read, reads: 0.into() },
                                 Urls::for_passphrase(Some("paper")));
        let out = evaluate_swap(&o, &v, id).await;
        assert!(matches!(out, Outcome::Skipped("split_hold" | "changed")), "read {at_read}: {out:?}");
        assert!(t.posted_orders().is_empty(), "read {at_read}");
    }
}

/// T1: MarketData.get_top_coins' row: the category's own with a present index_category_id, else `top-coins` (any source).
#[test]
fn a_top_bot_and_a_category_bot_without_an_id_read_the_top_coins_row() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    let bot = || model::load_bot(&o.primary, id).unwrap();
    assert_eq!(bot().index_external_id(), "nasdaq-100");
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.index_category_id', '  ')", []).unwrap();
    assert_eq!(bot().index_external_id(), "top-coins", "a blank id is not present?");
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.index_category_id', 'nasdaq-100', '$.index_type', 'top')", []).unwrap();
    assert_eq!(bot().index_external_id(), "top-coins");
    assert_eq!(index::candidate_pairs(&o.primary, &bot()).unwrap(), Vec::<String>::new(), "no top-coins row yet");
    seed::insert_index(&o.primary, "top-coins", &["CCC.US", "AAA.US"], &json!({}));
    o.primary.execute("UPDATE indices SET source = 'internal' WHERE external_id = 'top-coins'", []).unwrap();
    assert_eq!(index::candidate_pairs(&o.primary, &bot()).unwrap(), vec!["AAA", "CCC"]);
}

/// D-INDEX-TOP-N: selection fills down the complete ranking after venue and taxonomy exclusions.
#[tokio::test(flavor = "current_thread")]
async fn top_twenty_fills_from_tradable_non_stable_assets_and_reports_shortage() {
    use deltabadger::venue::fake::FakeVenue;
    for available in [25, 16] {
        let (_d, o, s) = common::install();
        let c = &o.primary;
        let mut venue = FakeVenue::new().balance_body("ZEUR", "100000", "0");
        let mut ids = vec![];
        let mut members = vec![];
        for i in 0..30 {
            let ext = if i == 0 { "usd-coin".into() } else { format!("coin-{i}") };
            c.execute("INSERT INTO assets (external_id,symbol,name,category,market_cap,created_at,updated_at) VALUES (?1,?1,?1,?2,?3,'2026-01-01','2099-01-01')",
                rusqlite::params![ext, if i == 0 { "Stablecoin" } else { "Cryptocurrency" }, 1000-i]).unwrap();
            let asset = c.last_insert_rowid();
            c.execute("INSERT INTO tickers (exchange_id,base_asset_id,quote_asset_id,ticker,base,quote,available,trading_enabled,base_decimals,quote_decimals,price_decimals,minimum_base_size,minimum_quote_size,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,'EUR',?6,1,8,2,2,0.001,0.01,'2026-01-01','2099-01-01')",
                rusqlite::params![s.exchange_id,asset,s.quote,format!("C{i}EUR"),format!("C{i}"),if available == 25 { i % 6 != 5 } else { i < available }]).unwrap();
            members.push((asset,c.last_insert_rowid()));
            venue = venue.ticker(&format!("C{i}EUR"), "10", "10", "10");
            ids.push(ext);
        }
        seed::insert_index(c, "top-coins", &ids.iter().map(String::as_str).collect::<Vec<_>>(), &json!({}));
        c.execute("UPDATE indices SET source='internal'", []).unwrap();
        let id = seed::index_bot(c, &s, "top-coins", 20, 1.0, false);
        seed::fresh_stock_jobs(c,at(T0));
        // The ten old members remain held until the next ordinary composition step.
        for &(asset,ticker) in &members[..10] { seed::insert_member(c,id,asset,ticker,0.1,true,"2026-08-01",None); }
        assert!(eligibility::bot_reasons(c,&model::load_bot(c,id).unwrap()).unwrap().is_empty(),"a formerly included stablecoin is a holding, not a reason to refuse the index bot");
        assert_eq!(in_index(&o,id).len(),10);
        let out = tick::tick(c,&venue,id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
        assert!(matches!(out,TickOutcome::Done{placed:true}), "{out:?}");
        let chosen = in_index(&o,id);
        assert_eq!(chosen.len(),20.min(available-1) as usize);
        assert!(!chosen.iter().any(|(s,_)| s=="usd-coin"));
        let expected: Vec<_> = (1..30).filter(|i|if available==25 {i%6!=5} else {*i<available}).take(20).map(|i|format!("coin-{i}")).collect();
        assert_eq!(chosen.iter().map(|(s,_)|s.clone()).collect::<Vec<_>>(),expected);
        let held: bool = c.query_row("SELECT in_index=0 FROM bot_index_assets WHERE bot_id=?1 AND asset_id=?2",(id,members[0].0),|r|r.get(0)).unwrap();
        assert!(held,"the stablecoin holding remains recorded outside the corrected composition");
        let note: Option<String> = c.query_row("SELECT message FROM bot_activity_logs WHERE bot_id=?1 AND event='index_composition_shortfall'",[id],|r|r.get(0)).optional().unwrap();
        if available < 20 { assert!(note.unwrap().contains("15 of 20")); } else { assert!(note.is_none()); }
        assert_eq!(venue.sent().len(),chosen.len(),"new members are bought by the ordinary tick");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn index_searches_beyond_three_times_n_and_skips_taxonomy_stablecoins() {
    let (_d,o,_s,id,m) = universe(2,0.0,false);
    o.primary.execute("UPDATE assets SET category='stablecoin' WHERE id=?1",[m[0].0]).unwrap();
    let mut ids: Vec<String> = (0..8).map(|i|format!("unlisted-{i}")).collect();
    ids.extend(SYMBOLS.iter().map(|s|format!("{s}.US")));
    o.primary.execute("UPDATE indices SET source='coingecko',top_coins=?1",[json!(ids).to_string()]).unwrap();
    let t = script([100.0;5]);
    let out = tick::tick(&o.primary,&venue(&t),id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
    assert!(matches!(out,TickOutcome::Done{placed:true}),"{out:?}");
    assert_eq!(in_index(&o,id).iter().map(|(s,_)|s.as_str()).collect::<Vec<_>>(),["BBB","CCC"]);
}

/// D-ASSET-TAXONOMY: the generated catalogue labels these stablecoins Cryptocurrency.
/// Import its real rows, preserving that sync-routing category, and rank them ahead of BTC/ETH.
async fn bundled_stablecoin_index() -> (tempfile::TempDir, store::Opened, seed::Seeded, i64) {
    use deltabadger::jobs::{import, Db};
    let (d, o, s) = common::install();
    let catalogue: Value = serde_json::from_str(include_str!("../../db/seed_data/assets.json")).unwrap();
    let ids = ["usds", "ethena-usde", "usd1-wlfi", "bitcoin", "ethereum"];
    let rows: Vec<Value> = ids.iter().enumerate().map(|(n, id)| {
        let mut row = catalogue["data"].as_array().unwrap().iter().find(|a| a["external_id"] == *id).unwrap().clone();
        assert_eq!(row["category"], "Cryptocurrency");
        row["market_cap"] = json!(1000 - n);
        row
    }).collect();
    let db = Db::new(rusqlite::Connection::open(o.primary.path().unwrap()).unwrap(), seed::cipher());
    import::import_assets(&db, import::plan_assets(&rows, "").unwrap(), at(T0)).await.unwrap();
    let c = &o.primary;
    for row in rows {
        c.execute("INSERT OR REPLACE INTO tickers (exchange_id,base_asset_id,quote_asset_id,ticker,base,quote,available,trading_enabled,base_decimals,quote_decimals,price_decimals,minimum_base_size,minimum_quote_size,created_at,updated_at) SELECT ?1,id,?2,symbol||'EUR',symbol,'EUR',1,1,8,2,2,0.001,0.01,'2026-01-01','2099-01-01' FROM assets WHERE external_id=?3",
            rusqlite::params![s.exchange_id,s.quote,row["external_id"].as_str().unwrap()]).unwrap();
    }
    seed::insert_index(c, "top-coins", &ids, &json!({}));
    let id = seed::index_bot(c, &s, "top-coins", 2, 1.0, false);
    (d, o, s, id)
}

#[tokio::test(flavor = "current_thread")]
async fn bundled_stablecoins_are_excluded_from_top_n() {
    let (_d, o, _s, id) = bundled_stablecoin_index().await;
    let venue = deltabadger::venue::fake::FakeVenue::new()
        .ticker("USDSEUR", "10", "10", "10").ticker("USDEEUR", "10", "10", "10").ticker("USD1EUR", "10", "10", "10")
        .ticker("BTCEUR", "10", "10", "10").ticker("ETHEUR", "10", "10", "10");
    let bot = model::load_bot(&o.primary, id).unwrap();
    index::refresh_composition(&o.primary, &venue, &bot, &FixedClock(at(T0)), &Default::default()).await.unwrap().unwrap();
    assert_eq!(in_index(&o, id), [("BTC".into(), 0.5), ("ETH".into(), 0.5)]);
}

#[tokio::test(flavor = "current_thread")]
async fn bundled_stablecoins_are_excluded_from_the_preview() {
    use common::web::{self, Browser, Csrf, TestClock};
    use deltabadger::web::session;
    let (d, o, s, id) = bundled_stablecoin_index().await;
    o.primary.execute("UPDATE users SET confirmed_at=created_at", []).unwrap();
    let app = web::app(d.path(), "engine-test-secret", TestClock::at(T0));
    let cookie = session::seal(&app.keys.session, &session::SessionData { user: Some((s.user_id, "x".into())), ..Default::default() }, app.now());
    let mut browser = Browser { cookie: Some(cookie), page: None };
    let page = browser.send(&app, "GET", &format!("/bots/{id}"), None, Csrf::None, &[]).await;
    assert_eq!(page.status, 200);
    let preview = page.body.split("class=\"index-assets\"").nth(1).expect("index preview").split("</form>").next().unwrap();
    for symbol in ["USDS", "USDE", "USD1"] { assert!(!preview.contains(&format!("data-ticker-symbol=\"{symbol}\"")), "{symbol} in preview"); }
    for symbol in ["BTC", "ETH"] { assert!(preview.contains(&format!("data-ticker-symbol=\"{symbol}\"")), "{symbol} missing from preview"); }
}

/// D-ASSET-TAXONOMY grid: Rails' generated rows retain their routing category;
/// the asset classifier corrects all 55 stablecoins in this supported catalogue.
#[test]
fn bundled_stablecoin_classification_grid() {
    use sha2::{Digest, Sha256};
    assert_eq!(format!("{:x}", Sha256::digest(include_bytes!("../../db/seed_data/assets.json"))),
        "14c08bfb06ba8c04b932f15bd7ef67b03c8cd5a4d9d7a04989e5c89506419e46",
        "the generated catalogue changed: audit its stablecoin category overrides before updating this guard");
    let stablecoins = "agora-dollar anchored-coins-eur aussie-dollar-token bfusd brl1 cash-4 chain-key-usdt dai ethena-usde eurite euro-coin falcon-finance fathom-dollar felix-feusd fidelity-digital-dollar first-digital-usd frankencoin frax freedom-dollar frontier-stable-token gemini-dollar gho global-dollar hex-trust-usdx hyper-usd kyrgyz-som-stablecoin last-usd mxnb novatti-australian-digital-dollar paxos-standard paypal-usd pht-stablecoin quantoz-eurq quantoz-usdq ripple-usd schuman-europ stablr-euro stablr-usd straitsx-xusd terrausd tether tokenised-gbp true-usd united-stables usa usd-coin usd-mapped-token usd1-wlfi usdd usdh-2 usds usdt0 usdtb worldwide-usd xsgd";
    let catalogue: Value = serde_json::from_str(include_str!("../../db/seed_data/assets.json")).unwrap();
    let rows = catalogue["data"].as_array().unwrap();
    assert_eq!(stablecoins.split_whitespace().count(), 50 + 5, "50 category matches plus five legacy/bridged catalogue identities");
    for id in stablecoins.split_whitespace() {
        let row = rows.iter().find(|a| a["external_id"] == id).unwrap_or_else(|| panic!("{id} missing from supported catalogue"));
        assert_eq!(row["category"], "Cryptocurrency", "{id}: oracle taxonomy changed; review D-ASSET-TAXONOMY");
        assert!(index::stablecoin(row["category"].as_str(), id), "{id} must be classified as a stablecoin");
    }
    // Governance tokens, chain tokens and gold/treasury wrappers are not fiat stablecoins.
    for id in ["bitcoin", "ethereum", "ethena", "sky", "stable-2", "pax-gold", "tether-gold", "ondo-us-dollar-yield"] {
        let row = rows.iter().find(|a| a["external_id"] == id).unwrap();
        assert!(!index::stablecoin(row["category"].as_str(), id), "{id} is not a stablecoin");
    }
    assert!(index::stablecoin(Some("stablecoin"), "new-provider-stablecoin"), "new provider classifications remain authoritative without a bundled override");
    assert!(!index::stablecoin(Some("Stablecoin"), "pax-gold"), "the explicit RWA override wins over CoinGecko's commodity-backed stablecoin category");
}
