//! Tax::PriceService#price_at and Tax::AssetIdentity, for the walk: the USD price of a symbol on a day, read from
//! `historical_prices`, and the ranges Rails fetches from data-api when the table lacks it (`prefetch`,
//! `fetch_single_price`). The walk is synchronous and reads the database only; a price Rails would fetch first halts
//! it (`Halt::Fetch`), the caller fetches (`fetch_range`) and walks again. A day whose fetch came back without its
//! price is asked again, as Rails asks on every lookup; one still missing after that refuses the walk rather than
//! valuing anything at zero.
use super::rows::Stored;
use super::{fiat, stable};
use crate::figures::{dec::Dec, FiguresError};
use crate::jobs::data_api::{ApiError, DataApi};
use crate::venue::http::Transport;
use chrono::{Days, NaiveDate};
use rusqlite::{Connection, OptionalExtension};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Tax::PriceService::SINGLE_PRICE_WINDOW: half the window asked for around one day.
pub const SINGLE_PRICE_WINDOW: u64 = 60;
/// How many fetches one day's price gets. Rails caches no failure: `price_at` fetches again on every lookup, and a
/// walk looks each opening up twice, account-wide and then located (Tracker::Ledger#walk, ledger.rb:595-603).
pub const LOOKUPS: u32 = 2;

/// One range of daily prices Rails fetches (Tax::PriceService#fetch_price_range), and the day that asked for it when
/// it is one price's fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetch { pub coin: String, pub symbol: String, pub from: NaiveDate, pub to: NaiveDate, pub asked: Option<NaiveDate> }

/// Why a walk stopped before its end.
#[derive(Debug)]
pub enum Halt { Fetch(Fetch), Fail(FiguresError) }
impl<E: Into<FiguresError>> From<E> for Halt { fn from(e: E) -> Self { Halt::Fail(e.into()) } }

/// The venue a symbol is read on: its `name_id`, and whether it is a stock venue (Exchange#stock_venue?).
#[derive(Clone, Debug)]
pub struct Venue { pub id: i64, pub name_id: String, pub stock: bool }

impl Venue {
    pub fn alpaca(c: &Connection) -> Result<Venue, FiguresError> {
        let id: Option<i64> = c.query_row("SELECT id FROM exchanges WHERE type = ?1 ORDER BY id LIMIT 1", [super::VENUE_TYPE], |r| r.get(0)).optional()?;
        Ok(Venue { id: id.unwrap_or(0), name_id: super::VENUE.into(), stock: true })
    }
}

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
fn fits(alias_venue: Option<&str>, venue: &Venue) -> bool { alias_venue.is_none_or(|v| v == venue.name_id) }

/// Tax::AssetIdentity.alias_coin.
pub fn alias_coin(symbol: &str, venue: &Venue, at: Option<NaiveDate>) -> Option<String> {
    if symbol.trim().is_empty() { return None; }
    ALIASES.iter()
        .find(|(s, _, v, until)| *s == symbol && fits(*v, venue) && until.is_none_or(|u| at.is_some_and(|at| at <= ymd(u))))
        .map(|(_, coin, _, _)| coin.to_string())
}

/// One catalogue asset: its external id and category.
type Asset = (Option<String>, Option<String>);

/// What a pass reads of the catalogue and the stored prices, for the symbols it names: each table read in one query,
/// every row read a step of the open budget scope, and indexed in memory, so no lookup issues SQL.
#[derive(Default)]
pub struct Reference {
    /// symbol → what the venue lists under it: its first ticker's base asset (external id, category).
    listed: HashMap<String, Asset>,
    /// symbol → every asset of the catalogue under it (external id, category), by market rank.
    assets: HashMap<String, Vec<Asset>>,
    /// price key (the symbol, or `stock:SYM`) → day → the stored USD price, as Rails reads the column.
    prices: HashMap<String, BTreeMap<NaiveDate, Dec>>,
}

