mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, TxSpec};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{eligibility, index, model, FixedClock};
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};

const SYMBOLS: [&str; 5] = ["AAA", "BBB", "CCC", "DDD", "EEE"];
const T0: &str = "2026-09-01T14:00:00.5Z";
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
/// Five Alpaca stocks ranked AAA..EEE in data-api's ND100 row by caps 5, 3, 2, 1 and 0.5 trillion, and an index bot over it
/// (top 3, pure market cap) with a fresh ledger sync: (dir, store, seed, bot, [(asset, ticker)] in SYMBOLS order).
fn universe(num_coins: i64, flattening: f64, hold_all: bool) -> (tempfile::TempDir, store::Opened, seed::Seeded, i64, Vec<(i64, i64)>) {
    let (d, o, s) = common::install_alpaca();
    let members: Vec<(i64, i64)> = SYMBOLS.iter().map(|sym| seed::add_alpaca_stock(&o.primary, &s, sym)).collect();
    seed::insert_index(&o.primary, "nasdaq-100", &["AAA.US", "BBB.US", "CCC.US", "DDD.US", "EEE.US"],
                       &json!({ "AAA.US": 5e12, "BBB.US": 3e12, "CCC.US": 2e12, "DDD.US": 1e12, "EEE.US": 5e11 }));
    let id = seed::index_bot(&o.primary, &s, "nasdaq-100", num_coins, flattening, hold_all);
    seed::fresh_stock_jobs(&o.primary, at(T0));
    (d, o, s, id, members)
}
/// Every symbol quoted at an ask of `asks[i]` (0 = unpriced), an open clock, five accepted orders, a funded account.
fn script(asks: [f64; 5]) -> ScriptedTransport {
    let mut m = json!({
        "GET /v2/clock": ok(json!({ "timestamp": "2026-09-01T10:00:00.5-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" })),
        "POST /v2/orders": (1..=5).map(|n| json!({ "status": 200, "body": { "id": format!("OTX-{n}"), "status": "accepted" } })).collect::<Vec<_>>(),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([])),
    });
    for (sym, ask) in SYMBOLS.iter().zip(asks) {
        m[format!("GET /v2/stocks/{sym}/quotes/latest")] = ok(json!({ "quote": { "ap": ask, "bp": ask } }));
    }
    ScriptedTransport::from_script(&m)
}
fn venue(t: &ScriptedTransport) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(t.clone(), Urls::for_passphrase(Some("paper"))) }
fn posted_symbols(t: &ScriptedTransport) -> Vec<String> { t.posted_orders().iter().map(|b| b["symbol"].as_str().unwrap().to_string()).collect() }
fn in_index(o: &store::Opened, bot: i64) -> Vec<(String, f64)> {
    let mut s = o.primary.prepare("SELECT a.symbol, m.target_allocation FROM bot_index_assets m JOIN assets a ON a.id = m.asset_id \
                                   WHERE m.bot_id = ?1 AND m.in_index = 1 ORDER BY m.target_allocation DESC, m.id").unwrap();
    s.query_map([bot], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
async fn tick_until_settled(o: &store::Opened, t: &ScriptedTransport, id: i64, start: DateTime<Utc>) -> TickOutcome {
    let (mut now, mut attempts) = (start, Attempts::default());
    loop {
        match tick::tick(&o.primary, &venue(t), id, &FixedClock(now), &mut attempts).await.unwrap() {
            TickOutcome::RetryAfter(d) => now += Duration::from_std(d).unwrap(),
            other => return other,
        }
    }
}
fn quote_reads(t: &ScriptedTransport, sym: &str) -> usize { t.requests().iter().filter(|r| r.path == format!("/v2/stocks/{sym}/quotes/latest")).count() }

#[test]
fn every_recorded_rails_blend_is_reproduced() {
    let cases = common::vectors()["index_blends"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 12);
    for c in cases {
        let caps: Vec<f64> = c["caps"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let want: Vec<f64> = c["weights"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        for (got, want) in index::blend(&caps, c["flattening"].as_f64().unwrap()).iter().zip(want) { assert!((got-want).abs() <= 2.0*f64::EPSILON, "{c}: {got} != {want}"); }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_fresh_index_bot_probes_and_buys_the_top_three_by_cap_weight() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(in_index(&o, id), vec![("AAA".into(), 0.5), ("BBB".into(), 0.3), ("CCC".into(), 0.2)], "caps 5/3/2 of 10");
    assert_eq!(posted_symbols(&t), vec!["AAA", "BBB", "CCC"], "in target_allocation DESC order");
    let notionals: Vec<Value> = t.posted_orders().iter().map(|b| b["notional"].clone()).collect();
    assert_eq!(notionals, vec![json!("30.00"), json!("18.00"), json!("12.00")], "2c's split over empty holdings (its vectors pin the arithmetic)");
    for sym in ["AAA", "BBB", "CCC"] { assert_eq!(quote_reads(&t, sym), 1, "{sym}: the probe's price is the one Step 1 reuses (5 s cache)"); }
    assert_eq!(quote_reads(&t, "DDD"), 0, "the walk stops at N");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unpriced_newcomer_gives_its_seat_to_the_next_name() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    let t = script([100.0, 200.0, 0.0, 400.0, 500.0]);
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert_eq!(in_index(&o, id).into_iter().map(|(s, _)| s).collect::<Vec<_>>(), vec!["AAA", "BBB", "DDD"]);
    assert_eq!(posted_symbols(&t), vec!["AAA", "BBB", "DDD"]);
}

#[tokio::test(flavor = "current_thread")]
async fn an_unpriced_incumbent_stalls_the_buy_and_nothing_is_placed() {
    let (_d, o, _s, id, m) = universe(3, 0.0, false);
    for (i, w) in [0.5, 0.3, 0.2].into_iter().enumerate() { seed::insert_member(&o.primary, id, m[i].0, m[i].1, w, true, "2026-08-01 00:00:00", None); }
    let t = script([100.0, 200.0, 0.0, 400.0, 500.0]);
    let out = tick_until_settled(&o, &t, id, at(T0)).await;
    assert!(matches!(out, TickOutcome::Rescheduled), "retry_on ×4, then the next checkpoint: {out:?}");
    assert!(t.posted_orders().is_empty(), "Step 1 prices every member before any order");
    let error: String = o.primary.query_row("SELECT json_extract(details, '$.error') FROM bot_activity_logs WHERE event = 'execution_retrying'", [], |r| r.get(0)).unwrap();
    assert_eq!(error, "No price for CCC: Wrong ask price for CCC: 0.0", "an incumbent is never probed: it keeps its seat and stalls the tick");
    assert_eq!(in_index(&o, id).len(), 3, "CCC is still a member");
}

#[tokio::test(flavor = "current_thread")]
async fn a_leaver_is_marked_out_and_never_sold() {
    let (_d, o, s, id, m) = universe(3, 0.0, false);
    for (i, w) in [0.5, 0.3, 0.2].into_iter().enumerate() {
        seed::insert_member(&o.primary, id, m[i].0, m[i].1, w, true, "2026-08-01 00:00:00", None);
        seed::insert_stock_tx(&o.primary, &s, id, m[i].0, SYMBOLS[i], &TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("OC-{i}")),
            order_type: 0, amount: Some("0.1"), quote_amount: Some("10"), price: Some("100"), quote_amount_exec: Some("10"), amount_exec: Some("0.1"),
            created_at: "2026-08-01 14:00:01".into() });
    }
    o.primary.execute("UPDATE indices SET top_coins = '[\"AAA.US\",\"BBB.US\",\"DDD.US\",\"EEE.US\"]'", []).unwrap(); // CCC left the index
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    let (in_idx, exited): (bool, Option<String>) = o.primary.query_row(
        "SELECT in_index, exited_at FROM bot_index_assets WHERE asset_id = ?1", [m[2].0], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((in_idx, exited.as_deref()), (false, Some("2026-09-01 14:00:00.500000")), "marked out at the tick (update_all, updated_at kept)");
    assert!(!posted_symbols(&t).contains(&"CCC".to_string()), "a quitter is not bought");
    assert!(t.posted_orders().iter().all(|b| b["side"] == "buy"), "and never sold");
    assert_eq!(in_index(&o, id).into_iter().map(|(s, _)| s).collect::<Vec<_>>(), vec!["AAA", "BBB", "DDD"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_reentrant_keeps_its_entered_at() {
    let (_d, o, _s, id, m) = universe(3, 0.0, false);
    seed::insert_member(&o.primary, id, m[2].0, m[2].1, 0.2, false, "2026-08-01 00:00:00", Some("2026-08-15 10:30:00"));
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    let row: (bool, String, Option<String>) = o.primary.query_row(
        "SELECT in_index, entered_at, exited_at FROM bot_index_assets WHERE asset_id = ?1", [m[2].0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!(row, (true, "2026-08-01 00:00:00".into(), None), "entered_at ||= now; exited_at cleared");
    assert_eq!(quote_reads(&t, "CCC"), 1, "an exited row is not an incumbent: it is probed like a newcomer");
}

#[tokio::test(flavor = "current_thread")]
async fn a_missing_index_fails_the_tick_as_rails_does() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    o.primary.execute("DELETE FROM indices", []).unwrap();
    o.primary.execute("INSERT INTO indices (external_id, source, created_at, updated_at) VALUES ('other', 'deltabadger', '2026-01-01 00:00:00', '2099-01-01 00:00:00')", []).unwrap();
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    let out = tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    assert!(matches!(out, TickOutcome::Rescheduled), "{out:?}");
    let error: String = o.primary.query_row("SELECT json_extract(details, '$.error') FROM bot_activity_logs WHERE event = 'execution_failed'", [], |r| r.get(0)).unwrap();
    assert_eq!(error, "Index not found");
    assert!(t.posted_orders().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn hold_all_buys_the_whole_universe_with_flattened_weights() {
    let (_d, o, _s, id, _) = universe(3, 0.5, true);
    let t = script([100.0, 200.0, 300.0, 400.0, 500.0]);
    tick::tick(&o.primary, &venue(&t), id, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    let members = in_index(&o, id);
    assert_eq!(members.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), SYMBOLS.to_vec(), "the bounded universe, not num_coins");
    let want = index::blend(&[5e12, 3e12, 2e12, 1e12, 5e11], 0.5);
    for ((sym, got), w) in members.iter().zip(want) { assert!((got - w).abs() < 5e-7, "{sym}: {got} vs {w} (decimal(10,6))"); }
    assert_eq!(t.posted_orders().len(), 5);
}

#[test]
fn an_index_bot_is_admitted_on_a_data_api_category_index_only() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    let reasons = |o: &store::Opened| eligibility::bot_reasons(&o.primary, &model::load_bot(&o.primary, id).unwrap()).unwrap();
    assert_eq!(reasons(&o), Vec::<String>::new());
    o.primary.execute("UPDATE indices SET source = 'coingecko'", []).unwrap();
    assert!(reasons(&o).iter().any(|r| r.contains("not a data-api index")), "{:?}", reasons(&o));
    o.primary.execute("UPDATE indices SET source = 'deltabadger'", []).unwrap();
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.index_type', 'top')", []).unwrap();
    assert!(reasons(&o).iter().any(|r| r.starts_with("index_type")), "{:?}", reasons(&o));
    o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.index_type', 'category', '$.num_coins', 'ten', '$.allocation_flattening', 1.5)", []).unwrap();
    let r = reasons(&o);
    assert!(r.iter().any(|r| r.starts_with("num_coins")) && r.iter().any(|r| r.starts_with("allocation_flattening")), "{r:?}");
}


#[test]
fn stock_and_index_intents_recheck_status_members_settings_and_every_ticker() {
    use deltabadger::engine::{amount, basket, placement, venue_rules::ALPACA};
    use deltabadger::ruby::BigDec;
    for index in [false,true] {
        for mutation in ["status", "settings", "member", "ticker", "split", "instrument", "unavailable", "provider"] {
            let (_d,o,s)=common::install_alpaca();
            let (asset,ticker)=seed::add_alpaca_stock(&o.primary,&s,"AAPL");
            seed::insert_index(&o.primary,"nasdaq-100",&["AAPL.US"],&json!({"AAPL.US":3e12}));
            let id=if index {seed::index_bot(&o.primary,&s,"nasdaq-100",1,0.0,false)} else {
                seed::insert_bot(&o.primary,&s,&seed::BotSpec::weekly(60.0,"2026-09-01 14:00:00").weights(&[(asset,1.0)]))
            };
            basket::write_members(&o.primary,id,&[(asset,ticker,1.0)],at(T0)).unwrap();
            seed::fresh_stock_jobs(&o.primary,at(T0));
            let bot=model::load_bot(&o.primary,id).unwrap();
            let ticker=model::ticker_by_id(&o.primary,s.exchange_id,ticker).unwrap().unwrap();
            let composition=placement::composition_snapshot(&o.primary,&bot).unwrap();
            let amount::Sizing::Place(plan)=amount::size(&bot,&ticker,&BigDec::from_i64(60),&BigDec::from_i64(100),ALPACA.minimum_logic).unwrap() else {panic!("sized")};
            let reconciled=deltabadger::engine::splits::snapshot(&o.primary,&bot).unwrap();
            let writer=rusqlite::Connection::open(o.primary.path().unwrap()).unwrap();
            match mutation {
                "status"=>{writer.execute("UPDATE bots SET status=2 WHERE id=?1",[id]).unwrap();},
                "settings"=>{writer.execute("UPDATE bots SET settings=json_set(settings,'$.num_coins',2) WHERE id=?1",[id]).unwrap();},
                "member"=>{writer.execute("UPDATE bot_index_assets SET target_allocation=0.5 WHERE bot_id=?1",[id]).unwrap();},
                "instrument"=>{writer.execute("UPDATE assets SET instrument_type='option' WHERE id=?1",[asset]).unwrap();},
                "unavailable"=>{writer.execute("UPDATE tickers SET available=0 WHERE id=?1",[ticker.id]).unwrap();},
                "provider"=>{ if !index {continue;} writer.execute("DELETE FROM app_configs WHERE key='market_data_provider'",[]).unwrap();},
                "ticker"=>{writer.execute("UPDATE tickers SET base_decimals=8 WHERE id=?1",[ticker.id]).unwrap();},
                _=>{writer.execute("UPDATE bots SET restatement_generation=restatement_generation+1 WHERE id=?1",[id]).unwrap();},
            }
            assert!(matches!(placement::begin_unless_changed(&o.primary,&bot,&plan,&[&ticker],&composition,&reconciled,&FixedClock(at(T0))).unwrap(), placement::Begun::Changed),"index={index}, {mutation}");
            assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
        }
    }
}


#[tokio::test(flavor="current_thread")]
async fn unsupported_newcomers_never_publish_or_trade() {
    for instrument in ["option", "tokenized_equity"] {
        let (_d,o,_s,id,m)=universe(3,0.0,false);
        o.primary.execute("UPDATE assets SET instrument_type=?1 WHERE id=?2",rusqlite::params![instrument,m[0].0]).unwrap();
        assert!(eligibility::check_install(&o.primary).unwrap().refusal().is_ok());
        let t=script([100.0;5]);
        tick::tick(&o.primary,&venue(&t),id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
        assert!(t.posted_orders().is_empty());
        assert!(in_index(&o,id).is_empty());
        assert!(eligibility::check_install(&o.primary).unwrap().refusal().is_ok());
    }
}

#[test]
fn index_constituent_reclassification_is_guarded_even_without_allocations() {
    use deltabadger::jobs::import::{in_transaction,Touched};
    for active in [true,false] {
        let (_d,o,_s,id,m)=universe(3,0.0,false);
        seed::insert_member(&o.primary,id,m[0].0,m[0].1,1.0,active,"2026-08-01 00:00:00",None);
        let result=in_transaction(&o.primary,&seed::cipher(),"index reclassification",|tx| {
            tx.execute("UPDATE assets SET instrument_type='tokenized_equity' WHERE id=?1",[m[0].0]).map_err(|e|e.to_string())?;
            Ok(((),Touched::Assets(vec![m[0].0])))
        });
        assert!(result.is_err(),"guard must reject in/out constituent reclassification");
        let instrument:String=o.primary.query_row("SELECT instrument_type FROM assets WHERE id=?1",[m[0].0],|r|r.get(0)).unwrap();
        assert_eq!(instrument,"stock");
        assert!(eligibility::check_install(&o.primary).unwrap().refusal().is_ok());
    }
}

#[tokio::test(flavor="current_thread")]
async fn switching_or_removing_the_provider_invalidates_index_admission_and_freshness() {
    use deltabadger::{app_config,engine::staleness};
    for change in ["coingecko","removed","token_removed","url_changed"] {
        let (_d,o,_s,id,_)=universe(3,0.0,false);
        match change {
            "coingecko"=>app_config::set(&o.primary,&seed::cipher(),"market_data_provider","coingecko",at(T0)).unwrap(),
            "removed"=>{o.primary.execute("DELETE FROM app_configs WHERE key='market_data_provider'",[]).unwrap();},
            "token_removed"=>{o.primary.execute("DELETE FROM app_configs WHERE key='market_data_token'",[]).unwrap();},
            _=>app_config::set(&o.primary,&seed::cipher(),"market_data_url","https://changed.invalid",at(T0)).unwrap(),
        }
        let bot=model::load_bot(&o.primary,id).unwrap();
        if change!="url_changed" { assert!(!eligibility::bot_reasons(&o.primary,&bot).unwrap().is_empty(),"{change}"); }
        assert!(staleness::stale(&o.primary,&bot,at(T0)).unwrap().is_some(),"{change}");
        let t=script([100.0;5]);
        tick::tick(&o.primary,&venue(&t),id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
        assert!(t.posted_orders().is_empty(),"{change}");
    }
}

#[tokio::test(flavor="current_thread")]
async fn duplicate_ranked_assets_use_the_recorded_rails_asset_map_blend() {
    let (_d,o,_s,id,_)=universe(3,0.5,false);
    o.primary.execute("UPDATE indices SET top_coins='[\"AAA.US\",\"AAA.US\",\"BBB.US\"]',weights='{\"AAA.US\":100,\"BBB.US\":10}'",[]).unwrap();
    let t=script([100.0;5]);
    tick::tick(&o.primary,&venue(&t),id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
    let recorded=&common::vectors()["index_duplicate_blend"];
    assert_eq!(in_index(&o,id),vec![("AAA".into(),recorded[0]["weight"].as_f64().unwrap()),("BBB".into(),recorded[2]["weight"].as_f64().unwrap())]);
}

#[tokio::test(flavor="current_thread")]
async fn a_split_for_another_held_asset_during_positions_ends_the_tick() {
    use deltabadger::venue::http::{Transport,HttpRequest,HttpResponse,TransportError};
    struct DuringPositions { script:ScriptedTransport, db:rusqlite::Connection, bot:i64, row:i64 }
    impl Transport for DuringPositions {
        async fn send(&self, req:&HttpRequest)->Result<HttpResponse,TransportError> {
            if req.path=="/v2/positions" {
                tokio::task::yield_now().await;
                // B's incorrect old split arrives while reconciliation is checking A's old split.
                self.db.execute("UPDATE account_transactions SET raw_data=json_set(raw_data,'$.corporate_action','split') WHERE id=?1",[self.row]).unwrap();
                self.db.execute("UPDATE bots SET restatement_generation=restatement_generation+1 WHERE id=?1",[self.bot]).unwrap();
            }
            self.script.send(req).await
        }
    }
    let (_d,o,s,id,m)=universe(3,0.0,false);
    for (i,sym) in ["AAA","BBB"].iter().enumerate() {
        seed::insert_stock_tx(&o.primary,&s,id,m[i].0,sym,&TxSpec{status:0,external_status:Some(2),external_id:Some(format!("held-{sym}")),order_type:0,amount:Some("10"),quote_amount:Some("1000"),price:Some("100"),amount_exec:Some("10"),quote_amount_exec:Some("1000"),created_at:"2026-08-01 14:00:00".into()});
    }
    let mut b=0;
    for sym in ["AAA","BBB"] {
        let row=seed::insert_split(&o.primary,&s,sym,"2026-08-05 00:00:00",Some("3:2"));
        o.primary.execute("UPDATE account_transactions SET base_amount=5,raw_data=?1 WHERE id=?2",rusqlite::params![json!({"corporate_action":if sym=="AAA" {"split"} else {"pending"},"qty":"-10","split_ratio":"3:2","merged_activity_ids":["a","b"]}).to_string(),row]).unwrap();
        b=row;
    }
    let scripted=script([100.0;5]);
    // Override positions only, keeping all other requests visible in the underlying script.
    struct Positions { inner:DuringPositions }
    impl Transport for Positions {
        async fn send(&self,req:&HttpRequest)->Result<HttpResponse,TransportError> {
            let reply=self.inner.send(req).await?;
            if req.path=="/v2/positions" {return Ok(HttpResponse{status:200,body:json!([{"symbol":"AAA","asset_class":"us_equity","qty":"15"},{"symbol":"BBB","asset_class":"us_equity","qty":"30"}]).to_string()});}
            Ok(reply)
        }
    }
    let transport=Positions{inner:DuringPositions{script:scripted.clone(),db:rusqlite::Connection::open(o.primary.path().unwrap()).unwrap(),bot:id,row:b}};
    let v=AlpacaVenue::new(transport,Urls::for_passphrase(Some("paper")));
    let out=tick::tick(&o.primary,&v,id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
    assert!(matches!(out,TickOutcome::Done{placed:false}),"{out:?}");
    assert!(scripted.posted_orders().is_empty());
    assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
    let reason:String=o.primary.query_row("SELECT json_extract(details,'$.reason') FROM bot_activity_logs WHERE event='dca_skipped_restatement' ORDER BY id DESC LIMIT 1",[],|r|r.get(0)).unwrap();
    assert!(reason.contains("changed during reconciliation"),"{reason}");
    let out=tick::tick(&o.primary,&v,id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
    assert!(matches!(out,TickOutcome::Done{placed:false}),"next pass re-reconciles: {out:?}");
    assert!(model::load_bot(&o.primary,id).unwrap().transient["rust_split_hold"]["reason"].as_str().unwrap().contains("BBB"));
}

#[tokio::test(flavor="current_thread")]
async fn a_long_split_history_stands_down_with_a_visible_reason() {
    let (_d,o,s,id,m)=universe(1,0.0,false);
    seed::insert_stock_tx(&o.primary,&s,id,m[0].0,"AAA",&TxSpec{status:0,external_status:Some(2),external_id:Some("old-fill".into()),order_type:0,amount:Some("1"),quote_amount:Some("100"),price:Some("100"),amount_exec:Some("1"),quote_amount_exec:Some("100"),created_at:"2000-01-01 00:00:00".into()});
    for i in 0..1500 {
        let day=(at("2000-01-02T00:00:00Z")+Duration::days(i)).format("%Y-%m-%d %H:%M:%S").to_string();
        seed::insert_split(&o.primary,&s,"AAA",&day,Some("3:2"));
    }
    let t=script([100.0;5]);
    let out=tick::tick(&o.primary,&venue(&t),id,&FixedClock(at(T0)),&mut Attempts::default()).await.unwrap();
    assert!(matches!(out,TickOutcome::Rescheduled),"{out:?}");
    assert!(t.posted_orders().is_empty());
    assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
    let error:String=o.primary.query_row("SELECT json_extract(details,'$.error') FROM bot_activity_logs WHERE event='execution_failed'",[],|r|r.get(0)).unwrap();
    assert!(error.contains("split walk refused"),"{error}");
}

#[test]
fn encrypted_provider_configuration_and_environment_precedence_match_rails() {
    use deltabadger::{app_config,engine::{provider,staleness}};
    let (_d,o,_s,id,_)=universe(1,0.0,false);
    for (key,value) in [("market_data_provider","coingecko"),("market_data_url","https://encrypted.invalid"),("market_data_token","encrypted-script-token")] {
        app_config::set(&o.primary,&seed::cipher(),key,value,at(T0)).unwrap();
    }
    provider::bind(&o.primary,&seed::cipher(),&|k|(k=="MARKET_DATA_URL").then(||"https://environment.invalid".into())).unwrap();
    let config=provider::config(&o.primary).unwrap().unwrap();
    assert_eq!(config.url,"https://encrypted.invalid","database credential rows win; env URL selects provider");
    assert_eq!(config.token,"encrypted-script-token");
    let bot=model::load_bot(&o.primary,id).unwrap();
    assert!(eligibility::bot_reasons(&o.primary,&bot).unwrap().is_empty());
    assert!(staleness::stale(&o.primary,&bot,at(T0)).unwrap().is_some());
    assert!(matches!(staleness::verdict(&o.primary,&bot,at(T0)).unwrap(),staleness::Verdict::Unknown(_)),"scheduler can start to refresh");
    seed::fresh_stock_jobs(&o.primary,at(T0));
    assert!(staleness::stale(&o.primary,&bot,at(T0)).unwrap().is_none());
    app_config::set(&o.primary,&seed::cipher(),"market_data_token","",at(T0)).unwrap();
    assert!(provider::config(&o.primary).unwrap().is_none(),"a blank db token does not fall back to environment");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stock_only_index_ignores_stale_unrelated_crypto_feeds() {
    let (_d, o, _s, id, _) = universe(3, 0.0, false);
    let old = at(T0) - Duration::days(7);
    for job in [deltabadger::jobs::reference::ALPACA_CRYPTO, deltabadger::jobs::reference::ASSETS] {
        deltabadger::app_config::set_plain(&o.primary,
            &deltabadger::jobs::state::key(job, None),
            &json!({"last_success_at":old.to_rfc3339(),"rails_at":old.to_rfc3339()}).to_string(), old).unwrap();
    }
    let t = script([100.0; 5]);
    let out = tick_until_settled(&o, &t, id, at(T0)).await;
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!(posted_symbols(&t), ["AAA", "BBB", "CCC"]);
}
