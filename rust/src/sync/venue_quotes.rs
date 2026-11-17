//! The venue's own last prices as the market-data provider's fallback in a balance sync (Rust only,
//! PLAN-venue-prices S2, D-VENUE-PRICE-FALLBACK). The bot pages' reader (`figures::page_market`) asks the venue's
//! public market data; `venue_routes` walks the priced pairs to a fiat end, the ECB's rate takes it to dollars.
//!
//! Never fails a sync: whatever cannot be read, routed or kept within a venue number's caps in time is left out, and the
//! row goes stale or unpriced exactly as when the provider has no price.
use super::venue_routes::{self, End, Pair};
use crate::crypto::Credentials;
use crate::figures::{market::{MarketData, Venue}, page_market::{Cache, Reader}};
use crate::jobs::Db;
use crate::v2::BigDec;
use crate::venue::http::{HttpRequest, Transport};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// The venue's reads, and the ECB's load before them: together well inside the balance job's deadline, with room for
/// the writes. What either leaves out stays unpriced; what was priced before it ran out is kept.
pub const BUDGET: Duration = Duration::from_secs(45);
pub const ECB_BUDGET: Duration = Duration::from_secs(15);
/// Requests one pass sends at most (Kraken's one-code reads past its bulk Ticker are the only venue needing many).
const MAX_SINGLE_READS: usize = 32;
const KRAKEN: &str = "Exchanges::Kraken";
const ALPACA: &str = "Exchanges::Alpaca";
const IBKR: &str = "Exchanges::Ibkr";

/// One holding the provider left unpriced: its external id, asset id, whether it is cash, and how much is held.
#[derive(Clone, Debug)]
pub struct Wanted { pub external_id: String, pub asset: i64, pub cash: bool, pub total: BigDec }

pub type QuotesFuture<'a> = Pin<Box<dyn Future<Output = HashMap<String, BigDec>> + 'a>>;

/// Dollar prices by external id for what the venue can price; absent what it cannot.
pub trait VenueQuotes {
    fn prices<'a>(&'a self, db: &'a Db, key_id: i64, exchange_id: i64, wanted: Vec<Wanted>, now: DateTime<Utc>) -> QuotesFuture<'a>;
    /// The days of `segments` not in `have`, from the venue's daily candles (S3, `venue_history`); none by default.
    fn history<'a>(&'a self, _db: &'a Db, _exchange_id: i64, _api_key_id: Option<i64>, _segments: Vec<super::venue_history::Segment>,
                   _have: HashSet<chrono::NaiveDate>, _deadline: tokio::time::Instant) -> Pin<Box<dyn Future<Output = super::venue_history::Days> + 'a>> {
        Box::pin(async { super::venue_history::Days::default() })
    }
}

/// Each venue's public, proxy-aware transport, as the figures build them (`venue_egresses`).
pub type Egresses = Vec<(&'static str, Result<crate::venue::http::ReqwestTransport, String>)>;
/// The wire a sync's quotes go out on: the venue's public market data, and the account's key for Alpaca's.
pub type MakeWire<T> = Box<dyn Fn(Option<&Credentials>, Egresses) -> T>;

pub struct Quotes<T: Transport> { make: MakeWire<T>, ecb: Option<crate::tracker::fx::Source> }

impl<T: Transport> Quotes<T> {
    pub fn new(make: MakeWire<T>, ecb: Option<crate::tracker::fx::Source>) -> Self { Self { make, ecb } }
}

impl Quotes<crate::web::figure::loading::Wire> {
    /// The live quotes: the figures' wire over every venue's egress (its proxy), the ECB from the network.
    pub fn live() -> Self {
        use crate::web::figure::loading::{Source, Wire};
        Self::new(Box::new(|credentials, venues| Wire::of(credentials, Source::Live).with_venues(venues)),
                  Some(crate::tracker::fx::Source::Live(crate::tracker::fx::Plain::default())))
    }
}

/// What one sync reads from the database before asking the venue.
struct Plan<T> { venue: Venue, pairs: Vec<Pair>, ends: HashMap<i64, End>, codes: Vec<String>, wire: T }
struct Read { venue: Venue, pairs: Vec<Pair>, ends: HashMap<i64, End>, codes: Vec<String>, credentials: Option<Credentials>, venues: Egresses }

fn log(exchange_id: i64, why: &str) { crate::engine::log(&format!("[sync] venue prices for exchange {exchange_id}: {why}")); }

fn big(d: &crate::figures::dec::Dec) -> Option<BigDec> { BigDec::parse(&d.to_s_f()).ok() }

/// The pairs within `MAX_LEGS` of the wanted assets, whatever their prices, nearest first: the codes to ask for. A route
/// stops at the first fiat it reaches, so the search never goes on from one (`ends`).
fn codes_near(pairs: &[Pair], from: &[i64], ends: &HashSet<i64>) -> Vec<String> {
    let mut reached: HashSet<i64> = from.iter().copied().collect();
    let mut codes: Vec<String> = vec![];
    let mut taken: HashSet<i64> = HashSet::new();
    let mut frontier: Vec<i64> = from.to_vec();
    for _ in 0..venue_routes::MAX_LEGS {
        let mut next = vec![];
        for p in pairs.iter().filter(|p| frontier.contains(&p.base) || frontier.contains(&p.quote)) {
            if taken.insert(p.id) { codes.push(p.code.clone()); }
            for a in [p.base, p.quote] { if reached.insert(a) && !ends.contains(&a) { next.push(a); } }
        }
        frontier = next;
    }
    codes
}

impl<T: Transport + 'static> VenueQuotes for Quotes<T> {
    fn history<'a>(&'a self, db: &'a Db, exchange_id: i64, api_key_id: Option<i64>, segments: Vec<super::venue_history::Segment>,
                   have: HashSet<chrono::NaiveDate>, deadline: tokio::time::Instant) -> Pin<Box<dyn Future<Output = super::venue_history::Days> + 'a>> {
        Box::pin(super::venue_history::history(&self.make, db, exchange_id, api_key_id, segments, have, deadline))
    }
    fn prices<'a>(&'a self, db: &'a Db, key_id: i64, exchange_id: i64, wanted: Vec<Wanted>, now: DateTime<Utc>) -> QuotesFuture<'a> {
        Box::pin(async move {
            self.run(db, key_id, exchange_id, wanted, now).await
        })
    }
}