impl Reference {
    /// The catalogue and the prices of `symbols` (and of `stock:<symbol>`) on `venue`.
    pub fn load(c: &Connection, venue: &Venue, symbols: &HashSet<String>) -> Result<Reference, FiguresError> {
        let mut r = Reference::default();
        let mut s = c.prepare("SELECT t.base, a.external_id, a.category FROM tickers t JOIN assets a ON a.id = t.base_asset_id WHERE t.exchange_id = ?1 ORDER BY t.id")?;
        let mut q = s.query([venue.id])?;
        while let Some(row) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let base: String = row.get(0)?;
            if symbols.contains(&base) && !r.listed.contains_key(&base) { r.listed.insert(base, (row.get(1)?, row.get(2)?)); }
        }
        let mut s = c.prepare("SELECT symbol, external_id, category FROM assets ORDER BY market_cap_rank IS NULL, market_cap_rank, id")?;
        let mut q = s.query([])?;
        while let Some(row) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let Some(symbol) = row.get::<_, Option<String>>(0)? else { continue };
            if symbols.contains(&symbol) { r.assets.entry(symbol).or_default().push((row.get(1)?, row.get(2)?)); }
        }
        let mut s = c.prepare("SELECT asset, date, price FROM historical_prices WHERE currency = 'USD'")?;
        let mut q = s.query([])?;
        while let Some(row) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let asset: String = row.get(0)?;
            if !(symbols.contains(&asset) || asset.strip_prefix("stock:").is_some_and(|sym| symbols.contains(sym))) { continue; }
            let (Ok(day), Some(price)) = (NaiveDate::parse_from_str(&row.get::<_, String>(1)?, "%Y-%m-%d"), Dec::from_sql(row.get_ref(2)?)?) else { continue };
            r.prices.entry(asset).or_default().insert(day, price);
        }
        Ok(r)
    }

    /// `HistoricalPrice.lookup`: the stored price of `asset` on `date`. A zero is no price.
    pub fn price(&self, asset: &str, date: NaiveDate) -> Option<&Dec> { self.prices.get(asset)?.get(&date).filter(|p| !p.is_zero()) }

    /// The stored prices of `asset` over `from..=to`, by day; none when `from` is after `to` (a symbol first traded
    /// today, swept to yesterday), as Rails' `where(date: from..to)` selects none. `BTreeMap::range` panics on a
    /// reversed range, so it is never called with one.
    pub fn prices_over(&self, asset: &str, from: NaiveDate, to: NaiveDate) -> impl Iterator<Item = (&NaiveDate, &Dec)> {
        self.prices.get(asset).filter(|_| from <= to).into_iter().flat_map(move |days| days.range(from..=to))
    }

    /// The category of the asset the venue lists under `symbol`: None when it lists nothing, Some(None) uncategorised.
    pub fn listed_category(&self, symbol: &str) -> Option<Option<&str>> { self.listed.get(symbol).map(|(_, cat)| cat.as_deref()) }
}

/// Every symbol the rows name, base and quote.
pub fn symbols(rows: &[Stored]) -> HashSet<String> {
    rows.iter().flat_map(|r| std::iter::once(r.base.clone()).chain(r.quote.clone())).collect()
}

/// Tax::AssetIdentity.catalogue_coin: what the venue lists under the symbol (its first ticker's base asset; on a stock
/// venue whatever it is), else the catalogue by market rank, a stock first on a stock venue, else a coin.
pub fn catalogue_coin(r: &Reference, symbol: &str, venue: &Venue) -> Option<String> {
    if symbol.trim().is_empty() { return None; }
    if let Some((external_id, category)) = r.listed.get(symbol) {
        if venue.stock || category.as_deref() == Some("Cryptocurrency") { return external_id.clone(); }
    }
    let assets = r.assets.get(symbol).map_or(&[][..], Vec::as_slice);
    let stock = if venue.stock { assets.iter().find(|(_, cat)| cat.as_deref() == Some("Stock")) } else { None };
    stock.or_else(|| assets.iter().find(|(_, cat)| cat.as_deref() == Some("Cryptocurrency"))).and_then(|(id, _)| id.clone())
}

/// Tax::AssetIdentity.coin_id.
pub fn coin_id(r: &Reference, symbol: &str, venue: &Venue, at: Option<NaiveDate>) -> Option<String> {
    alias_coin(symbol, venue, at).or_else(|| catalogue_coin(r, symbol, venue))
}

