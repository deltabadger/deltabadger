//! Real scheduler ledger job populates a coherent cache after its writes.
mod common;
use deltabadger::{engine::FixedClock,jobs::{self,Cx,Db,Job,Outcome},tracker::{self,cache::{self,State}},sync::jobs::Connect,venue::{alpaca::{AlpacaVenue,Urls},http::ScriptedTransport}};
use std::{rc::Rc,sync::Arc};
use serde_json::json;
#[derive(Clone)]
struct Venues;
impl Connect for Venues {type T=ScriptedTransport;fn connect(&self,_:&deltabadger::crypto::Credentials)->AlpacaVenue<Self::T>{AlpacaVenue::new(ScriptedTransport::from_script(&json!({})),Urls::for_passphrase(Some("paper")))}}
fn clock()->FixedClock{FixedClock("2026-09-20T02:00:00Z".parse().unwrap())}
fn job(owner:i64)->Box<dyn Job>{let now=clock().0;tracker::jobs::resolve::<Venues,ScriptedTransport>(tracker::jobs::TRACKER_LEDGER,owner,&Venues,Rc::new(None),Arc::new(move||now)).unwrap()}
fn cx<'a>(db:&Db,clock:&'a FixedClock)->Cx<'a>{Cx{db:db.clone(),clock,wakers:jobs::Wakers::default()}}
fn seed(c:&rusqlite::Connection,owner:i64,venue:i64){common::seed::fresh_stock_jobs(c,clock().0);c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,4,'USD',100,'2026-09-01 00:00:00','{}','{}','2026-09-01 00:00:00','2026-09-01 00:00:00')",[owner,venue]).unwrap();}
#[tokio::test(flavor="current_thread")]
async fn producer_publishes_warm_and_failed_states_and_propagates_write_errors(){
    let (_dir,o,s)=common::install_alpaca();seed(&o.primary,s.user_id,s.exchange_id);
    let db=Db::new(o.primary,common::seed::cipher());let owner=s.user_id;
    assert_eq!(job(owner).run(cx(&db,&clock()),vec![]).await,Outcome::Done);
    db.run(move|c,_|{assert!(matches!(cache::read(c,owner,None,clock().0).unwrap(),State::Warm(_)));c.execute_batch("UPDATE account_transactions SET raw_data='not json',updated_at='2026-09-20 00:00:00';").unwrap();Ok(())}).await.unwrap();
    assert!(matches!(job(owner).run(cx(&db,&clock()),vec![]).await,Outcome::Failed(_)));
    db.run(move|c,_|{assert!(matches!(cache::read(c,owner,None,clock().0).unwrap(),State::Failed(cache::UNAVAILABLE)));c.execute_batch("UPDATE account_transactions SET raw_data='{}',updated_at='2026-09-21 00:00:00'; CREATE TRIGGER refuse_cache BEFORE UPDATE ON app_configs WHEN NEW.key LIKE 'rust_tracker_ledger.%' BEGIN SELECT RAISE(ABORT,'cache write refused'); END;").unwrap();Ok(())}).await.unwrap();
    assert!(matches!(job(owner).run(cx(&db,&clock()),vec![]).await,Outcome::Failed(_)),"cache write failure must not claim success");
}
#[tokio::test(flavor="current_thread")]
async fn a_write_that_moves_the_ledger_leaves_the_cache_unpublished(){
    let (_dir,o,s)=common::install_alpaca();seed(&o.primary,s.user_id,s.exchange_id);
    o.primary.execute_batch("CREATE TABLE passes(n INTEGER); INSERT INTO passes VALUES(0); CREATE TRIGGER change_on_insert AFTER INSERT ON portfolio_snapshots BEGIN UPDATE passes SET n=n+1; UPDATE account_transactions SET updated_at=datetime(updated_at,'+1 second'); END; CREATE TRIGGER change_on_update AFTER UPDATE ON portfolio_snapshots BEGIN UPDATE passes SET n=n+1; UPDATE account_transactions SET updated_at=datetime(updated_at,'+1 second'); END;").unwrap();
    let db=Db::new(o.primary,common::seed::cipher());let owner=s.user_id;
    assert!(matches!(job(owner).run(cx(&db,&clock()),vec![]).await,Outcome::Failed(_)),"moving inputs must not look successful");
    // One pass read a still ledger; the write unit's own snapshot moved it, so the cache stays cold.
    db.run(move|c,_|{assert!(matches!(cache::read(c,owner,None,clock().0).unwrap(),State::Cold));assert_eq!(c.query_row("SELECT n FROM passes",[],|r|r.get::<_,i64>(0)).unwrap(),1);Ok(())}).await.unwrap();
}
fn ledger_wakes(wakers:&jobs::Wakers)->usize{wakers.pending().iter().filter(|(job,_)|job==tracker::jobs::TRACKER_LEDGER).count()}
/// A walk that fails can never publish: the run ends with the walk's own error and wakes nothing (it used to wake
/// itself, and a refused walk then ran forever in a tight loop).
#[tokio::test(flavor="current_thread")]
async fn a_failed_walk_returns_its_error_and_never_wakes_itself(){
    let (_dir,o,s)=common::install_alpaca();seed(&o.primary,s.user_id,s.exchange_id);
    o.primary.execute_batch("UPDATE account_transactions SET raw_data='not json'").unwrap();
    let db=Db::new(o.primary,common::seed::cipher());let wakers=jobs::Wakers::default();
    let outcome=job(s.user_id).run(Cx{db,clock:&clock(),wakers:wakers.clone()},vec![]).await;
    assert_eq!(outcome,Outcome::Failed("a JSON column Rails did not write".into()));
    assert_eq!(ledger_wakes(&wakers),0,"a failed walk must not wake another tracker_ledger run");
}
/// Inputs that another writer moves during every pass end the run as moving. The run stored no price itself, so it
/// queues no wake: the writer that moved them wakes it.
#[tokio::test(flavor="current_thread")]
async fn moving_inputs_end_the_run_without_waking_itself(){
    let (_dir,o,s)=common::install_alpaca();seed(&o.primary,s.user_id,s.exchange_id);
    o.primary.execute_batch("CREATE TRIGGER change_on_insert AFTER INSERT ON portfolio_snapshots BEGIN UPDATE account_transactions SET updated_at=datetime(updated_at,'+1 second'); END; CREATE TRIGGER change_on_update AFTER UPDATE ON portfolio_snapshots BEGIN UPDATE account_transactions SET updated_at=datetime(updated_at,'+1 second'); END;").unwrap();
    let db=Db::new(o.primary,common::seed::cipher());let wakers=jobs::Wakers::default();
    let outcome=job(s.user_id).run(Cx{db,clock:&clock(),wakers:wakers.clone()},vec![]).await;
    assert_eq!(outcome,Outcome::Failed("Tracker ledger unavailable: inputs changed during calculation".into()));
    assert_eq!(ledger_wakes(&wakers),0,"moving inputs rely on the writer's wake, not a self-wake");
}
/// Stands in for the writer that moved the inputs: it wakes the job, as the syncs do (`wake_for_key`).
struct WriterWakes { inner:Box<dyn Job>,stop:tokio::sync::watch::Sender<bool>,runs:Arc<std::sync::atomic::AtomicUsize> }
impl Job for WriterWakes {
    fn spec(&self)->jobs::Spec{self.inner.spec()}
    fn run<'a>(&'a self,cx:Cx<'a>,wakes:Vec<jobs::Wake>)->jobs::JobFuture<'a>{Box::pin(async move{
        self.runs.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        let wakers=cx.wakers.clone();let scope=self.inner.spec().scope;
        let outcome=self.inner.run(cx,wakes).await;
        match &outcome {
            Outcome::Done=>{self.stop.send(true).unwrap();}
            _=>{assert_eq!(ledger_wakes(&wakers),0,"the job woke itself");wakers.wake(tracker::jobs::TRACKER_LEDGER,scope.as_deref(),None);}
        }
        outcome
    })}
}
#[tokio::test(flavor="current_thread")]
async fn producer_changed_inputs_run_again_on_the_writers_wake(){
    let (dir,o,s)=common::install_alpaca();seed(&o.primary,s.user_id,s.exchange_id);
    o.primary.execute_batch("CREATE TABLE passes(n INTEGER); INSERT INTO passes VALUES(0); CREATE TRIGGER change_on_insert AFTER INSERT ON portfolio_snapshots WHEN (SELECT n FROM passes)<3 BEGIN UPDATE passes SET n=n+1; UPDATE account_transactions SET updated_at=datetime(updated_at,'+1 second'); END; CREATE TRIGGER change_on_update AFTER UPDATE ON portfolio_snapshots WHEN (SELECT n FROM passes)<3 BEGIN UPDATE passes SET n=n+1; UPDATE account_transactions SET updated_at=datetime(updated_at,'+1 second'); END;").unwrap();
    let (stop,rx)=tokio::sync::watch::channel(false);let runs=Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let scheduler=jobs::Scheduler::new(o.primary,common::seed::cipher(),vec![Box::new(WriterWakes{inner:job(s.user_id),stop,runs:runs.clone()})],None);
    scheduler.wakers().wake(tracker::jobs::TRACKER_LEDGER,Some(&s.user_id.to_string()),None);
    tokio::time::timeout(std::time::Duration::from_secs(3),scheduler.run(rx,&clock())).await.expect("the writer's wake must be delivered").unwrap();
    // Each run's own snapshot moves the ledger while the trigger lasts (three runs); the fourth publishes.
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst),4);
    let c=rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();assert!(matches!(cache::read(&c,s.user_id,None,clock().0).unwrap(),State::Warm(_)));
}
/// 120 coin fees on 120 days, each day's price arriving only with its own fetch.
fn priced_one_day_at_a_time(c:&rusqlite::Connection,owner:i64,venue:i64)->(Rc<Option<deltabadger::jobs::data_api::DataApi<ScriptedTransport>>>,ScriptedTransport){
    common::seed::fresh_stock_jobs(c,clock().0);
    let days:Vec<chrono::NaiveDate>=(0..120).map(|i|"2026-05-01".parse::<chrono::NaiveDate>().unwrap()+chrono::Days::new(i)).collect();
    let row="INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,?3,'BTC',?4,?5,?6,?7,'{}','{}','2026-09-01 00:00:00','2026-09-01 00:00:00')";
    c.execute(row,rusqlite::params![owner,venue,0,"1","USD","60000","2026-04-30 00:00:00"]).unwrap();
    for d in &days {c.execute(row,rusqlite::params![owner,venue,10,"0.0001",None::<&str>,None::<&str>,format!("{d} 12:00:00")]).unwrap();}
    let price=|ds:&[chrono::NaiveDate]|json!({"status":200,"body":{"prices":ds.iter().map(|d|json!([d.and_hms_opt(0,0,0).unwrap().and_utc().timestamp_millis(),60000.0])).collect::<Vec<_>>()}});
    let mut answers=vec![price(&[])]; // the first run's prefetch
    answers.extend(days.iter().map(|d|price(&[*d])));
    let t=ScriptedTransport::from_script(&json!({"GET /api/v1/historical_prices":answers}));
    (Rc::new(Some(deltabadger::jobs::data_api::DataApi::new(deltabadger::jobs::data_api::Config{url:"http://data-api:3000".into(),token:"t".into()},t.clone(),t.clone()))),t)
}
fn job_with(owner:i64,api:Rc<Option<deltabadger::jobs::data_api::DataApi<ScriptedTransport>>>)->Box<dyn Job>{let now=clock().0;tracker::jobs::resolve::<Venues,ScriptedTransport>(tracker::jobs::TRACKER_LEDGER,owner,&Venues,api,Arc::new(move||now)).unwrap()}
struct StopWhenDone { inner:Box<dyn Job>,stop:tokio::sync::watch::Sender<bool>,runs:Arc<std::sync::atomic::AtomicUsize> }
impl Job for StopWhenDone {
    fn spec(&self)->jobs::Spec{self.inner.spec()}
    fn run<'a>(&'a self,cx:Cx<'a>,wakes:Vec<jobs::Wake>)->jobs::JobFuture<'a>{Box::pin(async move{
        self.runs.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        let outcome=self.inner.run(cx,wakes).await;
        if outcome==Outcome::Done {self.stop.send(true).unwrap();}
        outcome
    })}
}
/// One run fetches every price its walk asks for, inline, as Rails does (no allowance of fetches): the prefetch, then
/// each day's window; the stored prices move the generation, so compute! walks again, asks nothing, and publishes.
#[tokio::test(flavor="current_thread")]
async fn a_history_priced_one_day_at_a_time_is_priced_in_one_run(){
    let (dir,o,s)=common::install_alpaca();let (api,t)=priced_one_day_at_a_time(&o.primary,s.user_id,s.exchange_id);
    let (stop,rx)=tokio::sync::watch::channel(false);let runs=Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let scheduler=jobs::Scheduler::new(o.primary,common::seed::cipher(),vec![Box::new(StopWhenDone{inner:job_with(s.user_id,api),stop,runs:runs.clone()})],None);
    let wakers=scheduler.wakers();
    wakers.wake(tracker::jobs::TRACKER_LEDGER,Some(&s.user_id.to_string()),None);
    tokio::time::timeout(std::time::Duration::from_secs(30),scheduler.run(rx,&clock())).await.expect("a run that stored prices must wake the next").unwrap();
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst),1);
    assert_eq!(t.requests().len(),121,"the prefetch, then the 120 days");
    assert_eq!(ledger_wakes(&wakers),0,"the finished walk queues nothing");
    let c=rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(c.query_row("SELECT count(*) FROM historical_prices",[],|r|r.get::<_,i64>(0)).unwrap(),120);
    assert!(matches!(cache::read(&c,s.user_id,None,clock().0).unwrap(),State::Warm(_)));
}
/// A run whose prices another writer stored first (it stored none itself) still walks to the end and queues no wake.
#[tokio::test(flavor="current_thread")]
async fn a_run_whose_prices_another_writer_stored_queues_no_wake(){
    let (_dir,o,s)=common::install_alpaca();let (api,_t)=priced_one_day_at_a_time(&o.primary,s.user_id,s.exchange_id);
    o.primary.execute_batch("CREATE TRIGGER another_writer BEFORE INSERT ON historical_prices BEGIN INSERT INTO historical_prices(asset,currency,date,price) VALUES(NEW.asset,NEW.currency,NEW.date,NEW.price); END;").unwrap();
    let db=Db::new(o.primary,common::seed::cipher());let wakers=jobs::Wakers::default();
    let outcome=job_with(s.user_id,api).run(Cx{db:db.clone(),clock:&clock(),wakers:wakers.clone()},vec![]).await;
    assert_eq!(outcome,Outcome::Done);
    assert_eq!(db.run(|c,_|c.query_row("SELECT count(*) FROM historical_prices",[],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())).await.unwrap(),120,"the other writer stored them");
    assert_eq!(ledger_wakes(&wakers),0,"a run that stored nothing must not wake itself");
}
/// The run's own prefetch writes can move the price inputs under all three passes: fees on days 1 and 4 prefetch days
/// 1 to 4, answered [1,4], then [2], then [3]. Every walk succeeds but none can publish; the run stored prices, so it
/// wakes the next, which finds every price stored and publishes.
#[tokio::test(flavor="current_thread")]
async fn a_run_moved_only_by_its_own_price_writes_wakes_the_next_and_the_cache_warms(){
    let (dir,o,s)=common::install_alpaca();common::seed::fresh_stock_jobs(&o.primary,clock().0);
    let day=|d:&str|d.parse::<chrono::NaiveDate>().unwrap();
    let row="INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,?3,'BTC',?4,?5,?6,?7,'{}','{}','2026-09-01 00:00:00','2026-09-01 00:00:00')";
    o.primary.execute(row,rusqlite::params![s.user_id,s.exchange_id,0,"1","USD","60000","2026-04-30 00:00:00"]).unwrap();
    for d in ["2026-05-01","2026-05-04"] {o.primary.execute(row,rusqlite::params![s.user_id,s.exchange_id,10,"0.0001",None::<&str>,None::<&str>,format!("{d} 12:00:00")]).unwrap();}
    let price=|ds:&[&str]|json!({"status":200,"body":{"prices":ds.iter().map(|d|json!([day(d).and_hms_opt(0,0,0).unwrap().and_utc().timestamp_millis(),60000.0])).collect::<Vec<_>>()}});
    let t=ScriptedTransport::from_script(&json!({"GET /api/v1/historical_prices":[price(&["2026-05-01","2026-05-04"]),price(&["2026-05-02"]),price(&["2026-05-03"])]}));
    let api=Rc::new(Some(deltabadger::jobs::data_api::DataApi::new(deltabadger::jobs::data_api::Config{url:"http://data-api:3000".into(),token:"t".into()},t.clone(),t.clone())));
    let (stop,rx)=tokio::sync::watch::channel(false);let runs=Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let scheduler=jobs::Scheduler::new(o.primary,common::seed::cipher(),vec![Box::new(StopWhenDone{inner:job_with(s.user_id,api),stop,runs:runs.clone()})],None);
    let wakers=scheduler.wakers();
    wakers.wake(tracker::jobs::TRACKER_LEDGER,Some(&s.user_id.to_string()),None);
    tokio::time::timeout(std::time::Duration::from_secs(10),scheduler.run(rx,&clock())).await.expect("a run that stored prices must wake the next").unwrap();
    assert_eq!((runs.load(std::sync::atomic::Ordering::SeqCst),t.requests().len()),(2,3),"three moved passes, then one run that fetches nothing");
    assert_eq!(ledger_wakes(&wakers),0);
    let c=rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();assert!(matches!(cache::read(&c,s.user_id,None,clock().0).unwrap(),State::Warm(_)));
}

