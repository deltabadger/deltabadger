//! PLAN-withdrawal-rules V1: the venues' withdrawal address books, the §4 destination contract, the fee reads and quotes,
//! the §6 sizing and the fee stamp. Rails' recorded reads (withdrawal_vectors.json `reads`, honeymaker 0.12.5) are the
//! oracle where Rails is right; each divergence is asserted narrowly, by name.
use deltabadger::crypto::Credentials;
use deltabadger::ruby::BigDec;
use deltabadger::venue::binance::{self, BinanceVenue};
use deltabadger::venue::gemini::{self, GeminiVenue};
use deltabadger::venue::http::{HttpRequest, ScriptedTransport};
use deltabadger::venue::kraken::KrakenVenue;
use deltabadger::venue::mexc::{self, MexcVenue};
use deltabadger::venue::withdrawal::{self, Destination, Entry, Kind, Refusal, Sizing, Stored};
use deltabadger::venue::{Venue, VenueError};
use serde_json::{json, Value};
use std::collections::BTreeMap;

mod common;

fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap() }
fn cell<'a>(v: &'a Value, section: &str, name: &str) -> &'a Value {
    v[section].as_array().unwrap().iter().find(|c| c["case"] == name).unwrap_or_else(|| panic!("{section} has no case {name}"))
}
/// What a request sent, as the recorder lists it: the query and the form or payload fields, without what is drawn per send.
fn sent(r: &HttpRequest) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = r.query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    for (k, v) in r.body.as_ref().and_then(Value::as_object).into_iter().flatten() { out.insert(k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_string)); }
    for volatile in ["timestamp", "signature", "nonce", "request"] { out.remove(volatile); }
    out
}
fn recorded(r: &Value) -> (String, String, BTreeMap<String, String>) {
    (r["method"].as_str().unwrap().into(), r["path"].as_str().unwrap().into(),
     r["params"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect())
}
fn wire(t: &ScriptedTransport) -> Vec<(String, String, BTreeMap<String, String>)> {
    t.requests().iter().map(|r| (r.method.to_string(), r.path.clone(), sent(r))).collect()
}
fn admitted(entries: &[Entry]) -> Vec<&str> { entries.iter().filter(|e| e.refused.is_none()).map(|e| e.address.as_str()).collect() }
fn rails_names(c: &Value) -> Vec<&str> { c["value"]["returned"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect() }
fn script(c: &Value) -> ScriptedTransport {
    // The recorder's replies are {"body"} with an implied 200.
    let mut s = c["replies"].clone();
    for q in s.as_object_mut().unwrap().values_mut() { for r in q.as_array_mut().unwrap() { r.as_object_mut().unwrap().entry("status").or_insert(json!(200)); } }
    ScriptedTransport::from_script(&s)
}
const ADDRESS: &str = "bc1qx0destination0000000000000000000000";

// ---- Rails' cells where Rails is right

#[tokio::test(flavor = "current_thread")]
async fn binance_kraken_and_gemini_list_what_rails_lists_with_the_same_request() {
    let v = vectors();
    let c = cell(&v, "reads", "binance_listing");
    let t = script(c);
    let entries = BinanceVenue::new(t.clone(), binance::URL, "s").withdrawal_addresses("BTC").await.unwrap();
    assert_eq!(admitted(&entries), rails_names(c), "the coin's rows, in order");
    assert_eq!(wire(&t), c["requests"].as_array().unwrap().iter().map(recorded).collect::<Vec<_>>());

    let c = cell(&v, "reads", "kraken_listing");
    let t = script(c);
    let entries = KrakenVenue::new(t.clone()).withdrawal_addresses("XBT").await.unwrap();
    assert_eq!(admitted(&entries), rails_names(c), "verified entries only");
    assert_eq!(entries[0].key.as_deref(), c["value"]["returned"][0]["key"].as_str());
    assert_eq!(entries[1].refused, Some("the entry is not verified"));
    assert_eq!(wire(&t), c["requests"].as_array().unwrap().iter().map(recorded).collect::<Vec<_>>());

    let c = cell(&v, "reads", "gemini_listing");
    let t = script(c);
    let entries = GeminiVenue::new(t.clone(), gemini::URL).withdrawal_addresses("BTC").await.unwrap();
    assert_eq!(admitted(&entries), rails_names(c), "active entries, once per scope");
    assert_eq!(wire(&t), c["requests"].as_array().unwrap().iter().map(recorded).collect::<Vec<_>>());
    // Gemini lists one address under two scopes: one destination, not an ambiguous pair.
    let d = withdrawal::select(Kind::Gemini, "BTC", &Stored::new(ADDRESS, None, None, None), &entries, None).unwrap();
    assert_eq!(d, Destination::Gemini { ticker: "btc".into(), network: "bitcoin".into(), address: ADDRESS.into() });
}

#[tokio::test(flavor = "current_thread")]
async fn the_kraken_key_rails_sends_with_is_the_key_selected() {
    // Rails' withdraw looks the key up by address (kraken.rb:486): the recorded Withdraw carries the one Rust selects.
    let v = vectors();
    let c = cell(&v, "execute", "success_kraken");
    let t = script(c);
    let entries = KrakenVenue::new(t).withdrawal_addresses("XBT").await.unwrap();
    let sent = c["requests"].as_array().unwrap().iter().find(|r| r["path"] == "/0/private/Withdraw").unwrap();
    let d = withdrawal::select(Kind::Kraken, "XBT", &Stored::new(ADDRESS, None, None, None), &entries, None).unwrap();
    assert_eq!(d, Destination::Kraken { asset: "XBT".into(), key: sent["params"]["key"].as_str().unwrap().into(), address: ADDRESS.into() });
}

#[test]
fn the_coin_list_stamps_what_rails_writes_for_the_rules_asset() {
    let v = vectors();
    for name in ["binance_fees_fee_key", "mexc_fees_fee_key"] {
        let c = cell(&v, "reads", name);
        let body = &c["replies"].as_object().unwrap().values().next().unwrap()[0]["body"];
        let coin = withdrawal::coin_networks(body, "BTC", "venue").unwrap().unwrap();
        let rails = c["exchange_assets"].as_array().unwrap().iter().find(|a| a["asset"] == "BTC").unwrap();
        let stamp = withdrawal::coin_stamp(&coin).unwrap();
        assert_eq!(json!(stamp.fee), rails["withdrawal_fee"], "{name}: the default network's fee");
        assert_eq!(stamp.chains.as_ref(), Some(&rails["withdrawal_chains"]), "{name}: every network, Rails' shape");
        assert_eq!(rails["withdrawal_fee_updated_at"], v["at"], "{name}: Rails stamps the read's time");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_coin_list_is_read_with_the_requests_rails_sends() {
    let v = vectors();
    let c = cell(&v, "reads", "binance_fees_fee_key");
    let t = script(c);
    let coin = BinanceVenue::new(t.clone(), binance::URL, "s").withdrawal_networks("BTC").await.unwrap().unwrap();
    assert_eq!(withdrawal::network_fee(&coin, "LIGHTNING"), Some(bd("0.00001")));
    assert_eq!(wire(&t), c["requests"].as_array().unwrap().iter().map(recorded).collect::<Vec<_>>());
    let c = cell(&v, "reads", "mexc_fees_fee_key");
    let t = script(c);
    MexcVenue::new(t.clone(), mexc::URL, "s").withdrawal_networks("BTC").await.unwrap().unwrap();
    assert_eq!(wire(&t), c["requests"].as_array().unwrap().iter().map(recorded).collect::<Vec<_>>());
}

// ---- the divergences

#[tokio::test(flavor = "current_thread")]
async fn d_rule_mexc_listing_reads_every_page_of_data() {
    let v = vectors();
    let c = cell(&v, "reads", "mexc_listing_paginated");
    assert_eq!(c["value"]["raised"], "TypeError", "Rails' Array(result.data) raises on MEXC's page object");
    let t = script(c);
    let entries = MexcVenue::new(t.clone(), mexc::URL, "s").withdrawal_addresses("BTC").await.unwrap();
    assert_eq!(admitted(&entries), [ADDRESS]);
    assert_eq!(sent(&t.requests()[0]).into_iter().collect::<Vec<_>>(),
               [("coin", "BTC"), ("limit", "50"), ("page", "1"), ("recvWindow", "5000")].map(|(k, v)| (k.to_string(), v.to_string())));
    // Two pages: both read, in order (answered by path: the signed query differs per send).
    let page = |n: i64, a: &str| json!({ "data": [{ "coin": "BTC", "network": "BTC", "address": a, "addressTag": "", "memo": "" }], "totalRecords": 2, "page": n, "totalPageNum": 2 });
    let t2 = ScriptedTransport::default();
    t2.reply(&format!("GET {}", withdrawal::MEXC_ADDRESSES), 200, page(1, "a1"));
    t2.reply(&format!("GET {}", withdrawal::MEXC_ADDRESSES), 200, page(2, "a2"));
    let entries = MexcVenue::new(t2.clone(), mexc::URL, "s").withdrawal_addresses("BTC").await.unwrap();
    assert_eq!(admitted(&entries), ["a1", "a2"]);
    assert_eq!(t2.requests().len(), 2);
    // An unreadable page refuses the whole listing, never a partial one.
    let t3 = ScriptedTransport::default();
    t3.reply(&format!("GET {}", withdrawal::MEXC_ADDRESSES), 200, json!([{ "coin": "BTC", "address": "a" }]));
    assert!(matches!(MexcVenue::new(t3, mexc::URL, "s").withdrawal_addresses("BTC").await, Err(VenueError::Ambiguous(_))));
}

#[test]
fn d_rule_mexc_addresstag_takes_the_memo_from_memo_only() {
    let body = json!({ "data": [
        { "coin": "XRP", "network": "XRP", "address": "r1", "addressTag": "77", "memo": "" },
        { "coin": "XRP", "network": "XRP", "address": "r2", "addressTag": "88", "memo": "88" },
    ], "totalPageNum": 1 });
    let (entries, _) = withdrawal::mexc_page(&body, "XRP").unwrap();
    let coin = Some(withdrawal::Coin { networks: vec![network("XRP", Some("XRP"), true)] });
    assert_eq!(withdrawal::select(Kind::Mexc, "XRP", &Stored::new("r1", None, Some("77"), None), &entries, coin.as_ref()),
               Err(Refusal::Inadmissible("the entry has an addressTag and no memo")));
    assert_eq!(withdrawal::select(Kind::Mexc, "XRP", &Stored::new("r2", None, None, None), &entries, coin.as_ref()),
               Ok(Destination::Mexc { coin: "XRP".into(), network: "XRP".into(), net_work: "XRP".into(), address: "r2".into(), memo: Some("88".into()) }));
}

#[test]
fn d_rule_listed_entry_refuses_the_free_form_tag_rails_stored() {
    // Rails created this rule with a tag the listed entry does not carry (create.rb:93); its withdraw would send it.
    let v = vectors();
    let c = v["api"].as_array().unwrap().iter().find(|c| c["case"] == "create_listed" && c["surface"] == "rest").unwrap();
    assert_eq!(c["status"], 201);
    let rule = &c["rules"][0];
    let body = &c_listing(&v);
    let entries = withdrawal::binance_entries(body, "BTC").unwrap();
    let coin = binance_coin();
    let stored = Stored::new(rule["address"].as_str().unwrap(), rule["settings"]["network"].as_str(), rule["settings"]["address_tag"].as_str(), None);
    assert_eq!(stored.tag.as_deref(), Some("free-form"));
    assert_eq!(withdrawal::select(Kind::Binance, "BTC", &stored, &entries, Some(&coin)), Err(Refusal::NotListed));
    // Without the tag the one entry is the destination, its network taken from the listing.
    let stored = Stored::new(rule["address"].as_str().unwrap(), None, None, None);
    assert_eq!(withdrawal::select(Kind::Binance, "BTC", &stored, &entries, Some(&coin)),
               Ok(Destination::Binance { coin: "BTC".into(), network: "BTC".into(), address: ADDRESS.into(), tag: None }));
}
fn c_listing(v: &Value) -> Value { cell(v, "reads", "binance_listing")["replies"]["GET /sapi/v1/capital/withdraw/address/list"][0]["body"].clone() }
fn network(name: &str, net_work: Option<&str>, tag: bool) -> withdrawal::Network {
    withdrawal::Network { network: name.into(), net_work: net_work.map(str::to_string), withdraw_enable: true, withdraw_tag: Some(tag), fee: Some(bd("0.0001")),
                          fee_raw: json!("0.0001"), is_default: true, multiple: None }
}
fn binance_coin() -> withdrawal::Coin {
    withdrawal::coin_networks(&json!([{ "coin": "BTC", "networkList": [
        { "network": "BTC", "isDefault": true, "withdrawEnable": true, "withdrawTag": false, "withdrawFee": "0.0002" },
        { "network": "BSC", "isDefault": false, "withdrawEnable": true, "withdrawTag": false, "withdrawFee": "0.00001" },
        { "network": "XRPL", "isDefault": false, "withdrawEnable": true, "withdrawTag": true, "withdrawFee": "0.1" },
        { "network": "OFF", "isDefault": false, "withdrawEnable": false, "withdrawTag": false, "withdrawFee": "0.1" },
        { "network": "NOFLAG", "isDefault": false, "withdrawEnable": true, "withdrawFee": "0.1" }
    ] }]), "BTC", "Binance").unwrap().unwrap()
}

#[test]
fn d_rule_fee_stamp_writes_no_unreadable_fee() {
    let v = vectors();
    let c = cell(&v, "reads", "binance_fees_unreadable_fee");
    let rails = c["exchange_assets"].as_array().unwrap().iter().find(|a| a["asset"] == "BTC").unwrap();
    assert!(rails["withdrawal_fee"].is_null() && rails["withdrawal_fee_updated_at"] == v["at"], "Rails stamps a missing fee as fresh");
    let body = &c["replies"]["GET /sapi/v1/capital/config/getall"][0]["body"];
    assert_eq!(withdrawal::coin_stamp(&withdrawal::coin_networks(body, "BTC", "Binance").unwrap().unwrap()), None);
    // Rails writes every coin it can map (USDT here); Rust writes the rule's asset row only, and never creates one.
    assert!(cell(&v, "reads", "binance_fees_fee_key")["exchange_assets"].as_array().unwrap().iter().any(|a| a["asset"] == "USDT" && a["withdrawal_fee"] == "4"));
    let db = db();
    db.execute_batch("INSERT INTO exchange_assets (id, exchange_id, asset_id) VALUES (1, 1, 1), (2, 1, 2);").unwrap();
    let at = "2026-10-25T12:00:00Z".parse().unwrap();
    assert!(withdrawal::write_stamp(&db, 1, 1, &withdrawal::quote_stamp(&bd("0.00015")), at).unwrap());
    assert!(!withdrawal::write_stamp(&db, 1, 9, &withdrawal::quote_stamp(&bd("1")), at).unwrap(), "no row is created");
    let rows: Vec<(Option<String>, Option<String>)> = db.prepare("SELECT withdrawal_fee, withdrawal_fee_updated_at FROM exchange_assets ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(rows[0].0.as_deref(), Some("0.00015"));
    assert!(rows[0].1.as_deref().unwrap().starts_with("2026-10-25 12:00:00"));
    assert_eq!(rows[1], (None, None));
    assert_eq!(db.query_row("SELECT COUNT(*) FROM exchange_assets", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
}

#[test]
fn q1_the_fee_list_is_read_with_the_withdrawal_key_when_there_is_no_fee_key() {
    let v = vectors();
    for name in ["binance_fees_no_fee_key", "mexc_fees_no_fee_key"] {
        let c = cell(&v, "reads", name);
        assert!(c["requests"].as_array().unwrap().is_empty() && c["value"]["data"] == json!({}), "{name}: Rails reads no fee without a FeeApiKey");
    }
    let cipher = common::seed::cipher();
    let db = db();
    assert!(withdrawal::fee_key(&db, &cipher, 1, 1).unwrap().is_none());
    db.execute("INSERT INTO api_keys (id, user_id, exchange_id, key_type, key, secret) VALUES (7, 1, 1, 1, ?1, ?2)", [cipher.encrypt("wk"), cipher.encrypt("ws")]).unwrap();
    db.execute("INSERT INTO api_keys (id, user_id, exchange_id, key_type, key, secret) VALUES (8, 1, 1, 0, ?1, ?2)", [cipher.encrypt("tk"), cipher.encrypt("ts")]).unwrap();
    match withdrawal::fee_key(&db, &cipher, 1, 1).unwrap() {
        Some(withdrawal::FeeKey::Withdrawal(c, id)) => assert_eq!((c.key.as_str(), c.secret.as_str(), id), ("wk", "ws", 7), "the withdrawal key, never the trading key"),
        other => panic!("{other:?}"),
    }
    // An unreadable FeeApiKey is no FeeApiKey; a readable one wins.
    let bad = r#"{"p":"AAAA","h":{"iv":"AAAAAAAAAAAAAAAA","at":"AAAAAAAAAAAAAAAAAAAAAA=="}}"#;
    db.execute("INSERT INTO fee_api_keys (id, exchange_id, key, secret) VALUES (1, 1, ?1, ?1)", [bad]).unwrap();
    assert!(matches!(withdrawal::fee_key(&db, &cipher, 1, 1).unwrap(), Some(withdrawal::FeeKey::Withdrawal(..))));
    db.execute("UPDATE fee_api_keys SET key = ?1, secret = ?2", [cipher.encrypt("fk"), cipher.encrypt("fs")]).unwrap();
    assert!(matches!(withdrawal::fee_key(&db, &cipher, 1, 1).unwrap(), Some(withdrawal::FeeKey::Fee(Credentials { key, .. })) if key == "fk"));
    // An undecryptable withdrawal key is refused before any request.
    db.execute("UPDATE api_keys SET secret = ?1 WHERE id = 7", [bad]).unwrap();
    assert!(deltabadger::engine::model::withdrawal_credentials(&db, &cipher, 1, 1).unwrap().is_none());
}

#[test]
fn d_rule_gemini_memo_lists_every_network_map_coin_but_sends_memo_free_ones_only() {
    let entries = withdrawal::gemini_entries(&json!({ "approvedAddresses": [{ "network": "solana", "address": "So1", "status": "active" }] })).unwrap();
    assert_eq!(withdrawal::gemini_network("SOL"), Some("solana"));
    assert_eq!(withdrawal::select(Kind::Gemini, "SOL", &Stored::new("So1", None, None, None), &entries, None),
               Err(Refusal::Inadmissible("Gemini withdrawals on this network need a memo")));
}

#[tokio::test(flavor = "current_thread")]
async fn binance_us_lists_nothing() {
    let t = ScriptedTransport::default();
    let us = BinanceVenue::new(t.clone(), binance::US_URL, "s").with_rules(&deltabadger::engine::venue_rules::BINANCE_US);
    assert!(us.withdrawal_addresses("BTC").await.is_err() && us.withdrawal_networks("BTC").await.is_err());
    assert!(t.requests().is_empty());
}

// ---- the §4 contract: ambiguous or incomplete entries are refused

#[test]
fn ambiguous_entries_are_refused() {
    // Kraken: a legacy rule (no key stored) whose address carries two keys.
    let k = |key: &str| Entry { address: "a".into(), network: Some("Bitcoin".into()), tag: None, key: Some(key.into()), label: None, refused: None };
    let two = [k("cold"), k("cold-2")];
    assert_eq!(withdrawal::select(Kind::Kraken, "XBT", &Stored::new("a", None, None, None), &two, None), Err(Refusal::Ambiguous));
    assert_eq!(withdrawal::select(Kind::Kraken, "XBT", &Stored::new("a", None, None, Some("cold-2")), &two, None),
               Ok(Destination::Kraken { asset: "XBT".into(), key: "cold-2".into(), address: "a".into() }));
    assert_eq!(withdrawal::select(Kind::Kraken, "XBT", &Stored::new("b", None, None, Some("cold")), &two, None), Err(Refusal::NotListed), "the key's own address");

    // Binance: one address on two networks; the rule's network picks one.
    let coin = binance_coin();
    let b = |network: &str, tag: Option<&str>| Entry { address: "a".into(), network: Some(network.into()), tag: tag.map(str::to_string), key: None, label: None, refused: None };
    let both = [b("BTC", None), b("BSC", None)];
    assert_eq!(withdrawal::select(Kind::Binance, "BTC", &Stored::new("a", None, None, None), &both, Some(&coin)), Err(Refusal::Ambiguous));
    assert_eq!(withdrawal::select(Kind::Binance, "BTC", &Stored::new("a", Some("BSC"), Some(""), None), &both, Some(&coin)),
               Ok(Destination::Binance { coin: "BTC".into(), network: "BSC".into(), address: "a".into(), tag: None }));
    // Binance's tag rule: withdrawTag true needs one, false takes none, absent is not guessed; a closed network is refused.
    let refused = |e: Entry| withdrawal::select(Kind::Binance, "BTC", &Stored::new("a", None, None, None), &[e], Some(&coin));
    assert_eq!(refused(b("XRPL", None)), Err(Refusal::Inadmissible("the network needs a tag and the entry has none")));
    assert!(refused(b("XRPL", Some("9"))).is_ok());
    assert_eq!(refused(b("BTC", Some("9"))), Err(Refusal::Inadmissible("the network takes no tag and the entry has one")));
    assert_eq!(refused(b("NOFLAG", None)), Err(Refusal::Inadmissible("the coin list does not say whether the network needs a tag")));
    assert_eq!(refused(b("OFF", None)), Err(Refusal::Inadmissible("the network is closed for withdrawals")));
    assert_eq!(refused(b("GONE", None)), Err(Refusal::Inadmissible("the network is not in the coin list")));
    assert_eq!(withdrawal::binance_entries(&json!([{ "coin": "BTC", "address": "a" }]), "BTC").unwrap()[0].refused, Some("the entry names no network"));

    // MEXC: a listed network must map to exactly one netWork.
    let m = [Entry { address: "a".into(), network: Some("BTC".into()), tag: None, key: None, label: None, refused: None }];
    let two_codes = withdrawal::Coin { networks: vec![network("BTC", Some("BTC"), false), network("Bitcoin(BTC)", Some("BTC"), false), network("x", Some("BTC2"), false)] };
    let mut clash = two_codes.clone();
    clash.networks[2].network = "BTC".into();
    assert_eq!(withdrawal::select(Kind::Mexc, "BTC", &Stored::new("a", None, None, None), &m, Some(&clash)),
               Err(Refusal::Inadmissible("the entry's network maps to no single netWork")));
    assert_eq!(withdrawal::select(Kind::Mexc, "BTC", &Stored::new("a", None, None, None), &m, None),
               Err(Refusal::Inadmissible("the entry's network maps to no single netWork")));
    assert!(withdrawal::select(Kind::Mexc, "BTC", &Stored::new("a", Some("BTC"), None, None), &m, Some(&two_codes)).is_ok());
    // MEXC: one address on ETH and BSC. The stored network (by any alias) picks its entry; none stored is ambiguous.
    let on = |n: &str| Entry { address: "a".into(), network: Some(n.into()), tag: None, key: None, label: None, refused: None };
    let eth_bsc = [on("Ethereum(ERC20)"), on("BNB Smart Chain(BEP20)")];
    let usdt = withdrawal::Coin { networks: vec![network("Ethereum(ERC20)", Some("ETH"), false), network("BNB Smart Chain(BEP20)", Some("BSC"), false)] };
    let bsc = Ok(Destination::Mexc { coin: "USDT".into(), network: "BNB Smart Chain(BEP20)".into(), net_work: "BSC".into(), address: "a".into(), memo: None });
    assert_eq!(withdrawal::select(Kind::Mexc, "USDT", &Stored::new("a", Some("BSC"), None, None), &eth_bsc, Some(&usdt)), bsc);
    assert_eq!(withdrawal::select(Kind::Mexc, "USDT", &Stored::new("a", Some("BNB Smart Chain(BEP20)"), None, None), &eth_bsc, Some(&usdt)), bsc);
    assert_eq!(withdrawal::select(Kind::Mexc, "USDT", &Stored::new("a", None, None, None), &eth_bsc, Some(&usdt)), Err(Refusal::Ambiguous));
    assert_eq!(withdrawal::select(Kind::Mexc, "USDT", &Stored::new("a", Some("TRX"), None, None), &eth_bsc, Some(&usdt)), Err(Refusal::NotListed));

    // An address nobody lists, and an unverified one.
    assert_eq!(withdrawal::select(Kind::Kraken, "XBT", &Stored::new("z", None, None, None), &two, None), Err(Refusal::NotListed));
    let mut unverified = k("cold");
    unverified.refused = Some("the entry is not verified");
    assert_eq!(withdrawal::select(Kind::Kraken, "XBT", &Stored::new("a", None, None, None), &[unverified], None),
               Err(Refusal::Inadmissible("the entry is not verified")));
}

// ---- fee quotes and sizing

fn kraken_info(t: &ScriptedTransport, amount: &str, body: Value) {
    t.reply(&format!("POST {}?amount={amount}&asset=XBT&key=cold", withdrawal::KRAKEN_INFO), 200, body);
}
fn info(net: &str, fee: &str) -> Value { json!({ "error": [], "result": { "method": "Bitcoin", "limit": "10", "amount": net, "fee": fee } }) }
fn kraken_dest() -> Destination { Destination::Kraken { asset: "XBT".into(), key: "cold".into(), address: "a".into() } }

#[tokio::test(flavor = "current_thread")]
async fn an_inconsistent_kraken_quote_is_unknown() {
    let t = ScriptedTransport::default();
    kraken_info(&t, "1.0", info("0.99", "0.02"));
    kraken_info(&t, "2.0", info("2.0", "-0.01"));
    kraken_info(&t, "3.0", json!({ "error": [], "result": { "amount": "2.99" } }));
    kraken_info(&t, "4.0", json!({ "error": ["EFunding:Unknown withdraw key"] }));
    let venue = KrakenVenue::new(t.clone());
    let d = kraken_dest();
    assert_eq!(venue.withdrawal_fee(&d, &bd("1")).await.unwrap(), None, "net ≠ amount − fee");
    assert_eq!(venue.withdrawal_fee(&d, &bd("2")).await.unwrap(), None, "a negative fee");
    assert_eq!(venue.withdrawal_fee(&d, &bd("3")).await.unwrap(), None, "no fee");
    assert_eq!(venue.withdrawal_fee(&d, &bd("4")).await, Err(VenueError::Rejected(vec!["EFunding:Unknown withdraw key".into()])));
    assert_eq!(sent(&t.requests()[0]).into_iter().collect::<Vec<_>>(),
               [("amount", "1.0"), ("asset", "XBT"), ("key", "cold")].map(|(k, v)| (k.to_string(), v.to_string())));
    // Sized through an unknown quote: Rails' "fee unknown" outcome, nothing to send.
    let sized = withdrawal::size(&bd("1"), &bd("100"), None, |x| { let (venue, d) = (&venue, &d); async move { venue.withdrawal_fee(d, &x).await } }).await.unwrap();
    assert_eq!(sized, Sizing::FeeUnknown { budget: bd("1") });
}

#[tokio::test(flavor = "current_thread")]
async fn a_kraken_percentage_quote_is_sized_exactly() {
    // A 1% method: the fee follows the amount, so the second quote, at the amount sent, is the one that caps it.
    let t = ScriptedTransport::default();
    kraken_info(&t, "1.0", info("0.99", "0.01"));
    kraken_info(&t, "0.99", info("0.9801", "0.0099"));
    let venue = KrakenVenue::new(t.clone());
    let d = kraken_dest();
    let sized = withdrawal::size(&bd("2"), &bd("50"), None, |x| { let (venue, d) = (&venue, &d); async move { venue.withdrawal_fee(d, &x).await } }).await.unwrap();
    assert_eq!(sized, Sizing::Send { budget: bd("1"), amount: bd("0.99"), first_fee: bd("0.01"), fee_cap: bd("0.0099") });
    let Sizing::Send { budget, amount, fee_cap, .. } = sized else { unreachable!() };
    assert!(&amount + &fee_cap <= budget, "the debit stays inside the budget");
    assert_eq!(t.requests().len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn sizing_floors_and_refuses_what_does_not_cover_the_fee() {
    let flat = |f: &'static str| move |_x: BigDec| async move { Ok::<_, VenueError>(Some(bd(f))) };
    // Binance's withdrawIntegerMultiple floors the budget and the amount.
    let sized = withdrawal::size(&bd("1.23456789"), &bd("33.333"), Some(&bd("0.0001")), flat("0.00012345")).await.unwrap();
    assert_eq!(sized, Sizing::Send { budget: bd("0.4115"), amount: bd("0.4113"), first_fee: bd("0.00012345"), fee_cap: bd("0.00012345") });
    assert_eq!(withdrawal::size(&bd("0.00005"), &bd("100"), None, flat("0.0001")).await.unwrap(),
               Sizing::DoesNotCover { budget: bd("0.00005"), fee: bd("0.0001") });
    assert_eq!(withdrawal::floor_p(&bd("0.123456789"), None), bd("0.12345678"));
    assert_eq!(withdrawal::floor_p(&bd("0.3"), Some(&bd("0.1"))), bd("0.3"));
}

#[tokio::test(flavor = "current_thread")]
async fn the_gemini_fee_is_its_estimate_for_the_destination() {
    let t = ScriptedTransport::default();
    t.reply("POST /v2/withdraw/bitcoin/btc/feeEstimate", 200, json!({ "currency": "BTC", "fee": 0.0001, "isOverride": false, "monthlyLimit": 1 }));
    t.reply("POST /v2/withdraw/bitcoin/btc/feeEstimate", 200, json!({ "currency": "ETH", "fee": "0.0001" }));
    let venue = GeminiVenue::new(t.clone(), gemini::URL);
    let d = Destination::Gemini { ticker: "btc".into(), network: "bitcoin".into(), address: ADDRESS.into() };
    assert_eq!(venue.withdrawal_fee(&d, &bd("0.5")).await.unwrap(), Some(bd("0.0001")));
    assert_eq!(venue.withdrawal_fee(&d, &bd("0.5")).await.unwrap(), None, "another currency's fee is unknown");
    assert_eq!(sent(&t.requests()[0]).into_iter().collect::<Vec<_>>(), [("address", ADDRESS), ("amount", "0.5")].map(|(k, v)| (k.to_string(), v.to_string())));
}

#[tokio::test(flavor = "current_thread")]
async fn binance_and_mexc_fees_are_the_selected_networks() {
    let t = ScriptedTransport::default();
    t.reply(&format!("GET {}", withdrawal::BINANCE_COINS), 200, json!([{ "coin": "BTC", "networkList": [
        { "network": "BTC", "isDefault": true, "withdrawFee": "0.0002" }, { "network": "LIGHTNING", "withdrawFee": "x" }] }]));
    let venue = BinanceVenue::new(t, binance::URL, "s");
    let d = |n: &str| Destination::Binance { coin: "BTC".into(), network: n.into(), address: "a".into(), tag: None };
    assert_eq!(venue.withdrawal_fee(&d("BTC"), &bd("1")).await.unwrap(), Some(bd("0.0002")));
    assert_eq!(venue.withdrawal_fee(&d("LIGHTNING"), &bd("1")).await.unwrap(), None, "an unreadable fee is unknown, never 0");
    let t = ScriptedTransport::default();
    t.reply(&format!("GET {}", withdrawal::MEXC_COINS), 200, json!([{ "coin": "BTC", "networkList": [{ "network": "Bitcoin(BTC)", "netWork": "BTC", "withdrawFee": "0.0003" }] }]));
    let d = Destination::Mexc { coin: "BTC".into(), network: "Bitcoin(BTC)".into(), net_work: "BTC".into(), address: "a".into(), memo: None };
    assert_eq!(MexcVenue::new(t, mexc::URL, "s").withdrawal_fee(&d, &bd("1")).await.unwrap(), Some(bd("0.0003")));
}

// ---- D-RULE-LOG-SECRETS

#[tokio::test(flavor = "current_thread")]
async fn d_rule_log_secrets_no_url_query_or_html_reaches_a_message() {
    let v = vectors();
    let rails = cell(&v, "classifier", "binance_empty_500")["rule_logs"][0]["message"].as_str().unwrap();
    assert!(rails.contains("signature="), "Rails writes the signed URL into the row");
    let safe = withdrawal::log_safe(rails);
    assert!(!safe.contains('?') && !safe.contains("signature") && safe.contains("https://api.binance.com/sapi/v1/capital/withdraw/apply"), "{safe}");
    let html = cell(&v, "classifier", "binance_html_502")["rule_logs"][0]["message"].as_str().unwrap().trim_start_matches("Withdrawal failed: ");
    assert_eq!(withdrawal::log_safe(html), "the venue answered with an HTML page");
    assert_eq!(withdrawal::log_safe(&"x".repeat(500)).chars().count(), 201);
    // A read answered with an empty 500: Faraday's text names the signed URL; what V1 returns does not.
    let t = ScriptedTransport::default();
    t.reply_typed(&format!("GET {}", withdrawal::BINANCE_ADDRESSES), 500, "", "");
    let e = BinanceVenue::new(t, binance::URL, "s").withdrawal_addresses("BTC").await.unwrap_err();
    let text = format!("{e:?}");
    assert!(text.contains("status 500") && !text.contains("signature") && !text.contains("timestamp"), "{text}");
}

// ---- the venue code Rails withdraws (Exchange#symbol_from_asset)

#[test]
fn the_venue_code_is_the_first_available_tickers() {
    let db = db();
    db.execute_batch("INSERT INTO tickers (id, exchange_id, base_asset_id, quote_asset_id, base, quote, available) VALUES
                      (1, 1, 1, 2, 'XXBT', 'USD', 0), (2, 1, 1, 2, 'XBT', 'USD', 1), (3, 1, 3, 1, 'ETH', 'XBTQ', 1);").unwrap();
    assert_eq!(withdrawal::venue_code(&db, 1, 1).unwrap().as_deref(), Some("XBT"));
    assert_eq!(withdrawal::venue_code(&db, 1, 2).unwrap().as_deref(), Some("USD"));
    assert_eq!(withdrawal::venue_code(&db, 1, 9).unwrap(), None);
}

/// The columns these reads touch, as db/schema.rb has them.
fn db() -> rusqlite::Connection {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE exchange_assets (id INTEGER PRIMARY KEY, asset_id INTEGER NOT NULL, exchange_id INTEGER NOT NULL, available BOOLEAN DEFAULT 1,
                       created_at DATETIME, updated_at DATETIME, withdrawal_chains JSON, withdrawal_fee VARCHAR, withdrawal_fee_updated_at DATETIME);
                     CREATE UNIQUE INDEX ea ON exchange_assets (asset_id, exchange_id);
                     CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, base_asset_id INTEGER, quote_asset_id INTEGER, base VARCHAR, quote VARCHAR, available BOOLEAN);
                     CREATE TABLE fee_api_keys (id INTEGER PRIMARY KEY, exchange_id INTEGER NOT NULL, key VARCHAR, passphrase VARCHAR, secret VARCHAR);
                     CREATE TABLE api_keys (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, key_type INTEGER, key VARCHAR, secret VARCHAR, passphrase VARCHAR,
                       access_token VARCHAR, rsa_signature_key VARCHAR, rsa_encryption_key VARCHAR, dh_param VARCHAR);").unwrap();
    c
}
