//! What the figures read from the database: SELECTs only, shaped as Rails' own queries are.
use super::at::At;
use super::FiguresError;
use crate::enums::{BotStatus, TxExternalStatus, TxSide, TxStatus};
use super::dec::Dec;
use rusqlite::{params, Connection, OptionalExtension, Row as SqlRow};
use serde_json::Value;

fn data(what: impl std::fmt::Debug) -> FiguresError { FiguresError::Data(format!("{what:?}")) }
/// A decimal column: the one way a number of the database comes in, measured (`dec`).
fn decimal(r: &SqlRow<'_>, i: usize) -> Result<Option<Dec>, FiguresError> { Ok(Dec::from_sql(r.get_ref(i)?)?) }
fn instant(text: &str) -> Result<At, FiguresError> { At::from_sql(text).ok_or_else(|| FiguresError::Data(format!("a time Rails did not write: {text:?}"))) }
/// Ids as SQL literals, so each query has the shape of the one Rails sends (`IN (1, 2, 3)`); they are integers.
fn id_list(ids: &[i64]) -> String { ids.iter().map(i64::to_string).collect::<Vec<_>>().join(", ") }

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

/// String#to_i / Integer: the leading digits, else zero.
fn to_i(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n.as_i64().unwrap_or(0),
        Value::String(s) => {
            let s = s.trim_start();
            let end = s.char_indices().take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && (*c == '-' || *c == '+'))).count();
            s[..end].parse().unwrap_or(0)
        }
        _ => 0,
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
        (Kind::Basket, Some(weights)) if !weights.is_empty() => weights.keys().map(|k| to_i(&Value::String(k.clone()))).collect(),
        (Kind::Basket, _) => settings.get("base_asset_ids").and_then(Value::as_array).map(|ids| ids.iter().map(to_i).collect()).unwrap_or_default(),
    };
    let exchange_type = match exchange_id {
        Some(e) => c.query_row("SELECT type FROM exchanges WHERE id = ?1", [e], |r| r.get::<_, Option<String>>(0)).optional()?.flatten(),
        None => None,
    };
    let quote_asset_id = settings.get("quote_asset_id").filter(|v| !v.is_null()).map(to_i);
    Ok(Bot { id, user_id: user_id.unwrap_or(0), exchange_id: exchange_id.filter(|_| exchange_type.is_some()), kind, exchange_type, quote_asset_id, base_asset_ids })
}

/// One submitted order, as the walks pluck it.
#[derive(Clone, Debug)]
pub struct Order {
    pub id: i64, pub at: At, pub exchange_id: Option<i64>,
    pub price: Option<Dec>, pub amount: Option<Dec>, pub amount_exec: Option<Dec>, pub quote_amount_exec: Option<Dec>,
    pub base: Option<String>, pub asset_id: Option<i64>,
    pub sell: bool, pub buy: bool, pub closed: bool,
    /// transactions.transaction_type: REGULAR, REBALANCE, LIQUIDATION, REDEPLOY.
    pub kind: String,
}

/// `transactions.submitted.order(:created_at, :id)`: every walk reads this one list.
pub fn orders(c: &Connection, bot_id: i64) -> Result<Vec<Order>, FiguresError> {
    let mut statement = c.prepare(
        "SELECT id, created_at, exchange_id, price, amount, amount_exec, quote_amount_exec, base, base_asset_id, side, external_status, transaction_type \
         FROM transactions WHERE bot_id = ?1 AND status = ?2 ORDER BY created_at ASC, id ASC")?;
    let mut rows = statement.query(params![bot_id, TxStatus::Submitted as i64])?;
    let mut out = vec![];
    while let Some(r) = rows.next()? {
        let (side, status): (Option<i64>, Option<i64>) = (r.get(9)?, r.get(10)?);
        out.push(Order {
            id: r.get(0)?, at: instant(&r.get::<_, String>(1)?)?, exchange_id: r.get(2)?,
            price: decimal(r, 3)?, amount: decimal(r, 4)?, amount_exec: decimal(r, 5)?, quote_amount_exec: decimal(r, 6)?,
            base: r.get(7)?, asset_id: r.get(8)?, sell: side == Some(TxSide::Sell as i64), buy: side == Some(TxSide::Buy as i64),
            closed: status == Some(TxExternalStatus::Closed as i64),
            kind: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
        });
    }
    Ok(out)
}

/// An asset's `(id, symbol, name)`.
pub type AssetName = (i64, Option<String>, Option<String>);

/// `Asset.where(id: ids).pluck(:id, :symbol, :name)`.
pub fn asset_names(c: &Connection, ids: &[i64]) -> Result<Vec<AssetName>, FiguresError> {
    let mut statement = c.prepare(&format!("SELECT id, symbol, name FROM assets WHERE id IN ({})", id_list(ids)))?;
    let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
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
        Kind::Index => c.prepare(&format!("{TICKER} WHERE {live}"))?.query_map(params![exchange_id, quote], ticker)?.collect::<Result<Vec<_>, _>>()?,
        Kind::Basket => {
            let mut ids = bot.base_asset_ids.clone();
            let mut statement = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1")?;
            for id in statement.query_map([bot.id], |r| r.get::<_, i64>(0))? { ids.push(id?); }
            c.prepare(&format!("{TICKER} WHERE {live} AND t.base_asset_id IN ({})", id_list(&ids)))?
                .query_map(params![exchange_id, quote], ticker)?.collect::<Result<Vec<_>, _>>()?
        }
    };
    Ok(rows)
}

