//! Slice 6c-5: the redeploy prompt's Yes and No, the halt's Clear, the status bar, and the same service on REST and MCP.
mod common;
use common::{scripted::ok,seed::{self,BotSpec},web::{self,Browser,Csrf,TestClock}};
use deltabadger::{engine::{model,redeploy,run,FixedClock},web::{csrf,session,App,bot_api::{self,Write},bot::action_params::ActionParams}};
use deltabadger::venue::{alpaca::{AlpacaVenue,Urls},http::ScriptedTransport};
use serde_json::{json,Value};
use rusqlite::Connection;
const NOW: &str="2026-09-01T12:30:00Z";
const HEADERS: &[(&str,&str)]=&[("origin","http://localhost:3000"),("accept","text/vnd.turbo-stream.html, text/html")];
const STARTED: &str="Putting the money back into the basket.";
const IN_FLIGHT: &str="A redeploy is still working. Try again once it settles.";
fn one<T:rusqlite::types::FromSql>(c:&Connection,sql:&str)->T { c.query_row(sql,[],|r|r.get(0)).unwrap() }
fn at(s:&str)->chrono::DateTime<chrono::Utc> { s.parse().unwrap() }

/// A stopped 50/50 BTC/ETH basket whose sale of an exited SOL holding banked 100 USD: the prompt offers 100.
fn install()->(tempfile::TempDir,deltabadger::store::Opened,seed::Seeded,i64,App,Browser) {
    let (dir,o,s)=common::install_alpaca(); let c=&o.primary;
    let (eth,sol)=seed::add_eth_sol(c,&s);
    let id=seed::insert_bot(c,&s,&BotSpec {status:2,..BotSpec::weekly(60.0,"2026-09-01 10:00:00")}.with("rebalance_enabled",json!(false)).weights(&[(s.btc,0.5),(eth,0.5)]));
    deltabadger::engine::basket::refresh_composition(c,&model::load_bot(c,id).unwrap(),at("2026-09-01T09:00:00Z")).unwrap().unwrap();
    for (asset,qty,value,price,n) in [(s.btc,"0.01","640","64000",1),(eth,"0.04","100","2500",2),(sol,"1","100","100",3)] {
        seed::insert_row(c,&s,id,asset,&json!({"external_status":2,"external_id":format!("OHELD-{n}"),"amount":qty,"amount_exec":qty,"quote_amount":value,"quote_amount_exec":value,"price":price,"created_at":"2026-09-01 10:00:01"}));
    }
    let sold=seed::insert_row(c,&s,id,sol,&json!({"external_status":2,"external_id":"OSOLD","side":1,"amount":"1","amount_exec":"1","quote_amount":"100","quote_amount_exec":"100","price":"100","created_at":"2026-09-01 10:00:02"}));
    c.execute("UPDATE transactions SET transaction_type='LIQUIDATION' WHERE id=?1",[sold]).unwrap();
    c.execute("UPDATE users SET confirmed_at=created_at,setup_completed=1,wash_sale_enabled=0",[]).unwrap();
    let hash:String=c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[s.user_id],|r|r.get(0)).unwrap();
    let app=web::app(dir.path(),"engine-test-secret",TestClock::at(NOW)).with_figure_source(deltabadger::web::figure::loading::Source::Script(json!({}))).unwrap();
    let token=csrf::new_token();
    let data=session::SessionData {user:Some((s.user_id,hash.chars().take(29).collect())),csrf:Some(token.clone()),..Default::default()};
    let browser=Browser {cookie:Some(session::seal(&app.keys.session,&data,app.now())),page:Some(format!("<meta name=\"csrf-token\" content=\"{}\">",csrf::masked(&token)))};
    (dir,o,s,id,app,browser)
}
fn api(c:&Connection,app:&App,user:i64,id:i64,args:Value)->bot_api::Res {
    bot_api::write_result(bot_api::bot_write(c,app,user,id,Write::Redeploy,&ActionParams::from_mcp(args).unwrap()).unwrap())
}
fn events(c:&Connection,event:&str)->i64 { c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event=?1",[event],|r|r.get(0)).unwrap() }
fn offset(c:&Connection)->f64 { one(c,"SELECT CAST(COALESCE(redeploy_declined_offset,0) AS REAL) FROM bots") }
fn request(c:&Connection,id:i64)->Value { model::load_bot(c,id).unwrap().transient[redeploy::REQUEST_KEY].clone() }
async fn yes(b:&mut Browser,app:&App,id:i64)->web::Answer { b.send(app,"POST",&format!("/bots/{id}/redeploy"),Some(&[]),Csrf::Header,HEADERS).await }
async fn no(b:&mut Browser,app:&App,id:i64)->web::Answer { b.send(app,"POST",&format!("/bots/{id}/redeploy"),Some(&[("_method","delete")]),Csrf::Header,HEADERS).await }
fn status_text(page:&str,id:i64)->String {
    let page=scraper::Html::parse_document(page);
    let selector=scraper::Selector::parse(&format!("#status_bar_bots_dca_multi_asset_{id}")).unwrap();
    page.select(&selector).next().expect("status bar").text().collect::<String>().trim().to_string()
}
fn panel(c:&Connection,id:i64)->String {
    let market=deltabadger::figures::scripted::Scripted::new(&json!({"GET data.alpaca.markets/v1beta3/crypto/us/latest/trades":{"body":{"trades":{"BTC/USD":{"p":64000},"ETH/USD":{"p":2500},"SOL/USD":{"p":100}}}}}),None);
    deltabadger::web::figure::metrics_panel(c,id,&market,deltabadger::figures::at::At::from_sql(NOW).unwrap(),"en","").unwrap()
}

#[tokio::test(flavor="current_thread")]
async fn yes_swaps_the_actions_for_the_spinner_and_a_second_yes_is_refused() {
    let (_d,o,s,id,app,mut b)=install(); let c=&o.primary;
    let first=yes(&mut b,&app,id).await;
    assert_eq!(first.status,200,"{}",first.body);
    assert!(first.body.contains(STARTED),"{}",first.body);
    assert!(first.body.contains(&format!("<turbo-stream action=\"update\" target=\"redeploy-prompt-actions\"><template><span class=\"loader--small\" role=\"status\" aria-label=\"{STARTED}\"></span></template></turbo-stream>")),"{}",first.body);
    let taken=request(c,id);
    assert!(uuid::Uuid::parse_str(taken["id"].as_str().unwrap()).is_ok() && taken["at"]==at(NOW).timestamp() && taken.as_object().unwrap().len()==2,"{taken}");
    assert_eq!(events(c,"redeploy_requested"),1);
    // D-6C-8: a second Yes before the engine took the first is refused, not queued again.
    let second=yes(&mut b,&app,id).await;
    assert_eq!(second.status,422,"{}",second.body);
    assert!(second.body.contains(IN_FLIGHT) && !second.body.contains("redeploy-prompt-actions"),"{}",second.body);
    assert_eq!(request(c,id),taken);
    assert_eq!(events(c,"redeploy_requested"),1);
    // D-6C-6: the status bar says so while the request waits or runs, a taken one included.
    for taken_at in [None,Some(1)] {
        let mut shown=taken.clone(); if let Some(t)=taken_at {shown["taken_at"]=json!(t);}
        model::merge_transient_compact(c,id,&[(redeploy::REQUEST_KEY,shown)]).unwrap();
        let page=b.get(&app,&format!("/bots/{id}")).await;
        assert_eq!(page.status,200,"{}",page.body);
        assert_eq!(status_text(&page.body,id),STARTED);
        assert!(!panel(c,id).contains("redeploy-prompt__actions"),"the prompt hides while the Yes is in flight");
    }
    model::merge_transient_compact(c,id,&[(redeploy::REQUEST_KEY,Value::Null)]).unwrap();
    assert!(!status_text(&b.get(&app,&format!("/bots/{id}")).await.body,id).contains(STARTED));
    let _=s;
}

#[tokio::test(flavor="current_thread")]
async fn no_writes_the_offset_in_the_request() {
    let (_d,o,_s,id,app,mut b)=install(); let c=&o.primary;
    assert!(panel(c,id).contains("Redeploy 100.00 USD?"),"{}",panel(c,id));
    let answer=no(&mut b,&app,id).await;
    assert_eq!(answer.status,200,"{}",answer.body);
    assert!(answer.body.contains("Taken off the table.") && answer.body.contains("aria-label=\"Taken off the table.\""),"{}",answer.body);
    assert_eq!(offset(c),100.0,"written before the answer, not by a later job");
    assert_eq!(events(c,"redeploy_declined"),1);
    assert!(request(c,id).is_null());
    assert!(!panel(c,id).contains("redeploy-prompt"),"nothing is offered after a No");
}

#[tokio::test(flavor="current_thread")]
async fn yes_and_no_race_through_the_web_in_either_order() {
    for no_first in [false,true] {
        let (d,o,s,id,app,mut b)=install();
        if no_first {assert_eq!(no(&mut b,&app,id).await.status,200);}
        let answer=yes(&mut b,&app,id).await;
        assert_eq!(answer.status,200,"{}",answer.body);
        if !no_first {
            let refused=no(&mut b,&app,id).await;
            assert_eq!(refused.status,422,"{}",refused.body);
            assert!(refused.body.contains(IN_FLIGHT),"{}",refused.body);
            assert_eq!(events(&o.primary,"redeploy_decline_refused"),1);
        }
        assert_eq!(offset(&o.primary),if no_first {100.0} else {0.0});
        let t=ScriptedTransport::from_script(&json!({
            "GET /v1beta3/crypto/us/latest/quotes":[ok(json!({"quotes":{"BTC/USD":{"ap":64000,"bp":64000},"ETH/USD":{"ap":2500,"bp":2500},"SOL/USD":{"ap":100,"bp":100}}}))],
            "GET /v1beta3/crypto/us/latest/trades":[ok(json!({"trades":{"BTC/USD":{"p":64000},"ETH/USD":{"p":2500},"SOL/USD":{"p":100}}}))],
            "POST /v2/orders":[common::scripted::accepted("OTX-1")],
            "GET /v2/account":[ok(json!({"cash":"100000","non_marginable_buying_power":"100000"}))],
            "GET /v2/positions":[ok(json!([]))],
        }));
        let mut e=engine(&d,o,&t);
        run::run_commands(&mut e,&FixedClock(at(NOW))).await.unwrap();
        assert_eq!(t.posted_orders().len(),usize::from(!no_first),"no_first={no_first}");
        assert!(request(&e.primary,id).is_null());
        let _=s;
    }
}

async fn clear(b:&mut Browser,app:&App,id:i64,nonce:&str)->web::Answer {
    b.send(app,"POST",&format!("/bots/{id}/redeploy_resolutions"),Some(&[("intent_id",nonce)]),Csrf::Header,HEADERS).await
}
struct Factory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for Factory {
    type V=AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self,_t:&str,_c:Option<deltabadger::crypto::Credentials>)->Self::V { AlpacaVenue::new(self.0.clone(),Urls::for_passphrase(Some("paper"))) }
}
fn engine(d:&tempfile::TempDir,o:deltabadger::store::Opened,t:&ScriptedTransport)->run::Engine<Factory> {
    let paths=deltabadger::store::Paths::from_env(&|_|None,d.path());
    let lock=deltabadger::lease::lock(&paths,at(NOW)).unwrap();
    run::Engine::new(o.primary,Factory(t.clone()),seed::cipher(),lock)
}

