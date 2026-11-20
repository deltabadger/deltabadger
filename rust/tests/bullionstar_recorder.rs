//! The BullionStar recorder (rust/examples/record_bullionstar, PLAN-bullionstar.md slice 0): the Guard (allow-list,
//! orders only with testOrder: true, the PIN only in an order body), the arguments, and the read and orders flows over a
//! scripted transport, scrubbed and leak-checked. No live call anywhere.
#[path = "../examples/record_bullionstar/recorder.rs"]
mod recorder;

use deltabadger::venue::http::{HttpRequest, ScriptedTransport};
use recorder::*;
use serde_json::{json, Value};
use std::time::Duration;

const PIN: &str = "4821";
const KEY: &str = "test-api-key-0123456789abcdef";

fn opts() -> Opts { Opts { pause: Duration::ZERO, backoff: Duration::ZERO, wait_cap: Duration::from_secs(60), margin: Duration::ZERO } }
fn sc() -> Scrubber { Scrubber::new(b"test salt".to_vec(), 100).with_names(KEEP_NAMES, TIME_NAMES) }
fn req(method: &'static str, path: &str, body: Option<Value>) -> HttpRequest {
    HttpRequest { method, base: URL.into(), path: format!("{PREFIX}{path}"), query: vec![], body, not_after: None }
}
fn k(method: &str, path: &str) -> String { format!("{method} {PREFIX}{path}") }
fn files(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_dir(dir).unwrap().map(|f| {
        let p = f.unwrap().path();
        (p.file_name().unwrap().to_string_lossy().to_string(), std::fs::read_to_string(&p).unwrap())
    }).collect();
    v.sort();
    v
}

#[test]
fn the_guard_sends_an_order_only_as_a_test_order_in_orders_mode() {
    let order = |body| req("POST", "/checkout/buy/order", Some(body));
    let good = order(json!({"priceLockToken": "t", "pin": PIN, "testOrder": true}));
    assert!(check(&good, Mode::Orders, PIN).is_ok());
    assert!(check(&good, Mode::Read, PIN).is_err(), "no order outside orders mode");
    for bad in [json!({"priceLockToken": "t", "pin": PIN, "testOrder": false}), json!({"priceLockToken": "t", "pin": PIN}),
                json!({"priceLockToken": "t", "pin": PIN, "testOrder": "true"}), json!({"priceLockToken": "t", "pin": PIN, "testOrder": true, "x": 1})] {
        assert!(check(&order(bad.clone()), Mode::Orders, PIN).is_err(), "{bad}");
    }
    let sell = req("POST", "/checkout/vault-sell/order", Some(json!({"priceLockToken": "t", "pin": PIN, "testOrder": false})));
    assert!(check(&sell, Mode::Orders, PIN).is_err());
    let lock = req("POST", "/checkout/buy/price-lock-token", Some(json!({"products": [{"productId": PIN}]})));
    assert!(check(&lock, Mode::Orders, PIN).is_err(), "the PIN goes in no other body");
    let mut q = req("GET", "/order/get-by-id", None);
    q.query = vec![("orderId", PIN.into())];
    assert!(check(&q, Mode::Orders, PIN).is_err(), "the PIN goes in no query");
    assert!(check(&req("GET", "/product/detail/480", None), Mode::Read, "").is_ok());
    assert!(check(&req("GET", "/product/detail/../x", None), Mode::Read, "").is_err());
    assert!(check(&req("DELETE", "/vault/holdings", None), Mode::Orders, PIN).is_err());
    assert!(check(&req("GET", "/account/withdraw", None), Mode::Orders, PIN).is_err());
    let mut elsewhere = req("GET", "/vault/holdings", None);
    elsewhere.base = "https://api.bullionstar.com.evil.example".into();
    assert!(check(&elsewhere, Mode::Read, "").is_err(), "the key goes to the dealer's host only");
    assert!(check(&req("GET", &format!("/product/detail/{PIN}"), None), Mode::Read, PIN).is_err(), "nor the PIN in a path");
    assert!(check(&req("GET", "/product/detail/14821", None), Mode::Read, PIN).is_ok(), "an id holding its digits is no PIN");
}

