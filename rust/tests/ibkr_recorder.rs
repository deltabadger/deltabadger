//! The IBKR recorder (rust/examples/record_ibkr, PLAN-A27-1-ibkr-recorder.md): the Guard's allow-list and paper check,
//! the scrub per endpoint family, the credential boundary, the all-or-nothing write, and a whole read and paper run over
//! a scripted IBKR signed by I0's recorded login. No live call anywhere.
#[path = "../examples/record_ibkr/recorder.rs"]
mod recorder;

use deltabadger::engine::model::prepared_credential_version;
use deltabadger::engine::FixedClock;
use deltabadger::venue::http::{HttpRequest, HttpResponse, ReqwestTransport, ScriptedTransport, Transport, TransportError};
use deltabadger::venue::ibkr::{Login, Session};
use deltabadger::venue::signers::ibkr::Material;
use recorder::*;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

const ACCT: &str = "DU1234567";
const LIVE: &str = "U7654321";
const ORDER_ID: &str = "1799796559";
const COID: &str = "dca-4f2a9c";

fn fixture() -> (Material, Value) {
    let f: Value = serde_json::from_str(include_str!("fixtures/ibkr_signing.json")).unwrap();
    let k = &f["keys"];
    let s = |n: &str| k[n].as_str().unwrap().to_string();
    let session = f["signing"][1].clone();
    let m = Material { consumer_key: s("consumer_key"), access_token: s("access_token"), access_token_secret: s("access_token_secret"),
                       signature_key: s("signature_key"), encryption_key: s("encryption_key"), dh_param: session["dh_param"].as_str().unwrap().into(), realm: s("realm") };
    (m, session)
}

fn scripted(session: &Value) -> ScriptedTransport {
    let t = ScriptedTransport::default();
    t.reply("POST /v1/api/oauth/live_session_token", 200, json!({ "diffie_hellman_response": session["server"]["diffie_hellman_response"],
            "live_session_token_signature": session["server"]["live_session_token_signature"], "live_session_token_expiration": 1 }));
    t.reply("POST /v1/api/iserver/auth/ssodh/init", 200, json!({ "authenticated": true, "connected": true, "competing": false }));
    t
}

fn stocks() -> Vec<Stock> {
    vec![Stock { tag: "us", symbol: "AAPL".into(), conid: 265598, isin: "US0378331005".into() },
         Stock { tag: "eu", symbol: "SAP".into(), conid: 14204, isin: "DE0007164600".into() }]
}

fn accounts(ids: &[&str]) -> Value {
    json!({ "accounts": ids, "selectedAccount": ids.first(), "aliases": ids.iter().map(|a| (a.to_string(), json!(a))).collect::<serde_json::Map<_, _>>(),
            "isPaper": true, "serverInfo": { "serverName": "JifN19053", "serverVersion": "Build 10.25.0p" } })
}

