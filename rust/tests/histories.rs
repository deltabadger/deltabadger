mod common;
use common::seed::{self, BotSpec};
use deltabadger::{engine::{amount, basket, model}, ruby::BigDec};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn cipher() -> &'static deltabadger::crypto::Cipher {
    static CIPHER: std::sync::OnceLock<deltabadger::crypto::Cipher> = std::sync::OnceLock::new();
    CIPHER.get_or_init(seed::cipher)
}
fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/histories.json")).unwrap() }
fn now() -> DateTime<Utc> { "2026-01-05T12:00:00Z".parse().unwrap() }
fn stamp(offset: i64) -> String { deltabadger::codec::format_time(now() + Duration::seconds(offset)) }
fn build(case: &Value) -> (tempfile::TempDir, deltabadger::store::Opened, i64, BTreeMap<String, i64>) {
    let d=common::rails_install();
    let o=deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_|None,d.path())).unwrap();
    let s=seed::seed_alpaca(&o.primary,cipher());
    let pair = json!({"base_decimals":9,"quote_decimals":2,"price_decimals":2,"minimum_base_size":"0.000000001","minimum_quote_size":"1"});
    let ids: BTreeMap<String, i64> = ["AAA", "BBB"].into_iter().map(|name| (name.into(), seed::add_alpaca_crypto(&o.primary, &s, name, &pair).0)).collect();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(100.0, &stamp(0)).weights(&[(ids["AAA"],0.5),(ids["BBB"],0.5)]));
    o.primary.execute("UPDATE bots SET settings_changed_at=?1 WHERE id=?2",rusqlite::params![stamp(0),id]).unwrap();
    for (n, r) in case["rows"].as_array().unwrap().iter().enumerate() {
        let status = match r[2].as_str().unwrap() {"closed"=>2,"cancelled"=>3,"abandoned"=>4,"open"=>1,"unknown"=>0,x=>panic!("{x}")};
        seed::insert_row(&o.primary,&s,id,ids[r[0].as_str().unwrap()], &json!({
            "external_id":format!("synthetic-{n}"),"external_status":status,"side":if r[1]=="sell" {1}else{0},
            "amount":r[3],"amount_exec":r[4],"quote_amount_exec":r[5],"quote_amount":if r.as_array().unwrap().len()>8 {r[8].clone()} else {r[5].clone()},"price":r[6],"created_at":stamp(r[7].as_i64().unwrap())
        }));
    }
    if case["limited"]==true {
        o.primary.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount_limited',json('true'),'$.quote_amount_limit',60),transient_data=json_set(transient_data,'$.quote_amount_limit_enabled_at',?1) WHERE id=?2",rusqlite::params![stamp(0),id]).unwrap();
    }
    if case["merged"]==true {
        o.primary.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.merged_history_until_id',(SELECT max(id) FROM transactions WHERE bot_id=?1)) WHERE id=?1",[id]).unwrap();
    }
    if let Some(offset)=case["split_offset"].as_i64() {
        o.primary.execute("UPDATE assets SET category='Stock', instrument_type='stock' WHERE id IN (?1,?2)",[ids["AAA"],ids["BBB"]]).unwrap();
        o.primary.execute("INSERT INTO account_transactions (user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,raw_data,created_at,updated_at) VALUES (?1,?2,15,'AAA',5,?3,?4,?3,?3)",rusqlite::params![s.user_id,s.exchange_id,stamp(offset),json!({"corporate_action":"split","split_ratio":"2:1","qty":"-5","merged_activity_ids":["a","b"]}).to_string()]).unwrap();
    }
    (d,o,id,ids)
}

