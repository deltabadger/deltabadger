//! AccountBalance::SyncJob for an Alpaca key: Exchanges::Alpaca#get_balances (GET /v2/account, GET /v2/positions),
//! then AccountBalance::Sync#sync! (USD prices, the `account_balances` rows, removal of what is gone, the key's
//! `balances_synced_at`). Rails is the oracle (rust/tests/sync_parity.rs).
//!
//! Where a USD price comes from, as on a hosted instance:
//! - cash (asset category Fiat or Currency): USD is 1;
//! - a stock or ETF (category Stock): Alpaca's own latest trade, GET /v2/stocks/snapshots on data.alpaca.markets;
//! - everything else (coins), and a stock Alpaca gave no snapshot or no latest trade for: the market-data source, by `assets.external_id`
//!   (hosted: the data API's GET api/v1/prices).
use super::activities::{CryptoPairs, Raw};
use crate::jobs::data_api::{ApiError, PriceFuture, PriceSource};
use crate::jobs::Db;
use super::wire::{self, Budget, Node};
use super::{commit, load_key, number, parsed, phase, record_sync_error, sql_time, venue_failure, Failure, SyncError, Unread, LIVE_REFUSED};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::ruby::BigDec;
use crate::venue::alpaca::AlpacaVenue;
use crate::venue::http::Transport;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

pub const SNAPSHOTS_PATH: &str = "/v2/stocks/snapshots";
pub const PRICES_PATH: &str = "/api/v1/prices";
/// MarketData.get_prices' failure on a hosted instance whose data API did not answer (no CoinGecko key to fall to).
pub const NO_PROVIDER: &str = "No market data provider available for prices";
/// What one answer may hold, in bytes: the account is one small object; the positions and the snapshots have one
/// entry per holding. Past it, or past `MAX_POSITIONS` positions, the sync fails and nothing is written.
pub const MAX_ACCOUNT_BYTES: usize = 64 * 1024;
pub const MAX_LIST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_POSITIONS: usize = 5_000;
/// The values and keys one answer may hold, counted while it is read: an account has about 150, a position about 30.
pub const MAX_ACCOUNT_NODES: usize = 2_000;
pub const MAX_LIST_NODES: usize = 300_000;
/// Balance rows per write transaction.
pub const BATCH: usize = 100;
/// Fiat::CATEGORIES: the two asset categories that mean cash.
const CASH_CATEGORIES: [&str; 2] = ["Fiat", "Currency"];

/// No market-data source connected: every lookup fails as Rails' does with no provider.
#[derive(Clone, Copy)]
pub struct NoPrices;
impl PriceSource for NoPrices {
    fn prices<'a>(&'a self, _: &'a [String], _: &'a str) -> PriceFuture<'a> {
        Box::pin(async { Err(ApiError::Failed { status: None, message: NO_PROVIDER.into() }) })
    }
}

/// The data API's answer to GET api/v1/prices, read as MarketData.get_prices reads it: `data[id]["usd"]`, `to_f`,
/// absent when missing or null.
/// ponytail: String#to_f reads a numeric prefix of garbage; this reads garbage as 0. The data API sends JSON numbers.
pub fn parse_prices(body: &Value, external_ids: &[String]) -> BTreeMap<String, f64> {
    external_ids.iter().filter_map(|id| {
        let price = match &body["data"][id.as_str()]["usd"] {
            Value::Number(n) => n.as_f64()?,
            Value::String(s) => s.trim().parse().unwrap_or(0.0),
            _ => return None,
        };
        Some((id.clone(), price))
    }).collect()
}

/// Recorded answers for "GET /api/v1/prices", in the shape of http::ScriptedTransport's script (the parity harness
/// serves the same replies beneath Rails' Clients::MarketData). The last reply repeats; an unscripted call panics.
#[derive(Default)]
pub struct ScriptedPrices { replies: RefCell<VecDeque<Value>>, requests: RefCell<Vec<Vec<String>>> }

impl ScriptedPrices {
    pub fn from_script(script: &Value) -> Self {
        Self { replies: RefCell::new(script[format!("GET {PRICES_PATH}")].as_array().cloned().unwrap_or_default().into()), requests: RefCell::default() }
    }
    /// The id lists asked for, in order.
    pub fn requests(&self) -> Vec<Vec<String>> { self.requests.borrow().clone() }
}

