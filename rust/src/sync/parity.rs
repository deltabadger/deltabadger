//! The Rust half of the sync row-parity harness (script/rust/sync.rb is the Rails half): the scenario's steps on a
//! marked scratch copy, each a real sync over the recorded bodies Rails' jobs read, reported in the shape Rails reports.
use super::balances::{self, ScriptedPrices, EXCHANGE_RATES_PATH, PRICES_PATH};
use crate::jobs::Db;
use super::{balance_reading_keys, ledger, SyncError};
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

/// A Binance account served from a step's `binance_state`, as script/rust/sync.rb's FakeBinance serves it (see there):
/// the histories by window and page, the trade list by `fromId`, by `startTime`, or the most recent; failures by path
/// (or path and symbol).
#[derive(Clone)]
struct FakeBinance { state: Arc<Value>, requests: std::rc::Rc<std::cell::RefCell<Vec<HttpRequest>>> }

impl FakeBinance {
    fn body(&self, r: &HttpRequest) -> Result<Value, String> {
        let q = |k: &str| r.query.iter().find(|(name, _)| *name == k).map(|(_, v)| v.clone());
        let int = |k: &str| q(k).and_then(|v| v.parse::<i64>().ok());
        let list = |k: &str| self.state[k].as_array().cloned().unwrap_or_default();
        let window = |rows: Vec<Value>, time: &dyn Fn(&Value) -> Option<i64>| -> Value {
            let (from, to) = (int("startTime"), int("endTime"));
            let (offset, limit) = (int("offset").unwrap_or(0) as usize, int("limit").unwrap_or(1000) as usize);
            Value::Array(rows.into_iter().filter(|r| time(r).is_some_and(|t| from.is_none_or(|f| t >= f) && to.is_none_or(|e| t <= e))).skip(offset).take(limit).collect())
        };
        Ok(match r.path.as_str() {
            "/api/v3/account" => json!({ "accountType": "SPOT", "balances": list("balances").iter().map(|b| json!({ "asset": b[0], "free": b[1], "locked": b.get(2).cloned().unwrap_or(json!("0.00000000")) })).collect::<Vec<_>>() }),
            "/api/v3/exchangeInfo" => json!({ "timezone": "UTC", "symbols": list("symbols").iter().map(|s| json!({ "symbol": s[0], "status": s.get(3).cloned().unwrap_or(json!("TRADING")), "baseAsset": s[1], "quoteAsset": s[2] })).collect::<Vec<_>>() }),
            "/sapi/v1/capital/deposit/hisrec" => window(list("deposits"), &|d| d["insertTime"].as_i64()),
            "/sapi/v1/capital/withdraw/history" => window(list("withdrawals"), &|w| w["applyTime"].as_str().and_then(super::binance_ledger::apply_time).map(|t| t.timestamp_millis())),
            "/api/v3/myTrades" => {
                let mut trades = self.state["trades"][q("symbol").unwrap_or_default()].as_array().cloned().unwrap_or_default();
                trades.sort_by_key(|t| t["id"].as_i64());
                let limit = int("limit").unwrap_or(500) as usize;
                Value::Array(match (int("fromId"), int("startTime")) {
                    (Some(from), _) => trades.into_iter().filter(|t| t["id"].as_i64().is_some_and(|id| id >= from)).take(limit).collect(),
                    (None, Some(from)) => trades.into_iter().filter(|t| t["time"].as_i64().is_some_and(|at| at >= from)).take(limit).collect(),
                    (None, None) => { let skip = trades.len().saturating_sub(limit); trades.into_iter().skip(skip).collect() }
                })
            }
            // Rails' FakeBinance serves these from the state's lists, each row by its own time.
            "/sapi/v1/convert/tradeFlow" => json!({ "list": window(list("converts"), &|c| c["createTime"].as_i64()), "moreData": false }),
            "/sapi/v1/fiat/payments" => {
                let key = if q("transactionType").as_deref() == Some("1") { "fiat_sells" } else { "fiat_buys" };
                let (from, to) = (int("beginTime"), int("endTime"));
                let rows: Vec<Value> = list(key).into_iter().filter(|p| p["createTime"].as_i64().is_some_and(|t| from.is_none_or(|f| t >= f) && to.is_none_or(|e| t <= e))).collect();
                let (page, size) = (int("page").unwrap_or(1).max(1) as usize, int("rows").unwrap_or(100) as usize);
                json!({ "code": "000000", "message": "success", "total": rows.len(), "success": true, "data": rows.iter().skip((page - 1) * size).take(size).cloned().collect::<Vec<_>>() })
            }
            "/sapi/v1/asset/dribblet" => { let rows = window(list("dust"), &|d| d["operateTime"].as_i64()); json!({ "total": rows.as_array().map_or(0, Vec::len), "userAssetDribblets": rows }) }
            "/sapi/v1/asset/assetDividend" => { let rows = window(list("dividends"), &|d| d["divTime"].as_i64()); json!({ "rows": rows, "total": rows.as_array().map_or(0, Vec::len) }) }
            "/sapi/v1/margin/interestHistory" | "/sapi/v1/margin/forceLiquidationRec" => json!({ "rows": [], "total": 0 }),
            "/fapi/v1/income" | "/dapi/v1/income" => json!([]),
            path if path.starts_with("/sapi/v1/simple-earn/") => {
                let key = path.trim_start_matches("/sapi/v1/simple-earn/").replace("/history/", "_");
                let (from, to) = (int("startTime"), int("endTime"));
                let time = |r: &Value| ["time", "deliverDate", "createTime"].iter().find_map(|k| r[*k].as_i64());
                let kind = q("type").filter(|k| k != "ALL");
                let rows: Vec<Value> = self.state["earn"][&key].as_array().cloned().unwrap_or_default().into_iter()
                    .filter(|r| time(r).is_some_and(|t| from.is_none_or(|f| t >= f) && to.is_none_or(|e| t <= e)))
                    .filter(|r| kind.as_deref().is_none_or(|k| r["type"] == k)).collect();
                let (current, size) = (int("current").unwrap_or(1).max(1) as usize, int("size").unwrap_or(10) as usize);
                json!({ "rows": rows.iter().skip((current - 1) * size).take(size).cloned().collect::<Vec<_>>(), "total": rows.len() })
            }
            other => return Err(format!("fake Binance has no {other}")),
        })
    }
}

