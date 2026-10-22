//! Exchanges::Bitrue#get_ledger (T9–T11) on the shared continuation kit.
//! Trades use overlapping 89-day windows and recursively overlapping halves; transfers use offset pages.
//! Every discovery set, pending half, dedupe set and frozen pair survives a checkpoint. The production pacing is
//! 250 ms before each history request; admission reserves it. Rails starts at 2018-07-01, without a lookback clamp.
//! D-BITRUE-PAGE: a repeated full transfer page fails with its endpoint rather than Rails' undefined `path` exception.
use super::activities::{Entry, Raw, BUY, SELL, DEPOSIT, WITHDRAWAL};
use super::binance_ledger::{amount, entry, page_of, ruby_time_at_ms, text, Fail};
use super::{continuation, capture_bound_version, load_key_of, phase, Failure, Key, SyncError};
use super::ledger::Outcome;
use super::wire::Node;
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::venue::bitrue::{BitrueVenue, ACCOUNT_PATH, BITRUE, RECV_WINDOW, URL};
use crate::venue::bitvavo::ruby_ms;
use crate::venue::http::Transport;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};

pub const TRADES_PATH: &str = "/api/v2/myTrades";
pub const DEPOSITS_PATH: &str = "/api/v1/deposit/history";
pub const WITHDRAWALS_PATH: &str = "/api/v1/withdraw/history";
pub const EXCHANGE_INFO_PATH: &str = "/api/v1/exchangeInfo";
pub const PAGE: usize = 1000;
pub const WINDOW_MS: i64 = 89 * 86_400_000;
pub const READ_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_NODES: usize = 8_000_000;

fn to_i(v: &Value) -> i64 { match v { Value::String(s) => crate::ruby::to_i(s), Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or_default() as i64), _ => 0 } }
fn field(v: &Value, key: &str) -> String { text(v.get(key)).unwrap_or_default() }
fn shown(v: &Value) -> String { match v { Value::Null => String::new(), Value::String(s) => s.clone(), other => super::mexc_ledger::inspect(other) } }
fn positive(v: &Value) -> bool { crate::ruby::json_to_d(v).ok().flatten().is_some_and(|d| d.is_positive()) }
fn rows(node: &Node, member: Option<&str>) -> Result<Vec<String>, Fail> {
    let n = match member { Some(k) => match node { Node::Object(m) => m.iter().find(|(name,_)| name == k).map(|(_,v)| v), _ => None }, None => Some(node) };
    match n { Some(Node::Array(a)) => Ok(a.iter().map(Node::text).collect()), None if member.is_some() => Ok(vec![]), Some(Node::Scalar(s)) if member.is_some() && s == "null" => Ok(vec![]),
              _ => Err(("Bitrue ledger read failed: unreadable rows".into(), false)) }
}
fn raw(s: &str) -> Result<Raw, Fail> { Raw::parse(s).and_then(Result::ok).ok_or_else(|| ("unreadable Bitrue ledger row".into(), true)) }

/// Ruby's `value['key']` on the exchange listing. In particular, a top-level nil or Array raises;
/// it is not an object with an absent member. Keep the exception text for the sync's existing failure path.
fn listing_field(value: &Value, key: &str) -> Result<Value, Fail> {
    let error = match value {
        Value::Object(m) => return Ok(m.get(key).cloned().unwrap_or(Value::Null)),
        Value::String(s) => return Ok(if s.contains(key) { json!(key) } else { Value::Null }),
        Value::Array(_) => "TypeError: no implicit conversion of String into Integer".into(),
        Value::Number(n) if n.is_i64() || n.is_u64() => "TypeError: no implicit conversion of String into Integer".into(),
        Value::Number(_) => "NoMethodError: undefined method '[]' for an instance of Float".into(),
        other => format!("NoMethodError: undefined method '[]' for {}", super::mexc_ledger::inspect(other)),
    };
    Err((error, true))
}

