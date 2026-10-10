//! The engine's rows, read and written exactly as Rails reads and writes them.
use super::schedule::Interval;
use super::EngineError;
use crate::codec::{format_time, parse_time};
use crate::crypto::{Cipher, Credentials};
use crate::enums::{BotStatus, BOT_WORKING};
use crate::ruby::{from_sql, BigDec, Num};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Bot {
    pub id: i64, pub user_id: i64, pub exchange_id: i64, pub status: BotStatus, pub bot_type: String,
    pub settings: Value, pub transient: Value,
    started_at: Option<String>, settings_changed_at: Option<String>, pub restatement_generation: i64,
}

fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }
fn us(s: Option<&str>) -> Result<Option<i64>, EngineError> {
    s.map(|s| parse_time(s).map(|t| t.timestamp_micros()).map_err(data)).transpose()
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
    let settings=json_col(settings)?; let transient=json_col(transient)?;
    Ok(Bot {
        id, user_id: user.unwrap_or(0), exchange_id: exchange.unwrap_or(0),
        status: BotStatus::from_i64(status).ok_or_else(|| EngineError::Data(format!("bot {id}: status {status}")))?,
        bot_type: ty.unwrap_or_default(), settings, transient,
        started_at: started, settings_changed_at: changed, restatement_generation: restated,
    })
}

impl Bot {
    /// R9c: loading preserves damaged data; money/schedule accessors parse on use.
    pub fn started_at_us(&self) -> Result<Option<i64>, EngineError> { us(self.started_at.as_deref()) }
    pub fn settings_changed_at_us(&self) -> Result<Option<i64>, EngineError> { us(self.settings_changed_at.as_deref()) }
    pub fn validate_times(&self) -> Result<(), EngineError> {
        self.started_at_us()?;
        self.settings_changed_at_us()?;
        crate::codec::validate_bot_times(&self.settings, &self.transient).map_err(data)
    }

