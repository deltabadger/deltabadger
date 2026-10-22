//! One run, checkpoint after every page, and a cancelled run must import the same Coinbase history.
mod common;
use deltabadger::{
    crypto::Credentials,
    engine::FixedClock,
    jobs::Db,
    sync::{
        coinbase_ledger,
        continuation::{Run, Stopwatch},
    },
    venue::http::{HttpRequest, HttpResponse, Transport, TransportError},
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
const NOW: &str = "2026-09-20T02:00:00Z";
#[derive(Default)]
struct Watch(Cell<Duration>);
impl Stopwatch for Watch {
    fn elapsed(&self) -> Duration {
        self.0.get()
    }
}
#[derive(Clone)]
struct Server {
    watch: Rc<Watch>,
    latency: Duration,
    log: Rc<RefCell<Vec<String>>>,
    cut: Rc<Cell<bool>>,
    notify: Rc<tokio::sync::Notify>,
    adjusted: bool,
}
impl Transport for Server {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.watch.0.set(self.watch.0.get() + self.latency);
        self.log.borrow_mut().push(r.path.clone());
        if self.cut.replace(false) {
            self.notify.notify_one();
            std::future::pending::<()>().await;
        }
        let q = |k| {
            r.query
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.as_str())
        };
        let fill = |id, quote| json!({"trade_id":id,"entry_id":format!("e{id}"),"trade_type":"FILL","product_id":"ETH-EUR","side":"BUY","price":"2385.55","size":"9.947791211","commission":"0.049738956055","size_in_quote":quote,"trade_time":"2026-09-19T01:00:00Z"});
        let txn = |id| json!({"id":id,"type":"receive","amount":{"amount":"1","currency":"ETH"},"created_at":"2026-09-19T02:00:00Z"});
        let body = match r.path.as_str() {
            coinbase_ledger::FILLS if q("cursor").is_none() => {
                json!({"fills":[fill("1",true)],"cursor":"next"})
            }
            coinbase_ledger::FILLS if self.adjusted => {
                let mut adjusted = fill("1", true);
                adjusted["trade_type"] = json!("ADJUSTMENT");
                adjusted["entry_id"] = json!("adjusted-1");
                json!({"fills":[fill("2",false),adjusted]})
            }
            coinbase_ledger::FILLS => json!({"fills":[fill("1",true),fill("2",false)]}),
            coinbase_ledger::ACCOUNTS if q("cursor").is_none() => {
                json!({"accounts":[{"uuid":"a"}],"cursor":"next","has_next":true})
            }
            coinbase_ledger::ACCOUNTS => json!({"accounts":[{"uuid":"b"}],"has_next":false}),
            "/v2/accounts/a/transactions" if q("starting_after").is_none() => {
                json!({"data":[txn("d1")],"pagination":{"next_starting_after":"next"}})
            }
            "/v2/accounts/a/transactions" => json!({"data":[txn("d2")]}),
            "/v2/accounts/b/transactions" => json!({"data":[txn("d3")]}),
            p => panic!("unscripted {p}"),
        };
        Ok(HttpResponse {
            status: 200,
            body: body.to_string(),
        })
    }
}
fn install() -> (tempfile::TempDir, Db, i64) {
    let (d, o, s) = common::install_alpaca();
    o.primary
        .execute(
            "UPDATE exchanges SET type='Exchanges::Coinbase',name='Coinbase' WHERE id=?1",
            [s.exchange_id],
        )
        .unwrap();
    (d, Db::new(o.primary, common::seed::cipher()), s.api_key_id)
}
fn server(latency: Duration) -> Server {
    Server {
        watch: Rc::default(),
        latency,
        log: Rc::default(),
        cut: Rc::default(),
        notify: Rc::default(),
        adjusted: false,
    }
}
async fn once(db: &Db, key: i64, v: &Server, deadline: Duration) -> Option<bool> {
    v.watch.0.set(Duration::ZERO);
    let run = Run {
        deadline,
        chain_runs: 2,
        stopwatch: &*v.watch,
    };
    let c = Credentials {
        key: "PKTEST".into(),
        secret: "paper-secret".into(),
        passphrase: Some("paper".into()),
        redaction_values: vec![],
    };
    let clock = FixedClock(NOW.parse().unwrap());
    tokio::select! {biased;_ = v.notify.notified()=>None,o=coinbase_ledger::sync_within(db,v.clone(),key,&c,&clock,&run)=>Some(match o.unwrap(){Ok(out)=>out.complete,Err(f)=>{assert!(f.error.starts_with("the ledger import is still running after"),"{f:?}");false}})}
}
async fn import(db: &Db, key: i64, v: &Server, deadline: Duration) -> usize {
    for i in 1..=20 {
        if once(db, key, v, deadline).await == Some(true) {
            return i;
        }
    }
    panic!("no progress")
}
async fn rows(db: &Db, key: i64) -> Value {
    db.run(move|c,_|{
    let mut s=c.prepare("SELECT tx_id,quote(base_amount),quote(quote_amount),raw_data FROM account_transactions ORDER BY id").unwrap();
    let rows=s.query_map([],|r|Ok(json!([r.get::<_,Option<String>>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?]))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    let at:Option<String>=c.query_row("SELECT last_synced_at FROM api_keys WHERE id=?1",[key],|r|r.get(0)).unwrap();Ok(json!([rows,at]))}).await.unwrap()
}
#[tokio::test(flavor = "current_thread")]
async fn coinbase_many_runs_and_replayed_cut_equal_one_run() {
    let (_d, db, key) = install();
    let v = server(Duration::ZERO);
    assert_eq!(import(&db, key, &v, Duration::from_secs(3600)).await, 1);
    let expected = rows(&db, key).await;
    assert_eq!(expected[0].as_array().unwrap().len(), 5);
    let (_d2, db2, key2) = install();
    let v2 = server(Duration::from_secs(60));
    assert!(import(&db2, key2, &v2, Duration::from_secs(200)).await >= 7);
    assert_eq!(rows(&db2, key2).await, expected);
    let (_d3, db3, key3) = install();
    let v3 = server(Duration::from_secs(60));
    v3.cut.set(true);
    assert_eq!(once(&db3, key3, &v3, Duration::from_secs(200)).await, None);
    import(&db3, key3, &v3, Duration::from_secs(200)).await;
    assert_eq!(rows(&db3, key3).await, expected);
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_adjusted_fill_on_a_later_page_does_not_hide_the_original() {
    async fn seeded(has_entry: bool) -> (tempfile::TempDir, Db, i64) {
        let (d, db, key) = install();
        db.run(move |c,_| {
            let mut raw = json!({"trade_id":"1","trade_type":"ADJUSTMENT","product_id":"ETH-EUR","side":"BUY","price":"2385.55","size":"9.947791211","commission":"0.049738956055","size_in_quote":true,"trade_time":"2026-09-19T01:00:00Z"});
            if has_entry { raw["entry_id"] = json!("adjusted-1"); }
            let normalized = coinbase_ledger::amounts(&deltabadger::sync::activities::Raw::from_value(raw.clone())).unwrap().0.to_s_f();
            let user: i64 = c.query_row("SELECT user_id FROM api_keys WHERE id=?1", [key], |r|r.get(0)).unwrap();
            let exchange: i64 = c.query_row("SELECT exchange_id FROM api_keys WHERE id=?1", [key], |r|r.get(0)).unwrap();
            c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,tx_id,raw_data,transacted_at,created_at,updated_at) VALUES(?1,?2,0,'ETH',?3,'EUR',9.947791211,'1',?4,'2026-09-19 01:00:00','2026-09-19','2026-09-19')",rusqlite::params![user,exchange,normalized,raw.to_string()]).unwrap();
            Ok(())
        }).await.unwrap();
        (d, db, key)
    }
    for has_entry in [true, false] {
        let (_d, db, key) = seeded(has_entry).await;
        let mut v = server(Duration::ZERO);
        v.adjusted = true;
        import(&db, key, &v, Duration::from_secs(3600)).await;
        let expected = rows(&db, key).await;
        assert_eq!(
            expected[0].as_array().unwrap().len(),
            6,
            "both the original and adjusted fill must survive: {expected}"
        );
        assert_eq!(
            expected[0][0][0], "adjusted-1",
            "the stored adjusted row is rekeyed in place: {expected}"
        );
        assert_eq!(
            expected[0][1][0], "1",
            "the earlier page's original fill owns trade_id"
        );
        let (_d2, db2, key2) = seeded(has_entry).await;
        let mut v2 = server(Duration::from_secs(60));
        v2.adjusted = true;
        import(&db2, key2, &v2, Duration::from_secs(200)).await;
        assert_eq!(rows(&db2, key2).await, expected);
    }
}
