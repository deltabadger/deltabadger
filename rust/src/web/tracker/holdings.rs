//! Complete cold-ledger pages; any unfinished record/history/warm region is refused.
use super::{Ctx,WebError};
use crate::{figures::{dec::Dec,totals::Denomination},tracker::figures::page::{Cold,Holding},web::{i18n::{self,Arg},format::{float_round,float_to_s},colors}};
use rusqlite::Connection;
use askama::Template;
pub fn supported(c:&Connection,owner:i64)->Result<bool,WebError>{
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1)
    AND NOT EXISTS(SELECT 1 FROM api_keys k LEFT JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND (e.type IS NULL OR e.type!='Exchanges::Alpaca' OR COALESCE(k.status,-1)!=1 OR COALESCE(k.key_type,-1) NOT IN(0,2) OR COALESCE(k.last_sync_error,'')!=''))
    AND (SELECT COUNT(DISTINCT exchange_id) FROM api_keys WHERE user_id=?1)=1
    AND NOT EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1)
    AND NOT EXISTS(SELECT 1 FROM portfolio_snapshots WHERE user_id=?1)
    AND NOT EXISTS(SELECT 1 FROM portfolio_venue_snapshots WHERE user_id=?1)
    AND NOT EXISTS(SELECT 1 FROM app_configs WHERE key=?2)
    AND EXISTS(SELECT 1 FROM account_balances WHERE user_id=?1)
    AND NOT EXISTS(SELECT 1 FROM account_balances WHERE user_id=?1 AND exchange_id NOT IN(SELECT exchange_id FROM api_keys WHERE user_id=?1))",rusqlite::params![owner,crate::tracker::cache::key(owner)],|r|r.get(0))?)
}
pub struct Presentation {pub value:String,pub card:String,pub sync:String}
#[derive(Template)]
#[template(path="tracker/holdings.html")]
struct Card<'a>{v:&'a Ctx,hidden:bool,count:usize,shares:String,arcs:Vec<Arc>,shown:Vec<String>,folded:Vec<String>,only_cash:bool}
struct Arc {color:String,dash:String,offset:String,label:String}
#[derive(Template)]
#[template(path="tracker/holding.html")]
struct Row<'a>{v:&'a Ctx,hidden:bool,symbol:String,name:String,logo:String,kind:String,color:String,share:String,percent:String,value:String}
fn invalid()->WebError{super::row::invalid()}
fn dec(raw:&str)->Result<Dec,WebError>{Dec::parse(raw).map_err(|_|invalid())}
fn label(h:&Holding)->&str{if crate::tracker::stable(&h.symbol){"Stable"}else{match h.category.as_deref(){Some("Stock"|"Common Stock")=>"Stock",Some("Fund")=>"Fund",Some("ETF")=>"ETF",Some("Cryptocurrency")=>"Crypto",Some("Currency"|"Fiat")=>"Cash",_=>""}}}
fn percent(d:&Dec,digits:i64)->Result<String,WebError>{let text=d.round(digits).map_err(|_|invalid())?.to_s_f();let (whole,fraction)=text.split_once('.').ok_or_else(invalid)?;Ok(format!("{whole}.{:0<width$}",fraction,width=digits as usize))}
fn money(d:&Dec,denomination:&Denomination)->Result<String,WebError>{super::row::formatted(&(d*&denomination.rate).map_err(|_|invalid())?,denomination)}
fn logo(h:&Holding)->Result<String,WebError>{super::row::asset_logo(h.image.clone(),h.color.clone(),h.symbol.clone(),h.category.clone())}
fn ring(holdings:&[Holding],other:&str)->Result<Vec<Arc>,WebError>{
    let total=holdings.iter().try_fold(Dec::zero(),|sum,h|&sum+&h.value).map_err(|_|invalid())?;
    if !total.is_positive(){return Ok(vec![])}
    let mut large=vec![];let mut small=vec![];
    for h in holdings.iter().filter(|h|h.value.is_positive()){
        let slice=(h.value.clone(),h.color.clone(),h.symbol.clone());
        if h.value.div(&total).map_err(|_|invalid())?<dec("0.02")?{small.push(slice)}else{large.push(slice)}
    }
    if small.len()==1{large.extend(small);}else if !small.is_empty(){large.push((small.iter().try_fold(Dec::zero(),|sum,h|&sum+&h.0).map_err(|_|invalid())?,Some(colors::NEUTRAL.into()),other.into()));}
    let circumference=2.0*std::f64::consts::PI*100.0;let mut walked=0.0;let mut arcs=vec![];
    let spans=large.iter().map(|(value,_,_)|Ok((value.div(&total).map_err(|_|invalid())?.to_s_f().parse::<f64>().map_err(|_|invalid())?*circumference).max(11.5))).collect::<Result<Vec<_>,WebError>>()?;
    let scale=circumference/spans.iter().sum::<f64>();
    for ((_,color,label),span) in large.into_iter().zip(spans) {
        let span=span*scale;let length=(span-11.5).max(0.0);
        let color=colors::ensure_contrast(color.as_deref().filter(|s|!s.trim().is_empty()).unwrap_or(colors::NEUTRAL)).ok_or_else(invalid)?;
        arcs.push(Arc{color,dash:format!("{} {}",float_to_s(float_round(length,2)),float_to_s(float_round(circumference-length,2))),offset:float_to_s(-float_round(walked+5.75,2)),label:i18n::escape(&label)});walked+=span;
    }Ok(arcs)
}
fn card(ctx:&Ctx,data:&Cold,d:&Denomination)->Result<String,WebError>{
    let hidden=ctx.user().ok_or_else(invalid)?.hide_balances;
    let total=data.value.as_ref().ok_or_else(invalid)?;let mut shares:Vec<(String,Dec)>=vec![];let mut rows=vec![];
    for h in &data.holdings{
        let kind=label(h);let group=if kind.is_empty(){i18n::text(ctx.locale,"tracker.other",&[])}else{kind.into()};
        if let Some((_,value))=shares.iter_mut().find(|(k,_)|k==&group){*value=(&*value+&h.value).map_err(|_|invalid())?;}else{shares.push((group,h.value.clone()));}
        let share=if total.is_positive(){(&h.value.div(total).map_err(|_|invalid())?*&dec("100.0")?).map_err(|_|invalid())?}else{Dec::zero()};
        let color=colors::ensure_contrast(h.color.as_deref().filter(|s|!s.trim().is_empty()).unwrap_or(colors::NEUTRAL)).ok_or_else(invalid)?;
        rows.push(Row{v:ctx,hidden,symbol:i18n::escape(&h.symbol),name:i18n::escape(&h.name),logo:logo(h)?,kind:kind.into(),color,share:share.round(2).map_err(|_|invalid())?.to_s_f(),percent:percent(&share,1)?,value:money(&h.value,d)?}.render()?);
    }
    let mut rounded=vec![];
    if total.is_positive(){for(label,value)in shares{let share=(&value.div(total).map_err(|_|invalid())?*&dec("100.0")?).map_err(|_|invalid())?.round(0).map_err(|_|invalid())?;if !share.is_zero(){rounded.push((label,share));}}}
    rounded.sort_by(|a,b|b.1.cmp(&a.1));
    let shares=rounded.into_iter().map(|(label,value)|format!("{label} {}%",value.to_s_f().trim_end_matches(".0"))).collect::<Vec<_>>().join(" · ");
    let folded=if rows.len()>6{rows.split_off(6)}else{vec![]};
    Ok(Card{v:ctx,hidden,count:data.holdings.len(),shares:i18n::escape(&shares),arcs:ring(&data.holdings,&i18n::text(ctx.locale,"tracker.other",&[]))?,shown:rows,folded,only_cash:data.only_cash}.render()?)
}
pub fn prepare(c:&Connection,ctx:&Ctx,owner:i64,fx:&super::fx::Prepared)->Result<Presentation,WebError>{
    let scope=ctx.params.query("exchange_id").filter(|s|!s.trim().is_empty()).map(|s|crate::web::bot::id_from_path(s).ok_or_else(invalid)).transpose()?;
    let settings:Option<String>=c.query_row("SELECT tracker_settings FROM users WHERE id=?1",[owner],|r|r.get(0))?;
    let data=crate::figures::budget::within(||crate::tracker::figures::page::cold(c,owner,scope,crate::web::bots::show_cash(settings.as_deref()))).map_err(|e|match e{crate::figures::FiguresError::Sqlite(e)=>WebError::from(e),_=>invalid()})?;
    let d=fx.denomination(c,owner,ctx.app.now().timestamp())?;
    let reason=data.reason.or(if d.is_none(){Some("Currency conversion unavailable")}else{None});
    let (value,card)=if let Some(reason)=reason{
        let value=format!("<span class=\"no-value\" title=\"{reason}\">—</span>");
        let card=format!("<div class=\"widget tracker-holdings\"><p class=\"tracker-holdings__empty\">{reason}</p></div>");(value,card)
    }else{let d=d.ok_or_else(invalid)?;(money(data.value.as_ref().ok_or_else(invalid)?,&d)?,card(ctx,&data,&d)?)};
    let sync=if let Some(at)=data.synced{
        let seconds=ctx.app.now().signed_duration_since(at).num_seconds().unsigned_abs();
        let calendar=if seconds>=2_419_200 {
            let now=ctx.app.now();let (years,months,weeks,days)=crate::web::format::calendar_distance(at.min(now),at.max(now));
            [("years",years),("months",months),("weeks",weeks),("days",days)].into_iter().find(|(_,n)|*n>0).map(|(unit,n)|(unit,n as u64))
        }else{None};
        let (unit,n)=if let Some(part)=calendar{part}else if seconds>=604800{("weeks",seconds/604800)}else if seconds>=86400{("days",seconds/86400)}else if seconds>=3600{("hours",seconds/3600)}else if seconds>=60{("minutes",seconds/60)}else{("seconds",seconds)};
        let word=i18n::text(ctx.locale,&format!("datetime.dotiw.{unit}"),&[("count",Arg::Count(n as i64))]);
        let ago=if n==0{i18n::text(ctx.locale,"datetime.dotiw.less_than_x",&[("distance",Arg::Text(&i18n::text(ctx.locale,"datetime.dotiw.seconds",&[("count",Arg::Count(1))])))])}else{word};
        let text=i18n::t(ctx.locale,"tracker.synced_ago",&[("ago",Arg::Text(&ago))]);
        format!("        <span class=\"label tracker-sync {}\">\n          <i class=\"tracker-sync__dot\"></i>{text}\n        </span>\n",if data.stale{"tracker-sync--stale"}else{""})
    }else{String::new()};
    Ok(Presentation{value,card,sync})
}
