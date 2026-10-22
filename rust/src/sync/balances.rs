//! AccountBalance::SyncJob for an Alpaca key: Exchanges::Alpaca#get_balances (GET /v2/account, GET /v2/positions),
//! then AccountBalance::Sync#sync! (USD prices, the `account_balances` rows, removal of what is gone, the key's
//! `balances_synced_at`). Rails is the oracle (rust/tests/sync_parity.rs).
//!
//! Where a USD price comes from, as on a hosted instance:
//! - cash (asset category Fiat or Currency): USD is 1;
//! - a stock or ETF (category Stock): Alpaca's own latest trade, GET /v2/stocks/snapshots on data.alpaca.markets;
//! - everything else (coins), and a stock Alpaca gave no snapshot or no latest trade for: the market-data source, by `assets.external_id`
//!   (hosted: the data API's GET api/v1/prices).
use super::activities::Raw;
use crate::jobs::data_api::{ApiError, PriceFuture, PriceSource};
use crate::jobs::Db;
use super::wire::{self, Budget, Node};
use super::{ load_key, number, parsed, phase, record_sync_error, sql_time, venue_failure, Failure, SyncError, Unread};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::ruby::BigDec;
use crate::venue::alpaca::AlpacaVenue;
use crate::venue::binance;
use crate::venue::http::Transport;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

pub const SNAPSHOTS_PATH: &str = "/v2/stocks/snapshots";
pub const PRICES_PATH: &str = "/api/v1/prices";
pub const EXCHANGE_RATES_PATH: &str = "/api/v1/exchange_rates";
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
pub fn parse_prices(body: &Value, external_ids: &[String]) -> Result<BTreeMap<String, f64>, ApiError> {
    crate::jobs::data_api::read_prices(body, &external_ids.iter().map(String::as_str).collect::<Vec<_>>(), "usd")
}

/// Recorded answers for "GET /api/v1/prices", in the shape of http::ScriptedTransport's script (the parity harness
/// serves the same replies beneath Rails' Clients::MarketData). The last reply repeats; an unscripted call panics.
#[derive(Default)]
pub struct ScriptedPrices { replies: RefCell<VecDeque<Value>>, requests: RefCell<Vec<Vec<String>>>, rates: RefCell<VecDeque<Value>>, rate_requests: RefCell<usize>,
                            rate_cache: Arc<crate::jobs::data_api::RateCache> }

impl ScriptedPrices {
    pub fn from_script(script: &Value) -> Self {
        let replies = |path: &str| RefCell::new(script[format!("GET {path}")].as_array().cloned().unwrap_or_default().into());
        Self { replies: replies(PRICES_PATH), requests: RefCell::default(), rates: replies(EXCHANGE_RATES_PATH), rate_requests: RefCell::default(), rate_cache: Arc::default() }
    }
    /// The rate cache one run of syncs shares (Rails' cache store across the steps of one scenario).
    pub fn with_rate_cache(mut self, cache: Arc<crate::jobs::data_api::RateCache>) -> Self { self.rate_cache = cache; self }
    /// The id lists asked for, in order.
    pub fn requests(&self) -> Vec<Vec<String>> { self.requests.borrow().clone() }
    /// How many times the exchange rates were asked for.
    pub fn rate_requests(&self) -> usize { *self.rate_requests.borrow() }
}

/// A scripted reply: a network failure, an HTTP failure, or the body.
fn scripted(reply: &Value) -> Result<&Value, ApiError> {
    let message = reply["message"].as_str().unwrap_or_default().to_string();
    match (reply["network"].as_str(), reply["status"].as_u64().unwrap_or(200)) {
        // A certificate failure is returned by Rails' client, not raised.
        (Some("permanent"), _) => Err(ApiError::Failed { status: None, message }),
        (Some(_), _) => Err(ApiError::Transient(message)),
        (None, status) if !(200..300).contains(&status) => Err(ApiError::Failed { status: Some(status as u16), message: reply["body"].to_string() }),
        (None, _) => Ok(&reply["body"]),
    }
}

fn next_reply(q: &RefCell<VecDeque<Value>>, path: &str) -> Value {
    let mut q = q.borrow_mut();
    if q.is_empty() { panic!("unscripted call GET {path}"); }
    if q.len() > 1 { q.pop_front().unwrap_or(Value::Null) } else { q[0].clone() }
}

impl PriceSource for ScriptedPrices {
    fn prices<'a>(&'a self, external_ids: &'a [String], _currency: &'a str) -> PriceFuture<'a> {
        Box::pin(async move {
            self.requests.borrow_mut().push(external_ids.to_vec());
            let reply = next_reply(&self.replies, PRICES_PATH);
            scripted(&reply).and_then(|body| parse_prices(body, external_ids))
        })
    }
    fn exchange_rates<'a>(&'a self) -> crate::jobs::data_api::RatesFuture<'a> {
        Box::pin(async move {
            *self.rate_requests.borrow_mut() += 1;
            let reply = next_reply(&self.rates, EXCHANGE_RATES_PATH);
            scripted(&reply).map(|body| body["data"].clone())
        })
    }
    fn rate_cache(&self) -> Option<&crate::jobs::data_api::RateCache> { Some(&self.rate_cache) }
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

pub struct Catalog {
    user_id: i64, exchange_id: i64,
    /// `exchange.assets.pluck(:id)`: the assets the venue lists, in the order the rows are written.
    asset_ids: Vec<i64>,
    /// Venue asset class + native symbol, including unavailable listings; ambiguity stays unresolved.
    by_symbol: HashMap<(String, String), HashSet<i64>>,
    cash_ids: Vec<i64>,
}

struct Asset { id: i64, external_id: String, symbol: Option<String>, category: Option<String> }

/// A fresh price as Rails holds it: the market's is a Float, the venue's and cash are BigDecimals.
enum Price { Float(f64), Decimal(BigDec) }

/// The account as #get_balances needs it: an object whose `cash` is a number. A missing or null `cash` is a
/// malformed answer, here and in Rails (`the account has no cash figure`). The outer error
/// is an answer that could not be read at all; the inner one is the account's shape, judged once both answers are in.
pub(crate) fn account(text: &str) -> Result<Result<BigDec, String>, Unread> {
    let node = wire::read(text, &mut Budget(MAX_ACCOUNT_NODES), None).map_err(|r| Unread::refused(r, "an account"))?;
    let cash = |account: Raw| account.number("cash")?.ok_or_else(|| "the account has no cash figure".to_string());
    Ok(Raw::from_node(&node).map_err(|_| "unreadable account".to_string()).and_then(cash))
}

/// The positions as #get_balances needs them: an array of at most `MAX_POSITIONS` objects (the reader stops at the
/// next one). Unsupported classes are logged and individually excluded. A supported position with no symbol fails the
/// response, here and in Rails (`a position without a symbol`): skipped, its holding would read as 0 and its stored
/// balance be deleted. For a supported identity a missing/unreadable quantity fails it too; it is never manufactured as zero.
pub(crate) type Position = (String, String, Node);

pub(crate) fn positions(text: &str) -> Result<Result<Vec<Position>, String>, Unread> {
    let node = match wire::read_identity(text, &mut Budget(MAX_LIST_NODES), Some(MAX_POSITIONS)) {
        Err(wire::Refused::TooManyItems) => return Err(Unread::Raised(format!("more than {MAX_POSITIONS} positions"))),
        other => other.map_err(|r| Unread::refused(r, "positions"))?,
    };
    let Node::Array(items) = node else { return Ok(Err("unreadable positions".into())) };
    Ok(items.iter().filter_map(|item| {
        let Node::Object(fields) = item else { return Some(Err("unreadable position".to_string())); };
        let identity = |key: &str| fields.iter().find(|(name, _)| name == key).and_then(|(_, value)| {
            if let Node::Scalar(token) = value { serde_json::from_str::<String>(token).ok() } else { None }
        });
        let category = match identity("asset_class").as_deref() {
            Some("us_equity") => "Stock", Some("crypto") => "Cryptocurrency",
            _ => { eprintln!("Alpaca position skipped: unsupported class"); return None; }
        };
        let Some(symbol) = identity("symbol").filter(|s| !s.trim().is_empty()) else {
            return Some(Err("a position without a symbol".to_string()));
        };
        // Keep the whole row raw until the catalog resolves its class and native symbol.
        Some(Ok((category.to_string(), symbol, item.clone())))
    }).collect())
}

