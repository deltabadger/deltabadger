//! Tax::PriceService (app/models/tax/price_service.rb) and Tax::AssetIdentity (app/models/tax/asset_identity.rb) for
//! the walk: a symbol's price on a day, read from `historical_prices` and fetched from data-api where Rails fetches it
//! (`prefetch`, `fetch_single_price`), fiat converted at the ECB's rates (`convert_fiat`) with Rails' BTC cross when a
//! rate is missing, and the warnings every lookup that found nothing leaves (a price nobody has is a zero and a
//! warning: the figure is incomplete, never refused).
//!
//! A pass reads the database once (`Reference`, `FxTable`); a fetch is made inline, at the lookup Rails makes it at,
//! through a `Fetcher` (the job hands the request to its runtime thread and blocks for the answer), and what it brings
//! serves the rest of the pass, as Rails' `@price_cache` does.
use super::fx::FxTable;
use super::rows::Stored;
use super::{fiat, stable};
use crate::figures::{at::At, dec::Dec, FiguresError};
use crate::jobs::data_api::{ApiError, DataApi};
use crate::venue::http::Transport;
use chrono::{Days, NaiveDate};
use rusqlite::{Connection, OptionalExtension};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Tax::PriceService::SINGLE_PRICE_WINDOW: half the window asked for around one day.
pub const SINGLE_PRICE_WINDOW: u64 = 60;

/// One range of daily prices Rails fetches (Tax::PriceService#fetch_price_range).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetch { pub coin: String, pub symbol: String, pub currency: String, pub from: NaiveDate, pub to: NaiveDate }

impl Fetch {
    pub fn usd(coin: String, symbol: String, from: NaiveDate, to: NaiveDate) -> Fetch { Fetch { coin, symbol, currency: "USD".into(), from, to } }
}

/// Where a pass's fetches go: `range` makes the request for the days `have` lacks, stores what came back, and returns
/// the new days' prices (an empty answer when data-api had none). An `Err` ends the walk (a transport failure Rails
/// raises out of the job, or a provider this build does not fetch from).
pub trait Fetcher { fn range(&mut self, f: &Fetch, have: &HashSet<NaiveDate>) -> Result<Vec<(NaiveDate, Dec)>, String>; }

/// A fetcher for passes that must ask nothing: every request is an error.
pub struct NoFetch;
impl Fetcher for NoFetch {
    fn range(&mut self, f: &Fetch, _: &HashSet<NaiveDate>) -> Result<Vec<(NaiveDate, Dec)>, String> { Err(format!("unexpected price fetch for {} {}..{}", f.symbol, f.from, f.to)) }
}

/// A fetcher that asks nothing and finds nothing: every lookup it is asked for stays a warning.
pub struct Unfetched;
impl Fetcher for Unfetched {
    fn range(&mut self, _: &Fetch, _: &HashSet<NaiveDate>) -> Result<Vec<(NaiveDate, Dec)>, String> { Ok(vec![]) }
}

/// A venue as a symbol is read on it: its id, `name_id`, and whether it is a stock venue (Exchange#stock_venue?).
#[derive(Clone, Debug, PartialEq)]
pub struct Venue { pub id: i64, pub name_id: String, pub stock: bool }

impl Venue {
    pub fn alpaca(c: &Connection) -> Result<Venue, FiguresError> {
        let id: Option<i64> = c.query_row("SELECT id FROM exchanges WHERE type = ?1 ORDER BY id LIMIT 1", [super::VENUE_TYPE], |r| r.get(0)).optional()?;
        Ok(Venue { id: id.unwrap_or(0), name_id: super::VENUE.into(), stock: true })
    }
}

/// Exchange::STOCK_TYPES.
const STOCK_TYPES: [&str; 2] = ["Exchanges::Alpaca", "Exchanges::Ibkr"];

/// Tax::AssetIdentity::ALIASES: (symbol, coin, venue, last day it holds).
type Alias = (&'static str, &'static str, Option<&'static str>, Option<(i32, u32, u32)>);
const ALIASES: [Alias; 8] = [
    ("LUNA", "terra-luna", None, Some((2022, 5, 27))),
    ("QUICK", "quick", None, Some((2023, 7, 20))),
    ("MATIC", "matic-network", None, None),
    ("LIT", "litentry", Some("binance"), None),
    ("GAL", "project-galaxy", Some("binance"), None),
    ("DAR", "mines-of-dalarnia", Some("binance"), None),
    ("PNT", "pnetwork", None, None),
    ("BURGER", "burger-swap", None, None),
];

fn ymd((y, m, d): (i32, u32, u32)) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, d).unwrap_or(NaiveDate::MIN) }
fn fits(alias_venue: Option<&str>, venue: Option<&Venue>) -> bool { alias_venue.is_none_or(|v| venue.is_some_and(|venue| v == venue.name_id)) }

/// Tax::AssetIdentity.alias_coin.
pub fn alias_coin(symbol: &str, venue: Option<&Venue>, at: Option<NaiveDate>) -> Option<String> {
    if symbol.trim().is_empty() { return None; }
    ALIASES.iter()
        .find(|(s, _, v, until)| *s == symbol && fits(*v, venue) && until.is_none_or(|u| at.is_some_and(|at| at <= ymd(u))))
        .map(|(_, coin, _, _)| coin.to_string())
}

/// One catalogue asset: its id, external id and category.
pub type Asset = (i64, Option<String>, Option<String>);

/// Tax::AssetIdentity::STOCK_CATEGORIES: the categories a broker's securities are filed under.
pub const STOCK_CATEGORIES: [&str; 4] = ["Stock", "Common Stock", "ETF", "Fund"];
fn security(a: &Asset) -> bool { a.2.as_deref().is_some_and(|c| STOCK_CATEGORIES.contains(&c)) }
fn crypto(a: &Asset) -> bool { a.2.as_deref() == Some("Cryptocurrency") }

/// What a pass reads of the catalogue, the venues and the stored prices, for the symbols it names: each table read in
/// one query, every row read a step of the open budget scope, and indexed in memory, so no lookup issues SQL.
#[derive(Default, Clone)]
pub struct Reference {
    /// The venue `listed_category` answers for.
    default: i64,
    /// `Exchange.find_by(type:)` per name_id: the first exchange of each type.
    venues: HashMap<String, Venue>,
    /// (exchange id, symbol) → the assets the venue lists under it, in ticker order, each once.
    listed: HashMap<(i64, String), Vec<Asset>>,
    /// symbol → every asset of the catalogue under it, by market rank.
    assets: HashMap<String, Vec<Asset>>,
    /// (price key: the symbol, or `stock:SYM`; currency) → day → the stored price, as Rails reads the column.
    prices: HashMap<(String, String), BTreeMap<NaiveDate, Dec>>,
}