/// Venues whose ECB is a script: the tracker loads rates from it whatever the market-data provider is.
#[derive(Clone)]
struct EcbVenues(ScriptedTransport);
impl Connect for EcbVenues {
    type T=ScriptedTransport;
    fn connect(&self,_:&deltabadger::crypto::Credentials)->AlpacaVenue<Self::T>{AlpacaVenue::new(ScriptedTransport::from_script(&json!({})),Urls::for_passphrase(Some("paper")))}
    fn ecb(&self)->Option<deltabadger::tracker::fx::Source>{Some(deltabadger::tracker::fx::Source::Scripted(self.0.clone()))}
}
/// No data-api configured (the install's provider is CoinGecko): Rails' Tax::EcbFxRates still loads the ECB's rates
/// when they are stale, and so does the ledger job, from the source its venues give it.
#[tokio::test(flavor="current_thread")]
async fn the_ecb_is_loaded_with_no_data_api_configured(){
    let (_dir,o,s)=common::install_alpaca();seed(&o.primary,s.user_id,s.exchange_id);
    o.primary.execute_batch("DELETE FROM fx_rates; INSERT INTO fx_rates(currency,date,rate) VALUES('USD','2026-09-01',1.1);").unwrap();
    let body="KEY,FREQ,CURRENCY,CURRENCY_DENOM,EXR_TYPE,EXR_SUFFIX,TIME_PERIOD,OBS_VALUE\nEXR.D.USD.EUR.SP00.A,D,USD,EUR,SP00,A,2026-09-18,1.17\n";
    let ecb=ScriptedTransport::from_script(&json!({format!("GET {}",deltabadger::tracker::fx::ecb_path()):[{"status":200,"body":body}]}));
    let now=clock().0;
    let job=tracker::jobs::resolve::<EcbVenues,ScriptedTransport>(tracker::jobs::TRACKER_LEDGER,s.user_id,&EcbVenues(ecb.clone()),Rc::new(None),Arc::new(move||now)).unwrap();
    let db=Db::new(o.primary,common::seed::cipher());
    assert_eq!(job.run(cx(&db,&clock()),vec![]).await,Outcome::Done);
    assert_eq!(ecb.requests().len(),1);
    let newest:String=db.run(|c,_|c.query_row("SELECT max(date) FROM fx_rates WHERE currency='USD'",[],|r|r.get(0)).map_err(|e|e.to_string())).await.unwrap();
    assert_eq!(newest,"2026-09-18");
}

