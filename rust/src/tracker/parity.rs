//! The Rust half of the tracker's value-parity harness (script/rust/tracker.rb is the Rails half): on a marked scratch
//! copy, the ledger job's own run (`jobs::ledger_run`: walk, then today's rows and wash-sale locks in its one write
//! unit) and then the backfill's, over the scripted data-api and Alpaca bodies, at the scenario's instant; reported as
//! Rails reports it.
use super::figures;
use super::walk::{Summary, Walked};
use crate::crypto::{Cipher, Credentials};
use crate::engine::FixedClock;
use crate::figures::{dec::Dec, FiguresError};
use crate::jobs::data_api::{Config, DataApi};
use crate::jobs::Db;
use crate::store::{self, Paths};
use crate::sync::jobs::Connect;
use crate::venue::alpaca::{AlpacaVenue, Urls};
use crate::venue::http::ScriptedTransport;
use chrono::{DateTime, Utc};
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
struct Scripted(ScriptedTransport);
impl Connect for Scripted {
    type T = ScriptedTransport;
    fn connect(&self, _: &Credentials) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(self.0.clone(), Urls::for_passphrase(None)) }
}

pub fn summary_json(s: &Summary) -> Value {
    let mut positions = s.positions.clone();
    positions.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    json!({
        "positions": positions.iter().map(|p| json!({ "symbol": p.symbol, "quantity": p.quantity.to_s_f(), "cost": p.cost.to_s_f(), "avg_cost": p.avg_cost.to_s_f(),
                                                     "estimated": p.estimated, "unpriced": p.unpriced.to_s_f() })).collect::<Vec<_>>(),
        "total_invested": s.total_invested.to_s_f(),
        "cash": s.cash.iter().map(|(c, a)| json!([c, a.to_s_f()])).collect::<Vec<_>>(),
        "cash_basis": s.cash_basis.iter().map(|(c, a)| json!([c, a.to_s_f()])).collect::<Vec<_>>(),
        "incomplete": s.incomplete,
        "loss_sales": s.loss_sales.iter().map(|(sym, on)| json!([sym, on.to_string()])).collect::<Vec<_>>(),
    })
}

fn figures_json(f: &figures::Figures) -> Result<Value, FiguresError> {
    let (held_value, held_cost) = f.without_cash()?;
    let mut holdings = f.holdings.clone();
    holdings.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    Ok(json!({
        "value": f.value.to_s_f(), "invested": f.invested.to_s_f(), "held_value": held_value.to_s_f(), "held_cost": held_cost.to_s_f(),
        "holdings": holdings.iter().map(|h| json!({ "symbol": h.symbol, "quantity": h.quantity.to_s_f(), "value": h.value.to_s_f(),
                                                   "cost": h.cost.as_ref().map(Dec::to_s_f) })).collect::<Vec<_>>(),
    }))
}

/// The scopes the walk stated and the figures each of today's rows was made from, as Rails' harness dumps them.
fn report(c: &Connection, user_id: i64, walked: &Walked, today: chrono::NaiveDate) -> Result<Value, FiguresError> {
    let mut venues = Map::new();
    let mut listed = walked.venues.clone();
    listed.sort_by_key(|(id, _)| *id);
    for (id, v) in &listed { venues.insert(id.to_string(), summary_json(v)); }
    let fx = super::fx::FxTable::load(c)?;
    let mut figs = Map::new();
    let empty = Summary::empty();
    let mut scopes: Vec<(Option<i64>, &Summary)> = vec![(None, &walked.whole)];
    for id in super::snapshot::venues(c, user_id)? { scopes.push((Some(id), walked.venue(id).unwrap_or(&empty))); }
    for (scope, ledger) in scopes {
        let balances = figures::balances(c, user_id, scope)?;
        let rows: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE user_id = ?1 AND (?2 IS NULL OR exchange_id = ?2))", rusqlite::params![user_id, scope], |r| r.get(0))?;
        if balances.is_empty() && !rows { continue; }
        let f = figures::compute_on(c, user_id, ledger, &balances, &figures::pending(c, user_id, scope)?, &fx, today)?;
        figs.insert(scope.map_or("whole".into(), |id| id.to_string()), figures_json(&f)?);
    }
    Ok(json!({ "whole": summary_json(&walked.whole), "venues": venues, "figures": figs }))
}

pub fn raw(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!({ "f": format!("{:016x}", f.to_bits()) }),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(_) => json!("<blob>"),
    }
}

