//! Real HTTP writes, including rollback, ownership and cancellation-safe delivery.
mod common;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::web::{csrf, session, App};
use rusqlite::Connection;
use serde_json::{json, Value};

fn install() -> (tempfile::TempDir, Connection, i64, App, Browser) {
    let (dir, opened, seeded) = common::install_alpaca();
    let c = opened.primary;
    c.execute("UPDATE users SET confirmed_at=created_at, setup_completed=1", []).unwrap();
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [seeded.user_id], |r| r.get(0)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at("2026-09-10T12:00:30.123456Z"));
    let token = csrf::new_token();
    let data = session::SessionData { user: Some((seeded.user_id, hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
    let browser = Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: Some(format!("<meta name=\"csrf-token\" content=\"{}\">", csrf::masked(&token))) };
    (dir, c, seeded.user_id, app, browser)
}
const HEADERS: &[(&str, &str)] = &[("origin", "http://localhost:3000"), ("accept", "text/vnd.turbo-stream.html, text/html")];

fn row(c:&Connection,owner:i64,kind:i64,base:&str,amount:&str,at:&str)->i64 {
    c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,created_at,updated_at) VALUES(?1,1,?2,?3,?4,?5,'2026-01-01','2026-01-01')",(owner,kind,base,amount,at)).unwrap();c.last_insert_rowid()
}
fn manual(c:&Connection,id:i64)->Value { let s:String=c.query_row("SELECT manual_values FROM account_transactions WHERE id=?1",[id],|r|r.get(0)).unwrap();serde_json::from_str(&s).unwrap() }
fn linked(c:&Connection,id:i64)->(Option<i64>,bool) { c.query_row("SELECT linked_transaction_id,transfer_link_rejected FROM account_transactions WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap() }
fn path(id:i64,price:bool)->String {format!("/tracker/transactions/{id}/{}",if price{"price"}else{"toggle_transfer"})}

#[tokio::test(flavor="current_thread")]
async fn price_invalid_retains_stated_zero_is_a_price_and_blank_clears_only_price() {
    let (_dir,c,owner,app,mut b)=install();
    let id=row(&c,owner,8,"BTC","1","2024-04-01 00:00:00");
    c.execute("UPDATE account_transactions SET manual_values='{\"price\":\"17.0\",\"other\":\"keep\"}' WHERE id=?1",[id]).unwrap();
    for value in ["-1","+1","1e2","NaN","Infinity",".5","1.","1,000","1000000000000000","0.0000000000000000001","1.2.3","abc","\u{a0}12\u{a0}"] {
        assert_eq!(b.send(&app,"PATCH",&path(id,true),Some(&[("price",value)]),Csrf::Header,HEADERS).await.status,422,"{value}");
        assert_eq!(manual(&c,id),json!({"price":"17.0","other":"keep"}));
    }
    assert_eq!(b.send(&app,"PATCH",&path(id,true),Some(&[("price","0")]),Csrf::Header,HEADERS).await.status,200);
    assert_eq!(manual(&c,id),json!({"price":"0.0","other":"keep"}));
    assert_eq!(b.send(&app,"PATCH",&path(id,true),Some(&[("price"," ")]),Csrf::Header,HEADERS).await.status,200);
    assert_eq!(manual(&c,id),json!({"other":"keep"}));
}

#[tokio::test(flavor="current_thread")]
async fn price_refuses_quote_and_opposite_cash_group_but_clear_succeeds() {
    let (_dir,c,owner,app,mut b)=install();
    let id=row(&c,owner,0,"BTC","1","2024-04-01 00:00:00");
    c.execute("UPDATE account_transactions SET quote_currency='USD',quote_amount=12,manual_values='{\"price\":\"17.0\"}' WHERE id=?1",[id]).unwrap();
    assert_eq!(b.send(&app,"PATCH",&path(id,true),Some(&[("price","19")]),Csrf::Header,HEADERS).await.status,422);
    assert_eq!(manual(&c,id),json!({"price":"17.0"}));
    c.execute("UPDATE account_transactions SET quote_currency=NULL,quote_amount=NULL,group_id='g' WHERE id=?1",[id]).unwrap();
    let cash=row(&c,owner,1,"USDC","12","2024-04-01 00:00:00");
    c.execute("UPDATE account_transactions SET group_id='g' WHERE id=?1",[cash]).unwrap();
    assert_eq!(b.send(&app,"PATCH",&path(id,true),Some(&[("price","19")]),Csrf::Header,HEADERS).await.status,422);
    assert_eq!(b.send(&app,"PATCH",&path(id,true),Some(&[("price","")]),Csrf::Header,HEADERS).await.status,200);
    assert_eq!(manual(&c,id),json!({}));
}

#[tokio::test(flavor="current_thread")]
async fn transfer_window_direction_quantity_ownership_and_ambiguity() {
    let (_dir,c,owner,app,mut b)=install();
    c.execute("INSERT INTO users(id,email,encrypted_password,created_at,updated_at) VALUES(999,'foreign@example.test','x','2026-01-01','2026-01-01')",[]).unwrap();
    let withdrawal=row(&c,owner,5,"USD","100","2024-04-01 00:00:00");
    for (who,kind,symbol,amount,at) in [(999,4,"USD","99","2024-04-02 00:00:00"),(owner,4,"EUR","99","2024-04-02 00:00:00"),(owner,4,"USD","101","2024-04-02 00:00:00"),(owner,4,"USD","99","2024-03-31 23:59:59.999999"),(owner,4,"USD","99","2024-04-15 00:00:00.000001"),(owner,5,"USD","99","2024-04-02 00:00:00")] {row(&c,who,kind,symbol,amount,at);}
    assert_eq!(b.send(&app,"PATCH",&path(withdrawal,false),Some(&[]),Csrf::Header,HEADERS).await.status,200);
    assert_eq!(linked(&c,withdrawal),(None,false));
    let deposit=row(&c,owner,4,"USD","99","2024-04-15 00:00:00");
    let ambiguous=row(&c,owner,4,"USD","99","2024-04-01 00:00:00");
    assert_eq!(b.send(&app,"PATCH",&path(withdrawal,false),Some(&[]),Csrf::Header,HEADERS).await.status,200);
    assert_eq!(linked(&c,withdrawal),(None,false));
    c.execute("DELETE FROM account_transactions WHERE id=?1",[ambiguous]).unwrap();
    assert_eq!(b.send(&app,"PATCH",&path(withdrawal,false),Some(&[]),Csrf::Header,HEADERS).await.status,200);
    assert_eq!(linked(&c,withdrawal),(Some(deposit),false));
    assert_eq!(b.send(&app,"PATCH",&path(deposit,false),Some(&[]),Csrf::Header,HEADERS).await.status,200);
    assert_eq!(linked(&c,withdrawal),(None,true));
    c.execute("DELETE FROM account_transactions WHERE id NOT IN (?1,?2)",(withdrawal,deposit)).unwrap();
    assert_eq!(b.send(&app,"PATCH",&path(deposit,false),Some(&[]),Csrf::Header,HEADERS).await.status,200);
    assert_eq!(linked(&c,withdrawal),(Some(deposit),false));
}

#[tokio::test(flavor="current_thread")]
async fn each_write_enforces_csrf_owner_guard_and_atomic_sql_failure() {
    let (_dir,c,owner,app,mut b)=install();
    c.execute("INSERT INTO users(id,email,encrypted_password,created_at,updated_at) VALUES(999,'foreign@example.test','x','2026-01-01','2026-01-01')",[]).unwrap();
    let id=row(&c,owner,5,"USD","100","2024-04-01 00:00:00");
    row(&c,owner,4,"USD","99","2024-04-02 00:00:00");
    let foreign=row(&c,999,5,"USD","100","2024-04-01 00:00:00");
    let wake=std::sync::Arc::new(tokio::sync::Notify::new());app.attach_engine(wake.clone());
    for price in [true,false] {
        let p=path(id,price);let fields=[("price","12")];
        assert_eq!(b.send(&app,"PATCH",&p,Some(&fields),Csrf::None,HEADERS).await.status,302);
        assert_eq!(b.send(&app,"PATCH",&p,Some(&fields),Csrf::Header,&[("origin","https://foreign.invalid")]).await.status,302);
        assert_eq!(b.send(&app,"PATCH",&path(foreign,price),Some(&fields),Csrf::Header,HEADERS).await.status,404);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(10),wake.notified()).await.is_err());
        c.execute("INSERT INTO bots(user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES(?1,1,'Bots::DcaSingleAsset',1,'{}','{}','2026-01-01','2026-01-01')",[owner]).unwrap();
        assert_eq!(b.send(&app,"PATCH",&p,Some(&fields),Csrf::Header,HEADERS).await.status,422);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(10),wake.notified()).await.is_err());
        c.execute("DELETE FROM bots WHERE type='Bots::DcaSingleAsset'",[]).unwrap();
        c.execute_batch("CREATE TRIGGER fail_row BEFORE UPDATE ON account_transactions BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
        assert_eq!(b.send(&app,"PATCH",&p,Some(&fields),Csrf::Header,HEADERS).await.status,500);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(10),wake.notified()).await.is_err());
        c.execute_batch("DROP TRIGGER fail_row;").unwrap();
        assert_eq!(manual(&c,id),json!({}));assert_eq!(linked(&c,id),(None,false));
        assert_eq!(manual(&c,foreign),json!({}));assert_eq!(linked(&c,foreign),(None,false));
    }
}

