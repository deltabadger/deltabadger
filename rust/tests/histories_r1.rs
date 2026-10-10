mod common;
use chrono::{DateTime,Utc};
use common::seed::{self,BotSpec};
use deltabadger::{engine::{amount,model,placement,eligibility,FixedClock},ruby::{BigDec,Num}};
use serde_json::{json,Value};
fn bd(s:&str)->BigDec { BigDec::parse(s).unwrap() }
fn now()->DateTime<Utc>{"2026-01-05T12:00:01Z".parse().unwrap()}
fn setup(cap:Value,spent:&str)->(tempfile::TempDir,deltabadger::store::Opened,common::seed::Seeded,i64){
    let (d,o,s)=common::install_alpaca();
    let id=seed::insert_bot(&o.primary,&s,&BotSpec::weekly(100.0,"2026-01-05 12:00:00").with("quote_amount_limited",json!(true)).with("quote_amount_limit",cap).transient("quote_amount_limit_enabled_at",json!("2026-01-05T12:00:00Z")));
    seed::insert_row(&o.primary,&s,id,s.btc,&json!({"external_status":3,"amount":"10","amount_exec":bd(spent).div(&bd("10")).unwrap().to_s_f(),"quote_amount_exec":spent,"price":"10","created_at":"2026-01-05 12:00:00"}));
    (d,o,s,id)
}
#[tokio::test(flavor="current_thread")]
async fn r1_null_closed_fill_refuses_and_places_nothing(){
    let (_d,o,s,id)=setup(json!(100),"0");
    let pair=json!({"base_decimals":9,"quote_decimals":2,"price_decimals":2,"minimum_base_size":"0.000000001","minimum_quote_size":"1"});
    let a=seed::add_alpaca_crypto(&o.primary,&s,"AAA",&pair).0;
    let b=seed::add_alpaca_crypto(&o.primary,&s,"BBB",&pair).0;
    o.primary.execute("UPDATE bots SET settings=json_set(settings,'$.allocations',json(?1)) WHERE id=?2",rusqlite::params![json!({a.to_string():0.5,b.to_string():0.5}).to_string(),id]).unwrap();
    for (asset,side,qty,value) in [(a,0,"5","50"),(b,0,"5","50"),(a,1,"2","20")] {
        seed::insert_row(&o.primary,&s,id,asset,&json!({"side":side,"external_status":2,"amount":qty,"amount_exec":qty,"quote_amount_exec":value,"price":"10","created_at":"2026-01-05 11:59:59"}));
    }
    seed::insert_row(&o.primary,&s,id,s.btc,&json!({"external_status":2,"amount":null,"amount_exec":null,"quote_amount_exec":null,"quote_amount":"50","price":"10","created_at":"2026-01-05 12:00:00"}));
    assert_history_tick_refusal(&o.primary,id,"closed fill quantity unavailable","R1 NULL closed fill").await;
    let report=eligibility::check_install(&o.primary).unwrap();
    assert_eq!(report.unreadable.len(),1,"missing closed fill is unreadable");
    let tx=model::immediate(&o.primary).unwrap();
    assert!(matches!(eligibility::guard(&tx,&seed::cipher(),Some(id)),Err(eligibility::Refusal::Unreadable(_))));
    drop(tx);
    assert!(amount::pending_quote_amount(&o.primary,&model::load_bot(&o.primary,id).unwrap(),now().timestamp_micros()).is_err());

}
#[test]
fn r1_cap_float_bits_and_decimal_conversion_match_rails(){
    for (cap,spent,bits,decimal,reached) in [(json!(60.03),"60.02",0x3f847ae147ae1000,"0.00999999999999801",true),(json!(100),"99.99",0x3f847ae147ae2000,"0.01000000000000511",false)]{
        let (_d,o,_s,id)=setup(cap,spent);let bot=model::load_bot(&o.primary,id).unwrap();
        let Some(Num::Float(value))=amount::quote_amount_available_num(&o.primary,&bot).unwrap() else{panic!("Rails Float required")};
        assert_eq!(value.to_bits(),bits);
        assert_eq!(amount::quote_amount_available(&o.primary,&bot).unwrap(),Some(bd(decimal)));
        assert_eq!(amount::quote_amount_limit_reached(&o.primary,&bot).unwrap(),reached);
    }
}
#[test]
fn r1_guard_limits_persisted_intents_and_alpaca_wire(){
    use deltabadger::engine::venue_rules::{MinimumLogic,WireFormat};
    for (limit,requested,expected) in [(false,"0.01000000000000511","0.01"),(false,"0.0150000000001","0.01"),(false,"0.02","0.01"),(true,"0.02","0.001")]{
        let (_d,o,s,id)=setup(json!(100),"99.99");
        o.primary.execute("UPDATE tickers SET minimum_quote_size=0.01",[]).unwrap();
        if limit {o.primary.execute("UPDATE tickers SET base_decimals=3",[]).unwrap();o.primary.execute("UPDATE bots SET settings=json_set(settings,'$.limit_ordered',json('true'),'$.limit_order_pcnt_distance',0)",[]).unwrap();}
        let bot=model::load_bot(&o.primary,id).unwrap();
        let ticker=model::ticker_by_id(&o.primary,bot.exchange_id,s.ticker_id).unwrap().unwrap();
        let amount::Sizing::Place(plan)=amount::size(&bot,&ticker,&bd(requested),&bd("10"),MinimumLogic::Quote).unwrap() else {panic!("place")};
        let intent=placement::begin(&o.primary,&bot,&plan,&FixedClock(now())).unwrap();
        let order=intent.plan.to_order(intent.cl_ord_id,intent.deadline,WireFormat::Alpaca).unwrap();
        assert_eq!(bd(&order.volume),bd(expected),"quantized wire must respect exact availability");
        let cost=if order.quote_volume {bd(&order.volume)}else{&bd(&order.volume)*&bd("10")};
        assert!(cost<=bd("0.01"));
        // R2 preserves the entire safe Rails plan, including its unrounded commitment.
        if bd(requested).floor(2) <= bd("0.01") { assert_eq!(intent.plan.quote_amount,plan.quote_amount); }
        else { assert!(intent.plan.quote_amount<=bd("0.01")); }
    }
}

#[tokio::test(flavor="current_thread")]
async fn r1_merged_first_tick_warning_matches_rails_and_never_repeats(){
    use deltabadger::engine::tick::{self,TickContext,PriceCache};
    use common::scripted::{script,venue,ok};
    let vectors:Value=serde_json::from_str(include_str!("fixtures/histories_r1.json")).unwrap();
    for case in vectors["warnings"].as_array().unwrap(){
        let count=case["count"].as_i64().unwrap();
        let (_d,o,s)=common::install_alpaca();
        let pair=json!({"base_decimals":9,"quote_decimals":2,"price_decimals":2,"minimum_base_size":"0.000000001","minimum_quote_size":if count==2 {"1"}else{"2"}});
        let a=seed::add_alpaca_crypto(&o.primary,&s,"AAA",&pair).0;
        let b=seed::add_alpaca_crypto(&o.primary,&s,"BBB",&pair).0;
        let weights=if count==2 {vec![(a,0.5),(b,0.5)]}else{vec![(a,1.0)]};
        let id=seed::insert_bot(&o.primary,&s,&BotSpec::weekly(1.0,"2026-01-05 12:00:00").weights(&weights));
        for (asset,side,qty,value) in [(a,0,"7","70"),(b,0,"5","50"),(a,1,"2","20")] {
            seed::insert_row(&o.primary,&s,id,asset,&json!({"side":side,"external_status":2,"amount":qty,"amount_exec":qty,"quote_amount_exec":value,"price":"10","created_at":"2026-01-05 11:59:59"}));
        }
        o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.merged_history_until_id',(SELECT max(id) FROM transactions WHERE bot_id=?1)) WHERE id=?1",[id]).unwrap();
        let transport=script(json!({"GET /v1beta3/crypto/us/latest/quotes":[ok(json!({"quotes":{"AAA/USD":{"ap":10},"BBB/USD":{"ap":10}}}))]}));
        let notices=std::cell::RefCell::new(Vec::new());
        let notify=|bot,ids|notices.borrow_mut().push((bot,ids));
        let prices=PriceCache::default();
        let cx=TickContext{credential_version:model::credential_version(&o.primary,&model::load_bot(&o.primary,id).unwrap()).unwrap(),prices:&prices,process_start:now(),stopping:&||false,below_minimum:&notify};
        tick::tick_recovering(&o.primary,&venue(&transport),id,&FixedClock(now()),&mut Default::default(),&mut None,&cx).await.unwrap();
        assert!(transport.posted_orders().is_empty());
        assert_eq!(notices.borrow().len(),1,"merged first tick warning");
        let rows=notices.borrow()[0].1.clone();
        assert_eq!(rows.len() as i64,count);
        let activities:Vec<Value>=o.primary.prepare("SELECT event,level,details FROM bot_activity_logs WHERE event='order_skipped' ORDER BY id").unwrap().query_map([],|r|Ok(json!({"event":r.get::<_,String>(0)?,"level":if r.get::<_,i64>(1)?==1 {"warning"}else{"unexpected"},"details":serde_json::from_str::<Value>(&r.get::<_,String>(2)?).unwrap()}))).unwrap().map(Result::unwrap).collect();
        assert_eq!(json!(activities),case["activities"]);
        for (locale,html) in case["captures"][0]["payloads"].as_object().unwrap(){
            o.primary.execute("UPDATE users SET locale=?1 WHERE id=?2",rusqlite::params![locale,s.user_id]).unwrap();
            let (stream,payload)=deltabadger::web::below_minimum::render(&o.primary,id,&rows).unwrap();
            assert_eq!(stream,format!("user_{}:bot_updates",s.user_id));
            assert_eq!(payload,deltabadger::web::turbo::stream("replace","modal",html.as_str().unwrap()),"{locale} count={count}");
        }
        tick::tick_recovering(&o.primary,&venue(&transport),id,&FixedClock(now()),&mut Default::default(),&mut None,&cx).await.unwrap();
        assert_eq!(notices.borrow().len(),1,"second tick must not repeat");
        assert!(transport.posted_orders().is_empty());
    }
}

#[test]
fn r1_recorded_sources_and_all_cap_boundaries(){
    use sha2::{Digest,Sha256};
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let vectors:Value=serde_json::from_str(include_str!("fixtures/histories_r1.json")).unwrap();
    for (file,hash) in vectors["sources"].as_object().unwrap(){assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(file)).unwrap())),hash.as_str().unwrap(),"{file}");}
    for case in vectors["caps"].as_array().unwrap(){
        let (_d,o,_s,id)=setup(case["cap"].clone(),case["spent"].as_str().unwrap());
        let bot=model::load_bot(&o.primary,id).unwrap();
        let Some(Num::Float(value))=amount::quote_amount_available_num(&o.primary,&bot).unwrap()else{panic!("float")};
        assert_eq!(format!("{:016x}",value.to_bits()),case["float_bits"]);
        assert_eq!(amount::quote_amount_available(&o.primary,&bot).unwrap().unwrap().to_s_f(),case["decimal"]);
        assert_eq!(amount::quote_amount_limit_reached(&o.primary,&bot).unwrap(),case["reached"]);
        assert_eq!(amount::exact_cap_available(&o.primary,&bot).unwrap().unwrap().to_s_f(),case["exact"]);
    }
}

#[tokio::test(flavor="current_thread")]
async fn r1_real_ticks_floor_at_the_wire_and_skip_below_minimum(){
    use common::scripted::{script,venue};
    use deltabadger::engine::tick;
    for (cap,spent,want) in [(json!(60.03),"60.02",None),(json!(100),"99.99",Some("0.01")),(json!(100),"99.985",Some("0.01"))]{
        let (_d,o,_s,id)=setup(cap,spent);
        o.primary.execute("UPDATE tickers SET minimum_quote_size=0.01",[]).unwrap();
        let bot=model::load_bot(&o.primary,id).unwrap();
        let exact=amount::exact_cap_available(&o.primary,&bot).unwrap().unwrap();
        let t=script(json!({}));
        tick::tick(&o.primary,&venue(&t),id,&FixedClock(now()+chrono::Duration::weeks(1)),&mut Default::default()).await.unwrap();
        let orders=t.posted_orders();
        assert_eq!(orders.len(),usize::from(want.is_some()));
        if let Some(want)=want {assert_eq!(orders[0]["notional"],want);assert!(bd(orders[0]["notional"].as_str().unwrap())<=exact);}
    }
}

#[test]
fn r1_guard_skips_if_reducing_one_increment_goes_below_minimum(){
    use deltabadger::engine::venue_rules::MinimumLogic;
    let (_d,o,s,id)=setup(json!(100),"99.991");
    o.primary.execute("UPDATE tickers SET minimum_quote_size=0.01",[]).unwrap();
    let bot=model::load_bot(&o.primary,id).unwrap();
    let ticker=model::ticker_by_id(&o.primary,bot.exchange_id,s.ticker_id).unwrap().unwrap();
    let amount::Sizing::Place(plan)=amount::size(&bot,&ticker,&bd("0.0100000000001"),&bd("10"),MinimumLogic::Quote).unwrap()else{panic!("place")};
    assert!(placement::begin(&o.primary,&bot,&plan,&FixedClock(now())).is_err());
    assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
}

