//! AccountTransaction::SyncJob for a MEXC key (plan VB slice A5): Exchanges::Mexc#get_ledger (post-audit X8-X12) on the
//! venue continuation (sync::continuation), then AccountTransactionSync#store! and TransferMatcher.run! as ledger.rs ports
//! them. Read only: signed GETs, and the public exchange information. Rows, tx_ids, fees and raw_data are Rails' (rust/tests/
//! sync_parity.rs, the `mexc-ledger-` scenarios).
//!
//! As Rails reads it, from `start` (the watermark less 25 hours, floored to 80 days ago: #ledger_window) to the moment the
//! import began:
//! - deposits, then withdrawals (`each_transfer`): 7-day ranges from `start` (or as far back as MEXC keeps them, 89 days),
//!   each range read whole, a full page (1000) halved and both halves read, older first (`read_range`); 700007 (no
//!   permission) ends that history with what earlier ranges held; a deposit is kept at status 5 or 12, a withdrawal at 7;
//! - then the pairs to read (`get_ledger`): every symbol of the exchange information whose base the kept transfers moved
//!   or the account holds (free or locked), or whose pair the account's stored buys and sells name;
//! - then each pair's trades from `start` (or MEXC's month) to the import's start, read whole and halved as transfers are
//!   (100 a page); a pair MEXC no longer lists (SYMBOL_GONE_CODES) is skipped; repeats dropped by id;
//! - any other failed read fails the whole sync, with the venue's text.
//!
//! Across runs (plan VB §1.6): the cursor holds the source, the range being read (the halving's pending ranges included)
//! and the rows that range (or pair) read so far, which are kept only when it ends as Rails keeps them; the coins the kept
//! transfers named; and, once the walk enters its trade phase, the frozen pair list. A resumed trade phase reads exactly
//! that list.
//!
//! Where it reads more strictly than Rails (as kucoin_ledger does): an unreadable answer or amount fails the sync (Rails
//! stores a garbage amount as 0).
use super::activities::{Entry, Raw, BUY, DEPOSIT, SELL, WITHDRAWAL};
use super::binance_ledger::{amount, entry, page_of, ruby_time_at_ms, text, Fail};
use super::continuation;
use super::ledger::Outcome;
use super::wire::Node;
use super::{capture_bound_version, load_key_of, phase, Failure, Key, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::venue::bitvavo::ruby_ms;
use crate::venue::http::Transport;
use crate::venue::mexc::{MexcVenue, ACCOUNT_PATH, MEXC, RECV_WINDOW, URL};
use chrono::{DateTime, Duration, Months, Utc};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};

pub const DEPOSITS_PATH: &str = "/api/v3/capital/deposit/hisrec";
pub const WITHDRAWALS_PATH: &str = "/api/v3/capital/withdraw/history";
pub const TRADES_PATH: &str = "/api/v3/myTrades";
pub const EXCHANGE_INFO_PATH: &str = "/api/v3/exchangeInfo";
/// Exchanges::Mexc::TRANSFER_REACH, TRANSFER_RANGE and TRANSFER_PAGE.
pub const TRANSFER_REACH_DAYS: i64 = 89;
pub const TRANSFER_RANGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const TRANSFER_PAGE: usize = 1000;
/// Exchanges::Mexc::NO_PERMISSION_CODE: a key without the wallet-history permission has no transfers to give.
pub const NO_PERMISSION_CODE: i64 = 700_007;
/// Exchanges::Mexc::TRADES_PAGE: myTrades answers 100 at most.
pub const TRADES_PAGE: usize = 100;
/// Exchanges::Mexc::SYMBOL_GONE_CODES: a pair MEXC no longer lists has no trades to give.
pub const SYMBOL_GONE_CODES: [i64; 5] = [-1121, 10_007, 30_014, 30_021, 730_001];
/// Exchanges::Mexc::DEPOSIT_CREDITED: 5 SUCCESS, 12 COMPLETED.
pub const DEPOSIT_CREDITED: [i64; 2] = [5, 12];
/// A withdrawal's completed status.
pub const WITHDRAWAL_COMPLETED: i64 = 7;
/// Exchanges::Mexc#ledger_window: Rails floors an incremental start to 80 days ago.
pub const LEDGER_WINDOW_DAYS: i64 = 80;
pub const OVERLAP_HOURS: i64 = 25;
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PAGE_NODES: usize = 400_000;
/// The exchange information lists every pair MEXC has (a few thousand).
pub const MAX_INFO_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_INFO_NODES: usize = 8_000_000;
/// ponytail: Rails has no request cap; one stops a halving that never ends.
const MAX_PAGES: usize = 100_000;