impl<T: Transport + 'static> Quotes<T> {
    async fn run(&self, db: &Db, key_id: i64, exchange_id: i64, wanted: Vec<Wanted>, now: DateTime<Utc>) -> HashMap<String, BigDec> {
        let today = now.date_naive();
        if tokio::time::timeout(ECB_BUDGET, crate::tracker::fx::ensure_loaded(db, self.ecb.as_ref(), today)).await.is_err() { log(exchange_id, "the ECB's rates out of time"); }
        let assets: Vec<i64> = wanted.iter().map(|w| w.asset).collect();
        let plan = db.run(move |c, cipher| {
            let exchange_type: String = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [exchange_id], |r| r.get(0)).map_err(|e| e.to_string())?;
            let mut s = c.prepare("SELECT id, ticker, base_asset_id, quote_asset_id FROM tickers WHERE exchange_id = ?1 AND available = 1 \
                                   AND base_asset_id IS NOT NULL AND quote_asset_id IS NOT NULL ORDER BY id").map_err(|e| e.to_string())?;
            let pairs: Vec<Pair> = s.query_map([exchange_id], |r| Ok(Pair { id: r.get(0)?, code: r.get(1)?, base: r.get(2)?, quote: r.get(3)? }))
                .map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
            // Alpaca's market data is asked for its crypto only (its stocks have their snapshots), with the account's key.
            let pairs: Vec<Pair> = if exchange_type == ALPACA { pairs.into_iter().filter(|p| p.code.contains('/')).collect() } else { pairs };
            let fx = crate::figures::budget::within(|| crate::tracker::fx::FxTable::load(c)).map_err(|e| format!("{e:?}"))?;
            let mut ends = HashMap::new();
            let ids: HashSet<i64> = pairs.iter().flat_map(|p| [p.base, p.quote]).chain(assets.iter().copied()).collect();
            let mut s = c.prepare("SELECT id, symbol FROM assets WHERE category IN ('Fiat','Currency')").map_err(|e| e.to_string())?;
            for row in s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))).map_err(|e| e.to_string())? {
                let (id, symbol) = row.map_err(|e| e.to_string())?;
                let Some(symbol) = symbol.map(|s| s.to_uppercase()).filter(|_| ids.contains(&id)) else { continue };
                if symbol == "USD" { ends.insert(id, End { usd: true, rate: BigDec::one() }); continue; }
                let rate = crate::figures::budget::within(|| fx.rate(&symbol, "USD", today)).ok().flatten().and_then(|r| big(&r)).filter(BigDec::is_positive);
                if let Some(rate) = rate { ends.insert(id, End { usd: false, rate }); }
            }
            let credentials = if exchange_type == ALPACA { Some(super::credentials(c, cipher, key_id).map_err(|e| e.0)?) } else { None };
            let venues = crate::web::figure::loading::venue_egresses(c, cipher);
            let codes = codes_near(&pairs, &assets, &ends.keys().copied().collect());
            Ok(Read { venue: Venue { exchange_id, exchange_type }, pairs, ends, codes, credentials, venues })
        }).await;
        let plan = match plan {
            Ok(r) => Plan { wire: (self.make)(r.credentials.as_ref(), r.venues), venue: r.venue, pairs: r.pairs, ends: r.ends, codes: r.codes },
            Err(why) => { log(exchange_id, &why); return HashMap::new() }
        };
        let mut found: HashMap<String, BigDec> = HashMap::new();
        // Cash with an ECB rate needs no venue: its rate is its price.
        for w in wanted.iter().filter(|w| w.cash) {
            if let Some(end) = plan.ends.get(&w.asset).filter(|end| super::balances::fits(&w.total, &end.rate)) { found.insert(w.external_id.clone(), end.rate.clone()); }
        }
        let routed: Vec<&Wanted> = wanted.iter().filter(|w| !found.contains_key(&w.external_id)).collect();
        if routed.is_empty() || plan.venue.exchange_type == IBKR || plan.codes.is_empty() { return found; }
        // Only the venue's reads are timed here: the cash priced above, and every quote read in time, are kept.
        let (mut cache, mut quotes) = (Cache::default(), HashMap::new());
        if tokio::time::timeout(BUDGET, read_quotes(&plan, now, &mut cache, &mut quotes)).await.is_err() {
            // What a cut-off fill had already answered is in the cache: read once more.
            take(&plan, &cache, now.timestamp(), &mut quotes);
            log(exchange_id, "out of time: routing what was read");
        }
        // The search is CPU work: on a worker of its own, never the engine's runtime thread; the graph built once.
        let (pairs, ends) = (plan.pairs, plan.ends);
        let routed: Vec<Wanted> = routed.into_iter().cloned().collect();
        let picked = tokio::task::spawn_blocking(move || {
            let graph = venue_routes::Graph::new(&pairs, &quotes);
            routed.into_iter().filter_map(|w| (1..=venue_routes::MAX_LEGS)
                .find_map(|legs| graph.candidates(&ends, w.asset, legs).into_iter().find(|c| super::balances::fits(&w.total, &c.price)))
                .map(|c| (w.external_id, c.price))).collect::<Vec<_>>()
        }).await;
        match picked { Ok(picked) => found.extend(picked), Err(why) => log(exchange_id, &format!("route search failed: {why}")) }
        found
    }
}

