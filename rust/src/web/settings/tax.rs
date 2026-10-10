use crate::{
    codec::format_time,
    engine::eligibility,
    tracker::jobs::TRACKER_LEDGER,
    web::{
        auth, flash, i18n,
        layout::{self, Ctx},
        turbo, App, WebError,
    },
};
use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::{Transaction, TransactionBehavior};
fn refuse(ctx: &Ctx, message: String) -> Result<Response, WebError> {
    let markup = flash::render(&flash::take(&ctx.session, &[(flash::ALERT, message)]))?;
    Ok((
        StatusCode::UNPROCESSABLE_ENTITY,
        [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
        turbo::prepend_flash(markup.trim_end()),
    )
        .into_response())
}
pub async fn write(app: App, ctx: Ctx, headers: HeaderMap) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let Some(answer) = ctx
        .params
        .form("wash_sale[enabled]")
        .filter(|s| !crate::ruby::blank(s))
    else {
        return refuse(
            &ctx,
            i18n::text(ctx.locale, "settings.wash_sale.prompt_missing", &[]),
        );
    };
    let enabled = !["0", "f", "F", "false", "FALSE", "off", "OFF"].contains(&answer);
    let jurisdiction = ctx
        .params
        .form("wash_sale[jurisdiction]")
        .filter(|s| !crate::ruby::blank(s))
        .map(str::to_string);
    let jobs = app.job_wakers()?;
    let inner = app.clone();
    let refusal=app.db(move|c|{
  let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
  let (old,old_jurisdiction):(Option<bool>,Option<String>)=tx.query_row("SELECT wash_sale_enabled,wash_sale_jurisdiction FROM users WHERE id=?1",[user.id],|r|Ok((r.get(0)?,r.get(1)?)))?;
  let jurisdiction=jurisdiction.or(old_jurisdiction.clone());
  if jurisdiction.as_deref().is_some_and(|s|!crate::ruby::blank(s)&&!["US","GB","IE"].contains(&s)){return Ok(Some(None));}
  let changed=old!=Some(enabled)||old_jurisdiction!=jurisdiction;
  if changed{tx.execute("UPDATE users SET wash_sale_enabled=?1,wash_sale_jurisdiction=?2,updated_at=?3 WHERE id=?4",(enabled,jurisdiction,format_time(inner.now()),user.id))?;}
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(Some(refusal.reason())));}
  tx.commit()?;
  if changed{inner.wake_engine();}
  if enabled&&old!=Some(true){jobs.wake(TRACKER_LEDGER,Some(&user.id.to_string()),Some(user.id));}
  Ok(None)
 }).await?;
    if let Some(reason) = refusal {
        return refuse(
            &ctx,
            match reason {
                Some(reason) => i18n::text(
                    ctx.locale,
                    "engine.write_refused",
                    &[("reason", i18n::Arg::Text(&reason))],
                ),
                None => i18n::text(ctx.locale, "errors.messages.inclusion", &[]),
            },
        );
    }
    let destination =
        layout::back(&app.config, &headers).unwrap_or_else(|| ctx.path("/settings/account"));
    Ok(layout::redirect(StatusCode::SEE_OTHER, &destination))
}
