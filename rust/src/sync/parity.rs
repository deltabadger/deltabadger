//! The Rust half of the sync row-parity harness (script/rust/sync.rb is the Rails half): the scenario's steps on a
//! marked scratch copy, each a real sync over the recorded bodies Rails' jobs read, reported in the shape Rails reports.
use super::balances::{self, ScriptedPrices, PRICES_PATH};
use crate::jobs::Db;
use super::{ledger, reading_keys, SyncError};
use crate::crypto::{Cipher, Credentials};
use crate::engine::FixedClock;
use crate::lease;
use crate::store::{self, Paths};
use crate::venue::alpaca::{AlpacaVenue, Urls};
use crate::venue::http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError};
use chrono::{DateTime, Utc};
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::Arc;

const TABLES: [&str; 5] = ["account_transactions", "account_balances", "api_keys", "bots", "bot_activity_logs"];
const JSON_COLUMNS: [&str; 5] = ["raw_data", "manual_values", "settings", "transient_data", "details"];
/// Ciphertext, and nothing a sync touches.
const SECRET_COLUMNS: [&str; 7] = ["key", "secret", "passphrase", "access_token", "rsa_signature_key", "rsa_encryption_key", "dh_param"];

pub use super::wire::exact;

fn raw(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!({ "f": format!("{:016x}", f.to_bits()) }),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(_) => json!("<blob>"),
    }
}

pub fn snapshot(c: &Connection) -> Result<Value, SyncError> {
    let mut out = Map::new();
    for table in TABLES {
        let (names, snapshot) = crate::figures::fill::parity_rows(c,table,"id")?;
        let mut rows = Map::new();
        for r in snapshot {
            let mut row = Map::new();
            for (i, name) in names.iter().enumerate() {
                if SECRET_COLUMNS.contains(&name.as_str()) { continue; }
                let mut v = raw((&r[i]).into());
                // raw_data is compared twice: as the text in the column, byte for byte, and parsed (for what reads it).
                if name == "raw_data" { row.insert("raw_data_text".into(), v.clone()); }
                if JSON_COLUMNS.contains(&name.as_str()) { if let Value::String(s) = &v { v = exact(s).unwrap_or(v); } }
                row.insert(name.clone(), v);
            }
            rows.insert(row["id"].to_string(), Value::Object(row));
        }
        out.insert(table.into(), Value::Object(rows));
    }
    Ok(Value::Object(out))
}

/// Changed and new rows, then removed ones (`after: null`), each list in id order.
pub fn diff(before: &Value, after: &Value) -> Value {
    let by_id = |rows: Vec<(i64, Value)>| { let mut rows = rows; rows.sort_by_key(|(n, _)| *n); rows.into_iter().map(|(_, v)| v).collect::<Vec<_>>() };
    let id = |id: &String| id.parse::<i64>().unwrap_or_default();
    let mut out = Map::new();
    for table in TABLES {
        let (Some(b), Some(a)) = (before[table].as_object(), after[table].as_object()) else { continue };
        let mut changes = by_id(a.iter().filter(|(k, row)| b.get(k.as_str()) != Some(*row))
            .map(|(k, row)| (id(k), json!({ "id": id(k), "before": b.get(k.as_str()).cloned().unwrap_or(Value::Null), "after": row }))).collect());
        changes.extend(by_id(b.iter().filter(|(k, _)| !a.contains_key(k.as_str())).map(|(k, row)| (id(k), json!({ "id": id(k), "before": row, "after": null }))).collect()));
        out.insert(table.into(), Value::Array(changes));
    }
    Value::Object(out)
}

/// The script, served as Alpaca serves it where a step says so (`server_filters_after`): an activities page holds
/// only what is later than the request's `after`. The Rails harness filters the same way (script/rust/sync.rb), so a
/// scenario can show what each side's `after` would and would not bring back.
struct Served { script: ScriptedTransport, filters_after: bool }

impl Transport for Served {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let mut response = self.script.send(r).await?;
        let after = r.query.iter().find(|(k, _)| *k == "after").and_then(|(_, v)| v.parse::<DateTime<Utc>>().ok());
        if let (true, Some(after), Ok(Value::Array(page))) = (self.filters_after && r.path == ledger::ACTIVITIES_PATH, after, serde_json::from_str::<Value>(&response.body)) {
            let time = |a: &Value| {
                let text = a["transaction_time"].as_str().or(a["date"].as_str())?;
                text.parse::<DateTime<Utc>>().ok().or_else(|| chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0).map(|t| t.and_utc()))
            };
            response.body = Value::Array(page.into_iter().filter(|a| time(a).is_none_or(|t| t > after)).collect()).to_string();
        }
        Ok(response)
    }
}

