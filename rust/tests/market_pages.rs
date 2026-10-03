use deltabadger::figures::market::{MarketData, Venue};
use deltabadger::figures::page_market::{Cache, Reader};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::json;
use std::cell::RefCell;

struct Wire { calls: RefCell<Vec<HttpRequest>>, body: String }
impl Transport for Wire {
    async fn send(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.calls.borrow_mut().push(request.clone());
        Ok(HttpResponse { status: 200, body: self.body.clone() })
    }
}
fn venue() -> Venue { Venue { exchange_id: 1, exchange_type: "Exchanges::Alpaca".into() } }

#[tokio::test]
async fn prices_expire_and_missing_trades_are_not_zero() {
    let mut cache = Cache::default();
    let symbols = vec!["BBB".into(), "AAA".into()];
    let wire = Wire { calls: RefCell::default(), body: json!({"AAA":{"latestTrade":{"p":104.52}},"BBB":{}}).to_string() };
    let reader = Reader::new(&cache, 100);
    assert!(reader.prices(&venue(), &symbols).is_err());
    let demand = reader.demands();
    assert_eq!(demand.len(), 1);
    assert_eq!(demand[0].query, vec![("symbols", "AAA,BBB".into())]);
    cache.fill(&wire, demand, 100).await;
    let reader = Reader::new(&cache, 159);
    let prices = reader.prices(&venue(), &symbols).unwrap();
    assert_eq!(prices.len(), 1);
    assert_eq!(prices[0].0, "AAA");
    assert_eq!(prices[0].1.as_ref().unwrap().to_s_f(), "104.52");
    assert!(reader.demands().is_empty());
    assert!(Reader::new(&cache, 160).prices(&venue(), &symbols).is_err());
    assert_eq!(wire.calls.borrow().len(), 1);
}

#[tokio::test]
async fn crypto_empty_requests_failures_and_bad_unused_numbers() {
    let mut cache = Cache::default();
    let symbols = vec!["BTC/USD".into(), "ETH/USD".into(), "BAD/USD".into()];
    let wire = Wire { calls: RefCell::default(), body: json!({"trades":{"BTC/USD":{"p":0},"ETH/USD":{},"BAD/USD":{"p":"NaN"}}}).to_string() };
    assert!(Reader::new(&cache, 0).prices(&venue(), &[]).unwrap().is_empty());
    let reader = Reader::new(&cache, 0);
    let _ = reader.prices(&venue(), &symbols);
    cache.fill(&wire, reader.demands(), 0).await;
    let prices = Reader::new(&cache, 0).prices(&venue(), &symbols).unwrap();
    assert_eq!(prices.len(), 2);
    assert!(prices.iter().find(|p| p.0 == "BTC/USD").unwrap().1.as_ref().unwrap().is_zero());
    assert!(prices.iter().find(|p| p.0 == "BAD/USD").unwrap().1.is_err());
    assert_eq!(wire.calls.borrow()[0].base, "https://data.alpaca.markets");
    assert_eq!(wire.calls.borrow()[0].path, "/v1beta3/crypto/us/latest/trades");
}

fn ticker(crypto: bool) -> deltabadger::figures::db::Ticker {
    deltabadger::figures::db::Ticker { id: 1, ticker: if crypto { "BTC/USD" } else { "AAA" }.into(), base: "AAA".into(), base_asset_id: 1,
        quote_decimals: Some(2), base_category: Some(if crypto { "Cryptocurrency" } else { "Stock" }.into()), base_asset_exists: true }
}

#[tokio::test]
async fn closed_candles_overlap_and_rebuild_instead_of_splicing_a_split() {
    use deltabadger::figures::at::At;
    let mut cache = Cache::default();
    let at = |seconds: i64| At(seconds * 1_000_000_000);
    let bars = |price: f64| json!({"bars":[{"t":"1970-01-01T00:01:00Z","o":price},{"t":"1970-01-01T00:00:00Z","o":10},{"t":"1970-01-01T00:02:00Z","o":30}]}).to_string();
    let wire = Wire { calls: RefCell::default(), body: bars(20.0) };
    let reader = Reader::new(&cache, 120);
    assert!(reader.candles(&venue(), &ticker(false), at(0), 60, true).is_err());
    cache.fill(&wire, reader.demands(), 120).await;
    let reader = Reader::new(&cache, 179);
    let got = reader.candles(&venue(), &ticker(false), at(0), 60, true).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, at(0));
    assert!(reader.demands().is_empty());
    let reader = Reader::new(&cache, 180);
    assert!(reader.candles(&venue(), &ticker(false), at(0), 60, true).is_err());
    let demand = reader.demands();
    assert!(demand[0].query.contains(&("start", "1970-01-01T00:00:00Z".into())));
    // Cache.fill changes the wire start to the overlapping bar; a restatement then refetches the head.
    let changed = Wire { calls: RefCell::default(), body: bars(2.0) };
    cache.fill(&changed, demand, 180).await;
    assert_eq!(changed.calls.borrow().len(), 2);
    assert!(changed.calls.borrow()[0].query.contains(&("start", "1970-01-01T00:01:00Z".into())));
    assert!(changed.calls.borrow()[1].query.contains(&("start", "1970-01-01T00:00:00Z".into())));
    let got = Reader::new(&cache, 180).candles(&venue(), &ticker(false), at(0), 60, true).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got[1].1.to_s_f(), "2.0");
}
