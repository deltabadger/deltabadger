//! The Rust half of the decision-parity harness (script/rust/decisions.rb is the Rails half): the same
//! ticks, with retries, on a marked scratch copy, reported in the canonical shape Rails reports.
use crate::engine::polling;
use crate::engine::tick::{self, Attempts, PriceCache, TickContext, TickOutcome};
use crate::engine::{EngineError, FixedClock};
use crate::lease;
use crate::store::{self, Paths};
use crate::store::Opened;
use crate::venue::alpaca::{AlpacaVenue, Urls};
use crate::venue::http::ScriptedTransport;
use crate::venue::Venue;
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
    let before = snapshot(&o.primary)?;
    if scenario["venue"] == "alpaca" {
        // Rust's real Alpaca client over the recorded bodies Rails' harness serves beneath Clients::Alpaca.
        let transport = ScriptedTransport::from_script(&scenario["script"]["alpaca"]);
        let poll = play(&o, &AlpacaVenue::new(transport.clone(), Urls::for_passphrase(Some("paper"))), &scenario, bot_id, start).await?;
        // A raised follow-up is reported like Rails' job raise; a throttle or transient failure is a retry Rails enqueues.
        let poll_error = match poll { Some(polling::PollFailure::General(m)) => json!(m), _ => Value::Null };
        let funds_notified: bool = o.primary.query_row("SELECT last_end_of_funds_notification IS NOT NULL FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))?;
        return Ok(json!({ "sent": transport.posted_orders(), "changes": diff(&before, &snapshot(&o.primary)?),
                          "funds_notified": funds_notified, "poll_error": poll_error }));
    }
    let venue = FakeVenue::from_script(&scenario["script"]);
    if let Some(e) = play(&o, &venue, &scenario, bot_id, start).await? {
        return Err(EngineError::Data(format!("follow-up poll of {}: {e:?}", scenario["poll"])));
    }
    let sent: Vec<Value> = venue.sent().iter().map(wire).collect();
    Ok(json!({ "sent": sent, "changes": diff(&before, &snapshot(&o.primary)?) }))
}

/// The scenario's tick (with Rails' retries) and the follow-up poll Rails enqueues for one order; returns how that poll failed.
async fn play<V: Venue>(o: &Opened, venue: &V, scenario: &Value, bot_id: i64, start: DateTime<Utc>) -> Result<Option<polling::PollFailure>, EngineError> {
    if scenario["tick"] != false {
        // One price cache across the retries, as Rails' 5 s cache spans its retried jobs (Task 8).
        let (mut at, mut attempts) = (start, Attempts::default());
        let prices = PriceCache::default();
        let cx = TickContext { prices: &prices, process_start: DateTime::<Utc>::MIN_UTC, stopping: &|| false };
        for _ in 0..MAX_ATTEMPTS {
            match tick::tick_recovering(&o.primary, venue, bot_id, &FixedClock(at), &mut attempts, &mut None, &cx).await? {
                TickOutcome::RetryAfter(d) => at += chrono::Duration::from_std(d).unwrap(),
                _ => break,
            }
        }
    }
    // The follow-up poll Rails enqueues for one order (Bot::FetchAndUpdateOrderJob), 5 s after the tick; its retries
    // are not replayed (ponytail: no grid scenario fails a poll, add the retry loop when one does).
    if let Some(ext) = scenario["poll"].as_str() {
        let tx: i64 = o.primary.query_row("SELECT id FROM transactions WHERE bot_id = ?1 AND external_id = ?2", rusqlite::params![bot_id, ext], |r| r.get(0))?;
        return Ok(polling::follow_up(&o.primary, venue, bot_id, tx, start + chrono::Duration::seconds(5)).await.err());
    }
    Ok(None)
}

