//! A12 PR 1: pure classification only; the live writer still uses the recorded legacy IDs.
use deltabadger::sync::binance_identity::{classify, Candidate, Context, Decision, Event, Source};
use serde_json::{json, Value};
mod common;

fn raw() -> Value {
    json!({"id":"d1","txId":"hash","coin":"BTC","network":"BTC","address":"addr","addressTag":"",
        "amount":"1.00","insertTime":1786960800000_i64,"applyTime":"2026-08-17 10:00:00","transactionFee":"0.01",
        "asset":"BTC","time":1786960800000_i64,"projectId":"p1","type":"BONUS","positionId":"pos1",
        "purchaseId":"buy1","redeemId":"redeem1","rewards":"1","purchaseAmount":"1",
        "transId":"detail","fromAsset":"BTC","transferedAmount":"0.1","serviceChargeAmount":"0.01"})
}
fn event(source: Source, raw: &Value) -> Event {
    Event::read(source, raw, Context { parent: Some("parent1"), requested_type: Some("BONUS"), occurred_ms:Some(1786960800000) }).unwrap()
}
fn stored(event: Event) -> Candidate { Candidate { row_id: 7, event: Some(event), legacy_id: true } }

#[test]
fn capital_ids_separate_sources_coins_networks_and_us_without_changing_payloads() {
    for source in [Source::Deposit, Source::Withdrawal] {
        let a = event(source, &raw());
        for (coin, network) in [("BTC", "BTC"), ("ETH", "ETH")] {
            let mut b = raw(); b["id"] = json!("d2"); b["coin"] = json!(coin); b["network"] = json!(network);
            let b = event(source, &b);
            assert_ne!(a.key(), b.key());
            for batch in [vec![a.clone(), b.clone()], vec![b.clone(), a.clone()]] {
                assert_eq!(classify(&batch, &[stored(a.clone())]), batch.iter().map(|e| if e.key()==a.key() { Decision::Existing(7) } else { Decision::Missing }).collect::<Vec<_>>());
            }
            assert_eq!(classify(&[b.clone(), b.clone()], &[stored(b)]), [Decision::Existing(7); 2]);
        }
    }
    assert_ne!(event(Source::Deposit, &raw()).key(), event(Source::Withdrawal, &raw()).key());
}

#[test]
fn rewards_and_earn_use_discriminators_and_refuse_corrections_or_indistinguishable_events() {
    for (source, field) in [(Source::FlexibleReward,"projectId"), (Source::LockedReward,"positionId"),
        (Source::FlexibleSubscription,"purchaseId"), (Source::LockedSubscription,"purchaseId"),
        (Source::FlexibleRedemption,"redeemId"), (Source::LockedRedemption,"redeemId")] {
        let a = event(source, &raw()); let mut b = raw(); b[field] = json!("different");
        assert_ne!(a.key(), event(source, &b).key());
        b = raw(); b["amount"] = json!("2"); b["rewards"] = json!("2"); b["purchaseAmount"] = json!("2");
        assert_eq!(a.key(), event(source, &b).key(), "amount cannot mint an identity");
        assert_eq!(classify(&[event(source,&b)], &[stored(a.clone())]), [Decision::Conflict]);
        if matches!(source, Source::FlexibleReward | Source::LockedReward) {
            let mut formatted=raw();formatted["rewards"]=json!("1.000");formatted["time"]=json!("01786960800000");
            assert_eq!(classify(&[event(source,&formatted)], &[stored(a.clone())]),[Decision::Existing(7)]);
            let mut changed=raw();changed["principal"]=json!("2");
            assert_eq!(classify(&[event(source,&changed)], &[stored(a.clone())]),[Decision::Conflict],"same reward tuple with changed payload");
            assert_eq!(classify(&[a.clone(),a], &[]), [Decision::Unresolved; 2]);
        }
    }
    let mut b=raw(); b["type"]=json!("REALTIME");
    assert!(Event::read(Source::FlexibleReward,&b,Context {parent:None,requested_type:Some("BONUS"),..Context::default()}).is_err());
    let realtime=Event::read(Source::FlexibleReward,&b,Context {parent:None,requested_type:Some("REALTIME"),..Context::default()}).unwrap();
    assert_ne!(realtime.key(),event(Source::FlexibleReward,&raw()).key());
    let mut fallback=raw(); fallback.as_object_mut().unwrap().remove("purchaseId");
    let a=event(Source::FlexibleSubscription,&fallback);
    assert!(a.key().is_none());
    assert_eq!(classify(std::slice::from_ref(&a), &[]),[Decision::Unresolved]);
    assert_eq!(classify(std::slice::from_ref(&a), &[stored(a.clone())]),[Decision::Existing(7)]);
    for field in ["asset","projectId"] { let mut other=fallback.clone(); other[field]=json!("other");
        assert_eq!(classify(&[event(Source::FlexibleSubscription,&other)], &[stored(a.clone())]),[Decision::Unresolved]); }
}

