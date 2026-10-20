//! Slice S (PLAN-VB §1.2): the Gemini, Coinbase and Hyperliquid signers against the requests honeymaker 0.12.5 and
//! hyperliquid-rb 0.1.1 sent under frozen clocks, nonces and throwaway keys (script/rust/record_venue_signing.rb).
use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
use base64::Engine;
use deltabadger::ruby::BigDec;
use deltabadger::venue::signers::{coinbase, gemini, hyperliquid, Nonce};
use hyperliquid::{Action, OrderWire};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(format!("{name}_signing.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn the_fixtures_are_recorded_from_the_current_recorder() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for name in ["gemini", "coinbase", "hyperliquid"] {
        let f = fixture(name);
        for (file, recorded) in f["sources"].as_object().unwrap() {
            use sha2::Digest;
            let now = hex::encode(sha2::Sha256::digest(std::fs::read(root.join(file)).unwrap()));
            assert_eq!(recorded.as_str().unwrap(), now, "{file} changed: re-record with script/rust/repin");
        }
        assert_eq!(f["gems"]["honeymaker"], "0.12.5");
        assert_eq!(f["gems"]["hyperliquid-rb"], "0.1.1");
    }
}

// ---- the nonce: max(now_ms, last + 1), shared by every caller

#[test]
fn nonces_are_strictly_increasing_within_one_ms_and_never_behind_the_clock() {
    let n = Nonce::new();
    assert_eq!(n.next_at(1_000), 1_000);
    assert_eq!(n.next_at(1_000), 1_001, "two calls in one ms never collide");
    assert_eq!(n.next_at(1_000), 1_002);
    assert_eq!(n.next_at(999), 1_003, "a clock stepping back never reuses a nonce");
    assert_eq!(n.next_at(5_000), 5_000, "and the clock wins once it is ahead");
    let real = gemini::NONCE.next();
    assert!(gemini::NONCE.next() > real);
}

// ---- Gemini: X-GEMINI-PAYLOAD + HMAC-SHA384

/// The params each recorded call passed, as Exchanges::Gemini passes them.
fn gemini_params(call: &str) -> (&'static str, Vec<(&'static str, Value)>) {
    use serde_json::json;
    match call {
        "get_raw_balances" => ("/v1/balances", vec![]),
        "new_order ioc" => ("/v1/order/new", vec![("symbol", json!("btcusd")), ("amount", json!("0.00012")), ("price", json!("64150.12")), ("side", json!("buy")),
                              ("type", json!("exchange limit")), ("client_order_id", Value::Null), ("options", json!(["immediate-or-cancel"]))]),
        "new_order limit" => ("/v1/order/new", vec![("symbol", json!("ethusd")), ("amount", json!("1.5")), ("price", json!("2400")), ("side", json!("sell")),
                                ("type", json!("exchange limit")), ("client_order_id", Value::Null), ("options", json!([]))]),
        "new_order client id" => ("/v1/order/new", vec![("symbol", json!("dogeusd")), ("amount", json!("10")), ("price", json!("0.1")), ("side", json!("buy")),
                                    ("type", json!("exchange limit")), ("client_order_id", json!("5f1c1f3e-<&>/\"é\u{2028}\u{2029} \\x")),
                                    ("options", json!(["immediate-or-cancel"]))]),
        "order_status" => ("/v1/order/status", vec![("order_id", json!("106817811"))]),
        "cancel_order probe" => ("/v1/order/cancel", vec![("order_id", json!(0))]),
        "cancel_order" => ("/v1/order/cancel", vec![("order_id", json!("106817811"))]),
        "get_my_trades" => ("/v1/mytrades", vec![("symbol", Value::Null), ("limit_trades", json!(500)), ("timestamp", json!(1_789_000_000_001u64))]),
        "get_transfers_v2" => ("/v2/transfers", vec![("timestamp", json!(1_789_000_000_001u64)), ("limit_transfers", json!(50)), ("currency", Value::Null),
                                 ("network", Value::Null), ("account", Value::Null), ("show_completed_deposit_advances", Value::Null)]),
        c => panic!("unrecorded call {c}"),
    }
}

#[test]
fn every_recorded_gemini_request_is_signed_byte_for_byte() {
    let f = fixture("gemini");
    let (key, secret) = (f["api_key"].as_str().unwrap(), f["api_secret"].as_str().unwrap());
    let cases = f["signing"].as_array().unwrap();
    assert_eq!(cases.len(), 9);
    for c in cases {
        let call = c["call"].as_str().unwrap();
        let (path, params) = gemini_params(call);
        assert_eq!(c["path"], path, "{call}");
        assert_eq!(c["method"], "POST");
        assert_eq!(c["body"], "", "{call}: the params travel in the payload header only");
        let mut got: Vec<(String, String)> = gemini::headers(key, secret, path, &params, c["nonce"].as_u64().unwrap())
            .into_iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect();
        let mut want: Vec<(String, String)> = c["headers"].as_object().unwrap().iter()
            .filter(|(k, _)| *k != "Content-Length") // the transport's, not the signer's
            .map(|(k, v)| (k.to_ascii_lowercase(), v.as_str().unwrap().to_string())).collect();
        got.sort();
        want.sort();
        assert_eq!(got, want, "{call}");
    }
}