/// For every bot this engine would run on the copy at `src`: its own scenario, ticking 1 s after its next
/// checkpoint. The copy is duplicated per bot, so each engine writes only into its own.
pub fn plan_copy(src: &Path, tickers: &Value, out: &Path, now: DateTime<Utc>) -> Result<usize, EngineError> {
    use crate::engine::{eligibility, model, schedule};
    store::check(&Paths::from_env(&|_| None, src))?;
    let c = Connection::open_with_flags(src.join("production.sqlite3"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let report = eligibility::check_install(&c)?;
    if !report.problems.is_empty() { return Err(EngineError::Ineligible(report.problems)); }
    for id in &report.eligible {
        let bot = model::load_bot(&c, *id)?;
        let (Some(anchor), Some(interval), Some(quote)) = (bot.started_at_us, bot.interval(), bot.quote_amount()) else { continue };
        let next = schedule::checkpoints(anchor, now.timestamp_micros(), schedule::effective(interval, quote, bot.smart_quote_amount())).next_us;
        let at = DateTime::from_timestamp_micros(next + 1_000_000).unwrap();
        let dir = out.join(format!("bot-{id}"));
        std::fs::create_dir_all(&dir).map_err(|e| EngineError::Data(e.to_string()))?;
        for f in ["production.sqlite3", "production_queue.sqlite3"] {
            // `VACUUM INTO` gives a consistent single-file copy even when the source has a -wal file.
            let target = dir.join(f);
            Connection::open_with_flags(src.join(f), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?
                .execute("VACUUM INTO ?1", [target.to_string_lossy()])?;
        }
        let pair = model::ticker_for(&c, &bot)?.map(|t| t.ticker).unwrap_or_default();
        let recorded = tickers.get(&pair).cloned().ok_or_else(|| EngineError::Data(format!("no recorded Ticker body for {pair}")))?;
        let alpaca = model::exchange_type(&c, &bot)? == "Exchanges::Alpaca";
        let script = if alpaca { alpaca_copy_script(&c, &bot, &pair, &recorded, *id)? } else { json!({ "http": {
            "/0/public/Ticker": [recorded],
            "/0/private/AddOrder": [{ "error": [], "result": { "txid": [format!("OPARITY-{id}")] } }],
            "/0/private/BalanceEx": [{ "error": [], "result": { "ZEUR": { "balance": "1000000000", "hold_trade": "0" }, "ZUSD": { "balance": "1000000000", "hold_trade": "0" } } }],
            "/0/private/QueryOrders": [{ "error": [], "result": {} }],
            "/0/private/TradesHistory": [{ "error": [], "result": { "trades": {}, "count": 0 } }] } }) };
        let scenario = json!({ "parity_scratch": true, "bot_id": id, "at": at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
                               "venue": if alpaca { "alpaca" } else { "kraken" }, "script": script });
        std::fs::write(dir.join("scenario.json"), serde_json::to_string_pretty(&scenario).unwrap()).map_err(|e| EngineError::Data(e.to_string()))?;
    }
    Ok(report.eligible.len())
}

/// An Alpaca copy's script: the recorded quote and trade bodies, an accepted order, a funded account, an open clock, and
/// every order the bot still waits on answered as resting and unfilled. Both engines read the same bodies, so that answer
/// is neutral.
fn alpaca_copy_script(c: &Connection, bot: &crate::engine::model::Bot, pair: &str, recorded: &Value, id: i64) -> Result<Value, EngineError> {
    let ok = |body: Value| json!([{ "status": 200, "body": body }]);
    let mut a = json!({
        "GET /v1beta3/crypto/us/latest/quotes": ok(recorded["quotes"].clone()),
        "GET /v1beta3/crypto/us/latest/trades": ok(recorded["trades"].clone()),
        "POST /v2/orders": ok(json!({ "id": format!("OPARITY-{id}"), "status": "pending_new" })),
        "GET /v2/account": ok(json!({ "cash": "1000000000", "buying_power": "1000000000", "non_marginable_buying_power": "1000000000" })),
        "GET /v2/positions": ok(json!([])),
        "GET /v2/clock": ok(json!({ "is_open": true, "next_open": "2026-01-05T09:30:00-05:00", "next_close": "2026-01-05T16:00:00-05:00" })),
    });
    let mut s = c.prepare("SELECT external_id, order_type FROM transactions WHERE bot_id = ?1 AND exchange_id = ?2 AND status = 0 \
                           AND external_status IN (0, 1) AND external_id IS NOT NULL")?;
    let waiting = s.query_map(rusqlite::params![bot.id, bot.exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (ext, order_type) in waiting {
        let kind = if order_type == Some(1) { "limit" } else { "market" };
        a[format!("GET /v2/orders/{ext}")] = ok(json!({ "id": ext, "status": "accepted", "symbol": pair, "type": kind, "side": "buy",
            "filled_qty": "0", "filled_avg_price": null, "qty": null, "notional": null, "limit_price": null }));
    }
    Ok(json!({ "alpaca": a }))
}