#[test]
fn dust_retains_parent_context_and_requires_whole_unambiguous_pairs() {
    let out=event(Source::DustOut,&raw()); let inn=event(Source::DustIn,&raw());
    assert_ne!(out.key(),inn.key()); assert_eq!(out.pair_key(),inn.pair_key()); assert_eq!(out.group_id(),inn.group_id());
    for (parent,asset) in [("parent1","ETH"),("parent2","BTC")] {
        let mut r=raw(); r["fromAsset"]=json!(asset);
        let sibling=Event::read(Source::DustOut,&r,Context {parent:Some(parent),requested_type:None,occurred_ms:Some(1786960800000)}).unwrap();
        assert_ne!(out.key(),sibling.key()); assert_ne!(out.group_id(),sibling.group_id());
    }
    assert!(out.sibling_id().unwrap().starts_with("dust-a12-v1-"));
    assert!(out.sibling_id().unwrap().ends_with("-out"));
    assert_eq!(classify(&[out.clone(),inn.clone()],&[stored(out.clone())]),[Decision::Unresolved;2]);
    assert_eq!(classify(&[out.clone(),inn.clone()],&[]),[Decision::Missing;2]);
    assert_eq!(classify(std::slice::from_ref(&out),&[]),[Decision::Unresolved]);
    let old=[stored(out.clone()), Candidate {row_id:8,event:Some(inn.clone()),legacy_id:true}];
    assert_eq!(classify(&[out,inn],&old),[Decision::Existing(7),Decision::Existing(8)]);
    assert!(Event::read(Source::DustOut,&raw(),Context {occurred_ms:Some(1786960800000),..Context::default()}).unwrap().key().is_none());
}

#[test]
fn missing_malformed_ambiguous_nil_id_and_deleted_candidates_never_authorize_insertion() {
    let a=event(Source::Deposit,&raw());
    for candidates in [vec![stored(a.clone()),stored(a.clone())], vec![Candidate {row_id:7,event:None,legacy_id:true}],
        vec![Candidate {row_id:7,event:Some(a.clone()),legacy_id:false}]] {
        assert_eq!(classify(std::slice::from_ref(&a),&candidates),[Decision::Unresolved]);
    }
    for field in ["id","network","address","amount","insertTime"] {
        let mut bad=raw(); bad.as_object_mut().unwrap().remove(field);
        if let Ok(e)=Event::read(Source::Deposit,&bad,Context::default()) {assert_eq!(classify(&[e],&[]),[Decision::Unresolved]);}
    }
    for value in [json!("NaN"),json!({}),json!("1x")] { let mut bad=raw(); bad["amount"]=value;
        assert!(Event::read(Source::Deposit,&bad,Context::default()).is_err()); }
    let mut exact=raw(); exact["amount"]=json!("1e0");
    assert_eq!(classify(&[event(Source::Deposit,&exact)],&[stored(a)]),[Decision::Existing(7)]);
}

#[test]
fn tuple_encoding_is_unambiguous_versioned_and_full_sha256() {
    let a=event(Source::Deposit,&raw()); let key=a.key().unwrap();
    assert!(key.tuple().contains("a12-v1")); assert_eq!(key.digest().len(),64);
    let mut b=raw(); b["id"]=json!("d1:withdrawal");
    assert_ne!(a.key(),event(Source::Deposit,&b).key());
    assert!(a.sibling_id().unwrap().starts_with("binance-a12-v1:deposit:"));
}

