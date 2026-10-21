//! AccountTransaction::SyncJob for a Bitvavo key (plan VB slice A3): Exchanges::Bitvavo#get_ledger (post-audit V5/V6) on the
//! venue continuation (sync::continuation), then AccountTransactionSync#store! and TransferMatcher.run! as ledger.rs ports
//! them. Read only: signed GETs, and the public market listing. Rows, tx_ids (the trade's `id`; none for a transfer),
//! fees and raw_data are Rails' (rust/tests/sync_parity.rs, the `bitvavo-ledger-` scenarios).
//!
//! As Rails reads it:
//! - deposits, then withdrawals (`read_transfers`): newest first, 1000 a page from `start` (0 on a first sync), each page
//!   ending at the oldest timestamp of the one before plus a millisecond (a row sharing it is read again), identical rows
//!   counted per page and kept as often as the page that holds it most; a full page that brings nothing new fails. A
//!   deposit with a status other than completed, and a withdrawal not completed, are not entries;
//! - then the markets to read: every listed market whose base the kept transfers moved or the balance holds (free or in
//!   order), or whose pair the account's stored buys and sells, or its bots' orders, name (`get_ledger`);
//! - then each of those markets' trades (`read_trades`): newest first, 1000 a page before the oldest id of the one before,
//!   until a short page or one reaching past `start`; a trade before `start` is not an entry;
//! - every read paced (LEDGER_READ_PAUSE) and any failed read failing the whole sync.
//!
//! Across runs (plan VB §1.6): the walk's cursor holds where it is, the transfer counts of the source it is reading (so a
//! boundary row read again in the next run is not stored twice), the currencies the kept transfers named (`discovered`),
//! and, once the walk enters its trade phase, the frozen market list; a resumed trade phase reads exactly that list.
//!
//! Where it reads more strictly than Rails (as kucoin_ledger does): an unreadable answer or amount fails the sync (Rails
//! stores a garbage amount as 0).
use super::activities::{Entry, Raw, BUY, DEPOSIT, SELL, WITHDRAWAL};
use super::binance_ledger::{amount, entry, objects, page_of, ruby_time_at_ms, text, Fail};
use super::continuation;
use super::ledger::Outcome;
use super::{capture_bound_version, load_key_of, phase, Failure, Key, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::ruby::BigDec;
use crate::venue::bitvavo::{BitvavoVenue, BALANCE_PATH, BITVAVO, MARKETS_PATH, TRADES_PAGE, TRADES_PATH, URL};
use crate::venue::http::Transport;
use chrono::Duration;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};

pub const DEPOSITS_PATH: &str = "/v2/depositHistory";
pub const WITHDRAWALS_PATH: &str = "/v2/withdrawalHistory";
/// Exchanges::Bitvavo::TRANSFERS_PAGE_LIMIT.
const TRANSFERS_PAGE: usize = 1000;
/// Exchanges::Bitvavo::LEDGER_READ_PAUSE: 60 / 800 × 5 s, the ledger's reads weighing 5 of Bitvavo's 1000 a minute.
pub const READ_PAUSE: std::time::Duration = std::time::Duration::from_millis(375);
pub const OVERLAP_HOURS: i64 = 25;
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PAGE_NODES: usize = 400_000;
/// ponytail: Rails has no page cap; one stops a cursor that never ends.
const MAX_PAGES: usize = 100_000;

/// `#to_i` of a JSON value (nil.to_i is 0).
fn to_i(v: &Value) -> i64 { match v { Value::Number(n) => n.as_i64().unwrap_or_default(), Value::String(s) => crate::ruby::to_i(s), _ => 0 } }
fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() } }
/// `x.to_d.positive?` (a value that is no decimal reads as 0).
fn positive(v: &Value) -> bool { crate::ruby::json_to_d(v).ok().flatten().is_some_and(|d| d.is_positive()) }

/// Which part of Exchanges::Bitvavo#get_ledger the walk is in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source { Deposits, Withdrawals, Markets, Trades, Done }

impl Source {
    fn name(self) -> &'static str {
        match self { Self::Deposits => "deposits", Self::Withdrawals => "withdrawals", Self::Markets => "markets", Self::Trades => "trades", Self::Done => "done" }
    }
    fn named(s: &str) -> Option<Self> { [Self::Deposits, Self::Withdrawals, Self::Markets, Self::Trades, Self::Done].into_iter().find(|v| v.name() == s) }
}

/// One deposit as #get_ledger maps it: kept unless it has a status other than completed; the fee only when positive.
fn deposit(raw: Raw) -> Result<Option<Entry>, Fail> {
    let status = &raw.value["status"];
    // `status.present?`: a blank string, false, nil and an empty list or object are not.
    let present = match status { Value::Null => false, Value::Bool(b) => *b, Value::String(s) => !crate::ruby::blank(s), Value::Array(a) => !a.is_empty(),
                                 Value::Object(o) => !o.is_empty(), Value::Number(_) => true };
    if present && status != "completed" { return Ok(None); }
    transfer(DEPOSIT, raw, true).map(Some)
}