/// Every read the read mode sends, answered as a hand-written IBKR would.
fn script_reads(t: &ScriptedTransport, account: &str) {
    t.reply(&format!("GET /v1/api/portfolio/{account}/positions/0"), 200, json!([{ "acctId": account, "conid": 265598, "contractDesc": "AAPL", "position": 3.0, "mktPrice": 190.1,
            "mktValue": 570.3, "currency": "USD", "avgCost": 180.25, "ticker": "AAPL", "name": "APPLE INC", "assetClass": "STK" }]));
    t.reply(&format!("GET /v1/api/portfolio/{account}/positions/1"), 200, json!([]));
    t.reply(&format!("GET /v1/api/portfolio/{account}/ledger"), 200, json!({ "USD": { "cashbalance": 25000.5, "currency": "USD", "acctcode": account, "timestamp": 1761400000 },
            "BASE": { "cashbalance": 25000.5, "currency": "BASE", "netliquidationvalue": 25570.8 } }));
    t.reply("GET /v1/api/iserver/account/orders", 200, json!({ "orders": [{ "acct": account, "conid": 265598, "orderId": ORDER_ID.parse::<i64>().unwrap(), "order_ref": COID,
            "status": "Filled", "side": "BUY", "totalSize": 3, "filledQuantity": 3, "avgPrice": "180.25", "orderType": "Market", "lastExecutionTime": "231211180049",
            "lastExecutionTime_r": 1702317649000_i64, "companyName": "APPLE INC" }], "snapshot": true }));
    t.reply("GET /v1/api/iserver/account/trades", 200, json!([{ "execution_id": "0000e0d5.6576fd38.01.01", "order_ref": COID, "order_id": ORDER_ID.parse::<i64>().unwrap(),
            "conid": 265598, "side": "B", "size": 3, "price": "180.25", "account": account, "trade_time": "20231211-18:00:49", "trade_time_r": 1702317649000_i64,
            "commission": "1.00", "net_amount": 540.75 }]));
    t.reply(&format!("GET /v1/api/iserver/account/order/status/{ORDER_ID}"), 200, json!({ "order_id": ORDER_ID.parse::<i64>().unwrap(), "conid": 265598, "side": "B",
            "total_size": "3.0", "order_type": "MARKET", "cum_fill": "3.0", "order_status": "Filled", "average_price": "180.25", "account": account }));
    t.reply("GET /v1/api/iserver/marketdata/snapshot", 200, json!([{ "conid": 265598, "conidEx": "265598", "31": "190.50", "84": "190.49", "86": "190.51", "6509": "RpB",
            "_updated": 1761400000000_i64, "server_id": "q0" }]));
    t.reply("GET /v1/api/iserver/marketdata/history", 200, json!({ "symbol": "AAPL", "text": "APPLE INC", "priceFactor": 100, "startTime": "20231211-04:00:00",
            "data": [{ "o": 190.1, "c": 191.2, "h": 192.0, "l": 189.8, "v": 51000.0, "t": 1702252800000_i64 }], "points": 1, "mdAvailability": "S" }));
    t.reply("POST /v1/api/iserver/contract/rules", 200, json!({ "rules": { "sizeIncrement": 1, "incrementRules": [{ "lowerEdge": 0.0, "increment": 0.01 }],
            "priceMagnifier": 1, "cashCcy": "USD", "orderTypes": ["limit", "market"] }, "canTradeAcctIds": [account] }));
    t.reply("POST /v1/api/pa/transactions", 200, json!({ "rc": 0, "nd": 90, "currency": "USD", "id": "getTransactions", "includesRealTime": true,
            "transactions": [{ "date": "20231211", "cur": "USD", "fxRate": 1, "pr": 180.25, "qty": 3, "acctid": account, "amt": -540.75, "conid": 265598,
                               "type": "Buy", "desc": "Apple Inc" }] }));
    t.reply("GET /v1/api/iserver/secdef/search", 200, json!([{ "conid": "265598", "companyHeader": "APPLE INC - NASDAQ", "companyName": "APPLE INC", "symbol": "AAPL",
            "description": "NASDAQ", "sections": [{ "secType": "STK", "exchange": "NASDAQ;" }] }]));
    t.reply("GET /v1/api/iserver/secdef/info", 200, json!([{ "conid": 265598, "symbol": "AAPL", "secType": "STK", "listingExchange": "NASDAQ", "currency": "USD",
            "isin": "US0378331005", "priceMagnifier": 1 }]));
    t.reply("GET /v1/api/trsrv/stocks", 200, json!({ "AAPL": [{ "name": "APPLE INC", "assetClass": "STK", "contracts": [{ "conid": 265598, "exchange": "NASDAQ", "isUS": true }] }],
                                                    "SAP": [{ "name": "SAP SE", "assetClass": "STK", "contracts": [{ "conid": 14204, "exchange": "IBIS", "isUS": false }] }] }));
    t.reply("GET /v1/api/trsrv/secdef", 200, json!({ "secdef": [{ "conid": 265598, "currency": "USD", "listingExchange": "NASDAQ", "countryCode": "US", "ticker": "AAPL",
            "allExchanges": "AMEX,NASDAQ,SMART", "name": "APPLE INC" }] }));
    for c in [265598, 14204] {
        t.reply(&format!("GET /v1/api/iserver/contract/{c}/info"), 200, json!({ "con_id": c, "symbol": "AAPL", "instrument_type": "STK", "currency": "USD",
                "valid_exchanges": "SMART,AMEX,NASDAQ", "company_name": "APPLE INC" }));
    }
}

struct Run { result: Result<Outcome, String>, guard_log: Vec<Exchange>, refused: Vec<String>, sent: Vec<HttpRequest>, dir: tempfile::TempDir }

fn run(cmd: Command, script: impl FnOnce(&ScriptedTransport), unauthorized: Option<(u16, String)>) -> Run {
    let (m, session) = fixture();
    let secrets = secrets(&m);
    let login = Arc::new(Login::new(m, prepared_credential_version(0, &[Some("t".into()), None, None, None, None, None, None])).unwrap());
    let t = scripted(&session);
    script(&t);
    let mut sensitive = login.sensitive();
    sensitive.extend(secrets.iter().cloned());
    let guard = Guard::new(t.clone(), cmd.mode, sensitive);
    let clock = FixedClock("2026-11-16T15:00:00Z".parse().unwrap());
    let dh = session["dh_random"].as_str().unwrap().to_string();
    let s = Session::new(&guard, login, &clock).with_dh_random(move || dh.clone());
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
    let result = rt.block_on(record(&s, &guard, &cmd, unauthorized, &secrets, &mut Scrubber::new(b"test salt".to_vec(), 98), dir.path()));
    Run { result, guard_log: guard.log(), refused: guard.refused(), sent: t.requests(), dir }
}

fn read_cmd() -> Command { Command { mode: Mode::Read, stocks: stocks(), limit: None, untradable: None } }
fn paper_cmd() -> Command { Command { mode: Mode::Paper, stocks: vec![stocks().remove(0)], limit: Some("95.5".into()), untradable: None } }