/// One balance sync of one Alpaca key, as AccountBalance::SyncJob runs it for that key. Idempotent: the rows are
/// upserted on (user, exchange, asset) and a second run at the same instant changes nothing.
///
/// `Ok(Err(failure))`: the balances or a price could not be read; no row was touched, `balances_synced_at` did not
/// move, the key's `last_sync_error` holds `failure.error`, and an "unauthorized" answer has marked the key incorrect.
/// A balance sync never clears `last_sync_error` (the ledger sync does).
///
/// The account and numeric/structural validity are checked before a row is written. Unresolvable position identities
/// are individually excluded; other malformed data still fails the sync. Rows are written in units of `BATCH`;
/// `balances_synced_at` moves in the last one.
pub async fn sync<T: Transport>(db: &Db, venue: &AlpacaVenue<T>, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    sync_captured(db, venue, prices, key_id, credentials, clock, &version).await
}

/// Test boundary after the incomplete-origin commit (zero) and each balance batch.
pub type BalanceStep<'a> = dyn Fn(usize) -> std::pin::Pin<Box<dyn std::future::Future<Output=()> + 'a>> + 'a;
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn sync_with_steps<T:Transport>(db:&Db,venue:&AlpacaVenue<T>,prices:&dyn PriceSource,key_id:i64,credentials:&Credentials,clock:&dyn Clock,step:&BalanceStep<'_>)->Result<Result<Summary,Failure>,SyncError>{
    let version=super::capture_bound_version(db,key_id,credentials).await?;
    sync_steps(db,venue,prices,key_id,credentials,clock,&version,Some(step)).await
}

pub(crate) async fn sync_captured<T: Transport>(db: &Db, venue: &AlpacaVenue<T>, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                -> Result<Result<Summary, Failure>, SyncError> {
    sync_steps(db,venue,prices,key_id,credentials,clock,version,None).await
}
#[allow(clippy::too_many_arguments)]
async fn sync_steps<T:Transport>(db:&Db,venue:&AlpacaVenue<T>,prices:&dyn PriceSource,key_id:i64,credentials:&Credentials,clock:&dyn Clock,version:&crate::engine::model::CredentialVersion,step:Option<&BalanceStep<'_>>)->Result<Result<Summary,Failure>,SyncError>{
    let captured_handle=crate::venue::alpaca::Captured::new(venue,version.clone());
    let venue=&captured_handle;
    let version=venue.origin();
    let catalog = phase(db, move |c| catalog(c, key_id)).await?;
    // A failure the job only records (`condemn`: #handle_api_key_failure marks a key Alpaca calls unauthorized incorrect).
    let fail = |text: String, condemn: bool| {
        let (creds, now) = (credentials.clone(), clock.now());
        async move {
            let error = super::commit_for(db, version, move |c| {
                if condemn && text.contains("unauthorized") {
                    c.execute("UPDATE api_keys SET status = 2, updated_at = ?1 WHERE id = ?2 AND status != 2", params![sql_time(now), key_id])?;
                }
                record_sync_error(c, key_id, &text, &creds)
            }).await?;
            Ok::<_, SyncError>(Err(Failure { error, raised: false }))
        }
    };

    // #get_balances: the account, then the positions. A failed request, or an answer that is not JSON, ends it there
    // (a failure the venue returned is its word on the key, `condemn`; a transport failure is not). Both answers are
    // then validated whole, the account first, as Ruby raises only once it uses them.
    let cash = match venue.read(false, "/v2/account", vec![], MAX_ACCOUNT_BYTES).await.value.map_err(venue_failure) {
        Err((text, raised)) => return fail(text, !raised).await,
        Ok(body) => match parsed(body, account).await { Ok(checked) => checked, Err((text, raised)) => return fail(text, !raised).await },
    };
    let held = match venue.read(false, "/v2/positions", vec![], MAX_LIST_BYTES).await.value.map_err(venue_failure) {
        Err((text, raised)) => return fail(text, !raised).await,
        Ok(body) => match parsed(body, positions).await { Ok(checked) => checked, Err((text, raised)) => return fail(text, !raised).await },
    };
    let (cash, held) = match (cash, held) {
        (Ok(cash), Ok(held)) => (cash, held),
        (Err(why), _) | (_, Err(why)) => return fail(why, false).await,
    };
    let held = match complete_balances(&catalog, cash, held) {
        Ok(rows) => rows.into_iter().filter(|(_, free, locked)| (free + locked).is_positive()).collect::<Vec<_>>(),
        Err(why) => return fail(why, false).await,
    };

    let ids: Vec<i64> = held.iter().map(|(id, _, _)| *id).collect();
    let assets = Arc::new(phase(db, move |c| assets(c, &ids)).await?);

    // The venue's own quotes for stocks; a failure there sends the stocks to the market source with everything else.
    let mut fresh: HashMap<String, Price> = HashMap::new();
    let mut errors: Vec<String> = vec![];
    let mut symbols: Vec<String> = assets.iter().filter(|a| a.category.as_deref() == Some("Stock")).filter_map(|a| a.symbol.clone()).collect();
    symbols.sort_unstable();
    symbols.dedup();
    if !symbols.is_empty() {
        let read = match venue.read(true, SNAPSHOTS_PATH, vec![("symbols", symbols.join(","))], MAX_LIST_BYTES).await.value.map_err(venue_failure) {
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
    match finish(db, version, &catalog, held, assets, fresh, errors, prices, key_id, clock, step).await? {
        Ok(summary) => Ok(Ok(summary)),
        Err(why) => fail(why, false).await,
    }
}

/// GET /api/v3/account with `omitZeroBalances`: Exchanges::Binance#get_balances.
pub const BINANCE_ACCOUNT_PATH: &str = "/api/v3/account";
/// What a Binance account answer may hold: one entry per asset held (a few hundred at most), and its permissions.
pub const MAX_BINANCE_ACCOUNT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BINANCE_ACCOUNT_NODES: usize = 300_000;

/// One balance sync of one Binance or Binance.US key, as AccountBalance::SyncJob runs it for that key: `transport`
/// reaches the venue (through the configured proxy, `jobs`), the key's exchange type picks the host. Read only: one
/// signed GET. What it writes and when is the Alpaca sync's (`sync`): nothing until the answer is read whole and every
/// number in it checked; a failure records the venue's text, and Binance's invalid-key words mark the key incorrect.
pub async fn sync_binance<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                        -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    let host = |t: &str| binance::host(t).map(|(base, _)| base.to_string());
    sync_binance_captured(db, transport, &host, prices, key_id, credentials, clock, &version).await
}

/// `host`: the base URL for the key's exchange type (`Connect::binance_base`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn sync_binance_captured<T: Transport>(db: &Db, transport: T, host: &dyn Fn(&str) -> Option<String>, prices: &dyn PriceSource, key_id: i64,
                                                        credentials: &Credentials, clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                                        -> Result<Result<Summary, Failure>, SyncError> {
    let (secret, now) = (credentials.secret.clone(), clock.now());
    let read = move |exchange_type: String| async move {
        let base = host(&exchange_type).ok_or_else(|| SyncError(format!("api key {key_id} is not a Binance key")))?;
        let venue = binance::BinanceVenue::new(transport, &base, &secret);
        let query = vec![("omitZeroBalances", "true".to_string()), ("recvWindow", binance::READ_RECV_WINDOW.to_string())];
        Ok(venue.signed_get(BINANCE_ACCOUNT_PATH, query, now, MAX_BINANCE_ACCOUNT_BYTES).await)
    };
    // #handle_api_key_failure then #record_failure: Binance's invalid-key words condemn the key; nothing else does.
    let venue = VenueBalances { types: &[binance::BINANCE, binance::BINANCE_US], failure_text: binance::error_message, condemns: |_, text| binance::condemns(text),
                                held: |text, names, ids| binance_held(text, names, ids).map(|held| held.map_err(|why| (why, false))) };
    sync_venue(db, version, prices, key_id, credentials, clock, venue, read).await
}

/// What KuCoin's trade-account answer may hold: one row per currency held.
pub const KUCOIN_ACCOUNTS_PATH: &str = "/api/v1/accounts";

/// One balance sync of one KuCoin key, as AccountBalance::SyncJob runs it for that key: Exchanges::Kucoin#get_balances
/// (the classic `trade` account, as Rails reads it) then AccountBalance::Sync. Read only: one signed GET. A failure records
/// the venue's text; KuCoin's invalid-key words mark the key incorrect unless they name a missing permission
/// (ApiKeyFailureHandling#failure_reason).
pub async fn sync_kucoin<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                       -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    sync_kucoin_captured(db, transport, prices, key_id, credentials, clock, &version).await
}

pub(crate) async fn sync_kucoin_captured<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials,
                                                       clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                                       -> Result<Result<Summary, Failure>, SyncError> {
    use crate::venue::kucoin::{KucoinVenue, KUCOIN, URL};
    let read = move |_: String| async move {
        Ok(KucoinVenue::new(transport, URL).read_text(KUCOIN_ACCOUNTS_PATH, vec![("type", "trade".to_string())], MAX_BINANCE_ACCOUNT_BYTES).await)
    };
    let venue = VenueBalances { types: &[KUCOIN], failure_text: str::to_string, condemns: |venue, text| condemns(&crate::engine::venue_rules::KUCOIN, venue, text), held: kucoin_held };
    sync_venue(db, version, prices, key_id, credentials, clock, venue, read).await
}

/// One balance sync of one Bitvavo key, as AccountBalance::SyncJob runs it for that key: Exchanges::Bitvavo#get_balances
/// (GET /v2/balance) then AccountBalance::Sync. Read only: one signed GET. A failure records the venue's text; Bitvavo's
/// invalid-key words mark the key incorrect unless they name a missing permission (307/310/311/312).
pub async fn sync_bitvavo<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                        -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    sync_bitvavo_captured(db, transport, prices, key_id, credentials, clock, &version).await
}