/// The precision of the tickers the composition trades right now, tradable or not (Allocatable#composition_tickers):
/// what a basket rounds with once none of its members is tradable.
pub fn composition_quote_decimals(c: &Connection, bot_id: i64) -> Result<Vec<Option<i64>>, FiguresError> {
    let mut statement = c.prepare("SELECT t.quote_decimals FROM bot_index_assets m INNER JOIN tickers t ON t.id = m.ticker_id WHERE m.bot_id = ?1 AND m.in_index = 1")?;
    let rows = statement.query_map([bot_id], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// `Ticker.where(exchange_id:, base_asset_id: ids)` with each base asset's symbol: every listing of these assets on
/// the venue, tradable or not, as `(base, asset id, symbol)`.
pub fn listings(c: &Connection, exchange_id: i64, asset_ids: &[i64]) -> Result<Vec<(String, i64, Option<String>)>, FiguresError> {
    let mut statement = c.prepare(&format!("SELECT base, base_asset_id FROM tickers WHERE exchange_id = ?1 AND base_asset_id IN ({})", id_list(asset_ids)))?;
    let rows = statement.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    let names = asset_names(c, asset_ids)?;
    Ok(rows.into_iter().map(|(base, id)| { let symbol = names.iter().find(|(asset, _, _)| *asset == id).and_then(|(_, symbol, _)| symbol.clone()); (base, id, symbol) }).collect())
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
pub struct SplitRow { pub exchange_id: i64, pub base_currency: String, pub raw_data: Value, pub at: At }

/// The `adjustment` rows marked `corporate_action: split` on these venues (Bot::Restatable#grouped_split_rows).
pub fn split_rows(c: &Connection, user_id: i64, exchange_ids: &[i64]) -> Result<Vec<SplitRow>, FiguresError> {
    let mut statement = c.prepare(&format!(
        "SELECT exchange_id, base_currency, raw_data, transacted_at FROM account_transactions \
         WHERE user_id = ?1 AND entry_type = ?2 AND exchange_id IN ({}) \
         AND CASE WHEN json_valid(raw_data) THEN json_extract(raw_data, '$.corporate_action') END = 'split'", id_list(exchange_ids)))?;
    let mut rows = statement.query(params![user_id, ADJUSTMENT])?;
    let mut out = vec![];
    while let Some(r) = rows.next()? {
        let raw: Option<String> = r.get(2)?;
        out.push(SplitRow {
            exchange_id: r.get(0)?, base_currency: r.get(1)?,
            raw_data: raw.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null),
            at: instant(&r.get::<_, String>(3)?)?,
        });
    }
    Ok(out)
}

/// Ticker.asset_ids_by_name: `(NAME, asset id)` for every asset the venue lists under one of these names, by its
/// spelling there (a replaced listing's prefix removed) or by the asset's symbol. Names arrive upper-cased.
pub fn asset_ids_by_name(c: &Connection, exchange_id: i64, wanted: &[String]) -> Result<Vec<(String, i64)>, FiguresError> {
    if wanted.is_empty() { return Ok(vec![]); }
    let names = Value::from(wanted.to_vec()).to_string();
    let mut found: Vec<(String, i64)> = vec![];
    let mut add = |name: String, asset_id: i64| if !found.contains(&(name.clone(), asset_id)) { found.push((name, asset_id)); };
    let mut statement = c.prepare(
        "SELECT base, base_asset_id FROM tickers WHERE exchange_id = ?1 \
         AND (upper(base) IN (SELECT value FROM json_each(?2)) OR base LIKE '\\_\\_stale\\_%' ESCAPE '\\')")?;
    for row in statement.query_map(params![exchange_id, names], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (base, asset_id) = row?;
        let spelling = base_spelling(&base).to_uppercase();
        if wanted.contains(&spelling) { add(spelling, asset_id); }
    }
    let mut statement = c.prepare(
        "SELECT upper(a.symbol), t.base_asset_id FROM tickers t INNER JOIN assets a ON a.id = t.base_asset_id \
         WHERE t.exchange_id = ?1 AND upper(a.symbol) IN (SELECT value FROM json_each(?2))")?;
    for row in statement.query_map(params![exchange_id, names], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
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
    let rows = statement.query_map(params![user_id, BotStatus::Deleted as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
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
        let bot = bot(c, bot_id)?;
        Ok(Subject { orders: orders(c, bot_id)?, tickers: tickers(c, &bot)?, quote: asset_symbol(c, bot.quote_asset_id)?, bot })
    }

    /// Exchange#restated_candles?: whether the venue rewrites this ticker's price history behind us. Alpaca does
    /// for everything but crypto; no other venue does.
    pub fn restated_candles(&self, ticker: &Ticker) -> bool {
        self.bot.exchange_type.as_deref() == Some("Exchanges::Alpaca") && ticker.base_category.as_deref() != Some("Cryptocurrency")
    }
}