fn all_text(dir: &Path) -> String {
    let mut out = String::new();
    for f in std::fs::read_dir(dir).unwrap() { out += &std::fs::read_to_string(f.unwrap().path()).unwrap(); }
    out
}
fn file(dir: &Path, name: &str) -> Value { serde_json::from_str(&std::fs::read_to_string(dir.join(name)).unwrap()).unwrap() }
fn order_calls(sent: &[HttpRequest]) -> usize { sent.iter().filter(|r| r.path.ends_with("/orders") && r.method == "POST" || r.path.contains("/reply/") || r.method == "DELETE").count() }

#[test]
fn a_read_run_writes_the_whole_set_scrubbed_and_sends_no_order() {
    let r = run(read_cmd(), |t| { t.reply("GET /v1/api/iserver/accounts", 200, accounts(&[LIVE])); script_reads(t, LIVE); }, Some((401, "{\"error\":\"not authenticated\"}".into())));
    let out = r.result.unwrap();
    assert_eq!(order_calls(&r.sent), 0);
    let dir = r.dir.path().join("read");
    for f in ["accounts", "positions.0", "positions.1", "ledger", "live_orders.1", "live_orders.2", "trades", "order_status", "snapshot.us.1", "snapshot.us.2",
              "snapshot_figures.eu", "history.us", "rules.us.buy", "rules.eu.sell", "pa_transactions.us", "pa_transactions.both", "secdef_search.us.isin",
              "secdef_info.eu", "trsrv_stocks.us", "trsrv_secdef.us", "contract_info.eu", "unauthorized", "manifest"] {
        assert!(dir.join(format!("{f}.json")).exists(), "{f}");
    }
    let text = all_text(&dir);
    for leaked in [LIVE, ORDER_ID, COID, "25000.5", "180.25", "190.50", "JifN19053", "0000e0d5.6576fd38.01.01"] { assert!(!text.contains(leaked), "{leaked} survived"); }
    // The account keeps its prefix and maps the same everywhere, keys included.
    let acc = file(&dir, "accounts.json");
    let fake = acc["accounts"][0].as_str().unwrap().to_string();
    assert!(fake.starts_with('U') && fake.len() == LIVE.len() && deltabadger::venue::ibkr_account::parse(&fake).is_some());
    assert_eq!(acc["selectedAccount"], json!(fake));
    assert!(acc["aliases"].get(&fake).is_some(), "an account-id key maps to its fake");
    assert_eq!(file(&dir, "positions.0.json")[0]["acctId"], json!(fake));
    assert!(file(&dir, "manifest.json")["files"].as_array().unwrap().iter().any(|f| f["path"] == json!(format!("/v1/api/portfolio/{fake}/ledger"))));
    // Public contract facts kept; a position's name replaced.
    let snap = file(&dir, "snapshot.us.1.json");
    assert_eq!((snap[0]["conid"].clone(), snap[0]["6509"].clone(), snap[0]["conidEx"].clone()), (json!(265598), json!("RpB"), json!("265598")));
    assert_ne!(snap[0]["31"], json!("190.50"));
    assert_eq!(file(&dir, "secdef_info.us.json")[0]["isin"], json!("US0378331005"));
    assert_eq!(file(&dir, "rules.us.buy.json")["rules"]["priceMagnifier"], json!(1));
    assert!(file(&dir, "trsrv_stocks.us.json").get("AAPL").is_some(), "a stock's symbol key in a contract file");
    assert_ne!(file(&dir, "positions.0.json")[0]["name"], json!("APPLE INC"));
    assert!(!file(&dir, "manifest.json")["scrubbed"].to_string().contains(LIVE), "report paths name scrubbed keys");
    // Times move back by whole weeks, in their own format.
    assert_eq!(file(&dir, "snapshot.us.1.json")[0]["_updated"], json!(1761400000000_i64 - 98 * 86_400_000));
    assert_eq!(file(&dir, "trades.json")[0]["trade_time"], json!("20230904-18:00:49"));
    assert_eq!(file(&dir, "live_orders.1.json")["orders"][0]["lastExecutionTime"], json!("230904180049"));
    assert_eq!(file(&dir, "pa_transactions.us.json")["transactions"][0]["date"], json!("20230904"));
    assert_eq!(file(&dir, "pa_transactions.us.json")["transactions"][0]["desc"], json!("[text]"));
    // The order status read the id the live orders listed, under its fake in the manifest.
    let manifest = file(&dir, "manifest.json");
    assert_eq!(manifest["mode"], json!("read"));
    let status = manifest["files"].as_array().unwrap().iter().find(|f| f["file"] == json!("order_status.json")).unwrap();
    assert!(!status["path"].as_str().unwrap().contains(ORDER_ID));
    assert_eq!(manifest["files"].as_array().unwrap().iter().find(|f| f["file"] == json!("unauthorized.json")).unwrap()["status"], json!(401));
    assert!(out.report.contains("not 2xx (recorded): unauthorized 401") && !out.report.contains(LIVE));
}

