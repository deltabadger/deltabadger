//! A25 (PLAN-A25 r4, D-JOBS-RUST-VIEW): the install admin's view of Rust's own scheduler and engine at `/jobs`, in place of
//! Rails' Mission Control. Read-only but for one action, Run now, which wakes a listed job through `Wakers::wake`.
//! Nothing here places, cancels or settles an order or touches a bot's intent.
//!
//! Never shown (D-JOBS-DIAGNOSTIC-OMITTED): a job's stored error text (it can carry a venue's or data-api's reply), a
//! scope that is not all digits (it renders hidden and has no button, so no form field carries it), a bot's name or
//! settings, or any email. Owners are `user #id`.
use crate::jobs::{schedule::Schedule, state, Listed};
use crate::web::{auth, flash, layout::{self, Ctx, Page}, shell::{self, Shell}, App, WebError};
use askama::Template;
use axum::{extract::{Extension, State}, http::{header, StatusCode}, response::{IntoResponse, Response}};
use chrono::{DateTime, Utc};

const PATH: &str = "/jobs";

struct Row { scope: String, form_scope: Option<String>, cadence: String, last_run: String, last_refresh: String,
             latest_error: String, failing: Option<&'static str>, incomplete: String, state: String, runnable: bool }
struct Group { name: &'static str, count: usize, failing: usize, scoped: bool, rows: Vec<Row> }
struct BotRow { id: i64, user_id: i64, status: &'static str, link: Option<String> }

struct Engine { working: Vec<(&'static str, usize)>, sections: Vec<(&'static str, Vec<BotRow>)>, unreadable: usize }

#[derive(Template)]
#[template(path = "admin_jobs.html")]
struct View { csrf: String, attached: bool, groups: Vec<Group>, engine: Engine, as_of: String }

/// Shown and posted only when every character is a digit: every scope this crate makes (key, user and asset ids,
/// the repair version). Anything else stays off the page.
fn shown(scope: &str) -> bool { !scope.is_empty() && scope.len() <= 20 && scope.bytes().all(|b| b.is_ascii_digit()) }

fn at(t: Option<DateTime<Utc>>) -> String { t.map_or_else(|| "—".into(), |t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string()) }

fn cadence(s: Option<Schedule>) -> String {
    match s {
        Some(Schedule::Daily { hour, minute }) => format!("daily at {hour:02}:{minute:02} UTC"),
        Some(Schedule::EveryHours { every, minute }) => format!("every {every} h at :{minute:02} UTC"),
        None => "on demand".into(),
    }
}

fn row(l: &Listed, st: &state::JobState) -> Row {
    let scope = match l.scope.as_deref() { None => String::new(), Some(s) if shown(s) => s.to_string(), Some(_) => "(scope hidden)".into() };
    let visible = l.scope.as_deref().is_none_or(shown);
    let state = if l.running { "running".into() } else if let Some(t) = l.retry_at { format!("retry at {}", at(Some(t))) }
        else if l.woken { "queued".into() } else if let Some(t) = l.scheduled { format!("next at {}", at(Some(t))) } else { "—".into() };
    Row { form_scope: visible.then(|| l.scope.clone().unwrap_or_default()), scope, cadence: cadence(l.schedule),
          last_run: at(st.last_run_at), last_refresh: at(st.last_success_at), latest_error: at(st.last_error_at), failing: st.error_class(),
          incomplete: st.incomplete_since.map_or_else(String::new, |t| at(Some(t))), state, runnable: l.manual && visible }
}

/// Unscoped jobs one group each in registration order; scoped ones grouped by name, failing scopes first, then by scope.
fn groups(listed: &[Listed], states: &[state::JobState]) -> Vec<Group> {
    let mut out: Vec<Group> = vec![];
    for (l, st) in listed.iter().zip(states) {
        let scoped = l.scope.is_some();
        let i = match out.iter().position(|g| g.name == l.name && g.scoped == scoped) {
            Some(i) => i,
            None => { out.push(Group { name: l.name, count: 0, failing: 0, scoped, rows: vec![] }); out.len() - 1 }
        };
        let r = row(l, st);
        out[i].count += 1;
        if r.failing.is_some() { out[i].failing += 1; }
        out[i].rows.push(r);
    }
    for g in &mut out {
        g.rows.sort_by_key(|r| (r.failing.is_none(), r.form_scope.as_deref().and_then(|s| s.parse::<u128>().ok()).unwrap_or(u128::MAX)));
    }
    out
}

/// One read over `bots`, guarded so a malformed `transient_data` never aborts it: such a row is counted unreadable.
fn engine(c: &rusqlite::Connection, admin: i64) -> Result<Engine, WebError> {
    use crate::enums::{BotStatus, BOT_WORKING};
    let mut s = c.prepare("SELECT id, user_id, status, json_valid(coalesce(transient_data, '{}')), \
        CASE WHEN json_valid(coalesce(transient_data, '{}')) THEN json_extract(coalesce(transient_data, '{}'), '$.rust_placement') IS NOT NULL END, \
        CASE WHEN json_valid(coalesce(transient_data, '{}')) THEN json_extract(coalesce(transient_data, '{}'), '$.rust_set_aside') IS NOT NULL END \
        FROM bots WHERE status <> ?1 ORDER BY id")?;
    let mut working: Vec<(&'static str, usize)> = BOT_WORKING.iter().map(|s| (s.label(), 0)).collect();
    let (mut executing, mut retrying, mut unresolved, mut set_aside, mut unreadable) = (vec![], vec![], vec![], vec![], 0);
    let rows = s.query_map([BotStatus::Deleted as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, bool>(3)?,
                                                                 r.get::<_, Option<bool>>(4)?.unwrap_or(false), r.get::<_, Option<bool>>(5)?.unwrap_or(false))))?;
    for found in rows {
        let Ok((id, user_id, status, valid, placement, aside)) = found else { unreadable += 1; continue };
        let Some(status) = BotStatus::from_i64(status) else { unreadable += 1; continue };
        if !valid { unreadable += 1; }
        if let Some(w) = working.iter_mut().find(|(label, _)| *label == status.label()) { w.1 += 1; }
        let bot = || BotRow { id, user_id, status: status.label(), link: (user_id == admin).then(|| format!("/bots/{id}")) };
        if status == BotStatus::Executing { executing.push(bot()); }
        if status == BotStatus::Retrying { retrying.push(bot()); }
        if placement { unresolved.push(bot()); }
        if aside { set_aside.push(bot()); }
    }
    Ok(Engine { working, unreadable, sections: vec![("Executing (a tick in hand)", executing), ("Retrying", retrying),
                                                     ("Order unresolved", unresolved), ("Set aside", set_aside)] })
}

/// GET /jobs.
pub async fn show(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    // Rails mounts /jobs outside the locale scope: `/de/jobs` stays the refusal it was.
    if ctx.params.path_locale.is_some() { return Ok(layout::not_ported_for(&ctx)); }
    let user = match auth::admin(&ctx) { Ok(user) => user.clone(), Err(answer) => return Ok(answer) };
    let wakers = app.job_wakers().ok();
    let listed = wakers.as_ref().map(|w| w.listing()).unwrap_or_default();
    let (inner, owner, keys) = (app.clone(), user.clone(), listed.iter().map(|l| (l.name, l.scope.clone())).collect::<Vec<_>>());
    let (shell, states, engine) = app.db(move |c| {
        let states = keys.iter().map(|(name, scope)| state::read(c, name, scope.as_deref()).map_err(|e| WebError::Config(format!("job state: {e}"))))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((Shell::load(c, &inner, &owner)?, states, engine(c, owner.id)?))
    }).await?;
    let csrf = ctx.csrf_token();
    let body = View { csrf: csrf.clone(), attached: wakers.is_some(), groups: groups(&listed, &states), engine, as_of: at(Some(ctx.now)) }.render()?;
    shell::application(&ctx, &csrf, &user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] })
}

fn unprocessable() -> Response { (StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, "text/html")]).into_response() }

