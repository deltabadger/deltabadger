//! AccountTransaction::SyncJob for a Binance key: Exchanges::Binance#get_ledger (deposits, withdrawals, spot trades,
//! converts, fiat payments, dust, dividends, Simple Earn rewards, subscriptions and redemptions), its row mapping, then
//! AccountTransactionSync#store! and TransferMatcher.run! as ledger.rs ports them. Read only: signed GETs and the
//! public exchange information. Rows, tx_ids, group ids, fees and raw_data are Rails' to the byte (D2b ruling Q9).
//!
//! Where it reads more than Rails (D2b rulings; each a listed divergence in rust/tests/sync_parity.rs, Rails' result
//! beside Rust's, and a Rails defect to fix so the engines converge):
//! - every failed request, unreadable answer or number fails the whole sync and nothing is stored (Rails skips a failed
//!   symbol or source and stores a garbage amount as 0);
//! - trades are read per symbol by id (`fromId`: one past the largest trade id stored for it, else 0), every page of
//!   1000, so a symbol's history is whole whatever its length (Rails reads the 500 most recent, or 500 from a time);
//!   the watermark still counts what Rails' read returns, stored trades included (`rails_trades_seen`), so it moves as
//!   Rails' does;
//! - the symbols are found again every run, from Binance's exchange information: every pair whose base or quote the
//!   account ever held (this run's transfers, today's balances, every stored row); Rails re-reads only fiat-quoted pairs
//!   it already stored, so a coin/coin pair or a new pair is never read again after the first sync;
//! - transfers are walked from the watermark less 25 hours to now, in Rails' own 89-day windows, with no 80-day floor
//!   (Rails' floor drops what happened while an instance was off longer); deposits are stored at status 1 and 6
//!   (credited), and a transfer still pending holds the watermark at its time, so it is read again until it settles;
//! - every other source is walked in windows Binance accepts from that same start (or from the source's opening on a
//!   full sync: Rails asks a full sync's fiat, dust, dividend and earn sources with no start, and Binance answers only
//!   its recent days), each window paged where the endpoint pages; a window that fills an unpageable answer fails the
//!   sync (Rails keeps the first page: 100 converts, 500 dividends, 100 dust sweeps);
//! - withdrawal history (18000 of the 180000-a-minute account weight), converts (3000 account weight) and Simple Earn
//!   (150 IP weight) are paced, as are the trade lists; Rails sends everything back to back;
//! - flexible earn rewards are asked once per reward type, which Binance requires (Rails sends none, is refused, and
//!   stores no flexible reward);
//! - a Simple Earn row without Rails' amount field (`rewards`, `purchaseAmount`) is read from the `amount` Binance
//!   documents for locked rewards, subscriptions and redemptions (Rails stores 0 for those);
//! - an applyTime is read as UTC (Rails reads it in the process's zone);
//! - margin and futures are not read (ruling Q11); the key carries no note of it: `last_sync_error` is the tracker's
//!   "Sync failed" banner, so a run that completes clears it, as Rails' does;
//! - a Binance.US key's ledger is not read (Rails reads it from api.binance.com, which refuses the key): the run sends
//!   nothing, stores nothing, and completes (clearing the key's error), so the tracker states the venue's balances.
use super::activities::{Entry, Raw};
use super::ledger::{self, Outcome, Progress};
use super::wire::{self, Budget, Node};
use super::{capture_bound_version, load_key_of, phase, record_sync_error, sql_time, Failure, Key, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::ruby::BigDec;
use crate::venue::binance::{self, BinanceVenue, BINANCE, BINANCE_US, READ_RECV_WINDOW};
use crate::venue::http::Transport;
use chrono::{DateTime, Duration, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

pub const DEPOSITS_PATH: &str = "/sapi/v1/capital/deposit/hisrec";
pub const WITHDRAWALS_PATH: &str = "/sapi/v1/capital/withdraw/history";
pub const TRADES_PATH: &str = "/api/v3/myTrades";
pub const EXCHANGE_INFO_PATH: &str = "/api/v3/exchangeInfo";
pub const ACCOUNT_PATH: &str = "/api/v3/account";
/// Exchanges::Binance::TRANSFER_PAGE, and Binance's maximum for myTrades.
pub const PAGE: usize = 1000;
/// Exchanges::Binance#each_window's span for transfers (Binance refuses 90 days or more).
pub const WINDOW_DAYS: i64 = 89;
/// TRANSFER_HISTORY_FROM: Binance opened in July 2017.
pub const HISTORY_FROM_MS: i64 = 1_498_867_200_000; // 2017-07-01T00:00:00Z
pub const OVERLAP_HOURS: i64 = 25;
/// Exchanges::Binance#ledger_window: Rails floors an incremental start to 80 days ago.
pub const LEDGER_WINDOW_DAYS: i64 = 80;
/// What one myTrades answer holds when Rails sends no limit.
pub const RAILS_TRADES: usize = 500;
/// What one answer may hold. The exchange information lists every symbol Binance ever had (a few thousand).
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_INFO_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PAGE_NODES: usize = 400_000;
pub const MAX_INFO_NODES: usize = 8_000_000;
/// What one run may read in all; past it the sync fails, nothing stored.
pub const MAX_ENTRIES: usize = 500_000;
/// Ledger rows per write transaction (ledger::BATCH).
const BATCH: usize = 100;

/// Exchanges::Binance::FIAT_CURRENCIES: a trade quoted in one of these, or a dollar stablecoin, is a buy or a sell;
/// any other is a swap pair.
pub const FIAT_CURRENCIES: [&str; 21] = ["USD", "EUR", "GBP", "AUD", "CAD", "JPY", "CHF", "TRY", "BRL", "PLN", "UAH", "CZK", "SEK", "NOK", "DKK", "HUF", "RON", "BGN", "ZAR", "NGN", "KES"];
fn fiat_like(quote: &str) -> bool { FIAT_CURRENCIES.contains(&quote) || ["USD", "USDT", "USDC", "BUSD", "DAI", "FDUSD"].contains(&quote) }

const BUY: i64 = 0;
const SELL: i64 = 1;
const SWAP_IN: i64 = 2;
const SWAP_OUT: i64 = 3;
const DEPOSIT: i64 = 4;
const WITHDRAWAL: i64 = 5;
const STAKING_REWARD: i64 = 6;
const AIRDROP: i64 = 8;

pub const CONVERT_PATH: &str = "/sapi/v1/convert/tradeFlow";
pub const FIAT_PATH: &str = "/sapi/v1/fiat/payments";
pub const DUST_PATH: &str = "/sapi/v1/asset/dribblet";
pub const DIVIDEND_PATH: &str = "/sapi/v1/asset/assetDividend";
/// CONVERT_HISTORY_FROM: Convert opened in 2020.
pub const CONVERT_FROM_MS: i64 = 1_577_836_800_000; // 2020-01-01T00:00:00Z
/// DustLog keeps only records after 2020-12-01.
pub const DUST_FROM_MS: i64 = 1_606_780_800_000; // 2020-12-01T00:00:00Z
/// Simple Earn's history endpoints opened in 2023; nothing older is served by them.
pub const EARN_FROM_MS: i64 = 1_672_531_200_000; // 2023-01-01T00:00:00Z
/// The longest window each source accepts: converts and Simple Earn 30 days, dividends 180. Fiat payments and dust
/// document none; they are asked in the transfers' 89 days.
const CONVERT_DAYS: i64 = 30;
const EARN_DAYS: i64 = 30;
/// Flexible rewards are asked once per reward type: Binance refuses the call without one (-1102, "Mandatory parameter
/// 'type'"), though its docs say it defaults to ALL. Rails sends none, so Binance refuses Rails' every call, and Rails
/// skips the source in silence.
const FLEXIBLE_REWARD_TYPES: [&str; 3] = ["BONUS", "REALTIME", "REWARDS"];
const DIVIDEND_DAYS: i64 = 179;
/// What one answer of an unpageable source holds at most: past it the window may be cut short.
const CONVERT_LIMIT: usize = 1000;
const DIVIDEND_LIMIT: usize = 500;
const DUST_LIMIT: usize = 100;
const FIAT_ROWS: usize = 500;
const EARN_SIZE: usize = 100;
/// #import_earn_rewards and #import_earn_subscriptions: path, entry type, tx_id prefix and the amount's field.
const EARN: [(&str, i64, &str, &str); 6] = [
    ("/sapi/v1/simple-earn/flexible/history/rewardsRecord", STAKING_REWARD, "flex-reward", "rewards"),
    ("/sapi/v1/simple-earn/locked/history/rewardsRecord", STAKING_REWARD, "lock-reward", "rewards"),
    ("/sapi/v1/simple-earn/flexible/history/subscriptionRecord", WITHDRAWAL, "flex-sub", "purchaseAmount"),
    ("/sapi/v1/simple-earn/flexible/history/redemptionRecord", DEPOSIT, "flex-redeem", "amount"),
    ("/sapi/v1/simple-earn/locked/history/subscriptionRecord", WITHDRAWAL, "lock-sub", "purchaseAmount"),
    ("/sapi/v1/simple-earn/locked/history/redemptionRecord", DEPOSIT, "lock-redeem", "amount"),
];

/// How far apart the calls go. LIVE keeps withdrawal history under Binance's account weight (10 a minute) and the
/// trade lists at about a third of the IP weight the bots share.
#[derive(Clone, Copy, Debug)]
pub struct Pacing { pub withdrawals: std::time::Duration, pub trades: std::time::Duration, pub converts: std::time::Duration, pub earn: std::time::Duration }
impl Pacing {
    /// Withdrawals at 10 a minute (account weight 18000 of 180000), trades at 4000 of the 6000 IP weight a minute (20
    /// each; a full sync reads every pair of every coin held, a few thousand), converts at 40 a minute (3000 account
    /// weight each), Simple Earn at 30 a minute (150 IP weight each, 4500 of the 6000).
    pub const LIVE: Pacing = Pacing { withdrawals: std::time::Duration::from_secs(6), trades: std::time::Duration::from_millis(300),
                                      converts: std::time::Duration::from_millis(1500), earn: std::time::Duration::from_secs(2) };
    pub const NONE: Pacing = Pacing { withdrawals: std::time::Duration::ZERO, trades: std::time::Duration::ZERO, converts: std::time::Duration::ZERO, earn: std::time::Duration::ZERO };
}

/// `Time.at(ms / 1000.0)` as Rails stores it: the Float quotient, its microseconds truncated (ActiveRecord keeps six
/// digits of the exact value of that double, so ...123 ms is stored as .122999).
pub fn ruby_time_at_ms(ms: i64) -> Option<DateTime<Utc>> {
    let f = ms as f64 / 1000.0;
    if !f.is_finite() { return None; }
    let bits = f.to_bits();
    let negative = bits >> 63 == 1;
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = (bits & ((1u64 << 52) - 1)) as i128;
    let (mantissa, exp) = if exponent == 0 { (fraction, -1074) } else { (fraction | (1i128 << 52), exponent - 1075) };
    // value = ±mantissa × 2^exp; the microseconds are floor(value × 10^6).
    let scaled = mantissa.checked_mul(1_000_000)?;
    let micros = if exp >= 0 { scaled.checked_shl(exp as u32)? } else if exp > -127 { let q = scaled >> (-exp); if negative && (q << (-exp)) != scaled { q + 1 } else { q } } else { 0 };
    let micros = if negative { -micros } else { micros };
    DateTime::<Utc>::from_timestamp_micros(i64::try_from(micros).ok()?)
}

/// An applyTime ("YYYY-MM-DD HH:MM:SS"), as UTC (D2b ruling Q8).
pub fn apply_time(text: &str) -> Option<DateTime<Utc>> {
    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").ok().map(|t| Utc.from_utc_datetime(&t))
}

fn ms(t: DateTime<Utc>) -> i64 { t.timestamp_millis() }

/// Each_window's [start, end] pairs: abutting, at most WINDOW_DAYS long, from `start` up to `end`, pinned once.
pub fn windows(start: i64, end: i64, days: i64) -> Vec<(i64, i64)> {
    let span = days * 24 * 60 * 60 * 1000;
    let mut out = vec![];
    let mut from = start;
    while from < end { let to = (from + span).min(end); out.push((from, to)); from = to; }
    out
}

/// A read that failed: the text the key records, and whether Rails would have raised (a malformed answer) rather
/// than returned the venue's failure.
type Fail = (String, bool);

fn page_of(text: &str, what: &str, nodes: usize) -> Result<Node, Fail> {
    match wire::read(text, &mut Budget(nodes), None) {
        Ok(node) => Ok(node),
        // Not JSON: honeymaker's failure is the body itself.
        Err(wire::Refused::NotJson | wire::Refused::TooDeep) => Err((text.to_string(), false)),
        Err(refused) => Err((refused.text(what), true)),
    }
}

fn objects(node: &Node, what: &str) -> Result<Vec<Raw>, Fail> {
    let Node::Array(items) = node else { return Err((format!("unreadable {what}"), true)) };
    items.iter().map(|item| Raw::from_node(item).map_err(|_| (format!("unreadable {what}"), true))).collect()
}

/// An integer field (a time in milliseconds, an id).
fn integer(raw: &Raw, key: &str, what: &str) -> Result<i64, Fail> {
    raw.value.get(key).and_then(Value::as_i64).ok_or_else(|| (format!("unreadable {what} {key}"), true))
}

/// `field.to_d`, strictly: a number the venue wrote, never a missing or garbage value read as 0 (D2b ruling Q10).
fn amount(raw: &Raw, key: &str, what: &str) -> Result<BigDec, Fail> {
    match raw.number(key) {
        Ok(Some(n)) => Ok(n),
        Ok(None) => Err((format!("unreadable {what} {key}: missing"), true)),
        Err(why) => Err((format!("{what}: {why}"), true)),
    }
}

/// A JSON value as Rails would store it in a string column (`tx_id`): a string as it is, a number as its digits.
fn text(v: Option<&Value>) -> Option<String> {
    match v { Some(Value::String(s)) => Some(s.clone()), Some(Value::Number(n)) => Some(n.to_string()), _ => None }
}

fn entry(entry_type: i64, base: &str, base_amount: BigDec, raw: &Raw, at: DateTime<Utc>) -> Entry {
    Entry { entry_type, base_currency: Some(base.to_string()), base_amount, quote_currency: None, quote_amount: None, fee_currency: None, fee_amount: None,
            tx_id: None, group_id: None, description: None, transacted_at: Some(at), raw: raw.clone() }
}

/// What the transfers held: entries, coins named, and the earliest time of a transfer still pending.
#[derive(Default)]
struct Transfers { entries: Vec<Entry>, coins: BTreeSet<String>, pending: Option<DateTime<Utc>> }

/// One deposit: #get_ledger's deposit entry at status 1, and at 6 (credited); pending at 0 and 8.
fn deposit(raw: &Raw, out: &mut Transfers) -> Result<(), Fail> {
    let status = raw.value.get("status").and_then(Value::as_i64).ok_or_else(|| ("unreadable deposit status".to_string(), true))?;
    let at = ruby_time_at_ms(integer(raw, "insertTime", "deposit")?).ok_or_else(|| ("unreadable deposit insertTime".to_string(), true))?;
    match status {
        1 | 6 => {
            let coin = raw.value["coin"].as_str().ok_or_else(|| ("unreadable deposit coin".to_string(), true))?;
            let mut e = entry(DEPOSIT, coin, amount(raw, "amount", "deposit")?, raw, at);
            e.tx_id = text(raw.value.get("txId"));
            out.coins.insert(coin.to_string());
            out.entries.push(e);
        }
        0 | 8 => out.pending = Some(out.pending.map_or(at, |p| p.min(at))),
        _ => {}
    }
    Ok(())
}

/// One withdrawal: #get_ledger's withdrawal entry at status 6; pending at 0, 2 and 4.
fn withdrawal(raw: &Raw, out: &mut Transfers) -> Result<(), Fail> {
    let status = raw.value.get("status").and_then(Value::as_i64).ok_or_else(|| ("unreadable withdrawal status".to_string(), true))?;
    let at = raw.value["applyTime"].as_str().and_then(apply_time).ok_or_else(|| ("unreadable withdrawal applyTime".to_string(), true))?;
    match status {
        6 => {
            let coin = raw.value["coin"].as_str().ok_or_else(|| ("unreadable withdrawal coin".to_string(), true))?;
            let mut e = entry(WITHDRAWAL, coin, amount(raw, "amount", "withdrawal")?, raw, at);
            // `wd['transactionFee']&.to_d`: absent is no fee.
            e.fee_amount = raw.number("transactionFee").map_err(|why| (format!("withdrawal: {why}"), true))?;
            e.fee_currency = Some(coin.to_string());
            e.tx_id = text(raw.value.get("txId")).or_else(|| text(raw.value.get("id")));
            out.coins.insert(coin.to_string());
            out.entries.push(e);
        }
        0 | 2 | 4 => out.pending = Some(out.pending.map_or(at, |p| p.min(at))),
        _ => {}
    }
    Ok(())
}

/// #normalize_trade: a fiat-quoted trade is one buy or sell row; any other is a swap pair, the received leg carrying the
/// commission. Ids and group ids are Rails' to the byte (D2b ruling Q9): "SYMBOL-orderId-id", "-in" the base leg.
pub fn normalize_trade(raw: &Raw, symbol: &str, base: &str, quote: &str) -> Result<Vec<Entry>, Fail> {
    let buyer = !matches!(raw.value.get("isBuyer"), None | Some(Value::Null) | Some(Value::Bool(false)));
    let id = text(raw.value.get("id")).ok_or_else(|| ("unreadable trade id".to_string(), true))?;
    let order = text(raw.value.get("orderId")).ok_or_else(|| ("unreadable trade orderId".to_string(), true))?;
    let trade_id = format!("{symbol}-{order}-{id}");
    let at = ruby_time_at_ms(integer(raw, "time", "trade")?).ok_or_else(|| ("unreadable trade time".to_string(), true))?;
    let (qty, quote_qty, commission) = (amount(raw, "qty", "trade")?, amount(raw, "quoteQty", "trade")?, amount(raw, "commission", "trade")?);
    let commission_asset = raw.value.get("commissionAsset").and_then(Value::as_str).map(str::to_string);
    if fiat_like(quote) {
        let mut e = entry(if buyer { BUY } else { SELL }, base, qty, raw, at);
        e.quote_currency = Some(quote.to_string());
        e.quote_amount = Some(quote_qty);
        e.fee_currency = commission_asset;
        e.fee_amount = Some(commission);
        e.tx_id = Some(trade_id);
        return Ok(vec![e]);
    }
    let group = Some(format!("swap_{trade_id}"));
    let mut base_leg = entry(if buyer { SWAP_IN } else { SWAP_OUT }, base, qty, raw, at);
    base_leg.tx_id = Some(format!("{trade_id}-in"));
    base_leg.group_id = group.clone();
    let mut quote_leg = entry(if buyer { SWAP_OUT } else { SWAP_IN }, quote, quote_qty, raw, at);
    quote_leg.tx_id = Some(format!("{trade_id}-out"));
    quote_leg.group_id = group;
    let fee_leg = if buyer { &mut base_leg } else { &mut quote_leg };
    fee_leg.fee_currency = commission_asset;
    fee_leg.fee_amount = Some(commission);
    Ok(vec![base_leg, quote_leg])
}

/// The largest trade id stored per symbol, read from Rails' own tx_ids ("SYMBOL-orderId-id", a swap leg with "-in" or
/// "-out" after it). A tx_id of any other shape (a file import's) is not one.
pub fn trade_cursor(tx_id: &str) -> Option<(String, i64)> {
    let mut parts = tx_id.split('-');
    let (symbol, order, id) = (parts.next()?, parts.next()?, parts.next()?);
    if !matches!(parts.next(), None | Some("in") | Some("out")) || parts.next().is_some() { return None; }
    if symbol.is_empty() || !symbol.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) || order.parse::<i64>().is_err() { return None; }
    Some((symbol.to_string(), id.parse().ok()?))
}

/// What the run starts from, read in one phase.
/// `stored`: every stored trade per symbol (id, time); `pairs`: Exchanges::Binance#known_traded_symbols, the pairs Rails
/// re-reads on an incremental sync (base and quote of a stored trade row with a quote).
struct Start { key: Key, watermark: Option<DateTime<Utc>>, raw_watermark: Option<i64>, coins: BTreeSet<String>, cursors: HashMap<String, i64>,
               stored: HashMap<String, Vec<(i64, DateTime<Utc>)>>, pairs: HashSet<String> }

fn start(c: &Connection, key_id: i64, version: &crate::engine::model::CredentialVersion) -> Result<Start, SyncError> {
    let key = load_key_of(c, key_id, &[BINANCE, BINANCE_US])?;
    let raw_watermark = key.last_synced_at.map(|t| t.timestamp_micros());
    // A watermark another credential produced is read as none: the history is read whole (#511).
    let watermark = if super::cache::ledger_produced_by(c, key_id, version)? { key.last_synced_at } else { None };
    let mut coins = BTreeSet::new();
    let mut cursors: HashMap<String, i64> = HashMap::new();
    let mut stored: HashMap<String, Vec<(i64, DateTime<Utc>)>> = HashMap::new();
    let mut pairs = HashSet::new();
    let mut s = c.prepare("SELECT base_currency, quote_currency, fee_currency, tx_id, entry_type, transacted_at FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2")?;
    let mut rows = s.query(params![key.user_id, key.exchange_id])?;
    while let Some(r) = rows.next()? {
        for i in 0..3 { if let Some(coin) = r.get::<_, Option<String>>(i)? { coins.insert(coin); } }
        if ![BUY, SELL, SWAP_IN, SWAP_OUT].contains(&r.get::<_, i64>(4)?) { continue; }
        if let (Some(base), Some(quote)) = (r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?) { pairs.insert(format!("{base}{quote}")); }
        if let Some((symbol, id)) = r.get::<_, Option<String>>(3)?.as_deref().and_then(trade_cursor) {
            let max = cursors.entry(symbol.clone()).or_insert(id);
            *max = (*max).max(id);
            if let Some(at) = r.get::<_, Option<String>>(5)? { stored.entry(symbol).or_default().push((id, crate::codec::parse_time(&at)?)); }
        }
    }
    drop(rows);
    Ok(Start { key, watermark, raw_watermark, coins, cursors, stored, pairs })
}

struct Symbol { base: String, quote: String, trading: bool }

/// GET /api/v3/exchangeInfo: every symbol with its base, quote and whether it trades.
fn symbols(text: &str) -> Result<HashMap<String, Symbol>, Fail> {
    let node = page_of(text, "the exchange information", MAX_INFO_NODES)?;
    let info = Raw::from_node(&node).map_err(|_| ("unreadable exchange information".to_string(), true))?;
    let list = info.member("symbols").ok_or_else(|| ("unreadable exchange information".to_string(), true))?;
    let mut out = HashMap::new();
    for s in objects(list, "exchange information symbol")? {
        let field = |k: &str| s.value.get(k).and_then(Value::as_str).map(str::to_string).ok_or_else(|| (format!("unreadable exchange information {k}"), true));
        out.insert(field("symbol")?, Symbol { base: field("baseAsset")?, quote: field("quoteAsset")?, trading: s.value["status"] == "TRADING" });
    }
    Ok(out)
}

/// GET /api/v3/account: the coins held now (free or locked above zero).
fn held(text: &str) -> Result<Vec<String>, Fail> {
    let node = page_of(text, "an account", MAX_PAGE_NODES)?;
    let account = Raw::from_node(&node).map_err(|_| ("unreadable account".to_string(), true))?;
    let list = account.member("balances").ok_or_else(|| ("KeyError: Key path balances not found".to_string(), true))?;
    let mut out = vec![];
    for b in objects(list, "balance")? {
        let Some(asset) = b.value.get("asset").and_then(Value::as_str) else { continue };
        if (&amount(&b, "free", "balance")? + &amount(&b, "locked", "balance")?).is_positive() { out.push(asset.to_string()); }
    }
    Ok(out)
}

/// `seen`: the newest row time Rails' read of the same venue returns (AccountTransactionSync's `max_seen`).
struct Fetched { entries: Vec<Entry>, pending: Option<DateTime<Utc>>, seen: Option<DateTime<Utc>> }

/// Every read of the run; no database handle in reach.
async fn fetch<T: Transport>(venue: &BinanceVenue<T>, clock: &dyn Clock, pacing: Pacing, from: i64, start: &Start, full: bool) -> Result<Fetched, Fail> {
    let now = clock.now();
    let mut t = Transfers::default();
    let parse = |text: String, what: &'static str| async move {
        tokio::task::spawn_blocking(move || page_of(&text, what, MAX_PAGE_NODES).and_then(|node| objects(&node, what)))
            .await.unwrap_or_else(|_| Err(("reading the venue's answer was interrupted".into(), true)))
    };
    let mut withdrawal_calls = 0;
    for (window_start, window_end) in windows(from, ms(now), WINDOW_DAYS) {
        for (path, what) in [(DEPOSITS_PATH, "deposit"), (WITHDRAWALS_PATH, "withdrawal")] {
            let mut offset = 0;
            loop {
                if path == WITHDRAWALS_PATH { if withdrawal_calls > 0 { tokio::time::sleep(pacing.withdrawals).await; } withdrawal_calls += 1; }
                let query = vec![("startTime", window_start.to_string()), ("endTime", window_end.to_string()), ("offset", offset.to_string()),
                                 ("limit", PAGE.to_string()), ("recvWindow", READ_RECV_WINDOW.to_string())];
                let rows = parse(venue.signed_get(path, query, clock.now(), MAX_PAGE_BYTES).await.map_err(|e| (e, false))?, what).await?;
                for raw in &rows { if path == DEPOSITS_PATH { deposit(raw, &mut t)?; } else { withdrawal(raw, &mut t)?; } }
                if t.entries.len() > MAX_ENTRIES { return Err((format!("more than {MAX_ENTRIES} ledger entries in one run"), true)); }
                if rows.len() < PAGE { break; }
                offset += PAGE;
            }
        }
    }
    // The symbols, found again every run.
    let info = venue.public_get(EXCHANGE_INFO_PATH, vec![("showPermissionSets", "false".to_string())], MAX_INFO_BYTES).await.map_err(|e| (e, false))?;
    let all = tokio::task::spawn_blocking(move || symbols(&info)).await.unwrap_or_else(|_| Err(("reading the venue's answer was interrupted".into(), true)))?;
    let account = venue.signed_get(ACCOUNT_PATH, vec![("omitZeroBalances", "true".to_string()), ("recvWindow", READ_RECV_WINDOW.to_string())], clock.now(), MAX_PAGE_BYTES)
        .await.map_err(|e| (e, false))?;
    let mut coins = start.coins.clone();
    coins.extend(t.coins.iter().cloned());
    let balances = tokio::task::spawn_blocking(move || held(&account)).await.unwrap_or_else(|_| Err(("reading the venue's answer was interrupted".into(), true)))?;
    coins.extend(balances.iter().cloned());
    let list: Vec<(&str, &str, &str, bool)> = all.iter().map(|(n, s)| (n.as_str(), s.base.as_str(), s.quote.as_str(), s.trading)).collect();
    let held: HashSet<&str> = coins.iter().map(String::as_str).collect();
    let stored: HashSet<&str> = start.cursors.keys().map(String::as_str).collect();
    let candidates = candidate_symbols(&list, &held, &stored, full);
    let mut seen = t.entries.iter().filter_map(|e| e.transacted_at).max();
    // Rails' trade read starts at the floored start (an incremental sync), or reads the latest page (a full one).
    let rails_from = (!full).then(|| from.max(ms(now - Duration::days(LEDGER_WINDOW_DAYS))));
    let mut entries = t.entries;
    let mut calls = 0;
    for name in &candidates {
        let s = &all[name];
        let mut read: Vec<(i64, DateTime<Utc>)> = vec![];
        let mut from_id = start.cursors.get(name).map_or(0, |id| id + 1);
        loop {
            if calls > 0 { tokio::time::sleep(pacing.trades).await; }
            calls += 1;
            let query = vec![("symbol", name.clone()), ("fromId", from_id.to_string()), ("limit", PAGE.to_string()), ("recvWindow", READ_RECV_WINDOW.to_string())];
            let trades = parse(venue.signed_get(TRADES_PATH, query, clock.now(), MAX_PAGE_BYTES).await.map_err(|e| (e, false))?, "trade").await?;
            let mut last = None;
            for raw in &trades {
                let id = integer(raw, "id", "trade")?;
                if id < from_id || last.is_some_and(|l| id <= l) { return Err(("the trade list's ids do not advance: nothing was read".into(), true)); }
                last = Some(id);
                let rows = normalize_trade(raw, name, &s.base, &s.quote)?;
                if let Some(at) = rows[0].transacted_at { read.push((id, at)); }
                entries.extend(rows);
            }
            if entries.len() > MAX_ENTRIES { return Err((format!("more than {MAX_ENTRIES} ledger entries in one run"), true)); }
            match (trades.len() < PAGE, last) { (true, _) | (_, None) => break, (false, Some(l)) => from_id = l + 1 }
        }
        let rails_reads = if full { t.coins.contains(&s.base) || balances.contains(&s.base) } else { start.pairs.contains(name) };
        let newest = if rails_reads { rails_trades_seen(start.stored.get(name).map_or(&[][..], Vec::as_slice), &read, rails_from) } else { read.iter().map(|r| r.1).max() };
        seen = seen.max(newest);
    }
    let rest = others(venue, clock, pacing, (!full).then_some(from)).await?;
    seen = seen.max(rest.iter().filter_map(|e| e.transacted_at).max());
    entries.extend(rest);
    if entries.len() > MAX_ENTRIES { return Err((format!("more than {MAX_ENTRIES} ledger entries in one run"), true)); }
    Ok(Fetched { entries, pending: t.pending, seen })
}

