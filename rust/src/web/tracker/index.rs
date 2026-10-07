//! Early index views only. Populated pages and date-filtered views stay explicitly deferred.
use super::{auth,layout,Ctx,App,WebError};
use crate::web::{shell::{self,Shell},i18n::{self,Arg}};
use axum::{extract::{Extension,State},http::StatusCode,response::Response};
use askama::Template;
use rusqlite::Connection;

#[derive(Template)]
#[template(path="tracker/early.html")]
struct Early<'a>{v:&'a Ctx,preferences:String,tax_report:String,sync:String,missing:bool,connect:String}

fn needs_market_data(c:&Connection,owner:i64)->Result<bool,WebError>{
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND exchange_id NOT IN (SELECT id FROM exchanges WHERE type IN ('Exchanges::Alpaca','Exchanges::Ibkr'))) OR EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1 AND exchange_id NOT IN (SELECT id FROM exchanges WHERE type IN ('Exchanges::Alpaca','Exchanges::Ibkr')))",[owner],|r|r.get(0))?)
}

fn render(c:&Connection,ctx:&Ctx,owner:i64)->Result<Response,WebError>{
    let missing=!crate::web::bots::market_data(c,&ctx.app)?.1 && needs_market_data(c,owner)?;
    if !missing {
        // Only scope IDs used by this slice are interpreted. Date parsing and populated filters
        // arrive with the populated index. A refusal never claims that those pages are empty.
        if let Some(raw)=ctx.params.query("exchange_id").filter(|s|!s.trim().is_empty()) {
            let Some(id)=crate::web::bot::id_from_path(raw) else{return Ok(layout::missing())};
            let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM exchanges WHERE id=?1)",[id],|r|r.get(0))?;
            if !exists{return Ok(layout::missing());}
        }
        if ["from","to"].iter().any(|key|ctx.params.query(key).is_some_and(|v|!v.trim().is_empty())) || ctx.params.query.iter().any(|(k,_)| k.starts_with("exchange_id[") || k.starts_with("from[") || k.starts_with("to[")) {
            return Ok(layout::refused(ctx,"tracker date and structured filters"));
        }
        let populated:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1) OR EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1)",[owner],|r|r.get(0))?;
        let settings:Option<String>=c.query_row("SELECT tracker_settings FROM users WHERE id=?1",[owner],|r|r.get(0))?;
        let settings:serde_json::Value=match settings {Some(text)=>serde_json::from_str(&text).map_err(|_|super::row::invalid())?,None=>serde_json::Value::Null};
        if populated || settings.get("pending_report").is_some_and(serde_json::Value::is_object) {
            return Ok(layout::refused(ctx,"populated tracker index and pending reports"));
        }
    }
    let Some(user)=ctx.user() else{return Ok(auth::unauthenticated(ctx))};
    let connect=i18n::t(ctx.locale,"bot.setup.connect_to",&[("name",Arg::Text("CoinGecko"))]);
    let body=Early{v:ctx,preferences:format!("user_{owner}:preferences"),tax_report:format!("user_{owner}:tax_report"),sync:format!("user_{owner}:sync"),missing,connect}.render()?;
    let page=layout::Page{status:StatusCode::OK,body,flash_now:vec![]};
    if ctx.turbo_frame.is_some() {return layout::frame(&ctx.csrf_token(),&page);}
    let shell=Shell::load(c,&ctx.app,user)?;
    shell::application_with_flash_extra(ctx,&ctx.csrf_token(),user,&shell,page,"  <div id=\"sync-warnings\" data-turbo-permanent>\n    \n  </div>\n")
}

pub async fn index(State(app):State<App>,Extension(ctx):Extension<Ctx>)->Result<Response,WebError>{
    let Some(owner)=ctx.user().map(|u|u.id)else{return Ok(auth::unauthenticated(&ctx))};
    app.db(move|c|super::read::only(c,|c|render(c,&ctx,owner))).await
}