#[test]
fn history_decisions_match_the_recorded_oracle() {
    let v=vectors();
    assert_eq!(v["cases"].as_array().unwrap().len(),24);
    for case in v["cases"].as_array().unwrap() {
        let (_d,o,id,ids)=build(case);
        let c=&o.primary;
        let bot=model::load_bot(c,id).unwrap();
        let at=now()+Duration::seconds(1);
        let w=basket::walk(c,&bot,at).unwrap_or_else(|e|panic!("{}: {e:?}",case["name"]));
        let holdings:Value=ids.iter().map(|(name,id)|(name.clone(),json!(w.amounts.get(id).cloned().unwrap_or_else(BigDec::zero).to_s_f()))).collect();
        assert_eq!(holdings,case["normalized"]["holdings"],"{} holdings",case["name"]);
        assert_eq!(w.contributed.to_s_f(),case["normalized"]["contributed"],"{} contributed",case["name"]);
        assert_eq!(w.cash.to_s_f(),case["normalized"]["cash"],"{} cash",case["name"]);
        let available=amount::quote_amount_available(c,&bot).unwrap();
        assert_eq!(available.as_ref().map(|v|json!(v.to_s_f())).unwrap_or(Value::Null),case["normalized"]["available"]);
        let pending=amount::pending_quote_amount(c,&bot,at.timestamp_micros()).unwrap();
        let pending=available.as_ref().map_or(pending.clone(),|cap|pending.min(cap.clone()));
        assert_eq!(pending.to_s_f(),case["normalized"]["pending"],"{} pending",case["name"]);
        basket::refresh_composition(c,&bot,at).unwrap().unwrap();
        let priced:Vec<_>=basket::members(c,&bot).unwrap().into_iter().map(|member|basket::Priced{member,reference:BigDec::from_i64(10),price:BigDec::from_i64(10)}).collect();
        let orders:Vec<_>=basket::split(&priced,&w.amounts,&basket::reserved(c,&bot).unwrap(),&pending).unwrap().into_iter().map(|leg|json!({"side":"buy","asset":leg.ticker.base_symbol,"amount":leg.quote.div(&BigDec::from_i64(10)).unwrap().to_s_f(),"quote":leg.quote.to_s_f()})).collect();
        assert_eq!(json!(orders),case["normalized"]["orders"],"{} orders",case["name"]);
        let decision=if pending.is_zero(){"skip_zero_pending"}else{"buy"};
        assert_eq!(decision,case["normalized"]["decision"]);
        let later=amount::pending_quote_amount(c,&bot,(at+Duration::weeks(1)).timestamp_micros()).unwrap();
        let later=available.map_or(later.clone(),|cap|later.min(cap));
        assert_eq!(later.to_s_f(),case["one_week_pending"],"{} later",case["name"]);
    }
}

#[test]
fn sells_and_merges_pass_check_and_the_write_guard() {
    use deltabadger::engine::eligibility;
    for case in vectors()["cases"].as_array().unwrap() {
        let (_d,o,id,_)=build(case);
        let report=eligibility::check_install(&o.primary).unwrap();
        assert!(report.problems.is_empty(),"{}: {:?}",case["name"],report.problems);
        assert!(report.unreadable.is_empty(),"{:?}",report.unreadable);
        assert_eq!(report.eligible,vec![id]);
        let tx=model::immediate(&o.primary).unwrap();
        tx.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.unrelated','preserved') WHERE id=?1",[id]).unwrap();
        eligibility::guard(&tx,cipher(),Some(id)).unwrap();
        tx.commit().unwrap();
    }
}

