use deltabadger::ruby::BigDec;
use deltabadger::venue::fake::{kraken_order, AddOutcome, FakeVenue};
use deltabadger::venue::*;
use serde_json::json;

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
    assert_eq!(v.prices("XBTEUR").await.unwrap(), Prices { bid: bd("49990.1"), ask: bd("50000.2"), last: bd("49995.3") });
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