/// Tax::AssetIdentity.coin_ids_over: the coin a symbol means over each stretch of `from..=to`, cut where a dated alias
/// ends.
pub fn coin_ids_over(r: &Reference, symbol: &str, venue: &Venue, from: NaiveDate, to: NaiveDate) -> Vec<(NaiveDate, NaiveDate, String)> {
    let mut cuts: Vec<NaiveDate> = ALIASES.iter().filter(|(s, _, v, until)| *s == symbol && until.is_some() && fits(*v, venue))
        .filter_map(|(_, _, _, until)| until.map(ymd)).filter(|cut| *cut >= from && *cut < to).collect();
    cuts.sort();
    let starts: Vec<NaiveDate> = std::iter::once(from).chain(cuts.iter().map(|cut| cut.succ_opt().unwrap_or(*cut))).collect();
    let ends: Vec<NaiveDate> = cuts.iter().copied().chain(std::iter::once(to)).collect();
    starts.into_iter().zip(ends).filter_map(|(first, last)| coin_id(r, symbol, venue, Some(first)).map(|coin| (first, last, coin))).collect()
}

fn plus(d: NaiveDate, n: u64) -> NaiveDate { d.checked_add_days(Days::new(n)).unwrap_or(d) }
fn minus(d: NaiveDate, n: u64) -> NaiveDate { d.checked_sub_days(Days::new(n)).unwrap_or(d) }

/// Tax::PriceService for one walk: its warnings (Rails' count of prices it valued at zero; none here, where a missing
/// price refuses the walk), and how often each day's single fetch already ran in this job's earlier passes.
pub struct PriceBook<'a> {
    pub r: &'a Reference,
    pub venue: Venue,
    pub today: NaiveDate,
    /// (asset, day) → the single fetches that already ran for it and left it unpriced.
    pub fetched: &'a HashMap<(String, NaiveDate), u32>,
    pub warnings: usize,
}

/// Why a walk is refused when a price that could be fetched still is not there.
pub fn unpriced(asset: &str, date: NaiveDate) -> FiguresError {
    FiguresError::NotComputed(format!("no price of {asset} on {date} after {LOOKUPS} fetches: the tracker states no figure on a zero basis, and walks again on its next run"))
}

impl PriceBook<'_> {
    /// `price_at(asset:, currency: 'USD', timestamp:, exchange:)`: the stored price. A day Rails would fetch for halts
    /// the walk, `LOOKUPS` times; still unpriced after that, the walk is refused (Rails would value it at zero). So is
    /// a symbol no coin can be named for, which nothing can fetch (Rails' price nobody has, at zero).
    pub fn usd(&mut self, asset: &str, date: NaiveDate) -> Result<Dec, Halt> {
        if asset == "USD" || stable(asset) { return Ok(Dec::one()); }
        crate::figures::budget::charge(1, 0)?;
        if let Some(price) = self.r.price(asset, date) { return Ok(price.clone()); }
        if let Some(fetch) = self.single(asset, date) {
            if self.fetched.get(&(asset.to_string(), date)).copied().unwrap_or(0) < LOOKUPS { return Err(Halt::Fetch(fetch)); }
            return Err(Halt::Fail(unpriced(asset, date)));
        }
        Err(Halt::Fail(FiguresError::NotComputed(format!("no price of {asset} on {date}: no coin can be named for it, so none can be fetched; \
                                                          the tracker states no figure on a zero basis"))))
    }

    /// The same, its warning taken back (an opening balance's lookup: a day with no price is the asset's own gap).
    pub fn usd_quiet(&mut self, asset: &str, date: NaiveDate) -> Result<Dec, Halt> {
        let kept = self.warnings;
        let price = self.usd(asset, date);
        self.warnings = kept;
        price
    }

    /// fetch_single_price's window: up to 60 days either side, anchored on its end (never past today), clipped to the
    /// coin the symbol meant that day.
    fn single(&self, asset: &str, date: NaiveDate) -> Option<Fetch> {
        let to = plus(date, SINGLE_PRICE_WINDOW).min(self.today);
        let from = minus(to, SINGLE_PRICE_WINDOW * 2);
        let (first, last, coin) = coin_ids_over(self.r, asset, &self.venue, from, to).into_iter().find(|(f, l, _)| *f <= date && date <= *l)?;
        let end = if last == to { plus(first, SINGLE_PRICE_WINDOW * 2).min(self.today) } else { last };
        Some(Fetch { coin, symbol: asset.into(), from: first, to: end, asked: Some(date) })
    }
}

