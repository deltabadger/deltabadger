//! B2b: Alpaca index bots whose history holds REBALANCE, LIQUIDATION and REDEPLOY rows, against the Rails oracle
//! (script/rust/index_histories.rb -> fixtures/index_histories.json). Each scenario is built as Rails built it; the clock is
//! the oracle's own instant.
mod common;
use chrono::{DateTime, Utc};
use common::scripted::{ok, script, venue};
use common::seed;
use deltabadger::engine::{accounting, amount, basket, eligibility, model, tick, FixedClock};
use deltabadger::figures::{at::At, db, walk};
use deltabadger::ruby::{from_sql, BigDec};
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const SYMBOLS: [&str; 4] = ["AAA", "BBB", "CCC", "DDD"];
const EXTERNAL: [&str; 5] = ["unknown", "open", "closed", "cancelled", "abandoned"];

fn fixture() -> &'static Value {
    static V: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    V.get_or_init(|| serde_json::from_str(include_str!("fixtures/index_histories.json")).unwrap())
}
fn cipher() -> &'static deltabadger::crypto::Cipher {
    static C: std::sync::OnceLock<deltabadger::crypto::Cipher> = std::sync::OnceLock::new();
    C.get_or_init(seed::cipher)
}
fn time(iso: &str) -> DateTime<Utc> { iso.parse().unwrap() }
fn at() -> DateTime<Utc> { time(fixture()["universe"]["at"].as_str().unwrap()) }
fn stored(iso: &str) -> String { deltabadger::codec::format_time(time(iso)) }
fn scenario(name: &str) -> &'static Value {
    fixture()["scenarios"].as_array().unwrap().iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("{name}"))
}

/// What the engine keeps refusing, and the reason `check` names. Rails trades on the last two (RULING-B2B-1 MQ1,
/// RULING-B2B-R1 item 1): their fill is unknown, so a Yes or the next split could spend the same money twice. Rails also
/// trades on the two special buys it skips; the normalizer would count their units and buy more of every other member.
const REFUSED: [(&str, &str); 14] = [
    ("liquidation_waiting", "waiting LIQUIDATION/REDEPLOY/REBALANCE"),
    ("liquidation_partly_filled_open", "waiting LIQUIDATION/REDEPLOY/REBALANCE"),
    ("liquidation_unknown", "waiting LIQUIDATION/REDEPLOY/REBALANCE"),
    ("liquidation_placing_intent", "liquidation_pending"),
    ("liquidation_ambiguous", "liquidation_pending"),
    ("liquidation_abandoned_unresolved", "unresolved abandoned LIQUIDATION"),
    ("redeploy_waiting", "waiting LIQUIDATION/REDEPLOY/REBALANCE"),
    ("redeploy_ambiguous", "redeploy_pending"),
    ("rebalance_selling", "rebalance_pending"),
    ("rebalance_buying", "rebalance_pending"),
    ("redeploy_abandoned", "unresolved abandoned REDEPLOY"),
    ("rebalance_abandoned", "abandoned REBALANCE"),
    ("redeploy_zero_quote", "REBALANCE/REDEPLOY buy(s) the Rails walk skips"),
    ("redeploy_unpriced_positive_quote", "REBALANCE/REDEPLOY buy(s) the Rails walk skips"),
];
/// Special buys Rails' walk skips: the engine refuses to size from them (display keeps the normalized reading).
const SKIPPED_BUYS: [&str; 2] = ["redeploy_zero_quote", "redeploy_unpriced_positive_quote"];
fn refusal(name: &str) -> Option<&'static str> { REFUSED.iter().find(|(n, _)| *n == name).map(|(_, r)| *r) }

struct Built { _d: tempfile::TempDir, o: deltabadger::store::Opened, id: i64, assets: BTreeMap<String, i64> }

