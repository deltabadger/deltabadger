use deltabadger::engine::model::Ticker;
use deltabadger::ruby::BigDec;
use deltabadger::venue::fake::{kraken_order, AddOutcome, FakeVenue};
use deltabadger::venue::*;
use serde_json::json;

fn xbteur() -> Ticker {
    Ticker { id: 1, ticker: "XBTEUR".into(), base_code: "XBT".into(), base_symbol: "BTC".into(), quote_symbol: "EUR".into(), exchange_name: "Kraken".into(),
             base_asset_id: 1, quote_asset_id: 2, base_decimals: 8, quote_decimals: 5, price_decimals: 1,
             minimum_base_size: BigDec::parse("0.00005").unwrap(), minimum_quote_size: BigDec::parse("0.5").unwrap(), trading_enabled: true, available: true }
}
fn bd(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn order(cl: &str) -> NewOrder {
    NewOrder { pair: "XBTEUR".into(), kind: OrderKind::Market, volume: "0.0012".into(), quote_volume: false,
               cl_ord_id: cl.into(), deadline: "2026-09-30T12:00:10Z".parse().unwrap() }
}
fn at() -> chrono::DateTime<chrono::Utc> { "2026-09-30T11:00:00Z".parse().unwrap() }

#[tokio::test(flavor = "current_thread")]
async fn raw_kraken_bodies_are_parsed_as_rails_parses_them() {
    let v = FakeVenue::from_script(&json!({ "http": {
        "/0/public/Ticker": [{ "error": [], "result": { "XXBTZEUR": { "a": ["50000.2", "1", "1.000"], "b": ["49990.1", "1", "1.000"], "c": ["49995.3", "0.001"], "v": ["12.5", "30.1"], "p": ["49995.3", "49995.3"], "t": [100, 250], "l": ["49995.3", "49995.3"], "h": ["49995.3", "49995.3"], "o": "49995.3" } } }],
        "/0/private/AddOrder": [{ "error": [], "result": { "descr": { "order": "buy" }, "txid": ["OTX-1"] } }, { "error": ["EOrder:Insufficient funds"] }],
        "/0/private/QueryOrders": [{ "error": [], "result": { "OTX-1": { "status": "closed", "price": "50010.5", "vol": "60", "vol_exec": "0.00119975",
            "cost": "60.0", "oflags": "fciq,viqc", "descr": { "pair": "XBTEUR", "type": "buy", "ordertype": "market", "price": "0" } } } }],
        "/0/private/BalanceEx": [{ "error": [], "result": { "ZEUR": { "balance": "1000.5", "hold_trade": "0.5" } } }]
    }}));
    assert_eq!(v.price(&xbteur(), PriceSide::Ask).await.unwrap(), bd("50000.2"));
    assert_eq!(v.price(&xbteur(), PriceSide::Last).await.unwrap(), bd("49995.3"));
    assert_eq!(v.add_order(&order("c-1")).await, Ok("OTX-1".into()));
    assert_eq!(v.add_order(&order("c-2")).await, Err(VenueError::Rejected(vec!["EOrder:Insufficient funds".into()])));
    let o = &v.orders(&["OTX-1".into()]).await.unwrap()[0];
    assert_eq!((o.status, o.quote_amount.clone(), o.amount.clone()), (OrderStatus::Closed, Some(bd("60")), None));
    assert_eq!(v.balance("EUR").await.unwrap(), bd("1000"));
    assert_eq!(v.balance("USD").await.unwrap(), BigDec::zero(), "absent from BalanceEx: Rails' {{ free: 0 }} default");
}

#[tokio::test(flavor = "current_thread")]
async fn a_limit_order_without_a_fill_takes_its_price_from_the_description() {
    let o = kraken_order("O1", &json!({ "status": "open", "price": "0", "vol": "0.001", "vol_exec": "0", "cost": "0", "oflags": "",
                                        "descr": { "ordertype": "limit", "price": "49870.0" } }));
    assert_eq!((o.price, o.amount, o.limit), (Some(bd("49870")), Some(bd("0.001")), true));
}

#[tokio::test(flavor = "current_thread")]
async fn accepted_and_lost_replies_are_on_the_book_and_found_by_client_id() {
    let v = FakeVenue::new().next_add(AddOutcome::Accept("OTX-1".into())).next_add(AddOutcome::AmbiguousPlaced("OTX-9".into()));
    assert_eq!(v.add_order(&order("c-1")).await, Ok("OTX-1".into()));
    assert!(matches!(v.add_order(&order("c-9")).await, Err(VenueError::Ambiguous(_))));
    assert_eq!(v.order_by_client_id("c-9", at()).await.unwrap().unwrap().txid, "OTX-9");
    assert_eq!(v.order_by_client_id("c-nope", at()).await.unwrap(), None);
    assert_eq!(v.sent().len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_lookup_is_an_error_never_a_partial_answer() {
    let v = FakeVenue::new().lookup_fails(1);
    assert!(v.order_by_client_id("c-1", at()).await.is_err());
    assert_eq!(v.order_by_client_id("c-1", at()).await.unwrap(), None);
}

#[tokio::test(flavor = "current_thread")]
async fn trades_history_pages_until_its_count() {
    let page = |ids: &[(&str, &str)], count: u64| json!({ "error": [], "result": { "count": count, "trades":
        ids.iter().map(|(t, cost)| (t.to_string(), json!({ "ordertxid": "OTX-9", "vol": "0.0006", "cost": cost, "fee": "0.078", "type": "buy", "ordertype": "market" })))
           .collect::<serde_json::Map<_, _>>() } });
    let v = FakeVenue::from_script(&json!({ "http": { "/0/private/TradesHistory": [page(&[("T1", "30")], 2), page(&[("T2", "30.5")], 2)] } }));
    let fills = v.fills_from_trades(&["OTX-9".into()], at()).await.unwrap();
    assert_eq!(fills[0].quote_amount_exec.to_s_f(), "60.5", "both pages");
}

#[tokio::test(flavor = "current_thread")]
async fn trades_history_recovers_fills_per_order() {
    let v = FakeVenue::from_script(&json!({ "http": { "/0/private/TradesHistory": [{ "error": [], "result": { "count": 2, "trades": {
        "T1": { "ordertxid": "OTX-5", "vol": "0.0006", "cost": "30.0", "fee": "0.078", "type": "buy", "ordertype": "market", "pair": "XXBTZEUR" },
        "T2": { "ordertxid": "OTX-5", "vol": "0.0006", "cost": "30.012", "fee": "0.078", "type": "buy", "ordertype": "market", "pair": "XXBTZEUR" } } } }] } }));
    let fills = v.fills_from_trades(&["OTX-5".into()], at()).await.unwrap();
    assert_eq!((fills[0].status, fills[0].amount_exec.to_s_f(), fills[0].quote_amount_exec.to_s_f()), (OrderStatus::Closed, "0.0012".into(), "60.012".into()));
    assert_eq!(fills[0].price, BigDec::parse("60.012").unwrap().div(&bd("0.0012")));
}

fn raw(status: &str, ordertype: &str) -> serde_json::Value {
    json!({ "status": status, "price": "0", "vol": "1", "vol_exec": "0", "cost": "0", "oflags": "", "descr": { "ordertype": ordertype, "price": "0", "type": "buy" } })
}

#[tokio::test(flavor = "current_thread")]
async fn an_unknown_status_or_order_type_is_an_error_not_a_guess() {
    let v = FakeVenue::new().order("O1", raw("weird", "market")).order("O2", raw("open", "stop-loss")).order("O3", raw("pending", "market"));
    assert!(v.orders(&["O1".into()]).await.is_err());
    assert!(v.orders(&["O2".into()]).await.is_err());
    assert_eq!(v.orders(&["O3".into()]).await.unwrap()[0].status, OrderStatus::Unknown);
}

#[tokio::test(flavor = "current_thread")]
async fn a_body_without_error_or_result_is_unreadable() {
    let v = FakeVenue::from_script(&json!({ "http": { "/0/public/Ticker": [{ "result": {} }], "/0/private/BalanceEx": [{ "error": [] }] } }));
    let unreadable = VenueError::Ambiguous("Kraken: unreadable response".into());
    assert_eq!(v.price(&xbteur(), PriceSide::Ask).await, Err(unreadable.clone()));
    assert_eq!(v.balance("EUR").await, Err(unreadable));
}

#[tokio::test(flavor = "current_thread")]
async fn the_client_id_lookup_never_goes_through_query_orders() {
    let hits = std::rc::Rc::new(std::cell::Cell::new(0));
    let h = hits.clone();
    let v = FakeVenue::from_script(&json!({ "http": { "/0/private/QueryOrders": [{ "error": [], "result": {} }] } })).on_query(move || h.set(h.get() + 1));
    v.add_order(&order("c-1")).await.unwrap();
    assert!(v.order_by_client_id("c-1", at()).await.unwrap().is_some());
    assert_eq!((v.calls("/0/private/QueryOrders"), hits.get()), (0, 0));
}

#[tokio::test(flavor = "current_thread")]
async fn the_lookup_finds_an_order_in_a_scripted_closed_orders_body() {
    let mut closed = raw("closed", "market");
    closed["cl_ord_id"] = json!("c-7");
    let v = FakeVenue::from_script(&json!({ "http": {
        "/0/private/OpenOrders": [{ "error": [], "result": { "open": {} } }],
        "/0/private/ClosedOrders": [{ "error": [], "result": { "closed": { "OTX-7": closed }, "count": 1 } }] } }));
    let o = v.order_by_client_id("c-7", at()).await.unwrap().unwrap();
    assert_eq!((o.txid.as_str(), o.status), ("OTX-7", OrderStatus::Closed));
    assert_eq!(v.order_by_client_id("c-8", at()).await.unwrap(), None);
}

#[tokio::test(flavor = "current_thread")]
async fn a_registered_filled_order_is_reported_closed_and_a_duplicate_client_id_is_refused() {
    let v = FakeVenue::new().next_add(AddOutcome::AmbiguousPlaced("OTX-3".into())).order("OTX-3", raw("closed", "market"));
    assert!(v.add_order(&order("c-3")).await.is_err());
    assert_eq!(v.order_by_client_id("c-3", at()).await.unwrap().unwrap().status, OrderStatus::Closed);
    assert_eq!(v.add_order(&order("c-3")).await, Err(VenueError::Rejected(vec!["EOrder:Duplicate order".into()])));
    assert_eq!(v.sent().len(), 1, "the duplicate was not booked or sent");
}

#[tokio::test(flavor = "current_thread")]
async fn balance_splits_dotted_codes_and_the_last_row_wins() {
    let v = FakeVenue::new().balance_body("EUR.F", "10", "1");
    assert_eq!(v.balance("EUR").await.unwrap(), bd("9"));
    let v = FakeVenue::from_script(&json!({ "http": { "/0/private/BalanceEx": [{ "error": [], "result": {
        "ZEUR": { "balance": "100", "hold_trade": "0" }, "EUR.HOLD": { "balance": "7", "hold_trade": "2" } } }] } }));
    assert_eq!(v.balance("EUR").await.unwrap(), bd("5"));
}
