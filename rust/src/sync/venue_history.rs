//! The venue's own daily prices for the chart's history (Rust only, PLAN-venue-prices S3, D-VENUE-HISTORY-FALLBACK): the
//! days of a coin the provider left unstored, from the venue's daily candles along a route of its listed pairs to a fiat,
//! converted to dollars at that day's ECB rate.
//!
//! A leg's price on day D is the open of its candle that opens exactly at D 00:00 UTC (the provider's day convention); a
//! venue whose days start at another hour gives no days, never shifted ones. Never fails the sweep: what cannot be read
//! in time is left out, and `retry` says whether the next sweep should ask again (a transport failure or a throttle).
use super::venue_routes::{self, Pair, Route};
use crate::figures::{at::At, db::Ticker, market::{MarketData, Venue}, page_market::{Cache, Reader}};
use crate::jobs::Db;
use crate::ruby::BigDec;
use crate::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use chrono::{DateTime, NaiveDate, Utc};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

/// All of one sweep's venue history reads.
pub const BUDGET: Duration = Duration::from_secs(90);
/// Routes tried per asset.
const ROUTES: usize = 3;
const IBKR: &str = "Exchanges::Ibkr";
const ALPACA: &str = "Exchanges::Alpaca";
const DAY: i64 = 86_400;

/// Whether a 2xx body is a venue's own throttle answer, read as JSON: Bybit's `retCode` 10006/10018, Binance's `code`
/// -1003, KuCoin's `429000`, Bitget's `429`, Kraken's `EAPI:Rate limit exceeded` / `EGeneral:Too many requests`, and a
/// "too many requests"/"too many visits" message in any case. Any other body-level error (an unknown pair, Bitget's
/// `40014` permission refusal) is no reason to ask again.
fn throttled(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return false };
    let number = |key: &str| v.get(key).and_then(|c| c.as_i64().or_else(|| c.as_str().and_then(|s| s.parse().ok())));
    if matches!(number("retCode"), Some(10006 | 10018)) || matches!(number("code"), Some(-1003 | 429000 | 429)) { return true; }
    let words = |s: &str| { let s = s.to_lowercase(); s.contains("too many requests") || s.contains("too many visits") || s.contains("rate limit exceeded") };
    if v.get("error").and_then(|e| e.as_array()).is_some_and(|errors| errors.iter().filter_map(|e| e.as_str()).any(|e| e.starts_with("EAPI:Rate limit") || e.starts_with("EGeneral:Too many"))) { return true; }
    ["msg", "message", "retMsg", "reason"].iter().filter_map(|k| v.get(*k).and_then(|m| m.as_str())).any(words)
}

/// The fallback's wire: the venue's own, and a record of whether any request met a failure worth asking again for.
pub struct Watched<T> { pub inner: T, pub retry: Cell<bool> }

impl<T> Watched<T> {
    pub fn new(inner: T) -> Self { Self { inner, retry: Cell::new(false) } }
    fn note(&self, answer: &Result<HttpResponse, TransportError>) {
        let again = match answer {
            // A refusal that sends nothing (no usable key, no usable proxy) is no reason to ask again.
            Err(TransportError::Permanent(_)) => false,
            Err(_) => true,
            Ok(r) => r.status == 429 || r.status >= 500 || ((200..300).contains(&r.status) && throttled(&r.body)),
        };
        if again { self.retry.set(true); }
    }
}

impl<T: Transport> Transport for Watched<T> {
    fn producer(&self) -> Option<crate::engine::model::CredentialVersion> { self.inner.producer() }
    fn redact_diagnostic(&self, text: &str) -> String { self.inner.redact_diagnostic(text) }
    fn now(&self) -> DateTime<Utc> { self.inner.now() }
    fn deadline_clock(&self) -> Option<DateTime<Utc>> { self.inner.deadline_clock() }
    async fn send(&self, req: &HttpRequest) -> Result<HttpResponse, TransportError> { let a = self.inner.send(req).await; self.note(&a); a }
    async fn send_limited(&self, req: &HttpRequest, limit: usize) -> Result<HttpResponse, TransportError> { let a = self.inner.send_limited(req, limit).await; self.note(&a); a }
}

/// One coin's span on the venue: from, to, and the asset it was that span.
#[derive(Clone, Debug)]
pub struct Segment { pub from: NaiveDate, pub to: NaiveDate, pub asset: i64 }

/// What a venue's history read gave: dollar prices by day, whether to ask again, and the key version Alpaca's reads were
/// made with (its rows are stored fenced on it).
#[derive(Default)]
pub struct Days { pub prices: BTreeMap<NaiveDate, BigDec>, pub retry: bool, pub version: Option<crate::engine::model::CredentialVersion> }

