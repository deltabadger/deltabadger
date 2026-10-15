//! The Binance spot TESTNET recorder (spec E2 T8). It places and cancels real testnet orders, so it is #[ignore]d, gated by
//! a testnet key from the environment, and never runs in CI:
//!   BINANCE_TESTNET_KEY=… BINANCE_TESTNET_SECRET=… BINANCE_RECORD_DIR=<private dir outside the repo> \
//!     cargo test --test binance_live -- --ignored --nocapture --test-threads=1
//! It records what the adapter cannot prove from documentation: the newClientOrderId echo, the answer to an origClientOrderId
//! lookup of an order that does not exist (the -2013 status and text that may one day let `order_by_client_id` answer
//! Ok(None)), the exact -1021 wording, MARKET quoteOrderQty and LIMIT NEW/CANCELED bodies, and allOrders/myTrades shapes.
//! The testnet host lives only here (`with_base`); the shipped binary never names it.
use deltabadger::engine::model::Ticker;
use deltabadger::ruby::BigDec;
use deltabadger::venue::binance::BinanceVenue;
use deltabadger::venue::http::{client, HttpRequest, HttpResponse, ReqwestTransport, Transport, TransportError};
use deltabadger::venue::{NewOrder, OrderKind, PriceSide, Venue};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

const TESTNET: &str = "https://testnet.binance.vision";

/// The real transport, keeping every request and answer for the recorder.
#[derive(Clone)]
struct Recording(ReqwestTransport, Rc<RefCell<Vec<(HttpRequest, HttpResponse)>>>);

impl Transport for Recording {
    fn now(&self) -> chrono::DateTime<chrono::Utc> { self.0.now() }
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let resp = self.0.send(r).await?;
        self.1.borrow_mut().push((r.clone(), resp.clone()));
        Ok(resp)
    }
}

fn testnet_key() -> Option<(String, String, String)> {
    match (std::env::var("BINANCE_TESTNET_KEY"), std::env::var("BINANCE_TESTNET_SECRET"), std::env::var("BINANCE_RECORD_DIR")) {
        (Ok(k), Ok(s), Ok(d)) if !k.is_empty() && !s.is_empty() && !d.is_empty() => Some((k, s, d)),
        _ => { eprintln!("skipped: BINANCE_TESTNET_KEY / BINANCE_TESTNET_SECRET / BINANCE_RECORD_DIR are not set"); None }
    }
}

/// Recordings are private: refuse any directory inside the repo checkout.
fn refuse_inside_repo(dir: &str) {
    let mut abs = std::env::current_dir().unwrap().join(dir);
    let mut tail = std::path::PathBuf::new();
    while !abs.exists() {
        tail = std::path::PathBuf::from(abs.file_name().expect("BINANCE_RECORD_DIR has no existing ancestor")).join(tail);
        abs.pop();
    }
    let resolved = abs.canonicalize().unwrap().join(tail);
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().canonicalize().unwrap();
    assert!(!resolved.starts_with(&repo), "BINANCE_RECORD_DIR {} is inside the repo {}; recordings stay outside it", resolved.display(), repo.display());
}

/// Each exchange as `<dir>/NN.json` {method, path, query (without timestamp and signature), status, body}. The key travels
/// in a header and is never recorded.
fn write_bodies(dir: &str, log: &[(HttpRequest, HttpResponse)]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (n, (r, resp)) in log.iter().enumerate() {
        let query: Vec<(String, String)> = r.query.iter().filter(|(k, _)| !matches!(*k, "timestamp" | "signature")).map(|(k, v)| (k.to_string(), v.clone())).collect();
        let body = serde_json::from_str::<Value>(&resp.body).unwrap_or_else(|_| Value::String(resp.body.clone()));
        let out = json!({ "method": r.method, "path": r.path, "query": query, "status": resp.status, "body": body });
        std::fs::write(format!("{dir}/{n:02}.json"), serde_json::to_string_pretty(&out)?)?;
    }
    Ok(())
}

