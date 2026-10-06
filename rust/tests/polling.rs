mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::polling::{self, PollFailure};
use deltabadger::engine::model;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::FakeVenue;
use serde_json::json;

fn now() -> DateTime<Utc> { "2026-09-30T12:00:00Z".parse().unwrap() }
fn setup(transient: serde_json::Value) -> (tempfile::TempDir, store::Opened, seed::Seeded, i64) {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00");
    spec.transient = transient;
    let bot = seed::insert_bot(&o.primary, &s, &spec);
    (dir, o, s, bot)
}
fn open_market(created: &str, id: &str) -> TxSpec {
    TxSpec { status: 0, external_status: Some(0), external_id: Some(id.into()), order_type: 0, amount: Some("0.0012"), quote_amount: Some("60"),
             price: Some("50000"), quote_amount_exec: None, amount_exec: None, created_at: created.into() }
}
fn closed_raw(price: &str, vol_exec: &str, cost: &str) -> serde_json::Value {
    json!({ "status": "closed", "price": price, "vol": "60", "vol_exec": vol_exec, "cost": cost, "oflags": "viqc", "descr": { "type": "buy", "ordertype": "market", "price": "0" } })
}
fn bot(o: &store::Opened, id: i64) -> model::Bot { model::load_bot(&o.primary, id).unwrap() }