#[test]
fn recorded_rails_collisions_and_negative_controls_remain_the_oracle() {
    use sha2::{Digest,Sha256};
    let v:Value=serde_json::from_str(include_str!("fixtures/binance_identity.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path,hash) in v["sources"].as_object().unwrap() {
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root.join(path)).unwrap())),hash.as_str().unwrap(),"re-record {path}");
    }
    let cases=v["collisions"].as_array().unwrap();
    assert_eq!(cases.len(),8+8+4+1+2+1,"8 capital/reward/dust + 8 Earn + 4 excluded + flexible mapper + 2 US transfers + negative controls");
    for c in cases {
        let name=c["name"].as_str().unwrap();
        let (mapped,stored)=match name {
            "flexible_missing_type"=>(0,0),"flexible_mapper_only"=>(3,1),"dust_assets"|"dust_parents"=>(4,2),
            "negative_controls"=>(8,8),
            n if n.starts_with("simple_earn_") && !n.ends_with("fallback")=>(2,2),_ =>(2,1)
        };
        assert_eq!(c["entries"].as_array().unwrap().len(),mapped,"{name}");
        assert_eq!(c["imported"],stored,"{name}"); assert_eq!(c["repeated"],0,"{name}");
        assert_eq!(c["stored"].as_array().unwrap().len(),stored as usize,"{name}");
        // No A12 writer yet: Rails' loss is evidence, not an enabled Rust divergence.
        assert!(c["stored"].as_array().unwrap().iter().all(|r|!r["tx_id"].as_str().unwrap().contains("a12")));
        if name=="withdrawals" {assert_eq!(c["stored"][0]["fee_amount"],"0.01");}
    }
}

#[test]
fn recorded_collision_siblings_have_distinct_keys_independent_of_order_or_page_boundaries() {
    let v:Value=serde_json::from_str(include_str!("fixtures/binance_identity.json")).unwrap();
    for c in v["collisions"].as_array().unwrap() {
        let name=c["name"].as_str().unwrap();
        if name.contains("margin")||name.contains("futures")||name=="liquidations"||name=="negative_controls" {continue}
        let events:Vec<_>=c["entries"].as_array().unwrap().iter().map(|r| {
            let source=if name.starts_with("dust") {if r["entry_type"]=="swap_in" {Source::DustIn}else{Source::DustOut}}
                else if name.contains("flexible") && name.contains("subscriptions") {Source::FlexibleSubscription}
                else if name.contains("flexible") && name.contains("redemptions") {Source::FlexibleRedemption}
                else if name.contains("locked") && name.contains("subscriptions") {Source::LockedSubscription}
                else if name.contains("locked") && name.contains("redemptions") {Source::LockedRedemption}
                else if name=="locked_rewards" {Source::LockedReward}
                else if name=="flexible_mapper_only" {Source::FlexibleReward}
                else if r["entry_type"]=="withdrawal" {Source::Withdrawal}else{Source::Deposit};
            let parent=c["answers"]["dust_log"]["userAssetDribblets"].as_array().and_then(|ps|ps.iter().find(|p|
                p["userAssetDribbletDetails"].as_array().unwrap().contains(&r["raw_data"]))).and_then(|p|p["transId"].as_str());
            Event::read(source,&r["raw_data"],Context {parent,requested_type:r["raw_data"]["type"].as_str(),occurred_ms:Some(1786960800000)}).unwrap()
        }).collect();
        if name.ends_with("fallback") {assert!(events.iter().all(|e|e.key().is_none()));continue}
        let keys:std::collections::BTreeSet<_>=events.iter().map(|e|e.key().unwrap().tuple()).collect();
        assert_eq!(keys.len(),events.len(),"{name}");
        let old:Vec<_>=events.iter().enumerate().map(|(i,e)|Candidate {row_id:i as i64,event:Some(e.clone()),legacy_id:true}).collect();
        assert!(classify(&events,&old).iter().all(|d|matches!(d,Decision::Existing(_))),"{name}");
        let mut reversed=events.clone();reversed.reverse();
        assert!(classify(&reversed,&old).iter().all(|d|matches!(d,Decision::Existing(_))));
        if !name.starts_with("dust") {for page in events.chunks(1) {assert!(classify(page,&old).iter().all(|d|matches!(d,Decision::Existing(_))));}}
    }
}

