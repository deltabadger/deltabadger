//! The paper live check and the body recorder. Both make real requests on the OWNER'S ALPACA PAPER ACCOUNT, so both are
//! #[ignore]d, gated by his paper-key env vars, and never run in CI:
//!   ALPACA_PAPER_KEY_ID=… ALPACA_PAPER_SECRET_KEY=… [ALPACA_RECORD_DIR=<private dir outside the repo>] \
//!     cargo test --test alpaca_live -- --ignored --nocapture --test-threads=1
use chrono::Utc;
use deltabadger::engine::model::Ticker;
use deltabadger::ruby::BigDec;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls, PAPER_TRADING_URL};
use deltabadger::venue::http::{client, HttpRequest, HttpResponse, ReqwestTransport, Transport, TransportError};
use deltabadger::venue::{NewOrder, OrderKind, OrderStatus, PriceSide, Venue};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// The real transport, keeping every request and answer for the recorder.
#[derive(Clone)]
struct Recording(ReqwestTransport, Rc<RefCell<Vec<(HttpRequest, HttpResponse)>>>);

impl Transport for Recording {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let resp = self.0.send(r).await?;
        self.1.borrow_mut().push((r.clone(), resp.clone()));
        Ok(resp)
    }
}

fn paper_key() -> Option<(String, String)> {
    match (std::env::var("ALPACA_PAPER_KEY_ID"), std::env::var("ALPACA_PAPER_SECRET_KEY")) {
        (Ok(k), Ok(s)) if !k.is_empty() && !s.is_empty() => Some((k, s)),
        _ => { eprintln!("skipped: ALPACA_PAPER_KEY_ID / ALPACA_PAPER_SECRET_KEY are not set"); None }
    }
}

fn btc_usd() -> Ticker {
    let bd = |s: &str| BigDec::parse(s).unwrap();
    Ticker { id: 0, ticker: "BTC/USD".into(), base_code: "BTC".into(), base_symbol: "BTC".into(), quote_symbol: "USD".into(), exchange_name: "Alpaca".into(),
             base_asset_id: 0, quote_asset_id: 0, base_decimals: 9, quote_decimals: 2, price_decimals: 2,
             minimum_base_size: bd("0.000027"), minimum_quote_size: bd("1"), trading_enabled: true, available: true }
}

/// Recordings hold the owner's account bodies: refuse any directory inside the repo checkout.
fn refuse_inside_repo(dir: &str) {
    let mut abs = std::env::current_dir().unwrap().join(dir);
    let mut tail = std::path::PathBuf::new();
    while !abs.exists() {
        tail = std::path::PathBuf::from(abs.file_name().expect("ALPACA_RECORD_DIR has no existing ancestor")).join(tail);
        abs.pop();
    }
    let resolved = abs.canonicalize().unwrap().join(tail);
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().canonicalize().unwrap();
    assert!(!resolved.starts_with(&repo), "ALPACA_RECORD_DIR {} is inside the repo {}; recordings must stay outside it", resolved.display(), repo.display());
}

