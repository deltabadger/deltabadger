//! Current fiat conversion uses the hosted feed, including real HTTP and guarded writes.
mod common;
use common::web::{self,Browser,Csrf,TestClock};
use deltabadger::{app_config,web::{App,csrf,session}};
use rusqlite::Connection;
use serde_json::{json,Value};
use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};

struct Feed {url:String,hits:Arc<AtomicUsize>,body:Arc<std::sync::Mutex<Value>>,task:tokio::task::JoinHandle<()>}
impl Drop for Feed {fn drop(&mut self){self.task.abort();}}
async fn feed()->Feed {
    let hits=Arc::new(AtomicUsize::new(0));let body=Arc::new(std::sync::Mutex::new(json!({"data":{"usd":{"value":100.0},"eur":{"value":80.0},"gbp":{"value":50.0},"chf":{"value":90.0},"pln":{"value":400.0}}})));
    let (h,b)=(hits.clone(),body.clone());
    let router=axum::Router::new().route("/api/v1/exchange_rates",axum::routing::get(move|headers:axum::http::HeaderMap|{let(h,b)=(h.clone(),b.clone());async move {
        assert_eq!(headers.get("authorization").unwrap(),"Bearer fx-secret-placeholder");
        h.fetch_add(1,Ordering::SeqCst);([(axum::http::header::CONTENT_TYPE,"application/json")],b.lock().unwrap().to_string())
    }}));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}",listener.local_addr().unwrap());
    let task=tokio::spawn(async move{axum::serve(listener,router).await.unwrap()});Feed{url,hits,body,task}
}
fn install(url:&str,currency:&str)->(tempfile::TempDir,Connection,i64,App,Browser,Arc<TestClock>) {
    let(dir,opened,seed)=common::install_alpaca();let c=opened.primary;let owner=seed.user_id;
    c.execute("UPDATE users SET confirmed_at=created_at,setup_completed=1,display_currency=?1",[currency]).unwrap();
    let clock=TestClock::at("2026-09-10T12:00:30.123456Z");
    for (k,v) in [("market_data_provider","deltabadger"),("market_data_url",url),("market_data_token","fx-secret-placeholder")] {app_config::set_plain(&c,k,v,web::at("2026-01-01T00:00:00Z")).unwrap();}
    let app=web::app(dir.path(),web::SECRET,clock.clone());
    let hash:String=c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[owner],|r|r.get(0)).unwrap();
    let token=csrf::new_token();let data=session::SessionData{user:Some((owner,hash.chars().take(29).collect())),csrf:Some(token.clone()),..Default::default()};
    let browser=Browser{cookie:Some(session::seal(&app.keys.session,&data,app.now())),page:Some(format!("<meta name=\"csrf-token\" content=\"{}\">",csrf::masked(&token)))};
    c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,created_at,updated_at) VALUES(?1,1,8,'BTC',1,'2024-04-01 00:00:00','2024-04-01 00:00:00','2024-04-01 00:00:00')",[owner]).unwrap();
    let id=c.last_insert_rowid();(dir,c,id,app,browser,clock)
}
async fn price(app:&App,b:&mut Browser,id:i64,value:&str)->web::Answer {
    b.send(app,"PATCH",&format!("/tracker/transactions/{id}/price"),Some(&[("price",value)]),Csrf::Header,&[("origin","http://localhost:3000"),("accept","text/vnd.turbo-stream.html")]).await
}
fn manual(c:&Connection,id:i64)->Value {let s:String=c.query_row("SELECT manual_values FROM account_transactions WHERE id=?1",[id],|r|r.get(0)).unwrap();serde_json::from_str(&s).unwrap()}

#[tokio::test(flavor="current_thread")]
async fn current_fiat_price_is_saved_in_usd_and_rendered_in_the_requested_currency() {
    let feed=feed().await;
    for(currency,input,unit) in [("EUR","8","€"),("GBP","5","£"),("CHF","9","Fr."),("PLN","40","zł")] {
        let(_dir,c,id,app,mut b,_clock)=install(&feed.url,currency);
        let answer=price(&app,&mut b,id,input).await;
        assert_eq!(answer.status,200,"{}",answer.body);assert_eq!(manual(&c,id),json!({"price":"10.0"}),"{currency}");
        assert!(answer.body.contains(unit),"{}",answer.body);assert!(!answer.body.contains("fx-secret-placeholder"));
    }
    assert_eq!(feed.hits.load(Ordering::SeqCst),4);
}

#[tokio::test(flavor="current_thread")]
async fn warm_rates_expire_and_missing_rates_refuse_then_retry() {
    let feed=feed().await;let(_dir,c,id,app,mut b,clock)=install(&feed.url,"EUR");
    assert_eq!(price(&app,&mut b,id,"8").await.status,200);assert_eq!(manual(&c,id),json!({"price":"10.0"}));
    *feed.body.lock().unwrap()=json!({"data":{"usd":{"value":100.0}}});
    clock.set(app.now()+chrono::Duration::hours(11));
    assert_eq!(price(&app,&mut b,id,"16").await.status,200);assert_eq!(manual(&c,id),json!({"price":"20.0"}));assert_eq!(feed.hits.load(Ordering::SeqCst),1);
    clock.set(app.now()+chrono::Duration::hours(1));
    let answer=price(&app,&mut b,id,"16").await;assert_eq!(answer.status,422);assert!(answer.body.contains("Currency conversion unavailable"));assert_eq!(manual(&c,id),json!({"price":"20.0"}));assert_eq!(feed.hits.load(Ordering::SeqCst),2);
    *feed.body.lock().unwrap()=json!({"data":{"usd":{"value":100.0},"eur":{"value":80.0}}});
    clock.set(app.now()+chrono::Duration::seconds(299));assert_eq!(price(&app,&mut b,id,"8").await.status,422);assert_eq!(manual(&c,id),json!({"price":"20.0"}));assert_eq!(feed.hits.load(Ordering::SeqCst),2);
    clock.set(app.now()+chrono::Duration::seconds(1));assert_eq!(price(&app,&mut b,id,"8").await.status,200);assert_eq!(manual(&c,id),json!({"price":"10.0"}));assert_eq!(feed.hits.load(Ordering::SeqCst),3);
}

