//! Bots::Signal's writes (plan 8c-2): its rules (Bots::BotSignalsController), the Resolve of an order of unknown fate
//! (Bots::SignalResolutionsController), its settings (BotsController#update) and its own thin lifecycle (Bots::Signal#start,
//! #stop, #delete; Bot#archive, #unarchive; Automation::Executable#destroy), for the browser and for BotApi's services.
//! Each is one immediate transaction, judged by the engine's guard before it commits, as every other writer is.
use super::{action_params::ActionParams, action_view, draft::FieldError, page, signal, status, write::{self, Action, Failure, Outcome}, Bot, For, Kind};
use crate::engine::{eligibility, model::{self, Level, SIGNAL_TYPE}};
use crate::enums::BotStatus;
use crate::ruby::BigDec;
use crate::web::bot_api::Shown;
use crate::web::{auth, bots, flash, i18n::{self, Arg}, layout::{self, Ctx}, turbo, App, WebError};
use axum::{extract::{Extension, Path, State}, http::{header, HeaderMap, StatusCode}, response::{IntoResponse, Response}};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// What a browser write answers, decided under its transaction.
enum Answer {
    /// Bots::Botable#set_bot: not this user's, or deleted.
    Missing,
    /// ActiveRecord::RecordNotFound below the bot (a rule that is not this bot's): 404.
    NotFound,
    /// ActionController::ParameterMissing: 400.
    BadRequest,
    /// ActiveRecord::RecordInvalid from `update!`: 422.
    RecordInvalid,
    Unported(&'static str),
    Done(Response),
}

fn flash_stream(style: &'static str, text: String) -> Result<String, WebError> {
    Ok(turbo::prepend_flash(flash::render(&[flash::Message { style, text }])?.trim_end()))
}

fn alert(text: String) -> Result<String, WebError> { flash_stream("danger", text) }

/// `head :ok` of a Turbo stream request: no body, and the format's type without a charset.
fn head_ok() -> Response { (StatusCode::OK, [(header::CONTENT_TYPE, "text/vnd.turbo-stream.html")]).into_response() }

/// `errors.messages.values.flatten.to_sentence`: each attribute's messages together, attributes in the order first named.
fn sentence(errors: &[FieldError]) -> String {
    let mut fields: Vec<&str> = vec![];
    for e in errors { if !fields.contains(&e.field.as_str()) { fields.push(&e.field); } }
    crate::ruby::to_sentence(&fields.iter().flat_map(|f| errors.iter().filter(move |e| e.field == *f).map(|e| e.message.clone())).collect::<Vec<_>>())
}

/// An ActiveModel error's message, looked up as `errors.add(field, code)` looks it up for a Bots::Signal (a missing
/// translation is Rails' own text for it).
fn model_message(locale: &str, field: &str, code: &str) -> String {
    let keys = [format!("activerecord.errors.models.bots/signal.attributes.{field}.{code}"), format!("activerecord.errors.models.bots/signal.{code}"),
                format!("activerecord.errors.models.bot.attributes.{field}.{code}"), format!("activerecord.errors.models.bot.{code}"),
                format!("activerecord.errors.messages.{code}"), format!("errors.attributes.{field}.{code}"), format!("errors.messages.{code}")];
    keys.iter().map(|key| i18n::text(locale, key, &[])).find(|found| !found.starts_with("Translation missing:"))
        .unwrap_or_else(|| format!("Translation missing. Options considered were:\n{}", keys.iter().map(|key| format!("- {locale}.{key}")).collect::<Vec<_>>().join("\n")))
}

fn error(field: &str, message: String) -> FieldError { FieldError { field: field.into(), message } }

fn wash_sale(c: &Connection, owner: i64) -> Result<Option<bool>, WebError> {
    Ok(c.query_row("SELECT wash_sale_enabled FROM users WHERE id = ?1", [owner], |r| r.get(0))?)
}

/// Whether the bot is a signal bot of this user's (any status): the actions route to this module for it.
pub fn is_signal(c: &Connection, owner: i64, id: i64) -> Result<bool, WebError> {
    let class: Option<Option<String>> = c.query_row("SELECT type FROM bots WHERE id = ?1 AND user_id = ?2", [id, owner], |r| r.get(0)).optional()?;
    Ok(class.flatten().as_deref() == Some(SIGNAL_TYPE))
}

/// Bots::Botable#set_bot for a signal bot, and `bot::refusal` for what this build serves.
fn owned(c: &Connection, app: &App, owner: i64, id: i64, locale: &str) -> Result<Result<Bot, Answer>, WebError> {
    let status: Option<i64> = c.query_row("SELECT status FROM bots WHERE id = ?1 AND user_id = ?2 AND type = ?3", (id, owner, SIGNAL_TYPE), |r| r.get(0)).optional()?;
    match status {
        None => return Ok(Err(Answer::Missing)),
        Some(3) => return Ok(Err(Answer::Missing)),
        Some(_) => {}
    }
    let (provider, _) = bots::market_data(c, app)?;
    if let Some(reason) = super::refusal(c, id, wash_sale(c, owner)?, provider, For::Settings, app.signal_pages())? { return Ok(Err(Answer::Unported(reason))); }
    Ok(Bot::find(c, owner, id, For::Page, locale)?.ok_or(Answer::Missing))
}

/// What a write left for the pages to refuse: the wash-sale question its rules now owe (a modal this build does not
/// serve), or any other refusal of the bot as it now is.
fn refused_after(c: &Connection, app: &App, owner: i64, id: i64) -> Result<Option<&'static str>, WebError> {
    let (provider, _) = bots::market_data(c, app)?;
    super::refusal(c, id, wash_sale(c, owner)?, provider, For::Page, app.signal_pages())
}

