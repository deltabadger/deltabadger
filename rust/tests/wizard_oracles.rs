//! The wizard's two recorded oracles that are not pages: how Rails judges a Kraken or Binance key (key_verdicts.json, every
//! case replayed through web::settings::key_check) and which callbacks Rails runs when the wizard saves a bot
//! (bot_create_callbacks.json and index_create_callbacks.json, each one named in web::wizard::create::CALLBACKS and
//! web::wizard::index_create::CALLBACKS).
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
    let kucoin = &recorded["kucoin"];
    let listed = |name: &str| -> Vec<String> { serde_json::from_value(kucoin[name].clone()).unwrap() };
    assert_eq!(listed("rejected_codes"), key_check::KUCOIN_REJECTED);
    assert_eq!(listed("inconclusive_codes"), key_check::KUCOIN_INCONCLUSIVE);
    let invalid: Vec<String> = deltabadger::engine::venue_rules::KUCOIN.kinds.iter().find(|(k, _)| *k == "invalid_key").unwrap().1.iter().map(|s| s.to_string()).collect();
    assert_eq!(listed("invalid_key_errors"), invalid);
    for case in kucoin["cases"].as_array().unwrap() {
        let (name, kind) = (case["case"].as_str().unwrap(), kind(case["key_type"].as_str().unwrap()));
        let request = key_check::kucoin_request(deltabadger::venue::kucoin::URL, kind);
        let ours = if kind == 2 { key_check::kucoin_read_verdict(&answer(&case["response"]), request.method, request.url().as_str()) }
                   else { key_check::kucoin_verdict(kind, &answer(&case["response"]), request.method, request.url().as_str()) };
        // A reading key's balances #get_balances cannot walk raise in Rails (the form answers 500, the queued job is discarded
        // and the key stays pending): no verdict, as an inconclusive answer.
        let raised = kind == 2 && case["verdict"]["kind"] == "raises";
        let theirs = if raised { Verdict::Failure(String::new()) } else { expected(&case["verdict"]) };
        assert!(same(&ours, &theirs), "kucoin {} {name}: {ours:?}, Rails {theirs:?}", case["key_type"]);
        let query: serde_json::Map<String, Value> = request.query.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
        assert_eq!((request.method, request.url().host_str().unwrap(), request.path.as_str(), Value::Object(query), json!({})),
                   (case["request"]["method"].as_str().unwrap(), case["request"]["host"].as_str().unwrap(), case["request"]["path"].as_str().unwrap(),
                    case["request"]["query"].clone(), case["request"]["form"].clone()), "kucoin {name}");
        let headers = deltabadger::venue::kucoin::headers(&request, "test-key", "test-secret", "test-passphrase", 1);
        assert!(headers.iter().any(|(h, v)| h == "KC-API-KEY" && json!(v) == case["request"]["key_header"]), "kucoin {name}: the key header");
        replayed += 1;
    }
    // Gemini (plan VB B3): the cancel probe alone for a trading key, a balances read otherwise; the request's params are
    // the signed payload's (recorded as its form).
    let gemini = &recorded["gemini"];
    let invalid: Vec<String> = deltabadger::engine::venue_rules::GEMINI.kinds.iter().find(|(k, _)| *k == "invalid_key").unwrap().1.iter().map(|s| s.to_string()).collect();
    assert_eq!(serde_json::from_value::<Vec<String>>(gemini["invalid_key_errors"].clone()).unwrap(), invalid);
    for case in gemini["cases"].as_array().unwrap() {
        let (name, kind) = (case["case"].as_str().unwrap(), kind(case["key_type"].as_str().unwrap()));
        let request = key_check::gemini_request(deltabadger::venue::gemini::URL, kind);
        let url = request.url();
        let ours = if kind == 2 { key_check::gemini_read_verdict(&answer(&case["response"]), url.as_str()) }
                   else { key_check::gemini_verdict(kind, &answer(&case["response"]), url.as_str()) };
        // Rails raises on balances #get_balances cannot walk (the key stays pending): no verdict, as an inconclusive answer. A
        // 200 that Faraday leaves unparsed (text/plain "OK") is a valid key to Rails, which reads no `result: error` in it;
        // Rust sees no content type and reads a 2xx that is not JSON as no verdict (D-GEMINI-PLAIN-200: never valid on a
        // body Gemini does not send).
        let lenient = name == "plain_text_200" && kind != 2;
        let theirs = if case["verdict"]["kind"] == "raises" || lenient { Verdict::Failure(String::new()) } else { expected(&case["verdict"]) };
        if lenient { assert_eq!(case["verdict"]["data"], true, "gemini {name}: Rails no longer accepts it: drop the listed divergence"); }
        assert!(same(&ours, &theirs), "gemini {} {name}: {ours:?}, Rails {theirs:?}", case["key_type"]);
        assert_eq!((request.method, url.host_str().unwrap(), request.path.as_str(), json!({}), request.body.clone().unwrap()),
                   (case["request"]["method"].as_str().unwrap(), case["request"]["host"].as_str().unwrap(), case["request"]["path"].as_str().unwrap(),
                    case["request"]["query"].clone(), case["request"]["form"].clone()), "gemini {name}");
        let headers = deltabadger::venue::signers::gemini::headers("test-key", "test-secret", &request.path, &[], 1);
        assert!(headers.iter().any(|(h, v)| *h == "X-GEMINI-APIKEY" && json!(v) == case["request"]["key_header"]), "gemini {name}: the key header");
        replayed += 1;
    }
    // Binance.US (plan VB slice A2): the account read, then the cancel probe, each case's whole request sequence replayed
    // through the check itself over Rails' recorded answers.
    let us = &recorded["binance_us"];
    assert_eq!(serde_json::from_value::<Vec<i64>>(us["invalid_key_codes"].clone()).unwrap(), key_check::BINANCE_INVALID_KEY);
    assert_eq!(json!({ "symbol": key_check::BINANCE_US_PROBE[0].1, "order_id": key_check::BINANCE_US_PROBE[1].1 }), us["trade_probe"]);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for case in us["cases"].as_array().unwrap() {
        let (name, kind) = (case["case"].as_str().unwrap(), kind(case["key_type"].as_str().unwrap()));
        let reading = case["method"] == "get_read_api_key_validity";
        let answers: Vec<Value> = case["responses"].as_array().cloned().unwrap_or_else(|| vec![case["response"].clone()]);
        let sent: Vec<Value> = case["requests"].as_array().cloned().unwrap_or_else(|| vec![case["request"].clone()]);
        let reply = |a: &Value| if a["timeout"] == true { json!({ "network": "post_send", "message": "Net::ReadTimeout" }) }
                                else if a["refused"] == true { json!({ "network": "pre_send", "message": "Connection refused" }) }
                                else { json!({ "status": a["status"], "body": a["body"] }) };
        let mut script = serde_json::Map::new();
        for (request, answer) in sent.iter().zip(&answers) {
            script.insert(format!("{} {}", request["method"].as_str().unwrap(), request["path"].as_str().unwrap()), json!([reply(answer)]));
        }
        let transport = deltabadger::venue::http::ScriptedTransport::from_script(&Value::Object(script));
        let venue = deltabadger::venue::binance::BinanceVenue::new(transport.clone(), deltabadger::venue::binance::US_URL, "test-secret");
        let (_, ours) = rt.block_on(key_check::binance_us_check(&transport, &venue, kind, reading));
        let theirs = expected(&case["verdict"]);
        assert!(same(&ours, &theirs), "binance_us {} {} {name}: {ours:?}, Rails {theirs:?}", case["key_type"], case["method"]);
        let ours_sent: Vec<Value> = transport.requests().iter().map(|r| {
            let query: serde_json::Map<String, Value> = r.query.iter().filter(|(k, _)| !["signature", "timestamp"].contains(k)).map(|(k, v)| (k.to_string(), json!(v))).collect();
            json!([r.method, r.url().host_str().unwrap(), r.path, query])
        }).collect();
        let theirs_sent: Vec<Value> = sent.iter().map(|r| json!([r["method"], r["host"], r["path"], r["query"]])).collect();
        assert_eq!(ours_sent, theirs_sent, "binance_us {} {} {name}: the request sequence", case["key_type"], case["method"]);
        assert!(sent.iter().all(|r| r["key_header"] == "test-key" && r["form"] == json!({})), "{name}");
        replayed += 1;
    }
    // Bitvavo (plan VB slice A3): the balance read, then (a key that is not a withdrawal key) the cancel probe, each case's
    // whole request sequence replayed through the check over Rails' recorded answers. Listed divergence D-BITVAVO-KEY-TEXT:
    // a 2xx body that is no JSON passes Rails' check when its content type is not JSON (Faraday leaves it a String, with no
    // errorCode); Rust's transport does not carry the content type and Bitvavo answers JSON, so Rust gives no verdict on it.
    let bv = &recorded["bitvavo"];
    assert_eq!((bv["refused_codes"].clone(), bv["not_found_code"].clone()), (json!(key_check::BITVAVO_REFUSED), json!(deltabadger::venue::bitvavo::ORDER_NOT_FOUND)));
    let (mut bitvavo, mut text_passes) = (0, 0);
    for case in bv["cases"].as_array().unwrap() {
        let (name, kind) = (case["case"].as_str().unwrap(), kind(case["key_type"].as_str().unwrap()));
        let answers: Vec<Value> = case["responses"].as_array().cloned().unwrap_or_else(|| vec![case["response"].clone()]);
        let sent: Vec<Value> = case["requests"].as_array().cloned().unwrap_or_else(|| vec![case["request"].clone()]);
        let reply = |a: &Value| if a["timeout"] == true { json!({ "network": "post_send", "message": "Net::ReadTimeout" }) }
                                else if a["refused"] == true { json!({ "network": "pre_send", "message": "Connection refused" }) }
                                else { json!({ "status": a["status"], "body": a["body"] }) };
        let mut script = serde_json::Map::new();
        for (request, answer) in sent.iter().zip(&answers) {
            script.insert(format!("{} {}", request["method"].as_str().unwrap(), request["path"].as_str().unwrap()), json!([reply(answer)]));
        }
        let transport = deltabadger::venue::http::ScriptedTransport::from_script(&Value::Object(script));
        let venue = deltabadger::venue::bitvavo::BitvavoVenue::new(transport.clone(), deltabadger::venue::bitvavo::URL);
        let (_, ours) = rt.block_on(key_check::bitvavo_check(&transport, &venue, kind));
        let theirs = expected(&case["verdict"]);
        let ours_sent: Vec<Value> = transport.requests().iter().map(|r| {
            let query: serde_json::Map<String, Value> = r.query.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
            json!([r.method, r.url().host_str().unwrap(), r.path, query])
        }).collect();
        let theirs_sent: Vec<Value> = sent.iter().map(|r| json!([r["method"], r["host"], r["path"], r["query"]])).collect();
        if name.contains("plain_text_200") {
            assert!(theirs == Verdict::Valid(true) && matches!(&ours, Verdict::Failure(t) if t == "OK"), "D-BITVAVO-KEY-TEXT {name}: {ours:?}, Rails {theirs:?}");
            assert_eq!(ours_sent[..], theirs_sent[..ours_sent.len()], "{name}");
            text_passes += 1;
        } else {
            assert!(same(&ours, &theirs), "bitvavo {} {} {name}: {ours:?}, Rails {theirs:?}", case["key_type"], case["method"]);
            assert_eq!(ours_sent, theirs_sent, "bitvavo {} {} {name}: the request sequence", case["key_type"], case["method"]);
        }
        assert!(sent.iter().all(|r| r["key_header"] == "test-key" && r["form"] == json!({})), "{name}");
        bitvavo += 1;
    }
    assert_eq!((bitvavo, text_passes), (214, 7), "a recorded Bitvavo case was added or dropped");
    replayed += bitvavo;
    // MEXC (plan VB slice A5): the account read and its canTrade, then (a reading key's #get_read_api_key_validity) the deposit
    // history, each case's whole request sequence replayed through the check over Rails' recorded answers.
    let mx = &recorded["mexc"];
    assert_eq!(mx["rejected_codes"], json!(key_check::MEXC_REJECTED));
    let invalid: Vec<String> = deltabadger::engine::venue_rules::MEXC.kinds.iter().find(|(k, _)| *k == "invalid_key").unwrap().1.iter().map(|s| s.to_string()).collect();
    assert_eq!(serde_json::from_value::<Vec<String>>(mx["invalid_key_errors"].clone()).unwrap(), invalid);
    let mut mexc = 0;
    for case in mx["cases"].as_array().unwrap() {
        let (name, kind) = (case["case"].as_str().unwrap(), kind(case["key_type"].as_str().unwrap()));
        let reading = case["method"] == "get_read_api_key_validity";
        let answers: Vec<Value> = case["responses"].as_array().cloned().unwrap_or_else(|| vec![case["response"].clone()]);
        let sent: Vec<Value> = case["requests"].as_array().cloned().unwrap_or_else(|| vec![case["request"].clone()]);
        let reply = |a: &Value| if a["timeout"] == true { json!({ "network": "post_send", "message": "Net::ReadTimeout" }) }
                                else if a["refused"] == true { json!({ "network": "pre_send", "message": "Connection refused" }) }
                                else { json!({ "status": a["status"], "body": a["body"] }) };
        let mut script = serde_json::Map::new();
        for (request, answer) in sent.iter().zip(&answers) {
            script.insert(format!("{} {}", request["method"].as_str().unwrap(), request["path"].as_str().unwrap()), json!([reply(answer)]));
        }
        let transport = deltabadger::venue::http::ScriptedTransport::from_script(&Value::Object(script));
        let venue = deltabadger::venue::mexc::MexcVenue::new(transport.clone(), deltabadger::venue::mexc::URL, "test-secret");
        let (_, ours) = rt.block_on(key_check::mexc_check(&transport, &venue, kind, reading));
        let theirs = expected(&case["verdict"]);
        // Listed divergence D-MEXC-KEY-TEXT (as Bitvavo's): a deposit history answered 2xx with a body that is no JSON passes
        // Rails' reading check when its content type is not JSON (Faraday leaves it a String, with no code); Rust's transport
        // does not carry the content type and MEXC answers JSON, so Rust gives no verdict on it.
        if name.ends_with("deposits plain_text_200") {
            assert!(theirs == Verdict::Valid(true) && matches!(&ours, Verdict::Failure(t) if t == "OK"), "D-MEXC-KEY-TEXT {name}: {ours:?}, Rails {theirs:?}");
            text_passes += 1;
        } else {
            assert!(same(&ours, &theirs), "mexc {} {} {name}: {ours:?}, Rails {theirs:?}", case["key_type"], case["method"]);
        }
        let ours_sent: Vec<Value> = transport.requests().iter().map(|r| {
            let query: serde_json::Map<String, Value> = r.query.iter().filter(|(k, _)| !["signature", "timestamp"].contains(k)).map(|(k, v)| (k.to_string(), json!(v))).collect();
            json!([r.method, r.url().host_str().unwrap(), r.path, query])
        }).collect();
        let theirs_sent: Vec<Value> = sent.iter().map(|r| json!([r["method"], r["host"], r["path"], r["query"]])).collect();
        assert_eq!(ours_sent, theirs_sent, "mexc {} {} {name}: the request sequence", case["key_type"], case["method"]);
        assert!(sent.iter().all(|r| r["key_header"] == "test-key" && r["form"] == json!({})), "{name}");
        mexc += 1;
    }
    assert_eq!((mexc, text_passes - 7), (240, 4), "a recorded MEXC case was added or dropped");
    replayed += mexc;
    assert_eq!(replayed, 578 + 84 + 212 + 214 + 240, "a recorded case was added or dropped");
}