#[tokio::test(flavor="current_thread")]
async fn invalid_auth_owner_and_overflow_never_fetch_and_usd_needs_no_feed() {
    let feed=feed().await;let(_dir,c,id,app,mut b,_)=install(&feed.url,"EUR");
    let p=format!("/tracker/transactions/{id}/price");
    assert_eq!(b.send(&app,"PATCH",&p,Some(&[("price","8")]),Csrf::None,&[]).await.status,302);
    assert_eq!(price(&app,&mut b,i64::MAX,"8").await.status,404);
    assert_eq!(b.send(&app,"PATCH","/tracker/transactions/9223372036854775808/price",Some(&[("price","8")]),Csrf::Header,&[]).await.status,404);
    assert_eq!(feed.hits.load(Ordering::SeqCst),0);
    c.execute("UPDATE users SET display_currency='USD'",[]).unwrap();
    assert_eq!(price(&app,&mut b,id,"8").await.status,200);assert_eq!(manual(&c,id),json!({"price":"8.0"}));assert_eq!(feed.hits.load(Ordering::SeqCst),0);
}

#[tokio::test(flavor="current_thread")]
async fn whole_number_feed_must_not_store_eur_as_usd() {
    let feed=feed().await;
    *feed.body.lock().unwrap()=json!({"data":{"usd":{"value":100},"eur":{"value":80}}});
    let(_dir,c,id,app,mut b,_)=install(&feed.url,"EUR");
    let answer=price(&app,&mut b,id,"80").await;
    assert_eq!(answer.status,200);
    // 80 EUR / (80 EUR per BTC / 100 USD per BTC) = 100 USD.
    assert_eq!(manual(&c,id),json!({"price":"100.0"}),"whole-number FX silently misstates the saved USD price");
}

#[tokio::test(flavor="current_thread")]
async fn unusable_rates_refuse_without_rows_or_jobs_and_clearing_still_works() {
    let feed=feed().await;
    for rates in [json!({"usd":{"value":100},"eur":{"value":0}}),json!({"usd":{"value":0},"eur":{"value":80}}),json!({"usd":{"value":100},"eur":{"value":-80}}),json!({"usd":{"value":100}}),json!({"usd":{"value":100},"eur":{"value":"fx-secret-placeholder"}})] {
        *feed.body.lock().unwrap()=json!({"data":rates});
        let(_dir,c,id,app,mut b,_)=install(&feed.url,"EUR");
        c.execute("UPDATE account_transactions SET manual_values='{\"price\":\"12.0\"}' WHERE id=?1",[id]).unwrap();
        let before=rows(&c);
        let answer=price(&app,&mut b,id,"80").await;
        assert_eq!(answer.status,422,"{}",answer.body);
        assert!(answer.body.contains("Currency conversion unavailable"));assert!(!answer.body.contains("fx-secret-placeholder"));
        assert_eq!(rows(&c),before);
        let answer=price(&app,&mut b,id,"").await;
        assert_eq!(answer.status,200);assert_eq!(manual(&c,id),json!({}));
        assert!(answer.body.contains("Currency conversion unavailable"));assert!(!answer.body.contains("value=\"0.0\""));
    }
}
fn rows(c:&Connection)->Vec<String> {
    let names:Vec<String>=c.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    names.into_iter().flat_map(|name| {
        let mut s=c.prepare(&format!("SELECT * FROM {name} ORDER BY 1")).unwrap();let count=s.column_count();
        s.query_map([],|r|Ok((0..count).map(|i|format!("{:?}",r.get_ref(i).unwrap())).collect::<Vec<_>>().join("|"))).unwrap().collect::<Result<Vec<_>,_>>().unwrap()
    }).collect()
}

#[tokio::test(flavor="current_thread")]
async fn numeric_string_feed_saves_usd_and_rejects_garbage_without_effects() {
    let feed=feed().await;
    for (usd,eur) in [("100.0","80.0"),(" 100.0 "," 80.0 ")] {
        *feed.body.lock().unwrap()=json!({"data":{"usd":{"value":usd},"eur":{"value":eur}}});
        let(_dir,c,id,app,mut b,_)=install(&feed.url,"EUR");
        let answer=price(&app,&mut b,id,"80").await;
        assert_eq!(answer.status,200);assert_eq!(manual(&c,id),json!({"price":"100.0"}));
    }
    for value in ["12abc","","NaN","inf","1e999","0","-80"] {
        *feed.body.lock().unwrap()=json!({"data":{"usd":{"value":"100.0"},"eur":{"value":value}}});
        let(_dir,c,id,app,mut b,_)=install(&feed.url,"EUR");
        let before=rows(&c);let answer=price(&app,&mut b,id,"80").await;
        assert_eq!(answer.status,422);assert!(answer.body.contains("Currency conversion unavailable"));
        assert_eq!(rows(&c),before);
    }
}
