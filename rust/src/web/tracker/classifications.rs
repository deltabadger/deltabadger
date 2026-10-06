//! Read-only broker classification proposals, the same cumulative universe as Rails' modal.
use super::{Ctx,WebError,row::invalid};
use crate::web::i18n::{self,escape,Arg};
use rusqlite::{Connection,OptionalExtension};
use serde_json::Value;
use std::collections::{BTreeMap,BTreeSet};
const KINDS:[&str;3]=["share","fund","other_security"];
const CATEGORIES:[&str;5]=["equity_fund","mixed_fund","real_estate_fund","foreign_real_estate_fund","other_fund"];
struct Classification {symbol:String,kind:Option<usize>,category:Option<usize>,persisted:bool,reasons:Vec<&'static str>}
impl Classification{
    fn group(&self)->&'static str{if self.kind.is_none(){"unclassified"}else if !self.reasons.is_empty(){"refused"}else if self.kind==Some(1) && !self.persisted{"proposed_fund"}else{"settled"}}
    fn kind(&self)->&str{self.kind.and_then(|k|KINDS.get(k).copied()).unwrap_or("")}
    fn category(&self)->&str{self.category.and_then(|k|CATEGORIES.get(k).copied()).unwrap_or("")}
}
fn rows(c:&Connection,owner:i64,exchange:i64)->Result<Vec<Classification>,WebError>{
    let mut statement=c.prepare("SELECT entry_type,base_currency,quote_currency,transacted_at FROM account_transactions WHERE user_id=?1 AND exchange_id=?2 AND transacted_at<'2026-01-01 00:00:00' ORDER BY transacted_at")?;
    let mut records=statement.query((owner,exchange))?;
    let mut universe:BTreeMap<String,Vec<(i64,String)>>=BTreeMap::new();
    while let Some(r)=records.next()?{
        let kind:i64=r.get(0)?;let symbol:Option<String>=if matches!(kind,11|13){r.get(2)?}else{r.get(1)?};
        if let Some(symbol)=symbol.filter(|s|!s.trim().is_empty() && !crate::tracker::fiat(s)) {universe.entry(symbol).or_default().push((kind,r.get(3)?));}
    }
    let mut out=vec![];
    for (symbol,records) in universe{
        let existing:Option<(i64,Option<i64>)>=c.query_row("SELECT kind,fund_category FROM fund_classifications WHERE user_id=?1 AND symbol=?2",(owner,&symbol),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let mut assets=c.prepare("SELECT category,instrument_type FROM assets WHERE symbol=?1 ORDER BY id")?;
        let assets:Vec<(Option<String>,Option<String>)>=assets.query_map([&symbol],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?;
        let categories:BTreeSet<&str>=assets.iter().filter_map(|(c,_)|c.as_deref()).collect();
        if existing.is_none() && !assets.iter().any(|(_,t)|t.as_deref().is_some_and(|t|matches!(t,"stock"|"etf"|"tokenized"))) && categories==BTreeSet::from(["Cryptocurrency"]){continue}
        let asset=assets.iter().find(|(_,t)|t.as_deref().is_some_and(|t|matches!(t,"stock"|"etf"))).or(assets.first());
        let (kind,category)=match existing {
            Some((k,cat))=>(Some(usize::try_from(k).map_err(|_|invalid())?),cat.map(usize::try_from).transpose().map_err(|_|invalid())?),
            None=>match asset.and_then(|(_,t)|t.as_deref()){Some("stock")=>(Some(0),None),Some("etf")=>(Some(1),Some(4)),_=>(None,None)},
        };
        if kind.is_some_and(|k|k>=KINDS.len()) || category.is_some_and(|k|k>=CATEGORIES.len()){return Err(invalid())}
        let mut reasons=vec![];
        if records.iter().any(|(kind,_)|*kind==16){reasons.push("unsupported_activity")}
        if kind==Some(1) && records.iter().any(|(kind,at)|*kind==0 && at.as_str()<"2018-01-01 00:00:00"){reasons.push("pre_2018_fund_lot")}
        out.push(Classification{symbol,kind,category,persisted:existing.is_some(),reasons});
    }
    Ok(out)
}
fn select(ctx:&Ctx,row:&Classification,category:bool)->String{
    let options=if category{&CATEGORIES[..]}else{&KINDS[..]};let role=if category{"category"}else{"kind"};let indent=if category{"    "}else{"  "};
    let mut out=if category{format!("  <div class=\"{}\">\n",if row.kind==Some(1){""}else{"hidden"})}else{String::new()};
    out.push_str(&format!("{indent}<select data-role=\"{role}\"\n{indent}        data-action=\"change->tracker-export#changeClassification\">\n{indent}  <option value=\"\">{}</option>\n",ctx.t("tracker.export_modal.classification_select")));
    for name in options{out.push_str(&format!("{indent}    <option value=\"{name}\" {}>{}</option>\n",if *name==if category{row.category()}else{row.kind()}{"selected"}else{""},ctx.t(&format!("tax_report.broker.{}.{name}",if category{"fund_categories"}else{"kinds"}))));}
    out.push_str(&format!("{indent}</select>\n"));if category{out.push_str("  </div>\n")}out
}
fn card(ctx:&Ctx,row:&Classification)->String{
    let group=row.group();let symbol=escape(&row.symbol);let kind=select(ctx,row,false);let category=select(ctx,row,true);
    if group=="settled"{return format!("  <tr data-tracker-export-target=\"classificationRow\" data-symbol=\"{symbol}\">\n    <td>{symbol}</td>\n    <td>{kind}</td>\n    <td>{category}</td>\n  </tr>\n")}
    let eyebrow=if group=="unclassified"{"unclassified"}else if group=="proposed_fund"{"fund"}else{"refused"};
    let mut why=String::new();
    if row.kind.is_none(){why.push_str(&format!("        <p class=\"fund-classification__why\">{}</p>\n",ctx.t("tracker.export_modal.classification_why_unclassified")))}
    if !row.reasons.is_empty(){let mut reasons:Vec<String>=row.reasons.iter().map(|r|ctx.t(&format!("tracker.export_modal.classification_refusal_reasons.{r}"))).collect();reasons.push(ctx.t("tracker.export_modal.classification_why_refused"));why.push_str(&format!("        <p class=\"fund-classification__why\">{}</p>\n",reasons.join(" ")))}
    if row.kind==Some(1) && !row.persisted{why.push_str(&format!("        <p class=\"fund-classification__why\">{}</p>\n",ctx.t("tracker.export_modal.classification_why_fund_html")))}
    let control=if group=="refused" && row.persisted{
        let category=if row.category.is_some(){format!("          <span class=\"fund-classification__static\">{}</span>\n",ctx.t(&format!("tax_report.broker.fund_categories.{}",row.category())))}else{String::new()};
        format!("        <span class=\"fund-classification__static\">{}: <b>{}</b></span>\n{category}        <input type=\"hidden\" data-role=\"kind\" value=\"{}\">\n        <input type=\"hidden\" data-role=\"category\" value=\"{}\">\n",ctx.t("tax_report.broker.headers.kind"),ctx.t(&format!("tax_report.broker.kinds.{}",row.kind())),row.kind(),row.category())
    }else{
        let hint=if row.kind==Some(1) && !row.persisted{format!("          <span class=\"fund-classification__hint\">{}</span>\n",ctx.t("tracker.export_modal.classification_fund_hint"))}else{String::new()};
        format!("        {kind}\n        {category}\n{hint}")
    };
    format!("  <div class=\"fund-classification__card fund-classification__card--{}\"\n       data-tracker-export-target=\"classificationRow\" data-symbol=\"{symbol}\">\n    <div class=\"fund-classification__id\">\n      <span class=\"fund-classification__eyebrow\">\n          {}\n      </span>\n      <span class=\"fund-classification__symbol\">{symbol}</span>\n{why}    </div>\n    <div class=\"fund-classification__control\">\n{control}    </div>\n  </div>\n",group.replace('_',"-"),ctx.t(&format!("tracker.export_modal.classification_eyebrow_{eyebrow}")))
}
fn classifications(ctx:&Ctx,rows:&[Classification])->String{
    if rows.is_empty(){return String::new()}
    let mut out=format!("<div data-tracker-export-target=\"classificationPanel\" class=\"form__row hidden\">\n  <h2>{}</h2>\n  <p>{}</p>\n",ctx.t("tracker.export_modal.classification_title"),ctx.t("tracker.export_modal.classification_hint"));
    if rows.iter().any(|r|r.group()!="settled"){
        out.push_str("    <div class=\"fund-classification\">\n");
        for group in ["unclassified","refused","proposed_fund"]{
            out.push_str("        ");for row in rows.iter().filter(|r|r.group()==group){out.push_str(&card(ctx,row))}out.push('\n');
        }
        out.push_str("    </div>\n");
    }
    let settled:Vec<_>=rows.iter().filter(|r|r.group()=="settled").collect();
    if !settled.is_empty(){
        out.push_str(&format!("    <details class=\"fund-classification__settled\">\n      <summary>{}</summary>\n      <table class=\"table\">\n        <thead>\n          <tr>\n",i18n::t(ctx.locale,"tracker.export_modal.classification_settled",&[("count",Arg::Count(settled.len() as i64))])));
        for key in ["symbol","kind","fund_category"]{out.push_str(&format!("            <th>{}</th>\n",ctx.t(&format!("tax_report.broker.headers.{key}"))))}
        out.push_str("          </tr>\n        </thead>\n        <tbody>\n          ");
        for row in settled{out.push_str(&card(ctx,row))}
        out.push_str("\n        </tbody>\n      </table>\n    </details>\n");
    }
    out.push_str("</div>\n");out
}
pub fn panel(c:&Connection,ctx:&Ctx,owner:i64,settings:&Value)->Result<(String,String),WebError>{
    let exchange:Option<i64>=c.query_row("SELECT e.id FROM exchanges e WHERE e.type='Exchanges::Alpaca' AND (EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND exchange_id=e.id) OR EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1 AND exchange_id=e.id)) ORDER BY e.id LIMIT 1",[owner],|r|r.get(0)).optional()?;
    let Some(exchange)=exchange else{return Ok((String::new(),String::new()))};
    let country=settings["country"].as_str().unwrap_or("DE");let broker=settings["report_scope"]=="broker" && country=="DE";
    let mut scope=format!("        <div class=\"flex form__row flex-center gap-3 {}\"\n             data-tracker-export-target=\"scopeRow\">\n",if settings["export_type"]=="transactions" || country!="DE"{"hidden"}else{""});
    for kind in ["crypto","broker"]{
        scope.push_str(&format!("          <label class=\"form__radio-group\">\n            <input type=\"radio\" name=\"report_scope\" value=\"{kind}\"\n                   data-action=\"tracker-export#toggle\"\n                   data-tracker-export-target=\"scopeRadio\"\n                   {}>\n            <span>{}</span>\n          </label>\n",if broker==(kind=="broker"){"checked"}else{""},ctx.t(&format!("tracker.export_modal.scope_{kind}"))));
    }
    scope.push_str("        </div>\n");
    Ok((scope,classifications(ctx,&rows(c,owner,exchange)?)))
}
