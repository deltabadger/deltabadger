//! The wizard's recorded oracles that are not pages: how Rails judges a Kraken or Binance key (key_verdicts.json, every
//! case replayed through web::settings::key_check).
use deltabadger::venue::http::{HttpResponse, TransportError};
use deltabadger::web::settings::key_check::{self, Verdict};
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A fixture's `sources` still hash as when it was recorded (script/rust/repin re-records it otherwise).
fn assert_pinned(name: &str) {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in fixture(name)["sources"].as_object().unwrap() {
        assert_eq!(&json!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap()))), hash, "{path}: re-record {name} (script/rust/repin)");
    }
}

fn kind(key_type: &str) -> i64 { match key_type { "trading" => 0, "withdrawal" => 1, "read_only" => 2, other => panic!("key type {other}") } }

/// A recorded answer as this crate's transport hands it over.
fn answer(recorded: &Value) -> Result<HttpResponse, TransportError> {
    if recorded["timeout"] == true { return Err(TransportError::MaybeSent("Net::ReadTimeout".into())) }
    if recorded["refused"] == true { return Err(TransportError::NotSent("Connection refused".into())) }
    Ok(HttpResponse { status: recorded["status"].as_u64().unwrap() as u16, body: recorded["body"].as_str().unwrap().to_string() })
}

/// Rails' verdict as `Verdict`: a Failure's text is only logged, so a failure is compared as one.
fn expected(verdict: &Value) -> Verdict {
    match verdict["kind"].as_str().unwrap() {
        "success" => match &verdict["data"] {
            Value::Bool(valid) => Verdict::Valid(*valid),
            data => Verdict::Permissions {
                missing: serde_json::from_value(data["missing_permissions"].clone()).unwrap(),
                forbidden: serde_json::from_value(data["forbidden_permissions"].clone()).unwrap(),
            },
        },
        "failure" => Verdict::Failure(String::new()),
        other => panic!("Rails raised on a key check ({other}): fix Rails first, then re-record"),
    }
}

fn same(ours: &Verdict, theirs: &Verdict) -> bool {
    matches!((ours, theirs), (Verdict::Failure(_), Verdict::Failure(_))) || ours == theirs
}

#[test]
fn key_verdicts_replay_every_recorded_case() {
    assert_pinned("key_verdicts.json");
    let recorded = fixture("key_verdicts.json");
    let mut replayed = 0;
    for case in recorded["kraken"]["cases"].as_array().unwrap() {
        let (name, kind) = (case["case"].as_str().unwrap(), kind(case["key_type"].as_str().unwrap()));
        let ours = key_check::kraken_verdict(kind, &answer(&case["response"]));
        let theirs = expected(&case["verdict"]);
        assert!(same(&ours, &theirs), "kraken {} {name}: {ours:?}, Rails {theirs:?}", case["key_type"]);
        // The request: a signed POST with its nonce only, the key in API-Key.
        let request = key_check::kraken_request(deltabadger::venue::kraken::URL);
        assert_eq!((request.method, request.url().host_str().unwrap(), request.path.as_str()),
                   (case["request"]["method"].as_str().unwrap(), case["request"]["host"].as_str().unwrap(), case["request"]["path"].as_str().unwrap()));
        assert_eq!((case["request"]["query"].clone(), case["request"]["form"].clone()), (json!({}), json!({})), "{name}: Rails sent more than the nonce");
        let headers = deltabadger::venue::kraken::private_headers(&request.path, "nonce=1", "test-key", "dGVzdC1zZWNyZXQ=");
        assert!(headers.iter().any(|(name, value)| name == "API-Key" && json!(value) == case["request"]["key_header"]), "{name}: the key header");
        if let Some(messages) = case["permission_message"].as_object() {
            let Verdict::Permissions { missing, forbidden } = &ours else { panic!("{name}: a message without a permission problem") };
            for (locale, message) in messages {
                assert_eq!(json!(key_check::permission_message(locale, "kraken", "Kraken", kind, missing, forbidden)), *message, "kraken {name} {locale}");
            }
        } else {
            assert!(!matches!(ours, Verdict::Permissions { .. }), "{name}: Rails named no permissions");
        }
        replayed += 1;
    }
    for case in recorded["binance"]["cases"].as_array().unwrap() {
        let (name, kind) = (case["case"].as_str().unwrap(), kind(case["key_type"].as_str().unwrap()));
        let reading = case["method"] == "get_read_api_key_validity";
        let ours = key_check::binance_verdict(kind, reading, &answer(&case["response"]), "https://api.binance.com/sapi/v1/account/apiRestrictions");
        let theirs = expected(&case["verdict"]);
        assert!(same(&ours, &theirs), "binance {} {} {name}: {ours:?}, Rails {theirs:?}", case["key_type"], case["method"]);
        assert!(case.get("permission_message").is_none(), "{name}: Binance reports no permissions");
        let transport = deltabadger::venue::http::ScriptedTransport::default();
        let venue = deltabadger::venue::binance::BinanceVenue::new(transport, deltabadger::venue::binance::URL, "test-secret");
        let request = key_check::binance_request(&venue, chrono::Utc::now());
        let query: serde_json::Map<String, Value> = request.query.iter().filter(|(k, _)| !["signature", "timestamp"].contains(k)).map(|(k, v)| (k.to_string(), json!(v))).collect();
        assert_eq!((request.method, request.url().host_str().unwrap(), request.path.as_str(), Value::Object(query)),
                   (case["request"]["method"].as_str().unwrap(), case["request"]["host"].as_str().unwrap(), case["request"]["path"].as_str().unwrap(), case["request"]["query"].clone()));
        assert_eq!(case["request"]["key_header"], "test-key");
        replayed += 1;
    }
    // Binance.US checks a key with an account read and a cancel probe; its key step is a named refusal, so its cases are
    // recorded and not replayed.
    let us = recorded["binance_us"]["cases"].as_array().unwrap().len();
    assert_eq!((replayed, us), (447, 212), "a recorded case was added or dropped");
}