fn r2_vectors()->Value {
    let path=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/histories_r2.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn r2_setup(case:&Value)->(tempfile::TempDir,deltabadger::store::Opened,common::seed::Seeded,i64){
    let (d,o,s)=common::install_alpaca();
    let pair=json!({"base_decimals":9,"quote_decimals":2,"price_decimals":case["price_decimals"],"minimum_base_size":"0.000000001","minimum_quote_size":case["minimum"]});
    let a=seed::add_alpaca_crypto(&o.primary,&s,"AAA",&pair).0;
    let id=seed::insert_bot(&o.primary,&s,&BotSpec::weekly(case["pending"].as_str().unwrap().parse().unwrap(),"2026-01-05 12:00:00").weights(&[(a,1.0)])
        .with("quote_amount_limited",json!(true)).with("quote_amount_limit",serde_json::from_str(case["cap"].as_str().unwrap()).unwrap())
        .with("limit_ordered",json!(true)).with("limit_order_pcnt_distance",json!(0))
        .transient("quote_amount_limit_enabled_at",json!("2026-01-05T12:00:00Z")));
    for (side,qty,value) in [(0,"5","50"),(1,"2","20")] {
        seed::insert_row(&o.primary,&s,id,a,&json!({"side":side,"external_status":2,"amount":qty,"amount_exec":qty,"quote_amount_exec":value,"price":"10","created_at":"2026-01-05 11:59:59"}));
    }
    o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.merged_history_until_id',(SELECT max(id) FROM transactions WHERE bot_id=?1)) WHERE id=?1",[id]).unwrap();
    (d,o,s,id)
}
async fn r2_tick_case(name:&str){
    use common::scripted::{script,venue,ok};
    for case in r2_vectors()["cases"].as_array().unwrap().iter().filter(|c|c["name"]==name) {
        let (_d,o,_s,id)=r2_setup(case);
        let bot=model::load_bot(&o.primary,id).unwrap();
        assert!(eligibility::check_install(&o.primary).unwrap().eligible.contains(&id));
        assert_eq!(amount::pending_quote_amount(&o.primary,&bot,now().timestamp_micros()).unwrap().to_s_f(),case["rails_pending"]);
        let t=script(json!({"GET /v1beta3/crypto/us/latest/trades":[ok(json!({"trades":{"AAA/USD":{"p":case["price"]}}}))]}));
        deltabadger::engine::tick::tick(&o.primary,&venue(&t),id,&FixedClock(now()),&mut Default::default()).await.unwrap();
        let orders=t.posted_orders();
        assert_eq!(orders.len(),1,"R2 unchanged minimum must send: {}",case["name"]);
        let wire=&orders[0];
        let cost=bd(wire["qty"].as_str().unwrap()).checked_mul(&bd(wire["limit_price"].as_str().unwrap())).unwrap();
        assert!(cost<=bd(case["exact"].as_str().unwrap()),"R2 exact cost exceeds cap: {cost:?}");
        assert_eq!(bd(wire["qty"].as_str().unwrap()),bd(case["safe_quantity"].as_str().unwrap()));
        assert_eq!(cost.to_s_f(),case["exact_cost"]);
        if case["steps"]==0 {let mut actual=wire.clone();actual.as_object_mut().unwrap().remove("client_order_id");assert_eq!(actual,case["rails_wire"]);}
        assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
    }
}
#[tokio::test(flavor="current_thread")]
async fn r2_reduced_below_minimum_records_rails_row_activity_and_notice(){
    use common::scripted::{script,venue,ok};
    use deltabadger::engine::tick::{self,TickContext,PriceCache};
    let vectors=r2_vectors(); let case=&vectors["cases"][2];
    let (_d,o,s,id)=r2_setup(case);
    let t=script(json!({"GET /v1beta3/crypto/us/latest/trades":[ok(json!({"trades":{"AAA/USD":{"p":case["price"]}}}))]}));
    let notices=std::cell::RefCell::new(Vec::new());let notify=|bot,ids|notices.borrow_mut().push((bot,ids));
    let prices=PriceCache::default();let cx=TickContext{credential_version:model::credential_version(&o.primary,&model::load_bot(&o.primary,id).unwrap()).unwrap(),prices:&prices,process_start:now(),stopping:&||false,below_minimum:&notify};
    tick::tick_recovering(&o.primary,&venue(&t),id,&FixedClock(now()),&mut Default::default(),&mut None,&cx).await.unwrap();
    assert!(t.posted_orders().is_empty(),"R2 reduced below minimum must skip");
    assert_eq!(notices.borrow().len(),1,"R2 skipped notice");
    let rows=notices.borrow()[0].1.clone();assert_eq!(rows.len(),1);
    let activities:Vec<Value>=o.primary.prepare("SELECT event,level,details FROM bot_activity_logs WHERE event='order_skipped' ORDER BY id").unwrap().query_map([],|r|Ok(json!({"event":r.get::<_,String>(0)?,"level":if r.get::<_,i64>(1)?==1 {"warning"}else{"unexpected"},"details":serde_json::from_str::<Value>(&r.get::<_,String>(2)?).unwrap()}))).unwrap().map(Result::unwrap).collect();
    assert_eq!(json!(activities),case["activities"]);
    let row=&case["skipped"][0];
    let saved:Value=o.primary.query_row("SELECT side,status,external_status,order_type,base,quote,amount,quote_amount,price,amount_exec,quote_amount_exec FROM transactions WHERE id=?1",[rows[0]],|r|Ok(json!({
        "side":if r.get::<_,i64>(0)?==0{"buy"}else{"sell"},"status":if r.get::<_,i64>(1)?==2{"skipped"}else{"unexpected"},"external_status":r.get::<_,Option<i64>>(2)?,"order_type":if r.get::<_,i64>(3)?==1{"limit_order"}else{"market_order"},"base":r.get::<_,String>(4)?,"quote":r.get::<_,String>(5)?,
        "amount":r.get::<_,f64>(6)?.to_string(),"quote_amount":r.get::<_,f64>(7)?.to_string(),"price":r.get::<_,f64>(8)?.to_string(),"amount_exec":r.get::<_,f64>(9)?.to_string(),"quote_amount_exec":r.get::<_,f64>(10)?.to_string()}))).unwrap();
    for key in ["side","status","external_status","order_type","base","quote"] {assert_eq!(saved[key],row[key],"{key}");}
    for key in ["amount","quote_amount","price","amount_exec","quote_amount_exec"] {assert_eq!(bd(saved[key].as_str().unwrap()),bd(row[key].as_str().unwrap()),"{key}");}
    for (locale,html) in case["captures"][0]["payloads"].as_object().unwrap(){
        o.primary.execute("UPDATE users SET locale=?1 WHERE id=?2",rusqlite::params![locale,s.user_id]).unwrap();
        let (stream,payload)=deltabadger::web::below_minimum::render(&o.primary,id,&rows).unwrap();
        assert_eq!(stream,format!("user_{}:bot_updates",s.user_id));
        assert_eq!(payload,deltabadger::web::turbo::stream("replace","modal",html.as_str().unwrap()),"{locale}");
    }
    tick::tick_recovering(&o.primary,&venue(&t),id,&FixedClock(now()),&mut Default::default(),&mut None,&cx).await.unwrap();
    assert_eq!(notices.borrow().len(),1);assert!(t.posted_orders().is_empty());
    assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
}
#[test]
fn r2_guard_preserves_safe_plan_and_persists_reduced_quantity(){
    use deltabadger::engine::venue_rules::{MinimumLogic,WireFormat};
    let vectors=r2_vectors();
    for case in vectors["cases"].as_array().unwrap().iter().take(2) {
        let (_d,o,_s,id)=r2_setup(case);let bot=model::load_bot(&o.primary,id).unwrap();
        let ticker=model::ticker_for(&o.primary,&bot).unwrap().unwrap();
        let amount::Sizing::Place(plan)=amount::size(&bot,&ticker,&bd(case["pending"].as_str().unwrap()),&bd(case["price"].as_str().unwrap()),MinimumLogic::Quote).unwrap()else{panic!("place")};
        let intent=placement::begin(&o.primary,&bot,&plan,&FixedClock(now())).unwrap();
        if case["steps"]==0 {
            assert_eq!(intent.plan.log_details(),plan.log_details());assert_eq!(intent.plan.volume,plan.volume);assert_eq!(intent.plan.quote_type,plan.quote_type);
        }
        let wire=intent.plan.to_order(intent.cl_ord_id.clone(),intent.deadline,WireFormat::Alpaca).unwrap();
        let stored=model::load_bot(&o.primary,id).unwrap().rust_placement().unwrap().clone();
        assert_eq!(bd(stored["volume"].as_str().unwrap()),intent.plan.volume);
        assert_eq!(stored["quote_type"],intent.plan.quote_type);
        assert_eq!(bd(&wire.volume),bd(case["safe_quantity"].as_str().unwrap()));
        assert!(bd(&wire.volume).checked_mul(&intent.plan.price).unwrap()<=bd(case["exact"].as_str().unwrap()),"R2 exact persisted cost");
    }
}
#[test]
fn r2_recorded_sources_match(){
    use sha2::{Digest,Sha256};
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (file,hash) in r2_vectors()["sources"].as_object().unwrap(){assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(file)).unwrap())),hash.as_str().unwrap(),"{file}");}
}

#[tokio::test(flavor="current_thread")]
async fn r2_exact_cost_matches_recorded_wire(){r2_tick_case("exact_cost").await;}
#[tokio::test(flavor="current_thread")]
async fn r2_unchanged_minimum_matches_rails_wire(){r2_tick_case("minimum_unchanged").await;}
#[test]
fn r2_guard_refuses_when_four_increments_cannot_converge(){
    use deltabadger::engine::venue_rules::MinimumLogic;
    let (_d,o,s,id)=setup(json!(100),"99");
    let bot=model::load_bot(&o.primary,id).unwrap();
    let ticker=model::ticker_by_id(&o.primary,bot.exchange_id,s.ticker_id).unwrap().unwrap();
    let amount::Sizing::Place(plan)=amount::size(&bot,&ticker,&bd("2"),&bd("10"),MinimumLogic::Quote).unwrap()else{panic!("place")};
    let result=placement::begin(&o.primary,&bot,&plan,&FixedClock(now()));
    assert!(matches!(result,Err(deltabadger::engine::EngineError::Data(ref m)) if m.contains("did not converge within 4 venue increments")),"R2 bounded convergence refusal");
    assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
}


