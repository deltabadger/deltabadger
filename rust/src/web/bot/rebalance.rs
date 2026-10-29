//! The halted rebalance's owner-attested Resume. Rails exposes this only on the bot page.
use axum::{extract::{Extension,Path,State},http::{HeaderMap,StatusCode},response::{IntoResponse,Response}};
use crate::{engine::{model,rebalance},web::{auth,flash,i18n,layout::{self,Ctx},App,WebError}};
use super::{action_params::ActionParams,liquidation::{changed,missing}};

pub async fn resolve(State(app):State<App>,Extension(ctx):Extension<Ctx>,Path(segment):Path<String>,headers:HeaderMap)->Result<Response,WebError> {
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let Some(id)=super::id_from_path(&segment) else {return Ok(missing(&ctx))};
    let Ok(params)=ActionParams::parse_rails(&ctx.params) else {return Ok(StatusCode::BAD_REQUEST.into_response())};
    let view=ctx.clone();
    let runtime=tokio::runtime::Handle::current();
    let result=app.db(move |c| {
        let found:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM bots WHERE id=?1 AND user_id=?2 AND status<>3 AND type IN ('Bots::DcaIndex','Bots::DcaMultiAsset'))",(id,user.id),|r|r.get(0))?;
        if !found {return Ok(None)}
        let token=params.value()["halt_token"].as_str().unwrap_or("");
        let bot=model::load_bot(c,id)?;
        // Construct a venue only for a current confirmation. All credential/proxy rules are the engine's.
        let waiting=if rebalance::resolution_token(&bot)==token {
            let factory=crate::venue::alpaca::LiveFactory::for_install(c,&view.app.cipher).with_clock(view.app.clock.clone());
            let (venue,_)=crate::engine::run::venue_for(c,&factory,&view.app.cipher,&bot)?;
            runtime.block_on(rebalance::resolve(c,&venue,&view.app.cipher,id,user.id,token,view.now))?
        } else {None};
        changed(c,&view,id);
        Ok(Some(waiting))
    }).await;
    match result {
        Ok(None)=>Ok(missing(&ctx)),
        Ok(Some(waiting))=>{
            let (style,text)=match waiting {
                Some(order)=>(flash::ALERT,i18n::text(ctx.locale,"bot.settings.rebalance.still_open",&[("order_id",i18n::Arg::Text(&order))])),
                None=>(flash::NOTICE,i18n::text(ctx.locale,"bot.settings.rebalance.resolved",&[])),
            };
            flash::set(&ctx.session,style,text);
            Ok(layout::redirect(StatusCode::FOUND,&layout::back(&app.config,&headers).unwrap_or_else(||ctx.path(&format!("/bots/{id}")))))
        }
        Err(WebError::Engine(crate::engine::EngineError::Ineligible(reasons))) => {
            let reason = reasons.join("; ");
            let text = i18n::text(ctx.locale,"engine.write_refused",&[("reason",i18n::Arg::Text(&reason))]);
            Ok((StatusCode::UNPROCESSABLE_ENTITY, axum::response::Html(flash::render(&[flash::Message {style:"danger",text}])?)).into_response())
        }
        Err(e)=>layout::or_refused(&ctx,e),
    }
}

/// composition/_metrics: reachable even with hidden balances, no prices, or a disabled rule.
/// The caller inserts its CSRF field before the halt token (or keeps that slot for a cached panel).
pub fn control(id:i64,transient:&serde_json::Value,locale:&str,prefix:&str)->String {
    if transient["rebalance_pending"]["phase"] != "ambiguous" {return String::new()}
    let t=|key|i18n::text(locale,key,&[]);
    format!("<div class=\"widget redeploy-prompt\">\n<span class=\"text-error\">{}</span>\n<form class=\"button_to\" data-turbo-confirm=\"{}\" method=\"post\" action=\"{}/bots/{id}/rebalance_resolutions\"><button class=\"rbutton rbutton--danger\" type=\"submit\">{}</button><input type=\"hidden\" name=\"halt_token\" value=\"{}\" autocomplete=\"off\" /></form>\n</div>\n",
        i18n::escape(&t("bot.settings.rebalance.paused")),i18n::escape(&t("bot.settings.rebalance.resume_confirm")),i18n::escape(prefix),i18n::escape(&t("bot.settings.rebalance.resume")),rebalance::resolution_token_for(id,transient))
}
