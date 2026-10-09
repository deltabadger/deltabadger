//! What the figures read from the database: SELECTs only, shaped as Rails' own queries are.
use super::at::At;
use super::{budget, num::NumError};
use std::collections::{HashMap, HashSet};
use super::FiguresError;
use crate::enums::BotStatus;
use super::dec::Dec;
use rusqlite::{params, Connection, OptionalExtension, Row as SqlRow};
use serde_json::Value;

fn data(what: impl std::fmt::Debug) -> FiguresError { FiguresError::Data(format!("{what:?}")) }
/// A decimal column: the one way a number of the database comes in, measured (`dec`).
pub(super) fn decimal(r: &SqlRow<'_>, i: usize) -> Result<Option<Dec>, FiguresError> { Ok(Dec::from_sql(r.get_ref(i)?)?) }
pub(super) fn instant(text: &str) -> Result<At, FiguresError> { At::from_sql(text).ok_or_else(|| FiguresError::Data(format!("a time Rails did not write: {text:?}"))) }
/// Ids as SQL literals, so each query has the shape of the one Rails sends (`IN (1, 2, 3)`); they are integers.
fn id_list(ids: &[i64]) -> Result<String, FiguresError> {
    budget::charge(ids.len() as u64, 0)?;
    Ok(ids.iter().map(i64::to_string).collect::<Vec<_>>().join(", "))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Basket, Index }

#[derive(Clone, Debug)]
pub struct Bot {
    pub id: i64, pub user_id: i64, pub exchange_id: Option<i64>, pub kind: Kind,
    /// exchanges.type, e.g. `Exchanges::Alpaca`.
    pub exchange_type: Option<String>,
    pub quote_asset_id: Option<i64>,
    /// Bots::DcaMultiAsset#base_asset_ids: the allocations' keys in their stored order. Empty for an index.
    pub base_asset_ids: Vec<i64>,
}

/// Ruby's Integer/Float/String#to_i for the allocations keys and legacy base_asset_ids.
/// An integer we cannot represent is refused, never silently replaced with zero.
fn to_i(v: &Value) -> Result<i64, FiguresError> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() { return Ok(i); }
            if n.is_u64() { return Err(NumError::OutOfRange.into()); }
            let f = n.as_f64().ok_or(NumError::OutOfRange)?.trunc();
            if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&f) {
                return Err(NumError::OutOfRange.into());
            }
            Ok(f as i64)
        }
        Value::String(s) => {
            let mut bytes = s.trim_start_matches(|c: char| c.is_ascii_whitespace()).bytes().peekable();
            let negative = match bytes.peek() { Some(b'-') => { bytes.next(); true }, Some(b'+') => { bytes.next(); false }, _ => false };
            let mut n = 0i64;
            let mut digit = false;
            while let Some(b) = bytes.next() {
                if b.is_ascii_digit() {
                    let d = i64::from(b - b'0');
                    n = n.checked_mul(10).and_then(|n| if negative { n.checked_sub(d) } else { n.checked_add(d) }).ok_or(NumError::OutOfRange)?;
                    digit = true;
                } else if b == b'_' && digit && bytes.peek().is_some_and(u8::is_ascii_digit) {
                    digit = false;
                } else { break; }
            }
            Ok(n)
        }
        Value::Null => Ok(0),
        _ => Err(FiguresError::Raised("NoMethodError: undefined method 'to_i'".into())),
    }
}

/// Asset.find_by(id: quote_asset_id) / Ticker.where(quote_asset_id:): Active Record's integer
/// predicate truncates Floats and makes an out-of-range integer an unsatisfiable predicate.
fn quote_id(v: &Value) -> Result<Option<i64>, FiguresError> {
    match v {
        Value::Null | Value::Array(_) | Value::Object(_) => Ok(None),
        Value::Bool(b) => Ok(Some(i64::from(*b))),
        Value::String(s) if {
            let text = s.trim_start_matches(|c: char| c.is_ascii_whitespace());
            let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
            !digits.starts_with(|c: char| c.is_ascii_digit())
        } => Ok(None),
        _ => match to_i(v) {
            Ok(id) => Ok(Some(id)),
            Err(FiguresError::NotComputed(reason)) if reason == super::OUT_OF_RANGE => Ok(None),
            Err(e) => Err(e),
        },
    }
}

