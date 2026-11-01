//! The export preferences view: a pure read, including unsaved classification proposals.
use super::{auth,App,Ctx,WebError,row::invalid};
use axum::{extract::{Extension,State},http::header,response::{IntoResponse,Response}};
use askama::Template;
use chrono::Datelike;
use rusqlite::Connection;
use serde_json::Value;
use crate::web::i18n::escape;

pub const COUNTRIES:[&str;18]=["DE","AT","FR","IT","ES","BG","GR","NL","PT","CH","PL","GB","US","SE","IE","DK","CZ","SK"];
#[derive(Template)]
#[template(path="tracker/export_modal.html")]
struct Modal<'a>{ctx:&'a Ctx,pl:String,countries:String,years:String,type_switch:String,export_type:&'static str,checked_stable:&'static str,hide_tax:&'static str,hide_transactions:&'static str,from:String,to:String,label:String,scope:String,panel:String}
/// D-TAX-PL-DECLARED-CARRY: the PL form for one report year: column d as declared (for 2019 the costs before 2019 never
/// deducted, later the previous return's column f), submitted with the request itself (Rails' Generate is hidden for PL),
/// so the amount is in the request that queues the report. `error`: the amount was refused (shown as typed).
pub fn pl_block(ctx:&Ctx,year:i64,value:&str,error:bool)->String{
    let label=if year==i64::from(crate::tax::pl::FIRST_YEAR){ctx.t("tracker.export_modal.pl_costs_before_2019")}
              else{escape(&crate::web::i18n::text(ctx.locale,"tracker.export_modal.pl_carried_costs",&[("year",crate::web::i18n::Arg::Text(&(year-1).to_string()))]))};
    let error=if error{format!("\n  <p class=\"salert salert--danger\">{}</p>",ctx.t("tracker.export_modal.pl_carried_invalid"))}else{String::new()};
    format!("<form id=\"pl-carry-{year}\" class=\"pl-carry\" data-action=\"turbo:submit-end-&gt;modal--base#submitEnd\" action=\"{}\" accept-charset=\"UTF-8\" method=\"post\">\
<input type=\"hidden\" name=\"authenticity_token\" value=\"{}\" /><input type=\"hidden\" name=\"year\" value=\"{year}\" />
  <div class=\"srow\"><div class=\"srow__text\"><div class=\"srow__label\">{label}</div></div><div class=\"srow__control\">
    <input type=\"text\" inputmode=\"decimal\" name=\"pl_carried_costs\" class=\"rinput rinput--outline rinput--text\" aria-label=\"{label}\" value=\"{}\">
  </div></div>
  <p class=\"text-inactive\">{}</p>{error}
  <div class=\"modal__footer\"><button type=\"submit\" class=\"rbutton rbutton--primary\">{}</button></div>
</form>\n",escape(&ctx.path("/tracker/tax_report")),escape(&ctx.csrf_token()),escape(value),ctx.t("tracker.export_modal.pl_carried_hint"),ctx.t("tracker.export_modal.generate"))
}

/// Every offered crypto year from 2019 gets its PL form; CSS (`:has`, no script: Rails' controller is unchanged) shows the
/// selected year's while the tax report and Poland are selected, and hides Rails' Generate then.
fn pl_blocks(c:&Connection,ctx:&Ctx,owner:i64,years:&[i32])->Result<String,WebError>{
    let mut s=c.prepare("SELECT year, costs FROM rust_tax_pl_carry WHERE user_id=?1")?;
    let stored:std::collections::HashMap<i64,String>=s.query_map([owner],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?;
    let years:Vec<i32>=years.iter().copied().filter(|y|*y>=crate::tax::pl::FIRST_YEAR).collect();
    let on="[data-controller=\"tracker-export\"]:has(input[name=export_type][value=tax_report]):has(select[name=country] option[value=PL]:checked)";
    let mut out=format!("      <style>.pl-carry{{display:none}} {on} [data-tracker-export-target=downloadBtn]{{display:none}}");
    for y in &years{out.push_str(&format!(" {on}:has(select[name=year] option[value=\"{y}\"]:checked) #pl-carry-{y}{{display:block}}"));}
    out.push_str("</style>\n");
    for y in years{let y=i64::from(y);out.push_str(&pl_block(ctx,y,stored.get(&y).map_or("",String::as_str),false));}
    Ok(out)
}
fn text(v:&Value)->String{match v{Value::Null=>String::new(),Value::String(s)=>s.clone(),value=>value.to_string()}}
fn render(c:&Connection,ctx:&Ctx,owner:i64)->Result<String,WebError>{
    let stored:Option<String>=c.query_row("SELECT tracker_settings FROM users WHERE id=?1",[owner],|r|r.get(0))?;
    let settings:Value=match stored{Some(s)=>serde_json::from_str(&s).map_err(|_|invalid())?,None=>serde_json::json!({})};
    if !settings.is_object(){return Err(invalid())}
    let transactions=settings["export_type"]=="transactions";
    let checked=|yes|if yes{"checked"}else{""};let hidden=|yes|if yes{"hidden"}else{""};
    let option=|value:&'static str,active:bool|crate::web::bots::SegmentedOption{value:value.into(),label:ctx.t(&format!("tracker.export_modal.{value}")),active,href:None};
    let type_switch=crate::web::bots::Segmented{fluid:true,label:ctx.t("tracker.export_modal.title"),key:None,frame:None,options:vec![option("tax_report",!transactions),option("transactions",transactions)],links:false}.render()?;
    let mut countries=String::new();
    for code in COUNTRIES{
        countries.push_str(&format!("              <option value=\"{code}\" {} data-stablecoin-ambiguous=\"{}\">{}</option>\n",if settings["country"]==code{"selected"}else{""},crate::tax::jurisdictions::find(code).is_some_and(|j|j.stablecoin_ambiguous),ctx.t(&format!("tracker.export_modal.countries.{code}"))));
    }
    let year=ctx.now.year();
    let selected=if settings["year"].is_null(){i64::from(year-1)}else{crate::ruby::to_i(&text(&settings["year"]))};
    let mut offered:Vec<i32>=(year-5..=year).chain(2023..=2025).collect();offered.sort_unstable();offered.dedup();
    let pl=pl_blocks(c,ctx,owner,&(year-5..=year).collect::<Vec<_>>())?;
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
    Ok(Modal{ctx,pl,countries,years,type_switch,export_type:if transactions{"transactions"}else{"tax_report"},checked_stable:checked(!settings["stablecoin_as_fiat"].is_null() && settings["stablecoin_as_fiat"]!=false),hide_tax:hidden(transactions),hide_transactions:hidden(!transactions),from,to,label:ctx.t(if transactions{"tracker.export_modal.download_label"}else{"tracker.export_modal.generate"}),scope,panel}.render()?)
}
pub async fn export_modal(State(app):State<App>,Extension(ctx):Extension<Ctx>)->Result<Response,WebError>{
    let Some(owner)=ctx.user().map(|u|u.id)else{return Ok(auth::unauthenticated(&ctx))};
    app.db(move|c|{
        if !crate::web::bots::market_data(c,&ctx.app)?.1{return Ok(super::layout::refused(&ctx,"tracker market-data setup"))}
        Ok(([(header::CONTENT_TYPE,"text/html; charset=utf-8")],render(c,&ctx,owner)?).into_response())
    }).await
}
