//! The engine's rows, read and written exactly as Rails reads and writes them.
use super::schedule::Interval;
use super::EngineError;
use crate::codec::{format_time, parse_time};
use crate::crypto::{Cipher, Credentials};
use crate::enums::{BotStatus, BOT_WORKING};
use crate::ruby::{from_sql, BigDec};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct Bot {
    pub id: i64, pub user_id: i64, pub exchange_id: i64, pub status: BotStatus, pub bot_type: String,
    pub settings: Value, pub transient: Value,
    pub started_at_us: Option<i64>, pub settings_changed_at_us: Option<i64>, pub restatement_generation: i64,
}

fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }
fn us(s: Option<String>) -> Result<Option<i64>, EngineError> {
    s.map(|s| parse_time(&s).map(|t| t.timestamp_micros()).map_err(data)).transpose()
}
fn json_col(s: String) -> Result<Value, EngineError> { serde_json::from_str(&s).map_err(data) }
pub fn working_list() -> String { BOT_WORKING.iter().map(|s| (*s as i64).to_string()).collect::<Vec<_>>().join(",") }

pub fn load_bot(c: &Connection, id: i64) -> Result<Bot, EngineError> {
    let row = c.query_row(
        "SELECT id, user_id, exchange_id, status, type, settings, transient_data, started_at, settings_changed_at, restatement_generation FROM bots WHERE id = ?1",
        [id],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, Option<String>>(7)?, r.get::<_, Option<String>>(8)?, r.get::<_, i64>(9)?)),
    )?;
    let (id, user, exchange, status, ty, settings, transient, started, changed, restated) = row;
    Ok(Bot {
        id, user_id: user.unwrap_or(0), exchange_id: exchange.unwrap_or(0),
        status: BotStatus::from_i64(status).ok_or_else(|| EngineError::Data(format!("bot {id}: status {status}")))?,
        bot_type: ty.unwrap_or_default(), settings: json_col(settings)?, transient: json_col(transient)?,
        started_at_us: us(started)?, settings_changed_at_us: us(changed)?, restatement_generation: restated,
    })
}

impl Bot {
    pub fn interval(&self) -> Option<Interval> { self.settings.get("interval")?.as_str().and_then(Interval::parse) }
    pub fn quote_amount(&self) -> Option<f64> { self.settings.get("quote_amount")?.as_f64() }
    /// Bot::SmartIntervalable: `smart_intervaled?` is `== true`; the split amount must be a JSON number. A string
    /// makes Rails' `effective_quote_amount * intervals` raise TypeError, so eligibility refuses it (never traded).
    pub fn smart_quote_amount(&self) -> Option<f64> {
        if self.settings.get("smart_intervaled") != Some(&Value::Bool(true)) || self.quote_amount().is_none() { return None; }
        self.settings.get("smart_interval_quote_amount")?.as_f64()
    }
    pub fn limit_ordered(&self) -> bool { self.settings.get("limit_ordered") == Some(&Value::Bool(true)) }
    /// Bot::LimitOrderable: `limit_ordered?` is `== true`; #limit_order_pcnt_distance_decimal is 0.001 for a blank
    /// value, else `.to_d`. None on a limit bot = a shape this build does not read as a number: eligibility refuses it.
    pub fn limit_distance(&self) -> Option<BigDec> {
        if !self.limit_ordered() { return None; }
        match self.settings.get("limit_order_pcnt_distance") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => BigDec::parse("0.001").ok(),
            Some(Value::String(s)) if s.trim().is_empty() => BigDec::parse("0.001").ok(),
            Some(Value::Array(a)) if a.is_empty() => BigDec::parse("0.001").ok(),
            Some(Value::Object(o)) if o.is_empty() => BigDec::parse("0.001").ok(),
            Some(Value::Number(n)) => match n.as_i64() { Some(i) => Some(BigDec::from_i64(i)), None => BigDec::from_f64(n.as_f64()?).ok() },
            Some(Value::String(s)) => BigDec::parse(s).ok(),
            Some(_) => None,
        }
    }
    pub fn asset_ids(&self) -> Vec<i64> {
        self.settings.get("allocations").and_then(Value::as_object).map(|m| m.keys().filter_map(|k| k.parse().ok()).collect()).unwrap_or_default()
    }
    pub fn quote_asset_id(&self) -> Option<i64> { self.settings.get("quote_asset_id")?.as_i64() }
    /// Bot::Accountable#missed_quote_amount: `value.present? ? value.to_d : 0`.
    pub fn missed_quote_amount(&self) -> Result<BigDec, EngineError> {
        match self.transient.get("missed_quote_amount") {
            None | Some(Value::Null) => Ok(BigDec::zero()),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(BigDec::zero()),
            Some(Value::String(s)) => BigDec::parse(s).map_err(data),
            Some(Value::Number(n)) => match n.as_i64() {
                Some(i) => Ok(BigDec::from_i64(i)),
                None => BigDec::from_f64(n.as_f64().unwrap()).map_err(data),
            },
            Some(other) => Err(EngineError::Data(format!("missed_quote_amount {other}"))),
        }
    }
    pub fn rust_placement(&self) -> Option<Value> { self.transient.get("rust_placement").filter(|v| !v.is_null()).cloned() }
    pub fn last_failure_kind(&self) -> Option<String> { self.transient.get("last_failure_kind")?.as_str().map(str::to_string) }
    pub fn last_action_job_at_us(&self) -> Result<Option<i64>, EngineError> {
        match self.transient.get("last_action_job_at") {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| Some(t.with_timezone(&Utc).timestamp_micros()))
                .ok_or_else(|| EngineError::Data(format!("bot {}: last_action_job_at {v}", self.id))),
        }
    }
    /// Bot::Accountable#carry_window_marks.compact.max
    pub fn calc_since_us(&self) -> Option<i64> { [self.started_at_us, self.settings_changed_at_us].into_iter().flatten().max() }
}