/// The scenario's install: four Alpaca stocks with the oracle's ticker defaults, data-api's ranking of exactly the stored
/// in-index members (cap = target × 10, pure cap weight, so the tick's refresh rewrites nothing), the bot, its members,
/// rows and transient data as IndexOracle#build writes them.
fn build(sc: &Value) -> Built {
    let u = &fixture()["universe"];
    let (d, o, s) = common::install_alpaca();
    let c = &o.primary;
    let mut assets = BTreeMap::new();
    let mut tickers = BTreeMap::new();
    for sym in SYMBOLS {
        let (a, t) = seed::add_alpaca_stock(c, &s, sym);
        assets.insert(sym.to_string(), a);
        tickers.insert(sym.to_string(), t);
    }
    let members = sc["members"].as_array().unwrap();
    let ranked: Vec<&Value> = members.iter().filter(|m| m["in_index"] == true).collect();
    let top: Vec<String> = ranked.iter().map(|m| format!("{}.US", m["symbol"].as_str().unwrap())).collect();
    let caps: serde_json::Map<String, Value> = ranked.iter().map(|m| {
        (format!("{}.US", m["symbol"].as_str().unwrap()), json!((m["target"].as_str().unwrap().parse::<f64>().unwrap() * 10.0).round()))
    }).collect();
    seed::insert_index(c, "nasdaq-100", &top.iter().map(String::as_str).collect::<Vec<_>>(), &Value::Object(caps));
    let id = seed::index_bot(c, &s, "nasdaq-100", 3, 0.0, false);
    let started = stored(u["started_at"].as_str().unwrap());
    c.execute("UPDATE bots SET started_at = ?1, settings_changed_at = NULL, redeploy_declined_offset = ?2 WHERE id = ?3",
              params![started, sc["declined_offset"].as_str().unwrap(), id]).unwrap();
    for m in members {
        let sym = m["symbol"].as_str().unwrap();
        let in_index = m["in_index"] == true;
        c.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, exited_at, created_at, updated_at) \
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?6, ?6)",
                  params![id, assets[sym], tickers[sym], m["target"].as_str(), in_index, started, (!in_index).then_some("2026-08-11 15:00:00")]).unwrap();
    }
    for (i, r) in sc["rows"].as_array().unwrap().iter().enumerate() {
        let text = |k: &str| r[k].as_str();
        let status = ["submitted", "failed", "skipped"].iter().position(|s| r["status"] == *s).unwrap() as i64;
        let ext = EXTERNAL.iter().position(|s| r["external_status"] == *s).unwrap() as i64;
        let sym = text("base").unwrap();
        c.execute(
            "INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, amount, quote_amount, price, \
             amount_exec, quote_amount_exec, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, \
             error_messages, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, ?9, ?10, ?11, ?12, 'USD', ?13, ?14, 'week', 60, ?15, '[]', ?16, ?16)",
            params![id, s.exchange_id, format!("ORD-{}", i + 1), status, ext, i64::from(r["side"] == "sell"), text("amount"), text("quote_amount"),
                    text("price"), text("amount_exec"), text("quote_amount_exec"), sym, assets[sym], s.quote, text("transaction_type"),
                    stored(text("created_at").unwrap())]).unwrap();
    }
    let mut transient = sc["transient"].clone();
    if transient["rebalance_pending"]["sell_transaction_id"] == "last row" {
        let last: i64 = c.query_row("SELECT max(id) FROM transactions WHERE bot_id = ?1", [id], |r| r.get(0)).unwrap();
        transient["rebalance_pending"]["sell_transaction_id"] = json!(last);
    }
    if sc["resolve_abandoned"] == true {
        let mut q = c.prepare("SELECT id FROM transactions WHERE bot_id = ?1 AND transaction_type = 'LIQUIDATION' AND external_status = 4").unwrap();
        let ids: Vec<i64> = q.query_map([id], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        transient["liquidation_resolved_orders"] = json!(ids);
    }
    c.execute("UPDATE bots SET transient_data = ?1 WHERE id = ?2", params![transient.to_string(), id]).unwrap();
    seed::fresh_stock_jobs(c, at());
    Built { _d: d, o, id, assets }
}

