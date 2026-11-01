//! TrackerController#tax_report and #download_tax_report, and the four panels (tracker/_report_{progress,ready,refused,
//! failed}.html.erb).
//!
//! D-TAX-RECONCILE: every panel this build renders is its request's (`tax-report-<request>`, Rails' shared
//! `tax-report-progress`), so a late or queued broadcast for one request can only touch that request's panel, and crypto
//! and broker panels are independent. The progress panel asks for its request's state once the page's streams are
//! connected and again every 15 s for a day while its bar is shown (the existing `broadcast--on-connect` controller,
//! `POST /broadcasts/tax_report_state`): a terminal state is broadcast again, so an answer that went out before the panel
//! was there still arrives. Known limit: a tab suspended past that day keeps its bar until the page is reloaded, and the
//! reload shows the outcome (`pending_report`). Rails reconciles only a refusal, at request time.
use super::{auth, Ctx, WebError};
use crate::tax::job::{self, Panel, Request};
use crate::web::i18n::{self, escape};
use crate::web::{flash, layout, locale, turbo};
use axum::extract::{Extension, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

const CLOSE: &str = include_str!("../../../templates/svg/_24x24_close.html");

/// `download_tax_report_tracker_path(country:, year:, report_scope:)` in `locale` (`default_url_options`).
fn download_path(locale: &str, country: &str, year: i64, scope: &str) -> String {
    let query = locale::switch_query(&[("country".into(), country.into()), ("report_scope".into(), scope.into()), ("year".into(), year.to_string())]);
    locale::path(locale, &format!("/tracker/download_tax_report?{query}"))
}

/// A request's panel: the partial Rails renders, under the request's own id.
pub fn panel(locale: &str, r: &Request, p: &Panel) -> String {
    let (id, req) = (format!("tax-report-{}", r.request), &r.request);
    match p {
        Panel::Progress(percent) => {
            let wait = escape(&format!("#{id} .slider"));
            let args = escape(&json!({ "request": req }).to_string());
            format!("<div id=\"{id}\" class=\"flash__message salert salert--download\" data-tax-request=\"{req}\" data-controller=\"broadcast--on-connect\" \
                     data-broadcast--on-connect-method-value=\"tax_report_state\" data-broadcast--on-connect-method-args-value=\"{args}\" \
                     data-broadcast--on-connect-retry-while-value=\"{wait}\" data-broadcast--on-connect-retry-after-value=\"15000\" \
                     data-broadcast--on-connect-retry-for-value=\"86400000\">\n  <span>{} {percent}%</span>\n  <div class=\"slider\">\n    <div class=\"slider__style\">\n      \
                     <div class=\"slider__style__track\" style=\"grid-template-columns: {percent}% auto\">\n        <div class=\"slider__style__progress\"></div>\n      </div>\n    \
                     </div>\n  </div>\n</div>\n", escape(&i18n::text(locale, "tracker.tax_report.generating", &[])))
        }
        Panel::Ready => format!("<div id=\"{id}\" data-tax-request=\"{req}\" data-controller=\"auto-download\" data-auto-download-url-value=\"{}\"></div>\n",
                                escape(&download_path(locale, &r.country, r.year, &r.scope))),
        Panel::Refused(refusal) => alert(&id, Some(req), "warning", &escape(&job::refusal_message(locale, refusal))),
        Panel::Failed => alert(&id, Some(req), "danger", &escape(&i18n::text(locale, "tracker.tax_report.failed", &[]))),
    }
}

/// The refused and failed partials.
fn alert(id: &str, request: Option<&str>, style: &str, message: &str) -> String {
    let tag = request.map_or(String::new(), |r| format!(" data-tax-request=\"{r}\""));
    format!("<div id=\"{id}\" class=\"flash__message salert salert--{style}\" data-controller=\"removals\" role=\"alert\"{tag}>\n  \
             <button class=\"modal__close\" data-action=\"removals#remove\">\n    {CLOSE}\n  </button>\n  {message}\n</div>\n")
}

fn stream(body: &str) -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], turbo::stream("append", "flash", body)).into_response()
}

/// D-TAX-NOT-PORTED-GATE (and the CoinGecko refusal): a report this build cannot compute is refused by the request
/// itself, never queued, with the refused flash every unported page answers.
async fn refused(ctx: &Ctx, headers: &HeaderMap, reason: &str) -> Result<Response, WebError> {
    let response = layout::refused(ctx, reason);
    if !crate::web::header_text(headers, "accept").is_some_and(|s| s.contains("text/vnd.turbo-stream.html")) { return Ok(response); }
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024).await.map_err(|_| super::row::invalid())?;
    let body = std::str::from_utf8(&body).map_err(|_| super::row::invalid())?;
    Ok((StatusCode::NOT_IMPLEMENTED, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], turbo::stream("append", "flash", body)).into_response())
}

/// Whether the install's historical prices come from CoinGecko, which this build does not fetch from (A17 lifts it).
pub fn coingecko(c: &rusqlite::Connection, app: &crate::web::App) -> Result<bool, WebError> {
    let (deltabadger, configured) = crate::web::bots::market_data(c, app)?;
    Ok(configured && !deltabadger)
}

