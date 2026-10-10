//! Tracker sync wakes the existing registered jobs for this owner's reading keys.
mod common;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::{jobs::{self, Job, JobFuture, Spec, Retry}, web::{csrf, session}};

fn browser(app: &deltabadger::web::App, owner: i64, hash: &str) -> Browser {
    let token = csrf::new_token();
    let data = session::SessionData { user: Some((owner,hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
    Browser { cookie: Some(session::seal(&app.keys.session,&data,app.now())), page:Some(format!("<meta name=\"csrf-token\" content=\"{}\">",csrf::masked(&token))) }
}
const HEADERS: &[(&str,&str)] = &[("origin","http://localhost:3000"),("accept","text/vnd.turbo-stream.html, text/html")];

#[tokio::test(flavor="current_thread")]
async fn sync_no_keys_has_no_content_and_never_changes_credentials() {
    let (dir,opened,s) = common::install_alpaca();
    let c = opened.primary;
    c.execute("UPDATE users SET confirmed_at=created_at,setup_completed=1",[]).unwrap();
    c.execute("UPDATE api_keys SET status=2",[]).unwrap();
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[s.user_id],|r|r.get(0)).unwrap();
    let app = web::app(dir.path(),web::SECRET,TestClock::at("2026-09-10T12:00:30.123456Z"));
    let mut b = browser(&app,s.user_id,&hash);
    for (status,kind) in [(2,0),(1,1)] {
        c.execute("UPDATE api_keys SET status=?1,key_type=?2",(status,kind)).unwrap();
        let response = b.send(&app,"POST","/tracker/sync",Some(&[]),Csrf::Header,HEADERS).await;
        assert_eq!(response.status,204);
        assert!(response.body.is_empty());
        assert_eq!(c.query_row("SELECT status FROM api_keys WHERE id=?1",[s.api_key_id],|r|r.get::<_,i64>(0)).unwrap(),status);
    }
}

#[tokio::test(flavor="current_thread")]
async fn sync_renders_the_real_progress_fragment() {
    let (dir,opened,s) = common::install_alpaca();
    let c = opened.primary;
    c.execute("UPDATE users SET confirmed_at=created_at,setup_completed=1",[]).unwrap();
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[s.user_id],|r|r.get(0)).unwrap();
    let app = web::app(dir.path(),web::SECRET,TestClock::at("2026-09-10T12:00:30.123456Z"));
    let mut b = browser(&app,s.user_id,&hash);
    let response = b.send(&app,"POST","/tracker/sync",Some(&[]),Csrf::Header,HEADERS).await;
    assert_eq!(response.status,200);
    assert_eq!(response.header("content-type"),Some("text/vnd.turbo-stream.html; charset=utf-8"));
    assert!(response.body.starts_with("<turbo-stream action=\"append\" target=\"flash\"><template>"));
    assert!(response.body.contains("id=\"sync-progress\""));
    assert!(response.body.contains("Alpaca"));
    assert!(!response.body.contains("test-key"));
}

// Exercise the real attributed jobs through a local, empty-account protocol adapter.
#[derive(Clone)] struct ProbeVenue;
struct ProbeTransport;
impl deltabadger::venue::http::Transport for ProbeTransport {
    async fn send(&self,r:&deltabadger::venue::http::HttpRequest)->Result<deltabadger::venue::http::HttpResponse,deltabadger::venue::http::TransportError> {
        let body=match r.path.as_str(){
            "/v2/account/activities"|"/v2/positions"=>"[]",
            "/v2/account"=>r#"{"status":"ACTIVE","cash":"0","currency":"USD"}"#,
            path=>panic!("unexpected probe request: {path}"),
        };
        Ok(deltabadger::venue::http::HttpResponse{status:200,body:body.into()})
    }
}
impl deltabadger::sync::jobs::Connect for ProbeVenue {
    type T=ProbeTransport;
    fn connect(&self,_:&deltabadger::crypto::Credentials)->deltabadger::venue::alpaca::AlpacaVenue<Self::T>{
        deltabadger::venue::alpaca::AlpacaVenue::new(ProbeTransport,deltabadger::venue::alpaca::Urls::for_passphrase(None))
    }
}
struct Probe { name: &'static str, key: i64, inner:Box<dyn Job>, heard: tokio::sync::mpsc::UnboundedSender<(&'static str,i64)> }
impl Job for Probe {
    fn spec(&self) -> Spec { Spec { name:self.name, scope:Some(self.key.to_string()),schedule:None,jitter:jobs::schedule::Jitter::NONE,retry:Retry::None,deadline:std::time::Duration::from_secs(5) } }
    fn run<'a>(&'a self,cx:jobs::Cx<'a>,wakes:Vec<jobs::Wake>)->JobFuture<'a> {
        Box::pin(async move { self.run_attributed(cx,wakes).await.value })
    }
    fn run_attributed<'a>(&'a self,cx:jobs::Cx<'a>,wakes:Vec<jobs::Wake>)->jobs::AttributedJobFuture<'a> {
        Box::pin(async move { self.heard.send((self.name,self.key)).unwrap();self.inner.run_attributed(cx,wakes).await })
    }
}