/// Each exchange as `<dir>/NN.json` {key, query, status, body}. Keys are headers and are never recorded; the account's own
/// identifiers are redacted.
fn write_bodies(dir: &str, log: &[(HttpRequest, HttpResponse)]) {
    refuse_inside_repo(dir);
    std::fs::create_dir_all(dir).unwrap();
    for (n, (r, resp)) in log.iter().enumerate() {
        let mut body: Value = serde_json::from_str(&resp.body).unwrap_or_else(|_| Value::String(resp.body.clone()));
        if r.path == "/v2/account" { for k in ["id", "account_number"] { if body.get(k).is_some() { body[k] = json!("REDACTED"); } } }
        let record = json!({ "key": format!("{} {}", r.method, r.path), "query": r.query, "status": resp.status, "body": body });
        std::fs::write(std::path::Path::new(dir).join(format!("{n:02}.json")), serde_json::to_string_pretty(&record).unwrap()).unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "places a real order on the owner's Alpaca PAPER account; run by hand with --ignored"]
async fn a_paper_market_buy_is_found_by_client_order_id_and_fills() {
    let Some((key, secret)) = paper_key() else { return };
    if let Ok(dir) = std::env::var("ALPACA_RECORD_DIR") { refuse_inside_repo(&dir); }
    let log = Rc::new(RefCell::new(vec![]));
    let v = AlpacaVenue::new(Recording(ReqwestTransport::new(client(), key, secret), log.clone()), Urls::for_passphrase(Some("paper")));
    assert_eq!(v.urls().trading, PAPER_TRADING_URL, "paper only");
    let t = btc_usd();
    assert!(v.price(&t, PriceSide::Ask).await.unwrap().is_positive());
    assert!(v.price(&t, PriceSide::Last).await.unwrap().is_positive());
    assert!(v.balance("USD").await.unwrap().is_positive(), "the paper account has buying power");
    let cl = uuid::Uuid::new_v4().to_string();
    assert_eq!(v.order_by_client_id(&cl, Utc::now()).await.unwrap(), None, "an unknown client order id is Alpaca's own not-found envelope");
    let (_, not_found) = log.borrow().iter().rev().find(|(r, _)| r.path == "/v2/orders:by_client_order_id").cloned().unwrap();
    assert_eq!(not_found.status, 404);
    assert!(deltabadger::venue::alpaca::alpaca_not_found(&not_found.body), "the real envelope must match the matcher: {}", not_found.body);
    let order = NewOrder { pair: "BTC/USD".into(), kind: OrderKind::Market, volume: "10.00".into(), quote_volume: true, cl_ord_id: cl.clone(), deadline: Utc::now() };
    let id = v.add_order(&order).await.unwrap();
    let mut state = v.order_by_client_id(&cl, Utc::now()).await.unwrap().expect("found by client_order_id");
    assert_eq!(state.txid, id);
    for _ in 0..30 {
        if state.status == OrderStatus::Closed { break; }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        state = v.orders(std::slice::from_ref(&id)).await.unwrap().remove(0);
    }
    assert_eq!(state.status, OrderStatus::Closed, "{state:?}");
    assert!(state.amount_exec.is_positive() && state.quote_amount_exec.is_positive(), "{state:?}");
    assert_eq!(state.quote_amount, Some(BigDec::parse("10").unwrap()));
    if let Ok(dir) = std::env::var("ALPACA_RECORD_DIR") {
        write_bodies(&dir, &log.borrow());
        eprintln!("recorded {} answers in {dir}", log.borrow().len());
    }
}

#[test]
#[ignore = "reads bodies recorded from the owner's paper account; run by hand with --ignored"]
fn recorded_bodies_carry_every_field_the_grid_scripts() {
    let Ok(dir) = std::env::var("ALPACA_RECORD_DIR") else { eprintln!("skipped: ALPACA_RECORD_DIR is not set"); return };
    // Every field the Rails parse and the Rust port read, per endpoint, as JSON pointers ("/" in BTC/USD is "~1").
    let need: [(&str, &[&str]); 6] = [
        ("GET /v1beta3/crypto/us/latest/quotes", &["/quotes/BTC~1USD/ap", "/quotes/BTC~1USD/bp"]),
        ("GET /v1beta3/crypto/us/latest/trades", &["/trades/BTC~1USD/p"]),
        ("POST /v2/orders", &["/id", "/client_order_id", "/status", "/symbol", "/type", "/side", "/notional"]),
        ("GET /v2/orders:by_client_order_id", &["/id", "/client_order_id", "/status", "/filled_qty"]),
        ("GET /v2/orders/", &["/id", "/status", "/symbol", "/type", "/side", "/filled_qty", "/filled_avg_price", "/notional"]),
        ("GET /v2/account", &["/cash", "/non_marginable_buying_power", "/buying_power"]),
    ];
    let files: Vec<Value> = std::fs::read_dir(&dir).unwrap()
        .map(|e| serde_json::from_str(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap()).collect();
    for (prefix, pointers) in need {
        let matching: Vec<&Value> = files.iter().filter(|f| f["key"].as_str().is_some_and(|k| k.starts_with(prefix)) && f["status"] == 200).collect();
        assert!(!matching.is_empty(), "no recorded 200 for {prefix}");
        for f in matching { for p in pointers { assert!(f["body"].pointer(p).is_some(), "{prefix}: {p} missing in {}", f["body"]); } }
    }
}