#[test]
fn accept_is_required_and_strict_on_every_surface() {
    let (_d,o,s,id,app,_)=install(); let c=&o.primary;
    for args in [json!({}),json!({"accept":null}),json!({"accept":""}),json!({"accept":"maybe"}),json!({"accept":1})] {
        let res=api(c,&app,s.user_id,id,args.clone());
        assert_eq!((res.code,res.status),(Some("accept_required"),bot_api::Domain::ValidationFailed),"{args}");
        assert_eq!(res.message,"accept must be true or false.");
    }
    assert!(request(c,id).is_null() && offset(c)==0.0);
    // "true" and "false" are answers, as Rails' Boolean.parse reads a form or a query.
    assert_eq!(api(c,&app,s.user_id,id,json!({"accept":"false","dry_run":true})).data["accepted"],false);
    let dry=api(c,&app,s.user_id,id,json!({"accept":"true","dry_run":true}));
    assert_eq!(dry.code,None);
    assert_eq!(dry.data,json!({"id":id,"label":dry.data["label"],"accepted":true,"offer":"100.0","dry_run":true}));
    assert!(request(c,id).is_null(),"a dry run queues nothing");
    c.execute("UPDATE bots SET status=7",[]).unwrap();
    assert_eq!(api(c,&app,s.user_id,id,json!({"accept":true})).code,Some("bot_archived"));
    c.execute("UPDATE bots SET status=2,type='Bots::Signal'",[]).unwrap();
    let other=api(c,&app,s.user_id,id,json!({"accept":true}));
    assert_eq!((other.code,other.message.as_str()),(Some("not_composition_bot"),"Only index and basket bots have a redeploy offer."));
}