/// The venue's last prices of `plan.codes`, by code, into `quotes`, through `cache`: both owned by the caller, so a read
/// cut off by the budget keeps every answer it had, a fill's finished ones included. Each pass first takes every answer the cache holds (the whole set at once,
/// and for Kraken, where that read fails as soon as one code does, each code on its own), then a second reader records
/// what to ask next: the whole-market read for most venues, and for Kraken one code at a time, nearest first, until the
/// pass holds `MAX_SINGLE_READS` requests. At most three fills; the last one is read too.
async fn read_quotes<T: Transport>(plan: &Plan<T>, now: DateTime<Utc>, cache: &mut Cache, quotes: &mut HashMap<String, BigDec>) {
    let at = now.timestamp();
    for pass in 0..=3 {
        take(plan, cache, at, quotes);
        if pass == 3 { break; }
        let demands: Vec<HttpRequest> = {
            let next = Reader::new(cache, at);
            if plan.venue.exchange_type == KRAKEN {
                for code in plan.codes.iter().filter(|c| !quotes.contains_key(*c)) {
                    if next.demands().len() >= MAX_SINGLE_READS { break; }
                    let _ = ask(plan, &next, std::slice::from_ref(code));
                }
            } else {
                let _ = ask(plan, &next, &plan.codes);
            }
            next.demands()
        };
        if demands.is_empty() { break; }
        cache.fill(&plan.wire, demands, at).await;
    }
}

fn ask<T>(plan: &Plan<T>, reader: &Reader<'_>, codes: &[String]) -> Option<Vec<(String, crate::figures::market::Member<crate::figures::dec::Dec>)>> {
    crate::figures::budget::within(|| Ok::<_, crate::figures::FiguresError>(reader.prices(&plan.venue, codes))).ok().and_then(Result::ok)
}

/// Every price the cache holds for `plan.codes` into `quotes`: the whole set at once, and for Kraken, whose read fails as
/// soon as one code does, each code on its own.
fn take<T>(plan: &Plan<T>, cache: &Cache, at: i64, quotes: &mut HashMap<String, BigDec>) {
    let reader = Reader::new(cache, at);
    let mut rows = ask(plan, &reader, &plan.codes).unwrap_or_default();
    if rows.is_empty() && plan.venue.exchange_type == KRAKEN {
        for code in plan.codes.iter().filter(|c| !quotes.contains_key(*c)) { rows.extend(ask(plan, &reader, std::slice::from_ref(code)).unwrap_or_default()); }
    }
    for (code, price) in rows { if let Some(p) = price.ok().as_ref().and_then(big).filter(BigDec::is_positive) { quotes.insert(code, p); } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codes_asked_are_the_pairs_within_reach() {
        let pairs = [Pair { id: 1, code: "A/B".into(), base: 1, quote: 2 }, Pair { id: 2, code: "B/C".into(), base: 2, quote: 3 },
                     Pair { id: 3, code: "C/D".into(), base: 3, quote: 4 }, Pair { id: 4, code: "D/E".into(), base: 4, quote: 5 }, Pair { id: 5, code: "X/Y".into(), base: 8, quote: 9 }];
        assert_eq!(codes_near(&pairs, &[1], &HashSet::new()), ["A/B", "B/C", "C/D"]);
        assert_eq!(codes_near(&pairs, &[1], &HashSet::from([2])), ["A/B"], "nothing past a fiat end");
    }
}