fn d(v: Option<BigDec>) -> String { v.unwrap_or_else(BigDec::zero).to_s_f() }

/// The oracle's ask prices, an open market, accepted orders and a funded account.
fn transport() -> deltabadger::venue::http::ScriptedTransport {
    let mut m = json!({
        "GET /v2/clock": [ok(json!({ "timestamp": "2026-08-24T10:00:01-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" }))],
        "POST /v2/orders": (1..=5).map(|n| json!({ "status": 200, "body": { "id": format!("OTX-{n}"), "status": "accepted" } })).collect::<Vec<_>>(),
        "GET /v2/account": [ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" }))],
        "GET /v2/positions": [ok(json!([]))],
    });
    for (sym, ask) in fixture()["universe"]["ask_prices"].as_object().unwrap() {
        m[format!("GET /v2/stocks/{sym}/quotes/latest")] = json!([ok(json!({ "quote": { "ap": ask.as_str().unwrap().parse::<f64>().unwrap(), "bp": 1 } }))]);
    }
    script(m)
}

/// Units per held asset (engine walk and figures walk), and every figure-page book, against `metrics(force: true)`; the
/// redeploy banked/spent against Redeployable. Every scenario, in flight or not: the walk itself is what Rails walks.
#[test]
fn walks_and_books_match_rails_for_every_index_history() {
    let scenarios = fixture()["scenarios"].as_array().unwrap();
    assert_eq!(scenarios.len(), 40);
    for sc in scenarios {
        let name = sc["name"].as_str().unwrap();
        let b = build(sc);
        let c = &b.o.primary;
        let rails = &sc["rails"]["walk"];
        let bot = model::load_bot(c, b.id).unwrap();
        let mut rails = rails.clone();
        let mut spent_want = sc["rails"]["redeploy"]["spent"].as_str().unwrap().to_string();
        if SKIPPED_BUYS.contains(&name) {
            let refused = basket::walk(c, &bot, at()).unwrap_err();
            assert!(format!("{refused:?}").contains("the Rails walk skips"), "{name}: {refused:?}");
            // The figures page keeps the normalized reading: the 0.03 DDD at $12 Rails skips, paid from the proceeds.
            rails["holdings"]["DDD"] = json!({ "amount": "0.03", "invested": "12.0" });
            for k in ["uninvested_cash", "realised_cash"] { rails[k] = json!("14.4"); }
            spent_want = "12.0".into();
        } else {
            let w = basket::walk(c, &bot, at()).unwrap_or_else(|e| panic!("{name}: engine walk {e:?}"));
            for (sym, want) in rails["holdings"].as_object().unwrap() {
                assert_eq!(BigDec::parse(&d(w.amounts.get(&b.assets[sym]).cloned())).unwrap(), BigDec::parse(want["amount"].as_str().unwrap()).unwrap(),
                           "{name}: engine units of {sym}");
            }
        }
        let rails = &rails;
        let subject = db::Subject::load(c, b.id).unwrap();
        let m = walk::metrics(c, &subject, At::from_utc(at()).unwrap()).unwrap_or_else(|e| panic!("{name}: figures walk {e:?}"));
        let num = |n: &deltabadger::figures::num::Num| n.to_d().unwrap().to_s_f();
        let symbol_of = |key: &str| -> String {
            let id = m.key_assets.iter().find(|(k, _)| k == key).and_then(|(_, id)| *id).unwrap();
            b.assets.iter().find(|(_, a)| **a == id).map(|(s, _)| s.clone()).unwrap()
        };
        let holdings: BTreeMap<String, Value> = m.asset_breakdown.iter()
            .map(|(k, h)| (symbol_of(k), json!({ "amount": num(&h.amount), "invested": num(&h.quote_invested) }))).collect();
        assert_eq!(json!(holdings), rails["holdings"], "{name}: holdings");
        let walked = m.walked.as_ref().unwrap();
        let got = json!({
            "contributed": num(&m.total_quote_amount_invested), "uninvested_cash": num(&m.rebalance_cash), "realised_cash": num(&walked.realised_cash),
            "estimated_proceeds": num(&m.estimated_proceeds), "realised_pnl": num(&m.realised_pnl), "external_sales": walked.external_sales,
        });
        let mut want = rails.clone();
        want.as_object_mut().unwrap().remove("holdings");
        assert_eq!(got, want, "{name}: books");
        let (banked, spent) = accounting::index_redeploy_totals(&db::orders(c, b.id).unwrap()).unwrap();
        assert_eq!((banked.to_s_f(), spent.to_s_f()), (sc["rails"]["redeploy"]["banked"].as_str().unwrap().into(), spent_want.clone()),
                   "{name}: redeploy banked/spent");
        assert_eq!(amount::pending_quote_amount(c, &bot, at().timestamp_micros()).unwrap().to_s_f(), sc["rails"]["pending_quote_amount"], "{name}: carry");
    }
}

/// Settled histories pass `check` and the write guard; every in-flight or unknown-fill state is refused by name.
#[test]
fn settled_histories_are_eligible_and_in_flight_ones_refused_by_name() {
    for sc in fixture()["scenarios"].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap();
        let b = build(sc);
        let report = eligibility::check_install(&b.o.primary).unwrap();
        assert!(report.unreadable.is_empty(), "{name}: {:?}", report.unreadable);
        match refusal(name) {
            None => {
                assert!(report.problems.is_empty(), "{name}: {:?}", report.problems);
                assert_eq!(report.eligible, vec![b.id], "{name}");
                let tx = model::immediate(&b.o.primary).unwrap();
                tx.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.unrelated', 'kept') WHERE id = ?1", [b.id]).unwrap();
                eligibility::guard(&tx, cipher(), Some(b.id)).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            }
            Some(reason) => {
                assert!(report.problems.iter().any(|p| p.contains(reason)), "{name}: {reason} not in {:?}", report.problems);
                assert!(report.eligible.is_empty(), "{name}");
            }
        }
    }
}