/// What Binance refuses before reading anything, as script/rust/sync.rb's FakeBinance refuses it: a missing mandatory
/// parameter (its docs, and `type` on flexible rewards, which Binance requires though its docs say otherwise), a
/// signed path without its timestamp, and a window longer than the endpoint accepts.
pub fn binance_refusal(path: &str, query: &[(String, String)]) -> Option<String> {
    let has = |k: &str| query.iter().any(|(name, v)| name == k && !v.is_empty());
    let int = |k: &str| query.iter().find(|(name, _)| name == k).and_then(|(_, v)| v.parse::<i64>().ok());
    let mandatory: &[&str] = match path {
        "/api/v3/exchangeInfo" => &[],
        "/api/v3/myTrades" => &["symbol", "timestamp"],
        "/sapi/v1/convert/tradeFlow" => &["startTime", "endTime", "timestamp"],
        "/sapi/v1/fiat/payments" => &["transactionType", "timestamp"],
        "/sapi/v1/simple-earn/flexible/history/rewardsRecord" => &["type", "timestamp"],
        _ => &["timestamp"],
    };
    if let Some(missing) = mandatory.iter().find(|k| !has(k)) {
        return Some(format!(r#"{{"code":-1102,"msg":"Mandatory parameter '{missing}' was not sent, was empty/null, or malformed."}}"#));
    }
    let days: Option<i64> = match path {
        "/sapi/v1/capital/deposit/hisrec" | "/sapi/v1/capital/withdraw/history" => Some(90),
        "/sapi/v1/convert/tradeFlow" => Some(30),
        "/sapi/v1/asset/assetDividend" => Some(180),
        p if p.starts_with("/sapi/v1/simple-earn/") => Some(30),
        "/api/v3/myTrades" => Some(1),
        _ => None,
    };
    match (days, int("startTime"), int("endTime")) {
        (Some(days), Some(from), Some(to)) if to - from > days * 86_400_000 =>
            Some(format!(r#"{{"code":-1127,"msg":"More than {} hours between startTime and endTime."}}"#, days * 24)),
        _ => None,
    }
}

impl Transport for FakeBinance {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.borrow_mut().push(r.clone());
        let query: Vec<(String, String)> = r.query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        if let Some(body) = binance_refusal(&r.path, &query) { return Ok(HttpResponse { status: 400, body }); }
        let symbol = r.query.iter().find(|(k, _)| *k == "symbol").map(|(_, v)| v.as_str()).unwrap_or_default();
        let fails = &self.state["fail"];
        if let Some(reply) = fails.get(format!("{}?symbol={symbol}", r.path)).or_else(|| fails.get(&r.path)) {
            if let Some(message) = reply["network"].as_str().map(|_| reply["message"].as_str().unwrap_or_default().to_string()) { return Err(TransportError::NotSent(message)); }
            let body = match &reply["body"] { Value::String(s) => s.clone(), other => other.to_string() };
            return Ok(HttpResponse { status: reply["status"].as_u64().unwrap_or(200) as u16, body });
        }
        let body = self.body(r).unwrap_or_else(|e| panic!("{e}"));
        Ok(HttpResponse { status: 200, body: body.to_string() })
    }
}

/// After every step, each bot's `restatement_generation`: a split that arrives in two syncs moves it twice.
fn generations(c: &Connection) -> Result<Vec<(i64, i64)>, SyncError> {
    let mut s = c.prepare("SELECT id, restatement_generation FROM bots ORDER BY id")?;
    let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// A step of kind `import`, as script/rust/sync.rb's `run_import` reports it: the controller's zone step, the key the
/// step's `answer` names (the user's first key on that exchange type), then the run.
fn import_step(c: &Connection, user_id: i64, step: &Value, at: DateTime<Utc>) -> Result<Value, SyncError> {
    use crate::tracker::import::run::{self, Answer};
    let (format, bytes) = (step["format"].as_str().unwrap_or_default(), step["text"].as_str().unwrap_or_default().as_bytes());
    let none = json!({ "imported": 0, "duplicates": 0, "skipped": 0, "unrecognised": [], "unrouted": [], "error": null, "ambiguous": null });
    let offset = match run::offset(format, step["filename"].as_str(), bytes) {
        Some(None) => { let mut out = none; out["error"] = json!("unknown_timezone"); return Ok(out); }
        Some(Some(offset)) => Some(offset),
        None => None,
    };
    let answered: Option<i64> = match step["answer"].as_str() {
        Some(kind) => rusqlite::OptionalExtension::optional(c.query_row("SELECT k.id FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id WHERE k.user_id = ?1 AND e.type = ?2 ORDER BY k.id LIMIT 1",
                                                                       rusqlite::params![user_id, kind], |r| r.get(0)))?,
        None => None,
    };
    let mut out = none;
    match run::import(c, user_id, format, bytes, offset.as_deref(), answered.map(|id| id.to_string()).as_deref(), at)? {
        Answer::Error(error) => out["error"] = json!(error),
        Answer::Ambiguous(keys) => out["ambiguous"] = json!(keys.iter().map(|k| k.id).collect::<Vec<_>>()),
        Answer::Written { imported, duplicates, skipped, unrecognised, unrouted } =>
            out = json!({ "imported": imported, "duplicates": duplicates, "skipped": skipped, "unrecognised": unrecognised, "unrouted": unrouted, "error": null, "ambiguous": null }),
    }
    Ok(out)
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
    let reading = balance_reading_keys(&opened.primary)?;
    let db = Db::new(opened.primary, (*cipher).clone());

    let mut steps = vec![];
    let rate_cache = Arc::new(crate::jobs::data_api::RateCache::default()); // Rails' cache store, cleared per scenario
    for step in scenario["steps"].as_array().ok_or_else(|| data("steps"))? {
        let at: DateTime<Utc> = step["at"].as_str().and_then(|s| s.parse().ok()).ok_or_else(|| data("step.at"))?;
        let binance = step.get("binance").is_some();
        let kucoin = step.get("kucoin").is_some();
        let fake = step.get("binance_state").map(|state| FakeBinance { state: Arc::new(state.clone()), requests: Default::default() });
        let transport = ScriptedTransport::from_script(if binance { &step["binance"] } else if kucoin { &step["kucoin"] } else { &step["alpaca"] });
        let served = Served { script: transport.clone(), filters_after: step["server_filters_after"] == true };
        let venue = AlpacaVenue::new(served, Urls::for_passphrase(credentials.passphrase.as_deref()));
        let prices = ScriptedPrices::from_script(&step["market"]).with_rate_cache(rate_cache.clone());
        let mut imported = None;
        let raised = match (step["kind"].as_str(), &fake) {
            (Some("import"), _) => {
                let (user_id, step) = (scenario["user_id"].as_i64().ok_or_else(|| data("user_id"))?, step.clone());
                imported = Some(db.run(move |c, _| import_step(c, user_id, &step, at).map_err(|e| e.0)).await.map_err(SyncError)?);
                false
            }
            (Some("ledger"), None) if kucoin => super::kucoin_ledger::sync(&db, transport.clone(), key_id, &credentials, &FixedClock(at), std::time::Duration::ZERO).await?.is_err_and(|f| f.raised),
            (Some("ledger"), Some(fake)) => super::binance_ledger::sync(&db, fake.clone(), key_id, &credentials, &FixedClock(at), super::binance_ledger::Pacing::NONE).await?.is_err_and(|f| f.raised),
            (kind, _) => match kind {
            Some("ledger") => ledger::sync(&db, &venue, key_id, &credentials, &FixedClock(at)).await?.is_err_and(|f| f.raised),
            Some("balances") if kucoin => balances::sync_kucoin(&db, transport.clone(), &prices, key_id, &credentials, &FixedClock(at)).await?.is_err_and(|f| f.raised),
            Some("balances") if binance => balances::sync_binance(&db, transport.clone(), &prices, key_id, &credentials, &FixedClock(at)).await?.is_err_and(|f| f.raised),
            Some("balances") => balances::sync(&db, &venue, &prices, key_id, &credentials, &FixedClock(at)).await?.is_err_and(|f| f.raised),
            other => return Err(SyncError(format!("unknown step kind {other:?}"))),
        } };
        // As Rails' harness logs them: "METHOD path" and the query pairs, sorted.
        let sent = match &fake { Some(fake) => fake.requests.borrow().clone(), None => transport.requests() };
        let mut requests: Vec<Value> = sent.iter().map(|r| {
            let mut query: Vec<(String, String)> = r.query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            query.sort();
            json!([format!("{} {}", r.method, r.path), query])
        }).collect();
        requests.extend((0..prices.rate_requests()).map(|_| json!([format!("GET {EXCHANGE_RATES_PATH}"), []])));
        requests.extend(prices.requests().iter().map(|ids| json!([format!("GET {PRICES_PATH}"), [["coin_ids", ids.join(",")], ["vs_currencies", "usd"]]])));
        let generations = db.run(|c, _| generations(c).map_err(|e| e.0)).await.map_err(SyncError)?;
        let mut out = json!({ "requests": requests, "raised": raised, "generations": generations });
        if let Some(imported) = imported { out["import"] = imported; }
        steps.push(out);
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