/// Rails uses `Array(info['symbols'])`: absent/null and an empty Hash are empty, while a nonempty
/// Hash becomes key/value arrays (which raise on the string subscript below). Scalars become one row.
fn listing_pairs(node: &Node) -> Result<Vec<(String, String, String)>, Fail> {
    let value: Value = serde_json::from_str(&node.text()).map_err(|_| ("unreadable Bitrue listing".into(), true))?;
    let symbols = match listing_field(&value, "symbols")? {
        Value::Null => vec![],
        Value::Array(a) => a,
        Value::Object(m) => m.into_iter().map(|(k, v)| json!([k, v])).collect(),
        scalar => vec![scalar],
    };
    let mut listed: Vec<(String, String, String)> = vec![];
    for symbol in symbols {
        let p = (shown(&listing_field(&symbol, "symbol")?), shown(&listing_field(&symbol, "baseAsset")?).to_uppercase(),
                 shown(&listing_field(&symbol, "quoteAsset")?).to_uppercase());
        if let Some(old) = listed.iter_mut().find(|v| v.0 == p.0) { *old = p; } else { listed.push(p); }
    }
    Ok(listed)
}

fn trade(symbol: &str, base: &str, quote: &str, r: Raw) -> Result<Entry, Fail> {
    let at = ruby_time_at_ms(to_i(&r.value["time"])).ok_or_else(|| ("unreadable trade time".into(), true))?;
    let buy = !matches!(r.value.get("isBuyer"), None | Some(Value::Null | Value::Bool(false)));
    let qty = amount(&r, "qty", "trade")?;
    let quote_amount = if r.value.get("quoteQty").is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)) && v.as_str().is_none_or(|s| !s.trim().is_empty())) {
        amount(&r, "quoteQty", "trade")?
    } else { &qty * &amount(&r, "price", "trade")? };
    let mut e = entry(if buy { BUY } else { SELL }, base, qty, &r, at);
    e.quote_currency = Some(quote.into()); e.quote_amount = Some(quote_amount);
    let fee = r.value.get("commissionAsset").filter(|v| !matches!(v, Value::Null | Value::Bool(false))).or_else(|| r.value.get("commissionAssert"));
    e.fee_currency = text(fee).filter(|s| !s.trim().is_empty()).map(|s| s.to_uppercase());
    let commission = amount(&r, "commission", "trade")?;
    e.fee_amount = Some(if commission < crate::ruby::BigDec::zero() { &crate::ruby::BigDec::zero() - &commission } else { commission });
    e.tx_id = Some(format!("{symbol}-{}-{}", field(&r.value, "orderId"), field(&r.value, "id")));
    Ok(e)
}
fn transfer(r: Raw, deposit: bool) -> Result<Entry, Fail> {
    let at = ruby_time_at_ms(to_i(&r.value["createdAt"])).ok_or_else(|| ("unreadable transfer time".into(), true))?;
    let coin = field(&r.value, "symbol").split('_').next().unwrap_or_default().to_uppercase();
    let mut e = entry(if deposit { DEPOSIT } else { WITHDRAWAL }, &coin, amount(&r, "amount", "transfer")?, &r, at);
    let id = field(&r.value, "id");
    if deposit { e.tx_id = if !id.trim().is_empty() { Some(format!("deposit-{id}")) } else { text(r.value.get("txid")).filter(|s| !s.trim().is_empty()) }; }
    else {
        e.tx_id = Some(format!("withdrawal-{id}")); e.fee_currency = Some(coin);
        e.fee_amount = r.number("fee").map_err(|e| (e, true))?;
    }
    Ok(e)
}