/// The next tick of every eligible scenario: Rails' legs exactly (member, BigDecimal quote with its division digits), the
/// orders on the wire, and the rows written. A refused scenario places nothing.
#[tokio::test(flavor = "current_thread")]
async fn next_ticks_match_rails_and_refused_states_place_nothing() {
    for sc in fixture()["scenarios"].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap();
        let b = build(sc);
        let c = &b.o.primary;
        let rails = &sc["rails"]["tick"];
        let bot = model::load_bot(c, b.id).unwrap();
        if refusal(name).is_none() {
            // The split as get_orders_data computes it, before any rounding.
            let holdings = basket::walk(c, &bot, at()).unwrap().amounts;
            let asks = &fixture()["universe"]["ask_prices"];
            let priced: Vec<basket::Priced> = basket::members(c, &bot).unwrap().into_iter().map(|member| {
                let ask = BigDec::parse(asks[member.ticker.base_symbol.as_str()].as_str().unwrap()).unwrap();
                basket::Priced { price: amount::order_price(&bot, &member.ticker, &ask).unwrap(), reference: ask, member }
            }).collect();
            let x = amount::pending_quote_amount(c, &bot, at().timestamp_micros()).unwrap();
            let legs: Vec<Value> = basket::split(&priced, &holdings, &basket::reserved(c, &bot).unwrap(), &x).unwrap().into_iter()
                .map(|l| json!({ "base": l.ticker.base_symbol, "quote_amount": l.quote.to_s_f() })).collect();
            let want: Vec<Value> = rails["orders"].as_array().unwrap().iter().map(|o| json!({ "base": o["base"], "quote_amount": o["quote_amount"] })).collect();
            assert_eq!(json!(legs), json!(want), "{name}: legs");
        }
        // The engine never ticks a refused bot (run.rs ticks check's eligible list); a waiting row would only be polled.
        let waiting: i64 = c.query_row("SELECT count(*) FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status IN (0, 1)", [b.id], |r| r.get(0)).unwrap();
        if refusal(name).is_some() && waiting > 0 {
            assert!(!eligibility::check_install(c).unwrap().eligible.contains(&b.id), "{name}");
            continue;
        }
        let before: i64 = c.query_row("SELECT coalesce(max(id), 0) FROM transactions", [], |r| r.get(0)).unwrap();
        let t = transport();
        let _ = tick::tick(c, &venue(&t), b.id, &FixedClock(at()), &mut tick::Attempts::default()).await.unwrap();
        let posted: Vec<Value> = t.posted_orders().iter().map(|p| json!({ "symbol": p["symbol"], "side": p["side"], "notional": p["notional"] })).collect();
        if refusal(name).is_some() {
            assert!(posted.is_empty(), "{name}: refused, yet posted {posted:?}");
            continue;
        }
        let wire: Vec<Value> = rails["orders"].as_array().unwrap().iter().map(|o| {
            let q = BigDec::parse(o["submitted_amount"].as_str().unwrap()).unwrap();
            // What is sent: floored to the pair's quote decimals (2), as amount::size sizes it.
            json!({ "symbol": o["base"], "side": o["side"], "notional": format!("{:.2}", q.floor(2).to_f()) })
        }).collect();
        assert_eq!(json!(posted), json!(wire), "{name}: wire");
        let mut q = c.prepare("SELECT base, side, status, external_status, transaction_type, price, amount, quote_amount FROM transactions WHERE id > ?1 ORDER BY id").unwrap();
        let rows: Vec<Value> = q.query_map([before], |r| Ok(json!({
            "base": r.get::<_, String>(0)?, "side": if r.get::<_, i64>(1)? == 1 { "sell" } else { "buy" }, "status": (["submitted", "failed", "skipped"][r.get::<_, usize>(2)?]),
            "external_status": EXTERNAL[r.get::<_, usize>(3)?], "transaction_type": r.get::<_, String>(4)?,
            "price": d(from_sql(r.get_ref(5)?).unwrap()), "amount": d(from_sql(r.get_ref(6)?).unwrap()), "quote_amount": d(from_sql(r.get_ref(7)?).unwrap()),
        }))).unwrap().map(Result::unwrap).collect();
        let want: Vec<Value> = rails["rows"].as_array().unwrap().iter().map(|r| {
            let mut r = r.clone();
            for k in ["price", "amount", "quote_amount"] { r[k] = json!(BigDec::parse(r[k].as_str().unwrap()).unwrap().to_s_f()); }
            r
        }).collect();
        assert_eq!(json!(rows), json!(want), "{name}: rows");
    }
}