#[test]
fn paper_mode_refuses_a_live_or_mixed_account_list_before_any_order() {
    for ids in [vec![LIVE], vec![ACCT, LIVE], vec![LIVE, ACCT], vec![]] {
        let r = run(paper_cmd(), |t| { t.reply("GET /v1/api/iserver/accounts", 200, accounts(&ids)); }, None);
        assert!(r.result.is_err(), "{ids:?}");
        assert_eq!(order_calls(&r.sent), 0, "{ids:?}");
        assert!(!r.dir.path().join("paper").exists());
    }
}

fn paper_script(t: &ScriptedTransport) { paper_with(t, true) }

fn paper_with(t: &ScriptedTransport, fills: bool) {
    t.reply("GET /v1/api/iserver/accounts", 200, accounts(&[ACCT]));
    let orders = format!("POST /v1/api/iserver/account/{ACCT}/orders");
    let prompt = json!([{ "id": "07a13a5a-4a48-44a5-bb25-5ab37b79186c", "message": ["You are about to submit a market order of 1 share for DU1234567. Are you sure?"],
                          "isSuppressed": false, "messageIds": ["o163"] }]);
    t.reply(&orders, 200, prompt).reply(&orders, 200, json!([{ "order_id": "102", "order_status": "Submitted" }])).reply(&orders, 200, json!([{ "order_id": "103", "order_status": "PreSubmitted" }]));
    t.reply("POST /v1/api/iserver/reply/07a13a5a-4a48-44a5-bb25-5ab37b79186c", 200, json!([{ "order_id": "101", "order_status": "PreSubmitted", "encrypt_message": "1" }]));
    let status = |id: &str, s: &str| json!({ "order_id": id.parse::<i64>().unwrap(), "conid": 265598, "side": "B", "total_size": "1.0", "order_type": "MARKET", "cum_fill": "1.0", "order_status": s, "account": ACCT });
    t.reply("GET /v1/api/iserver/account/order/status/101", 200, status("101", "PreSubmitted"));
    if fills { t.reply("GET /v1/api/iserver/account/order/status/101", 200, status("101", "Filled")); }
    t.reply("GET /v1/api/iserver/account/order/status/102", 200, status("102", "Submitted")).reply("GET /v1/api/iserver/account/order/status/102", 200, status("102", "Cancelled"));
    t.reply("GET /v1/api/iserver/account/order/status/103", 200, status("103", "Filled"));
    t.reply(&format!("DELETE /v1/api/iserver/account/{ACCT}/order/102"), 200, json!({ "msg": "Request was submitted", "order_id": 102, "conid": -1, "account": null }));
    script_reads(t, ACCT);
}

#[test]
fn a_paper_run_buys_cancels_and_sells_confirming_each_prompt_and_records_its_ids() {
    let r = run(paper_cmd(), paper_script, None);
    let out = r.result.unwrap();
    let dir = r.dir.path().join("paper");
    for f in ["accounts", "buy_market.order", "buy_market.reply.1", "buy_market.status.first", "buy_market.status.final", "buy_limit.order", "buy_limit.status",
              "buy_limit.cancel", "buy_limit.status_after_cancel", "sell_market.order", "sell_market.status.final", "positions.0", "ledger", "trades"] {
        assert!(dir.join(format!("{f}.json")).exists(), "{f}");
    }
    let reply = r.sent.iter().find(|q| q.path.contains("/reply/")).unwrap();
    assert_eq!(reply.body, Some(json!({ "confirmed": true })));
    assert_eq!(file(&dir, "manifest.json")["message_ids"], json!(["o163"]));
    assert_eq!(file(&dir, "buy_market.order.json")[0]["message"], json!(["[text]"]), "prompt prose never written");
    assert_eq!(out.prompts[0].0, vec!["o163".to_string()], "the text goes to the terminal only");
    assert_eq!(file(&dir, "buy_market.status.first.json")["order_status"], json!("PreSubmitted"));
    assert_eq!(file(&dir, "buy_market.status.final.json")["order_status"], json!("Filled"));
    // Each order POST and reply came straight after its own account list; the limit order went at the given price.
    let log = r.guard_log;
    for (i, x) in log.iter().enumerate().filter(|(_, x)| x.method != "GET" && (x.path.ends_with("/orders") || x.path.contains("/reply/") || x.method == "DELETE")) {
        let before = log[..i].iter().rev().find(|p| p.path != "/v1/api/oauth/live_session_token").unwrap();
        assert_eq!(before.path, "/v1/api/iserver/accounts", "{} {}", x.method, x.path);
    }
    let limit = r.sent.iter().filter(|q| q.path.ends_with("/orders")).nth(1).unwrap();
    assert_eq!(limit.body.as_ref().unwrap()["orders"][0]["price"], json!(95.5));
    let text = all_text(&dir);
    for leaked in [ACCT, "07a13a5a-4a48-44a5-bb25-5ab37b79186c", "1799796559"] { assert!(!text.contains(leaked), "{leaked}"); }
    assert!(r.refused.is_empty(), "{:?}", r.refused);
}