/// `#to_i` of a JSON value (nil.to_i is 0; a Float truncates).
fn to_i(v: &Value) -> i64 {
    match v { Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(0, |f| f as i64)), Value::String(s) => crate::ruby::to_i(s), _ => 0 }
}
/// `x.to_d.positive?` (a value that is no decimal reads as 0).
fn positive(v: &Value) -> bool { crate::ruby::json_to_d(v).ok().flatten().is_some_and(|d| d.is_positive()) }
/// Exchange#response_field(result, 'code') of a failure's text, compared as Ruby's `==` compares a number with an Integer.
fn code_is(text: &str, codes: &[i64]) -> bool {
    serde_json::from_str::<Value>(text).ok().and_then(|v| v.get("code").and_then(Value::as_f64)).is_some_and(|c| codes.iter().any(|n| c == *n as f64))
}

/// Ruby's `inspect` of what `JSON.parse` made of a value (Hash#inspect as Ruby 3.4 writes it).
pub(crate) fn inspect(v: &Value) -> String {
    match v {
        Value::Null => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => if n.is_i64() || n.is_u64() { n.to_string() } else { crate::ruby::float_to_s(n.as_f64().unwrap_or(f64::NAN)) },
        Value::String(s) => {
            let mut out = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""), '\\' => out.push_str("\\\\"), '\n' => out.push_str("\\n"), '\t' => out.push_str("\\t"), '\r' => out.push_str("\\r"),
                    '#' => out.push('#'),
                    c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02X}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
        Value::Array(items) => format!("[{}]", items.iter().map(inspect).collect::<Vec<_>>().join(", ")),
        Value::Object(members) if members.is_empty() => "{}".into(),
        Value::Object(members) => format!("{{{}}}", members.iter().map(|(k, v)| format!("{} => {}", inspect(&Value::String(k.clone())), inspect(v))).collect::<Vec<_>>().join(", ")),
    }
}

/// `Array(x)` of a member: none is empty; a list is itself; anything else is refused.
fn list(v: Option<&Value>, what: &str) -> Result<Vec<Value>, Fail> {
    match v { None | Some(Value::Null) => Ok(vec![]), Some(Value::Array(rows)) => Ok(rows.clone()), Some(_) => Err((format!("unreadable {what}"), true)) }
}

/// `read_range`'s refusal of a 2xx that is no list: "MEXC ledger read failed: #{result.data}" (`to_s`: nil is empty, a
/// String itself, anything else its inspect).
fn not_a_list(v: &Value) -> Fail {
    let shown = match v { Value::Null => String::new(), Value::String(s) => s.clone(), other => inspect(other) };
    (format!("MEXC ledger read failed: {shown}"), false)
}

/// Which part of Exchanges::Mexc#get_ledger the walk is in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source { Deposits, Withdrawals, Pairs, Trades, Done }

impl Source {
    fn name(self) -> &'static str {
        match self { Self::Deposits => "deposits", Self::Withdrawals => "withdrawals", Self::Pairs => "pairs", Self::Trades => "trades", Self::Done => "done" }
    }
    fn named(s: &str) -> Option<Self> { [Self::Deposits, Self::Withdrawals, Self::Pairs, Self::Trades, Self::Done].into_iter().find(|v| v.name() == s) }
}

/// One deposit as #get_ledger maps it (X9): kept at status 5 or 12; its txId the tx_id.
fn deposit(raw: Raw) -> Result<Option<Entry>, Fail> {
    if !DEPOSIT_CREDITED.contains(&to_i(&raw.value["status"])) { return Ok(None); }
    let at = ruby_time_at_ms(to_i(&raw.value["insertTime"])).ok_or_else(|| ("unreadable deposit insertTime".to_string(), true))?;
    let mut e = entry(DEPOSIT, "", amount(&raw, "amount", "deposit")?, &raw, at);
    e.base_currency = raw.value["coin"].as_str().map(str::to_string);
    e.tx_id = text(raw.value.get("txId"));
    Ok(Some(e))
}