/// The bot, if it is one whose figures this library computes: a basket or an index bot.
pub fn bot(c: &Connection, id: i64) -> Result<Bot, FiguresError> {
    let row = c.query_row("SELECT user_id, exchange_id, type, settings FROM bots WHERE id = ?1", [id], |r| {
        Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?))
    }).optional()?;
    let Some((user_id, exchange_id, kind, settings)) = row else { return Err(FiguresError::Data(format!("no bot {id}"))) };
    let kind = match kind.as_deref() {
        Some("Bots::DcaMultiAsset") => Kind::Basket,
        Some("Bots::DcaIndex") => Kind::Index,
        other => return Err(FiguresError::NotComputed(format!("bot {id} is a {}: only baskets and index bots are computed", other.unwrap_or("bot without a type")))),
    };
    let settings: Value = serde_json::from_str(&settings).map_err(data)?;
    let base_asset_ids = match (kind, settings.get("allocations").and_then(Value::as_object)) {
        (Kind::Index, _) => vec![],
        (Kind::Basket, Some(weights)) if !weights.is_empty() => weights.keys().map(|k| to_i(&Value::String(k.clone()))).collect::<Result<Vec<_>, _>>()?,
        (Kind::Basket, _) => settings.get("base_asset_ids").and_then(Value::as_array).map(|ids| ids.iter().map(to_i).collect::<Result<Vec<_>, _>>()).transpose()?.unwrap_or_default(), // allow-swallow: an Option; a basket with no assets saved has none
    };
    let exchange_type = match exchange_id {
        Some(e) => c.query_row("SELECT type FROM exchanges WHERE id = ?1", [e], |r| r.get::<_, Option<String>>(0)).optional()?.flatten(),
        None => None,
    };
    let quote_asset_id = settings.get("quote_asset_id").map(quote_id).transpose()?.flatten();
    Ok(Bot { id, user_id: user_id.unwrap_or(0), exchange_id: exchange_id.filter(|_| exchange_type.is_some()), kind, exchange_type, quote_asset_id, base_asset_ids })
}

/// One submitted order, as the walks pluck it.
#[derive(Clone, Debug)]
pub struct Order {
    pub id: i64, pub at: At, pub exchange_id: Option<i64>,
    pub raw: super::fill::Raw,
    pub base: Option<String>, pub asset_id: Option<i64>,
    pub sell: bool, pub buy: bool, pub closed: bool,
    /// transactions.transaction_type: REGULAR, REBALANCE, LIQUIDATION, REDEPLOY.
    pub kind: String,
}

/// `transactions.submitted.order(:created_at, :id)`: every walk reads this one list.
pub fn orders(c:&Connection,bot_id:i64)->Result<Vec<Order>,FiguresError>{super::fill::orders(c,bot_id)}

/// An asset's `(id, symbol, name)`.
pub type AssetName = (i64, Option<String>, Option<String>);

/// `Asset.where(id: ids).pluck(:id, :symbol, :name)`.
pub fn asset_names(c: &Connection, ids: &[i64]) -> Result<Vec<AssetName>, FiguresError> {
    let mut statement = c.prepare(&format!("SELECT id, symbol, name FROM assets WHERE id IN ({})", id_list(ids)?))?;
    let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.map(|row| { budget::charge(1, 0)?; Ok(row?) }).collect::<Result<Vec<_>, FiguresError>>()?;
    Ok(rows)
}

