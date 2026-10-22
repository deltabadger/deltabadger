//! N-run equivalence, interrupted-page replay, cursor refusal and retry pacing for Bybit history.
mod common;
use deltabadger::crypto::Credentials;
use deltabadger::engine::FixedClock;
use deltabadger::jobs::Db;
use deltabadger::sync::{bybit_ledger, continuation::{Run, Stopwatch}};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::{cell::{Cell, RefCell}, rc::Rc, time::Duration};
const NOW: &str = "2026-09-20T02:00:00Z";
fn now() -> chrono::DateTime<chrono::Utc> { NOW.parse().unwrap() }
fn creds() -> Credentials { Credentials { redaction_values: vec![], key: "PKTEST".into(), secret: "paper-secret".into(), passphrase: Some("paper".into()) } }
fn install() -> (tempfile::TempDir, Db, i64) {
    let (d,o,s) = common::install_alpaca();
    o.primary.execute("UPDATE exchanges SET type='Exchanges::Bybit', name='Bybit' WHERE id=?1", [s.exchange_id]).unwrap();
    o.primary.execute("UPDATE tickers SET ticker='BTCUSDT', base='BTC', quote='USDT' WHERE id=?1", [s.ticker_id]).unwrap();
    (d, Db::new(o.primary, common::seed::cipher()), s.api_key_id)
}
#[derive(Default)]
struct Watch(Cell<Duration>);
impl Stopwatch for Watch { fn elapsed(&self) -> Duration { self.0.get() } }
#[derive(Clone)]
struct Server { watch: Rc<Watch>, requests: Rc<RefCell<Vec<String>>>, slow: bool, stuck: bool, cut: Rc<Cell<Option<usize>>>, notify: Rc<tokio::sync::Notify> }
impl Server {
    fn new(slow: bool) -> Self { Self { watch: Rc::default(), requests: Rc::default(), slow, stuck: false, cut: Rc::default(), notify: Rc::default() } }
}
impl Transport for Server {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.watch.0.set(self.watch.0.get() + if self.slow { Duration::from_secs(60) } else { Duration::ZERO });
        let n = self.requests.borrow().len() + 1;
        self.requests.borrow_mut().push(deltabadger::venue::http::request_key(r));
        if self.cut.get() == Some(n) { self.notify.notify_one(); std::future::pending::<()>().await; }
        let q = |k| r.query.iter().find(|(name,_)| *name == k).map(|(_,v)| v.as_str());
        let start: i64 = q("startTime").unwrap().parse().unwrap();
        let end: i64 = q("endTime").unwrap().parse().unwrap();
        let fills = r.path == bybit_ledger::FILLS_PATH;
        assert!(end - start <= if fills { 7 } else { 29 } * 86_400_000);
        assert!(end <= now().timestamp_millis());
        if fills { assert_eq!(q("category"), Some("spot")); }
        if r.path == bybit_ledger::WITHDRAWALS_PATH { assert_eq!(q("withdrawType"), Some("2")); }
        // Each window has two pages, with a duplicate raw row across their edge; one id-less fill tests persistent dedup.
        let second = q("cursor").is_some();
        let id = format!("{}-{start}", r.path);
        let raw = if fills { json!({"symbol":"BTCUSDT","side":"Buy","execQty":"0.001","execValue":"60","execFee":"-0.06","feeCurrency":"USDT","execTime":start.to_string()}) }
            else { json!({"coin":"USDT","amount":"10","txID":id,"status":if r.path == bybit_ledger::WITHDRAWALS_PATH {json!("success")} else if r.path == bybit_ledger::INTERNAL_PATH {json!(2)} else {json!(70012)},
                          "successAt":start.to_string(),"createdTime":start.to_string(),"updateTime":start.to_string(),"withdrawFee":"1"}) };
        let key = if fills { "list" } else { "rows" };
        Ok(HttpResponse { status:200, body:json!({"retCode":0,"result":{key:[raw],"nextPageCursor":if !second || self.stuck {"page-2"} else {""}}}).to_string() })
    }
}
async fn stored(db: &Db, key: i64) -> (Vec<Value>, Option<String>) {
    db.run(move |c,_| {
        let mut s=c.prepare("SELECT entry_type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,tx_id,transacted_at,raw_data FROM account_transactions ORDER BY id").unwrap();
        let rows=s.query_map([],|r|Ok(json!([r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,f64>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<f64>>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<f64>>(6)?,r.get::<_,Option<String>>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?]))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        let at=c.query_row("SELECT last_synced_at FROM api_keys WHERE id=?1",[key],|r|r.get(0)).unwrap(); Ok((rows,at))
    }).await.unwrap()
}
async fn import(db: &Db, key: i64, server: &Server) -> usize {
    for n in 1..500 {
        server.watch.0.set(Duration::ZERO);
        let run=Run { deadline:Duration::from_secs(if server.slow { 500 } else { 3600 }),chain_runs:1000,stopwatch:&*server.watch };
        let (credentials,clock)=(creds(),FixedClock(now()));
        let out=tokio::select! { biased; _=server.notify.notified()=>None,
            out=bybit_ledger::sync_within(db,server.clone(),key,&credentials,&clock,&run)=>Some(out) };
        if let Some(out)=out {
            if out.unwrap().unwrap().complete { return n; }
            assert!(stored(db,key).await.1.is_none(), "an incomplete import cannot move its watermark");
        }
    }
    panic!("import did not finish")
}
#[tokio::test(flavor="current_thread")]
async fn n_runs_and_an_interrupted_page_equal_one_complete_walk() {
    let (_d,db,key)=install(); let one=Server::new(false);
    assert_eq!(import(&db,key,&one).await,1);
    let expected=stored(&db,key).await;
    assert_eq!((expected.0[0][1].as_str(), expected.0[0][3].as_str()), (Some("BTC"), Some("USDT")));
    assert_eq!(expected.0.len(),105 + 3 * 26, "one row per seven-day fill window and 29-day transfer source/window");
    assert_eq!(one.requests.borrow().len(),2 * (105 + 3 * 26), "pages, not windows, count against admission");
    let (_d2,db2,key2)=install(); let many=Server::new(true);
    many.cut.set(Some(3));
    assert!(import(&db2,key2,&many).await > 100);
    assert_eq!(stored(&db2,key2).await,expected);
}
#[tokio::test(flavor="current_thread")]
async fn a_repeated_cursor_fails_without_advancing_the_watermark() {
    let (_d,db,key)=install(); let mut server=Server::new(false); server.stuck=true;
    let out=bybit_ledger::sync(&db,server,key,&creds(),&FixedClock(now())).await.unwrap();
    assert!(out.is_err()); assert!(stored(&db,key).await.1.is_none());
}