impl PriceSource for ScriptedPrices {
    fn prices<'a>(&'a self, external_ids: &'a [String], _currency: &'a str) -> PriceFuture<'a> {
        Box::pin(async move {
            self.requests.borrow_mut().push(external_ids.to_vec());
            let reply = {
                let mut q = self.replies.borrow_mut();
                if q.is_empty() { panic!("unscripted call GET {PRICES_PATH}"); }
                if q.len() > 1 { q.pop_front().unwrap_or(Value::Null) } else { q[0].clone() }
            };
            let message = reply["message"].as_str().unwrap_or_default().to_string();
            match (reply["network"].as_str(), reply["status"].as_u64().unwrap_or(200)) {
                // A certificate failure is returned by Rails' client, not raised.
                (Some("permanent"), _) => Err(ApiError::Failed { status: None, message }),
                (Some(_), _) => Err(ApiError::Transient(message)),
                (None, status) if !(200..300).contains(&status) => Err(ApiError::Failed { status: Some(status as u16), message: reply["body"].to_string() }),
                (None, _) => Ok(parse_prices(&reply["body"], external_ids)),
            }
        })
    }
}

/// AccountBalance::Sync::Summary.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub synced: usize,
    pub priced_fresh: usize,
    pub priced_stale: usize,
    pub unpriced: usize,
    /// Why the venue's or the market's prices could not be read, when the sync went on without them.
    pub pricing_error: Option<String>,
}

pub(crate) struct Catalog {
    user_id: i64, exchange_id: i64,
    /// `exchange.assets.pluck(:id)`: the assets the venue lists, in the order the rows are written.
    asset_ids: Vec<i64>,
    /// #asset_from_symbol: a ticker's base or quote name → its asset; the first ticker wins a name.
    by_symbol: HashMap<String, i64>,
    pairs: CryptoPairs,
}

struct Asset { id: i64, external_id: String, symbol: Option<String>, category: Option<String> }

/// A fresh price as Rails holds it: the market's is a Float, the venue's and cash are BigDecimals.
enum Price { Float(f64), Decimal(BigDec) }

/// The account as #get_balances needs it: an object whose `cash` is a number. Rails reads a missing or null `cash`
/// as 0 (`nil.to_d`), removes the cash balance and reports success; here it is a malformed answer. The outer error
/// is an answer that could not be read at all; the inner one is the account's shape, judged once both answers are in.
pub(crate) fn account(text: &str) -> Result<Result<BigDec, String>, Unread> {
    let node = wire::read(text, &mut Budget(MAX_ACCOUNT_NODES), None).map_err(|r| Unread::refused(r, "an account"))?;
    let cash = |account: Raw| account.number("cash")?.ok_or_else(|| "the account has no cash figure".to_string());
    Ok(Raw::from_node(&node).map_err(|_| "unreadable account".to_string()).and_then(cash))
}

/// The positions as #get_balances needs them: an array of at most `MAX_POSITIONS` objects (the reader stops at the
/// next one), each with a `symbol` and a `qty` that is a number. Rails skips a position with no symbol and reads a
/// missing `qty` as 0, which removes that holding's balance; here either is a malformed answer.
pub(crate) fn positions(text: &str) -> Result<Result<Vec<(String, BigDec)>, String>, Unread> {
    let node = match wire::read(text, &mut Budget(MAX_LIST_NODES), Some(MAX_POSITIONS)) {
        Err(wire::Refused::TooManyItems) => return Err(Unread::Raised(format!("more than {MAX_POSITIONS} positions"))),
        other => other.map_err(|r| Unread::refused(r, "positions"))?,
    };
    let Node::Array(items) = node else { return Ok(Err("unreadable positions".into())) };
    Ok(items.iter().map(|item| {
        let position = Raw::from_node(item).map_err(|_| "unreadable position".to_string())?;
        let symbol = position.value["symbol"].as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| "a position without a symbol".to_string())?;
        let qty = position.number("qty")?.ok_or_else(|| "a position without a quantity".to_string())?;
        Ok((symbol.to_string(), qty))
    }).collect())
}