/// A pinned callback list against a port's table: the same callbacks, in Rails' order, each handled.
fn assert_handled(fixture_name: &str, table: &[(&str, &str, &str, &str, &str)]) {
    assert_pinned(fixture_name);
    let recorded = fixture(fixture_name);
    let mut theirs = vec![];
    for (chain, callbacks) in recorded["callbacks"].as_object().unwrap() {
        for callback in callbacks.as_array().unwrap() {
            let text = |list: &Value, prefix: &str| list.as_array().unwrap().iter().map(|c| format!("{prefix}{}", c.as_str().unwrap())).collect::<Vec<_>>();
            let conditions = [text(&callback["if"], ""), text(&callback["unless"], "unless ")].concat().join(" & ");
            theirs.push((chain.clone(), callback["kind"].as_str().unwrap().to_string(), callback["callback"].as_str().unwrap().to_string(), conditions));
        }
    }
    let ours: Vec<(String, String, String, String)> = table.iter()
        .map(|(chain, kind, callback, conditions, _)| (chain.to_string(), kind.to_string(), callback.to_string(), conditions.to_string())).collect();
    for (index, (a, b)) in theirs.iter().zip(&ours).enumerate() { assert_eq!(a, b, "{fixture_name} callback {index}: Rails changed it; port it or name why it does nothing on create"); }
    assert_eq!(theirs.len(), ours.len(), "{fixture_name}: Rails added or removed a callback");
    assert!(table.iter().all(|(.., handling)| handling.starts_with("ported") || handling.starts_with("no-op") || handling.starts_with("passes")));
}

