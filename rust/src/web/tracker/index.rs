//! Early index views only. Populated pages and date-filtered views stay explicitly deferred.
use super::{auth,layout,Ctx,App,WebError};
use crate::web::{shell::{self,Shell},i18n::{self,Arg}};
use axum::{extract::{Extension,State},http::StatusCode,response::Response};
use askama::Template;
use rusqlite::Connection;

#[derive(Template)]
#[template(path="tracker/early.html")]
struct Early<'a>{v:&'a Ctx,preferences:String,tax_report:String,sync:String,missing:bool,connect:String,pending:String}

fn needs_market_data(c:&Connection,owner:i64)->Result<bool,WebError>{
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND exchange_id NOT IN (SELECT id FROM exchanges WHERE type IN ('Exchanges::Alpaca','Exchanges::Ibkr'))) OR EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1 AND exchange_id NOT IN (SELECT id FROM exchanges WHERE type IN ('Exchanges::Alpaca','Exchanges::Ibkr')))",[owner],|r|r.get(0))?)
}

fn render(c:&Connection,ctx:&Ctx,owner:i64,fx:Option<&super::fx::Prepared>)->Result<Response,WebError>{
    let missing=!crate::web::bots::market_data(c,&ctx.app)?.1 && needs_market_data(c,owner)?;
    if !missing {
        // Only scope IDs used by this slice are interpreted. Date parsing and populated filters
        // arrive with the populated index. A refusal never claims that those pages are empty.
        if let Some(raw)=ctx.params.query("exchange_id").filter(|s|!s.trim().is_empty()) {
            let Some(id)=crate::web::bot::id_from_path(raw) else{return Ok(layout::missing())};
            let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM exchanges WHERE id=?1)",[id],|r|r.get(0))?;
            if !exists{return Ok(layout::missing());}
        }
        if ctx.params.query.iter().any(|(k,_)| k.starts_with("exchange_id[") || k.starts_with("from[") || k.starts_with("to[")) {
            return Ok(layout::refused(ctx,"tracker date and structured filters"));
        }
        let populated:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1) OR EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1)",[owner],|r|r.get(0))?;
        let settings:Option<String>=c.query_row("SELECT tracker_settings FROM users WHERE id=?1",[owner],|r|r.get(0))?;
        let settings:serde_json::Value=match settings {Some(text)=>serde_json::from_str(&text).map_err(|_|super::row::invalid())?,None=>serde_json::Value::Null};
        let pending=settings.get("pending_report").is_some_and(serde_json::Value::is_object);
        if populated {
            // The first-sync and cold first-day pages state their own never-a-number reasons; a pending report is
            // read by the full page only.
            if !pending && super::first_sync::supported(c,owner)? {return super::first_sync::render(c,ctx,owner);}
            if !pending && super::holdings::supported(c,owner)? {
                if let Some(fx)=fx {let cold=super::holdings::prepare(c,ctx,owner,fx)?;return super::first_sync::render_with(c,ctx,owner,Some(cold));}
            }
            // Date filters are the next slice: a window is never served as the whole record.
            if ["from","to"].iter().any(|key|ctx.params.query(key).is_some_and(|v|!v.trim().is_empty())) {
                return Ok(layout::refused(ctx,"tracker date filters"));
            }
            let Some(fx)=fx else{return Ok(layout::refused(ctx,"tracker display currency"))};
            return super::populated::page(c,ctx,owner,fx);
        }
        if ["from","to"].iter().any(|key|ctx.params.query(key).is_some_and(|v|!v.trim().is_empty())) {
            return Ok(layout::refused(ctx,"tracker date and structured filters"));
        }
    }
    let Some(user)=ctx.user() else{return Ok(auth::unauthenticated(ctx))};
    // `check_pending_report` runs after the market-data check returned: a missing provider shows no report.
    let pending=if missing{String::new()}else{
        let settings:Option<String>=c.query_row("SELECT tracker_settings FROM users WHERE id=?1",[owner],|r|r.get(0))?;
        let settings:serde_json::Value=match settings {Some(text)=>serde_json::from_str(&text).map_err(|_|super::row::invalid())?,None=>serde_json::Value::Null};
        super::populated::pending_report(ctx,owner,&settings)?
    };
    let connect=i18n::t(ctx.locale,"bot.setup.connect_to",&[("name",Arg::Text("CoinGecko"))]);
    let body=Early{v:ctx,preferences:format!("user_{owner}:preferences"),tax_report:format!("user_{owner}:tax_report"),sync:format!("user_{owner}:sync"),missing,connect,pending}.render()?;
    let page=layout::Page{status:StatusCode::OK,body,flash_now:vec![]};
    if ctx.turbo_frame.is_some() {return layout::frame(&ctx.csrf_token(),&page);}
    let shell=Shell::load(c,&ctx.app,user)?;
    shell::application_with_flash_extra(ctx,&ctx.csrf_token(),user,&shell,page,"  <div id=\"sync-warnings\" data-turbo-permanent>\n    \n  </div>\n")
}

/// `load_ledgers` and `load_history`: the jobs this page asks for, past the market-data check, for any account (Rails
/// runs both loaders whether or not anything is connected). The ledger walks while `Ledger.cached` is nil, warming or
/// failed. The sweep is asked for by its own gate, `backfill::wanted`, which covers the page's test for every scope;
/// unlike `stale_prices?` it ignores a partial row outside the window (today's, the sync's to rewrite), which no sweep
/// writes. A gate that cannot be read drops its own wake only.
fn wakes(c:&Connection,ctx:&Ctx,owner:i64)->Result<Vec<(&'static str,String)>,WebError>{
    use crate::tracker::{backfill,cache,jobs};
    if !crate::web::bots::market_data(c,&ctx.app)?.1 && needs_market_data(c,owner)? {return Ok(vec![]);}
    let now=ctx.app.now();
    let mut out=vec![];
    if !matches!(crate::figures::budget::within(||cache::read(c,owner,None,now)),Ok(cache::State::Warm(_))) {out.push((jobs::TRACKER_LEDGER,owner.to_string()));}
    match crate::figures::budget::within(||backfill::wanted(c,&ctx.app.cipher,owner,now.date_naive())) {
        Ok(true)=>out.push((jobs::PORTFOLIO_BACKFILL,owner.to_string())),
        Ok(false)=>{}
        Err(e)=>crate::engine::log(&format!("[tracker] backfill check for user {owner} failed: {e:?}")),
    }
    Ok(out)
}

pub async fn index(State(app):State<App>,Extension(ctx):Extension<Ctx>)->Result<Response,WebError>{
    let Some(owner)=ctx.user().map(|u|u.id)else{return Ok(auth::unauthenticated(&ctx))};
    // The display currency's rate as held: a page never fetches it (`fx::held`). Only a page served asks for jobs.
    let (page,wanted)=app.db(move|c|super::read::only(c,|c|{
        let fx=super::fx::held(&ctx.app,c,owner)?;
        let page=render(c,&ctx,owner,Some(&fx))?;
        let wanted=if page.status()==StatusCode::OK {wakes(c,&ctx,owner)?} else {vec![]};
        Ok((page,wanted))
    })).await?;
    for (name,scope) in wanted {app.wake_job(name,&scope);}
    Ok(page)
}