struct Probe {name:&'static str,owner:i64,heard:tokio::sync::mpsc::UnboundedSender<&'static str>}
impl deltabadger::jobs::Job for Probe {
    fn spec(&self)->deltabadger::jobs::Spec {use deltabadger::jobs::*;Spec{name:self.name,scope:Some(self.owner.to_string()),schedule:None,jitter:schedule::Jitter::NONE,retry:Retry::None,deadline:std::time::Duration::from_secs(5)}}
    fn run<'a>(&'a self,_cx:deltabadger::jobs::Cx<'a>,_wakes:Vec<deltabadger::jobs::Wake>)->deltabadger::jobs::JobFuture<'a>{Box::pin(async move{self.heard.send(self.name).unwrap();deltabadger::jobs::Outcome::Done})}
}

#[tokio::test(flavor="current_thread")]
async fn cancellation_after_the_write_starts_still_commits_and_wakes_the_existing_jobs() {
    use deltabadger::{jobs,tracker::jobs::{TRACKER_LEDGER,PORTFOLIO_BACKFILL}};
    let (dir,c,owner,app,b)=install();
    let id=row(&c,owner,5,"USD","100","2024-04-01 00:00:00");
    let deposit=row(&c,owner,4,"USD","99","2024-04-02 00:00:00");
    let (sent,mut heard)=tokio::sync::mpsc::unbounded_channel();
    let probes:Vec<Box<dyn jobs::Job>>=[TRACKER_LEDGER,PORTFOLIO_BACKFILL].into_iter().map(|name|Box::new(Probe{name,owner,heard:sent.clone()}) as Box<dyn jobs::Job>).collect();
    let scheduler=jobs::Scheduler::new(Connection::open(dir.path().join("production.sqlite3")).unwrap(),app.cipher.clone(),probes,None);
    app.attach_jobs(scheduler.wakers()).unwrap();
    let wake=std::sync::Arc::new(tokio::sync::Notify::new());app.attach_engine(wake.clone());
    let (stop,stopped)=tokio::sync::watch::channel(false);
    let clock=TestClock::at("2026-09-10T12:00:30.123456Z");
    let requests=async {
        for price in [true,false] {
            let (entered,started)=tokio::sync::oneshot::channel();
            let (release,wait)=std::sync::mpsc::sync_channel::<()>(1);
            app.db(move|c|{
                let entered=std::sync::Mutex::new(Some(entered));
                c.create_scalar_function("d5_pause",0,rusqlite::functions::FunctionFlags::SQLITE_UTF8,move|_|{
                    entered.lock().unwrap().take().unwrap().send(()).unwrap();wait.recv().unwrap();Ok(1i64)
                })?;
                c.execute_batch("CREATE TRIGGER pause_row BEFORE UPDATE ON account_transactions BEGIN SELECT d5_pause(); END;")?;Ok(())
            }).await.unwrap();
            let cloned=app.clone();let mut browser=Browser{cookie:b.cookie.clone(),page:b.page.clone()};
            let request=tokio::spawn(async move{browser.send(&cloned,"PATCH",&path(id,price),Some(&[("price","12")]),Csrf::Header,HEADERS).await});
            tokio::time::timeout(std::time::Duration::from_secs(5),started).await.unwrap().unwrap();
            request.abort();assert!(matches!(request.await,Err(e) if e.is_cancelled()));
            release.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2),wake.notified()).await.unwrap();
            assert_eq!(manual(&c,id),json!({"price":"12.0"}));
            if !price{assert_eq!(linked(&c,id),(Some(deposit),false));}
            let mut names=vec![];
            for _ in 0..if price{1}else{2}{names.push(tokio::time::timeout(std::time::Duration::from_secs(2),heard.recv()).await.unwrap().unwrap());}
            names.sort();assert_eq!(names,if price{vec![TRACKER_LEDGER]}else{vec![PORTFOLIO_BACKFILL,TRACKER_LEDGER]});
            assert!(tokio::time::timeout(std::time::Duration::from_millis(20),heard.recv()).await.is_err());
            app.db(|c|{c.execute_batch("DROP TRIGGER pause_row")?;Ok(())}).await.unwrap();
        }
        stop.send(true).unwrap();
    };
    let ((),result)=tokio::join!(requests,scheduler.run(stopped,clock.as_ref()));result.unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn round1_sql_predicate_preserves_sqlite_precision() {
    for from_deposit in [false,true] {
        let (_dir,c,owner,app,mut b)=install();
        let w=row(&c,owner,5,"USD","1","2024-04-01 00:00:00");
        let d=row(&c,owner,4,"USD","1.0000000000000002","2024-04-02 00:00:00");
        assert_eq!(b.send(&app,"PATCH",&path(if from_deposit{d}else{w},false),Some(&[]),Csrf::Header,HEADERS).await.status,200);
        assert_eq!(linked(&c,w),(if from_deposit{Some(d)}else{None},false));
    }
}