    /// Bots::DcaIndex settings (dca_index.rb:19-30): 'top' or 'category'.
    pub fn index_type(&self) -> Option<&str> { self.settings.get("index_type")?.as_str() }
    pub fn index_category_id(&self) -> Option<&str> { self.settings.get("index_category_id")?.as_str().filter(|s| !s.trim().is_empty()) }
    /// `num_coins.to_i`: a JSON integer or a string of digits (the slider posts strings). None for anything else: refused.
    pub fn num_coins(&self) -> Option<i64> {
        match self.settings.get("num_coins")? { Value::Number(n) => n.as_i64(), Value::String(s) => s.trim().parse().ok(), _ => None }
    }
    /// `ActiveModel::Type::Boolean.new.cast(hold_all)` read as `hold_all?`: nil and "" are nil (false); false, 0, "0", "f", "F",
    /// "false", "FALSE", "off", "OFF" are false; anything else is true.
    pub fn hold_all(&self) -> bool {
        match self.settings.get("hold_all") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64() != Some(0.0),
            Some(Value::String(s)) => !(s.is_empty() || ["0", "f", "F", "false", "FALSE", "off", "OFF"].contains(&s.as_str())),
            Some(_) => true,
        }
    }
    /// `allocation_flattening.to_f` (after_initialize `||= 0.0`): a JSON number or a numeric string. None: refused.
    pub fn allocation_flattening(&self) -> Option<f64> {
        match self.settings.get("allocation_flattening") {
            None | Some(Value::Null) => Some(0.0),
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => s.trim().parse().ok(),
            Some(_) => None,
        }
    }

    pub fn interval(&self) -> Option<Interval> { self.settings.get("interval")?.as_str().and_then(Interval::parse) }
    // Float views below are for Rails' explicitly floating interval-duration calculations.
    pub fn quote_amount(&self) -> Option<f64> { self.settings.get("quote_amount")?.as_f64() }
    pub fn quote_amount_num(&self) -> Result<Num, EngineError> {
        super::accounting::setting_num(self.settings.get("quote_amount").ok_or_else(||data("quote_amount missing"))?)
    }
    pub fn effective_quote_amount_num(&self) -> Result<Num, EngineError> {
        if self.settings.get("smart_intervaled") == Some(&Value::Bool(true)) {
            if let Some(value) = self.settings.get("smart_interval_quote_amount").filter(|v|v.is_number()) {
                return super::accounting::setting_num(value);
            }
        }
        self.quote_amount_num()
    }
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
    /// settings.allocations as Bots::DcaMultiAsset#derive_composition reads it: (asset id, weight) in the stored order (the
    /// JSON column keeps it). None when a key is not an id or a weight is not a JSON number (Rails' sliders store Floats):
    /// eligibility refuses it.
    pub fn allocations(&self) -> Option<Vec<(i64, f64)>> {
        let m = self.settings.get("allocations")?.as_object()?;
        m.iter().map(|(k, v)| Some((k.parse().ok()?, v.as_f64()?))).collect()
    }
    /// Bots::DcaMultiAsset#weighting: `super.presence || 'manual'`. A non-string is returned as its JSON, never "manual".
    pub fn weighting(&self) -> String {
        match self.settings.get("weighting") {
            None | Some(Value::Null) => "manual".into(),
            Some(Value::String(s)) if s.trim().is_empty() => "manual".into(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        }
    }
    /// Bot::QuoteAmountLimitable#quote_amount_limited?: `== true`.
    pub fn quote_amount_limited(&self) -> bool { self.settings.get("quote_amount_limited") == Some(&Value::Bool(true)) }
    /// The cap as #quote_amount_available_before_limit_reached reads it, as the Ruby Integer or Float the settings JSON holds;
    /// None when the limit is off. A stored nil or false reads as Integer 1000 (after_initialize: `quote_amount_limit ||= 1000`).
    /// Err for anything else that is not a JSON number (a blank string is Float::INFINITY in Rails, other strings raise there):
    /// eligibility refuses it.
    pub fn quote_amount_limit(&self) -> Result<Option<Num>, String> {
        if !self.quote_amount_limited() { return Ok(None); }
        match self.settings.get("quote_amount_limit") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(Some(Num::Int(1000))),
            Some(Value::Number(n)) => match (n.as_i64(), n.as_f64()) {
                (Some(i), _) => Ok(Some(Num::Int(i))),
                (None, Some(f)) if f.is_finite() => Ok(Some(Num::Float(f))),
                _ => Err(format!("quote_amount_limit {n} is not a number")),
            },
            Some(other) => Err(format!("quote_amount_limit {other} is not a number")),
        }
    }
    /// transient_data.quote_amount_limit_enabled_at (`Time.zone.parse` of what Time#as_json wrote), in µs. None when unset:
    /// Rails' `created_at >= NULL` then counts nothing, and the whole cap is available.
    pub fn quote_amount_limit_enabled_at_us(&self) -> Result<Option<i64>, String> {
        crate::codec::optional_time(self.transient.get("quote_amount_limit_enabled_at"))
            .map(|at|at.map(|at|at.timestamp_micros())).map_err(|e|format!("quote_amount_limit_enabled_at: {e:?}"))
    }
    /// Bot::Composition::OrderSetter::MERGED_HISTORY_KEY: a merge left inherited rows (Bot::Merge).
    pub fn merged_history(&self) -> bool { self.transient.get("merged_history_until_id").is_some_and(|v| !v.is_null()) }
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
    /// transient_data.rust_defer_until (placement::defer_to_next_checkpoint): `{"until": RFC 3339, "schedule": schedule_key}`,
    /// no tick before `until` (in µs) while the bot's schedule is still `schedule`.
    pub fn rust_defer(&self) -> Result<Option<(i64, String)>, EngineError> {
        match self.transient.get("rust_defer_until") {
            None => Ok(None),
            // No writer stores a null marker (placement::remove_wait removes the key): null is damage, never absence.
            Some(Value::Null) => Err(data("null defer marker")),
            Some(v) => {
                let at=crate::codec::optional_time(v.get("until")).map_err(data)?.ok_or_else(||data("missing defer timestamp"))?;
                let schedule=v["schedule"].as_str().ok_or_else(||data("missing defer schedule"))?;
                Ok(Some((at.timestamp_micros(),schedule.to_string())))
            }
        }
    }
    /// Tests only: `rust_defer`'s time.
    pub fn rust_defer_until_us(&self) -> Result<Option<i64>, EngineError> { Ok(self.rust_defer()?.map(|(t, _)| t)) }
    /// What the bot's checkpoints are computed from: its start (a fresh start moves it) and its effective interval.
    pub fn schedule_key(&self) -> Result<Option<String>, EngineError> {
        let (Some(anchor), Some(interval), Some(quote)) = (self.started_at_us()?, self.interval(), self.quote_amount()) else { return Ok(None) };
        Ok(Some(format!("{anchor}/{:?}", super::schedule::effective(interval, quote, self.smart_quote_amount()))))
    }
    /// What an amount-limit stop is counted under: the bot's start (a fresh start moves it) and its limit settings. A count
    /// whose key no longer matches lost its cause (the user started the bot afresh, or changed or switched off the limit).
    pub fn amount_limit_key(&self) -> Result<String, EngineError> {
        Ok(serde_json::json!([self.started_at_us()?, self.settings.get("quote_amount_limited"), self.settings.get("quote_amount_limit")]).to_string())
    }
    /// transient_data.rust_amount_limit_stops_pending: its count, and whether it still applies: counted under this bot's current
    /// key, and no continue start pending (`rust_continue_start`): the user's resume overrides a stop counted before it, which
    /// Rails would already have run, and a plain resume keeps the key. A count that raced the resume inside one tick is
    /// discarded too (the safe direction for a resume the user asked for).
    pub fn pending_amount_limit_stops(&self) -> Result<Option<(i64, bool)>, EngineError> {
        let Some(v) = self.transient.get("rust_amount_limit_stops_pending").filter(|v| !v.is_null()) else { return Ok(None) };
        let continued = self.transient.get("rust_continue_start").is_some();
        Ok(Some((v["count"].as_i64().unwrap_or(0), !continued && v["key"].as_str() == Some(self.amount_limit_key()?.as_str()))))
    }
    pub fn rust_placement(&self) -> Option<Value> { self.transient.get("rust_placement").filter(|v| !v.is_null()).cloned() }
    pub fn last_failure_kind(&self) -> Option<String> { self.transient.get("last_failure_kind")?.as_str().map(str::to_string) }
    pub fn last_action_job_at_us(&self) -> Result<Option<i64>, EngineError> {
        crate::codec::optional_time(self.transient.get("last_action_job_at"))
            .map(|at|at.map(|at|at.timestamp_micros())).map_err(data)
    }
    /// Bot::Accountable#carry_window_marks.compact.max
    pub fn calc_since_us(&self) -> Result<Option<i64>, EngineError> { Ok([self.started_at_us()?, self.settings_changed_at_us()?].into_iter().flatten().max()) }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ticker {
    pub id: i64, pub ticker: String,
    /// tickers.base: the venue's own code for the base (Kraken "XBT", Alpaca "BTC").
    pub base_code: String, pub quote_code: String, pub base_symbol: String, pub quote_symbol: String, pub exchange_name: String,
    pub base_asset_id: i64, pub quote_asset_id: i64, pub base_decimals: i64, pub quote_decimals: i64, pub price_decimals: i64,
    pub minimum_base_size: BigDec, pub minimum_quote_size: BigDec, pub trading_enabled: bool, pub available: bool, pub crypto: bool,
}