#[derive(Debug, Clone)]
pub struct Ticker {
    pub id: i64, pub ticker: String,
    /// tickers.base: the venue's own code for the base (Kraken "XBT", Alpaca "BTC").
    pub base_code: String, pub base_symbol: String, pub quote_symbol: String, pub exchange_name: String,
    pub base_asset_id: i64, pub quote_asset_id: i64, pub base_decimals: i64, pub quote_decimals: i64, pub price_decimals: i64,
    pub minimum_base_size: BigDec, pub minimum_quote_size: BigDec, pub trading_enabled: bool, pub available: bool,
}

/// The member's ticker: this venue, the member asset, the bot's quote asset (Bots::DcaMultiAsset#set_tickers).
pub fn ticker_for(c: &Connection, bot: &Bot) -> Result<Option<Ticker>, EngineError> {
    let (Some(&base), Some(quote)) = (bot.asset_ids().first(), bot.quote_asset_id()) else { return Ok(None) };
    let row = c.query_row(
        "SELECT t.id, t.ticker, b.symbol, q.symbol, e.name, t.base_asset_id, t.quote_asset_id, t.base_decimals, t.quote_decimals, t.price_decimals, \
                t.minimum_base_size, t.minimum_quote_size, t.trading_enabled, t.available, t.base \
         FROM tickers t JOIN assets b ON b.id = t.base_asset_id JOIN assets q ON q.id = t.quote_asset_id JOIN exchanges e ON e.id = t.exchange_id \
         WHERE t.exchange_id = ?1 AND t.base_asset_id = ?2 AND t.quote_asset_id = ?3",
        params![bot.exchange_id, base, quote],
        |r| {
            let dec = |i: usize| from_sql(r.get_ref(i)?).map_err(|e| rusqlite::Error::InvalidColumnName(format!("{e:?}")));
            Ok(Ticker {
                id: r.get(0)?, ticker: r.get(1)?, base_code: r.get(14)?, base_symbol: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                quote_symbol: r.get::<_, Option<String>>(3)?.unwrap_or_default(), exchange_name: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                base_asset_id: r.get(5)?, quote_asset_id: r.get(6)?, base_decimals: r.get(7)?, quote_decimals: r.get(8)?, price_decimals: r.get(9)?,
                minimum_base_size: dec(10)?.unwrap_or_else(BigDec::zero), minimum_quote_size: dec(11)?.unwrap_or_else(BigDec::zero),
                // Ticker#available? on a NULL column is false, exactly as the placement guard reads it.
                trading_enabled: r.get::<_, Option<bool>>(12)?.unwrap_or(false), available: r.get::<_, Option<bool>>(13)?.unwrap_or(false),
            })
        },
    ).optional()?;
    Ok(row)
}

pub fn exchange_name(c: &Connection, bot: &Bot) -> Result<String, EngineError> {
    let name: Option<String> = c.query_row("SELECT name FROM exchanges WHERE id = ?1", [bot.exchange_id], |r| r.get(0)).optional()?.flatten();
    Ok(name.unwrap_or_default())
}