#[test]
fn an_order_that_never_fills_writes_nothing() {
    let r = run(paper_cmd(), |t| { paper_with(t, false); }, None);
    assert!(r.result.unwrap_err().contains("not filled"));
    assert!(!r.dir.path().join("paper").exists());
    assert_eq!(r.sent.iter().filter(|q| q.path.ends_with("/order/status/101")).count(), POLLS);
}

// ---- the Guard on its own

fn guard(mode: Mode) -> (Guard<ScriptedTransport>, ScriptedTransport) {
    let t = ScriptedTransport::default();
    (Guard::new(t.clone(), mode, vec!["CONSUMERK".into()]), t)
}
fn req(method: &'static str, path: &str, body: Option<Value>) -> HttpRequest {
    HttpRequest { method, base: "https://api.ibkr.com".into(), path: path.into(), query: vec![], body, not_after: None }
}
fn send(g: &Guard<ScriptedTransport>, r: HttpRequest) -> Result<HttpResponse, TransportError> {
    tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(g.send_typed(&r)).map(|(r, _)| r)
}
fn order_body() -> Value { json!({ "orders": [{ "conid": 265598, "orderType": "MKT", "side": "BUY", "quantity": 1, "tif": "DAY" }] }) }
fn orders_path(a: &str) -> String { format!("/v1/api/iserver/account/{a}/orders") }

#[test]
fn read_mode_refuses_every_order_reply_and_cancel_path_and_anything_off_the_list() {
    let (g, t) = guard(Mode::Read);
    t.reply("GET /v1/api/iserver/accounts", 200, accounts(&[ACCT]));
    send(&g, req("GET", "/v1/api/iserver/accounts", None)).unwrap();
    for r in [req("POST", &orders_path(ACCT), Some(order_body())), req("POST", "/v1/api/iserver/reply/abc", Some(json!({ "confirmed": true }))),
              req("DELETE", &format!("/v1/api/iserver/account/{ACCT}/order/1"), None), req("POST", &format!("{}/whatif", orders_path(ACCT)), Some(order_body())),
              req("GET", "/v1/api/iserver/account/pnl/partitioned", None), req("POST", "/v1/api/iserver/secdef/search", None)] {
        assert!(matches!(send(&g, r.clone()), Err(TransportError::Permanent(_))), "{} {}", r.method, r.path);
    }
    assert_eq!(t.requests().len(), 1, "only the account list was sent");
}