/// One signed GET whose answer is an object.
async fn object_at<T: Transport>(venue: &BinanceVenue<T>, clock: &dyn Clock, path: &'static str, query: Vec<(&'static str, String)>) -> Result<Raw, Fail> {
    let text = venue.signed_get(path, query, clock.now(), MAX_PAGE_BYTES).await.map_err(|e| (e, false))?;
    tokio::task::spawn_blocking(move || page_of(&text, path, MAX_PAGE_NODES).and_then(|node| Raw::from_node(&node).map_err(|_| (format!("unreadable {path}"), true))))
        .await.unwrap_or_else(|_| Err(("reading the venue's answer was interrupted".into(), true)))
}

/// `Array(page[key])`: the objects listed under `key`, none when it is absent or null.
fn listed(page: &Raw, key: &str, what: &str) -> Result<Vec<Raw>, Fail> {
    match page.member(key) {
        None => Ok(vec![]),
        Some(Node::Scalar(s)) if s == "null" => Ok(vec![]),
        Some(node) => objects(node, what),
    }
}

/// `value.to_i / 1000.0` of the first of `keys` present: a time in milliseconds, written as a number or its digits.
fn millis_at(raw: &Raw, keys: &[&str], what: &str) -> Result<DateTime<Utc>, Fail> {
    let value = keys.iter().find_map(|k| raw.value.get(*k).filter(|v| !v.is_null()));
    let ms = match value { Some(Value::Number(n)) => n.as_i64(), Some(Value::String(s)) => s.parse().ok(), _ => None };
    ms.and_then(ruby_time_at_ms).ok_or_else(|| (format!("unreadable {what} {}", keys[0]), true))
}

