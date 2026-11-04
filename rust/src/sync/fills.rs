//! A28-1: a tracker ledger row (`account_transactions`) read back as the venue fill it records, for the report of trades
//! no bot recorded. Read only: one `tickers` lookup per row, nothing written.
//!
//! Everything comes from the venue's own object in `raw_data` (and `tx_id` where the importer builds the fill's identity
//! from it), never from the row's currencies or amounts; `entry_type` only tells a trade row (0..=3) from the rest.
//! - `fill_id`: the importer's identity for the fill. Binance (+US), MEXC and Bitrue keep theirs ("SYMBOL-orderId-id");
//!   every other venue's is its fill id behind the market symbol ("SYMBOL|id"), so a fill id reused on another pair is
//!   another fill. Both swap rows of one crypto-quoted fill (Binance, Coinbase) carry the same raw object and read as
//!   the same `Fill`; one leg alone reads the same too.
//! - `order_key`: the venue order id in the form `transactions.external_id` takes for an order placed there (each
//!   venue's `add_order`, the Rails `get_order` parse beside it).
//! - the market symbol resolves to exactly one `tickers` row of the exchange; none or several is unreadable.
//! - a trade row whose raw object this can't read (a file import's, a missing or malformed field) is `Unreadable`;
//!   a Binance convert, fiat payment or dust conversion is `NotATrade` (no order of a bot is ever one).
use super::activities::{BUY, SELL, SWAP_IN, SWAP_OUT};
use super::binance_ledger::{ruby_time_at_ms, text};
use super::number::{decimal, VENUE};
use crate::ruby::BigDec;
use crate::venue::Side;
use crate::venue::{binance::{BINANCE, BINANCE_US}, bingx::BINGX, bitget::BITGET, bitrue::BITRUE, bitvavo::BITVAVO, bybit::BYBIT, coinbase::COINBASE,
                   gemini::GEMINI, hyperliquid::HYPERLIQUID, mexc::MEXC};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::Value;

pub struct LedgerRow { pub id: i64, pub tx_id: Option<String>, pub entry_type: i64, pub raw_data: Value, pub transacted_at: DateTime<Utc>, pub exchange_id: i64 }

#[derive(Clone, Debug, PartialEq)]
pub struct Fill { pub fill_id: String, pub order_key: String, pub ticker_id: i64, pub base_asset_id: i64, pub quote_asset_id: i64, pub side: Side,
                  pub base_qty: BigDec, pub quote_qty: BigDec, pub at: DateTime<Utc> }

#[derive(Clone, Debug, PartialEq)]
pub enum Read { Fill(Fill), NotATrade, Unreadable(String) }

const SUPPORTED: [&str; 12] = [BINANCE, BINANCE_US, COINBASE, BITGET, BYBIT, MEXC, GEMINI, BITVAVO, BINGX, BITRUE, HYPERLIQUID, super::ALPACA];

/// The venues A28-1 reads fills of. Not Kraken (its rows name no order), KuCoin or IBKR.
pub fn supported(venue: &str) -> bool { SUPPORTED.contains(&venue) }

/// How the order key is made once the ticker is known.
enum Key { Raw(String), Ticker(String), Base(String) }
/// How the market symbol finds its ticker.
#[derive(Clone, Copy)]
enum Match { Exact, Lower, Alpaca }
struct Parsed { symbol: String, matching: Match, fill_id: String, key: Key, side: Side, base_qty: BigDec, quote_qty: BigDec, at: DateTime<Utc> }

