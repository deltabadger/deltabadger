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
