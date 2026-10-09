//! Settings and lifecycle writes own their observation, response construction and post-commit delivery.
//! Call from App::db: cancelling its await must not cancel the committed write's wake.
use super::{action_params::ActionParams, composition, draft::{Draft, FieldError, ParseError, ValidationContext}, Bot, For, Kind};
use crate::{codec, engine::{eligibility, model}, ruby::BigDec};
use crate::web::format::Num;
use crate::web::{bots, i18n, layout::Ctx, WebError};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

pub enum Outcome<T> {
    Missing,
    Unported(&'static str),
    Invalid(T),
    GuardRefused(T),
    NoChange(T),
    Committed(T),
}

/// Fragments are built under the transaction and published only after commit succeeds.
pub struct Prepared<T> {
    pub response: T,
    pub broadcasts: Vec<(String, String)>,
}

const SET_SETTING: &str = "UPDATE bots
SET settings = json_set(settings, ?1, json(?2))
WHERE id = ?3 AND user_id = ?4 AND type = ?5 AND status <> 3;";
const SET_TRANSIENT: &str = "UPDATE bots
SET transient_data = json_set(transient_data, ?1, json(?2))
WHERE id = ?3 AND user_id = ?4 AND type = ?5 AND status <> 3;";
const REMOVE_TRANSIENT: &str = "UPDATE bots
SET transient_data = json_remove(transient_data, ?1)
WHERE id = ?2 AND user_id = ?3 AND type = ?4 AND status <> 3;";

/// Bot::Startable steps a passed start time forward in fixed UTC days; across a DST change that lands an hour BEFORE the
/// chosen local time, and the engine does not buy early (start::StartAt). Rails starts these.
const EARLY_START: &str = "this starting time crosses a daylight-saving change, where Rails would run the first order an hour before the chosen time";

fn error(message: &str) -> WebError { super::data(message.into()) }
fn one(rows: usize) -> Result<(), WebError> {
    if rows == 1 { Ok(()) } else { Err(error("settings writer lost its owned bot")) }
}

/// Only keys owned by the model/parser may form SQL paths. Unknown stored values are never
/// assigned back, even when a load supplied defaults for a different key.
fn path(key: &str, transient: bool) -> Result<String, WebError> {
    let allowed = if transient {
        matches!(key, "missed_quote_amount" | "missed_quote_amount_was_set")
            || ["quote_amount_limit", "base_amount_limit", "price_limit", "price_drop_limit", "moving_average_limit", "indicator_limit", "sell_price_limit", "sell_price_drop_limit", "sell_moving_average_limit", "sell_indicator_limit"]
                .iter().any(|prefix| key == format!("{prefix}_enabled_at") || key == format!("{prefix}_condition_met_at"))
    } else {
        super::SETTINGS.iter().any(|(name, _)| *name == key)
            || matches!(key, "quote_amount" | "quote_asset_id" | "interval" | "allocations" | "smart_interval_quote_amount" | "smart_interval_base_amount" | "limit_order_pcnt_distance" | "start_time_enabled" | "start_time_mode" | "rebalance_enabled" | "base_amount_limited" | "base_amount_limit" | "sell_interval" | "sell_amount" | "sell_quote_amount" | "sell_denomination" | "hold_all" | "index_name_prefix")
            || key.strip_prefix("sell_").is_some_and(|key| super::SETTINGS.iter().any(|(name, _)| *name == key))
    };
    if allowed { Ok(format!("$.{key}")) } else { Err(error("settings writer received a key outside its allowlist")) }
}

pub fn settings<T>(
    c: &Connection, ctx: &Ctx, owner: i64, id: i64, submitted: &ActionParams,
    response_builder: impl FnOnce(&Connection, &Ctx, &Draft) -> Result<Prepared<T>, WebError>,
) -> Result<Outcome<T>, WebError> {
    match settings_inner(c, ctx, owner, id, submitted, response_builder) {
        Err(e) if super::unreadable(&e) => Ok(Outcome::Unported("settings exceed the supported history, member or numeric bounds")),
        other => other,
    }
}

fn settings_inner<T>(
    c: &Connection, ctx: &Ctx, owner: i64, id: i64, submitted: &ActionParams,
    response_builder: impl FnOnce(&Connection, &Ctx, &Draft) -> Result<Prepared<T>, WebError>,
) -> Result<Outcome<T>, WebError> {
    let tx = model::immediate(c)?;
    let mut ctx = ctx.clone();
    ctx.now = ctx.app.now();
    let class: Option<String> = tx.query_row("SELECT type FROM bots WHERE id=?1 AND user_id=?2 AND status<>3 AND type IN ('Bots::DcaMultiAsset','Bots::DcaIndex')", (id,owner), |r| r.get(0)).optional()?;
    let Some(class) = class else { return Ok(Outcome::Missing) };
    let kind = if class == "Bots::DcaIndex" { Kind::Index } else { Kind::Basket };
    let fields = submitted.permitted(kind);
    let Some(mut draft) = Draft::load(&tx, owner, id, ctx.locale)? else { return Ok(Outcome::Missing) };
    let fields = match fields {
        Ok(fields) => fields,
        Err(_) => {
            draft.errors.push(FieldError { field: "settings".into(), message: "missing or invalid bot parameter root".into() });
            let prepared = response_builder(&tx, &ctx, &draft)?;
            tx.rollback()?;
            return Ok(Outcome::Invalid(prepared.response));
        }
    };
    let (zone, wash_sale): (String, Option<bool>) = tx.query_row("SELECT time_zone, wash_sale_enabled FROM users WHERE id = ?1", [owner], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let (provider, configured) = bots::market_data(&tx, &ctx.app)?;
    // Stored scope refusals survive. Only the pending lock's validation must precede its
    // page refusal, since a rejected composition still has a renderable narrow settings form.
    let original_refusal = if submitted.mcp() { None } else { super::refusal(&tx, id, wash_sale, provider, For::Settings)? };
    if let Some(reason) = original_refusal.filter(|reason| *reason != "a rebalance, liquidation or redeploy in progress") { return Ok(Outcome::Unported(reason)); }
    let mut mcp_carry = None;
    let mut mcp_refused = false;
    if submitted.mcp() {
        match super::mcp_input::apply(&tx, &mut draft, &fields)? {
            super::mcp_input::InputOutcome::Parsed => {},
            super::mcp_input::InputOutcome::AsciiAliasRefused => mcp_refused = true,
        }
        if draft.errors.is_empty() {
            // BotApi captures under the old settings before model validation; the save
            // callback later caps it or restores the original if the window did not move.
            let carry = match pending(&tx, &draft.original, ctx.now) {
                Ok(amount) => amount,
                Err(e) if super::unreadable(&e) => return Ok(Outcome::Unported("settings carry exceeds the supported history or numeric bounds")),
                Err(e) => return Err(e),
            };
            draft.candidate.transient.insert("missed_quote_amount".into(), serialized(carry.clone())?);
            mcp_carry = Some(carry);
            draft.validate(&tx, ValidationContext::Update, ctx.now, configured, ctx.locale)?;
        }
    } else { match draft.parse(&tx, &fields, &zone, ctx.now) {
        Err(ParseError::Database(e)) => return Err(e),
        Err(ParseError::Invalid(_)) => {},
        Ok(()) => draft.validate(&tx, ValidationContext::Update, ctx.now, configured, ctx.locale)?,
    }
    }
    mcp_refused |= draft.validate_schedule_bounds(ctx.locale, ctx.now)?;
    if !draft.errors.is_empty() {
        let prepared = response_builder(&tx, &ctx, &draft)?;
        tx.rollback()?;
        return Ok(if mcp_refused {Outcome::GuardRefused(prepared.response)} else {Outcome::Invalid(prepared.response)});
    }
    if let Some(reason) = original_refusal { return Ok(Outcome::Unported(reason)); }
    if !submitted.mcp() { if let Some(reason) = draft.original.unrendered() { return Ok(Outcome::Unported(reason)); } }
    let mut effects = draft.save_effects(&tx, ctx.now)?;
    // Callback rewrites (notably an exchange change) can change settings after the initial
    // dirty check. Defaults alone are compared to the load-time baseline, not the disk JSON.
    let mut final_settings = draft.candidate.settings.clone();
    final_settings.extend(effects.settings.set.clone());
    effects.settings_changed = !super::draft::equal(&json!(final_settings), &json!(draft.baseline));
    if effects.settings_changed {
        let old = match mcp_carry {
            Some(amount) => amount,
            None => match pending(&tx, &draft.original, ctx.now) {
            Ok(amount) => amount,
            Err(e) if super::unreadable(&e) => return Ok(Outcome::Unported("settings carry exceeds the supported history or numeric bounds")),
            Err(e) => return Err(e),
            },
        };
        let cap = effective_amount(&draft.candidate)?;
        let carry = minimum(old, cap)?;
        effects.transient.set.insert("missed_quote_amount".into(), serialized(carry)?);
        effects.transient.set.insert("missed_quote_amount_was_set".into(), Value::Null);
    }
    if submitted.mcp() && !effects.settings_changed {
        // BotApi explicitly captures before save. When the carry window does not move,
        // Accountable restores the old value (nil even when the key was absent).
        effects.transient.set.remove("missed_quote_amount");
        for (key,value) in [("missed_quote_amount",draft.raw_transient.get("missed_quote_amount").cloned().unwrap_or(Value::Null)),("missed_quote_amount_was_set",Value::Null)] {
            if draft.raw_transient.get(key) != Some(&value) { effects.transient.set.insert(key.into(),value); }
        }
    }
    let class = match draft.original.kind { Kind::Basket => "Bots::DcaMultiAsset", Kind::Index => "Bots::DcaIndex" };
    // A bot loaded without a name shows a generated one; submitting it on purpose saves it (Automation::Labelable#label=).
    let label_changed = draft.candidate.label != draft.original.label || (draft.original.label_unsaved && draft.submitted_label.is_some());
    let exchange_changed = draft.candidate.exchange.id != draft.original.exchange.id;
    let changed = label_changed || exchange_changed || !effects.settings.set.is_empty() || !effects.transient.set.is_empty() || !effects.transient.remove.is_empty();
    for (key, value) in &effects.settings.set { one(tx.execute(SET_SETTING, (path(key, false)?, if submitted.mcp() {super::mcp_input::encode(value)} else {value.to_string()}, id, owner, class))?)?; }
    for (key, value) in &effects.transient.set { one(tx.execute(SET_TRANSIENT, (path(key, true)?, if submitted.mcp() {super::mcp_input::encode(value)} else {value.to_string()}, id, owner, class))?)?; }
    for key in &effects.transient.remove { one(tx.execute(REMOVE_TRANSIENT, (path(key, true)?, id, owner, class))?)?; }
    if label_changed { one(tx.execute("UPDATE bots SET label = ?1 WHERE id = ?2 AND user_id = ?3 AND type = ?4 AND status <> 3", (&draft.candidate.label, id, owner, class))?)?; }
    if exchange_changed { one(tx.execute("UPDATE bots SET exchange_id = ?1 WHERE id = ?2 AND user_id = ?3 AND type = ?4 AND status <> 3", (draft.candidate.exchange.id, id, owner, class))?)?; }
    let at = codec::format_time(ctx.now);
    if effects.settings_changed { one(tx.execute("UPDATE bots SET settings_changed_at = ?1 WHERE id = ?2 AND user_id = ?3 AND type = ?4 AND status <> 3", (&at, id, owner, class))?)?; }
    if changed { one(tx.execute("UPDATE bots SET updated_at = ?1 WHERE id = ?2 AND user_id = ?3 AND type = ?4 AND status <> 3", (&at, id, owner, class))?)?; }
    if draft.candidate.kind == Kind::Basket && (effects.composition_changed || exchange_changed) {
        composition::reconcile(&tx, &draft.candidate, ctx.now)?;
    }
    if let Err(refusal) = eligibility::guard(&tx, &ctx.app.cipher, Some(id)) {
        let reason = refusal.reason();
        draft.errors.push(FieldError { field: "base".into(), message: i18n::text(ctx.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(&reason))]) });
        // Build while the same lock still owns the observation, then roll everything back.
        let response = response_builder(&tx, &ctx, &draft)?;
        tx.rollback()?;
        return Ok(Outcome::GuardRefused(response.response));
    }
    if !submitted.mcp() { if let Some(reason) = super::refusal(&tx, id, wash_sale, provider, For::Settings)? { return Ok(Outcome::Unported(reason)); } }
    // Rails keeps the name it showed in memory: a label the save did not write is not regenerated from the new settings.
    let shown = std::mem::take(&mut draft.candidate.label);
    draft.candidate = Bot::find(&tx, owner, id, For::Page, ctx.locale)?.ok_or_else(|| error("saved bot disappeared"))?;
    if draft.candidate.label_unsaved { draft.candidate.label = shown; }
    if !submitted.mcp() { if let Some(reason) = draft.candidate.unrendered() { return Ok(Outcome::Unported(reason)); } }
    let prepared = response_builder(&tx, &ctx, &draft)?;
    tx.commit()?;
    if !changed { return Ok(Outcome::NoChange(prepared.response)); }
    // This tail is deliberately in the App::db job, under its mutex. Nothing fallible may
    // follow commit; the HTTP awaiter may already have gone away.
    ctx.app.wake_engine();
    for (stream, html) in prepared.broadcasts { ctx.app.hub.broadcast(&stream, &html); }
    Ok(Outcome::Committed(prepared.response))
}