/// One balance sync of one Alpaca key, as AccountBalance::SyncJob runs it for that key. Idempotent: the rows are
/// upserted on (user, exchange, asset) and a second run at the same instant changes nothing.
///
/// `Ok(Err(failure))`: the balances or a price could not be read; no row was touched, `balances_synced_at` did not
/// move, the key's `last_sync_error` holds `failure.error`, and an "unauthorized" answer has marked the key incorrect.
/// A balance sync never clears `last_sync_error` (the ledger sync does).
///
/// Every answer is validated whole before one row is written: a malformed account or position fails the sync and
/// removes nothing. The rows are then written in units of `BATCH`; `balances_synced_at` moves in the last one.
pub async fn sync<T: Transport>(db: &Db, venue: &AlpacaVenue<T>, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                -> Result<Result<Summary, Failure>, SyncError> {
    let catalog = phase(db, move |c| catalog(c, key_id)).await?;
    // A failure the job only records (`condemn`: #handle_api_key_failure marks a key Alpaca calls unauthorized incorrect).
    let fail = |text: String, condemn: bool| {
        let (creds, now) = (credentials.clone(), clock.now());
        async move {
            let error = commit(db, move |c| {
                if condemn && text.contains("unauthorized") {
                    c.execute("UPDATE api_keys SET status = 2, updated_at = ?1 WHERE id = ?2 AND status != 2", params![sql_time(now), key_id])?;
                }
                record_sync_error(c, key_id, &text, &creds)
            }).await?;
            Ok::<_, SyncError>(Err(Failure { error, raised: false }))
        }
    };
    if super::live(credentials) { return fail(LIVE_REFUSED.into(), false).await; }

    // #get_balances: the account, then the positions. A failed request, or an answer that is not JSON, ends it there
    // (a failure the venue returned is its word on the key, `condemn`; a transport failure is not). Both answers are
    // then validated whole, the account first, as Ruby raises only once it uses them.
    let cash = match venue.read(false, "/v2/account", vec![], MAX_ACCOUNT_BYTES).await.map_err(venue_failure) {
        Err((text, raised)) => return fail(text, !raised).await,
        Ok(body) => match parsed(body, account).await { Ok(checked) => checked, Err((text, raised)) => return fail(text, !raised).await },
    };
    let held = match venue.read(false, "/v2/positions", vec![], MAX_LIST_BYTES).await.map_err(venue_failure) {
        Err((text, raised)) => return fail(text, !raised).await,
        Ok(body) => match parsed(body, positions).await { Ok(checked) => checked, Err((text, raised)) => return fail(text, !raised).await },
    };
    let (cash, held) = match (cash, held) {
        (Ok(cash), Ok(held)) => (cash, held),
        (Err(why), _) | (_, Err(why)) => return fail(why, false).await,
    };
    let held = holdings(&catalog, cash, held);

    let ids: Vec<i64> = held.iter().map(|(id, _)| *id).collect();
    let assets = Arc::new(phase(db, move |c| assets(c, &ids)).await?);

    // The venue's own quotes for stocks; a failure there sends the stocks to the market source with everything else.
    let mut fresh: HashMap<String, Price> = HashMap::new();
    let mut errors: Vec<String> = vec![];
    let mut symbols: Vec<String> = assets.iter().filter(|a| a.category.as_deref() == Some("Stock")).filter_map(|a| a.symbol.clone()).collect();
    symbols.sort_unstable();
    symbols.dedup();
    if !symbols.is_empty() {
        let read = match venue.read(true, SNAPSHOTS_PATH, vec![("symbols", symbols.join(","))], MAX_LIST_BYTES).await.map_err(venue_failure) {
            Err(failure) => Err(failure),
            Ok(body) => parsed(body, move |text| snapshot_prices(text, &symbols)).await,
        };
        match read {
            Ok(prices) => {
                for a in assets.iter().filter(|a| a.category.as_deref() == Some("Stock")) {
                    if let Some(price) = a.symbol.as_ref().and_then(|s| prices.get(s)) { fresh.insert(a.external_id.clone(), Price::Decimal(price.clone())); }
                }
            }
            Err((text, true)) => return fail(text, false).await,
            Err((text, false)) => errors.push(text),
        }
    }
    // Cash is decided by category, never by symbol, and wins over every feed: a dollar is worth a dollar.
    let cash = |a: &Asset| a.category.as_deref().is_some_and(|c| CASH_CATEGORIES.contains(&c)) && a.symbol.as_deref() == Some("USD");
    let mut remaining: Vec<String> = vec![];
    for a in assets.iter().filter(|a| !cash(a) && !fresh.contains_key(&a.external_id)) {
        if !remaining.contains(&a.external_id) { remaining.push(a.external_id.clone()); }
    }
    if !remaining.is_empty() {
        match prices.prices(&remaining, "usd").await {
            Ok(found) => { for (id, p) in found { fresh.entry(id).or_insert(Price::Float(p)); } }
            // Rails' client raises on a transport failure a retry could fix, and the job records it and stops.
            Err(ApiError::Transient(text)) => return fail(format!("Client::TransientNetworkError: {text}"), false).await,
            // Any other failure: hosted Rails has no CoinGecko key to fall to, and goes on with its last prices.
            Err(ApiError::Failed { .. }) => errors.push(NO_PROVIDER.into()),
        }
    }
    for a in assets.iter().filter(|a| cash(a)) { fresh.insert(a.external_id.clone(), Price::Decimal(BigDec::one())); }

    // Every price, and every value a row will hold (from a fresh price or the last stored one), within a venue
    // number's caps before the first write. Rails has no such check: a price of 1e300 on a billion coins would be
    // stored as an infinite value and reported as a success. Here the sync fails and writes no balance.
    if !fresh.values().all(|p| within(price_f(p))) { return fail(PRICE_OUT_OF_RANGE.into(), false).await; }
    let (user_id, exchange_id) = (catalog.user_id, catalog.exchange_id);
    let last = phase(db, move |c| last_prices(c, user_id, exchange_id)).await?;
    for (asset_id, total) in &held {
        let Some(asset) = assets.iter().find(|a| a.id == *asset_id) else { continue };
        let price = match fresh.get(&asset.external_id) {
            Some(Price::Float(f)) => BigDec::from_f64(*f).ok(),
            Some(Price::Decimal(d)) => Some(d.clone()),
            None => last.get(asset_id).cloned().flatten(),
        };
        if price.is_some_and(|p| !within(value_of(total, &p))) { return fail(VALUE_OUT_OF_RANGE.into(), false).await; }
    }

    // The rows, `BATCH` at a time; then what is gone, and the key's clock in the last unit.
    let now = clock.now();
    let (held, fresh) = (Arc::new(held), Arc::new(fresh));
    let mut summary = Summary::default();
    for from in (0..held.len()).step_by(BATCH) {
        let (held, assets, fresh) = (held.clone(), assets.clone(), fresh.clone());
        summary = commit(db, move |c| upsert(c, user_id, exchange_id, &held[from..(from + BATCH).min(held.len())], &assets, &fresh, summary, now)).await?;
    }
    let kept: HashSet<i64> = held.iter().map(|(id, _)| *id).collect();
    let mut gone = phase(db, move |c| gone(c, user_id, exchange_id, &kept)).await?;
    loop {
        let rest = gone.split_off(gone.len().min(BATCH));
        let last = rest.is_empty();
        commit(db, move |c| {
            for id in &gone { c.execute("DELETE FROM account_balances WHERE id = ?1", [id])?; }
            if last { c.execute("UPDATE api_keys SET balances_synced_at = ?1 WHERE id = ?2", params![sql_time(now), key_id])?; }
            Ok(())
        }).await?;
        if last { break; }
        gone = rest;
    }
    summary.synced = held.len();
    summary.pricing_error = (!errors.is_empty()).then(|| errors.join("; "));
    Ok(Ok(summary))
}

/// Alpaca's snapshots, reduced to the latest trade price of each asked symbol that has one. A snapshot with no latest
/// trade (or a price of 0) is no venue price: the stock is asked of the market source, like one with no snapshot.
/// Rails reads it as 0 (`nil.to_d`) and values the holding at nothing with a fresh `priced_at` (a listed divergence).
fn snapshot_prices(text: &str, symbols: &[String]) -> Result<HashMap<String, BigDec>, Unread> {
    let node = wire::read(text, &mut Budget(MAX_LIST_NODES), None).map_err(|r| Unread::refused(r, "snapshots"))?;
    let snapshots = Raw::from_node(&node).map_err(|_| Unread::Raised("unreadable snapshots".into()))?;
    let mut prices = HashMap::new();
    for symbol in symbols {
        let unreadable = |_| Unread::Raised(format!("unreadable snapshot for {symbol}"));
        let Some(snapshot) = snapshots.object(symbol).map_err(unreadable)? else { continue };
        let Some(trade) = snapshot.object("latestTrade").map_err(unreadable)? else { continue };
        // Rails omits just the symbol whose price is unreadable. Keep the healthy prices in this batch;
        // the omitted holding can use the market source or its previous price. Numeric caps still apply.
        if let Some(price) = trade.number("p").ok().flatten().filter(BigDec::is_positive) { prices.insert(symbol.clone(), price); }
    }
    Ok(prices)
}

fn catalog(c: &Connection, key_id: i64) -> Result<Catalog, SyncError> {
    let key = load_key(c, key_id)?;
    // AccountBalance::SyncJob syncs reading keys only: status correct, never a withdrawal key.
    if key.status != 1 || key.key_type == 1 { return Err(SyncError(format!("api key {key_id} is not a reading key"))); }
    catalog_for(c, key.user_id, key.exchange_id)
}

/// What #get_balances reads about the venue: the assets it lists and how a position's symbol names one.
pub(crate) fn catalog_for(c: &Connection, user_id: i64, exchange_id: i64) -> Result<Catalog, SyncError> {
    let mut s = c.prepare("SELECT assets.id FROM assets INNER JOIN exchange_assets ON assets.id = exchange_assets.asset_id WHERE exchange_assets.exchange_id = ?1 ORDER BY exchange_assets.id")?;
    let asset_ids = s.query_map([exchange_id], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    let mut by_symbol = HashMap::new();
    let mut s = c.prepare("SELECT base, quote, base_asset_id, quote_asset_id FROM tickers WHERE exchange_id = ?1 AND available = 1 ORDER BY id")?;
    for row in s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))? {
        let (base, quote, base_asset, quote_asset) = row?;
        by_symbol.entry(base).or_insert(base_asset);
        by_symbol.entry(quote).or_insert(quote_asset);
    }
    Ok(Catalog { user_id, exchange_id, asset_ids, by_symbol, pairs: super::ledger::crypto_pairs(c, exchange_id)? })
}