#[derive(Clone, Debug)]
pub struct Ticker {
    pub id: i64,
    /// tickers.ticker: the venue's code for the pair ("AAPL", "BTC/USD").
    pub ticker: String,
    /// tickers.base: the venue's name for the base.
    pub base: String,
    pub base_asset_id: i64, pub quote_decimals: Option<i64>,
    /// assets.category of the base asset.
    pub base_category: Option<String>,
    /// Whether the base asset's row exists.
    pub base_asset_exists: bool,
}

const TICKER: &str = "SELECT t.id, t.ticker, t.base, t.base_asset_id, t.quote_decimals, a.category, a.id IS NOT NULL FROM tickers t LEFT JOIN assets a ON a.id = t.base_asset_id";
fn ticker(r: &SqlRow<'_>) -> rusqlite::Result<Ticker> {
    Ok(Ticker { id: r.get(0)?, ticker: r.get(1)?, base: r.get(2)?, base_asset_id: r.get(3)?, quote_decimals: r.get(4)?, base_category: r.get(5)?, base_asset_exists: r.get(6)? })
}

/// `bot.tickers`. A basket: its members and every holding that left it, on this venue and quote
/// (Bots::DcaMultiAsset#set_tickers). An index: every ticker of the venue in that quote (Bots::DcaIndex#set_tickers).
/// Only what is available and trading; none without a venue.
pub fn tickers(c: &Connection, bot: &Bot) -> Result<Vec<Ticker>, FiguresError> {
    let (Some(exchange_id), Some(quote)) = (bot.exchange_id, bot.quote_asset_id) else { return Ok(vec![]) };
    let live = "t.exchange_id = ?1 AND t.available = 1 AND t.trading_enabled = 1 AND t.quote_asset_id = ?2";
    let rows = match bot.kind {
        Kind::Index => c.prepare(&format!("{TICKER} WHERE {live}"))?.query_map(params![exchange_id, quote], ticker)?.map(|row| { budget::charge(1, 0)?; Ok(row?) }).collect::<Result<Vec<_>, FiguresError>>()?,
        Kind::Basket => {
            let mut ids = bot.base_asset_ids.clone();
            let mut statement = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1")?;
            for id in statement.query_map([bot.id], |r| r.get::<_, i64>(0))? { budget::charge(1, 0)?; ids.push(id?); }
            c.prepare(&format!("{TICKER} WHERE {live} AND t.base_asset_id IN ({})", id_list(&ids)?))?
                .query_map(params![exchange_id, quote], ticker)?.map(|row| { budget::charge(1, 0)?; Ok(row?) }).collect::<Result<Vec<_>, FiguresError>>()?
        }
    };
    Ok(rows)
}

/// AssetConfigurable#ticker has no availability/trading filter.
pub fn pair_ticker(c: &Connection, bot: &Bot) -> Result<Vec<Ticker>, FiguresError> {
    let (Some(exchange), Some(quote), Some(base)) = (bot.exchange_id, bot.quote_asset_id, bot.base_asset_ids.first()) else { return Ok(vec![]) };
    budget::charge(1, 0)?;
    Ok(c.prepare(&format!("{TICKER} WHERE t.exchange_id=?1 AND t.quote_asset_id=?2 AND t.base_asset_id=?3 ORDER BY t.id LIMIT 1"))?
        .query_map(params![exchange, quote, base], ticker)?.collect::<Result<Vec<_>, _>>()?)
}

