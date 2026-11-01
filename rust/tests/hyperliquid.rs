//! The Hyperliquid trading adapter (venue::hyperliquid) over recorded bodies, its signed /exchange wire, its rules, and the
//! limit lock every Hyperliquid bot has (Bot::LimitOrderable#limit_orders_locked?). Rails-free: the decisions against Rails
//! are tests/parity.rs's grid-hyperliquid.
mod common;
use chrono::{DateTime, Duration, Utc};
use deltabadger::engine::venue_rules::HYPERLIQUID;
use deltabadger::engine::FixedClock;
use deltabadger::venue::binance::{Pair, Pairs};
use deltabadger::venue::http::{self, ScriptedTransport};
use deltabadger::venue::hyperliquid::{self, HyperliquidVenue, URL};
use deltabadger::venue::signers::hyperliquid as signer;
use deltabadger::venue::{NewOrder, OrderKind, OrderStatus, Side, Venue, VenueError};
use serde_json::{json, Value};

const AGENT: &str = "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";
const CLOID_OF: &str = "0b1d2c3e-4f50-6172-8394-a5b6c7d8e9f0";

fn t0() -> DateTime<Utc> { "2026-09-01T10:00:00.5Z".parse().unwrap() }
fn ok(body: Value) -> Value { json!({ "status": 200, "body": body }) }
fn pairs() -> Pairs {
    Pairs { by_symbol: [("@142".to_string(), Pair { base: "BTC".into(), quote: "USDC".into(), price_decimals: 5 })].into(), assets: [("USDC".to_string(), "USDC".to_string())].into() }
}
fn venue(t: &ScriptedTransport) -> HyperliquidVenue<ScriptedTransport> { HyperliquidVenue::new(t.clone(), URL, "0xACCOUNT").with_pairs(pairs()) }
fn spot_meta() -> Value {
    // Sparse: the pair's asset is 10000 + its `index`, never its position.
    ok(json!({ "tokens": [{ "name": "USDC", "index": 0 }, { "name": "BTC", "index": 1 }],
               "universe": [{ "name": "@7", "tokens": [1, 0], "index": 7 }, { "name": "@142", "tokens": [1, 0], "index": 142 }] }))
}
fn new_order(kind: OrderKind, volume: &str) -> NewOrder {
    NewOrder { pair: "@142".into(), side: Side::Buy, kind, volume: volume.into(), quote_volume: false, cl_ord_id: CLOID_OF.into(), deadline: t0() + Duration::seconds(10), day: false }
}
fn script(exchange: Value) -> ScriptedTransport { ScriptedTransport::from_script(&json!({ "POST /info spotMeta": [spot_meta()], "POST /exchange": [exchange] })) }

#[tokio::test(flavor = "current_thread")]
async fn an_order_is_one_gtc_limit_with_the_intents_cloid_on_the_pairs_asset() {
    let t = script(ok(json!({ "status": "ok", "response": { "type": "order", "data": { "statuses": [{ "resting": { "oid": 77738308u64 } }] } } })));
    let id = venue(&t).add_order(&new_order(OrderKind::Limit { price: "40.022".into() }, "1.490")).await;
    assert_eq!(id, Ok("BTC-77738308".into()), "the order id is <base>-<oid>");
    let sent = t.requests().into_iter().find(|r| r.path == hyperliquid::EXCHANGE).unwrap().body.unwrap();
    // Utils.order_request_to_order_wire: a b p s r t in that order, p and s through float_to_wire, `c` the cloid (D9).
    assert_eq!(sent, json!({ "action": { "type": "order", "orders": [{ "a": 10142, "b": true, "p": "40.022", "s": "1.49", "r": false,
                                                                         "t": { "limit": { "tif": "Gtc" } }, "c": "0x0b1d2c3e4f5061728394a5b6c7d8e9f0" }], "grouping": "na" } }));
    // A market plan reaches the adapter crossed: an IOC limit at the persisted crossed price.
    let t = script(ok(json!({ "status": "ok", "response": { "type": "order", "data": { "statuses": [{ "filled": { "oid": 5u64, "totalSz": "1", "avgPx": "1" } }] } } })));
    assert_eq!(venue(&t).add_order(&new_order(OrderKind::Crossed { price: "40.525".into() }, "1.48")).await, Ok("BTC-5".into()));
    let t = ScriptedTransport::from_script(&json!({}));
    assert!(matches!(venue(&t).add_order(&new_order(OrderKind::Market, "1")).await, Err(VenueError::Transient(_))), "never sent uncrossed");
    assert!(t.requests().is_empty());
}