#[tokio::test(flavor="current_thread")]
async fn sync_selects_owned_reading_keys_and_wakes_both_jobs_only_after_guard_and_csrf() {
    let (dir,opened,s) = common::install_alpaca();
    let c = opened.primary;
    c.execute("UPDATE users SET confirmed_at=created_at,setup_completed=1",[]).unwrap();
    c.execute("INSERT INTO users(id,email,encrypted_password,created_at,updated_at) VALUES(999,'foreign@example.test','x','2026-01-01','2026-01-01')",[]).unwrap();
    for (id,user,kind,status) in [(100,s.user_id,2,1),(101,s.user_id,1,1),(102,999,0,1),(103,s.user_id,0,2)] {
        c.execute("INSERT INTO api_keys(id,user_id,exchange_id,key,secret,key_type,status,created_at,updated_at) SELECT ?1,?2,exchange_id,key,secret,?3,?4,created_at,updated_at FROM api_keys WHERE id=?5",(id,user,kind,status,s.api_key_id)).unwrap();
    }
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[s.user_id],|r|r.get(0)).unwrap();
    let clock = TestClock::at("2026-09-10T12:00:30.123456Z");
    let app = web::app(dir.path(),web::SECRET,clock.clone());
    let mut b = browser(&app,s.user_id,&hash);
    let (sent,mut heard) = tokio::sync::mpsc::unbounded_channel();
    let mut probes: Vec<Box<dyn Job>> = vec![];
    for key in [s.api_key_id,100,101,102,103] {
        for name in [deltabadger::sync::jobs::LEDGER_SYNC,deltabadger::sync::jobs::BALANCE_SYNC] {
            let inner:Box<dyn Job>=if name==deltabadger::sync::jobs::LEDGER_SYNC {
                Box::new(deltabadger::sync::jobs::LedgerSync::new(ProbeVenue,key))
            }else{
                Box::new(deltabadger::sync::jobs::BalanceSync::new(ProbeVenue,std::rc::Rc::new(deltabadger::sync::balances::NoPrices),key))
            };
            probes.push(Box::new(Probe { name,key,inner,heard:sent.clone() }));
        }
    }
    let scheduler = jobs::Scheduler::new(rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap(),app.cipher.clone(),probes,None);
    app.attach_jobs(scheduler.wakers()).unwrap();
    let (stop,stopped) = tokio::sync::watch::channel(false);
    let requests = async {
        assert_eq!(b.send(&app,"POST","/tracker/sync",Some(&[]),Csrf::None,HEADERS).await.status,302);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(40),heard.recv()).await.is_err());
        c.execute("INSERT INTO bots(user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES(?1,?2,'Bots::DcaSingleAsset',1,'{}','{}','2026-01-01','2026-01-01')",(s.user_id,s.exchange_id)).unwrap();
        assert_eq!(b.send(&app,"POST","/tracker/sync",Some(&[]),Csrf::Header,HEADERS).await.status,422);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(40),heard.recv()).await.is_err());
        c.execute("DELETE FROM bots",[]).unwrap();
        assert_eq!(b.send(&app,"POST","/tracker/sync",Some(&[]),Csrf::Header,HEADERS).await.status,200);
        let mut jobs = vec![];
        for _ in 0..2 { jobs.push(tokio::time::timeout(std::time::Duration::from_secs(2),heard.recv()).await.unwrap().unwrap()); }
        jobs.sort();
        assert_eq!(jobs,vec![("balance_sync",s.api_key_id),("ledger_sync",s.api_key_id)]);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(40),heard.recv()).await.is_err());
        stop.send(true).unwrap();
    };
    let ((),run) = tokio::join!(requests,scheduler.run(stopped,clock.as_ref()));
    run.unwrap();
}