fn r3_history() -> (tempfile::TempDir,deltabadger::store::Opened,common::seed::Seeded,i64) {
    let (d,o,s)=common::install_alpaca();
    let pair=json!({"base_decimals":9,"quote_decimals":2,"price_decimals":2,"minimum_base_size":"0.000000001","minimum_quote_size":"1"});
    let a=seed::add_alpaca_crypto(&o.primary,&s,"AAA",&pair).0;
    let b=seed::add_alpaca_crypto(&o.primary,&s,"BBB",&pair).0;
    let id=seed::insert_bot(&o.primary,&s,&BotSpec::weekly(100.0,"2026-01-05 12:00:00").weights(&[(a,0.5),(b,0.5)]));
    for (asset,side,qty,value) in [(a,0,"5","50"),(b,0,"5","50"),(a,1,"2","20")] {
        seed::insert_row(&o.primary,&s,id,asset,&json!({"side":side,"external_status":2,"amount":qty,"amount_exec":qty,"quote_amount_exec":value,"price":"10","created_at":"2026-01-05 11:59:59"}));
    }
    o.primary.execute("UPDATE bots SET settings_changed_at=started_at WHERE id=?1",[id]).unwrap();
    assert_eq!(o.primary.query_row("SELECT max(id) FROM transactions",[],|r|r.get::<_,i64>(0)).unwrap(),3);
    (d,o,s,id)
}
async fn r3_refuses(c:&rusqlite::Connection,id:i64,label:&str,reason:&str) {
    assert_history_tick_refusal(c,id,reason,label).await;
    let report=eligibility::check_install(c).unwrap();
    assert_eq!(report.unreadable.len(),1,"R3 unreadable at check: {label}");
    assert!(report.unreadable[0].1.contains(reason),"same check reason: {label}");
    assert!(matches!(report.refusal(),Err(eligibility::Refusal::Unreadable(_))),"{label}");
    let before=model::load_bot(c,id).unwrap().transient;
    let tx=model::immediate(c).unwrap();
    tx.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.r3_write','must roll back') WHERE id=?1",[id]).unwrap();
    let guard=eligibility::guard(&tx,&seed::cipher(),Some(id));
    assert!(matches!(&guard,Err(eligibility::Refusal::Unreadable(_))),"{label}");
    assert!(format!("{guard:?}").contains(reason),"same guard reason: {label}");
    drop(tx);
    assert_eq!(model::load_bot(c,id).unwrap().transient,before,"guarded write rollback: {label}");
    assert_history_tick_refusal(c,id,reason,label).await;

}
#[tokio::test(flavor="current_thread")]
async fn r3_negative_fill_is_never_silently_discarded() {
    for field in ["amount","amount_exec","quote_amount_exec","price"] {
        let (_d,o,s,id)=r3_history();
        let mut row=json!({"external_status":2,"amount":"1","amount_exec":"1","quote_amount_exec":"10","price":"10","created_at":"2026-01-05 12:00:00"});
        row[field]=json!("-1");
        if field=="amount_exec" {row["quote_amount_exec"]=json!("0");}
        seed::insert_row(&o.primary,&s,id,s.btc,&row);
        r3_refuses(&o.primary,id,field,"negative fill field").await;
    }
}
#[tokio::test(flavor="current_thread")]
async fn r3_invalid_fill_columns_refuse_even_when_fallback_is_valid() {
    for field in ["amount","amount_exec","quote_amount_exec","price"] {
        for value in ["NaN","Infinity","-Infinity","unparsable"] {
            let (_d,o,s,id)=r3_history();
            let tx=seed::insert_row(&o.primary,&s,id,s.btc,&json!({"external_status":2,"amount":"1","amount_exec":"1","quote_amount_exec":"10","price":"10","created_at":"2026-01-05 12:00:00"}));
            o.primary.execute(&format!("UPDATE transactions SET {field}=?1 WHERE id=?2"),rusqlite::params![value,tx]).unwrap();
            r3_refuses(&o.primary,id,&format!("{field}={value}"),deltabadger::figures::NOT_A_NUMBER).await;
        }
    }
}
#[tokio::test(flavor="current_thread")]
async fn r3_malformed_merge_cutoffs_refuse_at_check_guard_and_tick() {
    for value in [json!("3a"),json!(-1),json!(3.5),Value::Null,json!(""),json!(" 3"),json!("+3"),json!("-1"),json!("٣"),json!(true),json!([]),json!({})] {
        let (_d,o,_s,id)=r3_history();
        o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.merged_history_until_id',json(?1)) WHERE id=?2",rusqlite::params![value.to_string(),id]).unwrap();
        r3_refuses(&o.primary,id,&value.to_string(),"unreadable merged history cutoff").await;
    }
}
#[tokio::test(flavor="current_thread")]
async fn r3_integer_and_ascii_string_cutoffs_match_rails_orders() {
    use common::scripted::venue;
    let vectors:Value=serde_json::from_str(include_str!("fixtures/histories_r3.json")).unwrap();
    for case in vectors["cases"].as_array().unwrap() {
        let (_d,o,_s,id)=r3_history();
        o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.merged_history_until_id',json(?1)) WHERE id=?2",rusqlite::params![case["cutoff"].to_string(),id]).unwrap();
        let bot=model::load_bot(&o.primary,id).unwrap();
        let cutoff=model::merged_history_cutoff(&o.primary,&bot).unwrap();
        let own:Vec<i64>=o.primary.prepare("SELECT id FROM transactions WHERE bot_id=?1 AND (?2 IS NULL OR id>?2) ORDER BY id").unwrap().query_map(rusqlite::params![id,cutoff],|r|r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(json!(own),case["own_ids"],"Rails SQL boundary: {case}");
        let report=eligibility::check_install(&o.primary).unwrap();
        assert_eq!(report.eligible,vec![id],"{case}");
        assert!(report.unreadable.is_empty());
        let tx=model::immediate(&o.primary).unwrap();
        eligibility::guard(&tx,&seed::cipher(),Some(id)).unwrap();
        tx.commit().unwrap();
        let t=history_market();
        let result=deltabadger::engine::tick::tick(&o.primary,&venue(&t),id,&FixedClock(now()),&mut Default::default()).await;
        assert!(result.is_ok(),"R3 valid cutoff tick: {case}: {result:?}");
        let orders=t.posted_orders();
        assert_eq!(orders.len(),case["orders"].as_array().unwrap().len(),"{case}: {result:?}");
        for (actual,expected) in orders.iter().zip(case["orders"].as_array().unwrap()) {
            assert_eq!(actual["side"],expected["side"]);
            assert_eq!(actual["symbol"],format!("{}/USD",expected["asset"].as_str().unwrap()));
            assert_eq!(bd(actual["notional"].as_str().unwrap()),bd(expected["quote"].as_str().unwrap()));
        }
    }
}

#[test]
fn r3_absent_cutoff_and_empty_null_are_ordinary_valid_shapes() {
    let (_d,o,_s,id)=r3_history();
    assert_eq!(model::merged_history_cutoff(&o.primary,&model::load_bot(&o.primary,id).unwrap()).unwrap(),None);
    o.primary.execute("DELETE FROM transactions WHERE bot_id=?1",[id]).unwrap();
    o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.merged_history_until_id',NULL) WHERE id=?1",[id]).unwrap();
    assert_eq!(model::merged_history_cutoff(&o.primary,&model::load_bot(&o.primary,id).unwrap()).unwrap(),None);
    assert_eq!(eligibility::check_install(&o.primary).unwrap().eligible,vec![id]);
}

#[tokio::test(flavor="current_thread")]
async fn r4_valid_history_control_places_exact_oracle_sixty_forty(){
    let (_d,o,_s,id)=r3_history();
    let vectors:Value=serde_json::from_str(include_str!("fixtures/histories_r3.json")).unwrap();
    let t=history_market();
    let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()),&mut Default::default()).await.unwrap();
    assert!(matches!(result,deltabadger::engine::tick::TickOutcome::Done{placed:true}));
    let decisions:Vec<Value>=t.posted_orders().iter().map(|order|json!({"side":order["side"],"asset":order["symbol"].as_str().unwrap().strip_suffix("/USD").unwrap(),"quote":bd(order["notional"].as_str().unwrap()).to_s_f()})).collect();
    let expected:Vec<Value>=vectors["cases"][0]["orders"].as_array().unwrap().iter().map(|order|json!({"side":order["side"],"asset":order["asset"],"quote":order["quote"]})).collect();
    assert_eq!(serde_json::to_vec(&decisions).unwrap(),serde_json::to_vec(&expected).unwrap(),"byte-equal Rails $60/$40 decisions");
    assert_eq!(decisions[0]["quote"],"60.0");assert_eq!(decisions[1]["quote"],"40.0");
}