pub(crate) async fn sync_bitvavo_captured<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials,
                                                        clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                                        -> Result<Result<Summary, Failure>, SyncError> {
    use crate::venue::bitvavo::{BitvavoVenue, BALANCE_PATH, BITVAVO, URL};
    let read = move |_: String| async move { Ok(BitvavoVenue::new(transport, URL).read_text(BALANCE_PATH, vec![], MAX_BINANCE_ACCOUNT_BYTES).await) };
    let venue = VenueBalances { types: &[BITVAVO], failure_text: str::to_string, condemns: |venue, text| condemns(&crate::engine::venue_rules::BITVAVO, venue, text), held: bitvavo_held };
    sync_venue(db, version, prices, key_id, credentials, clock, venue, read).await
}

/// One balance sync of one MEXC key, as AccountBalance::SyncJob runs it for that key: Exchanges::Mexc#get_balances (GET
/// /api/v3/account, the gem's recvWindow) then AccountBalance::Sync. Read only: one signed GET. A failure records
/// #parse_error_message's text; MEXC's invalid-key words mark the key incorrect unless they name a missing permission.
pub async fn sync_mexc<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                     -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    sync_mexc_captured(db, transport, prices, key_id, credentials, clock, &version).await
}

pub(crate) async fn sync_mexc_captured<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials,
                                                     clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                                     -> Result<Result<Summary, Failure>, SyncError> {
    use crate::venue::mexc::{MexcVenue, ACCOUNT_PATH, MEXC, RECV_WINDOW, URL};
    let (secret, now) = (credentials.secret.clone(), clock.now());
    let read = move |_: String| async move {
        let venue = MexcVenue::new(transport, URL, &secret);
        Ok(venue.read_text(&venue.signed("GET", ACCOUNT_PATH, vec![("recvWindow", RECV_WINDOW.to_string())], now), MAX_BINANCE_ACCOUNT_BYTES).await)
    };
    // The account answer is Binance's shape (`binance_held`).
    let venue = VenueBalances { types: &[MEXC], failure_text: binance::error_message, condemns: |venue, text| condemns(&crate::engine::venue_rules::MEXC, venue, text),
                                held: |text, names, ids| binance_held(text, names, ids).map(|held| held.map_err(|why| (why, false))) };
    sync_venue(db, version, prices, key_id, credentials, clock, venue, read).await
}

pub async fn sync_bitrue<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                     -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    sync_bitrue_captured(db, transport, prices, key_id, credentials, clock, &version).await
}

pub(crate) async fn sync_bitrue_captured<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials,
                                                     clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                                     -> Result<Result<Summary, Failure>, SyncError> {
    use crate::venue::bitrue::{BitrueVenue, ACCOUNT_PATH, BITRUE, RECV_WINDOW, URL};
    let (secret, now) = (credentials.secret.clone(), clock.now());
    let read = move |_: String| async move {
        let venue = BitrueVenue::new(transport, URL, &secret);
        Ok(venue.read_text(&venue.signed("GET", ACCOUNT_PATH, vec![("recvWindow", RECV_WINDOW.to_string())], now), MAX_BINANCE_ACCOUNT_BYTES).await)
    };
    // The account answer is Binance's shape (`binance_held`).
    let venue = VenueBalances { types: &[BITRUE], failure_text: binance::error_message, condemns: |venue, text| condemns(&crate::engine::venue_rules::BITRUE, venue, text),
                                held: |text, names, ids| account_held(text, names, ids, str::to_uppercase).map(|held| held.map_err(|why| (why, false))) };
    sync_venue(db, version, prices, key_id, credentials, clock, venue, read).await
}

pub async fn sync_bingx<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                     -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    sync_bingx_captured(db, transport, prices, key_id, credentials, clock, &version).await
}

pub(crate) async fn sync_bingx_captured<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials,
                                                     clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                                     -> Result<Result<Summary, Failure>, SyncError> {
    use crate::venue::bingx::{BingxVenue, ACCOUNT_PATH, BINGX, URL};
    let (secret, now) = (credentials.secret.clone(), clock.now());
    let read = move |_: String| async move {
        let venue = BingxVenue::new(transport, URL, &secret);
        Ok(venue.read_text(&venue.signed("GET", ACCOUNT_PATH, vec![], now), MAX_BINANCE_ACCOUNT_BYTES).await)
    };
    // The account answer is Binance's shape (`binance_held`).
    let venue = VenueBalances { types: &[BINGX], failure_text: binance::error_message, condemns: |venue, text| condemns(&crate::engine::venue_rules::BINGX, venue, text),
                                held: |text, names, ids| {
        let node = wire::read(text, &mut Budget(MAX_BINANCE_ACCOUNT_NODES), None).map_err(|r| Unread::refused(r, "an account"))?;
        let Ok(raw) = Raw::from_node(&node) else { return Ok(Err(("unreadable account".into(),false))) };
        if crate::venue::bingx::code(&raw.value) != Some(0) { return Ok(Err((raw.value["msg"].as_str().unwrap_or("unreadable BingX account").into(),true))); }
        let Some(data) = raw.member("data") else { return Ok(Err(("KeyError: Key path data not found".into(),false))) };
        binance_held(&data.text(), names, ids).map(|held| held.map_err(|why| (why,false)))
    } };
    sync_venue(db, version, prices, key_id, credentials, clock, venue, read).await
}

/// ApiKeyFailureHandling#failure_reason's :invalid: the venue's invalid-key words, unless the text names a missing permission.
fn condemns(rules: &crate::engine::venue_rules::VenueRules, venue: &str, text: &str) -> bool {
    !crate::tracker::import::run::missing_permission(venue, Some(text)) && rules.is_invalid_key(&[text.to_string()])
}

/// A venue's balance read as `sync_venue` runs it (plan VB §1.6): the key's exchange types, the failure text of an answer
/// that failed or is no JSON, whether a failure condemns the key (exchange type, text), and how the answer is read:
/// the held balances, or the failure and whether the key's failure handling reads it (`true`: it may condemn).
struct VenueBalances { types: &'static [&'static str], failure_text: fn(&str) -> String, condemns: fn(&str, &str) -> bool, held: HeldRead }
/// A venue's answer read into its held balances, or its failure (and whether the key's failure handling reads it).
type HeldRead = fn(&str, &HashMap<String, i64>, &[i64]) -> Result<Result<Vec<Held>, (String, bool)>, Unread>;

/// One honeymaker venue's balance sync for one key, as AccountBalance::SyncJob runs it: `read` (given the key's exchange
/// type) sends the one signed GET; nothing is written until its answer is read whole and every number in it checked; then
/// AccountBalance::Sync (`finish`). A failure records the venue's text and, where `condemns`, marks the key incorrect.
#[allow(clippy::too_many_arguments)]
async fn sync_venue<R>(db: &Db, version: &crate::engine::model::CredentialVersion, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials,
                       clock: &dyn Clock, venue: VenueBalances, read: impl FnOnce(String) -> R) -> Result<Result<Summary, Failure>, SyncError>
