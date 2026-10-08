//! Rails-free cache lifecycle and source-pin contract against a fresh Rails recording.
use deltabadger::tracker::{cache::{self,State},walk::{Summary,Walked}};
use chrono::{DateTime,Duration,Utc};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
fn fixture()->Value{serde_json::from_str(include_str!("fixtures/tracker_cache.json")).unwrap()}
#[test]
fn cache_oracle_sources_stay_pinned(){
    let f=fixture();let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path,hash) in f["sources"].as_object().unwrap(){assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(path)).unwrap())),hash.as_str().unwrap(),"{path}: re-record cache oracle");}
}
#[test]
fn cache_lifecycle_matches_rails_with_documented_eviction_miss(){
    let c=rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE app_configs(key TEXT UNIQUE,value TEXT,created_at TEXT,updated_at TEXT);CREATE TABLE account_transactions(id INTEGER,user_id INTEGER,updated_at TEXT);CREATE TABLE historical_prices(id INTEGER);").unwrap();
    let now:DateTime<Utc>="2026-10-08T12:00:00Z".parse().unwrap();
    let w=Walked{whole:Summary::empty(),venue:None,terms:vec![]};
    let warm=||{assert!(cache::publish(&c,1,&cache::version(&c,1).unwrap(),Some(&w),7,now).unwrap());};
    let read=|owner,at|match cache::read(&c,owner,None,at).unwrap(){State::Warm(_)=>"warm",State::Cold=>"cold",State::Failed(_)=>"failed"};
    let mut states=serde_json::Map::new();states.insert("absent".into(),json!(read(1,now)));warm();states.insert("computed".into(),json!(read(1,now)));states.insert("other_owner".into(),json!(read(42,now)));
    let State::Warm(empty)=cache::read(&c,1,Some(99),now).unwrap() else {panic!("empty scope")};states.insert("other_scope".into(),json!(empty.total_invested.to_s_f()));
    c.execute_batch("INSERT INTO account_transactions VALUES(1,42,'2026-10-08 12:00:00')").unwrap();states.insert("foreign_transaction".into(),json!(read(1,now)));
    c.execute_batch("INSERT INTO account_transactions VALUES(2,1,'2026-10-08 12:00:00')").unwrap();states.insert("insert".into(),json!(read(1,now)));warm();
    c.execute_batch("UPDATE account_transactions SET updated_at='2026-10-08 12:00:01' WHERE id=2").unwrap();states.insert("update".into(),json!(read(1,now)));warm();
    c.execute_batch("DELETE FROM account_transactions WHERE id=2").unwrap();states.insert("delete".into(),json!(read(1,now)));warm();
    c.execute_batch("INSERT INTO historical_prices VALUES(1)").unwrap();states.insert("price".into(),json!(read(1,now)));warm();
    states.insert("before_expiry".into(),json!(read(1,now+Duration::days(30)-Duration::seconds(1))));states.insert("expiry".into(),json!(read(1,now+Duration::days(30))));
    deltabadger::app_config::set_plain(&c,&cache::key(1),"wrong shape",now).unwrap();states.insert("malformed".into(),json!(read(1,now)));
    let mut expected=fixture()["states"].clone();assert_eq!(expected["delete"],"warm");assert_eq!(states["delete"],"cold","one retained version: a safe cache miss, no numeric page enabled");expected["delete"]=json!("cold");
    assert_eq!(Value::Object(states),expected);
}