fn coin(raw: &Raw, key: &str, what: &str) -> Result<String, Fail> {
    raw.value.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(|| (format!("unreadable {what} {key}"), true))
}

/// The total a paged answer states (`page['total'].to_i`).
fn total(page: &Raw) -> i64 {
    match page.value.get("total") { Some(Value::Number(n)) => n.as_i64().unwrap_or(0), Some(Value::String(s)) => s.parse().unwrap_or(0), _ => 0 }
}

/// #import_convert_trades, #import_fiat_payments, #import_dust_conversions, #import_dividends, #import_earn_rewards and
/// #import_earn_subscriptions, in Rails' order, each walked from `since` (a full sync: from the source's opening) to now.
async fn others<T: Transport>(venue: &BinanceVenue<T>, clock: &dyn Clock, pacing: Pacing, since: Option<i64>) -> Result<Vec<Entry>, Fail> {
    let now = ms(clock.now());
    let recv = || ("recvWindow", READ_RECV_WINDOW.to_string());
    let mut entries = vec![];
    for (call, (start, end)) in windows(since.unwrap_or(CONVERT_FROM_MS), now, CONVERT_DAYS).into_iter().enumerate() {
        if call > 0 { tokio::time::sleep(pacing.converts).await; }
        let page = object_at(venue, clock, CONVERT_PATH, vec![("startTime", start.to_string()), ("endTime", end.to_string()), ("limit", CONVERT_LIMIT.to_string()), recv()]).await?;
        let list = listed(&page, "list", "convert")?;
        if list.len() >= CONVERT_LIMIT || page.value.get("moreData") == Some(&Value::Bool(true)) {
            return Err(("more converts in one 30-day window than Binance answers at once: nothing was stored".into(), true));
        }
        for conv in &list {
            let quote = text(conv.value.get("quoteId")).ok_or_else(|| ("unreadable convert quoteId".to_string(), true))?;
            let at = millis_at(conv, &["createTime"], "convert")?;
            let group = Some(format!("convert_{quote}"));
            let mut out = entry(SWAP_OUT, &coin(conv, "fromAsset", "convert")?, amount(conv, "fromAmount", "convert")?, conv, at);
            (out.tx_id, out.group_id, out.description) = (Some(format!("{quote}-out")), group.clone(), Some("Convert".into()));
            let mut inn = entry(SWAP_IN, &coin(conv, "toAsset", "convert")?, amount(conv, "toAmount", "convert")?, conv, at);
            (inn.tx_id, inn.group_id, inn.description) = (Some(format!("{quote}-in")), group, Some("Convert".into()));
            entries.extend([out, inn]);
        }
    }
    for (kind, entry_type, prefix, description) in [(0, BUY, "fiat-buy", "Fiat purchase"), (1, SELL, "fiat-sell", "Fiat sale")] {
        for (start, end) in windows(since.unwrap_or(HISTORY_FROM_MS), now, WINDOW_DAYS) {
            let mut number = 1;
            loop {
                let page = object_at(venue, clock, FIAT_PATH, vec![("transactionType", kind.to_string()), ("beginTime", start.to_string()), ("endTime", end.to_string()),
                                                                   ("page", number.to_string()), ("rows", FIAT_ROWS.to_string()), recv()]).await?;
                let rows = listed(&page, "data", "fiat payment")?;
                for pay in &rows {
                    if pay.value.get("status").and_then(Value::as_str) != Some("Completed") { continue; }
                    let (crypto, fiat) = (coin(pay, "cryptoCurrency", "fiat payment")?, coin(pay, "fiatCurrency", "fiat payment")?);
                    let (base_amount, quote_amount) = if entry_type == BUY { ("obtainAmount", "sourceAmount") } else { ("sourceAmount", "obtainAmount") };
                    let order = text(pay.value.get("orderNo")).ok_or_else(|| ("unreadable fiat payment orderNo".to_string(), true))?;
                    let mut e = entry(entry_type, &crypto, amount(pay, base_amount, "fiat payment")?, pay, millis_at(pay, &["createTime"], "fiat payment")?);
                    e.quote_amount = Some(amount(pay, quote_amount, "fiat payment")?);
                    e.fee_amount = Some(amount(pay, "totalFee", "fiat payment")?);
                    (e.quote_currency, e.fee_currency) = (Some(fiat.clone()), Some(fiat));
                    (e.tx_id, e.description) = (Some(format!("{prefix}-{order}")), Some(description.into()));
                    entries.push(e);
                }
                if rows.is_empty() || (number * FIAT_ROWS) as i64 >= total(&page).max(rows.len() as i64) { break; }
                number += 1;
            }
        }
    }
    for (start, end) in windows(since.unwrap_or(DUST_FROM_MS), now, WINDOW_DAYS) {
        let page = object_at(venue, clock, DUST_PATH, vec![("startTime", start.to_string()), ("endTime", end.to_string()), recv()]).await?;
        let dribblets = listed(&page, "userAssetDribblets", "dust conversion")?;
        if dribblets.len() >= DUST_LIMIT { return Err(("more dust conversions in one window than Binance answers at once: nothing was stored".into(), true)); }
        for dribblet in &dribblets {
            let at = millis_at(dribblet, &["operateTime"], "dust conversion")?;
            for detail in &listed(dribblet, "userAssetDribbletDetails", "dust conversion detail")? {
                let id = text(detail.value.get("transId")).ok_or_else(|| ("unreadable dust conversion transId".to_string(), true))?;
                let group = Some(format!("dust_{id}"));
                let mut out = entry(SWAP_OUT, &coin(detail, "fromAsset", "dust conversion")?, amount(detail, "amount", "dust conversion")?, detail, at);
                (out.tx_id, out.group_id, out.description) = (Some(format!("dust-{id}-out")), group.clone(), Some("Dust conversion".into()));
                let mut inn = entry(SWAP_IN, "BNB", amount(detail, "transferedAmount", "dust conversion")?, detail, at);
                (inn.fee_currency, inn.fee_amount) = (Some("BNB".into()), Some(amount(detail, "serviceChargeAmount", "dust conversion")?));
                (inn.tx_id, inn.group_id, inn.description) = (Some(format!("dust-{id}-in")), group, Some("Dust conversion".into()));
                entries.extend([out, inn]);
            }
        }
    }
    for (start, end) in windows(since.unwrap_or(HISTORY_FROM_MS), now, DIVIDEND_DAYS) {
        let page = object_at(venue, clock, DIVIDEND_PATH, vec![("startTime", start.to_string()), ("endTime", end.to_string()), ("limit", DIVIDEND_LIMIT.to_string()), recv()]).await?;
        let rows = listed(&page, "rows", "dividend")?;
        if rows.len() >= DIVIDEND_LIMIT { return Err(("more dividends in one window than Binance answers at once: nothing was stored".into(), true)); }
        for div in &rows {
            let id = text(div.value.get("id")).ok_or_else(|| ("unreadable dividend id".to_string(), true))?;
            let mut e = entry(AIRDROP, &coin(div, "asset", "dividend")?, amount(div, "amount", "dividend")?, div, millis_at(div, &["divTime"], "dividend")?);
            (e.tx_id, e.description) = (Some(format!("dividend-{id}")), div.value.get("enInfo").and_then(Value::as_str).map(str::to_string));
            entries.push(e);
        }
    }
    let mut calls = 0;
    for (path, entry_type, prefix, field) in EARN {
        let (id_keys, time_keys, description): (&[&str], &[&str], String) = if entry_type == STAKING_REWARD {
            (&["time", "id"], &["time", "deliverDate"], "Earn reward".into())
        } else {
            (&["purchaseId", "redeemId", "time"], &["time", "createTime"], format!("Earn {}", prefix.replace('-', " ")))
        };
        let kinds: &[Option<&str>] = if path == EARN[0].0 { &[Some(FLEXIBLE_REWARD_TYPES[0]), Some(FLEXIBLE_REWARD_TYPES[1]), Some(FLEXIBLE_REWARD_TYPES[2])] } else { &[None] };
        for (start, end) in windows(since.unwrap_or(EARN_FROM_MS), now, EARN_DAYS) { for kind in kinds {
            let mut current = 1;
            loop {
                if calls > 0 { tokio::time::sleep(pacing.earn).await; }
                calls += 1;
                let mut query = vec![("startTime", start.to_string()), ("endTime", end.to_string()), ("current", current.to_string()), ("size", EARN_SIZE.to_string())];
                if let Some(kind) = kind { query.push(("type", kind.to_string())); }
                let page = object_at(venue, clock, path, query).await?;
                let rows = listed(&page, "rows", "earn record")?;
                if rows.is_empty() { break; }
                for row in &rows {
                    let asset = coin(row, "asset", "earn record")?;
                    let id = id_keys.iter().find_map(|k| text(row.value.get(*k))).ok_or_else(|| (format!("unreadable earn record {}", id_keys[0]), true))?;
                    let tx_id = if entry_type == STAKING_REWARD { format!("{prefix}-{id}-{asset}") } else { format!("{prefix}-{id}") };
                    // Rails' field, else the one Binance documents for locked rewards and every subscription and
                    // redemption (`amount`; Rails reads nil there and stores 0).
                    let quantity = if row.value.get(field).is_some_and(|v| !v.is_null()) { amount(row, field, "earn record")? } else { amount(row, "amount", "earn record")? };
                    let mut e = entry(entry_type, &asset, quantity, row, millis_at(row, time_keys, "earn record")?);
                    (e.tx_id, e.description) = (Some(tx_id), Some(description.clone()));
                    entries.push(e);
                }
                if (current * EARN_SIZE) as i64 >= total(&page) { break; }
                current += 1;
            }
        } }
    }
    Ok(entries)
}