/// bots/signals/_settings, as a stream replacing the column.
fn settings_stream(ctx: &Ctx, csrf: &str, bot: &Bot, base_url: &str) -> Result<String, WebError> {
    let hide = ctx.user().is_some_and(|user| user.hide_balances);
    Ok(turbo::stream("replace", "settings", &format!("{}\n", signal::column(ctx, csrf, bot, hide, base_url)?)))
}

/// `request.base_url`, which a rule's webhook URL starts with.
fn base_url(app: &App, headers: &HeaderMap) -> String { app.config.request_origin(headers).unwrap_or_default() }

fn finish(ctx: &Ctx, answer: Result<Answer, WebError>) -> Result<Response, WebError> {
    match answer {
        Ok(Answer::Missing) => Ok(page::not_found(ctx)),
        Ok(Answer::NotFound) => Ok(layout::missing()),
        Ok(Answer::BadRequest) => Ok(layout::bad_request()),
        Ok(Answer::RecordInvalid) => Err(WebError::RecordInvalid),
        Ok(Answer::Unported(reason)) => Ok(layout::refused(ctx, reason)),
        Ok(Answer::Done(response)) => Ok(response),
        Err(e) => layout::or_refused(ctx, e),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Rules: Bots::BotSignalsController.

/// The rule action, and the rule it names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleOp { Create, Update(i64), Destroy(i64) }

/// SecureRandom.urlsafe_base64(32), drawn again while another rule holds it (BotSignal#generate_token).
fn new_token(c: &Connection) -> Result<String, WebError> { draw_token(c, rand::random) }

fn draw_token(c: &Connection, mut draw: impl FnMut() -> [u8; 32]) -> Result<String, WebError> {
    loop {
        let token = URL_SAFE_NO_PAD.encode(draw());
        let taken: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM bot_signals WHERE token = ?1)", [&token], |r| r.get(0))?;
        if !taken { return Ok(token); }
    }
}

/// ActiveModel::Type::Boolean#cast of a submitted value: nil for a blank text.
fn boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::String(s) => Some(!["0", "f", "F", "false", "FALSE", "off", "OFF"].contains(&s.as_str())),
        Value::Number(n) => Some(n.as_f64() != Some(0.0)),
        _ => Some(true),
    }
}

/// What `@signal.update!(signal_params)` assigns: each permitted scalar cast as BotSignal's attributes cast it.
enum Cast<T> { Absent, Value(T), Invalid, Raises }

fn enum_value(fields: &Map<String, Value>, key: &str, names: [&str; 2]) -> Cast<i64> {
    match fields.get(key) {
        None | Some(Value::Array(_) | Value::Object(_)) => Cast::Absent,
        // A blank is nil: the presence validation (direction) or the NOT NULL column (amount_type) refuses it.
        Some(Value::Null) => Cast::Invalid,
        Some(Value::String(s)) if s.is_empty() => if key == "direction" { Cast::Invalid } else { Cast::Raises },
        Some(value) => {
            let text = match value { Value::String(s) => s.clone(), other => other.to_string() };
            match names.iter().position(|name| *name == text) { Some(i) => Cast::Value(i as i64), None => Cast::Raises }
        }
    }
}