/// Only an answer that proves the refusal clears the intent: Hyperliquid's own `err`, a status error, a 4xx. Every other
/// answer to a placement leaves it unknown (Ambiguous: the intent is kept for the cloid lookup).
#[tokio::test(flavor = "current_thread")]
async fn a_placement_is_refused_only_on_an_answer_that_proves_it() {
    let cases: Vec<(&str, Value, &str)> = vec![
        ("err", ok(json!({ "status": "err", "response": "User or API Wallet 0x1 does not exist." })), "rejected"),
        ("status error", ok(json!({ "status": "ok", "response": { "type": "order", "data": { "statuses": [{ "error": "Insufficient spot balance asset=10142" }] } } })), "rejected"),
        ("4xx", json!({ "status": 422, "body": "\"Failed to deserialize the JSON body into the target type\"" }), "rejected"),
        ("5xx", json!({ "status": 502, "body": "Bad Gateway" }), "ambiguous"),
        ("not json", json!({ "status": 200, "body": "<html>upstream</html>" }), "ambiguous"),
        ("json not an object", ok(json!(["ok"])), "ambiguous"),
        ("no status", ok(json!({ "response": "?" })), "ambiguous"),
        ("odd status", ok(json!({ "status": "unknown" })), "ambiguous"),
        ("ok without statuses", ok(json!({ "status": "ok", "response": { "type": "order", "data": { "statuses": [] } } })), "ambiguous"),
        ("ok without an oid", ok(json!({ "status": "ok", "response": { "type": "order", "data": { "statuses": [{}] } } })), "ambiguous"),
        ("lost reply", json!({ "network": "post_send", "message": "Net::ReadTimeout" }), "ambiguous"),
        // Nothing reached Hyperliquid: the order failed, as Rails writes it.
        ("never sent", json!({ "network": "pre_send:api.hyperliquid.xyz", "message": "refused" }), "rejected"),
    ];
    for (name, answer, want) in cases {
        let t = script(answer);
        let got = venue(&t).add_order(&new_order(OrderKind::Limit { price: "40.022".into() }, "1.49")).await;
        let class = match &got { Err(VenueError::Rejected(_)) => "rejected", Err(VenueError::Ambiguous(_)) => "ambiguous", Err(VenueError::Transient(_)) => "transient", other => panic!("{name}: {other:?}") };
        assert_eq!(class, want, "{name}: {got:?}");
    }
    let t = script(ok(json!({ "status": "err", "response": "User or API Wallet 0x1 does not exist." })));
    let Err(VenueError::Rejected(m)) = venue(&t).add_order(&new_order(OrderKind::Limit { price: "40.022".into() }, "1.49")).await else { panic!() };
    assert_eq!(m, ["Hyperliquid order failed: {\"status\" => \"err\", \"response\" => \"User or API Wallet 0x1 does not exist.\"}"], "Ruby's Hash#to_s");
    assert!(HYPERLIQUID.is_invalid_key(&m), "Exchanges::Hyperliquid::ERRORS[:invalid_key] condemns it");
    // A value float_to_wire cannot carry is a refused order, never re-rounded; nothing is sent.
    let t = ScriptedTransport::from_script(&json!({}));
    assert!(matches!(venue(&t).add_order(&new_order(OrderKind::Limit { price: "40.022".into() }, "0.000000001")).await, Err(VenueError::Rejected(_))));
    assert!(t.requests().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn an_order_is_read_from_its_nested_status_and_its_own_fills() {
    let status = |status: &str, left: &str| ok(json!({ "status": "order", "order": { "order": { "coin": "@142", "side": "B", "limitPx": "40.525", "sz": left,
        "oid": 201, "timestamp": 1_788_000_000_000u64, "origSz": "1.48" }, "status": status, "statusTimestamp": 1 } }));
    let fill = |oid: u64, sz: &str, px: &str, tid: u64| json!({ "coin": "@142", "px": px, "sz": sz, "side": "B", "time": 1_788_000_000_100u64, "oid": oid, "tid": tid });
    let t = ScriptedTransport::from_script(&json!({ "POST /info orderStatus": [status("filled", "0.0")],
                                                    "POST /info userFillsByTime": [ok(json!([fill(201, "1.0", "40.3", 1), fill(201, "0.48", "40.5", 2), fill(9, "1", "1", 3)]))] }));
    let got = venue(&t).orders(&["BTC-201".into()]).await.unwrap();
    assert_eq!((got[0].status, got[0].amount_exec.to_s_f(), got[0].quote_amount_exec.as_ref().unwrap().to_s_f()), (OrderStatus::Closed, "1.48".into(), "59.74".into()));
    // The fills gone: the limit price × executed (an estimate, never 0).
    let t = ScriptedTransport::from_script(&json!({ "POST /info orderStatus": [status("filled", "0.0")], "POST /info userFillsByTime": [ok(json!([]))] }));
    assert_eq!(venue(&t).orders(&["BTC-201".into()]).await.unwrap()[0].quote_amount_exec.as_ref().unwrap().to_s_f(), "59.977");
    for (s, want) in [("open", OrderStatus::Open), ("marginCanceled", OrderStatus::Cancelled), ("perpMarginRejected", OrderStatus::Cancelled), ("odd", OrderStatus::Unknown)] {
        let t = ScriptedTransport::from_script(&json!({ "POST /info orderStatus": [status(s, "1.48")] }));
        assert_eq!(venue(&t).orders(&["BTC-201".into()]).await.unwrap()[0].status, want, "{s}");
    }
    // unknownOid: left out (missing), then settled from the fills from the oldest missing row less an hour, oldest first.
    let t = ScriptedTransport::from_script(&json!({ "POST /info orderStatus": [ok(json!({ "status": "unknownOid" }))],
                                                    "POST /info userFillsByTime": [ok(json!([fill(208, "1.48", "40.4", 7)]))] }));
    let v = venue(&t);
    assert!(v.orders(&["BTC-208".into(), "BTC-209".into()]).await.unwrap().is_empty());
    let found = v.fills_from_trades(&["BTC-208".into(), "BTC-209".into()], t0()).await.unwrap();
    assert_eq!((found.len(), found[0].txid.as_str(), found[0].status), (1, "BTC-208", OrderStatus::Closed));
    let asked = t.requests().into_iter().filter_map(|r| r.body).find(|b| b["type"] == "userFillsByTime" && b["startTime"] != 1_788_000_000_000u64).unwrap();
    assert_eq!(asked["startTime"], deltabadger::venue::bitvavo::ruby_ms(t0()));
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_by_cloid_can_only_find_an_order() {
    let found = ok(json!({ "status": "order", "order": { "order": { "coin": "@142", "side": "B", "limitPx": "40.525", "sz": "0.0", "oid": 77u64,
        "timestamp": 1_788_000_000_000u64, "origSz": "1.48", "cloid": "$cloid" }, "status": "filled" } }));
    let t = ScriptedTransport::from_script(&json!({ "POST /info orderStatus": [found], "POST /info userFillsByTime": [ok(json!([]))] }));
    let got = venue(&t).order_by_client_id(CLOID_OF, "@142", t0()).await.unwrap().unwrap();
    assert_eq!((got.txid.as_str(), got.status), ("BTC-77", OrderStatus::Closed));
    assert_eq!(t.requests()[0].body.clone().unwrap()["oid"], "0x0b1d2c3e4f5061728394a5b6c7d8e9f0", "orderStatus by the cloid");
    let t = ScriptedTransport::from_script(&json!({ "POST /info orderStatus": [ok(json!({ "status": "unknownOid" }))] }));
    assert_eq!(venue(&t).order_by_client_id(CLOID_OF, "@142", t0()).await, Ok(None), "absence proves nothing");
}

/// D-HL-CANCEL-REFUSED: Rails takes any HTTP 200 to a cancel as cancelled; here only Hyperliquid's "success" is. The cancel
/// addresses the pair orderStatus names for the order (H4: never "<base>/USDC").
#[tokio::test(flavor = "current_thread")]
async fn a_cancel_is_a_success_only_when_hyperliquid_says_so() {
    let status = ok(json!({ "status": "order", "order": { "order": { "coin": "@142", "oid": 77u64 }, "status": "open" } }));
    let answer = |a: Value| ScriptedTransport::from_script(&json!({ "POST /info orderStatus": [status.clone()], "POST /info spotMeta": [spot_meta()], "POST /exchange": [a] }));
    let t = answer(ok(json!({ "status": "ok", "response": { "type": "cancel", "data": { "statuses": ["success"] } } })));
    assert_eq!(venue(&t).cancel("BTC-77").await, Ok(()));
    assert_eq!(t.requests().last().unwrap().body.clone().unwrap(), json!({ "action": { "type": "cancel", "cancels": [{ "a": 10142, "o": 77 }] } }));
    let t = answer(ok(json!({ "status": "ok", "response": { "type": "cancel", "data": { "statuses": [{ "error": "Order was never placed, already canceled, or filled." }] } } })));
    assert!(matches!(venue(&t).cancel("BTC-77").await, Err(VenueError::Rejected(_))));
}

#[tokio::test(flavor = "current_thread")]
async fn prices_come_from_the_book_and_the_mids() {
    let t = ScriptedTransport::from_script(&json!({ "POST /info l2Book": [ok(json!({ "levels": [[{ "px": "40.121" }], []] }))], "POST /info allMids": [ok(json!({ "@142": "40.1225" }))] }));
    let v = venue(&t);
    let zero = deltabadger::ruby::BigDec::zero();
    let ticker = deltabadger::engine::model::Ticker { id: 1, ticker: "@142".into(), base_code: "BTC".into(), quote_code: "USDC".into(), base_symbol: "BTC".into(),
        quote_symbol: "USDC".into(), exchange_name: "Hyperliquid".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals: 2, quote_decimals: 2, price_decimals: 5,
        minimum_base_size: zero.clone(), minimum_quote_size: zero, trading_enabled: true, available: true, crypto: true };
    assert_eq!(v.price(&ticker, deltabadger::venue::PriceSide::Bid).await.unwrap().to_s_f(), "40.121");
    assert_eq!(v.price(&ticker, deltabadger::venue::PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Hyperliquid has no asks for @142".into()])), "H3");
    assert_eq!(v.price(&ticker, deltabadger::venue::PriceSide::Last).await.unwrap().to_s_f(), "40.1225");
}

/// The transport signs an /exchange action at send time (Signer#sign_l1_action over the action as posted, with the nonce from
/// the process-wide source) and posts {action, nonce, signature}; /info reads go out as they are. A secret that is no private
/// key sends nothing.
#[tokio::test(flavor = "current_thread")]
async fn the_transport_signs_each_action_with_a_fresh_nonce() {
    use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/exchange"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "ok", "response": { "type": "order", "data": { "statuses": [{ "resting": { "oid": 9 } }] } } })))
        .mount(&server).await;
    Mock::given(method("POST")).and(path("/info")).respond_with(ResponseTemplate::new(200).set_body_json(spot_meta()["body"].clone())).mount(&server).await;
    let at: DateTime<Utc> = "2026-09-20T02:30:00.750Z".parse().unwrap();
    let t = http::ReqwestTransport::hyperliquid(http::client(), "0xACCOUNT".into(), AGENT.into()).with_clock(std::sync::Arc::new(FixedClock(at)));
    let v = HyperliquidVenue::new(t, &server.uri(), "0xACCOUNT").with_pairs(pairs());
    let o = NewOrder { deadline: at + Duration::seconds(10), ..new_order(OrderKind::Limit { price: "40.022".into() }, "1.49") };
    assert_eq!(v.add_order(&o).await, Ok("BTC-9".into()));
    assert_eq!(v.add_order(&o).await, Ok("BTC-9".into()));
    let key = signer::signing_key(AGENT).unwrap();
    let mut nonces = vec![];
    for g in server.received_requests().await.unwrap().iter().filter(|g| g.url.path() == "/exchange") {
        let body: Value = serde_json::from_slice(&g.body).unwrap();
        let nonce = body["nonce"].as_u64().unwrap();
        assert_eq!(String::from_utf8(g.body.clone()).unwrap(), signer::signed_body(&key, &body["action"], nonce), "signed over the action as posted");
        nonces.push(nonce);
    }
    assert!(nonces.len() == 2 && nonces[1] > nonces[0] && nonces[0] >= at.timestamp_millis() as u64, "strictly increasing, never behind the clock: {nonces:?}");
    let refused = http::ReqwestTransport::hyperliquid(http::client(), "0xACCOUNT".into(), "not-a-key".into()).with_clock(std::sync::Arc::new(FixedClock(at)));
    let v = HyperliquidVenue::new(refused, &server.uri(), "0xACCOUNT").with_pairs(pairs());
    let before = server.received_requests().await.unwrap().len();
    assert!(matches!(v.add_order(&o).await, Err(VenueError::Rejected(_))), "nothing sent: the order failed");
    assert_eq!(server.received_requests().await.unwrap().len(), before + 1, "the asset read only, no /exchange");
}

/// Exchanges::Hyperliquid's key check, as the wizard and settings send it: userRole for the address the agent key signs as
/// (an agent of the key's address is correct; a sub-account is not), the reading key's spotClearinghouseState.
#[tokio::test(flavor = "current_thread")]
async fn a_key_is_checked_by_its_agents_role() {
    use deltabadger::web::settings::key_check;
    use deltabadger::web::settings::validator::Validity;
    use wiremock::{matchers::{body_json, method, path}, Mock, MockServer, ResponseTemplate};
    let signer = signer::address(&signer::signing_key(AGENT).unwrap());
    let account = "0x7f2e1a6c9b3d4e5f60718293a4b5c6d7e8f90123";
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/info")).and(body_json(json!({ "type": "userRole", "user": signer })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "role": "agent", "data": { "user": account.to_uppercase().replace("0X", "0x") } }))).mount(&server).await;
    Mock::given(method("POST")).and(path("/info")).and(body_json(json!({ "type": "spotClearinghouseState", "user": account })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "balances": [] }))).mount(&server).await;
    let base = server.uri();
    let check = |key: &str, secret: &str, kind| {
        let credentials = deltabadger::crypto::Credentials { ibkr: None, key: key.into(), secret: secret.into(), passphrase: None, redaction_values: vec![] };
        let base = base.clone();
        async move { key_check::check("Exchanges::Hyperliquid", &base, http::client(), &credentials, kind).await }
    };
    assert!(matches!(check(account, AGENT, 0).await, Validity::Correct), "an agent of this account");
    assert!(matches!(check("0x0000000000000000000000000000000000000001", AGENT, 0).await, Validity::Incorrect), "an agent of another account");
    assert!(matches!(check(account, AGENT, 2).await, Validity::Correct), "a reading key");
    let before = server.received_requests().await.unwrap().len();
    assert!(matches!(check(account, "not-a-key", 0).await, Validity::Incorrect), "a secret that is no private key");
    assert_eq!(server.received_requests().await.unwrap().len(), before, "nothing is sent for it");
}

/// Bot::LimitOrderable#limit_orders_locked?: a Hyperliquid bot is a limit bot whatever its settings say (one stored before the
/// lock, or edited outside the form); at the default distance when it has none.
#[test]
fn every_hyperliquid_bot_places_limit_orders() {
    let (_dir, o, s) = common::install_alpaca();
    let id = common::seed::insert_bot(&o.primary, &s, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.limit_ordered', json('false')) WHERE id = ?1", [id]).unwrap();
    let bot = deltabadger::engine::model::load_bot(&o.primary, id).unwrap();
    assert!(!bot.limit_ordered(), "on Alpaca the setting decides");
    o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Hyperliquid' WHERE id = ?1", [s.exchange_id]).unwrap();
    let bot = deltabadger::engine::model::load_bot(&o.primary, id).unwrap();
    assert!(bot.limit_ordered(), "on Hyperliquid it is locked on");
    assert_eq!(bot.limit_distance().map(|d| d.to_s_f()), Some("0.001".into()));
}

/// D-HYPERLIQUID-MARKET-IOC: sign IOC at Rails' unchanged 1% cross, adjusted before
/// and after crossing with Floor on both sides.
#[tokio::test(flavor = "current_thread")]
async fn market_ioc_buy_and_sell_sign_the_rails_crossed_price() {
    use deltabadger::engine::{amount, model};
    use deltabadger::ruby::BigDec;
    let (_dir, o, s) = common::install_alpaca();
    let id = common::seed::insert_bot(&o.primary, &s, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let bot = model::load_bot(&o.primary, id).unwrap();
    let mut ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    ticker.ticker = "@142".into();
    let d = |s: &str| BigDec::parse(s).unwrap();
    for (touch, decimals, buy, sell) in [("40.1234567", 5, "40.524", "39.721"), ("0.01234567", 2, "0.012468", "0.012221"), ("64012.7", 5, "64652", "63371")] {
        ticker.base_decimals = decimals;
        for (side, want) in [(Side::Buy, buy), (Side::Sell, sell)] {
            let price = HYPERLIQUID.market_price(&d(touch), &ticker, side).unwrap();
            assert_eq!(price, d(want));
            let (plan, _) = amount::size_order(&ticker, side, &price, &d("1"), &price, HYPERLIQUID.minimum_logic, amount::TxKind::Regular).unwrap();
            let plan = plan.with_wire_price(&HYPERLIQUID, &price, true).unwrap();
            let order = plan.to_order(CLOID_OF.into(), t0() + Duration::seconds(10), HYPERLIQUID.wire).unwrap();
            let t = script(ok(json!({"status":"ok","response":{"type":"order","data":{"statuses":[{"filled":{"oid":5,"totalSz":"1","avgPx":touch}}]}}})));
            assert_eq!(venue(&t).add_order(&order).await, Ok("BTC-5".into()));
            let action = t.requests().last().unwrap().body.as_ref().unwrap()["action"].clone();
            let signed: Value = serde_json::from_str(&signer::signed_body(&signer::signing_key(AGENT).unwrap(), &action, 1788000000000)).unwrap();
            assert_eq!(signed["action"]["orders"][0]["t"]["limit"]["tif"], "Ioc");
            assert_eq!(signed["action"]["orders"][0]["p"], want);
            let mut gtc = action.clone();
            gtc["orders"][0]["t"]["limit"]["tif"] = json!("Gtc");
            let gtc: Value = serde_json::from_str(&signer::signed_body(&signer::signing_key(AGENT).unwrap(), &gtc, 1788000000000)).unwrap();
            assert_ne!(signed["signature"], gtc["signature"], "the signature commits to IOC");
        }
    }
}

/// Placement and the normal fill poll share the existing final-order accounting; no IOC remainder rests.
#[tokio::test(flavor = "current_thread")]
async fn market_ioc_fills_no_match_and_uncertainty_settle_through_placement_and_polling() {
    use deltabadger::engine::{amount, model, placement, polling};
    use deltabadger::ruby::BigDec;
    let filled = |size: &str| ok(json!({"status":"ok","response":{"type":"order","data":{"statuses":[{"filled":{"oid":5,"totalSz":size,"avgPx":"40"}}]}}}));
    let no_match = ok(json!({"status":"ok","response":{"type":"order","data":{"statuses":[{"error":"Order could not immediately match against any resting orders."}]}}}));
    for (answer, executed, status) in [(filled("1"), Some("1"), "filled"), (filled("0.4"), Some("0.4"), "filled"),
        (filled("0.4"), Some("0.4"), "iocCanceled"), (no_match, Some("0"), "rejected"),
        (json!({"status":503,"body":"unavailable"}), None, "ambiguous"),
        (json!({"network":"post_send","message":"timeout"}), None, "ambiguous"),
        (json!({"status":200,"body":"unreadable"}), None, "ambiguous")] {
        let (_dir, o, s) = common::install_alpaca();
        let c = &o.primary;
        let id = common::seed::insert_bot(c, &s, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
        c.execute("UPDATE exchanges SET type = 'Exchanges::Hyperliquid' WHERE id = ?1", [s.exchange_id]).unwrap();
        let bot = model::load_bot(c, id).unwrap();
        let mut ticker = model::ticker_for(c, &bot).unwrap().unwrap();
        ticker.ticker = "@142".into();
        ticker.base_decimals = 5;
        let d = |s: &str| BigDec::parse(s).unwrap();
        let (plan, _) = amount::size_order(&ticker, Side::Buy, &d("40.4"), &d("1"), &d("40.4"), HYPERLIQUID.minimum_logic, amount::TxKind::Regular).unwrap();
        let plan = plan.with_wire_price(&HYPERLIQUID, &d("40"), false).unwrap();
        let size = executed.unwrap_or("0");
        let left = (&d("1") - &d(size)).to_s_f();
        let t = ScriptedTransport::from_script(&json!({"POST /info spotMeta":[spot_meta()], "POST /exchange":[answer],
            "POST /info orderStatus":[ok(json!({"status":"order","order":{"order":{"coin":"@142","side":"B","limitPx":"40.4","sz":left,"oid":5,"timestamp":1788000000000u64,"origSz":"1","tif":"Ioc"},"status":status}}))],
            "POST /info userFillsByTime":[ok(json!([{"oid":5,"sz":size,"px":"40","tid":1,"time":1788000000001u64}]))]}));
        let v = venue(&t);
        let owed = amount::pending_quote_amount(c, &bot, t0().timestamp_micros()).unwrap();
        let intent = placement::begin(c, &bot, &plan, &FixedClock(t0())).unwrap();
        let sent = placement::send_recorded(c, &v, &bot, &intent, &FixedClock(t0())).await.unwrap();
        match executed {
            None => {
                assert!(matches!(sent, placement::Sent::Ambiguous(_)), "{sent:?}");
                assert!(model::load_bot(c, id).unwrap().rust_placement().is_some());
                assert_eq!(c.query_row("SELECT count(*) FROM transactions", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
            }
            Some("0") => {
                assert!(matches!(sent, placement::Sent::Rejected(_)), "{sent:?}");
                assert!(model::load_bot(c, id).unwrap().rust_placement().is_none());
                assert_eq!(amount::pending_quote_amount(c, &model::load_bot(c, id).unwrap(), t0().timestamp_micros()).unwrap(), owed);
                assert_eq!(c.query_row("SELECT count(*) FROM transactions WHERE status=0", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
                assert_eq!(t.requests().len(), 2, "no resting order to poll");
            }
            Some(size) => {
                assert!(matches!(sent, placement::Sent::Accepted(_)), "{sent:?}");
                let row = c.query_row("SELECT id FROM transactions", [], |r| r.get::<_, i64>(0)).unwrap();
                assert_eq!(polling::follow_up(c, &v, id, row, t0()).await.unwrap(), polling::FollowUp::Done);
                let (exec, cost, state): (f64, f64, i64) = c.query_row("SELECT amount_exec, quote_amount_exec, external_status FROM transactions", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
                assert_eq!((exec, cost), (d(size).to_f(), (&d(size) * &d("40")).to_f()));
                assert_eq!(state, if status == "iocCanceled" { 3 } else { 2 });
                let before = t.requests().len();
                assert_eq!(polling::follow_up(c, &v, id, row, t0()).await.unwrap(), polling::FollowUp::Done);
                assert_eq!(t.requests().len(), before, "the remainder is final, never polled open");
                assert_eq!(amount::pending_quote_amount(c, &model::load_bot(c, id).unwrap(), t0().timestamp_micros()).unwrap(), &owed - &(&d(size) * &d("40")));
            }
        }
    }
}

/// PLAN-A9 O4 r1 §1: `add_order` reads spotMeta before its POST. A read that ends past the order's deadline (`send_until`
/// on every path: an intent's window, or an API request's earlier send_by) sends nothing, as a pre-transmission refusal.
#[tokio::test(flavor = "current_thread")]
async fn an_order_whose_spot_meta_read_outlasts_its_deadline_is_not_sent() {
    /// The scripted transport, whose clock moves 5 s on while spotMeta is read.
    struct Slow(ScriptedTransport);
    impl http::Transport for Slow {
        fn now(&self) -> DateTime<Utc> { self.0.now() }
        fn deadline_clock(&self) -> Option<DateTime<Utc>> { self.0.deadline_clock() }
        async fn send(&self, r: &http::HttpRequest) -> Result<http::HttpResponse, http::TransportError> {
            let answer = self.0.send(r).await;
            if r.body.as_ref().is_some_and(|b| b["type"] == "spotMeta") { self.0.set_now(t0() + Duration::seconds(14)); }
            answer
        }
    }
    let t = script(ok(json!({"status": "ok", "response": {"type": "order", "data": {"statuses": [{"resting": {"oid": 1}}]}}})));
    t.set_now(t0() + Duration::seconds(9)); // claimed 9 s into a 10 s window
    let v = HyperliquidVenue::new(Slow(t.clone()), URL, "0xACCOUNT").with_pairs(pairs());
    let refused = v.add_order(&new_order(OrderKind::Limit { price: "60000".into() }, "0.001")).await;
    assert!(matches!(&refused, Err(VenueError::Transient(m)) if m.contains("past its send window")), "{refused:?}");
    assert!(t.requests().iter().all(|r| r.path != hyperliquid::EXCHANGE), "no order POST");
}