impl Reference {
    /// The venues, and the catalogue and the prices of `symbols` (and of `stock:<symbol>`), in every currency.
    pub fn load(c: &Connection, default: &Venue, symbols: &HashSet<String>) -> Result<Reference, FiguresError> {
        let mut r = Reference { default: default.id, ..Reference::default() };
        let mut s = c.prepare("SELECT id, type FROM exchanges ORDER BY id")?;
        let mut q = s.query([])?;
        while let Some(row) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let (id, kind): (i64, Option<String>) = (row.get(0)?, row.get(1)?);
            let Some(kind) = kind else { continue };
            let name = super::rows::name_id(&kind);
            r.venues.entry(name.clone()).or_insert(Venue { id, name_id: name, stock: STOCK_TYPES.contains(&kind.as_str()) });
        }
        let mut s = c.prepare("SELECT t.exchange_id, t.base, a.id, a.external_id, a.category FROM tickers t JOIN assets a ON a.id = t.base_asset_id ORDER BY t.id")?;
        let mut q = s.query([])?;
        while let Some(row) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let (exchange, base): (i64, String) = (row.get(0)?, row.get(1)?);
            if symbols.contains(&base) {
                let asset: Asset = (row.get(2)?, row.get(3)?, row.get(4)?);
                let list = r.listed.entry((exchange, base)).or_default();
                if !list.iter().any(|a| a.0 == asset.0) { list.push(asset); }
            }
        }
        let mut s = c.prepare("SELECT symbol, id, external_id, category FROM assets ORDER BY market_cap_rank IS NULL, market_cap_rank, id")?;
        let mut q = s.query([])?;
        while let Some(row) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let Some(symbol) = row.get::<_, Option<String>>(0)? else { continue };
            if symbols.contains(&symbol) { r.assets.entry(symbol).or_default().push((row.get(1)?, row.get(2)?, row.get(3)?)); }
        }
        let mut s = c.prepare("SELECT asset, currency, date, price FROM historical_prices")?;
        let mut q = s.query([])?;
        while let Some(row) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let asset: String = row.get(0)?;
            if !(symbols.contains(&asset) || asset.strip_prefix("stock:").is_some_and(|sym| symbols.contains(sym))) { continue; }
            let currency: String = row.get(1)?;
            let (Ok(day), Some(price)) = (NaiveDate::parse_from_str(&row.get::<_, String>(2)?, "%Y-%m-%d"), Dec::from_sql(row.get_ref(3)?)?) else { continue };
            r.prices.entry((asset, currency)).or_default().insert(day, price);
        }
        Ok(r)
    }

    /// `Exchange.find_by(type: "Exchanges::#{name.camelize}")`.
    pub fn venue(&self, name_id: &str) -> Option<&Venue> { self.venues.get(name_id) }

    /// `HistoricalPrice.lookup` in USD: the stored price of `asset` on `date`. A zero is no price.
    pub fn price(&self, asset: &str, date: NaiveDate) -> Option<&Dec> { self.price_in(asset, "USD", date) }
    pub fn price_in(&self, asset: &str, currency: &str, date: NaiveDate) -> Option<&Dec> {
        self.prices.get(&(asset.to_string(), currency.to_string()))?.get(&date).filter(|p| !p.is_zero())
    }
    /// Whether any row, a zero included, is stored for the day (`fetch_price_range`'s coverage test).
    fn stored(&self, asset: &str, currency: &str, date: NaiveDate) -> bool {
        self.prices.get(&(asset.to_string(), currency.to_string())).is_some_and(|d| d.contains_key(&date))
    }

    /// The stored USD prices of `asset` over `from..=to`, by day; none when `from` is after `to`.
    pub fn prices_over(&self, asset: &str, from: NaiveDate, to: NaiveDate) -> impl Iterator<Item = (&NaiveDate, &Dec)> {
        self.prices.get(&(asset.to_string(), "USD".to_string())).filter(|_| from <= to).into_iter().flat_map(move |days| days.range(from..=to))
    }

    /// The category of the asset `venue_id` lists under `symbol`: None when it lists nothing, Some(None) uncategorised.
    pub fn listed_category_on(&self, venue_id: i64, symbol: &str) -> Option<Option<&str>> { self.listed.get(&(venue_id, symbol.to_string())).and_then(|l| l.first()).map(|a| a.2.as_deref()) }
    /// The category of the asset the default venue lists under `symbol`: None when it lists nothing.
    pub fn listed_category(&self, symbol: &str) -> Option<Option<&str>> { self.listed_category_on(self.default, symbol) }
}

/// Every symbol the rows name: base, quote and fee, and BTC (the cross rate a missing ECB rate falls back to).
pub fn symbols(rows: &[Stored]) -> HashSet<String> {
    let mut s: HashSet<String> = rows.iter().flat_map(|r| std::iter::once(r.base.clone()).chain(r.quote.clone()).chain(r.fee_currency.clone())).collect();
    s.insert("BTC".into());
    s
}

/// Tax::AssetIdentity.catalogue_coin: the venue's first Cryptocurrency listing under the symbol, else the catalogue's
/// first Cryptocurrency by market rank. Never a security.
pub fn catalogue_coin_on(r: &Reference, symbol: &str, venue: Option<&Venue>) -> Option<String> {
    if symbol.trim().is_empty() { return None; }
    let listed = venue.and_then(|v| r.listed.get(&(v.id, symbol.to_string()))).and_then(|l| l.iter().find(|a| crypto(a)));
    let ranked = || r.assets.get(symbol).and_then(|l| l.iter().find(|a| crypto(a)));
    listed.or_else(ranked).and_then(|a| a.1.clone())
}