/// `venue` = exchanges.type of the row's exchange. Err: an unsupported venue or a database failure.
pub fn read(c: &Connection, venue: &str, row: &LedgerRow) -> Result<Read, String> {
    if !supported(venue) { return Err(format!("no fill reader for {venue}")); }
    if ![BUY, SELL, SWAP_IN, SWAP_OUT].contains(&row.entry_type) { return Ok(Read::NotATrade); }
    let (raw, tx_id) = (&row.raw_data, row.tx_id.as_deref());
    let parsed = match venue {
        BINANCE | BINANCE_US => binance(raw, tx_id),
        MEXC => symbol_order_id(raw, tx_id, false),
        BITRUE => symbol_order_id(raw, tx_id, true),
        COINBASE => coinbase(raw, tx_id),
        BITGET => bitget(raw),
        BYBIT => bybit(raw),
        GEMINI => gemini(raw),
        BITVAVO => bitvavo(raw),
        BINGX => bingx(raw, tx_id),
        HYPERLIQUID => hyperliquid(raw),
        _ => alpaca(raw),
    };
    let p = match parsed { Ok(Some(p)) => p, Ok(None) => return Ok(Read::NotATrade), Err(why) => return Ok(Read::Unreadable(why)) };
    let filter = match p.matching {
        Match::Exact => "ticker = ?2",
        Match::Lower => "lower(ticker) = lower(?2)",
        // activities.rs#normalize: "BTC/USD" as listed, a compact crypto pair ("BTCUSD") by base and quote, a stock by its ticker.
        Match::Alpaca => "(ticker = ?2 OR (instr(ticker, '/') > 0 AND base || quote = ?2))",
    };
    let mut s = c.prepare_cached(&format!("SELECT id, base_asset_id, quote_asset_id, ticker, base FROM tickers WHERE exchange_id = ?1 AND {filter} LIMIT 2"))
        .map_err(|e| e.to_string())?;
    let rows = s.query_map(params![row.exchange_id, p.symbol], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>()).map_err(|e| e.to_string())?;
    let [(ticker_id, base_asset_id, quote_asset_id, ticker, base)] = &rows[..] else {
        return Ok(Read::Unreadable(if rows.is_empty() { "unmapped symbol" } else { "symbol names several tickers" }.into()));
    };
    // A listing the catalogue replaced keeps its row under "__stale_<id>_<ticker>" (jobs::import::tombstone), so an old
    // fill's symbol may name the replaced pair, not today's: which one can't be told (Codex A28 branch r3).
    if reused(c, row.exchange_id, &p.symbol, p.matching)? { return Ok(Read::Unreadable("symbol reused by a later listing".into())); }
    let order_key = match p.key { Key::Raw(id) => id, Key::Ticker(id) => format!("{ticker}-{id}"), Key::Base(id) => format!("{base}-{id}") };
    Ok(Read::Fill(Fill { fill_id: p.fill_id, order_key, ticker_id: *ticker_id, base_asset_id: *base_asset_id, quote_asset_id: *quote_asset_id,
                         side: p.side, base_qty: p.base_qty, quote_qty: p.quote_qty, at: p.at }))
}

/// Does a tombstoned ticker of the exchange carry this symbol?
fn reused(c: &Connection, exchange_id: i64, symbol: &str, matching: Match) -> Result<bool, String> {
    let mut s = c.prepare_cached("SELECT ticker FROM tickers WHERE exchange_id = ?1 AND substr(ticker, 1, 8) = '__stale_'").map_err(|e| e.to_string())?;
    let names = s.query_map([exchange_id], |r| r.get::<_, String>(0)).and_then(|r| r.collect::<Result<Vec<_>, _>>()).map_err(|e| e.to_string())?;
    Ok(names.iter().filter_map(|n| n.strip_prefix("__stale_").and_then(|rest| rest.split_once('_')).map(|(_, old)| old)).any(|old| match matching {
        Match::Lower => old.eq_ignore_ascii_case(symbol),
        Match::Exact | Match::Alpaca => old == symbol,
    }))
}

type Out = Result<Option<Parsed>, String>;

/// A field as text (a string as it is, an integer as its digits), not blank.
fn s(raw: &Value, k: &str) -> Result<String, String> { text(raw.get(k)).filter(|t| !crate::ruby::blank(t)).ok_or_else(|| format!("no {k}")) }
/// A number the venue wrote (string or JSON number), within the venue caps; never a missing value read as 0.
fn d(raw: &Value, k: &str) -> Result<BigDec, String> {
    let t = match raw.get(k) { Some(Value::String(t)) => t.clone(), Some(Value::Number(n)) => n.to_string(), _ => return Err(format!("no {k}")) };
    decimal(&t, &VENUE).map_err(|_| format!("unreadable {k}"))
}
/// A time in milliseconds (number or digits), as the importers store it.
fn ms(raw: &Value, k: &str) -> Result<DateTime<Utc>, String> {
    let n = match raw.get(k) { Some(Value::Number(n)) => n.as_i64(), Some(Value::String(t)) => t.parse().ok(), _ => None };
    n.and_then(ruby_time_at_ms).ok_or_else(|| format!("unreadable {k}"))
}
fn rfc3339(raw: &Value, k: &str) -> Result<DateTime<Utc>, String> {
    raw[k].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc)).ok_or_else(|| format!("unreadable {k}"))
}
fn side(raw: &Value, k: &str, buy: &str, sell: &str) -> Result<Side, String> {
    match raw[k].as_str() { Some(v) if v == buy => Ok(Side::Buy), Some(v) if v == sell => Ok(Side::Sell), _ => Err(format!("unreadable {k}")) }
}
fn buyer(raw: &Value) -> Result<Side, String> {
    match raw.get("isBuyer") { Some(Value::Bool(true)) => Ok(Side::Buy), Some(Value::Bool(false)) => Ok(Side::Sell), _ => Err("unreadable isBuyer".into()) }
}