where R: std::future::Future<Output = Result<Result<String, String>, SyncError>> {
    let types = venue.types;
    let (catalog, names, exchange_type) = phase(db, move |c| {
        let key = super::load_key_of(c, key_id, types)?;
        if key.status != 1 || key.key_type == 1 { return Err(SyncError(format!("api key {key_id} is not a reading key"))); }
        Ok((catalog_for(c, key.user_id, key.exchange_id)?, binance_names(c, key.exchange_id)?, key.exchange_type))
    }).await?;
    let condemning = venue.condemns;
    let fail = |text: String, condemn: bool| {
        let (creds, now, venue_type) = (credentials.clone(), clock.now(), exchange_type.clone());
        async move {
            let error = super::commit_for(db, version, move |c| {
                if condemn && condemning(&venue_type, &text) {
                    c.execute("UPDATE api_keys SET status = 2, updated_at = ?1 WHERE id = ?2 AND status != 2", params![sql_time(now), key_id])?;
                }
                record_sync_error(c, key_id, &text, &creds)
            }).await?;
            Ok::<_, SyncError>(Err(Failure { error, raised: false }))
        }
    };
    let text = match read(exchange_type.clone()).await? {
        Ok(text) => text,
        Err(text) => return fail((venue.failure_text)(&text), true).await,
    };
    let (asset_ids, held) = (catalog.asset_ids.clone(), venue.held);
    let read = tokio::task::spawn_blocking(move || held(&text, &names, &asset_ids).map_err(|unread| (unread, text))).await
        .unwrap_or_else(|_| Err((Unread::Raised("reading the venue's answer was interrupted".into()), String::new())));
    let held = match read {
        Ok(Ok(held)) => held,
        Ok(Err((why, condemn))) => return fail(why, condemn).await,
        // A 2xx body that is not JSON: honeymaker's failure is the body itself, read as any failure is.
        Err((Unread::NotJson, body)) => return fail((venue.failure_text)(&body), true).await,
        Err((Unread::Raised(why), _)) => return fail(why, false).await,
    };
    let ids: Vec<i64> = held.iter().map(|(id, _, _)| *id).collect();
    let assets = Arc::new(phase(db, move |c| assets(c, &ids)).await?);
    match finish(db, version, &catalog, held, assets, HashMap::new(), vec![], prices, key_id, clock, None).await? {
        Ok(summary) => Ok(Ok(summary)),
        Err(why) => fail(why, false).await,
    }
}

/// Exchanges::Bitvavo#get_balances then AccountBalance::Sync's `nonzero`: each row's `symbol` (#asset_from_symbol: a row
/// naming no listed asset, or none, is skipped), its `available` and `inOrder` read for the rest (Rails' KeyError when
/// absent), a later row for the same asset winning. A quantity that is no number fails the sync (Rails reads it as 0: the
/// Binance sync's listed divergence). An answer that is not a list is unreadable.
fn bitvavo_held(text: &str, names: &HashMap<String, i64>, asset_ids: &[i64]) -> Result<Result<Vec<Held>, (String, bool)>, Unread> {
    let node = wire::read(text, &mut Budget(MAX_BINANCE_ACCOUNT_NODES), None).map_err(|r| Unread::refused(r, "an account"))?;
    let Node::Array(rows) = node else { return Ok(Err(("unreadable balances".into(), false))) };
    let mut found: HashMap<i64, (BigDec, BigDec)> = HashMap::new();
    for row in &rows {
        let Ok(row) = Raw::from_node(row) else { return Ok(Err(("unreadable balance".into(), false))) };
        let Some(asset) = row.value["symbol"].as_str().and_then(|c| names.get(c)).filter(|id| asset_ids.contains(id)) else { continue };
        let quantity = |key: &str| match row.number(key) {
            _ if row.value[key].as_str().is_some_and(|s| s.trim().is_empty()) => Err(format!("unreadable {key}: not a plain decimal number")),
            Ok(Some(n)) => Ok(n),
            Ok(None) => Err(format!("KeyError: Key path {key} not found")),
            Err(why) => Err(why),
        };
        let (free, locked) = match (quantity("available"), quantity("inOrder")) {
            (Ok(free), Ok(locked)) => (free, locked),
            (Err(why), _) | (_, Err(why)) => return Ok(Err((why, false))),
        };
        found.insert(*asset, (free, locked));
    }
    Ok(Ok(asset_ids.iter().filter_map(|id| found.remove(id).map(|(free, locked)| (*id, free, locked))).filter(|(_, free, locked)| (free + locked).is_positive()).collect()))
}

/// Exchanges::Kucoin#get_balances then AccountBalance::Sync's `nonzero`. Only the success code with `data` means balances,
/// else "Failed to get KuCoin balances: <code> <msg>" (a Failure the key's failure handling reads: `true`). Each row's
/// `currency` must be there; a row naming no listed asset is skipped; `available` and `locked` (before KuCoin's
/// 2026-08-27 rename `holds`) are read for the rest, a later row for the same asset winning. A missing field is Rails'
/// KeyError; a quantity that is no number fails the sync (Rails reads it as 0: the Binance sync's listed divergence).
fn kucoin_held(text: &str, names: &HashMap<String, i64>, asset_ids: &[i64]) -> Result<Result<Vec<Held>, (String, bool)>, Unread> {
    let node = wire::read(text, &mut Budget(MAX_BINANCE_ACCOUNT_NODES), None).map_err(|r| Unread::refused(r, "an account"))?;
    let Ok(body) = Raw::from_node(&node) else { return Ok(Err(("unreadable account".into(), false))) };
    let part = |v: &Value| match v { Value::Null => None, Value::String(s) => Some(s.clone()), other => Some(other.to_string()) };
    let code = part(&body.value["code"]);
    let rows = match body.member("data") {
        Some(Node::Array(rows)) if code.as_deref() == Some("200000") => rows,
        Some(Node::Scalar(token)) if token != "null" && code.as_deref() == Some("200000") => return Ok(Err(("unreadable balances".into(), false))),
        Some(Node::Object(_)) if code.as_deref() == Some("200000") => return Ok(Err(("unreadable balances".into(), false))),
        _ => {
            let detail: Vec<String> = [code, part(&body.value["msg"])].into_iter().flatten().collect();
            return Ok(Err((format!("Failed to get KuCoin balances: {}", detail.join(" ")), true)));
        }
    };
    let mut found: HashMap<i64, (BigDec, BigDec)> = HashMap::new();
    for row in rows {
        let Ok(row) = Raw::from_node(row) else { return Ok(Err(("unreadable balance".into(), false))) };
        let Some(currency) = row.value.get("currency").filter(|v| !v.is_null()) else { return Ok(Err(("KeyError: Key path currency not found".into(), false))) };
        let Some(asset) = currency.as_str().and_then(|c| names.get(c)).filter(|id| asset_ids.contains(id)) else { continue };
        let quantity = |key: &str| match row.number(key) {
            _ if row.value[key].as_str().is_some_and(|s| s.trim().is_empty()) => Err(format!("unreadable {key}: not a plain decimal number")),
            Ok(Some(n)) => Ok(n),
            Ok(None) => Err(format!("KeyError: Key path {key} not found")),
            Err(why) => Err(why),
        };
        let free = match quantity("available") { Ok(n) => n, Err(why) => return Ok(Err((why, false))) };
        // `balance.fetch('locked') { dig_or_raise('holds') }.to_d`: a present null is nil.to_d, 0.
        let locked = match row.member("locked") {
            Some(Node::Scalar(token)) if token == "null" => BigDec::zero(),
            Some(_) => match quantity("locked") { Ok(n) => n, Err(why) => return Ok(Err((why, false))) },
            None => match quantity("holds") { Ok(n) => n, Err(why) => return Ok(Err((why, false))) },
        };
        found.insert(*asset, (free, locked));
    }
    Ok(Ok(asset_ids.iter().filter_map(|id| found.remove(id).map(|(free, locked)| (*id, free, locked))).filter(|(_, free, locked)| (free + locked).is_positive()).collect()))
}

/// One balance sync of one Gemini key, as AccountBalance::SyncJob runs it for that key: Exchanges::Gemini#get_balances
/// (Clients::Gemini#get_raw_balances, one signed POST) then AccountBalance::Sync. A failure records the venue's text;
/// Gemini's invalid-key reasons mark the key incorrect unless they name a missing role (ApiKeyFailureHandling#failure_reason).
pub async fn sync_gemini<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                       -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    sync_gemini_captured(db, transport, prices, key_id, credentials, clock, &version).await
}

pub(crate) async fn sync_gemini_captured<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials,
                                                       clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                                       -> Result<Result<Summary, Failure>, SyncError> {
    use crate::venue::gemini::{GeminiVenue, BALANCES, GEMINI, URL};
    let read = move |_: String| async move { Ok(GeminiVenue::new(transport, URL).read_text(BALANCES, vec![], MAX_BINANCE_ACCOUNT_BYTES).await) };
    let venue = VenueBalances { types: &[GEMINI], failure_text: str::to_string, condemns: |venue, text| condemns(&crate::engine::venue_rules::GEMINI, venue, text), held: gemini_held };
    sync_venue(db, version, prices, key_id, credentials, clock, venue, read).await
}