/// Composition minimums include disabled tickers; an empty/no-minimum set is unavailable, never zero.
pub fn composition_minimum(c: &Connection, s: &Subject) -> Result<Option<Dec>, FiguresError> {
    let mut q=c.prepare("SELECT t.minimum_quote_size FROM bot_index_assets m JOIN tickers t ON t.id=m.ticker_id WHERE m.bot_id=?1 AND m.in_index=1")?;
    let mut values=vec![];
    let mut rows=0;
    for v in q.query_map([s.bot.id], |r| r.get::<_,rusqlite::types::Value>(0))? {
        budget::charge(1,0)?; rows+=1;
        if let Some(v)=Dec::from_sql((&v?).into())? { values.push(v); }
    }
    if rows==0 {
        for t in &s.tickers {
            budget::charge(1,0)?;
            let v=c.query_row("SELECT minimum_quote_size FROM tickers WHERE id=?1",[t.id],|r|r.get::<_,rusqlite::types::Value>(0))?;
            if let Some(v)=Dec::from_sql((&v).into())? { values.push(v); }
        }
    }
    Ok(values.into_iter().min())
}

/// The precision of the tickers the composition trades right now, tradable or not (Allocatable#composition_tickers):
/// what a basket rounds with once none of its members is tradable.
pub fn composition_quote_decimals(c: &Connection, bot_id: i64) -> Result<Vec<Option<i64>>, FiguresError> {
    let mut statement = c.prepare("SELECT t.quote_decimals FROM bot_index_assets m INNER JOIN tickers t ON t.id = m.ticker_id WHERE m.bot_id = ?1 AND m.in_index = 1")?;
    let rows = statement.query_map([bot_id], |r| r.get(0))?.map(|row| { budget::charge(1, 0)?; Ok(row?) }).collect::<Result<Vec<_>, FiguresError>>()?;
    Ok(rows)
}

/// `Ticker.where(exchange_id:, base_asset_id: ids)` with each base asset's symbol: every listing of these assets on
/// the venue, tradable or not, as `(base, asset id, symbol)`.
pub fn listings(c: &Connection, exchange_id: i64, asset_ids: &[i64]) -> Result<Vec<(String, i64, Option<String>)>, FiguresError> {
    let mut statement = c.prepare(&format!("SELECT base, base_asset_id FROM tickers WHERE exchange_id = ?1 AND base_asset_id IN ({})", id_list(asset_ids)?))?;
    let rows = statement.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.map(|row| { budget::charge(1, 0)?; Ok(row?) }).collect::<Result<Vec<_>, FiguresError>>()?;
    let mut names = HashMap::new();
    for (id, symbol, _) in asset_names(c, asset_ids)? { budget::charge(1, 0)?; names.insert(id, symbol); }
    rows.into_iter().map(|(base, id)| {
        budget::charge(1, 0)?;
        Ok((base, id, names.get(&id).cloned().flatten()))
    }).collect()
}

/// The ids among these that are assets (`Asset.where(id: ids)`).
pub fn existing_assets(c: &Connection, ids: &[i64]) -> Result<Vec<i64>, FiguresError> {
    Ok(asset_names(c, ids)?.into_iter().map(|(id, _, _)| id).collect())
}

