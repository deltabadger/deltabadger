//! The stored account record, never an order fill. Shared by row writes and the ledger table.
use super::{Ctx, WebError};
use crate::{codec, figures::{dec::Dec, num::NumError, totals::Denomination}, web::{format, i18n::{self, escape}, figure::NO_VALUE}};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

pub const TYPES: [&str;17] = ["buy","sell","swap_in","swap_out","deposit","withdrawal","staking_reward","lending_interest","airdrop","mining","fee","other_income","lost","withholding_tax","return_of_capital","adjustment","unsupported_activity"];
pub fn invalid() -> WebError { WebError::Config("invalid tracker record".into()) }
pub fn number<T>(r: Result<T,NumError>) -> Result<T,WebError> { r.map_err(|_|invalid()) }
pub struct Row {
    pub id:i64, pub owner:i64, pub exchange:i64, pub kind:i64, pub base:String, pub amount:Dec,
    pub quote:Option<String>, pub quoted:Option<Dec>, pub fee:Option<Dec>, pub fee_currency:Option<String>,
    pub at:chrono::DateTime<chrono::Utc>, pub group:Option<String>, pub manual:Value, pub manual_null:bool, pub linked:Option<i64>, pub inverse:Option<i64>,
    pub asset:Option<i64>, pub bot:Option<i64>, pub venue:String,
}
impl Row {
    pub fn load(c:&Connection, owner:i64,id:i64)->Result<Option<Self>,WebError> {
        let mut s=c.prepare("SELECT t.id,t.exchange_id,t.entry_type,t.base_currency,t.base_amount,t.quote_currency,t.quote_amount,t.fee_amount,t.fee_currency,t.transacted_at,t.group_id,t.manual_values,t.linked_transaction_id,(SELECT id FROM account_transactions WHERE linked_transaction_id=t.id),t.base_asset_id,(SELECT bot_id FROM transactions WHERE id=t.transaction_id),COALESCE(e.type,'') FROM account_transactions t LEFT JOIN exchanges e ON e.id=t.exchange_id WHERE t.user_id=?1 AND t.id=?2")?;
        let mut rows=s.query((owner,id))?;
        let Some(r)=rows.next()? else {return Ok(None)};
        let manual:Option<String>=r.get(11)?;
        let manual_null=manual.is_none();
        let manual=match manual {Some(s)=>serde_json::from_str(&s).map_err(|_|invalid())?,None=>serde_json::json!({})};
        Ok(Some(Self {id:r.get(0)?,owner,exchange:r.get(1)?,kind:r.get(2)?,base:r.get(3)?,amount:number(Dec::from_sql(r.get_ref(4)?))?.ok_or_else(invalid)?,
            quote:r.get(5)?,quoted:number(Dec::from_sql(r.get_ref(6)?))?,fee:number(Dec::from_sql(r.get_ref(7)?))?,fee_currency:r.get(8)?,
            at:codec::parse_time(&r.get::<_,String>(9)?).map_err(|_|invalid())?,group:r.get(10)?,manual,manual_null,linked:r.get(12)?,inverse:r.get(13)?,asset:r.get(14)?,bot:r.get(15)?,venue:r.get(16)?}))
    }
    pub fn linked(&self)->bool { self.linked.is_some() || self.inverse.is_some() }
    pub fn counterpart(&self,c:&Connection)->Result<Option<(Dec,String)>,WebError> {
        if self.quoted.is_some() && self.quote.as_deref().is_some_and(crate::tracker::cash) {
            return Ok(self.quoted.clone().zip(self.quote.clone()));
        }
        let Some(group)=self.group.as_deref().filter(|s|!s.trim().is_empty()) else{return Ok(None)};
        let mut s=c.prepare("SELECT id,entry_type,base_currency,base_amount FROM account_transactions WHERE user_id=?1 AND exchange_id=?2 AND group_id=?3")?;
        let mut q=s.query((self.owner,self.exchange,group))?;
        let mut count=0; let mut opposite=None;
        while let Some(r)=q.next()? {
            count+=1;
            let (id,kind,currency):(i64,i64,String)=(r.get(0)?,r.get(1)?,r.get(2)?);
            if id!=self.id && crate::tracker::cash(&currency) && ((matches!(self.kind,0|2) && matches!(kind,1|3)) || (matches!(self.kind,1|3) && matches!(kind,0|2))) {
                opposite=number(Dec::from_sql(r.get_ref(3)?))?.map(|d|(d,currency));
            }
        }
        Ok(if count==2 {opposite}else{None})
    }
}


