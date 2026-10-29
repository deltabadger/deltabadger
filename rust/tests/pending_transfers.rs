//! D-PENDING-TRANSFERS: settlement is independent of the 25-hour incremental overlap.
mod common;
use chrono::{DateTime, Duration, Utc};
use deltabadger::{crypto::Credentials, engine::FixedClock, jobs::Db, sync};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

const HOUR: i64 = 3_600_000;
fn now() -> DateTime<Utc> { "2026-09-20T02:00:00Z".parse().unwrap() }
fn creds() -> Credentials { Credentials { ibkr: None, redaction_values: vec![], key: "PKTEST".into(), secret: "paper-secret".into(), passphrase: Some("paper".into()) } }
#[derive(Clone)]
struct Venue { name: &'static str, rows: Rc<RefCell<Vec<(bool, i64, Value)>>>, requests: Rc<RefCell<Vec<HttpRequest>>> }
fn row(name: &str, deposit: bool, id: &str, at: i64, complete: bool) -> Value {
    let status = match (name, deposit, complete) {
        ("Mexc", true, true) => json!(5), ("Mexc", false, true) => json!(7), ("Mexc", _, false) => json!(1),
        ("Bingx", true, true) | ("Bitrue", true, true) => json!(1), ("Bingx", false, true) => json!(6),
        ("Bitrue", false, true) => json!(5), ("Bingx" | "Bitrue", _, false) => json!(0),
        ("Bitvavo", _, true) => json!("completed"), ("Bitvavo", _, false) => json!("awaiting_processing"),
        ("Kucoin", _, true) => json!("SUCCESS"), ("Kucoin", _, false) => json!("PROCESSING"),
        ("Bybit", true, true) => json!(3), ("Bybit", true, false) => json!(1),
        (_, _, true) => json!("success"), _ => json!("pending"),
    };
    json!({"id":id,"txId":id,"txID":id,"txid":id,"orderId":id,"walletTxId":id,"status":status,
        "coin":"BTC","symbol":"BTC","currency":"BTC","amount":"1","size":"1","fee":"0.1","transactionFee":"0.1","withdrawFee":"0.1",
        "insertTime":at,"applyTime":at,"createdAt":at,"timestamp":at,"cTime":at,"successAt":at.to_string(),"updateTime":at.to_string()})
}
impl Transport for Venue {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.borrow_mut().push(r.clone());
        let q = |k: &str| r.query.iter().find(|(a,_)| *a == k).map(|(_,v)| v.as_str());
        let path = r.path.as_str();
        let transfer = path.to_lowercase().contains("deposit") || path.to_lowercase().contains("withdraw");
        let deposit = path.to_lowercase().contains("deposit");
        let start = ["startTime","startAt","start"].iter().find_map(|k| q(k)).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        let end = ["endTime","endAt","end"].iter().find_map(|k| q(k)).and_then(|s| s.parse::<i64>().ok()).unwrap_or(i64::MAX);
        let rows: Vec<Value> = if transfer && !path.contains("internal") {
            self.rows.borrow().iter().filter(|(d,t,v)| *d == deposit && (start..=end).contains(t) && q("status").is_none_or(|s| v["status"] == s || v["status"].as_i64().is_some_and(|n| s.parse::<i64>() == Ok(n)))).map(|(_,_,v)| v.clone()).collect()
        } else { vec![] };
        let body = match self.name {
            "Mexc" => if transfer { json!(rows) } else if path.ends_with("account") { json!({"balances":[]}) } else { json!({"symbols":[]}) },
            "Bingx" => if transfer { json!(rows) } else if path.ends_with("balance") { json!({"code":0,"balances":[]}) } else { json!({"code":0,"data":{"symbols":[]}}) },
            "Bitrue" => if transfer { json!({"code":200,"data":rows}) } else if path.ends_with("account") { json!({"balances":[{"asset":"BTC","free":"1","locked":"0"}]}) } else { json!({"symbols":[]}) },
            "Bitvavo" => json!(rows),
            "Bitget" => json!({"code":"00000","data":rows}),
            "Bybit" => json!({"retCode":0,"result":{"rows":rows,"list":[]}}),
            "Kucoin" => json!({"code":"200000","data":{"items":rows,"totalPage":1,"totalNum":rows.len()}}),
            _ => unreachable!(),
        };
        Ok(HttpResponse { status:200, body:body.to_string() })
    }
}
async fn run(db: &Db, key: i64, v: &Venue, at: DateTime<Utc>) {
    let c = creds(); let clock = FixedClock(at); let zero = std::time::Duration::ZERO;
    let out = match v.name {
        "Mexc" => sync::mexc_ledger::sync(db,v.clone(),key,&c,&clock).await,
        "Bingx" => sync::bingx_ledger::sync(db,v.clone(),key,&c,&clock).await,
        "Bitrue" => sync::bitrue_ledger::sync(db,v.clone(),key,&c,&clock,zero).await,
        "Bitvavo" => sync::bitvavo_ledger::sync(db,v.clone(),key,&c,&clock,zero).await,
        "Bitget" => sync::bitget_ledger::sync(db,v.clone(),key,&c,&clock).await,
        "Bybit" => sync::bybit_ledger::sync(db,v.clone(),key,&c,&clock).await,
        "Kucoin" => sync::kucoin_ledger::sync(db,v.clone(),key,&c,&clock,zero).await,
        _ => unreachable!(),
    }.unwrap().unwrap();
    assert!(out.complete);
}
fn install(name: &'static str) -> (tempfile::TempDir, Db, i64, Venue) {
    let (dir,o,s) = common::install_alpaca();
    o.primary.execute("UPDATE exchanges SET type=?1 WHERE id=?2", rusqlite::params![format!("Exchanges::{name}"),s.exchange_id]).unwrap();
    (dir,Db::new(o.primary,common::seed::cipher()),s.api_key_id,Venue{name,rows:Rc::default(),requests:Rc::default()})
}
async fn stored(db:&Db) -> Vec<(i64,Value)> {
    db.run(|c,_| {
        let mut s=c.prepare("SELECT id,raw_data FROM account_transactions ORDER BY id").unwrap();
        Ok(s.query_map([],|r| Ok((r.get(0)?,serde_json::from_str::<Value>(&r.get::<_,String>(1)?).unwrap()))).unwrap().collect::<Result<Vec<_>,_>>().unwrap())
    }).await.unwrap()
}
async fn lifecycle(name: &'static str) {
    let (_dir,db,key,v)=install(name);
    let t=now().timestamp_millis()-HOUR;
    v.rows.borrow_mut().extend([(true,t,row(name,true,"deposit",t,false)),(false,t,row(name,false,"withdrawal",t,false)),(true,t+HOUR,row(name,true,"recent",t+HOUR,true))]);
    run(&db,key,&v,now()).await;
    assert_eq!(stored(&db).await.len(),1,"pending transfers do not book funds");
    for hours in [30,60] {
        let at=now()+Duration::hours(hours);
        v.rows.borrow_mut().push((true,at.timestamp_millis(),row(name,true,&format!("recent{hours}"),at.timestamp_millis(),true)));
        v.requests.borrow_mut().clear();
        run(&db,key,&v,at).await;
        let wm: String=db.run(move |c,_| Ok(c.query_row("SELECT last_synced_at FROM api_keys WHERE id=?1",[key],|r| r.get(0)).unwrap())).await.unwrap();
        assert!(wm <= sync::sql_time(DateTime::from_timestamp_millis(t).unwrap()),"{name}: still-pending transfers must hold the watermark: {wm}");
    }
    for (deposit,_,r) in v.rows.borrow_mut().iter_mut().take(2) { *r=row(name,*deposit,if *deposit {"deposit"} else {"withdrawal"},t,true); }
    run(&db,key,&v,now()+Duration::hours(90)).await;
    let rows=stored(&db).await;
    assert_eq!(rows.len(),5,"{name}: transfers settling after 30 h are imported");
    run(&db,key,&v,now()+Duration::hours(91)).await;
    assert_eq!(stored(&db).await,rows,"{name}: stable row identities, no duplicates");
}
macro_rules! venue_test { ($test:ident,$name:literal) => { #[tokio::test] async fn $test() { lifecycle($name).await; } }; }
venue_test!(mexc_pending_settles,"Mexc");
venue_test!(bingx_pending_settles,"Bingx");
venue_test!(bitrue_pending_settles,"Bitrue");
venue_test!(bitvavo_pending_settles,"Bitvavo");
venue_test!(bitget_pending_settles,"Bitget");
venue_test!(bybit_pending_settles,"Bybit");
venue_test!(kucoin_pending_settles,"Kucoin");

#[tokio::test]
async fn mexc_100_day_gap_reads_available_history_and_keeps_a_visible_gap() {
    let (_dir,db,key,v)=install("Mexc");
    let t=now().timestamp_millis();
    v.rows.borrow_mut().push((true,t,row("Mexc",true,"old",t,true)));
    run(&db,key,&v,now()).await;
    let later=now()+Duration::days(100);
    let available=(later-Duration::days(85)).timestamp_millis();
    v.rows.borrow_mut().push((true,available,row("Mexc",true,"available",available,true)));
    v.requests.borrow_mut().clear();
    run(&db,key,&v,later).await;
    let first = v.requests.borrow().iter().filter(|r| r.path.contains("capital/")).filter_map(|r| r.query.iter().find(|(k,_)| *k=="startTime").map(|(_,v)| v.parse::<i64>().unwrap())).min();
    assert_eq!(first,Some((later-Duration::days(90)).timestamp_millis()),"request every available day, bounded by MEXC retention");
    assert_eq!(stored(&db).await.len(),2,"the 80-day floor must not discard a servable deposit");
    let error:Option<String>=db.run(move |c,_| Ok(c.query_row("SELECT last_sync_error FROM api_keys WHERE id=?1",[key],|r| r.get(0)).unwrap())).await.unwrap();
    assert!(error.as_ref().is_some_and(|e| e.contains("history gap") && e.contains("2026-09-19 01:00:00") && e.contains("2026-09-30 02:00:00")),"unservable dates must be visible: {error:?}");
    run(&db,key,&v,later+Duration::days(1)).await;
    let later_error:Option<String>=db.run(move |c,_| Ok(c.query_row("SELECT last_sync_error FROM api_keys WHERE id=?1",[key],|r| r.get(0)).unwrap())).await.unwrap();
    assert_eq!(later_error,error,"a successful later read must not erase a historical gap");
}

async fn legacy(name: &'static str) {
    let (_dir,db,key,v)=install(name);
    let t=now().timestamp_millis();
    v.rows.borrow_mut().push((true,t,row(name,true,"recent",t,true)));
    run(&db,key,&v,now()).await;
    let old=t-30*HOUR;
    let raw=row(name,false,"legacy",old,false);
    let tx_id=match name { "Bitrue"|"Bitget"=>Some("withdrawal-legacy"),"Bitvavo"=>None,_=>Some("legacy") };
    let original=db.run(move |c,_| {
        c.execute("INSERT INTO account_transactions (user_id,api_key_id,exchange_id,entry_type,base_currency,base_amount,tx_id,transacted_at,raw_data,created_at,updated_at) SELECT user_id,id,exchange_id,5,'BTC',1,?1,?2,?3,?2,?2 FROM api_keys WHERE id=?4",
            rusqlite::params![tx_id,sync::sql_time(DateTime::from_timestamp_millis(old).unwrap()),raw.to_string(),key]).unwrap();
        Ok(c.last_insert_rowid())
    }).await.unwrap();
    let completed=row(name,false,"legacy",old,true);
    v.rows.borrow_mut().push((false,old,completed.clone()));
    run(&db,key,&v,now()+Duration::hours(1)).await;
    let rows=stored(&db).await;
    assert_eq!(rows.len(),2,"{name}: completion updates an existing pending row");
    assert_eq!(rows.iter().find(|(id,_)| *id==original).unwrap().1,completed,"{name}: old state picked up without resetting the watermark");
}
macro_rules! legacy_test { ($test:ident,$name:literal) => { #[tokio::test] async fn $test() { legacy($name).await; } }; }
legacy_test!(mexc_existing_pending,"Mexc");
legacy_test!(bingx_existing_pending,"Bingx");
legacy_test!(bitrue_existing_pending,"Bitrue");
legacy_test!(bitvavo_existing_pending,"Bitvavo");
legacy_test!(bitget_existing_pending,"Bitget");
legacy_test!(bybit_existing_pending,"Bybit");
legacy_test!(kucoin_existing_pending,"Kucoin");

#[tokio::test]
async fn an_omitted_pending_transfer_survives_until_a_terminal_reply() {
    let (_dir,db,key,v)=install("Mexc");
    let t=now().timestamp_millis()-HOUR;
    v.rows.borrow_mut().extend([(false,t,row("Mexc",false,"missing",t,false)),(true,t+HOUR,row("Mexc",true,"recent",t+HOUR,true))]);
    run(&db,key,&v,now()).await;
    v.rows.borrow_mut().remove(0);
    run(&db,key,&v,now()+Duration::hours(30)).await;
    v.rows.borrow_mut().push((false,t,row("Mexc",false,"missing",t,true)));
    run(&db,key,&v,now()+Duration::hours(60)).await;
    assert_eq!(stored(&db).await.len(),2);
}

#[tokio::test]
async fn bitvavo_pending_without_hash_settles_when_the_hash_arrives() {
    let (_dir,db,key,v)=install("Bitvavo");
    let t=now().timestamp_millis()-HOUR;
    let mut pending=row("Bitvavo",false,"late-hash",t,false);
    pending.as_object_mut().unwrap().remove("id"); pending.as_object_mut().unwrap().remove("txId");
    v.rows.borrow_mut().extend([(false,t,pending),(true,t+HOUR,row("Bitvavo",true,"recent",t+HOUR,true))]);
    run(&db,key,&v,now()).await;
    let mut complete=row("Bitvavo",false,"late-hash",t,true); complete.as_object_mut().unwrap().remove("id");
    v.rows.borrow_mut()[0].2=complete;
    run(&db,key,&v,now()+Duration::hours(30)).await;
    let wm:String=db.run(move|c,_| Ok(c.query_row("SELECT last_synced_at FROM api_keys WHERE id=?1",[key],|r|r.get(0)).unwrap())).await.unwrap();
    assert_eq!(wm,sync::sql_time(now()),"a learned hash must release the pending hold");
    assert_eq!(stored(&db).await.len(),2);
}

#[tokio::test]
async fn bitvavo_existing_pending_without_hash_updates_its_amounts_in_place() {
    let (_dir,db,key,v)=install("Bitvavo");
    let t=now().timestamp_millis()-30*HOUR;
    let mut pending=row("Bitvavo",false,"late-hash",t,false);
    pending.as_object_mut().unwrap().remove("id"); pending.as_object_mut().unwrap().remove("txId");
    pending["fee"]=json!("0");
    let original=db.run(move |c,_| {
        c.execute("INSERT INTO account_transactions (user_id,api_key_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,raw_data,created_at,updated_at) SELECT user_id,id,exchange_id,5,'BTC',1,?1,?2,?1,?1 FROM api_keys WHERE id=?3",
            rusqlite::params![sync::sql_time(DateTime::from_timestamp_millis(t).unwrap()),pending.to_string(),key]).unwrap();
        c.execute("UPDATE api_keys SET last_synced_at=?1 WHERE id=?2",rusqlite::params![sync::sql_time(now()),key]).unwrap();
        Ok(c.last_insert_rowid())
    }).await.unwrap();
    let mut complete=row("Bitvavo",false,"late-hash",t,true); complete.as_object_mut().unwrap().remove("id");
    v.rows.borrow_mut().push((false,t,complete.clone()));
    run(&db,key,&v,now()).await;
    assert_eq!(stored(&db).await,vec![(original,complete)],"a newly assigned hash keeps the original row");
    let fee:Option<f64>=db.run(move|c,_| Ok(c.query_row("SELECT fee_amount FROM account_transactions WHERE id=?1",[original],|r|r.get(0)).unwrap())).await.unwrap();
    assert_eq!(fee,Some(0.1),"settlement refreshes the fee as well as raw state");
}

#[tokio::test]
async fn mexc_cancellation_releases_the_hold_without_booking_funds() {
    let (_dir,db,key,v)=install("Mexc");
    let t=now().timestamp_millis()-HOUR;
    v.rows.borrow_mut().extend([(false,t,row("Mexc",false,"canceled",t,false)),(true,t+HOUR,row("Mexc",true,"recent",t+HOUR,true))]);
    run(&db,key,&v,now()).await;
    v.rows.borrow_mut()[0].2["status"]=json!(9);
    run(&db,key,&v,now()+Duration::hours(30)).await;
    assert_eq!(stored(&db).await.len(),1);
    let pending:bool=db.run(move|c,_| Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM app_configs WHERE key=?1)",[format!("rust_sync.pending_transfers:{key}")],|r|r.get(0)).unwrap())).await.unwrap();
    assert!(!pending,"a proved terminal state releases the durable hold");
}