// R4: every refusal probe and its trading control use this funded, priced market.
fn history_market() -> deltabadger::venue::http::ScriptedTransport {
    use common::scripted::{script,ok};
    script(json!({"GET /v1beta3/crypto/us/latest/quotes":[ok(json!({"quotes":{"AAA/USD":{"ap":10},"BBB/USD":{"ap":10}}}))]}))
}
async fn assert_history_tick_refusal(c:&rusqlite::Connection,id:i64,reason:&str,label:&str) {
    let previous:i64=c.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id=?1 AND event='execution_failed'",[id],|r|r.get(0)).unwrap();
    let t=history_market();
    let result=deltabadger::engine::tick::tick(c,&common::scripted::venue(&t),id,&FixedClock(now()),&mut Default::default()).await;
    assert!(matches!(result,Ok(deltabadger::engine::tick::TickOutcome::Rescheduled)),"specific unreadable tick outcome: {label}: {result:?}");
    let errors:Vec<String>=c.prepare("SELECT json_extract(details,'$.error') FROM bot_activity_logs WHERE bot_id=?1 AND event='execution_failed' ORDER BY id").unwrap()
        .query_map([id],|r|r.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(errors.len(),previous as usize+1,"specific unreadable tick reason: {label}: {errors:?}");
    assert!(errors[0].contains(reason),"specific unreadable tick reason: {label}: expected {reason}, got {errors:?}");
    assert!(t.posted_orders().is_empty(),"unreadable tick placed an order: {label}");
    assert!(model::load_bot(c,id).unwrap().rust_placement().is_none(),"unreadable tick left an intent: {label}");
}

// The pre-R1 event subscription API makes this a compile-valid runtime red.
// Match the event's Debug envelope because its typed variant arrives with the green implementation.
struct HistoryFactory(deltabadger::venue::http::ScriptedTransport);
impl deltabadger::venue::VenueFactory for HistoryFactory {
    type V=deltabadger::venue::alpaca::AlpacaVenue<deltabadger::venue::http::ScriptedTransport>;
    fn for_bot(&self,_t:&str,_c:Option<deltabadger::crypto::Credentials>)->Self::V{common::scripted::venue(&self.0)}
}
#[tokio::test(flavor="current_thread")]
async fn r1_merged_first_tick_warning_is_emitted_before_implementation(){
    use deltabadger::engine::run::{self,Engine};
    let vectors:Value=serde_json::from_str(include_str!("fixtures/histories_r1.json")).unwrap();
    let case=vectors["warnings"].as_array().unwrap().iter().find(|c|c["count"]==2).unwrap();
    let (d,o,s)=common::install_alpaca();
    let pair=json!({"base_decimals":9,"quote_decimals":2,"price_decimals":2,"minimum_base_size":"0.000000001","minimum_quote_size":"1"});
    let a=seed::add_alpaca_crypto(&o.primary,&s,"AAA",&pair).0;
    let b=seed::add_alpaca_crypto(&o.primary,&s,"BBB",&pair).0;
    let id=seed::insert_bot(&o.primary,&s,&BotSpec::weekly(1.0,"2026-01-05 12:00:00").weights(&[(a,0.5),(b,0.5)]));
    for (asset,side,qty,value) in [(a,0,"7","70"),(b,0,"5","50"),(a,1,"2","20")] {
        seed::insert_row(&o.primary,&s,id,asset,&json!({"side":side,"external_status":2,"amount":qty,"amount_exec":qty,"quote_amount_exec":value,"price":"10","created_at":"2026-01-05 11:59:59"}));
    }
    o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.merged_history_until_id',3),settings_changed_at=started_at WHERE id=?1",[id]).unwrap();
    let paths=deltabadger::store::Paths::from_env(&|_|None,d.path());
    let lock=deltabadger::lease::lock(&paths,now()).unwrap();
    let transport=history_market();
    let mut engine=Engine::new(o.primary,HistoryFactory(transport.clone()),seed::cipher(),lock);
    let mut events=engine.subscribe_to(|_|true);
    run::step(&mut engine,&FixedClock(now())).await.unwrap();
    let rows:Vec<i64>=engine.primary.prepare("SELECT id FROM transactions WHERE bot_id=?1 AND status=2 AND id>3 ORDER BY id").unwrap()
        .query_map([id],|r|r.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(rows.len(),2,"both half-dollar legs must skip");
    assert!(transport.posted_orders().is_empty());
    let warnings:Vec<String>=std::iter::from_fn(||events.try_recv().ok()).map(|e|format!("{e:?}")).filter(|e|e.starts_with("BelowMinimum ")).collect();
    assert_eq!(warnings,vec![format!("BelowMinimum {{ bot_id: {id}, transaction_ids: {rows:?} }}")],"merged first tick warning");
    let activities:Vec<Value>=engine.primary.prepare("SELECT event,level,details FROM bot_activity_logs WHERE event='order_skipped' ORDER BY id").unwrap().query_map([],|r|Ok(json!({"event":r.get::<_,String>(0)?,"level":if r.get::<_,i64>(1)?==1 {"warning"}else{"unexpected"},"details":serde_json::from_str::<Value>(&r.get::<_,String>(2)?).unwrap()}))).unwrap().map(Result::unwrap).collect();
    assert_eq!(json!(activities),case["activities"]);
}


fn r5_history() -> (tempfile::TempDir,deltabadger::store::Opened,seed::Seeded,i64) {
    let (d,o,s,id)=r3_history();
    o.primary.execute("UPDATE transactions SET external_status=3,quote_amount_exec=NULL,created_at='2026-01-05 12:00:00' WHERE id=1",[]).unwrap();
    o.primary.execute("UPDATE transactions SET created_at='2026-01-05 12:00:00.500000' WHERE id=3",[]).unwrap();
    o.primary.execute("UPDATE users SET confirmed_at=created_at,wash_sale_enabled=0",[]).unwrap();
    o.primary.execute("UPDATE bots SET label='R5',settings=json_set(settings,'$.weighting','manual')",[]).unwrap();
    (d,o,s,id)
}
fn r5_bot(c:&rusqlite::Connection,s:&seed::Seeded,id:i64)->deltabadger::web::bot::Bot {
    deltabadger::web::bot::Bot::find(c,s.user_id,id,deltabadger::web::bot::For::Page, "en").unwrap().unwrap()
}
async fn r5_settings_tick(mcp:bool) {
    use deltabadger::web;
    use axum::{body::{Body,to_bytes},http::Request};
    use tower::ServiceExt;
    let (d,o,s,id)=r5_history();
    // Settings actions require a stopped bot. Reactivation below changes only its schedule status.
    o.primary.execute("UPDATE bots SET status=2 WHERE id=?1",[id]).unwrap();
    let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
    if mcp {
        o.primary.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('R5','r5','http://localhost/cb',0,'mcp',?1,?1)",["2026-01-05 12:00:01"]).unwrap();
        let application=o.primary.last_insert_rowid();
        o.primary.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,'r5-token','mcp',?3,3600)",(application,s.user_id,"2026-01-05 12:00:01")).unwrap();
        o.primary.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)",(s.user_id,application,json!(["update_bot_settings"]).to_string(),"2026-01-05 12:00:01")).unwrap();
        o.primary.execute("UPDATE users SET mcp_settings=?1",[json!({"tool_permissions":{"update_bot_settings":true},"dry_run":true}).to_string()]).unwrap();
        let req=|body:Value,sid:Option<&str>|{
            let mut r=Request::builder().method("POST").uri("/mcp").header("host","localhost:3000").header("authorization","Bearer r5-token").header("content-type","application/json").header("accept","application/json, text/event-stream");
            if let Some(sid)=sid {r=r.header("mcp-session-id",sid);}
            r.body(Body::from(body.to_string())).unwrap()
        };
        let response=web::router(app.clone()).oneshot(req(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"R5","version":"1"}}}),None)).await.unwrap();
        let sid=response.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
        let response=web::router(app.clone()).oneshot(req(json!({"jsonrpc":"2.0","method":"notifications/initialized"}),Some(&sid))).await.unwrap();
        assert_eq!(response.status(),202);
        let response=web::router(app.clone()).oneshot(req(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"update_bot_settings","arguments":{"bot_id":id,"quote_amount":200}}}),Some(&sid))).await.unwrap();
        assert_eq!(response.status(),200);
        let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),100_000).await.unwrap()).unwrap();
        assert_ne!(body["result"]["isError"],true,"{body}");
        assert!(body["result"]["content"][0]["text"].as_str().unwrap().contains("settings updated"),"{body}");
    } else {
        let token=web::csrf::new_token();
        let session=web::session::SessionData{user:Some((s.user_id,"x".into())),csrf:Some(token.clone()),..Default::default()};
        let cookie=web::session::seal(&app.keys.session,&session,app.now());
        let request=Request::builder().method("PATCH").uri(format!("/bots/{id}")).header("host","localhost:3000")
            .header("cookie",format!("{}={cookie}",web::session::COOKIE)).header("x-csrf-token",web::csrf::masked(&token))
            .header("content-type","application/json").header("accept","text/vnd.turbo-stream.html")
            .body(Body::from(json!({"bots_dca_multi_asset":{"quote_amount":"200"}}).to_string())).unwrap();
        let response=web::router(app).oneshot(request).await.unwrap();
        let status=response.status();let body=to_bytes(response.into_body(),1_000_000).await.unwrap();
        assert_eq!(status,200,"{}",String::from_utf8_lossy(&body));
    }
    let saved=model::load_bot(&o.primary,id).unwrap();
    o.primary.execute("UPDATE bots SET status=1 WHERE id=?1",[id]).unwrap();
    assert_eq!(saved.settings["quote_amount"],json!(200.0));
    let t=history_market();
    let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()+chrono::Duration::seconds(1)),&mut Default::default()).await.unwrap();
    assert!(matches!(result,deltabadger::engine::tick::TickOutcome::Done{placed:true}),"{result:?}");
    let orders=t.posted_orders();
    let actual:Vec<_>=orders.iter().map(|o|json!({"side":o["side"],"asset":o["symbol"].as_str().unwrap().strip_suffix("/USD").unwrap(),"quote":bd(o["notional"].as_str().unwrap()).to_s_f()})).collect();
    let oracle:Value=serde_json::from_str(include_str!("fixtures/histories_r5.json")).unwrap();
    assert_eq!(serde_json::to_vec(&actual).unwrap(),serde_json::to_vec(&oracle["normalized"]["orders"]).unwrap(),"R5 normalized settings action then tick, mcp={mcp}");
    assert_eq!(saved.transient["missed_quote_amount"],oracle["normalized"]["carry"]);
}
#[tokio::test(flavor="current_thread")]
async fn r5_web_settings_then_tick_credits_normalized_fill(){r5_settings_tick(false).await;}
#[tokio::test(flavor="current_thread")]
async fn r5_mcp_settings_then_tick_credits_normalized_fill(){r5_settings_tick(true).await;}
#[test]
fn r5_start_cap_and_pending_share_normalized_credit() {
    let (_d,o,s,id)=r5_history();
    o.primary.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount_limited',json('true'),'$.quote_amount_limit',60),transient_data=json_set(transient_data,'$.quote_amount_limit_enabled_at','2026-01-05T12:00:00Z') WHERE id=?1",[id]).unwrap();
    let bot=r5_bot(&o.primary,&s,id);
    let limit=deltabadger::web::bot::start::amount_limit(&o.primary,&bot).unwrap().unwrap();
    assert_eq!(limit.left.as_ref().unwrap().to_d().unwrap(),bd("10"),"R5 normalized web start cap");
    assert_eq!(deltabadger::web::bot::write::pending(&o.primary,&bot,now()).unwrap().to_d().unwrap(),bd("10"));
    o.primary.execute("UPDATE transactions SET amount_exec=-1 WHERE id=1",[]).unwrap();
    assert!(deltabadger::web::bot::start::amount_limit(&o.primary,&bot).is_err());
    assert!(deltabadger::web::bot::write::pending(&o.primary,&bot,now()).is_err());
}
#[test]
fn r5_draft_sell_cap_rejects_invalid_fill() {
    use deltabadger::web::bot::draft::{Draft,ValidationContext};
    let (_d,o,s,id)=r5_history();
    o.primary.execute("UPDATE bots SET settings=json_set(settings,'$.allocations',json(?1),'$.direction','selling','$.base_amount_limited',json('true'),'$.base_amount_limit',10),transient_data=json_set(transient_data,'$.base_amount_limit_enabled_at','2026-01-01T00:00:00Z') WHERE id=?2",rusqlite::params![json!({s.btc.to_string():1.0}).to_string(),id]).unwrap();
    for (requested,executed) in [(Some("2"),Some("-1")),(Some("2"),Some("NaN")),(None,None)] {
        o.primary.execute("UPDATE transactions SET amount=?1,amount_exec=?2 WHERE side=1",rusqlite::params![requested,executed]).unwrap();
        let mut draft=Draft::from_bot(r5_bot(&o.primary,&s,id));
        assert!(draft.validate(&o.primary,ValidationContext::Start,now(),false,"en").is_err(),"R5 draft sell cap must reject negative execution or missing quantity");
    }
}

#[test]
fn r5_failure_compaction_owns_only_supplied_keys() {
    let (_d,o,_s,id)=r3_history();
    let original=json!({"merged_history_until_id":null,"liquidation_pending":null,"redeploy_pending":null,"rebalance_pending":null,"private":null,"nested":{"null":null},"last_failure_kind":"old"});
    o.primary.execute("UPDATE bots SET transient_data=?1 WHERE id=?2",rusqlite::params![original.to_string(),id]).unwrap();
    model::merge_transient_compact(&o.primary,id,&[("last_failure_kind",Value::Null)]).unwrap();
    let mut expected=original;expected.as_object_mut().unwrap().shift_remove("last_failure_kind");
    assert_eq!(model::load_bot(&o.primary,id).unwrap().transient,expected,"R5 cleanup only owns supplied keys");
}
#[test]
fn r5_oracle_sources_and_normalized_decisions_are_pinned() {
    use sha2::{Digest,Sha256};
    let v:Value=serde_json::from_str(include_str!("fixtures/histories_r5.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (f,h) in v["sources"].as_object().unwrap(){assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(f)).unwrap())),h.as_str().unwrap());}
    assert_eq!(v["normalized"]["carry"],"50.0");assert_eq!(v["rails_unchanged"]["carry"],"100.0");
}


fn r6_history() -> (tempfile::TempDir,deltabadger::store::Opened,seed::Seeded,i64) {
    let (d,o,s,id)=r5_history();
    let asset:i64=o.primary.query_row("SELECT base_asset_id FROM transactions WHERE id=1",[],|r|r.get(0)).unwrap();
    o.primary.execute("DELETE FROM transactions",[]).unwrap();
    o.primary.execute("UPDATE bots SET settings=json_set(settings,'$.allocations',json(?1),'$.quote_amount_limited',json('true'),'$.quote_amount_limit',16.06),transient_data=json_set(transient_data,'$.quote_amount_limit_enabled_at','2026-01-05T12:00:00Z') WHERE id=?2",rusqlite::params![json!({asset.to_string():1.0}).to_string(),id]).unwrap();
    for _ in 0..6 { seed::insert_row(&o.primary,&s,id,asset,&json!({"external_status":3,"amount":"0.101","amount_exec":"0.101","quote_amount_exec":"1.01","price":"10","created_at":"2026-01-05 12:00:00"})); }
    seed::insert_row(&o.primary,&s,id,asset,&json!({"side":1,"external_status":2,"amount":"0.1","amount_exec":"0.1","quote_amount_exec":"1","price":"10","created_at":"2026-01-05 12:00:00.500000"}));
    assert_eq!(o.primary.query_row("SELECT count(*) FROM transactions WHERE side=0 AND typeof(quote_amount_exec)='real'",[],|r|r.get::<_,i64>(0)).unwrap(),6);
    (d,o,s,id)
}
async fn r6_settings_tick(mcp:bool) {
    use deltabadger::web;
    use axum::{body::{Body,to_bytes},http::Request};
    use tower::ServiceExt;
    let (d,o,s,id)=r6_history();
    // Settings actions require a stopped bot. Reactivation below changes only its schedule status.
    o.primary.execute("UPDATE bots SET status=2 WHERE id=?1",[id]).unwrap();
    let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
    if mcp {
        o.primary.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('R5','r5','http://localhost/cb',0,'mcp',?1,?1)",["2026-01-05 12:00:01"]).unwrap();
        let application=o.primary.last_insert_rowid();
        o.primary.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,'r5-token','mcp',?3,3600)",(application,s.user_id,"2026-01-05 12:00:01")).unwrap();
        o.primary.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)",(s.user_id,application,json!(["update_bot_settings","get_bot_details"]).to_string(),"2026-01-05 12:00:01")).unwrap();
        o.primary.execute("UPDATE users SET mcp_settings=?1",[json!({"tool_permissions":{"update_bot_settings":true,"get_bot_details":true},"dry_run":true}).to_string()]).unwrap();
        let req=|body:Value,sid:Option<&str>|{
            let mut r=Request::builder().method("POST").uri("/mcp").header("host","localhost:3000").header("authorization","Bearer r5-token").header("content-type","application/json").header("accept","application/json, text/event-stream");
            if let Some(sid)=sid {r=r.header("mcp-session-id",sid);}
            r.body(Body::from(body.to_string())).unwrap()
        };
        let response=web::router(app.clone()).oneshot(req(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"R5","version":"1"}}}),None)).await.unwrap();
        let sid=response.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
        let response=web::router(app.clone()).oneshot(req(json!({"jsonrpc":"2.0","method":"notifications/initialized"}),Some(&sid))).await.unwrap();
        assert_eq!(response.status(),202);
        let before=model::load_bot(&o.primary,id).unwrap().transient;
        let response=web::router(app.clone()).oneshot(req(json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"get_bot_details","arguments":{"bot_id":id}}}),Some(&sid))).await.unwrap();
        assert_eq!(response.status(),200);
        let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),100_000).await.unwrap()).unwrap();
        assert_ne!(body["result"]["isError"],true,"{body}");
        assert_eq!(model::load_bot(&o.primary,id).unwrap().transient,before,"MCP read preserves accounting window");
        let response=web::router(app.clone()).oneshot(req(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"update_bot_settings","arguments":{"bot_id":id,"quote_amount":200}}}),Some(&sid))).await.unwrap();
        assert_eq!(response.status(),200);
        let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),100_000).await.unwrap()).unwrap();
        assert_ne!(body["result"]["isError"],true,"{body}");
        assert!(body["result"]["content"][0]["text"].as_str().unwrap().contains("settings updated"),"{body}");
    } else {
        let token=web::csrf::new_token();
        let session=web::session::SessionData{user:Some((s.user_id,"x".into())),csrf:Some(token.clone()),..Default::default()};
        let cookie=web::session::seal(&app.keys.session,&session,app.now());
        let request=Request::builder().method("PATCH").uri(format!("/bots/{id}")).header("host","localhost:3000")
            .header("cookie",format!("{}={cookie}",web::session::COOKIE)).header("x-csrf-token",web::csrf::masked(&token))
            .header("content-type","application/json").header("accept","text/vnd.turbo-stream.html")
            .body(Body::from(json!({"bots_dca_multi_asset":{"quote_amount":"200","quote_amount_limit":"100"}}).to_string())).unwrap();
        let response=web::router(app.clone()).oneshot(request).await.unwrap();
        let status=response.status();let body=to_bytes(response.into_body(),1_000_000).await.unwrap();
        assert_eq!(status,200,"{}",String::from_utf8_lossy(&body));
    }
    let saved=model::load_bot(&o.primary,id).unwrap();
    assert_eq!(saved.missed_quote_amount().unwrap().to_s_f(),"9.999999999999998","R6 settings captured compensated carry, mcp={mcp}");
    r6_continue(app.clone(),s.user_id,id).await;
    assert_eq!(saved.settings["quote_amount"],json!(200.0));
    let t=history_market();
    let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()+chrono::Duration::seconds(1)),&mut Default::default()).await.unwrap();
    assert!(matches!(result,deltabadger::engine::tick::TickOutcome::Done{placed:true}),"{result:?}");
    let orders=t.posted_orders();
    let actual:Vec<_>=orders.iter().map(|o|json!({"side":o["side"],"asset":o["symbol"].as_str().unwrap().strip_suffix("/USD").unwrap(),"notional":o["notional"]})).collect();
    let oracle:Value=serde_json::from_str(include_str!("fixtures/histories_r6.json")).unwrap();
    assert_eq!(serde_json::to_vec(&actual).unwrap(),serde_json::to_vec(&oracle["normalized"]["orders"].as_array().unwrap().iter().map(|o|json!({"side":o["side"],"asset":o["asset"],"notional":o["notional"]})).collect::<Vec<_>>()).unwrap(),"R6 compensated settings action then tick, mcp={mcp}");
    assert_eq!(saved.missed_quote_amount().unwrap().to_s_f(),oracle["normalized"]["carry"]);
}
#[tokio::test(flavor="current_thread")]
async fn r6_web_settings_then_tick_matches_compensated_rails(){r6_settings_tick(false).await;}
#[tokio::test(flavor="current_thread")]
async fn r6_mcp_settings_then_tick_matches_compensated_rails(){r6_settings_tick(true).await;}
#[tokio::test(flavor="current_thread")]
async fn r6_engine_and_preview_then_tick_match_compensated_rails() {
    let (_d,o,s,id)=r6_history();
    let oracle:Value=serde_json::from_str(include_str!("fixtures/histories_r6.json")).unwrap();
    let bot=model::load_bot(&o.primary,id).unwrap();
    assert_eq!(amount::quote_amount_available(&o.primary,&bot).unwrap().unwrap().to_s_f(),oracle["normalized"]["before_pending"]);
    let web=r5_bot(&o.primary,&s,id);
    let preview=deltabadger::web::bot::start::amount_limit(&o.primary,&web).unwrap().unwrap();
    assert_eq!(preview.left.as_ref().unwrap().to_s(),oracle["normalized"]["cap"],"R6 preview compensated cap");
    assert_eq!(deltabadger::web::bot::write::pending(&o.primary,&web,now()).unwrap().to_d().unwrap().to_s_f(),oracle["normalized"]["before_pending"],"R6 shared pending");
    let mut draft=deltabadger::web::bot::draft::Draft::from_bot(web);
    draft.validate(&o.primary,deltabadger::web::bot::draft::ValidationContext::Start,now(),false,"en").unwrap();
    assert!(draft.errors.is_empty(),"{:?}",draft.errors.iter().map(|e|&e.message).collect::<Vec<_>>());
    let t=history_market();
    let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()),&mut Default::default()).await.unwrap();
    assert!(matches!(result,deltabadger::engine::tick::TickOutcome::Done{placed:true}),"{result:?}");
    assert_eq!(t.posted_orders()[0]["notional"],"9.99","R6 engine order matches Rails");
}
#[test]
fn r6_oracle_sources_are_pinned() {
    use sha2::{Digest,Sha256};
    let v:Value=serde_json::from_str(include_str!("fixtures/histories_r6.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (f,h) in v["sources"].as_object().unwrap(){assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(f)).unwrap())),h.as_str().unwrap());}
    assert_eq!(v["rails_unchanged"],v["normalized"]);
    assert_eq!(v["normalized"]["orders"][0]["notional"],"9.99");
}

#[tokio::test(flavor="current_thread")]
async fn r6_web_start_action_then_tick_matches_compensated_rails() {
    let (d,o,s,id)=r6_history();
    o.primary.execute("UPDATE bots SET status=2 WHERE id=?1",[id]).unwrap();
    let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
    r6_continue(app,s.user_id,id).await;
    assert_eq!(model::load_bot(&o.primary,id).unwrap().status,deltabadger::enums::BotStatus::Scheduled);
    let t=history_market();
    let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()+chrono::Duration::seconds(1)),&mut Default::default()).await.unwrap();
    assert!(matches!(result,deltabadger::engine::tick::TickOutcome::Done{placed:true}),"{result:?}");
    assert_eq!(t.posted_orders()[0]["notional"],"9.99","R6 start tick matches Rails");
}