#[test]
fn unknown_fill_value_refuses_check_and_rolls_back_a_guarded_write() {
    use deltabadger::engine::eligibility;
    let case=vectors()["cases"][0].clone();
    let (_d,o,id,_)=build(&case);
    for sql in [
        "UPDATE transactions SET price=NULL,quote_amount_exec=NULL WHERE side=1",
        "UPDATE transactions SET amount=NULL,amount_exec=NULL WHERE side=1",
        "UPDATE transactions SET amount_exec=0 WHERE side=1",
    ] {
        let tx=model::immediate(&o.primary).unwrap();
        tx.execute_batch(sql).unwrap();
        let report=eligibility::check_install(&tx).unwrap();
        assert_eq!(report.unreadable.len(),1,"{:?}",report.problems);
        assert_eq!(report.unreadable[0].0,id);
        assert!(matches!(eligibility::guard(&tx,cipher(),Some(id)),Err(eligibility::Refusal::Unreadable(_))));
        drop(tx);
        let row:(i64,i64)=o.primary.query_row("SELECT quote_amount_exec,amount_exec FROM transactions WHERE bot_id=?1 AND side=1",[id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(row,(20,2));
    }
}

#[test]
fn deferred_classes_and_existing_refusals_still_roll_back() {
    use deltabadger::engine::eligibility;
    let (_d,o,id,_)=build(&vectors()["cases"][0]);
    for (sql,reason) in [
        ("UPDATE bots SET settings=json_set(settings,'$.start_time_enabled',json('true'))","start_time_enabled"),
        ("UPDATE bots SET settings=json_set(settings,'$.price_limited',json('true'))","price_limited"),
        ("UPDATE bots SET settings=json_set(settings,'$.indicator_limited',json('true'))","indicator_limited"),
        ("UPDATE bots SET settings=json_set(settings,'$.direction','selling')","direction"),
        ("UPDATE bots SET settings=json_set(settings,'$.weighting','market_cap')","weighting"),
        ("UPDATE bots SET transient_data=json_set(transient_data,'$.liquidation_pending',json('{}'))","liquidation_pending"),
        ("UPDATE bots SET transient_data=json_set(transient_data,'$.redeploy_pending',json('{}'))","redeploy_pending"),
        ("UPDATE bots SET transient_data=json_set(transient_data,'$.rebalance_pending',json('{}'))","rebalance_pending"),
        ("UPDATE bots SET settings=json_set(settings,'$.rebalance_enabled',json('true'))","rebalance_enabled"),
        ("UPDATE users SET wash_sale_enabled=1","wash_sale"),
        ("UPDATE transactions SET external_id='imported_test' WHERE side=1","imported"),
        ("UPDATE transactions SET base_asset_id=NULL WHERE side=1","without base_asset_id"),
        ("UPDATE transactions SET transaction_type='REBALANCE' WHERE side=1","REBALANCE/LIQUIDATION/REDEPLOY"),
        ("UPDATE transactions SET transaction_type='REDEPLOY' WHERE side=0","REBALANCE/LIQUIDATION/REDEPLOY"),
        ("UPDATE transactions SET transaction_type='LIQUIDATION' WHERE side=1","REBALANCE/LIQUIDATION/REDEPLOY"),
        ("UPDATE transactions SET transaction_type='LIQUIDATION',external_status=1 WHERE side=1","waiting LIQUIDATION"),
        ("UPDATE transactions SET transaction_type='LIQUIDATION',external_status=4 WHERE side=1","unresolved abandoned LIQUIDATION"),
    ] {
        let tx=model::immediate(&o.primary).unwrap();
        tx.execute_batch(sql).unwrap();
        let report=eligibility::check_install(&tx).unwrap();
        assert!(report.problems.iter().any(|p|p.contains(reason)),"{reason}: {:?}",report.problems);
        let error=eligibility::guard(&tx,cipher(),Some(id)).unwrap_err();
        assert!(error.reason().contains(reason),"{reason}: {error:?}");
        drop(tx);
        assert!(eligibility::check_install(&o.primary).unwrap().problems.is_empty(),"{reason}: rollback");
    }
}

#[tokio::test(flavor="current_thread")]
async fn merged_histories_drive_real_ticks_without_early_or_duplicate_buys() {
    use common::scripted::{script,ok,venue};
    use deltabadger::engine::{eligibility,tick,FixedClock};
    for case in vectors()["cases"].as_array().unwrap().iter().filter(|c|c["name"]=="merge_before_start" || c["name"]=="merge_at_start") {
        let (_d,o,id,_)=build(case);
        assert!(eligibility::check_install(&o.primary).unwrap().problems.is_empty());
        let transport=script(json!({"GET /v1beta3/crypto/us/latest/quotes":[ok(json!({"quotes":{"AAA/USD":{"ap":10},"BBB/USD":{"ap":10}}}))]}));
        let clock=FixedClock(now()+Duration::seconds(1));
        tick::tick(&o.primary,&venue(&transport),id,&clock,&mut tick::Attempts::default()).await.unwrap();
        let posted:Vec<_>=transport.posted_orders().iter().map(|o|json!({"side":o["side"],"asset":o["symbol"].as_str().unwrap().trim_end_matches("/USD"),"quote":BigDec::parse(o["notional"].as_str().unwrap()).unwrap().to_s_f()})).collect();
        let want:Vec<_>=case["normalized"]["orders"].as_array().unwrap().iter().map(|o|json!({"side":o["side"],"asset":o["asset"],"quote":o["quote"]})).collect();
        assert_eq!(posted,want,"{} wire decisions",case["name"]);
        assert!(model::load_bot(&o.primary,id).unwrap().rust_placement().is_none());
    }
}

#[test]
fn every_remaining_eligibility_branch_has_a_named_witness() {
    use deltabadger::engine::eligibility;
    let (_d,o,id,ids)=build(&vectors()["cases"][0]);
    o.primary.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    let many: serde_json::Map<String,Value> = (0..101).map(|i|((10000+i).to_string(),json!(1.0/101.0))).collect();
    let dynamic = [
        (format!("UPDATE bots SET settings=json_set(settings,'$.allocations',json('{{\"{}\":0.5,\"0{}\":0.5}}'))",ids["AAA"],ids["AAA"]),"listed more than once"),
        (format!("UPDATE bots SET settings=json_set(settings,'$.allocations',json('{}'))",Value::Object(many)),"101 assets"),
    ];
    let fixed = [
        ("UPDATE bots SET type='Bots::Unsupported'","type Bots::Unsupported"),
        ("UPDATE exchanges SET type='Exchanges::Binance'","exchange Exchanges::Binance"),
        ("UPDATE bots SET settings=json_set(settings,'$.smart_intervaled',json('true'))","smart interval amount missing"),
        ("UPDATE bots SET settings=json_set(settings,'$.limit_ordered',json('true'),'$.limit_order_pcnt_distance','broken')","limit_order_pcnt_distance"),
        ("UPDATE bots SET started_at=NULL","started_at missing"),
        ("UPDATE bots SET settings=json_set(settings,'$.interval','bad')","interval"),
        ("UPDATE bots SET settings=json_set(settings,'$.quote_amount',0)","quote_amount"),
        ("UPDATE bots SET restatement_generation=1","restated prices"),
        ("DELETE FROM users","user not found"),
        ("UPDATE exchanges SET type='Exchanges::Kraken'; UPDATE bots SET settings=json_set(settings,'$.quote_amount_limited',json('true'))","quote_amount_limited (Kraken"),
        ("UPDATE bots SET settings=json_set(settings,'$.quote_amount_limited',json('true'),'$.quote_amount_limit','bad')","quote_amount_limit"),
        ("UPDATE bots SET settings=json_set(settings,'$.quote_amount_limited',json('true')),transient_data=json_set(transient_data,'$.quote_amount_limit_enabled_at',json('{}'))","quote_amount_limit_enabled_at"),
        ("UPDATE exchanges SET type='Exchanges::Kraken'; UPDATE bots SET transient_data=json_set(transient_data,'$.merged_history_until_id',3)","merged history"),
        ("UPDATE exchanges SET type='Exchanges::Kraken'","sell order(s)"),
        ("UPDATE exchanges SET type='Exchanges::Kraken'","Kraken: only one-asset"),
        ("UPDATE bots SET settings=json_set(settings,'$.allocations',json('{\"bad\":1}'))","a weight this build does not read"),
        ("UPDATE bots SET settings=json_set(settings,'$.allocations',json('{}'))","allocations: none"),
        ("UPDATE bots SET settings=json_set(settings,'$.allocations',json('{\"123456\":0.5}'))","weights sum"),
        ("DELETE FROM tickers","no ticker for the asset"),
        ("UPDATE tickers SET base_decimals=-1","base_decimals -1"),
        ("UPDATE assets SET category='Unsupported' WHERE symbol='AAA'","asset category Unsupported"),
        ("UPDATE assets SET symbol='EUR' WHERE symbol='USD'","quote EUR"),
        ("INSERT INTO bot_index_assets (bot_id,asset_id,ticker_id,in_index,created_at,updated_at) SELECT id,999,1,1,'2026-01-01','2026-01-01' FROM bots","index assets present"),
        ("INSERT INTO account_transactions (user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,raw_data,created_at,updated_at) SELECT user_id,exchange_id,15,'AAA',0,'2026-01-01','{\"corporate_action\":\"split\"}','2026-01-01','2026-01-01' FROM bots","split(s) recorded"),
        ("UPDATE bots SET type='Bots::DcaIndex'","configured deltabadger market-data provider"),
        ("UPDATE bots SET type='Bots::DcaIndex'; UPDATE exchanges SET type='Exchanges::Kraken'","index bot (only on Alpaca)"),
        ("UPDATE bots SET type='Bots::DcaIndex'; UPDATE assets SET symbol='EUR' WHERE symbol='USD'","quote EUR"),
        ("UPDATE bots SET type='Bots::DcaIndex'","index_type"),
        ("UPDATE bots SET type='Bots::DcaIndex'","index_category_id missing"),
        ("UPDATE bots SET type='Bots::DcaIndex',settings=json_set(settings,'$.index_category_id','SYNTH')","index SYNTH is not a data-api"),
        ("UPDATE bots SET type='Bots::DcaIndex'","num_coins is not a positive integer"),
        ("UPDATE bots SET type='Bots::DcaIndex',settings=json_set(settings,'$.allocation_flattening',2)","allocation_flattening"),
    ];
    for (sql,reason) in dynamic.iter().map(|(sql,reason)|(sql.as_str(),*reason)).chain(fixed) {
        let tx=model::immediate(&o.primary).unwrap();
        tx.execute_batch(sql).unwrap();
        let bot=model::load_bot(&tx,id).unwrap();
        let reasons=eligibility::bot_reasons(&tx,&bot).unwrap();
        assert!(reasons.iter().any(|r|r.contains(reason)),"{reason}: {reasons:?}");
    }
}