/// Bot#api_key: `user.api_keys.find_by(exchange_id:, key_type: :trading)`, any status.
pub fn credentials_for(c: &Connection, cipher: &Cipher, bot: &Bot) -> Result<Option<Credentials>, EngineError> {
    let row: Option<KeyRow> = c.query_row(
        "SELECT k.key, k.secret, k.passphrase, e.type FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id \
         WHERE k.user_id = ?1 AND k.exchange_id = ?2 AND k.key_type = 0 LIMIT 1",
        params![bot.user_id, bot.exchange_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
    let Some((key, secret, passphrase, exchange_type)) = row else { return Ok(None) };
    // A row that does not decrypt is not "no key": sent keyless, the venue would answer invalid key and the bot
    // would be stopped as invalid_key. As in Rails (ActiveRecord::Encryption raises), the tick fails instead.
    let open = |v: Option<String>| v.map(|v| cipher.decrypt(&v).map_err(|e| EngineError::Data(format!("api key unreadable for bot {}: {e:?}", bot.id)))).transpose();
    // Only Alpaca reads the passphrase (its mode). Elsewhere it is never decrypted, so an unreadable unused value cannot
    // fail a Kraken bot's otherwise valid key: Kraken's credential loading stays exactly as merged.
    let passphrase = if exchange_type.as_deref() == Some("Exchanges::Alpaca") { open(passphrase)? } else { None };
    Ok(match (open(key)?, open(secret)?) { (Some(key), Some(secret)) => Some(Credentials { key, secret, passphrase }), _ => None })
}

/// api_keys.key, secret, passphrase and the exchange's STI type, all still encrypted.
type KeyRow = (Option<String>, Option<String>, Option<String>, Option<String>);

#[derive(Debug, Clone, Copy)]
pub enum Level { Info = 0, Warning = 1, Error = 2 }

/// `update!(status:)`: updated_at moves.
pub fn update_status(c: &Connection, bot_id: i64, status: BotStatus, now: DateTime<Utc>) -> Result<(), EngineError> {
    c.execute("UPDATE bots SET status = ?1, updated_at = ?2 WHERE id = ?3", params![status as i64, format_time(now), bot_id])?;
    Ok(())
}

/// `executing`/`waiting` exist only mid-tick: met outside one, the last run was cut short. Back to scheduled.
pub fn unstick(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let n = c.execute("UPDATE bots SET status = ?1, updated_at = ?2 WHERE id = ?3 AND status IN (?4, ?5)",
                      params![BotStatus::Scheduled as i64, format_time(now), bot_id, BotStatus::Executing as i64, BotStatus::Waiting as i64])?;
    Ok(n == 1)
}

/// Bot::ActionJob.transition_working_bot!: only while the row is still working.
pub fn transition_working(c: &Connection, bot_id: i64, status: BotStatus, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let n = c.execute(&format!("UPDATE bots SET status = ?1, updated_at = ?2 WHERE id = ?3 AND status IN ({})", working_list()),
                      params![status as i64, format_time(now), bot_id])?;
    Ok(n == 1)
}

/// A write transaction that takes SQLite's write lock up front (BEGIN IMMEDIATE), so a read-modify-write can
/// never interleave with another writer. Every multi-statement write in the engine opens one of these.
pub fn immediate(c: &Connection) -> Result<rusqlite::Transaction<'_>, EngineError> {
    Ok(rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?)
}

/// Runs `f` under the write lock: inside the caller's transaction if there is one, else in its own.
fn locked<T>(c: &Connection, f: impl FnOnce(&Connection) -> Result<T, EngineError>) -> Result<T, EngineError> {
    if !c.is_autocommit() { return f(c); }
    let tx = immediate(c)?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

fn read_transient(c: &Connection, bot_id: i64) -> Result<Map<String, Value>, EngineError> {
    let raw: String = c.query_row("SELECT transient_data FROM bots WHERE id = ?1", [bot_id], |r| r.get(0))?;
    match json_col(raw)? { Value::Object(m) => Ok(m), other => Err(EngineError::Data(format!("transient_data {other}"))) }
}

/// `update!(key: value, …)` through store_accessor: nulls are stored, updated_at moves.
pub fn update_transient(c: &Connection, bot_id: i64, pairs: &[(&str, Value)], now: DateTime<Utc>) -> Result<(), EngineError> {
    locked(c, |c| {
        let mut m = read_transient(c, bot_id)?;
        for (k, v) in pairs { m.insert((*k).to_string(), v.clone()); }
        c.execute("UPDATE bots SET transient_data = ?1, updated_at = ?2 WHERE id = ?3", params![Value::Object(m).to_string(), format_time(now), bot_id])?;
        Ok(())
    })
}

/// Bot#merge_transient_data!: `transient_data.merge(values).compact` through update_columns.
pub fn merge_transient_compact(c: &Connection, bot_id: i64, pairs: &[(&str, Value)]) -> Result<(), EngineError> {
    locked(c, |c| {
        let mut m = read_transient(c, bot_id)?;
        for (k, v) in pairs { m.insert((*k).to_string(), v.clone()); }
        m.retain(|_, v| !v.is_null());
        c.execute("UPDATE bots SET transient_data = ?1 WHERE id = ?2", params![Value::Object(m).to_string(), bot_id])?;
        Ok(())
    })
}

pub fn log_activity(c: &Connection, bot_id: i64, event: &str, level: Level, details: Value, now: DateTime<Utc>) -> Result<(), EngineError> {
    c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
              params![bot_id, event, level as i64, details.to_string(), format_time(now)])?;
    Ok(())
}
