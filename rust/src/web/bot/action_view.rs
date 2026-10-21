//! Shared page fragments, constructed while the writer still owns its transaction.
use super::{draft::{Draft, FieldError}, page, settings, status, write::{self, Action, LifecycleView, Prepared}, Bot};
use crate::enums::BotStatus;
use crate::web::{bots, flash, format, i18n::{self, Arg}, layout::Ctx, shell, turbo, WebError};
use askama::Template;
use axum::{http::StatusCode, response::{IntoResponse, Response}};
use rusqlite::{Connection, OptionalExtension};

pub(super) fn response(body: String, invalid: bool) -> Response {
    (if invalid { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::OK }, [(axum::http::header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response()
}
fn alert(errors: &[FieldError]) -> Result<String, WebError> {
    let messages = if errors.is_empty() { vec![] } else { vec![flash::Message { style: "danger", text: crate::ruby::to_sentence(&errors.iter().map(|e|e.message.clone()).collect::<Vec<_>>()) }] };
    Ok(turbo::prepend_flash(flash::render(&messages)?.trim_end()))
}
/// The status bar and the progress bar, each replaced on its own target.
fn bar_streams(bot: &Bot, state: &status::Status) -> String {
    turbo::stream("replace",&bot.dom_id("status_bar"),&format!("{}\n",state.bar.trim_end()))
        + "\n" + &turbo::stream("replace",&bot.dom_id("progress_bar"),state.progress.trim_end())
}
/// BotHelper#status_button_streams: the bot page's Start/Stop and the tile's, back to back (`safe_join`).
fn button_streams(bot: &Bot, state: &status::Status) -> String {
    turbo::stream("replace",&bot.dom_id("status_button"),&state.button) + &turbo::stream("replace",&bot.dom_id("tile_status_button"),&state.tile_button)
}
fn fragments(c: &Connection, ctx: &Ctx, draft: &Draft) -> Result<(String, String, status::Status), WebError> {
    let user = ctx.user().ok_or_else(||WebError::Config("action has no authenticated user".into()))?;
    let csrf = ctx.csrf_token();
    let (_, configured) = bots::market_data(c, &ctx.app)?;
    let stored = (!draft.errors.is_empty()).then_some(&draft.original);
    let state = status::render_unsaved(c,ctx,&csrf,&draft.candidate,stored,configured)?;
    let forms = settings::draft_column(c,ctx,&csrf,draft,&user.time_zone,user.hide_balances)?;
    let exchange = page::exchange_select(c,ctx,&csrf,&draft.candidate,user.id)?;
    Ok((forms,exchange,state))
}
pub(super) fn settings_response(c: &Connection, ctx: &Ctx, draft: &Draft) -> Result<Prepared<Response>, WebError> {
    // BotsController#update: an exchange, quote or interval that does not exist cannot be drawn, so a
    // refused update redraws the bot as stored, with the refusal only in the flash.
    let stored;
    let shown = if !draft.errors.is_empty() && draft.undrawable() { stored = Draft::from_bot(draft.original.clone()); &stored } else { draft };
    let (forms,exchange,state) = fragments(c,ctx,shown)?;
    let bot = &shown.candidate;
    let body = turbo::stream("update",&bot.dom_id("label"),&i18n::escape(&bot.label))
        + "\n" + &turbo::stream("replace","settings",&format!("\n{forms}\n"))
        + "\n" + &turbo::stream("replace","exchange_select",exchange.trim_end())
        + "\n" + &bar_streams(bot,&state)
        + "\n" + &button_streams(bot,&state)
        + "\n" + &alert(&draft.errors)? + "\n";
    Ok(Prepared { response: response(body,!draft.errors.is_empty()), broadcasts: vec![], after_scheduling: None })
}

pub(super) fn lifecycle_response(c: &Connection, ctx: &Ctx, id: i64, action: Action, view: &LifecycleView) -> Result<Prepared<Response>, WebError> {
    if !view.errors.is_empty() {
        if action == Action::Start && view.draft.as_ref().is_some_and(|d|d.original.working()) {
            if let Some(draft) = &view.draft {
                let mut current = Draft::from_bot(draft.original.clone());
                current.errors = view.errors.clone();
                return settings_response(c,ctx,&current);
            }
        }
        return Ok(Prepared { response: response(alert(&view.errors)?,true), broadcasts: vec![], after_scheduling: None });
    }
    let user = ctx.user().ok_or_else(||WebError::Config("action has no authenticated user".into()))?;
    // Bot#with_user_locale: the owner's open pages hear the change in the owner's language and paths; the answer keeps the request's.
    let mut heard = super::live::owner_ctx(&ctx.app,user.clone());
    heard.now = ctx.now;
    let heard = &heard;
    let csrf = ctx.csrf_token();
    let (_,configured) = bots::market_data(c,&ctx.app)?;
    let stop_notice = if action == Action::Stop && write::orders_may_execute(c, id)? {
        turbo::prepend_flash(flash::render(&[flash::Message { style: "primary", text: "Orders already sent to the exchange may still execute.".into() }])?.trim_end())
    } else { String::new() };
    let bot = match &view.draft {
        Some(d) => Some(d.candidate.clone()),
        None if action == Action::Archive => match Bot::find(c,user.id,id,super::For::Page,ctx.locale) {
            Ok(bot) => bot,
            Err(e) if super::unreadable(&e) => None,
            Err(e) => return Err(e),
        },
        None => None,
    };
    let mut body = match action {
        Action::Delete => turbo::redirect(&ctx.path("/bots")),
        Action::Unarchive => turbo::refresh().into(),
        Action::Archive => {
            if let Some(bot) = &bot {
                let state = status::render(c,ctx,&csrf,bot,configured)?;
                let count:i64 = c.query_row("SELECT COUNT(*) FROM bots WHERE user_id=?1 AND status NOT IN (3,7)",[user.id],|r|r.get(0))?;
                let size = shell::bot_count_font_size(count);
                let count = format!("<svg id=\"bot-count\" class=\"icon-24 icon-24--count\" viewBox=\"0 0 24 24\" fill=\"none\" xmlns=\"http://www.w3.org/2000/svg\">\n  <text class=\"fill--icon\" x=\"12\" y=\"{}\" text-anchor=\"middle\" font-size=\"{size}\">{count}</text>\n</svg>\n",shell::bot_count_baseline(size));
                bar_streams(bot,&state)
                    + "\n" + &button_streams(bot,&state)
                    + "\n" + &turbo::stream("replace",&bot.dom_id("menu"),&page::menu(ctx,&csrf,bot,configured)?)
                    + "\n" + &turbo::stream("replace","bot-count",&count)
            } else { turbo::refresh().into() }
        }
        Action::Start | Action::Stop => {
            // A Stop always commits: any fragment that cannot be rendered becomes a refresh, never an error that rolls the Stop back.
            let rendered = (|| -> Result<Option<String>, WebError> {
            if let Some(draft) = &view.draft {
                let (forms,exchange,_) = fragments(c,ctx,draft)?;
                let mut body = turbo::stream("replace","settings",&format!("\n{forms}\n"))+"\n"+&turbo::stream("replace","exchange_select",exchange.trim_end());
                if action == Action::Stop {
                    let ids = c.prepare("SELECT id FROM bots WHERE user_id=?1 AND status<>3 ORDER BY id")?.query_map([user.id],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
                    for id in ids {
                        let Some(other) = Bot::find(c,user.id,id,super::For::Page,ctx.locale)? else { return Ok(None) };
                        if other.unrendered().is_some() { return Ok(None); }
                        let state = status::render(c,ctx,&csrf,&other,configured)?;
                        body += "\n"; body += &button_streams(&other,&state);
                    }
                }
                Ok(Some(body))
            } else { Ok(Some(turbo::refresh().into())) }
            })();
            match rendered {
                Ok(Some(body)) => body,
                Ok(None) => return Ok(Prepared { response: response(format!("{}{stop_notice}",turbo::refresh()),false), broadcasts: broadcasts(c,heard,view,action).unwrap_or_default(), after_scheduling: None }),
                Err(_) if action == Action::Stop => return Ok(Prepared { response: response(format!("{}{stop_notice}",turbo::refresh()),false), broadcasts: broadcasts(c,heard,view,action).unwrap_or_default(), after_scheduling: None }),
                Err(e) => return Err(e),
            }
        }
    };
    if !stop_notice.is_empty() { body.push('\n'); body.push_str(&stop_notice); }
    Ok(Prepared { response: response(if matches!(action,Action::Delete|Action::Unarchive) { body } else { body + "\n" },false), broadcasts: if action == Action::Stop { broadcasts(c,heard,view,action).unwrap_or_default() } else { broadcasts(c,heard,view,action)? },
        after_scheduling: if action == Action::Start { awaits_scheduling(c,ctx,view)? } else { None } })
}
/// The stream messages a service write sends, and the bot whose bars wait for the engine to schedule it.
pub type ServiceBroadcasts = (Vec<(String,String)>, Option<i64>);
/// What a lifecycle write through the services (REST, MCP) sends after it commits, as the web's own action does: the
/// owner's bot streams, and a Continue's bars once the engine has scheduled it.
pub fn service_broadcasts(c: &Connection, ctx: &Ctx, view: &LifecycleView, action: Action) -> Result<ServiceBroadcasts, WebError> {
    let sent = if action == Action::Stop { broadcasts(c,ctx,view,action).unwrap_or_default() } else { broadcasts(c,ctx,view,action)? };
    Ok((sent, if action == Action::Start { awaits_scheduling(c,ctx,view)? } else { None }))
}
fn broadcasts(c: &Connection, ctx: &Ctx, view: &LifecycleView, action: Action) -> Result<Vec<(String,String)>, WebError> {
    let Some(draft) = &view.draft else { return Ok(vec![]) };
    let bot = &draft.candidate;
    if draft.original.status == bot.status { return Ok(vec![]) }
    let user = ctx.user().ok_or_else(||WebError::Config("broadcast has no owner".into()))?;
    let (_,configured) = bots::market_data(c,&ctx.app)?;
    // Broadcast forms cannot carry a requester's session token into another browser.
    let state = status::broadcast(c,ctx,bot,configured)?;
    let stream = format!("user_{}:bot_updates",user.id);
    let mut output = vec![];
    // A start that waits for its job skips Bot#broadcast_status_bar_update; Bot::BroadcastAfterScheduledActionJob sends it
    // once the job is enqueued. A delayed first start is scheduled by this write, so its bars follow at once; a Continue
    // waits for the engine (`awaits_scheduling`).
    let delayed = action == Action::Start && bot.started_at.is_some_and(|at|at>ctx.now);
    let deferred = delayed || awaits_scheduling(c,ctx,view)?.is_some();
    if !deferred { output.extend(bar_broadcasts(&stream,bot,&state)); }
    output.extend(button_broadcasts(&stream,bot,&state));
    if delayed { output.extend(bar_broadcasts(&stream,bot,&state)); }
    Ok(output)
}
/// Bot#broadcast_status_button_update (one message per target), then Bot#broadcast_columns_lock_update.
pub(super) fn button_broadcasts(stream: &str, bot: &Bot, state: &status::Status) -> [(String,String); 3] {
    [(stream.to_string(),turbo::stream("replace",&bot.dom_id("status_button"),&state.button)),
     (stream.to_string(),turbo::stream("replace",&bot.dom_id("tile_status_button"),&state.tile_button)),
     (stream.to_string(),if bot.working() { turbo::add_class(&bot.dom_id("columns"),"bot-locked") } else { turbo::remove_class(&bot.dom_id("columns"),"bot-locked") })]
}
/// Bot#broadcast_status_bar_update: the status bar, then the progress bar, one message each.
pub(super) fn bar_broadcasts(stream: &str, bot: &Bot, state: &status::Status) -> [(String,String); 2] {
    [(stream.to_string(),turbo::stream("replace",&bot.dom_id("status_bar"),&format!("{}\n",state.bar.trim_end()))),
     (stream.to_string(),turbo::stream("replace",&bot.dom_id("progress_bar"),state.progress.trim_end()))]
}
/// A Continue within the interval: the engine decides when the bot next acts (engine::run::continue_start), so its bars
/// wait for that decision.
fn awaits_scheduling(c: &Connection, ctx: &Ctx, view: &LifecycleView) -> Result<Option<i64>, WebError> {
    let Some(draft) = &view.draft else { return Ok(None) };
    let bot = &draft.candidate;
    if draft.original.status == bot.status || !bot.transient.contains_key("rust_continue_start") || !within(c,&draft.original,ctx)? { return Ok(None) }
    Ok(Some(bot.id))
}
/// Bot::BroadcastAfterScheduledActionJob for a Continue: waits up to five seconds (Rails' 50 × 0.1 s) for the engine to take
/// the request, then broadcasts the bot's status and progress bars as they are then, whatever it found. Rendered as the
/// owner sees them (Bot#with_user_locale), whatever the language of the request that asked: every tab of theirs hears it.
pub fn broadcast_after_scheduling(ctx: Ctx, id: i64) {
    tokio::spawn(async move {
        for _ in 0..50 {
            let pending = ctx.app.db(move |c| Ok(c.query_row("SELECT json_type(transient_data, '$.rust_continue_start') IS NOT NULL FROM bots WHERE id = ?1",
                                                                [id], |r| r.get::<_, bool>(0)).optional()?)).await;
            if !matches!(pending, Ok(Some(true))) { break; }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let app = ctx.app.clone();
        let rendered = app.db(move |c| {
            let user = ctx.user().ok_or_else(||WebError::Config("broadcast has no owner".into()))?;
            let Some(user) = crate::web::auth::User::find(c,user.id)? else { return Ok(None) };
            let ctx = super::live::owner_ctx(&ctx.app,user.clone());
            // A bot deleted meanwhile: Rails' job cannot load it, and broadcasts nothing.
            let Some(bot) = Bot::find(c,user.id,id,super::For::Page,ctx.locale)? else { return Ok(None) };
            let (_,configured) = bots::market_data(c,&ctx.app)?;
            let state = status::broadcast(c,&ctx,&bot,configured)?;
            Ok(Some(bar_broadcasts(&format!("user_{}:bot_updates",user.id),&bot,&state)))
        }).await;
        if let Ok(Some(messages)) = rendered { for (stream,html) in messages { app.hub.broadcast(&stream,&html); } }
    });
}
fn within(c: &Connection, bot: &Bot, ctx: &Ctx) -> Result<bool,WebError> {
    if !bot.restarting() { return Ok(false) }
    let pending = write::pending(c,bot,ctx.now)?;
    let key = if bot.on("smart_intervaled") { "smart_interval_quote_amount" } else { "quote_amount" };
    let effective = bot.number(key).ok_or_else(||super::data("missing effective amount".into()))?;
    Ok(effective.sub(&pending).ok_or_else(||super::data("pending amount comparison overflow".into()))?.is_positive())
}

#[derive(Template)]
#[template(path="bots/edit.html")]
struct Rename<'a> { v: &'a Ctx, csrf: &'a str, bot: &'a Bot, path: String }
#[derive(Template)]
#[template(path="bots/deletes/edit.html")]
struct Delete<'a> { v: &'a Ctx, csrf: &'a str, bot: &'a Bot, path: String }
#[derive(Template)]
#[template(path="bots/archives/edit.html")]
struct Archive<'a> { v: &'a Ctx, csrf: &'a str, bot: &'a Bot, path: String }
#[derive(Template)]
#[template(path="bots/starts/edit.html")]
struct Restart<'a> { bot: &'a Bot, csrf: &'a str, path: String, info: String, skip: String, resume: String }

pub(super) fn modal(c: &Connection, ctx: &Ctx, bot: &Bot, form: &str, csrf: &str) -> Result<String,WebError> {
    let path = ctx.path(&format!("/bots/{}",bot.id));
    Ok(match form {
        "rename" => Rename { v:ctx,csrf,bot,path }.render()?,
        "delete" => Delete { v:ctx,csrf,bot,path }.render()?,
        "archive" => Archive { v:ctx,csrf,bot,path }.render()?,
        _ if bot.status != BotStatus::Stopped || !bot.restarting() => "<turbo-frame id=\"modal\"></turbo-frame>".into(),
        _ => {
            let on_schedule = within(c,bot,ctx)?;
            let branch = if on_schedule {"on_schedule"}else{"missed"};
            let info = if on_schedule {
                let next = bot.checkpoints(ctx.now)?.ok_or_else(||super::data("missing checkpoint".into()))?.next_us;
                let seconds = next.checked_sub(ctx.now.timestamp_micros()).ok_or_else(||super::data("checkpoint overflow".into()))? as f64 / 1_000_000.0;
                let time = if seconds.abs() < 60.0 {
                    let minute = i18n::text(ctx.locale,"datetime.dotiw.minutes",&[("count",Arg::Count(1))]);
                    i18n::text(ctx.locale,"datetime.dotiw.less_than_x",&[("distance",Arg::Text(&minute))])
                } else { format::distance_of_time_in_words((seconds / 60.0).trunc() * 60.0,ctx.now,ctx.locale) };
                i18n::t(ctx.locale,"bot.buttons.start.on_schedule.info_html",&[("time",Arg::Text(&time))])
            } else {
                let amount = write::pending(c,bot,ctx.now)?.round(bot.quote_decimals().unwrap_or(2)).to_s();
                i18n::t(ctx.locale,"bot.buttons.start.missed.info_html",&[("amount",Arg::Text(&amount)),("quote",Arg::Text(bot.quote_symbol().unwrap_or("")))])
            };
            Restart { bot,csrf,path,info,skip:i18n::text(ctx.locale,&format!("bot.buttons.start.{branch}.skip"),&[]),resume:i18n::text(ctx.locale,&format!("bot.buttons.start.{branch}.continue"),&[]) }.render()?
        }
    })
}