#[test]
fn bot_create_callbacks_are_all_handled() {
    assert_handled("bot_create_callbacks.json", deltabadger::web::wizard::create::CALLBACKS);
}

#[test]
fn index_create_callbacks_are_all_handled() {
    assert_eq!(fixture("index_create_callbacks.json")["class"], "Bots::DcaIndex");
    assert_handled("index_create_callbacks.json", deltabadger::web::wizard::index_create::CALLBACKS);
}

/// The live key check of a Binance.US key, as the wizard and settings send it: the signed account read, then the probe, both
/// to the venue's host with the key in X-MBX-APIKEY; a read-only key is judged by the read alone.
#[tokio::test(flavor = "current_thread")]
async fn a_binance_us_key_is_checked_by_a_read_and_a_probe() {
    use wiremock::{matchers::{header, method, path}, Mock, MockServer, ResponseTemplate};
    use deltabadger::web::settings::validator::Validity;
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/api/v3/account")).and(header("X-MBX-APIKEY", "us-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "canTrade": true, "balances": [] }))).mount(&server).await;
    Mock::given(method("DELETE")).and(path("/api/v3/order")).and(header("X-MBX-APIKEY", "us-key"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "code": -2011, "msg": "Unknown order sent." }))).mount(&server).await;
    let credentials = deltabadger::crypto::Credentials { key: "us-key".into(), secret: "us-secret".into(), passphrase: None, redaction_values: vec![] };
    let base = server.uri();
    let check = |kind| key_check::check("Exchanges::BinanceUs", &base, deltabadger::venue::http::client(), &credentials, kind);
    assert!(matches!(check(0).await, Validity::Correct), "a trading key that reads and may trade");
    assert!(matches!(check(1).await, Validity::Incorrect), "a withdrawal key that may trade");
    assert!(matches!(check(2).await, Validity::Correct), "a reading key that reads");
    let sent: Vec<(String, String)> = server.received_requests().await.unwrap().iter().map(|r| (r.method.to_string(), r.url.path().to_string())).collect();
    let (read, probe) = (("GET".to_string(), "/api/v3/account".to_string()), ("DELETE".to_string(), "/api/v3/order".to_string()));
    assert_eq!(sent, [read.clone(), probe.clone(), read.clone(), probe, read]);
}

