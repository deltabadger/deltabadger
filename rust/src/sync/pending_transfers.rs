//! D-PENDING-TRANSFERS: durable nonfinal transfer observations, shared by the continuation ledgers.
//! Pending transfers do not book funds. Their oldest time holds the watermark (the Binance D2b design),
//! even if a later response omits them. Each page checkpoints observations with its rows and cursor.
//! D-MEXC-HISTORY-GAP: expired history is retained as a dated, visible warning rather than silently clipped.
use super::{activities::Entry, binance_ledger::Fail, sql_time, Key, SyncError};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::{collections::BTreeMap, sync::{Arc, Mutex}};

#[derive(Clone, Default)]
pub(super) struct State {
    pending: BTreeMap<String, i64>,
    coins: BTreeMap<String, String>,
    gaps: Vec<(i64, i64)>,
    covered: BTreeMap<String, Vec<(i64, i64)>>,
    finalized: BTreeMap<String, (String, Value)>,
}
impl State {
    fn of(v: &Value) -> Option<Self> { Some(Self { covered: serde_json::from_value(v["covered"].clone()).unwrap_or_default(), pending: serde_json::from_value(v["pending"].clone()).ok()?, coins: serde_json::from_value(v["coins"].clone()).unwrap_or_default(), gaps: serde_json::from_value(v["gaps"].clone()).ok()?, finalized: serde_json::from_value(v["finalized"].clone()).unwrap_or_default() }) }
    fn json(&self) -> Value { serde_json::json!({"covered":self.covered,"pending":self.pending,"coins":self.coins,"gaps":self.gaps,"finalized":self.finalized}) }
}
#[derive(Clone, Default)]
pub(super) struct Watch {
    state: Arc<Mutex<State>>,
    pub(super) legacy: Legacy,
    venue: String,
    fallback: i64,
    floor: Option<i64>,
}
fn name(venue: &str) -> &str { venue.strip_prefix("Exchanges::").unwrap_or(venue) }
fn source(venue: &str, kind: i64, raw: &Value, description: Option<&str>) -> &'static str {
    if kind == 5 { "withdrawals" } else if name(venue) == "Bybit" && (description == Some("Internal transfer") || (raw.get("createdTime").is_some() && raw.get("successAt").is_none())) { "internal" } else { "deposits" }
}
fn config(key: i64) -> String { format!("rust_sync.pending_transfers:{key}") }
fn string(v: &Value) -> Option<String> {
    match v { Value::String(s) if !s.trim().is_empty() => Some(s.clone()), Value::Number(n) => Some(n.to_string()), _ => None }
}
/// None means this row has no transfer state, not that an unknown state has settled.
pub(super) fn final_state(venue: &str, source: &str, raw: &Value) -> Option<bool> {
    let status = string(&raw["status"])?;
    let deposit = source == "deposits";
    let final_status = match name(venue) {
        "Mexc" if deposit => ["5","7","8","10","12"].contains(&status.as_str()),
        "Mexc" => ["7","8","9"].contains(&status.as_str()),
        "Bingx" if deposit => ["1","2","7"].contains(&status.as_str()),
        "Bingx" => ["1","3","5","6"].contains(&status.as_str()),
        "Bitrue" if deposit => status == "1",
        "Bitrue" => ["5","6"].contains(&status.as_str()),
        "Bitvavo" => ["completed","canceled"].contains(&status.as_str()),
        "Bitget" => ["success","fail","failed","cancel","cancelled"].contains(&status.as_str()),
        "Kucoin" => ["SUCCESS","FAILURE"].contains(&status.as_str()),
        "Bybit" if deposit => ["3","4","70012"].contains(&status.as_str()),
        "Bybit" if source == "internal" => ["2","3"].contains(&status.as_str()),
        "Bybit" => ["success","CancelByUser","Reject","Fail"].contains(&status.as_str()),
        _ => return None,
    };
    Some(final_status)
}
fn at(venue: &str, source: &str, raw: &Value) -> Option<i64> {
    let field = match name(venue) {
        "Mexc" | "Bingx" => if source == "deposits" { "insertTime" } else { "applyTime" },
        "Bitrue" | "Kucoin" => "createdAt", "Bitvavo" => "timestamp", "Bitget" => "cTime",
        "Bybit" => match source { "deposits" => "successAt", "internal" => "createdTime", _ => "updateTime" },
        _ => return None,
    };
    let s = string(&raw[field])?;
    let t = s.parse::<i64>().ok().filter(|n| *n > 0).or_else(|| s.parse::<DateTime<Utc>>().ok().map(|t| t.timestamp_millis()))
        .or_else(|| super::binance_ledger::apply_time(&s).map(|t| t.timestamp_millis()))?;
    Some(if name(venue) == "Bybit" && source == "internal" && t < 1_000_000_000_000 { t * 1000 } else { t })
}
fn identity(venue: &str, source: &str, raw: &Value) -> String {
    let fields: &[&str] = match name(venue) {
        "Bitget" => &["orderId"], "Bitrue" => &["id","txid"], "Kucoin" => &["id","walletTxId"],
        "Bybit" => &["id","withdrawId","txID"], _ => &["id","txId"],
    };
    if let Some(id) = fields.iter().find_map(|k| string(&raw[*k])) { return format!("{source}:{id}"); }
    anonymous(venue,source,raw)
}
fn anonymous(venue: &str, source: &str, raw: &Value) -> String {
    // Some APIs expose no record id or hash while pending. Use the immutable event fields, never its status/fee.
    format!("{source}:{}", serde_json::json!([at(venue,source,raw),raw["coin"],raw["symbol"],raw["currency"],raw["amount"],raw["size"],raw["address"]]))
}
/// One scan per sync, shared by settlement and terminal-failure cleanup. Stable venue ids are
/// independent of the hash and timestamp assigned at settlement. The map contains only pending rows.
#[derive(Clone, Default)]
pub(super) struct Legacy(Arc<BTreeMap<String, (i64, i64)>>);
fn supported(venue: &str) -> bool { ["Mexc","Bingx","Bitrue","Bitvavo","Bitget","Bybit","Kucoin"].contains(&name(venue)) }
impl Legacy {
    pub fn load(c: &Connection, key: &Key) -> Result<Self, SyncError> {
        let mut pending = BTreeMap::new();
        if !supported(&key.exchange_type) { return Ok(Self(Arc::new(pending))); }
        let mut s = c.prepare("SELECT id,entry_type,transacted_at,raw_data,description FROM account_transactions WHERE api_key_id=?1 AND entry_type IN (4,5)")?;
        for row in s.query_map([key.id],|r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?)))? {
            let (id,kind,time,raw,description) = row?;
            let Ok(raw) = serde_json::from_str::<Value>(&raw) else { continue };
            let source = source(&key.exchange_type,kind,&raw,description.as_deref());
            if final_state(&key.exchange_type,source,&raw) == Some(false) {
                if let Some(t) = at(&key.exchange_type,source,&raw).or_else(|| super::binance_ledger::apply_time(&time).map(|t| t.timestamp_millis())) {
                    pending.insert(identity(&key.exchange_type,source,&raw), (id,t));
                }
            }
        }
        Ok(Self(Arc::new(pending)))
    }
    fn row(&self, venue: &str, source: &str, raw: &Value) -> Option<i64> {
        self.0.get(&identity(venue,source,raw)).or_else(|| self.0.get(&anonymous(venue,source,raw))).map(|&(id,_)| id)
    }
}
fn merge(ranges: &mut Vec<(i64,i64)>, mut range: (i64,i64)) {
    ranges.retain(|&(a,b)| {
        if a <= range.1.saturating_add(1) && b.saturating_add(1) >= range.0 { range=(range.0.min(a),range.1.max(b)); false } else { true }
    });
    ranges.push(range);
    ranges.sort_unstable();
}
/// Subtract completed reads from the expired interval. Adjacent millisecond windows cover it continuously.
fn uncovered(mut from: i64, to: i64, covered: &[(i64,i64)]) -> Vec<(i64,i64)> {
    let mut gaps = vec![];
    for &(a,b) in covered {
        if from >= to { break; }
        if a > from.saturating_add(1) { gaps.push((from,a.min(to))); }
        from = from.max(b);
    }
    if from < to { gaps.push((from,to)); }
    gaps
}
impl Watch {
    pub fn load(c: &Connection, key: &Key, stamp: &Value, start: Option<DateTime<Utc>>, now: DateTime<Utc>, cursor: Option<&Value>) -> Result<Self, SyncError> {
        if !supported(&key.exchange_type) { return Ok(Self::default()); }
        let saved: Option<String> = c.query_row("SELECT value FROM app_configs WHERE key=?1",[config(key.id)],|r| r.get(0)).optional()?;
        let saved: Value = saved.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let mut state: State = if crate::engine::model::stamp_current_for(&saved["credential_origin"],stamp).is_fresh() {
            State::of(&saved["state"]).unwrap_or_default()
        } else { State::default() };
        if let Some(v) = cursor.and_then(|v| v.get("_pending_transfers")) { state = State::of(v).ok_or_else(|| SyncError("unreadable pending transfer checkpoint".into()))?; }
        let legacy = Legacy::load(c,key)?;
        // Resume the checkpoint's observations; otherwise discover legacy pending rows once.
        if cursor.and_then(|c| c.get("_pending_transfers")).is_none() {
            for (id,&(_,time)) in legacy.0.iter() { state.pending.entry(id.clone()).or_insert(time); }
        }
        let floor = match name(&key.exchange_type) { "Mexc" => Some(now-Duration::days(90)), "Bybit" => Some(now-Duration::days(super::bybit_ledger::HISTORY_DAYS)), _ => None }.map(|t| t.timestamp_millis());
        if let Some(floor) = floor {
            if name(&key.exchange_type) == "Mexc" {
                let from = start.map(|s| s.timestamp_millis()).or_else(|| state.covered.values().flatten().map(|&(a,_)| a).min());
                if let Some(from) = from.filter(|from| *from < floor) {
                    for source in ["deposits","withdrawals"] {
                        for gap in uncovered(from,floor,state.covered.get(source).map_or(&[],Vec::as_slice)) { merge(&mut state.gaps,gap); }
                    }
                }
            }
            if let Some(from) = state.pending.values().copied().min().filter(|from| *from < floor) { merge(&mut state.gaps,(from,floor)); }
        }
        Ok(Self { legacy,state:Arc::new(Mutex::new(state)),venue:key.exchange_type.clone(),fallback:start.unwrap_or(now).timestamp_millis(),floor })
    }
    /// Called only after all pages of a transfer window were read and normalized successfully.
    /// Coverage rides the same checkpoint as the rows, including empty windows and resumed reads.
    pub fn covered(&self, source: &str, from: i64, to: i64) {
        merge(self.state.lock().expect("pending transfer state").covered.entry(source.into()).or_default(),(from,to));
    }
    pub fn observe(&self, source: &str, raw: &Value) -> Result<(), Fail> { self.observe_from(source, raw, self.fallback) }
    pub fn observe_from(&self, source: &str, raw: &Value, fallback: i64) -> Result<(), Fail> {
        let Some(finalized) = final_state(&self.venue,source,raw) else { return Ok(()) };
        let id=identity(&self.venue,source,raw);
        let mut s=self.state.lock().expect("pending transfer state");
        if finalized {
            let had = s.pending.remove(&id).is_some();
            s.coins.remove(&id);
            s.coins.remove(&anonymous(&self.venue,source,raw));
            let anonymous = s.pending.remove(&anonymous(&self.venue,source,raw)).is_some();
            if had || anonymous { s.finalized.insert(id, (source.into(), raw.clone())); }
        }
        else {
            let t=at(&self.venue,source,raw).unwrap_or(fallback);
            if let Some(coin) = string(&raw["symbol"]).or_else(|| string(&raw["coin"])) { s.coins.insert(id.clone(), coin.split('_').next().unwrap_or(&coin).to_uppercase()); }
            s.pending.entry(id).and_modify(|old| *old=(*old).min(t)).or_insert(t);
        }
        Ok(())
    }
    pub fn coins(&self) -> Vec<String> { self.state.lock().expect("pending transfer state").coins.values().cloned().collect() }
    pub fn oldest(&self) -> Option<DateTime<Utc>> {
        self.state.lock().expect("pending transfer state").pending.values().copied().filter(|t| self.floor.is_none_or(|f| *t >= f)).min().and_then(DateTime::from_timestamp_millis)
    }
    pub fn cursor(&self, mut cursor: Value) -> Value {
        cursor["_pending_transfers"]=self.state.lock().expect("pending transfer state").json(); cursor
    }
    pub fn warning(cursor: &Value) -> Option<String> {
        let state: State=State::of(&cursor["_pending_transfers"])?;
        if state.gaps.is_empty() { return None; }
        Some(format!("Transfer history gap: the venue can no longer serve {}. Import an account statement for this interval.",state.gaps.iter().filter_map(|&(a,b)| Some(format!("{} to {}",sql_time(DateTime::from_timestamp_millis(a)?),sql_time(DateTime::from_timestamp_millis(b)?)))).collect::<Vec<_>>().join(", ")))
    }
    pub fn save(&self, c: &Connection, key: &Key, stamp: &Value, cursor: &Value, now: DateTime<Utc>) -> Result<(), SyncError> {
        let Some(mut state) = State::of(&cursor["_pending_transfers"]) else { return Ok(()) };
        // Terminal failures have no bookable Entry. Refresh a legacy row's state too, so it is not rediscovered as pending.
        for (source, raw) in state.finalized.values() {
            if let Some(stored) = self.legacy.row(&key.exchange_type,source,raw) {
                if pending_row(c,stored,&key.exchange_type,source)? {
                    c.execute("UPDATE account_transactions SET raw_data=?1,updated_at=?2 WHERE id=?3",params![raw.to_string(),sql_time(now),stored])?;
                }
            }
        }
        state.finalized.clear();
        if state.pending.is_empty() && state.gaps.is_empty() && state.covered.is_empty() { c.execute("DELETE FROM app_configs WHERE key=?1",[config(key.id)])?; return Ok(()); }
        let value=serde_json::json!({"credential_origin":stamp,"state":state.json()});
        c.execute("INSERT INTO app_configs (key,value,created_at,updated_at) VALUES (?1,?2,?3,?3) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",params![config(key.id),value.to_string(),sql_time(now)])?; Ok(())
    }
}
fn pending_row(c: &Connection, id: i64, venue: &str, source: &str) -> Result<bool,SyncError> {
    let raw: Option<String> = c.query_row("SELECT raw_data FROM account_transactions WHERE id=?1",[id],|r|r.get(0)).optional()?;
    Ok(raw.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()).is_some_and(|raw| final_state(venue,source,&raw)==Some(false)))
}
/// Only a nonfinal row with the same venue identity may be replaced; manual fields and links remain on its row.
pub(super) fn existing(c: &Connection, key: &Key, e: &Entry, legacy: &Legacy) -> Result<Option<i64>,SyncError> {
    if ![4,5].contains(&e.entry_type) { return Ok(None); }
    let source=source(&key.exchange_type,e.entry_type,&e.raw.value,e.description.as_deref());
    // Bitrue's bookable entries come from success-filtered requests; status is redundant there.
    let statusless_bitrue = name(&key.exchange_type)=="Bitrue" && e.raw.value.get("status").is_none();
    if final_state(&key.exchange_type,source,&e.raw.value) != Some(true) && !statusless_bitrue { return Ok(None); }
    if let Some(id) = legacy.row(&key.exchange_type,source,&e.raw.value) {
        if pending_row(c,id,&key.exchange_type,source)? { return Ok(Some(id)); }
    }
    Ok(None)
}