/// Exchanges::Gemini#get_balances then AccountBalance::Sync's `nonzero`. A null answer is "Failed to get Gemini balances" (a
/// Failure the key's failure handling reads: `true`); anything but a list Rails cannot walk. Each row's `currency` must be
/// there, upcased to name its asset; a row naming no listed asset is skipped; `available` is free and `amount - available`
/// locked, a later row for the same asset winning. A missing field is Rails' KeyError; a quantity that is no number fails
/// the sync (Rails reads it as 0: the Binance sync's listed divergence).
fn gemini_held(text: &str, names: &HashMap<String, i64>, asset_ids: &[i64]) -> Result<Result<Vec<Held>, (String, bool)>, Unread> {
    let node = wire::read(text, &mut Budget(MAX_BINANCE_ACCOUNT_NODES), None).map_err(|r| Unread::refused(r, "an account"))?;
    let rows = match &node {
        Node::Array(rows) => rows,
        Node::Scalar(token) if token == "null" => return Ok(Err(("Failed to get Gemini balances".into(), true))),
        _ => return Ok(Err(("unreadable balances".into(), false))),
    };
    let mut found: HashMap<i64, (BigDec, BigDec)> = HashMap::new();
    for row in rows {
        let Ok(row) = Raw::from_node(row) else { return Ok(Err(("unreadable balance".into(), false))) };
        let Some(currency) = row.value.get("currency").filter(|v| !v.is_null()) else { return Ok(Err(("KeyError: Key path currency not found".into(), false))) };
        let Some(currency) = currency.as_str() else { return Ok(Err(("unreadable balance currency".into(), false))) };
        let Some(asset) = names.get(&currency.to_uppercase()).filter(|id| asset_ids.contains(id)) else { continue };
        let quantity = |key: &str| match row.number(key) {
            _ if row.value[key].as_str().is_some_and(|s| s.trim().is_empty()) => Err(format!("unreadable {key}: not a plain decimal number")),
            Ok(Some(n)) => Ok(n),
            Ok(None) => Err(format!("KeyError: Key path {key} not found")),
            Err(why) => Err(why),
        };
        let free = match quantity("available") { Ok(n) => n, Err(why) => return Ok(Err((why, false))) };
        let amount = match quantity("amount") { Ok(n) => n, Err(why) => return Ok(Err((why, false))) };
        found.insert(*asset, (free.clone(), &amount - &free));
    }
    Ok(Ok(asset_ids.iter().filter_map(|id| found.remove(id).map(|(free, locked)| (*id, free, locked))).filter(|(_, free, locked)| (free + locked).is_positive()).collect()))
}

/// One balance sync of one Bitget key, as AccountBalance::SyncJob runs it for that key: Exchanges::Bitget#get_balances
/// (GET /api/v2/spot/account/assets) then AccountBalance::Sync. Read only: one signed GET. A failure records the venue's text;
/// Bitget's invalid-key words mark the key incorrect unless they name a missing permission (40014/40025).
pub async fn sync_bitget<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                       -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    sync_bitget_captured(db, transport, prices, key_id, credentials, clock, &version).await
}

pub(crate) async fn sync_bitget_captured<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials,
                                                       clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                                       -> Result<Result<Summary, Failure>, SyncError> {
    use crate::venue::bitget::{BitgetVenue, ASSETS_PATH, BITGET, URL};
    let read = move |_: String| async move { Ok(BitgetVenue::new(transport, URL).read_text(ASSETS_PATH, vec![], MAX_BINANCE_ACCOUNT_BYTES).await) };
    let venue = VenueBalances { types: &[BITGET], failure_text: str::to_string, condemns: |venue, text| condemns(&crate::engine::venue_rules::BITGET, venue, text), held: bitget_held };
    sync_venue(db, version, prices, key_id, credentials, clock, venue, read).await
}

/// Exchanges::Bitget#get_balances then AccountBalance::Sync's `nonzero`. Only the success code (`code.to_s == "00000"`) with
/// `data` means balances, else "Failed to get Bitget balances: <code> <msg>" (a Failure the key's failure handling reads:
/// `true`). Each row's `coin` must be there (as Rails spells it: AUDIT-bitget S4, ported as is); a row naming no listed asset
/// is skipped; `available` and `frozen` are read for the rest, a later row for the same asset winning (the fiat merchant's
/// `locked` is not read, S5). A missing field is Rails' KeyError; a quantity that is no number fails the sync (Rails reads it
/// as 0: the Binance sync's listed divergence).
fn bitget_held(text: &str, names: &HashMap<String, i64>, asset_ids: &[i64]) -> Result<Result<Vec<Held>, (String, bool)>, Unread> {
    let node = wire::read(text, &mut Budget(MAX_BINANCE_ACCOUNT_NODES), None).map_err(|r| Unread::refused(r, "an account"))?;
    let Ok(body) = Raw::from_node(&node) else { return Ok(Err(("unreadable account".into(), false))) };
    let part = |v: &Value| match v { Value::Null => None, Value::String(s) => Some(s.clone()), other => Some(other.to_string()) };
    let code = part(&body.value["code"]);
    let rows = match body.member("data") {
        Some(Node::Array(rows)) if code.as_deref() == Some("00000") => rows,
        Some(Node::Scalar(token)) if token == "null" => return Ok(Err((failed_bitget(&body.value), true))),
        Some(_) if code.as_deref() == Some("00000") => return Ok(Err(("unreadable balances".into(), false))),
        _ => return Ok(Err((failed_bitget(&body.value), true))),
    };
    let mut found: HashMap<i64, (BigDec, BigDec)> = HashMap::new();
    for row in rows {
        let Ok(row) = Raw::from_node(row) else { return Ok(Err(("unreadable balance".into(), false))) };
        let Some(coin) = row.value.get("coin").filter(|v| !v.is_null()) else { return Ok(Err(("KeyError: Key path coin not found".into(), false))) };
        let Some(asset) = coin.as_str().and_then(|c| names.get(c)).filter(|id| asset_ids.contains(id)) else { continue };
        let quantity = |key: &str| match row.number(key) {
            _ if row.value[key].as_str().is_some_and(|s| s.trim().is_empty()) => Err(format!("unreadable {key}: not a plain decimal number")),
            Ok(Some(n)) => Ok(n),
            Ok(None) => Err(format!("KeyError: Key path {key} not found")),
            Err(why) => Err(why),
        };
        let (free, locked) = match (quantity("available"), quantity("frozen")) {
            (Ok(free), Ok(locked)) => (free, locked),
            (Err(why), _) | (_, Err(why)) => return Ok(Err((why, false))),
        };
        found.insert(*asset, (free, locked));
    }
    Ok(Ok(asset_ids.iter().filter_map(|id| found.remove(id).map(|(free, locked)| (*id, free, locked))).filter(|(_, free, locked)| (free + locked).is_positive()).collect()))
}

/// Exchanges::Bitget#get_balances' failure: "Failed to get Bitget balances: " and the envelope's code and msg, as present.
fn failed_bitget(body: &Value) -> String {
    let part = |v: &Value| match v { Value::Null => None, Value::String(s) => Some(s.clone()), other => Some(other.to_string()) };
    let detail: Vec<String> = [part(&body["code"]), part(&body["msg"])].into_iter().flatten().collect();
    format!("Failed to get Bitget balances: {}", detail.join(" "))
}

/// One balance sync of one Hyperliquid key, as AccountBalance::SyncJob runs it for that key: Exchanges::Hyperliquid#get_balances
/// (Clients::Hyperliquid#spot_balances: POST /info spotClearinghouseState for the key's address) then AccountBalance::Sync. A
/// failure records the venue's text; "User or API Wallet" marks the key incorrect (ApiKeyFailureHandling#failure_reason).
pub async fn sync_hyperliquid<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                            -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    sync_hyperliquid_captured(db, transport, prices, key_id, credentials, clock, &version).await
}