fn rule_inner(c: &Connection, ctx: &Ctx, owner: i64, id: i64, op: RuleOp, params: &ActionParams, base_url: &str) -> Result<Answer, WebError> {
    let tx = model::immediate(c)?;
    let mut ctx = ctx.clone();
    ctx.now = ctx.app.now();
    let bot = match owned(&tx, &ctx.app, owner, id, ctx.locale)? { Ok(bot) => bot, Err(answer) => return Ok(answer) };
    let csrf = ctx.csrf_token();
    // refuse_while_running (Rails R8b): a stale tab's rule write on a running bot changes nothing.
    if bot.working() {
        let body = alert(i18n::text(ctx.locale, "errors.bots.signal_locked_while_running", &[]))? + &settings_stream(&ctx, &csrf, &bot, base_url)?;
        return Ok(Answer::Done(action_view::response(body, true)));
    }
    let now = crate::codec::format_time(ctx.now);
    let found = match op { RuleOp::Update(rule) | RuleOp::Destroy(rule) => bot.signals.iter().find(|r| r.id == rule).cloned(), RuleOp::Create => None };
    let mut changed = true;
    let mut settings_answer = true;
    match op {
        RuleOp::Create => {
            let token = new_token(&tx)?;
            tx.execute("INSERT INTO bot_signals (bot_id, direction, amount_type, amount, enabled, token, last_triggered_at, created_at, updated_at) \
                        VALUES (?1, 0, 0, '100.0', 1, ?2, NULL, ?3, ?3)", (id, &token, &now))?;
        }
        RuleOp::Update(_) => {
            let Some(current) = found.clone() else { return Ok(Answer::NotFound) };
            let Some(fields) = params.value().get("bot_signal").and_then(Value::as_object).filter(|f| !f.is_empty()).cloned() else { return Ok(Answer::BadRequest) };
            let mut sets: Vec<(&str, rusqlite::types::Value)> = vec![];
            let mut sell = current.sell;
            let mut percentage = current.percentage;
            let mut amount = Some(current.amount.clone());
            let mut amount_text = None;
            for (key, names) in [("direction", ["buy", "sell"]), ("amount_type", ["fixed", "percentage"])] {
                match enum_value(&fields, key, names) {
                    Cast::Absent => {}
                    Cast::Invalid => return Ok(Answer::RecordInvalid),
                    Cast::Raises => return Err(WebError::Config(format!("bot_signals {}: {key} is not one Rails knows", current.id))),
                    Cast::Value(v) => {
                        let old = if key == "direction" { &mut sell } else { &mut percentage };
                        if (v == 1) != *old { *old = v == 1; sets.push((key, v.into())); }
                    }
                }
            }
            if let Some(value) = fields.get("amount").filter(|v| !v.is_array() && !v.is_object()) {
                let text = match value { Value::String(s) => s.trim().to_string(), Value::Null => String::new(), other => other.to_string() };
                amount = BigDec::parse(&text).ok();
                amount_text = Some(text);
            }
            // validates :amount, presence, numericality > 0, and <= 100 for a percentage.
            let Some(new_amount) = amount.filter(|a| a.is_positive() && (!percentage || *a <= BigDec::from_i64(100))) else { return Ok(Answer::RecordInvalid) };
            if amount_text.is_some() && new_amount != current.amount { sets.push(("amount", new_amount.to_s_f().into())); }
            if let Some(value) = fields.get("enabled").filter(|v| !v.is_array() && !v.is_object()) {
                let Some(enabled) = boolean(value) else { return Err(WebError::Config(format!("bot_signals {}: enabled may not be null", current.id))) };
                if enabled != current.enabled { sets.push(("enabled", enabled.into())); }
            } else { settings_answer = false; }
            changed = !sets.is_empty();
            if changed {
                let columns = sets.iter().enumerate().map(|(i, (column, _))| format!("{column} = ?{}", i + 1)).collect::<Vec<_>>().join(", ");
                let mut values: Vec<rusqlite::types::Value> = sets.into_iter().map(|(_, v)| v).collect();
                values.push(now.clone().into());
                values.push(current.id.into());
                let n = values.len();
                tx.execute(&format!("UPDATE bot_signals SET {columns}, updated_at = ?{} WHERE id = ?{n}", n - 1), rusqlite::params_from_iter(values))?;
            }
        }
        RuleOp::Destroy(_) => {
            let Some(current) = found.clone() else { return Ok(Answer::NotFound) };
            if bot.signals.len() <= 1 {
                return Ok(Answer::Done(action_view::response(alert(i18n::text(ctx.locale, "errors.bots.signal_minimum", &[]))?, true)));
            }
            tx.execute("DELETE FROM bot_signals WHERE id = ?1", [current.id])?;
        }
    }
    if changed {
        if let Err(refusal) = eligibility::guard(&tx, &ctx.app.cipher, Some(id)) {
            let body = alert(i18n::text(ctx.locale, "engine.write_refused", &[("reason", Arg::Text(&refusal.reason()))]))?;
            tx.rollback()?;
            return Ok(Answer::Done(action_view::response(body, true)));
        }
        if let Some(reason) = refused_after(&tx, &ctx.app, owner, id)? { return Ok(Answer::Unported(reason)); }
    }
    let response = if settings_answer {
        let bot = Bot::find(&tx, owner, id, For::Page, ctx.locale)?.ok_or(WebError::Config("the rule's bot disappeared".into()))?;
        action_view::response(settings_stream(&ctx, &csrf, &bot, base_url)?, false)
    } else { head_ok() };
    // Nothing to wake the engine for: a call names its rule, and the engine reads the rule when it runs the call.
    tx.commit()?;
    Ok(Answer::Done(response))
}

async fn rule_write(app: App, ctx: Ctx, segment: String, rule: Option<String>, op: fn(i64) -> RuleOp, headers: HeaderMap) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(page::not_found(&ctx)) };
    let op = match rule { Some(text) => match super::id_from_path(&text) { Some(rule) => op(rule), None => return Ok(layout::missing()) }, None => op(0) };
    let params = match ActionParams::parse(&ctx.params) { Ok(p) => p, Err(e) => return Ok(e.status().into_response()) };
    let (view, url) = (ctx.clone(), base_url(&app, &headers));
    let answer = app.db(move |c| rule_inner(c, &view, user.id, id, op, &params, &url)).await;
    finish(&ctx, answer)
}

/// POST /bots/:bot_id/bot_signals: a buy rule for 100.
pub async fn create_rule(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>, headers: HeaderMap) -> Result<Response, WebError> {
    rule_write(app, ctx, segment, None, |_| RuleOp::Create, headers).await
}

/// PATCH /bots/:bot_id/bot_signals/:id.
pub async fn update_rule(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path((segment, rule)): Path<(String, String)>, headers: HeaderMap) -> Result<Response, WebError> {
    rule_write(app, ctx, segment, Some(rule), RuleOp::Update, headers).await
}

/// DELETE /bots/:bot_id/bot_signals/:id: never the last rule.
pub async fn destroy_rule(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path((segment, rule)): Path<(String, String)>, headers: HeaderMap) -> Result<Response, WebError> {
    rule_write(app, ctx, segment, Some(rule), RuleOp::Destroy, headers).await
}

// ---------------------------------------------------------------------------------------------------------------------
// Resolve: Bots::SignalResolutionsController and Bots::Signal#resolve_unresolved_placement!.

enum Resolved { Lifted(Vec<(String, String)>), Stale, Refused(String), Other(Answer) }