impl Ticker {
    /// Base, quote and price decimals as rounding scales (`ruby::scale`). Out of range is a data problem eligibility refuses.
    pub fn scales(&self) -> Result<(u8, u8, u8), String> {
        let one = |name: &str, v: i64| crate::ruby::scale(v).map_err(|_| format!("ticker {name} {v} (only 0..={})", crate::ruby::MAX_SCALE));
        Ok((one("base_decimals", self.base_decimals)?, one("quote_decimals", self.quote_decimals)?, one("price_decimals", self.price_decimals)?))
    }
}

const TICKER_SELECT: &str = "SELECT t.id, t.ticker, b.symbol, q.symbol, e.name, t.base_asset_id, t.quote_asset_id, t.base_decimals, \
    t.quote_decimals, t.price_decimals, t.minimum_base_size, t.minimum_quote_size, t.trading_enabled, t.available, t.base, t.quote, b.category \
    FROM tickers t JOIN assets b ON b.id = t.base_asset_id JOIN assets q ON q.id = t.quote_asset_id JOIN exchanges e ON e.id = t.exchange_id";

fn ticker_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Ticker> {
    let dec = |i: usize| from_sql(r.get_ref(i)?).map_err(|e| rusqlite::Error::InvalidColumnName(format!("{e:?}")));
    Ok(Ticker {
        id: r.get(0)?, ticker: r.get(1)?, base_code: r.get(14)?, quote_code: r.get(15)?, base_symbol: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        quote_symbol: r.get::<_, Option<String>>(3)?.unwrap_or_default(), exchange_name: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        base_asset_id: r.get(5)?, quote_asset_id: r.get(6)?, base_decimals: r.get(7)?, quote_decimals: r.get(8)?, price_decimals: r.get(9)?,
        minimum_base_size: dec(10)?.unwrap_or_else(BigDec::zero), minimum_quote_size: dec(11)?.unwrap_or_else(BigDec::zero),
        // Ticker#available? on a NULL column is false, exactly as the placement guard reads it.
        trading_enabled: r.get::<_, Option<bool>>(12)?.unwrap_or(false), available: r.get::<_, Option<bool>>(13)?.unwrap_or(false), crypto: r.get::<_, Option<String>>(16)?.as_deref() == Some("Cryptocurrency"),
    })
}

fn one_ticker(c: &Connection, filter: &str, p: impl rusqlite::Params) -> Result<Option<Ticker>, EngineError> {
    Ok(c.query_row(&format!("{TICKER_SELECT} WHERE {filter}"), p, ticker_row).optional()?)
}

/// The first allocation's ticker: this venue, that asset, the bot's quote asset (Bots::DcaMultiAsset#set_tickers).
pub fn ticker_for(c: &Connection, bot: &Bot) -> Result<Option<Ticker>, EngineError> {
    let Some(&base) = bot.asset_ids().first() else { return Ok(None) };
    ticker_for_asset(c, bot, base)
}