pub(crate) async fn sync_hyperliquid_captured<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials,
                                                            clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                                            -> Result<Result<Summary, Failure>, SyncError> {
    use crate::venue::hyperliquid::{HyperliquidVenue, HYPERLIQUID, URL};
    let user = credentials.key.clone();
    let read = move |_: String| async move {
        let venue = HyperliquidVenue::new(transport, URL, &user);
        Ok(venue.read_text("spotClearinghouseState", vec![("user", Value::from(user.clone()))], MAX_BINANCE_ACCOUNT_BYTES).await)
    };
    let venue = VenueBalances { types: &[HYPERLIQUID], failure_text: str::to_string, condemns: |venue, text| condemns(&crate::engine::venue_rules::HYPERLIQUID, venue, text),
                                held: hyperliquid_held };
    sync_venue(db, version, prices, key_id, credentials, clock, venue, read).await
}

/// Exchanges::Hyperliquid#get_balances then AccountBalance::Sync's `nonzero`: `data['balances'] || []`, each row's `coin`
/// naming its asset (#asset_from_symbol: a row naming none is skipped), free = total - hold and hold locked (an absent total
/// or hold is 0), a later row for the same asset winning. A quantity that is no number fails the sync (Rails reads it as 0:
/// the Binance sync's listed divergence). An answer Rails cannot walk (not an object, balances that are no list of objects)
/// raises there; here it fails the sync.
fn hyperliquid_held(text: &str, names: &HashMap<String, i64>, asset_ids: &[i64]) -> Result<Result<Vec<Held>, (String, bool)>, Unread> {
    let node = wire::read(text, &mut Budget(MAX_BINANCE_ACCOUNT_NODES), None).map_err(|r| Unread::refused(r, "an account"))?;
    let Ok(body) = Raw::from_node(&node) else { return Ok(Err(("unreadable balances".into(), false))) };
    let rows = match body.member("balances") {
        None => vec![],
        Some(Node::Scalar(token)) if token == "null" => vec![],
        Some(Node::Array(rows)) => rows.clone(),
        Some(_) => return Ok(Err(("unreadable balances".into(), false))),
    };
    let mut found: HashMap<i64, (BigDec, BigDec)> = HashMap::new();
    for row in &rows {
        let Ok(row) = Raw::from_node(row) else { return Ok(Err(("unreadable balance".into(), false))) };
        let Some(asset) = row.value["coin"].as_str().and_then(|c| names.get(c)).filter(|id| asset_ids.contains(id)) else { continue };
        let quantity = |key: &str| match row.number(key) {
            _ if row.value[key].as_str().is_some_and(|s| s.trim().is_empty()) => Err(format!("unreadable {key}: not a plain decimal number")),
            Ok(n) => Ok(n.unwrap_or_else(BigDec::zero)),
            Err(why) => Err(why),
        };
        let (total, hold) = match (quantity("total"), quantity("hold")) {
            (Ok(total), Ok(hold)) => (total, hold),
            (Err(why), _) | (_, Err(why)) => return Ok(Err((why, false))),
        };
        found.insert(*asset, (&total - &hold, hold));
    }
    Ok(Ok(asset_ids.iter().filter_map(|id| found.remove(id).map(|(free, locked)| (*id, free, locked))).filter(|(_, free, locked)| (free + locked).is_positive()).collect()))
}

/// Exchange#asset_from_symbol: the venue's available tickers in id order, each naming its base asset and then its quote
/// asset by the ticker's own spelling; the first ticker to name a symbol wins it.
fn binance_names(c: &Connection, exchange_id: i64) -> Result<HashMap<String, i64>, SyncError> {
    let mut s = c.prepare("SELECT base, base_asset_id, quote, quote_asset_id FROM tickers WHERE exchange_id = ?1 AND available = 1 ORDER BY id")?;
    let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?.collect::<Result<Vec<_>, _>>()?;
    let mut names = HashMap::new();
    for (base, base_id, quote, quote_id) in rows {
        names.entry(base).or_insert(base_id);
        names.entry(quote).or_insert(quote_id);
    }
    Ok(names)
}

/// One held asset: its id, free and locked quantities.
type Held = (i64, BigDec, BigDec);

/// Exchanges::Binance#get_balances then AccountBalance::Sync's `nonzero`: per asset the venue lists, its free and locked
/// quantities (a later entry for the same asset wins), in the order of `asset_ids`, only those holding something. An
/// entry whose symbol names no listed asset is not a balance. A missing `balances` or a missing quantity is Rails'
/// KeyError; a quantity that is no number, blank and whitespace included, fails the sync (Rails reads it as 0 and
/// deletes the stored balance: a listed divergence, D2b ruling Q10).
fn binance_held(text: &str, names: &HashMap<String, i64>, asset_ids: &[i64]) -> Result<Result<Vec<Held>, String>, Unread> {
    account_held(text, names, asset_ids, str::to_string)
}

fn account_held(text: &str, names: &HashMap<String, i64>, asset_ids: &[i64], code: impl Fn(&str) -> String) -> Result<Result<Vec<Held>, String>, Unread> {
    let node = wire::read(text, &mut Budget(MAX_BINANCE_ACCOUNT_NODES), None).map_err(|r| Unread::refused(r, "an account"))?;
    let Ok(account) = Raw::from_node(&node) else { return Ok(Err("unreadable account".into())) };
    let entries = match account.member("balances") {
        None => return Ok(Err("KeyError: Key path balances not found".into())),
        Some(Node::Scalar(token)) if token == "null" => return Ok(Err("KeyError: Key path balances not found".into())),
        Some(Node::Array(entries)) => entries,
        Some(_) => return Ok(Err("unreadable balances".into())),
    };
    let mut found: HashMap<i64, (BigDec, BigDec)> = HashMap::new();
    for entry in entries {
        let Ok(entry) = Raw::from_node(entry) else { return Ok(Err("unreadable balance".into())) };
        let Some(asset) = entry.value["asset"].as_str().and_then(|a| names.get(&code(a))).filter(|id| asset_ids.contains(id)) else { continue };
        // number::json reads a blank string as 0 (String#to_d, which the Alpaca sync matches); here it is no number.
        let quantity = |key: &str| match entry.number(key) {
            _ if entry.value[key].as_str().is_some_and(|s| s.trim().is_empty()) => Err(format!("unreadable {key}: not a plain decimal number")),
            Ok(Some(n)) => Ok(n),
            Ok(None) => Err(format!("KeyError: Key path {key} not found")),
            Err(why) => Err(why),
        };
        let free = match quantity("free") { Ok(n) => n, Err(why) => return Ok(Err(why)) };
        let locked = match quantity("locked") { Ok(n) => n, Err(why) => return Ok(Err(why)) };
        found.insert(*asset, (free, locked));
    }
    Ok(Ok(asset_ids.iter().filter_map(|id| found.remove(id).map(|(free, locked)| (*id, free, locked))).filter(|(_, free, locked)| (free + locked).is_positive()).collect()))
}

/// Utilities::Currency#fiat?'s asset half: the first asset by that symbol is of category Currency.
fn first_asset_is_currency(c: &Connection, symbol: &str) -> Result<bool, SyncError> {
    Ok(c.query_row("SELECT category FROM assets WHERE symbol = ?1 ORDER BY id LIMIT 1", [symbol], |r| r.get::<_, Option<String>>(0)).optional()?.flatten().as_deref() == Some("Currency"))
}

/// Utilities::Currency#fiat_to_fiat_rate into USD over MarketData.get_exchange_rates' `data`: both currencies' BTC-based
/// `value` read as Floats, both positive, USD's over the currency's; then `to_d` (Float#to_d). None where Rails has a
/// Failure.
/// Err: a rate `to_f` refuses, named (the sync stops there rather than leave the cash at a stale price).
fn fiat_rate(rates: &Value, symbol: &str) -> Result<Option<BigDec>, String> {
    let value = |currency: &str| crate::jobs::data_api::to_f(&rates[currency]["value"]).map_err(|why| format!("unreadable exchange rate for {currency}: {why}"));
    let (Some(from), Some(to)) = (value(&symbol.to_lowercase())?, value("usd")?) else { return Ok(None) };
    if !(from > 0.0 && to > 0.0) { return Ok(None); }
    Ok(BigDec::from_f64(to / from).ok())
}

