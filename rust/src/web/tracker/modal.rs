//! The export preferences view: a pure read, including unsaved classification proposals.
use super::{auth,App,Ctx,WebError,row::invalid};
use axum::{extract::{Extension,State},http::header,response::{IntoResponse,Response}};
use askama::Template;
use chrono::Datelike;
use rusqlite::Connection;
use serde_json::Value;

pub const COUNTRIES:[&str;18]=["DE","AT","FR","IT","ES","BG","GR","NL","PT","CH","PL","GB","US","SE","IE","DK","CZ","SK"];
#[derive(Template)]
#[template(path="tracker/export_modal.html")]
struct Modal<'a>{ctx:&'a Ctx,countries:String,years:String,checked_tax:&'static str,checked_transactions:&'static str,checked_stable:&'static str,hide_tax:&'static str,hide_transactions:&'static str,from:String,to:String,label:String,scope:String,panel:String}
fn text(v:&Value)->String{match v{Value::Null=>String::new(),Value::String(s)=>s.clone(),value=>value.to_string()}}
fn render(c:&Connection,ctx:&Ctx,owner:i64)->Result<String,WebError>{
    let stored:Option<String>=c.query_row("SELECT tracker_settings FROM users WHERE id=?1",[owner],|r|r.get(0))?;
    let settings:Value=match stored{Some(s)=>serde_json::from_str(&s).map_err(|_|invalid())?,None=>serde_json::json!({})};
    if !settings.is_object(){return Err(invalid())}
    let transactions=settings["export_type"]=="transactions";
    let checked=|yes|if yes{"checked"}else{""};let hidden=|yes|if yes{"hidden"}else{""};
    let mut countries=String::new();
    for code in COUNTRIES{
        countries.push_str(&format!("              <option value=\"{code}\" {} data-stablecoin-ambiguous=\"{}\">{}</option>\n",if settings["country"]==code{"selected"}else{""},matches!(code,"AT"|"SK"),ctx.t(&format!("tracker.export_modal.countries.{code}"))));
    }
    let year=ctx.now.year();
    let selected=if settings["year"].is_null(){i64::from(year-1)}else{crate::ruby::to_i(&text(&settings["year"]))};
    let mut offered:Vec<i32>=(year-5..=year).chain(2023..=2025).collect();offered.sort_unstable();offered.dedup();
    let mut years=String::new();
    for y in offered.into_iter().rev(){
        let scopes=[((year-5..=year).contains(&y),"crypto"),((2023..=2025).contains(&y),"broker")].iter().filter(|(yes,_)|*yes).map(|(_,scope)|*scope).collect::<Vec<_>>().join(" ");
        years.push_str(&format!("              <option value=\"{y}\" data-scopes=\"{scopes}\" {}>{y}</option>\n",if selected==i64::from(y){"selected"}else{""}));
    }
    let (earliest,latest):(Option<String>,Option<String>)=c.query_row("SELECT min(transacted_at),max(transacted_at) FROM account_transactions WHERE user_id=?1",[owner],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let day=|value:Option<String>|->Result<Option<String>,WebError>{value.map(|s|crate::codec::parse_time(&s).map(|t|t.date_naive().to_string()).map_err(|_|invalid())).transpose()};
    let from=if settings["export_from"].is_null() || settings["export_from"]==false{day(earliest)?.unwrap_or(String::new())}else{text(&settings["export_from"])};
    let to=if settings["export_to"].is_null() || settings["export_to"]==false{day(latest)?.unwrap_or(ctx.now.date_naive().to_string())}else{text(&settings["export_to"])};
    let (scope,panel)=super::classifications::panel(c,ctx,owner,&settings)?;
    Ok(Modal{ctx,countries,years,checked_tax:checked(!transactions),checked_transactions:checked(transactions),checked_stable:checked(!settings["stablecoin_as_fiat"].is_null() && settings["stablecoin_as_fiat"]!=false),hide_tax:hidden(transactions),hide_transactions:hidden(!transactions),from,to,label:ctx.t(if transactions{"tracker.export_modal.download_label"}else{"tracker.export_modal.generate"}),scope,panel}.render()?)
}
pub async fn export_modal(State(app):State<App>,Extension(ctx):Extension<Ctx>)->Result<Response,WebError>{
    let Some(owner)=ctx.user().map(|u|u.id)else{return Ok(auth::unauthenticated(&ctx))};
    app.db(move|c|{
        if !crate::web::bots::market_data(c,&ctx.app)?.1{return Ok(super::layout::refused(&ctx,"tracker market-data setup"))}
        Ok(([(header::CONTENT_TYPE,"text/html; charset=utf-8")],render(c,&ctx,owner)?).into_response())
    }).await
}

/// The modal fetches reports as Turbo streams. Keep the deferred action visibly refused.
pub async fn deferred_report(Extension(ctx):Extension<Ctx>,headers:axum::http::HeaderMap)->Result<Response,WebError>{
    if ctx.user().is_none(){return Ok(auth::unauthenticated(&ctx))}
    let response=super::layout::refused(&ctx,"tracker tax reports");
    if !super::super::header_text(&headers,"accept").is_some_and(|s|s.contains("text/vnd.turbo-stream.html")){return Ok(response)}
    let body=axum::body::to_bytes(response.into_body(),64*1024).await.map_err(|_|invalid())?;
    let body=std::str::from_utf8(&body).map_err(|_|invalid())?;
    Ok((axum::http::StatusCode::NOT_IMPLEMENTED,[(header::CONTENT_TYPE,super::super::turbo::CONTENT_TYPE)],super::super::turbo::stream("append","flash",body)).into_response())
}