/// A member's ticker on the bot's venue at its quote asset, whatever its availability.
pub fn ticker_for_asset(c: &Connection, bot: &Bot, asset_id: i64) -> Result<Option<Ticker>, EngineError> {
    let Some(quote) = bot.quote_asset_id() else { return Ok(None) };
    one_ticker(c, "t.exchange_id = ?1 AND t.base_asset_id = ?2 AND t.quote_asset_id = ?3", params![bot.exchange_id, asset_id, quote])
}

/// The ticker with this id, on this venue only (bot_index_assets.ticker_id, a placement intent's ticker_id).
pub fn ticker_by_id(c: &Connection, exchange_id: i64, id: i64) -> Result<Option<Ticker>, EngineError> {
    one_ticker(c, "t.id = ?1 AND t.exchange_id = ?2", params![id, exchange_id])
}

/// The venue's ticker for a pair as the venue names it (`tickers.find_by(ticker:)` in Exchanges::*#parse_order_data).
pub fn ticker_for_pair(c: &Connection, exchange_id: i64, pair: &str) -> Result<Option<Ticker>, EngineError> {
    one_ticker(c, "t.exchange_id = ?1 AND t.ticker = ?2", params![exchange_id, pair])
}

/// Identity for Alpaca venue data with no stored asset ids. Unavailable tombstones remain identities.
pub fn alpaca_order_ticker(c: &Connection, exchange_id: i64, symbol: Option<&str>, class: Option<&str>) -> Result<Option<Ticker>, EngineError> {
    let category = match class { Some("crypto") => "Cryptocurrency", Some("us_equity") => "Stock", _ => return Ok(None) };
    let Some(symbol) = symbol else { return Ok(None) };
    let mut statement = c.prepare("SELECT t.id,t.ticker FROM tickers t JOIN assets a ON a.id=t.base_asset_id WHERE t.exchange_id=?1 AND a.category=?2")?;
    let mut found = None;
    for row in statement.query_map(params![exchange_id,category], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))? {
        let (id,native) = row?;
        let native = crate::sync::balances::position_spelling(&native);
        if native == symbol || (category == "Cryptocurrency" && native.replace('/', "") == symbol) {
            if found.is_some() { return Ok(None); }
            found = Some(id);
        }
    }
    found.map(|id| ticker_by_id(c,exchange_id,id)).transpose().map(Option::flatten)
}

pub fn exchange_name(c: &Connection, bot: &Bot) -> Result<String, EngineError> {
    let name: Option<String> = c.query_row("SELECT name FROM exchanges WHERE id = ?1", [bot.exchange_id], |r| r.get(0)).optional()?.flatten();
    Ok(name.unwrap_or_default())
}

/// The bot's exchanges.type (Exchanges::Kraken, Exchanges::Alpaca): it picks the venue and its VenueRules.
pub fn exchange_type(c: &Connection, bot: &Bot) -> Result<String, EngineError> {
    let t: Option<String> = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [bot.exchange_id], |r| r.get(0)).optional()?.flatten();
    Ok(t.unwrap_or_default())
}

/// Bot#api_key: any status, trading slot. P versions stored ciphertext, never metadata.
#[derive(Clone)]
pub struct CredentialVersion { pub id: i64, digest: [u8; 32] }
impl PartialEq for CredentialVersion {
    fn eq(&self,other:&Self)->bool { self.same_credentials(other) }
}
impl Eq for CredentialVersion {}
impl std::fmt::Debug for CredentialVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialVersion").field("id", &self.id).finish_non_exhaustive()
    }
}
impl CredentialVersion {
    pub(crate) fn same_credentials(&self, other:&Self)->bool { current_for(Some(self),Some(other)).is_fresh() }
    /// R's opaque cache provenance, stored only in Rust-owned state. Never log it.
    pub(crate) fn cache_stamp(&self) -> Value {
        serde_json::json!({"key_id": self.id, "ciphertext_digest": hex::encode(self.digest)})
    }
    pub(crate) fn from_stamp(stamp:&Value)->Option<Self> {
        let bytes=hex::decode(stamp["ciphertext_digest"].as_str()?).ok()?;
        Some(Self{id:stamp["key_id"].as_i64()?,digest:bytes.try_into().ok()?})
    }

}
/// The only credential-provenance comparison. Missing or malformed producers are stale.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum Freshness { Fresh, Stale(&'static str) }
impl Freshness { pub fn is_fresh(self)->bool { matches!(self,Self::Fresh) } }
pub fn current_for(state_digest:Option<&CredentialVersion>,current_credential_digest:Option<&CredentialVersion>)->Freshness {
    use subtle::ConstantTimeEq;
    match (state_digest,current_credential_digest) {
        (Some(state),Some(current)) if state.id==current.id && bool::from(state.digest.ct_eq(&current.digest))=>Freshness::Fresh,
        _=>Freshness::Stale("credential provenance missing or changed"),
    }
}