/// One withdrawal (X8): kept when completed (7); its record id the tx_id, its fee in its coin.
fn withdrawal(raw: Raw) -> Result<Option<Entry>, Fail> {
    if to_i(&raw.value["status"]) != WITHDRAWAL_COMPLETED { return Ok(None); }
    let at = ruby_time_at_ms(to_i(&raw.value["applyTime"])).ok_or_else(|| ("unreadable withdrawal applyTime".to_string(), true))?;
    let mut e = entry(WITHDRAWAL, "", amount(&raw, "amount", "withdrawal")?, &raw, at);
    e.base_currency = raw.value["coin"].as_str().map(str::to_string);
    e.fee_currency = e.base_currency.clone();
    e.fee_amount = raw.number("transactionFee").map_err(|why| (format!("withdrawal: {why}"), true))?;
    e.tx_id = text(raw.value.get("id"));
    Ok(Some(e))
}

/// One trade on `symbol` (`base`/`quote` from the exchange information): a buy when `isBuyer` is truthy, else a sell; the
/// commission's absolute value in its asset; the tx_id "SYMBOL-<orderId>-<id>".
fn trade(symbol: &str, base: &str, quote: &str, raw: Raw) -> Result<Entry, Fail> {
    let buy = !matches!(raw.value.get("isBuyer"), None | Some(Value::Null) | Some(Value::Bool(false)));
    let at = ruby_time_at_ms(to_i(&raw.value["time"])).ok_or_else(|| ("unreadable trade time".to_string(), true))?;
    let mut e = entry(if buy { BUY } else { SELL }, base, amount(&raw, "qty", "trade")?, &raw, at);
    e.quote_currency = Some(quote.to_string());
    e.quote_amount = Some(amount(&raw, "quoteQty", "trade")?);
    e.fee_currency = raw.value["commissionAsset"].as_str().map(str::to_string);
    let commission = amount(&raw, "commission", "trade")?;
    e.fee_amount = Some(if commission < crate::ruby::BigDec::zero() { &crate::ruby::BigDec::zero() - &commission } else { commission });
    let part = |k: &str| text(raw.value.get(k)).unwrap_or_default();
    e.tx_id = Some(format!("{symbol}-{}-{}", part("orderId"), part("id")));
    Ok(e)
}

/// Exchanges::Mexc#get_ledger from `transfers_from` and `trades_from` to `now_ms` (the import's first run), a
/// request at a time. `window` is the current transfer range's start; `ranges` the halving's ranges still to read, the
/// next one last; `pending` the rows (as MEXC wrote them) the current range or pair read so far; `coins` the coins the kept
/// transfers named; `pairs` the frozen pair list (symbol, base, quote), `pair` the one being read.
struct Walk<'a, T: Transport> {
    venue: &'a MexcVenue<T>, db: &'a Db, key: &'a Key, clock: &'a dyn Clock, now_ms: i64, transfers_from: i64, trades_from: i64,
    source: Source, window: i64, ranges: Vec<(i64, i64)>, pending: Vec<String>, coins: BTreeSet<String>, pairs: Vec<(String, String, String)>, pair: usize,
    requests: usize,
}

