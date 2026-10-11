//! Rails-free cold figures and complete-page boundary regressions.
use deltabadger::{tracker::figures::page as holdings,web::tracker::{holdings as page,read}};
use rusqlite::Connection;
fn db()->Connection {
    let c=Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE assets(id INTEGER,symbol TEXT,name TEXT,category TEXT,color TEXT,image_url TEXT);
    CREATE TABLE account_balances(user_id INTEGER,exchange_id INTEGER,asset_id INTEGER,id INTEGER,free NUMERIC,locked NUMERIC,usd_value NUMERIC,priced_at TEXT,synced_at TEXT);
    CREATE TABLE api_keys(id INTEGER,user_id INTEGER,exchange_id INTEGER,status INTEGER,key_type INTEGER,last_sync_error TEXT,balances_synced_at TEXT,key TEXT,secret TEXT,passphrase TEXT,access_token TEXT,rsa_signature_key TEXT,rsa_encryption_key TEXT,dh_param TEXT);
    CREATE TABLE exchanges(id INTEGER,type TEXT);CREATE TABLE app_configs(key TEXT UNIQUE,value TEXT,created_at TEXT,updated_at TEXT);
    CREATE TABLE account_transactions(user_id INTEGER);CREATE TABLE portfolio_snapshots(user_id INTEGER);CREATE TABLE portfolio_venue_snapshots(user_id INTEGER);
    INSERT INTO exchanges VALUES(1,'Exchanges::Alpaca');INSERT INTO api_keys(id,user_id,exchange_id,status,key_type,balances_synced_at,key,secret) VALUES(1,7,1,1,0,'2026-10-08 12:00:00','ciphertext-key','ciphertext-secret');
    INSERT INTO assets VALUES(1,'AAPL','Apple','Stock','#112233',NULL),(2,'USD','Dollar','Currency',NULL,NULL),(3,'AAPL','Other class','Cryptocurrency',NULL,NULL);
    INSERT INTO account_balances VALUES(7,1,1,1,2,0,200,'2026-10-08 12:00:00','2026-10-08 12:00:00'),(7,1,2,2,50,0,50,'2026-10-08 12:00:00','2026-10-08 12:00:00'),(8,1,1,3,1,0,99999,'2026-10-08 12:00:00','2026-10-08 12:00:00');").unwrap();
    stamp(&c,true);c
}
/// A completed balance sync by the current key, as the sync job records it.
fn stamp(c:&Connection,complete:bool){
    let v=deltabadger::engine::model::credential_version_by_id(c,1).unwrap().unwrap();
    deltabadger::engine::model::credential_write(c,&Some(v.clone()),|f|deltabadger::sync::cache::record(f,7,1,&v,complete,"2026-10-08T12:00:00Z".parse().unwrap())).unwrap();
}
#[test]
fn cold_figures_owner_scope_cash_and_asset_identity_are_exact(){
    let c=db();c.pragma_update(None,"query_only",true).unwrap();let before=c.total_changes();
    for (scope,cash,expected,count) in [(None,true,"250.0",2),(None,false,"200.0",1),(Some(1),true,"250.0",2),(Some(2),true,"0.0",0)]{
        let result=holdings::cold(&c,7,scope,cash).unwrap();assert_eq!(result.value.unwrap().to_s_f(),expected);assert_eq!(result.holdings.len(),count);
    }
    assert_eq!(before,c.total_changes());c.pragma_update(None,"query_only",false).unwrap();
    c.execute_batch("INSERT INTO account_balances VALUES(7,2,1,4,1,0,100,'2026-10-08 12:00:00','2026-10-08 12:00:00'),(7,1,3,5,1,0,20,'2026-10-08 12:00:00','2026-10-08 12:00:00')").unwrap();
    let result=holdings::cold(&c,7,None,false).unwrap();assert_eq!(result.holdings.len(),2,"cold Rails groups by asset identity");assert_eq!(result.holdings[0].value.to_s_f(),"300.0");
}
#[test]
fn cold_missing_or_stale_values_never_become_money(){
    for change in ["usd_value=NULL","usd_value=0","usd_value=-10","priced_at=NULL","priced_at='2026-10-08 11:54:59'"]{
        let c=db();c.execute(&format!("UPDATE account_balances SET {change} WHERE user_id=7 AND asset_id=1"),[]).unwrap();
        let result=holdings::cold(&c,7,None,true).unwrap();assert!(result.value.is_none(),"{change}: cannot show numeric aggregate");assert!(result.reason.is_some());
    }
    let c=db();c.execute("UPDATE account_balances SET priced_at='2026-10-08 11:55:00' WHERE user_id=7",[]).unwrap();assert!(holdings::cold(&c,7,None,true).unwrap().value.is_some(),"five minutes exactly is not stale");
    c.execute("UPDATE api_keys SET balances_synced_at='2026-10-08 12:00:01'",[]).unwrap();assert!(holdings::cold(&c,7,None,true).unwrap().stale,"watermark without balance row participates");
}
#[test]
fn cold_sql_predicate_empty_cash_and_database_errors(){
    let c=db();c.execute("UPDATE account_balances SET free=0,locked=0 WHERE asset_id=1",[]).unwrap();let result=holdings::cold(&c,7,None,false).unwrap();assert!(result.only_cash);assert_eq!(result.value.unwrap().to_s_f(),"0.0");
    c.execute("DROP TABLE account_balances",[]).unwrap();assert!(holdings::cold(&c,7,None,false).is_err());
}
#[test]
fn cold_page_boundary_refuses_every_unfinished_region(){
    let c=db();assert!(read::only(&c,|c|page::supported(c,7)).unwrap());assert!(!page::supported(&c,8).unwrap());
    for table in ["account_transactions","portfolio_snapshots","portfolio_venue_snapshots"]{
        let c=db();c.execute(&format!("INSERT INTO {table} VALUES(8)"),[]).unwrap();assert!(page::supported(&c,7).unwrap());c.execute(&format!("INSERT INTO {table} VALUES(7)"),[]).unwrap();assert!(!page::supported(&c,7).unwrap(),"{table}");
    }
    for change in ["status=0","key_type=1","last_sync_error='failed'","exchange_id=2"]{let c=db();c.execute(&format!("UPDATE api_keys SET {change}"),[]).unwrap();assert!(!page::supported(&c,7).unwrap(),"{change}");}
    let c=db();c.execute("INSERT INTO app_configs(key,value) VALUES('rust_tracker_ledger.8','{}')",[]).unwrap();assert!(page::supported(&c,7).unwrap());c.execute("INSERT INTO app_configs(key,value) VALUES('rust_tracker_ledger.7','{}')",[]).unwrap();assert!(!page::supported(&c,7).unwrap());
    let c=db();c.execute("DROP TABLE portfolio_snapshots",[]).unwrap();assert!(page::supported(&c,7).is_err());
}