/// The tables the tracker writes, every column but the two timestamps a SQL clock writes, and the two history keys
/// decrypted.
pub fn tables(c: &Connection, cipher: &Cipher, user_id: i64) -> Result<Value, String> {
    let mut out = Map::new();
    for (table, order) in [("portfolio_snapshots", "date"), ("portfolio_venue_snapshots", "exchange_id, date"), ("wash_sale_locks", "id"), ("historical_prices", "id")] {
        let (names,snapshot) = crate::figures::fill::parity_rows(c,table,order).map_err(|e| e.to_string())?;
        let mut rows = vec![];
        for r in snapshot {
            let mut row = Map::new();
            for (i, name) in names.iter().enumerate() {
                if table.starts_with("portfolio") && (name == "created_at" || name == "updated_at" || name == "id") { continue; }
                row.insert(name.clone(), raw((&r[i]).into()));
            }
            rows.push(Value::Object(row));
        }
        out.insert(table.into(), Value::Array(rows));
    }
    for key in [super::backfill::history_key(user_id), super::backfill::price_key(user_id)] {
        out.insert(key.clone(), json!(crate::app_config::get(c, cipher, &key)?));
    }
    Ok(Value::Object(out))
}

pub fn logged(t: &ScriptedTransport) -> Vec<Value> {
    t.requests().iter().map(|r| {
        let mut query: Vec<(String, String)> = r.query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        query.sort();
        json!([format!("{} {}", r.method, r.path), query])
    }).collect()
}

/// Runs the scenario in `dir` (a scratch copy the Rails harness built) and reports what Rails' `record` reports.
pub async fn run(dir: &Path, cipher: Arc<Cipher>) -> Result<Value, String> {
    let read = std::fs::read_to_string(dir.join("scenario.json")).map_err(|e| e.to_string())?;
    let scenario: Value = serde_json::from_str(&read).map_err(|e| e.to_string())?;
    if scenario["parity_scratch"] != true { return Err(format!("{} is not a parity scratch copy (no parity_scratch marker)", dir.display())); }
    let user_id = scenario["user_id"].as_i64().ok_or("scenario.json: user_id")?;
    let at: DateTime<Utc> = scenario["at"].as_str().and_then(|s| s.parse().ok()).ok_or("scenario.json: at")?;
    let opened = store::open(&Paths::from_env(&|_| None, dir)).map_err(|e| format!("{e:?}"))?;
    let db = Db::new(opened.primary, (*cipher).clone());
    let market = ScriptedTransport::from_script(&scenario["market"]);
    let alpaca = ScriptedTransport::from_script(&scenario["alpaca"]);
    if scenario["balances_synced_by_rust"] == true {
        // The balances as a Rust balance sync leaves them: produced, complete, by the reading key of their venue.
        db.run(move |c, _| {
            let mut s = c.prepare("SELECT id, exchange_id FROM api_keys WHERE user_id = ?1 AND status = 1 AND key_type != 1 ORDER BY id").map_err(|e| e.to_string())?;
            let keys: Vec<(i64, i64)> = s.query_map([user_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
            drop(s);
            for (id, exchange) in keys {
                let Some(version) = crate::engine::model::credential_version_by_id(c, id).map_err(|e| format!("{e:?}"))? else { continue };
                let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
                let fenced = crate::engine::model::fence_versions(&tx, std::slice::from_ref(&version)).map_err(|e| format!("{e:?}"))?;
                crate::sync::cache::record(&fenced, user_id, exchange, &version, true, at).map_err(|e| format!("{e:?}"))?;
                drop(fenced);
                tx.commit().map_err(|e| e.to_string())?;
            }
            Ok(())
        }).await?;
    }
    let ecb_script = ScriptedTransport::from_script(&scenario["ecb"]);
    let ecb = super::fx::Source::Scripted(ecb_script.clone());
    let api = DataApi::new(Config { url: "http://data-api:3000".into(), token: "parity-token".into() }, market.clone(), market.clone());
    let clock = FixedClock(at);
    let (ledger, ledger_error) = match super::jobs::ledger_run(&db, Some(&api), Some(&ecb), user_id, &clock, std::sync::Arc::new(move || at), &mut super::jobs::Allowance::run()).await {
        // The figures read the balances, which the job's writes leave as they were.
        Ok(walked) => (db.run(move |c, _| report(c, user_id, &walked, at.date_naive()).map_err(super::jobs::message)).await?, Value::Null),
        Err(e) => (Value::Null, json!(e)),
    };
    let backfill_error = match super::jobs::backfill_run(&db, &Scripted(alpaca.clone()), Some(&api), Some(&ecb), user_id, &clock).await { Ok(_) => Value::Null, Err(e) => json!(e) };
    let tables = db.run(move |c, cipher| tables(c, cipher, user_id)).await?;
    Ok(json!({ "ledger": ledger, "ledger_error": ledger_error, "backfill_error": backfill_error, "tables": tables,
               "requests": { "market": logged(&market), "alpaca": logged(&alpaca), "ecb": logged(&ecb_script) } }))
}