/// The live key check of a Bitvavo key, as the wizard and settings send it: the signed balance read, then the cancel probe
/// (240: past the permission check) for a trading key and a reading key alike, both signed with the key in
/// BITVAVO-ACCESS-KEY; a withdrawal key is judged by the read alone.
#[tokio::test(flavor = "current_thread")]
async fn a_bitvavo_key_is_checked_by_a_balance_read_and_a_probe() {
    use wiremock::{matchers::{header, header_exists, method, path, query_param}, Mock, MockServer, ResponseTemplate};
    use deltabadger::web::settings::validator::Validity;
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/balance")).and(header("BITVAVO-ACCESS-KEY", "bv-key")).and(header_exists("BITVAVO-ACCESS-SIGNATURE"))
        .and(header("BITVAVO-ACCESS-WINDOW", "10000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "symbol": "EUR", "available": "1", "inOrder": "0" }]))).mount(&server).await;
    Mock::given(method("DELETE")).and(path("/v2/order")).and(query_param("market", "BTC-EUR")).and(query_param("operatorId", "1")).and(header("BITVAVO-ACCESS-KEY", "bv-key"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "errorCode": 240, "error": "No order found." }))).mount(&server).await;
    let credentials = deltabadger::crypto::Credentials { key: "bv-key".into(), secret: "bv-secret".into(), passphrase: None, redaction_values: vec![] };
    let base = server.uri();
    let check = |kind| key_check::check("Exchanges::Bitvavo", &base, deltabadger::venue::http::client(), &credentials, kind);
    assert!(matches!(check(0).await, Validity::Correct), "a trading key that reads and may trade");
    assert!(matches!(check(1).await, Validity::Correct), "a withdrawal key that reads");
    assert!(matches!(check(2).await, Validity::Correct), "a reading key that reads and may trade (the ledger reads trades)");
    let sent: Vec<(String, String)> = server.received_requests().await.unwrap().iter().map(|r| (r.method.to_string(), r.url.path().to_string())).collect();
    let (read, probe) = (("GET".to_string(), "/v2/balance".to_string()), ("DELETE".to_string(), "/v2/order".to_string()));
    assert_eq!(sent, [read.clone(), probe.clone(), read.clone(), read, probe]);
}