/// RULING-B2B-R1 item 2: a special row the venue rejected (status failed, nothing executed) is ignored exactly as Rails'
/// `submitted` scope ignores it. One that reports an execution anyway is unknown, and refused.
#[test]
fn rejected_special_rows_are_ignored_unless_they_report_an_execution() {
    for name in ["liquidation_rejected", "redeploy_rejected", "rebalance_rejected"] {
        let b = build(scenario(name));
        assert!(eligibility::check_install(&b.o.primary).unwrap().problems.is_empty(), "{name}");
        b.o.primary.execute("UPDATE transactions SET amount_exec = 0.01 WHERE bot_id = ?1 AND status = 1", [b.id]).unwrap();
        let problems = eligibility::check_install(&b.o.primary).unwrap().problems;
        assert!(problems.iter().any(|p| p.contains("rejected LIQUIDATION/REDEPLOY/REBALANCE order(s) reporting an execution")), "{name}: {problems:?}");
    }
}

/// Each B2b refusal holds through a refused tick, the check, a guarded write (rolled back) and a second tick: the failure
/// bookkeeping of the first tick never clears a row-based refusal.
#[tokio::test(flavor = "current_thread")]
async fn each_new_refusal_survives_a_refused_tick_and_rolls_back_a_guarded_write() {
    let cases: [(&str, &str, &str); 6] = [
        ("redeploy_abandoned", "", "unresolved abandoned REDEPLOY"),
        ("rebalance_abandoned", "", "abandoned REBALANCE"),
        ("full_cycle", "UPDATE bots SET transient_data = json_set(transient_data, '$.liquidation_selling_since', '2026-08-24T13:00:00Z')", "liquidation_selling_since"),
        ("full_cycle", "UPDATE transactions SET transaction_type = 'SWAP' WHERE transaction_type = 'REDEPLOY'", "unsupported transaction type"),
        ("full_cycle", "UPDATE transactions SET side = 1 WHERE transaction_type = 'REDEPLOY'", "REDEPLOY sell"),
        ("full_cycle", "UPDATE transactions SET base_asset_id = NULL WHERE transaction_type = 'LIQUIDATION'", "without base_asset_id"),
    ];
    for (name, sql, reason) in cases {
        let b = build(scenario(name));
        let c = &b.o.primary;
        if !sql.is_empty() { c.execute_batch(sql).unwrap(); }
        for round in 0..2 {
            let t = transport();
            let _ = tick::tick(c, &venue(&t), b.id, &FixedClock(at()), &mut tick::Attempts::default()).await;
            assert!(t.posted_orders().is_empty(), "{reason}: tick {round} posted");
            let problems = eligibility::check_install(c).unwrap().problems;
            assert!(problems.iter().any(|p| p.contains(reason)), "{reason}: round {round}: {problems:?}");
            let tx = model::immediate(c).unwrap();
            tx.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.probe', 'write') WHERE id = ?1", [b.id]).unwrap();
            let refused = eligibility::guard(&tx, cipher(), Some(b.id)).unwrap_err();
            assert!(refused.reason().contains(reason), "{reason}: {refused:?}");
            drop(tx);
            let probe: Option<String> = c.query_row("SELECT json_extract(transient_data, '$.probe') FROM bots WHERE id = ?1", [b.id], |r| r.get(0)).unwrap();
            assert_eq!(probe, None, "{reason}: the guarded write rolled back");
        }
    }
}