#[test]
fn the_prompt_hides_while_a_liquidation_row_waits_and_the_halt_offers_clear() {
    let (_d,o,s,id,_app,_)=install(); let c=&o.primary;
    let shown=panel(c,id);
    assert!(shown.contains("id=\"redeploy-prompt-actions\"") && shown.contains("Redeploy 100.00 USD?"),"{shown}");
    let row=seed::insert_row(c,&s,id,s.btc,&json!({"side":1,"external_status":0,"status":0,"external_id":"waiting","created_at":"2026-09-01 12:00:00"}));
    c.execute("UPDATE transactions SET transaction_type='LIQUIDATION' WHERE id=?1",[row]).unwrap();
    assert!(!panel(c,id).contains("redeploy-prompt"),"a waiting sale's proceeds are not in yet");
    c.execute("DELETE FROM transactions WHERE id=?1",[row]).unwrap();
    model::merge_transient_compact(c,id,&[(redeploy::PENDING_KEY,json!({"id":"halt-nonce","state":"ambiguous"}))]).unwrap();
    let halted=panel(c,id);
    assert!(halted.contains("Redeploy unconfirmed — check your exchange") && halted.contains("/bots/1/redeploy_resolutions?intent_id=halt-nonce"),"{halted}");
    assert!(!halted.contains("redeploy-prompt__actions"),"the halt replaces the prompt: {halted}");
    model::merge_transient_compact(c,id,&[(redeploy::PENDING_KEY,json!({"id":"placing-nonce","state":"placing"}))]).unwrap();
    let placing=panel(c,id);
    assert!(!placing.contains("redeploy_resolutions") && !placing.contains("redeploy-prompt"),"a placing intent is neither clearable nor offered: {placing}");
}

