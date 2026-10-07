//! Tracker writes under the existing authenticated, CSRF-checked web pipeline.
use super::{auth, bot::action_params::ActionParams, layout::{self, Ctx}, App, WebError};
use crate::{codec, engine::{eligibility, model}};
use axum::{extract::{Extension, State}, http::{header, HeaderMap, StatusCode}, response::{IntoResponse, Response}};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

pub mod fx;
pub mod row;
pub mod transaction;
pub mod modal;
pub mod classifications;

struct Prepared {
    response: Response,
    jobs: Vec<(&'static str, String)>,
}

/// All D5 database writes share the same immediate transaction and the engine's guard.
/// Run inside App::db: cancellation of the HTTP awaiter cannot lose the post-commit wake.
fn write(c: &Connection, ctx: &Ctx, owner: i64,
    change: impl FnOnce(&Connection, i64, &str) -> Result<Prepared, WebError>,
) -> Result<Response, WebError> {
    let tx = model::immediate(c)?;
    let owned = tx.query_row("SELECT id FROM users WHERE id=?1", [owner], |r| r.get::<_, i64>(0)).optional()?;
    if owned.is_none() { return Ok(StatusCode::NOT_FOUND.into_response()); }
    let prepared = change(&tx, owner, &codec::format_time(ctx.app.now()))?;
    if let Err(refusal) = eligibility::guard(&tx, &ctx.app.cipher, None) {
        tx.rollback()?;
        let message = super::i18n::text(ctx.locale, "engine.write_refused", &[("reason",super::i18n::Arg::Text(&refusal.reason()))]);
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE,"text/plain; charset=utf-8")], message).into_response());
    }
    // Rails permits valid classification rows to survive another row's validation error.
    // Database errors above instead unwind the whole transaction and propagate to the 500.
    tx.commit()?;
    ctx.app.wake_engine();
    for (name,scope) in prepared.jobs { ctx.app.wake_job(name,&scope); }
    Ok(prepared.response)
}

fn blank(value: &Value) -> bool {
    match value { Value::Null | Value::Bool(false) => true, Value::String(s) => s.trim().is_empty(),
        Value::Array(v) => v.is_empty(), Value::Object(v) => v.is_empty(), _ => false }
}

fn save_settings(c: &Connection, owner: i64, now: &str, params: &Value) -> Result<StatusCode, WebError> {
    let stored: Option<String> = c.query_row("SELECT tracker_settings FROM users WHERE id=?1", [owner], |r| r.get(0))?;
    let was_null = stored.is_none();
    let original: Value = match stored {
        Some(text) => serde_json::from_str(&text).map_err(|_| WebError::Config("invalid stored tracker settings".into()))?,
        None => Value::Object(Map::new()),
    };
    let mut settings = original.as_object().cloned().ok_or_else(|| WebError::Config("invalid stored tracker settings".into()))?;
    for name in ["export_type", "country", "year", "report_scope"] {
        if let Some(value) = params.get(name).filter(|v| !v.is_array() && !v.is_object() && !blank(v)) {
            settings.insert(name.into(),value.clone());
        }
    }
    let updated = Value::Object(settings);
    // Rails ignores update's false return and still answers head :ok.
    if !super::user::validate_save(c, owner)? { return Ok(StatusCode::OK); }
    if was_null || updated != original {
        c.execute("UPDATE users SET tracker_settings=?1, updated_at=?2 WHERE id=?3", (updated.to_string(),now,owner))?;
    }
    Ok(StatusCode::OK)
}

