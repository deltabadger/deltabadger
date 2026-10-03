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

// The tick rewrites a basket's members, so they are compared too.
const TABLES: [&str; 4] = ["bots", "transactions", "bot_activity_logs", "bot_index_assets"];
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
                if name == "transient_data" { if let Value::Object(m) = &mut v { m.remove("failure_notifications"); m.remove("rust_placement"); m.remove("rust_defer_until"); m.remove("rust_amount_limit_stops_pending"); } }
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
        let posted = || transport.posted_orders().len();
        let venue = AlpacaVenue::new(transport.clone(), Urls::for_passphrase(Some("paper")));
        let (poll, recover_sent) = play(&o, &venue, &scenario, bot_id, start, &posted).await?;
        // A raised follow-up is reported like Rails' job raise; a throttle or transient failure is a retry Rails enqueues.
        let poll_error = match poll { Some(polling::PollFailure::General(m)) => json!(m), _ => Value::Null };
        let funds_notified: bool = o.primary.query_row("SELECT last_end_of_funds_notification IS NOT NULL FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))?;
        let mut out = json!({ "sent": transport.posted_orders(), "changes": diff(&before, &snapshot(&o.primary)?),
                              "funds_notified": funds_notified, "poll_error": poll_error });
        // The engine's reconciliation tick has no Rails counterpart: what it sent (nothing, expected) goes to the asserters only.
        if let Some(n) = recover_sent { out["recover_sent"] = json!(n); }
        return Ok(out);
    }
    let venue = FakeVenue::from_script(&scenario["script"]);
    let posted = || venue.sent().len();
    if let (Some(e), _) = play(&o, &venue, &scenario, bot_id, start, &posted).await? {
        return Err(EngineError::Data(format!("follow-up poll of {}: {e:?}", scenario["poll"])));
    }
    let sent: Vec<Value> = venue.sent().iter().map(wire).collect();
    Ok(json!({ "sent": sent, "changes": diff(&before, &snapshot(&o.primary)?) }))
}

/// The scenario's phases, in Rails' harness order: the tick at `at` (with Rails' retries); the follow-up poll Rails enqueues for
/// one order, 5 s after it; the SQL a scenario runs `between` the ticks on both sides; the engine's own reconciliation tick at
/// `recover_at` (Rust only: Rails has no job then, its next run is at next_interval_checkpoint_at); the next checkpoint's tick
/// at `next_at` (both). Returns how the poll failed and, when there is a reconciliation tick, how many orders it sent.
async fn play<V: Venue>(o: &Opened, venue: &V, scenario: &Value, bot_id: i64, start: DateTime<Utc>, posted: &dyn Fn() -> usize)
    -> Result<(Option<polling::PollFailure>, Option<usize>), EngineError> {
    // One price cache across retries and phases, as Rails' 5 s cache spans its retried jobs.
    let prices = PriceCache::default();
    let cx = TickContext { prices: &prices, process_start: DateTime::<Utc>::MIN_UTC, stopping: &|| false };
    let at = |key: &str| -> Result<Option<DateTime<Utc>>, EngineError> {
        scenario[key].as_str().map(|s| s.parse::<DateTime<Utc>>().map_err(|e| EngineError::Data(format!("scenario.{key}: {e}")))).transpose()
    };
    if scenario["tick"] != false { ticks(o, venue, bot_id, start, &cx).await?; }
    // ponytail: the poll's retries are not replayed; no grid scenario fails a poll with a retryable error.
    let mut poll = None;
    if let Some(ext) = scenario["poll"].as_str() {
        let tx: i64 = o.primary.query_row("SELECT id FROM transactions WHERE bot_id = ?1 AND external_id = ?2", rusqlite::params![bot_id, ext], |r| r.get(0))?;
        poll = polling::follow_up(&o.primary, venue, bot_id, tx, start + chrono::Duration::seconds(5)).await.err();
    }
    // What changes between the first tick and the next, outside both engines; the same SQL runs on Rails' copy.
    for sql in scenario["between"].as_array().into_iter().flatten() {
        o.primary.execute_batch(sql.as_str().ok_or_else(|| EngineError::Data(format!("scenario.between: {sql}")))?)?;
    }
    let mut recover_sent = None;
    if let Some(when) = at("recover_at")? {
        let before = posted();
        tick::tick_recovering(&o.primary, venue, bot_id, &FixedClock(when), &mut Attempts::default(), &mut None, &cx).await?;
        recover_sent = Some(posted() - before);
    }
    if let Some(when) = at("next_at")? { ticks(o, venue, bot_id, when, &cx).await?; }
    Ok((poll, recover_sent))
}

/// One Bot::ActionJob run at `start` and the retries it enqueues for itself.
async fn ticks<V: Venue>(o: &Opened, venue: &V, bot_id: i64, start: DateTime<Utc>, cx: &TickContext<'_>) -> Result<(), EngineError> {
    let (mut at, mut attempts) = (start, Attempts::default());
    for _ in 0..MAX_ATTEMPTS {
        match tick::tick_recovering(&o.primary, venue, bot_id, &FixedClock(at), &mut attempts, &mut None, cx).await? {
            TickOutcome::RetryAfter(d) => at += chrono::Duration::from_std(d).unwrap(),
            _ => break,
        }
    }
    Ok(())
}

