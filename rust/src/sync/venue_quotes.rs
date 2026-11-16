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
use crate::ruby::BigDec;
use crate::venue::http::{HttpRequest, Transport};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// The whole fallback, the ECB's load included, inside the balance job's deadline with room for the writes.
pub const BUDGET: Duration = Duration::from_secs(60);
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

/// The pairs within `MAX_LEGS` of the wanted assets, whatever their prices: the codes to ask for.
fn codes_near(pairs: &[Pair], from: &[i64]) -> Vec<String> {
    let mut reached: HashSet<i64> = from.iter().copied().collect();
    let mut codes: Vec<String> = vec![];
    let mut taken: HashSet<i64> = HashSet::new();
    let mut frontier: Vec<i64> = from.to_vec();
    for _ in 0..venue_routes::MAX_LEGS {
        let mut next = vec![];
        for p in pairs.iter().filter(|p| frontier.contains(&p.base) || frontier.contains(&p.quote)) {
            if taken.insert(p.id) { codes.push(p.code.clone()); }
            for a in [p.base, p.quote] { if reached.insert(a) { next.push(a); } }
        }
        frontier = next;
    }
    codes
}

impl<T: Transport + 'static> VenueQuotes for Quotes<T> {
    fn prices<'a>(&'a self, db: &'a Db, key_id: i64, exchange_id: i64, wanted: Vec<Wanted>, now: DateTime<Utc>) -> QuotesFuture<'a> {
        Box::pin(async move {
            match tokio::time::timeout(BUDGET, self.run(db, key_id, exchange_id, wanted, now)).await {
                Ok(found) => found,
                Err(_) => { log(exchange_id, "out of time"); HashMap::new() }
            }
        })
    }
}

impl<T: Transport + 'static> Quotes<T> {
    async fn run(&self, db: &Db, key_id: i64, exchange_id: i64, wanted: Vec<Wanted>, now: DateTime<Utc>) -> HashMap<String, BigDec> {
        let today = now.date_naive();
        crate::tracker::fx::ensure_loaded(db, self.ecb.as_ref(), today).await;
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
            let codes = codes_near(&pairs, &assets);
            Ok(Read { venue: Venue { exchange_id, exchange_type }, pairs, ends, codes, credentials, venues })
        }).await;
        let plan = match plan {
            Ok(r) => Plan { wire: (self.make)(r.credentials.as_ref(), r.venues), venue: r.venue, pairs: r.pairs, ends: r.ends, codes: r.codes },
            Err(why) => { log(exchange_id, &why); return HashMap::new() }
        };
        let mut found: HashMap<String, BigDec> = HashMap::new();
        // Cash with an ECB rate needs no venue: its rate is its price.
        for w in wanted.iter().filter(|w| w.cash) {
            if let Some(end) = plan.ends.get(&w.asset) { found.insert(w.external_id.clone(), end.rate.clone()); }
        }
        let routed: Vec<&Wanted> = wanted.iter().filter(|w| !found.contains_key(&w.external_id)).collect();
        if routed.is_empty() || plan.venue.exchange_type == IBKR || plan.codes.is_empty() { return found; }
        let quotes = read_quotes(&plan, now).await;
        for w in routed {
            let pick = (1..=venue_routes::MAX_LEGS).find_map(|legs| venue_routes::candidates(&plan.pairs, &quotes, &plan.ends, w.asset, legs).into_iter()
                .find(|c| super::balances::fits(&w.total, &c.price)));
            if let Some(c) = pick { found.insert(w.external_id.clone(), c.price); }
        }
        found
    }
}

/// The venue's last prices of `plan.codes`, by code: the whole set asked at once, and where that read fails, each code on
/// its own (Kraken's one-code reads, at most `MAX_SINGLE_READS` requests a pass), so one failed code never drops the others. Each pass
/// records what it lacks; a fill answers it; at most three passes.
async fn read_quotes<T: Transport>(plan: &Plan<T>, now: DateTime<Utc>) -> HashMap<String, BigDec> {
    let mut cache = Cache::default();
    let at = now.timestamp();
    let mut quotes: HashMap<String, BigDec> = HashMap::new();
    for _ in 0..3 {
        let demands: Vec<HttpRequest> = {
            let reader = Reader::new(&cache, at);
            let mut rows: Vec<(String, crate::figures::market::Member<crate::figures::dec::Dec>)> = vec![];
            match crate::figures::budget::within(|| Ok::<_, crate::figures::FiguresError>(reader.prices(&plan.venue, &plan.codes))) {
                Ok(Ok(all)) => rows = all,
                _ if plan.venue.exchange_type == KRAKEN => {
                    for code in plan.codes.iter().filter(|c| !quotes.contains_key(*c)) {
                        if let Ok(Ok(one)) = crate::figures::budget::within(|| Ok::<_, crate::figures::FiguresError>(reader.prices(&plan.venue, std::slice::from_ref(code)))) { rows.extend(one); }
                    }
                }
                _ => {}
            }
            for (code, price) in rows { if let Some(p) = price.ok().as_ref().and_then(big).filter(BigDec::is_positive) { quotes.insert(code, p); } }
            let mut demands = reader.demands();
            // Kraken's one-code reads past its bulk Ticker, and any venue's: a bounded number per pass.
            demands.truncate(MAX_SINGLE_READS);
            demands
        };
        if demands.is_empty() { break; }
        cache.fill(&plan.wire, demands, at).await;
    }
    quotes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codes_asked_are_the_pairs_within_reach() {
        let pairs = [Pair { id: 1, code: "A/B".into(), base: 1, quote: 2 }, Pair { id: 2, code: "B/C".into(), base: 2, quote: 3 },
                     Pair { id: 3, code: "C/D".into(), base: 3, quote: 4 }, Pair { id: 4, code: "D/E".into(), base: 4, quote: 5 }, Pair { id: 5, code: "X/Y".into(), base: 8, quote: 9 }];
        assert_eq!(codes_near(&pairs, &[1]), ["A/B", "B/C", "C/D"]);
    }
}