#[derive(Clone, Copy, PartialEq)]
enum Source { Account, Listing, Trades, Coins, Deposits, Withdrawals, Done }
struct Cursor {
    source: Source, window: i64, ranges: Vec<(i64, i64)>, pending: Vec<String>, coins: BTreeSet<String>,
    pairs: Vec<(String, String, String)>, pair: usize, transfer_coins: Vec<String>, coin: usize, offset: usize, seen: HashSet<Value>,
}
impl Source {
    fn label(self) -> &'static str { match self { Self::Account => "account", Self::Listing => "listing", Self::Trades => "trades", Self::Coins => "coins", Self::Deposits => "deposits", Self::Withdrawals => "withdrawals", Self::Done => "done" } }
    fn of(s: &str) -> Option<Self> { Some(match s { "account" => Self::Account, "listing" => Self::Listing, "trades" => Self::Trades, "coins" => Self::Coins, "deposits" => Self::Deposits, "withdrawals" => Self::Withdrawals, "done" => Self::Done, _ => return None }) }
}
impl Cursor {
    fn json(&self) -> Value {
        json!({"source": self.source.label(), "window": self.window, "ranges": self.ranges, "pending": self.pending, "coins": self.coins,
               "pairs": self.pairs, "pair": self.pair, "transfer_coins": self.transfer_coins, "coin": self.coin, "offset": self.offset, "seen": self.seen})
    }
    fn of(v: &Value) -> Option<Self> {
        Some(Self { source: Source::of(v["source"].as_str()?)?, window: v["window"].as_i64()?, ranges: serde_json::from_value(v["ranges"].clone()).ok()?,
            pending: serde_json::from_value(v["pending"].clone()).ok()?, coins: serde_json::from_value(v["coins"].clone()).ok()?,
            pairs: serde_json::from_value(v["pairs"].clone()).ok()?, pair: v["pair"].as_u64()?.try_into().ok()?,
            transfer_coins: serde_json::from_value(v["transfer_coins"].clone()).ok()?, coin: v["coin"].as_u64()?.try_into().ok()?,
            offset: v["offset"].as_u64()?.try_into().ok()?, seen: serde_json::from_value(v["seen"].clone()).ok()? })
    }
}
struct Walk<'a, T: Transport> {
    venue: &'a BitrueVenue<T>, db: &'a Db, key: &'a Key, clock: &'a dyn Clock,
    start: i64, end: i64, pause: std::time::Duration, c: Cursor,
}
impl<'a, T: Transport> Walk<'a, T> {
    fn new(venue: &'a BitrueVenue<T>, db: &'a Db, key: &'a Key, clock: &'a dyn Clock, pause: std::time::Duration, begin: continuation::Begin<'_>) -> Self {
        let start = ruby_ms(begin.start.unwrap_or_else(|| "2018-07-01T00:00:00Z".parse::<chrono::DateTime<Utc>>().expect("Rails LEDGER_FROM")));
        let c = begin.cursor.and_then(Cursor::of).unwrap_or(Cursor {
            source: Source::Account, window: start, ranges: vec![], pending: vec![], coins: BTreeSet::new(), pairs: vec![], pair: 0,
            transfer_coins: vec![], coin: 0, offset: 0, seen: HashSet::new(),
        });
        Self { venue, db, key, clock, start, end: ruby_ms(begin.end), pause, c }
    }
    async fn get(&self, path: &str, mut params: Vec<(&'static str, String)>, paced: bool) -> Result<Node, Fail> {
        if paced && !self.pause.is_zero() { tokio::time::sleep(self.pause).await; }
        let r = if path == EXCHANGE_INFO_PATH { self.venue.request("GET", path, params) } else {
            params.push(("recvWindow", RECV_WINDOW.into())); self.venue.signed("GET", path, params, self.clock.now())
        };
        let answer = self.venue.read_text(&r, MAX_BYTES).await.map_err(|t| (t, false))?;
        page_of(&answer, "Bitrue ledger", MAX_NODES)
    }
    async fn account(&mut self) -> Result<Vec<Entry>, Fail> {
        let node = self.get(ACCOUNT_PATH, vec![], false).await?;
        let value: Value = serde_json::from_str(&node.text()).map_err(|_| ("unreadable Bitrue account".into(), true))?;
        if !value.is_object() { return Err(("unreadable Bitrue account".into(), true)); }
        for s in rows(&node, Some("balances"))? {
            let r = raw(&s)?;
            if positive(&r.value["free"]) || positive(&r.value["locked"]) { self.c.coins.insert(field(&r.value, "asset").to_uppercase()); }
        }
        self.c.source = Source::Listing; Ok(vec![])
    }
    async fn listing(&mut self) -> Result<Vec<Entry>, Fail> {
        let node = self.get(EXCHANGE_INFO_PATH, vec![], false).await?;
        let listed = listing_pairs(&node)?;
        let (user, exchange) = (self.key.user_id, self.key.exchange_id);
        let before: HashSet<String> = phase(self.db, move |db| {
            let mut s = db.prepare("SELECT base_currency, quote_currency FROM account_transactions WHERE user_id=?1 AND exchange_id=?2 AND entry_type IN (0,1) UNION SELECT t.base, t.quote FROM transactions t JOIN bots b ON b.id=t.bot_id WHERE b.user_id=?1 AND t.exchange_id=?2")?;
            let rows = s.query_map([user, exchange], |r| Ok(format!("{}{}", r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?.unwrap_or_default()).to_uppercase()))?;
            Ok(rows.collect::<Result<HashSet<_>, _>>()?)
        }).await.map_err(|e| (e.0, true))?;
        self.c.pairs = listed.into_iter().filter(|(s,b,_)| self.c.coins.contains(b) || before.contains(s)).collect();
        self.c.source = if self.c.pairs.is_empty() { Source::Coins } else { Source::Trades };
        Ok(vec![])
    }
    async fn trades(&mut self) -> Result<Vec<Entry>, Fail> {
        let (symbol, base, quote) = self.c.pairs[self.c.pair].clone();
        if self.c.ranges.is_empty() && self.c.window < self.end { self.c.ranges.push((self.c.window, (self.c.window + WINDOW_MS).min(self.end))); }
        if let Some((from,to)) = self.c.ranges.pop() {
            let q = vec![("symbol", symbol.clone()), ("startTime", from.to_string()), ("endTime", to.to_string()), ("limit", PAGE.to_string())];
            let page = match self.get(TRADES_PATH, q, true).await {
                Err((message, _)) if serde_json::from_str::<Value>(&message).ok().is_some_and(|v| to_i(&v["code"]) == -1121) => vec![],
                Err(e) => return Err(e),
                Ok(n) if serde_json::from_str::<Value>(&n.text()).ok().is_some_and(|v| to_i(&v["code"]) == -1121) => vec![],
                Ok(n) => {
                    if !matches!(n, Node::Array(_)) {
                        let value: Value = serde_json::from_str(&n.text()).map_err(|_| ("unreadable Bitrue trades".into(), true))?;
                        return Err((format!("Bitrue ledger read failed: {}", shown(&value)), false));
                    }
                    rows(&n, None)?
                },
            };
            if page.len() >= PAGE {
                if to-from < 2 { return Err((format!("Bitrue ledger read failed: over {PAGE} {symbol} trades in {from}..{to}"), false)); }
                let mid = (from+to)/2; self.c.ranges.extend([(mid,to),(from,mid)]);
            } else { self.c.pending.extend(page); }
            if !self.c.ranges.is_empty() { return Ok(vec![]); }
            self.c.window = (self.c.window + WINDOW_MS).min(self.end);
            if self.c.window < self.end { return Ok(vec![]); }
        }
        let mut entries = vec![];
        let mut ids = HashSet::new();
        for s in std::mem::take(&mut self.c.pending) {
            let r = raw(&s)?; let id = r.value["id"].clone();
            if !ids.insert(id) { continue; }
            let e = trade(&symbol, &base, &quote, r)?;
            self.c.coins.extend([base.clone(), quote.clone()]); entries.push(e);
        }
        self.c.pair += 1; self.c.window = self.start;
        if self.c.pair == self.c.pairs.len() { self.c.source = Source::Coins; }
        Ok(entries)
    }
    async fn coins(&mut self) -> Result<Vec<Entry>, Fail> {
        let (user, exchange) = (self.key.user_id, self.key.exchange_id);
        let stored: Vec<String> = phase(self.db, move |db| {
            let mut s = db.prepare("SELECT DISTINCT base_currency FROM account_transactions WHERE user_id=?1 AND exchange_id=?2")?;
            let rows = s.query_map([user, exchange], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }).await.map_err(|e| (e.0, true))?;
        self.c.coins.extend(stored.into_iter().map(|s| s.to_uppercase()));
        self.c.transfer_coins = self.c.coins.iter().filter(|s| !s.trim().is_empty()).cloned().collect();
        self.c.source = if self.c.transfer_coins.is_empty() { Source::Done } else { Source::Deposits }; Ok(vec![])
    }
    fn next_history(&mut self) {
        self.c.window = self.start; self.c.offset = 0; self.c.seen.clear();
        if self.c.source == Source::Deposits { self.c.source = Source::Withdrawals; }
        else { self.c.coin += 1; self.c.source = if self.c.coin == self.c.transfer_coins.len() { Source::Done } else { Source::Deposits }; }
    }
    async fn transfers(&mut self) -> Result<Vec<Entry>, Fail> {
        if self.c.window >= self.end { self.next_history(); return Ok(vec![]); }
        let deposit = self.c.source == Source::Deposits;
        let path = if deposit { DEPOSITS_PATH } else { WITHDRAWALS_PATH };
        let end = (self.c.window + WINDOW_MS).min(self.end);
        let q = vec![("coin", self.c.transfer_coins[self.c.coin].to_lowercase()), ("status", if deposit { "1" } else { "5" }.into()),
            ("startTime", self.c.window.to_string()), ("endTime", end.to_string()), ("offset", self.c.offset.to_string()), ("limit", PAGE.to_string())];
        let node = match self.get(path, q, true).await {
            Err((message, _)) if crate::engine::venue_rules::BITRUE.kinds.iter().any(|(kind, patterns)| *kind == "permission_denied" && patterns.iter().any(|p| message.contains(p))) => { self.next_history(); return Ok(vec![]); }
            other => other?,
        };
        let value: Value = serde_json::from_str(&node.text()).map_err(|_| ("unreadable Bitrue transfer".into(), true))?;
        if !value.is_object() || to_i(&value["code"]) != 200 {
            let message = if value.is_object() { format!("{} {}", field(&value,"code"), field(&value,"msg")) } else { shown(&value) };
            return Err((format!("Bitrue ledger read failed: {message}"), false));
        }
        let page = rows(&node, Some("data"))?; let count = page.len();
        let mut entries = vec![];
        for s in page {
            let r = raw(&s)?;
            if !self.c.seen.insert(r.value.clone()) { continue; }
            entries.push(transfer(r, deposit)?);
        }
        if count >= PAGE {
            if entries.is_empty() { return Err((format!("Bitrue ledger read failed: a full page of {path} brought nothing new"), false)); }
            self.c.offset += count;
        } else { self.c.window = end; self.c.offset = 0; }
        if self.c.window >= self.end { self.next_history(); }
        Ok(entries)
    }
}
impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool { self.c.source == Source::Done }
    fn cursor(&self) -> Value { self.c.json() }
    fn page_rows(&self) -> usize { PAGE + self.c.pending.len() }
    fn request_reserve(&self) -> std::time::Duration { crate::venue::http::TOTAL_TIMEOUT + self.pause }
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        match self.c.source { Source::Account => self.account().await, Source::Listing => self.listing().await, Source::Trades => self.trades().await,
            Source::Coins => self.coins().await, Source::Deposits | Source::Withdrawals => self.transfers().await, Source::Done => Ok(vec![]) }
    }
}

pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, pause: std::time::Duration) -> Result<Result<Outcome, Failure>, SyncError> {
    let watch = continuation::Wall::start();
    let run = continuation::Run { deadline: super::jobs::LEDGER_DEADLINE, chain_runs: continuation::CHAIN_RUNS, stopwatch: &watch };
    sync_within(db, transport, key_id, credentials, clock, pause, &run).await
}
pub async fn sync_within<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, pause: std::time::Duration, run: &continuation::Run<'_>) -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let captured = version.clone();
    let (key, produced) = phase(db, move |c| Ok((load_key_of(c, key_id, &[BITRUE])?, super::cache::ledger_produced_by(c, key_id, &captured)?))).await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = BitrueVenue::new(transport, URL, &credentials.secret); let walking = key.clone();
    continuation::sync(db, &version, key, watermark, Duration::hours(25), credentials, clock.now(), run,
        |begin| Walk::new(&venue, db, &walking, clock, pause, begin)).await
}