impl<'a, T: Transport> Walk<'a, T> {
    fn new(venue: &'a MexcVenue<T>, db: &'a Db, key: &'a Key, clock: &'a dyn Clock, begin: continuation::Begin<'_>) -> Self {
        let end = begin.end;
        // AccountTransactionSync's floor (#ledger_window), then #each_transfer's and #trades_reach's.
        let start = begin.start.map(|s| s.max(end - Duration::days(LEDGER_WINDOW_DAYS)));
        let reach = (end - Duration::days(TRANSFER_REACH_DAYS)).max(start.unwrap_or(DateTime::<Utc>::MIN_UTC));
        let month = end.checked_sub_months(Months::new(1)).unwrap_or(end).max(end - Duration::days(30));
        // `(...).to_i * 1000`: whole seconds, unlike the transfers' `(from.to_f * 1000).to_i`.
        let trades_from = start.map_or(month, |s| s.max(month)).timestamp() * 1000;
        let transfers_from = ruby_ms(reach);
        let mut walk = Self { venue, db, key, clock, now_ms: ruby_ms(end), transfers_from, trades_from, source: Source::Deposits,
                              window: transfers_from, ranges: vec![], pending: vec![], coins: BTreeSet::new(), pairs: vec![], pair: 0, requests: 0 };
        if let Some(c) = begin.cursor {
            if let Some(source) = c["source"].as_str().and_then(Source::named) {
                let strings = |k: &str| -> Vec<String> { c[k].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string)).collect() };
                walk.source = source;
                walk.window = c["window"].as_i64().unwrap_or(transfers_from);
                walk.ranges = c["ranges"].as_array().into_iter().flatten().filter_map(|r| Some((r[0].as_i64()?, r[1].as_i64()?))).collect();
                walk.pending = strings("pending");
                walk.coins = strings("coins").into_iter().collect();
                walk.pairs = c["pairs"].as_array().into_iter().flatten()
                    .filter_map(|p| Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string(), p[2].as_str()?.to_string()))).collect();
                walk.pair = c["pair"].as_u64().unwrap_or(0) as usize;
            }
        }
        walk
    }

    /// One signed GET (the gem's recvWindow), stamped now, its body read as honeymaker returns it: the failure's text
    /// (`Err(Ok)`), or the parsed answer.
    async fn get(&self, path: &str, mut params: Vec<(&'static str, String)>, what: &str) -> Result<Node, Result<String, Fail>> {
        params.push(("recvWindow", RECV_WINDOW.to_string()));
        let r = self.venue.signed("GET", path, params, self.clock.now());
        let text = self.venue.read_text(&r, MAX_PAGE_BYTES).await.map_err(Ok)?;
        page_of(&text, what, MAX_PAGE_NODES).map_err(Err)
    }

    fn counted(&mut self) -> Result<(), Fail> {
        self.requests += 1;
        if self.requests > MAX_PAGES { return Err(("Too many pages of MEXC history".into(), false)); }
        Ok(())
    }

    /// One request of `read_range` over the next pending range: its rows kept for later, or (a full page) the range halved,
    /// older half first. `Ok(Err(text))`: the venue's failure, for the caller's codes.
    async fn range(&mut self, path: &str, params: Vec<(&'static str, String)>, page: usize, what: &str) -> Result<Result<(), String>, Fail> {
        let (from, to) = self.ranges.pop().expect("a range to read");
        let mut query = params;
        query.extend([("startTime", from.to_string()), ("endTime", to.to_string()), ("limit", page.to_string())]);
        let node = match self.get(path, query, what).await {
            Ok(node) => node,
            Err(Ok(failure)) => return Ok(Err(failure)),
            Err(Err(fail)) => return Err(fail),
        };
        let Node::Array(rows) = &node else { return Err(not_a_list(&serde_json::from_str(&node.text()).unwrap_or(Value::Null))) };
        // ponytail: a full page inside a single millisecond is taken as is; it cannot be split further (as Rails).
        if rows.len() >= page && to - from >= 1 {
            let mid = (from + to).div_euclid(2);
            self.ranges.extend([(mid + 1, to), (from, mid)]);
        } else {
            self.pending.extend(rows.iter().map(Node::text));
        }
        Ok(Ok(()))
    }

    /// The rows the current range or pair read, as objects, and the next one begun.
    fn take_pending(&mut self, what: &str) -> Result<Vec<Raw>, Fail> {
        std::mem::take(&mut self.pending).iter().map(|t| match Raw::parse(t) { Some(Ok(raw)) => Ok(raw), _ => Err((format!("unreadable {what}"), true)) }).collect()
    }

    /// One request of #each_transfer on the current history: a range read (or halved), and when it is read whole its kept
    /// entries; 700007 ends this history.
    async fn transfers(&mut self) -> Result<Vec<Entry>, Fail> {
        let deposits = self.source == Source::Deposits;
        let next = if deposits { Source::Withdrawals } else { Source::Pairs };
        if self.ranges.is_empty() {
            if self.window >= self.now_ms { (self.source, self.window) = (next, self.transfers_from); return Ok(vec![]); }
            self.ranges.push((self.window, (self.window + TRANSFER_RANGE_MS).min(self.now_ms)));
        }
        let path = if deposits { DEPOSITS_PATH } else { WITHDRAWALS_PATH };
        if let Err(failure) = self.range(path, vec![], TRANSFER_PAGE, "MEXC transfers").await? {
            if !code_is(&failure, &[NO_PERMISSION_CODE]) { return Err((failure, false)); }
            (self.source, self.window, self.ranges, self.pending) = (next, self.transfers_from, vec![], vec![]);
            return Ok(vec![]);
        }
        if !self.ranges.is_empty() { return Ok(vec![]); }
        self.window = (self.window + TRANSFER_RANGE_MS).min(self.now_ms) + 1;
        let mut rows = vec![];
        for raw in self.take_pending("MEXC transfer")? {
            if let Some(e) = if deposits { deposit(raw)? } else { withdrawal(raw)? } {
                if let Some(c) = &e.base_currency { self.coins.insert(c.clone()); }
                rows.push(e);
            }
        }
        Ok(rows)
    }

    /// #get_ledger between the transfers and the trades: the account (its coins held free or locked) and the exchange
    /// information, then every symbol whose base the transfers or the account name, or whose pair the stored buys and sells
    /// name, in the listing's order. The list is frozen here.
    async fn pairs(&mut self) -> Result<(), Fail> {
        let account = match self.get(ACCOUNT_PATH, vec![], "MEXC account").await { Ok(node) => node, Err(Ok(failure)) => return Err((failure, false)), Err(Err(fail)) => return Err(fail) };
        let account: Value = serde_json::from_str(&account.text()).map_err(|_| ("unreadable MEXC account".to_string(), true))?;
        let Some(account) = account.as_object() else { return Err(("unreadable MEXC account".into(), true)) };
        let mut coins = self.coins.clone();
        // `Array(data['balances'])`; ponytail: a `balances` that is no list is refused (Ruby's Array() of it raises or reads nothing).
        let balances = list(account.get("balances"), "MEXC balances")?;
        for b in &balances {
            if positive(&b["free"]) || positive(&b["locked"]) { if let Some(s) = b["asset"].as_str() { coins.insert(s.to_string()); } }
        }
        let r = self.venue.request("GET", EXCHANGE_INFO_PATH, vec![]);
        let info = self.venue.read_text(&r, MAX_INFO_BYTES).await.map_err(|t| (t, false))?;
        let info = page_of(&info, "MEXC exchange information", MAX_INFO_NODES)?;
        let info: Value = serde_json::from_str(&info.text()).map_err(|_| ("unreadable MEXC exchange information".to_string(), true))?;
        // `@exchange_symbols_map[s['symbol']] = {...}`: a later listing of a symbol replaces its pair, in its first place.
        let mut listed: Vec<(String, String, String)> = vec![];
        let symbols = list(info.get("symbols"), "MEXC exchange information")?;
        for s in &symbols {
            let field = |k: &str| s[k].as_str().unwrap_or_default().to_string();
            let (symbol, base, quote) = (field("symbol"), field("baseAsset"), field("quoteAsset"));
            match listed.iter_mut().find(|(sym, _, _)| *sym == symbol) { Some(p) => *p = (symbol, base, quote), None => listed.push((symbol, base, quote)) }
        }
        let (user_id, exchange_id) = (self.key.user_id, self.key.exchange_id);
        let traded_before: HashSet<String> = phase(self.db, move |c| {
            let mut s = c.prepare("SELECT DISTINCT base_currency, quote_currency FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND entry_type IN (?3, ?4)")?;
            let pairs = s.query_map(rusqlite::params![user_id, exchange_id, BUY, SELL], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(pairs.into_iter().map(|(b, q)| format!("{}{}", b.unwrap_or_default(), q.unwrap_or_default())).collect())
        }).await.map_err(|e| (e.0, true))?;
        self.pairs = listed.into_iter().filter(|(symbol, base, _)| coins.contains(base) || traded_before.contains(symbol)).collect();
        (self.source, self.pair, self.ranges, self.pending) = (Source::Trades, 0, vec![], vec![]);
        if self.pairs.is_empty() { self.source = Source::Done; }
        Ok(())
    }

    /// One request of the current pair's trades: a range read (or halved), and when the pair is read whole its trades
    /// (each id once); a pair MEXC no longer lists is skipped.
    async fn trades(&mut self) -> Result<Vec<Entry>, Fail> {
        let (symbol, base, quote) = self.pairs[self.pair].clone();
        if self.ranges.is_empty() { self.ranges.push((self.trades_from, self.now_ms)); }
        let mut rows = vec![];
        match self.range(TRADES_PATH, vec![("symbol", symbol.clone())], TRADES_PAGE, "MEXC trades").await? {
            Err(failure) if code_is(&failure, &SYMBOL_GONE_CODES) => (self.ranges, self.pending) = (vec![], vec![]),
            Err(failure) => return Err((failure, false)),
            Ok(()) if !self.ranges.is_empty() => return Ok(vec![]),
            Ok(()) => {
                let mut seen: Vec<Value> = vec![];
                for raw in self.take_pending("MEXC trade")? {
                    let id = raw.value.get("id").cloned().unwrap_or(Value::Null);
                    if seen.contains(&id) { continue; }
                    seen.push(id);
                    rows.push(trade(&symbol, &base, &quote, raw)?);
                }
            }
        }
        self.pair += 1;
        if self.pair >= self.pairs.len() { self.source = Source::Done; }
        Ok(rows)
    }
}

impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool { self.source == Source::Done }
    fn cursor(&self) -> Value {
        json!({ "source": self.source.name(), "window": self.window, "ranges": self.ranges.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
                "pending": self.pending, "coins": self.coins, "pairs": self.pairs.iter().map(|(s, b, q)| json!([s, b, q])).collect::<Vec<_>>(),
                "pair": self.pair })
    }
    /// A page, and the rows the range or pair read before it, which the page's end may keep.
    fn page_rows(&self) -> usize { TRANSFER_PAGE + self.pending.len() }
    /// One try at the transport's whole budget (two on the pairs step).
    fn request_reserve(&self) -> std::time::Duration { crate::venue::http::TOTAL_TIMEOUT * 2 }
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        self.counted()?;
        match self.source {
            Source::Deposits | Source::Withdrawals => self.transfers().await,
            Source::Pairs => self.pairs().await.map(|()| vec![]),
            Source::Trades => self.trades().await,
            Source::Done => Ok(vec![]),
        }
    }
}