#[tokio::test(flavor="current_thread")]
async fn the_page_is_served_during_a_halt_and_clear_answers_rails_flashes() {
    let (_d,o,s,id,app,mut b)=install(); let c=&o.primary;
    model::merge_transient_compact(c,id,&[(redeploy::PENDING_KEY,json!({"id":"halt-nonce","state":"ambiguous"}))]).unwrap();
    let page=b.get(&app,&format!("/bots/{id}")).await;
    assert_eq!(page.status,200,"redeploy_pending no longer refuses the page: {}",page.body);
    let row=seed::insert_row(c,&s,id,s.btc,&json!({"external_status":0,"status":0,"external_id":"OPEN-1","created_at":"2026-09-01 12:00:00"}));
    c.execute("UPDATE transactions SET transaction_type='REDEPLOY' WHERE id=?1",[row]).unwrap();
    let path=format!("/bots/{id}/redeploy_resolutions");
    let open=b.send(&app,"POST",&path,Some(&[("intent_id","halt-nonce")]),Csrf::Header,HEADERS).await;
    assert!(matches!(open.status,302|303),"{}",open.body);
    let after=b.get(&app,&format!("/bots/{id}")).await;
    assert!(after.body.contains("Order OPEN-1 is still open on your exchange. Wait for it to settle first."),"{}",after.body);
    assert_eq!(model::load_bot(c,id).unwrap().transient[redeploy::PENDING_KEY]["state"],"ambiguous","still_open clears nothing");
    c.execute("UPDATE transactions SET external_status=2 WHERE id=?1",[row]).unwrap();
    for (nonce,cleared) in [("stale",false),("halt-nonce",true)] {
        let answer=b.send(&app,"POST",&path,Some(&[("intent_id",nonce)]),Csrf::Header,HEADERS).await;
        assert!(matches!(answer.status,302|303),"{}",answer.body);
        assert!(b.get(&app,&format!("/bots/{id}")).await.body.contains("Cleared. The bot can trade again."),"Rails' flash for both");
        assert_eq!(model::load_bot(c,id).unwrap().transient[redeploy::PENDING_KEY].is_null(),cleared,"{nonce}");
    }
    assert_eq!(events(c,"redeploy_manually_resolved"),1);
}

