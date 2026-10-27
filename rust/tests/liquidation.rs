//! Slice 6c-2: requested sales and durable progress on stopped bots.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::scripted::{accepted, ok};
use common::seed::{self, BotSpec};
use deltabadger::engine::{liquidation, model, polling, run, FixedClock};
use deltabadger::engine::tick::{PriceCache, TickContext};
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::{ScriptedTransport, Transport};
use serde_json::{json, Value};
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const T0: &str = "2026-09-01T12:30:00Z";
fn one<T: rusqlite::types::FromSql>(o: &store::Opened, sql: &str) -> T { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }

/// A stopped 50/50 BTC/ETH basket holding 0.01 BTC (640 USD) and 0.04 ETH (100 USD).
fn swap_bot(spec: BotSpec) -> (tempfile::TempDir, store::Opened, i64, seed::Seeded, i64) {
    let (d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &spec.with("rebalance_enabled", json!(false)).weights(&[(s.btc, 0.5), (eth, 0.5)]));
    deltabadger::engine::basket::refresh_composition(&o.primary, &model::load_bot(&o.primary, id).unwrap(), at("2026-09-01T09:00:00Z")).unwrap().unwrap();
    for (asset, qty, price, quote, n) in [(s.btc, "0.01", "64000", "640", 1), (eth, "0.04", "2500", "100", 2)] {
        seed::insert_row(&o.primary, &s, id, asset, &json!({ "external_status": 2, "external_id": format!("OHELD-{n}"), "amount": qty, "amount_exec": qty,
            "quote_amount": quote, "quote_amount_exec": quote, "price": price, "created_at": "2026-09-01 10:00:01" }));
    }
    (d, o, id, s, eth)
}
fn stopped() -> BotSpec { BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") } }

fn position(sym: &str, qty: &str) -> Value { json!({ "symbol": sym, "asset_class": "crypto", "qty": qty, "qty_available": qty, "side": "long" }) }
#[allow(clippy::too_many_arguments)]
fn order(id: &str, status: &str, sym: &str, side: &str, qty: Option<&str>, notional: Option<&str>, filled: &str, avg: Option<&str>) -> Value {
    ok(json!({ "id": id, "status": status, "symbol": sym, "type": "market", "side": side, "notional": notional, "qty": qty,
               "filled_qty": filled, "filled_avg_price": avg, "limit_price": null }))
}
/// Prices at one level for bid, ask and last; the account's positions and settled cash; orders accepted as OTX-1, OTX-2…
fn script(over: Value) -> ScriptedTransport {
    let mut m = json!({
        "GET /v1beta3/crypto/us/latest/quotes": [ok(json!({ "quotes": { "BTC/USD": { "ap": 64000, "bp": 64000 }, "ETH/USD": { "ap": 2500, "bp": 2500 }, "SOL/USD": { "ap": 150, "bp": 150 } } }))],
        "GET /v1beta3/crypto/us/latest/trades": [ok(json!({ "trades": { "BTC/USD": { "p": 64000 }, "ETH/USD": { "p": 2500 }, "SOL/USD": { "p": 150 } } }))],
        "POST /v2/orders": [accepted("OTX-1"), accepted("OTX-2"), accepted("OTX-3")],
        "GET /v2/account": [ok(json!({ "cash": "100000", "non_marginable_buying_power": "100000" }))],
        "GET /v2/positions": [ok(json!([position("BTCUSD", "0.01"), position("ETHUSD", "0.04")]))],
    });
    for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
    ScriptedTransport::from_script(&m)
}
fn venue(t: &ScriptedTransport) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(t.clone(), Urls::for_passphrase(Some("paper"))) }