#[test]
fn cold_nullable_asset_labels_are_empty_text_without_losing_value(){
    for change in ["name=NULL","symbol=NULL","name=NULL,symbol=NULL"]{
        let c=db();c.execute(&format!("UPDATE assets SET {change} WHERE id=1"),[]).unwrap();
        let result=holdings::cold(&c,7,None,false);
        assert!(result.is_ok(),"nullable labels must not reject known money: {result:?}");
        let result=result.unwrap();assert_eq!(result.value.unwrap().to_s_f(),"200.0");
        if change.contains("name=NULL"){assert_eq!(result.holdings[0].name,"");}
        if change.contains("symbol=NULL"){assert_eq!(result.holdings[0].symbol,"");}
    }
}

#[test]
fn cold_figures_need_balances_synced_by_the_current_key(){
    // Rails has no credential provenance; Rust's navbar already withholds these balances, so the page does too.
    for case in ["no stamp","incomplete sync","replaced key"]{
        let c=db();
        match case {"no stamp"=>{c.execute("DELETE FROM app_configs",[]).unwrap();},"incomplete sync"=>stamp(&c,false),_=>{c.execute("UPDATE api_keys SET secret='replaced'",[]).unwrap();}}
        c.pragma_update(None,"query_only",true).unwrap();
        let result=holdings::cold(&c,7,None,true).unwrap();
        assert!(result.value.is_none(),"{case}: cannot show numeric aggregate");
        assert_eq!(result.reason,Some("Tracker figures unavailable: balances not yet synced with the current key"),"{case}");
    }
}
