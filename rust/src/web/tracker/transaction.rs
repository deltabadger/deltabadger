//! Stated prices and user-asserted transfer corrections, in the guarded transaction.
use super::{auth, row::{self,Row,number,invalid}, write, ActionParams, App, Ctx, Prepared, WebError};
use crate::{codec, figures::{budget,dec::Dec}, web::{flash,turbo}};
use axum::{extract::{Extension,State,Path},http::{header,HeaderMap,StatusCode},response::{IntoResponse,Response}};
use rusqlite::{Connection,OptionalExtension};
use serde_json::Value;

fn prepared(status:StatusCode)->Prepared {Prepared{response:(status,[(header::CONTENT_TYPE,"text/vnd.turbo-stream.html")],"").into_response(),jobs:vec![]}}
#[allow(clippy::manual_ok_err)] // Input refusal is a value; SQL and stored-data errors never use this path.
fn plain(text:&str)->Option<Dec>{
    let text=crate::web::bot::mcp_input::strip(text);let (whole,part)=text.split_once('.').map_or((text,None),|(a,b)|(a,Some(b)));
    if whole.is_empty() || whole.len()>15 || !whole.bytes().all(|b|b.is_ascii_digit()) || part.is_some_and(|p|p.is_empty() || p.len()>18 || !p.bytes().all(|b|b.is_ascii_digit())){return None}
    match Dec::strict(text){Ok(n)=>Some(n),Err(_)=>None}
}
fn stream(c:&Connection,ctx:&Ctx,owner:i64,ids:&[i64],d:&crate::figures::totals::Denomination)->Result<String,WebError>{
    let mut out=String::new();
    // Rails reloads WHERE id IN (...) in primary-key order, rather than withdrawal order.
    let mut ids=ids.to_vec();ids.sort_unstable();ids.dedup();
    for id in ids{let row=Row::load(c,owner,id)?.ok_or_else(invalid)?;out.push_str(&turbo::stream("replace",&format!("account_transaction_{id}"),&row::render(c,ctx,&row,d)?));}
    Ok(out)
}
// AccountTransaction save! validations, including required associations, scoped tx_id
// uniqueness and the proposed link. Nothing is persisted until every check passes.
fn validate_save(c:&Connection,row:&Row,linked:Option<i64>)->Result<(),WebError>{
    if row.base.trim().is_empty(){return Err(WebError::RecordInvalid)}
    let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1) AND EXISTS(SELECT 1 FROM exchanges WHERE id=?2) AND NOT EXISTS(SELECT 1 FROM account_transactions other JOIN account_transactions current ON current.id=?3 WHERE other.id<>current.id AND other.user_id=current.user_id AND other.exchange_id=current.exchange_id AND other.tx_id=current.tx_id)",(row.owner,row.exchange,row.id),|r|r.get(0))?;
    if !valid{return Err(WebError::RecordInvalid)}
    // Row::load already requires a present base_amount and parseable transacted_at.
    if let Some(id)=linked{
        let target_owner:Option<i64>=c.query_row("SELECT user_id FROM account_transactions WHERE id=?1",[id],|r|r.get(0)).optional()?;
        let Some(owner)=target_owner else{return Err(WebError::RecordInvalid)};
        let target=Row::load(c,owner,id)?.ok_or(WebError::RecordInvalid)?;
        if target.owner!=row.owner || target.base!=row.base || row.kind!=5 || target.kind!=4 || target.at<row.at || target.amount>row.amount{return Err(WebError::RecordInvalid)}
    }
    Ok(())
}
fn price(c:&Connection,ctx:&Ctx,owner:i64,now:&str,id:i64,params:&Value)->Result<Prepared,WebError>{
    let Some(row)=Row::load(c,owner,id)? else{return Ok(Prepared{response:super::layout::missing(),jobs:vec![]})};
    let raw=match params.get("price"){None|Some(Value::Null)=>String::new(),Some(Value::String(s))=>s.clone(),Some(Value::Number(n)) if n.is_f64()=>crate::web::format::float_to_s(n.as_f64().ok_or_else(invalid)?),Some(value)=>value.to_string()};
    let d=row::denomination(c,ctx)?;
    let usd=if raw.trim().is_empty(){None}else{
        let Some(value)=plain(&raw)else{return Ok(prepared(StatusCode::UNPROCESSABLE_ENTITY))};
        let usd=if d.rate.is_zero(){value}else{number(value.div(&d.rate))?};
        // The service validates the converted plain decimal too (not only the input).
        let Some(usd)=plain(&usd.to_s_f())else{return Ok(prepared(StatusCode::UNPROCESSABLE_ENTITY))};
        if row.counterpart(c)?.is_some(){return Ok(prepared(StatusCode::UNPROCESSABLE_ENTITY))}
        Some(usd)
    };
    let mut manual=row.manual.as_object().cloned().ok_or_else(invalid)?;
    manual.shift_remove("price");
    if let Some(usd)=usd{manual.insert("price".into(),Value::String(usd.to_s_f()));}
    let manual=Value::Object(manual);
    validate_save(c,&row,row.linked)?;
    if row.manual_null || manual!=row.manual {
        c.execute("UPDATE account_transactions SET manual_values=?1,updated_at=?2 WHERE id=?3 AND user_id=?4",(manual.to_string(),now,id,owner))?;
    }
    let body=stream(c,ctx,owner,&[id],&d)?;
    Ok(Prepared{response:([(header::CONTENT_TYPE,turbo::CONTENT_TYPE)],body).into_response(),jobs:vec![(crate::tracker::jobs::TRACKER_LEDGER,owner.to_string())]})
}
fn transfer(c:&Connection,ctx:&Ctx,owner:i64,now:&str,id:i64)->Result<Prepared,WebError>{
    let Some(row)=Row::load(c,owner,id)?else{return Ok(Prepared{response:super::layout::missing(),jobs:vec![]})};
    let was_linked=row.linked();
    let pair=if was_linked {
        match (row.linked,row.inverse){(Some(deposit),_)=>Some((row.id,deposit)),(_,Some(withdrawal))=>Some((withdrawal,row.id)),_=>None}
    }else if matches!(row.kind,4|5){
        let withdrawal=row.kind==5;
        let edge=row.at.checked_add_signed(chrono::Duration::days(if withdrawal{14}else{-14})).ok_or_else(invalid)?;
        let (from,to)=if withdrawal{(row.at,edge)}else{(edge,row.at)};
        let mut statement=c.prepare("SELECT id FROM account_transactions t WHERE user_id=?1 AND base_currency=?2 AND entry_type=?3 AND transacted_at BETWEEN ?4 AND ?5 AND ((?3=4 AND base_amount<=?6 AND id NOT IN (SELECT linked_transaction_id FROM account_transactions WHERE user_id=?1 AND linked_transaction_id IS NOT NULL)) OR (?3=5 AND base_amount>=?6 AND linked_transaction_id IS NULL)) ORDER BY id LIMIT 2")?;
        let candidates=statement.query_map((owner,&row.base,if withdrawal{4}else{5},codec::format_time(from),codec::format_time(to),row.amount.to_s_f()),|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
        if candidates.len()==1{Some(if withdrawal{(row.id,candidates[0])}else{(candidates[0],row.id)})}else{None}
    }else{None};
    let (ids,kind,message,jobs)=if let Some((withdrawal,deposit))=pair {
        let saving=Row::load(c,owner,withdrawal)?.ok_or_else(invalid)?;
        validate_save(c,&saving,if was_linked{None}else{Some(deposit)})?;
        c.execute("UPDATE account_transactions SET linked_transaction_id=?1,transfer_link_rejected=?2,updated_at=?3 WHERE id=?4 AND user_id=?5",(if was_linked{None}else{Some(deposit)},was_linked,now,withdrawal,owner))?;
        (vec![withdrawal,deposit],flash::NOTICE,if was_linked{"tracker.transfer_unlinked"}else{"tracker.transfer_linked"},vec![(crate::tracker::jobs::PORTFOLIO_BACKFILL,owner.to_string()),(crate::tracker::jobs::TRACKER_LEDGER,owner.to_string())])
    }else{(vec![id],flash::ALERT,"tracker.transfer_no_candidate",vec![])};
    let d=row::denomination(c,ctx)?;
    let mut body=stream(c,ctx,owner,&ids,&d)?;
    // The foundation flash template has insignificant boundary whitespace; this route
    // also pins Rails' unnormalised bytes, including the blank line after the icon.
    let messages=flash::render(&flash::take(&ctx.session,&[(kind,ctx.t(message))]))?;
    let messages=messages.trim_matches('\n').replace("</svg>\n</button>","</svg>\n\n</button>");
    body.push_str(&turbo::prepend_flash(&messages));
    Ok(Prepared{response:([(header::CONTENT_TYPE,turbo::CONTENT_TYPE)],body).into_response(),jobs})
}
async fn mutate(app:App,ctx:Ctx,id:String,is_price:bool,headers:HeaderMap)->Result<Response,WebError>{
    let Some(owner)=ctx.user().map(|u|u.id)else{return Ok(auth::unauthenticated(&ctx))};
    let params=match ActionParams::parse_rails(&ctx.params){Ok(p)=>p,Err(e)=>return Ok(e.status().into_response())};
    let Some(id)=crate::web::bot::id_from_path(&id) else{return Ok(super::layout::missing())};
    drop(headers);
    app.db(move|c|budget::within(||write(c,&ctx,owner,|c,owner,now|if is_price{price(c,&ctx,owner,now,id,params.value())}else{transfer(c,&ctx,owner,now,id)}))).await

}
pub async fn update_price(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(id):Path<String>,headers:HeaderMap)->Result<Response,WebError>{mutate(app,ctx,id,true,headers).await}
pub async fn toggle_transfer(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(id):Path<String>,headers:HeaderMap)->Result<Response,WebError>{mutate(app,ctx,id,false,headers).await}