#[tokio::test(flavor="current_thread")]
async fn clear_and_a_successful_recovery_are_exclusive_in_either_order() {
    for clear_first in [true,false] {
        let (d,o,_s,id,app,mut b)=install();
        let t=ScriptedTransport::from_script(&json!({
            "GET /v1beta3/crypto/us/latest/quotes":[ok(json!({"quotes":{"BTC/USD":{"ap":64000,"bp":64000},"ETH/USD":{"ap":2500,"bp":2500},"SOL/USD":{"ap":100,"bp":100}}}))],
            "GET /v1beta3/crypto/us/latest/trades":[ok(json!({"trades":{"BTC/USD":{"p":64000},"ETH/USD":{"p":2500},"SOL/USD":{"p":100}}}))],
            "POST /v2/orders":[{"network":"post_send","message":common::scripted::POST_SEND}],
            "GET /v2/account":[ok(json!({"cash":"100000","non_marginable_buying_power":"100000"}))],
            "GET /v2/positions":[ok(json!([]))],
        }));
        assert_eq!(yes(&mut b,&app,id).await.status,200);
        let mut e=engine(&d,o,&t);
        run::run_commands(&mut e,&FixedClock(at(NOW))).await.unwrap();
        let bot=model::load_bot(&e.primary,id).unwrap();
        assert_eq!(bot.transient[redeploy::PENDING_KEY]["state"],"ambiguous");
        let intent=deltabadger::engine::placement::load_intent(&e.primary,&bot,None).unwrap().expect("the unknown send keeps its intent");
        let nonce=bot.transient[redeploy::PENDING_KEY]["id"].as_str().unwrap().to_string();
        if clear_first { assert!(matches!(clear(&mut b,&app,id,&nonce).await.status,302|303)); }
        let recovered=deltabadger::engine::placement::record_accepted(&e.primary,&bot,&intent,"recovered");
        assert_eq!(recovered.is_ok(),!clear_first,"clear_first={clear_first}");
        if !clear_first {
            // Recovery won: its waiting row refuses Clear, and once it settles the old page's nonce clears nothing.
            assert!(matches!(clear(&mut b,&app,id,&nonce).await.status,302|303));
            assert!(model::load_bot(&e.primary,id).unwrap().transient[redeploy::PENDING_KEY].is_null(),"recovery's settlement cleared the halt");
            e.primary.execute("UPDATE transactions SET external_status=2 WHERE external_id='recovered'",[]).unwrap();
            assert!(matches!(clear(&mut b,&app,id,&nonce).await.status,302|303));
        }
        let bot=model::load_bot(&e.primary,id).unwrap();
        assert!(bot.rust_placement().is_none() && bot.transient[redeploy::PENDING_KEY].is_null());
        assert_eq!(one::<i64>(&e.primary,"SELECT count(*) FROM transactions WHERE external_id='recovered'"),i64::from(!clear_first));
        // D-6C-11: only the cleared intent's request joins the offset; a recovered row is spend instead.
        assert_eq!(offset(&e.primary)>0.0,clear_first);
        assert_eq!(events(&e.primary,"redeploy_manually_resolved"),i64::from(clear_first));
    }
}