/// Tax::PriceService#prefetch for USD: one range per (symbol, coin) over the days of the rows that need a price of
/// ours, in the order the rows first name them. A row valued by its own quote in cash, or by a price the user stated
/// (and no cash quote), needs none; neither does a fiat symbol or one no coin can be named for. Each row is a step
/// of the open budget scope; ranges are found through an index.
pub fn prefetch(reference: &Reference, rows: &[Stored], venue: &Venue) -> Result<Vec<Fetch>, FiguresError> {
    let mut coins: Vec<Fetch> = vec![];
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    for r in rows {
        crate::figures::budget::charge(1, 0)?;
        if r.stated.is_some() && !r.quoted() { continue; }
        if r.quote.as_deref() == Some("USD") && r.quote_amount.is_some() { continue; }
        if r.quote.as_deref().is_some_and(|q| stable(q) || fiat(q)) && r.quote_amount.is_some() { continue; }
        if r.base.trim().is_empty() || fiat(&r.base) { continue; }
        let date = r.date();
        let Some(coin) = alias_coin(&r.base, venue, Some(date)).or_else(|| catalogue_coin(reference, &r.base, venue)) else { continue };
        match index.get(&(r.base.clone(), coin.clone())) {
            Some(&i) => { let f = &mut coins[i]; f.from = f.from.min(date); f.to = f.to.max(date); }
            None => {
                index.insert((r.base.clone(), coin.clone()), coins.len());
                coins.push(Fetch { coin, symbol: r.base.clone(), from: date, to: date, asked: None });
            }
        }
    }
    Ok(coins)
}

/// The days of `from..=to` that already have a stored price of `asset`.
pub fn stored_days(c: &Connection, asset: &str, from: NaiveDate, to: NaiveDate) -> Result<HashSet<NaiveDate>, FiguresError> {
    let mut s = c.prepare("SELECT date FROM historical_prices WHERE asset = ?1 AND currency = 'USD' AND date >= ?2 AND date <= ?3")?;
    let days = s.query_map(rusqlite::params![asset, from.to_string(), to.to_string()], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(days.iter().filter_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).collect())
}

/// Whether the table already has every day of a fetch's range (`fetch_price_range` then asks nothing).
pub fn covered(fetch: &Fetch, have: &HashSet<NaiveDate>) -> bool {
    (0..span(fetch.from, fetch.to)).all(|i| have.contains(&plus(fetch.from, i as u64)))
}

/// How many days a range spans, inclusive.
pub fn span(from: NaiveDate, to: NaiveDate) -> i64 { (to - from).num_days() + 1 }

/// What `fetch_range` stores: (asset, date, price as Rails writes it).
pub type PriceRow = (String, NaiveDate, String);