/// The blanket refusal stays for every bot outside the slice: a basket keeps "REBALANCE/LIQUIDATION/REDEPLOY row(s)".
#[test]
fn a_basket_keeps_the_blanket_special_history_refusal() {
    let b = build(scenario("rebalance_completed"));
    let c = &b.o.primary;
    c.execute("UPDATE bots SET type = 'Bots::DcaMultiAsset', settings = json_set(settings, '$.allocations', json(?1)) WHERE id = ?2",
              params![json!({ b.assets["AAA"].to_string(): 0.5, b.assets["BBB"].to_string(): 0.3, b.assets["CCC"].to_string(): 0.2 }).to_string(), b.id]).unwrap();
    let problems = eligibility::check_install(c).unwrap().problems;
    assert!(problems.iter().any(|p| p.contains("REBALANCE/LIQUIDATION/REDEPLOY row(s) in its history")), "{problems:?}");
}

/// RULING-B2B-1 MQ7 under B2-1, the one recorded divergence from the oracle's walk: a closed REDEPLOY buy that executed
/// 0.03 of the 0.06 it asked and reported no proceeds is read at what executed (0.03 × 400 = 12.0). Rails'
/// confirmed_exec_amounts books it at price × REQUESTED: DDD invested 24.0, spent 24.0, realised_cash 2.4. The units are
/// the same either way, so the tick's split is unchanged; the figures page shows 12.0 more realised cash than Rails, and
/// Rails' offer would be 12.0 lower (display only: this build has no redeploy action).
#[test]
fn a_partly_executed_unpriced_redeploy_is_read_at_what_executed() {
    let b = build(scenario("redeploy_closed_unpriced"));
    let c = &b.o.primary;
    c.execute("UPDATE transactions SET amount = 0.06 WHERE bot_id = ?1 AND transaction_type = 'REDEPLOY'", [b.id]).unwrap();
    let bot = model::load_bot(c, b.id).unwrap();
    assert_eq!(d(basket::walk(c, &bot, at()).unwrap().amounts.get(&b.assets["DDD"]).cloned()), "0.03");
    let m = walk::metrics(c, &db::Subject::load(c, b.id).unwrap(), At::from_utc(at()).unwrap()).unwrap();
    let ddd = m.asset_breakdown.iter().find(|(_, h)| h.amount.to_d().unwrap().to_s_f() == "0.03").unwrap();
    assert_eq!(ddd.1.quote_invested.to_d().unwrap().to_s_f(), "12.0");
    assert_eq!(m.walked.unwrap().realised_cash.to_d().unwrap().to_s_f(), "14.4");
    let (banked, spent) = accounting::index_redeploy_totals(&db::orders(c, b.id).unwrap()).unwrap();
    assert_eq!((banked.to_s_f(), spent.to_s_f()), ("26.4".into(), "12.0".into()));
}

