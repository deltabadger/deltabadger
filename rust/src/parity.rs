//! The Rust half of the decision-parity harness (script/rust/decisions.rb is the Rails half): the same
//! ticks, with retries, on a marked scratch copy, reported in the canonical shape Rails reports.
use crate::engine::polling;
use crate::engine::tick::{self, Attempts, TickOutcome};
use crate::engine::{EngineError, FixedClock};
use crate::lease;
use crate::store::{self, Paths};
use crate::venue::fake::FakeVenue;
use crate::venue::{NewOrder, OrderKind};
use chrono::{DateTime, Utc};
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::path::Path;

const TABLES: [&str; 3] = ["bots", "transactions", "bot_activity_logs"];
const JSON_COLUMNS: [&str; 4] = ["settings", "transient_data", "details", "error_messages"];
const MAX_ATTEMPTS: usize = 6;

fn raw(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!({ "f": format!("{:016x}", f.to_bits()) }),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(_) => json!("<blob>"),
    }
}

pub fn snapshot(c: &Connection) -> Result<Value, EngineError> {
    let mut out = Map::new();
    for table in TABLES {
        let mut s = c.prepare(&format!("SELECT * FROM {table} ORDER BY id"))?;
        let names: Vec<String> = s.column_names().iter().map(|n| n.to_string()).collect();
        let mut rows = Map::new();
        let mut q = s.query([])?;
        while let Some(r) = q.next()? {
            let mut row = Map::new();
            for (i, name) in names.iter().enumerate() {
                if name == "last_end_of_funds_notification" { continue; }
                let mut v = raw(r.get_ref(i)?);
                if JSON_COLUMNS.contains(&name.as_str()) { if let Value::String(s) = &v { v = serde_json::from_str(s).unwrap_or(v); } }
                if name == "transient_data" { if let Value::Object(m) = &mut v { m.remove("failure_notifications"); m.remove("rust_placement"); } }
                row.insert(name.clone(), v);
            }
            rows.insert(row["id"].to_string(), Value::Object(row));
        }
        out.insert(table.into(), Value::Object(rows));
    }
    Ok(Value::Object(out))
}

pub fn diff(before: &Value, after: &Value) -> Value {
    let mut out = Map::new();
    for table in TABLES {
        let mut changes: Vec<(i64, Value)> = after[table].as_object().unwrap().iter()
            .filter(|(id, row)| before[table].get(id.as_str()) != Some(*row))
            .map(|(id, row)| { let n = id.parse::<i64>().unwrap(); (n, json!({ "id": n, "before": before[table].get(id.as_str()).cloned().unwrap_or(Value::Null), "after": row })) })
            .collect();
        changes.sort_by_key(|(n, _)| *n); // Rails lists rows in id order; the map above is keyed by id TEXT
        out.insert(table.into(), Value::Array(changes.into_iter().map(|(_, v)| v).collect()));
    }
    Value::Object(out)
}

/// The AddOrder body Honeymaker::Clients::Kraken posts for this order, minus nonce (and Rust's cl_ord_id/deadline).
pub fn wire(o: &NewOrder) -> Value {
    let mut v = json!({ "type": "buy", "volume": o.volume, "pair": o.pair });
    if o.quote_volume { v["oflags"] = json!("viqc"); }
    match &o.kind {
        OrderKind::Market => { v["ordertype"] = json!("market"); }
        OrderKind::Limit { price } => { v["ordertype"] = json!("limit"); v["price"] = json!(price); }
    }
    v
}

pub async fn decide(dir: &Path) -> Result<Value, EngineError> {
    let read = std::fs::read_to_string(dir.join("scenario.json")).map_err(|e| EngineError::Data(e.to_string()))?;
    let scenario: Value = serde_json::from_str(&read).map_err(|e| EngineError::Data(e.to_string()))?;
    if scenario["parity_scratch"] != true {
        return Err(EngineError::Data(format!("{} is not a parity scratch copy (no parity_scratch marker)", dir.display())));
    }
    let start: DateTime<Utc> = scenario["at"].as_str().and_then(|s| s.parse().ok()).ok_or_else(|| EngineError::Data("scenario.at".into()))?;
    let bot_id = scenario["bot_id"].as_i64().ok_or_else(|| EngineError::Data("scenario.bot_id".into()))?;
    let paths = Paths::from_env(&|_| None, dir);
    // The same exclusive lock the engine takes, judged at the real clock: the scenario time is for the tick only, and a
    // copy's last Rails heartbeat must not read as alive against it.
    let _lock = lease::lock(&paths, chrono::Utc::now())?;
    let o = store::open(&paths)?;
    let venue = FakeVenue::from_script(&scenario["script"]);
    let before = snapshot(&o.primary)?;
    if scenario["tick"] != false {
        let (mut at, mut attempts) = (start, Attempts::default());
        for _ in 0..MAX_ATTEMPTS {
            match tick::tick(&o.primary, &venue, bot_id, &FixedClock(at), &mut attempts).await? {
                TickOutcome::RetryAfter(d) => at += chrono::Duration::from_std(d).unwrap(),
                _ => break,
            }
        }
    }
    // The follow-up poll Rails enqueues for one order (Bot::FetchAndUpdateOrderJob), 5 s after the tick; its retries
    // are not replayed (ponytail: no grid scenario fails a poll, add the retry loop when one does).
    if let Some(ext) = scenario["poll"].as_str() {
        let tx: i64 = o.primary.query_row("SELECT id FROM transactions WHERE bot_id = ?1 AND external_id = ?2", rusqlite::params![bot_id, ext], |r| r.get(0))?;
        polling::follow_up(&o.primary, &venue, bot_id, tx, start + chrono::Duration::seconds(5)).await
            .map_err(|e| EngineError::Data(format!("follow-up poll of {ext}: {e:?}")))?;
    }
    let after = snapshot(&o.primary)?;
    Ok(json!({ "sent": venue.sent().iter().map(wire).collect::<Vec<_>>(), "changes": diff(&before, &after) }))
}
