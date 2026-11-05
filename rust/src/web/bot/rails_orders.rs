//! A28-2a (PLAN-A28-2 §2, r2.1): the owner's answer to a `rails_order_open`/`listed` line. "This was this bot's order"
//! leaves a request the engine serves (`rails_unrecorded::serve_bookings`); "None of these was this bot's" is decided here.
//! A28-2b (PLAN-A28-2b, D-RAILS-UNRECORDED-API): MCP's `answer_unrecorded_order` and REST's
//! `POST /api/v1/bots/:id/unrecorded_orders` answer through the same `answer`; the outcome is read in `get_bot_details`.
use axum::{extract::{Extension,Path,State},http::StatusCode,response::{IntoResponse,Response}};
use rusqlite::{Connection,OptionalExtension};
use serde_json::{json,Map,Value};
use chrono::{DateTime,Utc};
use crate::engine::{rails_unrecorded as ru,EngineError};
use crate::web::{auth,flash,i18n,layout::{self,Ctx},App,WebError};
use crate::web::bot_api::{Domain,Res};

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum Answer { Offer(i64), None }

/// The one answer every surface files: the bot's venue name and whether it was taken; None when the bot is not the
/// user's (or deleted).
/// The bot's venue name, and the answer taken or why not.
pub type Filed=(String,Result<(),ru::Refused>);

pub fn answer(c:&Connection,user:i64,bot:i64,obligation:i64,answer:Answer,now:DateTime<Utc>)->Result<Option<Filed>,EngineError> {
    let venue:Option<String>=c.query_row("SELECT coalesce(e.name, '') FROM bots b LEFT JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1 AND b.user_id = ?2 AND b.status <> 3",
                                         (bot,user),|r|r.get(0)).optional()?;
    let Some(venue)=venue else {return Ok(None)};
    let done=match answer {
        Answer::Offer(offer)=>ru::request_booking(c,user,bot,obligation,offer,now)?,
        Answer::None=>ru::dismiss(c,user,bot,obligation,now)?,
    };
    Ok(Some((venue,done)))
}

/// POST /bots/:id/rails_orders with `obligation` and `offer` (an offer id) or `none=1`.
pub async fn create(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(segment):Path<String>)->Result<Response,WebError> {
    let Some(user)=ctx.user().map(|u|u.id) else {return Ok(auth::unauthenticated(&ctx))};
    let Some(id)=super::id_from_path(&segment) else {return Ok(super::liquidation::missing(&ctx))};
    let number=|name:&str| ctx.params.form(name).and_then(|v|v.trim().parse::<i64>().ok());
    let given=match (number("offer"),ctx.params.form("none")) {
        (Some(offer),None)=>Answer::Offer(offer),
        (None,Some("1"))=>Answer::None,
        _=>return Ok(StatusCode::BAD_REQUEST.into_response()),
    };
    let Some(obligation)=number("obligation") else {return Ok(StatusCode::BAD_REQUEST.into_response())};
    let now=ctx.now;
    let result=app.db(move |c| Ok(answer(c,user,id,obligation,given,now)?)).await;
    let (venue,done)=match result {
        Err(e)=>return layout::or_refused(&ctx,e),
        Ok(None)=>return Ok(super::liquidation::missing(&ctx)),
        Ok(Some(found))=>found,
    };
    let (style,text)=match (done,given) {
        (Err(refused),_)=>(flash::ALERT,refused.text().to_string()),
        (Ok(()),Answer::Offer(_))=>{
            app.wake_engine();
            (flash::NOTICE,i18n::text(ctx.locale,"bot.rails_orders.requested",&[("venue",i18n::Arg::Text(&venue))]))
        }
        (Ok(()),Answer::None)=>(flash::NOTICE,i18n::text(ctx.locale,"bot.rails_orders.dismissed",&[])),
    };
    flash::set(&ctx.session,style,text);
    Ok(layout::redirect(StatusCode::FOUND,&ctx.path(&format!("/bots/{id}"))))
}

// ---- MCP and REST (A28-2b) ----------------------------------------------------------------------------------------------

pub const TOOL:&str="answer_unrecorded_order";
const INVALID:&str="Give the obligation and either offer (an offer id from get_bot_details) or none: true, as integers.";

/// A JSON id: an integer within i64, nothing else (PLAN-A28-2b §1). Absent is None; present but not an id is Err.
fn json_id(v:Option<&Value>)->Result<Option<i64>,()> {
    match v { None=>Ok(None), Some(Value::Number(n))=>n.as_i64().map(Some).ok_or(()), Some(_)=>Err(()) }
}
/// A form id: ASCII digits within i64.
pub fn digits(s:&str)->Option<i64> { (!s.is_empty()&&s.bytes().all(|b|b.is_ascii_digit())).then(||s.parse().ok()).flatten() }
fn form_id(v:Option<&Value>)->Result<Option<i64>,()> {
    match v { None=>Ok(None), Some(Value::String(s))=>digits(s).map(Some).ok_or(()), Some(_)=>Err(()) }
}

/// The obligation and the answer from a JSON object (MCP's arguments, a JSON body) or a form body: exactly one of an
/// `offer` or `none` true; any other shape is None (nothing is written).
pub fn parse(args:&Map<String,Value>,form:bool)->Option<(i64,Answer)> {
    let id=|name:&str| if form {form_id(args.get(name))} else {json_id(args.get(name))};
    let none=match args.get("none") {
        None=>None,
        Some(Value::Bool(true))=>Some(true),
        Some(Value::String(s)) if form && (s=="1"||s=="true") =>Some(true),
        Some(_)=>return None,
    };
    let obligation=id("obligation").ok()??;
    match (id("offer").ok()?,none) {
        (Some(offer),None)=>Some((obligation,Answer::Offer(offer))),
        (None,Some(true))=>Some((obligation,Answer::None)),
        _=>None,
    }
}