/// A status outside Rails' enum (submitted, failed, skipped) proves nothing about the order: here a still-working $24 DDD
/// redeploy. It refuses the takeover whatever the bot's own status (a stopped, archived or deleted bot's order still sits at
/// the venue), and the tick's sweep refuses rather than skipping the row.
#[tokio::test(flavor = "current_thread")]
async fn a_status_rails_never_writes_refuses_the_install_and_the_sweep() {
    for status in ["NULL", "99"] {
        for bot_status in [2, 7, 3, 1] {
            let b = build(scenario("redeploy_waiting"));
            let c = &b.o.primary;
            c.execute_batch(&format!("UPDATE transactions SET status = {status} WHERE transaction_type = 'REDEPLOY'; UPDATE bots SET status = {bot_status}")).unwrap();
            let report = eligibility::check_install(c).unwrap();
            assert!(report.eligible.is_empty(), "status {status}, bot {bot_status}");
            assert!(report.problems.iter().any(|p| p.contains("row(s) with a status Rails never writes")), "status {status}, bot {bot_status}: {:?}", report.problems);
            assert!(report.refusal().is_err());
            // Engine start follows up every outstanding order of any bot: the row is unresolved, never skipped.
            let row: i64 = c.query_row("SELECT id FROM transactions WHERE transaction_type = 'REDEPLOY'", [], |r| r.get(0)).unwrap();
            let t = transport();
            let followed = deltabadger::engine::polling::follow_up(c, &venue(&t), b.id, row, at()).await;
            assert!(format!("{followed:?}").contains("status Rails never writes"), "status {status}, bot {bot_status}: {followed:?}");
            assert!(t.requests().is_empty(), "status {status}, bot {bot_status}");
            if bot_status == 1 {
                let t = transport();
                let _ = tick::tick(c, &venue(&t), b.id, &FixedClock(at()), &mut tick::Attempts::default()).await;
                let refused: i64 = c.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'execution_failed' \
                    AND (details LIKE '%unreadable transaction status%' OR details LIKE '%status Rails never writes%')", [b.id], |r| r.get(0)).unwrap();
                assert_eq!(refused, 1, "status {status}: the tick refuses (at the shared row reader, or else at the sweep)");
                assert!(t.posted_orders().is_empty() && !t.requests().iter().any(|r| r.path.starts_with("/v2/orders/")), "status {status}");
            }
        }
    }
}

/// The figures read every row's status before filtering: a completed liquidation stored with an unreadable status makes
/// the figures unavailable, never a holding of 0.08 CCC at $24 with no proceeds.
#[test]
fn figures_refuse_a_history_with_an_unreadable_status() {
    for status in ["NULL", "99"] {
        let b = build(scenario("liquidated_proceeds_waiting"));
        let c = &b.o.primary;
        c.execute_batch(&format!("UPDATE transactions SET status = {status} WHERE transaction_type = 'LIQUIDATION'")).unwrap();
        let figures = db::Subject::load(c, b.id).and_then(|s| walk::metrics(c, &s, At::from_utc(at()).unwrap()));
        assert!(format!("{figures:?}").contains("unreadable transaction status"), "status {status}: {figures:?}");
        assert!(format!("{:?}", db::orders(c, b.id)).contains("unreadable transaction status"), "status {status}");
    }
}
