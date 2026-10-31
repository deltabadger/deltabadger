//! UTA-0 (issue #841): the KuCoin UTA spot fixtures are pinned and self-consistent. No UTA behavior is connected; the
//! U1–U4 rows of fixtures/kucoin_uta/MATRIX.md drive the real adapter later.
use base64::{engine::general_purpose::STANDARD, Engine};
use deltabadger::ruby::BigDec;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kucoin_uta") }
fn fixture(name: &str) -> Value { serde_json::from_slice(&std::fs::read(root().join(format!("{name}.json"))).unwrap()).unwrap() }
fn cases(name: &str) -> Vec<Value> { fixture(name)["cases"].as_array().unwrap().clone() }
fn d(v: &Value) -> Option<BigDec> { v.as_str().and_then(|s| BigDec::parse(s).ok()) }
fn hmac(secret: &str, input: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(input.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

#[test]
fn kucoin_uta_provenance_and_generated_assets_are_pinned() {
    let m = fixture("manifest");
    assert_eq!(m["schema_version"], 1);
    assert_eq!(m["oracle"], "official-docs-and-sdk; synthetic variants are not exchange recordings");
    assert_eq!(m["sdk_revision"].as_str().unwrap().len(), 40);
    for (name, hash) in m["generated_sha256"].as_object().unwrap() {
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root().join(name)).unwrap())), hash.as_str().unwrap(), "regenerate {name}");
    }
    let recorder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../script/rust/record_kucoin_uta.py");
    assert_eq!(hex::encode(Sha256::digest(std::fs::read(recorder).unwrap())), m["recorder_sha256"]);
    for source in m["sources"].as_object().unwrap().values() {
        assert!(source["url"].as_str().unwrap().starts_with("https://"));
        assert_eq!(source["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(source["fetched_date"], "2026-10-31");
    }
    let (mut ids, mut total) = (BTreeSet::new(), 0);
    for name in ["orders", "fills", "balances", "errors", "apikey", "signing"] {
        let all = cases(name);
        for c in &all {
            assert!(ids.insert(c["id"].as_str().unwrap().to_owned()), "duplicate case {}", c["id"]);
            assert_eq!(c["provenance"]["kind"], "synthetic-variant");
            assert!(!c["provenance"]["note"].as_str().unwrap().is_empty());
            for source in c["provenance"]["sources"].as_array().unwrap() { assert!(m["sources"].get(source.as_str().unwrap()).is_some(), "{source}"); }
            assert!(c["expect"].is_object());
        }
        assert_eq!(all.len() as u64, m["case_counts"][name].as_u64().unwrap());
        total += all.len() as u64;
    }
    assert_eq!(total, m["case_counts"]["total"].as_u64().unwrap());
    // The docs' own examples stay verbatim beside the synthetic SPOT cases.
    assert!(fixture("orders")["source_examples"]["detail"].as_str().unwrap().contains("tradeType: FUTURES"));
    assert!(fixture("fills")["source_examples"]["execution"].as_str().unwrap().contains("lastId: 10000000138744534"));
}

#[test]
fn kucoin_uta_endpoint_contract_covers_every_planned_route() {
    let m = fixture("manifest");
    let e = &m["endpoints"];
    for (id, method, path) in [("mode", "GET", "/api/ua/v2/account/mode"), ("place", "POST", "/api/ua/v2/unified/order/place"),
                               ("cancel", "POST", "/api/ua/v2/unified/order/cancel"), ("detail", "GET", "/api/ua/v2/unified/order/detail"),
                               ("execution", "GET", "/api/ua/v2/unified/order/execution"), ("balance", "GET", "/api/ua/v2/unified/account/balance"),
                               ("apikey", "GET", "/api/v1/user/api-key")] {
        assert_eq!((e[id]["method"].as_str(), e[id]["path"].as_str()), (Some(method), Some(path)), "{id}");
    }
    assert_eq!(e["detail"]["retention_hours"]["cancelled_without_fills"], 2);
    assert_eq!(e["detail"]["retention_hours"]["with_fills"], 168);
    assert_eq!(e["execution"]["retention"], "UNKNOWN");
    assert_eq!(e["apikey"]["trading_permission"], "Unified");
    for c in cases("orders").iter().filter(|c| c["request"]["query"].is_array()) {
        assert_eq!(c["request"]["query"][0], serde_json::json!(["tradeType", "SPOT"]), "{}", c["id"]);
    }
}

/// The reading the plan specifies (UTA.md §3, r1.3): status 2–6 agreeing with filledSize, on our order only.
fn status_of(o: &Value) -> &'static str {
    let ours = o["tradeType"] == "SPOT" && o["orderId"] == "385577101236916224" && o["symbol"] == "BTC-USDT" && o["side"] == "BUY" && o["orderType"] == "LIMIT";
    let Some(filled) = d(&o["filledSize"]) else { return "Unreadable" };
    let some = filled > BigDec::zero();
    match (ours, o["status"].as_i64(), some) {
        (true, Some(2), false) | (true, Some(4), true) => "Open",
        (true, Some(3), true) => "Closed",
        (true, Some(5), false) | (true, Some(6), true) => "Cancelled",
        _ => "Unreadable",
    }
}