fn credentials(c:&Connection,user:i64) {
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('redeploy','redeploy','http://localhost/cb',0,'mcp api','2026-09-01 12:00:00','2026-09-01 12:00:00')",[]).unwrap();
    let application=c.last_insert_rowid();
    for (token,scope) in [("redeploy-mcp","mcp"),("redeploy-api","api")] {
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,'2026-09-01 12:00:00',86400)",(application,user,token,scope)).unwrap();
    }
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,NULL,NULL,'2026-09-01 12:00:00','2026-09-01 12:00:00')",(user,application)).unwrap();
    c.execute("UPDATE users SET mcp_settings=?1,rest_settings=?1",[json!({"tool_permissions":{"answer_redeploy_offer":true}}).to_string()]).unwrap();
}
async fn wire(app:&App,path:&str,token:&str,body:Value,session:Option<&str>,key:Option<&str>)->(u16,Option<String>,String) {
    use axum::{body::{Body,to_bytes},http::Request};
    use tower::ServiceExt;
    let mut request=Request::builder().method("POST").uri(path).header("host","localhost:3000").header("authorization",format!("Bearer {token}"))
        .header("content-type","application/json").header("accept","application/json, text/event-stream");
    if let Some(key)=key {request=request.header("idempotency-key",key);}
    if let Some(session)=session {request=request.header("mcp-session-id",session).header("mcp-protocol-version","2025-11-25");}
    let response=deltabadger::web::router(app.clone()).oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status=response.status().as_u16();
    let session=response.headers().get("mcp-session-id").map(|v|v.to_str().unwrap().to_string());
    (status,session,String::from_utf8(to_bytes(response.into_body(),8_000_000).await.unwrap().to_vec()).unwrap())
}
async fn mcp(app:&App,arguments:Value)->Value {
    let (_,session,_)=wire(app,"/mcp","redeploy-mcp",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"redeploy","version":"1"}}}),None,None).await;
    let session=session.unwrap();
    assert_eq!(wire(app,"/mcp","redeploy-mcp",json!({"jsonrpc":"2.0","method":"notifications/initialized"}),Some(&session),None).await.0,202);
    let (_,_,body)=wire(app,"/mcp","redeploy-mcp",json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"answer_redeploy_offer","arguments":arguments}}),Some(&session),None).await;
    serde_json::from_str::<Value>(&body).unwrap()["result"].clone()
}
/// The web's own rows after one Yes and one No, for the API surfaces to match.
async fn web_rows()->(Value,f64,i64) {
    let (_d,o,_s,id,app,mut b)=install();
    yes(&mut b,&app,id).await; let asked=request(&o.primary,id);
    model::merge_transient_compact(&o.primary,id,&[(redeploy::REQUEST_KEY,Value::Null)]).unwrap();
    no(&mut b,&app,id).await;
    (json!({"keys":asked.as_object().unwrap().keys().collect::<Vec<_>>(),"at":asked["at"]}),offset(&o.primary),events(&o.primary,"redeploy_requested")+events(&o.primary,"redeploy_declined"))
}