/// `user.update(tracker_settings: settings.merge('pending_report' => …))`: the pending report's identity only, so the
/// tracker auto-downloads it on the next visit. Validations as `update` runs them; nothing written when nothing changed.
pub fn set_pending(c: &rusqlite::Connection, owner: i64, now: &str, pending: Value) -> Result<(), WebError> {
    let stored: Option<String> = c.query_row("SELECT tracker_settings FROM users WHERE id=?1", [owner], |r| r.get(0))?;
    let was_null = stored.is_none();
    let original: Value = match stored { Some(text) => serde_json::from_str(&text).map_err(|_| WebError::Config("invalid stored tracker settings".into()))?, None => json!({}) };
    let mut settings = original.as_object().cloned().ok_or_else(|| WebError::Config("invalid stored tracker settings".into()))?;
    settings.insert("pending_report".into(), pending);
    let updated = Value::Object(settings);
    if !crate::web::user::validate_save(c, owner)? { return Ok(()); }
    if was_null || updated != original { c.execute("UPDATE users SET tracker_settings=?1, updated_at=?2 WHERE id=?3", (updated.to_string(), now, owner))?; }
    Ok(())
}

/// TrackerController#tax_report.
pub async fn tax_report(State(app): State<crate::web::App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    let get = |name: &str| ctx.params.query(name).map(str::to_string);
    let asked = (get("country"), get("year"), get("report_scope"), get("stablecoin_as_fiat"));
    generate(app, ctx, headers, asked, None).await
}

/// D-TAX-PL-DECLARED-CARRY: the PL form of the export modal (its own submit, Rails' GET untouched): the same request,
/// with column d as declared, saved only if the report is queued. An amount `pl::carried` does not read answers 422 with
/// the form again and nothing saved.
pub async fn tax_report_pl(State(app): State<crate::web::App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let year = ctx.params.form("year").map_or(0, crate::ruby::to_i);
    // Absent keeps what is saved and "" clears it, as on REST and MCP; the text is read as typed (no trimming).
    let raw = ctx.params.form("pl_carried_costs").map(str::to_string);
    let Ok(carry) = crate::tax::pl::carried(raw.clone().map(Value::String).as_ref()) else {
        let block = super::modal::pl_block(&ctx, year, raw.as_deref().unwrap_or(""), true);
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
                   turbo::stream("replace", &format!("pl-carry-{year}"), &block)).into_response());
    };
    let asked = (Some("PL".to_string()), Some(year.to_string()), None, None);
    generate(app, ctx, headers, asked, carry).await
}

type Asked = (Option<String>, Option<String>, Option<String>, Option<String>);

async fn generate(app: crate::web::App, ctx: Ctx, headers: HeaderMap, asked: Asked, carry: Option<Option<crate::figures::dec::Dec>>) -> Result<Response, WebError> {
    let Some(owner) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)) };
    let (country, year, scope, stablecoin) = asked;
    let year = year.as_deref().map_or(0, crate::ruby::to_i);
    let scope = match scope.as_deref() { Some(s) if job::SCOPES.contains(&s) => job::scope_of(s), _ => "crypto" };
    // `Tax::Jurisdictions.for(country)`: the registry's exact key.
    let Some(country) = country.filter(|c| super::modal::COUNTRIES.contains(&c.as_str())) else {
        flash::set(&ctx.session, flash::ALERT, ctx.t("tracker.tax_report.invalid_country"));
        return Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/tracker")));
    };
    if scope == "broker" { return refused(&ctx, &headers, "tracker broker tax reports").await; }
    if !crate::tax::jurisdictions::ported(&country, year) { return refused(&ctx, &headers, "tracker tax reports for this jurisdiction").await; }
    let stablecoin = stablecoin.as_deref() == Some("true");
    let (app2, ctx2) = (app.clone(), ctx.clone());
    let answer = app.db(move |c| {
        if coingecko(c, &app2)? { return Ok(None); }
        let tmp = &app2.config.tmp_dir;
        let _files = job::files(owner, scope);
        let request = job::token();
        let now = app2.now();
        // An earlier refusal steps aside before the enqueue and comes back if it is not accepted.
        let aside = job::Aside::step(&job::refusal_path(tmp, owner, &country, year, scope), &request).map_err(|e| WebError::Config(e.to_string()))?;
        let ask = job::Ask { user: owner, scope, country: &country, year, stablecoin_as_fiat: stablecoin, locale: ctx2.locale };
        let written = (|| -> Result<bool, WebError> {
            let tx = crate::engine::model::immediate(c)?;
            set_pending(&tx, owner, &crate::codec::format_time(now), json!({ "country": country, "year": year, "report_scope": scope }))?;
            let accepted = job::enqueue(&tx, &ask, &request, now)?;
            // Saved only with a queued request, so it never changes under a queued or running report.
            if let Some(value) = carry.as_ref().filter(|_| accepted) { crate::tax::pl::declare(&tx, owner, year, value.as_ref(), &crate::codec::format_time(now))?; }
            tx.commit()?;
            Ok(accepted)
        })();
        let accepted = match written {
            Ok(accepted) => accepted,
            Err(e) => { if let Some(a) = aside { let _ = a.restore(); } return Err(e); }
        };
        match aside { Some(a) if accepted => a.drop_stale(), Some(a) => a.restore(), None => Ok(()) }.map_err(|e| WebError::Config(e.to_string()))?;
        if accepted { app2.job_wakers().map(|w| w.wake(job::JOB, None, None)).ok(); }
        // The reconcile, under the files' lock: a run that already ended answers its outcome.
        let body = if accepted {
            let r = job::by_request(c, &request)?.ok_or_else(|| WebError::Config("an accepted tax report request vanished".into()))?;
            let shown = job::terminal(tmp, &r).unwrap_or(Panel::Progress(0));
            panel(ctx2.locale, &r, &shown)
        } else if let Some(refusal) = job::refusal(tmp, owner, &country, year, scope) {
            // Rails: the restored refusal of the report asked for, though another request holds the slot.
            alert(&format!("tax-report-{}", job::token()), None, "warning", &escape(&job::refusal_message(ctx2.locale, &refusal)))
        } else {
            let r = job::active(c, owner, scope)?.ok_or_else(|| WebError::Config("a refused tax report request has no slot holder".into()))?;
            panel(ctx2.locale, &r, &Panel::Progress(0))
        };
        Ok(Some(body))
    }).await?;
    match answer {
        Some(body) => Ok(stream(&body)),
        None => refused(&ctx, &headers, "tracker tax reports from CoinGecko prices").await,
    }
}