#[test]
fn kucoin_uta_status_cases_agree_with_filled_size() {
    let all: Vec<_> = cases("orders").into_iter().filter(|c| c["id"].as_str().unwrap().starts_with("status_")).collect();
    assert_eq!(all.len(), 20);
    for c in &all {
        assert_eq!(status_of(&c["response"]["data"]), c["expect"]["state"].as_str().unwrap(), "{}", c["id"]);
    }
}

/// Totals from executions (r1.3): unique by tradeId (the same id read differently is unreadable), each one this order's,
/// symbol's and side's with positive size and value; Σ size must equal filledSize (short = waiting, over = unreadable).
fn totals_of(pages: &[Value], filled: &BigDec) -> Result<(BigDec, BigDec, BTreeMap<String, BigDec>), &'static str> {
    let mut seen: BTreeMap<String, Value> = BTreeMap::new();
    for p in pages {
        for e in p["data"]["items"].as_array().unwrap() {
            let ok = e["orderId"] == "385577101236916224" && e["symbol"] == "BTC-USDT" && e["side"] == "BUY" && e["tradeType"] == "SPOT"
                && d(&e["size"]).is_some_and(|v| v > BigDec::zero()) && d(&e["value"]).is_some_and(|v| v > BigDec::zero()) && d(&e["fee"]).is_some();
            let Some(id) = e["tradeId"].as_str().filter(|_| ok) else { return Err("Unreadable") };
            if seen.get(id).is_some_and(|held| held != e) { return Err("Unreadable"); }
            seen.insert(id.into(), e.clone());
        }
    }
    let (mut base, mut quote, mut fees) = (BigDec::zero(), BigDec::zero(), BTreeMap::<String, BigDec>::new());
    for e in seen.values() {
        base = &base + &d(&e["size"]).unwrap();
        quote = &quote + &d(&e["value"]).unwrap();
        let fee = fees.entry(e["feeCurrency"].as_str().unwrap().into()).or_insert_with(BigDec::zero);
        *fee = &*fee + &d(&e["fee"]).unwrap();
    }
    if base < *filled { return Err("Waiting"); }
    if base > *filled { return Err("Unreadable"); }
    Ok((base, quote, fees))
}