/// `identity`'s partition: Some((true, the security)) for securities only, Some((false, the one coin)) for no security,
/// None for both.
pub fn classify(candidates: &[Asset]) -> Option<(bool, Option<Asset>)> {
    let mut unique: Vec<&Asset> = vec![];
    for a in candidates { if !unique.iter().any(|u| u.0 == a.0) { unique.push(a); } }
    let (stocks, coins): (Vec<&Asset>, Vec<&Asset>) = unique.into_iter().partition(|a| security(a));
    match (stocks.first(), coins.len()) {
        (None, 1) => Some((false, Some(coins[0].clone()))),
        (None, _) => Some((false, None)),
        (Some(s), 0) => Some((true, Some((*s).clone()))),
        _ => None,
    }
}

/// Tax::AssetIdentity.identity: on a crypto venue a coin; on a stock venue the assets recorded under the name decide,
/// else what the venue lists, else the catalogue by rank.
pub fn identity(r: &Reference, symbol: &str, venue: Option<&Venue>, recorded: &[Asset]) -> Option<(bool, Option<Asset>)> {
    let Some(v) = venue.filter(|v| v.stock) else { return Some((false, None)) };
    let listed = r.listed.get(&(v.id, symbol.to_string())).filter(|l| !l.is_empty());
    let catalogue = || r.assets.get(symbol).cloned().unwrap_or_default();
    let candidates: Vec<Asset> = if !recorded.is_empty() { recorded.to_vec() } else { listed.cloned().unwrap_or_else(catalogue) };
    classify(&candidates)
}

/// Tax::AssetIdentity.price_key: the key a symbol's prices are stored under and the security when it is one; None for a
/// name that is still both. A security's key is `stock:SYM` whenever a coin has the symbol (the catalogue, or
/// `recorded_coins`), else its symbol.
pub fn price_key(r: &Reference, symbol: &str, venue: Option<&Venue>, at: Option<NaiveDate>, recorded: &[Asset], recorded_coins: bool) -> Option<(String, Option<Asset>)> {
    if alias_coin(symbol, venue, at).is_some() { return Some((symbol.to_string(), None)); }
    let (stock, asset) = identity(r, symbol, venue, recorded)?;
    if !stock { return Some((symbol.to_string(), None)); }
    let shared = recorded_coins || r.assets.get(symbol).is_some_and(|l| l.iter().any(crypto));
    Some((if shared { format!("stock:{symbol}") } else { symbol.to_string() }, asset))
}

/// Tax::AssetIdentity.dated_alias?: whether a dated alias cuts this symbol's history on this venue.
pub fn dated_alias(symbol: &str, venue: Option<&Venue>) -> bool {
    ALIASES.iter().any(|(s, _, v, until)| *s == symbol && until.is_some() && fits(*v, venue))
}

/// TrackerHelper#tracker_price_key, read off the database for one row of the table: the row's asset, else what the
/// owner's rows at its venue recorded under the name, else the venue's listings, else the catalogue.
pub fn row_price_key(c: &Connection, owner: i64, exchange_id: i64, venue_type: &str, symbol: &str, own: Option<i64>, at: NaiveDate) -> Result<Option<String>, rusqlite::Error> {
    if !STOCK_TYPES.contains(&venue_type) { return Ok(Some(symbol.to_string())); }
    let venue = Venue { id: exchange_id, name_id: super::rows::name_id(venue_type), stock: true };
    if alias_coin(symbol, Some(&venue), Some(at)).is_some() { return Ok(Some(symbol.to_string())); }
    let read = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> Result<Vec<Asset>, rusqlite::Error> {
        let mut s = c.prepare_cached(sql)?;
        let rows = s.query_map(params, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    };
    let mut candidates = match own {
        Some(id) => read("SELECT id, external_id, category FROM assets WHERE id = ?1", rusqlite::params![id])?,
        None => read("SELECT a.id, a.external_id, a.category FROM assets a WHERE a.id IN (SELECT base_asset_id FROM account_transactions \
                      WHERE user_id = ?1 AND exchange_id = ?2 AND base_currency = ?3) ORDER BY a.id", rusqlite::params![owner, exchange_id, symbol])?,
    };
    if candidates.is_empty() {
        candidates = read("SELECT a.id, a.external_id, a.category FROM tickers t JOIN assets a ON a.id = t.base_asset_id WHERE t.exchange_id = ?1 AND t.base = ?2 ORDER BY t.id",
                          rusqlite::params![exchange_id, symbol])?;
    }
    if candidates.is_empty() {
        candidates = read("SELECT id, external_id, category FROM assets WHERE symbol = ?1 ORDER BY market_cap_rank IS NULL, market_cap_rank, id", rusqlite::params![symbol])?;
    }
    let Some((stock, _)) = classify(&candidates) else { return Ok(None) };
    if !stock { return Ok(Some(symbol.to_string())); }
    let shared: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE symbol = ?2 AND category = 'Cryptocurrency') \
                                    OR EXISTS(SELECT 1 FROM account_transactions t JOIN assets a ON a.id = t.base_asset_id \
                                              WHERE t.user_id = ?1 AND t.base_currency = ?2 AND a.category = 'Cryptocurrency')",
                                   rusqlite::params![owner, symbol], |r| r.get(0))?;
    Ok(Some(if shared { format!("stock:{symbol}") } else { symbol.to_string() }))
}
pub fn catalogue_coin(r: &Reference, symbol: &str, venue: &Venue) -> Option<String> { catalogue_coin_on(r, symbol, Some(venue)) }

/// Tax::AssetIdentity.coin_id.
pub fn coin_id(r: &Reference, symbol: &str, venue: Option<&Venue>, at: Option<NaiveDate>) -> Option<String> {
    alias_coin(symbol, venue, at).or_else(|| catalogue_coin_on(r, symbol, venue))
}

/// Tax::AssetIdentity.coin_ids_over: the coin a symbol means over each stretch of `from..=to`, cut where a dated alias
/// ends.
pub fn coin_ids_over_on(r: &Reference, symbol: &str, venue: Option<&Venue>, from: NaiveDate, to: NaiveDate) -> Vec<(NaiveDate, NaiveDate, String)> {
    let mut cuts: Vec<NaiveDate> = ALIASES.iter().filter(|(s, _, v, until)| *s == symbol && until.is_some() && fits(*v, venue))
        .filter_map(|(_, _, _, until)| until.map(ymd)).filter(|cut| *cut >= from && *cut < to).collect();
    cuts.sort();
    let starts: Vec<NaiveDate> = std::iter::once(from).chain(cuts.iter().map(|cut| cut.succ_opt().unwrap_or(*cut))).collect();
    let ends: Vec<NaiveDate> = cuts.iter().copied().chain(std::iter::once(to)).collect();
    starts.into_iter().zip(ends).filter_map(|(first, last)| coin_id(r, symbol, venue, Some(first)).map(|coin| (first, last, coin))).collect()
}
pub fn coin_ids_over(r: &Reference, symbol: &str, venue: &Venue, from: NaiveDate, to: NaiveDate) -> Vec<(NaiveDate, NaiveDate, String)> {
    coin_ids_over_on(r, symbol, Some(venue), from, to)
}