/// After every step, each bot's `restatement_generation`: a split that arrives in two syncs moves it twice.
fn generations(c: &Connection) -> Result<Vec<(i64, i64)>, SyncError> {
    let mut s = c.prepare("SELECT id, restatement_generation FROM bots ORDER BY id")?;
    let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Runs the scenario in `dir` (a scratch copy the Rails harness built) and reports what Rails' `record` reports.
pub async fn run(dir: &Path, cipher: Arc<Cipher>) -> Result<Value, SyncError> {
    let data = |what: &str| SyncError(format!("scenario.json: {what}"));
    let read = std::fs::read_to_string(dir.join("scenario.json")).map_err(|e| SyncError(e.to_string()))?;
    let scenario: Value = serde_json::from_str(&read).map_err(|e| SyncError(e.to_string()))?;
    if scenario["parity_scratch"] != true { return Err(SyncError(format!("{} is not a parity scratch copy (no parity_scratch marker)", dir.display()))); }
    let key_id = scenario["api_key_id"].as_i64().ok_or_else(|| data("api_key_id"))?;
    let text = |k: &str| scenario["credentials"][k].as_str().map(str::to_string);
    let credentials = Credentials { redaction_values:vec![], key: text("key").unwrap_or_default(), secret: text("secret").unwrap_or_default(), passphrase: text("passphrase") };
    let paths = Paths::from_env(&|_| None, dir);
    // The same exclusive lock every command takes, judged at the real clock.
    let _lock = lease::lock(&paths, Utc::now()).map_err(|e| SyncError(format!("{e:?}")))?;
    let opened = store::open(&paths).map_err(|e| SyncError(format!("{e:?}")))?;
    // The oracle fixture explicitly describes a single known account. Bind its seeded
    // incremental watermark to that producer; unknown real installations reread fully.
    let producer=crate::engine::model::credential_version_by_id(&opened.primary,key_id)?.ok_or_else(||data("credential producer"))?;
    { let tx=opened.primary.unchecked_transaction()?; super::cache::record_ledger(&crate::engine::model::check_credential_result(&tx,&Some(producer.clone()))?,key_id,&producer,Utc::now())?; tx.commit()?; }
    let before = snapshot(&opened.primary)?;
    let reading = reading_keys(&opened.primary)?;
    let db = Db::new(opened.primary, (*cipher).clone());

    let mut steps = vec![];
    for step in scenario["steps"].as_array().ok_or_else(|| data("steps"))? {
        let at: DateTime<Utc> = step["at"].as_str().and_then(|s| s.parse().ok()).ok_or_else(|| data("step.at"))?;
        let transport = ScriptedTransport::from_script(&step["alpaca"]);
        let served = Served { script: transport.clone(), filters_after: step["server_filters_after"] == true };
        let venue = AlpacaVenue::new(served, Urls::for_passphrase(credentials.passphrase.as_deref()));
        let prices = ScriptedPrices::from_script(&step["market"]);
        let raised = match step["kind"].as_str() {
            Some("ledger") => ledger::sync(&db, &venue, key_id, &credentials, &FixedClock(at)).await?.is_err_and(|f| f.raised),
            Some("balances") => balances::sync(&db, &venue, &prices, key_id, &credentials, &FixedClock(at)).await?.is_err_and(|f| f.raised),
            other => return Err(SyncError(format!("unknown step kind {other:?}"))),
        };
        // As Rails' harness logs them: "METHOD path" and the query pairs, sorted.
        let mut requests: Vec<Value> = transport.requests().iter().map(|r| {
            let mut query: Vec<(String, String)> = r.query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            query.sort();
            json!([format!("{} {}", r.method, r.path), query])
        }).collect();
        requests.extend(prices.requests().iter().map(|ids| json!([format!("GET {PRICES_PATH}"), [["coin_ids", ids.join(",")], ["vs_currencies", "usd"]]])));
        let generations = db.run(|c, _| generations(c).map_err(|e| e.0)).await.map_err(SyncError)?;
        steps.push(json!({ "requests": requests, "raised": raised, "generations": generations }));
    }
    let after = db.run(|c, _| snapshot(c).map_err(|e| e.0)).await.map_err(SyncError)?;
    // What Rails reads as a split row afterwards (Bot::Restatable#split_row?): its JSON type gives nil for a raw_data
    // its parser refuses, so a row Rust stored counts only if Rails can read it.
    let splits_read = db.run(|c, _| {
        let mut s = c.prepare("SELECT raw_data FROM account_transactions WHERE entry_type = 15 ORDER BY id").map_err(|e| e.to_string())?;
        let rows = s.query_map([], |r| r.get::<_, Option<String>>(0)).and_then(|rows| rows.collect::<Result<Vec<_>, _>>()).map_err(|e| e.to_string())?;
        Ok(rows.iter().flatten().filter(|raw| matches!(super::activities::Raw::parse(raw), Some(Ok(r)) if r.value["corporate_action"] == "split")).count())
    }).await.map_err(SyncError)?;
    Ok(json!({ "reading": reading, "steps": steps, "splits_read": splits_read, "changes": diff(&before, &after) }))
}
