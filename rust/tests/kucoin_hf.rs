//! HF-0 contract assets only. These checks do not exercise an HF adapter or certify database effects.
//! The implementation acceptance matrix lives beside the fixtures in MATRIX.md.
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr;

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kucoin_hf") }
fn fixture(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root().join(format!("{name}.json"))).unwrap()).unwrap()
}
fn case<'a>(f: &'a Value, id: &str) -> &'a Value {
    f["cases"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("missing {id}"))
}
fn hmac(secret: &str, input: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(input.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

#[test]
fn kucoin_hf_provenance_and_generated_assets_are_pinned() {
    let m = fixture("manifest");
    assert_eq!(m["schema_version"], 1);
    assert_eq!(m["sdk_revision"].as_str().unwrap().len(), 40);
    assert_eq!(m["oracle"], "official-docs-and-sdk; synthetic variants are not exchange recordings");
    for (name, hash) in m["generated_sha256"].as_object().unwrap() {
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root().join(name)).unwrap())), hash.as_str().unwrap(), "regenerate {name}");
    }
    let recorder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../script/rust/record_kucoin_hf.py");
    assert_eq!(hex::encode(Sha256::digest(std::fs::read(recorder).unwrap())), m["recorder_sha256"]);
    for source in m["sources"].as_object().unwrap().values() {
        assert!(source["url"].as_str().unwrap().starts_with("https://"));
        assert_eq!(source["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(source["fetched_date"], "2026-10-30");
    }
    let mut ids = BTreeSet::new();
    let mut total = 0;
    for name in ["account", "orders", "fills", "errors", "signing"] {
        let f = fixture(name);
        for c in f["cases"].as_array().unwrap() {
            total += 1;
            assert!(ids.insert(c["id"].as_str().unwrap().to_owned()));
            assert_eq!(c["provenance"]["kind"], "synthetic-variant");
            assert!(!c["provenance"]["note"].as_str().unwrap().is_empty());
            for source in c["provenance"]["sources"].as_array().unwrap() {
                assert!(m["sources"].get(source.as_str().unwrap()).is_some());
            }
            assert!(c["expect"].is_object());
        }
        assert_eq!(f["cases"].as_array().unwrap().len() as u64, m["case_counts"][name].as_u64().unwrap());
    }
    assert_eq!(total, m["case_counts"]["total"].as_u64().unwrap(), "sum of account + orders + fills + errors + signing cases");
    assert_eq!(total, 21 + 34 + 10 + 24 + 10, "21 account + 34 orders + 10 fills + 24 errors + 10 signing = 99 cases");
}

#[test]
fn kucoin_hf_endpoint_contract_covers_every_planned_route() {
    let m = fixture("manifest");
    let endpoints = m["endpoints"].as_object().unwrap();
    for (id, method, path) in [
        ("mode", "GET", "/api/ua/v2/account/mode"),
        ("opened", "GET", "/api/v1/hf/accounts/opened"),
        ("balances", "GET", "/api/v1/accounts"),
        ("apikey", "GET", "/api/v1/user/api-key"),
        ("place", "POST", "/api/v1/hf/orders"),
        ("status_id", "GET", "/api/v1/hf/orders/{orderId}"),
        ("status_client", "GET", "/api/v1/hf/orders/client-order/{clientOid}"),
        ("fills", "GET", "/api/v1/hf/fills"),
        ("cancel", "DELETE", "/api/v1/hf/orders/{orderId}"),
    ] {
        assert_eq!(endpoints[id]["method"], method);
        assert_eq!(endpoints[id]["path"], path);
        assert!(m["sources"].get(id).is_some());
    }
    assert_eq!(endpoints.len(), 9);
    for id in ["status_id", "status_client", "fills", "cancel"] {
        assert!(endpoints[id]["required_query"].as_array().unwrap().contains(&Value::from("symbol")));
    }
    assert_eq!(endpoints["balances"]["query"], serde_json::json!([["type", "trade_hf"]]));
}

#[test]
fn kucoin_hf_source_examples_remain_separate_from_coherent_scenarios() {
    let a = fixture("account");
    assert_eq!(a["examples"]["opened"]["body"]["data"], false);
    assert_eq!(a["examples"]["balances"]["body"]["data"][0]["type"], "trade");
    let o = fixture("orders");
    // The published request/response IDs differ; do not silently repair the source example.
    assert_ne!(o["examples"]["status_id"]["body"]["data"]["id"], "6717422bd51c29000775ea03");
    assert_ne!(o["examples"]["status_client"]["body"]["data"]["clientOid"], "5c52e11203aa677f33e493fb");
    let f = fixture("fills");
    let item = &f["examples"]["fills"]["body"]["data"]["items"][0];
    assert_eq!(item["tradeId"].as_u64(), Some(11_029_373_945_659_392));
    assert_ne!(item["id"], item["tradeId"]);
    assert!(item["createdAt"].as_u64().unwrap() > 1_728_692_138_000, "published request bounds do not contain its response");
}

#[test]
fn kucoin_hf_detection_contract_never_guesses_or_synthesizes_zero() {
    let f = fixture("account");
    for (id, mode) in [("detect_classic", "ClassicLf"), ("detect_hf", "ClassicHf"), ("detect_uta", "UnsupportedUta")] {
        assert_eq!(case(&f, id)["expect"]["mode"], mode);
    }
    for c in f["cases"].as_array().unwrap() {
        assert_eq!(c["expect"]["placement_posts"], 0);
        if c["expect"]["mode"] == "Unknown" || c["id"].as_str().unwrap().starts_with("balances_fail") {
            assert_eq!(c["expect"]["preserve_stored_balances"], true);
            assert_eq!(c["expect"]["placement_intents_created"], 0);
        }
    }
    assert_eq!(case(&f, "balances_holds")["expect"]["available"], "25.12345678");
    assert_eq!(case(&f, "balances_holds")["expect"]["tracker_total"], "26.66759503");
    assert_eq!(case(&f, "balances_locked")["expect"], case(&f, "balances_holds")["expect"]);
    assert_eq!(case(&f, "permissions_general_only")["expect"]["trading_allowed"], false);
}

#[test]
fn kucoin_hf_wire_cases_preserve_intent_uuid_and_sizing_inputs() {
    let f = fixture("orders");
    for side in ["buy", "sell"] {
        for shape in ["market_size", "market_funds", "limit"] {
            let c = case(&f, &format!("place_{side}_{shape}"));
            let b = &c["request"]["body"];
            assert_eq!(b["clientOid"], f["identity"]["clientOid"]);
            assert_eq!(b["side"], side);
            assert_eq!(b["symbol"], "BTC-USDT");
            assert_eq!(b.get("price").is_some(), shape == "limit");
            assert_eq!(b.get("funds").is_some(), shape == "market_funds");
            assert_eq!(b.get("size").is_some(), shape != "market_funds");
            assert_eq!(c["expect"]["transaction_rows_delta"], 1);
            assert_eq!(c["expect"]["placement_posts"], 1);
        }
    }
}

#[test]
fn kucoin_hf_status_cancel_and_identity_contract_is_conservative() {
    let f = fixture("orders");
    for id in ["status_active_before_book", "status_active_in_book"] {
        assert_eq!(case(&f, id)["expect"]["status"], "Open");
        assert_eq!(case(&f, id)["response"]["body"]["data"]["active"], true);
    }
    for id in ["status_missing_active", "status_string_active", "status_inactive_in_book", "status_missing_execution", "status_conflicting_execution"] {
        assert_eq!(case(&f, id)["expect"]["status"], "Unresolved");
        assert_eq!(case(&f, id)["expect"]["reserve_credit"], "0");
    }
    let partial = &case(&f, "status_cancelled_partial")["expect"];
    assert_eq!(partial["status"], "Cancelled");
    assert_eq!(partial["gross_base"], "0.000004");
    assert_eq!(partial["gross_quote"], "0.2708704");
    assert_eq!(case(&f, "status_price_round_half_up")["expect"]["price"], "67717.7");
    for (id, status) in [("status_zero_59s", "Unknown"), ("status_zero_60s", "Unknown"), ("status_zero_after_60s", "Closed"), ("status_zero_limit", "Unknown")] {
        assert_eq!(case(&f, id)["expect"]["status"], status);
    }
    for c in f["cases"].as_array().unwrap().iter().filter(|c| c["id"].as_str().unwrap().starts_with("cancel_")) {
        assert_eq!(c["request"]["query"], serde_json::json!([["symbol", "BTC-USDT"]]));
        assert_eq!(c["expect"]["reserve_credit"], "0");
        assert_eq!(c["expect"]["terminal_proven"], false);
    }
    for id in ["lookup_wrong_id", "lookup_wrong_client_oid", "lookup_wrong_symbol", "lookup_wrong_side", "lookup_wrong_type", "lookup_missing"] {
        assert_eq!(case(&f, id)["expect"]["intent"], "retained");
        assert_eq!(case(&f, id)["expect"]["placement_posts"], 0);
    }
}

#[test]
fn kucoin_hf_placement_classification_pins_database_obligations() {
    let f = fixture("errors");
    for c in f["cases"].as_array().unwrap() {
        let e = &c["expect"];
        assert_eq!(e["transaction_rows_delta"], 0);
        assert_eq!(e["reserve_credit"], "0");
        match e["classification"].as_str().unwrap() {
            "Ambiguous" => { assert_eq!(e["intent"], "retained"); assert_eq!(e["replacement_posts"], 0); }
            "Rejected" | "NotSent" => assert_eq!(e["intent"], "cleared"),
            x => panic!("unexpected classification {x}"),
        }
    }
    for id in ["duplicate_client_oid", "unknown_code", "missing_code", "success_missing_id", "success_empty_id", "success_conflicting_client_oid", "truncated_success", "lost_reply", "generic_503", "internal_200", "internal_500", "busy_200", "busy_503"] {
        assert_eq!(case(&f, id)["expect"]["classification"], "Ambiguous", "{id}");
    }
    for code in ["400002", "400003", "400004", "400005", "400006", "400007", "400100", "200004", "429000"] {
        assert_eq!(case(&f, &format!("reject_{code}"))["expect"]["classification"], "Rejected");
    }
}

#[test]
fn kucoin_hf_paging_fixture_requires_returned_integer_cursor_and_empty_terminator() {
    let f = fixture("fills");
    let c = case(&f, "pages_boundary_and_large_ids");
    let pages = c["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 3);
    assert_eq!(pages[0]["response"]["body"]["data"]["items"].as_array().unwrap().len(), 100);
    assert_eq!(pages[1]["response"]["body"]["data"]["items"].as_array().unwrap().len(), 3);
    assert!(pages[2]["response"]["body"]["data"]["items"].as_array().unwrap().is_empty());
    let mut identities = BTreeSet::new();
    for (i, page) in pages.iter().enumerate() {
        let q = page["request"]["query"].as_array().unwrap();
        assert!(!q.iter().any(|p| p[0] == "orderId"));
        if i > 0 {
            let cursor = pages[i - 1]["response"]["body"]["data"]["lastId"].as_u64().unwrap();
            assert!(cursor > (1 << 53));
            assert!(q.contains(&serde_json::json!(["lastId", cursor.to_string()])));
        }
        for item in page["response"]["body"]["data"]["items"].as_array().unwrap() {
            assert_ne!(item["id"], item["tradeId"]);
            identities.insert(format!("{}-{}", item["symbol"].as_str().unwrap(), item["tradeId"].as_u64().unwrap()));
        }
    }
    assert_eq!(identities.len(), 102, "100 + 3 - one repeated boundary fill");
    assert_eq!(c["expect"]["unique_fills"], 102);
    for c in f["cases"].as_array().unwrap().iter().filter(|c| c["expect"]["coverage"] == "incomplete") {
        assert_eq!(c["expect"]["watermark"], "unchanged");
        assert_eq!(c["expect"]["visible_api_rows_delta"], 0);
        assert_eq!(c["expect"]["csv_rows_deleted"], 0);
    }
}

#[test]
fn kucoin_hf_fill_amounts_fees_and_symbol_identity_are_exact() {
    use rust_decimal::Decimal;
    let f = fixture("fills");
    let c = case(&f, "pages_boundary_and_large_ids");
    let mut seen = BTreeSet::new();
    let mut sides = BTreeMap::new();
    let mut totals = [Decimal::ZERO; 3];
    for page in c["pages"].as_array().unwrap() {
        for item in page["response"]["body"]["data"]["items"].as_array().unwrap() {
            let id = item["tradeId"].as_u64().unwrap();
            if !seen.insert(id) { continue; }
            let order = item["orderId"].as_str().unwrap();
            let side = item["side"].as_str().unwrap();
            assert_eq!(*sides.entry(order).or_insert(side), side, "one order cannot change side");
            let decimal = |field: &str| Decimal::from_str(item[field].as_str().unwrap()).unwrap();
            assert_eq!(decimal("price") * decimal("size"), decimal("funds"));
            assert_eq!(item["feeCurrency"], "USDT");
            for (i, field) in ["size", "funds", "fee"].iter().enumerate() { totals[i] += decimal(field); }
        }
    }
    assert_eq!(sides.len(), 2, "separate buy and sell orders, each represented by partial fills");
    for (i, want) in ["0.00102", "69.071952", "0.069071952"].iter().enumerate() {
        assert_eq!(totals[i], Decimal::from_str(want).unwrap());
        assert_eq!(c["expect"]["gross_totals"][i], *want);
    }
    let twins = case(&f, "symbol_scoped_trade_identity");
    assert_eq!(twins["items"][0]["tradeId"], twins["items"][1]["tradeId"]);
    assert_ne!(twins["expect"]["ledger_ids"][0], twins["expect"]["ledger_ids"][1]);
    for (i, item) in twins["items"].as_array().unwrap().iter().enumerate() {
        assert_eq!(twins["expect"]["ledger_ids"][i], format!("{}-{}", item["symbol"].as_str().unwrap(), item["tradeId"].as_u64().unwrap()));
    }
}

#[test]
fn kucoin_hf_independent_hmac_vectors_include_delete_query_and_exact_body() {
    let f = fixture("signing");
    let secret = f["credentials"]["secret"].as_str().unwrap();
    let passphrase = f["credentials"]["passphrase"].as_str().unwrap();
    let mut methods = BTreeSet::new();
    for c in f["cases"].as_array().unwrap() {
        let r = &c["request"];
        let method = r["method"].as_str().unwrap();
        methods.insert(method);
        let query = r["query"].as_array().unwrap();
        let raw_query = query.iter().map(|p| format!("{}={}", p[0].as_str().unwrap(), p[1].as_str().unwrap())).collect::<Vec<_>>().join("&");
        let path = r["path"].as_str().unwrap();
        let endpoint = if query.is_empty() { path.to_owned() } else { format!("{path}?{raw_query}") };
        let body = r["body_bytes"].as_str().unwrap();
        let input = format!("{}{method}{endpoint}{body}", f["timestamp"].as_str().unwrap());
        assert_eq!(input, c["expect"]["prehash"]);
        assert_eq!(hmac(secret, &input), c["expect"]["headers"]["KC-API-SIGN"]);
        assert_eq!(hmac(secret, passphrase), c["expect"]["headers"]["KC-API-PASSPHRASE"]);
        let mut url = reqwest::Url::parse(&format!("https://api.kucoin.com{path}")).unwrap();
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query.iter().map(|p| (p[0].as_str().unwrap(), p[1].as_str().unwrap())));
        }
        assert_eq!(url.as_str(), c["expect"]["transport_url"].as_str().unwrap());
        if method == "DELETE" {
            assert!(raw_query.contains("symbol=BTC-USDT"));
            assert_ne!(hmac(secret, &format!("{}DELETE{path}", f["timestamp"].as_str().unwrap())), c["expect"]["headers"]["KC-API-SIGN"]);
        }
        if c["id"] == "sign_get_unescaped_query" {
            assert_ne!(format!("https://api.kucoin.com{endpoint}"), url.as_str());
        }
    }
    assert_eq!(methods, BTreeSet::from(["DELETE", "GET", "POST"]));
}