fn plus(d: NaiveDate, n: u64) -> NaiveDate { d.checked_add_days(Days::new(n)).unwrap_or(d) }
fn minus(d: NaiveDate, n: u64) -> NaiveDate { d.checked_sub_days(Days::new(n)).unwrap_or(d) }

/// How many days a range spans, inclusive.
pub fn span(from: NaiveDate, to: NaiveDate) -> i64 { (to - from).num_days() + 1 }

/// Tax::PriceService for one pass: its warnings, its caches, and where its fetches go.
pub struct PriceService<'a> {
    pub r: &'a Reference,
    pub fx: &'a FxTable,
    pub today: NaiveDate,
    pub warnings: Vec<String>,
    warned_fx: HashSet<String>,
    fx_cache: HashMap<String, Dec>,
    coin_ids: HashMap<(String, Option<i64>), Option<String>>,
    /// (exchange id, symbol) → the assets the rows `record` was handed recorded there; the names a coin was recorded under.
    recorded: HashMap<(i64, String), Vec<Asset>>,
    recorded_coins: HashSet<String>,
    price_keys: HashMap<(String, Option<i64>, Vec<i64>), Option<(String, Option<Asset>)>>,
    /// (asset, currency) → what this pass's fetches brought.
    fetched: HashMap<(String, String), BTreeMap<NaiveDate, Dec>>,
    fetcher: &'a mut dyn Fetcher,
}

impl<'a> PriceService<'a> {
    pub fn new(r: &'a Reference, fx: &'a FxTable, today: NaiveDate, fetcher: &'a mut dyn Fetcher) -> PriceService<'a> {
        PriceService { r, fx, today, warnings: vec![], warned_fx: HashSet::new(), fx_cache: HashMap::new(), coin_ids: HashMap::new(), recorded: HashMap::new(), recorded_coins: HashSet::new(), price_keys: HashMap::new(), fetched: HashMap::new(), fetcher }
    }

    fn venue(&self, name_id: Option<&str>) -> Option<&'a Venue> { let r: &'a Reference = self.r; name_id.and_then(|n| r.venue(n)) }

    /// A price this pass has: fetched, or stored; never a zero.
    fn known(&self, asset: &str, currency: &str, date: NaiveDate) -> Option<Dec> {
        if let Some(p) = self.fetched.get(&(asset.to_string(), currency.to_string())).and_then(|d| d.get(&date)).filter(|p| !p.is_zero()) { return Some(p.clone()); }
        self.r.price_in(asset, currency, date).cloned()
    }

    /// `record_assets`: what each venue's rows recorded under each name, and the names a coin was recorded under.
    pub fn record(&mut self, rows: &[Stored]) {
        for r in rows {
            let Some(asset) = &r.base_asset else { continue };
            let list = self.recorded.entry((r.exchange_id, r.base.clone())).or_default();
            if !list.iter().any(|a| a.0 == asset.0) { list.push(asset.clone()); }
            if crypto(asset) { self.recorded_coins.insert(r.base.clone()); }
        }
    }

    /// `price_key_for`: the key and the security, cached per venue and recorded asset; a dated alias is a coin's.
    fn price_key_for(&mut self, symbol: &str, exchange: Option<&str>, date: NaiveDate, recorded: Option<&Asset>) -> Option<(String, Option<Asset>)> {
        let venue = self.venue(exchange);
        if alias_coin(symbol, venue, Some(date)).is_some() { return Some((symbol.to_string(), None)); }
        let assets: Vec<Asset> = match recorded {
            Some(a) => vec![a.clone()],
            None => venue.and_then(|v| self.recorded.get(&(v.id, symbol.to_string()))).cloned().unwrap_or_default(),
        };
        let cache = (symbol.to_string(), venue.map(|v| v.id), assets.iter().map(|a| a.0).collect());
        if let Some(hit) = self.price_keys.get(&cache) { return hit.clone(); }
        let key = price_key(self.r, symbol, venue, None, &assets, self.recorded_coins.contains(symbol));
        self.price_keys.insert(cache, key.clone());
        key
    }

    /// `price_at(asset:, currency:, timestamp:, exchange:)`: the price this pass has, else Rails' single-day fetch;
    /// still nothing is a zero and a warning.
    pub fn price_at(&mut self, asset: &str, currency: &str, at: At, exchange: Option<&str>) -> Result<Dec, FiguresError> {
        self.price_at_recorded(asset, currency, at, exchange, None)
    }

    /// `price_at(..., recorded:)`: with the asset the row recorded.
    pub fn price_at_recorded(&mut self, asset: &str, currency: &str, at: At, exchange: Option<&str>, recorded: Option<&Asset>) -> Result<Dec, FiguresError> {
        if asset == currency { return Ok(Dec::one()); }
        if stable(asset) { return self.stablecoin_rate(currency, at); }
        let date = at.utc().date_naive();
        let Some((key, security)) = self.price_key_for(asset, exchange, date, recorded) else {
            self.warnings.push(format!("{asset}/{currency} {date}"));
            return Ok(Dec::zero());
        };
        crate::figures::budget::charge(1, 0)?;
        if let Some(p) = self.known(&key, currency, date) { return Ok(p); }
        match self.fetch_single_price(asset, &key, security.as_ref(), currency, date, exchange)? {
            Some(p) => Ok(p),
            None => { self.warnings.push(format!("{asset}/{currency} {date}")); Ok(Dec::zero()) }
        }
    }