/// #get_balances' answer, reduced to what AccountBalance::Sync keeps: the listed assets with a positive quantity, in
/// listing order. Settled cash is USD's quantity; a position's `qty` is its asset's (a stock by its symbol, a coin by
/// its compact pair). A later position for the same asset replaces an earlier one.
fn holdings(catalog: &Catalog, cash: BigDec, positions: Vec<(String, BigDec)>) -> Vec<(i64, BigDec)> {
    balances(catalog, cash, positions).into_iter().filter(|(_, qty)| qty.is_positive()).collect()
}

/// #get_balances' answer: every listed asset with the quantity the answers gave it (cash for USD, a position's `qty`),
/// in listing order; an asset nothing named is left out, as its zero would be.
pub(crate) fn balances(catalog: &Catalog, cash: BigDec, positions: Vec<(String, BigDec)>) -> Vec<(i64, BigDec)> {
    let mut free: HashMap<i64, BigDec> = HashMap::new();
    let listed = |id: &i64| catalog.asset_ids.contains(id);
    if let Some(usd) = catalog.by_symbol.get("USD").filter(|id| listed(id)) { free.insert(*usd, cash); }
    for (symbol, qty) in positions {
        let asset = catalog.by_symbol.get(&symbol).copied().or_else(|| catalog.pairs.0.get(&symbol).map(|pair| pair.base_asset_id));
        if let Some(asset) = asset.filter(listed) { free.insert(asset, qty); }
    }
    catalog.asset_ids.iter().filter_map(|id| free.remove(id).map(|qty| (*id, qty))).collect()
}