async fn r6_continue(app:deltabadger::web::App,user:i64,id:i64) {
    use deltabadger::web;
    use axum::{body::{Body,to_bytes},http::Request};
    use tower::ServiceExt;
    let token=web::csrf::new_token();
    let session=web::session::SessionData{user:Some((user,"x".into())),csrf:Some(token.clone()),..Default::default()};
    let cookie=web::session::seal(&app.keys.session,&session,app.now());
    let request=Request::builder().method("PATCH").uri(format!("/bots/{id}/start?start_fresh=false")).header("host","localhost:3000")
        .header("cookie",format!("{}={cookie}",web::session::COOKIE)).header("x-csrf-token",web::csrf::masked(&token))
        .header("content-type","application/json").header("accept","text/vnd.turbo-stream.html")
        .body(Body::from("{}")).unwrap();
    let response=web::router(app).oneshot(request).await.unwrap();
    let status=response.status();let body=to_bytes(response.into_body(),1_000_000).await.unwrap();
    assert_eq!(status,200,"R6 actual start action: {}",String::from_utf8_lossy(&body));
}


#[tokio::test(flavor="current_thread")]
async fn r7_integer_cap_overflow_settings_then_tick_refuses() {
    use deltabadger::web;
    use axum::{body::{Body,to_bytes},http::Request};
    use tower::ServiceExt;
    let (d,o,s,id)=r6_history();
    o.primary.execute("DELETE FROM transactions",[]).unwrap();
    let asset=model::load_bot(&o.primary,id).unwrap().asset_ids()[0];
    for _ in 0..2 {
        seed::insert_row(&o.primary,&s,id,asset,&json!({"external_status":3,"amount":"1","amount_exec":"1","quote_amount_exec":"5000000000000000000","price":"10","created_at":"2026-01-05 11:59:59"}));
    }
    seed::insert_row(&o.primary,&s,id,asset,&json!({"side":1,"external_status":2,"amount":"0.1","amount_exec":"0.1","quote_amount_exec":"1","price":"10","created_at":"2026-01-05 11:59:59.500000"}));
    o.primary.execute("UPDATE bots SET status=2,settings=json_set(settings,'$.quote_amount_limit',100),transient_data=json_set(transient_data,'$.quote_amount_limit_enabled_at','2026-01-05T11:59:58Z') WHERE id=?1",[id]).unwrap();
    assert_eq!(o.primary.query_row("SELECT count(*) FROM transactions WHERE side=0 AND typeof(quote_amount_exec)='integer' AND quote_amount_exec=5000000000000000000",[],|r|r.get::<_,i64>(0)).unwrap(),2);
    let before: (String,String,Option<String>)=o.primary.query_row("SELECT settings,transient_data,settings_changed_at FROM bots WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
    let token=web::csrf::new_token();
    let session=web::session::SessionData{user:Some((s.user_id,"x".into())),csrf:Some(token.clone()),..Default::default()};
    let cookie=web::session::seal(&app.keys.session,&session,app.now());
    let request=Request::builder().method("PATCH").uri(format!("/bots/{id}")).header("host","localhost:3000")
        .header("cookie",format!("{}={cookie}",web::session::COOKIE)).header("x-csrf-token",web::csrf::masked(&token))
        .header("content-type","application/json").header("accept","text/vnd.turbo-stream.html")
        .body(Body::from(json!({"bots_dca_multi_asset":{"quote_amount":"200","quote_amount_limit":"100000000000000000000"}}).to_string())).unwrap();
    let response=web::router(app).oneshot(request).await.unwrap();
    let status=response.status();let body=to_bytes(response.into_body(),1_000_000).await.unwrap();
    assert_eq!(status,422,"R7 settings must refuse: {}",String::from_utf8_lossy(&body));
    let after: (String,String,Option<String>)=o.primary.query_row("SELECT settings,transient_data,settings_changed_at FROM bots WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(before,after,"R7 refused settings save nothing");
    // Schedule the unchanged bot: the next engine attempt must refuse independently.
    // Never repair or replace the overflowing history between settings/check/guard/ticks.
    o.primary.execute("UPDATE bots SET status=?2 WHERE id=?1",(id,deltabadger::enums::BotStatus::Scheduled as i64)).unwrap();
    let bot=model::load_bot(&o.primary,id).unwrap();
    let cap=amount::quote_amount_available(&o.primary,&bot);
    assert!(format!("{cap:?}").contains("accounting magnitude exceeds 2^53"),"R7 typed cap refusal: {cap:?}");
    for _ in 0..2 {
        let report=deltabadger::engine::eligibility::check_install(&o.primary).unwrap();
        assert!(format!("{:?}",report.unreadable).contains("accounting magnitude exceeds 2^53"),"R7 check refusal: {:?}",report.unreadable);
        assert!(report.refusal().is_err());
        let tx=model::immediate(&o.primary).unwrap();
        tx.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount',222) WHERE id=?1",[id]).unwrap();
        let guarded=eligibility::guard(&tx,&seed::cipher(),Some(id));
        assert!(matches!(&guarded,Err(eligibility::Refusal::Unreadable(_))),"R7 guarded refusal: {guarded:?}");
        assert!(format!("{guarded:?}").contains("accounting magnitude exceeds 2^53"));
        drop(tx);
        assert_eq!(model::load_bot(&o.primary,id).unwrap().settings["quote_amount"],json!(100.0));
        let t=history_market();
        let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()),&mut Default::default()).await;
        assert!(t.posted_orders().is_empty(),"R7 no orders: {result:?}");
        let bot=model::load_bot(&o.primary,id).unwrap();
        assert!(format!("{result:?} {:?}",bot.transient).contains("accounting magnitude exceeds 2^53"),"R7 tick specific refusal: {result:?} {:?}",bot.transient);
        assert!(bot.rust_placement().is_none());
    }
}


