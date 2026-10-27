//! Slice 6c-1: composition placement context stays dark, but its durable protocol is complete.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::scripted::{accepted, ok, script, venue, POST_SEND, PRE_SEND};
use common::seed::{self, BotSpec};
use deltabadger::engine::{amount::{self, TxKind}, composition, model, placement::{self, Op}, rebalance, tick, FixedClock};
use deltabadger::{ruby::BigDec, store, venue::Side};
use serde_json::{json, Value};

fn now() -> DateTime<Utc> { "2026-09-01T12:30:00Z".parse().unwrap() }
fn setup(kind: TxKind) -> (tempfile::TempDir, store::Opened, i64, amount::OrderPlan) {
    let (d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let bot = model::load_bot(&o.primary, id).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let side = match kind { TxKind::Liquidation => Side::Sell, TxKind::Regular | TxKind::Rebalance | TxKind::Redeploy => Side::Buy };
    let (plan, below) = amount::size_order(&ticker, side, &BigDec::from_i64(64000), &BigDec::parse("0.001").unwrap(), &BigDec::from_i64(64), deltabadger::engine::venue_rules::ALPACA.minimum_logic, kind).unwrap();
    assert!(!below);
    (d, o, id, plan)
}
fn begin(o: &store::Opened, id: i64, plan: &amount::OrderPlan, loss: bool) -> placement::Intent {
    let pending = rebalance::Pending { phase: rebalance::PHASE_BUYING.into(), sell_transaction_id: None, buy_transaction_id: None, remaining_quote_amount: Some(BigDec::from_i64(64)), buy_attempted: true };
    let op = match plan.kind { TxKind::Regular => None, TxKind::Rebalance => Some(Op::Rebalance(&pending)), TxKind::Liquidation => Some(Op::Liquidation { symbol: "BTC" }), TxKind::Redeploy => Some(Op::Redeploy) };
    placement::begin_unfenced(&o.primary, &model::load_bot(&o.primary, id).unwrap(), plan, op, loss, &FixedClock(now())).unwrap()
}
fn state(o: &store::Opened, id: i64) -> Value { model::load_bot(&o.primary, id).unwrap().transient }
fn events(o: &store::Opened) -> Vec<String> { o.primary.prepare("SELECT event FROM bot_activity_logs ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect() }
fn rows(o: &store::Opened) -> i64 { o.primary.query_row("SELECT count(*) FROM transactions", [], |r| r.get(0)).unwrap() }

#[tokio::test(flavor = "current_thread")]
async fn outcome_matrix_by_kind_is_atomic_and_never_borrows_rebalance_state() {
    for kind in [TxKind::Regular, TxKind::Rebalance, TxKind::Liquidation, TxKind::Redeploy] {
        for (label, response) in [
            ("accepted", accepted("order-1")),
            ("safe", json!({"status":400,"body":{"message":"InvalidNonce"}})),
            ("unsafe", json!({"status":400,"body":{"message":"insufficient funds"}})),
            ("server", json!({"status":503,"body":{}})),
            ("timeout", json!({"network":"post_send","message":POST_SEND})),
            ("no_id", ok(json!({"status":"pending_new"}))),
            ("pre", json!({"network":"pre_send","message":PRE_SEND})),
        ] {
            let (_d, o, id, plan) = setup(kind);
            if kind == TxKind::Liquidation { o.primary.execute("UPDATE users SET wash_sale_enabled=1", []).unwrap(); }
            let intent = begin(&o, id, &plan, kind == TxKind::Liquidation);
            let t = script(json!({"POST /v2/orders":[response]}));
            let sent = placement::send_recorded(&o.primary, &venue(&t), &model::load_bot(&o.primary, id).unwrap(), &intent, &FixedClock(now())).await.unwrap();
            let special = matches!(kind, TxKind::Liquidation | TxKind::Redeploy);
            let expected_rows = i64::from(label == "accepted" || (label == "safe" && kind != TxKind::Regular) || (label == "unsafe" && kind == TxKind::Regular));
            assert_eq!(rows(&o), expected_rows, "{kind:?} {label}: {sent:?}");
            let s = state(&o, id);
            if special {
                let prefix = if kind == TxKind::Liquidation { "liquidation" } else { "redeploy" };
                let pending = &s[format!("{prefix}_pending")];
                if matches!(label, "accepted" | "safe" | "pre") { assert!(pending.is_null(), "{kind:?} {label}: {s}"); }
                else { assert_eq!(pending["state"], "ambiguous", "{kind:?} {label}"); }
                assert!(s.get("rebalance_pending").is_none());
                assert!(!s.as_object().unwrap().keys().any(|k| k.starts_with("rebalance_")), "{kind:?} {label}");
                if kind == TxKind::Liquidation {
                    let locked: bool = o.primary.query_row("SELECT EXISTS(SELECT 1 FROM wash_sale_locks WHERE buy_locked_until IS NOT NULL)", [], |r| r.get(0)).unwrap();
                    assert_eq!(locked, !matches!(label, "safe" | "pre"), "{label}");
                    assert_eq!(events(&o).contains(&"wash_sale_locked".into()), label == "accepted");
                }
                assert!(s.get("rust_defer_until").is_none());
                let event = match label { "accepted" => Some("placed"), "pre" => Some("skipped"), "safe" => None, _ => Some("ambiguous") };
                if let Some(event) = event { assert!(events(&o).contains(&format!("{prefix}_{event}")), "{kind:?} {label}"); }
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn liquidation_restore_survives_restart_and_never_writes_rebalance_wash_keys() {
    let (d, o, id, plan) = setup(TxKind::Liquidation);
    o.primary.execute("UPDATE users SET wash_sale_enabled=1", []).unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    o.primary.execute("INSERT INTO wash_sale_locks(asset_id,user_id,buy_locked_until,confirmed_locked_until,created_at,updated_at) VALUES (?1,?2,'2026-09-10 00:00:00','2026-09-10 00:00:00',?3,?3)", rusqlite::params![plan.ticker.base_asset_id,bot.user_id,"2026-09-01 00:00:00"]).unwrap();
    begin(&o, id, &plan, true);
    assert!(state(&o,id)["rust_placement"]["wash_claim"].is_object());
    assert!(!state(&o,id).as_object().unwrap().keys().any(|k| k.starts_with("rebalance_")));
    drop(o);
    let o = store::open(&store::Paths::from_env(&|_| None,d.path())).unwrap();
    // The durable intent is rehydrated after the commit/send crash boundary.
    let intent = placement::load_intent(&o.primary, &model::load_bot(&o.primary,id).unwrap(), model::credential_version(&o.primary, &model::load_bot(&o.primary,id).unwrap()).unwrap()).unwrap().unwrap();
    let t = script(json!({"POST /v2/orders":[{"network":"pre_send","message":PRE_SEND}]}));
    placement::send_recorded(&o.primary,&venue(&t),&model::load_bot(&o.primary,id).unwrap(),&intent,&FixedClock(now())).await.unwrap();
    let until: String = o.primary.query_row("SELECT buy_locked_until FROM wash_sale_locks",[],|r|r.get(0)).unwrap();
    assert_eq!(until,"2026-09-10 00:00:00");
    assert!(state(&o,id)["liquidation_pending"].is_null());
    assert_eq!(rows(&o),0);
}

#[test]
fn shared_predicate_keeps_old_cases_and_bounds_only_legacy_selling_markers() {
    let (_d,o,id,_) = setup(TxKind::Regular);
    for (data, busy) in [
        (json!({}),false), (json!({"rebalance_pending":{"phase":"selling"}}),true),
        (json!({"liquidation_pending":{"state":"ambiguous"}}),true), (json!({"redeploy_pending":{"state":"placing"}}),true),
        (json!({"liquidation_selling_since":{"at":now().timestamp()-899}}),true),
        (json!({"liquidation_selling_since":{"at":now().timestamp()-900}}),false),
        (json!({"liquidation_selling_since":{"at":1,"holdings":["BTC"]}}),true),
        (json!({"liquidation_selling_since":{"at":1,"holdings":["BTC"],"taken_at":2}}),true),
        (json!({"rust_redeploy_request":{"at":1}}),true),
        (json!({"rust_redeploy_request":{"at":1,"taken_at":2}}),true),
    ] {
        o.primary.execute("UPDATE bots SET transient_data=?1 WHERE id=?2",rusqlite::params![data.to_string(),id]).unwrap();
        let bot = model::load_bot(&o.primary,id).unwrap();
        assert_eq!(composition::in_flight(&o.primary,&bot,now()).unwrap(),busy,"{data}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn tick_stands_down_in_rails_order_and_promotes_orphan_markers() {
    for (data,event) in [
        (json!({"rebalance_pending":{"phase":"selling"},"liquidation_pending":{"state":"placing"},"redeploy_pending":{"state":"placing"}}),"dca_skipped_rebalance_pending"),
        (json!({"liquidation_pending":{"id":"old","symbol":"BTC","state":"placing"},"redeploy_pending":{"state":"placing"}}),"dca_skipped_liquidation_pending"),
        (json!({"redeploy_pending":{"id":"old","state":"placing"}}),"dca_skipped_redeploy_pending"),
    ] {
        let (_d,o,id,_) = setup(TxKind::Regular);
        o.primary.execute("UPDATE bots SET transient_data=?1 WHERE id=?2",rusqlite::params![data.to_string(),id]).unwrap();
        let t=script(json!({}));
        tick::tick(&o.primary,&venue(&t),id,&FixedClock(now()),&mut Default::default()).await.unwrap();
        assert!(events(&o).contains(&event.to_string()),"{:?}",events(&o));
        if event != "dca_skipped_redeploy_pending" {
            assert!(!events(&o).contains(&"redeploy_ambiguous".to_string()), "Rails reaches redeploy only after liquidation stands aside");
        }
        assert!(t.posted_orders().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn working_bot_recovery_halts_after_margin_even_with_rebalancing_off() {
    for kind in [TxKind::Liquidation,TxKind::Redeploy] {
        let (d,o,id,plan)=setup(kind);
        begin(&o,id,&plan,false);
        drop(o);
        let o=store::open(&store::Paths::from_env(&|_|None,d.path())).unwrap();
        let t=script(json!({"GET /v2/orders:by_client_order_id":[{"status":503,"body":{}}]}));
        tick::tick(&o.primary,&venue(&t),id,&FixedClock(now()+Duration::hours(1)),&mut Default::default()).await.unwrap();
        let key=if kind==TxKind::Liquidation {"liquidation_pending"} else {"redeploy_pending"};
        assert_eq!(state(&o,id)[key]["state"],"ambiguous");
        assert!(state(&o,id)["rust_placement"].is_object());
    }
}

#[test]
fn quote_cap_counts_only_regular_intents_including_legacy_ones() {
    let (_d,o,id,_) = setup(TxKind::Regular);
    o.primary.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount_limited',json('true'),'$.quote_amount_limit',100)",[]).unwrap();
    for kind in [None,Some("REGULAR"),Some("REBALANCE"),Some("LIQUIDATION"),Some("REDEPLOY")] {
        o.primary.execute("UPDATE bots SET transient_data=json_object('rust_placement',json(?1))",[json!({"transaction_type":kind,"side":"buy","quote_amount":"60"}).to_string()]).unwrap();
        let bot=model::load_bot(&o.primary,id).unwrap();
        let expected=if kind.is_none() || kind==Some("REGULAR") {40} else {100};
        assert_eq!(deltabadger::engine::accounting::exact_cap_available(&o.primary,&bot).unwrap(),Some(BigDec::from_i64(expected)),"{kind:?}");
        assert_eq!(deltabadger::engine::accounting::quote_amount_available(&o.primary,&bot).unwrap(),Some(BigDec::from_i64(expected)),"{kind:?}");
    }
}

#[test]
fn shared_predicate_preserves_waiting_abandoned_and_split_exclusions() {
    for (kind,status,busy) in [("LIQUIDATION",0,true),("LIQUIDATION",1,true),("LIQUIDATION",2,false),("LIQUIDATION",4,true),("REDEPLOY",0,true),("REDEPLOY",4,false),("REGULAR",0,false)] {
        let (_d,o,id,plan)=setup(TxKind::Regular);
        let bot=model::load_bot(&o.primary,id).unwrap();
        let s=seed::Seeded { user_id:bot.user_id, exchange_id:bot.exchange_id, btc:plan.ticker.base_asset_id, quote:plan.ticker.quote_asset_id, ticker_id:plan.ticker.id, api_key_id:1 };
        let row=seed::insert_row(&o.primary,&s,id,plan.ticker.base_asset_id,&json!({"external_status":status,"created_at":"2026-09-01 12:00:00"}));
        o.primary.execute("UPDATE transactions SET transaction_type=?1 WHERE id=?2",rusqlite::params![kind,row]).unwrap();
        let bot=model::load_bot(&o.primary,id).unwrap();
        assert_eq!(composition::in_flight(&o.primary,&bot,now()).unwrap(),busy,"{kind} {status}");
        assert_eq!(composition::busy(&o.primary,&bot,now(),false).unwrap(),busy && kind!="REDEPLOY");
        if kind=="LIQUIDATION" && status==4 {
            o.primary.execute("UPDATE bots SET transient_data=json_object('liquidation_resolved_orders',json(?1))",[json!([row.to_string()]).to_string()]).unwrap();
            assert!(!composition::in_flight(&o.primary,&model::load_bot(&o.primary,id).unwrap(),now()).unwrap());
        }
    }
}

struct Factory(deltabadger::venue::http::ScriptedTransport);
impl deltabadger::venue::VenueFactory for Factory {
    type V = deltabadger::venue::alpaca::AlpacaVenue<deltabadger::venue::http::ScriptedTransport>;
    fn for_bot(&self,_t:&str,_c:Option<deltabadger::crypto::Credentials>)->Self::V { venue(&self.0) }
}

#[tokio::test(flavor="current_thread")]
async fn idle_scan_promotes_stopped_rails_markers_without_an_intent() {
    let (d,o,id,_)=setup(TxKind::Regular);
    o.primary.execute("UPDATE bots SET status=2,transient_data=?1",[json!({"liquidation_pending":{"id":"old","symbol":"BTC","state":"placing"},"redeploy_pending":{"id":"old2","state":"placing"}}).to_string()]).unwrap();
    drop(o);
    let paths=store::Paths::from_env(&|_|None,d.path());
    let lock=deltabadger::lease::lock(&paths,now()).unwrap();
    let o=store::open(&paths).unwrap();
    let t=script(json!({}));
    let mut engine=deltabadger::engine::run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
    assert!(deltabadger::engine::eligibility::check_install(&engine.primary).unwrap().problems.is_empty(), "both markers are admitted since 6c-4");
    let mut wake=i64::MAX;
    deltabadger::engine::run::reconcile_idle_intents(&mut engine,&FixedClock(now()),&mut wake).await.unwrap();
    let bot=model::load_bot(&engine.primary,id).unwrap();
    assert_eq!(bot.transient["liquidation_pending"],json!({"id":"old","symbol":"BTC","state":"ambiguous"}));
    assert_eq!(bot.transient["redeploy_pending"],json!({"id":"old2","state":"ambiguous"}));
    assert!(t.posted_orders().is_empty());
    deltabadger::engine::run::reconcile_idle_intents(&mut engine,&FixedClock(now()+Duration::minutes(1)),&mut wake).await.unwrap();
    let n:i64=engine.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE event LIKE '%_ambiguous'",[],|r|r.get(0)).unwrap();
    assert_eq!(n,2);
}

#[tokio::test(flavor="current_thread")]
async fn crash_boundaries_recover_each_composition_kind_once_without_dca_deferral() {
    for kind in [TxKind::Liquidation,TxKind::Redeploy] {
        for boundary in ["before_commit","before_send","after_send","after_settle"] {
            let (d,o,id,plan)=setup(kind);
            let t=script(json!({"POST /v2/orders":[accepted("landed")]}));
            if boundary!="before_commit" {
                let intent=begin(&o,id,&plan,false);
                if boundary=="after_send" { placement::send(&o.primary,&venue(&t),&intent,&FixedClock(now())).await.unwrap(); }
                if boundary=="after_settle" { placement::send_recorded(&o.primary,&venue(&t),&model::load_bot(&o.primary,id).unwrap(),&intent,&FixedClock(now())).await.unwrap(); }
            }
            drop(o);
            let o=store::open(&store::Paths::from_env(&|_|None,d.path())).unwrap();
            let bot=model::load_bot(&o.primary,id).unwrap();
            let answer=if boundary=="after_send" {
                ok(json!({"id":"landed","client_order_id":bot.rust_placement().unwrap()["cl_ord_id"],"symbol":"BTC/USD","status":"new","type":"market","side":plan.side.as_str(),"qty":"0.001","notional":null,"filled_qty":"0","filled_avg_price":null,"limit_price":null}))
            } else { common::scripted::not_found() };
            let lookup=script(json!({"GET /v2/orders:by_client_order_id":[answer]}));
            for _ in 0..2 {
                placement::recover_since(&o.primary,&venue(&lookup),&model::load_bot(&o.primary,id).unwrap(),&FixedClock(now()+Duration::hours(1)),now()+Duration::minutes(1)).await.unwrap();
            }
            assert_eq!(rows(&o),i64::from(matches!(boundary,"after_send"|"after_settle")),"{kind:?} {boundary}: {:?}",bot.transient);
            assert!(state(&o,id)["rust_placement"].is_null(),"{kind:?} {boundary}");
            assert!(state(&o,id)["rust_defer_until"].is_null());
            assert!(lookup.posted_orders().is_empty());
        }
    }
}

#[test]
fn failed_intent_commit_rolls_back_the_pending_marker_and_wash_claim() {
    let (_d,o,id,plan)=setup(TxKind::Liquidation);
    o.primary.execute("UPDATE users SET wash_sale_enabled=1",[]).unwrap();
    o.primary.execute_batch("CREATE TRIGGER fail_pending BEFORE UPDATE OF transient_data ON bots WHEN json_extract(NEW.transient_data,'$.liquidation_pending') IS NOT NULL BEGIN SELECT RAISE(ABORT,'crash'); END;").unwrap();
    let bot=model::load_bot(&o.primary,id).unwrap();
    assert!(placement::begin_unfenced(&o.primary,&bot,&plan,Some(Op::Liquidation{symbol:"BTC"}),true,&FixedClock(now())).is_err());
    assert!(state(&o,id)["rust_placement"].is_null());
    assert!(state(&o,id)["liquidation_pending"].is_null());
    let n:i64=o.primary.query_row("SELECT count(*) FROM wash_sale_locks",[],|r|r.get(0)).unwrap();
    assert_eq!(n,0);
}

#[test]
fn accepted_row_and_pending_clear_roll_back_together_then_restart_settles_once() {
    for kind in [TxKind::Liquidation,TxKind::Redeploy] {
        let (d,o,id,plan)=setup(kind);
        let intent=begin(&o,id,&plan,false);
        o.primary.execute_batch("CREATE TRIGGER fail_log BEFORE INSERT ON bot_activity_logs BEGIN SELECT RAISE(ABORT,'crash'); END;").unwrap();
        assert!(placement::record_accepted(&o.primary,&model::load_bot(&o.primary,id).unwrap(),&intent,"landed").is_err());
        assert_eq!(rows(&o),0);
        assert!(state(&o,id)["rust_placement"].is_object());
        drop(o);
        let o=store::open(&store::Paths::from_env(&|_|None,d.path())).unwrap();
        o.primary.execute_batch("DROP TRIGGER fail_log").unwrap();
        placement::resolve_by_operator(&o.primary,id,placement::OperatorResolution::Placed("landed".into()),now()).unwrap();
        assert_eq!(rows(&o),1);
        assert!(state(&o,id)["rust_placement"].is_null());
        assert!(state(&o,id)["rust_defer_until"].is_null());
        assert!(placement::resolve_by_operator(&o.primary,id,placement::OperatorResolution::Placed("landed".into()),now()).is_err());
    }
}

#[tokio::test(flavor="current_thread")]
async fn composition_dispatch_allows_stop_but_fences_archive_composition_edits_and_reversal() {
    for kind in [TxKind::Liquidation,TxKind::Redeploy] {
        for (edit,allowed) in [
            ("UPDATE bots SET status=2",true),
            ("UPDATE bots SET status=7",false),
            ("UPDATE bots SET settings=json_set(settings,'$.quote_amount',61)",false),
            ("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_reversal_cancels',json('[{\"id\":1}]'))",false),
        ] {
            let (_d,o,id,plan)=setup(kind);
            let intent=begin(&o,id,&plan,false);
            o.primary.execute(edit,[]).unwrap();
            let t=script(json!({}));
            let sent=placement::send_recorded(&o.primary,&venue(&t),&model::load_bot(&o.primary,id).unwrap(),&intent,&FixedClock(now())).await.unwrap();
            assert_eq!(matches!(sent,placement::Sent::Accepted(_)),allowed,"{kind:?} {edit}: {sent:?}");
            assert_eq!(t.posted_orders().len(),usize::from(allowed));
        }
    }
}

#[tokio::test(flavor="current_thread")]
async fn rebalance_stands_down_for_liquidation_and_redeploy_requests() {
    for key in ["liquidation_pending","rust_redeploy_request"] {
        let (_d,o,id,_)=setup(TxKind::Regular);
        o.primary.execute("UPDATE bots SET status=2,settings=json_set(settings,'$.rebalance_enabled',json('true')),transient_data=json_object(?1,json('{\"id\":\"queued\"}'))",[key]).unwrap();
        let bot=model::load_bot(&o.primary,id).unwrap();
        let prices=tick::PriceCache::default();
        let cx=tick::TickContext{credential_version:model::credential_version(&o.primary,&bot).unwrap(),prices:&prices,process_start:now(),stopping:&||false,below_minimum:&|_,_|{},claimed:&|_|{}};
        let t=script(json!({}));
        assert_eq!(rebalance::run(&o.primary,&venue(&t),id,&FixedClock(now()),&cx).await.unwrap(),rebalance::Outcome::Skipped("not_due"));
        assert!(t.posted_orders().is_empty());
        assert!(!rebalance::due(&bot.settings,&json!({}),true,Some(&BigDec::from_i64(100))).unwrap());
    }
}

#[tokio::test(flavor="current_thread")]
async fn idle_recovery_halts_unknown_intents_once_at_the_margin() {
    for kind in [TxKind::Liquidation,TxKind::Redeploy,TxKind::Rebalance] {
        let (d,o,id,plan)=setup(kind);
        o.primary.execute("UPDATE bots SET status=2",[]).unwrap();
        begin(&o,id,&plan,false);
        drop(o);
        let paths=store::Paths::from_env(&|_|None,d.path());
        let lock=deltabadger::lease::lock(&paths,now()).unwrap();
        let o=store::open(&paths).unwrap();
        let t=script(json!({"GET /v2/orders:by_client_order_id":[{"status":503,"body":{}}]}));
        let mut engine=deltabadger::engine::run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock);
        let mut wake=i64::MAX;
        for minutes in [0,20,21] {
            deltabadger::engine::run::reconcile_idle_intents(&mut engine,&FixedClock(now()+Duration::minutes(minutes)),&mut wake).await.unwrap();
            let bot=model::load_bot(&engine.primary,id).unwrap();
            let (key,field)=match kind {TxKind::Liquidation=>("liquidation_pending","state"),TxKind::Redeploy=>("redeploy_pending","state"),TxKind::Rebalance=>("rebalance_pending","phase"),TxKind::Regular=>unreachable!()};
            assert_eq!(bot.transient[key][field]=="ambiguous",minutes>=20,"{kind:?} {minutes}");
        }
        let n:i64=engine.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE event LIKE '%_ambiguous'",[],|r|r.get(0)).unwrap();
        assert_eq!(n,1);
        assert!(t.posted_orders().is_empty());
    }
}

#[test]
fn composition_context_uses_the_pinned_rails_oracle() {
    use sha2::{Digest,Sha256};
    let fixture:Value=serde_json::from_str(include_str!("fixtures/index_histories.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for file in ["app/models/bot/liquidation_state.rb","app/models/bot/composition/liquidatable.rb","app/models/bot/composition/redeployable.rb","app/models/bot/rebalanceable.rb"] {
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap())),fixture["ported_sources"][file].as_str().unwrap(),"{file}: re-record and re-check the composition context");
    }
}

#[test]
fn fenced_operation_claims_allow_stopped_bots_and_commit_no_dca_run() {
    for kind in [TxKind::Liquidation,TxKind::Redeploy] {
        let (_d,o,id,plan)=setup(kind);
        o.primary.execute("UPDATE bots SET status=2",[]).unwrap();
        let bot=model::load_bot(&o.primary,id).unwrap();
        let snapshot=placement::composition_snapshot(&o.primary,&bot).unwrap();
        let reconciled=deltabadger::engine::splits::snapshot(&o.primary,&bot).unwrap();
        let op=if kind==TxKind::Liquidation {Op::Liquidation{symbol:"BTC"}} else {Op::Redeploy};
        let begun=placement::begin_operation(&o.primary,&bot,&plan,&[&plan.ticker],&snapshot,&reconciled,&model::credential_version(&o.primary,&bot).unwrap(),None,op,false,None,&FixedClock(now())).unwrap();
        assert!(matches!(begun,placement::Begun::Intent(_)));
        let raw=state(&o,id);
        assert!(raw["rust_placement"]["run_identity"].is_null());
        assert_eq!(raw["rust_placement"]["composition"],snapshot);
        assert!(!placement::pending_requires_checkpoint(&model::load_bot(&o.primary,id).unwrap()).unwrap());
    }
}