/// The answer for both surfaces: REST's envelope (`res`, 202 when `accepted`) and MCP's text. The caller wakes the engine
/// when `accepted`.
pub struct Answered { pub res:Res, pub text:String, pub accepted:bool }

pub fn invalid()->Answered { Answered{res:Res::failure(Domain::ValidationFailed,"invalid_answer",INVALID.into()),text:format!("Invalid input: {INVALID}"),accepted:false} }

/// A path id that names no bot.
pub fn api_missing()->Answered { Answered{res:Res::failure(Domain::NotFound,"bot_not_found","Bot not found.".into()),text:"Bot not found.".into(),accepted:false} }

pub fn api(c:&Connection,user:i64,bot:i64,obligation:i64,given:Answer,now:DateTime<Utc>)->Result<Answered,WebError> {
    let en="en";
    let Some((venue,done))=answer(c,user,bot,obligation,given,now)? else { return Ok(api_missing()) };
    let fail=|status,code,message:String,data:Value| Answered{res:Res{data,..Res::failure(status,code,message.clone())},text:message,accepted:false};
    Ok(match done {
        Ok(())=>match given {
            Answer::Offer(offer)=>{
                let message=i18n::text(en,"bot.rails_orders.requested",&[("venue",i18n::Arg::Text(&venue))]);
                Answered{res:Res::success(json!({"bot_id":bot,"obligation":obligation,"offer":offer,"state":"checking"})),
                         text:format!("{message} get_bot_details shows the result under unrecorded orders."),accepted:true}
            }
            Answer::None=>Answered{res:Res::success(json!({"bot_id":bot,"obligation":obligation,"state":"not_mine"})),
                                   text:i18n::text(en,"bot.rails_orders.dismissed",&[]),accepted:false},
        },
        Err(ru::Refused::NotFound)=>fail(Domain::NotFound,"order_not_listed",ru::Refused::NotFound.text().into(),Value::Null),
        Err(ru::Refused::Pending)=>fail(Domain::Conflict,"already_checking",ru::Refused::Pending.text().into(),json!({"state":"checking"})),
        Err(ru::Refused::Decided)=>{
            // A repeat call asks about this obligation at any listing size (PLAN-A28-2b §3, r5).
            let (state,tx)=match ru::decision(c,obligation)? { Some((r,tx,_)) if r=="booked"=>("booked",tx), _=>("not_mine",None) };
            let outcome=match tx { Some(tx)=>format!(" Added to the bot's history as transaction {tx}."), None=>" You said none was this bot's.".into() };
            fail(Domain::Conflict,"already_decided",format!("{}{outcome}",ru::Refused::Decided.text()),json!({"state":state,"transaction_id":tx}))
        }
    })
}

/// `unrecorded_orders` of get_bot_details (PLAN-A28-2b §3): the data, and the MCP text's lines (none when the bot has
/// no answerable obligation). `quote` names the carry's currency.
pub fn listing(c:&Connection,bot:i64,quote:&str,now:DateTime<Utc>)->Result<(Value,Vec<String>),WebError> {
    // An install without the A28 tables (a Rails-only schema) has none.
    let present:bool=c.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'rust_rails_unrecorded_offers')",[],|r|r.get(0))?;
    if !present {return Ok((Value::Null,vec![]))}
    let Some(l)=ru::listing(c,bot,now)? else {return Ok((Value::Null,vec![]))};
    let en="en";
    let amount=|v:&Option<crate::ruby::BigDec>| v.as_ref().map(crate::ruby::BigDec::to_s_f);
    let mut lines=vec![format!("Unrecorded orders from before the update (answer with {TOOL}); carry now {} {quote}:",amount(&l.carry).unwrap_or_else(||"unknown".into()))];
    let mut rows=vec![];
    for o in &l.obligations {
        let refused=o.refused.as_ref().map(|(reason,venue,at)| json!({"reason":reason,"at":at,
            "message":i18n::text(en,&format!("bot_activity.events.rails_order_not_added_{reason}"),&[("venue",i18n::Arg::Text(venue))])}));
        let offers:Vec<Value>=o.offers.iter().map(|f|json!({"offer":f.id,"order":f.order,"carry_after":amount(&f.carry_after)})).collect();
        lines.push(match o.state {
            ru::ListedState::Open=>{
                let named:Vec<String>=o.offers.iter().map(|f|format!("offer {} = {}{}",f.id,f.order,
                    amount(&f.carry_after).map(|a|format!(", carry with it {a} {quote}")).unwrap_or_default())).collect();
                let mut line=format!("- obligation {} ({} to {}): {}; or none: true",o.obligation,o.from,o.to,named.join("; "));
                if let Some(r)=&refused { line.push_str(&format!("\n  last try refused: {}",r["message"].as_str().unwrap_or_default())); }
                line
            }
            ru::ListedState::Checking=>format!("- obligation {}: being checked",o.obligation),
            ru::ListedState::Booked=>format!("- obligation {}: added to the bot's history as transaction {}",o.obligation,o.transaction_id.map_or_else(||"?".into(),|t|t.to_string())),
            ru::ListedState::NotMine=>format!("- obligation {}: none was this bot's",o.obligation),
        });
        rows.push(json!({"obligation":o.obligation,"state":o.state.as_str(),"from":o.from,"to":o.to,"offers":offers,"transaction_id":o.transaction_id,"refused":refused}));
    }
    if l.more>0 { lines.push(format!("- and {} more",l.more)); }
    Ok((json!({"carry":amount(&l.carry),"obligations":rows,"more":l.more>0}),lines))
}