/// One ledger sync of one Binance key, as AccountTransaction::SyncJob#perform runs it, slice 1. Idempotent.
/// `Ok(Err(failure))`: nothing fetched is stored, the watermark is untouched, the key's `last_sync_error` holds why.
pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, pacing: Pacing) -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    sync_captured(db, transport, binance::URL, key_id, credentials, clock, pacing, &version).await
}

/// `base`: where Binance is reached (`Connect::binance_base`; a test points it at a local server).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn sync_captured<T: Transport>(db: &Db, transport: T, base: &str, key_id: i64, credentials: &Credentials, clock: &dyn Clock, pacing: Pacing,
                                                version: &crate::engine::model::CredentialVersion) -> Result<Result<Outcome, Failure>, SyncError> {
    let now = clock.now();
    let captured = version.clone();
    let start = phase(db, move |c| start(c, key_id, &captured)).await?;
    let user_id = start.key.user_id;
    let fail = |text: String, raised: bool| {
        let (creds, now) = (credentials.clone(), clock.now());
        async move {
            let error = super::commit_for(db, version, move |c| record_sync_error(c, key_id, &text, &creds)).await?;
            if !raised { ledger::link_transfers_for(db, user_id, Some(version), now).await?; }
            Ok::<_, SyncError>(Err(Failure { error, raised }))
        }
    };
    if start.key.exchange_type == BINANCE_US {
        // Nothing is read, nothing stored, the watermark stays; the run completes, so the tracker can state the venue
        // from its balances (Rails' read of it always fails, storing nothing either). A completed run clears the error.
        let (producer, stale) = (version.clone(), start.key.last_sync_error.is_some());
        super::commit_for(db, version, move |c| {
            super::cache::record_ledger(c, key_id, &producer, now)?;
            if stale { c.execute("UPDATE api_keys SET last_sync_error = NULL, updated_at = ?1 WHERE id = ?2", params![sql_time(now), key_id])?; }
            Ok(())
        }).await?;
        let mut outcome = Outcome { watermark: start.watermark, complete: true, ..Outcome::default() };
        outcome.linked = ledger::link_transfers_for(db, user_id, Some(version), clock.now()).await?;
        return Ok(Ok(outcome));
    }
    let full = start.watermark.is_none();
    // Transfers from the watermark (never past now) less the overlap, else from Binance's opening; no 80-day floor.
    let from = start.watermark.map_or(HISTORY_FROM_MS, |w| ms(w.min(now) - Duration::hours(OVERLAP_HOURS)));
    let venue = BinanceVenue::new(transport, base, &credentials.secret);
    let fetched = match fetch(&venue, clock, pacing, from, &start, full).await {
        Ok(fetched) => fetched,
        Err((text, raised)) => return fail(text, raised).await,
    };
    let (prior, raw_watermark) = (start.watermark, start.raw_watermark);
    let key = Arc::new(start.key);
    let entries = Arc::new(fetched.entries);
    let mut progress = {
        let (key, batch) = (key.clone(), entries.clone());
        phase(db, move |c| Ok(Progress::new(ledger::asset_rule(c, key.exchange_id, &key.exchange_type)?, ledger::stored_merged(c, &key)?, batch))).await?
    };
    let mut from_entry = 0;
    while from_entry < entries.len() {
        let (key, to) = (key.clone(), entries.len().min(from_entry + BATCH));
        let at = from_entry;
        progress = super::commit_for(db, version, move |c| ledger::store(c, &key, at..to, None, progress, now)).await?;
        from_entry = to;
    }
    ledger::resolve_recent(db, version, &key, progress.assets.clone(), now).await?;
    let mut outcome = progress.out;
    let max_seen = fetched.seen;
    // The watermark: the newest row Rails' read returns (its trade page re-reads stored trades; Rust's reads by id never do), held back by a row that could not be saved and by a transfer still pending,
    // never past the moment this sync began; unchanged when nothing was read.
    let watermark = [max_seen, progress.min_skipped, fetched.pending].into_iter().flatten().min().or(prior).map(|w| w.min(now));
    // `update!(last_synced_at:, last_sync_error: nil)`: written only when one of them moves.
    let changed = watermark.map(|w| w.timestamp_micros()) != raw_watermark || key.last_sync_error.is_some();
    let producer = version.clone();
    super::commit_for(db, version, move |c| {
        super::cache::record_ledger(c, key_id, &producer, now)?;
        if changed { c.execute("UPDATE api_keys SET last_synced_at = ?1, last_sync_error = NULL, updated_at = ?2 WHERE id = ?3", params![watermark.map(sql_time), sql_time(now), key_id])?; }
        Ok(())
    }).await?;
    outcome.watermark = watermark;
    outcome.complete = true;
    outcome.linked = ledger::link_transfers_for(db, user_id, Some(version), clock.now()).await?;
    Ok(Ok(outcome))
}