fn btc_usdt() -> Ticker {
    let bd = |s: &str| BigDec::parse(s).unwrap();
    Ticker { id: 0, ticker: "BTCUSDT".into(), base_code: "BTC".into(), quote_code: "USDT".into(), base_symbol: "BTC".into(), quote_symbol: "USDT".into(),
             exchange_name: "Binance".into(), base_asset_id: 0, quote_asset_id: 0, base_decimals: 5, quote_decimals: 8, price_decimals: 2,
             minimum_base_size: bd("0.00001"), minimum_quote_size: bd("5"), trading_enabled: true, available: true, crypto: true }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "places real orders on the Binance spot testnet; needs BINANCE_TESTNET_KEY, BINANCE_TESTNET_SECRET and BINANCE_RECORD_DIR"]
async fn record_the_testnet_answers_the_port_relies_on() {
    let Some((key, secret, dir)) = testnet_key() else { return };
    refuse_inside_repo(&dir);
    let log = Rc::new(RefCell::new(vec![]));
    let transport = Recording(ReqwestTransport::binance(client(), key, secret.clone()), log.clone());
    let v = BinanceVenue::new(transport.clone(), TESTNET, &secret);
    let deadline = || chrono::Utc::now() + chrono::Duration::seconds(10);
    let since = chrono::Utc::now() - chrono::Duration::hours(1);

    // A LIMIT buy far under the market (it rests), found again by its client order id, then cancelled.
    let last = v.price(&btc_usdt(), PriceSide::Last).await.expect("last price");
    let rest = last.floor(0).div(&BigDec::from_i64(2)).unwrap().floor(2);
    let resting_id = uuid::Uuid::new_v4().to_string();
    let limit = NewOrder { pair: "BTCUSDT".into(), kind: OrderKind::Limit { price: rest.to_s_f() }, volume: "0.001".into(), quote_volume: false,
                           cl_ord_id: resting_id.clone(), deadline: deadline(), day: false };
    let id = v.add_order(&limit).await.expect("a resting limit buy");
    let looked_up = v.order_by_client_id(&resting_id, "BTCUSDT", since).await;
    eprintln!("lookup of the resting order by its client id: {looked_up:?}");
    let ext = id.rsplit('-').next().unwrap().to_string();
    let cancel = v.signed("DELETE", "/api/v3/order", vec![("symbol", "BTCUSDT".into()), ("orderId", ext.clone()), ("recvWindow", "5000".into())], transport.now());
    transport.send(&cancel).await.expect("cancel");
    let _ = v.order_identified(&id, |_, _, _| Ok(true)).await; // the CANCELED body

    // A MARKET buy by quote, then its order and trades.
    let market = NewOrder { pair: "BTCUSDT".into(), kind: OrderKind::Market, volume: "15".into(), quote_volume: true,
                            cl_ord_id: uuid::Uuid::new_v4().to_string(), deadline: deadline(), day: false };
    let filled = v.add_order(&market).await.expect("a market buy by quote");
    let polled = v.orders(&[filled.clone(), id.clone()]).await;
    eprintln!("allOrders + myTrades for {filled} and {id}: {polled:?}");

    // A client order id that was never used: the answer `order_by_client_id` would need to prove absence (-2013).
    let absent = v.order_by_client_id(&uuid::Uuid::new_v4().to_string(), "BTCUSDT", since).await;
    eprintln!("lookup of an unused client id: {absent:?}");

    // A signed read stamped an hour ago: the exact -1021 wording.
    let stale = v.signed("GET", "/api/v3/account", vec![("omitZeroBalances", "true".into()), ("recvWindow", "5000".into())],
                         transport.now() - chrono::Duration::hours(1));
    let answer = transport.send(&stale).await.expect("an answer");
    assert_eq!(answer.status, 400, "a stale timestamp is refused: {}", answer.body);

    write_bodies(&dir, &log.borrow()).expect("recordings written");
    eprintln!("wrote {} exchanges to {dir}", log.borrow().len());
}