#[test]
fn arguments_never_carry_a_credential() {
    assert!(refuse_key_args(&["read".into(), "--api-key".into()], "").is_err());
    assert!(refuse_key_args(&["orders".into(), "--x".into(), PIN.into()], PIN).is_err());
    assert!(refuse_key_args(&["orders".into(), "--wrong-pin".into()], PIN).is_ok());
    assert!(refuse_key_args(&["orders".into(), "--gram".into(), "14821".into()], PIN).is_ok(), "an id holding the PIN's digits is no PIN");
    assert_eq!(parse_args(vec!["probe".into()]).unwrap(), Command::Probe { label: "whitelisted".into(), location: None, product: None });
    assert!(parse_args(vec!["probe".into(), "--label".into(), "off_whitelist".into()]).is_ok());
    assert!(parse_args(vec!["probe".into(), "--label".into(), "a1b2c3d4e5".into()]).is_err(), "a label is never free text (a pasted key)");
    assert_eq!(parse_args(vec!["read".into(), "--details".into(), "5".into()]).unwrap(), Command::Read { details: Some(5) });
    let a: Vec<String> = "orders --location 1 --currency usd --gram 480 --bar 77 --payment 3 --shipping 4 --sell-payment 5 --expiry"
        .split(' ').map(String::from).collect();
    let Command::Orders(o) = parse_args(a).unwrap() else { panic!() };
    assert_eq!((o.location, o.currency.as_str(), o.gram, o.bar, o.expiry, o.wrong_pin), (1, "USD", 480, 77, true, false));
    assert!(parse_args(vec!["orders".into(), "--location".into(), "1".into()]).is_err());
    assert!(parse_args(vec!["read".into(), "--bogus".into(), "1".into()]).is_err());
}

fn read_script() -> ScriptedTransport { script(ScriptedTransport::default()) }
fn script(t: ScriptedTransport) -> ScriptedTransport {
    t.reply(&k("GET", "/stock-location/"), 200, json!([{"id": 1, "name": "Singapore"}]));
    t.reply(&k("GET", "/product/"), 200, json!({"items": [{"id": 480, "title": "1 g BSP", "url": "u", "locationId": 1, "categoryId": 2}], "totalCount": 1, "cursor": ""}));
    t.reply(&k("GET", "/product/detail/480"), 200, json!({"id": 480, "title": "1 g BSP", "locationId": 1, "metal": "XAG", "fineWeightAmount": 1, "fineWeightUnit": "G"}));
    t.reply(&k("GET", "/product/price/"), 200, json!([{"id": 480, "unitPrice": "1.31", "availableForPurchase": true}]));
    t.reply(&k("GET", "/shipping-method/list"), 200, json!([{"id": 4, "title": "Vault Storage"}]));
    t.reply(&k("GET", "/payment-method/list"), 200, json!([{"id": 3, "title": "SVF"}]));
    t.reply(&k("GET", "/sell-payment-method/list"), 200, json!([{"id": 5, "title": "SVF", "fields": []}]));
    t.reply(&k("GET", "/svf/holdings"), 200, json!({"currencies": [{"currency": "USD", "balance": [987.65, 12.5]}]}));
    t.reply(&k("GET", "/vault/holdings"), 200, json!({"locations": [{"location": {"id": 1, "name": "Singapore"},
        "王小明": 1, "products": [{"productId": 9480, "name": "王小明 1 g BSP", "quantity": 37, "serialNumbers": "SN-77812", "paid": true, "stored": true}]}]}));
    t
}