async fn sell(o: &store::Opened, t: &ScriptedTransport, id: i64, holdings: Value) {
    let prices = PriceCache::default();
    let bot = model::load_bot(&o.primary, id).unwrap();
    let cx = TickContext { credential_version: model::credential_version(&o.primary, &bot).unwrap(), prices: &prices,
        process_start: DateTime::<Utc>::MIN_UTC, stopping: &|| false, below_minimum: &|_, _| {}, claimed: &|_| {} };
    liquidation::run(&o.primary, &venue(t), id, &holdings, &FixedClock(at(T0)), &cx).await.unwrap();
}
fn logs(o: &store::Opened) -> Vec<(String, Value)> {
    o.primary.prepare("SELECT event,details FROM bot_activity_logs ORDER BY id").unwrap().query_map([], |r| {
        let details: String = r.get(1)?;
        Ok((r.get(0)?, serde_json::from_str(&details).unwrap()))
    }).unwrap().map(Result::unwrap).collect()
}
#[tokio::test(flavor="current_thread")]
async fn caps_to_free_dedupes_and_ignores_fee_cutter() {
    let (_d,o,id,s,_) = swap_bot(stopped().with("limit_ordered",json!(true)));
    let t=script(json!({"GET /v2/positions":[ok(json!([position("BTCUSD","0.003")]))]}));
    sell(&o,&t,id,json!([["BTC",s.btc],["BTC",s.btc]])).await;
    let orders=t.posted_orders();
    assert_eq!(orders.len(),1);
    assert_eq!(orders[0]["type"],"market");
    assert_eq!(orders[0]["qty"],"0.003000000");
    assert_eq!(one::<String>(&o,"SELECT transaction_type FROM transactions WHERE external_id='OTX-1'"),"LIQUIDATION");
    assert!(logs(&o).iter().any(|(e,_)|e=="liquidation_placed"));
}
#[tokio::test(flavor="current_thread")]
async fn renamed_asset_and_duplicate_name_cannot_sell_another_position() {
    let (_d,o,id,s,eth)=swap_bot(stopped());
    let t=script(json!({}));
    sell(&o,&t,id,json!([["BTC",eth],["BTC",s.btc],["old",s.btc]])).await;
    assert!(t.posted_orders().is_empty());
    assert_eq!(logs(&o),vec![("liquidation_not_started".into(),json!({"reason":["not_held"]}))]);
}
#[tokio::test(flavor="current_thread")]
async fn partial_fill_then_cancel_banks_only_executed_proceeds() {
    let (_d,o,id,s,_)=swap_bot(stopped());
    let t=script(json!({"GET /v2/orders/OTX-1":[order("OTX-1","canceled","BTC/USD","sell",Some("0.01"),None,"0.002",Some("64000"))]}));
    sell(&o,&t,id,json!([["BTC",s.btc]])).await;
    polling::sweep(&o.primary,&venue(&t),&model::load_bot(&o.primary,id).unwrap(),at(T0)+Duration::minutes(1)).await.unwrap();
    assert_eq!(one::<f64>(&o,"SELECT quote_amount_exec FROM transactions WHERE external_id='OTX-1'"),128.0);
    assert_eq!(one::<i64>(&o,"SELECT external_status FROM transactions WHERE external_id='OTX-1'"),3);
}
#[tokio::test(flavor="current_thread")]
async fn ambiguous_first_holding_stops_the_batch_and_promotes_rails_intents() {
    for orphan in [false,true] {
        let (_d,o,id,s,eth)=swap_bot(stopped());
        if orphan { model::merge_transient_compact(&o.primary,id,&[("liquidation_pending",json!({"id":"rails","symbol":"BTC","state":"placing"}))]).unwrap(); }
        let t=script(json!({"POST /v2/orders":[{"network":"post_send","message":common::scripted::POST_SEND}]}));
        sell(&o,&t,id,json!([["BTC",s.btc],["ETH",eth]])).await;
        assert_eq!(t.posted_orders().len(),usize::from(!orphan));
        assert_eq!(model::load_bot(&o.primary,id).unwrap().transient["liquidation_pending"]["state"],"ambiguous");
    }
}
struct Factory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for Factory {
    type V=AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self,_t:&str,_c:Option<deltabadger::crypto::Credentials>)->Self::V { venue(&self.0) }
}

