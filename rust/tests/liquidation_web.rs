//! Slice 6c-3: HTTP selection, queued state, and the shared API writer.
mod common;
use common::{seed::{self,BotSpec},web::{self,Browser,Csrf,TestClock}};
use deltabadger::{engine::model,web::{csrf,session,App,bot_api::{self,Write},bot::action_params::ActionParams}};
use serde_json::{json,Value};
use rusqlite::Connection;
const NOW: &str="2026-09-01T12:30:00Z";
const HEADERS: &[(&str,&str)]=&[("origin","http://localhost:3000"),("accept","text/vnd.turbo-stream.html, text/html")];
fn install()->(tempfile::TempDir,Connection,seed::Seeded,i64,App,Browser) {
    let (dir,o,s)=common::install_alpaca(); let c=o.primary;
    let (eth,_)=seed::add_eth_sol(&c,&s);
    let id=seed::insert_bot(&c,&s,&BotSpec {status:2,..BotSpec::weekly(60.0,"2026-09-01 10:00:00")}.weights(&[(eth,1.0)]));
    deltabadger::engine::basket::refresh_composition(&c,&model::load_bot(&c,id).unwrap(),NOW.parse().unwrap()).unwrap().unwrap();
    c.execute("INSERT INTO bot_index_assets(bot_id,asset_id,ticker_id,in_index,target_allocation,entered_at,exited_at,created_at,updated_at) VALUES (?1,?2,?3,0,0,'2026-09-01 10:00:00','2026-09-01 11:00:00','2026-09-01 10:00:00','2026-09-01 11:00:00')",(id,s.btc,s.ticker_id)).unwrap();
    seed::insert_row(&c,&s,id,s.btc,&json!({"external_id":"held","external_status":2,"amount":"0.012345","amount_exec":"0.012345","price":"64000","quote_amount":"790.08","quote_amount_exec":"790.08","created_at":"2026-09-01 10:00:01"}));
    c.execute("UPDATE users SET confirmed_at=created_at,setup_completed=1,wash_sale_enabled=0",[]).unwrap();
    let hash:String=c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[s.user_id],|r|r.get(0)).unwrap();
    let app=web::app(dir.path(),"engine-test-secret",TestClock::at(NOW)).with_figure_source(deltabadger::web::figure::loading::Source::Script(json!({}))).unwrap();
    let token=csrf::new_token();
    let data=session::SessionData {user:Some((s.user_id,hash.chars().take(29).collect())),csrf:Some(token.clone()),..Default::default()};
    let browser=Browser {cookie:Some(session::seal(&app.keys.session,&data,app.now())),page:Some(format!("<meta name=\"csrf-token\" content=\"{}\">",csrf::masked(&token)))};
    (dir,c,s,id,app,browser)
}
fn api(c:&Connection,app:&App,user:i64,id:i64,args:Value)->bot_api::Res {
    bot_api::write_result(bot_api::bot_write(c,app,user,id,Write::Liquidate,&ActionParams::from_mcp(args).unwrap()).unwrap())
}
fn status_text(page:&str,id:i64)->String {
    let page=scraper::Html::parse_document(page);
    let selector=scraper::Selector::parse(&format!("#status_bar_bots_dca_multi_asset_{id}")).unwrap();
    page.select(&selector).next().expect("status bar").text().collect::<String>().trim().to_string()
}
#[tokio::test(flavor="current_thread")]
async fn cold_modal_names_ledger_amount_and_web_intersects_then_refuses_double_post() {
    let (_d,c,s,id,app,mut b)=install();
    let modal=b.get(&app,&format!("/bots/{id}/liquidation/new?symbol[]=BTC&symbol[]=gone&asset_id[]={}&asset_id[]=999",s.btc)).await;
    assert_eq!(modal.status,200,"{}",modal.body);
    assert!(modal.body.contains("liquidation_confirm_list") && modal.body.contains("0.012345"),"{}",modal.body);
    assert!(!modal.body.contains("data-symbol=\"gone\""));
    let path=format!("/bots/{id}/liquidation");
    let form=[("symbol[]","BTC"),("symbol[]","gone")];
    let first=b.send(&app,"POST",&path,Some(&form),Csrf::Header,HEADERS).await;
    assert_eq!(first.status,200,"{}",first.body);
    let request=model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].clone();
    assert_eq!(request["holdings"],json!([["BTC",s.btc]]));
    let second=b.send(&app,"POST",&path,Some(&form),Csrf::Header,HEADERS).await;
    assert_eq!(second.status,422,"{}",second.body);
    assert_eq!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"],request);
    assert_eq!(c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event=\'liquidation_requested\'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    let page=b.get(&app,&format!("/bots/{id}")).await;
    assert_eq!(status_text(&page.body,id),"Selling…");
}
#[test]
fn api_selection_is_all_or_nothing_and_dry_run_does_not_write() {
    let (_d,c,s,id,app,_)=install();
    assert_eq!(api(&c,&app,s.user_id,id,json!({"symbol":["BTC","gone"]})).code,Some("holding_not_held"));
    assert!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null());
    c.execute("UPDATE users SET mcp_settings=\'{\"dry_run\":true}\'",[]).unwrap();
    let dry=api(&c,&app,s.user_id,id,json!({"symbol":"BTC"}));
    assert_eq!(dry.data["dry_run"],true);
    assert!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null());
    c.execute("UPDATE users SET mcp_settings=\'{}\'",[]).unwrap();
    let sold=api(&c,&app,s.user_id,id,json!({"symbol":s.btc.to_string()}));
    assert_eq!(sold.code,None); assert_eq!(sold.data["symbols"],json!(["BTC"]));
    assert_eq!(api(&c,&app,s.user_id,id,json!({"symbol":"BTC"})).code,Some("write_refused"));
}
#[test]
fn api_uses_the_web_wash_refusal_and_rails_domain_codes() {
    let (_d,c,s,id,app,_)=install();
    for wash in [Some(true),None] {
        c.execute("UPDATE users SET wash_sale_enabled=?1",[wash]).unwrap();
        assert_eq!(api(&c,&app,s.user_id,id,json!({"symbol":"BTC"})).code,Some("not_ported"));
    }
    c.execute("UPDATE users SET wash_sale_enabled=0",[]).unwrap();
    c.execute("UPDATE bots SET status=7 WHERE id=?1",[id]).unwrap();
    let archived=api(&c,&app,s.user_id,id,json!({"symbol":"BTC"}));
    assert_eq!(archived.code,Some("bot_archived"));
    assert_eq!(archived.message,"Bot 'ETH' is archived; reactivate it first.");
    c.execute("UPDATE bots SET status=2,type=\'Bots::Signal\' WHERE id=?1",[id]).unwrap();
    assert_eq!(api(&c,&app,s.user_id,id,json!({"symbol":"BTC"})).code,Some("not_composition_bot"));
}