/// One withdrawal: kept only when completed; its fee always (a zero one is stored as none).
fn withdrawal(raw: Raw) -> Result<Option<Entry>, Fail> {
    if raw.value["status"].as_str() != Some("completed") { return Ok(None); }
    transfer(WITHDRAWAL, raw, false).map(Some)
}

fn transfer(kind: i64, raw: Raw, positive_fee_only: bool) -> Result<Entry, Fail> {
    let symbol = raw.value["symbol"].as_str().ok_or_else(|| ("unreadable transfer symbol".to_string(), true))?.to_string();
    let at = ruby_time_at_ms(to_i(&raw.value["timestamp"])).ok_or_else(|| ("unreadable transfer timestamp".to_string(), true))?;
    let fee = raw.number("fee").map_err(|why| (format!("transfer: {why}"), true))?.unwrap_or_else(BigDec::zero);
    let mut e = entry(kind, &symbol, amount(&raw, "amount", "transfer")?, &raw, at);
    if !positive_fee_only || fee.is_positive() { e.fee_currency = Some(symbol); e.fee_amount = Some(fee); }
    Ok(e)
}

/// One trade on `market`: a buy or a sell of its base for amount × price of its quote, the fee as Bitvavo states it.
fn trade(market: &str, raw: Raw) -> Result<Entry, Fail> {
    let mut parts = market.split('-');
    let (base, quote) = (parts.next().unwrap_or_default().to_string(), parts.next().map(str::to_string));
    let at = ruby_time_at_ms(to_i(&raw.value["timestamp"])).ok_or_else(|| ("unreadable trade timestamp".to_string(), true))?;
    let size = amount(&raw, "amount", "trade")?;
    let price = amount(&raw, "price", "trade")?;
    let mut e = entry(if raw.value["side"] == "buy" { BUY } else { SELL }, &base, size.clone(), &raw, at);
    e.quote_currency = quote;
    e.quote_amount = Some(&size * &price);
    e.fee_currency = raw.value["feeCurrency"].as_str().map(str::to_string);
    e.fee_amount = Some(raw.number("fee").map_err(|why| (format!("trade: {why}"), true))?.unwrap_or_else(BigDec::zero));
    e.tx_id = text(raw.value.get("id"));
    Ok(e)
}

/// Exchanges::Bitvavo#get_ledger from `start` (the watermark less the overlap; None is a full sync) to `end` (the import's
/// first run), a page at a time. `end_ms` is the next transfer page's `end`; `counts` the current transfer source's rows
/// (stored text) and the most of each one page held; `discovered` the currencies the kept transfers named; `markets` the
/// frozen market list (from the trade phase on), `market` the one being read and `trade_to` its next `tradeIdTo`.
struct Walk<'a, T: Transport> {
    venue: &'a BitvavoVenue<T>, db: &'a Db, key: &'a Key, pause: std::time::Duration, start_ms: Option<i64>, begun_ms: i64,
    source: Source, end_ms: i64, counts: Vec<(String, usize)>, discovered: BTreeSet<String>, markets: Vec<String>, market: usize,
    trade_to: Option<String>, pages: usize,
    /// The trade ids the walk read on the current market in this run (#read_trades' repeat rule).
    seen: HashSet<String>,
}