    /// `fetch_single_price`: a window of up to 60 days either side, anchored on its end (never past today), clipped to
    /// the coin the symbol meant that day; then the day's price, if the window brought one.
    fn fetch_single_price(&mut self, asset: &str, key: &str, security: Option<&Asset>, currency: &str, date: NaiveDate, exchange: Option<&str>) -> Result<Option<Dec>, FiguresError> {
        let to = plus(date, SINGLE_PRICE_WINDOW).min(self.today);
        let from = minus(to, SINGLE_PRICE_WINDOW * 2);
        let venue = self.venue(exchange);
        // A security has no dated alias: its own id, over the whole window.
        let ranges = match security {
            Some(s) => s.1.iter().map(|id| (from, to, id.clone())).collect(),
            None => coin_ids_over_on(self.r, asset, venue, from, to),
        };
        let Some((first, last, coin)) = ranges.into_iter().find(|(f, l, _)| *f <= date && date <= *l) else { return Ok(None) };
        let end = if last == to { plus(first, SINGLE_PRICE_WINDOW * 2).min(self.today) } else { last };
        self.fetch_price_range(&Fetch { coin, symbol: key.into(), currency: currency.into(), from: first, to: end })?;
        Ok(self.known(key, currency, date))
    }

    /// `fetch_price_range`: nothing when every day of the range has a row; else the request, and the new days kept.
    pub fn fetch_price_range(&mut self, f: &Fetch) -> Result<(), FiguresError> {
        let key = (f.symbol.clone(), f.currency.clone());
        let mut have: HashSet<NaiveDate> = HashSet::new();
        let mut day = f.from;
        while day <= f.to {
            crate::figures::budget::charge(1, 0)?;
            if self.r.stored(&f.symbol, &f.currency, day) || self.fetched.get(&key).is_some_and(|d| d.contains_key(&day)) { have.insert(day); }
            day = match day.succ_opt() { Some(d) => d, None => break };
        }
        if span(f.from, f.to) <= have.len() as i64 { return Ok(()); }
        let rows = self.fetcher.range(f, &have).map_err(FiguresError::NotComputed)?;
        let days = self.fetched.entry(key).or_default();
        for (day, price) in rows { days.entry(day).or_insert(price); }
        Ok(())
    }

    fn stablecoin_rate(&mut self, currency: &str, at: At) -> Result<Dec, FiguresError> {
        if currency == "USD" { return Ok(Dec::one()); }
        self.fiat_exchange_rate("USD", currency, at)
    }

    /// `convert_fiat`.
    pub fn convert_fiat(&mut self, amount: &Dec, from: &str, to: &str, at: At) -> Result<Dec, FiguresError> {
        if from == to { return Ok(amount.clone()); }
        let rate = self.fiat_exchange_rate(from, to, at)?;
        Ok((amount * &rate)?)
    }

    /// `fiat_exchange_rate`: the ECB rate, cached; a missing one warns on every record that leans on it and falls back to
    /// the BTC cross, else zero.
    fn fiat_exchange_rate(&mut self, from: &str, to: &str, at: At) -> Result<Dec, FiguresError> {
        let date = at.utc().date_naive();
        let key = format!("FX/{from}/{to}/{date}");
        let warning = format!("FX {from}/{to} {date}");
        if self.warned_fx.contains(&key) { self.warnings.push(warning.clone()); }
        if let Some(r) = self.fx_cache.get(&key) { return Ok(r.clone()); }
        if let Some(rate) = self.fx.rate(from, to, date)? {
            self.fx_cache.insert(key, rate.clone());
            return Ok(rate);
        }
        self.warned_fx.insert(key.clone());
        self.warnings.push(warning);
        let fallback = self.btc_cross_rate(from, to, at)?.unwrap_or_else(Dec::zero);
        self.fx_cache.insert(key, fallback.clone());
        Ok(fallback)
    }

    /// `btc_cross_rate`: BTC in both currencies that day; None when either is not positive.
    fn btc_cross_rate(&mut self, from: &str, to: &str, at: At) -> Result<Option<Dec>, FiguresError> {
        let btc_from = self.price_at("BTC", from, at, None)?;
        let btc_to = self.price_at("BTC", to, at, None)?;
        if !(btc_from.is_positive() && btc_to.is_positive()) { return Ok(None); }
        Ok(Some(btc_to.div(&btc_from)?))
    }

    /// `coin_id_for`: the alias of the day, else the catalogue's answer, cached per venue.
    fn coin_id_for(&mut self, symbol: &str, exchange: Option<&str>, date: NaiveDate) -> Option<String> {
        let venue = self.venue(exchange);
        if let Some(coin) = alias_coin(symbol, venue, Some(date)) { return Some(coin); }
        let r = self.r;
        self.coin_ids.entry((symbol.to_string(), venue.map(|v| v.id))).or_insert_with(|| catalogue_coin_on(r, symbol, venue)).clone()
    }

    /// `prefetch(transactions, currency: 'USD')`: one range per (symbol, coin) over the days of the rows that need a
    /// price of ours, in the order the rows first name them, then each range fetched. `skip(row)` is `cash_leg_value`.
    pub fn prefetch(&mut self, rows: &[Stored], skip: &dyn Fn(&Stored) -> bool) -> Result<(), FiguresError> {
        let mut coins: Vec<(String, String, NaiveDate, NaiveDate)> = vec![];
        let mut index: HashMap<(String, String), usize> = HashMap::new();
        let mut add = |me: &mut Self, symbol: Option<&str>, date: NaiveDate, exchange: &str, recorded: Option<&Asset>| {
            let Some(symbol) = symbol.filter(|s| !s.trim().is_empty() && !fiat(s)) else { return };
            let Some((key, security)) = me.price_key_for(symbol, Some(exchange), date, recorded) else { return };
            let coin = match security { Some(s) => s.1, None => me.coin_id_for(symbol, Some(exchange), date) };
            let Some(coin) = coin else { return };
            match index.get(&(key.clone(), coin.clone())) {
                Some(&i) => { let e = &mut coins[i]; e.2 = e.2.min(date); e.3 = e.3.max(date); }
                None => { index.insert((key.clone(), coin.clone()), coins.len()); coins.push((key, coin, date, date)); }
            }
        };
        for r in rows {
            crate::figures::budget::charge(1, 0)?;
            if skip(r) { continue; }
            if r.stated.is_some() && !r.quoted() { continue; }
            if r.quote.as_deref() == Some("USD") && r.quote_amount.is_some() { continue; }
            if r.quote.as_deref().is_some_and(|q| stable(q) || fiat(q)) && r.quote_amount.is_some() { continue; }
            let date = r.date();
            add(self, Some(&r.base), date, &r.venue, r.base_asset.as_ref());
            if r.fee_amount.as_ref().is_some_and(Dec::is_positive) && !r.fee_currency.as_deref().is_some_and(stable) {
                add(self, r.fee_currency.as_deref(), date, &r.venue, None);
            }
        }
        for (symbol, coin, from, to) in coins { self.fetch_price_range(&Fetch::usd(coin, symbol, from, to))?; }
        Ok(())
    }
}