#[tokio::test(flavor="current_thread")]
async fn web_modal_and_submit_use_the_same_wash_page_refusal() {
    for wash in [Some(true),None] {
        let (_d,c,_s,id,app,mut b)=install();
        c.execute("UPDATE users SET wash_sale_enabled=?1",[wash]).unwrap();
        assert_eq!(b.get(&app,&format!("/bots/{id}/liquidation/new?symbol=BTC")).await.status,501);
        let answer=b.send(&app,"POST",&format!("/bots/{id}/liquidation"),Some(&[("symbol","BTC")]),Csrf::Header,HEADERS).await;
        assert_eq!(answer.status,501,"{}",answer.body);
        assert!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null());
    }
}

fn credentials(c:&Connection,user:i64) {
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('liquidation','liquidation','http://localhost/cb',0,'mcp api','2026-09-01 12:00:00','2026-09-01 12:00:00')",[]).unwrap();
    let application=c.last_insert_rowid();
    for (token,scope) in [("liquidation-mcp","mcp"),("liquidation-api","api")] {
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,'2026-09-01 12:00:00',86400)",(application,user,token,scope)).unwrap();
    }
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,NULL,NULL,'2026-09-01 12:00:00','2026-09-01 12:00:00')",(user,application)).unwrap();
    c.execute("UPDATE users SET mcp_settings=?1,rest_settings=?1",[json!({"tool_permissions":{"liquidate_exited_asset":true}}).to_string()]).unwrap();
}
async fn wire(app:&App,path:&str,token:&str,body:Value,session:Option<&str>)->(u16,Option<String>,String) {
    wire_with_key(app,path,token,body,session,Some(&uuid::Uuid::new_v4().to_string())).await
}
async fn wire_with_key(app:&App,path:&str,token:&str,body:Value,session:Option<&str>,key:Option<&str>)->(u16,Option<String>,String) {
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
async fn mcp(app:&App,id:i64)->Value {
    let (_,session,_)=wire(app,"/mcp","liquidation-mcp",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"liquidation","version":"1"}}}),None).await;
    let session=session.unwrap();
    assert_eq!(wire(app,"/mcp","liquidation-mcp",json!({"jsonrpc":"2.0","method":"notifications/initialized"}),Some(&session)).await.0,202);
    let (_,_,body)=wire(app,"/mcp","liquidation-mcp",json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"liquidate_exited_asset","arguments":{"bot_id":id,"symbol":"BTC"}}}),Some(&session)).await;
    serde_json::from_str::<Value>(&body).unwrap()["result"].clone()
}
#[tokio::test(flavor="current_thread")]
async fn rest_and_mcp_real_entry_points_queue_the_webs_rows_with_the_wash_sale_rule_on() {
    for rest in [true,false] {
        let (_d,c,s,id,app,_)=install();credentials(&c,s.user_id);
        // PLAN-wash-sale WS-4a: the rule is no refusal (Rails asks nothing); the engine claims the lock if the sale is at a loss.
        c.execute("UPDATE users SET wash_sale_enabled=1",[]).unwrap();
        if rest {
            let (status,_,body)=wire(&app,&format!("/api/v1/bots/{id}/liquidations"),"liquidation-api",json!({"symbol":"BTC"}),None).await;
            assert_eq!(status,202,"{body}");
            assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["data"]["symbols"],json!(["BTC"]));
        } else {let result=mcp(&app,id).await;assert!(result.get("isError").is_none(),"{result}");assert!(result["content"][0]["text"].as_str().unwrap().contains("Selling BTC"));}
        assert_eq!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"]["holdings"],json!([["BTC",s.btc]]));
        assert_eq!(c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event='liquidation_requested'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(c.query_row("SELECT count(*) FROM transactions",[],|r|r.get::<_,i64>(0)).unwrap(),1,"only the engine places orders");
    }
}