fn r8_integer_history(amount:i64,cost:i64)->(tempfile::TempDir,deltabadger::store::Opened,common::seed::Seeded,i64) {
    let (d,o,s,id)=r6_history();
    o.primary.execute("DELETE FROM transactions",[]).unwrap();
    o.primary.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount',?2,'$.quote_amount_limited',json('false')),transient_data='{}',settings_changed_at=NULL WHERE id=?1",(id,amount)).unwrap();
    let asset=model::load_bot(&o.primary,id).unwrap().asset_ids()[0];
    for (side,qty,value) in [(0,"1",cost),(1,"0.1",1)] {
        let row=seed::insert_row(&o.primary,&s,id,asset,&json!({"side":side,"external_status":2,"amount":qty,"amount_exec":qty,"quote_amount_exec":"1","price":"10","created_at":"2026-01-05 12:00:00"}));
        o.primary.execute("UPDATE transactions SET quote_amount_exec=?1 WHERE id=?2",(value,row)).unwrap();
        assert_eq!(o.primary.query_row("SELECT typeof(quote_amount_exec),quote_amount_exec FROM transactions WHERE id=?1",[row],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).unwrap(),("integer".into(),value));
    }
    (d,o,s,id)
}
#[tokio::test(flavor="current_thread")]
async fn r8_integer_engine_adapter_keeps_exact_kind_and_bounds() {
    let oracle:Value=serde_json::from_str(include_str!("fixtures/histories_r8.json")).unwrap();
    for case in oracle["cases"].as_array().unwrap() {
        let amount=case["amount"].as_i64().unwrap();
        let (_d,o,s,id)=r8_integer_history(amount,amount-100);
        let intervals=case["intervals"].as_i64().unwrap();
        let at=now()+chrono::Duration::weeks(intervals-1);
        let asset=model::load_bot(&o.primary,id).unwrap().asset_ids()[0];
        for _ in 1..intervals {
            let row=seed::insert_row(&o.primary,&s,id,asset,&json!({"external_status":2,"amount":"1","amount_exec":"1","quote_amount_exec":"1","price":"10","created_at":"2026-01-05 12:00:00"}));
            o.primary.execute("UPDATE transactions SET quote_amount_exec=?1 WHERE id=?2",(amount,row)).unwrap();
            assert_eq!(o.primary.query_row("SELECT typeof(quote_amount_exec),quote_amount_exec FROM transactions WHERE id=?1",[row],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).unwrap(),("integer".into(),amount));
        }
        let bot=model::load_bot(&o.primary,id).unwrap();
        assert_eq!(bot.settings["quote_amount"].as_i64(),Some(amount));
        let pending=amount::pending_quote_amount(&o.primary,&bot,at.timestamp_micros());
        if amount>9007199254740992 {
            assert!(format!("{pending:?}").contains("accounting magnitude exceeds 2^53"),"R8 adapter must refuse: {pending:?}");
            assert_history_tick_refusal(&o.primary,id,"accounting magnitude exceeds 2^53","R8 schedule bound").await;
        } else {
            assert_eq!(pending.unwrap().to_s_f(),case["pending"],"R8 exact Integer pending");
            let t=history_market();
            let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(at),&mut Default::default()).await.unwrap();
            assert!(matches!(result,deltabadger::engine::tick::TickOutcome::Done{placed:true}),"{result:?}");
            assert_eq!(t.posted_orders()[0]["notional"],"100.00","R8 exact wire");
        }
    }
}
#[tokio::test(flavor="current_thread")]
async fn r8_fill_cost_boundary_and_reconstructed_cost_refuse() {
    for cost in [9007199254740992i64,9007199254740993] {
        let (_d,o,_s,id)=r8_integer_history(100,cost);
        let bot=model::load_bot(&o.primary,id).unwrap();
        let pending=amount::pending_quote_amount(&o.primary,&bot,now().timestamp_micros());
        if cost==9007199254740992 { assert_eq!(pending.unwrap(),bd("0")); }
        else {
            assert!(format!("{pending:?}").contains("accounting magnitude exceeds 2^53"),"R8 fill bound: {pending:?}");
            assert_history_tick_refusal(&o.primary,id,"accounting magnitude exceeds 2^53","R8 fill bound").await;
        }
    }
    let (_d,o,_s,id)=r8_integer_history(100,1);
    o.primary.execute("UPDATE transactions SET quote_amount_exec=NULL,amount_exec=3,price=4503599627370496 WHERE side=0",[]).unwrap();
    assert_history_tick_refusal(&o.primary,id,"accounting magnitude exceeds 2^53","R8 normalized product bound").await;
}
#[tokio::test(flavor="current_thread")]
async fn r8_settings_bound_rollback_page_and_tick() {
    use deltabadger::web;
    use axum::{body::{Body,to_bytes},http::Request};
    use tower::ServiceExt;
    for (key,transient) in [("quote_amount",false),("smart_interval_quote_amount",false),("quote_amount_limit",false),("missed_quote_amount",true),("fill_cost",false),("fill_quantity",false)] {
        let (d,o,s,id)=r8_integer_history(100,0);
        let column=if transient {"transient_data"}else{"settings"};
        if key.starts_with("fill_") {
            o.primary.execute("UPDATE bots SET status=2 WHERE id=?1",[id]).unwrap();
            let field=if key=="fill_cost" {"quote_amount_exec"}else{"amount_exec"};
            o.primary.execute(&format!("UPDATE transactions SET {field}=9007199254740993 WHERE side=0"),[]).unwrap();
        } else {
            o.primary.execute(&format!("UPDATE bots SET {column}=json_set({column},?2,9007199254740993),status=2 WHERE id=?1"),(id,format!("$.{key}"))).unwrap();
        }
        let before:(String,String,Option<String>)=o.primary.query_row("SELECT settings,transient_data,settings_changed_at FROM bots WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
        let token=web::csrf::new_token();
        let session=web::session::SessionData{user:Some((s.user_id,"x".into())),csrf:Some(token.clone()),..Default::default()};
        let cookie=web::session::seal(&app.keys.session,&session,app.now());
        let request=Request::builder().method("PATCH").uri(format!("/bots/{id}")).header("host","localhost:3000").header("cookie",format!("{}={cookie}",web::session::COOKIE))
            .header("x-csrf-token",web::csrf::masked(&token)).header("content-type","application/json").header("accept","text/vnd.turbo-stream.html")
            .body(Body::from(json!({"bots_dca_multi_asset":{"quote_amount":"200"}}).to_string())).unwrap();
        let response=web::router(app.clone()).oneshot(request).await.unwrap();
        let status=response.status();let body=to_bytes(response.into_body(),1_000_000).await.unwrap();
        assert_eq!(status,501,"R8 settings refusal {key}: {}",String::from_utf8_lossy(&body));
        let after:(String,String,Option<String>)=o.primary.query_row("SELECT settings,transient_data,settings_changed_at FROM bots WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(before,after,"R8 saves nothing {key}");
        let request=Request::builder().uri(format!("/bots/{id}")).header("host","localhost:3000").header("cookie",format!("{}={cookie}",web::session::COOKIE)).body(Body::empty()).unwrap();
        let response=web::router(app).oneshot(request).await.unwrap();
        let status=response.status();let body=to_bytes(response.into_body(),1_000_000).await.unwrap();
        assert_eq!(status,501,"R8 page refusal {key}: {}",String::from_utf8_lossy(&body));
        assert!(String::from_utf8_lossy(&body).contains("Not available in the Rust build yet"),"R8 page states refusal: {}",String::from_utf8_lossy(&body));
        o.primary.execute("UPDATE bots SET status=1 WHERE id=?1",[id]).unwrap();
        assert!(eligibility::check_install(&o.primary).unwrap().refusal().is_err());
        let tx=model::immediate(&o.primary).unwrap();
        assert!(matches!(eligibility::guard(&tx,&seed::cipher(),Some(id)),Err(eligibility::Refusal::Unreadable(_))));drop(tx);
        assert_history_tick_refusal(&o.primary,id,"accounting magnitude exceeds 2^53","R8 settings bound").await;
    }
}
#[tokio::test(flavor="current_thread")]
async fn r8_merged_mcp_paper_start_refuses() {
    use deltabadger::web;
    use axum::{body::{Body,to_bytes},http::Request};
    use tower::ServiceExt;
    let (d,o,s,id)=r3_history();
    o.primary.execute("UPDATE bots SET status=2,transient_data=json_set(transient_data,'$.merged_history_until_id',3) WHERE id=?1",[id]).unwrap();
    o.primary.execute("UPDATE users SET confirmed_at=created_at,wash_sale_enabled=0",[]).unwrap();
    let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
    o.primary.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('R8','r8','http://localhost/cb',0,'mcp',?1,?1)",["2026-01-05 12:00:01"]).unwrap();
    let application=o.primary.last_insert_rowid();
    o.primary.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,'r8-token','mcp',?3,3600)",(application,s.user_id,"2026-01-05 12:00:01")).unwrap();
    o.primary.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)",(s.user_id,application,json!(["start_bot"]).to_string(),"2026-01-05 12:00:01")).unwrap();
    o.primary.execute("UPDATE users SET mcp_settings=?1",[json!({"tool_permissions":{"start_bot":true},"dry_run":true}).to_string()]).unwrap();
    let req=|body:Value,sid:Option<&str>|{
        let mut r=Request::builder().method("POST").uri("/mcp").header("host","localhost:3000").header("authorization","Bearer r8-token").header("content-type","application/json").header("accept","application/json, text/event-stream");
        if let Some(sid)=sid {r=r.header("mcp-session-id",sid);} r.body(Body::from(body.to_string())).unwrap()
    };
    let response=web::router(app.clone()).oneshot(req(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"R8","version":"1"}}}),None)).await.unwrap();
    let sid=response.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
    assert_eq!(web::router(app.clone()).oneshot(req(json!({"jsonrpc":"2.0","method":"notifications/initialized"}),Some(&sid))).await.unwrap().status(),202);
    let before=model::load_bot(&o.primary,id).unwrap();
    let response=web::router(app).oneshot(req(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"start_bot","arguments":{"bot_id":id}}}),Some(&sid))).await.unwrap();
    let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),100_000).await.unwrap()).unwrap();
    assert_eq!(body["result"]["isError"],true,"R8 upstream paper gate: {body}");
    assert!(body["result"]["content"][0]["text"].as_str().unwrap().contains("Paper"),"{body}");
    let after=model::load_bot(&o.primary,id).unwrap();
    assert_eq!(before.status,after.status);assert_eq!(before.settings,after.settings);assert_eq!(before.transient,after.transient);
    let t=history_market();
    deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()),&mut Default::default()).await.unwrap();
    assert!(t.posted_orders().is_empty());assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
}