#[tokio::test(flavor="current_thread")]
async fn unbalanced_stopped_basket_liquidation_is_polled_in_process_and_after_restart() {
    for restart in [false, true] {
        let (d,o,id,s,eth)=swap_bot(stopped());
        o.primary.execute("UPDATE bots SET settings=json_set(settings,'$.allocations',json(?1)) WHERE id=?2",
            rusqlite::params![json!({s.btc.to_string():0.2,eth.to_string():0.3}).to_string(),id]).unwrap();
        model::merge_transient_compact(&o.primary,id,&[(liquidation::REQUEST_KEY,json!({"id":"unbalanced","at":1,"holdings":[["BTC",s.btc]]}))]).unwrap();
        let t=script(json!({"GET /v2/orders/OTX-1":[order("OTX-1","filled","BTC/USD","sell",Some("0.01"),None,"0.01",Some("64000"))]}));
        let paths=store::Paths::from_env(&|_|None,d.path());
        let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
        let mut e=run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
        run::step(&mut e,&FixedClock(at(T0))).await.unwrap();
        assert_eq!(t.posted_orders().len(),1);
        let report=deltabadger::engine::eligibility::check_install(&e.primary).unwrap();
        assert!(report.problems.is_empty(),"{:?}",report.problems);
        assert!(report.unreadable.is_empty(),"{:?}",report.unreadable);
        if restart {
            drop(e);
            let o=store::open(&paths).unwrap();
            let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
            e=run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
        }
        run::step(&mut e,&FixedClock(at(T0)+Duration::minutes(1))).await.unwrap();
        let filled:(i64,f64)=e.primary.query_row("SELECT external_status,quote_amount_exec FROM transactions WHERE external_id='OTX-1'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(filled,(2,640.0),"restart={restart}");
        assert_eq!(model::load_bot(&e.primary,id).unwrap().status,deltabadger::enums::BotStatus::Stopped);
        assert_eq!(t.posted_orders().len(),1);
    }
}

async fn command_sweep_amount_limit(case: &str) {
    use deltabadger::engine::events::EngineEvent;
    use deltabadger::engine::{notice, tick};
    let spec=BotSpec::weekly(60.0,"2026-09-01 10:00:00").with("quote_amount_limited",json!(true)).with("quote_amount_limit",json!(60.0))
        .transient("quote_amount_limit_enabled_at",json!("2026-09-01T11:00:00Z"));
    let (d,o,id,s,_)=swap_bot(spec);
    for (external_id,side) in [("WAIT-BUY",0),("WAIT-SELL",1)] {
        seed::insert_row(&o.primary,&s,id,s.btc,&json!({"external_id":external_id,"external_status":1,"side":side,
            "amount":"0.0009375","quote_amount":"60","price":"64000","amount_exec":"0","quote_amount_exec":"0","created_at":"2026-09-01 11:00:01"}));
    }
    if case=="error" {
        o.primary.execute_batch("CREATE TRIGGER refuse_log_failure BEFORE INSERT ON bot_activity_logs WHEN NEW.event='liquidation_not_started'
            BEGIN SELECT RAISE(ABORT,'liquidation refusal log unavailable'); END;").unwrap();
    }
    model::merge_transient_compact(&o.primary,id,&[(liquidation::REQUEST_KEY,json!({"id":"cap","at":1,"holdings":[["BTC",s.btc]]}))]).unwrap();
    let t=script(json!({
        "GET /v2/orders/WAIT-BUY":[order("WAIT-BUY","filled","BTC/USD","buy",Some("0.0009375"),None,"0.0009375",Some("64000"))],
        "GET /v2/orders/WAIT-SELL":[order("WAIT-SELL",if case=="success" {"canceled"} else {"new"},"BTC/USD","sell",Some("0.0009375"),None,"0",None)]
    }));
    let paths=store::Paths::from_env(&|_|None,d.path());
    let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
    let mut e=run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
    let mut page=e.subscribe_to(EngineEvent::is_live);
    // No engine step or tick: the command itself must finish the queued stop.
    for _ in 0..2 {
        run::run_commands(&mut e,&FixedClock(at(T0))).await.unwrap();
        let bot=model::load_bot(&e.primary,id).unwrap();
        assert_eq!(bot.status,deltabadger::enums::BotStatus::Stopped,"{case}");
        assert!(bot.transient["rust_amount_limit_stops_pending"].is_null());
        assert!(bot.transient[liquidation::REQUEST_KEY].is_null());
        assert_eq!(e.primary.query_row("SELECT stop_message_key FROM bots WHERE id=?1",[id],|r|r.get::<_,String>(0)).unwrap(),tick::AMOUNT_SPENT);
        let (mail,_)=notice::pending(&e.primary,0,10).unwrap();
        assert_eq!(mail.len(),1);
        assert_eq!(mail[0].notice,notice::Notice::StoppedByAmountLimit);
        assert_eq!(e.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE event='stopped'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
    let mut statuses=vec![];
    while let Ok(event)=page.try_recv() {
        if matches!(event,EngineEvent::StatusChanged {..}) { statuses.push(event); }
    }
    assert_eq!(statuses,vec![EngineEvent::StatusChanged {bot_id:id}],"{case}: the committed stop reaches the page exactly once");
    assert_eq!(t.posted_orders().len(),usize::from(case=="success"));
    if case!="success" {
        let event=if case=="error" {"liquidation_failed"} else {"liquidation_not_started"};
        assert_eq!(e.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE event=?1",[event],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
}

#[tokio::test(flavor="current_thread")]
async fn successful_liquidation_command_drains_swept_amount_limit_stops() { command_sweep_amount_limit("success").await; }

#[tokio::test(flavor="current_thread")]
async fn refused_liquidation_command_drains_swept_amount_limit_stops() { command_sweep_amount_limit("refusal").await; }

#[tokio::test(flavor="current_thread")]
async fn failed_liquidation_command_drains_swept_amount_limit_stops() { command_sweep_amount_limit("error").await; }

#[tokio::test(flavor="current_thread")]
async fn requests_are_compare_cleared_and_taken_requests_are_never_replayed() {
    for case in ["queued","taken","archived","pending_key","rails_marker","invalid"] {
        let (d,o,id,s,_)=swap_bot(stopped());
        let mut request=json!({"id":"request-1","at":at(T0).timestamp(),"holdings":[["BTC",s.btc]]});
        if case=="taken" { request["taken_at"]=json!(1); }
        if case=="rails_marker" { request.as_object_mut().unwrap().remove("holdings"); }
        if case=="invalid" { request["holdings"]=Value::Null; }
        model::merge_transient_compact(&o.primary,id,&[("liquidation_selling_since",request)]).unwrap();
        if case=="archived" { o.primary.execute("UPDATE bots SET status=?1",[deltabadger::enums::BotStatus::Archived as i64]).unwrap(); }
        if case=="pending_key" { o.primary.execute("UPDATE api_keys SET status=3",[]).unwrap(); }
        let paths=store::Paths::from_env(&|_|None,d.path());
        let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
        let t=script(json!({}));
        let mut e=run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
        run::run_commands(&mut e,&FixedClock(at(T0))).await.unwrap();
        run::run_commands(&mut e,&FixedClock(at(T0))).await.unwrap();
        assert_eq!(t.posted_orders().len(),usize::from(case=="queued"),"{case}");
        let b=model::load_bot(&e.primary,id).unwrap();
        assert_eq!(b.transient["liquidation_selling_since"].is_null(),case!="rails_marker","{case}");
        if case=="archived" || case=="pending_key" {
            let reason:String=e.primary.query_row("SELECT json_extract(details,'$.reason') FROM bot_activity_logs WHERE event='liquidation_not_started'",[],|r|r.get(0)).unwrap();
            assert_eq!(reason,if case=="archived" {"archived"} else {"api_key_pending"});
        }
        if case=="invalid" {
            assert_eq!(e.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE event='liquidation_failed'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        }
    }
}

#[tokio::test(flavor="current_thread")]
async fn completion_preserves_a_request_that_replaced_the_taken_one() {
    let (d,o,id,s,_)=swap_bot(stopped());
    model::merge_transient_compact(&o.primary,id,&[("liquidation_selling_since",json!({"id":"first","at":1,"holdings":[["BTC",s.btc]]}))]).unwrap();
    o.primary.execute_batch("CREATE TRIGGER replace_request AFTER INSERT ON transactions WHEN NEW.transaction_type='LIQUIDATION' BEGIN
        UPDATE bots SET transient_data=json_set(transient_data,'$.liquidation_selling_since',json('{\"id\":\"replacement\",\"at\":2,\"holdings\":[]}')) WHERE id=NEW.bot_id;
        END;").unwrap();
    let paths=store::Paths::from_env(&|_|None,d.path());
    let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
    let t=script(json!({}));
    let mut e=run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
    run::run_commands(&mut e,&FixedClock(at(T0))).await.unwrap();
    assert_eq!(t.posted_orders().len(),1);
    assert_eq!(model::load_bot(&e.primary,id).unwrap().transient["liquidation_selling_since"],json!({"id":"replacement","at":2,"holdings":[]}));
}

#[tokio::test(flavor="current_thread")]
async fn orders_waiting_and_account_wide_buys_block_sales() {
    for sell_order in [false,true] {
        let (_d,o,id,s,_)=swap_bot(stopped());
        let owner=if sell_order {id} else {seed::insert_bot(&o.primary,&s,&stopped())};
        seed::insert_row(&o.primary,&s,owner,s.btc,&json!({"external_id":"REST","external_status":1,"side":i64::from(sell_order),"amount":"0.001","amount_exec":"0","quote_amount_exec":"0","created_at":"2026-09-01 10:00:02"}));
        let t=script(json!({"GET /v2/orders/REST":[order("REST","new","BTC/USD",if sell_order {"sell"} else {"buy"},Some("0.001"),None,"0",None)]}));
        sell(&o,&t,id,json!([["BTC",s.btc]])).await;
        assert!(t.posted_orders().is_empty());
        let expected=if sell_order {("liquidation_not_started".into(),json!({"reason":["orders_waiting"]}))}
                     else {("liquidation_skipped".into(),json!({"base":"BTC","reason":"open_order"}))};
        assert!(logs(&o).contains(&expected),"{:?}",logs(&o));
    }
}
#[tokio::test(flavor="current_thread")]
async fn closed_stock_market_never_sizes_and_split_mismatch_holds() {
    for closed in [true,false] {
        let (_d,o,s)=common::install_alpaca();
        let (asset,_)=seed::add_alpaca_stock(&o.primary,&s,"KLAC");
        let id=seed::insert_bot(&o.primary,&s,&stopped().weights(&[(asset,1.0)]));
        seed::fresh_stock_jobs(&o.primary,at(T0));
        seed::insert_stock_tx(&o.primary,&s,id,asset,"KLAC",&seed::TxSpec {status:0,external_status:Some(2),external_id:Some("held".into()),order_type:0,amount:Some("10"),quote_amount:Some("1000"),price:Some("100"),amount_exec:Some("10"),quote_amount_exec:Some("1000"),created_at:"2026-08-01 10:00:01".into()});
        if !closed {
            let row=seed::insert_split(&o.primary,&s,"KLAC","2026-08-05 00:00:00",Some("3:2"));
            o.primary.execute("UPDATE account_transactions SET base_amount=5,raw_data=?1 WHERE id=?2",rusqlite::params![json!({"corporate_action":"split","qty":"-10","split_ratio":"3:2","merged_activity_ids":["a","b"]}).to_string(),row]).unwrap();
        }
        let t=common::scripted::script(json!({
            "GET /v2/clock":[ok(json!({"is_open":!closed,"next_open":"2026-09-02T13:30:00Z","next_close":"2026-09-01T20:00:00Z","timestamp":T0}))],
            "GET /v2/positions":[ok(json!([{"symbol":"KLAC","asset_class":"us_equity","qty":"30"}]))]
        }));
        sell(&o,&t,id,json!([["KLAC",asset]])).await;
        assert!(t.posted_orders().is_empty());
        assert!(logs(&o).contains(&if closed {("liquidation_market_closed".into(),json!({}))}
            else {("liquidation_not_started".into(),json!({"reason":["split_hold"]}))}),"{:?}",logs(&o));
    }
}

#[tokio::test(flavor="current_thread")]
async fn stopped_order_is_repolled_after_eight_attempts_and_after_restart() {
    for restart in [false,true] {
        let (d,o,id,s,_)=swap_bot(stopped());
        let row=seed::insert_row(&o.primary,&s,id,s.btc,&json!({"external_id":"WAIT","external_status":1,"side":1,"amount":"0.001","created_at":"2026-09-01 10:00:02"}));
        o.primary.execute("UPDATE transactions SET transaction_type='LIQUIDATION' WHERE id=?1",[row]).unwrap();
        let mut replies=vec![order("WAIT","new","BTC/USD","sell",Some("0.001"),None,"0",None);8];
        replies.push(order("WAIT","filled","BTC/USD","sell",Some("0.001"),None,"0.001",Some("64000")));
        let t=script(json!({"GET /v2/orders/WAIT":replies}));
        let paths=store::Paths::from_env(&|_|None,d.path());
        let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
        let mut e=run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
        for n in 0..8 { run::step(&mut e,&FixedClock(at(T0)+Duration::hours(n))).await.unwrap(); }
        if restart {
            drop(e);
            let o=store::open(&paths).unwrap();
            let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
            e=run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
        }
        run::step(&mut e,&FixedClock(at(T0)+Duration::hours(8))).await.unwrap();
        let status:i64=e.primary.query_row("SELECT external_status FROM transactions WHERE id=?1",[row],|r|r.get(0)).unwrap();
        assert_eq!(status,2,"restart={restart}");
    }
}
#[tokio::test(flavor="current_thread")]
async fn zero_open_then_terminal_missing_then_late_positive_proceeds_for_both_terminal_states() {
    for terminal in ["filled","canceled"] {
        let (d,o,id,s,_)=swap_bot(stopped());
        let row=seed::insert_row(&o.primary,&s,id,s.btc,&json!({"external_id":"LATE","external_status":1,"side":1,"amount":"0.002","created_at":"2026-09-01 10:00:02"}));
        o.primary.execute("UPDATE transactions SET transaction_type='LIQUIDATION',quote_amount_exec=NULL WHERE id=?1",[row]).unwrap();
        // Exercise the normalized venue boundary: Alpaca itself rejects a missing average on an executed fill.
        // Other venue adapters may report a terminal state with proceeds still absent.
        for (seconds,status,exec,quote) in [(0,deltabadger::venue::OrderStatus::Open,"0",Some("0")),
            (5,if terminal=="filled" {deltabadger::venue::OrderStatus::Closed} else {deltabadger::venue::OrderStatus::Cancelled},"0.001",None)] {
            let mut state=deltabadger::venue::fake::kraken_order("LATE",&json!({"status":"open","vol":"0.002","vol_exec":"0","cost":"0","price":"0","oflags":"","descr":{"pair":"BTCUSD","type":"sell","ordertype":"market"}}));
            state.status=status;
            state.amount_exec=deltabadger::ruby::BigDec::parse(exec).unwrap();
            state.quote_amount_exec=quote.map(|q|deltabadger::ruby::BigDec::parse(q).unwrap());
            let version=model::credential_version(&o.primary,&model::load_bot(&o.primary,id).unwrap()).unwrap();
            model::credential_write(&o.primary,&version,|tx|polling::apply_in(tx,id,row,&state,false,at(T0)+Duration::seconds(seconds))).unwrap();
        }
        assert_eq!(one::<f64>(&o,"SELECT quote_amount_exec FROM transactions WHERE external_id='LATE'"),0.0);
        let t=script(json!({"GET /v2/orders/LATE":[order("LATE",terminal,"BTC/USD","sell",Some("0.002"),None,"0.001",Some("64000"))]}));
        let paths=store::Paths::from_env(&|_|None,d.path());
        let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
        let mut e=run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
        run::step(&mut e,&FixedClock(at(T0)+Duration::minutes(10))).await.unwrap();
        assert_eq!(e.primary.query_row("SELECT quote_amount_exec FROM transactions WHERE id=?1",[row],|r|r.get::<_,f64>(0)).unwrap(),64.0);
    }
}

#[cfg(unix)]
fn liquidation_grid_difference(name: &str, _: &std::path::Path, rails: &Value, rust: &Value) -> Result<(),String> {
    let mut expected=rails.clone();
    if name.ends_with("-split_unresolved") {
        // D-6C-SPLIT-HOLD (T3): refuse before sizing, with the explicit reconciliation reason.
        assert_eq!(expected["changes"]["bot_activity_logs"][0]["after"]["details"]["reason"],json!(["not_held"]));
        expected["changes"]["bot_activity_logs"][0]["after"]["details"]["reason"]=json!(["split_hold"]);
    } else {
        let alpaca=name.contains("-alpaca-");
        let hyperliquid=name.contains("-hyperliquid-");
        let kraken=name.contains("-kraken-");
        let timeout=name.ends_with("-timeout");
        if timeout || !alpaca {
            let bots=expected["changes"]["bots"].as_array_mut().unwrap();
            assert_eq!(bots.len(),1);
            let pending=&bots[0]["after"]["transient_data"]["liquidation_pending"];
            assert_eq!(pending["state"],"ambiguous");
            let nonce=pending["id"].as_str().unwrap();
            assert_eq!(nonce.len(),12);
            assert!(nonce.bytes().all(|b|b.is_ascii_hexdigit()));
            assert!(bots[0]["before"]["transient_data"].get("liquidation_pending").is_none());
            if timeout || !kraken {
                // Both sides generate an independent intent nonce; compare every other field.
                let nonce=&rust["changes"]["bots"][0]["after"]["transient_data"]["liquidation_pending"]["id"];
                assert!(uuid::Uuid::parse_str(nonce.as_str().unwrap()).is_ok());
                bots[0]["after"]["transient_data"]["liquidation_pending"]["id"]=nonce.clone();
            } else {
                // D-6C-PRE-SEND: Kraken's transport proves a refused connection sent nothing.
                // Honeymaker wraps it as a generic failure, which Rails leaves ambiguous.
                bots[0]["after"]["transient_data"].as_object_mut().unwrap().remove("liquidation_pending");
                bots.retain(|b|b["before"]!=b["after"]);
            }
        }
        let logs=expected["changes"]["bot_activity_logs"].as_array_mut().unwrap();
        assert_eq!(logs.len(),1);
        let log=&mut logs[0]["after"];
        if timeout {
            let reason=if alpaca {"placement outcome unknown: Faraday::TimeoutError: Net::ReadTimeout"}
                else if hyperliquid {"placement failed: Hyperliquid::Error: Network error: Net::ReadTimeout"}
                else {"placement failed: Net::ReadTimeout"};
            assert_eq!(log["details"]["reason"],reason);
            // The transport boundary keeps Client.network_failure's spelling on all venues.
            log["details"]["reason"]=json!("placement outcome unknown: Faraday::TimeoutError: Net::ReadTimeout");
        } else if !alpaca {
            assert_eq!(log["event"],"liquidation_ambiguous");
            assert_eq!(log["level"],2);
            assert_eq!(log["details"]["base"],"BTC");
            assert_eq!(log["details"]["amount"],"0.01");
            let price=deltabadger::ruby::BigDec::parse(log["details"]["price"].as_str().unwrap()).unwrap();
            let quote=deltabadger::ruby::BigDec::parse(log["details"]["quote_amount"].as_str().unwrap()).unwrap();
            assert_eq!(&price*&deltabadger::ruby::BigDec::parse("0.01").unwrap(),quote);
            let prefix=if hyperliquid {"placement failed: Hyperliquid::Error: Network error: "} else {"placement failed: "};
            let message=log["details"]["reason"].as_str().unwrap().strip_prefix(prefix).unwrap();
            assert!(message.starts_with("Connection refused - connect(2) for \"") && message.ends_with("\" port 443"));
            if kraken {
                log["details"]=json!({"base":"XBT","reason":format!("transient: Faraday::ConnectionFailed: {message}")});
                log["event"]=json!("liquidation_skipped");
                log["level"]=json!(0);
            } else {
                log["details"]["reason"]=json!(format!("placement failed: Faraday::ConnectionFailed: {message}"));
            }
        }
    }
    if expected==*rust {Ok(())} else {Err(format!("rails: {expected}\nrust: {rust}"))}
}

#[cfg(unix)]
#[tokio::test(flavor="current_thread")]
async fn rails_liquidation_grid_matches_every_venue() {
    let outputs=common::venue_grid::run_venue_grid("grid-liquidation",450,&[common::venue_grid::Validator {
        names:&|name|["-timeout","-pre_send","-split_unresolved"].iter().any(|suffix|name.ends_with(suffix)),
        check:&liquidation_grid_difference,
    }]).await;
    for (name,out) in outputs {
        let count=out["sent"].as_array().unwrap().len();
        if ["-accepted","-duplicate","-fee_cutter","-partial_fill","-former","-free_cap","-timeout","-pre_send"].iter().any(|suffix|name.ends_with(suffix)) {
            assert_eq!(count,1,"{name}: {out}");
            if name.contains("-gemini-") { assert!(out["sent"][0]["price"].as_str().unwrap().parse::<f64>().unwrap()>0.0,"{name}"); }
            if name.contains("-hyperliquid-") { assert!(out["sent"][0]["p"].as_str().unwrap().parse::<f64>().unwrap()>0.0,"{name}"); }
        } else { assert_eq!(count,0,"{name}: {out}"); }
    }
}

#[tokio::test(flavor="current_thread")]
async fn discarded_taken_request_still_recovers_and_halts_its_unknown_intent() {
    let (d,o,id,s,_)=swap_bot(stopped());
    let t=script(json!({"POST /v2/orders":[{"network":"post_send","message":common::scripted::POST_SEND}]}));
    sell(&o,&t,id,json!([["BTC",s.btc]])).await;
    // Crash while the durable request was taken; the intent is still in its original placing phase.
    o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.liquidation_pending.state','placing','$.liquidation_selling_since',json(?1))",
        [json!({"id":"crashed","at":1,"taken_at":2,"holdings":[["BTC",s.btc]]}).to_string()]).unwrap();
    drop(o);
    let paths=store::Paths::from_env(&|_|None,d.path());
    let o=store::open(&paths).unwrap();
    let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
    let t=script(json!({"GET /v2/orders:by_client_order_id":[{"status":503,"body":{}}]}));
    let mut e=run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
    run::step(&mut e,&FixedClock(at(T0)+Duration::hours(1))).await.unwrap();
    let bot=model::load_bot(&e.primary,id).unwrap();
    assert!(bot.transient["liquidation_selling_since"].is_null());
    assert!(bot.transient["rust_placement"].is_object());
    assert_eq!(bot.transient["liquidation_pending"]["state"],"ambiguous");
    assert!(t.posted_orders().is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn handback_refuses_queued_and_taken_liquidation_requests() {
    for taken in [false,true] {
        let (d,o,id,s,_)=swap_bot(stopped());
        let mut request=json!({"id":"request","at":1,"holdings":[["BTC",s.btc]]});
        if taken { request["taken_at"]=json!(2); }
        model::merge_transient_compact(&o.primary,id,&[("liquidation_selling_since",request)]).unwrap();
        let paths=store::Paths::from_env(&|_|None,d.path());
        let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
        deltabadger::lease::claim(&lock,&o.primary,&seed::cipher(),"test",at(T0)).unwrap();
        let result=deltabadger::engine::handover::hand_back(&lock,&o,&Factory(script(json!({}))),&seed::cipher(),&FixedClock(at(T0))).await;
        assert!(matches!(result,Err(deltabadger::engine::EngineError::Ineligible(ref r)) if r.iter().any(|r|r.contains("liquidation request"))));
        assert_eq!(deltabadger::lease::read(&o.primary,&seed::cipher()).unwrap().unwrap()["engine"],"rust");
    }
}

#[tokio::test(flavor="current_thread")]
async fn stopped_missing_order_becomes_one_abandoned_halt_and_stays_blocked() {
    let (d,o,s)=common::install();
    let id=seed::insert_bot(&o.primary,&s,&stopped());
    let row=seed::insert_row(&o.primary,&s,id,s.btc,&json!({"external_id":"MISSING","external_status":1,"side":1,"amount":"0.001","created_at":"2026-08-01 10:00:02"}));
    o.primary.execute("UPDATE transactions SET transaction_type='LIQUIDATION' WHERE id=?1",[row]).unwrap();
    let paths=store::Paths::from_env(&|_|None,d.path());
    let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
    let mut e=run::Engine::new(o.primary,deltabadger::venue::fake::FakeFactory(deltabadger::venue::fake::FakeVenue::new()),seed::cipher(),lock);
    for minutes in [0,10,20] { run::step(&mut e,&FixedClock(at(T0)+Duration::minutes(minutes))).await.unwrap(); }
    assert_eq!(e.primary.query_row("SELECT external_status FROM transactions WHERE id=?1",[row],|r|r.get::<_,i64>(0)).unwrap(),4);
    assert_eq!(e.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE event='liquidation_ambiguous'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    let bot=model::load_bot(&e.primary,id).unwrap();
    assert!(deltabadger::engine::composition::in_flight(&e.primary,&bot,at(T0)).unwrap());
    assert!(bot.transient["liquidation_pending"].is_null(),"the unresolved row is the halt");
}

#[test]
fn liquidation_sources_match_the_live_rails_grid_oracle() {
    use sha2::{Digest,Sha256};
    let pins:Value=serde_json::from_str(include_str!("fixtures/liquidation_sources.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    assert_eq!(pins.as_object().unwrap().len(),4);
    for (file,hash) in pins.as_object().unwrap() {
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap())),hash.as_str().unwrap(),"{file}: re-record grid-liquidation and check the engine");
    }
}

#[tokio::test(flavor="current_thread")]
async fn expired_crypto_catalogue_refuses_liquidation_before_sizing() {
    let (_d,o,id,s,_)=swap_bot(stopped());
    deltabadger::jobs::state::seed(&o.primary,deltabadger::jobs::reference::ALPACA_CRYPTO,
        Some(at(T0)-Duration::hours(50)),at(T0)).unwrap();
    let reason=deltabadger::engine::staleness::stale(&o.primary,&model::load_bot(&o.primary,id).unwrap(),at(T0)).unwrap().unwrap().message;
    let t=script(json!({}));
    sell(&o,&t,id,json!([["BTC",s.btc]])).await;
    assert!(t.posted_orders().is_empty());
    assert_eq!(logs(&o),vec![("liquidation_not_started".into(),json!({"reason":[reason]}))]);
    assert!(t.requests().is_empty(),"freshness is checked before fetching prices or balances");
}

#[tokio::test(flavor="current_thread")]
async fn missing_terminal_partial_stays_cancelled_and_can_receive_late_proceeds() {
    use deltabadger::venue::fake::{FakeFactory,FakeVenue};
    let (d,o,s)=common::install();
    let id=seed::insert_bot(&o.primary,&s,&stopped());
    let row=seed::insert_row(&o.primary,&s,id,s.btc,&json!({"external_id":"LATE","external_status":3,"side":1,
        "amount":"0.002","amount_exec":"0.001","created_at":"2026-08-01 10:00:02"}));
    // The order is old, but its partial cancellation was recorded recently and awaits proceeds.
    o.primary.execute("UPDATE transactions SET transaction_type='LIQUIDATION',quote_amount_exec=NULL,updated_at=?2 WHERE id=?1",
        rusqlite::params![row,deltabadger::codec::format_time(at(T0))]).unwrap();
    let paths=store::Paths::from_env(&|_|None,d.path());
    let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
    let v=FakeVenue::new();
    let mut e=run::Engine::new(o.primary,FakeFactory(v.clone()),seed::cipher(),lock);
    run::step(&mut e,&FixedClock(at(T0))).await.unwrap();
    let stored=||e.primary.query_row("SELECT external_status,amount_exec,quote_amount_exec FROM transactions WHERE id=?1",
        [row],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,f64>(1)?,r.get::<_,Option<f64>>(2)?))).unwrap();
    assert_eq!(stored(),(3,0.001,None));
    assert!(v.calls("/0/private/QueryOrders")>0 && v.calls("/0/private/TradesHistory")>0,"both endpoints were asked");
    assert_eq!(e.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE event='order_abandoned'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    // The next scheduled scan must still see the row and accept the venue's positive fill.
    let _v=v.order("LATE",json!({"status":"canceled","vol":"0.002","vol_exec":"0.001","cost":"64","price":"64000","oflags":"",
        "descr":{"pair":"XBTUSD","type":"sell","ordertype":"market"}}));
    run::step(&mut e,&FixedClock(at(T0)+Duration::minutes(10))).await.unwrap();
    assert_eq!(e.primary.query_row("SELECT external_status,quote_amount_exec FROM transactions WHERE id=?1",
        [row],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,f64>(1)?))).unwrap(),(3,64.0));
}

/// Commit a competing write during the last price read, after composition and freshness checks.
#[derive(Clone)]
struct During {
    script: ScriptedTransport,
    change: std::rc::Rc<dyn Fn()>,
}
impl Transport for During {
    async fn send(&self,req:&deltabadger::venue::http::HttpRequest)->Result<deltabadger::venue::http::HttpResponse,deltabadger::venue::http::TransportError> {
        if req.path=="/v2/stocks/KLAC/quotes/latest" { (self.change)(); }
        self.script.send(req).await
    }
}
struct DuringFactory(During);
impl deltabadger::venue::VenueFactory for DuringFactory {
    type V=AlpacaVenue<During>;
    fn for_bot(&self,_t:&str,_c:Option<deltabadger::crypto::Credentials>)->Self::V {
        AlpacaVenue::new(self.0.clone(),Urls::for_passphrase(Some("paper")))
    }
}
async fn placement_refusal(case: &'static str) {
    let (d,o,s)=common::install_alpaca();
    let (asset,_)=seed::add_alpaca_stock(&o.primary,&s,"KLAC");
    o.primary.execute("INSERT INTO exchange_assets (exchange_id,asset_id,available,created_at,updated_at) SELECT exchange_id,base_asset_id,1,created_at,updated_at FROM tickers WHERE ticker='KLAC'",[]).unwrap();
    let id=seed::insert_bot(&o.primary,&s,&stopped().weights(&[(asset,1.0)]));
    seed::insert_stock_tx(&o.primary,&s,id,asset,"KLAC",&seed::TxSpec {status:0,external_status:Some(2),
        external_id:Some("held".into()),order_type:0,amount:Some("10"),quote_amount:Some("1000"),price:Some("100"),
        amount_exec:Some("10"),quote_amount_exec:Some("1000"),created_at:"2026-08-01 10:00:01".into()});
    seed::fresh_stock_jobs(&o.primary,at(T0));
    let key:i64=one(&o,"SELECT id FROM api_keys WHERE key_type=0");
    let stale=|c:&rusqlite::Connection|deltabadger::jobs::state::mark_incomplete(c,"ledger_sync",Some(&key.to_string()),at(T0)).unwrap();
    if case=="stale_at_start" { stale(&o.primary); }
    model::merge_transient_compact(&o.primary,id,&[(liquidation::REQUEST_KEY,json!({"id":"refusal","at":1,"holdings":[["KLAC",asset]]}))]).unwrap();
    let t=script(json!({
        "GET /v2/clock":[ok(json!({"is_open":true,"next_open":"2026-09-02T13:30:00Z","next_close":"2026-09-01T20:00:00Z","timestamp":T0}))],
        "GET /v2/positions":[ok(json!([{"symbol":"KLAC","asset_class":"us_equity","qty":"10","qty_available":"10"}]))],
        "GET /v2/stocks/snapshots":[ok(json!({"KLAC":{"latestTrade":{"p":100}}}))],
        "GET /v2/stocks/KLAC/quotes/latest":[ok(json!({"quote":{"ap":100,"bp":100}}))]
    }));
    let writer=rusqlite::Connection::open(o.primary.path().unwrap()).unwrap();
    let changed=std::rc::Rc::new(std::cell::Cell::new(false));
    let observed=changed.clone();
    let transport=During {script:t.clone(),change:std::rc::Rc::new(move|| {
        observed.set(true);
        if case=="ledger_changed" {
            deltabadger::jobs::state::mark_incomplete(&writer,"ledger_sync",Some(&key.to_string()),at(T0)).unwrap();
        } else if case=="settings_changed" {
            writer.execute("UPDATE bots SET settings=json_set(settings,'$.allocations',json(?1)) WHERE id=?2",
                rusqlite::params![json!({asset.to_string():0.5}).to_string(),id]).unwrap();
        }
    })};
    let paths=store::Paths::from_env(&|_|None,d.path());
    let lock=deltabadger::lease::lock(&paths,at(T0)).unwrap();
    let mut e=run::Engine::new(o.primary,DuringFactory(transport),seed::cipher(),lock);
    run::run_commands(&mut e,&FixedClock(at(T0))).await.unwrap();
    assert!(model::load_bot(&e.primary,id).unwrap().transient[liquidation::REQUEST_KEY].is_null(),"{case}");
    assert_eq!(t.posted_orders().len(),usize::from(case=="unchanged"),"{case}");
    if case=="stale_at_start" {
        assert!(!changed.get(),"stale data must be refused before sizing");
    } else { assert!(changed.get(),"the placement guard was reached: {case}"); }
    if case=="unchanged" { return; }
    let event=if case=="settings_changed" {"liquidation_batch_cut_short"} else {"liquidation_not_started"};
    let details:String=e.primary.query_row("SELECT details FROM bot_activity_logs WHERE event=?1",[event],|r|r.get(0)).unwrap();
    let details:Value=serde_json::from_str(&details).unwrap();
    if case=="settings_changed" { assert_eq!(details,json!({"bases":"KLAC"})); }
    else {
        let reason=deltabadger::engine::staleness::ledger_stale(&e.primary,&model::load_bot(&e.primary,id).unwrap(),at(T0)).unwrap().unwrap().message;
        assert_eq!(details,json!({"reason":[reason]}));
    }
}

#[tokio::test(flavor="current_thread")]
async fn placement_refusals_are_logged_unchanged() {
    placement_refusal("unchanged").await;
}

#[tokio::test(flavor="current_thread")]
async fn placement_refusals_are_logged_stale_at_start() {
    placement_refusal("stale_at_start").await;
}

#[tokio::test(flavor="current_thread")]
async fn placement_refusals_are_logged_ledger_changed() {
    placement_refusal("ledger_changed").await;
}

#[tokio::test(flavor="current_thread")]
async fn placement_refusals_are_logged_settings_changed() {
    placement_refusal("settings_changed").await;
}