#[test]
fn a_paper_only_request_needs_its_own_lone_paper_account_list_just_before() {
    let ok = || accounts(&[ACCT]);
    type Case = (&'static str, Vec<(u16, Value)>, Option<HttpRequest>);
    let cases: Vec<Case> = vec![
        ("live", vec![(200, accounts(&[LIVE]))], None),
        ("mixed", vec![(200, accounts(&[ACCT, LIVE]))], None),
        ("two paper", vec![(200, accounts(&[ACCT, "DU7654321"]))], None),
        ("failed", vec![(500, json!({ "error": "x" }))], None),
        ("unreadable", vec![(200, json!("not a list"))], None),
        ("an init in between", vec![(200, ok())], Some(req("POST", "/v1/api/iserver/auth/ssodh/init", Some(json!({}))))),
        ("a read in between", vec![(200, ok())], Some(req("GET", "/v1/api/iserver/account/trades", None))),
    ];
    for (what, answers, between) in cases {
        let (g, t) = guard(Mode::Paper);
        for (status, a) in answers { t.reply("GET /v1/api/iserver/accounts", status, a); }
        t.reply("POST /v1/api/iserver/auth/ssodh/init", 200, json!({})).reply("GET /v1/api/iserver/account/trades", 200, json!([]));
        let _ = send(&g, req("GET", "/v1/api/iserver/accounts", None));
        if let Some(b) = between { send(&g, b).unwrap(); }
        assert!(send(&g, req("POST", &orders_path(ACCT), Some(order_body()))).is_err(), "{what}");
        assert!(!t.requests().iter().any(|r| r.path.ends_with("/orders")), "{what}");
    }
    // An LST mint between is fine; the list allows one paper-only request, and a second needs a new read.
    let (g, t) = guard(Mode::Paper);
    t.reply("GET /v1/api/iserver/accounts", 200, ok()).reply("POST /v1/api/oauth/live_session_token", 200, json!({}));
    t.reply(&format!("POST {}", orders_path(ACCT)), 200, json!([{ "order_id": "9" }]));
    send(&g, req("GET", "/v1/api/iserver/accounts", None)).unwrap();
    send(&g, req("POST", "/v1/api/oauth/live_session_token", None)).unwrap();
    send(&g, req("POST", &orders_path(ACCT), Some(order_body()))).unwrap();
    assert!(send(&g, req("POST", &orders_path(ACCT), Some(order_body()))).is_err(), "a second order without a new read");
    assert_eq!(t.requests().iter().filter(|r| r.path.ends_with("/orders")).count(), 1);
}

#[test]
fn a_paper_only_request_is_bound_to_the_listed_account_and_this_runs_own_ids() {
    let try_after_list = |r: HttpRequest, seed: Option<(&str, Value)>| {
        let (g, t) = guard(Mode::Paper);
        t.reply("GET /v1/api/iserver/accounts", 200, accounts(&[ACCT]));
        if let Some((key, answer)) = seed { t.reply(key, 200, answer); t.reply("GET /v1/api/iserver/accounts", 200, accounts(&[ACCT])); }
        let key = format!("{} {}", r.method, r.path);
        t.reply(&key, 200, json!({}));
        if seed_needed(&r) {
            send(&g, req("GET", "/v1/api/iserver/accounts", None)).unwrap();
            send(&g, req("POST", &orders_path(ACCT), Some(order_body()))).unwrap();
        }
        send(&g, req("GET", "/v1/api/iserver/accounts", None)).unwrap();
        let out = send(&g, r);
        (out, t.requests().iter().filter(|q| q.path == key.split_once(' ').unwrap().1).count())
    };
    fn seed_needed(r: &HttpRequest) -> bool { r.path.contains("/reply/") || r.method == "DELETE" }
    let prompt = (format!("POST {}", orders_path(ACCT)).leak() as &str, json!([{ "id": "abc-1", "message": ["sure?"], "messageIds": ["o163"] }, { "order_id": "77" }]));
    // Foreign account, a body naming an account, unknown prompt and order ids, unsafe segments.
    for r in [req("POST", &orders_path("DU7654321"), Some(order_body())),
              req("POST", &orders_path(ACCT), Some(json!({ "orders": [{ "conid": 1, "acctId": LIVE }] }))),
              req("POST", &orders_path(ACCT), Some(json!({ "orders": [], }))),
              req("POST", &orders_path(ACCT), Some(json!({ "orders": [{}], "account": LIVE }))),
              req("POST", "/v1/api/iserver/reply/someone-elses", Some(json!({ "confirmed": true }))),
              req("DELETE", &format!("/v1/api/iserver/account/{ACCT}/order/12345"), None),
              req("POST", "/v1/api/iserver/account/DU1234567%2F..%2FU7654321/orders", Some(order_body())),
              req("POST", "/v1/api/iserver/account/../U7654321/orders", Some(order_body()))] {
        let (out, sent) = try_after_list(r.clone(), Some(prompt.clone()));
        assert!(out.is_err() && sent == 0, "{} {} {:?}", r.method, r.path, r.body);
    }
    // This run's own prompt and order ids pass.
    for r in [req("POST", "/v1/api/iserver/reply/abc-1", Some(json!({ "confirmed": true }))), req("DELETE", &format!("/v1/api/iserver/account/{ACCT}/order/77"), None)] {
        let (out, sent) = try_after_list(r.clone(), Some(prompt.clone()));
        assert!(out.is_ok() && sent == 1, "{} {}", r.method, r.path);
    }
}

#[test]
fn a_credential_in_an_answer_or_its_redaction_stops_the_run_with_nothing_written() {
    let (m, _) = fixture();
    let ck = m.consumer_key.clone();
    let r = run(read_cmd(), |t| { t.reply("GET /v1/api/iserver/accounts", 200, accounts(&[LIVE]));
        t.reply("GET /v1/api/iserver/account/trades", 200, json!([{ "order_ref": format!("x{ck}y") }])); script_reads(t, LIVE); }, None);
    assert!(r.result.unwrap_err().contains("credential"));
    assert!(!r.dir.path().join("read").exists());
    // In production the transport has already put its redaction in place of such a body: the Guard refuses that too.
    let login = Arc::new(Login::new(m, prepared_credential_version(0, &[Some("t".into()), None, None, None, None, None, None])).unwrap());
    let settled = ReqwestTransport::ibkr(deltabadger::venue::http::client(), login.clone()).settle(HttpResponse { status: 200, body: format!("{{\"order_ref\":\"{ck}\"}}") });
    assert!(!settled.body.contains(&ck));
    let (g, _) = guard(Mode::Read);
    assert!(g.credential_in(&settled.body));
    assert!(!g.credential_in("{\"order_ref\":\"plain\"}"));
    // A credential given as a stock symbol lands in no file or query.
    let mut cmd = read_cmd();
    cmd.stocks[0].symbol = ck.clone();
    let r = run(cmd, |t| { t.reply("GET /v1/api/iserver/accounts", 200, accounts(&[LIVE])); script_reads(t, LIVE); }, None);
    assert!(r.result.unwrap_err().contains("credential") && !r.dir.path().join("read").exists());
    // The unsigned 401 read is checked as well.
    let r = run(read_cmd(), |t| { t.reply("GET /v1/api/iserver/accounts", 200, accounts(&[LIVE])); script_reads(t, LIVE); }, Some((401, deltabadger::crypto::VENUE_TEXT_REDACTED.into())));
    assert!(r.result.is_err() && !r.dir.path().join("read").exists());
}

#[test]
fn a_failed_run_leaves_the_earlier_set_untouched() {
    let r = run(read_cmd(), |t| { t.reply("GET /v1/api/iserver/accounts", 200, accounts(&[LIVE])); script_reads(t, LIVE); }, None);
    r.result.unwrap();
    let before = all_text(&r.dir.path().join("read"));
    // A second run into the same output fails on its account list.
    let (m, session) = fixture();
    let secrets = secrets(&m);
    let login = Arc::new(Login::new(m, prepared_credential_version(0, &[Some("t".into()), None, None, None, None, None, None])).unwrap());
    let t = scripted(&session);
    t.reply("GET /v1/api/iserver/accounts", 503, json!({}));
    let g = Guard::new(t.clone(), Mode::Read, login.sensitive());
    let clock = FixedClock("2026-11-16T15:00:00Z".parse().unwrap());
    let dh = session["dh_random"].as_str().unwrap().to_string();
    let s = Session::new(&g, login, &clock).with_dh_random(move || dh.clone());
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
    assert!(rt.block_on(record(&s, &g, &read_cmd(), None, &secrets, &mut Scrubber::new(b"other".to_vec(), 7), r.dir.path())).is_err());
    assert_eq!(all_text(&r.dir.path().join("read")), before);
}

// ---- the scrub per endpoint family

fn scrub_one(file: &str, ok: bool, v: Value) -> (Value, Scrubber) {
    let mut sc = Scrubber::new(b"test salt".to_vec(), 98);
    sc.reserve(&v);
    seed_account(&mut sc, LIVE).unwrap();
    let symbols = vec!["AAPL".to_string()];
    let out = scrub_ibkr(&mut sc, Ctx { file, ok, symbols: &symbols }, v).unwrap();
    (out, sc)
}

#[test]
fn prose_unknown_keys_and_public_looking_fields_are_never_kept() {
    let (v, sc) = scrub_one("rules.us.buy", true, json!({ "error": "Quantity 12 exceeds available funds", "description": "Order ABCDE has quantity 12",
        "CUSTOMORDER": { "quantity": 12 }, "12345": { "x": 1 }, "status": "Filled", "conid": 265598, "total": 7, "limit": 3 }));
    assert_eq!(v["error"], json!("[text]"));
    assert_ne!(v["description"], json!("Order ABCDE has quantity 12"));
    assert!(v.get("CUSTOMORDER").is_none() && v.get("12345").is_none(), "unknown keys replaced: {v}");
    assert_eq!((v["status"].clone(), v["conid"].clone()), (json!("Filled"), json!(265598)), "schema keys and enums survive");
    let replaced: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).filter(|k| !["error", "description", "status", "conid"].contains(k)).collect();
    assert_eq!(replaced.len(), 4);
    assert!(!v.to_string().contains("12") || !v.to_string().contains("Quantity"));
    assert!(sc.leak_check("x", &v, &[]).is_ok());
    // A decimal or an id equal to a contract fact elsewhere in the set is still replaced outside its field.
    let (v, _) = { let mut sc = Scrubber::new(b"s".to_vec(), 7); let public = vec!["AAPL".to_string()];
        let out = scrub_ibkr(&mut sc, Ctx { file: "trades", ok: true, symbols: &public }, json!([{ "commission": "0.01", "order_id": "265598", "displaySize": 100 }])).unwrap(); (out, sc) };
    assert!(v[0]["commission"] != json!("0.01") && v[0]["order_id"] != json!("265598") && v[0]["displaySize"] != json!(100), "{v}");
    // Kept fields only in a 2xx contract file: the same rules answer as a 4xx, or as a positions row, keeps no symbol.
    let (v, _) = scrub_one("rules.us.buy", false, json!({ "symbol": "TSLA", "priceMagnifier": 100 }));
    assert_ne!(v["symbol"], json!("TSLA"));
    let (v, _) = scrub_one("positions.0", true, json!([{ "ticker": "MSFT", "symbol": "AAPL", "name": "APPLE INC", "description": "Jan's account", "total": 3, "acctId": LIVE }]));
    assert_ne!(v[0]["ticker"], json!("MSFT"), "a ticker the run does not name is replaced outside contract files");
    assert_eq!(v[0]["symbol"], json!("AAPL"), "the run's own stock is public everywhere");
    assert_ne!(v[0]["description"], json!("Jan's account"));
    assert_ne!(v[0]["total"], json!(3), "a short amount is replaced");
    assert!(v[0]["acctId"].as_str().unwrap().starts_with('U') && v[0]["acctId"] != json!(LIVE));
    // A numeric key only in a snapshot row; a currency key anywhere.
    let (v, _) = scrub_one("snapshot.us.1", true, json!([{ "31": "1.5", "6509": "DpB", "nested": { "4242": 1 } }]));
    assert!(v[0].get("31").is_some() && v[0].get("6509").is_some() && v[0]["nested"].get("4242").is_none());
    let (v, _) = scrub_one("ledger", true, json!({ "EUR": { "cashbalance": 0 }, "BASE": {} }));
    assert!(v.get("EUR").is_some() && v.get("BASE").is_some() && v["EUR"]["cashbalance"] == json!(0));
}