/// The live key check of a MEXC key, as the wizard and settings send it: the signed account read (the key in X-MEXC-APIKEY)
/// whose canTrade decides a trading key; a withdrawal key passes on the read; a reading key then reads the deposit history.
#[tokio::test(flavor = "current_thread")]
async fn a_mexc_key_is_checked_by_its_account_and_a_reading_key_by_its_deposits_too() {
    use wiremock::{matchers::{header, method, path, query_param}, Mock, MockServer, ResponseTemplate};
    use deltabadger::web::settings::validator::Validity;
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/api/v3/account")).and(header("X-MEXC-APIKEY", "mx-key")).and(query_param("recvWindow", "5000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "canTrade": false, "balances": [] }))).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/v3/capital/deposit/hisrec")).and(query_param("limit", "1")).and(header("X-MEXC-APIKEY", "mx-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&server).await;
    let credentials = deltabadger::crypto::Credentials { key: "mx-key".into(), secret: "mx-secret".into(), passphrase: None, redaction_values: vec![] };
    let base = server.uri();
    let check = |kind| key_check::check("Exchanges::Mexc", &base, deltabadger::venue::http::client(), &credentials, kind);
    assert!(matches!(check(0).await, Validity::Incorrect), "a trading key whose account cannot trade");
    assert!(matches!(check(1).await, Validity::Correct), "a withdrawal key that reads");
    assert!(matches!(check(2).await, Validity::Correct), "a reading key that reads the account and the deposits");
    let sent: Vec<String> = server.received_requests().await.unwrap().iter().map(|r| r.url.path().to_string()).collect();
    assert_eq!(sent, ["/api/v3/account", "/api/v3/account", "/api/v3/account", "/api/v3/capital/deposit/hisrec"]);
}