/// POST /bots/:bot_id/signal_resolution with `marker_id`: the user says they checked the venue. The marker it names is
/// lifted, with its activity line, and nothing is written to the ledger (plan §R12, the rows Rails has). A marker this
/// build's engine left beside its own unsettled order is the engine's recovery to settle, so that Resolve changes nothing
/// either (named divergence: the placed / not-placed choice of §R12's last rows has no form yet).
pub async fn resolve(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>, headers: HeaderMap) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(layout::missing()) };
    let params = match ActionParams::parse(&ctx.params) { Ok(p) => p, Err(e) => return Ok(e.status().into_response()) };
    // `params[:marker_id].to_s`.
    let marker_id = match params.value().get("marker_id") { None | Some(Value::Null) => String::new(), Some(Value::String(s)) => s.clone(), Some(other) => other.to_string() };
    let inner = app.clone();
    let owner = user.clone();
    let outcome = app.db(move |c| -> Result<Resolved, WebError> {
        let tx = model::immediate(c)?;
        let transient: Option<String> = tx.query_row("SELECT transient_data FROM bots WHERE id = ?1 AND user_id = ?2 AND type = ?3", (id, owner.id, SIGNAL_TYPE), |r| r.get(0)).optional()?;
        let Some(transient) = transient else { return Ok(Resolved::Other(Answer::NotFound)) };
        let (provider, _) = bots::market_data(&tx, &inner)?;
        if let Some(reason) = super::refusal(&tx, id, wash_sale(&tx, owner.id)?, provider, For::Settings, inner.signal_pages())? { return Ok(Resolved::Other(Answer::Unported(reason))); }
        let transient: Value = serde_json::from_str(&transient).unwrap_or(Value::Null);
        let marker = transient.get(crate::engine::signal::MARKER_KEY).and_then(Value::as_object);
        let engine_owns = transient.get("rust_placement").is_some_and(|v| !v.is_null());
        if marker_id.is_empty() || engine_owns || marker.and_then(|m| m.get("id")).and_then(Value::as_str) != Some(marker_id.as_str()) { return Ok(Resolved::Stale); }
        tx.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{}') WHERE id = ?1", crate::engine::signal::MARKER_KEY), [id])?;
        model::log_activity(&tx, id, "signal_placement_resolved", Level::Info, json!({ "user_id": owner.id, "marker_id": marker_id }), inner.now())?;
        if let Err(refusal) = eligibility::guard(&tx, &inner.cipher, Some(id)) {
            tx.rollback()?;
            return Ok(Resolved::Refused(refusal.reason()));
        }
        // Bots::Signal#broadcast_unresolved_placement, in the owner's language, without a form token.
        let seen = super::live::owner_ctx(&inner, owner.clone());
        let broadcast = Bot::find(&tx, owner.id, id, For::Page, seen.locale)?
            .map(|bot| (page::page_stream(&bot), signal::unresolved_broadcast(&seen, &bot)));
        tx.commit()?;
        Ok(Resolved::Lifted(broadcast.into_iter().collect()))
    }).await;
    let fallback = ctx.path(&format!("/bots/{id}"));
    let back = |kind: &str, text: String| {
        flash::set(&ctx.session, kind, text);
        layout::redirect(StatusCode::FOUND, &layout::back(&app.config, &headers).unwrap_or(fallback.clone()))
    };
    match outcome {
        Ok(Resolved::Lifted(broadcasts)) => {
            app.wake_engine();
            for (stream, html) in broadcasts { app.hub.broadcast(&stream, &html); }
            Ok(back(flash::NOTICE, i18n::text(ctx.locale, "bot.signal.resolved", &[])))
        }
        Ok(Resolved::Stale) => Ok(back(flash::ALERT, i18n::text(ctx.locale, "bot.signal.resolution_stale", &[]))),
        Ok(Resolved::Refused(reason)) => Ok(back(flash::ALERT, i18n::text(ctx.locale, "engine.write_refused", &[("reason", Arg::Text(&reason))]))),
        Ok(Resolved::Other(answer)) => finish(&ctx, Ok(answer)),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Settings: BotsController#update through Bot::SettingsForm.save.

/// bots/update.turbo_stream: the name, the settings column, the venue menu, both status widgets and the flash.
fn update_streams(c: &Connection, ctx: &Ctx, csrf: &str, bot: &Bot, flash: &str, base_url: &str, owner: i64) -> Result<String, WebError> {
    let (_, configured) = bots::market_data(c, &ctx.app)?;
    let state = status::render(c, ctx, csrf, bot, configured)?;
    Ok(turbo::stream("update", &bot.dom_id("label"), &i18n::escape(&bot.label))
        + "\n" + &settings_stream(ctx, csrf, bot, base_url)?
        + "\n" + &turbo::stream("replace", "exchange_select", page::exchange_select(c, ctx, csrf, bot, owner)?.trim_end())
        + "\n" + &action_view::bar_streams(bot, &state)
        + "\n" + &action_view::button_streams(bot, &state)
        + "\n" + flash + "\n")
}

fn settings_inner(c: &Connection, ctx: &Ctx, owner: i64, id: i64, params: &ActionParams, base_url: &str) -> Result<Answer, WebError> {
    let tx = model::immediate(c)?;
    let mut ctx = ctx.clone();
    ctx.now = ctx.app.now();
    let bot = match owned(&tx, &ctx.app, owner, id, ctx.locale)? { Ok(bot) => bot, Err(answer) => return Ok(answer) };
    let Ok(Value::Object(fields)) = params.permitted(Kind::Signal) else { return Ok(Answer::BadRequest) };
    let csrf = ctx.csrf_token();
    let text = |key: &str| fields.get(key).map(|v| match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() });
    // Bot::SettingsForm.save: `label.presence`, `exchange_id`, and Bots::Signal#parse_params (`presence&.to_i`), nils dropped.
    let label = text("label").filter(|l| !l.trim().is_empty());
    let exchange_id = match text("exchange_id") {
        None => None,
        Some(t) => match t.trim().parse::<i64>() { Ok(e) => Some(e), Err(_) => return Ok(Answer::Unported("an exchange id this build does not read")) },
    };
    let mut assets = Map::new();
    for key in ["base_asset_id", "quote_asset_id"] {
        if let Some(value) = fields.get(key).filter(|v| crate::web::bot_api::present(v)) {
            let Some(n) = crate::web::bot_api::to_i(value) else { return Ok(Answer::Unported("an asset id this build does not read")) };
            assets.insert(key.into(), json!(n));
        }
    }
    let new_exchange = match exchange_id {
        Some(e) if e != bot.exchange.id => {
            let name: Option<Option<String>> = tx.query_row("SELECT name FROM exchanges WHERE id = ?1", [e], |r| r.get(0)).optional()?;
            let Some(name) = name else { return Ok(Answer::Unported("an exchange that does not exist")) };
            Some((e, name.unwrap_or_default()))
        }
        _ => None,
    };
    let asset_changed = |key: &str| assets.get(key).is_some_and(|v| bot.settings.get(key) != Some(v));
    let settings_changed = asset_changed("base_asset_id") || asset_changed("quote_asset_id");
    let label_changed = label.as_ref().is_some_and(|l| *l != bot.label || bot.label_unsaved);
    let id_of = |key: &str| assets.get(key).or(bot.settings.get(key)).and_then(Value::as_i64);
    let exchange = new_exchange.as_ref().map_or((bot.exchange.id, bot.exchange.name.clone()), |e| e.clone());
    // The :update validations, in Rails' order.
    let mut errors = vec![];
    if !matches!(bot.status, BotStatus::Stopped | BotStatus::Deleted | BotStatus::Archived) && (new_exchange.is_some() || settings_changed) && !listed(&tx, exchange.0, id_of("base_asset_id"), id_of("quote_asset_id"))? {
        errors.push(error("exchange", i18n::text(ctx.locale, "errors.bots.exchange_asset_mismatch", &[("exchange_name", Arg::Text(&exchange.1))])));
    }
    let mut unknown_quote = false;
    for key in ["base_asset_id", "quote_asset_id"] {
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id IS ?1)", [id_of(key)], |r| r.get(0))?;
        if !exists { errors.push(error(key, model_message(ctx.locale, key, "invalid"))); if key == "quote_asset_id" { unknown_quote = true; } }
    }
    if settings_changed && bot.has_orders {
        for key in ["base_asset_id", "quote_asset_id"].into_iter().filter(|key| asset_changed(key)) { errors.push(error(key, model_message(ctx.locale, key, "unchangeable"))); }
    }
    if new_exchange.is_some() && bot.has_waiting_orders {
        errors.push(error("exchange", i18n::text(ctx.locale, "errors.bots.exchange_change_while_open_orders", &[("exchange_name", Arg::Text(&exchange.1))])));
    }
    let now = crate::codec::format_time(ctx.now);
    let write = |tx: &Connection| -> Result<(), WebError> {
        if let Some(label) = label.as_ref().filter(|_| label_changed) { tx.execute("UPDATE bots SET label = ?1 WHERE id = ?2", (label, id))?; }
        if let Some((e, _)) = &new_exchange { tx.execute("UPDATE bots SET exchange_id = ?1 WHERE id = ?2", (e, id))?; }
        for (key, value) in assets.iter().filter(|(key, _)| asset_changed(key)) {
            tx.execute("UPDATE bots SET settings = json_set(settings, ?1, json(?2)) WHERE id = ?3", (format!("$.{key}"), value.to_string(), id))?;
        }
        if settings_changed { tx.execute("UPDATE bots SET settings_changed_at = ?1 WHERE id = ?2", (&now, id))?; }
        if label_changed || new_exchange.is_some() || settings_changed { tx.execute("UPDATE bots SET updated_at = ?1 WHERE id = ?2", (&now, id))?; }
        Ok(())
    };
    let shown = |tx: &Connection| -> Result<Bot, WebError> {
        let mut bot = Bot::find(tx, owner, id, For::Page, ctx.locale)?.ok_or(WebError::Config("the bot disappeared".into()))?;
        if let Some(label) = &label { bot.label = label.clone(); }
        Ok(bot)
    };
    if !errors.is_empty() {
        // An exchange or quote that does not exist cannot be drawn: the bot as stored, with the refusal in the flash.
        // Otherwise the bot as assigned, drawn from the rows the write would have left (rolled back).
        let shown = if unknown_quote { bot.clone() } else { write(&tx)?; shown(&tx)? };
        let body = update_streams(&tx, &ctx, &csrf, &shown, &alert(sentence(&errors))?, base_url, owner)?;
        tx.rollback()?;
        return Ok(Answer::Done(action_view::response(body, true)));
    }
    write(&tx)?;
    let changed = label_changed || new_exchange.is_some() || settings_changed;
    if changed {
        if let Err(refusal) = eligibility::guard(&tx, &ctx.app.cipher, Some(id)) {
            let shown = shown(&tx)?;
            let flash = alert(i18n::text(ctx.locale, "engine.write_refused", &[("reason", Arg::Text(&refusal.reason()))]))?;
            let body = update_streams(&tx, &ctx, &csrf, &shown, &flash, base_url, owner)?;
            tx.rollback()?;
            return Ok(Answer::Done(action_view::response(body, true)));
        }
        if let Some(reason) = refused_after(&tx, &ctx.app, owner, id)? { return Ok(Answer::Unported(reason)); }
    }
    let bot = shown(&tx)?;
    let body = update_streams(&tx, &ctx, &csrf, &bot, &turbo::prepend_flash(flash::render(&[])?.trim_end()), base_url, owner)?;
    tx.commit()?;
    if changed { ctx.app.wake_engine(); }
    Ok(Answer::Done(action_view::response(body, false)))
}