#[test]
fn an_off_shape_value_under_a_kept_name_is_replaced() {
    let (v, _) = scrub_one("trades", true, json!([{ "side": "B", "status": "my order 7", "currency": "usd1", "messageIds": ["o163", "x9"], "conidex": "265598@SMART", "conid": "12a" }]));
    let r = &v[0];
    assert_eq!((r["side"].clone(), r["conidex"].clone(), r["messageIds"][0].clone()), (json!("B"), json!("265598@SMART"), json!("o163")));
    assert_eq!(r["status"], json!("[text]"));
    assert_ne!(r["currency"], json!("usd1"));
    assert_ne!(r["messageIds"][1], json!("x9"));
    assert_ne!(r["conid"], json!("12a"));
}

// ---- the write, the material and the command line

#[test]
fn write_set_keeps_the_old_set_on_a_failure_and_keeps_non_json_files() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("read");
    write_set(&dir, &[("a.json".into(), json!(1))]).unwrap();
    std::fs::write(dir.join("NOTES.txt"), "kept").unwrap();
    // A file that cannot be written (its name a missing subdir) fails after staging began.
    assert!(write_set(&dir, &[("b.json".into(), json!(2)), ("missing/c.json".into(), json!(3))]).is_err());
    assert_eq!(std::fs::read_to_string(dir.join("a.json")).unwrap(), "1\n");
    assert!(!dir.join("b.json").exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1, "no stage left behind");
    write_set(&dir, &[("b.json".into(), json!(2))]).unwrap();
    assert!(!dir.join("a.json").exists() && dir.join("b.json").exists());
    assert_eq!(std::fs::read_to_string(dir.join("NOTES.txt")).unwrap(), "kept");
}

