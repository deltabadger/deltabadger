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