fn amount(value:&Dec,currency:&str)->Result<String,WebError> {
    let absolute=if value.is_negative(){value.neg()}else{value.clone()};
    let cents=absolute>=Dec::one() || crate::tracker::cash(currency);
    let precision=if cents{2}else{8};
    let plain=number(value.round(precision))?.to_s_f();
    let (whole,fraction)=plain.split_once('.').unwrap_or((&plain,""));
    let mut out=String::new();
    for (i,ch) in whole.char_indices() {
        if cents && i>0 && ch!='-' && (whole.len()-i)%3==0 && &whole[..i]!="-" {out.push(',');}
        out.push(ch);
    }
    let fraction=format!("{fraction:0<width$}",width=precision as usize);
    let fraction=if cents{fraction.as_str()}else{fraction.trim_end_matches('0')};
    if !fraction.is_empty(){out.push('.');out.push_str(fraction);}
    Ok(out)
}
fn figure(value:&Dec,currency:&str)->Result<String,WebError>{Ok(format!("{} <small>{}</small>",amount(value,currency)?,escape(currency)))}
fn in_display(c:&Connection,amount:&Dec,currency:&str,day:chrono::NaiveDate,d:&Denomination)->Result<Option<Dec>,WebError>{
    let currency=if crate::tracker::stable(currency){"USD"}else{currency};
    if currency==d.currency{return Ok(Some(amount.clone()))}
    let rate=|currency:&str|->Result<Option<Dec>,WebError>{
        if currency=="EUR" {return Ok(Some(Dec::one()))}
        let from=day.checked_sub_days(chrono::Days::new(7)).ok_or_else(invalid)?;
        let text:Option<rusqlite::types::Value>=c.query_row("SELECT rate FROM fx_rates WHERE currency=?1 AND date BETWEEN ?2 AND ?3 ORDER BY date DESC LIMIT 1",(currency,from.to_string(),day.to_string()),|r|r.get(0)).optional()?;
        text.map(|v|number(Dec::from_sql((&v).into()))).transpose().map(Option::flatten)
    };
    match (rate(currency)?,rate(&d.currency)?) {(Some(from),Some(to))=>Ok(Some(number(amount * &number(to.div(&from))?)?)),_=>Ok(None)}
}
fn unit(d:&Denomination)->&str {match d.currency.as_str(){"USD"=>"$","EUR"=>"€","GBP"=>"£","CHF"=>"Fr.","PLN"=>"zł",other=>other}}
fn formatted(value:&Dec,d:&Denomination)->Result<String,WebError>{
    let negative=value.is_negative(); let abs=if negative{value.neg()}else{value.clone()};
    let sign=if negative{"-"}else{""};let u=escape(unit(d));let a=amount(&abs,&d.currency)?;
    Ok(if matches!(d.currency.as_str(),"CHF"|"PLN"){format!("{sign}{a} <small>{u}</small>")}else{format!("{sign}<small>{u}</small>{a}")})
}
fn logo(c:&Connection,row:&Row)->Result<String,WebError>{
    // Recorded asset, then the owner's balances, then the ranked crypto catalogue.
    let id=match row.asset {Some(id)=>Some(id),None=>c.query_row("SELECT a.id FROM account_balances b JOIN assets a ON a.id=b.asset_id WHERE b.user_id=?1 AND a.symbol=?2 ORDER BY b.id LIMIT 1",(row.owner,&row.base),|r|r.get(0)).optional()?.or(c.query_row("SELECT id FROM assets WHERE symbol=?1 AND category IN ('Cryptocurrency','Fiat','Currency') ORDER BY category='Cryptocurrency' DESC,id LIMIT 1",[&row.base],|r|r.get(0)).optional()?)};
    let Some(id)=id else{return Ok(String::new())};
    let (image,color,symbol,category):(Option<String>,Option<String>,String,Option<String>)=c.query_row("SELECT image_url,color,symbol,category FROM assets WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let flag=if category.as_deref().is_none_or(|c|c.is_empty() || matches!(c,"Currency"|"Fiat")) {
        [("USD","us"),("EUR","eu"),("GBP","gb"),("JPY","jp"),("CHF","ch"),("CAD","ca"),("AUD","au"),("PLN","pl"),("ARS","ar"),("BRL","br"),("TRY","tr"),("MXN","mx"),("ZAR","za"),("UAH","ua"),("CZK","cz"),("RUB","ru"),("IDR","id")].iter().find(|(s,_)|*s==symbol.to_uppercase()).map(|(_,flag)|crate::web::assets::path(&format!("flags/{flag}.svg")).to_string())
    }else{None};
    Ok(match flag.or(image).filter(|s|!s.trim().is_empty()) {Some(src)=>format!("  <img class=\"asset-logo\" src=\"{}\" alt=\"\" loading=\"lazy\">\n",escape(&src)),None=>format!("  <span class=\"asset-logo\" style=\"background: {}\"></span>\n",escape(&crate::web::colors::pill_color(color.as_deref()).ok_or_else(invalid)?))})
}

