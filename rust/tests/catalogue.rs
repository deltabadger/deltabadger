//! venue::catalogue against Rails' `get_tickers_info(force: true)` (PLAN-A17 A17-2): every vector recorded by
//! script/rust/catalogue_vectors.rb is replayed over its own ScriptedTransport, and must give Rails' rows (or Rails'
//! failure sentence) after the same requests — method, host, path, query, body, count, and for Gemini the same pacing,
//! on paused time. The one named divergence, D-CG-DECIMALS-EXPONENT, is asserted value by value.
use deltabadger::venue::catalogue;
use deltabadger::venue::http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError};
use serde_json::{json, Value};
use std::cell::RefCell;
use tokio::time::{Duration, Instant};

/// The scripted transport, noting when (on tokio's clock) each request was sent.
struct Timed { inner: ScriptedTransport, start: Instant, at: RefCell<Vec<Duration>> }

impl Transport for Timed {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.at.borrow_mut().push(self.start.elapsed());
        self.inner.send(r).await
    }
}

/// D-CG-DECIMALS-EXPONENT: (scenario, row, field, Rails' value, Rust's value).
const DIVERGENCES: [(&str, usize, &str, i64, i64); 9] = [
    ("bingx_exponent_steps", 0, "base_decimals", 5, 6),   // stepSize 1e-06: Float#to_s "1.0e-06"
    ("bingx_exponent_steps", 0, "quote_decimals", 5, 8),  // tickSize 1e-08
    ("bingx_exponent_steps", 0, "price_decimals", 5, 8),
    ("bingx_exponent_steps", 1, "quote_decimals", 5, 12), // tickSize 1.5e-11: "1.5e-11"
    ("bingx_exponent_steps", 1, "price_decimals", 5, 12),
    ("bingx_exponent_steps", 2, "quote_decimals", 0, 7),  // tickSize "1e-7": no point at all
    ("bingx_exponent_steps", 2, "price_decimals", 0, 7),
    // 1e-05 is "1.0e-05": five characters after the point, which happens to be right.
    ("bingx_exponent_steps", 1, "base_decimals", 5, 5),
    // The plan's Gemini example: `tick_size.to_d.to_s` is plain notation under ActiveSupport, so Rails is right there.
    ("gemini_exponent_ticks", 1, "base_decimals", 8, 8),
];

#[tokio::test(start_paused = true)]
async fn every_recorded_catalogue_reads_as_rails() {
    let vectors: Value = serde_json::from_str(include_str!("fixtures/catalogue_vectors.json")).unwrap();
    let scenarios = vectors["scenarios"].as_array().unwrap();
    assert_eq!(scenarios.len(), 29);
    for sc in scenarios {
        let name = sc["name"].as_str().unwrap();
        let t = Timed { inner: ScriptedTransport::from_script(&sc["script"]), start: Instant::now(), at: RefCell::new(vec![]) };
        let got = catalogue::read(sc["venue"].as_str().unwrap(), &t).await;

        let rails = &sc["rails"];
        match (&got, rails.get("rows")) {
            (Ok(rows), Some(expected)) => {
                let mut expected = expected.clone();
                for (scenario, i, field, in_rails, in_rust) in DIVERGENCES.iter().filter(|d| d.0 == name) {
                    assert_eq!(expected[*i][*field], json!(in_rails), "{name}: Rails' {field} of row {i}");
                    expected[*i][*field] = json!(in_rust);
                    assert_eq!(rows[*i][*field], json!(in_rust), "{scenario}: Rust's {field} of row {i}");
                }
                assert_eq!(&Value::Array(rows.clone()), &expected, "{name}: rows");
            }
            (Err(message), None) => assert_eq!(message, rails["message"].as_str().unwrap(), "{name}: failure"),
            (got, _) => panic!("{name}: Rust {got:?}, Rails {rails}"),
        }

        let sent = t.inner.requests();
        let recorded = sc["requests"].as_array().unwrap();
        assert_eq!(sent.len(), recorded.len(), "{name}: request count");
        for ((r, want), at) in sent.iter().zip(recorded).zip(t.at.borrow().iter()) {
            let mut query: Vec<Value> = r.query.iter().map(|(k, v)| json!([k, v])).collect();
            query.sort_by_key(|q| q.to_string());
            let got = json!({ "method": r.method, "host": r.base, "path": r.path, "query": query, "body": r.body });
            let want_req = json!({ "method": want["method"], "host": want["host"], "path": want["path"], "query": want["query"], "body": want["body"] });
            assert_eq!(got, want_req, "{name}: request");
            assert_eq!(at.as_secs_f64(), want["at"].as_f64().unwrap(), "{name}: {} sent after Rails' waits", r.path);
        }
        assert_eq!(t.start.elapsed().as_secs_f64(), sc["slept"].as_f64().unwrap(), "{name}: time slept");
    }
}

#[tokio::test]
async fn a_retired_stock_or_unknown_venue_has_no_catalogue() {
    let t = ScriptedTransport::default();
    for venue in ["Exchanges::Bitmart", "Exchanges::Alpaca", "Exchanges::Ibkr", "Exchanges::Nope"] {
        assert!(catalogue::read(venue, &t).await.is_err(), "{venue}");
    }
    assert!(t.requests().is_empty(), "nothing asked");
}
