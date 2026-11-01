//! Imported rows (script/rust/record_histories_imported.rb records fixtures/histories_imported.json): synthetic bots fed
//! through Rails' own CSV import, their tables installed as Rails stored them, then read by the engine's readers. Rails reads
//! an imported row as any submitted REGULAR row; so must every reader here. Also B1's PAXG basket (Cryptocurrency/tokenized).
mod common;
use chrono::{DateTime, Duration, Utc};
use deltabadger::{engine::{accounting, amount, basket, eligibility, model, tick}, ruby::BigDec};
use rusqlite::{types::Value as Sql, Connection};
use serde_json::{json, Value};

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/histories_imported.json")).unwrap() }
fn now() -> DateTime<Utc> { "2026-01-05T12:00:00Z".parse().unwrap() }

/// The case's tables, row for row and storage class for class (a Float as {"real": text}).
fn install(c: &Connection, tables: &Value) {
    c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    for (table, rows) in tables.as_object().unwrap() {
        c.execute(&format!("DELETE FROM {table}"), []).unwrap();
        for row in rows.as_array().unwrap() {
            let row = row.as_object().unwrap();
            let columns: Vec<String> = row.keys().map(|k| format!("\"{k}\"")).collect();
            let values: Vec<Sql> = row.values().map(|v| match v {
                Value::Object(real) => Sql::Real(real["real"].as_str().unwrap().parse().unwrap()),
                Value::Null => Sql::Null,
                Value::Number(n) if n.is_i64() => Sql::Integer(n.as_i64().unwrap()),
                Value::Number(n) => Sql::Real(n.as_f64().unwrap()),
                Value::String(s) => Sql::Text(s.clone()),
                other => Sql::Text(other.to_string()),
            }).collect();
            c.execute(&format!("INSERT INTO {table} ({}) VALUES ({})", columns.join(","), vec!["?"; row.len()].join(",")),
                rusqlite::params_from_iter(values)).unwrap();
        }
    }
}

fn build(case: &Value) -> (tempfile::TempDir, deltabadger::store::Opened, model::Bot) {
    let d = common::baseline_install();
    let o = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, d.path())).unwrap();
    install(&o.primary, &case["tables"]);
    let bot = model::load_bot(&o.primary, case["bot_id"].as_i64().unwrap()).unwrap();
    (d, o, bot)
}