struct Read { venue: Venue, pairs: Vec<Pair>, tickers: HashMap<i64, Ticker>, fiat: HashMap<i64, (bool, String)>, fx: crate::tracker::fx::FxTable,
              key: Option<(crate::crypto::Credentials, crate::engine::model::CredentialVersion)>, venues: super::venue_quotes::Egresses }

fn log(exchange_id: i64, why: &str) { crate::engine::log(&format!("[tracker] venue history for exchange {exchange_id}: {why}")); }
fn big(d: &crate::figures::dec::Dec) -> Option<BigDec> { BigDec::parse(&d.to_s_f()).ok() }
fn midnight(day: NaiveDate) -> i64 { day.and_hms_opt(0, 0, 0).map_or(0, |t| t.and_utc().timestamp()) }

/// The days of `segments` not in `have`, priced from the venue's daily candles (`make` builds the wire as S2's quotes
/// do). `deadline`: when the sweep's venue reads must stop; what was answered by then is used.
pub async fn history<T: Transport>(make: &super::venue_quotes::MakeWire<T>, db: &Db, exchange_id: i64, api_key_id: Option<i64>, segments: Vec<Segment>,
                                   have: HashSet<NaiveDate>, deadline: tokio::time::Instant) -> Days {
    let assets: Vec<i64> = segments.iter().map(|s| s.asset).collect();
    let read = db.run(move |c, cipher| {
        let exchange_type: String = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [exchange_id], |r| r.get(0)).map_err(|e| e.to_string())?;
        let mut s = c.prepare("SELECT t.id, t.ticker, t.base, t.base_asset_id, t.quote_asset_id, t.quote_decimals, a.category, a.id IS NOT NULL FROM tickers t \
                               LEFT JOIN assets a ON a.id = t.base_asset_id WHERE t.exchange_id = ?1 AND t.base_asset_id IS NOT NULL AND t.quote_asset_id IS NOT NULL ORDER BY t.id")
            .map_err(|e| e.to_string())?;
        let rows = s.query_map([exchange_id], |r| Ok((Pair { id: r.get(0)?, code: r.get(1)?, base: r.get(3)?, quote: r.get(4)? },
            Ticker { id: r.get(0)?, ticker: r.get(1)?, base: r.get(2)?, base_asset_id: r.get(3)?, quote_decimals: r.get(5)?, base_category: r.get(6)?, base_asset_exists: r.get(7)? })))
            .map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
        // Alpaca's history is asked for its crypto only (its stocks have their bars).
        let rows: Vec<(Pair, Ticker)> = if exchange_type == ALPACA { rows.into_iter().filter(|(p, _)| p.code.contains('/')).collect() } else { rows };
        let ids: HashSet<i64> = rows.iter().flat_map(|(p, _)| [p.base, p.quote]).chain(assets.iter().copied()).collect();
        let mut fiat = HashMap::new();
        let mut s = c.prepare("SELECT id, symbol FROM assets WHERE category IN ('Fiat','Currency')").map_err(|e| e.to_string())?;
        for row in s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))).map_err(|e| e.to_string())? {
            let (id, symbol) = row.map_err(|e| e.to_string())?;
            let Some(symbol) = symbol.map(|s| s.to_uppercase()).filter(|s| ids.contains(&id) && (s == "USD" || crate::tracker::fiat(s))) else { continue };
            fiat.insert(id, (symbol == "USD", symbol));
        }
        let fx = crate::figures::budget::within(|| crate::tracker::fx::FxTable::load(c)).map_err(|e| format!("{e:?}"))?;
        let key = match api_key_id.filter(|_| exchange_type == ALPACA) { Some(id) => Some(super::credentials_with_version(c, cipher, id).map_err(|e| e.0)?), None => None };
        let venues = crate::web::figure::loading::venue_egresses(c, cipher);
        let (pairs, tickers): (Vec<Pair>, Vec<Ticker>) = rows.into_iter().unzip();
        Ok(Read { venue: Venue { exchange_id, exchange_type }, tickers: tickers.into_iter().map(|t| (t.id, t)).collect(), pairs, fiat, fx, key, venues })
    }).await;
    let read = match read { Ok(r) => r, Err(why) => { log(exchange_id, &why); return Days::default() } };
    if read.venue.exchange_type == IBKR { return Days::default(); }
    let version = read.key.as_ref().map(|(_, v)| v.clone());
    let wire = Watched::new(make(read.key.as_ref().map(|(c, _)| c), read.venues));
    let usd: HashMap<i64, bool> = read.fiat.iter().map(|(id, (usd, _))| (*id, *usd)).collect();
    // Each segment's routes; every leg's candles since its segment's first day.
    // Each segment from its first day not stored; a segment stored whole asks nothing.
    let first_missing = |s: &Segment| { let mut d = s.from; while d <= s.to { if !have.contains(&d) { return Some(d); } d = d.succ_opt()?; } None };
    let routes: Vec<(Segment, Vec<Route>)> = segments.into_iter().filter_map(|s| first_missing(&s).map(|from| Segment { from, ..s }))
        .map(|s| { let r = venue_routes::topology(&read.pairs, &usd, s.asset, ROUTES); (s, r) }).collect();
    let mut since: HashMap<i64, NaiveDate> = HashMap::new();
    for (s, rs) in &routes { for leg in rs.iter().flat_map(|r| &r.legs) { let d = since.entry(leg.pair).or_insert(s.from); *d = (*d).min(s.from); } }
    // One clock for the sweep's fills and reads: an answer kept by an early fill is still readable at the end.
    let at_start = Utc::now().timestamp();
    let mut cache = Cache::default();
    let mut opens: HashMap<i64, HashMap<i64, BigDec>> = HashMap::new();
    let take = |cache: &Cache, opens: &mut HashMap<i64, HashMap<i64, BigDec>>| -> Vec<HttpRequest> {
        let reader = Reader::new(cache, at_start);
        for (pair, from) in &since {
            if opens.contains_key(pair) { continue; }
            let Some(ticker) = read.tickers.get(pair) else { continue };
            let Some(at) = At::from_utc(DateTime::from_timestamp(midnight(*from), 0).unwrap_or_default()) else { continue };
            let got = crate::figures::budget::within(|| Ok::<_, crate::figures::FiguresError>(reader.candles(&read.venue, ticker, at, DAY, false)));
            if let Ok(Ok(candles)) = got {
                // Only a candle opening exactly at a day's midnight is that day's price.
                let days = candles.into_iter().filter_map(|(at, open)| { let t = at.utc().timestamp(); (t.rem_euclid(DAY) == 0).then_some(t).zip(big(&open).filter(BigDec::is_positive)) }).collect();
                opens.insert(*pair, days);
            }
        }
        reader.demands()
    };
    let read_all = async {
        for _ in 0..3 {
            let demands = take(&cache, &mut opens);
            if demands.is_empty() { return; }
            cache.fill(&wire, demands, at_start).await;
        }
    };
    let cut = tokio::time::timeout_at(deadline, read_all).await.is_err();
    take(&cache, &mut opens);
    if cut { log(exchange_id, "out of time"); }
    let mut days = Days { retry: cut || wire.retry.get(), version, prices: BTreeMap::new() };
    let one = BigDec::one();
    for (s, rs) in &routes {
        let mut d = s.from;
        while d <= s.to {
            if !have.contains(&d) {
                let at = midnight(d);
                let price = rs.iter().find_map(|r| {
                    let mut p = BigDec::one();
                    for leg in &r.legs {
                        let open = opens.get(&leg.pair)?.get(&at)?;
                        p = if leg.forward { &p * open } else { p.div(open)? };
                    }
                    let (usd, symbol) = read.fiat.get(&r.end)?;
                    let rate = if *usd { BigDec::one() } else {
                        crate::figures::budget::within(|| read.fx.rate(symbol, "USD", d)).ok().flatten().and_then(|r| big(&r)).filter(BigDec::is_positive)?
                    };
                    Some(&p * &rate).filter(|p| p.is_positive() && super::balances::fits(&one, p))
                });
                if let Some(price) = price { days.prices.insert(d, price); }
            }
            d = match d.succ_opt() { Some(n) => n, None => break };
        }
    }
    days
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_throttle_is_told_from_a_permanent_miss() {
        let w = Watched::new(());
        let ok = |status: u16, body: &str| Ok(HttpResponse { status, body: body.into() });
        w.note(&ok(200, r#"{"retCode":0}"#));
        w.note(&ok(400, r#"{"error":["EQuery:Unknown asset pair"]}"#));
        w.note(&ok(200, r#"{"code":"40014","msg":"Incorrect permissions"}"#));
        assert!(!w.retry.get(), "an unknown pair and a permission refusal are permanent");
        w.note(&ok(200, r#"{"retCode":10006,"retMsg":"Too many visits!"}"#));
        assert!(w.retry.get());
        let refused = Watched::new(());
        refused.note(&Err(TransportError::Permanent("Market data unavailable".into())));
        assert!(!refused.retry.get(), "a refusal that sent nothing is permanent");
        for answer in [ok(429, ""), ok(503, ""), ok(200, r#"{"error":["EAPI:Rate limit exceeded"]}"#), ok(200, r#"{"code": "429000", "msg": "Too Many Requests"}"#),
                       ok(200, r#"{ "code" : -1003 }"#), Err(TransportError::NotSent("dns".into()))] {
            let w = Watched::new(());
            w.note(&answer);
            assert!(w.retry.get(), "{answer:?}");
        }
    }
}