/// `Asset.find_by(id:)&.symbol`.
pub fn asset_symbol(c: &Connection, id: Option<i64>) -> Result<Option<String>, FiguresError> {
    let Some(id) = id else { return Ok(None) };
    Ok(c.query_row("SELECT symbol FROM assets WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0)).optional()?.flatten())
}

/// What a currency's asset says about it: its category and its id at the market-data provider.
#[derive(Clone, Debug)]
pub struct Asset { pub category: Option<String>, pub external_id: Option<String> }

/// `Asset.find_by(symbol:)`. One row of several, as SQLite hands it to Rails for the same query.
pub fn asset_by_symbol(c: &Connection, symbol: &str) -> Result<Option<Asset>, FiguresError> {
    Ok(c.query_row("SELECT category, external_id FROM assets WHERE symbol = ?1 LIMIT 1", [symbol], |r| Ok(Asset { category: r.get(0)?, external_id: r.get(1)? })).optional()?)
}

/// AccountTransaction.entry_types[:adjustment] (pinned by rust/tests/figures_vectors.rs).
pub const ADJUSTMENT: i64 = 15;

/// A corporate-action row of the account's ledger, marked as a split.
#[derive(Clone, Debug)]
pub struct SplitRow { pub id: i64, pub exchange_id: i64, pub base_currency: String, pub base_asset_id: Option<i64>, pub raw_data: Value, pub at: At }

/// The `adjustment` rows marked `corporate_action: split` on these venues (Bot::Restatable#grouped_split_rows).
pub fn split_rows(c: &Connection, user_id: i64, exchange_ids: &[i64]) -> Result<Vec<SplitRow>, FiguresError> {
    let mut statement = c.prepare(&format!(
        "SELECT exchange_id, base_currency, raw_data, transacted_at, id, base_asset_id FROM account_transactions \
         WHERE user_id = ?1 AND entry_type = ?2 AND exchange_id IN ({}) \
         AND CASE WHEN json_valid(raw_data) THEN json_extract(raw_data, '$.corporate_action') END = 'split'", id_list(exchange_ids)?))?;
    let mut rows = statement.query(params![user_id, ADJUSTMENT])?;
    let mut out = vec![];
    while let Some(r) = rows.next()? {
        budget::charge(1, 0)?;
        let raw: String = r.get(2)?;
        out.push(SplitRow {
            id: r.get(4)?, exchange_id: r.get(0)?, base_currency: r.get(1)?, base_asset_id: r.get(5)?,
            raw_data: serde_json::from_str(&raw).map_err(data)?,
            at: instant(&r.get::<_, String>(3)?)?,
        });
    }
    Ok(out)
}

/// Bot::Restatable#account_classes: `(symbol, category)` for each asset this user's ledger rows recorded under one of
/// these names, then each asset their balances hold under it (assets.symbol), distinct per source.
pub fn account_classes(c: &Connection, user_id: i64, symbols: &[String]) -> Result<Vec<(String, Option<String>)>, FiguresError> {
    if symbols.is_empty() { return Ok(vec![]); }
    budget::charge(symbols.len() as u64, 0)?;
    let names = Value::from(symbols.to_vec()).to_string();
    let mut out = vec![];
    for sql in ["SELECT DISTINCT t.base_currency, a.category FROM account_transactions t INNER JOIN assets a ON a.id = t.base_asset_id \
                 WHERE t.user_id = ?1 AND t.base_currency IN (SELECT value FROM json_each(?2))",
                "SELECT DISTINCT a.symbol, a.category FROM account_balances b INNER JOIN assets a ON a.id = b.asset_id \
                 WHERE b.user_id = ?1 AND a.symbol IN (SELECT value FROM json_each(?2))"] {
        let mut statement = c.prepare(sql)?;
        for row in statement.query_map(params![user_id, names], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))? {
            budget::charge(1, 0)?;
            out.push(row?);
        }
    }
    Ok(out)
}

/// An asset's category; None for an uncategorised or missing asset.
pub fn asset_category(c: &Connection, asset_id: i64) -> Result<Option<String>, FiguresError> {
    budget::charge(1, 0)?;
    Ok(c.query_row("SELECT category FROM assets WHERE id = ?1", [asset_id], |r| r.get::<_, Option<String>>(0)).optional()?.flatten())
}

/// Ticker.asset_ids_by_name: `(NAME, asset id)` for every asset the venue lists under one of these names, by its
/// spelling there (a replaced listing's prefix removed) or by the asset's symbol. Names arrive upper-cased.
pub fn asset_ids_by_name(c: &Connection, exchange_id: i64, wanted: &[String]) -> Result<Vec<(String, i64)>, FiguresError> {
    if wanted.is_empty() { return Ok(vec![]); }
    budget::charge(wanted.len() as u64, 0)?;
    let names = Value::from(wanted.to_vec()).to_string();
    let mut found: Vec<(String, i64)> = vec![];
    let mut seen = HashSet::new();
    let mut wanted_set = HashSet::new();
    for name in wanted { budget::charge(1, 0)?; wanted_set.insert(name.as_str()); }
    let mut add = |name: String, asset_id: i64| if seen.insert((name.clone(), asset_id)) { found.push((name, asset_id)); };
    let mut statement = c.prepare(
        "SELECT base, base_asset_id FROM tickers WHERE exchange_id = ?1 \
         AND (upper(base) IN (SELECT value FROM json_each(?2)) OR base LIKE '\\_\\_stale\\_%' ESCAPE '\\')")?;
    for row in statement.query_map(params![exchange_id, names], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        budget::charge(1, 0)?;
        let (base, asset_id) = row?;
        let spelling = base_spelling(&base).to_uppercase();
        if wanted_set.contains(spelling.as_str()) { add(spelling, asset_id); }
    }
    let mut statement = c.prepare(
        "SELECT upper(a.symbol), t.base_asset_id FROM tickers t INNER JOIN assets a ON a.id = t.base_asset_id \
         WHERE t.exchange_id = ?1 AND upper(a.symbol) IN (SELECT value FROM json_each(?2))")?;
    for row in statement.query_map(params![exchange_id, names], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        budget::charge(1, 0)?;
        let (name, asset_id) = row?;
        add(name, asset_id);
    }
    Ok(found)
}

/// Ticker#base_spelling: the venue's name for the base, without the `__stale_<id>_` prefix of a replaced listing.
pub fn base_spelling(base: &str) -> &str {
    let Some(rest) = base.strip_prefix("__stale_") else { return base };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    match rest[digits..].strip_prefix('_') { Some(name) if digits > 0 => name, _ => base }
}

/// What the figures need to know about the account's owner.
#[derive(Clone, Debug)]
pub struct User { pub id: i64, pub time_zone: String, pub display_currency: String, pub hide_balances: bool }

pub fn user(c: &Connection, id: i64) -> Result<User, FiguresError> {
    c.query_row("SELECT time_zone, display_currency, hide_balances FROM users WHERE id = ?1", [id], |r| {
        Ok(User { id, time_zone: r.get(0)?, display_currency: r.get(1)?, hide_balances: r.get(2)? })
    }).optional()?.ok_or_else(|| FiguresError::Data(format!("no user {id}")))
}

/// `user.bots.not_deleted`: the ids and types of the account's bots, archived ones included.
pub fn account_bots(c: &Connection, user_id: i64) -> Result<Vec<(i64, Option<String>)>, FiguresError> {
    let mut statement = c.prepare("SELECT id, type FROM bots WHERE user_id = ?1 AND status != ?2")?;
    let rows = statement.query_map(params![user_id, BotStatus::Deleted as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.map(|row| { budget::charge(1, 0)?; Ok(row?) }).collect::<Result<Vec<_>, FiguresError>>()?;
    Ok(rows)
}

/// Everything about one bot the figures are computed from, read once.
#[derive(Clone, Debug)]
pub struct Subject {
    pub bot: Bot,
    pub orders: Vec<Order>,
    pub tickers: Vec<Ticker>,
    /// `bot.quote_asset&.symbol`.
    pub quote: Option<String>,
}

impl Subject {
    pub fn load(c: &Connection, bot_id: i64) -> Result<Subject, FiguresError> {
        budget::within(|| {
            let bot = bot(c, bot_id)?;
            Ok(Subject { orders: orders(c, bot_id)?, tickers: tickers(c, &bot)?, quote: asset_symbol(c, bot.quote_asset_id)?, bot })
        })
    }

    /// Exchange#restated_candles?: whether the venue rewrites this ticker's price history behind us. Alpaca does
    /// for everything but crypto; no other venue does.
    pub fn restated_candles(&self, ticker: &Ticker) -> bool {
        self.bot.exchange_type.as_deref() == Some("Exchanges::Alpaca") && ticker.base_category.as_deref() != Some("Cryptocurrency")
    }
}