#[tokio::test(flavor="current_thread")]
async fn round1_overflow_id_never_selects_maximum_row() {
    for price in [true,false] {
        let (_dir,c,owner,app,mut b)=install();
        let id=row(&c,owner,5,"USD","1","2024-04-01 00:00:00");
        row(&c,owner,4,"USD","1","2024-04-02 00:00:00");
        c.execute("UPDATE account_transactions SET id=?1 WHERE id=?2",(i64::MAX,id)).unwrap();
        let p=format!("/tracker/transactions/9223372036854775808/{}",if price{"price"}else{"toggle_transfer"});
        assert_eq!(b.send(&app,"PATCH",&p,Some(&[("price","12")]),Csrf::Header,HEADERS).await.status,404);
        assert_eq!(manual(&c,i64::MAX),json!({}));assert_eq!(linked(&c,i64::MAX),(None,false));
    }
}

#[tokio::test(flavor="current_thread")]
async fn round1_model_validation_refuses_blank_currency_on_every_save() {
    for price in [true,false] {
        let (_dir,c,owner,app,mut b)=install();
        let id=row(&c,owner,5,"","1","2024-04-01 00:00:00");
        row(&c,owner,4,"","1","2024-04-02 00:00:00");
        assert_eq!(b.send(&app,"PATCH",&path(id,price),Some(&[("price","12")]),Csrf::Header,HEADERS).await.status,422);
        assert_eq!(manual(&c,id),json!({}));assert_eq!(linked(&c,id),(None,false));
    }
}