#[tokio::test(flavor = "current_thread")]
async fn a_closed_fill_updates_the_row_and_leaves_the_carry_alone() {
    let (_d, o, s, b) = setup(json!({ "missed_quote_amount": "100.0" }));
    let tx = seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-29 10:00:00", "OTX-1"));
    let v = FakeVenue::new().order("OTX-1", closed_raw("50010.5", "0.00119975", "60.0"));
    polling::sweep(&o.primary, &v, &bot(&o, b), now()).await.unwrap();
    let (ext, price, qexec): (i64, f64, f64) = o.primary.query_row("SELECT external_status, price, quote_amount_exec FROM transactions WHERE id = ?1", [tx], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((ext, price, qexec), (2, 50010.5, 60.0));
    assert_eq!(bot(&o, b).transient["missed_quote_amount"], json!("100.0"), "Rails (#448): a fill is credited by its own row, never by the carry");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unchanged_order_is_not_rewritten() {
    let (_d, o, s, b) = setup(json!({}));
    let tx = seed::insert_tx(&o.primary, &s, b, &TxSpec { external_status: Some(1), order_type: 1, quote_amount: None, amount_exec: Some("0"),
        quote_amount_exec: Some("0"), ..open_market("2026-09-29 10:00:00", "OTX-3") });
    let v = FakeVenue::new().order("OTX-3", json!({ "status": "open", "price": "0", "vol": "0.0012", "vol_exec": "0", "cost": "0", "oflags": "",
                                                     "descr": { "type": "buy", "ordertype": "limit", "price": "50000" } }));
    let before: String = o.primary.query_row("SELECT updated_at FROM transactions WHERE id = ?1", [tx], |r| r.get(0)).unwrap();
    polling::sweep(&o.primary, &v, &bot(&o, b), now()).await.unwrap();
    let after: String = o.primary.query_row("SELECT updated_at FROM transactions WHERE id = ?1", [tx], |r| r.get(0)).unwrap();
    assert_eq!(before, after, "Rails' update with no changes writes nothing");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unknown_status_fails_the_sweep_with_rails_message() {
    let (_d, o, s, b) = setup(json!({}));
    seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-29 10:00:00", "OTX-4"));
    let v = FakeVenue::new().order("OTX-4", json!({ "status": "pending", "price": "0", "vol": "60", "vol_exec": "0", "cost": "0", "oflags": "viqc", "descr": { "ordertype": "market" } }));
    assert_eq!(polling::sweep(&o.primary, &v, &bot(&o, b), now()).await, Err(PollFailure::General("Order OTX-4 status is unknown.".into())));
}

#[tokio::test(flavor = "current_thread")]
async fn a_kraken_error_is_classified_like_fetch_and_update_open_orders_job() {
    let (_d, o, s, b) = setup(json!({}));
    seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-29 10:00:00", "OTX-5"));
    let with = |err: &str| FakeVenue::from_script(&json!({ "http": { "/0/private/QueryOrders": [{ "error": [err] }] } }));
    assert_eq!(polling::sweep(&o.primary, &with("EAPI:Rate limit exceeded"), &bot(&o, b), now()).await, Err(PollFailure::RateLimited("EAPI:Rate limit exceeded".into())));
    assert_eq!(polling::sweep(&o.primary, &with("EService:Unavailable"), &bot(&o, b), now()).await, Err(PollFailure::Transient("EService:Unavailable".into())));
    assert_eq!(polling::sweep(&o.primary, &with("EOrder:Invalid order"), &bot(&o, b), now()).await,
               Err(PollFailure::General("Failed to fetch orders OTX-5. Result: [\"EOrder:Invalid order\"]".into())));
}

#[tokio::test(flavor = "current_thread")]
async fn a_missing_order_is_abandoned_only_after_fourteen_days() {
    let (_d, o, s, b) = setup(json!({}));
    let young = seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-20 10:00:00", "OTX-Y"));
    let old = seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-10 10:00:00", "OTX-O"));
    polling::sweep(&o.primary, &FakeVenue::new(), &bot(&o, b), now()).await.unwrap();
    let ext = |id: i64| -> i64 { o.primary.query_row("SELECT external_status FROM transactions WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
    assert_eq!((ext(young), ext(old)), (0, 4));
    let logged: String = o.primary.query_row("SELECT details FROM bot_activity_logs WHERE event = 'order_abandoned'", [], |r| r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&logged).unwrap(), json!({ "order_id": "OTX-O" }));
}

#[tokio::test(flavor = "current_thread")]
async fn a_follow_up_recovers_a_fill_queryorders_dropped_from_trades_history() {
    let (_d, o, s, b) = setup(json!({}));
    seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-29 10:00:00", "OTX-7"));
    let v = FakeVenue::from_script(&json!({ "http": { "/0/private/TradesHistory": [{ "error": [], "result": { "count": 1, "trades": {
        "T1": { "ordertxid": "OTX-7", "vol": "0.0012", "cost": "60", "fee": "0.156", "type": "buy", "ordertype": "market" } } } }] } }));
    let tx: i64 = o.primary.query_row("SELECT id FROM transactions WHERE external_id = 'OTX-7'", [], |r| r.get(0)).unwrap();
    polling::follow_up(&o.primary, &v, b, tx, now()).await.unwrap();
    let ext: i64 = o.primary.query_row("SELECT external_status FROM transactions WHERE external_id = 'OTX-7'", [], |r| r.get(0)).unwrap();
    assert_eq!(ext, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_follow_up_polls_only_its_own_order() {
    let (_d, o, s, b) = setup(json!({}));
    let mine = seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-29 10:00:00", "OTX-M"));
    let other = seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-29 10:00:00", "OTX-OTHER"));
    // OTX-OTHER is missing from QueryOrders and its TradesHistory fallback fails: batched, that would lose OTX-M's fill.
    let v = FakeVenue::from_script(&json!({ "http": {
        "/0/private/QueryOrders": [{ "error": [], "result": { "OTX-M": { "status": "closed", "price": "50000", "vol": "0.0012", "vol_exec": "0.0012",
            "cost": "60", "oflags": "", "descr": { "type": "buy", "ordertype": "market", "price": "0" } } } }],
        "/0/private/TradesHistory": [{ "error": ["EService:Unavailable"] }] } }));
    polling::follow_up(&o.primary, &v, b, mine, now()).await.unwrap();
    let ext = |id: i64| -> i64 { o.primary.query_row("SELECT external_status FROM transactions WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
    assert_eq!((ext(mine), ext(other)), (2, 0), "OTX-OTHER is its own job's business");
}

#[tokio::test(flavor = "current_thread")]
async fn a_sell_reported_by_the_exchange_stays_a_sell() {
    let (_d, o, s, b) = setup(json!({}));
    let tx = seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-29 10:00:00", "OTX-8"));
    o.primary.execute("UPDATE transactions SET side = 1 WHERE id = ?1", [tx]).unwrap();
    let v = FakeVenue::new().order("OTX-8", json!({ "status": "closed", "price": "50000", "vol": "0.0012", "vol_exec": "0.0012", "cost": "60", "oflags": "",
                                                     "descr": { "type": "sell", "ordertype": "market", "price": "0" } }));
    polling::sweep(&o.primary, &v, &bot(&o, b), now()).await.unwrap();
    let side: i64 = o.primary.query_row("SELECT side FROM transactions WHERE id = ?1", [tx], |r| r.get(0)).unwrap();
    assert_eq!(side, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn imported_rows_are_never_polled_and_a_follow_up_ignores_unknown() {
    let (_d, o, s, b) = setup(json!({}));
    seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-01 10:00:00", "imported_123"));
    polling::sweep(&o.primary, &FakeVenue::new(), &bot(&o, b), now()).await.unwrap();
    let ext: i64 = o.primary.query_row("SELECT external_status FROM transactions", [], |r| r.get(0)).unwrap();
    assert_eq!(ext, 0, "not abandoned either");
    let tx = seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-29 10:00:00", "OTX-6"));
    let v = FakeVenue::new().order("OTX-6", json!({ "status": "pending", "vol": "60", "price": "0", "vol_exec": "0", "cost": "0", "oflags": "viqc", "descr": { "ordertype": "market" } }));
    polling::follow_up(&o.primary, &v, b, tx, now()).await.unwrap(); // `unknown` is skipped; nothing changes
    let ext: i64 = o.primary.query_row("SELECT external_status FROM transactions WHERE external_id = 'OTX-6'", [], |r| r.get(0)).unwrap();
    assert_eq!(ext, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_poll_writes_nothing_to_the_bot() {
    let (_d, o, s, b) = setup(json!({ "missed_quote_amount": "100.0" }));
    seed::insert_tx(&o.primary, &s, b, &TxSpec { external_status: Some(1), order_type: 1, quote_amount: None, amount_exec: Some("0"),
        quote_amount_exec: Some("0"), ..open_market("2026-09-29 10:00:00", "OTX-C") });
    let v = FakeVenue::new().order("OTX-C", json!({ "status": "open", "price": "0", "vol": "0.0012", "vol_exec": "0", "cost": "0", "oflags": "",
                                                     "descr": { "type": "buy", "ordertype": "limit", "price": "50000" } }));
    let before: String = o.primary.query_row("SELECT json_array(updated_at, transient_data) FROM bots WHERE id = ?1", [b], |r| r.get(0)).unwrap();
    polling::sweep(&o.primary, &v, &bot(&o, b), now()).await.unwrap();
    let after: String = o.primary.query_row("SELECT json_array(updated_at, transient_data) FROM bots WHERE id = ?1", [b], |r| r.get(0)).unwrap();
    assert_eq!(after, before, "Rails' polls (#448) no longer rewrite the carry, so the bot row is untouched");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_created_at_fails_the_sweep_and_abandons_nothing() {
    let (_d, o, s, b) = setup(json!({}));
    let tx = seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-29 10:00:00", "OTX-G"));
    o.primary.execute("UPDATE transactions SET created_at = 'garbage' WHERE id = ?1", [tx]).unwrap();
    assert!(matches!(polling::sweep(&o.primary, &FakeVenue::new(), &bot(&o, b), now()).await, Err(PollFailure::General(_))));
    let ext: i64 = o.primary.query_row("SELECT external_status FROM transactions WHERE id = ?1", [tx], |r| r.get(0)).unwrap();
    assert_eq!(ext, 0);
}

#[test]
fn a_blank_row_is_filled_from_the_orders_own_pair_never_the_bots_first_member() {
    use deltabadger::ruby::BigDec;
    use deltabadger::venue::{OrderState, OrderStatus};
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").weights(&[(s.btc, 0.7), (eth, 0.3)]));
    o.primary.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, quote_amount, bot_interval, \
                       bot_quote_amount, transaction_type, error_messages, created_at, updated_at) \
                       VALUES (?1, ?2, 'OLEGACY', 0, 0, 0, 0, 18, 'week', 60, 'REGULAR', '[]', '2026-09-01 10:00:01', '2026-09-01 10:00:01')",
                      rusqlite::params![id, s.exchange_id]).unwrap();
    let tx = o.primary.last_insert_rowid();
    let state = |pair: &str| OrderState { txid: "OLEGACY".into(), status: OrderStatus::Open, price: Some(BigDec::from_i64(2500)), amount: None,
        quote_amount: Some(BigDec::from_i64(18)), amount_exec: BigDec::zero(), quote_amount_exec: BigDec::zero(), limit: false, sell: false, pair: Some(pair.into()) };
    let row = || -> (Option<String>, Option<String>, Option<i64>, Option<i64>) { o.primary.query_row(
        "SELECT base, quote, base_asset_id, quote_asset_id FROM transactions WHERE id = ?1", [tx], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap() };
    polling::apply_in(&o.primary, id, tx, &state("XRP/USD"), true, now()).unwrap();
    assert_eq!(row(), (None, None, None, None), "a pair this venue does not list fills nothing (order_data[:ticker] is nil)");
    polling::apply_in(&o.primary, id, tx, &state("ETH/USD"), true, now()).unwrap();
    assert_eq!(row(), (Some("ETH".into()), Some("USD".into()), Some(eth), Some(s.quote)), "the order's own pair, not BTC");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_kraken_fill_fails_the_sweep_and_records_no_zero_fill() {
    for bad in ["NaN", "Infinity", "garbage"] {
        let (_d, o, s, b) = setup(json!({ "missed_quote_amount": "100.0" }));
        let tx = seed::insert_tx(&o.primary, &s, b, &open_market("2026-09-29 10:00:00", "OTX-NAN"));
        let v = FakeVenue::new().order("OTX-NAN", closed_raw("50010.5", bad, "60.0"));
        assert!(polling::sweep(&o.primary, &v, &bot(&o, b), now()).await.is_err(), "{bad}");
        let (ext, exec): (i64, Option<f64>) = o.primary.query_row("SELECT external_status, amount_exec FROM transactions WHERE id = ?1", [tx], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((ext, exec), (0, None), "{bad}: the row is untouched");
        assert_eq!(bot(&o, b).transient["missed_quote_amount"], json!("100.0"), "{bad}: the carry is untouched");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn stored_trade_lookup_time_out_of_range_is_refused() {
    let (_d, o, s, b) = setup(json!({}));
    let created = deltabadger::codec::format_time(DateTime::<Utc>::MIN_UTC);
    seed::insert_tx(&o.primary, &s, b, &open_market(&created, "OTX-edge"));
    let result = polling::sweep(&o.primary, &FakeVenue::new(), &bot(&o, b), now()).await;
    assert!(matches!(result, Err(PollFailure::General(ref message)) if message.contains("time is outside the representable range")), "{result:?}");
    assert_eq!(o.primary.query_row("SELECT external_status FROM transactions WHERE external_id='OTX-edge'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}