#[test]
fn the_material_comes_from_a_dir_outside_the_repo_and_never_from_arguments() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    assert!(material(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"), &repo).err().unwrap().contains("outside the repository"));
    let outside = tempfile::tempdir().unwrap();
    assert!(material(outside.path(), &repo).err().unwrap().contains("consumer_key"));
    let (m, _) = fixture();
    for (f, v) in [("consumer_key", &m.consumer_key), ("access_token", &m.access_token), ("access_token_secret", &m.access_token_secret),
                   ("private_signature.pem", &m.signature_key), ("private_encryption.pem", &m.encryption_key), ("dhparam.pem", &m.dh_param)] {
        std::fs::write(outside.path().join(f), format!("{v}\n")).unwrap();
    }
    let read = material(outside.path(), &repo).unwrap();
    assert_eq!((read.consumer_key, read.realm), (m.consumer_key.clone(), String::new()));
    let s = secrets(&m);
    assert!(s.contains(&m.consumer_key) && s.iter().any(|l| m.signature_key.contains(l.as_str()) && l.len() >= 16 && !l.contains("-----")));
    for bad in ["--consumer-key=x", "--secret", "TOKEN", "./key.pem"] { assert!(refuse_key_args(&[bad.into()]).is_err(), "{bad}"); }
    assert!(refuse_key_args(&["read".into(), "--us".into(), "AAPL,265598,US0378331005".into()]).is_ok());
}

#[test]
fn the_command_line_names_its_stocks_and_paper_mode_its_limit() {
    let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
    let c = parse_args(args("read --us AAPL,265598,US0378331005 --eu SAP,14204,DE0007164600")).unwrap();
    assert_eq!((c.mode, c.stocks.len(), c.stocks[1].tag), (Mode::Read, 2, "eu"));
    let c = parse_args(args("paper --stock F,9599491,US3453708600 --limit 5.5 --untradable 416904")).unwrap();
    assert_eq!((c.limit.as_deref(), c.untradable), (Some("5.5"), Some(416904)));
    for bad in ["read --us AAPL,265598,US0378331005", "paper --stock F,9599491,US3453708600", "read --stock F,1,US3453708600", "paper --stock F,x,US3453708600 --limit 1",
                "paper --stock F,1,US3453708600 --limit -1", "read --us aapl,1,US0378331005 --eu SAP,14204,DE0007164600", "trade"] {
        assert!(parse_args(args(bad)).is_err(), "{bad}");
    }
}