pub fn stamp_current_for(state:&Value,current:&Value)->Freshness {
    current_for(CredentialVersion::from_stamp(state).as_ref(),CredentialVersion::from_stamp(current).as_ref())
}
/// Identity-set adapter; every member delegates to the one digest comparison.
pub fn stamp_set_current_for(state:&Value,current:&Value)->bool {
    match (state.as_array(),current.as_array()) {
        (Some(state),Some(current)) if state.len()==current.len()=>state.iter().zip(current).all(|(a,b)|stamp_current_for(a,b).is_fresh()),
        _=>false,
    }
}
pub fn identity_current_for(state:&str,current:&str)->Freshness {
    let state=serde_json::from_str::<Value>(state).ok();
    let current=serde_json::from_str::<Value>(current).ok();
    match (state,current) {
        (Some(a),Some(b)) if a[0]==b[0]=>stamp_current_for(&a[1],&b[1]),
        _=>Freshness::Stale("credential provenance missing or changed"),
    }
}
/// A venue-derived value keeps its immutable handle producer through maps and across awaits.
#[derive(Clone,Debug)]
pub struct Produced<T> { pub value:T, origin:Option<CredentialVersion>, complete:bool }
impl<T> Produced<T> {
    pub(crate) fn new(value:T,origin:Option<CredentialVersion>)->Self { Self{value,origin,complete:true} }
    pub fn origin(&self)->&Option<CredentialVersion> { &self.origin }
    pub fn map<U>(self,f:impl FnOnce(T)->U)->Produced<U> { Produced{value:f(self.value),origin:self.origin,complete:self.complete} }
    pub(crate) fn with_completion(mut self,complete:bool)->Self {self.complete=complete;self}
    pub fn current_for(&self,current:&Option<CredentialVersion>)->Freshness {
        if !self.complete {return Freshness::Stale("incomplete venue-derived set")}
        current_for(self.origin.as_ref(),current.as_ref())
    }
}
/// Fixed column order; tagged, length-prefixed values distinguish NULL and empty values.
/// Hash the exact stored encryption envelopes. Never decrypt or log this digest.
fn ciphertext_version(id: i64, columns: &[Option<String>; 7]) -> CredentialVersion {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"deltabadger.credentials.v1");
    for value in columns {
        match value {
            None => hash.update([0]),
            Some(value) => {
                hash.update([1]);
                hash.update((value.len() as u64).to_be_bytes());
                hash.update(value.as_bytes());
            }
        }
    }
    CredentialVersion { id, digest: hash.finalize().into() }
}
/// A submitted validation handle owns these exact encryption envelopes before HTTP.
/// Zero is a private pending slot; it is never used to label persisted account state.
pub(crate) fn prepared_credential_version(id:i64,columns:&[Option<String>;7])->CredentialVersion {
    ciphertext_version(id,columns)
}
fn stored_columns(r: &rusqlite::Row<'_>) -> rusqlite::Result<[Option<String>; 7]> {
    Ok([r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?])
}
pub const CREDENTIALS_CHANGED: &str = "credentials changed; venue result discarded";

pub fn credential_version_by_id(c: &Connection, id: i64) -> Result<Option<CredentialVersion>, EngineError> {
    Ok(c.query_row("SELECT key,secret,passphrase,access_token,rsa_signature_key,rsa_encryption_key,dh_param FROM api_keys WHERE id=?1",
        [id], |r| Ok(ciphertext_version(id, &stored_columns(r)?))).optional()?)
}

/// Shared Q check. Call only inside the transaction that writes the venue result.
/// A deleted row is a mismatch too. The digest and ciphertext never enter logs.
/// Compare cache/presentation provenance without decrypting or exposing credential material.
pub fn credential_is_current(c: &Connection, version: &CredentialVersion) -> Result<bool, EngineError> {
    produced_is_current(c,&Produced::new((),Some(version.clone())))
}
/// Completeness and immutable producer are compared in the transaction consuming the set.
pub fn produced_is_current<T>(c:&Connection,value:&Produced<T>)->Result<bool,EngineError>{
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let fresh=produced_is_current(&tx,value)?;tx.commit()?;return Ok(fresh)}
    let current=match value.origin(){Some(version)=>credential_version_by_id(c,version.id)?,None=>None};
    Ok(value.current_for(&current).is_fresh())
}

/// Durable waits are explicit local decisions or results from a producing venue handle.
/// Legacy rows without either label or producer fail the common freshness check.
pub fn wait_is_current(c:&Connection,wait:&Value)->Result<bool,EngineError>{
    match wait["origin"].as_str(){
        Some("local") if wait["producer"].is_null()=>Ok(true),
        Some("venue") | None=>{
            let produced=Produced::new((),CredentialVersion::from_stamp(&wait["producer"]));
            produced_is_current(c,&produced)
        }
        _=>Ok(false),
    }
}