#[test]
fn kucoin_uta_totals_come_from_unique_validated_executions() {
    let all: Vec<_> = cases("orders").into_iter().filter(|c| c["id"].as_str().unwrap().starts_with("totals_")).collect();
    assert_eq!(all.len(), 11);
    for c in &all {
        let got = totals_of(c["pages"].as_array().unwrap(), &d(&c["filled_size"]).unwrap());
        match c["expect"]["state"].as_str() {
            Some(state) => assert_eq!(got.err(), Some(state), "{}", c["id"]),
            None => {
                let (base, quote, fees) = got.unwrap_or_else(|e| panic!("{}: {e}", c["id"]));
                assert_eq!(base, d(&c["expect"]["base"]).unwrap(), "{}", c["id"]);
                assert_eq!(quote, d(&c["expect"]["quote"]).unwrap(), "{}", c["id"]);
                assert_eq!(fees["USDT"], d(&c["expect"]["fees"]["USDT"]).unwrap(), "{}", c["id"]);
                assert_eq!(&d(&c["expect"]["price"]).unwrap() * &base, quote, "{}", c["id"]);
            }
        }
    }
}

#[test]
fn kucoin_uta_fill_walk_is_account_wide_with_exact_cursor() {
    let walk = cases("fills").into_iter().find(|c| c["id"] == "walk_account_wide").unwrap();
    let query: Vec<&str> = walk["request"]["query"].as_array().unwrap().iter().map(|p| p[0].as_str().unwrap()).collect();
    assert!(!query.contains(&"symbol"), "account-wide: no symbol");
    let pages = walk["pages"].as_array().unwrap();
    let raw = serde_json::to_string(&walk).unwrap();
    // The cursor survives above 2^53 only when read from the JSON text, never through an f64.
    assert!(raw.contains("\"lastId\": 10000000138744534") || raw.contains("\"lastId\":10000000138744534"));
    assert!(pages.last().unwrap()["data"]["items"].as_array().unwrap().is_empty());
    let mut ids = BTreeSet::new();
    let mut read = 0;
    for p in pages {
        for e in p["data"]["items"].as_array().unwrap() {
            read += 1;
            assert!(matches!(e["side"].as_str(), Some("BUY" | "SELL")));
            ids.insert(format!("{}-{}", e["symbol"].as_str().unwrap(), e["tradeId"].as_str().unwrap()));
        }
    }
    assert_eq!((read, ids.len() as u64), (204, walk["expect"]["rows"].as_u64().unwrap()));
}

#[test]
fn kucoin_uta_balance_cases_never_spend_unpnl_or_show_debt() {
    for c in cases("balances") {
        let currencies = c["response"]["data"]["accounts"][0]["currencies"].as_array().unwrap();
        // r6.2: a debt (a liability or a negative balance) keeps that currency's previous row; an unreadable amount refuses all.
        type Row = (String, Option<(BigDec, BigDec)>);
        let read: Option<Vec<Row>> = currencies.iter().map(|x| {
            let (balance, hold, available, liability) = (d(&x["balance"])?, d(&x["hold"])?, d(&x["available"])?, d(&x["liability"])?);
            let currency = x["currency"].as_str()?.to_string();
            if balance < BigDec::zero() || liability > BigDec::zero() { return Some((currency, None)); }
            let free = &balance - &hold;
            Some((currency, Some((balance, if available < free { available } else { free }))))
        }).collect();
        match read {
            None => assert_eq!(c["expect"]["refused"], true, "{}", c["id"]),
            Some(rows) => {
                let kept: Vec<&str> = rows.iter().filter(|(_, r)| r.is_none()).map(|(c, _)| c.as_str()).collect();
                assert_eq!(serde_json::json!(kept), c["expect"]["kept"].as_array().cloned().map(Value::Array).unwrap_or(serde_json::json!([])), "{}", c["id"]);
                for (currency, (held, spendable)) in rows.into_iter().filter_map(|(c, r)| Some((c, r?))) {
                    assert_eq!(held, d(&c["expect"]["held"][&currency]).unwrap(), "{} {currency}", c["id"]);
                    assert_eq!(spendable, d(&c["expect"]["spendable"][&currency]).unwrap(), "{} {currency}", c["id"]);
                }
            }
        }
    }
}