/// The newest trade time Rails' `account_trade_list` returns for one pair, the venue holding `stored` and `read` (by id):
/// the first RAILS_TRADES from `from` (an incremental sync), else the latest RAILS_TRADES, which end at the newest.
pub fn rails_trades_seen(stored: &[(i64, DateTime<Utc>)], read: &[(i64, DateTime<Utc>)], from: Option<i64>) -> Option<DateTime<Utc>> {
    let venue: std::collections::BTreeMap<i64, DateTime<Utc>> = stored.iter().chain(read).copied().collect();
    match from {
        // A stored time is Ruby's float read of the venue's milliseconds, at most a microsecond under them.
        Some(from) => venue.values().filter(|at| ms(**at + Duration::microseconds(1)) >= from).take(RAILS_TRADES).max().copied(),
        None => venue.values().rev().take(RAILS_TRADES).max().copied(),
    }
}

/// The symbols a run reads, in name order: every pair whose base or quote the account held; on an incremental run only
/// those still trading, or with trades stored (a halted pair can have no new trade).
pub fn candidate_symbols(all: &[(&str, &str, &str, bool)], coins: &HashSet<&str>, stored: &HashSet<&str>, full: bool) -> Vec<String> {
    let mut out: Vec<String> = all.iter().filter(|(name, base, quote, trading)| (coins.contains(base) || coins.contains(quote)) && (full || *trading || stored.contains(name)))
        .map(|(name, ..)| name.to_string()).collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_rubys_float_milliseconds() {
        assert_eq!(sql_time(ruby_time_at_ms(1_785_578_400_123).unwrap()), "2026-08-01 10:00:00.122999");
        assert_eq!(sql_time(ruby_time_at_ms(1_786_010_400_987).unwrap()), "2026-08-06 10:00:00.986999");
        assert_eq!(sql_time(ruby_time_at_ms(1_786_096_800_500).unwrap()), "2026-08-07 10:00:00.500000");
        assert_eq!(sql_time(apply_time("2026-08-10 12:00:00").unwrap()), "2026-08-10 12:00:00");
        assert!(apply_time("2026-08-10T12:00:00Z").is_none());
    }

    #[test]
    fn windows_abut_and_stop_at_the_end() {
        let day = 86_400_000;
        assert_eq!(windows(0, 200 * day, WINDOW_DAYS), vec![(0, 89 * day), (89 * day, 178 * day), (178 * day, 200 * day)]);
        assert!(windows(5, 5, WINDOW_DAYS).is_empty());
        assert_eq!(windows(0, 40 * day, 30), vec![(0, 30 * day), (30 * day, 40 * day)]);
    }

    #[test]
    fn a_stored_trade_id_is_read_from_rails_tx_id() {
        assert_eq!(trade_cursor("BTCUSDT-10-100"), Some(("BTCUSDT".into(), 100)));
        assert_eq!(trade_cursor("ETHBTC-3-7-in"), Some(("ETHBTC".into(), 7)));
        assert_eq!(trade_cursor("ETHBTC-3-7-out"), Some(("ETHBTC".into(), 7)));
        assert_eq!(trade_cursor("0xd1"), None);
        assert_eq!(trade_cursor("fiat-buy-123"), None);
        assert_eq!(trade_cursor("ETHBTC-3-7-x"), None);
    }

    #[test]
    fn rails_trade_page_is_500_from_the_start_or_the_latest_500() {
        let venue_ms = |m: i64| 1_785_000_000_123 + m * 60_000;
        let at = |m: i64| ruby_time_at_ms(venue_ms(m)).unwrap();
        let stored: Vec<_> = (0..400).map(|i| (i, at(i))).collect();
        let read: Vec<_> = (400..700).map(|i| (i, at(i))).collect();
        assert_eq!(rails_trades_seen(&stored, &read, Some(venue_ms(10))), Some(at(509)), "500 from the start, stored ones first");
        assert_eq!(rails_trades_seen(&stored, &read, Some(ms(at(699)))), Some(at(699)), "a trade at the start's millisecond is read");
        assert_eq!(rails_trades_seen(&stored, &read, Some(venue_ms(699) + 1)), None);
        assert_eq!(rails_trades_seen(&stored, &read, None), Some(at(699)));
    }

    #[test]
    fn pairs_are_read_by_base_or_quote() {
        let all = [("BTCUSDT", "BTC", "USDT", true), ("SOLUSDT", "SOL", "USDT", true), ("ETHBTC", "ETH", "BTC", true), ("XRPEUR", "XRP", "EUR", true), ("LUNAUSDT", "LUNA", "USDT", false)];
        let coins: HashSet<&str> = ["USDT"].into();
        assert_eq!(candidate_symbols(&all, &coins, &HashSet::new(), false), ["BTCUSDT", "SOLUSDT"], "a coin bought and sold back is still read");
        assert_eq!(candidate_symbols(&all, &coins, &HashSet::new(), true), ["BTCUSDT", "LUNAUSDT", "SOLUSDT"], "a full sync reads halted pairs too");
    }

    #[test]
    fn a_trade_is_normalized_as_rails_does() {
        let raw = Raw::from_value(serde_json::json!({"id": 7, "orderId": 3, "price": "0.04", "qty": "0.5", "quoteQty": "0.02", "commission": "0.0005", "commissionAsset": "ETH", "time": 1_786_096_800_500_i64, "isBuyer": true}));
        let legs = normalize_trade(&raw, "ETHBTC", "ETH", "BTC").unwrap();
        assert_eq!((legs[0].entry_type, legs[0].tx_id.as_deref(), legs[0].fee_currency.as_deref()), (SWAP_IN, Some("ETHBTC-3-7-in"), Some("ETH")));
        assert_eq!((legs[1].entry_type, legs[1].tx_id.as_deref(), legs[1].fee_amount.is_none()), (SWAP_OUT, Some("ETHBTC-3-7-out"), true));
        let sold = Raw::from_value(serde_json::json!({"id": 9, "orderId": 4, "qty": "1", "quoteQty": "2", "commission": "0.1", "commissionAsset": "BTC", "time": 1, "isBuyer": false}));
        let legs = normalize_trade(&sold, "ETHBTC", "ETH", "BTC").unwrap();
        assert_eq!((legs[0].entry_type, legs[1].entry_type, legs[1].fee_currency.as_deref()), (SWAP_OUT, SWAP_IN, Some("BTC")), "a sale's fee is on the leg it received");
        let fiat = normalize_trade(&sold, "ETHUSDT", "ETH", "USDT").unwrap();
        assert_eq!((fiat.len(), fiat[0].entry_type, fiat[0].tx_id.as_deref()), (1, SELL, Some("ETHUSDT-4-9")));
        let garbage = Raw::from_value(serde_json::json!({"id": 9, "orderId": 4, "qty": "abc", "quoteQty": "2", "commission": "0", "time": 1, "isBuyer": true}));
        assert!(normalize_trade(&garbage, "ETHUSDT", "ETH", "USDT").is_err(), "a garbage quantity is refused, never read as 0");
    }

    #[test]
    fn transfers_hold_the_watermark_while_pending() {
        let mut t = Transfers::default();
        let d = |status: i64, at: i64| Raw::from_value(serde_json::json!({"coin": "BTC", "amount": "1", "status": status, "insertTime": at, "txId": "x"}));
        deposit(&d(1, 3_000), &mut t).unwrap();
        deposit(&d(6, 4_000), &mut t).unwrap();
        deposit(&d(0, 2_000), &mut t).unwrap();
        deposit(&d(8, 1_000), &mut t).unwrap();
        deposit(&d(7, 500), &mut t).unwrap();
        assert_eq!((t.entries.len(), t.pending.map(|p| p.timestamp_millis())), (2, Some(1_000)), "credited deposits stored, pending ones hold");
        let w = |status: i64| Raw::from_value(serde_json::json!({"id": "w", "coin": "BTC", "amount": "1", "status": status, "applyTime": "2026-08-10 12:00:00"}));
        let mut t = Transfers::default();
        for s in [6, 4, 3, 1] { withdrawal(&w(s), &mut t).unwrap(); }
        assert_eq!((t.entries.len(), t.entries[0].tx_id.as_deref(), t.entries[0].fee_amount.is_none(), t.pending.is_some()), (1, Some("w"), true, true));
    }
}