#[tokio::test(flavor = "current_thread")]
async fn a_read_keeps_the_catalogue_and_scrubs_the_account() {
    let out = tempfile::tempdir().unwrap();
    let g = Guard::new(read_script(), Mode::Read, String::new());
    let o = record::<_, ScriptedTransport>(&g, None, &Command::Read { details: None }, KEY, "", sc(), &opts(), out.path()).await.unwrap();
    let f = files(&out.path().join("read"));
    let get = |n: &str| serde_json::from_str::<Value>(&f.iter().find(|(name, _)| name == n).unwrap().1).unwrap();
    assert_eq!(get("detail.480.json")["metal"], "XAG", "the catalogue is kept as it came");
    let svf = get("svf_holdings.json");
    assert_eq!(svf["currencies"][0]["currency"], "USD");
    assert_ne!(svf["currencies"][0]["balance"][0], json!(987.65), "a balance is replaced");
    let vault = get("vault_holdings.json");
    assert_eq!(vault["locations"][0]["products"][0]["paid"], true);
    assert_ne!(vault["locations"][0]["products"][0]["serialNumbers"], "SN-77812");
    let all: String = f.iter().map(|(_, t)| t.as_str()).collect();
    assert!(!all.contains("SN-77812") && !all.contains("987.65") && !all.contains(KEY));
    assert!(!all.contains("王") && !all.contains("\\u738b"), "text in another script does not pass through");
    let obs = o.observations.join("\n");
    assert!(obs.contains("F3 product ids shared between locations: 0 (1 distinct ids)"), "{obs}");
    assert!(!obs.contains("INCOMPLETE"), "{obs}");
    assert!(obs.contains("F2 svf/holdings: HTTP 200; balance array lengths USD ×2"), "{obs}");
    assert!(obs.contains("F4 payment methods at 1 in USD: 3 SVF"), "{obs}");
}