pub struct FencedTransaction<'a> { connection: &'a Connection, versions: Vec<CredentialVersion> }
impl FencedTransaction<'_> {
    pub(crate) fn producer_version(&self)->Option<CredentialVersion> { self.versions.first().cloned() }
    pub(crate) fn producer_stamp(&self)->Value { Value::Array(self.versions.iter().map(CredentialVersion::cache_stamp).collect()) }
}
/// Shared writer for a failure decision's immutable producer; also used by explicit fixtures.
pub fn record_failure_origin(c:&FencedTransaction<'_>,bot_id:i64)->Result<(),EngineError>{
    let stamp=c.producer_version().map_or(Value::Null,|producer|producer.cache_stamp());
    let changed=c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_failure_origin',json(?2)) WHERE id=?1 AND json_type(transient_data)='object'",params![bot_id,stamp.to_string()])?;
    if changed==0{return Err(not_an_object(bot_id))}
    Ok(())
}
impl std::ops::Deref for FencedTransaction<'_> {
    type Target=Connection;
    fn deref(&self)->&Connection { self.connection }
}
pub fn fence_versions<'a>(tx: &'a rusqlite::Transaction<'_>, versions: &[CredentialVersion]) -> Result<FencedTransaction<'a>, EngineError> {
    for version in versions { check_credential_result(tx,&Some(version.clone()))?; }
    Ok(FencedTransaction{connection:tx,versions:versions.to_vec()})
}
pub fn check_credential_result<'a>(c: &'a rusqlite::Transaction<'_>, version: &Option<CredentialVersion>) -> Result<FencedTransaction<'a>, EngineError> {
    if c.is_autocommit() { return Err(EngineError::Data("credential result check requires a write transaction".into())); }
    if let Some(version) = version {
        if !credential_is_current(c,version)? {
            super::log(CREDENTIALS_CHANGED);
            return Err(EngineError::CredentialsChanged);
        }
    }
    Ok(FencedTransaction{connection:c,versions:version.iter().cloned().collect()})
}