pub use crate::engine::accounting::web_pending as pending;
use crate::engine::accounting::{web_effective_amount as effective_amount, web_serialized as serialized, web_minimum as minimum};
/// A stored transient value against the one assigned, by Ruby's `==`: `assigned` is nil, a BigDecimal (written as its
/// text) or a settings number. A stored text is a String, never equal to a BigDecimal.
fn ruby_equal(stored: &Value, assigned: &Value) -> bool {
    let number = |v: &Value| match v {
        Value::String(s) => BigDec::parse(s).ok(),
        v => Num::from_json(v).and_then(|n| n.to_d()),
    };
    match (stored, assigned) {
        (Value::Null, Value::Null) => true,
        (Value::Number(_), Value::String(_) | Value::Number(_)) => number(stored).is_some() && number(stored) == number(assigned),
        _ => false,
    }
}

const WEB_START: &str = "UPDATE bots SET status = 1, stop_message_key = NULL, \
    started_at = CASE WHEN ?4 THEN ?5 ELSE started_at END, \
    transient_data = CASE WHEN ?4 THEN json_set(json_replace(transient_data, \
      '$.last_action_job_at', json('null')), '$.missed_quote_amount', json('null')) \
      ELSE transient_data END, updated_at = ?5 \
    WHERE id = ?1 AND user_id = ?2 AND type = ?3 AND status IN (0, 2)";