/// What `fetch_range` stores: (asset, date, price as Rails writes it).
pub type PriceRow = (String, NaiveDate, String);

/// The days of `from..=to` that already have a stored price of `asset` in `currency`.
pub fn stored_days(c: &Connection, asset: &str, currency: &str, from: NaiveDate, to: NaiveDate) -> Result<HashSet<NaiveDate>, FiguresError> {
    let mut s = c.prepare("SELECT date FROM historical_prices WHERE asset = ?1 AND currency = ?2 AND date >= ?3 AND date <= ?4")?;
    let days = s.query_map(rusqlite::params![asset, currency, from.to_string(), to.to_string()], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(days.iter().filter_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).collect())
}

/// Whether the table already has every day of a fetch's range (`fetch_price_range` then asks nothing).
pub fn covered(fetch: &Fetch, have: &HashSet<NaiveDate>) -> bool {
    (0..span(fetch.from, fetch.to)).all(|i| have.contains(&plus(fetch.from, i as u64)))
}

/// Tax::PriceService#fetch_price_range's request: nothing when the table covers every day; else data-api's
/// `GET /api/v1/historical_prices` from the first day's midnight to the midnight after the last (UTC), and the days in
/// range it priced that the table lacks, a zero or null left out. A failed answer stores nothing; a transport failure
/// a retry could fix is an error (Rails raises it out of the job).
pub async fn fetch_range<T: Transport>(api: &DataApi<T>, fetch: &Fetch, have: &HashSet<NaiveDate>) -> Result<Vec<PriceRow>, String> {
    if covered(fetch, have) { return Ok(vec![]); }
    let midnight = |d: NaiveDate| d.and_hms_opt(0, 0, 0).map_or(0, |t| t.and_utc().timestamp());
    let (from, to) = (midnight(fetch.from).to_string(), midnight(plus(fetch.to, 1)).to_string());
    let currency = fetch.currency.to_lowercase();
    let body = match api.get("/api/v1/historical_prices", &[("coin_id", &fetch.coin), ("currency", &currency), ("from", &from), ("to", &to)], false).await {
        Ok(body) => body,
        Err(ApiError::Transient(m)) => return Err(m),
        Err(ApiError::Failed { .. } | ApiError::Refused(_)) => return Ok(vec![]),
    };
    let Some(prices) = body["prices"].as_array() else { return Ok(vec![]) };
    let mut seen = have.clone();
    let mut rows = vec![];
    for pair in prices {
        let (Some(ms), Some(price)) = (pair.get(0).and_then(serde_json::Value::as_f64), pair.get(1)) else { continue };
        let Ok(price) = crate::figures::budget::within(|| Dec::to_d(price)) else { continue };
        if price.is_zero() { continue; }
        let Some(day) = chrono::DateTime::from_timestamp_millis(ms.floor() as i64).map(|t| t.date_naive()) else { continue };
        if day < fetch.from || day > fetch.to || !seen.insert(day) { continue; }
        rows.push((fetch.symbol.clone(), day, price.to_s_f()));
    }
    Ok(rows)
}

/// HistoricalPrice.bulk_store in USD: insert-only. Returns how many rows it inserted.
pub fn store(c: &Connection, rows: &[PriceRow]) -> Result<usize, FiguresError> { store_in(c, "USD", rows) }

/// HistoricalPrice.bulk_store in `currency`.
pub fn store_in(c: &Connection, currency: &str, rows: &[PriceRow]) -> Result<usize, FiguresError> {
    let mut s = c.prepare_cached("INSERT INTO historical_prices (asset, currency, date, price) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (asset, currency, date) DO NOTHING")?;
    let mut stored = 0;
    for (asset, date, price) in rows { stored += s.execute(rusqlite::params![asset, currency, date.to_string(), price])?; }
    Ok(stored)
}