pub fn credential_write<T>(c: &Connection, version: &Option<CredentialVersion>, work: impl FnOnce(&FencedTransaction<'_>) -> Result<T, EngineError>) -> Result<T, EngineError> {
    let tx=immediate(c)?;
    let out=work(&check_credential_result(&tx,version)?)?;
    tx.commit()?;Ok(out)
}
pub fn credential_version(c: &Connection, bot: &Bot) -> Result<Option<CredentialVersion>, EngineError> {
    Ok(c.query_row("SELECT key,secret,passphrase,access_token,rsa_signature_key,rsa_encryption_key,dh_param,id FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=0 LIMIT 1",
        params![bot.user_id,bot.exchange_id], |r| Ok(ciphertext_version(r.get(7)?, &stored_columns(r)?))).optional()?)
}
pub fn credentials_for(c: &Connection, cipher: &Cipher, bot: &Bot) -> Result<Option<Credentials>, EngineError> {
    Ok(credentials_with_version(c,cipher,bot)?.0)
}
/// ONE read captures the encrypted version alongside the values used to build the venue.
pub fn credentials_with_version(c: &Connection, cipher: &Cipher, bot: &Bot) -> Result<(Option<Credentials>, Option<CredentialVersion>), EngineError> {
    let (credentials,version,_)=credentials_with_redaction(c,cipher,bot)?;
    Ok((credentials,version))
}

/// Diagnostic material comes from the same row snapshot as the credential digest.
pub type CredentialsWithRedaction=(Option<Credentials>,Option<CredentialVersion>,Vec<String>);
pub fn credentials_with_redaction(c:&Connection,cipher:&Cipher,bot:&Bot)->Result<CredentialsWithRedaction,EngineError>{
    type KeyRow = ([Option<String>;7], Option<String>, i64);
    let row: Option<KeyRow> = c.query_row(
        "SELECT k.key,k.secret,k.passphrase,k.access_token,k.rsa_signature_key,k.rsa_encryption_key,k.dh_param,e.type,k.id FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.exchange_id=?2 AND k.key_type=0 LIMIT 1",
        params![bot.user_id,bot.exchange_id], |r| Ok((stored_columns(r)?,r.get(7)?,r.get(8)?))).optional()?;
    let Some((columns,exchange_type,id)) = row else { return Ok((None,None,Vec::new())) };
    let version = ciphertext_version(id,&columns);
    let open = |v: &Option<String>| v.as_ref().map(|v| cipher.decrypt(v).map_err(|_| EngineError::Data(format!("api key unreadable for bot {}", bot.id)))).transpose();
    let passphrase = if exchange_type.as_deref()==Some("Exchanges::Alpaca") {open(&columns[2])?} else {None};
    let credentials = match (open(&columns[0])?,open(&columns[1])?) { (Some(key),Some(secret))=>Some(Credentials { redaction_values:vec![],key,secret,passphrase}), _=>None };
    let mut sensitive=Vec::new();
    for value in columns.iter().flatten() {
        // An unused unreadable field remains unavailable; its stored envelope is still redacted.
        let plain=match cipher.decrypt(value){Ok(plain)=>plain,Err(_)=>value.clone()};
        sensitive.push(plain);
    }
    let credentials=credentials.map(|mut credentials|{credentials.redaction_values=sensitive.clone();credentials});
    Ok((credentials,Some(version),sensitive))
}

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
pub fn locked<T>(c: &Connection, f: impl FnOnce(&Connection) -> Result<T, EngineError>) -> Result<T, EngineError> {
    if !c.is_autocommit() { return f(c); }
    let tx = immediate(c)?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

/// `json_set(transient_data, '$."k1"', json(?n), …)` for `pairs`, with its arguments numbered from `?first`. Only the
/// named keys change; every other key keeps its exact text (no whole-object rewrite, so a key another writer set
/// between the engine's read and this write is never lost).
fn set_keys(pairs: &[(&str, Value)], first: usize) -> (String, Vec<rusqlite::types::Value>) {
    let mut expr = String::from("json_set(transient_data");
    let mut args = vec![];
    for (i, (k, v)) in pairs.iter().enumerate() {
        expr.push_str(&format!(", ?{}, json(?{})", first + 2 * i, first + 2 * i + 1));
        args.push(rusqlite::types::Value::Text(format!("$.\"{k}\"")));
        args.push(rusqlite::types::Value::Text(v.to_string()));
    }
    expr.push(')');
    (expr, args)
}

fn not_an_object(bot_id: i64) -> EngineError { EngineError::Data(format!("bot {bot_id}: no such bot, or transient_data is not a JSON object")) }

/// `update!(key: value, …)` through store_accessor: nulls are stored, updated_at moves. One statement, own keys only.
pub fn update_transient(c: &Connection, bot_id: i64, pairs: &[(&str, Value)], now: DateTime<Utc>) -> Result<(), EngineError> {
    let (expr, keys) = set_keys(pairs, 3);
    let mut args = vec![rusqlite::types::Value::Integer(bot_id), rusqlite::types::Value::Text(format_time(now))];
    args.extend(keys);
    let n = c.execute(&format!("UPDATE bots SET transient_data = {expr}, updated_at = ?2 WHERE id = ?1 AND json_type(transient_data) = 'object'"),
                      rusqlite::params_from_iter(args))?;
    if n == 0 { return Err(not_an_object(bot_id)); }
    Ok(())
}

/// Bot#merge_transient_data!: `transient_data.merge(values).compact` through update_columns (updated_at stays). Its own
/// keys in one statement, then compact ONLY those keys. R5: never erase unreadable eligibility evidence
/// (including an explicit NULL merge cutoff) or another writer's state during failure bookkeeping.
pub fn merge_transient_compact(c: &Connection, bot_id: i64, pairs: &[(&str, Value)]) -> Result<(), EngineError> {
    locked(c, |c| {
        let (expr, keys) = set_keys(pairs, 2);
        let mut args = vec![rusqlite::types::Value::Integer(bot_id)];
        args.extend(keys);
        let n = c.execute(&format!("UPDATE bots SET transient_data = {expr} WHERE id = ?1 AND json_type(transient_data) = 'object'"),
                          rusqlite::params_from_iter(args))?;
        if n == 0 { return Err(not_an_object(bot_id)); }
        for (key, value) in pairs {
            if !value.is_null() { continue; }
            let path = format!("$.\"{key}\"");
            c.execute("UPDATE bots SET transient_data = json_remove(transient_data, ?2) WHERE id = ?1 AND json_type(transient_data, ?2) = 'null'",
                      params![bot_id, path])?;
        }
        Ok(())
    })
}

pub fn log_activity(c: &Connection, bot_id: i64, event: &str, level: Level, details: Value, now: DateTime<Utc>) -> Result<(), EngineError> {
    c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
              params![bot_id, event, level as i64, details.to_string(), format_time(now)])?;
    Ok(())
}

/// The bot's quote asset symbol: Bot::Fundable#funds_are_low? reads `get_balance(asset_id: quote_asset_id)`.
pub fn quote_symbol(c: &Connection, bot: &Bot) -> Result<String, EngineError> {
    let s: Option<Option<String>> = c.query_row("SELECT symbol FROM assets WHERE id = ?1", [bot.quote_asset_id()], |r| r.get(0)).optional()?;
    Ok(s.flatten().unwrap_or_default())
}

/// Exchanges::Alpaca#all_crypto?(bot.tickers) (exchanges/alpaca.rb:582-585), by the composition's asset categories:
/// - an index bot (Bots::DcaIndex#set_tickers: every ticker at its quote on the venue) is all crypto only when every such
///   ticker's base is a cryptocurrency;
/// - a basket (Bots::DcaMultiAsset#set_tickers: its allocations and every bot_index_assets row) only when it has members and
///   every member asset is a cryptocurrency.
///
/// Read from the assets, whatever their tickers' availability: a crypto bot whose ticker went untradable stays all crypto
/// (Plan 2b's `untradable_clock_closed` ruling), where Rails' empty ticker list is not.
pub fn all_crypto(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    let (members, crypto): (i64, i64) = if bot.bot_type == "Bots::DcaIndex" {
        c.query_row("SELECT count(*), coalesce(sum(a.category = 'Cryptocurrency'), 0) FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
                     WHERE t.exchange_id = ?1 AND t.quote_asset_id = ?2", params![bot.exchange_id, bot.quote_asset_id()], |r| Ok((r.get(0)?, r.get(1)?)))?
    } else {
        let mut ids = bot.asset_ids();
        let mut s = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1")?;
        for a in s.query_map([bot.id], |r| r.get::<_, i64>(0))? { let a = a?; if !ids.contains(&a) { ids.push(a); } }
        c.query_row("SELECT count(*), coalesce(sum(category = 'Cryptocurrency'), 0) FROM assets WHERE id IN (SELECT value FROM json_each(?1))",
                    [serde_json::to_string(&ids).map_err(|_| EngineError::Data("unreadable member ids".into()))?], |r| Ok((r.get(0)?, r.get(1)?)))?
    };
    Ok(members > 0 && members == crypto)
}


/// RULING-B2A-R3: one SQL boundary for eligibility, guarded writes and first-own-tick reads.
/// JSON integers and nonempty ASCII digit strings are accepted. Values beyond SQLite's
/// maximum row ID have the same boundary as i64::MAX; saturating here loses no row IDs.
/// An absent key is an ordinary bot. Explicit NULL is valid only with no inherited rows.
pub fn merged_history_cutoff(c: &Connection, bot: &Bot) -> Result<Option<i64>, EngineError> {
    let invalid = || EngineError::Data("unreadable merged history cutoff".into());
    let cutoff = match bot.transient.get("merged_history_until_id") {
        None => return Ok(None),
        Some(Value::Null) => {
            let rows: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id=?1)", [bot.id], |r| r.get(0))?;
            if rows { return Err(invalid()); }
            return Ok(None);
        }
        Some(Value::Number(n)) => n.as_u64().ok_or_else(invalid)?.min(i64::MAX as u64) as i64,
        Some(Value::String(s)) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            s.bytes().fold(0_i64, |n,b| n.saturating_mul(10).saturating_add(i64::from(b - b'0')))
        }
        _ => return Err(invalid()),
    };
    Ok(Some(cutoff))
}