#[tokio::test(flavor="current_thread")]
async fn live_sync_retains_legacy_losses_and_q11_exclusions_without_a12_writes() {
    let v:Value=serde_json::from_str(include_str!("fixtures/binance_identity.json")).unwrap();
    for case in v["collisions"].as_array().unwrap() {
        let name=case["name"].as_str().unwrap();
        if ["negative_controls","flexible_mapper_only"].contains(&name) {continue}
        let (dir,opened,seed)=common::install(); let c=&opened.primary;
        c.execute("UPDATE exchanges SET type=?1 WHERE id=?2",rusqlite::params![if name.starts_with("us_") {"Exchanges::BinanceUs"}else{"Exchanges::Binance"},seed.exchange_id]).unwrap();
        c.execute("UPDATE api_keys SET last_synced_at='2026-08-16 00:00:00' WHERE id=?1",[seed.api_key_id]).unwrap();
        let mut state=json!({"earn":{}});
        for (endpoint,answer) in case["answers"].as_object().unwrap() {
            match endpoint.as_str() {
                "deposit_history"=>state["deposits"]=answer.clone(),"withdraw_history"=>state["withdrawals"]=answer.clone(),
                "dust_log"=>state["dust"]=answer["userAssetDribblets"].clone(),
                e if e.starts_with("simple_earn_")=> {let key=e.trim_start_matches("simple_earn_").replace("_rewards","_rewardsRecord").replace("_subscriptions","_subscriptionRecord").replace("_redemptions","_redemptionRecord");state["earn"][key]=answer["rows"].clone();},
                _=>{}
            }
        }
        let step=json!({"kind":"ledger","at":"2026-08-18T00:00:00Z","binance_state":state});
        let scenario=json!({"parity_scratch":true,"api_key_id":seed.api_key_id,"credentials":{"key":"test-key","secret":"dGVzdC1zZWNyZXQ="},"steps":[step]});
        std::fs::write(dir.path().join("scenario.json"),scenario.to_string()).unwrap();drop(opened);
        let output=deltabadger::sync::parity::run(dir.path(),std::sync::Arc::new(common::seed::cipher())).await.unwrap();
        assert_eq!(output["steps"][0]["raised"],false,"{name}");
        for request in output["steps"][0]["requests"].as_array().unwrap() {let path=request[0].as_str().unwrap();
            assert!(!["/margin/","/fapi/","/dapi/"].iter().any(|p|path.contains(p)),"Q11 {path}");}
        let c=rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        assert_eq!(c.query_row("SELECT last_sync_error FROM api_keys WHERE id=?1",[seed.api_key_id],|r|r.get::<_,Option<String>>(0)).unwrap(),None,"{name}");
        let excluded=name.contains("margin")||name.contains("futures")||name=="liquidations";
        let count:i64=c.query_row("SELECT count(*) FROM account_transactions",[],|r|r.get(0)).unwrap();
        let expected=if excluded {0}else if name=="flexible_missing_type" {1}else{case["imported"].as_i64().unwrap()};
        assert_eq!(count,expected,"{name}");
        if !excluded && name!="flexible_missing_type" {for row in case["stored"].as_array().unwrap() {
            let (kind,coin,amount,fee,group,raw):(i64,String,f64,Option<f64>,Option<String>,String)=c.query_row(
                "SELECT entry_type,base_currency,base_amount,fee_amount,group_id,raw_data FROM account_transactions WHERE tx_id=?1",[row["tx_id"].as_str().unwrap()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
            let kinds=["buy","sell","swap_in","swap_out","deposit","withdrawal","staking_reward","lending_interest","airdrop"];
            assert_eq!(json!([kinds[kind as usize],coin,amount,fee,group,serde_json::from_str::<Value>(&raw).unwrap()]),json!([row["entry_type"],row["base_currency"],row["base_amount"].as_str().unwrap().parse::<f64>().unwrap(),row["fee_amount"].as_str().map(|s|s.parse::<f64>().unwrap()),row["group_id"],row["raw_data"]]),"{name}");
        }}
        let before=deltabadger::sync::parity::snapshot(&c).unwrap();drop(c);
        deltabadger::sync::parity::run(dir.path(),std::sync::Arc::new(common::seed::cipher())).await.unwrap();
        let c=rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        assert_eq!(before["account_transactions"],deltabadger::sync::parity::snapshot(&c).unwrap()["account_transactions"],"repeat {name}");
        assert_eq!(c.query_row("SELECT count(*) FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:%'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }
}