/// TrackerController#download_tax_report: the refusal's alert, else the file sent and deleted, else "expired".
pub async fn download_tax_report(State(app): State<crate::web::App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(owner) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)) };
    let country = ctx.params.query("country").unwrap_or("").to_string();
    let year = ctx.params.query("year").map_or(0, crate::ruby::to_i);
    let raw_scope = ctx.params.query("report_scope").map(str::to_string);
    let scope = job::scope_of(raw_scope.as_deref().unwrap_or(""));
    let tmp = app.config.tmp_dir.clone();
    let (c2, t2) = (country.clone(), tmp);
    let found = app.db(move |_| {
        let _files = job::files(owner, scope);
        if let Some(refusal) = job::refusal(&t2, owner, &c2, year, scope) { return Ok(Err(refusal)); }
        let path = job::report_path(&t2, owner, &c2, year, scope);
        // The read and the delete are one step under the lock: a publish never lands between them.
        match std::fs::read(&path) {
            Ok(bytes) => { std::fs::remove_file(&path).map_err(|e| WebError::Config(e.to_string()))?; Ok(Ok(Some(bytes))) }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ok(None)),
            Err(e) => Err(WebError::Config(e.to_string())),
        }
    }).await?;
    match found {
        Err(refusal) => {
            flash::set(&ctx.session, flash::ALERT, job::refusal_message(ctx.locale, &refusal));
            Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/tracker")))
        }
        Ok(Some(bytes)) => {
            let name = if raw_scope.as_deref() == Some("broker") { "broker-tax-report" } else { "tax-report" };
            let filename = format!("deltabadger-{name}-{}-{year}.csv", job::report_country(&country).to_lowercase());
            Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
                 (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"; filename*=UTF-8''{filename}")),
                 (header::HeaderName::from_static("content-transfer-encoding"), "binary".to_string())], bytes).into_response())
        }
        Ok(None) => {
            flash::set(&ctx.session, flash::ALERT, ctx.t("tracker.tax_report.expired"));
            Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/tracker")))
        }
    }
}

/// `POST /broadcasts/tax_report_state` (D-TAX-RECONCILE, this build's own): the request's terminal panel broadcast
/// again on the user's stream; nothing while it is queued or running. Another user's request is "not found".
pub async fn tax_report_state(State(app): State<crate::web::App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let head = |status: StatusCode| (status, [(header::CONTENT_TYPE, "text/html")]).into_response();
    let Some(owner) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)) };
    let request = ctx.params.json.as_ref().and_then(|j| j.get("request")).and_then(Value::as_str).map(str::to_string);
    let Some(request) = request.filter(|r| r.len() == 32 && r.bytes().all(|b| b.is_ascii_hexdigit())) else { return Ok(head(StatusCode::NOT_FOUND)) };
    let tmp = app.config.tmp_dir.clone();
    let found = app.db(move |c| {
        let Some(r) = job::by_request(c, &request)?.filter(|r| r.user_id == owner) else { return Ok(None) };
        let _files = job::files(owner, &r.scope);
        let r = job::by_request(c, &request)?.ok_or_else(|| WebError::Config("a tax report request vanished".into()))?;
        let shown = job::terminal(&tmp, &r);
        Ok(Some((r, shown)))
    }).await?;
    let Some((r, shown)) = found else { return Ok(head(StatusCode::NOT_FOUND)) };
    if let Some(p) = shown {
        let html = panel(&r.locale, &r, &p);
        app.hub.broadcast(&format!("user_{owner}:tax_report"), &turbo::stream("replace", &format!("tax-report-{}", r.request), &html));
    }
    Ok(head(StatusCode::OK))
}
