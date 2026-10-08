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
fn seed(c:&rusqlite::Connection,owner:i64,venue:i64){c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,4,'USD',100,'2026-09-01 00:00:00','{}','{}','2026-09-01 00:00:00','2026-09-01 00:00:00')",[owner,venue]).unwrap();}
#[tokio::test(flavor="current_thread")]
async fn producer_publishes_warm_and_failed_states_and_propagates_write_errors(){
    let (_dir,o,s)=common::install_alpaca();seed(&o.primary,s.user_id,s.exchange_id);
    let db=Db::new(o.primary,common::seed::cipher());let owner=s.user_id;
    assert_eq!(job(owner).run(cx(&db,&clock()),vec![]).await,Outcome::Done);
    db.run(move|c,_|{assert!(matches!(cache::read(c,owner,None,clock().0).unwrap(),State::Warm(_)));c.execute_batch("UPDATE account_transactions SET fee_currency='USD',fee_amount=1,updated_at='2026-09-20 00:00:00';").unwrap();Ok(())}).await.unwrap();
    assert!(matches!(job(owner).run(cx(&db,&clock()),vec![]).await,Outcome::Failed(_)));
    db.run(move|c,_|{assert!(matches!(cache::read(c,owner,None,clock().0).unwrap(),State::Failed(cache::UNAVAILABLE)));c.execute_batch("UPDATE account_transactions SET fee_amount=NULL,fee_currency=NULL,updated_at='2026-09-21 00:00:00'; CREATE TRIGGER refuse_cache BEFORE UPDATE ON app_configs WHEN NEW.key LIKE 'rust_tracker_ledger.%' BEGIN SELECT RAISE(ABORT,'cache write refused'); END;").unwrap();Ok(())}).await.unwrap();
    assert!(matches!(job(owner).run(cx(&db,&clock()),vec![]).await,Outcome::Failed(_)),"cache write failure must not claim success");
}
#[tokio::test(flavor="current_thread")]
async fn producer_retries_three_changed_passes_and_never_publishes_them(){
    let (_dir,o,s)=common::install_alpaca();seed(&o.primary,s.user_id,s.exchange_id);
    o.primary.execute_batch("CREATE TABLE passes(n INTEGER); INSERT INTO passes VALUES(0); CREATE TRIGGER change_on_insert AFTER INSERT ON portfolio_snapshots BEGIN UPDATE passes SET n=n+1; UPDATE account_transactions SET updated_at=datetime(updated_at,'+1 second'); END; CREATE TRIGGER change_on_update AFTER UPDATE ON portfolio_snapshots BEGIN UPDATE passes SET n=n+1; UPDATE account_transactions SET updated_at=datetime(updated_at,'+1 second'); END;").unwrap();
    let db=Db::new(o.primary,common::seed::cipher());let owner=s.user_id;
    assert!(matches!(job(owner).run(cx(&db,&clock()),vec![]).await,Outcome::Failed(_)),"moving inputs must not look successful");
    db.run(move|c,_|{assert!(matches!(cache::read(c,owner,None,clock().0).unwrap(),State::Cold));assert_eq!(c.query_row("SELECT n FROM passes",[],|r|r.get::<_,i64>(0)).unwrap(),3);Ok(())}).await.unwrap();
}
struct StopAfterSuccess { inner:Box<dyn Job>,stop:tokio::sync::watch::Sender<bool>,runs:Arc<std::sync::atomic::AtomicUsize> }
impl Job for StopAfterSuccess {
    fn spec(&self)->jobs::Spec{self.inner.spec()}
    fn run<'a>(&'a self,cx:Cx<'a>,wakes:Vec<jobs::Wake>)->jobs::JobFuture<'a>{Box::pin(async move{
        self.runs.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        let outcome=self.inner.run(cx,wakes).await;
        if outcome==Outcome::Done {self.stop.send(true).unwrap();}
        outcome
    })}
}
#[tokio::test(flavor="current_thread")]
async fn producer_changed_inputs_wake_the_scheduler_for_a_later_stable_run(){
    let (dir,o,s)=common::install_alpaca();seed(&o.primary,s.user_id,s.exchange_id);
    o.primary.execute_batch("CREATE TABLE passes(n INTEGER); INSERT INTO passes VALUES(0); CREATE TRIGGER change_on_insert AFTER INSERT ON portfolio_snapshots WHEN (SELECT n FROM passes)<3 BEGIN UPDATE passes SET n=n+1; UPDATE account_transactions SET updated_at=datetime(updated_at,'+1 second'); END; CREATE TRIGGER change_on_update AFTER UPDATE ON portfolio_snapshots WHEN (SELECT n FROM passes)<3 BEGIN UPDATE passes SET n=n+1; UPDATE account_transactions SET updated_at=datetime(updated_at,'+1 second'); END;").unwrap();
    let (stop,rx)=tokio::sync::watch::channel(false);let runs=Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let scheduler=jobs::Scheduler::new(o.primary,common::seed::cipher(),vec![Box::new(StopAfterSuccess{inner:job(s.user_id),stop,runs:runs.clone()})],None);
    scheduler.wakers().wake(tracker::jobs::TRACKER_LEDGER,Some(&s.user_id.to_string()),None);
    tokio::time::timeout(std::time::Duration::from_secs(3),scheduler.run(rx,&clock())).await.expect("changed-input retry wake must be delivered").unwrap();
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst),2);
    let c=rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();assert!(matches!(cache::read(&c,s.user_id,None,clock().0).unwrap(),State::Warm(_)));
}