/// Tax::PriceService#fetch_price_range: nothing when the table covers every day; else data-api's
/// `GET /api/v1/historical_prices` from the first day's midnight to the midnight after the last (UTC), and the days in
/// range it priced that the table lacks, a zero or null left out. A failed answer stores nothing; a transport failure
/// a retry could fix is an error (Rails raises it out of the job).
pub async fn fetch_range<T: Transport>(api: &DataApi<T>, fetch: &Fetch, have: &HashSet<NaiveDate>) -> Result<Vec<PriceRow>, String> {
    if covered(fetch, have) { return Ok(vec![]); }
    let midnight = |d: NaiveDate| d.and_hms_opt(0, 0, 0).map_or(0, |t| t.and_utc().timestamp());
    let (from, to) = (midnight(fetch.from).to_string(), midnight(plus(fetch.to, 1)).to_string());
    let body = match api.get("/api/v1/historical_prices", &[("coin_id", &fetch.coin), ("currency", "usd"), ("from", &from), ("to", &to)], false).await {
        Ok(body) => body,
        Err(ApiError::Transient(m)) => return Err(m),
        Err(ApiError::Failed { .. }) => return Ok(vec![]),
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

/// HistoricalPrice.bulk_store: insert-only.
pub fn store(c: &Connection, rows: &[PriceRow]) -> Result<(), FiguresError> {
    let mut s = c.prepare_cached("INSERT INTO historical_prices (asset, currency, date, price) VALUES (?1, 'USD', ?2, ?3) ON CONFLICT (asset, currency, date) DO NOTHING")?;
    for (asset, date, price) in rows { s.execute(rusqlite::params![asset, date.to_string(), price])?; }
    Ok(())
}

/// Authenticated bar results require Q's capability; public market-data writes use store.
pub fn store_bars(c:&crate::engine::model::FencedTransaction<'_>,rows:&[PriceRow])->Result<(),FiguresError>{store(c,rows)}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::data_api::Config;
    use crate::venue::http::ScriptedTransport;

    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }
    fn venue(name: &str) -> Venue { Venue { id: 1, name_id: name.into(), stock: name == "alpaca" } }

    fn catalogue() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, base TEXT, base_asset_id INTEGER);
                         CREATE TABLE assets (id INTEGER PRIMARY KEY, external_id TEXT, category TEXT, symbol TEXT, market_cap_rank INTEGER);
                         INSERT INTO assets VALUES (1, 'terra-luna-2', 'Cryptocurrency', 'LUNA', 300), (2, 'AAPL.US', 'Stock', 'AAPL', NULL), (3, 'apple-coin', 'Cryptocurrency', 'AAPL', 900);
                         CREATE TABLE historical_prices (asset TEXT, currency TEXT, date TEXT, price NUMERIC);").unwrap();
        c
    }
    fn names(list: &[&str]) -> HashSet<String> { list.iter().map(|s| s.to_string()).collect() }

    /// Tax::AssetIdentity: a dated alias until its last day, a venue's alias on that venue only, then the catalogue (a
    /// stock first on a stock venue).
    #[test]
    fn a_symbol_means_the_coin_of_its_day_and_venue() {
        let c = Reference::load(&catalogue(), &venue("alpaca"), &names(&["LUNA", "AAPL"])).unwrap();
        assert_eq!(alias_coin("LUNA", &venue("alpaca"), Some(day("2022-05-27"))).as_deref(), Some("terra-luna"));
        assert_eq!(alias_coin("LUNA", &venue("alpaca"), Some(day("2022-05-28"))), None);
        assert_eq!((alias_coin("LIT", &venue("binance"), None).as_deref(), alias_coin("LIT", &venue("alpaca"), None)), (Some("litentry"), None));
        assert_eq!(coin_ids_over(&c, "LUNA", &venue("alpaca"), day("2022-05-01"), day("2022-06-30")),
                   [(day("2022-05-01"), day("2022-05-27"), "terra-luna".to_string()), (day("2022-05-28"), day("2022-06-30"), "terra-luna-2".to_string())]);
        assert_eq!(catalogue_coin(&c, "AAPL", &venue("alpaca")).as_deref(), Some("AAPL.US"));
        assert_eq!(catalogue_coin(&c, "AAPL", &venue("kraken")).as_deref(), Some("apple-coin"));
    }

    /// fetch_price_range: nothing asked when the table covers the range; else one request from the first midnight to the
    /// one after the last, and only the days in range the table lacks, a zero and a null left out.
    #[tokio::test(flavor = "current_thread")]
    async fn a_range_is_asked_once_and_only_new_priced_days_are_kept() {
        let t = ScriptedTransport::from_script(&serde_json::json!({ "GET /api/v1/historical_prices": [{ "status": 200, "body": { "prices": [
            [1788134400000u64, 60100.5], [1788220800000u64, 0], [1788307200000u64, null], [1788393600000u64, 60300], [1788480000000u64, 60400], [1788566400000u64, 1] ] } }] }));
        let api = DataApi::new(Config { url: "http://data-api:3000".into(), token: "t".into() }, t.clone(), t.clone());
        let fetch = Fetch { coin: "bitcoin".into(), symbol: "BTC".into(), from: day("2026-08-31"), to: day("2026-09-03"), asked: None };
        let have: HashSet<NaiveDate> = [day("2026-09-03")].into();
        let rows = fetch_range(&api, &fetch, &have).await.unwrap();
        // 31 August priced; 1 September zero; 2 September null; 3 September already stored; 4 and 5 September out of range.
        assert_eq!(rows, [("BTC".to_string(), day("2026-08-31"), "60100.5".to_string())]);
        let q: Vec<(String, String)> = t.requests()[0].query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        assert_eq!(q, [("coin_id", "bitcoin"), ("currency", "usd"), ("from", "1788134400"), ("to", "1788480000")].map(|(k, v)| (k.to_string(), v.to_string())));
        let all: HashSet<NaiveDate> = ["2026-08-31", "2026-09-01", "2026-09-02", "2026-09-03"].map(day).into();
        assert!(fetch_range(&api, &fetch, &all).await.unwrap().is_empty());
        assert_eq!(t.requests().len(), 1, "a covered range asks nothing");
    }

    /// Rails asks again for a day a fetch left unpriced (it caches no failure), so a price that arrives on the second
    /// fetch is used; a day still unpriced after `LOOKUPS` fetches refuses the walk, never a zero basis, and so does a
    /// symbol no coin can be named for (Rails' price nobody has).
    #[test]
    fn a_failed_fetch_is_asked_again_and_then_refused() {
        let c = Reference::load(&catalogue(), &venue("alpaca"), &names(&["AAPL", "NOSUCH"])).unwrap();
        let mut fetched = HashMap::new();
        let asks = |fetched: &HashMap<(String, NaiveDate), u32>, asset: &str| {
            let mut book = PriceBook { r: &c, venue: venue("alpaca"), today: day("2026-10-01"), fetched, warnings: 0 };
            (book.usd(asset, day("2026-09-09")), book.warnings)
        };
        for n in 0..LOOKUPS {
            fetched.insert(("AAPL".to_string(), day("2026-09-09")), n);
            assert!(matches!(asks(&fetched, "AAPL").0, Err(Halt::Fetch(Fetch { ref coin, asked: Some(d), .. })) if coin == "AAPL.US" && d == day("2026-09-09")), "fetch {}", n + 1);
        }
        fetched.insert(("AAPL".to_string(), day("2026-09-09")), LOOKUPS);
        assert!(matches!(asks(&fetched, "AAPL").0, Err(Halt::Fail(FiguresError::NotComputed(ref m))) if m.starts_with("no price of AAPL on 2026-09-09 after 2 fetches")));
        let (price, warnings) = asks(&fetched, "NOSUCH");
        assert!(matches!(price, Err(Halt::Fail(FiguresError::NotComputed(ref m))) if m.starts_with("no price of NOSUCH on 2026-09-09: no coin can be named")));
        assert_eq!(warnings, 0);
    }

    /// Many distinct symbols are read from the catalogue in one query and prefetched in linear work, each row a step:
    /// 5,000 coins well inside a figure's limits, and refused under an allowance they exceed.
    #[test]
    fn many_symbols_prefetch_in_linear_work_within_the_allowance() {
        let c = catalogue();
        c.execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000)
                         INSERT INTO assets (external_id, category, symbol) SELECT 'coin-' || i, 'Cryptocurrency', 'C' || i FROM n").unwrap();
        let at = crate::figures::at::At::from_sql("2026-09-02 14:30:00").unwrap();
        let rows: Vec<Stored> = (1..=5000).flat_map(|i| [0, 1].map(move |k| (i, k))).map(|(i, k)| Stored {
            id: i * 2 + k, exchange_id: 1, kind: super::super::rows::Kind::Airdrop, base: format!("C{i}"), amount: Dec::one(), quote: None, quote_amount: None,
            tx_id: None, group: None, at, per_share: None, stated: None, linked_to: None }).collect();
        let catalogue = |rows: &[Stored]| Reference::load(&c, &venue("kraken"), &symbols(rows));
        let (out, used) = crate::figures::budget::scope(crate::figures::budget::FIGURE, || prefetch(&catalogue(&rows)?, &rows, &venue("kraken")));
        assert_eq!(out.unwrap().len(), 5000);
        assert!(used.steps >= 10_000 && used.steps < 1_000_000, "{} steps", used.steps);
        let small = crate::figures::budget::Limits { steps: 1_000, held: crate::figures::budget::FIGURE.held };
        let (out, _) = crate::figures::budget::scope(small, || prefetch(&catalogue(&rows)?, &rows, &venue("kraken")));
        assert!(matches!(out, Err(FiguresError::NotComputed(ref m)) if m == crate::figures::OVER_BUDGET));
    }
}