/// POST /jobs with `job` and `scope` (digits, or empty for an unscoped job): Run now. Only a listed, manual slot is woken,
/// so a wake can never reach the resolver (whose refusal would stop the scheduler).
pub async fn run(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.params.path_locale.is_some() { return Ok(layout::not_ported_for(&ctx)); }
    if let Err(answer) = auth::admin(&ctx) { return Ok(answer); }
    let name = ctx.params.form("job").unwrap_or_default();
    let scope = ctx.params.form("scope").filter(|s| !s.is_empty());
    if scope.is_some_and(|s| !shown(s)) { return Ok(unprocessable()); }
    let Ok(wakers) = app.job_wakers() else { return Ok(unprocessable()) };
    let Some(slot) = wakers.listing().into_iter().find(|l| l.name == name && l.scope.as_deref() == scope && l.manual) else { return Ok(unprocessable()) };
    wakers.wake(slot.name, scope, None);
    let what = match scope { Some(s) => format!("{} ({s})", slot.name), None => slot.name.to_string() };
    flash::set(&ctx.session, flash::NOTICE, format!("Queued {what}: it runs as soon as the runner is free."));
    Ok(layout::redirect(StatusCode::SEE_OTHER, PATH))
}

/// `/jobs/*`: nothing is there, for anyone (D-JOBS-SUBPATHS-ABSENT).
pub async fn absent(Extension(ctx): Extension<Ctx>) -> Response {
    if ctx.params.path_locale.is_some() { return layout::not_ported_for(&ctx); }
    layout::missing()
}
