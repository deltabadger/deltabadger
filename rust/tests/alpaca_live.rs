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
    Ticker { id: 0, ticker: "BTC/USD".into(), base_code: "BTC".into(), quote_code: "USD".into(), base_symbol: "BTC".into(), quote_symbol: "USD".into(), exchange_name: "Alpaca".into(),
             base_asset_id: 0, quote_asset_id: 0, base_decimals: 9, quote_decimals: 2, price_decimals: 2,
             minimum_base_size: bd("0.000027"), minimum_quote_size: bd("1"), trading_enabled: true, available: true, crypto: true, }
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
fn write_bodies(dir: &str, log: &[(HttpRequest, HttpResponse)]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (n, (r, resp)) in log.iter().enumerate() {
        let parsed = serde_json::from_str::<Value>(&resp.body);
        // A non-JSON /v2/account body cannot be redacted, so it is never written.
        let mut body = match parsed {
            Ok(v) => v,
            Err(_) if r.path == "/v2/account" => Value::String("REDACTED: non-JSON account body".into()),
            Err(_) => Value::String(resp.body.clone()),
        };
        if r.path == "/v2/account" { for k in ["id", "account_number"] { if body.get(k).is_some() { body[k] = json!("REDACTED"); } } }
        let record = json!({ "key": format!("{} {}", r.method, r.path), "query": r.query, "status": resp.status, "body": body });
        std::fs::write(std::path::Path::new(dir).join(format!("{n:02}.json")), serde_json::to_string_pretty(&record).unwrap())?;
    }
    Ok(())
}

/// Writes whatever was logged when the test ends, passed or panicked, so a failed run still leaves its bodies. Never panics.
struct Recorder(Option<String>, Rc<RefCell<Vec<(HttpRequest, HttpResponse)>>>);