/// One ledger sync of one MEXC key, as AccountTransaction::SyncJob#perform runs it, held to the job's deadline
/// (sync::continuation): a long import goes on over several runs. Idempotent.
pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock) -> Result<Result<Outcome, Failure>, SyncError> {
    let stopwatch = continuation::Wall::start();
    let run = continuation::Run { deadline: super::jobs::LEDGER_DEADLINE, chain_runs: continuation::CHAIN_RUNS, stopwatch: &stopwatch };
    sync_within(db, transport, key_id, credentials, clock, &run).await
}

/// `sync` within `run`'s bounds (a test states a short deadline, a scripted stopwatch or a short chain).
pub async fn sync_within<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, run: &continuation::Run<'_>)
                                       -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let now = clock.now();
    let captured = version.clone();
    let (key, produced) = phase(db, move |c| {
        let key = load_key_of(c, key_id, &[MEXC])?;
        // A watermark another credential produced is read as none: the history is read whole (#511).
        let produced = super::cache::ledger_produced_by(c, key_id, &captured)?;
        Ok((key, produced))
    }).await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = MexcVenue::new(transport, URL, &credentials.secret);
    let walking = key.clone();
    continuation::sync(db, &version, key, watermark, Duration::hours(OVERLAP_HOURS), credentials, now, run, |begin| Walk::new(&venue, db, &walking, clock, begin)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_refusal_names_the_answer_as_ruby_interpolates_it() {
        assert_eq!(not_a_list(&json!({ "code": 700_003, "msg": "Timestamp \"x\"" })).0, r#"MEXC ledger read failed: {"code" => 700003, "msg" => "Timestamp \"x\""}"#);
        assert_eq!(not_a_list(&Value::Null).0, "MEXC ledger read failed: ");
        assert_eq!(not_a_list(&json!({})).0, "MEXC ledger read failed: {}");
        assert_eq!(not_a_list(&json!({ "a": [1, null, true, 1.5] })).0, r#"MEXC ledger read failed: {"a" => [1, nil, true, 1.5]}"#);
        assert!(code_is(r#"{"code":700007,"msg":"No permission to access the endpoint."}"#, &[NO_PERMISSION_CODE]));
        assert!(!code_is(r#"{"code":"700007"}"#, &[NO_PERMISSION_CODE]) && !code_is("<html/>", &[NO_PERMISSION_CODE]));
    }
}