#[tokio::test(flavor="current_thread")]
async fn mcp_dry_run_reports_the_sale_without_queuing_it() {
    let (_d,c,s,id,app,_)=install();credentials(&c,s.user_id);
    c.execute("UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run',json('true'))",[]).unwrap();
    let answer=mcp(&app,id).await;
    assert!(answer.get("isError").is_none(),"{answer}");
    assert!(answer["content"][0]["text"].as_str().unwrap().starts_with("[DRY RUN] Would sell BTC"));
    assert!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null());
    assert_eq!(c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event='liquidation_requested'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}
#[tokio::test(flavor="current_thread")]
async fn clear_http_accounts_for_shown_rows_and_status_lasts_through_taken_request() {
    let (_d,c,s,id,app,mut b)=install();
    let row=seed::insert_row(&c,&s,id,s.btc,&json!({"side":1,"external_status":4,"external_id":"lost","created_at":"2026-09-01 12:00:00"}));
    c.execute("UPDATE transactions SET transaction_type='LIQUIDATION' WHERE id=?1",[row]).unwrap();
    let revision=deltabadger::web::figure::loading::revision(&c,s.user_id).unwrap().0;
    let answer=b.send(&app,"POST",&format!("/bots/{id}/liquidation_resolutions"),Some(&[("order_ids[]",&row.to_string())]),Csrf::Header,HEADERS).await;
    assert!(matches!(answer.status,302|303),"{}",answer.body);
    assert_eq!(model::load_bot(&c,id).unwrap().transient["liquidation_resolved_orders"],json!([row]));
    assert_ne!(deltabadger::web::figure::loading::revision(&c,s.user_id).unwrap().0,revision,"Clear invalidates cached halt controls even without a pending marker");
    for taken in [false,true] {
        let mut request=json!({"id":"request","at":1,"holdings":[["BTC",s.btc]]});
        if taken {request["taken_at"]=json!(2);}
        model::merge_transient_compact(&c,id,&[("liquidation_selling_since",request)]).unwrap();
        let page=b.get(&app,&format!("/bots/{id}")).await;
        assert_eq!(page.status,200,"{}",page.body);
        assert_eq!(status_text(&page.body,id),"Selling…","request remains visible after the Rails TTL, including taken requests");
    }
    model::merge_transient_compact(&c,id,&[("liquidation_selling_since",Value::Null),("liquidation_pending",json!({"id":"halt","state":"ambiguous","symbol":"BTC"}))]).unwrap();
    let page=b.get(&app,&format!("/bots/{id}")).await;
    assert_eq!(page.status,200,"{}",page.body);
    assert!(!status_text(&page.body,id).contains("Selling"),"halts stay in the panel");
}

