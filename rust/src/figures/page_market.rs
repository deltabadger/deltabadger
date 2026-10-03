//! Raw market responses are shared; bounded decimals are made and dropped only on the computing thread.
use super::at::At;
use super::db::Ticker;
use super::dec::Dec;
use super::market::{Failure, Fetch, MarketData, Member, Quoted, Venue};
use super::num::Num;
use crate::venue::http::{HttpRequest, Transport, TransportError};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;

pub const PRICE_TTL: i64 = 60;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 512;
const UNAVAILABLE: &str = "Market data unavailable";

#[derive(Clone)]
struct Entry { until: i64, value: Fetch<Value> }
#[derive(Clone, Default)]
pub struct Cache { entries: BTreeMap<String, Entry> }

fn key(request: &HttpRequest) -> String {
    format!("{}?{}", request.path, request.query.iter().map(|(k,v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"))
}
fn request(path: &str, query: Vec<(&'static str, String)>) -> HttpRequest {
    HttpRequest { method: "GET", base: "https://data.alpaca.markets".into(), path: path.into(), query, body: None, not_after: None }
}

impl Cache {
    /// Failures expire too: a bad key or rate limit must not create a retry loop per page load.
    pub async fn fill(&mut self, wire: &impl Transport, demands: Vec<HttpRequest>, now: i64) {
        for request in demands {
            let value = match wire.send_limited(&request, MAX_BYTES).await {
                Ok(reply) if (200..300).contains(&reply.status) => crate::venue::http::decode_json(&reply.body)
                    .map_err(|_| Failure::Failed(UNAVAILABLE.into())),
                Ok(_) | Err(TransportError::Permanent(_)) => Err(Failure::Failed(UNAVAILABLE.into())),
                Err(_) => Err(Failure::Raised(UNAVAILABLE.into())),
            };
            if self.entries.len() >= MAX_ENTRIES { self.entries.retain(|_, entry| entry.until > now); }
            if self.entries.len() >= MAX_ENTRIES { self.entries.clear(); }
            self.entries.insert(key(&request), Entry { until: now.saturating_add(PRICE_TTL), value });
        }
    }
}

pub struct Reader<'a> { cache: &'a Cache, now: i64, demands: RefCell<BTreeMap<String, HttpRequest>> }
impl<'a> Reader<'a> {
    pub fn new(cache: &'a Cache, now: i64) -> Self { Self { cache, now, demands: RefCell::default() } }
    pub fn demands(&self) -> Vec<HttpRequest> { self.demands.borrow().values().cloned().collect() }
    fn get(&self, request: HttpRequest) -> Fetch<Value> {
        let key = key(&request);
        if let Some(entry) = self.cache.entries.get(&key).filter(|entry| entry.until > self.now) { return entry.value.clone(); }
        self.demands.borrow_mut().insert(key, request);
        Err(Failure::Failed(UNAVAILABLE.into()))
    }
}

impl MarketData for Reader<'_> {
    fn prices(&self, venue: &Venue, symbols: &[String]) -> Fetch<Vec<(String, Member<Dec>)>> {
        if venue.exchange_type != "Exchanges::Alpaca" { return Err(Failure::Failed(UNAVAILABLE.into())); }
        let mut prices = vec![];
        let mut failed = None;
        for crypto in [false, true] {
            let mut names: Vec<&str> = symbols.iter().filter(|s| s.contains('/') == crypto).map(String::as_str).collect();
            names.sort_unstable(); names.dedup();
            if names.is_empty() { continue; }
            let path = if crypto { "/v1beta3/crypto/us/latest/trades" } else { "/v2/stocks/snapshots" };
            match self.get(request(path, vec![("symbols", names.join(","))])) {
                Err(error) => failed = Some(error),
                Ok(body) => {
                    let entries = if crypto { &body["trades"] } else { &body };
                    let Some(entries) = entries.as_object() else { failed = Some(Failure::Failed(UNAVAILABLE.into())); continue; };
                    for (code, item) in entries {
                        let price = if crypto { &item["p"] } else { &item["latestTrade"]["p"] };
                        // Owner ruling: absent trades carry no price. A reported numeric zero remains a price.
                        if !price.is_null() { prices.push((code.clone(), Dec::to_d(price))); }
                    }
                }
            }
        }
        match failed { Some(error) => Err(error), None => Ok(prices) }
    }
    fn candles(&self, _: &Venue, _: &Ticker, _: At, _: i64, _: bool) -> Fetch<Vec<(At, Dec)>> {
        Err(Failure::Failed(UNAVAILABLE.into()))
    }
    fn exchange_rates(&self) -> Fetch<Vec<(String, Member<Num>)>> { Err(Failure::Failed("Currency conversion unavailable".into())) }
    fn coin_price(&self, _: &str, _: &str) -> Fetch<Quoted> { Err(Failure::Failed("Currency conversion unavailable".into())) }
}