/// `exchange.tickers.available.trading_enabled.exists?(base_asset:, quote_asset:)`.
fn listed(c: &Connection, exchange: i64, base: Option<i64>, quote: Option<i64>) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM tickers WHERE exchange_id = ?1 AND available = 1 AND trading_enabled = 1 AND base_asset_id IS ?2 AND quote_asset_id IS ?3)",
                   (exchange, base, quote), |r| r.get(0))?)
}

// ---------------------------------------------------------------------------------------------------------------------
// Lifecycle: Bots::Signal#start, #stop, #delete; Bot#archive, #unarchive; Automation::Executable#destroy.

/// `valid?(:start)` with the status assigned `scheduled`: Bot's archived and key checks, Bot::AssetConfigurable's venue
/// check (always run in the :start context) and Bots::Signal's ticker check, in Rails' order.
fn start_errors(c: &Connection, locale: &str, bot: &Bot) -> Result<Vec<FieldError>, WebError> {
    let mut errors = vec![];
    if bot.status == BotStatus::Archived { errors.push(error("status", i18n::text(locale, "errors.bots.archived", &[]))); }
    if !bot.api_key_correct() { errors.push(error("base", i18n::text(locale, "engine.api_key_not_ready", &[]))); }
    let id = |key: &str| bot.settings.get(key).and_then(Value::as_i64);
    if !listed(c, bot.exchange.id, id("base_asset_id"), id("quote_asset_id"))? {
        errors.push(error("exchange", i18n::text(locale, "errors.bots.exchange_asset_mismatch", &[("exchange_name", Arg::Text(&bot.exchange.name))])));
    }
    if signal::start_blocked(bot) {
        for key in ["base_asset_id", "quote_asset_id"] { errors.push(error(key, model_message(locale, key, "invalid"))); }
    }
    Ok(errors)
}

