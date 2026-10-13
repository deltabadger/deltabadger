//! The Rust half of the tracker check on a copy of an instance (script/rust/tracker_on_copy.rb is the Rails half):
//! `tracker_ledger`'s run on a scratch copy at a frozen time, with data-api and the ECB scripted exactly as the Rails half
//! scripts them (every price answer empty, the ECB and the Bundesbank unavailable), then the scopes, today's snapshot
//! rows and the requests, as JSON; then the backfill's run, which must refuse (Plan D2b-2a) and leave every earlier day
//! and both history stamps exactly as they were.
//!
//! Ignored unless asked for: `TRACKER_COPY=<dir> TRACKER_COPY_USER=<id> TRACKER_COPY_AT=<ISO time> cargo test --test
//! tracker_on_copy -- --ignored`. It writes `<dir>/rust.json`. The copy is written to; no key is decrypted.
use deltabadger::tracker::{fx, jobs, parity};
use deltabadger::venue::http::ScriptedTransport;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

fn env(name: &str) -> String { std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set")) }

/// Every row of `table` matching `filter`, every column but the timestamps, in order.
fn rows(c: &Connection, table: &str, filter: &str, order: &str) -> Vec<Value> {
    let mut s = c.prepare(&format!("SELECT * FROM {table} WHERE {filter} ORDER BY {order}")).unwrap();
    let names: Vec<String> = s.column_names().iter().map(|n| n.to_string()).collect();
    let mut q = s.query([]).unwrap();
    let mut out = vec![];
    while let Some(r) = q.next().unwrap() {
        let mut row = Map::new();
        for (i, n) in names.iter().enumerate() {
            if ["id", "created_at", "updated_at"].contains(&n.as_str()) { continue; }
            row.insert(n.clone(), parity::raw(r.get_ref(i).unwrap()));
        }
        out.push(Value::Object(row));
    }
    out
}

fn history(c: &Connection, user: i64, today: &str) -> Value {
    let stamps: Vec<(String, Option<String>)> = c.prepare("SELECT key, value FROM app_configs WHERE key IN (?1, ?2) ORDER BY key").unwrap()
        .query_map([format!("snapshot_history_version_{user}"), format!("snapshot_price_generation_{user}")], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
        .collect::<Result<_, _>>().unwrap();
    json!({ "whole": rows(c, "portfolio_snapshots", &format!("user_id = {user} AND date < '{today}'"), "date"),
            "venues": rows(c, "portfolio_venue_snapshots", &format!("user_id = {user} AND date < '{today}'"), "exchange_id, date"),
            "stamps": stamps })
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn the_ledger_job_on_a_copy_states_rails_figures_and_the_backfill_leaves_history_alone() {
    let dir = PathBuf::from(env("TRACKER_COPY"));
    let user: i64 = env("TRACKER_COPY_USER").parse().unwrap();
    let at: chrono::DateTime<chrono::Utc> = env("TRACKER_COPY_AT").parse().unwrap();
    let today = at.date_naive().to_string();
    let path = dir.join("production.sqlite3");
    let c = Connection::open(&path).unwrap();
    c.execute("DELETE FROM app_configs WHERE key = 'market_data_provider'", []).unwrap();
    let before = history(&c, user, &today);
    let cipher = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "tracker-on-copy").unwrap());
    let db = deltabadger::jobs::Db::new(Connection::open(&path).unwrap(), cipher);
    let market = ScriptedTransport::from_script(&json!({ "GET /api/v1/historical_prices": [{ "status": 200, "body": { "prices": [] } }] }));
    let mut ecb_script = Map::new();
    ecb_script.insert(format!("GET {}", fx::ecb_path()), json!([{ "status": 500, "body": "" }]));
    for currency in fx::CURRENCIES { ecb_script.insert(format!("GET {}", fx::bundesbank_path(currency)), json!([{ "status": 500, "body": "" }])); }
    let ecb_t = ScriptedTransport::from_script(&Value::Object(ecb_script));
    let ecb = fx::Source::Scripted(ecb_t.clone());
    let api = deltabadger::jobs::data_api::DataApi::new(deltabadger::jobs::data_api::Config { url: "http://data-api:3000".into(), token: "copy-token".into() }, market.clone(), market.clone());
    let clock = deltabadger::engine::FixedClock(at);
    let out = jobs::ledger_run(&db, Some(&api), Some(&ecb), user, &clock, std::sync::Arc::new(move || at), &mut jobs::Allowance::run()).await;
    let (error, whole, venues) = match &out {
        Ok(w) => {
            let mut v = w.venues.clone();
            v.sort_by_key(|(id, _)| *id);
            (Value::Null, parity::summary_json(&w.whole), Value::Object(v.iter().map(|(id, s)| (id.to_string(), parity::summary_json(s))).collect()))
        }
        Err(e) => (json!(e), Value::Null, Value::Null),
    };
    let c = Connection::open(&path).unwrap();
    let tables = json!({ "portfolio_snapshots": rows(&c, "portfolio_snapshots", &format!("date = '{today}'"), "date"),
                         "portfolio_venue_snapshots": rows(&c, "portfolio_venue_snapshots", &format!("date = '{today}'"), "exchange_id, date") });
    let report = json!({ "error": error, "tables": tables, "requests": { "market": parity::logged(&market), "ecb": parity::logged(&ecb_t) }, "whole": whole, "venues": venues });
    std::fs::write(dir.join("rust.json"), serde_json::to_string_pretty(&report).unwrap()).unwrap();
    let backfill = jobs::backfill_run(&db, &deltabadger::venue::alpaca::LiveFactory::new(), Some(&api), Some(&ecb), user, &clock).await;
    println!("backfill: {backfill:?}");
    assert!(backfill.as_ref().is_err_and(|e| e.starts_with("the tracker backfill is not ported for ")), "{backfill:?}");
    assert_eq!(history(&c, user, &today), before, "the refused backfill changed an earlier day or a stamp");
}