/// Read-only completeness check; sync's existing partial-snapshot semantics stay unchanged.
pub(crate) fn complete_balances(catalog: &Catalog, cash: BigDec, positions: Vec<(String, BigDec)>) -> Result<Vec<(i64, BigDec)>, &'static str> {
    let listed = |id: &i64| catalog.asset_ids.contains(id);
    if !cash.is_zero() && catalog.by_symbol.get("USD").filter(|id| listed(id)).is_none() {
        return Err("Balances could not be fully read: unmapped nonzero cash");
    }
    for (symbol, qty) in &positions {
        let asset = catalog.by_symbol.get(symbol).copied().or_else(|| catalog.pairs.0.get(symbol).map(|pair| pair.base_asset_id));
        if !qty.is_zero() && asset.filter(listed).is_none() {
            return Err("Balances could not be fully read: unmapped nonzero position");
        }
    }
    Ok(balances(catalog, cash, positions))
}

fn assets(c: &Connection, ids: &[i64]) -> Result<Vec<Asset>, SyncError> {
    let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
    let mut s = c.prepare(&format!("SELECT id, external_id, symbol, category FROM assets WHERE id IN ({list}) ORDER BY id"))?;
    let rows = s.query_map([], |r| Ok(Asset { id: r.get(0)?, external_id: r.get(1)?, symbol: r.get(2)?, category: r.get(3)? }))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// ActiveModel::Type::Decimal with a scale, for a BigDecimal: rounded half up to `scale` places.
pub fn cast_decimal(d: &BigDec, scale: i64) -> BigDec { d.round(places(scale)) }

/// `BigDec::round` takes its places as a u8 (since #451). The scales here are the schema's, 8 and 16.
fn places<T: TryInto<u8>>(scale: T) -> u8 { scale.try_into().unwrap_or(u8::MAX) }

/// How that type reads a stored value back: SQLite keeps a decimal as INTEGER or REAL, and a REAL is cast as a Float.
fn stored_decimal(v: rusqlite::types::ValueRef<'_>, scale: i32) -> Result<Option<BigDec>, SyncError> {
    Ok(match v {
        rusqlite::types::ValueRef::Real(f) => Some(cast_float(f, scale).ok_or_else(|| SyncError(format!("{f} in a decimal column")))?),
        other => number::stored(other).map_err(SyncError)?.map(|d| d.round(places(scale))),
    })
}

/// The same type for a Float (precision 20, so 16 significant digits): `BigDecimal(float.round(scale), 16)`, then the
/// scale. `None` for a float that is not finite.
pub fn cast_float(f: f64, scale: i32) -> Option<BigDec> {
    let rounded = float_round(f, scale);
    BigDec::parse(&format!("{rounded:.15e}")).ok().map(|d| d.round(places(scale)))
}

/// Float#round(ndigits) for ndigits in 1..=14 (numeric.c: flo_round, round_half_up): half up, in double arithmetic.
pub fn float_round(number: f64, ndigits: i32) -> f64 {
    if number == 0.0 || !number.is_finite() { return number; }
    let binexp = { // frexp's exponent: number = m × 2^binexp with 0.5 <= |m| < 1
        let e = ((number.to_bits() >> 52) & 0x7ff) as i32;
        if e == 0 { (((number * 2f64.powi(54)).to_bits() >> 52) & 0x7ff) as i32 - 1022 - 54 } else { e - 1022 }
    };
    if ndigits >= 17 - (if binexp > 0 { binexp / 4 } else { binexp / 3 - 1 }) { return number; } // more digits than a double holds
    if number > 0.0 && ndigits < -(if binexp > 0 { binexp / 3 + 1 } else { binexp / 4 }) { return 0.0; }
    let s = 10f64.powi(ndigits);
    let mut f = (number * s).round();
    if number > 0.0 { if (f + 0.5) / s <= number { f += 1.0; } } else if (f - 0.5) / s >= number { f -= 1.0; }
    f / s
}

/// The user's balance rows on this venue for assets no longer held: AccountBalance::Sync#sync! deletes them.
fn gone(c: &Connection, user_id: i64, exchange_id: i64, kept: &HashSet<i64>) -> Result<Vec<i64>, SyncError> {
    let mut s = c.prepare("SELECT id, asset_id FROM account_balances WHERE user_id = ?1 AND exchange_id = ?2 ORDER BY id")?;
    let rows = s.query_map([user_id, exchange_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().filter(|(_, asset)| !kept.contains(asset)).map(|(id, _)| id).collect())
}

/// What a sync fails with when a price, or a value computed from one, is not a number within a venue number's caps.
pub const PRICE_OUT_OF_RANGE: &str = "a price outside a venue number's range";
pub const VALUE_OUT_OF_RANGE: &str = "a balance value outside a venue number's range";

/// Finite, and zero or within a venue number's caps (`number::VENUE`: 10^±40, 64 significant digits).
fn within(f: f64) -> bool { f.is_finite() && (f == 0.0 || number::decimal(&format!("{f:e}"), &number::VENUE).is_ok()) }
fn price_f(p: &Price) -> f64 { match p { Price::Float(f) => *f, Price::Decimal(d) => d.to_f() } }
/// `usd_value` as the row stores it: quantity × price, rounded to the column's scale, as a double.
fn value_of(total: &BigDec, price: &BigDec) -> f64 { cast_decimal(&(total * price), 8).to_f() }

/// The last stored price of each of the user's balances on this venue (the first row of an asset, as `upsert` reads it).
fn last_prices(c: &Connection, user_id: i64, exchange_id: i64) -> Result<HashMap<i64, Option<BigDec>>, SyncError> {
    let mut s = c.prepare("SELECT asset_id, usd_price FROM account_balances WHERE user_id = ?1 AND exchange_id = ?2 ORDER BY id")?;
    let mut rows = s.query([user_id, exchange_id])?;
    let mut last = HashMap::new();
    while let Some(r) = rows.next()? {
        let asset: i64 = r.get(0)?;
        if let std::collections::hash_map::Entry::Vacant(e) = last.entry(asset) { e.insert(stored_decimal(r.get_ref(1)?, 8)?); }
    }
    Ok(last)
}

/// AccountBalance::Sync#sync!'s write for at most `BATCH` holdings: each upserted on (user, exchange, asset).
#[allow(clippy::too_many_arguments)]
fn upsert(c: &Connection, user_id: i64, exchange_id: i64, held: &[(i64, BigDec)], assets: &[Asset], fresh: &HashMap<String, Price>, mut summary: Summary,
          now: DateTime<Utc>) -> Result<Summary, SyncError> {
    let at = sql_time(now);
    for (asset_id, total) in held {
        let Some(asset) = assets.iter().find(|a| a.id == *asset_id) else { continue };
        let existing = c.query_row("SELECT id, usd_price FROM account_balances WHERE user_id = ?1 AND exchange_id = ?2 AND asset_id = ?3 LIMIT 1",
                                   params![user_id, exchange_id, asset_id], |r| Ok((r.get::<_, i64>(0)?, stored_decimal(r.get_ref(1)?, 8)))).optional()?;
        let (id, old_price) = match existing { Some((id, price)) => (Some(id), price?), None => (None, None) };
        let free = cast_decimal(total, 16).to_f();
        // Checked before the first unit; checked again here, so no unit ever writes a value that is no number.
        let value = |price: &BigDec| Some(value_of(total, price)).filter(|v| within(*v)).ok_or_else(|| SyncError(VALUE_OUT_OF_RANGE.into()));
        match (fresh.get(&asset.external_id), old_price) {
            (Some(price), _) => {
                summary.priced_fresh += 1;
                let (stored, exact) = match price {
                    Price::Float(f) => (cast_float(*f, 8), BigDec::from_f64(*f).ok()),
                    Price::Decimal(d) => (Some(cast_decimal(d, 8)), Some(d.clone())),
                };
                let (Some(stored), Some(exact)) = (stored, exact) else { return Err(SyncError(format!("a price for {} that is not a number", asset.external_id))) };
                match id {
                    Some(id) => c.execute("UPDATE account_balances SET free = ?1, locked = 0, usd_price = ?2, priced_at = ?3, usd_value = ?4, synced_at = ?3, updated_at = ?3 WHERE id = ?5",
                                          params![free, stored.to_f(), at, value(&exact)?, id])?,
                    None => c.execute("INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, priced_at, usd_value, synced_at, created_at, updated_at) \
                                       VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?6, ?6, ?6)", params![user_id, exchange_id, asset_id, free, stored.to_f(), at, value(&exact)?])?,
                };
            }
            // Stale: the last price and its time stay as they are; the value follows the new quantity.
            (None, Some(price)) => {
                summary.priced_stale += 1;
                c.execute("UPDATE account_balances SET free = ?1, locked = 0, usd_value = ?2, synced_at = ?3, updated_at = ?3 WHERE id = ?4", params![free, value(&price)?, at, id])?;
            }
            (None, None) => {
                summary.unpriced += 1;
                match id {
                    Some(id) => c.execute("UPDATE account_balances SET free = ?1, locked = 0, usd_price = NULL, priced_at = NULL, usd_value = NULL, synced_at = ?2, updated_at = ?2 WHERE id = ?3",
                                          params![free, at, id])?,
                    None => c.execute("INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, synced_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5, ?5)",
                                      params![user_id, exchange_id, asset_id, free, at])?,
                };
            }
        }
    }
    Ok(summary)
}