#[test]
fn imported_rows_read_as_rails_reads_them() {
    let v = vectors();
    let cases = v["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 10);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let want = &case["normalized"];
        let (_d, o, bot) = build(case);
        let c = &o.primary;
        let at = now() + Duration::seconds(1);
        let assets: Vec<(String, i64)> = case["assets"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_i64().unwrap())).collect();
        let w = basket::walk(c, &bot, at).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let holdings: Value = assets.iter().map(|(s, id)| (s.clone(), json!(w.amounts.get(id).cloned().unwrap_or_else(BigDec::zero).to_s_f()))).collect();
        assert_eq!(holdings, want["holdings"], "{name} holdings");
        assert_eq!(w.contributed.to_s_f(), want["contributed"], "{name} contributed");
        assert_eq!(w.cash.to_s_f(), want["cash"], "{name} cash");
        let units: Value = assets.iter().map(|(s, id)| (s.clone(), json!(tick::local_units(c, &bot, *id).unwrap().to_s_f()))).collect();
        assert_eq!(units, want["local_units"], "{name} local_units");
        let quote = amount::quote_amount_available(c, &bot).unwrap();
        assert_eq!(quote.as_ref().map(|q| json!(q.to_s_f())).unwrap_or(Value::Null), want["quote_available"], "{name} quote cap");
        let base = accounting::base_amount_available_num(c, &bot).unwrap().map(|n| n.to_dec().unwrap());
        assert_eq!(base.as_ref().map(|b| json!(b.to_s_f())).unwrap_or(Value::Null), want["base_available"], "{name} base cap");
        let quote_reached = amount::quote_amount_limit_reached(c, &bot).unwrap();
        let base_reached = accounting::base_amount_limit_reached(c, &bot).unwrap();
        assert_eq!((json!(quote_reached), json!(base_reached)), (want["quote_reached"].clone(), want["base_reached"].clone()), "{name} reached");
        let pending = amount::pending_quote_amount(c, &bot, at.timestamp_micros()).unwrap();
        let pending = quote.as_ref().map_or(pending.clone(), |cap| pending.min(cap.clone()));
        assert_eq!(pending.to_s_f(), want["pending"], "{name} pending");
        let decision = if bot.selling() { if base_reached { "base_limit_reached" } else { "sell" } }
                       else if quote_reached { "quote_limit_reached" } else if pending.is_zero() { "skip_zero_pending" } else { "buy" };
        assert_eq!(decision, want["decision"], "{name} decision");
        if bot.bot_type == "Bots::DcaMultiAsset" && decision == "buy" {
            basket::refresh_composition(c, &bot, at).unwrap().unwrap();
            let priced: Vec<_> = basket::members(c, &bot, &[]).unwrap().into_iter()
                .map(|member| basket::Priced { member, reference: BigDec::from_i64(10), price: BigDec::from_i64(10) }).collect();
            let orders: Vec<_> = basket::split(&priced, &w.amounts, &basket::reserved(c, &bot).unwrap(), &pending).unwrap().into_iter()
                .map(|leg| json!({"side": "buy", "asset": leg.ticker.base_code, "amount": leg.quote.div(&BigDec::from_i64(10)).unwrap().to_s_f(), "quote": leg.quote.to_s_f()})).collect();
            assert_eq!(json!(orders), want["orders"], "{name} orders");
        }
        let later = amount::pending_quote_amount(c, &bot, (at + Duration::weeks(1)).timestamp_micros()).unwrap();
        let later = quote.map_or(later.clone(), |cap| later.min(cap));
        assert_eq!(later.to_s_f(), case["one_week_pending"], "{name} one week later");
    }
}

/// Rails' importer stores a CSV type outside market/limit as a NULL order_type (insert_all's enum cast).
#[test]
fn the_recorded_rows_are_what_rails_stores() {
    let v = vectors();
    let odd = v["cases"].as_array().unwrap().iter().find(|c| c["name"] == "odd_type").unwrap();
    assert_eq!(odd["stored"][0]["order_type"], Value::Null);
    for case in v["cases"].as_array().unwrap() {
        for row in case["stored"].as_array().unwrap() {
            assert!(row["external_id"].as_str().unwrap().starts_with("imported_"), "{}", case["name"]);
            let legacy = case["name"] == "legacy_open";
            assert_eq!(row["external_status"], json!(if legacy { 1 } else { 2 }), "{}", case["name"]);
        }
    }
}

/// A terminal imported row is admitted (and a Cryptocurrency/tokenized member, B1); a legacy waiting one is refused by
/// name (D-IMP-OPEN). An index bot is checked for its history only: its index data is outside these vectors.
#[test]
fn terminal_imported_rows_are_admitted_and_legacy_open_ones_refused() {
    for case in vectors()["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (_d, o, bot) = build(case);
        let reasons = eligibility::bot_reasons(&o.primary, &bot).unwrap();
        if name == "legacy_open" {
            assert!(reasons.iter().any(|r| r == "1 legacy imported row(s) left open (Rails never polls or settles them)"), "{reasons:?}");
            continue;
        }
        assert!(!reasons.iter().any(|r| r.contains("imported") || r.contains("asset category")), "{name}: {reasons:?}");
        if bot.bot_type == "Bots::DcaIndex" { continue; }
        let report = eligibility::check_install(&o.primary).unwrap();
        assert!(report.problems.is_empty() && report.unreadable.is_empty(), "{name}: {:?} {:?}", report.problems, report.unreadable);
        assert_eq!(report.eligible, vec![bot.id], "{name}");
    }
}