#[test]
fn kucoin_uta_independent_hmac_vectors_cover_get_query_and_post_body() {
    let f = fixture("signing");
    let (secret, passphrase, ts) = (f["credentials"]["secret"].as_str().unwrap(), f["credentials"]["passphrase"].as_str().unwrap(), f["timestamp"].as_str().unwrap());
    let mut methods = BTreeSet::new();
    for c in f["cases"].as_array().unwrap() {
        let r = &c["request"];
        let method = r["method"].as_str().unwrap();
        methods.insert(method);
        let query = r["query"].as_array().unwrap();
        let raw = query.iter().map(|p| format!("{}={}", p[0].as_str().unwrap(), p[1].as_str().unwrap())).collect::<Vec<_>>().join("&");
        let path = r["path"].as_str().unwrap();
        let endpoint = if query.is_empty() { path.to_owned() } else { format!("{path}?{raw}") };
        let body = r["body_bytes"].as_str().unwrap();
        if method == "POST" { assert!(query.is_empty() && body.starts_with("{\"tradeType\":\"SPOT\"")); }
        let input = format!("{ts}{method}{endpoint}{body}");
        assert_eq!(input, c["expect"]["prehash"], "{}", c["id"]);
        assert_eq!(hmac(secret, &input), c["expect"]["headers"]["KC-API-SIGN"], "{}", c["id"]);
        assert_eq!(hmac(secret, passphrase), c["expect"]["headers"]["KC-API-PASSPHRASE"]);
        let mut url = reqwest::Url::parse(&format!("https://api.kucoin.com{path}")).unwrap();
        if !query.is_empty() { url.query_pairs_mut().extend_pairs(query.iter().map(|p| (p[0].as_str().unwrap(), p[1].as_str().unwrap()))); }
        assert_eq!(url.as_str(), c["expect"]["transport_url"].as_str().unwrap(), "{}", c["id"]);
        if c["id"] == "sign_get_unescaped_query" { assert_ne!(format!("https://api.kucoin.com{endpoint}"), url.as_str()); }
    }
    assert_eq!(methods, BTreeSet::from(["GET", "POST"]));
}

/// U1 (r1.10): the vectors through the real signer and the URL the transport sends: a UTA route signs its query in
/// order and unescaped, the URL encodes it, and a POST signs its body.
#[test]
fn kucoin_uta_vectors_pass_through_the_real_signer() {
    use deltabadger::venue::http::HttpRequest;
    let f = fixture("signing");
    let creds = &f["credentials"];
    let ts: i64 = f["timestamp"].as_str().unwrap().parse().unwrap();
    for c in f["cases"].as_array().unwrap() {
        let r = &c["request"];
        let query: Vec<(&'static str, String)> = r["query"].as_array().unwrap().iter()
            .map(|p| (&*Box::leak(p[0].as_str().unwrap().to_string().into_boxed_str()), p[1].as_str().unwrap().to_string())).collect();
        let body = r["body_bytes"].as_str().filter(|b| !b.is_empty()).map(|b| serde_json::from_str::<Value>(b).unwrap());
        let method: &'static str = Box::leak(r["method"].as_str().unwrap().to_string().into_boxed_str());
        let req = HttpRequest { method, base: deltabadger::venue::kucoin::URL.into(), path: r["path"].as_str().unwrap().into(), query, body, not_after: None };
        let headers = deltabadger::venue::kucoin::headers(&req, creds["key"].as_str().unwrap(), creds["secret"].as_str().unwrap(), creds["passphrase"].as_str().unwrap(), ts);
        let header = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        for name in ["KC-API-SIGN", "KC-API-PASSPHRASE", "KC-API-KEY", "KC-API-TIMESTAMP", "KC-API-KEY-VERSION"] {
            assert_eq!(header(name).as_deref(), c["expect"]["headers"][name].as_str(), "{} {name}", c["id"]);
        }
        assert_eq!(req.url().as_str(), c["expect"]["transport_url"].as_str().unwrap(), "{}", c["id"]);
        if method == "POST" { assert_eq!(deltabadger::venue::kucoin::body_text(&req).as_deref(), r["body_bytes"].as_str(), "{}", c["id"]); }
    }
}
