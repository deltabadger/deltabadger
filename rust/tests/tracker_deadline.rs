//! Scheduler deadlines must complete the owner's UI after uncancellable database work.
use deltabadger::{crypto::{Cipher, EncryptionKeys}, engine::SystemClock, jobs::{self, Cx, Job, JobFuture, Outcome, Retry, Scheduler, Spec, Wake, schedule::Jitter, notifications::Notifications}};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

struct Slow { name: &'static str, scope: String, fail: bool, delete_key: bool }
impl Job for Slow {
    fn spec(&self) -> Spec { Spec { name:self.name, scope:Some(self.scope.clone()), schedule:None, jitter:Jitter::NONE, retry:Retry::None, deadline:Duration::from_millis(20) } }
    fn run<'a>(&'a self,cx:Cx<'a>,_:Vec<Wake>)->JobFuture<'a> {
        Box::pin(async move {
            let fail=self.fail;let delete_key=self.delete_key;
            cx.db.run(move|c,_| {
                c.execute_batch("BEGIN IMMEDIATE; INSERT INTO probe VALUES(1)").unwrap();
                if delete_key {c.execute_batch("DELETE FROM api_keys WHERE id=7").unwrap();}
                std::thread::sleep(Duration::from_millis(120));
                c.execute_batch(if fail {"ROLLBACK"} else {"COMMIT"}).unwrap();
                Ok(())
            }).await.unwrap();
            Outcome::Done
        })
    }
}
#[tokio::test(flavor="current_thread")]
async fn deadline_completion_waits_for_commit_or_rollback_and_targets_owner_once() {
    for (name,scope) in [("ledger_sync","7"),("tracker_ledger","42")] {
        for (fail,delete_key) in [(false,false),(true,false),(false,true)] {
            let dir=tempfile::tempdir().unwrap();
            let path=dir.path().join("primary.sqlite3");
            let c=rusqlite::Connection::open(&path).unwrap();
            c.execute_batch("CREATE TABLE app_configs(key TEXT UNIQUE,value TEXT,created_at TEXT,updated_at TEXT); CREATE TABLE api_keys(id INTEGER,user_id INTEGER); INSERT INTO api_keys VALUES(7,42),(42,99); CREATE TABLE probe(n INTEGER);").unwrap();
            let (stop,rx)=watch::channel(false);
            let seen=Arc::new(Mutex::new(Vec::new()));
            let output=seen.clone();
            let check=path.clone();
            let notifications=Notifications::new(move|stream,payload| {
                let c=rusqlite::Connection::open(&check).unwrap();
                c.busy_timeout(Duration::ZERO).unwrap();
                c.execute_batch("BEGIN IMMEDIATE").expect("deadline completion must follow outstanding database work");
                let count:i64=c.query_row("SELECT count(*) FROM probe",[],|r|r.get(0)).unwrap();
                assert_eq!(count,if fail {0}else{1});
                c.execute_batch("ROLLBACK").unwrap();
                output.lock().unwrap().push((stream.to_owned(),payload.to_owned()));
                Ok(())
            });
            let cipher=Cipher::new(&EncryptionKeys::resolve(&|_|None,"deadline-test").unwrap());
            let scheduler=Scheduler::new(c,cipher,vec![Box::new(Slow{name,scope:scope.into(),fail,delete_key})],None).with_notifications(notifications);
            scheduler.wakers().wake(name,Some(scope),None);
            let stopping=async move { tokio::time::sleep(Duration::from_millis(300)).await; stop.send(true).unwrap(); };
            let (result,())=tokio::join!(scheduler.run(rx,&SystemClock),stopping);
            result.unwrap();
            let oracle:serde_json::Value=serde_json::from_str(include_str!("fixtures/tracker_completion.json")).unwrap();
            assert_eq!(serde_json::to_value(&*seen.lock().unwrap()).unwrap(),oracle["records"]["sync_42_failure"],"deadline must remove progress exactly once");
            let c=rusqlite::Connection::open(&path).unwrap();
            assert!(jobs::state::read(&c,name,Some(scope)).unwrap().failing());
        }
    }
}


struct RecoverOwner { recover: bool, finished: Arc<std::sync::atomic::AtomicBool> }
impl Job for RecoverOwner {
    fn spec(&self) -> Spec { Spec { name:"ledger_sync", scope:Some("7".into()), schedule:None, jitter:Jitter::NONE, retry:Retry::None, deadline:Duration::from_millis(20) } }
    fn run<'a>(&'a self,cx:Cx<'a>,_:Vec<Wake>)->JobFuture<'a> {
        Box::pin(async move {
            let recover=self.recover;let finished=self.finished.clone();
            cx.db.run(move|c,_| {
                c.execute_batch("BEGIN IMMEDIATE; INSERT INTO probe VALUES(1)").unwrap();
                std::thread::sleep(Duration::from_millis(120));
                if recover { c.execute_batch("CREATE TABLE api_keys(id INTEGER,user_id INTEGER); INSERT INTO api_keys VALUES(7,42)").unwrap(); }
                c.execute_batch("COMMIT").unwrap();
                finished.store(true,std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }).await.unwrap();
            Outcome::Done
        })
    }
}
#[tokio::test(flavor="current_thread")]
async fn deadline_owner_error_still_drains_and_recovers_in_both_wrappers() {
    for scheduler in [false,true] { for recover in [true,false] {
        let c=rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE app_configs(key TEXT UNIQUE,value TEXT,created_at TEXT,updated_at TEXT); CREATE TABLE probe(n INTEGER)").unwrap();
        // The preliminary owner lookup really fails: the table appears only during the job.
        let finished=Arc::new(std::sync::atomic::AtomicBool::new(false));
        let job=RecoverOwner{recover,finished:finished.clone()};
        let seen=Arc::new(Mutex::new(Vec::new()));let output=seen.clone();let committed=finished.clone();
        let notifications=Notifications::new(move|stream,payload| {
            assert!(committed.load(std::sync::atomic::Ordering::SeqCst),"notification precedes database completion");
            output.lock().unwrap().push((stream.to_owned(),payload.to_owned()));Ok(())
        });
        let cipher=Cipher::new(&EncryptionKeys::resolve(&|_|None,"deadline-test").unwrap());
        if scheduler {
            let (stop,rx)=watch::channel(false);
            let scheduler=Scheduler::new(c,cipher,vec![Box::new(job)],None).with_notifications(notifications);
            scheduler.wakers().wake("ledger_sync",Some("7"),None);
            let stopping=async move {tokio::time::sleep(Duration::from_millis(300)).await;stop.send(true).unwrap();};
            let (result,())=tokio::join!(scheduler.run(rx,&SystemClock),stopping);result.unwrap();
        } else {
            let db=jobs::Db::new(c,cipher).with_notifications(notifications);
            let result=deltabadger::sync::jobs::run_within_deadline(&job,Cx{db:db.clone(),clock:&SystemClock,wakers:jobs::Wakers::default()},vec![]).await;
            assert!(matches!(result,Outcome::Failed(_)));
            assert!(finished.load(std::sync::atomic::Ordering::SeqCst),"owner lookup error must not bypass settling outstanding database work");
        }
        assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
        let oracle:serde_json::Value=serde_json::from_str(include_str!("fixtures/tracker_completion.json")).unwrap();
        let expected=if recover {oracle["records"]["sync_42_failure"].clone()} else {serde_json::json!([])};
        assert_eq!(serde_json::to_value(&*seen.lock().unwrap()).unwrap(),expected,"post-settle owner lookup must recover and remove progress exactly once");
    }}
}

#[test]
fn deadline_owner_lookup_error_logs_its_safe_reason() {
    let output=std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact","deadline_owner_error_still_drains_and_recovers_in_both_wrappers","--nocapture"])
        .output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stdout));
    let log=String::from_utf8(output.stdout).unwrap();
    assert!(log.contains("deadline completion owner unavailable: SQLite Unknown (extended 1): owner table missing"),"lookup reason must be logged: {log}");
}