/// For every bot this engine would run on the copy at `src`: its own scenario, ticking 1 s after its next
/// checkpoint. The copy is duplicated per bot, so each engine writes only into its own.
pub fn plan_copy(src: &Path, tickers: &Value, out: &Path, now: DateTime<Utc>) -> Result<usize, EngineError> {
    use crate::engine::{basket, eligibility, model, schedule};
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
        // Every member's pair: a basket's legs each read their own price from the copy's script.
        let pairs = basket::member_pairs(&c, &bot)?;
        let recorded: Vec<Value> = pairs.iter()
            .map(|p| tickers.get(p).cloned().ok_or_else(|| EngineError::Data(format!("no recorded Ticker body for {p}"))))
            .collect::<Result<_, _>>()?;
        let alpaca = model::exchange_type(&c, &bot)? == "Exchanges::Alpaca";
        let script = if alpaca { alpaca_copy_script(&c, &bot, &recorded, *id)? } else { json!({ "http": {
            "/0/public/Ticker": [recorded.first().cloned().unwrap_or(Value::Null)],
            "/0/private/AddOrder": (1..=placements(1)).map(|n| json!({ "error": [], "result": { "txid": [format!("OPARITY-{id}-{n}")] } })).collect::<Vec<_>>(),
            "/0/private/BalanceEx": [{ "error": [], "result": { "ZEUR": { "balance": "1000000000", "hold_trade": "0" }, "ZUSD": { "balance": "1000000000", "hold_trade": "0" } } }],
            "/0/private/QueryOrders": [{ "error": [], "result": {} }],
            "/0/private/TradesHistory": [{ "error": [], "result": { "trades": {}, "count": 0 } }] } }) };
        let scenario = json!({ "parity_scratch": true, "bot_id": id, "at": at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
                               "venue": if alpaca { "alpaca" } else { "kraken" }, "script": script });
        std::fs::write(dir.join("scenario.json"), serde_json::to_string_pretty(&scenario).unwrap()).map_err(|e| EngineError::Data(e.to_string()))?;
    }
    Ok(report.eligible.len())
}

/// How many distinct order ids a copy's script holds: every leg of every attempt Bot::ActionJob may make (MAX_ATTEMPTS), so
/// none repeats; the script repeats its last answer only past that.
fn placements(members: usize) -> usize { members.max(1) * MAX_ATTEMPTS }

/// An Alpaca copy's script: one quotes and one trades body naming every member's pair (as Alpaca answers `symbols=…`), an
/// accepted order, a funded account, an open clock, and every order the bot still waits on answered as resting and unfilled,
/// on its own pair. Both engines read the same bodies, so that answer is neutral.
fn alpaca_copy_script(c: &Connection, bot: &crate::engine::model::Bot, recorded: &[Value], id: i64) -> Result<Value, EngineError> {
    let ok = |body: Value| json!([{ "status": 200, "body": body }]);
    let merged = |endpoint: &str| {
        let mut all = Map::new();
        for r in recorded { if let Some(m) = r[endpoint][endpoint].as_object() { all.extend(m.clone()); } }
        json!({ endpoint: Value::Object(all) })
    };
    let mut a = json!({
        "GET /v1beta3/crypto/us/latest/quotes": ok(merged("quotes")),
        "GET /v1beta3/crypto/us/latest/trades": ok(merged("trades")),
        // One distinct order id per placement: both engines deduplicate an accepted order by its external id, so a repeated
        // id would fold a basket's legs into one row on both sides and still compare equal.
        "POST /v2/orders": Value::Array((1..=placements(recorded.len())).map(|n| json!({ "status": 200, "body": { "id": format!("OPARITY-{id}-{n}"), "status": "pending_new" } })).collect()),
        "GET /v2/account": ok(json!({ "cash": "1000000000", "buying_power": "1000000000", "non_marginable_buying_power": "1000000000" })),
        "GET /v2/positions": ok(json!([])),
        "GET /v2/clock": ok(json!({ "is_open": true, "next_open": "2026-01-05T09:30:00-05:00", "next_close": "2026-01-05T16:00:00-05:00" })),
    });
    let mut s = c.prepare("SELECT external_id, order_type, base_asset_id FROM transactions WHERE bot_id = ?1 AND exchange_id = ?2 AND status = 0 \
                           AND external_status IN (0, 1) AND external_id IS NOT NULL")?;
    let waiting = s.query_map(rusqlite::params![bot.id, bot.exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (ext, order_type, asset) in waiting {
        // Eligibility refuses a row without base_asset_id, so every waiting row names its member.
        let pair = match asset { Some(a) => crate::engine::model::ticker_for_asset(c, bot, a)?.map(|t| t.ticker).unwrap_or_default(), None => String::new() };
        let kind = if order_type == Some(1) { "limit" } else { "market" };
        a[format!("GET /v2/orders/{ext}")] = ok(json!({ "id": ext, "status": "accepted", "symbol": pair, "type": kind, "side": "buy",
            "filled_qty": "0", "filled_avg_price": null, "qty": null, "notional": null, "limit_price": null }));
    }
    Ok(json!({ "alpaca": a }))
}