impl Drop for Recorder {
    fn drop(&mut self) {
        let Some(dir) = &self.0 else { return };
        match write_bodies(dir, &self.1.borrow()) {
            Ok(()) => eprintln!("recorded {} answers in {dir}", self.1.borrow().len()),
            Err(e) => eprintln!("could not record into {dir}: {e}"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "places a real order on the owner's Alpaca PAPER account; run by hand with --ignored"]
async fn a_paper_market_buy_is_found_by_client_order_id_and_fills() {
    let Some((key, secret)) = paper_key() else { return };
    let dir = std::env::var("ALPACA_RECORD_DIR").ok();
    if let Some(d) = &dir { refuse_inside_repo(d); }
    let log = Rc::new(RefCell::new(vec![]));
    let _recorder = Recorder(dir, log.clone());
    let v = AlpacaVenue::new(Recording(ReqwestTransport::new(client(), key, secret), log.clone()), Urls::for_passphrase(Some("paper")));
    assert_eq!(v.urls().trading, PAPER_TRADING_URL, "paper only");
    let t = btc_usd();
    assert!(v.price(&t, PriceSide::Ask).await.unwrap().is_positive());
    assert!(v.price(&t, PriceSide::Last).await.unwrap().is_positive());
    assert!(v.balance("USD", true).await.unwrap().is_positive(), "the paper account has buying power");
    let cl = uuid::Uuid::new_v4().to_string();
    assert_eq!(v.order_by_client_id(&cl, Utc::now()).await.unwrap(), None, "an unknown client order id is Alpaca's own not-found envelope");
    let (_, not_found) = log.borrow().iter().rev().find(|(r, _)| r.path == "/v2/orders:by_client_order_id").cloned().unwrap();
    assert_eq!(not_found.status, 404);
    assert!(deltabadger::venue::alpaca::alpaca_not_found(&not_found.body), "the real envelope must match the matcher: {}", not_found.body);
    let order = NewOrder { pair: "BTC/USD".into(), kind: OrderKind::Market, volume: "10.00".into(), quote_volume: true, cl_ord_id: cl.clone(), deadline: Utc::now(), day: false, };
    let id = v.add_order(&order).await.unwrap();
    let found = v.order_by_client_id(&cl, Utc::now()).await.unwrap().expect("found by client_order_id");
    assert_eq!(found.txid, id);
    // The engine's follow-up poll reads GET /v2/orders/{id} (Venue::orders): always ask it at least once, so its body is
    // recorded even when the client-order-id lookup already saw the fill.
    let mut state = v.orders(std::slice::from_ref(&id)).await.unwrap().remove(0);
    for _ in 0..30 {
        if state.status == OrderStatus::Closed { break; }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        state = v.orders(std::slice::from_ref(&id)).await.unwrap().remove(0);
    }
    assert_eq!(state.status, OrderStatus::Closed, "{state:?}");
    if found.status == OrderStatus::Closed { assert_eq!(found, state, "both reads of one filled order parse the same"); }
    assert!(state.amount_exec.is_positive() && state.quote_amount_exec.is_positive(), "{state:?}");
    assert_eq!(state.quote_amount, Some(BigDec::parse("10").unwrap()));
}

/// The JSON type a read sees: Alpaca's trading API sends decimals as strings, its market data API sends numbers.
fn json_type(v: Option<&Value>) -> &'static str {
    match v {
        None => "absent", Some(Value::Null) => "null", Some(Value::String(_)) => "string", Some(Value::Number(_)) => "number",
        Some(Value::Bool(_)) => "bool", Some(Value::Array(_)) => "array", Some(Value::Object(_)) => "object",
    }
}

#[test]
#[ignore = "reads bodies recorded from the owner's paper account; run by hand with --ignored"]
fn recorded_bodies_carry_every_field_the_grid_scripts() {
    let Ok(dir) = std::env::var("ALPACA_RECORD_DIR") else { eprintln!("skipped: ALPACA_RECORD_DIR is not set"); return };
    // Every field the Rails parse and the Rust port read, per endpoint and status, as JSON pointers ("/" in BTC/USD is
    // "~1") with the JSON types allowed. An array body (positions) is checked element by element.
    const S: &str = "string";
    const SN: &str = "string|null";
    type Fields<'a> = &'a [(&'a str, &'a str)];
    let order: Fields = &[("/id", S), ("/client_order_id", S), ("/status", S), ("/symbol", S), ("/type", S), ("/side", S),
                                   ("/filled_qty", S), ("/filled_avg_price", SN), ("/notional", SN), ("/qty", SN), ("/limit_price", SN)];
    let need: [(&str, u64, Fields); 8] = [
        ("GET /v1beta3/crypto/us/latest/quotes", 200, &[("/quotes/BTC~1USD/ap", "number"), ("/quotes/BTC~1USD/bp", "number")]),
        ("GET /v1beta3/crypto/us/latest/trades", 200, &[("/trades/BTC~1USD/p", "number")]),
        ("POST /v2/orders", 200, order),
        ("GET /v2/orders:by_client_order_id", 200, order),
        ("GET /v2/orders:by_client_order_id", 404, &[("/code", "number"), ("/message", S)]),
        ("GET /v2/orders/", 200, order),
        ("GET /v2/account", 200, &[("/cash", S), ("/non_marginable_buying_power", S), ("/buying_power", S)]),
        ("GET /v2/positions", 200, &[("/symbol", S), ("/qty", S)]),
    ];
    let files: Vec<Value> = std::fs::read_dir(&dir).unwrap()
        .map(|e| serde_json::from_str(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap()).collect();
    for (prefix, status, fields) in need {
        let matching: Vec<&Value> = files.iter().filter(|f| f["key"].as_str().is_some_and(|k| k.starts_with(prefix)) && f["status"] == status).collect();
        assert!(!matching.is_empty(), "no recorded {status} for {prefix}");
        for f in matching {
            let bodies: Vec<&Value> = match &f["body"] { Value::Array(a) => a.iter().collect(), b => vec![b] };
            for body in bodies {
                for (p, allowed) in fields {
                    let got = json_type(body.pointer(p));
                    assert!(allowed.split('|').any(|t| t == got), "{prefix} {status}: {p} is {got}, the grid scripts {allowed}: {body}");
                }
            }
        }
    }
}