#[test]
fn halt_controls_survive_cold_prices_and_only_name_the_rendered_snapshot() {
    let (_d,c,s,id,_app,_)=install();
    let row=seed::insert_row(&c,&s,id,s.btc,&json!({"side":1,"external_status":4,"external_id":"lost","created_at":"2026-09-01 12:00:00"}));
    c.execute("UPDATE transactions SET transaction_type='LIQUIDATION' WHERE id=?1",[row]).unwrap();
    let market=deltabadger::figures::scripted::Scripted::new(&json!({"GET data.alpaca.markets/v1beta3/crypto/us/latest/trades":{"status":503}}),None);
    let render=||deltabadger::web::figure::metrics_panel(&c,id,&market,deltabadger::figures::at::At::from_sql(NOW).unwrap(),"en","").unwrap();
    model::merge_transient_compact(&c,id,&[("liquidation_pending",json!({"id":"placing","state":"placing","symbol":"ETH"}))]).unwrap();
    let panel=render();
    assert!(panel.contains("Sale unconfirmed") && panel.contains("BTC"),"{panel}");
    assert!(panel.contains(&format!("order_ids%5B%5D={row}")),"{panel}");
    assert!(!panel.contains("intent_id=placing") && !panel.contains("liquidation/new?"),"{panel}");
    model::merge_transient_compact(&c,id,&[("liquidation_pending",json!({"id":"ambiguous","state":"ambiguous","symbol":"ETH"}))]).unwrap();
    let panel=render();
    assert!(panel.contains("intent_id=ambiguous") && panel.contains("ETH, BTC"),"{panel}");
}