#[tokio::test(flavor = "current_thread")]
async fn an_order_run_sends_test_orders_only_and_records_the_refusals() {
    let out = tempfile::tempdir().unwrap();
    let t = read_script();
    let lock = json!({"priceLockToken": "tok-abcdef-998877", "expireTime": "2020-01-01T00:00:00Z", "currency": "USD",
        "paymentMethod": {"id": 3, "title": "SVF"}, "products": [{"quantity": 1, "unitPriceAmount": 1.31, "totalPriceAmount": 1.31}],
        "costs": {"subTotal": "1.31", "shipping": "0.00", "totalToPay": "1.31"}, "total": "1.31", "totalAmount": 1.31});
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, lock.clone());
    let mut big = lock.clone();
    big["products"][0]["quantity"] = json!(1000);
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, lock.clone());
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, lock.clone());
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, big);
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, lock.clone());
    t.reply(&k("POST", "/checkout/vault-sell/price-lock-token"), 400, json!({"message": "Insufficient holdings", "requestId": "req-55aa66bb"}));
    let ack = json!({"order": {"id": 773311, "orderRef": "BS-REF-9911", "accountId": 445566, "status": 1, "statusString": "Pending", "total": "1.31", "currency": "USD",
        "products": [{"product": {"id": 9480, "title": "1 g BSP"}, "quantity": 1}]}});
    // The vault (read_script) names the same product by `productId`: both are replaced alike, so the leak check passes.
    t.reply(&k("POST", "/checkout/buy/order"), 200, ack.clone());
    t.reply(&k("POST", "/checkout/buy/order"), 400, json!({"message": "Price lock token already used", "requestId": "req-11cc22dd"}));
    t.reply(&k("POST", "/checkout/buy/order"), 400, json!({"message": "Order 773311 refused for account 445566", "requestId": "req-33ee44ff"}));
    t.reply(&k("GET", "/order/get-by-id"), 200, json!([ack["order"].clone()]));
    t.reply(&k("GET", "/order/get-by-order-ref"), 200, json!([ack["order"].clone()]));
    let g = Guard::new(t.clone(), Mode::Orders, PIN.into());
    let o = Order { location: 1, currency: "USD".into(), gram: 480, bar: 77, payment: 3, shipping: 4, sell_payment: 5, unavailable: None, wrong_pin: false, expiry: true };
    let r = record::<_, ScriptedTransport>(&g, None, &Command::Orders(o), KEY, PIN, sc(), &opts(), out.path()).await.unwrap();
    let orders: Vec<Value> = t.requests().into_iter().filter(|r| r.path.ends_with("/order")).filter_map(|r| r.body).collect();
    assert!(!orders.is_empty());
    assert!(orders.iter().all(|b| b["testOrder"] == true), "every order is a test order");
    assert!(t.requests().iter().filter(|r| !r.path.ends_with("/order")).all(|r| !r.body.as_ref().is_some_and(|b| b.to_string().contains(PIN))));
    let f = files(&out.path().join("orders"));
    let all: String = f.iter().map(|(_, t)| t.as_str()).collect();
    for original in ["773311", "BS-REF-9911", "445566", "tok-abcdef-998877", "req-55aa66bb", "req-33ee44ff", KEY] { assert!(!all.contains(original), "{original} survives"); }
    assert!(f.iter().any(|(n, _)| n == "lock_buy_insufficient_funds.json"), "a lock echoing 1000 pieces is written");
    assert!(all.contains("Price lock token already used") && all.contains("Insufficient holdings"), "error messages are kept");
    assert!(f.iter().any(|(n, _)| n == "order_buy_expired_token.json"));
    let obs = r.observations.join("\n");
    assert!(obs.contains("order_buy_repeat_token: HTTP 400 \"Price lock token already used\""), "{obs}");
    assert!(obs.contains("F12 buy: order.id present, accountId present, orderRef present"), "{obs}");
    assert!(obs.contains("F7 get-by-id buy: HTTP 200, an array of 1, the order found, status 1 \"Pending\""), "{obs}");
    assert!(obs.contains("F7 get-by-order-ref buy: HTTP 200, an array of 1, the order found"), "{obs}");
    assert!(obs.contains("F7 buy moved a balance: no"), "{obs}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_pin_echoed_in_an_answer_writes_nothing() {
    let out = tempfile::tempdir().unwrap();
    let t = read_script();
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 400, json!({"message": "bad", "pin": PIN}));
    t.reply(&k("POST", "/checkout/vault-sell/price-lock-token"), 400, json!({"message": "bad"}));
    let g = Guard::new(t, Mode::Orders, PIN.into());
    let o = Order { location: 1, currency: "USD".into(), gram: 480, bar: 77, payment: 3, shipping: 4, sell_payment: 5, ..Order::default() };
    let e = record::<_, ScriptedTransport>(&g, None, &Command::Orders(o), KEY, PIN, sc(), &opts(), out.path()).await.err().unwrap();
    assert!(e.contains("nothing written"), "{e}");
    assert!(!out.path().join("orders").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn a_read_is_retried_after_a_429_and_counted() {
    let out = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    t.reply(&k("GET", "/stock-location/"), 429, json!({"message": "Too many requests"}));
    let g = Guard::new(script(t.clone()), Mode::Read, String::new());
    let o = record::<_, ScriptedTransport>(&g, None, &Command::Read { details: Some(1) }, KEY, "", sc(), &opts(), out.path()).await.unwrap();
    assert_eq!(t.requests().iter().filter(|r| r.path.ends_with("/stock-location/")).count(), 2);
    assert!(o.observations.iter().any(|l| l == "F11: 1 answers were 429"), "{:?}", o.observations);
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_first_test_order_sends_no_other() {
    let out = tempfile::tempdir().unwrap();
    let t = read_script();
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, json!({"priceLockToken": "tok-abcdef-998877", "expireTime": "2020-01-01T00:00:00Z"}));
    t.reply(&k("POST", "/checkout/buy/order"), 400, json!({"message": "Invalid PIN", "requestId": "req-55aa66bb"}));
    let g = Guard::new(t.clone(), Mode::Orders, PIN.into());
    let o = Order { location: 1, currency: "USD".into(), gram: 480, bar: 77, payment: 3, shipping: 4, sell_payment: 5, wrong_pin: true, expiry: true, ..Order::default() };
    let r = record::<_, ScriptedTransport>(&g, None, &Command::Orders(o), KEY, PIN, sc(), &opts(), out.path()).await.unwrap();
    assert_eq!(t.requests().iter().filter(|r| r.path.ends_with("/order")).count(), 1, "the PIN is sent once");
    assert!(r.observations.iter().any(|l| l.contains("no further order is sent")));
    let all: String = files(&out.path().join("orders")).iter().map(|(_, t)| t.clone()).collect();
    assert!(all.contains("Invalid PIN"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_public_read_is_scrubbed() {
    let out = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    t.reply(&k("GET", "/stock-location/"), 403, json!({"message": "Forbidden for 142.132.159.124", "requestId": "req-99887766"}));
    let g = Guard::new(t, Mode::Read, String::new());
    let e = record::<_, ScriptedTransport>(&g, None, &Command::Read { details: None }, KEY, "", sc(), &opts(), out.path()).await;
    assert!(e.is_err(), "a read without locations stops");
    // A probe whose keyed product read fails: that answer is scrubbed, its message (an IP) replaced.
    let t = ScriptedTransport::default();
    t.reply(&k("GET", "/stock-location/"), 200, json!([{"id": 1, "name": "Singapore"}]));
    t.reply(&k("GET", "/product/"), 403, json!({"message": "Forbidden for 142.132.159.124", "requestId": "req-99887766"}));
    t.reply(&k("GET", "/vault/holdings"), 200, json!({"locations": []}));
    let anon = ScriptedTransport::default();
    anon.reply(&k("GET", "/stock-location/"), 200, json!([{"id": 1, "name": "Singapore"}]));
    anon.reply(&k("GET", "/product/"), 401, json!({"message": "Unauthorized", "requestId": "req-12121212"}));
    anon.reply(&k("GET", "/vault/holdings"), 401, json!({"message": "Unauthorized", "requestId": "req-12121212"}));
    let g = Guard::new(t, Mode::Probe, String::new());
    let o = record(&g, Some((&anon, &anon)), &Command::Probe { label: "whitelisted".into(), location: None, product: None }, KEY, "", sc(), &opts(), out.path()).await.unwrap();
    let f = files(&out.path().join("probe_whitelisted"));
    let all: String = f.iter().map(|(_, t)| t.clone()).collect();
    for original in ["142.132.159.124", "req-99887766", "req-12121212"] { assert!(!all.contains(original), "{original} survives"); }
    assert!(all.contains("\"Unauthorized\""));
    assert!(o.observations.iter().any(|l| l == "F1 no_key: GET /product/ → HTTP 401 \"Unauthorized\""), "{:?}", o.observations);
    assert!(o.observations.iter().any(|l| l == "F1 wrong_key: GET /vault/holdings → HTTP 401 \"Unauthorized\""), "{:?}", o.observations);
    assert!(o.observations.iter().any(|l| l.contains("message withheld")), "the IP message is not printed: {:?}", o.observations);
    assert!(plain_message("Price lock token already used"));
    for m in ["Forbidden for 142.132.159.124", "Forbidden for 10.0.0.1", "Wrong PIN 4821", "mail someone@example.com", "key abcdefghijklmnopqrstuvwx refused", "名前"] {
        assert!(!plain_message(m), "{m}");
    }
    assert_eq!(message(400, &json!({"message": "Wrong PIN 4821"})), " (message withheld: it holds digits, an address or a long token)");
}

#[tokio::test(flavor = "current_thread")]
async fn a_test_order_that_moves_a_balance_stops_the_run() {
    let out = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    t.reply(&k("GET", "/vault/holdings"), 200, json!({"locations": []}));
    t.reply(&k("GET", "/vault/holdings"), 200, json!({"locations": [{"location": {"id": 1}, "products": [{"productId": 480, "quantity": 1, "paid": true}]}]}));
    t.reply(&k("GET", "/svf/holdings"), 200, json!({"currencies": []}));
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, json!({"priceLockToken": "tok-abcdef-998877", "expireTime": "2020-01-01T00:00:00Z"}));
    t.reply(&k("POST", "/checkout/buy/order"), 200, json!({"order": {"id": 773311, "accountId": 445566, "status": 1}}));
    t.reply(&k("GET", "/order/get-by-id"), 200, json!([{"id": 773311, "status": 1}]));
    let g = Guard::new(t.clone(), Mode::Orders, PIN.into());
    let o = Order { location: 1, currency: "USD".into(), gram: 480, bar: 77, payment: 3, shipping: 4, sell_payment: 5, ..Order::default() };
    let r = record::<_, ScriptedTransport>(&g, None, &Command::Orders(o), KEY, PIN, sc(), &opts(), out.path()).await.unwrap();
    assert_eq!(t.requests().iter().filter(|r| r.path.ends_with("/order")).count(), 1);
    assert!(r.observations.iter().any(|l| l == "F7 buy moved a balance: YES"), "{:?}", r.observations);
}

#[tokio::test(flavor = "current_thread")]
async fn an_ambiguous_sell_answer_stops_the_run() {
    let out = tempfile::tempdir().unwrap();
    let t = read_script();
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, json!({"priceLockToken": "tok-abcdef-998877", "expireTime": "2020-01-01T00:00:00Z"}));
    t.reply(&k("POST", "/checkout/vault-sell/price-lock-token"), 200, json!({"priceLockToken": "tok-fedcba-112233", "expireTime": "2020-01-01T00:00:00Z"}));
    t.reply(&k("POST", "/checkout/buy/order"), 200, json!({"order": {"id": 773311, "accountId": 445566, "status": 1}}));
    t.reply(&k("POST", "/checkout/buy/order"), 400, json!({"message": "Price lock token already used"}));
    t.reply(&k("POST", "/checkout/vault-sell/order"), 500, json!({"message": "Internal error"}));
    t.reply(&k("GET", "/order/get-by-id"), 200, json!([{"id": 773311, "status": 1}]));
    let g = Guard::new(t.clone(), Mode::Orders, PIN.into());
    let o = Order { location: 1, currency: "USD".into(), gram: 480, bar: 77, payment: 3, shipping: 4, sell_payment: 5, wrong_pin: true, ..Order::default() };
    let r = record::<_, ScriptedTransport>(&g, None, &Command::Orders(o), KEY, PIN, sc(), &opts(), out.path()).await.unwrap();
    assert_eq!(t.requests().iter().filter(|r| r.path.ends_with("/order")).count(), 3, "buy, repeat, sell: nothing after the 500");
    assert!(r.observations.iter().any(|l| l.contains("sell: HTTP 500 is neither an acknowledgement nor a definite refusal")), "{:?}", r.observations);
}

#[tokio::test(flavor = "current_thread")]
async fn a_malformed_catalogue_writes_nothing_and_a_message_is_judged_as_written() {
    let out = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    t.reply(&k("GET", "/stock-location/"), 200, json!({"unexpected": true}));
    let g = Guard::new(t, Mode::Read, String::new());
    let e = record::<_, ScriptedTransport>(&g, None, &Command::Read { details: None }, KEY, "", sc(), &opts(), out.path()).await.err().unwrap();
    assert!(e.contains("nothing written"), "{e}");
    assert!(!out.path().join("read").exists());
    // "José" is not ASCII: the message is scrubbed, never kept as "Josx".
    let t = read_script();
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 400, json!({"message": "Account José blocked"}));
    t.reply(&k("POST", "/checkout/vault-sell/price-lock-token"), 400, json!({"message": "Account José blocked"}));
    let g = Guard::new(t, Mode::Orders, PIN.into());
    let o = Order { location: 1, currency: "USD".into(), gram: 480, bar: 77, payment: 3, shipping: 4, sell_payment: 5, ..Order::default() };
    let r = record::<_, ScriptedTransport>(&g, None, &Command::Orders(o), KEY, PIN, sc(), &opts(), out.path()).await.unwrap();
    let all: String = files(&out.path().join("orders")).iter().map(|(_, t)| t.clone()).collect();
    assert!(!all.contains("Jos") && !all.contains("blocked"), "the message is scrubbed whole");
    assert!(r.observations.last().unwrap().ends_with("none"), "{:?}", r.observations);
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_order_answer_stops_and_says_to_check_the_account() {
    let out = tempfile::tempdir().unwrap();
    let t = read_script();
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, json!({"priceLockToken": "tok-abcdef-998877", "expireTime": "2020-01-01T00:00:00Z"}));
    t.network(&k("POST", "/checkout/buy/order"), "post_send", "timeout");
    let g = Guard::new(t.clone(), Mode::Orders, PIN.into());
    let o = Order { location: 1, currency: "USD".into(), gram: 480, bar: 77, payment: 3, shipping: 4, sell_payment: 5, ..Order::default() };
    let e = record::<_, ScriptedTransport>(&g, None, &Command::Orders(o), KEY, PIN, sc(), &opts(), out.path()).await.err().unwrap();
    assert!(e.contains("may have reached the dealer") && e.contains("1 test order(s) were sent in this run: check the account"), "{e}");
    assert_eq!(t.requests().iter().filter(|r| r.path.ends_with("/order")).count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn an_incomplete_catalogue_or_an_html_page_is_not_written_as_it_came() {
    let out = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    t.reply(&k("GET", "/product/"), 200, json!({"items": [{"id": 480}], "totalCount": 3, "cursor": ""}));
    let g = Guard::new(script(t), Mode::Read, String::new());
    let e = record::<_, ScriptedTransport>(&g, None, &Command::Read { details: None }, KEY, "", sc(), &opts(), out.path()).await.err().unwrap();
    assert!(e.contains("totalCount says Some(3); nothing written"), "{e}");
    // A 200 that is an HTML page (a challenge naming the caller's IP) is scrubbed like an account answer.
    let t = ScriptedTransport::default();
    t.reply_typed(&k("GET", "/stock-location/"), 200, "text/html", "<html>blocked 142.132.159.124</html>");
    t.reply(&k("GET", "/vault/holdings"), 200, json!({"locations": []}));
    let anon = ScriptedTransport::default();
    anon.reply(&k("GET", "/stock-location/"), 401, json!({"message": "Unauthorized"}));
    anon.reply(&k("GET", "/vault/holdings"), 401, json!({"message": "Unauthorized"}));
    let g = Guard::new(t, Mode::Probe, String::new());
    record(&g, Some((&anon, &anon)), &Command::Probe { label: "whitelisted".into(), location: None, product: None }, KEY, "", sc(), &opts(), out.path()).await.unwrap();
    let all: String = files(&out.path().join("probe_whitelisted")).iter().map(|(_, t)| t.clone()).collect();
    assert!(!all.contains("142.132.159.124") && !all.contains("blocked"), "the page is scrubbed");
}

#[tokio::test(flavor = "current_thread")]
async fn a_held_product_listed_in_the_catalogue_does_not_fail_the_read() {
    // read_script's vault holds product 9480; the catalogue lists it too, so its detail file and path name it.
    let out = tempfile::tempdir().unwrap();
    let t = ScriptedTransport::default();
    t.reply(&k("GET", "/product/"), 200, json!({"items": [{"id": 9480, "title": "1 g BSP", "locationId": 1}], "totalCount": 1, "cursor": ""}));
    t.reply(&k("GET", "/product/detail/9480"), 200, json!({"id": 9480, "metal": "XAG", "locationId": 1}));
    let g = Guard::new(script(t), Mode::Read, String::new());
    record::<_, ScriptedTransport>(&g, None, &Command::Read { details: None }, KEY, "", sc(), &opts(), out.path()).await.unwrap();
    assert!(out.path().join("read/detail.9480.json").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn an_unrecognised_refusal_stops_and_a_connect_failure_is_no_sent_order() {
    let out = tempfile::tempdir().unwrap();
    let t = read_script();
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, json!({"priceLockToken": "tok-abcdef-998877", "expireTime": "2020-01-01T00:00:00Z"}));
    t.reply(&k("POST", "/checkout/buy/order"), 200, json!({"order": {"id": 773311, "accountId": 445566, "status": 1}}));
    t.reply(&k("POST", "/checkout/buy/order"), 408, json!({"message": "Request timeout"}));
    t.reply(&k("GET", "/order/get-by-id"), 200, json!([{"id": 773311, "status": 1}]));
    let g = Guard::new(t.clone(), Mode::Orders, PIN.into());
    let o = Order { location: 1, currency: "USD".into(), gram: 480, bar: 77, payment: 3, shipping: 4, sell_payment: 5, ..Order::default() };
    record::<_, ScriptedTransport>(&g, None, &Command::Orders(o.clone()), KEY, PIN, sc(), &opts(), out.path()).await.unwrap();
    assert_eq!(t.requests().iter().filter(|r| r.path.ends_with("/order")).count(), 2, "nothing after the 408");

    let t = read_script();
    t.reply(&k("POST", "/checkout/buy/price-lock-token"), 200, json!({"priceLockToken": "tok-abcdef-998877", "expireTime": "2020-01-01T00:00:00Z"}));
    t.network(&k("POST", "/checkout/buy/order"), "pre_send", "connect refused");
    let g = Guard::new(t, Mode::Orders, PIN.into());
    let e = record::<_, ScriptedTransport>(&g, None, &Command::Orders(o), KEY, PIN, sc(), &opts(), out.path()).await.err().unwrap();
    assert!(e.contains("not sent") && !e.contains("were sent in this run"), "{e}");
}