/// Authenticated bar results require Q's capability; public market-data writes use store.
pub fn store_bars(c:&crate::engine::model::FencedTransaction<'_>,rows:&[PriceRow])->Result<(),FiguresError>{store(c,rows).map(|_|())}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::data_api::Config;
    use crate::venue::http::ScriptedTransport;

    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }
    fn venue(name: &str) -> Venue { Venue { id: 1, name_id: name.into(), stock: name == "alpaca" } }

    fn catalogue() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT); INSERT INTO exchanges VALUES (1, 'Exchanges::Alpaca'), (2, 'Exchanges::Binance');
                         CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, base TEXT, base_asset_id INTEGER);
                         CREATE TABLE assets (id INTEGER PRIMARY KEY, external_id TEXT, category TEXT, symbol TEXT, market_cap_rank INTEGER);
                         INSERT INTO assets VALUES (1, 'terra-luna-2', 'Cryptocurrency', 'LUNA', 300), (2, 'AAPL.US', 'Stock', 'AAPL', NULL), (3, 'apple-coin', 'Cryptocurrency', 'AAPL', 900),
                                                   (4, 'bitcoin', 'Cryptocurrency', 'BTC', 1), (5, 'BTC.US', 'Stock', 'BTC', NULL), (6, 'MSFT.US', 'Stock', 'MSFT', NULL);
                         CREATE TABLE historical_prices (id INTEGER PRIMARY KEY, asset TEXT, currency TEXT, date TEXT, price NUMERIC);
                         CREATE TABLE fx_rates (currency TEXT, date TEXT, rate NUMERIC);").unwrap();
        c
    }
    fn names(list: &[&str]) -> HashSet<String> { list.iter().map(|s| s.to_string()).collect() }

    /// Tax::AssetIdentity: a dated alias until its last day, a venue's alias on that venue only, then the catalogue (a
    /// stock first on a stock venue).
    #[test]
    fn a_symbol_means_the_coin_of_its_day_and_venue() {
        let c = Reference::load(&catalogue(), &venue("alpaca"), &names(&["LUNA", "AAPL"])).unwrap();
        assert_eq!(alias_coin("LUNA", Some(&venue("alpaca")), Some(day("2022-05-27"))).as_deref(), Some("terra-luna"));
        assert_eq!(alias_coin("LUNA", Some(&venue("alpaca")), Some(day("2022-05-28"))), None);
        assert_eq!((alias_coin("LIT", Some(&venue("binance")), None).as_deref(), alias_coin("LIT", Some(&venue("alpaca")), None)), (Some("litentry"), None));
        assert_eq!(alias_coin("LIT", Some(&venue("binance_us")), None), None, "Binance's own listing only");
        assert_eq!(coin_ids_over(&c, "LUNA", &venue("alpaca"), day("2022-05-01"), day("2022-06-30")),
                   [(day("2022-05-01"), day("2022-05-27"), "terra-luna".to_string()), (day("2022-05-28"), day("2022-06-30"), "terra-luna-2".to_string())]);
        assert_eq!(catalogue_coin(&c, "AAPL", &venue("alpaca")).as_deref(), Some("apple-coin"), "a coin lookup never answers a security");
        assert_eq!(catalogue_coin(&c, "AAPL", &venue("kraken")).as_deref(), Some("apple-coin"));
    }

    /// Tax::AssetIdentity.identity and .price_key: at a broker the recorded asset decides, then the venue's listings, then
    /// the catalogue, and a name that is both is unpriced; a security's key is `stock:SYM` whenever a coin has the symbol.
    #[test]
    fn a_broker_name_is_the_asset_recorded_and_a_security_keeps_off_a_coins_key() {
        let db = catalogue();
        db.execute_batch("INSERT INTO tickers VALUES (1, 1, 'BTC', 5), (2, 1, 'BTC', 4), (3, 1, 'MSFT', 6)").unwrap();
        let r = Reference::load(&db, &venue("alpaca"), &names(&["BTC", "MSFT", "AAPL", "LUNA"])).unwrap();
        let alpaca = venue("alpaca");
        let coin: Asset = (4, Some("bitcoin".into()), Some("Cryptocurrency".into()));
        let etf: Asset = (5, Some("BTC.US".into()), Some("Stock".into()));
        assert_eq!(identity(&r, "BTC", Some(&alpaca), &[]), None);
        assert_eq!(identity(&r, "BTC", Some(&alpaca), std::slice::from_ref(&coin)), Some((false, Some(coin.clone()))));
        assert_eq!(identity(&r, "BTC", Some(&venue("binance")), &[]), Some((false, None)));
        assert_eq!(price_key(&r, "BTC", Some(&alpaca), None, std::slice::from_ref(&etf), false), Some(("stock:BTC".to_string(), Some(etf.clone()))));
        assert_eq!(price_key(&r, "BTC", Some(&alpaca), None, std::slice::from_ref(&coin), false), Some(("BTC".to_string(), None)));
        assert_eq!(price_key(&r, "MSFT", Some(&alpaca), None, &[], false).map(|k| k.0).as_deref(), Some("MSFT"), "no coin is MSFT: the key it always had");
        assert_eq!(price_key(&r, "MSFT", Some(&alpaca), None, &[], true).map(|k| k.0).as_deref(), Some("stock:MSFT"), "a coin recorded under it elsewhere");
        assert_eq!(price_key(&r, "AAPL", Some(&alpaca), None, &[], false), None, "the catalogue has both and the venue lists neither");
        for at in ["2022-05-27", "2022-05-28"] { assert_eq!(price_key(&r, "LUNA", Some(&alpaca), Some(day(at)), &[], false).map(|k| k.0).as_deref(), Some("LUNA")); }
        assert!(dated_alias("LUNA", Some(&alpaca)) && !dated_alias("MATIC", Some(&alpaca)));
    }

    /// A broker listing only the BTC security still stores its closes off bitcoin's key, whichever is priced first; a
    /// row naming neither at a broker that lists both asks nothing and warns.
    #[test]
    fn a_security_and_bitcoin_never_share_a_key_across_venues() {
        let db = catalogue();
        db.execute_batch("INSERT INTO tickers VALUES (1, 1, 'BTC', 5)").unwrap();
        let r = Reference::load(&db, &venue("alpaca"), &names(&["BTC"])).unwrap();
        let fx = FxTable::default();
        let at = At::from_sql("2026-10-08 12:00:00").unwrap();
        for order in [["alpaca", "binance"], ["binance", "alpaca"]] {
            let mut script = Script { answers: vec![vec![(day("2026-10-08"), Dec::from_i64(36))], vec![(day("2026-10-08"), Dec::from_i64(83_000))]], asked: vec![] };
            if order[0] == "binance" { script.answers.reverse(); }
            {
                let mut ps = PriceService::new(&r, &fx, day("2026-10-15"), &mut script);
                for v in order {
                    let want = if v == "alpaca" { "36.0" } else { "83000.0" };
                    assert_eq!(ps.price_at("BTC", "USD", at, Some(v)).unwrap().to_s_f(), want, "{v} after {order:?}");
                }
            }
            let mut asked: Vec<(String, String)> = script.asked.iter().map(|f| (f.coin.clone(), f.symbol.clone())).collect();
            asked.sort();
            assert_eq!(asked, [("BTC.US".to_string(), "stock:BTC".to_string()), ("bitcoin".to_string(), "BTC".to_string())]);
        }
        db.execute_batch("INSERT INTO tickers VALUES (2, 1, 'BTC', 4)").unwrap();
        let r = Reference::load(&db, &venue("alpaca"), &names(&["BTC"])).unwrap();
        let mut none = NoFetch;
        let mut ps = PriceService::new(&r, &fx, day("2026-10-15"), &mut none);
        assert!(ps.price_at("BTC", "USD", at, Some("alpaca")).unwrap().is_zero());
        assert_eq!(ps.warnings, ["BTC/USD 2026-10-08"]);
    }

    /// fetch_price_range: nothing asked when the table covers the range; else one request from the first midnight to the
    /// one after the last, and only the days in range the table lacks, a zero and a null left out.
    #[tokio::test(flavor = "current_thread")]
    async fn a_range_is_asked_once_and_only_new_priced_days_are_kept() {
        let t = ScriptedTransport::from_script(&serde_json::json!({ "GET /api/v1/historical_prices": [{ "status": 200, "body": { "prices": [
            [1788134400000u64, 60100.5], [1788220800000u64, 0], [1788307200000u64, null], [1788393600000u64, 60300], [1788480000000u64, 60400], [1788566400000u64, 1] ] } }] }));
        let api = DataApi::new(Config { url: "http://data-api:3000".into(), token: "t".into() }, t.clone(), t.clone());
        let fetch = Fetch::usd("bitcoin".into(), "BTC".into(), day("2026-08-31"), day("2026-09-03"));
        let have: HashSet<NaiveDate> = [day("2026-09-03")].into();
        let rows = fetch_range(&api, &fetch, &have).await.unwrap();
        assert_eq!(rows, [("BTC".to_string(), day("2026-08-31"), "60100.5".to_string())]);
        let q: Vec<(String, String)> = t.requests()[0].query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        assert_eq!(q, [("coin_id", "bitcoin"), ("currency", "usd"), ("from", "1788134400"), ("to", "1788480000")].map(|(k, v)| (k.to_string(), v.to_string())));
        let all: HashSet<NaiveDate> = ["2026-08-31", "2026-09-01", "2026-09-02", "2026-09-03"].map(day).into();
        assert!(fetch_range(&api, &fetch, &all).await.unwrap().is_empty());
        assert_eq!(t.requests().len(), 1, "a covered range asks nothing");
    }

    /// Scripted fetches, recorded.
    struct Script { answers: Vec<Vec<(NaiveDate, Dec)>>, asked: Vec<Fetch> }
    impl Fetcher for Script {
        fn range(&mut self, f: &Fetch, _: &HashSet<NaiveDate>) -> Result<Vec<(NaiveDate, Dec)>, String> {
            self.asked.push(f.clone());
            Ok(if self.answers.is_empty() { vec![] } else { self.answers.remove(0) })
        }
    }

    /// Rails caches no failure: each lookup of a day nobody prices fetches again, and is a zero and a warning; a fetch
    /// that brings the day serves every later lookup without a request. A symbol no coin can be named for is a zero and a
    /// warning with no request.
    #[test]
    fn a_price_nobody_has_is_a_zero_and_a_warning_every_lookup() {
        let r = Reference::load(&catalogue(), &venue("alpaca"), &names(&["MSFT", "NOSUCH", "BTC"])).unwrap();
        let fx = FxTable::default();
        let at = At::from_sql("2026-09-09 12:00:00").unwrap();
        let mut script = Script { answers: vec![vec![], vec![(day("2026-09-09"), Dec::from_i64(200))]], asked: vec![] };
        {
            let mut ps = PriceService::new(&r, &fx, day("2026-10-01"), &mut script);
            assert!(ps.price_at("MSFT", "USD", at, Some("alpaca")).unwrap().is_zero());
            assert_eq!(ps.price_at("MSFT", "USD", at, Some("alpaca")).unwrap().to_s_f(), "200.0");
            assert_eq!(ps.price_at("MSFT", "USD", at, Some("alpaca")).unwrap().to_s_f(), "200.0");
            assert!(ps.price_at("NOSUCH", "USD", at, Some("alpaca")).unwrap().is_zero());
            assert_eq!(ps.warnings, ["MSFT/USD 2026-09-09", "NOSUCH/USD 2026-09-09"]);
        }
        assert_eq!(script.asked.len(), 2);
        assert_eq!((script.asked[0].coin.as_str(), script.asked[0].symbol.as_str(), script.asked[0].from, script.asked[0].to), ("MSFT.US", "MSFT", day("2026-06-03"), day("2026-10-01")));
    }

    /// TrackerHelper#tracker_price_key: the table reads a row's price under the key its identity names, with what the
    /// owner's rows at the venue recorded standing in for a row that recorded nothing; none for a name still both.
    #[test]
    fn the_table_reads_a_row_s_price_under_its_identity_s_key() {
        let c = catalogue();
        c.execute_batch("INSERT INTO tickers VALUES (1, 1, 'BTC', 5), (2, 1, 'BTC', 4), (3, 1, 'MSFT', 6);
                         CREATE TABLE account_transactions (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, base_currency TEXT, base_asset_id INTEGER);
                         INSERT INTO account_transactions VALUES (1, 2, 1, 'BTC', 5);").unwrap();
        let key = |owner: i64, exchange: i64, kind: &str, symbol: &str, own: Option<i64>| row_price_key(&c, owner, exchange, kind, symbol, own, day("2026-10-08")).unwrap();
        assert_eq!(key(1, 1, "Exchanges::Alpaca", "BTC", None), None, "nothing says which");
        assert_eq!(key(1, 1, "Exchanges::Alpaca", "BTC", Some(4)).as_deref(), Some("BTC"));
        assert_eq!(key(1, 1, "Exchanges::Alpaca", "MSFT", None).as_deref(), Some("MSFT"), "a name no coin shares: as before");
        assert_eq!(key(2, 1, "Exchanges::Alpaca", "BTC", None).as_deref(), Some("stock:BTC"), "the venue's other rows recorded the security");
        assert_eq!(key(2, 2, "Exchanges::Binance", "BTC", None).as_deref(), Some("BTC"));
    }

    /// A missing ECB rate warns on every record that leans on it and falls back to the BTC cross.
    #[test]
    fn a_missing_rate_falls_back_to_the_btc_cross_and_warns_each_time() {
        let c = catalogue();
        c.execute_batch("INSERT INTO historical_prices (asset, currency, date, price) VALUES ('BTC', 'EUR', '2026-09-09', '50000'), ('BTC', 'USD', '2026-09-09', '60000')").unwrap();
        let r = Reference::load(&c, &venue("alpaca"), &names(&["BTC"])).unwrap();
        let fx = FxTable::load(&c).unwrap();
        let at = At::from_sql("2026-09-09 12:00:00").unwrap();
        let mut none = NoFetch;
        let mut ps = PriceService::new(&r, &fx, day("2026-10-01"), &mut none);
        assert_eq!(ps.convert_fiat(&Dec::from_i64(10), "EUR", "USD", at).unwrap().to_s_f(), "12.0");
        assert_eq!(ps.convert_fiat(&Dec::from_i64(10), "EUR", "USD", at).unwrap().to_s_f(), "12.0");
        assert_eq!(ps.warnings, ["FX EUR/USD 2026-09-09", "FX EUR/USD 2026-09-09"]);
    }
}