#[tokio::test(flavor="current_thread")]
async fn rest_and_mcp_write_the_webs_rows() {
    let web=web_rows().await;
    for rest in [true,false] {
        let (_d,o,s,id,app,_)=install(); let c=&o.primary; credentials(c,s.user_id);
        let answer=|accept:Value|{let app=app.clone();async move {
            if rest {
                let (status,_,body)=wire(&app,&format!("/api/v1/bots/{id}/redeploy"),"redeploy-api",json!({"accept":accept}),None,Some(&uuid::Uuid::new_v4().to_string())).await;
                (status,serde_json::from_str::<Value>(&body).unwrap())
            } else { (0,mcp(&app,json!({"bot_id":id,"accept":accept})).await) }
        }};
        let (status,yes)=answer(json!(true)).await;
        if rest {
            assert_eq!(status,202,"{yes}");
            assert_eq!(yes["data"],json!({"id":id,"label":yes["data"]["label"],"accepted":true,"offer":"100.0","dry_run":false}));
        } else {
            assert!(yes.get("isError").is_none(),"{yes}");
            assert_eq!(yes["content"][0]["text"],format!("Redeploying 100.0 for bot '{}' — queued.",api_label(c,&app,s.user_id,id)));
        }
        let asked=request(c,id);
        model::merge_transient_compact(c,id,&[(redeploy::REQUEST_KEY,Value::Null)]).unwrap();
        let (status,declined)=answer(json!(false)).await;
        if rest {assert_eq!(status,202,"{declined}");} else {assert!(declined["content"][0]["text"].as_str().unwrap().starts_with("Declined redeploying 100.0 for bot '"),"{declined}");}
        let rows=(json!({"keys":asked.as_object().unwrap().keys().collect::<Vec<_>>(),"at":asked["at"]}),offset(c),events(c,"redeploy_requested")+events(c,"redeploy_declined"));
        assert_eq!(rows,web,"rest={rest}");
        assert_eq!(one::<i64>(c,"SELECT count(*) FROM transactions WHERE transaction_type='REDEPLOY'"),0,"only the engine places orders");
    }
}
fn api_label(c:&Connection,app:&App,user:i64,id:i64)->String {
    api(c,app,user,id,json!({"accept":true,"dry_run":true})).data["label"].as_str().unwrap().to_string()
}

#[tokio::test(flavor="current_thread")]
async fn rest_answers_accept_required_and_replays_its_idempotency_key() {
    let (_d,o,s,id,app,_)=install(); let c=&o.primary; credentials(c,s.user_id);
    let path=format!("/api/v1/bots/{id}/redeploy");
    let (status,_,body)=wire(&app,&path,"redeploy-api",json!({}),None,Some("missing")).await;
    assert_eq!(status,422,"{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["error"]["code"],"accept_required");
    assert_eq!(wire(&app,&path,"redeploy-api",json!({"accept":true}),None,None).await.0,400,"Idempotency-Key is required");
    let first=wire(&app,&path,"redeploy-api",json!({"accept":true}),None,Some("repeat")).await;
    assert_eq!(first.0,202,"{}",first.2);
    model::merge_transient_compact(c,id,&[(redeploy::REQUEST_KEY,Value::Null)]).unwrap();
    let replay=wire(&app,&path,"redeploy-api",json!({"accept":true}),None,Some("repeat")).await;
    assert_eq!((replay.0,replay.2),(first.0,first.2));
    assert!(request(c,id).is_null(),"a completed key cannot queue a second Yes");
    let other=wire(&app,&path,"redeploy-api",json!({"accept":false}),None,Some("repeat")).await;
    assert_eq!(other.0,409);assert!(other.2.contains("idempotency_key_reused"));
    // The same key on the liquidation route is another action: a reuse, never a replay.
    c.execute("UPDATE users SET rest_settings=?1",[json!({"tool_permissions":{"answer_redeploy_offer":true,"liquidate_exited_asset":true}}).to_string()]).unwrap();
    let crossed=wire(&app,&format!("/api/v1/bots/{id}/liquidations"),"redeploy-api",json!({"accept":true}),None,Some("repeat")).await;
    assert_eq!(crossed.0,409,"{}",crossed.2);
}

#[tokio::test(flavor="current_thread")]
async fn mcp_dry_run_says_so_and_queues_nothing() {
    let (_d,o,s,id,app,_)=install(); let c=&o.primary; credentials(c,s.user_id);
    c.execute("UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run',json('true'))",[]).unwrap();
    for (accept,verb) in [(true,"Would redeploy"),(false,"Would decline redeploying")] {
        let answer=mcp(&app,json!({"bot_id":id,"accept":accept})).await;
        let text=answer["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with(&format!("[DRY RUN] {verb} 100.0 for bot '")) && text.ends_with("— nothing was queued."),"{text}");
    }
    assert!(request(c,id).is_null() && offset(c)==0.0);
    let missing=mcp(&app,json!({"bot_id":id})).await;
    assert_eq!(missing["isError"],true,"{missing}");
}