#[tokio::test(flavor="current_thread")]
async fn round1_turbo_media_type_is_independent_of_accept() {
    for accept in ["application/json","text/html","*/*"] {
        let (_dir,c,owner,app,mut b)=install();
        let id=row(&c,owner,5,"USD","1","2024-04-01 00:00:00");
        row(&c,owner,4,"USD","1","2024-04-02 00:00:00");
        for price in [true,false] {
            let r=b.send(&app,"PATCH",&path(id,price),Some(&[("price","12")]),Csrf::Header,&[("origin","http://localhost:3000"),("accept",accept)]).await;
            assert_eq!(r.status,200);assert_eq!(r.header("content-type"),Some("text/vnd.turbo-stream.html; charset=utf-8"));
        }
    }
}

#[tokio::test(flavor="current_thread")]
async fn round1_historical_user_without_startup_key_runs_real_rebuild() {
    use deltabadger::{jobs,tracker,venue::{alpaca::LiveFactory,http::ScriptedTransport}};
    let (dir,c,owner,app,mut b)=install();
    c.execute("UPDATE api_keys SET status=2",[]).unwrap();
    c.execute("DELETE FROM bots",[]).unwrap();
    let id=row(&c,owner,8,"USD","1","2024-04-01 00:00:00");
    let api:std::rc::Rc<Option<jobs::data_api::DataApi<ScriptedTransport>>>=std::rc::Rc::new(None);
    let clock=TestClock::at("2026-09-10T12:00:30.123456Z");
    let wall:tracker::jobs::Wall=std::sync::Arc::new(||"2026-09-10T12:00:30.123456Z".parse().unwrap());
    let factory=LiveFactory::new();
    let registered=tracker::jobs::register(&c,&factory,api.clone(),wall.clone()).unwrap();
    assert!(registered.is_empty(),"regression requires no startup consumer");
    let scheduler=jobs::Scheduler::new(Connection::open(dir.path().join("production.sqlite3")).unwrap(),app.cipher.clone(),registered,None)
        .with_resolver(jobs::resolve::all(factory,api,wall));
    app.attach_jobs(scheduler.wakers()).unwrap();
    let (stop,stopped)=tokio::sync::watch::channel(false);
    let requests=async{
        assert_eq!(b.send(&app,"PATCH",&path(id,true),Some(&[("price","1")]),Csrf::Header,HEADERS).await.status,200);
        tokio::time::timeout(std::time::Duration::from_secs(5),async{
            loop{
                let state=jobs::state::read(&c,tracker::jobs::TRACKER_LEDGER,Some(&owner.to_string())).unwrap();
                assert!(state.last_error.is_none(),"{state:?}");
                if state.last_success_at.is_some(){break}
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert_eq!(manual(&c,id),json!({"price":"1.0"}));
        stop.send(true).unwrap();
    };
    let ((),result)=tokio::join!(requests,scheduler.run(stopped,clock.as_ref()));result.unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn round1_scheduler_resolves_non_tracker_jobs_and_reuses_consumer() {
    use deltabadger::jobs;
    let (dir,_c,owner,app,_b)=install();
    let (sent,mut heard)=tokio::sync::mpsc::unbounded_channel();
    let count=std::rc::Rc::new(std::cell::Cell::new(0));let resolved=count.clone();
    let scheduler=jobs::Scheduler::new(Connection::open(dir.path().join("production.sqlite3")).unwrap(),app.cipher.clone(),vec![],None)
        .with_resolver(Box::new(move|name,scope|{
            assert_eq!(name,"other_job");assert_eq!(scope,Some(owner.to_string().as_str()));
            resolved.set(resolved.get()+1);Ok(Box::new(Probe{name:"other_job",owner,heard:sent.clone()}))
        }));
    let wakers=scheduler.wakers();let (stop,stopped)=tokio::sync::watch::channel(false);
    let clock=TestClock::at("2026-09-10T12:00:30.123456Z");
    let requests=async{
        for _ in 0..2{
            wakers.wake("other_job",Some(&owner.to_string()),None);
            assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(2),heard.recv()).await.unwrap(),Some("other_job"));
        }
        assert_eq!(count.get(),1);stop.send(true).unwrap();
    };
    let ((),result)=tokio::join!(requests,scheduler.run(stopped,clock.as_ref()));result.unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn round1_model_validation_checks_association_unique_id_and_proposed_link() {
    for defect in ["exchange","tx_id","owner","currency","direction","time","amount","missing"] {
        let (_dir,c,owner,app,mut b)=install();
        let w=row(&c,owner,5,"USD","1","2024-04-01 00:00:00");
        let d=row(&c,owner,4,"USD","1","2024-04-02 00:00:00");
        c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        c.execute("UPDATE account_transactions SET linked_transaction_id=?1 WHERE id=?2",(d,w)).unwrap();
        match defect {
            "exchange"=>{c.execute("UPDATE account_transactions SET exchange_id=999 WHERE id=?1",[w]).unwrap();},
            "tx_id"=>{c.execute_batch("DROP INDEX index_account_transactions_on_user_exchange_tx_id; UPDATE account_transactions SET tx_id='duplicate'").unwrap();},
            "owner"=>{c.execute("UPDATE account_transactions SET user_id=999 WHERE id=?1",[d]).unwrap();},
            "currency"=>{c.execute("UPDATE account_transactions SET base_currency='EUR' WHERE id=?1",[d]).unwrap();},
            "direction"=>{c.execute("UPDATE account_transactions SET entry_type=5 WHERE id=?1",[d]).unwrap();},
            "time"=>{c.execute("UPDATE account_transactions SET transacted_at='2024-03-01 00:00:00' WHERE id=?1",[d]).unwrap();},
            "amount"=>{c.execute("UPDATE account_transactions SET base_amount=2 WHERE id=?1",[d]).unwrap();},
            "missing"=>{c.execute("UPDATE account_transactions SET linked_transaction_id=999 WHERE id=?1",[w]).unwrap();},
            _=>unreachable!(),
        }
        let before=linked(&c,w);
        assert_eq!(b.send(&app,"PATCH",&path(w,true),Some(&[("price","12")]),Csrf::Header,HEADERS).await.status,422,"{defect}");
        assert_eq!(manual(&c,w),json!({}));assert_eq!(linked(&c,w),before);
    }
}