pub fn render(c:&Connection,ctx:&Ctx,row:&Row,d:&Denomination)->Result<String,WebError>{
    let user=ctx.user().ok_or_else(invalid)?;
    let cash=crate::tracker::cash(&row.base);
    let absolute=if row.amount.is_negative(){row.amount.neg()}else{row.amount.clone()};
    let counterpart=row.counterpart(c)?;
    let (price,source,exchange_price)=if cash {
        (in_display(c,&Dec::one(),&row.base,row.at.date_naive(),d)?,"cash",None)
    }else if absolute.is_zero(){(None,"none",None)}else if let Some((amount,currency))=&counterpart{
        (None,"exchange",Some(figure(&number(amount.div(&absolute))?,currency)?))
    }else if let Some(stated)=row.manual.get("price").filter(|v|!super::blank(v)){
        (Some(number(&number(Dec::to_d(stated))? * &d.rate)?),"stated",None)
    }else{
        let mut statement=c.prepare("SELECT price FROM historical_prices WHERE asset=?1 AND currency='USD' AND date=?2 LIMIT 1")?;
        let mut rows=statement.query((&row.base,row.at.date_naive().to_string()))?;
        let historical=if let Some(r)=rows.next()?{number(Dec::from_sql(r.get_ref(0)?))?}else{None};
        match historical.filter(Dec::is_positive){Some(p)=>(Some(number(&p * &d.rate)?),"ours",None),None=>(None,"none",None)}
    };
    let value=if source=="exchange"{match &counterpart{Some((n,currency))=>in_display(c,n,currency,row.at.date_naive(),d)?,None=>None}}else{price.as_ref().map(|p|number(p * &row.amount)).transpose()?};
    let value=value.map(|v|number(v.round(2))).transpose()?;
    let kind=TYPES.get(usize::try_from(row.kind).map_err(|_|invalid())?).ok_or_else(invalid)?;
    let linked=row.linked();
    let label=if linked{"transfer_badge".into()}else{format!("types.{kind}")};
    let tone=if linked{"quiet"}else{match row.kind{0|2=>"up",1|3|12=>"down",4|6|7|8|9|11|14=>"info",5=>"warn",_=>"quiet"}};
    let path=|suffix:&str|escape(&ctx.path(&format!("/tracker/transactions/{}{suffix}",row.id)));
    let mut action=String::new();
    if matches!(row.kind,4|5){
        action=format!("      <form class=\"tracker-row__action\" method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"_method\" value=\"patch\" /><button type=\"submit\">{}</button><input type=\"hidden\" name=\"authenticity_token\" value=\"{}\" /></form>\n",path("/toggle_transfer"),ctx.t(if linked{"tracker.transfer_unlink"}else{"tracker.transfer_link"}),escape(&ctx.csrf_token()));
    }
    let price_cell=if let Some(exchange)=exchange_price{format!("      {exchange}\n")}else if source=="cash"{format!("      {}\n",price.as_ref().map(|p|formatted(p,d)).transpose()?.unwrap_or(NO_VALUE.into()))}else{
        let unit=format!("<small>{}</small>",escape(unit(d)));
        let suffix=matches!(d.currency.as_str(),"CHF"|"PLN");
        let title=ctx.t(&format!("tracker.price_source.{source}"));
        let value=if source=="stated"{format!(" value=\"{}\"",price.as_ref().map(Dec::to_s_f).ok_or_else(invalid)?)}else{String::new()};
        let placeholder=if source=="ours"{price.as_ref().map(Dec::to_s_f).ok_or_else(invalid)?}else{"—".into()};
        let info=if source=="none"{format!("          <span class=\"tracker-row__info\" role=\"button\" tabindex=\"0\" aria-label=\"{title}\"\n                data-controller=\"tooltip\" data-tooltip-fixed-value=\"true\"\n                data-action=\"click->tooltip#toggle keydown.enter->tooltip#toggle keydown.space->tooltip#toggle:prevent focus->tooltip#showTooltip blur->tooltip#hideTooltip mouseenter->tooltip#showTooltip mouseleave->tooltip#hideTooltip\">\n            {}\n            <div class=\"tooltip tooltip--hint\">{title}</div>\n          </span>\n",include_str!("../../../templates/svg/_24x24_info.html"))}else{String::new()};
        format!("      <form data-controller=\"form--submit\" action=\"{}\" accept-charset=\"UTF-8\" method=\"post\"><input type=\"hidden\" name=\"_method\" value=\"patch\" /><input type=\"hidden\" name=\"authenticity_token\" value=\"{}\" />\n        {}\n        <input type=\"number\" name=\"price\" id=\"price\"{value} step=\"any\" min=\"0\" placeholder=\"{placeholder}\" title=\"{title}\" class=\"tracker-row__price__input\" data-action=\"change-&gt;form--submit#submit\" />\n        {}\n{info}</form>",path("/price"),escape(&ctx.csrf_token()),if suffix{""}else{&unit},if suffix{&unit}else{""})
    };
    let amount_cell=if user.hide_balances{String::new()}else{format!("<td>{}</td>",amount(&row.amount,&row.base)?)};
    let money=if user.hide_balances{String::new()}else{format!("    <td class=\"tracker-row__value\">{}</td>\n    <td class=\"tracker-row__fee\">{}</td>\n",value.as_ref().map(|v|amount(v,&d.currency)).transpose()?.unwrap_or(NO_VALUE.into()),row.fee.as_ref().map(|f|figure(f,row.fee_currency.as_deref().unwrap_or(""))).transpose()?.unwrap_or(NO_VALUE.into()))};
    let bot=row.bot.map(|id|format!("<a href=\"{}\">#{id}</a>",escape(&ctx.path(&format!("/bots/{id}"))))).unwrap_or(NO_VALUE.into());
    let venue=venue(&row.venue);
    Ok(format!("<tr id=\"account_transaction_{}\" class=\"tracker-row\" data-order-filter-target=\"row\" data-order-type=\"all {kind}{}\">\n  <td class=\"text-left tracker-row__when table__when\">\n    {} <small>{}</small>\n  </td>\n  <td class=\"tracker-exchange-icon\">{}</td>\n  <td class=\"text-left\">\n      <span class=\"pill pill--{tone}\">{}</span>\n{action}  </td>\n  <td class=\"text-left\"><span class=\"tracker-row__asset\">{}{}</span></td>\n  {amount_cell}\n  <td class=\"tracker-row__price tracker-row__price--{source}\">\n{price_cell}  </td>\n{money}  <td>\n      {bot}\n  </td>\n</tr>\n",row.id,if linked{" transfer"}else{""},format::table_date(row.at,&user.time_zone),format::table_clock(row.at,&user.time_zone,ctx.locale),crate::web::bot::exchange_svg(&venue),i18n::t(ctx.locale,&format!("tracker.{label}"),&[]),logo(c,row)?,escape(&row.base)))
}

fn venue(class: &str) -> String {
    // ActiveSupport underscore, including acronym boundaries (BinanceUs -> binance_us).
    let word = class.rsplit("::").next().unwrap_or(class);
    let letters: Vec<char> = word.chars().collect();
    let mut out = String::new();
    for (i,ch) in letters.iter().enumerate() {
        if ch.is_uppercase() && i > 0 && (letters[i-1].is_lowercase() || letters[i-1].is_ascii_digit() || letters.get(i+1).is_some_and(|c| c.is_lowercase())) { out.push('_'); }
        out.extend(ch.to_lowercase());
    }
    out.replace('-',"_")
}

/// FX refusal replaces every monetary cell; never a zero or a mislabeled dollar figure.
pub fn unavailable(id:i64)->String {
    format!("<tr id=\"account_transaction_{id}\" class=\"tracker-row\"><td colspan=\"10\"><span class=\"no-value\">Currency conversion unavailable</span></td></tr>\n")
}
