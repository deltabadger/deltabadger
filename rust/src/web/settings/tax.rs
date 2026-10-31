use crate::{
    codec::format_time,
    engine::eligibility,
    tracker::jobs::TRACKER_LEDGER,
    web::{
        auth, flash, i18n,
        layout::Ctx,
        turbo, App, WebError,
    },
};
use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, Transaction, TransactionBehavior};
pub(crate) fn refuse(ctx: &Ctx, message: String) -> Result<Response, WebError> {
    let markup = flash::render(&flash::take(&ctx.session, &[(flash::ALERT, message)]))?;
    Ok((
        StatusCode::UNPROCESSABLE_ENTITY,
        [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
        turbo::prepend_flash(markup.trim_end()),
    )
        .into_response())
}
pub async fn write(app: App, ctx: Ctx, _headers: HeaderMap) -> Result<Response, WebError> {
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
    let uid = user.id;
    let decision = app.db(move |c| record_decision(c, &inner, &jobs, uid, enabled, jurisdiction)).await?;
    let refusal = match decision { Decision::Saved => None, Decision::Invalid => Some(None), Decision::Refused(reason) => Some(Some(reason)) };
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
    // The row answers with itself, as it now stands: the answer, and what the rule is locking right now.
    let (v, csrf, id) = (ctx.clone(), ctx.csrf_token(), user.id);
    let row = app.db(move |c| super::view::wash(c, &v, &csrf, id)).await?;
    Ok((
        [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
        turbo::stream("replace", "wash_sale", &row),
    )
        .into_response())
}

/// What answering the wash-sale question did.
pub enum Decision { Saved, Invalid, Refused(String) }

/// WashSalePromptable#record_wash_sale_decision: one writer for the answer, from the account box or a bot page's modal.
/// The jurisdiction is kept unless one is given; an unknown one writes nothing (`Invalid`). Switching the rule on re-walks
/// the account's ledger (Tracker::LedgerJob); any change wakes the engine.
pub fn record_decision(c: &Connection, app: &App, jobs: &crate::jobs::Wakers, user: i64, enabled: bool, jurisdiction: Option<String>) -> Result<Decision, WebError> {
    let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
    let (old, old_jurisdiction): (Option<bool>, Option<String>) = tx.query_row("SELECT wash_sale_enabled,wash_sale_jurisdiction FROM users WHERE id=?1", [user], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let jurisdiction = jurisdiction.or(old_jurisdiction.clone());
    if jurisdiction.as_deref().is_some_and(|s| !crate::ruby::blank(s) && !["US", "GB", "IE"].contains(&s)) { return Ok(Decision::Invalid); }
    let changed = old != Some(enabled) || old_jurisdiction != jurisdiction;
    if changed { tx.execute("UPDATE users SET wash_sale_enabled=?1,wash_sale_jurisdiction=?2,updated_at=?3 WHERE id=?4", (enabled, jurisdiction, format_time(app.now()), user))?; }
    if let Err(refusal) = eligibility::guard(&tx, &app.cipher, None) { return Ok(Decision::Refused(refusal.reason())); }
    tx.commit()?;
    if changed { app.wake_engine(); }
    if enabled && old != Some(true) { jobs.wake(TRACKER_LEDGER, Some(&user.to_string()), Some(user)); }
    Ok(Decision::Saved)
}