/// myTrades: tx_id "SYMBOL-orderId-id" (a swap leg "-in"/"-out"), external id "SYMBOL-orderId" (binance_order_links::fill_order).
fn binance(raw: &Value, tx_id: Option<&str>) -> Out {
    let Some(order) = super::binance_order_links::fill_order(tx_id, raw) else {
        // A convert (quoteId), a fiat payment (orderNo) or a dust conversion (transferedAmount): no spot order.
        return if ["quoteId", "orderNo", "transferedAmount"].iter().any(|k| raw.get(*k).is_some()) { Ok(None) } else { Err("not a myTrades row".into()) };
    };
    let order = order.ok_or("tx_id names no orderId and id")?;
    let tx_id = tx_id.unwrap_or_default();
    let fill_id = tx_id.strip_suffix("-in").or_else(|| tx_id.strip_suffix("-out")).unwrap_or(tx_id).to_string();
    let (symbol, order_id) = order.split_once('-').ok_or("tx_id names no symbol")?;
    if raw.get("symbol").is_some_and(|v| v.as_str() != Some(symbol)) { return Err("symbol differs from tx_id".into()); }
    Ok(Some(Parsed { symbol: symbol.into(), matching: Match::Exact, key: Key::Ticker(order_id.into()), side: buyer(raw)?, base_qty: d(raw, "qty")?,
                     quote_qty: d(raw, "quoteQty")?, at: ms(raw, "time")?, fill_id }))
}

/// MEXC and Bitrue myTrades: one buy or sell row, tx_id "SYMBOL-orderId-id" (the symbol as the importer read the pair).
/// Bitrue's quote is `quoteQty`, else qty × price, as bitrue_ledger::trade.
fn symbol_order_id(raw: &Value, tx_id: Option<&str>, bitrue: bool) -> Out {
    let (order, id) = (s(raw, "orderId")?, s(raw, "id")?);
    let tx_id = tx_id.ok_or("no tx_id")?;
    let symbol = tx_id.strip_suffix(&format!("-{order}-{id}")).filter(|s| !s.is_empty()).ok_or("tx_id names no orderId and id")?;
    let qty = d(raw, "qty")?;
    let quote = if bitrue && raw.get("quoteQty").is_none_or(|v| v.is_null() || v.as_str().is_some_and(crate::ruby::blank)) { &qty * &d(raw, "price")? } else { d(raw, "quoteQty")? };
    Ok(Some(Parsed { symbol: symbol.into(), matching: Match::Exact, fill_id: tx_id.into(), key: Key::Ticker(order), side: buyer(raw)?, base_qty: qty,
                     quote_qty: quote, at: ms(raw, "time")? }))
}

/// Advanced Trade fills: the importer's id (an adjusted fill's `entry_id`, else `trade_id`, else `entry_id`), a swap leg
/// "-in"/"-out" behind it; amounts by coinbase_ledger::amounts (a quote-sized fill's size is its quote).
fn coinbase(raw: &Value, tx_id: Option<&str>) -> Out {
    let product = s(raw, "product_id")?;
    let adjusted = raw["trade_type"].as_str().is_some_and(|t| !t.is_empty() && t != "FILL");
    let id = if adjusted { text(raw.get("entry_id")) } else { None }.or_else(|| text(raw.get("trade_id"))).or_else(|| text(raw.get("entry_id")))
        .filter(|t| !crate::ruby::blank(t)).ok_or("no trade_id")?;
    if !tx_id.is_some_and(|t| [id.clone(), format!("{id}-in"), format!("{id}-out")].iter().any(|x| x == t)) { return Err("tx_id differs from the fill's id".into()); }
    d(raw, "size")?;
    d(raw, "price")?;
    let (base_qty, quote_qty) = super::coinbase_ledger::amounts(&super::activities::Raw::from_value(raw.clone())).map_err(|(why, _)| why)?;
    Ok(Some(Parsed { fill_id: format!("{product}|{id}"), symbol: product, matching: Match::Exact, key: Key::Raw(s(raw, "order_id")?), side: side(raw, "side", "BUY", "SELL")?,
                     base_qty, quote_qty, at: rfc3339(raw, "trade_time")? }))
}