// Provisional spelling: MUST match the merged engine PR before execution.
const START_DECISION_KEY: &str = "rust_continue_start";

const WEB_REQUEST_START_DECISION: &str = "UPDATE bots SET \
    transient_data = json_set(transient_data, ?4, json(?5)) \
    WHERE id = ?1 AND user_id = ?2 AND type = ?3 AND status = 1";

const WEB_STOP: &str = "UPDATE bots SET status = 2, stopped_at = ?4, \
    stop_message_key = NULL, updated_at = ?4 \
    WHERE id = ?1 AND user_id = ?2 AND type = ?3 AND status <> 3";

const WEB_DELETE: &str = "UPDATE bots SET status = 3, updated_at = ?4 \
    WHERE id = ?1 AND user_id = ?2 AND type = ?3 AND status <> 3";

const WEB_UNARCHIVE: &str = "UPDATE bots SET status = 2, updated_at = ?4 \
    WHERE id = ?1 AND user_id = ?2 AND type = ?3 AND status = 7";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action { Start, Stop, Delete, Archive, Unarchive }

/// Safety actions can render a refresh/redirect even when the full page cannot be read.
pub struct LifecycleView {
    pub draft: Option<Draft>,
    pub errors: Vec<FieldError>,
}
impl LifecycleView {
    fn add(&mut self, locale: &str, key: &str) {
        self.errors.push(FieldError { field: "base".into(), message: i18n::text(locale, key, &[]) });
    }
}