// ---- Coinbase: one JWT per request

fn segment(s: &str) -> String { String::from_utf8(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).unwrap()).unwrap() }

#[test]
fn every_recorded_coinbase_jwt_is_reproduced() {
    let f = fixture("coinbase");
    let key_name = f["api_key"].as_str().unwrap();
    let public = hex::decode(f["es256_public_key"].as_str().unwrap()).unwrap();
    let es256 = UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public);
    let cases = f["signing"].as_array().unwrap();
    assert_eq!(cases.len(), 27);
    let (mut ec, mut ed, mut none) = (0, 0, 0);
    for c in cases {
        let label = format!("{} {}", c["key"].as_str().unwrap(), c["call"].as_str().unwrap());
        let secret = f["secrets"][c["key"].as_str().unwrap()].as_str().unwrap();
        let nonce = c["jwt_header"].as_str().and_then(|h| serde_json::from_str::<Value>(h).ok()).map(|h| h["nonce"].as_str().unwrap().to_string());
        let got = coinbase::jwt_at(key_name, secret, c["method"].as_str().unwrap(), c["path"].as_str().unwrap(), c["now"].as_i64().unwrap(),
                                   nonce.as_deref().unwrap_or("00"));
        let Some(want) = c["jwt"].as_str() else {
            assert_eq!(got, None, "{label}: honeymaker sent no Authorization");
            assert!(c["headers"].get("Authorization").is_none());
            none += 1;
            continue;
        };
        assert_eq!(c["headers"]["Authorization"].as_str().unwrap(), format!("Bearer {want}"), "{label}");
        let got = got.unwrap_or_else(|| panic!("{label}: signed nothing"));
        let (g, w): (Vec<&str>, Vec<&str>) = (got.split('.').collect(), want.split('.').collect());
        assert_eq!(segment(g[0]), c["jwt_header"].as_str().unwrap(), "{label}: header");
        assert_eq!(segment(g[1]), c["jwt_payload"].as_str().unwrap(), "{label}: claims");
        assert_eq!((g[0], g[1]), (w[0], w[1]), "{label}: the signing input, byte for byte");
        if serde_json::from_str::<Value>(c["jwt_header"].as_str().unwrap()).unwrap()["alg"] == "ES256" {
            // ES256 is randomized on both sides: each signature verifies under the key's public half.
            let input = format!("{}.{}", g[0], g[1]);
            for sig in [g[2], w[2]] {
                let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig).unwrap();
                assert_eq!(raw.len(), 64, "{label}: r‖s");
                es256.verify(input.as_bytes(), &raw).unwrap_or_else(|_| panic!("{label}: signature does not verify"));
            }
            ec += 1;
        } else {
            assert_eq!(got, want, "{label}: EdDSA is deterministic, byte for byte");
            ed += 1;
        }
    }
    assert_eq!((ec, ed, none), (10, 15, 2));
}

#[test]
fn a_fresh_coinbase_jwt_carries_the_clock_and_a_random_nonce() {
    let f = fixture("coinbase");
    let secret = f["secrets"]["ed25519"].as_str().unwrap();
    let (a, b) = (coinbase::jwt("k", secret, "get", "/api/v3/brokerage/accounts").unwrap(), coinbase::jwt("k", secret, "GET", "/api/v3/brokerage/accounts").unwrap());
    let header = |j: &str| serde_json::from_str::<Value>(&segment(j.split('.').next().unwrap())).unwrap();
    let claims: Value = serde_json::from_str(&segment(a.split('.').nth(1).unwrap())).unwrap();
    assert_ne!(header(&a)["nonce"], header(&b)["nonce"]);
    assert_eq!(header(&a)["nonce"].as_str().unwrap().len(), 32);
    assert_eq!(claims["uri"], "GET api.coinbase.com/api/v3/brokerage/accounts");
    let now = chrono::Utc::now().timestamp();
    assert!((claims["nbf"].as_i64().unwrap() - now).abs() <= 5);
    assert_eq!(claims["exp"].as_i64().unwrap(), claims["nbf"].as_i64().unwrap() + 120);
}

// ---- Hyperliquid: EIP-712 L1 action

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }

/// The action each recorded call signed, rebuilt from what the caller passed (coin → asset from the recorder's table).
fn hyperliquid_action(call: &str, assets: &Value) -> Action {
    let asset = |coin: &str| assets[coin].as_u64().unwrap();
    let order = |coin: &str, is_buy: bool, sz: &str, px: &str, cloid: Option<&str>| Action::Order(OrderWire {
        asset: asset(coin), is_buy, price: hyperliquid::float_to_wire(&bd(px)).unwrap(), size: hyperliquid::float_to_wire(&bd(sz)).unwrap(),
        reduce_only: false, tif: "Gtc", cloid: cloid.map(str::to_string) });
    match call {
        "order" => order("PURR/USDC", true, "1234.5", "0.19876", None),
        "order sell" => order("HYPE/USDC", false, "0.37", "41.234", None),
        "order cloid" => order("PURR/USDC", true, "10", "0.2", Some("0x5f1c1f3e2b8f4c4e9a590d9c3f1b6a01")),
        "cancel" => Action::Cancel { asset: asset("PURR/USDC"), oid: 41_234_567_890 },
        "cancel small" => Action::Cancel { asset: asset("@1"), oid: 7 },
        "same ms order" => order("PURR/USDC", true, "50", "0.2", None),
        "same ms cancel" => Action::Cancel { asset: asset("PURR/USDC"), oid: 41_234_567_891 },
        c if c.starts_with("order asset ") => {
            let coin = &c["order asset ".len()..];
            let i = assets.as_object().unwrap().keys().position(|k| k == coin).unwrap();
            order(coin, i % 2 == 0, "100", "1670.1", None)
        }
        c if c.starts_with("order wire ") => {
            let (sz, px) = c["order wire ".len()..].split_once(' ').unwrap();
            order("BTC", true, sz, px, None)
        }
        c => panic!("unrecorded call {c}"),
    }
}

#[test]
fn every_recorded_hyperliquid_action_is_packed_hashed_and_signed_byte_for_byte() {
    let f = fixture("hyperliquid");
    let key = hyperliquid::signing_key(f["agent_key"].as_str().unwrap()).unwrap();
    let cases = f["signing"].as_array().unwrap();
    assert_eq!(cases.len(), 21);
    for c in cases {
        let call = c["call"].as_str().unwrap();
        let action = hyperliquid_action(call, &f["assets"]);
        let nonce = c["nonce"].as_u64().unwrap();
        assert_eq!(action.to_json(), c["action"], "{call}: the action");
        assert_eq!(hex::encode(action.msgpack()), c["msgpack"].as_str().unwrap(), "{call}: MessagePack.pack");
        assert_eq!(hex::encode(hyperliquid::action_hash(&action, nonce)), c["action_hash"].as_str().unwrap(), "{call}: action_hash");
        let sig = hyperliquid::sign_l1_action(&key, &action, nonce);
        assert_eq!(serde_json::json!({ "r": sig.r, "s": sig.s, "v": sig.v }), c["signature"], "{call}: RFC 6979 signature");
        assert_eq!(hyperliquid::exchange_body(&action, nonce, &sig), c["body"].as_str().unwrap(), "{call}: the /exchange body");
        assert_eq!(c["path"], "/exchange");
    }
    // The same-ms pair: honeymaker's process-wide counter gave the cancel the order's nonce + 1.
    let pair: Vec<u64> = cases.iter().filter(|c| c["call"].as_str().unwrap().starts_with("same ms")).map(|c| c["nonce"].as_u64().unwrap()).collect();
    assert_eq!(pair[1], pair[0] + 1);
    let n = Nonce::new();
    assert_eq!([n.next_at(pair[0]), n.next_at(pair[0])], [pair[0], pair[1]]);
}

#[test]
fn float_to_wire_matches_hyperliquid_rb_on_every_recorded_edge() {
    let f = fixture("hyperliquid");
    for c in f["float_to_wire"].as_array().unwrap() {
        let x = c["x"].as_str().unwrap();
        match (hyperliquid::float_to_wire(&bd(x)), c["wire"].as_str()) {
            (Ok(got), Some(want)) => assert_eq!(got, want, "{x}"),
            (Err(_), None) => assert!(c["error"].as_str().unwrap().contains("float_to_wire causes rounding"), "{x}"),
            (got, _) => panic!("{x}: {got:?}, Ruby {c}"),
        }
    }
}

#[test]
fn an_unreadable_agent_key_signs_nothing() {
    assert!(hyperliquid::signing_key("0xzz").is_none());
    assert!(hyperliquid::signing_key(&"00".repeat(32)).is_none(), "zero is not a secp256k1 key");
    assert!(hyperliquid::signing_key(&"11".repeat(32)).is_some());
}