/// v2 spot fills: keyed by `tradeId` (a fill without one is unreadable), quote `amount`, else size × priceAvg.
fn bitget(raw: &Value) -> Out {
    let symbol = s(raw, "symbol")?;
    let size = d(raw, "size")?;
    let quote = if raw.get("amount").is_some() { d(raw, "amount")? } else { &size * &d(raw, "priceAvg")? };
    let side = match raw["side"].as_str().map(str::to_lowercase).as_deref() { Some("buy") => Side::Buy, Some("sell") => Side::Sell, _ => return Err("unreadable side".into()) };
    Ok(Some(Parsed { fill_id: format!("{symbol}|{}", s(raw, "tradeId")?), symbol, matching: Match::Exact, key: Key::Ticker(s(raw, "orderId")?), side,
                     base_qty: size, quote_qty: quote, at: ms(raw, "cTime")? }))
}

/// Execution list: keyed by `execId`, the external id the bare `orderId`.
fn bybit(raw: &Value) -> Out {
    let symbol = s(raw, "symbol")?;
    Ok(Some(Parsed { fill_id: format!("{symbol}|{}", s(raw, "execId")?), symbol, matching: Match::Exact, key: Key::Raw(s(raw, "orderId")?), side: side(raw, "side", "Buy", "Sell")?,
                     base_qty: d(raw, "execQty")?, quote_qty: d(raw, "execValue")?, at: ms(raw, "execTime")? }))
}

/// mytrades: keyed by `tid`, the symbol lowercase ("btcusd", gemini_ledger::Pairs), quote price × amount.
fn gemini(raw: &Value) -> Out {
    let symbol = s(raw, "symbol")?;
    let amount = d(raw, "amount")?;
    Ok(Some(Parsed { fill_id: format!("{}|{}", symbol.to_lowercase(), s(raw, "tid")?), symbol, matching: Match::Lower, key: Key::Raw(s(raw, "order_id")?),
                     side: side(raw, "type", "Buy", "Sell")?, quote_qty: &d(raw, "price")? * &amount, base_qty: amount, at: ms(raw, "timestampms")? }))
}

/// /trades: keyed by `id`, the external id "MARKET-orderId", quote amount × price.
fn bitvavo(raw: &Value) -> Out {
    let market = s(raw, "market")?;
    let amount = d(raw, "amount")?;
    Ok(Some(Parsed { fill_id: format!("{market}|{}", s(raw, "id")?), symbol: market, matching: Match::Exact, key: Key::Ticker(s(raw, "orderId")?),
                     side: side(raw, "side", "buy", "sell")?, quote_qty: &amount * &d(raw, "price")?, base_qty: amount, at: ms(raw, "timestamp")? }))
}

/// myTrades fills: the importer keys on the bare `id`, so the fill id here carries symbol and order.
fn bingx(raw: &Value, tx_id: Option<&str>) -> Out {
    let (symbol, order, id) = (s(raw, "symbol")?, s(raw, "orderId")?, s(raw, "id")?);
    if tx_id != Some(id.as_str()) { return Err("tx_id differs from the trade's id".into()); }
    Ok(Some(Parsed { fill_id: format!("{symbol}|{order}|{id}"), symbol, matching: Match::Exact, key: Key::Ticker(order), side: buyer(raw)?,
                     base_qty: d(raw, "qty")?, quote_qty: d(raw, "quoteQty")?, at: ms(raw, "time")? }))
}

/// userFillsByTime: a spot coin ("@N" or "PURR/USDC") names its ticker as hyperliquid_ledger::Pairs does; keyed by `tid`;
/// the external id "BASE-oid" (the ticker's base); quote px × sz.
fn hyperliquid(raw: &Value) -> Out {
    let coin = s(raw, "coin")?;
    if !(coin.starts_with('@') || coin.contains('/')) { return Err("not a spot fill".into()); }
    let size = d(raw, "sz")?;
    Ok(Some(Parsed { fill_id: format!("{coin}|{}", s(raw, "tid")?), symbol: coin, matching: Match::Exact, key: Key::Base(s(raw, "oid")?), side: side(raw, "side", "B", "A")?,
                     quote_qty: &d(raw, "px")? * &size, base_qty: size, at: ms(raw, "time")? }))
}

/// A FILL activity: keyed by its `id`, the external id the order's `order_id`, quote qty × price.
fn alpaca(raw: &Value) -> Out {
    if raw["activity_type"] != "FILL" { return Err("not a FILL activity".into()); }
    let symbol = s(raw, "symbol")?;
    let qty = d(raw, "qty")?;
    Ok(Some(Parsed { fill_id: format!("{symbol}|{}", s(raw, "id")?), symbol, matching: Match::Alpaca, key: Key::Raw(s(raw, "order_id")?), side: side(raw, "side", "buy", "sell")?,
                     quote_qty: &qty * &d(raw, "price")?, base_qty: qty, at: rfc3339(raw, "transaction_time")? }))
}
