//! A28-2a (PLAN-A28-2 §2, r2.1): the owner's answer to a `rails_order_open`/`listed` line. "This was this bot's order"
//! leaves a request the engine serves (`rails_unrecorded::serve_bookings`); "None of these was this bot's" is decided here.
use axum::{extract::{Extension,Path,State},http::StatusCode,response::{IntoResponse,Response}};
use rusqlite::OptionalExtension;
use crate::engine::rails_unrecorded as ru;
use crate::web::{auth,flash,i18n,layout::{self,Ctx},App,WebError};

#[derive(Clone,Copy)]
enum Answer { Offer(i64), None }

/// POST /bots/:id/rails_orders with `obligation` and `offer` (an offer id) or `none=1`.
pub async fn create(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(segment):Path<String>)->Result<Response,WebError> {
    let Some(user)=ctx.user().map(|u|u.id) else {return Ok(auth::unauthenticated(&ctx))};
    let Some(id)=super::id_from_path(&segment) else {return Ok(super::liquidation::missing(&ctx))};
    let number=|name:&str| ctx.params.form(name).and_then(|v|v.trim().parse::<i64>().ok());
    let answer=match (number("offer"),ctx.params.form("none")) {
        (Some(offer),None)=>Answer::Offer(offer),
        (None,Some("1"))=>Answer::None,
        _=>return Ok(StatusCode::BAD_REQUEST.into_response()),
    };
    let Some(obligation)=number("obligation") else {return Ok(StatusCode::BAD_REQUEST.into_response())};
    let now=ctx.now;
    let result=app.db(move |c| {
        let venue:Option<String>=c.query_row("SELECT coalesce(e.name, '') FROM bots b LEFT JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1 AND b.user_id = ?2 AND b.status <> 3",
                                             (id,user),|r|r.get(0)).optional()?;
        let Some(venue)=venue else {return Ok(None)};
        let done=match answer {
            Answer::Offer(offer)=>ru::request_booking(c,user,id,obligation,offer,now)?,
            Answer::None=>ru::dismiss(c,user,id,obligation,now)?,
        };
        Ok(Some((venue,done)))
    }).await;
    let (venue,done)=match result {
        Err(e)=>return layout::or_refused(&ctx,e),
        Ok(None)=>return Ok(super::liquidation::missing(&ctx)),
        Ok(Some(found))=>found,
    };
    let (style,text)=match (done,answer) {
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