#[tokio::test(flavor="current_thread")]
async fn r8_submitted_settings_boundary_is_checked_before_float() {
    use deltabadger::web;
    use axum::{body::{Body,to_bytes},http::Request};
    use tower::ServiceExt;
    for value in ["9007199254740992","9007199254740993","9_007_199_254_740_993junk"] {
        let (d,o,s,id)=r8_integer_history(100,0);
        o.primary.execute("UPDATE bots SET status=2 WHERE id=?1",[id]).unwrap();
        let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
        let token=web::csrf::new_token();
        let session=web::session::SessionData{user:Some((s.user_id,"x".into())),csrf:Some(token.clone()),..Default::default()};
        let cookie=web::session::seal(&app.keys.session,&session,app.now());
        let before:(String,String,Option<String>)=o.primary.query_row("SELECT settings,transient_data,settings_changed_at FROM bots WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        let request=Request::builder().method("PATCH").uri(format!("/bots/{id}")).header("host","localhost:3000").header("cookie",format!("{}={cookie}",web::session::COOKIE))
            .header("x-csrf-token",web::csrf::masked(&token)).header("content-type","application/json").header("accept","text/vnd.turbo-stream.html")
            .body(Body::from(json!({"bots_dca_multi_asset":{"quote_amount":value}}).to_string())).unwrap();
        let response=web::router(app).oneshot(request).await.unwrap();
        let status=response.status();let body=to_bytes(response.into_body(),1_000_000).await.unwrap();
        if value=="9007199254740992" {
            assert_eq!(status,200,"R8 at bound accepted: {}",String::from_utf8_lossy(&body));
            assert_eq!(model::load_bot(&o.primary,id).unwrap().settings["quote_amount"].as_f64(),Some(9007199254740992.0));
        } else {
            assert_eq!(status,422,"R8 proposed amount refused: {}",String::from_utf8_lossy(&body));
            let after:(String,String,Option<String>)=o.primary.query_row("SELECT settings,transient_data,settings_changed_at FROM bots WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
            assert_eq!(before,after,"R8 proposed settings save nothing");
        }
    }
}

#[test]
fn r8_numeric_kinds_reject_before_lossy_conversion() {
    for (settings,carry) in [
        (json!({"quote_amount":9007199254740994.0}),Value::Null),
        (json!({"quote_amount":18446744073709551615u64}),Value::Null),
        (json!({"quote_amount":100}),json!("9007199254740992.0000000001")),
        (json!({"quote_amount":100}),json!("-9007199254740993")),
    ] {
        let (_d,o,_s,id)=r8_integer_history(100,0);
        o.primary.execute("UPDATE bots SET settings=json_patch(settings,?2),transient_data=json_set(transient_data,'$.missed_quote_amount',json(?3)) WHERE id=?1",rusqlite::params![id,settings.to_string(),carry.to_string()]).unwrap();
        let bot=model::load_bot(&o.primary,id).unwrap();
        let pending=amount::pending_quote_amount(&o.primary,&bot,now().timestamp_micros());
        assert!(format!("{pending:?}").contains("accounting magnitude exceeds 2^53"),"R8 numeric kind bound: {settings} {carry}: {pending:?}");
    }
}


fn r9_history(stamp:&str) -> (tempfile::TempDir,deltabadger::store::Opened,seed::Seeded,i64) {
    let (d,o,s,id)=r6_history();
    o.primary.execute("DELETE FROM transactions",[]).unwrap();
    let a=model::load_bot(&o.primary,id).unwrap().asset_ids()[0];
    for (side,qty,cost) in [(0,"5","50"),(1,"2","20")] {
        seed::insert_row(&o.primary,&s,id,a,&json!({"side":side,"external_status":2,"amount":qty,"amount_exec":qty,"quote_amount_exec":cost,"price":"10","created_at":"2025-12-01 00:00:00"}));
    }
    seed::insert_row(&o.primary,&s,id,a,&json!({"side":0,"external_status":0,"amount":"6","quote_amount":"60","amount_exec":null,"quote_amount_exec":null,"price":"10","created_at":"2026-01-02 00:00:00"}));
    o.primary.execute("UPDATE bots SET status=2,settings_changed_at=NULL,settings=json_set(settings,'$.quote_amount',100,'$.quote_amount_limit',100),transient_data=json_object('quote_amount_limit_enabled_at',?1,'missed_quote_amount','0.0') WHERE id=?2",(stamp,id)).unwrap();
    (d,o,s,id)
}
fn r9_shapes() -> [&'static str;10] { ["2026-01-01 00:00:00","2026-01-01 00:00:00.000000","2026-01-01 00:00:00 UTC","2026-01-01 00:00:00 +0000","2026-01-01 02:00:00.000000 +02:00","2026-01-01T00:00:00Z","2026-01-01T02:00:00+02:00","2026-01-01 00:00:00.000000Z","2026-01-01 00:00:00.123456","2026-01-01T02:00:00.123456+02:00"] }
#[test]
fn r9_stored_timestamp_shapes_match_engine_and_web() {
    for stamp in r9_shapes() {
        let (_d,o,s,id)=r9_history(stamp);
        assert_eq!(o.primary.execute("UPDATE transactions SET created_at=?1 WHERE bot_id=?2 AND external_status=0",(stamp,id)).unwrap(),1);
        let bot=model::load_bot(&o.primary,id).unwrap();
        assert_eq!(amount::quote_amount_available(&o.primary,&bot).unwrap().unwrap().to_s_f(),"40.0","engine {stamp}");
        let web=r5_bot(&o.primary,&s,id);
        assert_eq!(deltabadger::web::bot::start::amount_limit(&o.primary,&web).unwrap().unwrap().left.unwrap().to_d().unwrap().to_s_f(),"40.0","web {stamp}");
        assert_eq!(deltabadger::web::bot::write::pending(&o.primary,&web,now()).unwrap().to_d().unwrap().to_s_f(),"40.0","pending {stamp}");
    }
}
fn r9_req(body:Value,sid:Option<&str>) -> axum::http::Request<axum::body::Body> {
    let mut r=axum::http::Request::builder().method("POST").uri("/mcp").header("host","localhost:3000").header("authorization","Bearer r9-token").header("content-type","application/json").header("accept","application/json, text/event-stream");
    if let Some(sid)=sid {r=r.header("mcp-session-id",sid);}
    r.body(axum::body::Body::from(body.to_string())).unwrap()
}
async fn r9_mcp(app:&deltabadger::web::App,c:&rusqlite::Connection,user:i64)->String {
    use tower::ServiceExt;
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('R9','r9','http://localhost/cb',0,'mcp',?1,?1)",["2026-01-05 12:00:01"]).unwrap();
    let application=c.last_insert_rowid();
    c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,'r9-token','mcp',?3,3600)",(application,user,"2026-01-05 12:00:01")).unwrap();
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)",(user,application,json!(["update_bot_settings","get_bot_details","start_bot","stop_bot"]).to_string(),"2026-01-05 12:00:01")).unwrap();
    c.execute("UPDATE users SET mcp_settings=?1",[json!({"tool_permissions":{"update_bot_settings":true,"get_bot_details":true,"start_bot":true,"stop_bot":true},"dry_run":false}).to_string()]).unwrap();
    let response=deltabadger::web::router(app.clone()).oneshot(r9_req(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"R9","version":"1"}}}),None)).await.unwrap();
    let sid=response.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
    let response=deltabadger::web::router(app.clone()).oneshot(r9_req(json!({"jsonrpc":"2.0","method":"notifications/initialized"}),Some(&sid))).await.unwrap();assert_eq!(response.status(),202);sid
}
async fn r9_call(app:&deltabadger::web::App,sid:&str,name:&str,args:Value)->Value {
    use tower::ServiceExt;
    let response=deltabadger::web::router(app.clone()).oneshot(r9_req(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}}),Some(sid))).await.unwrap();
    assert_eq!(response.status(),200);serde_json::from_slice(&axum::body::to_bytes(response.into_body(),1_000_000).await.unwrap()).unwrap()
}
#[tokio::test(flavor="current_thread")]
async fn r9_mcp_settings_cancel_resume_tick_spends_rails_40() {
    for stamp in r9_shapes() {
        let (d,o,s,id)=r9_history(stamp);
        let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
        let sid=r9_mcp(&app,&o.primary,s.user_id).await;
        let result=r9_call(&app,&sid,"update_bot_settings",json!({"bot_id":id,"quote_amount":200})).await;
        assert_ne!(result["result"]["isError"],true,"{stamp} {result}");
        assert_eq!(model::load_bot(&o.primary,id).unwrap().missed_quote_amount().unwrap().to_s_f(),"40.0","R9 captured carry {stamp}");
        // Venue cancellation without execution restores cap availability, but not owed carry.
        o.primary.execute("UPDATE transactions SET external_status=3 WHERE bot_id=?1 AND external_status=0",[id]).unwrap();
        let result=r9_call(&app,&sid,"start_bot",json!({"bot_id":id})).await;
        assert_ne!(result["result"]["isError"],true,"{result}");
        let t=history_market();
        let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()+chrono::Duration::seconds(1)),&mut Default::default()).await.unwrap();
        assert!(matches!(result,deltabadger::engine::tick::TickOutcome::Done{placed:true}),"{result:?}");
        let oracle:Value=serde_json::from_str(include_str!("fixtures/histories_r9.json")).unwrap();
        let case=oracle["cases"].as_array().unwrap().iter().find(|v|v["stamp"]==stamp).unwrap();
        let orders=t.posted_orders();
        let actual:Vec<_>=orders.iter().map(|o|json!({"side":o["side"],"asset":o["symbol"].as_str().unwrap().strip_suffix("/USD").unwrap(),"notional":o["notional"]})).collect();
        assert_eq!(serde_json::to_vec(&actual).unwrap(),serde_json::to_vec(&case["orders"]).unwrap(),"R9 final wire {stamp}");
    }
}
#[tokio::test(flavor="current_thread")]
async fn r9_garbage_timestamp_refuses_mcp_settings_and_reads_without_saving() {
    for key in ["quote_amount_limit_enabled_at","last_action_job_at","price_limit_condition_met_at"] {
        let (d,o,s,id)=r9_history("2026-01-01T00:00:00Z");
        o.primary.execute("UPDATE transactions SET external_status=3 WHERE external_status=0",[]).unwrap();
        o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,?1,'garbage') WHERE id=?2",(format!("$.{key}"),id)).unwrap();
        let before:(String,String,Option<String>)=o.primary.query_row("SELECT settings,transient_data,settings_changed_at FROM bots WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));let sid=r9_mcp(&app,&o.primary,s.user_id).await;
        for name in ["get_bot_details","update_bot_settings","start_bot"] {
            let result=r9_call(&app,&sid,name,if name=="update_bot_settings" {json!({"bot_id":id,"quote_amount":200})}else{json!({"bot_id":id})}).await;
            assert_eq!(result["result"]["isError"],true,"R9 {key} {name}: {result}");
        }
        let after:(String,String,Option<String>)=o.primary.query_row("SELECT settings,transient_data,settings_changed_at FROM bots WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();assert_eq!(before,after);
        o.primary.execute("UPDATE bots SET status=1 WHERE id=?1",[id]).unwrap();
        let t=history_market();
        let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()),&mut Default::default()).await;
        assert!(matches!(result,Err(deltabadger::engine::EngineError::Data(ref message)) if message.contains("Time(")),"{result:?}");
        assert!(t.posted_orders().is_empty());
    }
}

#[tokio::test(flavor="current_thread")]
async fn r9_each_stored_time_refuses_check_guard_web_draft_and_tick() {
    use deltabadger::web::{self,bot::draft::{Draft,ValidationContext}};
    use axum::{body::{Body,to_bytes},http::Request};use tower::ServiceExt;
    for field in ["started_at","settings_changed_at","last_action_job_at","quote_amount_limit_enabled_at","base_amount_limit_enabled_at","price_limit_condition_met_at","price_limit_enabled_at","start_at","created_at","updated_at"] {
        let (d,o,s,id)=r9_history("2026-01-01T00:00:00Z");
        o.primary.execute("UPDATE transactions SET external_status=3 WHERE external_status=0",[]).unwrap();
        let original=r5_bot(&o.primary,&s,id);
        match field {
            "started_at"|"settings_changed_at"=>{o.primary.execute(&format!("UPDATE bots SET {field}='garbage' WHERE id=?1"),[id]).unwrap();},
            "created_at"|"updated_at"=>{o.primary.execute(&format!("UPDATE transactions SET {field}='garbage' WHERE bot_id=?1"),[id]).unwrap();},
            key=>{let column=if key=="start_at" {"settings"}else{"transient_data"};o.primary.execute(&format!("UPDATE bots SET {column}=json_set({column},?1,'garbage') WHERE id=?2"),(format!("$.{key}"),id)).unwrap();},
        }
        let report=eligibility::check_install(&o.primary).unwrap();assert!(!report.unreadable.is_empty(),"check {field}");
        let tx=model::immediate(&o.primary).unwrap();assert!(eligibility::guard(&tx,&seed::cipher(),Some(id)).is_err(),"guard {field}");drop(tx);
        let loaded=model::load_bot(&o.primary,id).expect("R9e: loading preserves damaged timestamps");
        assert_eq!(loaded.id,id);
        let mut draft=Draft::from_bot(original);
        if !matches!(field,"started_at"|"settings_changed_at"|"created_at"|"updated_at") {
            let target=if field=="start_at" {&mut draft.candidate.settings}else{&mut draft.candidate.transient};target.insert(field.into(),json!("garbage"));
            assert!(draft.validate(&o.primary,ValidationContext::Start,now(),false,"en").is_err(),"draft {field}");
        }
        let ledger_before:String=o.primary.query_row("SELECT json_group_array(json_array(id,status,external_status,created_at,updated_at)) FROM transactions WHERE bot_id=?1",[id],|r|r.get(0)).unwrap();
        let before:String=o.primary.query_row("SELECT json_array(settings,transient_data,started_at,settings_changed_at,status) FROM bots WHERE id=?1",[id],|r|r.get(0)).unwrap();
        let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
        let token=web::csrf::new_token();let session=web::session::SessionData{user:Some((s.user_id,"x".into())),csrf:Some(token.clone()),..Default::default()};
        let cookie=web::session::seal(&app.keys.session,&session,app.now());
        for (method,path,body) in [("PATCH",format!("/bots/{id}"),json!({"bots_dca_multi_asset":{"quote_amount":"200"}})),("POST",format!("/bots/{id}/start"),json!({"start_fresh":"false"})),("GET",format!("/bots/{id}"),Value::Null)] {
            let request=Request::builder().method(method).uri(path).header("host","localhost:3000").header("cookie",format!("{}={cookie}",web::session::COOKIE)).header("x-csrf-token",web::csrf::masked(&token)).header("content-type","application/json").header("accept","text/vnd.turbo-stream.html").body(Body::from(body.to_string())).unwrap();
            let response=web::router(app.clone()).oneshot(request).await.unwrap();let status=response.status();let body=to_bytes(response.into_body(),1_000_000).await.unwrap();assert_eq!(status,501,"{field} {method}: {}",String::from_utf8_lossy(&body));assert!(!body.is_empty());
        }
        let sid=r9_mcp(&app,&o.primary,s.user_id).await;
        for name in ["get_bot_details","update_bot_settings","start_bot"] {
            let args=if name=="update_bot_settings" {json!({"bot_id":id,"quote_amount":200})}else{json!({"bot_id":id})};
            let result=r9_call(&app,&sid,name,args).await;assert_eq!(result["result"]["isError"],true,"{field} {name}: {result}");assert!(!result.to_string().contains("Invalid input:"));
        }
        let t=history_market();let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()),&mut Default::default()).await;
        assert!(result.is_err(),"tick {field}: {result:?}");assert!(t.posted_orders().is_empty());
        let after:String=o.primary.query_row("SELECT json_array(settings,transient_data,started_at,settings_changed_at,status) FROM bots WHERE id=?1",[id],|r|r.get(0)).unwrap();assert_eq!(before,after,"no save {field}");
        let ledger_after:String=o.primary.query_row("SELECT json_group_array(json_array(id,status,external_status,created_at,updated_at)) FROM transactions WHERE bot_id=?1",[id],|r|r.get(0)).unwrap();assert_eq!(ledger_before,ledger_after,"no ledger writes {field}");
        {
            // The read/settings fixture starts stopped. Prove a real safety transition,
            // not the MCP "already stopped" no-op; the damaged evidence stays untouched.
            o.primary.execute("UPDATE bots SET status=1 WHERE id=?1",[id]).unwrap();
            assert_eq!(model::load_bot(&o.primary,id).unwrap().status,deltabadger::enums::BotStatus::Scheduled);
            let market=history_market();
            let scheduled=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&market),id,&FixedClock(now()),&mut Default::default()).await;
            assert!(matches!(scheduled,Err(deltabadger::engine::EngineError::Data(ref e)) if e.contains("Time(")),"scheduled tick {field}: {scheduled:?}");
            assert!(market.posted_orders().is_empty());
            assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
            let refusal=eligibility::check_install(&o.primary).unwrap().unreadable;
            let result=r9_call(&app,&sid,"stop_bot",json!({"bot_id":id})).await;
            assert_ne!(result["result"]["isError"],true,"STOP {field}: {result}");
            assert_eq!(model::load_bot(&o.primary,id).unwrap().status,deltabadger::enums::BotStatus::Stopped);
            assert_eq!(eligibility::check_install(&o.primary).unwrap().unreadable,refusal,"STOP retains {field} refusal");
        }
    }
}