/// Same transaction, guard and delivery boundary as settings. No network or queue work.
#[allow(clippy::too_many_arguments)]
pub fn lifecycle<T>(
    c: &Connection, ctx: &Ctx, owner: i64, id: i64, action: Action, submitted: &ActionParams,
    response_builder: impl FnOnce(&Connection, &Ctx, &LifecycleView) -> Result<Prepared<T>, WebError>,
) -> Result<Outcome<T>, WebError> {
    match lifecycle_inner(c, ctx, owner, id, action, submitted, response_builder) {
        Err(e) if super::unreadable(&e) => Ok(Outcome::Unported("lifecycle exceeds the supported history, member or numeric bounds")),
        other => other,
    }
}

#[allow(clippy::too_many_arguments)]
fn lifecycle_inner<T>(
    c: &Connection, ctx: &Ctx, owner: i64, id: i64, action: Action, submitted: &ActionParams,
    response_builder: impl FnOnce(&Connection, &Ctx, &LifecycleView) -> Result<Prepared<T>, WebError>,
) -> Result<Outcome<T>, WebError> {
    use crate::enums::{ApiKeyStatus, BotStatus};
    let tx = model::immediate(c)?;
    let mut ctx = ctx.clone();
    ctx.now = ctx.app.now();
    let row = tx.query_row("SELECT type,status,settings,transient_data FROM bots WHERE id=?1 AND user_id=?2 AND status<>3 AND type IN ('Bots::DcaMultiAsset','Bots::DcaIndex')",
        (id,owner), |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional()?;
    let Some((class,status,raw,transient)) = row else { return Ok(Outcome::Missing) };
    let mut view = LifecycleView { draft: None, errors: vec![] };
    if submitted.mcp() {
        let label: String = tx.query_row("SELECT COALESCE(label,'') FROM bots WHERE id=?1",[id],|r|r.get(0))?;
        let name = match status {0=>"created",1=>"scheduled",2=>"stopped",3=>"deleted",4=>"executing",5=>"retrying",6=>"waiting",7=>"archived",_=>return Err(error("unknown bot status"))};
        let working = matches!(status,1|4|5|6);
        let message = match action {
            Action::Stop if !working => Some(format!("Bot '{label}' is not running ({name}).")),
            Action::Start if working => Some(format!("Bot '{label}' is already running ({name}).")),
            Action::Archive if status==7 => Some(format!("Bot '{label}' is already archived.")),
            Action::Unarchive if status!=7 => Some(format!("Bot '{label}' is not archived.")),
            _ => None,
        };
        if let Some(message)=message {
            view.errors.push(FieldError {field:"base".into(),message});
            let prepared=response_builder(&tx,&ctx,&view)?;
            tx.rollback()?;
            return Ok(Outcome::Invalid(prepared.response));
        }
    }
    // A stale Reactivate never validates, fills defaults, or wakes a running bot.
    if action == Action::Unarchive && status != 7 || action == Action::Archive && status == 7 {
        let prepared = response_builder(&tx, &ctx, &view)?;
        tx.rollback()?;
        return Ok(Outcome::NoChange(prepared.response));
    }
    let safety = matches!(action, Action::Stop | Action::Delete | Action::Archive);
    // Never replace an unreadable JSON document with an empty object. The guard owns the
    // refusal for safety operations; other operations retain the page's bounded refusal.
    let structural = raw.len() <= 65_536 && transient.len() <= 65_536
        && super::object(&raw,"bots.settings").is_ok() && super::object(&transient,"bots.transient_data").is_ok();
    if !structural {
        if !safety { return Ok(Outcome::Unported("invalid or oversized stored bot JSON")); }
        view.errors.push(FieldError { field: "base".into(), message: i18n::text(ctx.locale,"engine.write_refused", &[("reason",i18n::Arg::Text("invalid or oversized stored bot JSON"))]) });
        let prepared=response_builder(&tx,&ctx,&view)?;
        tx.rollback()?;
        return Ok(Outcome::GuardRefused(prepared.response));
    }
    view.draft = match Draft::load(&tx,owner,id,ctx.locale) {
        Ok(draft) => draft,
        Err(WebError::Engine(crate::engine::EngineError::Data(_))) if safety => None,
        Err(e) if safety && super::unreadable(&e) => None,
        Err(e) => return Err(e),
    };
    let mut fresh = true;
    let mut delayed = None;
    let mut writer_refused = false;
    if action == Action::Start {
        // Execute the final SQL gate even for a working row; it must touch zero rows.
        if matches!(status,1|4|5|6) {
            let rows=tx.execute(WEB_START,(id,owner,&class,true,codec::format_time(ctx.now)))?;
            if rows != 0 { return Err(error("working Start passed its SQL gate")); }
            view.add(ctx.locale,"engine.already_running");
        } else {
            if submitted.mcp() { fresh=status==0; } else { match submitted.start_fresh() { Ok(value) => fresh=value, Err(_) => view.add(ctx.locale,"engine.invalid_start_fresh") } }
        }
    }
    if view.errors.is_empty() && (!safety || submitted.mcp() && action != Action::Stop) {
        let (provider,configured)=bots::market_data(&tx,&ctx.app)?;
        let wash:Option<bool>=tx.query_row("SELECT wash_sale_enabled FROM users WHERE id=?1",[owner],|r|r.get(0))?;
        let refusal=if submitted.mcp() {None} else {super::refusal(&tx,id,wash,provider,For::Settings)?};
        let draft=view.draft.as_mut().ok_or_else(||error("owned lifecycle draft disappeared"))?;
        let refusal=if submitted.mcp() {None} else {refusal.or_else(||draft.original.unrendered())};
        if action==Action::Start {
            if let Some(reason)=refusal { return Ok(Outcome::Unported(reason)); }
        }
        draft.candidate.status=match action {
            Action::Start=>BotStatus::Scheduled, Action::Archive=>BotStatus::Archived,
            Action::Delete=>BotStatus::Deleted, Action::Unarchive|Action::Stop=>BotStatus::Stopped,
        };
        if action==Action::Start && fresh {
            // Lifecycle resets the candidate before valid?(:start), including an old
            // negative carry. Keep the original intact for the guarded save callbacks.
            draft.candidate.started_at=Some(ctx.now);
            if draft.candidate.transient.contains_key("last_action_job_at") {
                draft.candidate.transient.insert("last_action_job_at".into(),Value::Null);
            }
            draft.candidate.transient.insert("missed_quote_amount".into(),Value::Null);
        }
        draft.validate(&tx,if action==Action::Start {ValidationContext::Start}else{ValidationContext::Update},ctx.now,configured,ctx.locale)?;
        if action == Action::Start { writer_refused |= draft.validate_schedule_bounds(ctx.locale, ctx.now)?; }
        view.errors.extend(draft.errors.clone());
        if action==Action::Unarchive && view.errors.is_empty() {
            if let Some(reason)=refusal { return Ok(Outcome::Unported(reason)); }
        }
        if action==Action::Start && view.errors.is_empty() {
            if draft.candidate.api_key != Some(ApiKeyStatus::Correct as i64) {
                writer_refused = true;
                let reason=i18n::text(ctx.locale,"engine.api_key_not_ready",&[]);
                view.errors.push(FieldError {field:"base".into(),message:i18n::text(ctx.locale,"engine.write_refused",&[("reason",i18n::Arg::Text(&reason))])});
            } else if fresh {
                let zone:String=tx.query_row("SELECT time_zone FROM users WHERE id=?1",[owner],|r|r.get(0))?;
                // Lifecycle#start: `computed_start_at&.future?`.
                let start=draft.initial_start_at(ctx.now,&zone)?.filter(|s|s.at>ctx.now);
                if start.is_some_and(|s|s.early) {
                    writer_refused = true;
                    view.errors.push(FieldError {field:"base".into(),message:i18n::text(ctx.locale,"engine.write_refused",&[("reason",i18n::Arg::Text(EARLY_START))])});
                }
                delayed=start.map(|s|s.at);
            }
        }
    }
    if !view.errors.is_empty() {
        if let Some(draft)=view.draft.as_mut() { draft.errors=view.errors.clone(); }
        let prepared=response_builder(&tx,&ctx,&view)?;
        tx.rollback()?;
        return Ok(if writer_refused {Outcome::GuardRefused(prepared.response)} else {Outcome::Invalid(prepared.response)});
    }
    let at=codec::format_time(ctx.now);
    if let Some(draft)=view.draft.as_mut() {
        if let Some(future)=delayed {
            draft.candidate.settings.insert("start_at".into(),json!(future.to_rfc3339_opts(chrono::SecondsFormat::Secs,true)));
            draft.candidate.started_at=Some(future);
            // A store_accessor writer does not materialize an absent key when assigned nil.
            if draft.raw_transient.contains_key("last_action_job_at") { draft.candidate.transient.insert("last_action_job_at".into(),Value::Null); }
            draft.candidate.transient.insert("missed_quote_amount".into(),Value::Null);
        }
        let mut effects=draft.save_effects(&tx,ctx.now)?;
        if submitted.mcp() && effects.settings_changed && delayed.is_none() {
            let carry=pending(&tx,&draft.original,ctx.now)?;
            effects.transient.set.insert("missed_quote_amount".into(),serialized(minimum(carry,effective_amount(&draft.candidate)?)?)?);
            effects.transient.set.insert("missed_quote_amount_was_set".into(),Value::Null);
        }
        if submitted.mcp() && !effects.settings_changed &&
            (action==Action::Stop || action==Action::Start && !fresh || !effects.settings.set.is_empty()) {
            effects.transient.set.insert("missed_quote_amount".into(),draft.raw_transient.get("missed_quote_amount").cloned().unwrap_or(Value::Null));
            effects.transient.set.insert("missed_quote_amount_was_set".into(),Value::Null);
        }
        if delayed.is_some() {
            // Lifecycle#start assigns last_action_job_at and the carry nil, then captures after assigning the future
            // anchor. Accountable lands the capture (`[missed_quote_amount.to_d, effective_quote_amount].min`) only when
            // the settings changed (a new start_at) and restores the nil otherwise; ActiveRecord then writes
            // transient_data only when the hash differs, by Ruby's ==, from the stored one.
            let missed=if effects.settings_changed {
                let carry=pending(&tx,&draft.candidate,ctx.now)?.to_d().ok_or_else(||error("carry is not finite"))?;
                serialized(minimum(Num::Dec(carry),effective_amount(&draft.candidate)?)?)?
            } else {Value::Null};
            // A store_accessor writer does not materialize an absent key when assigned nil.
            let mut start=vec![("missed_quote_amount",missed),("missed_quote_amount_was_set",Value::Null)];
            if draft.raw_transient.contains_key("last_action_job_at") { start.push(("last_action_job_at",Value::Null)); }
            let own=|key:&String| key=="last_action_job_at" || start.iter().any(|(k,_)| k==key);
            let same=effects.transient.set.keys().all(own) && effects.transient.remove.is_empty()
                && start.iter().all(|(k,v)| draft.raw_transient.get(*k).is_some_and(|old| ruby_equal(old,v)));
            if same { effects.transient.set.retain(|k,_| !own(k)); } else { for (k,v) in start { effects.transient.set.insert(k.into(),v); } }
        }
        for (key,value) in &effects.settings.set { one(tx.execute(SET_SETTING,(path(key,false)?,if submitted.mcp() {super::mcp_input::encode(value)} else {value.to_string()},id,owner,&class))?)?; }
        for (key,value) in &effects.transient.set { one(tx.execute(SET_TRANSIENT,(if key=="last_action_job_at" {"$.last_action_job_at".into()} else {path(key,true)?},if submitted.mcp() {super::mcp_input::encode(value)} else {value.to_string()},id,owner,&class))?)?; }
        for key in &effects.transient.remove { one(tx.execute(REMOVE_TRANSIENT,(path(key,true)?,id,owner,&class))?)?; }
        if effects.settings_changed { one(tx.execute("UPDATE bots SET settings_changed_at=?4 WHERE id=?1 AND user_id=?2 AND type=?3",(id,owner,&class,&at))?)?; }
    } else {
        // These defaults need no ticker, interval, amount, exchange or history read.
        let settings=super::object(&raw,"bots.settings")?;
        for (key,value) in [("smart_intervaled",json!(false)),("limit_ordered",json!(false)),("limit_order_pcnt_distance",json!(0.001))] {
            if matches!(settings.get(key),None|Some(Value::Null|Value::Bool(false))) {
                one(tx.execute(SET_SETTING,(path(key,false)?,if submitted.mcp() {super::mcp_input::encode(&value)} else {value.to_string()},id,owner,&class))?)?;
            }
        }
    }
    match action {
        Action::Start => {
            one(tx.execute(WEB_START,(id,owner,&class,fresh && delayed.is_none(),&at))?)?;
            if submitted.mcp() && fresh && delayed.is_none() {
                one(tx.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.missed_quote_amount_was_set',json('true')) WHERE id=?1",[id])?)?;
            }
            if let Some(future)=delayed { one(tx.execute("UPDATE bots SET started_at=?4 WHERE id=?1 AND user_id=?2 AND type=?3 AND status=1",(id,owner,&class,codec::format_time(future)))?)?; }
            if !fresh {
                // PR #457 accepted requested_at; preserve the original status too, so a
                // created bot with a historical stamp cannot inherit stopped-only scheduling.
                let request=json!({"requested_at":ctx.now.to_rfc3339_opts(chrono::SecondsFormat::AutoSi,true),"was_stopped":status==2});
                one(tx.execute(WEB_REQUEST_START_DECISION,(id,owner,&class,format!("$.{START_DECISION_KEY}"),request.to_string()))?)?;
            }
        }
        Action::Stop | Action::Archive => {
            one(tx.execute(WEB_STOP,(id,owner,&class,&at))?)?;
            if action==Action::Archive { one(tx.execute("UPDATE bots SET status=7 WHERE id=?1 AND user_id=?2 AND type=?3 AND status=2",(id,owner,&class))?)?; }
        }
        Action::Delete => {
            one(tx.execute(WEB_DELETE,(id,owner,&class,&at))?)?;
            if submitted.mcp() { one(tx.execute("UPDATE bots SET stopped_at=?2 WHERE id=?1",(id,&at))?)?; }
        },
        Action::Unarchive => one(tx.execute(WEB_UNARCHIVE,(id,owner,&class,&at))?)?,
    }
    let guarded = match eligibility::guard(&tx,&ctx.app.cipher,Some(id)) {
        // STOP only reduces activity and writes no money, settings or history, so it always persists, whatever data
        // damage the install holds; check and every ordinary writer still refuse that damage.
        Err(eligibility::Refusal::Unreadable(_)) if action == Action::Stop => Ok(()),
        result => result,
    };
    if let Err(refusal)=guarded {
        let reason=refusal.reason();
        view.errors.push(FieldError {field:"base".into(),message:i18n::text(ctx.locale,"engine.write_refused",&[("reason",i18n::Arg::Text(&reason))])});
        if let Some(draft)=view.draft.as_mut() { draft.errors=view.errors.clone(); }
        let prepared=response_builder(&tx,&ctx,&view)?;
        tx.rollback()?;
        return Ok(Outcome::GuardRefused(prepared.response));
    }
    if matches!(action,Action::Start|Action::Stop|Action::Archive) {
        let (event,details)=if action==Action::Start {("started",json!({"start_fresh":fresh}))}else{("stopped",json!({}))};
        one(tx.execute("INSERT INTO bot_activity_logs (bot_id,event,level,message,details,created_at) VALUES (?1,?2,0,NULL,?3,?4)",(id,event,details.to_string(),&at))?)?;
    }
    if let Some(draft)=view.draft.as_mut() {
        let shown=std::mem::take(&mut draft.candidate.label);
        draft.candidate=Bot::find(&tx,owner,id,For::Page,ctx.locale)?.ok_or_else(||error("lifecycle bot disappeared"))?;
        if draft.candidate.label_unsaved { draft.candidate.label=shown; }
        if safety {
            let wash:Option<bool>=tx.query_row("SELECT wash_sale_enabled FROM users WHERE id=?1",[owner],|r|r.get(0))?;
            let (provider,_)=bots::market_data(&tx,&ctx.app)?;
            if draft.candidate.unrendered().is_some() || super::refusal(&tx,id,wash,provider,For::Page)?.is_some() { view.draft=None; }
        }
    }
    let prepared=response_builder(&tx,&ctx,&view)?;
    tx.commit()?;
    ctx.app.wake_engine();
    for (stream,html) in prepared.broadcasts { ctx.app.hub.broadcast(&stream,&html); }
    Ok(Outcome::Committed(prepared.response))
}