impl<'a, T: Transport> Walk<'a, T> {
    fn new(venue: &'a BitvavoVenue<T>, db: &'a Db, key: &'a Key, pause: std::time::Duration, begin: continuation::Begin<'_>) -> Self {
        // #read_transfers: `end` starts a millisecond past the moment the fetch began.
        // Both as Ruby's `(time.to_f * 1000).to_i`.
        let begun_ms = crate::venue::bitvavo::ruby_ms(begin.end) + 1;
        let mut walk = Self { venue, db, key, pause, start_ms: begin.start.map(crate::venue::bitvavo::ruby_ms), begun_ms, source: Source::Deposits, end_ms: begun_ms,
                              counts: vec![], discovered: BTreeSet::new(), markets: vec![], market: 0, trade_to: None, pages: 0, seen: HashSet::new() };
        if let Some(c) = begin.cursor {
            if let Some(source) = c["source"].as_str().and_then(Source::named) {
                walk.source = source;
                walk.end_ms = c["end_ms"].as_i64().unwrap_or(begun_ms);
                walk.counts = c["counts"].as_array().into_iter().flatten().filter_map(|p| Some((p[0].as_str()?.to_string(), p[1].as_u64()? as usize))).collect();
                walk.discovered = c["discovered"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string)).collect();
                walk.markets = c["markets"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string)).collect();
                walk.market = c["market"].as_u64().unwrap_or(0) as usize;
                walk.trade_to = c["trade_to"].as_str().map(str::to_string);
            }
        }
        walk
    }

    async fn pause(&self) { if !self.pause.is_zero() { tokio::time::sleep(self.pause).await; } }

    /// One signed GET, its pause after it (as #read_transfers and #read_trades sleep before they look at the answer), and its
    /// body read as Rails' honeymaker returns it: a failure's text, or the parsed page.
    async fn read(&self, path: &str, query: Vec<(&'static str, String)>, what: &str) -> Result<super::wire::Node, Fail> {
        let text = self.venue.read_text(path, query, MAX_PAGE_BYTES).await;
        self.pause().await;
        page_of(&text.map_err(|t| (t, false))?, what, MAX_PAGE_NODES)
    }

    fn counted(&self) -> Result<(), Fail> {
        if self.pages >= MAX_PAGES { return Err(("Too many pages of Bitvavo history".into(), false)); }
        Ok(())
    }

    /// One page of #read_transfers. The rows it holds more often than any earlier page did are the new ones, each as often as
    /// the difference (a row met again at the boundary is not new).
    async fn transfers(&mut self) -> Result<Vec<Entry>, Fail> {
        let (path, history) = if self.source == Source::Deposits { (DEPOSITS_PATH, "get_deposit_history") } else { (WITHDRAWALS_PATH, "get_withdrawal_history") };
        let query = vec![("start", self.start_ms.unwrap_or(0).to_string()), ("end", self.end_ms.to_string()), ("limit", TRANSFERS_PAGE.to_string())];
        let page = objects(&self.read(path, query, "Bitvavo transfers").await?, "Bitvavo transfer")?;
        // page.tally, in the page's order.
        let mut tally: Vec<(String, usize, Raw)> = vec![];
        for raw in &page {
            let key = raw.text();
            match tally.iter_mut().find(|(k, _, _)| *k == key) { Some(t) => t.1 += 1, None => tally.push((key, 1, raw.clone())) }
        }
        let mut fresh = 0;
        let mut rows = vec![];
        for (key, count, raw) in tally {
            let had = self.counts.iter().find(|(k, _)| *k == key).map_or(0, |(_, n)| *n);
            if had >= count { continue; }
            fresh += 1;
            match self.counts.iter_mut().find(|(k, _)| *k == key) { Some(slot) => slot.1 = count, None => self.counts.push((key, count)) }
            for _ in had..count {
                let kept = if self.source == Source::Deposits { deposit(raw.clone())? } else { withdrawal(raw.clone())? };
                if let Some(e) = kept {
                    if let Some(c) = &e.base_currency { self.discovered.insert(c.clone()); }
                    rows.push(e);
                }
            }
        }
        if page.len() < TRANSFERS_PAGE {
            (self.source, self.counts, self.end_ms) = (if self.source == Source::Deposits { Source::Withdrawals } else { Source::Markets }, vec![], self.begun_ms);
        } else if fresh == 0 {
            return Err((format!("Bitvavo {history} returned a full page with nothing new"), false));
        } else {
            self.end_ms = page.iter().map(|r| to_i(&r.value["timestamp"])).min().unwrap_or_default() + 1;
        }
        Ok(rows)
    }

    /// #get_ledger between the transfers and the trades: the balance (pause after it) and the market listing, then the
    /// markets whose base the kept transfers moved or the balance holds, or whose pair the stored buys and sells or the bots'
    /// orders name, in the listing's order. The list is frozen here.
    async fn markets(&mut self) -> Result<(), Fail> {
        let balances = self.read(BALANCE_PATH, vec![], "Bitvavo balances").await?;
        let mut coins = self.discovered.clone();
        for b in objects(&balances, "Bitvavo balance")? {
            if positive(&b.value["available"]) || positive(&b.value["inOrder"]) { if let Some(s) = b.value["symbol"].as_str() { coins.insert(s.to_string()); } }
        }
        let listing = self.venue.read_text(MARKETS_PATH, vec![], MAX_PAGE_BYTES).await.map_err(|t| (t, false))?;
        let listing = objects(&page_of(&listing, "Bitvavo markets", MAX_PAGE_NODES)?, "Bitvavo market")?;
        let (user_id, exchange_id) = (self.key.user_id, self.key.exchange_id);
        let traded_before: HashSet<String> = phase(self.db, move |c| {
            let mut s = c.prepare("SELECT DISTINCT base_currency, quote_currency FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND entry_type IN (?3, ?4)")?;
            let mut pairs: Vec<(Option<String>, Option<String>)> = s.query_map(rusqlite::params![user_id, exchange_id, BUY, SELL], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
            let mut s = c.prepare("SELECT DISTINCT t.base, t.quote FROM transactions t JOIN bots b ON b.id = t.bot_id WHERE t.exchange_id = ?1 AND b.user_id = ?2")?;
            pairs.extend(s.query_map(rusqlite::params![exchange_id, user_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?);
            Ok(pairs.into_iter().map(|(b, q)| format!("{}-{}", b.unwrap_or_default(), q.unwrap_or_default())).collect())
        }).await.map_err(|e| (e.0, true))?;
        self.markets = listing.iter().map(|m| to_s(&m.value["market"]))
            .filter(|m| coins.contains(m.split('-').next().unwrap_or_default()) || traded_before.contains(m)).collect();
        (self.source, self.market, self.trade_to) = (Source::Trades, 0, None);
        if self.markets.is_empty() { self.source = Source::Done; }
        Ok(())
    }

    /// One page of #read_trades on the current market; the next market (or the end) after its last page.
    async fn trades(&mut self) -> Result<Vec<Entry>, Fail> {
        let market = self.markets[self.market].clone();
        let mut query = vec![("market", market.clone()), ("limit", TRADES_PAGE.to_string())];
        query.extend(self.trade_to.clone().map(|c| ("tradeIdTo", c)));
        let page = objects(&self.read(TRADES_PATH, query, "Bitvavo trades").await?, "Bitvavo trade")?;
        let mut fresh = 0;
        let mut rows = vec![];
        for raw in &page {
            if !self.seen.insert(to_s(&raw.value["id"])) { continue; }
            fresh += 1;
            if self.start_ms.is_none_or(|s| to_i(&raw.value["timestamp"]) >= s) { rows.push(trade(&market, raw.clone())?); }
        }
        let oldest = page.iter().map(|r| to_i(&r.value["timestamp"])).min();
        if page.len() < TRADES_PAGE || self.start_ms.is_some_and(|s| oldest.is_some_and(|o| o < s)) {
            self.market += 1;
            (self.trade_to, self.seen) = (None, HashSet::new());
            if self.market >= self.markets.len() { self.source = Source::Done; }
        } else if fresh == 0 {
            return Err((format!("Bitvavo trades on {market} returned a full page with nothing new"), false));
        } else {
            self.trade_to = Some(to_s(&page[page.len() - 1].value["id"]));
        }
        Ok(rows)
    }
}

impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool { self.source == Source::Done }
    fn cursor(&self) -> Value {
        json!({ "source": self.source.name(), "end_ms": self.end_ms, "counts": self.counts.iter().map(|(k, n)| json!([k, n])).collect::<Vec<_>>(),
                "discovered": self.discovered, "markets": self.markets, "market": self.market, "trade_to": self.trade_to })
    }
    fn page_rows(&self) -> usize { TRANSFERS_PAGE.max(TRADES_PAGE) }
    /// One try at the transport's whole budget (two on the markets step), and the pause after each.
    fn request_reserve(&self) -> std::time::Duration { (crate::venue::http::TOTAL_TIMEOUT + self.pause) * 2 }
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        self.counted()?;
        self.pages += 1;
        match self.source {
            Source::Deposits | Source::Withdrawals => self.transfers().await,
            Source::Markets => self.markets().await.map(|()| vec![]),
            Source::Trades => self.trades().await,
            Source::Done => Ok(vec![]),
        }
    }
}

/// One ledger sync of one Bitvavo key, as AccountTransaction::SyncJob#perform runs it, held to the job's deadline
/// (sync::continuation): a long import goes on over several runs. Idempotent. `pause` is the pacing (READ_PAUSE; the parity
/// harness passes zero, as Rails' harness stubs its sleep).
pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, pause: std::time::Duration) -> Result<Result<Outcome, Failure>, SyncError> {
    let stopwatch = continuation::Wall::start();
    let run = continuation::Run { deadline: super::jobs::LEDGER_DEADLINE, chain_runs: continuation::CHAIN_RUNS, stopwatch: &stopwatch };
    sync_within(db, transport, key_id, credentials, clock, pause, &run).await
}

/// `sync` within `run`'s bounds (a test states a short deadline, a scripted stopwatch or a short chain).
pub async fn sync_within<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, pause: std::time::Duration, run: &continuation::Run<'_>)
                                       -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let now = clock.now();
    let captured = version.clone();
    let (key, produced) = phase(db, move |c| {
        let key = load_key_of(c, key_id, &[BITVAVO])?;
        // A watermark another credential produced is read as none: the history is read whole (#511).
        let produced = super::cache::ledger_produced_by(c, key_id, &captured)?;
        Ok((key, produced))
    }).await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = BitvavoVenue::new(transport, URL);
    let walking = key.clone();
    continuation::sync(db, &version, key, watermark, Duration::hours(OVERLAP_HOURS), credentials, now, run, |begin| Walk::new(&venue, db, &walking, pause, begin)).await
}
