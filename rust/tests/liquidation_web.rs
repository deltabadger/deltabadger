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
    // D-LIQUIDATION-ONE-LINE: the request writes no line; the page's "Selling …" row says it.
    assert_eq!(c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event=\'liquidation_requested\'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
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
fn api_asks_no_wash_question_and_uses_rails_domain_codes() {
    let (_d,c,s,id,app,_)=install();
    // BotApi::Bots::LiquidateExited never asks the wash-sale question (only the web confirmation does): an account that has
    // not answered is not refused for it (WS-4b), as one with the rule on is not (WS-4a).
    c.execute("UPDATE users SET wash_sale_enabled=NULL",[]).unwrap();
    assert_ne!(api(&c,&app,s.user_id,id,json!({"symbol":"BTC"})).code,Some("not_ported"));
    c.execute("UPDATE users SET wash_sale_enabled=0,mcp_settings='{}'",[]).unwrap();
    c.execute("UPDATE bots SET transient_data=json_remove(transient_data,'$.liquidation_selling_since') WHERE id=?1",[id]).unwrap();
    c.execute("UPDATE bots SET status=7 WHERE id=?1",[id]).unwrap();
    let archived=api(&c,&app,s.user_id,id,json!({"symbol":"BTC"}));
    assert_eq!(archived.code,Some("bot_archived"));
    assert_eq!(archived.message,"Bot 'ETH' is archived; reactivate it first.");
    c.execute("UPDATE bots SET status=2,type=\'Bots::Signal\' WHERE id=?1",[id]).unwrap();
    assert_eq!(api(&c,&app,s.user_id,id,json!({"symbol":"BTC"})).code,Some("not_composition_bot"));
}

/// PLAN-wash-sale WS-4b (LiquidationsController#wash_sale_answer_recorded?): an account that has not answered is asked in the
/// Sell confirmation itself, Sell dead until it picks; a submit without an answer is refused and queues nothing; one with it
/// records the answer first, then sells.
#[tokio::test(flavor="current_thread")]
async fn the_sell_confirmation_asks_an_unanswered_account_first() {
    let (_d,c,_s,id,app,mut b)=install();
    app.attach_jobs(deltabadger::jobs::Wakers::default()).unwrap();
    c.execute("UPDATE users SET wash_sale_enabled=NULL",[]).unwrap();
    let modal=b.get(&app,&format!("/bots/{id}/liquidation/new?symbol=BTC")).await;
    assert_eq!(modal.status,200,"{}",modal.body);
    assert!(modal.body.contains("wash-sale-question") && modal.body.contains("type=\"submit\" disabled=\"disabled\""),"{}",modal.body);
    let refused=b.send(&app,"POST",&format!("/bots/{id}/liquidation"),Some(&[("symbol","BTC")]),Csrf::Header,HEADERS).await;
    assert_eq!(refused.status,422,"{}",refused.body);
    assert!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null());
    let enabled=|| c.query_row("SELECT wash_sale_enabled FROM users",[],|r|r.get::<_,Option<bool>>(0)).unwrap();
    assert_eq!(enabled(),None);
    // Nothing named is held: the request ends there (set_symbols), the answer unrecorded and nothing queued.
    let unheld=b.send(&app,"POST",&format!("/bots/{id}/liquidation"),Some(&[("symbol","NOPE"),("wash_sale[enabled]","0")]),Csrf::Header,HEADERS).await;
    assert_eq!(unheld.status,404,"{}",unheld.body);
    assert_eq!(enabled(),None);
    assert!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null());
    let sold=b.send(&app,"POST",&format!("/bots/{id}/liquidation"),Some(&[("symbol","BTC"),("wash_sale[enabled]","0")]),Csrf::Header,HEADERS).await;
    assert_eq!(sold.status,200,"{}",sold.body);
    assert_eq!(enabled(),Some(false),"the answer is recorded with the sale");
    assert!(!model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null());
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
        assert_eq!(c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event='liquidation_requested'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
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
fn sell_links_stay_with_their_targets_while_anything_is_in_flight() {
    let (_d,c,s,id,_app,_)=install();
    let market=deltabadger::figures::scripted::Scripted::new(&json!({"GET data.alpaca.markets/v1beta3/crypto/us/latest/trades":{"body":{"trades":{"BTC/USD":{"p":64000},"ETH/USD":{"p":2000}}}}}),None);
    let render=||deltabadger::web::figure::metrics_panel(&c,id,&market,deltabadger::figures::at::At::from_sql(NOW).unwrap(),"en","").unwrap();
    let link=format!("data-bot--selling-target=\"sell\" data-asset=\"{}\" data-turbo-frame=\"modal\" href=\"/bots/{id}/liquidation/new?asset_id={}",s.btc,s.btc);
    let shown=render();assert!(shown.contains(&link),"{shown}");
    // D-LIQUIDATION-ONE-LINE: every blocker keeps the links; bot--selling makes them inert from the page's selling state.
    for (key,value) in [("liquidation_selling_since",json!({"id":"queued","at":1,"holdings":[["BTC",s.btc]]})),("rebalance_pending",json!({"phase":"selling"})),("liquidation_pending",json!({"id":"unknown","state":"ambiguous","symbol":"BTC"})),("redeploy_pending",json!({"id":"r","state":"ambiguous"}))] {
        model::merge_transient_compact(&c,id,&[(key,value)]).unwrap();
        assert!(selling(&c,id).blocked,"{key}");
        let shown=render();assert!(shown.contains(&link),"{key}: {shown}");
        model::merge_transient_compact(&c,id,&[(key,Value::Null)]).unwrap();
    }
    for status in [3,7] {
        c.execute("UPDATE bots SET status=?1 WHERE id=?2",(status,id)).unwrap();
        assert!(!render().contains("liquidation/new?"),"{status}");
    }
}

#[tokio::test(flavor="current_thread")]
async fn a_sell_link_for_a_position_no_longer_held_answers_with_a_flash_in_the_modal_frame() {
    let (_d,_c,_s,id,app,mut b)=install();
    let page=b.send(&app,"GET",&format!("/bots/{id}/liquidation/new?asset_id=999&symbol=DOGE"),None,Csrf::None,&[("Turbo-Frame","modal")]).await;
    assert_eq!(page.status,200,"{}",page.body);
    assert!(page.body.contains("<turbo-frame id=\"modal\"><turbo-stream action=\"prepend\" target=\"flash\"><template>"),"{}",page.body);
    assert!(page.body.contains("DOGE is no longer held."),"{}",page.body);
    // Nothing named at all is still a 404.
    assert_eq!(b.get(&app,&format!("/bots/{id}/liquidation/new")).await.status,404);
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

/// A20d, D-IDEMPOTENCY-DEAD-CLAIM: the write committed and the process died before the key completed. The next start
/// stores placement_ambiguous on the key; the retry runs no second write, and another body is still a reuse.
#[tokio::test(flavor="current_thread")]
async fn a_liquidation_key_its_process_died_holding_answers_placement_ambiguous_and_never_writes_again() {
    let (_d,c,s,id,app,_)=install();credentials(&c,s.user_id);
    let path=format!("/api/v1/bots/{id}/liquidations");
    let body=json!({"symbol":"BTC"});
    assert_eq!(wire_with_key(&app,&path,"liquidation-api",body.clone(),None,Some("dead")).await.0,202);
    c.execute("UPDATE idempotency_keys SET state='in_progress',response_status=NULL,response_body=NULL WHERE key='dead'",[]).unwrap();
    model::merge_transient_compact(&c,id,&[("liquidation_selling_since",Value::Null)]).unwrap();
    deltabadger::engine::api_orders::sweep_at_start(&c,NOW.parse().unwrap()).unwrap(); // what Engine::new does
    let retry=wire_with_key(&app,&path,"liquidation-api",body.clone(),None,Some("dead")).await;
    assert_eq!((retry.0,serde_json::from_str::<Value>(&retry.2).unwrap()),(502,json!({"data":null,"error":{"code":"placement_ambiguous",
        "message":"The order may or may not have been placed: the request was cut off by a restart. Check the exchange before sending it again."}})));
    assert!(model::load_bot(&c,id).unwrap().transient["liquidation_selling_since"].is_null(),"no second sale queued");
    let changed=wire_with_key(&app,&path,"liquidation-api",json!({"symbol":"ETH"}),None,Some("dead")).await;
    assert_eq!(changed.0,409);assert!(changed.2.contains("idempotency_key_reused"));
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

// D-LIQUIDATION-ONE-LINE (PLAN-liquidation-one-line slice 1).
use deltabadger::engine::{composition::{self,Selling},events::EngineEvent};
fn selling(c:&Connection,id:i64)->Selling { composition::selling(c,&model::load_bot(c,id).unwrap(),NOW.parse().unwrap()).unwrap() }
fn liquidation_row(c:&Connection,s:&seed::Seeded,id:i64,asset:i64,external_status:i64)->i64 {
    let tx=seed::insert_row(c,s,id,asset,&json!({"external_id":format!("L{external_status}"),"external_status":external_status,"side":1,"amount":"0.01","price":"64000","quote_amount":"640","created_at":"2026-09-01 12:00:00"}));
    c.execute("UPDATE transactions SET transaction_type='LIQUIDATION' WHERE id=?1",[tx]).unwrap();
    tx
}
fn log(c:&Connection,id:i64,event:&str,details:Value)->i64 {
    c.execute("INSERT INTO bot_activity_logs(bot_id,event,level,details,created_at) VALUES (?1,?2,0,?3,'2026-09-01 12:20:00')",(id,event,details.to_string())).unwrap();
    c.last_insert_rowid()
}

#[test]
fn selling_is_read_from_the_request_the_waiting_orders_and_every_blocker() {
    let (_d,c,s,id,_,_)=install();
    assert_eq!(selling(&c,id),Selling::default());
    // The request's holdings by asset id: a ledger key like ABC#123 and two keys of one symbol stay distinct assets.
    model::merge_transient_compact(&c,id,&[("liquidation_selling_since",json!({"id":"x","at":0,"holdings":[["BTC",s.btc],["ABC#123",901],["BTC",s.btc],["ABC",902]]}))]).unwrap();
    assert_eq!(selling(&c,id),Selling {assets:vec![s.btc,901,902],blocked:true});
    // The request cleared, its order still waiting on the venue.
    model::merge_transient_compact(&c,id,&[("liquidation_selling_since",Value::Null)]).unwrap();
    let tx=liquidation_row(&c,&s,id,s.btc,1);
    assert_eq!(selling(&c,id),Selling {assets:vec![s.btc],blocked:true});
    // An ambiguous placement and an abandoned unresolved order name nothing and still block.
    c.execute("UPDATE transactions SET external_status=2 WHERE id=?1",[tx]).unwrap();
    assert_eq!(selling(&c,id),Selling::default());
    model::merge_transient_compact(&c,id,&[("liquidation_pending",json!({"id":"abc","state":"ambiguous","symbol":"BTC"}))]).unwrap();
    assert_eq!(selling(&c,id),Selling {assets:vec![],blocked:true});
    model::merge_transient_compact(&c,id,&[("liquidation_pending",Value::Null)]).unwrap();
    c.execute("UPDATE transactions SET external_status=4 WHERE id=?1",[tx]).unwrap();
    assert_eq!(selling(&c,id),Selling {assets:vec![],blocked:true});
    // A rebalance in flight blocks too.
    c.execute("UPDATE transactions SET external_status=2 WHERE id=?1",[tx]).unwrap();
    model::merge_transient_compact(&c,id,&[("rebalance_pending",json!({"phase":"selling"}))]).unwrap();
    assert_eq!(selling(&c,id),Selling {assets:vec![],blocked:true});
}

#[tokio::test(flavor="current_thread")]
async fn the_page_shows_selling_rows_and_state_and_every_live_event_redraws_them() {
    let (_d,c,s,id,app,mut b)=install();
    let page=b.get(&app,&format!("/bots/{id}")).await;
    assert!(page.body.contains(&format!("<tbody id=\"orders_selling_{id}\"></tbody>")),"{}",page.body);
    assert!(page.body.contains(&format!("id=\"selling_state_{id}\" hidden data-bot--selling-target=\"state\" data-assets=\"[]\" data-blocked=\"false\"")),"{}",page.body);
    let answer=b.send(&app,"POST",&format!("/bots/{id}/liquidation"),Some(&[("symbol[]","BTC")]),Csrf::Header,HEADERS).await;
    assert_eq!(answer.status,200,"{}",answer.body);
    let page=b.get(&app,&format!("/bots/{id}")).await;
    assert!(page.body.contains("Selling BTC…") && page.body.contains("loader--small"),"{}",page.body);
    assert!(page.body.contains(&format!("data-assets=\"[{}]\" data-blocked=\"true\"",s.btc)),"{}",page.body);
    for event in [EngineEvent::TickEnded {bot_id:id},EngineEvent::StatusChanged {bot_id:id}] {
        let messages=deltabadger::web::bot::live::render(&c,&app,&event).unwrap();
        let ours:Vec<_>=messages.iter().filter(|(_,html)|html.contains(&format!("target=\"orders_selling_{id}\""))||html.contains(&format!("target=\"selling_state_{id}\""))).collect();
        assert_eq!(ours.len(),2,"{event:?}: {messages:?}");
        assert!(ours.iter().all(|(stream,html)|*stream==format!("user_{}:bot_updates",s.user_id) && html.contains("action=\"replace\"")));
        assert!(ours[0].1.contains("Selling BTC…"));
    }
    // The request is taken and cleared: the redraw empties the rows and lifts the block.
    c.execute("UPDATE bots SET transient_data=json_remove(transient_data,'$.liquidation_selling_since') WHERE id=?1",[id]).unwrap();
    let messages=deltabadger::web::bot::live::render(&c,&app,&EngineEvent::TickEnded {bot_id:id}).unwrap();
    assert!(messages.iter().any(|(_,html)|html.contains(&format!("<tbody id=\"orders_selling_{id}\"></tbody>"))),"{messages:?}");
    assert!(messages.iter().any(|(_,html)|html.contains("data-blocked=\"false\"")),"{messages:?}");
}

#[test]
fn a_signal_bots_live_events_carry_no_selling_targets() {
    let (_d,c,s,_,app,_)=install();
    let signal=seed::insert_bot(&c,&s,&BotSpec {status:2,..BotSpec::weekly(60.0,"2026-09-01 10:00:00")});
    c.execute("UPDATE bots SET type=?2 WHERE id=?1",(signal,deltabadger::engine::model::SIGNAL_TYPE)).unwrap();
    let messages=deltabadger::web::bot::live::render(&c,&app,&EngineEvent::TickEnded {bot_id:signal}).unwrap();
    assert!(!messages.iter().any(|(_,html)|html.contains("orders_selling_")||html.contains("selling_state_")),"{messages:?}");
}

#[tokio::test(flavor="current_thread")]
async fn new_lines_are_prepended_once_in_order_and_skipped_orders_never() {
    let (_d,c,_s,id,app,mut b)=install();
    let tick=EngineEvent::TickEnded {bot_id:id};
    let old=log(&c,id,"liquidation_failed",json!({"reason":"old"}));
    // The follower starts above what is already in the table: an old line is the feed's, not a broadcast.
    let mut lines=deltabadger::web::bot::live::Lines::starting(&c).unwrap();
    // The door refusal: a second Sell while the first waits.
    for _ in 0..2 { b.send(&app,"POST",&format!("/bots/{id}/liquidation"),Some(&[("symbol[]","BTC")]),Csrf::Header,HEADERS).await; }
    log(&c,id,"order_skipped",json!({}));
    let skipped=log(&c,id,"liquidation_skipped",json!({"base":"ETH","reason":"no_free_balance"}));
    let messages=lines.announce(&c,&app,&tick).unwrap();
    let prepends:Vec<&String>=messages.iter().filter(|(_,html)|html.contains("action=\"prepend\"")).map(|(_,html)|html).collect();
    assert_eq!(prepends.len(),2,"{messages:?}");
    assert!(prepends[0].contains("Nothing was sold — the bot is still busy with an earlier sale or rebalance"),"{}",prepends[0]);
    assert!(prepends[1].contains(&format!("bot_activity_log_{skipped}")) && prepends[1].contains("ETH was not sold — none of it is free to sell on the exchange"),"{}",prepends[1]);
    assert!(!messages.iter().any(|(_,html)|html.contains(&format!("bot_activity_log_{old}"))));
    assert!(messages.iter().any(|(_,html)|html.contains("action=\"remove\"") && html.contains("orders_list_placeholder")));
    assert!(messages.iter().any(|(_,html)|html.contains("order_filters")));
    // Announced once.
    assert_eq!(lines.announce(&c,&app,&EngineEvent::OrderWritten {bot_id:id,transaction_id:0}).unwrap(),vec![]);
}

#[tokio::test(flavor="current_thread")]
async fn the_feed_names_each_reason_and_keeps_legacy_lines() {
    let (_d,c,s,id,app,mut b)=install();
    for (event,details) in [("liquidation_skipped",json!({"base":"ETH","reason":"unpriced"})),("liquidation_skipped",json!({"base":"SOL","reason":"no_free_balance"})),
                            ("liquidation_skipped",json!({"base":"XRP"})),("liquidation_skipped",json!({"base":"ADA","reason":"something new"})),
                            ("liquidation_not_started",json!({"reason":["orders_waiting"]})),("liquidation_not_started",json!({"reason":"archived"})),
                            ("liquidation_not_started",json!({"reason":["prices are 3 hours old"]})),("liquidation_requested",json!({"base":"BTC"})),("liquidation_placed",json!({"base":"BTC"}))] {
        log(&c,id,event,details);
    }
    let feed=|locale:&'static str|{c.execute("UPDATE users SET locale=?2 WHERE id=?1",(s.user_id,locale)).unwrap();};
    feed("en");
    let en=b.send(&app,"GET",&format!("/bots/{id}.turbo_stream"),None,Csrf::None,&[("Turbo-Frame","orders_pagination")]).await.body;
    for text in ["ETH was not sold — no price could be read for it","SOL was not sold — none of it is free to sell on the exchange",
                 "Nothing was sold — the bot has orders still waiting on the exchange","Nothing was sold — the bot is archived",
                 "Selling a position at market","Selling a position"] { assert!(en.contains(text),"{text}: {en}"); }
    // A legacy skip (no reason), an unknown code and a staleness sentence keep the shared lines.
    assert_eq!(en.matches("A position could not be sold — left in place").count(),2,"{en}");
    assert_eq!(en.matches("Nothing was sold — the bot was busy, or the index could not be refreshed").count(),1,"{en}");
    feed("de");
    let de=b.send(&app,"GET",&format!("/de/bots/{id}.turbo_stream"),None,Csrf::None,&[("Turbo-Frame","orders_pagination")]).await.body;
    assert!(de.contains("ETH wurde nicht verkauft — es konnte kein Preis gelesen werden"),"{de}");
    assert!(de.contains("Nichts wurde verkauft — der Bot hat noch wartende Orders an der Börse"),"{de}");
}

#[tokio::test(flavor="current_thread")]
async fn an_orders_event_announces_only_the_lines_below_it_and_never_the_rails_order_answers() {
    let (_d,c,s,id,app,_)=install();
    let mut lines=deltabadger::web::bot::live::Lines::starting(&c).unwrap();
    // A batch: order A, then a skip, then order B a minute later; both orders' events are read after all three writes.
    let a=liquidation_row(&c,&s,id,s.btc,1);
    c.execute("UPDATE transactions SET external_id='LA' WHERE id=?1",[a]).unwrap();
    let early=log(&c,id,"liquidation_skipped",json!({"base":"ETH","reason":"unavailable"}));
    c.execute("UPDATE bot_activity_logs SET created_at='2026-09-01 12:00:30' WHERE id=?1",[early]).unwrap();
    let b=liquidation_row(&c,&s,id,s.btc,1);
    c.execute("UPDATE transactions SET created_at='2026-09-01 12:01:00' WHERE id=?1",[b]).unwrap();
    let late=log(&c,id,"rails_order_open",json!({"when":"x","venue":"Alpaca","orders":"1","obligation":1}));
    let prepended=|messages:&[(String,String)]|messages.iter().filter(|(_,html)|html.contains("action=\"prepend\"")).map(|(_,html)|html.clone()).collect::<Vec<_>>();
    // A's event: nothing older than A.
    assert_eq!(prepended(&lines.announce(&c,&app,&EngineEvent::OrderRecorded {bot_id:id,transaction_id:a}).unwrap()),Vec::<String>::new());
    // B's event: the skip, older than B, goes first so B lands above it.
    let at_b=prepended(&lines.announce(&c,&app,&EngineEvent::OrderRecorded {bot_id:id,transaction_id:b}).unwrap());
    assert!(at_b.len()==1 && at_b[0].contains(&format!("bot_activity_log_{early}")),"{at_b:?}");
    // The tick's end: the rest, except a line whose answers need the session.
    let ended=lines.announce(&c,&app,&EngineEvent::TickEnded {bot_id:id}).unwrap();
    assert!(!ended.iter().any(|(_,html)|html.contains(&format!("bot_activity_log_{late}"))),"{ended:?}");
    assert_eq!(lines.announce(&c,&app,&EngineEvent::TickEnded {bot_id:id}).unwrap(),vec![]);
}

#[test]
fn with_balances_hidden_the_selling_redraw_carries_the_filters() {
    let (_d,c,s,id,app,_)=install();
    let filters=|c:&Connection|deltabadger::web::bot::live::render(c,&app,&EngineEvent::TickEnded {bot_id:id}).unwrap().into_iter().filter(|(_,html)|html.contains("order_filters")).count();
    assert_eq!(filters(&c),0);
    c.execute("UPDATE users SET hide_balances=1 WHERE id=?1",[s.user_id]).unwrap();
    model::merge_transient_compact(&c,id,&[("liquidation_selling_since",json!({"id":"x","at":0,"holdings":[["BTC",s.btc]]}))]).unwrap();
    assert_eq!(filters(&c),1);
}
