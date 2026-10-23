//! Slice S (PLAN-VB §1.2): the Gemini, Coinbase and Hyperliquid signers against the requests honeymaker 0.12.5 and
//! hyperliquid-rb 0.1.1 sent under frozen clocks, nonces and throwaway keys (script/rust/record_venue_signing.rb); and IBKR's
//! OAuth 1.0a + live session token (slice I0) against Clients::Ibkr::Oauth/Session and oauth_test.rb's Python vectors.
use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use deltabadger::ruby::BigDec;
use deltabadger::venue::signers::{coinbase, gemini, hyperliquid, ibkr, Nonce};
use hyperliquid::{Action, OrderWire};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(format!("{name}_signing.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn the_fixtures_are_recorded_from_the_current_recorder() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for name in ["gemini", "coinbase", "hyperliquid", "ibkr"] {
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

// ---- IBKR: OAuth 1.0a, the live session token, and the connect wizard's keys

fn ibkr_material(f: &Value, dh_param: &str) -> ibkr::Material {
    let k = &f["keys"];
    let s = |n: &str| k[n].as_str().unwrap().to_string();
    ibkr::Material { consumer_key: s("consumer_key"), access_token: s("access_token"), access_token_secret: s("access_token_secret"),
                     signature_key: s("signature_key"), encryption_key: s("encryption_key"), dh_param: dh_param.into(), realm: s("realm") }
}

#[test]
fn ibkr_helpers_match_rails_on_every_recorded_edge() {
    let f = fixture("ibkr");
    for (input, want) in f["escape"].as_object().unwrap() { assert_eq!(ibkr::escape(input), want.as_str().unwrap(), "escape {input:?}") }
    for (n, want) in f["to_byte_array"].as_object().unwrap() {
        let got = ibkr::to_byte_array(&num_bigint::BigUint::parse_bytes(n.as_bytes(), 16).unwrap());
        assert_eq!(serde_json::json!(got), *want, "to_byte_array 0x{n}");
    }
    for c in f["base_string"].as_array().unwrap() {
        let params = c["params"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        assert_eq!(ibkr::base_string(c["method"].as_str().unwrap(), c["url"].as_str().unwrap(), &params, c["prepend"].as_str()), c["base_string"].as_str().unwrap());
    }
    for c in f["hmac_sha256"].as_array().unwrap() {
        assert_eq!(ibkr::hmac_sha256_signature(c["base_string"].as_str().unwrap(), c["lst"].as_str().unwrap()).unwrap(), c["signature"].as_str().unwrap());
    }
    let consumer = f["keys"]["consumer_key"].as_str().unwrap();
    for c in f["lst"].as_array().unwrap() {
        let s = |n: &str| c[n].as_str().unwrap();
        assert_eq!(ibkr::live_session_token(s("dh_prime"), s("dh_random"), s("dh_response"), s("prepend")).unwrap(), s("lst"));
        assert!(ibkr::validate_live_session_token(s("lst"), s("validation"), consumer));
        assert!(!ibkr::validate_live_session_token(s("lst"), &s("validation")[1..], consumer));
    }
}

/// oauth_test.rb's vectors, computed in Python against IBKR's reference (ibind oauth1a.py): a second, independent oracle.
#[test]
fn ibkr_matches_the_python_reference_vectors() {
    assert_eq!(ibkr::escape("a b~c/d"), "a+b~c%2Fd");
    assert_eq!(ibkr::escape("AAPL=100&x_y.z~"), "AAPL%3D100%26x_y.z~");
    assert_eq!(ibkr::escape("https://api.ibkr.com/v1/api"), "https%3A%2F%2Fapi.ibkr.com%2Fv1%2Fapi");
    let params = [("oauth_nonce", "abc"), ("oauth_consumer_key", "TESTCONS")].map(|(k, v)| (k.to_string(), v.to_string())).into();
    let base = ibkr::base_string("GET", "https://api.ibkr.com/v1/api/portfolio/accounts", &params, None);
    assert_eq!(base, "GET&https%3A%2F%2Fapi.ibkr.com%2Fv1%2Fapi%2Fportfolio%2Faccounts&oauth_consumer_key%3DTESTCONS%26oauth_nonce%3Dabc");
    assert_eq!(ibkr::hmac_sha256_signature(&base, "bGl2ZXNlc3Npb250b2tlbg==").unwrap(), "FuskAI5IUwEzj3ciHIRcKn%2FGmm%2B9daeFu2ip2V96bUU%3D");
    let prime = "00cc8f1bbe20a6993bb1e2d89f0f1b2b6f8b9b5d3a7c1e4f6079123456789abcd";
    assert_eq!(ibkr::live_session_token(prime, "3a7f9c2d1e", "6dcbd0d9d2a0a2860b9df30955d152a0776c9ac29bfa62c5900e46abd98717c", "deadbeef0011").unwrap(),
               "9stTxQv6fGt6TDQLWyVnlKn2M7o=");
    assert!(ibkr::validate_live_session_token("bGl2ZXNlc3Npb250b2tlbg==", "465b8c3dd799d2f14ac350df1bcdf6c552289341", "TESTCONS"));
    assert!(!ibkr::validate_live_session_token("bGl2ZXNlc3Npb250b2tlbg==", "deadbeef", "TESTCONS"));
    let p = num_bigint::BigUint::parse_bytes(prime.as_bytes(), 16).unwrap();
    let challenge = num_bigint::BigUint::from(2u8).modpow(&num_bigint::BigUint::parse_bytes(b"3a7f9c2d1e", 16).unwrap(), &p);
    assert_eq!(challenge.to_str_radix(16), "2b66064b7cfdf414ab0d8ff44103e42b46637c7402d69bd04b2dc2d7da8a9aa");
    let params = [("oauth_token", "tok"), ("oauth_consumer_key", "TESTCONS"), ("oauth_signature_method", "HMAC-SHA256"), ("oauth_nonce", "abc"),
                  ("oauth_timestamp", "1700000000"), ("oauth_signature", "SIG")].map(|(k, v)| (k.to_string(), v.to_string())).into();
    assert_eq!(ibkr::authorization_header("limited_poa", &params),
               "OAuth realm=\"limited_poa\", oauth_consumer_key=\"TESTCONS\", oauth_nonce=\"abc\", oauth_signature=\"SIG\", \
                oauth_signature_method=\"HMAC-SHA256\", oauth_timestamp=\"1700000000\", oauth_token=\"tok\"");
}

#[test]
fn ibkr_oauth_builders_are_byte_equal_to_rails() {
    let f = fixture("ibkr");
    let signer = ibkr::Signer::new(&ibkr_material(&f, f["signing"][0]["dh_param"].as_str().unwrap())).unwrap();
    assert_eq!(signer.prepend().unwrap(), f["prepend"].as_str().unwrap(), "the decrypted secret keeps its leading 0x00");
    assert_eq!(f["prepend"], f["keys"]["access_token_secret_plaintext"]);
    let headers = f["headers"].as_array().unwrap();
    assert_eq!(headers.len(), 4);
    let ts = "1789871400";
    for c in headers {
        let s = |n: &str| c[n].as_str().unwrap();
        let got = if s("call") == "live_session_token_header" {
            signer.live_session_token_header(s("url"), s("dh_challenge"), s("prepend"), s("nonce"), ts)
        } else {
            let query: Vec<(String, String)> = c["query"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
            signer.signed_header(s("method"), s("url"), s("lst"), &query, s("nonce"), ts).unwrap()
        };
        assert_eq!(got, s("header"), "{} {}", s("call"), c["method"]);
    }
}

/// The exit criterion of slice I0: with Rails' frozen nonces, timestamp and DH random, the scripted server's DH answer and
/// the fixed keys, the LST and the Authorization header of every request Clients::Ibkr::Session sent are reproduced.
#[test]
fn ibkr_session_requests_and_the_lst_are_byte_equal_to_rails() {
    let f = fixture("ibkr");
    let sessions = f["signing"].as_array().unwrap();
    assert_eq!(sessions.iter().map(|s| s["label"].as_str().unwrap()).collect::<Vec<_>>(), ["session_test dh", "ffdhe2048"]);
    assert_eq!(sessions[1]["dh_param"].as_str().unwrap(), ibkr::ffdhe2048_pem(), "D-IBKR-DHGROUP: the PEM Rails mints with");
    for s in sessions {
        let label = s["label"].as_str().unwrap();
        let str_ = |n: &str| s[n].as_str().unwrap();
        let signer = ibkr::Signer::new(&ibkr_material(&f, str_("dh_param"))).unwrap();
        assert_eq!(signer.dh_prime.to_str_radix(16), str_("dh_prime"), "{label}");
        assert_eq!(serde_json::json!(signer.dh_generator.to_u32_digits()[0]), s["dh_generator"]);
        let prepend = signer.prepend().unwrap();
        assert_eq!(prepend, str_("prepend"));
        let challenge = signer.dh_challenge(str_("dh_random")).unwrap();
        assert_eq!(challenge, str_("dh_challenge"), "{label}");
        let server = &s["server"];
        let lst = signer.live_session_token(str_("dh_random"), server["diffie_hellman_response"].as_str().unwrap(), &prepend).unwrap();
        assert_eq!(lst, server["lst"].as_str().unwrap(), "{label}: the live session token");
        assert!(ibkr::validate_live_session_token(&lst, server["live_session_token_signature"].as_str().unwrap(), signer.consumer_key()));

        let requests = s["requests"].as_array().unwrap();
        let nonces = s["nonces"].as_array().unwrap();
        assert_eq!(requests.len(), 5);
        for (i, (r, nonce)) in requests.iter().zip(nonces).enumerate() {
            let (method, path, nonce) = (r["method"].as_str().unwrap(), r["path"].as_str().unwrap(), nonce.as_str().unwrap());
            let url = format!("https://api.ibkr.com{path}");
            let got = if i == 0 {
                assert_eq!(path, "/v1/api/oauth/live_session_token");
                signer.live_session_token_header(&url, &challenge, &prepend, nonce, str_("timestamp"))
            } else {
                let query: Vec<(String, String)> = form_urlencoded::parse(r["query"].as_str().unwrap_or("").as_bytes()).into_owned().collect();
                signer.signed_header(method, &url, &lst, &query, nonce, str_("timestamp")).unwrap()
            };
            assert_eq!(got, r["authorization"].as_str().unwrap(), "{label}: {method} {path}");
        }
    }
}

#[test]
fn ibkr_refuses_unreadable_key_material() {
    let f = fixture("ibkr");
    let good = ibkr_material(&f, f["keys"]["dh_param"].as_str().unwrap());
    let with = |edit: &dyn Fn(&mut ibkr::Material)| { let mut m = good.clone(); edit(&mut m); ibkr::Signer::new(&m).err() };
    assert_eq!(with(&|_| ()), None);
    assert_eq!(with(&|m| m.signature_key = "not a key".into()), Some(ibkr::Error::SignatureKey));
    assert_eq!(with(&|m| m.encryption_key = m.dh_param.clone()), Some(ibkr::Error::EncryptionKey));
    assert_eq!(with(&|m| m.dh_param = m.signature_key.clone()), Some(ibkr::Error::DhParam));
    let mut blank_realm = good.clone();
    blank_realm.realm = String::new();
    let header = ibkr::Signer::new(&blank_realm).unwrap().signed_header("GET", "https://api.ibkr.com/x", "AAAA", &[], "n", "1").unwrap();
    assert!(header.starts_with("OAuth realm=\"limited_poa\", "), "Session's `ibkr_realm.presence || 'limited_poa'`");
    let mut bad_secret = good;
    bad_secret.access_token_secret = B64.encode([1u8; 256]);
    assert_eq!(ibkr::Signer::new(&bad_secret).unwrap().prepend(), None, "a secret that does not decrypt");
}

/// The connect wizard's keys (I3) as Ruby's OpenSSL reads them: Rust's PKCS#8 private PEM and SPKI public PEM load in
/// `OpenSSL::PKey::RSA.new` and agree, ffdhe2048 loads in `OpenSSL::PKey::DH.new` with Rails' own p and g, and the key
/// Ruby writes back (`to_pem`, PKCS#1, what Rails stores) signs in Rust byte-equal to Ruby's signature.
#[test]
fn ibkr_generated_keys_round_trip_through_ruby_openssl() {
    let (private, public) = ibkr::generate_rsa();
    assert!(private.starts_with("-----BEGIN PRIVATE KEY-----\n") && public.starts_with("-----BEGIN PUBLIC KEY-----\n"));
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [("private.pem", private.as_str()), ("public.pem", public.as_str()), ("dh.pem", &ibkr::ffdhe2048_pem())] {
        std::fs::write(dir.path().join(name), body).unwrap();
    }
    const RUBY: &str = r##"
        d = ARGV.fetch(0)
        key = OpenSSL::PKey::RSA.new(File.read("#{d}/private.pem"))
        pub = OpenSSL::PKey::RSA.new(File.read("#{d}/public.pem"))
        dh = OpenSSL::PKey::DH.new(File.read("#{d}/dh.pem"))
        msg = "POST&https%3A%2F%2Fapi.ibkr.com&x"
        print JSON.generate(private: key.private?, bits: key.n.num_bits, public_matches: key.public_key.to_der == pub.to_der,
                            public_pem: key.public_key.to_pem, pkcs1_pem: key.to_pem, dh_p: dh.p.to_s(16).downcase, dh_g: dh.g.to_i,
                            dh_pem: dh.to_pem, signature: Base64.strict_encode64(key.sign(OpenSSL::Digest.new("SHA256"), msg)))
    "##;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let out = std::process::Command::new("ruby").current_dir(&root).args(["-ropenssl", "-rjson", "-rbase64", "-e", RUBY])
        .arg(dir.path()).output().expect("ruby runs");
    assert!(out.status.success(), "ruby: {}", String::from_utf8_lossy(&out.stderr));
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((r["private"].as_bool(), r["bits"].as_u64(), r["public_matches"].as_bool()), (Some(true), Some(2048), Some(true)));
    assert_eq!(r["public_pem"].as_str().unwrap(), public, "Ruby's public_key.to_pem, byte for byte");
    assert_eq!((r["dh_p"].as_str().unwrap(), r["dh_g"].as_u64()), (ibkr::FFDHE2048_P, Some(2)));
    assert_eq!(r["dh_pem"].as_str().unwrap(), ibkr::ffdhe2048_pem(), "Ruby's DH#to_pem, byte for byte");
    let pkcs1 = r["pkcs1_pem"].as_str().unwrap();
    assert!(pkcs1.starts_with("-----BEGIN RSA PRIVATE KEY-----"));
    let dh = ibkr::ffdhe2048_pem();
    let material = |key: &str| ibkr::Material { consumer_key: "C".into(), access_token: "t".into(), access_token_secret: String::new(),
                                                signature_key: key.into(), encryption_key: key.into(), dh_param: dh.clone(), realm: String::new() };
    let msg = "POST&https%3A%2F%2Fapi.ibkr.com&x";
    let want = ibkr::escape(r["signature"].as_str().unwrap());
    assert_eq!(ibkr::Signer::new(&material(pkcs1)).unwrap().rsa_sha256_signature(msg), want, "Ruby's PKCS#1 PEM back in Rust");
    assert_eq!(ibkr::Signer::new(&material(&private)).unwrap().rsa_sha256_signature(msg), want, "Rust's own PKCS#8 PEM");
}