// S0: which venues hold the page cache back. A deleted key's rows are final history (Rails' `dependent: :nullify`);
// only a key that exists without a current import, over rows, refuses.
fn binance(c:&rusqlite::Connection)->i64{c.execute("INSERT INTO exchanges(type,name,maker_fee,taker_fee,created_at,updated_at) VALUES('Exchanges::Binance','Binance','0.1','0.1','2026-09-01 00:00:00','2026-09-01 00:00:00')",[]).unwrap();c.last_insert_rowid()}
fn key(c:&rusqlite::Connection,owner:i64,venue:i64,status:i64)->i64{let cipher=common::seed::cipher();c.execute("INSERT INTO api_keys(user_id,exchange_id,key,secret,status,key_type,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,0,'2026-09-01 00:00:00','2026-09-01 00:00:00')",rusqlite::params![owner,venue,cipher.encrypt("k"),cipher.encrypt("s"),status]).unwrap();c.last_insert_rowid()}
fn row(c:&rusqlite::Connection,owner:i64,venue:i64){c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,4,'USD',50,'2026-09-02 00:00:00','{}','{}','2026-09-02 00:00:00','2026-09-02 00:00:00')",[owner,venue]).unwrap();}
fn delete_key(c:&rusqlite::Connection,id:i64){c.execute("UPDATE account_transactions SET api_key_id=NULL WHERE api_key_id=?1",[id]).unwrap();c.execute("DELETE FROM api_keys WHERE id=?1",[id]).unwrap();}
async fn state(db:&Db,owner:i64)->State{db.run(move|c,_|Ok(cache::read(c,owner,None,clock().0).unwrap())).await.unwrap()}
#[tokio::test(flavor="current_thread")]
async fn a_pending_key_with_no_rows_does_not_hold_the_cache_back(){
    let (_dir,o,s)=common::install_alpaca();let b=binance(&o.primary);key(&o.primary,s.user_id,b,0);seed(&o.primary,s.user_id,s.exchange_id);
    let db=Db::new(o.primary,common::seed::cipher());
    assert_eq!(job(s.user_id).run(cx(&db,&clock()),vec![]).await,Outcome::Done);
    assert!(matches!(state(&db,s.user_id).await,State::Warm(_)),"a pending key on a venue with no rows has nothing to be current about");
}
#[tokio::test(flavor="current_thread")]
async fn a_key_without_a_current_import_over_rows_holds_the_cache_back(){
    let (_dir,o,s)=common::install_alpaca();let b=binance(&o.primary);key(&o.primary,s.user_id,b,2);row(&o.primary,s.user_id,b);seed(&o.primary,s.user_id,s.exchange_id);
    let db=Db::new(o.primary,common::seed::cipher());
    assert_eq!(job(s.user_id).run(cx(&db,&clock()),vec![]).await,Outcome::Done);
    assert!(matches!(state(&db,s.user_id).await,State::Cold));
}
#[tokio::test(flavor="current_thread")]
async fn the_last_key_deleted_leaves_its_rows_as_current_history(){
    let (_dir,o,s)=common::install_alpaca();seed(&o.primary,s.user_id,s.exchange_id);delete_key(&o.primary,s.api_key_id);
    let db=Db::new(o.primary,common::seed::cipher());
    assert_eq!(job(s.user_id).run(cx(&db,&clock()),vec![]).await,Outcome::Done);
    assert!(matches!(state(&db,s.user_id).await,State::Warm(_)),"Rails shows a deleted key's history");
}
#[tokio::test(flavor="current_thread")]
async fn a_key_deleted_beside_a_connected_venue_leaves_its_rows_as_current_history(){
    let (_dir,o,s)=common::install_alpaca();let b=binance(&o.primary);let gone=key(&o.primary,s.user_id,b,1);row(&o.primary,s.user_id,b);seed(&o.primary,s.user_id,s.exchange_id);delete_key(&o.primary,gone);
    let db=Db::new(o.primary,common::seed::cipher());
    assert_eq!(job(s.user_id).run(cx(&db,&clock()),vec![]).await,Outcome::Done);
    assert!(matches!(state(&db,s.user_id).await,State::Warm(_)));
}
#[tokio::test(flavor="current_thread")]
async fn a_pass_a_deletion_overtook_states_nothing_and_a_fresh_pass_publishes(){
    let (_dir,o,s)=common::install_alpaca();let b=binance(&o.primary);let gone=key(&o.primary,s.user_id,b,1);row(&o.primary,s.user_id,b);seed(&o.primary,s.user_id,s.exchange_id);
    // The deletion lands inside the pass's write unit, after its inputs were read.
    o.primary.execute(&format!("CREATE TRIGGER delete_key AFTER INSERT ON portfolio_snapshots BEGIN UPDATE account_transactions SET api_key_id=NULL WHERE api_key_id={gone}; DELETE FROM api_keys WHERE id={gone}; END;"),[]).unwrap();
    let db=Db::new(o.primary,common::seed::cipher());let owner=s.user_id;
    assert!(matches!(job(owner).run(cx(&db,&clock()),vec![]).await,Outcome::Failed(_)),"an overtaken pass must not look successful");
    assert!(matches!(state(&db,owner).await,State::Cold),"neither its result nor a failure is stated");
    db.run(|c,_|{c.execute("DROP TRIGGER delete_key",[]).unwrap();Ok(())}).await.unwrap();
    assert_eq!(job(owner).run(cx(&db,&clock()),vec![]).await,Outcome::Done);
    assert!(matches!(state(&db,owner).await,State::Warm(_)));
}
/// The deletion lands after the last pass captured its keys and before its write unit (a price that pass fetched
/// stores it): the write unit refuses on the changed credentials, and the run states no failure for it either.
#[tokio::test(flavor="current_thread")]
async fn a_pass_whose_key_was_deleted_before_its_write_states_no_failure(){
    let (_dir,o,s)=common::install_alpaca();let b=binance(&o.primary);let gone=key(&o.primary,s.user_id,b,1);row(&o.primary,s.user_id,b);
    common::seed::fresh_stock_jobs(&o.primary,clock().0);
    let day=|d:&str|d.parse::<chrono::NaiveDate>().unwrap();
    let rows="INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,?3,'BTC',?4,?5,?6,?7,'{}','{}','2026-09-01 00:00:00','2026-09-01 00:00:00')";
    o.primary.execute(rows,rusqlite::params![s.user_id,s.exchange_id,0,"1","USD","60000","2026-04-30 00:00:00"]).unwrap();
    for d in ["2026-05-01","2026-05-04"] {o.primary.execute(rows,rusqlite::params![s.user_id,s.exchange_id,10,"0.0001",None::<&str>,None::<&str>,format!("{d} 12:00:00")]).unwrap();}
    let price=|ds:&[&str]|json!({"status":200,"body":{"prices":ds.iter().map(|d|json!([day(d).and_hms_opt(0,0,0).unwrap().and_utc().timestamp_millis(),60000.0])).collect::<Vec<_>>()}});
    let t=ScriptedTransport::from_script(&json!({"GET /api/v1/historical_prices":[price(&["2026-05-01","2026-05-04"]),price(&["2026-05-02"]),price(&["2026-05-03"])]}));
    let api=Rc::new(Some(deltabadger::jobs::data_api::DataApi::new(deltabadger::jobs::data_api::Config{url:"http://data-api:3000".into(),token:"t".into()},t.clone(),t.clone())));
    // Three passes each moved by their own price; the third pass's fetch is where the key goes.
    o.primary.execute(&format!("CREATE TRIGGER delete_key AFTER INSERT ON historical_prices WHEN NEW.date='2026-05-03' BEGIN UPDATE account_transactions SET api_key_id=NULL WHERE api_key_id={gone}; DELETE FROM api_keys WHERE id={gone}; END;"),[]).unwrap();
    let db=Db::new(o.primary,common::seed::cipher());let owner=s.user_id;let wakers=jobs::Wakers::default();
    let outcome=job_with(owner,api.clone()).run(Cx{db:db.clone(),clock:&clock(),wakers:wakers.clone()},vec![]).await;
    assert_eq!(outcome,Outcome::Failed(deltabadger::engine::model::CREDENTIALS_CHANGED.into()));
    assert!(matches!(state(&db,owner).await,State::Cold),"no calculation failure is stated for an overtaken pass");
    assert_eq!(ledger_wakes(&wakers),1,"it stored prices, so the next run is woken");
    assert_eq!(job_with(owner,api).run(cx(&db,&clock()),vec![]).await,Outcome::Done);
    assert!(matches!(state(&db,owner).await,State::Warm(_)));
}