fn classifications(c: &Connection, owner: i64, now: &str, params: &Value) -> Result<StatusCode, WebError> {
    let Some(rows) = params.get("classifications").and_then(Value::as_array) else { return Ok(StatusCode::OK); };
    let mut invalid = false;
    for row in rows {
        let Some(symbol) = row.get("symbol").filter(|v| !blank(v)) else { continue };
        let Some(symbol) = super::string_column::cast(symbol) else { continue };
        let kind = match row.get("kind").and_then(Value::as_str) {
            Some("share") => 0, Some("fund") => 1, Some("other_security") => 2, _ => continue,
        };
        let category = if kind == 1 {
            ["equity_fund","mixed_fund","real_estate_fund","foreign_real_estate_fund","other_fund"].iter()
                .position(|name| Some(*name) == row.get("fund_category").and_then(Value::as_str)).map(|n| n as i64)
        } else { None };
        let old: Option<(i64, i64, Option<i64>)> = c.query_row("SELECT id,kind,fund_category FROM fund_classifications WHERE user_id=?1 AND symbol=?2",
            (owner,&symbol), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if !classifications::validate_save(c, owner, old.map(|r| r.0), &symbol, kind, category)? { invalid = true; continue; }
        match old {
            Some((_,old_kind,old_category)) if (old_kind,old_category) == (kind,category) => {},
            Some(_) => { c.execute("UPDATE fund_classifications SET kind=?1, fund_category=?2, updated_at=?3 WHERE user_id=?4 AND symbol=?5",
                (kind,category,now,owner,&symbol))?; },
            None => { c.execute("INSERT INTO fund_classifications(user_id,symbol,kind,fund_category,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5)",
                (owner,&symbol,kind,category,now))?; },
        }
    }
    Ok(if invalid { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::OK })
}

async fn mutate(app: App, ctx: Ctx, funds: bool, headers: HeaderMap) -> Result<Response, WebError> {
    let Some(owner) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)); };
    let params = match ActionParams::parse_rails(&ctx.params) { Ok(p) => p, Err(e) => return Ok(e.status().into_response()) };
    let stream = headers.get(header::ACCEPT).and_then(|h| h.to_str().ok()).is_some_and(|s| s.contains("text/vnd.turbo-stream.html"));
    app.db(move |c| write(c,&ctx,owner,|c,owner,now| {
        let status = if funds { classifications(c,owner,now,params.value())? } else { save_settings(c,owner,now,params.value())? };
        Ok(Prepared { response:(status, [(header::CONTENT_TYPE,if stream { "text/vnd.turbo-stream.html" } else { "text/html" })], "").into_response(), jobs:vec![] })
    })).await
}

pub async fn save_export_settings(State(app): State<App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    mutate(app,ctx,false,headers).await
}
pub async fn fund_classifications(State(app): State<App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    mutate(app,ctx,true,headers).await
}

pub async fn sync(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(owner) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)); };
    app.db(move |c| write(c,&ctx,owner,|c,owner,_| {
        // This build registers venue sync jobs only for Alpaca. Never announce a sync
        // for a venue whose sync implementation is not available.
        let unsupported: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.status=1 AND k.key_type<>1 AND e.type<>?2)",
            (owner,crate::sync::ALPACA),|r|r.get(0))?;
        if unsupported { return Ok(Prepared { response:layout::refused(&ctx,"tracker sync for a venue other than Alpaca"),jobs:vec![] }); }
        let keys = crate::sync::reading_keys(c).map_err(|_| WebError::Config("tracker reading keys unavailable".into()))?;
        let mut names = vec![];
        let mut jobs = vec![];
        for key in keys {
            let name: Option<String> = c.query_row("SELECT e.name FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.id=?1 AND k.user_id=?2",(key,owner),|r|r.get(0)).optional()?;
            let Some(name) = name else { continue };
            names.push(name);
            for job in [crate::sync::jobs::LEDGER_SYNC,crate::sync::jobs::BALANCE_SYNC] { jobs.push((job,key.to_string())); }
        }
        if names.is_empty() { return Ok(Prepared { response:StatusCode::NO_CONTENT.into_response(),jobs }); }
        let text = super::i18n::t(ctx.locale,"tracker.importing_from",&[("exchange",super::i18n::Arg::Text(&names.join(", ")))]);
        let content = format!("  <div id=\"sync-progress\" class=\"flash__message salert salert--syncing\">\n    <div class=\"loader--syncing\"></div>\n    {text}\n  </div>\n");
        Ok(Prepared { response:([(header::CONTENT_TYPE,super::turbo::CONTENT_TYPE)],super::turbo::stream("append","flash",&content)).into_response(),jobs })
    })).await
}