#[tokio::test(flavor="current_thread")]
async fn r9c_mcp_can_stop_a_bot_with_damaged_timestamps() {
    for field in ["last_action_job_at", "quote_amount_limit_enabled_at", "started_at", "settings_changed_at"] {
        let (d,o,s,id)=r9_history("2026-01-01T00:00:00Z");
        o.primary.execute("UPDATE bots SET status=1 WHERE id=?1",[id]).unwrap();
        if matches!(field,"started_at"|"settings_changed_at") {
            o.primary.execute(&format!("UPDATE bots SET {field}='garbage' WHERE id=?1"),[id]).unwrap();
        } else {
            o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,?1,'garbage') WHERE id=?2",(format!("$.{field}"),id)).unwrap();
        }
        let bot=model::load_bot(&o.primary,id).expect("damaged bot still loads");
        assert_eq!(bot.status,deltabadger::enums::BotStatus::Scheduled);
        let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
        let sid=r9_mcp(&app,&o.primary,s.user_id).await;
        let result=r9_call(&app,&sid,"stop_bot",json!({"bot_id":id})).await;
        assert_ne!(result["result"]["isError"],true,"{field}: {result}");
        let bot=model::load_bot(&o.primary,id).unwrap();assert_eq!(bot.status,deltabadger::enums::BotStatus::Stopped);
        let value:String=if matches!(field,"started_at"|"settings_changed_at") {
            o.primary.query_row(&format!("SELECT {field} FROM bots WHERE id=?1"),[id],|r|r.get(0)).unwrap()
        } else {
            bot.transient[field].as_str().unwrap().to_string()
        };
        assert_eq!(value,"garbage","{field}: stop preserves the diagnostic evidence");
        assert!(!eligibility::check_install(&o.primary).unwrap().unreadable.is_empty());
        let tx=model::immediate(&o.primary).unwrap();assert!(eligibility::guard(&tx,&seed::cipher(),Some(id)).is_err());drop(tx);
    }
}

// R10: Rails selects window rows with SQLite's text comparison, `created_at >= ?`, against its
// quoted boundary ('2026-01-05 12:00:00'). An RFC3339 row an hour earlier sorts after it ('T' > ' '),
// so Rails counts its $60. Parsed instants must not decide membership.
#[tokio::test(flavor="current_thread")]
async fn r10_window_membership_is_rails_text_comparison() {
    let (_d,o,s,id)=r9_history("2026-01-05 12:00:00");
    assert_eq!(o.primary.execute("UPDATE transactions SET external_status=2,amount_exec='6',quote_amount_exec='60',created_at='2026-01-05T11:00:00Z' WHERE bot_id=?1 AND external_status=0",[id]).unwrap(),1);
    o.primary.execute("UPDATE bots SET status=1,transient_data=json_set(transient_data,'$.merged_history_until_id',(SELECT max(id) FROM transactions WHERE bot_id=?1)) WHERE id=?1",[id]).unwrap();
    let counted:i64=o.primary.query_row("SELECT count(*) FROM transactions WHERE bot_id=?1 AND side=0 AND created_at >= '2026-01-05 12:00:00'",[id],|r|r.get(0)).unwrap();
    assert_eq!(counted,1,"SQLite oracle: Rails' created_at >= ? includes the RFC3339 row");
    let bot=model::load_bot(&o.primary,id).unwrap();
    assert_eq!(amount::quote_amount_available(&o.primary,&bot).unwrap().unwrap().to_s_f(),"40.0","cap");
    assert_eq!(amount::pending_quote_amount(&o.primary,&bot,now().timestamp_micros()).unwrap().to_s_f(),"40.0","amount");
    let web=r5_bot(&o.primary,&s,id);
    assert_eq!(deltabadger::web::bot::start::amount_limit(&o.primary,&web).unwrap().unwrap().left.unwrap().to_d().unwrap().to_s_f(),"40.0","web cap");
    assert_eq!(deltabadger::web::bot::write::pending(&o.primary,&web,now()).unwrap().to_d().unwrap().to_s_f(),"40.0","web pending");
    let t=history_market();
    let result=deltabadger::engine::tick::tick(&o.primary,&common::scripted::venue(&t),id,&FixedClock(now()),&mut Default::default()).await.unwrap();
    assert!(matches!(result,deltabadger::engine::tick::TickOutcome::Done{placed:true}),"{result:?}");
    let orders:Vec<_>=t.posted_orders().iter().map(|o|json!({"side":o["side"],"symbol":o["symbol"],"notional":o["notional"]})).collect();
    assert_eq!(json!(orders),json!([{"side":"buy","symbol":"AAA/USD","notional":"40.00"}]),"Rails buys $40, not $100");
}

// R10: a persisted deferral that cannot be read (null, garbage or out-of-range expiry) refuses the bot and is kept.
// AAA/BBB owe $100 (buys 5/$50 each, AAA sell 2/$20); a failed tick deferred the retrying bot to the next checkpoint.
// Rails places $0 before it; removing the damaged marker would let a restart buy $60/$40 at once.
#[tokio::test(flavor="current_thread")]
async fn r10_damaged_deferral_marker_refuses_and_is_kept() {
    use deltabadger::engine::run::{self,Engine};
    for until in [Value::Null, json!("garbage"), json!("+262144-01-01T00:00:00Z")] {
        let (d,o,_s,id)=r3_history();
        let schedule=model::load_bot(&o.primary,id).unwrap().schedule_key().unwrap().unwrap();
        o.primary.execute("UPDATE bots SET status=5,transient_data=json_set(transient_data,'$.last_action_job_at','2026-01-05T12:00:00.000Z','$.rust_defer_until',json(?1)) WHERE id=?2",
            rusqlite::params![json!({"until":until,"schedule":schedule,"origin":"local"}).to_string(),id]).unwrap();
        let before:String=o.primary.query_row("SELECT transient_data FROM bots WHERE id=?1",[id],|r|r.get(0)).unwrap();
        assert!(!eligibility::check_install(&o.primary).unwrap().unreadable.is_empty(),"{until}: load-time validation agrees with use");
        let paths=deltabadger::store::Paths::from_env(&|_|None,d.path());
        let lock=deltabadger::lease::lock(&paths,now()).unwrap();
        let transport=history_market();
        let mut engine=Engine::new(o.primary,HistoryFactory(transport.clone()),seed::cipher(),lock);
        for _ in 0..2 { run::step(&mut engine,&FixedClock(now())).await.unwrap(); }
        assert!(transport.posted_orders().is_empty(),"{until}: Rails places $0 before the checkpoint");
        let after:String=engine.primary.query_row("SELECT transient_data FROM bots WHERE id=?1",[id],|r|r.get(0)).unwrap();
        assert_eq!(before,after,"{until}: the damaged marker is kept");
        let t=history_market();
        let result=deltabadger::engine::tick::tick(&engine.primary,&common::scripted::venue(&t),id,&FixedClock(now()),&mut Default::default()).await;
        assert!(result.is_err(),"{until}: a direct tick refuses too: {result:?}");
        assert!(t.posted_orders().is_empty());
    }
}

// R11: no writer stores `"rust_defer_until": null` (placement::remove_wait removes the key), so a present-but-null
// marker is damage: refused and kept. Controls prove the same fixture reaches the scheduler: without a marker it
// buys at once; with a readable future marker step_bot honors the wait and buys nothing.
#[tokio::test(flavor="current_thread")]
async fn r11_null_deferral_marker_refuses_before_the_checkpoint() {
    use deltabadger::engine::run::{self,Engine};
    for case in ["absent","future","null"] {
        let (d,o,_s,id)=r3_history();
        let schedule=model::load_bot(&o.primary,id).unwrap().schedule_key().unwrap().unwrap();
        o.primary.execute("UPDATE bots SET status=5,transient_data=json_set(transient_data,'$.last_action_job_at','2026-01-05T12:00:00.000Z') WHERE id=?1",[id]).unwrap();
        let marker=match case {"future"=>Some(json!({"until":"2026-01-12T12:00:00.000000Z","schedule":schedule,"origin":"local"})),"null"=>Some(Value::Null),_=>None};
        if let Some(marker)=marker {
            o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_defer_until',json(?1)) WHERE id=?2",rusqlite::params![marker.to_string(),id]).unwrap();
        }
        let before:String=o.primary.query_row("SELECT transient_data FROM bots WHERE id=?1",[id],|r|r.get(0)).unwrap();
        let report=eligibility::check_install(&o.primary).unwrap();
        assert_eq!(report.unreadable.iter().any(|(bot,_)|*bot==id),case=="null","{case}: load-time validation agrees with use");
        let paths=deltabadger::store::Paths::from_env(&|_|None,d.path());
        let lock=deltabadger::lease::lock(&paths,now()).unwrap();
        let transport=history_market();
        let mut engine=Engine::new(o.primary,HistoryFactory(transport.clone()),seed::cipher(),lock);
        for _ in 0..2 { run::step(&mut engine,&FixedClock(now())).await.unwrap(); }
        let orders=transport.posted_orders();
        if case=="absent" { assert_eq!(orders.len(),2,"control: the retrying bot with no wait buys $60/$40 at once"); continue; }
        assert!(orders.is_empty(),"{case}: Rails places $0 before the checkpoint");
        let after:String=engine.primary.query_row("SELECT transient_data FROM bots WHERE id=?1",[id],|r|r.get(0)).unwrap();
        assert_eq!(before,after,"{case}: the marker is kept");
    }
}

// R11: STOP persists however a history timestamp is stored: text garbage, BLOB or integer, over HTTP and MCP.
// Check and every other writer still refuse the damage; STOP leaves it in place.
#[tokio::test(flavor="current_thread")]
async fn r11_stop_persists_with_non_text_history_timestamps() {
    use deltabadger::web;
    use axum::{body::{Body,to_bytes},http::Request};
    use tower::ServiceExt;
    for column in ["created_at","updated_at"] {
        for value in ["X'67617262616765'","20260105","'garbage'"] {
            for via in ["http","mcp"] {
                let (d,o,s,id)=r9_history("2026-01-01T00:00:00Z");
                o.primary.execute(&format!("UPDATE transactions SET {column}={value} WHERE bot_id=?1"),[id]).unwrap();
                o.primary.execute("UPDATE bots SET status=1 WHERE id=?1",[id]).unwrap();
                let refusal=eligibility::check_install(&o.primary).unwrap().unreadable;
                assert!(refusal.iter().any(|(bot,_)|*bot==id),"{column}={value}: check refuses the damage");
                let ledger:String=o.primary.query_row("SELECT json_group_array(json_array(id,quote(created_at),quote(updated_at))) FROM transactions WHERE bot_id=?1",[id],|r|r.get(0)).unwrap();
                let app=common::web::app(d.path(),"engine-test-secret",common::web::TestClock::at("2026-01-05T12:00:01Z"));
                if via=="mcp" {
                    let sid=r9_mcp(&app,&o.primary,s.user_id).await;
                    let result=r9_call(&app,&sid,"stop_bot",json!({"bot_id":id})).await;
                    assert_ne!(result["result"]["isError"],true,"MCP STOP {column}={value}: {result}");
                } else {
                    let token=web::csrf::new_token();
                    let session=web::session::SessionData{user:Some((s.user_id,"x".into())),csrf:Some(token.clone()),..Default::default()};
                    let cookie=web::session::seal(&app.keys.session,&session,app.now());
                    let request=Request::builder().method("PATCH").uri(format!("/bots/{id}/stop")).header("host","localhost:3000").header("cookie",format!("{}={cookie}",web::session::COOKIE)).header("x-csrf-token",web::csrf::masked(&token)).header("content-type","application/json").header("accept","text/vnd.turbo-stream.html").body(Body::from("{}")).unwrap();
                    let response=web::router(app.clone()).oneshot(request).await.unwrap();let status=response.status();
                    let body=to_bytes(response.into_body(),1_000_000).await.unwrap();
                    assert!(status.is_success(),"HTTP STOP {column}={value} {status}: {}",String::from_utf8_lossy(&body));
                }
                assert_eq!(model::load_bot(&o.primary,id).unwrap().status,deltabadger::enums::BotStatus::Stopped,"{via} STOP {column}={value} persists");
                assert_eq!(eligibility::check_install(&o.primary).unwrap().unreadable,refusal,"{via} STOP keeps the {column} refusal");
                let after:String=o.primary.query_row("SELECT json_group_array(json_array(id,quote(created_at),quote(updated_at))) FROM transactions WHERE bot_id=?1",[id],|r|r.get(0)).unwrap();
                assert_eq!(ledger,after,"{via} STOP leaves the history untouched");
            }
        }
    }
}
