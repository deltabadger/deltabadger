//! B2b: Alpaca index bots whose history holds REBALANCE, LIQUIDATION and REDEPLOY rows, against the Rails oracle
//! (script/rust/index_histories.rb -> fixtures/index_histories.json). Each scenario is built as Rails built it; the clock is
//! the oracle's own instant.
mod common;
use chrono::{DateTime, Utc};
use common::seed;
use deltabadger::engine::{accounting, amount, basket, model};
use deltabadger::figures::{at::At, db, walk};
use deltabadger::ruby::BigDec;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const SYMBOLS: [&str; 4] = ["AAA", "BBB", "CCC", "DDD"];
const EXTERNAL: [&str; 5] = ["unknown", "open", "closed", "cancelled", "abandoned"];

fn fixture() -> &'static Value {
    static V: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    V.get_or_init(|| serde_json::from_str(include_str!("fixtures/index_histories.json")).unwrap())
}
fn time(iso: &str) -> DateTime<Utc> { iso.parse().unwrap() }
fn at() -> DateTime<Utc> { time(fixture()["universe"]["at"].as_str().unwrap()) }
fn stored(iso: &str) -> String { deltabadger::codec::format_time(time(iso)) }
fn scenario(name: &str) -> &'static Value {
    fixture()["scenarios"].as_array().unwrap().iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("{name}"))
}

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

/// Units per held asset (engine walk and figures walk), and every figure-page book, against `metrics(force: true)`; the
/// redeploy banked/spent against Redeployable. Every scenario, in flight or not: the walk itself is what Rails walks.
#[test]
fn walks_and_books_match_rails_for_every_index_history() {
    let scenarios = fixture()["scenarios"].as_array().unwrap();
    assert_eq!(scenarios.len(), 38);
    for sc in scenarios {
        let name = sc["name"].as_str().unwrap();
        let b = build(sc);
        let c = &b.o.primary;
        let rails = &sc["rails"]["walk"];
        let bot = model::load_bot(c, b.id).unwrap();
        let w = basket::walk(c, &bot, at()).unwrap_or_else(|e| panic!("{name}: engine walk {e:?}"));
        for (sym, want) in rails["holdings"].as_object().unwrap() {
            assert_eq!(BigDec::parse(&d(w.amounts.get(&b.assets[sym]).cloned())).unwrap(), BigDec::parse(want["amount"].as_str().unwrap()).unwrap(),
                       "{name}: engine units of {sym}");
        }
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
        assert_eq!((banked.to_s_f(), spent.to_s_f()), (sc["rails"]["redeploy"]["banked"].as_str().unwrap().into(), sc["rails"]["redeploy"]["spent"].as_str().unwrap().into()),
                   "{name}: redeploy banked/spent");
        assert_eq!(amount::pending_quote_amount(c, &bot, at().timestamp_micros()).unwrap().to_s_f(), sc["rails"]["pending_quote_amount"], "{name}: carry");
    }
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
