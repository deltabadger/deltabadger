//! Which Alpaca host each call reaches for each key mode, recorded from Rails (script/rust/record_alpaca_hosts.rb, in a
//! clean export) and replayed through the Rust venue: the passphrase goes through `Urls`, the call through the venue
//! method the engine, the syncs and the tracker use, and the request's host must be the one Rails reached. No request
//! leaves the process (a scripted transport answers).
mod common;
use deltabadger::crypto::Credentials;
use deltabadger::engine::model::Ticker;
use deltabadger::ruby::BigDec;
use deltabadger::venue::alpaca::{AlpacaVenue, LiveFactory, Urls};
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::{NewOrder, OrderKind, PriceSide, Venue};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

fn grid() -> Value { serde_json::from_str(include_str!("fixtures/alpaca_hosts.json")).unwrap() }

#[test]
fn the_recorded_rails_sources_are_unchanged() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let pins = grid()["ported_sources"].clone();
    let client = std::fs::read(root.join("app/models/clients/alpaca.rb")).unwrap();
    assert_eq!(hex::encode(Sha256::digest(&client)), pins["app/models/clients/alpaca.rb"], "app/models/clients/alpaca.rb changed: re-record script/rust/record_alpaca_hosts.rb");
    let exchange = std::fs::read(root.join("app/models/exchanges/alpaca.rb")).unwrap();
    assert_eq!(hex::encode(Sha256::digest(&exchange)), pins["app/models/exchanges/alpaca.rb"], "app/models/exchanges/alpaca.rb changed: re-record script/rust/record_alpaca_hosts.rb");
}

fn btc_usd() -> Ticker {
    let bd = |s: &str| BigDec::parse(s).unwrap();
    Ticker { id: 1, ticker: "BTC/USD".into(), base_code: "BTC".into(), quote_code: "USD".into(), base_symbol: "BTC".into(), quote_symbol: "USD".into(), exchange_name: "Alpaca".into(),
             base_asset_id: 1, quote_asset_id: 2, base_decimals: 9, quote_decimals: 2, price_decimals: 2,
             minimum_base_size: bd("0.000027"), minimum_quote_size: bd("1"), trading_enabled: true, available: true, crypto: true }
}

fn host(base: &str) -> &str { base.split_once("://").map_or(base, |(_, h)| h) }

#[tokio::test(flavor = "current_thread")]
async fn every_call_reaches_the_host_rails_reaches_for_its_mode() {
    let rows = grid()["rows"].as_array().unwrap().clone();
    assert!(rows.len() >= 80);
    for row in rows {
        let passphrase = row["passphrase"].as_str();
        let call = row["call"].as_str().unwrap();
        let urls = Urls::for_passphrase(passphrase);
        if call == "key_validity" {
            // Key validation asks `Urls::pick(..).trading` of the key it validates (App::alpaca_trading_url).
            assert_eq!(host(&urls.trading), row["host"], "{row}");
            continue;
        }
        let reply = json!([{ "status": 200, "body": {} }]);
        let script = json!({ format!("{} {}", row["method"].as_str().unwrap(), row["path"].as_str().unwrap()): reply });
        let t = ScriptedTransport::from_script(&script);
        let v = AlpacaVenue::new(t.clone(), urls);
        let order = NewOrder { pair: "BTC/USD".into(), kind: OrderKind::Market, volume: "60".into(), quote_volume: true, cl_ord_id: "c-1".into(),
                               deadline: chrono::Utc::now() + chrono::Duration::seconds(30), day: false };
        // Only the request matters here; each answer's parse is tested elsewhere.
        match call {
            "account" | "no_key_account" => { let _ = v.read(false, "/v2/account", vec![], 1 << 20).await; }
            "positions" => { let _ = v.positions().await; }
            "order_create" => { let _ = v.add_order(&order).await; }
            "order_get" => { let _ = v.orders(&["OID".to_string()]).await; }
            "account_activities" => { let _ = v.read(false, "/v2/account/activities", vec![], 1 << 20).await; }
            "clock" => { let _ = v.clock().await; }
            "stock_snapshots" => { let _ = v.read(true, "/v2/stocks/snapshots", vec![("symbols", "AAPL".into())], 1 << 20).await; }
            "stock_bars" => { let _ = v.read(true, "/v2/stocks/AAPL/bars", vec![], 1 << 20).await; }
            "crypto_quotes" => { let _ = v.price(&btc_usd(), PriceSide::Ask).await; }
            "crypto_trades" => { let _ = v.price(&btc_usd(), PriceSide::Last).await; }
            other => panic!("a call this test does not map: {other}"),
        }
        let sent = t.requests();
        assert_eq!(sent.len(), 1, "{row}: {sent:?}");
        assert_eq!((sent[0].method, sent[0].path.as_str(), host(&sent[0].base)), (row["method"].as_str().unwrap(), row["path"].as_str().unwrap(), row["host"].as_str().unwrap()), "{row}");
    }
}

/// The production factory picks the same trading host Rails does for every recorded mode (only `urls()` is read).
#[test]
fn the_factory_picks_rails_host_for_every_recorded_mode() {
    let f = LiveFactory::new();
    for row in grid()["rows"].as_array().unwrap().iter().filter(|r| r["call"] == "account") {
        let credentials = Credentials { redaction_values: vec![], key: "PK".into(), secret: "s".into(), passphrase: row["passphrase"].as_str().map(str::to_string) };
        assert_eq!(host(&f.alpaca(Some(credentials), vec![]).urls().trading), row["host"], "{row}");
    }
    let unkeyed = grid()["rows"].as_array().unwrap().iter().find(|r| r["call"] == "no_key_account").unwrap().clone();
    assert_eq!(host(&f.alpaca(None, vec![]).urls().trading), unkeyed["host"]);
}

/// The live host is named in one place only, so no later edit can route to it around `Urls::pick`.
#[test]
fn only_the_alpaca_venue_names_the_live_host() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src];
    let mut naming = vec![];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() { stack.push(p); continue; }
            if p.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                if text.contains("//api.alpaca.markets") || text.replace("PAPER_TRADING_URL", "").contains("TRADING_URL") { naming.push(p); }
            }
        }
    }
    let naming: Vec<_> = naming.iter().map(|p| p.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap().display().to_string()).collect();
    assert_eq!(naming, ["src/venue/alpaca.rs"], "the live host must be named only by venue/alpaca.rs");
}

/// No test gives the real binary a live Alpaca key: its factory reaches Alpaca's real hosts, and nothing in it can be
/// pointed elsewhere. A test that launches the binary must fail before any network call some other way.
#[test]
fn no_test_launches_the_binary_with_a_live_key() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut stack = vec![tests];
    let mut offenders = vec![];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() { stack.push(p); continue; }
            if p.extension().is_some_and(|e| e == "rs") && p.file_name().is_some_and(|n| n != "alpaca_hosts.rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                if text.contains("CARGO_BIN_EXE_deltabadger") && text.contains(r#"encrypt("live")"#) { offenders.push(p.display().to_string()); }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
}