#[test]
fn sell_links_disappear_for_the_shared_in_flight_predicate() {
    let (_d,c,s,id,_app,_)=install();
    let market=deltabadger::figures::scripted::Scripted::new(&json!({"GET data.alpaca.markets/v1beta3/crypto/us/latest/trades":{"body":{"trades":{"BTC/USD":{"p":64000},"ETH/USD":{"p":2000}}}}}),None);
    let render=||deltabadger::web::figure::metrics_panel(&c,id,&market,deltabadger::figures::at::At::from_sql(NOW).unwrap(),"en","").unwrap();
    let shown=render();assert!(shown.contains("liquidation/new?"),"{shown}");
    for (key,value) in [("liquidation_selling_since",json!({"id":"queued","at":1,"holdings":[["BTC",s.btc]]})),("rebalance_pending",json!({"phase":"selling"})),("liquidation_pending",json!({"id":"unknown","state":"ambiguous","symbol":"BTC"}))] {
        model::merge_transient_compact(&c,id,&[(key,value)]).unwrap();
        assert!(!render().contains("liquidation/new?"),"{key}");
        model::merge_transient_compact(&c,id,&[(key,Value::Null)]).unwrap();
    }
}


#[tokio::test(flavor="current_thread")]
async fn rest_requires_and_replays_the_idempotency_key_after_engine_completion() {
    let (_d,c,s,id,app,_)=install();credentials(&c,s.user_id);
    let path=format!("/api/v1/bots/{id}/liquidations");
    let body=json!({"symbol":"BTC"});
    assert_eq!(wire_with_key(&app,&path,"liquidation-api",body.clone(),None,None).await.0,400);
    let first=wire_with_key(&app,&path,"liquidation-api",body.clone(),None,Some("repeat")).await;
    assert_eq!(first.0,202,"{}",first.2);
    model::merge_transient_compact(&c,id,&[("liquidation_selling_since",Value::Null)]).unwrap();
    let replay=wire_with_key(&app,&path,"liquidation-api",body.clone(),None,Some("repeat")).await;
    assert_eq!((replay.0,replay.2),(first.0,first.2));
    assert!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null(),"a completed key cannot queue a second sale");
    let changed=wire_with_key(&app,&path,"liquidation-api",json!({"symbol":"ETH"}),None,Some("repeat")).await;
    assert_eq!(changed.0,409);assert!(changed.2.contains("idempotency_key_reused"));
    c.execute("UPDATE idempotency_keys SET state='in_progress' WHERE key='repeat'",[]).unwrap();
    let progress=wire_with_key(&app,&path,"liquidation-api",body,None,Some("repeat")).await;
    assert_eq!(progress.0,409);assert!(progress.2.contains("idempotency_in_progress"));
}

#[test]
fn api_ambiguous_symbol_names_the_keys_and_asset_identity_cannot_drift() {
    let (_d,c,s,id,app,_)=install();
    let (asset,_)=seed::add_alpaca_stock(&c,&s,"OTHER");
    seed::insert_row(&c,&s,id,asset,&json!({"external_id":"second","external_status":2,"amount":"1","amount_exec":"1","price":"100","quote_amount":"100","quote_amount_exec":"100","created_at":"2026-09-01 10:00:02"}));
    c.execute("UPDATE assets SET symbol='BTC' WHERE id=?1",[asset]).unwrap();
    let result=api(&c,&app,s.user_id,id,json!({"symbol":"btc"}));
    assert_eq!(result.code,Some("holding_ambiguous"),"{}",result.message);
    assert!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null());
    let result=api(&c,&app,s.user_id,id,json!({"symbol":s.btc.to_string(),"asset_id":asset}));
    assert_eq!(result.code,Some("holding_not_held"));
}


#[tokio::test(flavor="current_thread")]
async fn stale_sell_while_rebalancing_logs_the_refusal_without_queuing() {
    let (_d,c,_s,id,app,mut b)=install();
    model::merge_transient_compact(&c,id,&[("rebalance_pending",json!({"phase":"selling"}))]).unwrap();
    let answer=b.send(&app,"POST",&format!("/bots/{id}/liquidation"),Some(&[("symbol","BTC")]),Csrf::Header,HEADERS).await;
    assert_eq!(answer.status,422,"{}",answer.body);
    assert_eq!(c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event='liquidation_not_started'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null());
}
