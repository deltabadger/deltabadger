//! Characterization of BinanceVenue::call and KucoinVenue::call (plan VB §1.4): every answer class a read, a placement and
//! the bulk price read can meet, pinned on what the two adapters answered before the transport-error split moved to
//! `http::transport_failure`. The two calls stay separate policies (401, 429/418, non-JSON on the bulk read, KuCoin's
//! envelope code); this table proves none of them moved. Rails-free.
//!
//! The pin is tests/fixtures/venue_call_classes.txt; WRITE_VENUE_CALL_CLASSES=1 rewrites it from the current code.
use chrono::{Duration, Utc};
use deltabadger::venue::binance::{self, BinanceVenue};
use deltabadger::venue::http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError};
use deltabadger::venue::kucoin::{self, KucoinVenue};
use deltabadger::venue::{NewOrder, OrderKind, Side, Venue, VenueError};
use serde_json::{json, Value};

const STATUSES: [u16; 11] = [200, 302, 400, 401, 403, 404, 418, 429, 500, 502, 503];
const NETWORK: [&str; 3] = ["pre_send", "post_send", "permanent"];

/// The scripted transport on a fixed clock: Binance's signed URLs (timestamp and signature) print the same every run.
#[derive(Clone)]
struct Fixed(ScriptedTransport);
impl Transport for Fixed {
    fn now(&self) -> chrono::DateTime<Utc> { "2026-10-20T08:00:00Z".parse().unwrap() }
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> { self.0.send(r).await }
}

#[derive(Clone, Copy)]
enum Mode { Read, Placement, Bulk }

fn shown(r: Result<String, VenueError>) -> String {
    match r {
        Ok(v) => format!("Ok({v})"),
        Err(VenueError::Rejected(m)) => format!("Rejected({m:?})"),
        Err(VenueError::Unauthorized(m)) => format!("Unauthorized({m:?})"),
        Err(VenueError::Ambiguous(m)) => format!("Ambiguous({m:?})"),
        Err(VenueError::Transient(m)) => format!("Transient({m:?})"),
    }
}

fn order() -> NewOrder {
    NewOrder { pair: "BTCUSDT".into(), side: Side::Buy, kind: OrderKind::Market, volume: "10".into(), quote_volume: true, cl_ord_id: "cl-1".into(),
               deadline: Utc::now() + Duration::minutes(5), day: false }
}

/// The scripted key each mode's one request answers to.
fn key(venue: &str, mode: Mode) -> &'static str {
    match (venue, mode) {
        ("binance", Mode::Read) => "DELETE /api/v3/order",
        ("binance", Mode::Placement) => "POST /api/v3/order",
        ("binance", Mode::Bulk) => "GET /api/v3/ticker/price",
        (_, Mode::Read) => "DELETE /api/v1/orders/abc",
        (_, Mode::Placement) => "POST /api/v1/orders",
        (_, Mode::Bulk) => "GET /api/v1/market/allTickers",
    }
}

async fn run(venue: &str, mode: Mode, reply: Value) -> String {
    let t = Fixed(ScriptedTransport::from_script(&json!({ key(venue, mode): [reply] })));
    let r = if venue == "binance" {
        let v = BinanceVenue::new(t, binance::URL, "secret");
        match mode {
            Mode::Read => v.cancel("BTCUSDT-1").await.map(|()| "()".to_string()),
            Mode::Placement => v.add_order(&order()).await,
            Mode::Bulk => v.tickers_prices(&[]).await.map(|p| format!("{p:?}")),
        }
    } else {
        let v = KucoinVenue::new(t, kucoin::URL);
        match mode {
            Mode::Read => v.cancel("abc").await.map(|()| "()".to_string()),
            Mode::Placement => v.add_order(&order()).await,
            Mode::Bulk => v.tickers_prices(&[]).await.map(|p| format!("{p:?}")),
        }
    };
    shown(r)
}

/// The bodies an answer can carry: the venue's success for that mode, its error envelope, an HTML page, nothing.
fn bodies(venue: &str, mode: Mode) -> Vec<(&'static str, Value)> {
    let success = match (venue, mode) {
        ("binance", Mode::Read) => json!({ "symbol": "BTCUSDT", "orderId": 1, "status": "CANCELED" }),
        ("binance", Mode::Placement) => json!({ "symbol": "BTCUSDT", "orderId": 28 }),
        ("binance", Mode::Bulk) => json!([{ "symbol": "BTCUSDT", "price": "1.5" }]),
        (_, Mode::Read) => json!({ "code": "200000", "data": { "cancelledOrderIds": ["abc"] } }),
        (_, Mode::Placement) => json!({ "code": "200000", "data": { "orderId": "k1" } }),
        (_, Mode::Bulk) => json!({ "code": "200000", "data": { "ticker": [{ "symbol": "BTC-USDT", "last": "2.5" }] } }),
    };
    let error = if venue == "binance" { json!({ "code": -1121, "msg": "Invalid symbol." }) } else { json!({ "code": "400100", "msg": "order not exist" }) };
    let mut out = vec![("success", success), ("error", error), ("html", Value::String("<html>bad gateway</html>".into())), ("empty", Value::String(String::new()))];
    // A 2xx whose envelope is not KuCoin's success code (the envelope check is the caller's, after `call`).
    if venue == "kucoin" { out.push(("code", json!({ "code": "400100", "msg": "Balance insufficient!" }))); }
    out
}

async fn table() -> String {
    let mut lines = vec![];
    for venue in ["binance", "kucoin"] {
        for (mode, name) in [(Mode::Read, "read"), (Mode::Placement, "placement"), (Mode::Bulk, "bulk")] {
            for status in STATUSES {
                for (body_name, body) in bodies(venue, mode) {
                    let got = run(venue, mode, json!({ "status": status, "body": body })).await;
                    lines.push(format!("{venue} {name} {status} {body_name} => {got}"));
                }
            }
            for kind in NETWORK {
                let got = run(venue, mode, json!({ "network": kind, "message": format!("{kind} failure") })).await;
                lines.push(format!("{venue} {name} network {kind} => {got}"));
            }
        }
    }
    lines.join("\n") + "\n"
}

#[tokio::test(flavor = "current_thread")]
async fn every_answer_class_is_classified_as_before() {
    let got = table().await;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/venue_call_classes.txt");
    if std::env::var_os("WRITE_VENUE_CALL_CLASSES").is_some() { std::fs::write(&path, &got).unwrap(); }
    let pinned = std::fs::read_to_string(&path).unwrap();
    for (i, (g, p)) in got.lines().zip(pinned.lines()).enumerate() { assert_eq!(g, p, "line {}", i + 1); }
    assert_eq!(got.lines().count(), pinned.lines().count());
}