/// One lifecycle write, as the model makes it: refused with the model's errors, refused by the engine's guard, or
/// written (with the broadcasts each status change sends, Bot's after_update_commit callbacks).
enum Life { Invalid(Vec<FieldError>), Refused(String), Written { broadcasts: Vec<(String, String)>, changed: bool } }

/// `update(...)` of the status columns: only what differs is written, and updated_at with it; a status change is
/// broadcast as Bot's callbacks broadcast it (`heard`: the context the owner's pages hear it in).
fn save(c: &Connection, heard: &Ctx, owner: i64, id: i64, status: BotStatus, stopped_at: bool, out: &mut Vec<(String, String)>) -> Result<bool, WebError> {
    let now = crate::codec::format_time(heard.now);
    let (old, old_stopped, old_key): (i64, Option<String>, Option<String>) = c.query_row("SELECT status, stopped_at, stop_message_key FROM bots WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let moved = old != status as i64;
    let changed = moved || (stopped_at && (old_stopped.as_deref() != Some(now.as_str()) || old_key.is_some()));
    if !changed { return Ok(false); }
    c.execute("UPDATE bots SET status = ?1, updated_at = ?2 WHERE id = ?3", (status as i64, &now, id))?;
    if stopped_at { c.execute("UPDATE bots SET stopped_at = ?1, stop_message_key = NULL WHERE id = ?2", (&now, id))?; }
    if moved {
        let bot = Bot::find(c, owner, id, For::Page, heard.locale)?.ok_or(WebError::Config("the bot disappeared".into()))?;
        let (_, configured) = bots::market_data(c, &heard.app)?;
        let state = status::broadcast(c, heard, &bot, configured)?;
        let stream = format!("user_{owner}:bot_updates");
        out.extend(action_view::bar_broadcasts(&stream, &bot, &state));
        out.extend(action_view::button_broadcasts(&stream, &bot, &state));
    }
    Ok(true)
}

/// Bots::Signal#stop: nothing on an archived bot; otherwise stopped now, with its line. Whether it wrote.
fn stop(c: &Connection, heard: &Ctx, owner: i64, bot: &Bot, out: &mut Vec<(String, String)>) -> Result<bool, WebError> {
    if bot.status == BotStatus::Archived { return Ok(false); }
    save(c, heard, owner, bot.id, BotStatus::Stopped, true, out)?;
    model::log_activity(c, bot.id, "stopped", Level::Info, json!({}), heard.now)?;
    Ok(true)
}

/// `service`: BotApi's (Bots::Signal#delete stamps stopped_at; the page's destroy does not).
fn lifecycle_write(c: &Connection, heard: &Ctx, owner: i64, bot: &Bot, action: Action, fresh: bool, service: bool) -> Result<Life, WebError> {
    let mut out = vec![];
    let now = heard.now;
    let changed = match action {
        Action::Start => {
            let errors = start_errors(c, heard.locale, bot)?;
            if !errors.is_empty() { return Ok(Life::Invalid(errors)); }
            let stamp = crate::codec::format_time(now);
            let (started, key): (Option<String>, Option<String>) = c.query_row("SELECT started_at, stop_message_key FROM bots WHERE id = ?1", [bot.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if fresh && started.as_deref() != Some(stamp.as_str()) || key.is_some() {
                c.execute("UPDATE bots SET started_at = CASE WHEN ?1 THEN ?2 ELSE started_at END, stop_message_key = NULL, updated_at = ?2 WHERE id = ?3", (fresh, &stamp, bot.id))?;
            }
            save(c, heard, owner, bot.id, BotStatus::Scheduled, false, &mut out)?;
            model::log_activity(c, bot.id, "started", Level::Info, json!({ "start_fresh": fresh }), now)?;
            true
        }
        Action::Stop => stop(c, heard, owner, bot, &mut out)?,
        Action::Archive => {
            if bot.status != BotStatus::Archived {
                // valid_as_archived?: the :update validations; only the asset ids can fail on a bot whose settings are not changing.
                let mut errors = vec![];
                for key in ["base_asset_id", "quote_asset_id"] {
                    let exists: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id IS ?1)", [bot.settings.get(key).and_then(Value::as_i64)], |r| r.get(0))?;
                    if !exists { errors.push(error(key, model_message(heard.locale, key, "invalid"))); }
                }
                if !errors.is_empty() { return Ok(Life::Invalid(errors)); }
                stop(c, heard, owner, bot, &mut out)? | save(c, heard, owner, bot.id, BotStatus::Archived, false, &mut out)?
            } else { false }
        }
        Action::Unarchive => bot.status == BotStatus::Archived && save(c, heard, owner, bot.id, BotStatus::Stopped, false, &mut out)?,
        Action::Delete => {
            save(c, heard, owner, bot.id, BotStatus::Deleted, false, &mut out)?;
            if service { c.execute("UPDATE bots SET stopped_at = ?1 WHERE id = ?2", (crate::codec::format_time(now), bot.id))?; }
            true
        }
    };
    // STOP only reduces activity (as for every bot): the guard judges the others.
    if action != Action::Stop {
        if let Err(refusal) = eligibility::guard(c, &heard.app.cipher, Some(bot.id)) { return Ok(Life::Refused(refusal.reason())); }
    }
    Ok(Life::Written { broadcasts: out, changed })
}

fn web_lifecycle(c: &Connection, ctx: &Ctx, owner: i64, id: i64, action: Action, params: &ActionParams, base_url: &str) -> Result<Answer, WebError> {
    let tx = if matches!(action, Action::Stop | Action::Archive | Action::Delete) { model::immediate_retry_busy(c)? } else { model::immediate(c)? };
    let mut ctx = ctx.clone();
    ctx.now = ctx.app.now();
    let bot = match owned(&tx, &ctx.app, owner, id, ctx.locale)? { Ok(bot) => bot, Err(answer) => return Ok(answer) };
    let refuse = |text: String| -> Result<Answer, WebError> { Ok(Answer::Done(action_view::response(alert(text)?, true))) };
    // Bots::StartsController reads the flag before Bot#start (the wash-sale question is `bot::refusal`'s).
    let fresh = if action == Action::Start {
        match params.start_fresh() { Ok(fresh) => fresh, Err(_) => return refuse(i18n::text(ctx.locale, "engine.invalid_start_fresh", &[])) }
    } else { true };
    let (written, changed) = match lifecycle_write(&tx, &ctx, owner, &bot, action, fresh, false)? {
        Life::Invalid(errors) => return refuse(sentence(&errors)),
        Life::Refused(reason) => return refuse(i18n::text(ctx.locale, "engine.write_refused", &[("reason", Arg::Text(&reason))])),
        Life::Written { broadcasts, changed } => (broadcasts, changed),
    };
    let csrf = ctx.csrf_token();
    let (_, configured) = bots::market_data(&tx, &ctx.app)?;
    let after = Bot::find(&tx, owner, id, For::Page, ctx.locale)?.ok_or(WebError::Config("the bot disappeared".into()))?;
    let body = match action {
        Action::Delete => {
            let label = after.label.clone();
            flash::set(&ctx.session, flash::NOTICE, i18n::text(ctx.locale, "errors.bots.destroy_success", &[("bot_label", Arg::Text(&label))]));
            turbo::redirect(&ctx.path("/bots"))
        }
        Action::Unarchive => turbo::refresh().into(),
        Action::Archive => action_view::archived_streams(&tx, &ctx, &csrf, &after, configured, owner)? + "\n",
        Action::Start | Action::Stop => {
            let mut body = settings_stream(&ctx, &csrf, &after, base_url)? + "\n"
                + &turbo::stream("replace", "exchange_select", page::exchange_select(&tx, &ctx, &csrf, &after, owner)?.trim_end());
            if action == Action::Stop {
                match action_view::all_buttons(&tx, &ctx, &csrf, owner, configured)? { Some(buttons) => body += &buttons, None => body = turbo::refresh().into() }
            }
            body + "\n"
        }
    };
    tx.commit()?;
    if changed { ctx.app.wake_engine(); }
    for (stream, html) in written { ctx.app.hub.broadcast(&stream, &html); }
    Ok(Answer::Done(action_view::response(body, false)))
}

/// A signal bot's settings (`action` None) or lifecycle write from the page (bot::actions).
pub async fn web_action(app: App, ctx: Ctx, owner: i64, id: i64, action: Option<Action>, params: ActionParams, headers: &HeaderMap) -> Result<Response, WebError> {
    let (view, url) = (ctx.clone(), base_url(&app, headers));
    let answer = app.db(move |c| match action {
        Some(action) => web_lifecycle(c, &view, owner, id, action, &params, &url),
        None => settings_inner(c, &view, owner, id, &params, &url),
    }).await;
    finish(&ctx, answer)
}

/// BotApi's Start, Stop, Archive, Unarchive and Delete on a signal bot: its refusals, then the model's write. `heard`:
/// the owner's language, in which their open pages hear the change.
pub fn service(c: &Connection, ctx: &Ctx, heard: &Ctx, owner: i64, id: i64, action: Action, submitted: &ActionParams) -> Result<Outcome<Shown>, WebError> {
    let tx = if matches!(action, Action::Stop | Action::Archive | Action::Delete) { model::immediate_retry_busy(c)? } else { model::immediate(c)? };
    let row: Option<(String, i64)> = tx.query_row("SELECT COALESCE(label, ''), status FROM bots WHERE id = ?1 AND user_id = ?2 AND status <> 3 AND type = ?3", (id, owner, SIGNAL_TYPE),
                                                  |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some((label, status)) = row else { return Ok(Outcome::Missing) };
    let status = BotStatus::from_i64(status).ok_or(WebError::Config(format!("bot {id}: status {status}")))?;
    let name = crate::web::mcp::tools::STATUSES.get(status as usize).copied().unwrap_or("");
    let failed = |failure: Failure| Ok(Outcome::Invalid(Shown { data: Value::Null, text: failure.message.clone() }, failure));
    let working = matches!(status, BotStatus::Scheduled | BotStatus::Executing | BotStatus::Retrying | BotStatus::Waiting);
    let mut fresh = true;
    if action == Action::Start {
        // BotApi::Bots::Start.wash_sale_question_owed?: an unanswered question and a rule that sells (Bots::Signal#sell_capable?).
        let sells: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM bot_signals WHERE bot_id = ?1 AND direction = 1 AND enabled = 1)", [id], |r| r.get(0))?;
        if wash_sale(&tx, owner)?.is_none() && sells {
            return failed(Failure::invalid("wash_sale_question_owed", i18n::text(ctx.locale, "engine.wash_sale_question_owed", &[("label", Arg::Text(&label))])));
        }
        match submitted.start_fresh() { Ok(f) => fresh = f, Err(_) => return failed(Failure::invalid("invalid_start_fresh", i18n::text(ctx.locale, "engine.invalid_start_fresh", &[]))) }
    }
    let conflict = match action {
        Action::Stop if !working => Some(("bot_not_running", format!("Bot '{label}' is not running ({name})."))),
        Action::Start if working => Some(("bot_already_running", format!("Bot '{label}' is already running ({name})."))),
        Action::Archive if status == BotStatus::Archived => Some(("bot_archived", format!("Bot '{label}' is already archived."))),
        Action::Unarchive if status != BotStatus::Archived => Some(("bot_not_archived", format!("Bot '{label}' is not archived."))),
        _ => None,
    };
    if let Some((code, message)) = conflict { return failed(Failure::conflict(code, message)); }
    let (provider, _) = bots::market_data(&tx, &ctx.app)?;
    if let Some(reason) = super::refusal(&tx, id, wash_sale(&tx, owner)?, provider, For::Settings, ctx.app.signal_pages())? { return Ok(Outcome::Unported(reason)); }
    let Some(bot) = Bot::find(&tx, owner, id, For::Page, ctx.locale)? else { return Ok(Outcome::Missing) };
    let mut heard = heard.clone();
    heard.now = ctx.now;
    let broadcasts = match lifecycle_write(&tx, &heard, owner, &bot, action, fresh, true)? {
        Life::Invalid(errors) => return failed(write::model_failure(action, &label, &errors)),
        Life::Refused(reason) => return Ok(Outcome::GuardRefused(Shown { data: Value::Null, text: crate::web::bot_api::refused(&reason) })),
        Life::Written { broadcasts, .. } => broadcasts,
    };
    let text = match action {
        Action::Stop => format!("Bot '{label}' stopped.{}", if write::orders_may_execute(&tx, id)? { " Orders already sent to the exchange may still execute." } else { "" }),
        Action::Archive => format!("Bot '{label}' archived."),
        Action::Unarchive => format!("Bot '{label}' reactivated (stopped)."),
        Action::Delete => format!("Bot '{label}' deleted."),
        Action::Start => format!("Bot '{label}' started successfully."),
    };
    let status: usize = tx.query_row("SELECT status FROM bots WHERE id = ?1", [id], |r| r.get(0))?;
    let data = json!({"id": id, "label": label, "status": crate::web::mcp::tools::STATUSES.get(status).copied().unwrap_or("")});
    tx.commit()?;
    ctx.app.wake_engine();
    for (stream, html) in broadcasts { ctx.app.hub.broadcast(&stream, &html); }
    Ok(Outcome::Committed(Shown { data, text }))
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    /// A drawn token another rule holds is drawn again; each is 43 URL-safe characters (32 bytes, unpadded).
    #[test]
    fn a_taken_token_is_drawn_again() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE bot_signals (token TEXT)").unwrap();
        let taken = super::URL_SAFE_NO_PAD.encode([7u8; 32]);
        c.execute("INSERT INTO bot_signals VALUES (?1)", [&taken]).unwrap();
        let mut draws = vec![[9u8; 32], [7u8; 32]];
        let token = super::draw_token(&c, || draws.pop().unwrap()).unwrap();
        assert_eq!(token, super::URL_SAFE_NO_PAD.encode([9u8; 32]));
        assert!(draws.is_empty(), "the taken token was drawn first");
        assert_eq!(token.len(), 43);
    }
}