#[cfg(test)]
mod r4_provenance_tests {
    use super::*;
    #[test]
    fn r4_current_for_rejects_missing_and_malformed_producers_and_map_keeps_origin(){
        let a=CredentialVersion{id:1,digest:[1;32]};let b=CredentialVersion{id:1,digest:[2;32]};
        assert!(current_for(Some(&a),Some(&a)).is_fresh());
        for (state,current) in [(None,None),(Some(&a),None),(None,Some(&a)),(Some(&a),Some(&b))]{assert!(!current_for(state,current).is_fresh(),"R4 absent or changed producer is stale");}
        assert!(!stamp_current_for(&serde_json::json!({"key_id":1,"ciphertext_digest":"bad"}),&a.cache_stamp()).is_fresh());
        let value=Produced::new(100,Some(a.clone())).map(|n|n*2);
        assert_eq!(value.value,200);assert!(value.current_for(&Some(a)).is_fresh());assert!(!value.current_for(&Some(b)).is_fresh(),"R4 map keeps original producer");
    }
    #[test]
    fn r5_incomplete_produced_values_stay_stale_through_maps(){
        let a=CredentialVersion{id:1,digest:[7;32]};
        let incomplete=Produced::new(10,Some(a.clone())).with_completion(false);
        assert!(!incomplete.current_for(&Some(a.clone())).is_fresh(),"R5 completeness is part of provenance");
        assert!(!incomplete.map(|n|n*2).current_for(&Some(a.clone())).is_fresh(),"R5 transformations cannot complete a partial set");
        assert!(Produced::new(20,Some(a.clone())).current_for(&Some(a)).is_fresh(),"R5 complete same-producer positive control");
    }

    #[test]
    fn r6_wait_origin_matrix_refuses_legacy_unknown_and_conflicting_rows(){
        use serde_json::json;
        let c=rusqlite::Connection::open_in_memory().unwrap();
        for wait in [json!({}),json!({"origin":"venue"}),json!({"origin":"unexpected"}),json!({"origin":"local","producer":{"id":1}})] {
            assert!(!wait_is_current(&c,&wait).unwrap(),"R6 unknown or conflicting wait provenance is stale: {wait}");
        }
        assert!(wait_is_current(&c,&json!({"origin":"local"})).unwrap(),"R6 explicit engine-local positive control");
    }
}