/// AccountBalance::Sync#sync! from the venue's balances on: other cash at its exchange rate, the market's prices, the
/// checks, then the rows `BATCH` at a time, what is gone, and the key's clock. `fresh` holds the venue's own quotes
/// (Alpaca's stocks), `errors` why some could not be read. `Ok(Err(text))`: the sync fails with `text`, nothing written.
#[allow(clippy::too_many_arguments)]
async fn finish(db: &Db, version: &crate::engine::model::CredentialVersion, catalog: &Catalog, held: Vec<Held>, assets: Arc<Vec<Asset>>,
                mut fresh: HashMap<String, Price>, mut errors: Vec<String>, prices: &dyn PriceSource, key_id: i64, clock: &dyn Clock, step: Option<&BalanceStep<'_>>)
                -> Result<Result<Summary, String>, SyncError> {
    // Cash is decided by category, never by symbol, and wins over every feed: a dollar is worth a dollar.
    // Other cash is converted at the market API's exchange rates (cash_prices_for); a currency with no rate is left out
    // and goes stale or unpriced, never to the coin market (where Rails sends it: `usd` and `eur` are coin ids there too).
    let cash = |a: &Asset| a.category.as_deref().is_some_and(|c| CASH_CATEGORIES.contains(&c));
    let mut rated: HashMap<String, Price> = HashMap::new();
    for a in assets.iter().filter(|a| cash(a) && a.symbol.as_deref() != Some("USD")) {
        let Some(symbol) = a.symbol.as_deref().map(str::to_uppercase) else { continue };
        if let Some(Price::Decimal(rate)) = rated.get(&symbol) { fresh.insert(a.external_id.clone(), Price::Decimal(rate.clone())); continue; }
        let fiat = { let s = symbol.clone(); phase(db, move |c| first_asset_is_currency(c, &s)).await? } || crate::figures::totals::FIAT_SYMBOLS.contains(&symbol.as_str());
        if !fiat { continue; }
        // Utilities::Currency.exchange_rate: a rate computed within the last 12 hours is used as it is.
        if let Some(rate) = prices.rate_cache().and_then(|cache| cache.get(&symbol, clock.now())) {
            rated.insert(symbol, Price::Decimal(rate.clone()));
            fresh.insert(a.external_id.clone(), Price::Decimal(rate));
            continue;
        }
        match prices.exchange_rates().await {
            Ok(rates) => if let Some(rate) = match fiat_rate(&rates, &symbol) { Ok(rate) => rate, Err(refused) => return Ok(Err(refused)) } {
                if let Some(cache) = prices.rate_cache() { cache.put(&symbol, rate.clone(), clock.now()); }
                rated.insert(symbol, Price::Decimal(rate.clone()));
                fresh.insert(a.external_id.clone(), Price::Decimal(rate));
            },
            Err(ApiError::Transient(text)) => return Ok(Err(format!("Client::TransientNetworkError: {text}"))),
            Err(ApiError::Refused(text)) => return Ok(Err(text)),
            Err(ApiError::Failed { .. }) => {}
        }
    }
    let mut remaining: Vec<String> = vec![];
    for a in assets.iter().filter(|a| !cash(a) && !fresh.contains_key(&a.external_id)) {
        if !remaining.contains(&a.external_id) { remaining.push(a.external_id.clone()); }
    }
    if !remaining.is_empty() {
        match prices.prices(&remaining, "usd").await {
            Ok(found) => { for (id, p) in found { fresh.entry(id).or_insert(Price::Float(p)); } }
            // Rails' client raises on a transport failure a retry could fix, and the job records it and stops.
            Err(ApiError::Transient(text)) => return Ok(Err(format!("Client::TransientNetworkError: {text}"))),
            // A price the engine will not read: the sync stops and says which, rather than go on at the last prices.
            Err(ApiError::Refused(text)) => return Ok(Err(text)),
            // Any other failure: hosted Rails has no CoinGecko key to fall to, and goes on with its last prices.
            Err(ApiError::Failed { .. }) => errors.push(NO_PROVIDER.into()),
        }
    }
    for a in assets.iter().filter(|a| cash(a) && a.symbol.as_deref() == Some("USD")) { fresh.insert(a.external_id.clone(), Price::Decimal(BigDec::one())); }

    // Every price, and every value a row will hold (from a fresh price or the last stored one), within a venue
    // number's caps before the first write. Rails has no such check: a price of 1e300 on a billion coins would be
    // stored as an infinite value and reported as a success. Here the sync fails and writes no balance.
    if !fresh.values().all(|p| within(price_f(p))) { return Ok(Err(PRICE_OUT_OF_RANGE.into())); }
    let (user_id, exchange_id) = (catalog.user_id, catalog.exchange_id);
    let origin = version.clone();
    let (last, allow_old) = phase(db, move |c| {
        let allowed = super::cache::produced_by(c, user_id, exchange_id, &origin).map_err(|_| SyncError("cannot read balance cache origin".into()))?;
        Ok((if allowed {last_prices(c, user_id, exchange_id)?} else {HashMap::new()}, allowed))
    }).await?;
    for (asset_id, free, locked) in &held {
        let total = &(free + locked);
        let Some(asset) = assets.iter().find(|a| a.id == *asset_id) else { continue };
        let price = match fresh.get(&asset.external_id) {
            Some(Price::Float(f)) => BigDec::from_f64(*f).ok(),
            Some(Price::Decimal(d)) => Some(d.clone()),
            None => last.get(asset_id).cloned().flatten(),
        };
        if price.is_some_and(|p| !within(value_of(total, &p))) { return Ok(Err(VALUE_OUT_OF_RANGE.into())); }
    }

    // The rows, `BATCH` at a time; then what is gone, and the key's clock in the last unit.
    let now = clock.now();
    let (held, fresh) = (Arc::new(held), Arc::new(fresh));
    let origin = version.clone();
    super::commit_for(db, version, move |c| {
        super::cache::record(c, user_id, exchange_id, &origin, false, now).map_err(|_| SyncError("cannot record balance cache origin".into()))
    }).await?;
    if let Some(step)=step {step(0).await;}
    let mut summary = Summary::default();
    for from in (0..held.len()).step_by(BATCH) {
        let (held, assets, fresh) = (held.clone(), assets.clone(), fresh.clone());
        summary = super::commit_for(db, version, move |c| upsert(c, user_id, exchange_id, &held[from..(from + BATCH).min(held.len())], &assets, &fresh, summary, now, allow_old)).await?;
        if let Some(step)=step {step(from/BATCH+1).await;}
    }
    let kept: HashSet<i64> = held.iter().map(|(id, _, _)| *id).collect();
    let mut gone = phase(db, move |c| gone(c, user_id, exchange_id, &kept)).await?;
    loop {
        let rest = gone.split_off(gone.len().min(BATCH));
        let last = rest.is_empty();
        let origin = version.clone();
        super::commit_for(db, version, move |c| {
            for id in &gone { c.execute("DELETE FROM account_balances WHERE id = ?1", [id])?; }
            if last {
                c.execute("UPDATE api_keys SET balances_synced_at = ?1 WHERE id = ?2", params![sql_time(now), key_id])?;
                super::cache::record(c, user_id, exchange_id, &origin, true, now).map_err(|_| SyncError("cannot record balance cache origin".into()))?;
            }
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
    let mut s = c.prepare("SELECT t.ticker, a.category, t.base_asset_id FROM tickers t JOIN assets a ON a.id=t.base_asset_id WHERE t.exchange_id = ?1 ORDER BY t.id")?;
    let tickers = s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    let mut s = c.prepare("SELECT DISTINCT a.id FROM assets a JOIN exchange_assets ea ON ea.asset_id=a.id WHERE ea.exchange_id=?1 AND a.category IN ('Fiat','Currency') AND a.symbol='USD'")?;
    let cash_ids = s.query_map([exchange_id], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    Ok(Catalog { user_id, exchange_id, ..Catalog::new(asset_ids, &tickers, cash_ids) })
}

impl Catalog {
    /// Exchanges::Alpaca#live_ticker_index over `tickers` (native ticker, base category, base asset id): a Stock or
    /// Cryptocurrency position is named by its class and native spelling (crypto also without the slash). `asset_ids` is
    /// `exchange.assets`, `cash_ids` the USD cash assets among them.
    pub fn new(asset_ids: Vec<i64>, tickers: &[(String, Option<String>, i64)], cash_ids: Vec<i64>) -> Self {
        let mut by_symbol: HashMap<(String, String), HashSet<i64>> = HashMap::new();
        for (native, category, asset) in tickers {
            let Some(category) = category.as_ref().filter(|c| *c == "Stock" || *c == "Cryptocurrency") else { continue };
            let native = position_spelling(native).to_string();
            let mut names = vec![native.clone()];
            if category == "Cryptocurrency" { names.push(native.replace('/', "")); }
            for name in names { by_symbol.entry((category.clone(), name)).or_default().insert(*asset); }
        }
        Catalog { user_id: 0, exchange_id: 0, asset_ids, by_symbol, cash_ids }
    }

    /// The distinct assets a position of this class and symbol could be (Rails' `candidates`, uniq by id).
    pub(crate) fn candidates(&self, category: &str, symbol: &str) -> Option<&HashSet<i64>> {
        self.by_symbol.get(&(category.to_string(), symbol.to_string()))
    }
}

pub(crate) fn position_spelling(native: &str) -> &str {
    native.strip_prefix("__stale_").and_then(|rest| {
        let (digits, name) = rest.split_once('_')?;
        (!digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit())).then_some(name)
    }).unwrap_or(native)
}

/// Unsupported/unmapped positions are individually excluded; cash and resolvable positions still publish.
/// Each held asset as (id, free, locked). A position's `qty` counts shares already committed to open sell orders and
/// `qty_available` what is left to sell: free is the second, locked the rest, so the holding stays whole. Both are
/// required, as in Rails (`a position without a quantity`).
pub(crate) fn complete_balances(catalog: &Catalog, cash: BigDec, positions: Vec<Position>) -> Result<Vec<(i64, BigDec, BigDec)>, String> {
    let mut free: HashMap<i64, (BigDec, BigDec)> = HashMap::new();
    if let [usd] = catalog.cash_ids.as_slice() { free.insert(*usd, (cash, BigDec::zero())); }
    else if !cash.is_zero() { return Err("Unmapped or ambiguous Alpaca cash asset".into()); }
    for (category, symbol, raw) in positions {
        let ids = catalog.by_symbol.get(&(category, symbol));
        let Some(asset) = ids.filter(|ids| ids.len() == 1).and_then(|ids| ids.iter().next()).copied().filter(|id| catalog.asset_ids.contains(id)) else {
            eprintln!("Alpaca position skipped: unmapped or ambiguous identity");
            continue;
        };
        let position = Raw::from_node(&raw).map_err(|_| "unreadable position".to_string())?;
        let missing = || "a position without a quantity".to_string();
        let (qty, available) = (position.number("qty")?, position.number("qty_available")?);
        let (qty, available) = (qty.ok_or_else(missing)?, available.ok_or_else(missing)?);
        let locked = &qty - &available;
        free.insert(asset, (available, locked));
    }
    Ok(catalog.asset_ids.iter().filter_map(|id| free.remove(id).map(|(free, locked)| (*id, free, locked))).collect())
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
fn upsert(c: &crate::engine::model::FencedTransaction<'_>, user_id: i64, exchange_id: i64, held: &[(i64, BigDec, BigDec)], assets: &[Asset], fresh: &HashMap<String, Price>, mut summary: Summary,
          now: DateTime<Utc>, allow_old: bool) -> Result<Summary, SyncError> {
    let at = sql_time(now);
    for (asset_id, free, locked) in held {
        let total = &(free + locked);
        let Some(asset) = assets.iter().find(|a| a.id == *asset_id) else { continue };
        let existing = c.query_row("SELECT id, usd_price FROM account_balances WHERE user_id = ?1 AND exchange_id = ?2 AND asset_id = ?3 LIMIT 1",
                                   params![user_id, exchange_id, asset_id], |r| Ok((r.get::<_, i64>(0)?, stored_decimal(r.get_ref(1)?, 8)))).optional()?;
        let (id, old_price) = match existing { Some((id, price)) => (Some(id), if allow_old {price?} else {None}), None => (None, None) };
        let (free, locked) = (cast_decimal(free, 16).to_f(), cast_decimal(locked, 16).to_f());
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
                    Some(id) => c.execute("UPDATE account_balances SET free = ?1, locked = ?6, usd_price = ?2, priced_at = ?3, usd_value = ?4, synced_at = ?3, updated_at = ?3 WHERE id = ?5",
                                          params![free, stored.to_f(), at, value(&exact)?, id, locked])?,
                    None => c.execute("INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, usd_price, priced_at, usd_value, synced_at, created_at, updated_at) \
                                       VALUES (?1, ?2, ?3, ?4, ?8, ?5, ?6, ?7, ?6, ?6, ?6)", params![user_id, exchange_id, asset_id, free, stored.to_f(), at, value(&exact)?, locked])?,
                };
            }
            // Stale: the last price and its time stay as they are; the value follows the new quantity.
            (None, Some(price)) => {
                summary.priced_stale += 1;
                c.execute("UPDATE account_balances SET free = ?1, locked = ?5, usd_value = ?2, synced_at = ?3, updated_at = ?3 WHERE id = ?4", params![free, value(&price)?, at, id, locked])?;
            }
            (None, None) => {
                summary.unpriced += 1;
                match id {
                    Some(id) => c.execute("UPDATE account_balances SET free = ?1, locked = ?4, usd_price = NULL, priced_at = NULL, usd_value = NULL, synced_at = ?2, updated_at = ?2 WHERE id = ?3",
                                          params![free, at, id, locked])?,
                    None => c.execute("INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, synced_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?6, ?5, ?5, ?5)",
                                      params![user_id, exchange_id, asset_id, free, at, locked])?,
                };
            }
        }
    }
    Ok(summary)
}

/// Exchanges::Bybit#get_balances through the common sync and credential fence.
pub async fn sync_bybit<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials, clock: &dyn Clock)
                                       -> Result<Result<Summary, Failure>, SyncError> {
    let version = super::capture_bound_version(db, key_id, credentials).await?;
    sync_bybit_captured(db, transport, prices, key_id, credentials, clock, &version).await
}
pub(crate) async fn sync_bybit_captured<T: Transport>(db: &Db, transport: T, prices: &dyn PriceSource, key_id: i64, credentials: &Credentials,
                                                       clock: &dyn Clock, version: &crate::engine::model::CredentialVersion)
                                                       -> Result<Result<Summary, Failure>, SyncError> {
    use crate::venue::bybit::{BybitVenue, ASSETS_PATH, BYBIT, URL};
    let read = move |_: String| async move { Ok(BybitVenue::new(transport, URL).read_text(ASSETS_PATH, vec![("accountType", "UNIFIED".into())], MAX_BINANCE_ACCOUNT_BYTES).await) };
    let venue = VenueBalances { types: &[BYBIT], failure_text: crate::venue::bybit::failure_text, condemns: |venue, text| condemns(&crate::engine::venue_rules::BYBIT, venue, text), held: bybit_held };
    sync_venue(db, version, prices, key_id, credentials, clock, venue, read).await
}
fn bybit_held(text: &str, names: &HashMap<String, i64>, asset_ids: &[i64]) -> Result<Result<Vec<Held>, (String, bool)>, Unread> {
    let node = wire::read(text, &mut Budget(MAX_BINANCE_ACCOUNT_NODES), None).map_err(|r| Unread::refused(r, "an account"))?;
    let Ok(body) = Raw::from_node(&node) else { return Ok(Err(("unreadable account".into(), false))) };
    if body.value["retCode"].as_i64() != Some(0) { return Ok(Err((crate::venue::bybit::to_s(&body.value["retMsg"]), true))); }
    let result = match body.object("result") { Ok(Some(r)) => r, _ => return Ok(Err(("KeyError: Key path result.list not found".into(), false))) };
    let Some(Node::Array(accounts)) = result.member("list") else { return Ok(Err(("KeyError: Key path result.list not found".into(), false))) };
    let mut found = HashMap::new();
    for account in accounts {
        let Ok(account) = Raw::from_node(account) else { return Ok(Err(("unreadable account".into(), false))) };
        let Some(Node::Array(coins)) = account.member("coin") else { return Ok(Err(("KeyError: Key path coin not found".into(), false))) };
        for coin in coins {
            let Ok(row) = Raw::from_node(coin) else { return Ok(Err(("unreadable balance".into(), false))) };
            let Some(asset) = row.value["coin"].as_str().and_then(|c| names.get(c)).filter(|id| asset_ids.contains(id)) else { continue };
            let amounts = || -> Result<(BigDec, BigDec), String> {
                let present = |key: &str| row.value.get(key).is_some_and(|v| !v.is_null() && v.as_str().is_none_or(|s| !crate::ruby::blank(s)));
                let locked = row.decimal("locked")?;
                let free = if present("walletBalance") { (&(&row.decimal("walletBalance")? - &locked) - &row.decimal("spotBorrow")?).max(BigDec::zero()) }
                           else { row.decimal(if present("availableToWithdraw") { "availableToWithdraw" } else { "free" })? };
                Ok((free, locked))
            };
            match amounts() { Ok(amounts) => { found.insert(*asset, amounts); }, Err(e) => return Ok(Err((e, false))) }
        }
    }
    Ok(Ok(asset_ids.iter().filter_map(|id| found.remove(id).map(|(free, locked)| (*id, free, locked))).filter(|(_, free, locked)| (free + locked).is_positive()).collect()))
}
