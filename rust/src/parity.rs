//! The Rust half of the decision-parity harness (script/rust/decisions.rb is the Rails half): the same
//! ticks, with retries, on a marked scratch copy, reported in the canonical shape Rails reports.
use crate::engine::notice::{self, Notice, Pending};
use crate::engine::{model, polling};
use crate::engine::tick::{self, Attempts, PriceCache, TickContext, TickOutcome};
use crate::engine::{EngineError, FixedClock};
use crate::crypto::{Cipher, EncryptionKeys};
use crate::jobs::{self, data_api::{self, DataApi}, reference, Outcome};
use crate::lease;
use crate::store::{self, Paths};
use crate::store::Opened;
use crate::venue::alpaca::{AlpacaVenue, Urls};
use crate::venue::http::{request_key, ScriptedTransport};
use crate::venue::Venue;
use crate::venue::kraken::KrakenVenue;
use chrono::{DateTime, Utc};
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::path::Path;

// The tick rewrites a basket's members, so they are compared too.
const TABLES: [&str; 4] = ["bots", "transactions", "bot_activity_logs", "bot_index_assets"];
const JSON_COLUMNS: [&str; 4] = ["settings", "transient_data", "details", "error_messages"];
const MAX_ATTEMPTS: usize = 6;
/// transient_data keys only this engine writes, besides the mail markers (notice::KEYS): Rails has none of them.
const RUST_KEYS: [&str; 10] = ["run_generation", "tick_generation", "rust_placement", crate::engine::model::SIGNAL_CALLS_KEY, "rust_defer_until", "rust_amount_limit_stops_pending", "rust_failure_origin",
                               "rust_rebalance_halted", crate::engine::conditions::DEAD, crate::engine::index::RESYNC_KEY];

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
        let (names, snapshot) = crate::figures::fill::parity_rows(c,table,"id")?;
        let mut rows = Map::new();
        for r in snapshot {
            let mut row = Map::new();
            for (i, name) in names.iter().enumerate() {
                // Rust-only ownership attribution; placement/tick_basket assert the exact
                // sending identity independently. Rails leaves this shared column NULL.
                if table == "transactions" && name == "run_identity" { continue; }
                let mut v = raw((&r[i]).into());
                if JSON_COLUMNS.contains(&name.as_str()) { if let Value::String(s) = &v { v = serde_json::from_str(s).unwrap_or(v); } }
                // What only this engine writes: its placement intent, its waits and counted stops, and the mails it owes.
                if name == "transient_data" { if let Value::Object(m) = &mut v { for key in RUST_KEYS.iter().chain(&notice::KEYS) { m.remove(*key); } } }
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

/// The Kraken script as the real adapter's transport is answered: each recorded body (the Rails harness serves it beneath
/// Honeymaker::Clients::Kraken#get_public / #post_private) as an HTTP 200 for "GET /0/public/…" or "POST /0/private/…".
pub fn kraken_script(script: &Value) -> Value {
    let mut out = Map::new();
    for (path, bodies) in script["http"].as_object().into_iter().flatten() {
        let method = if path.starts_with("/0/public/") { "GET" } else { "POST" };
        let replies = bodies.as_array().into_iter().flatten().map(|b| json!({ "status": 200, "body": b })).collect();
        out.insert(format!("{method} {path}"), Value::Array(replies));
    }
    Value::Object(out)
}

/// The AddOrder fields the adapter posted, as the Rails harness records them (nonce dropped there; the transport adds it
/// here), without Rust's own cl_ord_id and deadline.
fn kraken_sent(t: &ScriptedTransport) -> Vec<Value> {
    t.requests().into_iter().filter(|r| r.method == "POST" && r.path == "/0/private/AddOrder").filter_map(|r| r.body)
        .map(|mut b| { if let Value::Object(m) = &mut b { m.shift_remove("cl_ord_id"); m.shift_remove("deadline"); } b }).collect()
}

/// The orders the Binance adapter POSTed, as the Rails harness records them: the query without what changes per send
/// (timestamp, signature) and without Rust's own newClientOrderId.
fn binance_sent(t: &ScriptedTransport) -> Vec<Value> {
    t.requests().into_iter().filter(|r| r.method == "POST" && r.path == "/api/v3/order")
        .map(|r| Value::Object(r.query.into_iter().filter(|(k, _)| !matches!(*k, "timestamp" | "signature" | "newClientOrderId"))
                                .map(|(k, v)| (k.to_string(), Value::String(v))).collect())).collect()
}

/// The orders the KuCoin adapter POSTed, as the Rails harness records them: the JSON body without the clientOid.
fn kucoin_sent(t: &ScriptedTransport) -> Vec<Value> {
    t.requests().into_iter().filter(|r| r.method == "POST" && r.path == "/api/v1/orders").filter_map(|r| r.body)
        .map(|mut b| { if let Value::Object(m) = &mut b { m.shift_remove("clientOid"); } b }).collect()
}

/// The orders the Gemini adapter POSTed, as the Rails harness records them: the signed payload's params (the request's body
/// here; the transport adds `request` and `nonce`) without Rust's own client_order_id.
fn gemini_sent(t: &ScriptedTransport) -> Vec<Value> {
    t.requests().into_iter().filter(|r| r.method == "POST" && r.path == crate::venue::gemini::ORDER_NEW).filter_map(|r| r.body)
        .map(|mut b| { if let Value::Object(m) = &mut b { m.shift_remove("client_order_id"); } b }).collect()
}

/// The orders the Bitvavo adapter POSTed, as the Rails harness records them: the JSON body without the clientOrderId.
/// The orders the Bitget adapter POSTed, as the Rails harness records them: the JSON body without the clientOid.
fn bitget_sent(t: &ScriptedTransport) -> Vec<Value> {
    t.requests().into_iter().filter(|r| r.method == "POST" && r.path == crate::venue::bitget::PLACE_PATH).filter_map(|r| r.body)
        .map(|mut b| { if let Value::Object(m) = &mut b { m.shift_remove("clientOid"); } b }).collect()
}

fn bitvavo_sent(t: &ScriptedTransport) -> Vec<Value> {
    t.requests().into_iter().filter(|r| r.method == "POST" && r.path == crate::venue::bitvavo::ORDER_PATH).filter_map(|r| r.body)
        .map(|mut b| { if let Value::Object(m) = &mut b { m.shift_remove("clientOrderId"); } b }).collect()
}

/// The mails this engine owes since `before`, in the shape Rails' harness reports the mails it enqueued: the mailer action,
/// the error sentence Bot::ActionJob.humanized_errors hands it (in the user's language), the quote asset's symbol.
fn mails(c: &Connection, before: &[Pending]) -> Result<Value, EngineError> {
    let mut out = vec![];
    for p in notice::all_pending(c)?.into_iter().filter(|p| !before.contains(p)) {
        let (exchange_type, exchange_name, locale): (String, String, Option<String>) = c.query_row(
            "SELECT e.type, e.name, u.locale FROM bots b JOIN exchanges e ON e.id = b.exchange_id JOIN users u ON u.id = b.user_id WHERE b.id = ?1",
            [p.bot_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut mail = json!({ "mail": p.notice.mail() });
        match &p.notice {
            Notice::EndOfFunds { quote_asset_id } => mail["quote"] = c.query_row("SELECT symbol FROM assets WHERE id = ?1", [quote_asset_id], |r| r.get::<_, String>(0))?.into(),
            Notice::Error { error, .. } | Notice::StoppedByError { error } => {
                mail["errors"] = json!([crate::mail::render::humanize_error(&exchange_type, &exchange_name, crate::mail::render::user_locale(locale.as_deref()), error)]);
            }
            // Bot::Notifyable#notify_stopped_by_amount_limit hands the mailer the bot's quote symbol.
            Notice::StoppedByAmountLimit => mail["quote"] = c.query_row(
                "SELECT a.symbol FROM bots b JOIN assets a ON a.id = json_extract(b.settings, '$.quote_asset_id') WHERE b.id = ?1",
                [p.bot_id], |r| r.get::<_, String>(0))?.into(),
        }
        out.push(mail);
    }
    out.sort_by_key(|m| m["mail"].as_str().unwrap_or_default().to_string());
    Ok(Value::Array(out))
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
    if let (Some(primary),Some(salt))=(scenario["encryption"]["primary_key"].as_str(),scenario["encryption"]["key_derivation_salt"].as_str()) {
        let cipher=Cipher::new(&EncryptionKeys{primary_key:primary.into(),key_derivation_salt:salt.into()});
        crate::engine::provider::bind(&o.primary,&cipher,&|_|None)?;
    }
    let index_bot = model::load_bot(&o.primary, bot_id)?.bot_type == "Bots::DcaIndex";
    if index_bot && scenario["venue"] != "alpaca" {
        // A crypto index bot's reference data, as fresh as the scenario's Rails install (engine::staleness).
        if let Some(hash)=crate::engine::provider::fingerprint(&o.primary)? {crate::engine::provider::record(&o.primary,&hash,start)?;}
        for job in [reference::INDICES,reference::ASSETS] { jobs::state::record_success(&o.primary,job,None,start).map_err(EngineError::Data)?; }
    }
    if scenario["script"]["alpaca"].as_object().is_some_and(|m| m.keys().any(|k| k.contains("/v2/stocks/")) || index_bot) {
        if let Some(hash)=crate::engine::provider::fingerprint(&o.primary)? {crate::engine::provider::record(&o.primary,&hash,start)?;}
        for job in [reference::STOCKS,reference::INDICES,reference::ASSETS] { jobs::state::record_success(&o.primary,job,None,start).map_err(EngineError::Data)?; }
        let mut stmt=o.primary.prepare("SELECT id FROM api_keys WHERE key_type=0")?;
        let ids=stmt.query_map([],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
        for id in ids {
            let version=model::credential_version_by_id(&o.primary,id)?.ok_or_else(||EngineError::Data("parity credential missing".into()))?;
            model::credential_write(&o.primary,&Some(version.clone()),|tx| {
                crate::sync::cache::record_ledger(tx,id,&version,start)?;
                jobs::state::record_success(tx,"ledger_sync",Some(&id.to_string()),start).map_err(EngineError::Data)
            })?;
        }
    }
    // The recorder explicitly seeded this prior failure with the scratch account's key.
    // Unknown live rows receive no inferred producer and are stale.
    if scenario["parity_scratch"] == true {
        let bot=model::load_bot(&o.primary,bot_id)?;
        if bot.last_failure_kind().is_some() {
            let producer=model::credential_version(&o.primary,&bot)?;
            model::credential_write(&o.primary,&producer,|c|model::record_failure_origin(c,bot_id))?;
        }
    }
    let before = snapshot(&o.primary)?;
    let owed = notice::all_pending(&o.primary)?;
    if scenario["venue"] == "alpaca" {
        // Rust's real Alpaca client over the recorded bodies Rails' harness serves beneath Clients::Alpaca.
        let transport = ScriptedTransport::from_script(&scenario["script"]["alpaca"]);
        let posted = || transport.posted_orders().len();
        let venue = AlpacaVenue::new(transport.clone(), Urls::for_passphrase(Some("paper")));
        let (poll, recover_sent, checks) = play(&o, &venue, &scenario, bot_id, start, &posted).await?;
        // A raised follow-up is reported like Rails' job raise; a throttle or transient failure is a retry Rails enqueues.
        let poll_error = match poll { Some(polling::PollFailure::General(m)) => json!(m), _ => Value::Null };
        let funds_notified: bool = o.primary.query_row("SELECT last_end_of_funds_notification IS NOT NULL FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))?;
        let mut out = json!({ "sent": transport.posted_orders(), "changes": diff(&before, &snapshot(&o.primary)?), "mails": mails(&o.primary, &owed)?,
                              "funds_notified": funds_notified, "poll_error": poll_error });
        // The engine's reconciliation tick has no Rails counterpart: what it sent (nothing, expected) goes to the asserters only.
        if let Some(n) = recover_sent { out["recover_sent"] = json!(n); }
        if let Some(c) = checks { out["checks"] = json!(c); }
        return Ok(out);
    }
    if let Some(row) = scenario["venue"].as_str().filter(|v| ["binance", "binance_us"].contains(v)).and_then(|v| crate::venue::registry::ROWS.iter().find(|r| r.name_id == v)) {
        // Rust's real Binance adapter (Binance.US: configured with its host and rules) over the bodies Rails' harness serves
        // beneath Honeymaker::Clients::Binance (::BinanceUs), with the scenario's own tickers.
        let transport = ScriptedTransport::from_script(&scenario["script"][row.name_id]);
        let exchange_id: i64 = o.primary.query_row("SELECT id FROM exchanges WHERE type = ?1", [row.exchange_type], |r| r.get(0))?;
        let pairs = crate::venue::binance::Pairs::load(&o.primary, exchange_id).map_err(EngineError::Data)?;
        let rules = row.rules.ok_or_else(|| EngineError::Data(format!("{} does not trade", row.name)))?;
        let venue = crate::venue::binance::BinanceVenue::new(transport.clone(), row.base, "s").with_pairs(pairs).with_rules(rules);
        let posted = || binance_sent(&transport).len();
        let (poll, _, checks) = play(&o, &venue, &scenario, bot_id, start, &posted).await?;
        let poll_error = match poll { Some(polling::PollFailure::General(m)) => json!(m), _ => Value::Null };
        let funds_notified: bool = o.primary.query_row("SELECT last_end_of_funds_notification IS NOT NULL FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))?;
        let mut out = json!({ "sent": binance_sent(&transport), "changes": diff(&before, &snapshot(&o.primary)?), "mails": mails(&o.primary, &owed)?,
                          "funds_notified": funds_notified, "poll_error": poll_error });
        if let Some(c) = checks { out["checks"] = json!(c); }
        return Ok(out);
    }
    if scenario["venue"] == "kucoin" {
        // Rust's KuCoin adapter over the bodies Rails' harness serves beneath Honeymaker::Clients::Kucoin, with the
        // scenario's own tickers.
        let transport = ScriptedTransport::from_script(&scenario["script"]["kucoin"]);
        let exchange_id: i64 = o.primary.query_row("SELECT id FROM exchanges WHERE type = 'Exchanges::Kucoin'", [], |r| r.get(0))?;
        let pairs = crate::venue::binance::Pairs::load(&o.primary, exchange_id).map_err(EngineError::Data)?;
        let venue = crate::venue::kucoin::KucoinVenue::new(transport.clone(), crate::venue::kucoin::URL).with_pairs(pairs);
        let posted = || kucoin_sent(&transport).len();
        let (poll, _, checks) = play(&o, &venue, &scenario, bot_id, start, &posted).await?;
        let poll_error = match poll { Some(polling::PollFailure::General(m)) => json!(m), _ => Value::Null };
        let funds_notified: bool = o.primary.query_row("SELECT last_end_of_funds_notification IS NOT NULL FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))?;
        let mut out = json!({ "sent": kucoin_sent(&transport), "changes": diff(&before, &snapshot(&o.primary)?), "mails": mails(&o.primary, &owed)?,
                              "funds_notified": funds_notified, "poll_error": poll_error });
        if let Some(c) = checks { out["checks"] = json!(c); }
        return Ok(out);
    }
    if scenario["venue"] == "bitget" {
        // Rust's Bitget adapter over the bodies Rails' harness serves beneath Honeymaker::Clients::Bitget, with the
        // scenario's own tickers.
        let transport = ScriptedTransport::from_script(&scenario["script"]["bitget"]);
        let exchange_id: i64 = o.primary.query_row("SELECT id FROM exchanges WHERE type = ?1", [crate::venue::bitget::BITGET], |r| r.get(0))?;
        let pairs = crate::venue::binance::Pairs::load(&o.primary, exchange_id).map_err(EngineError::Data)?;
        let venue = crate::venue::bitget::BitgetVenue::new(transport.clone(), crate::venue::bitget::URL).with_pairs(pairs);
        let posted = || bitget_sent(&transport).len();
        let (poll, recover_sent, checks) = play(&o, &venue, &scenario, bot_id, start, &posted).await?;
        let poll_error = match poll { Some(polling::PollFailure::General(m)) => json!(m), _ => Value::Null };
        let funds_notified: bool = o.primary.query_row("SELECT last_end_of_funds_notification IS NOT NULL FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))?;
        let mut out = json!({ "sent": bitget_sent(&transport), "changes": diff(&before, &snapshot(&o.primary)?), "mails": mails(&o.primary, &owed)?,
                              "funds_notified": funds_notified, "poll_error": poll_error });
        if let Some(n) = recover_sent { out["recover_sent"] = json!(n); }
        if let Some(c) = checks { out["checks"] = json!(c); }
        return Ok(out);
    }
    if scenario["venue"] == "gemini" {
        // Rust's Gemini adapter over the bodies Rails' harness serves beneath Honeymaker::Clients::Gemini, with the
        // scenario's own tickers.
        let transport = ScriptedTransport::from_script(&scenario["script"]["gemini"]);
        let exchange_id: i64 = o.primary.query_row("SELECT id FROM exchanges WHERE type = 'Exchanges::Gemini'", [], |r| r.get(0))?;
        let pairs = crate::venue::binance::Pairs::load(&o.primary, exchange_id).map_err(EngineError::Data)?;
        let venue = crate::venue::gemini::GeminiVenue::new(transport.clone(), crate::venue::gemini::URL).with_pairs(pairs);
        let posted = || gemini_sent(&transport).len();
        let (poll, recover_sent, checks) = play(&o, &venue, &scenario, bot_id, start, &posted).await?;
        let poll_error = match poll { Some(polling::PollFailure::General(m)) => json!(m), _ => Value::Null };
        let funds_notified: bool = o.primary.query_row("SELECT last_end_of_funds_notification IS NOT NULL FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))?;
        let mut out = json!({ "sent": gemini_sent(&transport), "changes": diff(&before, &snapshot(&o.primary)?), "mails": mails(&o.primary, &owed)?,
                              "funds_notified": funds_notified, "poll_error": poll_error });
        if let Some(n) = recover_sent { out["recover_sent"] = json!(n); }
        if let Some(c) = checks { out["checks"] = json!(c); }
        return Ok(out);
    }
    if scenario["venue"] == "bitvavo" {
        // Rust's Bitvavo adapter over the bodies Rails' harness serves beneath Honeymaker::Clients::Bitvavo, with the
        // scenario's own tickers.
        let transport = ScriptedTransport::from_script(&scenario["script"]["bitvavo"]);
        let exchange_id: i64 = o.primary.query_row("SELECT id FROM exchanges WHERE type = ?1", [crate::venue::bitvavo::BITVAVO], |r| r.get(0))?;
        let pairs = crate::venue::binance::Pairs::load(&o.primary, exchange_id).map_err(EngineError::Data)?;
        let venue = crate::venue::bitvavo::BitvavoVenue::new(transport.clone(), crate::venue::bitvavo::URL).with_pairs(pairs);
        let posted = || bitvavo_sent(&transport).len();
        let (poll, _, checks) = play(&o, &venue, &scenario, bot_id, start, &posted).await?;
        let poll_error = match poll { Some(polling::PollFailure::General(m)) => json!(m), _ => Value::Null };
        let funds_notified: bool = o.primary.query_row("SELECT last_end_of_funds_notification IS NOT NULL FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))?;
        let mut out = json!({ "sent": bitvavo_sent(&transport), "changes": diff(&before, &snapshot(&o.primary)?), "mails": mails(&o.primary, &owed)?,
                              "funds_notified": funds_notified, "poll_error": poll_error });
        if let Some(c) = checks { out["checks"] = json!(c); }
        return Ok(out);
    }
    // Rust's real Kraken adapter over the bodies Rails' harness serves beneath Honeymaker::Clients::Kraken, or at the
    // Faraday adapter beneath it (`kraken_http`: the HTTP status and the transport failures too).
    let script = if scenario["script"]["kraken_http"].is_object() { scenario["script"]["kraken_http"].clone() } else { kraken_script(&scenario["script"]) };
    let transport = ScriptedTransport::from_script(&script);
    let posted = || kraken_sent(&transport).len();
    let venue = KrakenVenue::new(transport.clone());
    let (poll, _, checks) = play(&o, &venue, &scenario, bot_id, start, &posted).await?;
    let sent = kraken_sent(&transport);
    let mut out = json!({ "sent": sent, "changes": diff(&before, &snapshot(&o.primary)?), "mails": mails(&o.primary, &owed)? });
    if let Some(c) = checks { out["checks"] = json!(c); }
    // A failed Kraken follow-up is reported, as the Rails harness reports one that raised (only the listed unreadable-number
    // scenarios fail a Kraken poll; anywhere else the key's presence alone makes the outputs differ).
    if let Some(e) = poll { out["poll_error"] = json!(format!("{e:?}")); }
    Ok(out)
}

/// The scenario's phases, in Rails' harness order: the tick at `at` (with Rails' retries); the follow-up poll Rails enqueues for
/// one order, 5 s after it; the SQL a scenario runs `between` the ticks on both sides; the engine's own reconciliation tick at
/// `recover_at` (Rust only: Rails has no job then, its next run is at next_interval_checkpoint_at); the next checkpoint's tick
/// at `next_at` (both). Returns how the poll failed and, when there is a reconciliation tick, how many orders it sent.
async fn play<V: Venue>(o: &Opened, venue: &V, scenario: &Value, bot_id: i64, start: DateTime<Utc>, posted: &dyn Fn() -> usize)
    -> Result<(Option<polling::PollFailure>, Option<usize>, Option<Vec<String>>), EngineError> {
    // One price cache across retries and phases, as Rails' 5 s cache spans its retried jobs.
    let prices = PriceCache::default();
    let cx = TickContext { credential_version: crate::engine::model::credential_version(&o.primary, &crate::engine::model::load_bot(&o.primary, bot_id)?)?, prices: &prices, process_start: DateTime::<Utc>::MIN_UTC, stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
    let at = |key: &str| -> Result<Option<DateTime<Utc>>, EngineError> {
        scenario[key].as_str().map(|s| s.parse::<DateTime<Utc>>().map_err(|e| EngineError::Data(format!("scenario.{key}: {e}")))).transpose()
    };
    // A condition scenario lists every Bot::PriceLimitCheckJob a phase queues, at its time (conditions::end_of_minute and the
    // check's own next time), as Rails' harness reads them off its queue.
    let listing = scenario["conditions"] == true;
    let mut checks = vec![];
    let queued = |t: DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    // Plan CI T3b: what triggers Bot::ResyncIndexCompositionJob (the same SQL ran on Rails' copy), the marker it leaves, then
    // the engine's own resync and its retries, each at its time, before the tick.
    if let Some(when) = at("resync_at")? {
        for sql in scenario["trigger"].as_array().into_iter().flatten() {
            o.primary.execute_batch(sql.as_str().ok_or_else(|| EngineError::Data(format!("scenario.trigger: {sql}")))?)?;
        }
        crate::engine::index::mark_resync(&o.primary, bot_id, when)?;
        let mut state = crate::engine::run::Resyncs::default();
        let mut now = when;
        for _ in 0..3 {
            let Some((_, marker)) = crate::engine::index::resync_marked(&o.primary)?.into_iter().find(|(id, _)| *id == bot_id) else { break };
            let handle = |bot: &model::Bot| { let h = crate::venue::Handle::for_bot(venue, &o.primary, bot)?; let version = h.producer(); Ok((h, version)) };
            let mut wake = i64::MAX;
            crate::engine::run::resync_bot(&o.primary, bot_id, &marker, &FixedClock(now), &prices, &mut state, handle, &mut wake).await?;
            match state.retry_at(bot_id) { Some(at) => now = DateTime::from_timestamp_micros(at).ok_or_else(|| EngineError::Data("resync retry out of range".into()))?, None => break }
        }
    }
    if scenario["tick"] != false { if let Some(t) = ticks(o, venue, bot_id, start, &cx).await? { checks.push(queued(crate::engine::conditions::end_of_minute(t))); } }
    if scenario["signals"].is_array() { signal_calls(o, venue, scenario, bot_id, start, &cx).await?; }
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
    // Bot::PriceLimitCheckJob at `check_at`: met, the Bot::ActionJob it enqueues runs at once.
    if let Some(when) = at("check_at")? {
        use crate::engine::conditions::{self, Checked};
        let bot = model::load_bot(&o.primary, bot_id)?;
        match conditions::check(&o.primary, venue, &bot, &FixedClock(when), &prices, &cx.credential_version).await {
            Ok(Checked::Rearmed) => { if let Some(t) = ticks(o, venue, bot_id, when, &cx).await? { checks.push(queued(conditions::end_of_minute(t))); } }
            Ok(Checked::Next(t)) => checks.push(queued(t)),
            Ok(Checked::Gone) => {}
            // The job raised: no successor (the chain waits for Repair).
            Err(EngineError::Data(_)) => {}
            Err(e) => return Err(e),
        }
    }
    if let Some(when) = at("next_at")? { ticks(o, venue, bot_id, when, &cx).await?; }
    // Bot::EvaluateRebalancersJob's fires: one Bot::RebalanceJob each.
    for when in scenario["rebalance_at"].as_array().into_iter().flatten() {
        let when: DateTime<Utc> = when.as_str().and_then(|s| s.parse().ok()).ok_or_else(|| EngineError::Data(format!("scenario.rebalance_at: {when}")))?;
        crate::engine::rebalance::run(&o.primary, venue, bot_id, &FixedClock(when), &cx).await?;
    }
    Ok((poll, recover_sent, listing.then_some(checks)))
}

/// Plan 8a: each Bot::SignalJob of the scenario as the hook leaves it (a waiting call), then the engine's executor at `start`,
/// call after call; a call held by the barrier is tried again every 30 s, Bot::SignalJob's own retry cadence, until it
/// expires (the engine itself wakes at the expiry instead: tests/signal.rs). `before_signal` is the SQL Rails' side ran after
/// enqueueing (a rule removed).
async fn signal_calls<V: Venue>(o: &Opened, venue: &V, scenario: &Value, bot_id: i64, start: DateTime<Utc>, cx: &TickContext<'_>) -> Result<(), EngineError> {
    use crate::engine::signal::{run_next, Step};
    for (i, c) in scenario["signals"].as_array().into_iter().flatten().enumerate() {
        let triggered_at = c["triggered_at"].as_str().and_then(|s| s.parse().ok()).ok_or_else(|| EngineError::Data(format!("scenario.signals: {c}")))?;
        let call = model::SignalCall { source: format!("grid:{i}"), signal_id: c["signal_id"].as_i64().ok_or_else(|| EngineError::Data(format!("scenario.signals: {c}")))?, triggered_at };
        model::append_signal_call(&o.primary, bot_id, &call)?;
    }
    for sql in scenario["before_signal"].as_array().into_iter().flatten() {
        o.primary.execute_batch(sql.as_str().ok_or_else(|| EngineError::Data(format!("scenario.before_signal: {sql}")))?)?;
    }
    let mut at = start;
    for _ in 0..40 {
        match run_next(&o.primary, venue, bot_id, &FixedClock(at), cx).await? {
            Step::Idle => break,
            Step::Ran => {}
            Step::Waiting(_) | Step::Again => at += chrono::Duration::seconds(30),
        }
    }
    Ok(())
}

/// One Bot::ActionJob run at `start` and the retries it enqueues for itself. Some(at): the run at `at` paused the bot.
async fn ticks<V: Venue>(o: &Opened, venue: &V, bot_id: i64, start: DateTime<Utc>, cx: &TickContext<'_>) -> Result<Option<DateTime<Utc>>, EngineError> {
    let (mut at, mut attempts) = (start, Attempts::default());
    for _ in 0..MAX_ATTEMPTS {
        match tick::tick_recovering(&o.primary, venue, bot_id, &FixedClock(at), &mut attempts, &mut None, cx).await? {
            TickOutcome::RetryAfter(d) => at += chrono::Duration::from_std(d).unwrap(),
            outcome => return Ok(matches!(outcome, TickOutcome::Paused).then_some(at)),
        }
    }
    Ok(None)
}

/// For every bot this engine would run on the copy at `src`: its own scenario, ticking 1 s after its next
/// checkpoint. The copy is duplicated per bot, so each engine writes only into its own.
pub fn plan_copy(src: &Path, tickers: &Value, out: &Path, now: DateTime<Utc>) -> Result<usize, EngineError> {
    use crate::engine::{eligibility, model, schedule};
    store::check(&Paths::from_env(&|_| None, src))?;
    let c = Connection::open_with_flags(src.join("production.sqlite3"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let env=|k:&str|std::env::var(k).ok();
    if let Some(secret)=env("SECRET_KEY_BASE") {
        let keys=EncryptionKeys::resolve(&env,&secret).map_err(|_|EngineError::Data("index encryption configuration unreadable".into()))?;
        crate::engine::provider::bind(&c,&Cipher::new(&keys),&env)?;
    }
    let report = eligibility::check_install(&c)?;
    if !report.problems.is_empty() { return Err(EngineError::Ineligible(report.problems)); }
    for id in &report.eligible {
        let bot = model::load_bot(&c, *id)?;
        let (Some(anchor), Some(eff)) = (bot.started_at_us()?, bot.effective()) else { continue };
        let next = schedule::checkpoints(anchor, now.timestamp_micros(), eff)?.next_us;
        let at = DateTime::from_timestamp_micros(next.saturating_add(1_000_000)).ok_or_else(|| EngineError::Data("copy checkpoint out of range".into()))?;
        let dir = out.join(format!("bot-{id}"));
        std::fs::create_dir_all(&dir).map_err(|e| EngineError::Data(e.to_string()))?;
        for f in ["production.sqlite3", "production_queue.sqlite3"] {
            // `VACUUM INTO` gives a consistent single-file copy even when the source has a -wal file.
            let target = dir.join(f);
            Connection::open_with_flags(src.join(f), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?
                .execute("VACUUM INTO ?1", [target.to_string_lossy()])?;
        }
        // Every member's pair: a basket's legs each read their own price from the copy's script.
        let pairs = if bot.bot_type == "Bots::DcaIndex" { crate::engine::index::candidate_pairs(&c, &bot)? } else { crate::engine::basket::member_pairs(&c, &bot)? };
        let mut crypto = Vec::new();
        for pair in &pairs {
            if model::ticker_for_pair(&c, bot.exchange_id, pair)?.is_some_and(|t| t.crypto) {
                crypto.push(pair.clone());
            }
        }
        let recorded: Vec<Value> = (if model::exchange_type(&c, &bot)? == "Exchanges::Alpaca" { &crypto } else { &pairs }).iter()
            .map(|p| tickers.get(p).cloned().ok_or_else(|| EngineError::Data(format!("no recorded Ticker body for {p}"))))
            .collect::<Result<_, _>>()?;
        let alpaca = model::exchange_type(&c, &bot)? == "Exchanges::Alpaca";
        let mut script = if alpaca { alpaca_copy_script(&c, &bot, &recorded, *id)? } else { json!({ "http": {
            "/0/public/Ticker": [recorded.first().cloned().unwrap_or(Value::Null)],
            "/0/private/AddOrder": (1..=placements(1)).map(|n| json!({ "error": [], "result": { "txid": [format!("OPARITY-{id}-{n}")] } })).collect::<Vec<_>>(),
            "/0/private/BalanceEx": [{ "error": [], "result": { "ZEUR": { "balance": "1000000000", "hold_trade": "0" }, "ZUSD": { "balance": "1000000000", "hold_trade": "0" } } }],
            "/0/private/QueryOrders": [{ "error": [], "result": {} }],
            "/0/private/TradesHistory": [{ "error": [], "result": { "trades": {}, "count": 0 } }] } }) };
        if alpaca {
            add_stock_bodies(&c, &bot, &mut script, &pairs, tickers, at)?;
            script["alpaca"]["POST /v2/orders"] = json!((1..=placements(pairs.len())).map(|n| json!({"status":200,"body":{"id":format!("OPARITY-{id}-{n}"),"status":"pending_new"}})).collect::<Vec<_>>());
        }
        let scenario = json!({ "parity_scratch": true, "bot_id": id, "at": at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
                               "venue": if alpaca { "alpaca" } else { "kraken" }, "script": script });
        std::fs::write(dir.join("scenario.json"), serde_json::to_string_pretty(&scenario).map_err(|e| EngineError::Data(e.to_string()))?).map_err(|e| EngineError::Data(e.to_string()))?;
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

const REFERENCE_TABLES: [&str; 6] = ["assets", "tickers", "exchange_assets", "indices", "bot_activity_logs", "app_configs"];
const REFERENCE_JSON: [&str; 6] = ["top_coins", "top_coins_by_exchange", "available_exchanges", "weights", "withdrawal_chains", "details"];

/// The reference-data tables as script/rust/reference_data.rb snapshots them: JSON columns parsed, app_configs values
/// decrypted (the IV is random), and the runner's own `rust_job.*` rows left out.
pub fn reference_snapshot(c: &Connection, cipher: &Cipher) -> Result<Value, EngineError> {
    let mut out = Map::new();
    for table in REFERENCE_TABLES {
        let (names, snapshot) = crate::figures::fill::parity_rows(c,table,"id")?;
        let mut rows = Map::new();
        for r in snapshot {
            let mut row = Map::new();
            for (i, name) in names.iter().enumerate() {
                // Rust-only ownership attribution; placement/tick_basket assert the exact
                // sending identity independently. Rails leaves this shared column NULL.
                if table == "transactions" && name == "run_identity" { continue; }
                let mut v = raw((&r[i]).into());
                if REFERENCE_JSON.contains(&name.as_str()) { if let Value::String(s) = &v { v = serde_json::from_str(s).unwrap_or(v); } }
                row.insert(name.clone(), v);
            }
            if table == "app_configs" {
                if row["key"].as_str().is_some_and(|k| k.starts_with(jobs::state::PREFIX)) { continue; }
                if let Some(v) = row["value"].as_str().map(String::from) {
                    row.insert("value".into(), json!(cipher.decrypt(&v).map_err(|e| EngineError::Data(format!("app_configs value: {e:?}")))?));
                }
            }
            // Configuration rows by key: an id depends on which rows each side inserted first (the runner's
            // own `rust_job.*` rows take ids), and nothing refers to an app_configs id.
            let id = if table == "app_configs" { row.remove("id"); row["key"].as_str().unwrap_or_default().to_string() } else { row["id"].to_string() };
            rows.insert(id, Value::Object(row));
        }
        out.insert(table.into(), Value::Object(rows));
    }
    Ok(Value::Object(out))
}

/// Every row whose content changed, deletions included (`after: null`), in id order: Rails' `diff` in reference_data.rb.
pub fn diff_tables(before: &Value, after: &Value, tables: &[&str]) -> Value {
    let mut out = Map::new();
    for table in tables {
        let (b, a) = (&before[*table], &after[*table]);
        let by_key = *table == "app_configs"; // keyed by `key`, not by id (reference_snapshot)
        let mut ids: Vec<String> = b.as_object().into_iter().chain(a.as_object()).flat_map(|m| m.keys().cloned()).collect();
        if by_key { ids.sort(); } else { ids.sort_by_key(|k| k.parse::<i64>().unwrap_or(0)); }
        ids.dedup();
        let changes: Vec<Value> = ids.into_iter().filter_map(|k| {
            let (x, y) = (b.get(&k).cloned().unwrap_or(Value::Null), a.get(&k).cloned().unwrap_or(Value::Null));
            let id = if by_key { json!(k) } else { json!(k.parse::<i64>().unwrap_or(0)) };
            (x != y).then(|| json!({ "id": id, "before": x, "after": y }))
        }).collect();
        out.insert((*table).into(), Value::Array(changes));
    }
    Value::Object(out)
}

/// The Rust half of the reference-data parity (script/rust/reference_data.rb is the Rails half): the scenario's job, once,
/// as the scheduler runs it, on a marked scratch copy, against the same scripted data-api bodies. Rails' shape.
pub async fn reference(dir: &Path) -> Result<Value, EngineError> {
    let read = std::fs::read_to_string(dir.join("scenario.json")).map_err(|e| EngineError::Data(e.to_string()))?;
    let scenario: Value = serde_json::from_str(&read).map_err(|e| EngineError::Data(e.to_string()))?;
    if scenario["parity_scratch"] != true {
        return Err(EngineError::Data(format!("{} is not a parity scratch copy (no parity_scratch marker)", dir.display())));
    }
    let at: DateTime<Utc> = scenario["at"].as_str().and_then(|s| s.parse().ok()).ok_or_else(|| EngineError::Data("scenario.at".into()))?;
    let job = scenario["job"].as_str().ok_or_else(|| EngineError::Data("scenario.job".into()))?;
    let text = |v: &Value| v.as_str().map(String::from).ok_or_else(|| EngineError::Data("scenario.encryption".into()));
    let keys = EncryptionKeys { primary_key: text(&scenario["encryption"]["primary_key"])?, key_derivation_salt: text(&scenario["encryption"]["key_derivation_salt"])? };
    let paths = Paths::from_env(&|_| None, dir);
    let _lock = lease::lock(&paths, chrono::Utc::now())?;
    let o = store::open(&paths)?;
    let cipher = Cipher::new(&keys);
    let env_map = scenario["env"].clone();
    let env = move |k: &str| env_map[k].as_str().map(String::from);
    let before = reference_snapshot(&o.primary, &cipher)?;
    let transport = ScriptedTransport::from_script(&scenario["script"]);
    let api = data_api::config(&env, &o.primary, &cipher).map_err(EngineError::Data)?
        .map(|c| DataApi::new(c, transport.clone(), transport.clone()));
    // The job's own connection must read the same scenario environment as its client.
    let job_primary = store::open(&paths)?.primary;
    crate::engine::provider::bind(&job_primary, &cipher, &env)?;
    let db = jobs::Db::new(job_primary, cipher.clone());
    let outcome = reference::run_once(job, api, jobs::Cx { db, clock: &FixedClock(at), wakers: jobs::Wakers::default() }).await;
    let requests: Vec<String> = transport.requests().iter().map(request_key).collect();
    Ok(json!({ "requests": requests, "retry": matches!(outcome, Outcome::Transient(_) | Outcome::RateLimited(_)),
               "changes": diff_tables(&before, &reference_snapshot(&o.primary, &cipher)?, &REFERENCE_TABLES) }))
}

/// Plan 2d: each stock pair's recorded latest quote and trade (tickers.json `{"AAPL": {"quote": …, "trade": …}}`, the bodies
/// GET /v2/stocks/{base}/quotes/latest and /trades/latest return) at its own path, and a clock open at the copy's tick with its
/// close an hour on, so both engines decide an open session. Crypto pairs keep 2c's bodies.
fn add_stock_bodies(c: &Connection, bot: &crate::engine::model::Bot, script: &mut Value, pairs: &[String], tickers: &Value, at: DateTime<Utc>) -> Result<(), EngineError> {
    use crate::engine::model;
    let ok = |b: Value| json!([{ "status": 200, "body": b }]);
    for pair in pairs {
        let Some(t) = model::ticker_for_pair(c, bot.exchange_id, pair)? else { continue };
        if t.crypto { continue; }
        let rec = tickers.get(pair).ok_or_else(|| EngineError::Data(format!("no recorded stock bodies for {pair}")))?;
        script["alpaca"][format!("GET /v2/stocks/{}/quotes/latest", t.base_code)] = ok(rec["quote"].clone());
        script["alpaca"][format!("GET /v2/stocks/{}/trades/latest", t.base_code)] = ok(rec["trade"].clone());
    }
    let text = |t: DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let next_open = at.checked_add_signed(chrono::Duration::days(1))
        .ok_or_else(|| EngineError::Data("copy clock out of range".into()))?;
    let next_close = at.checked_add_signed(chrono::Duration::hours(1))
        .ok_or_else(|| EngineError::Data("copy clock out of range".into()))?;
    script["alpaca"]["GET /v2/clock"] = ok(json!({ "timestamp": text(at), "is_open": true,
        "next_open": text(next_open), "next_close": text(next_close) }));
    Ok(())
}
