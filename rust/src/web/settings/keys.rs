//! Credential saves validate outside the write lock, recheck ownership/state inside it, and guard before commit.
use crate::web::wizard::Picked;
use crate::{
    codec::format_time,
    engine::eligibility,
    web::{auth, flash, i18n, layout::{self,Ctx}, turbo, App, WebError},
};
use askama::Template;
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use crate::identity::Account;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::sync::Arc;
use subtle::ConstantTimeEq;
pub trait Logger: Send + Sync {
    fn warn(&self, line: &str);
}
struct Stderr;
impl Logger for Stderr {
    fn warn(&self, line: &str) {
        crate::eline!("{line}");
    }
}
pub fn logger() -> Arc<dyn Logger> {
    Arc::new(Stderr)
}
pub fn scrub(text: &str, values: &[Option<String>]) -> String {
    let values:Vec<_>=values.iter().flatten().map(String::as_str).collect();
    crate::crypto::scrub_known(text,&values)
}
/// K: no new tracking. Read the intent and the engine's actual polling rows under the write lock.
pub const SETTLING: &str = "An order is still being settled. Please try again shortly.";
/// #710 7a: an identity venue's account read failed or was masked, and the key could land on the wrong account.
pub const UNCONFIRMED: &str = "\u{0}identity_unconfirmed";
/// #710 7a rule Q1: the save would make the exchange's second account while another key of it has no account read yet;
/// `other_key_checking(name)` names the exchange.
const OTHER_KEY_CHECKING: &str = "\u{0}other_key_checking\u{1}";
fn other_key_checking(exchange: &str) -> String { format!("{OTHER_KEY_CHECKING}{exchange}") }
/// Rules Q1/P1: the keys a save must wait for: on an identity venue with a live account, every key of the exchange with no
/// UID read of its current material (`identity::unread`). A save routed to a new account waits for them; on an exchange
/// with two or more accounts every save does (it may replace or route against one). Inside the save's transaction too.
fn needs_reads(c: &Connection, user: i64, exchange: i64, venue: &str) -> Result<Vec<i64>, WebError> {
    if !crate::identity::VENUES.contains(&venue) || crate::identity::accounts_of(c, user, exchange)?.is_empty() { return Ok(vec![]); }
    let ids: Vec<i64> = c.prepare("SELECT id FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 ORDER BY id")?.query_map([user, exchange], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut out = vec![];
    for id in ids { if crate::identity::unread(c, id).map_err(WebError::Config)? { out.push(id); } }
    Ok(out)
}
/// Review r1 #1: whether a save routed `route` replaces, in its slot of type `kind`, a key of other material than `material`
/// that has no UID read of its current material (`identity::unread`): that key is read first, so a key of another exchange
/// account never takes its slot and its history, whatever the account count.
/// Bybit review r1 #1: any key of the destination account, not only the slot's: an empty read-only slot beside an unread
/// trading key reads that key first too.
fn replaces_unread(c: &Connection, app: &App, route: crate::identity::Route, _kind: i64, venue: &str, material: &crate::crypto::Credentials) -> Result<bool, WebError> {
    let crate::identity::Route::To(account) = route else { return Ok(false) };
    crate::identity::holds_unread_other(c, &app.cipher, account, venue, material).map_err(WebError::Config)
}
/// Whether the exchange has two or more live accounts (rule Q1: every save there reads the unread keys first).
fn several(c: &Connection, user: i64, exchange: i64) -> Result<bool, WebError> { Ok(crate::identity::accounts_of(c, user, exchange)?.len() >= 2) }
/// Rule Q1: before a save makes an exchange's second account, every key of it without a UID read of its current material
/// is read now (bounded by the key check's own timeout), each recorded as discovery records it (`identity::publish`:
/// fenced where it reads another account, routed nowhere else). `would_create` asks whether the save routes to a new
/// account or replaces a key of another material nothing has read yet (`replaces_unread`, review r1 #1, whatever the
/// account count);
/// account; on an exchange with two or more accounts every save reads them. A read that fails refuses the save (Q1's
/// words); nothing is allocated.
async fn read_unread(app: &App, user: i64, exchange: i64, venue: &str, name: &str, would_create: impl FnOnce(&Connection) -> Result<bool, WebError> + Send + 'static) -> Result<Option<String>, WebError> {
    let (inner, venue_s) = (app.clone(), venue.to_string());
    let due = app.db(move |c| {
        let ids = needs_reads(c, user, exchange, &venue_s)?;
        if ids.is_empty() || (!several(c, user, exchange)? && !would_create(c)?) { return Ok(vec![]); }
        let mut out = vec![];
        for id in ids {
            let (credentials, version, _) = crate::sync::validation_material(c, &inner.cipher, id).map_err(|e| WebError::Config(e.0))?;
            out.push(crate::identity::Due { api_key_id: id, exchange_type: venue_s.clone(), credentials, version });
        }
        Ok(out)
    }).await?;
    for due in due {
        let observed = json_identity(app, venue, &due.credentials).await?;
        let read = matches!(observed, Some(crate::identity::Observed::Read(_)));
        if let Some(observed) = observed {
            let at = app.now();
            let inner = app.clone();
            // Rules K5/M4: each committed read wakes the reader it moved, whatever a later read of this loop comes to.
            let moved = app.db(move |c| Ok((|| {
                let before = crate::identity::generations_of_key(c, due.api_key_id)?;
                crate::identity::publish(c, &inner.cipher, &due, &observed, at)?;
                crate::identity::moved_readers(c, due.api_key_id, &before)
            })())).await?.map_err(WebError::Config)?;
            if let Ok(jobs) = app.job_wakers() {
                for k in moved { for job in [crate::sync::jobs::LEDGER_SYNC, crate::sync::jobs::BALANCE_SYNC] { jobs.wake(job, Some(&k.to_string()), None); } }
            }
        }
        if !read { return Ok(Some(other_key_checking(name))); }
    }
    Ok(None)
}
/// #710 7a rule M2: a save that would make a new account for an exchange account another live account already holds
/// history of (a key or file observation of it), or a Reconnect whose account is another: asked first. The reason
/// carries the question (`question_html`); `key_answer` posts the answer back with the key.
const QUESTION: &str = "\u{0}key_question\u{1}";
/// Cancel on that question: nothing saved, the question goes.
const UNASKED: &str = "\u{0}key_unasked";
/// What `key_question` came to: a new account, the account the answer named instead, or the question.
enum KeyAsk { New, To(i64), Ask(String) }
/// Rule M2 (and N4's discipline): in the save's write transaction, for a save routed to a new account with read `read`.
/// Each answer is bound to the key (its material and slot type), its destination's slots and what was seen; it is
/// consumed in this transaction; any difference asks again.
#[allow(clippy::too_many_arguments)]
fn key_question(c: &Connection, cipher: &crate::crypto::Cipher, user: i64, exchange: i64, kind: i64, target: Option<i64>, read: &crate::identity::AccountId,
                material: &crate::crypto::Credentials, answer: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> Result<KeyAsk, WebError> {
    use crate::tracker::import::run::{self, Dest};
    let seen = crate::identity::import_evidence(c, cipher, user, exchange, target, read, now).map_err(WebError::Config)?;
    // An answer naming an account ("connect to account N instead") whose evidence went is asked again, never a new account.
    if seen.holders.is_empty() && !seen.foreign && !answer.is_some_and(|a| a.starts_with('a')) { return Ok(KeyAsk::New); }
    let subject = format!("{kind}:{}", crate::identity::existing_key(c, cipher).map(|k| k.material_digest(material)).unwrap_or_default());
    let sealed = |dest: Dest| -> Result<String, WebError> { Ok(run::seal('w', &subject, &run::sealed_dest(c, dest).map_err(|e| WebError::Config(e.0))?, &run::slot(c, user, dest).map_err(|e| WebError::Config(e.0))?, &seen.facts)) };
    let new = sealed(Dest::New(exchange))?;
    let consume = |d: &str| run::consume(c, d, now).map_err(|e| WebError::Config(e.0));
    if answer == Some(format!("n!w{new}").as_str()) && consume(&new)? { return Ok(KeyAsk::New); }
    let mut choices = vec![];
    for h in &seen.holders {
        let d = sealed(Dest::Account(*h))?;
        if answer == Some(format!("a{h}!w{d}").as_str()) && consume(&d)? { return Ok(KeyAsk::To(*h)); }
        choices.push(serde_json::json!({"value": format!("a{h}!w{d}"), "account": crate::identity::label(c, *h)?.plain()}));
    }
    choices.push(serde_json::json!({"value": format!("n!w{new}"), "account": null}));
    let name: String = c.query_row("SELECT coalesce(name,'') FROM exchanges WHERE id=?1", [exchange], |r| r.get(0))?;
    let held = seen.holders.first().map(|h| crate::identity::label(c, *h).map(|l| l.plain())).transpose()?;
    let other = target.filter(|_| seen.foreign).map(|t| crate::identity::label(c, t).map(|l| l.plain())).transpose()?;
    Ok(KeyAsk::Ask(format!("{QUESTION}{}", serde_json::json!({"changed": answer.is_some(), "exchange": name, "held": held, "other": other, "choices": choices}))))
}
/// The question's lines and answers, plain text (the JSON save's), or None for any other reason.
/// The question's lines, and its answers as (value, label).
type Asked = (Vec<String>, Vec<(String, String)>);
pub(crate) fn question_of(ctx: &Ctx, reason: &str) -> Option<Asked> {
    let q: serde_json::Value = serde_json::from_str(reason.strip_prefix(QUESTION)?).ok()?;
    let text = |key: &str, args: &[(&str, i18n::Arg)]| i18n::text(ctx.locale, &format!("settings.accounts.{key}"), args);
    let exchange = q["exchange"].as_str().unwrap_or_default();
    let mut lines = vec![];
    if q["changed"].as_bool() == Some(true) { lines.push(text("key_changed", &[])); }
    if let Some(a) = q["held"].as_str() { lines.push(text("key_held", &[("account", i18n::Arg::Text(a)), ("exchange", i18n::Arg::Text(exchange))])); }
    if let Some(a) = q["other"].as_str() { lines.push(text("key_other", &[("account", i18n::Arg::Text(a)), ("exchange", i18n::Arg::Text(exchange))])); }
    let choices = q["choices"].as_array()?.iter().map(|c| (c["value"].as_str().unwrap_or_default().to_string(), match c["account"].as_str() {
        Some(a) => text("connect_instead", &[("account", i18n::Arg::Text(a))]),
        None => text("add_anyway", &[]),
    })).collect();
    Some((lines, choices))
}
/// A key form's answer to the save's question (rule M2): in place of the form's question slot, so the typed key stays;
/// None for any other reason.
pub(crate) fn question_response(ctx: &Ctx, reason: &str) -> Option<Response> {
    let html = if reason == UNASKED { String::new() } else {
        let (lines, choices) = question_of(ctx, reason)?;
        let mut html: String = lines.iter().map(|l| format!("<p class=\"sform__error\" role=\"alert\">{}</p>", i18n::escape(l))).collect();
        html.push_str("<div class=\"set-api__actions\">");
        for (value, label) in choices.iter().chain(std::iter::once(&("x".to_string(), i18n::text(ctx.locale, "settings.accounts.key_cancel", &[])))) {
            html.push_str(&format!("<button type=\"submit\" name=\"key_answer\" value=\"{}\" class=\"rbutton rbutton--secondary\" data-turbo-stream=\"true\">{}</button>", i18n::escape(value), i18n::escape(label)));
        }
        html.push_str("</div>");
        html
    };
    Some((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], format!("{}\n", turbo::stream("update", "api_key_question", &html))).into_response())
}

/// The JSON save of an account's own key again, whose fresh read says it opens another account (#710 7a).
pub const KEY_READS_ELSEWHERE: &str = "\u{0}key_reads_elsewhere";
/// D-KEY-REPLACE-WAITS: an open order or a withdrawal still goes through the key being replaced or removed.
pub const ORDER_WAITS: &str = "\u{0}key_order_waits";
pub const WITHDRAWAL_WAITS: &str = "\u{0}key_withdrawal_waits";
/// The refusal while the account has anything in flight, as Merge asks it (`merge::in_flight`, #710 7a §1.1): its key is
/// neither replaced nor removed meanwhile, so no settlement or cancel runs through another key.
pub(crate) fn settling(c: &Connection, cipher: &crate::crypto::Cipher, account: &Account) -> Result<Option<&'static str>, WebError> {
    use crate::merge::Waiting;
    let Some(row) = crate::identity::AccountRow::of(c, account)? else { return Ok(None) };
    let waiting = crate::merge::in_flight(c, cipher, account.user_id, &[row]).map_err(|e| match e { crate::merge::Refusal::Db(e) => e.into(), other => WebError::Config(format!("{other:?}")) })?;
    Ok(waiting.map(|w| match w { Waiting::Settling => SETTLING, Waiting::Order => ORDER_WAITS, Waiting::Withdrawal => WITHDRAWAL_WAITS }))
}
pub(crate) fn refusal_text(ctx: &Ctx, reason: &str) -> String {
    if reason == SETTLING { reason.to_string() }
    else if let Some((name, exchange)) = reason.strip_prefix('\u{0}').and_then(|r| r.split_once('\u{1}')) {
        i18n::t(ctx.locale, &format!("settings.accounts.{name}"), &[("exchange", i18n::Arg::Text(exchange))])
    }
    else if let Some(name) = reason.strip_prefix('\u{0}') { ctx.t(&format!("settings.accounts.{name}")) }
    else { i18n::text(ctx.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(reason))]) }
}
#[derive(Clone)]
pub(crate) struct Key {
    pub(crate) id: Option<i64>,
    pub(crate) key: Option<String>,
    pub(crate) secret: Option<String>,
    pub(crate) passphrase: Option<String>,
    pub(crate) status: i64,
    last_error: Option<String>,
    updated_at: Option<String>,
    extras: [Option<String>;4],
    realm: Option<String>,
    version: Option<crate::engine::model::CredentialVersion>,
    /// The account it was read from, where a save of it goes (#710).
    pub(crate) account: Account,
    /// #710 7a: a venue tile's save, which names no account: `account` is only the exchange's, and the save goes where
    /// `identity::route_key` routes it, from a blank candidate.
    pub(crate) untargeted: bool,
}
impl Key {
    /// A venue tile's candidate (#710 7a): blank, inheriting nothing from a stored key, saved where §1 routes it.
    pub(crate) fn untargeted(account: Account) -> Key {
        Key { id: None, key: None, secret: None, passphrase: None, status: 0, last_error: None, updated_at: None, extras: [None, None, None, None],
              realm: None, version: None, account, untargeted: true }
    }
}
impl Key {
    pub(crate) fn read(
        c: &Connection,
        app: &App,
        account: Account,
        kind: i64,
    ) -> Result<Self, WebError> {
        if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=Self::read(&tx,app,account,kind)?;tx.commit()?;return Ok(out)}
        let row=c.query_row(&format!("SELECT id,key,secret,passphrase,status,last_sync_error,updated_at,access_token,rsa_signature_key,rsa_encryption_key,dh_param,ibkr_realm FROM api_keys k WHERE {} AND key_type=?1 ORDER BY id LIMIT 1",account.keys("k")),[kind],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,i64>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,String>(6)?,[r.get::<_,Option<String>>(7)?,r.get(8)?,r.get(9)?,r.get(10)?],r.get::<_,Option<String>>(11)?))).optional()?;
        let decrypt = |text: Option<String>| {
            text.map(|s| {
                app.cipher
                    .decrypt(&s)
                    .map_err(|_| WebError::Config("unreadable exchange credential".into()))
            })
            .transpose()
        };
        match row {
            Some((id, key, secret, passphrase, status, last_error, updated_at, [a,b,extra_c,d], realm)) => Ok(Self {
                id: Some(id),
                key: decrypt(key)?,
                secret: decrypt(secret)?,
                passphrase: decrypt(passphrase)?,
                status,
                last_error,
                updated_at: Some(updated_at),
                extras:[decrypt(a)?,decrypt(b)?,decrypt(extra_c)?,decrypt(d)?],realm,
                version:crate::engine::model::credential_version_by_id(c,id)?,
                account,untargeted:false,
            }),
            None => Ok(Self {
                id: None,
                key: None,
                secret: None,
                passphrase: None,
                status: 0,
                last_error: None,
                updated_at: None,
                extras: [None,None,None,None],realm:None,version:None,account,untargeted:false,
            }),
        }
    }
    /// Settings::IbkrConnectionsController#activate's credentials over the stored key: the consumer key, the access token and
    /// its secret as submitted, the stored RSA keys and DH params re-passed, and a blank realm as `limited_poa`.
    pub(crate) fn with_ibkr_credentials(&self, consumer_key: &str, access_token: &str, secret: &str) -> Self {
        let mut candidate = self.clone();
        candidate.key = Some(consumer_key.into());
        candidate.secret = Some(secret.into());
        candidate.extras[0] = Some(access_token.into());
        if candidate.realm.as_deref().is_none_or(crate::v2::blank) { candidate.realm = Some(crate::venue::signers::ibkr::DEFAULT_REALM.into()); }
        candidate
    }
    fn same(&self, other: &Self) -> bool {
        self.id == other.id
            && self.version == other.version
            && same(&self.key, &other.key)
            && same(&self.secret, &other.secret)
            && same(&self.passphrase, &other.passphrase)
            && self.extras.iter().zip(&other.extras).all(|(a,b)|same(a,b)) && self.realm==other.realm
    }
}
fn same(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => bool::from(a.as_bytes().ct_eq(b.as_bytes())),
        (None, None) => true,
        _ => false,
    }
}
use super::validator::Validity;
/// The prepared envelopes are the stored producer field for a newly validated slot.
/// The pending zero identity never enters any persisted account cache.
struct ValidatedKey {
    key:Key,
    columns:[Option<String>;7],
    handle:super::validator::ValidationHandle,
    result:crate::engine::model::Produced<Validity>,
    /// The key's account-identity read (crate::identity), recorded with the save under the same fence.
    identity:Option<crate::identity::Observed>,
}
async fn validate(app:&App,key:&Key,original:&Key,kind:i64,venue:&str)->Result<ValidatedKey,WebError>{
    let mut values=key.extras.iter().flatten().cloned().collect::<Vec<_>>();
    values.extend([original.key.as_ref(),original.secret.as_ref(),original.passphrase.as_ref()].into_iter().flatten().chain(original.extras.iter().flatten()).cloned());
    let columns=[key.key.as_ref(),key.secret.as_ref(),key.passphrase.as_ref(),key.extras[0].as_ref(),key.extras[1].as_ref(),key.extras[2].as_ref(),key.extras[3].as_ref()].map(|value|value.map(|value|app.cipher.encrypt(value)));
    // #710 (PLAN-710-6 §6.4): the form's account, so a save validated while Merge moved or removed it is refused at its commit.
    let producer=crate::engine::model::prepared_credential_version(key.id.unwrap_or(0),&columns).with_account(original.account.id);
    // Session#initialize's reads, from the submitted fields over the stored ones.
    let ibkr=match (venue==crate::venue::ibkr::IBKR,&key.extras) {
        (true,[Some(access_token),Some(signature_key),Some(encryption_key),Some(dh_param)])=>Some(Box::new(crate::venue::signers::ibkr::Material{
            consumer_key:key.key.clone().unwrap_or_default(),access_token:access_token.clone(),access_token_secret:key.secret.clone().unwrap_or_default(),
            signature_key:signature_key.clone(),encryption_key:encryption_key.clone(),dh_param:dh_param.clone(),realm:key.realm.clone().unwrap_or_default()})),
        _=>None,
    };
    let handle=super::validator::ValidationHandle::stored(crate::crypto::Credentials{redaction_values:values,key:key.key.clone().unwrap_or_default(),secret:key.secret.clone().unwrap_or_default(),passphrase:key.passphrase.clone(),ibkr},producer);
    let result=handle.check_on(app,venue,kind).await?;
    let (result,identity)=result;
    app.observe_validation(result.origin().clone())?;
    Ok(ValidatedKey{key:key.clone(),columns,handle,result,identity})
}
async fn tracker_exchange(app:&App,ctx:&Ctx)->Result<Option<i64>,WebError>{
    let requested=ctx.params.form("exchange_id").or_else(||ctx.params.query("exchange_id")).and_then(|s|s.parse::<i64>().ok());
    let saved=ctx.session.lock().tracker_connect;
    let found=app.db(move|c|{
        for id in [requested,saved].into_iter().flatten(){
            if c.query_row("SELECT EXISTS(SELECT 1 FROM exchanges WHERE id=?1)",[id],|r|r.get::<_,bool>(0))?{return Ok(Some(id))}
        }
        Ok(None)
    }).await?;
    Ok(found)
}
/// RetiredExchangeGuard: flash and back to the tracker, before any credential is read or a venue asked.
fn retired_answer(ctx:&Ctx)->Response{
    flash::set(&ctx.session,flash::ALERT,ctx.t("errors.exchange_retired"));
    crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker"))
}
pub async fn save(
    axum::extract::State(app): axum::extract::State<App>,
    axum::extract::Extension(ctx): axum::extract::Extension<Ctx>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let bot_request=ctx.params.route_path.starts_with("/bots/");
    let bot_account=if bot_request {owned_bot_account(&app,&ctx).await?} else {None};
    if bot_request && bot_account.is_none(){return Ok(crate::web::layout::missing())}
    // #710 7a rule S6: a bot sheet saves only for the exchange and account it was drawn for (its token says which); the bot
    // on another since is a 404, before any venue is asked. A token no sheet drew (a 2.x page, the header's) falls to the checks below.
    if let Some(account)=bot_account.filter(|_|bot_request) {
        let binding=app.db(move|c|Ok(sheet_binding(c,account.exchange_id,account.id)?)).await?;
        if ctx.params.authenticity_token().and_then(|t|crate::web::csrf::drawn_for(t,&app.keys.session,&sheet_scope(&ctx),&binding))==Some(false){return Ok(crate::web::layout::missing())}
    }
    // #710 7a §2: a bot sheet posts the account it showed; the bot on another one since is a 404, nothing saved.
    if bot_request && ctx.params.query("account_id").is_some_and(|a|Some(a.parse::<i64>().unwrap_or(0))!=bot_account.and_then(|a|a.id)){return Ok(crate::web::layout::missing())}
    // 7a rule R11: a sheet naming no account was rendered for the exchange's number 1 (`bot_step`), so it saves only while the
    // bot is still there; a bot moved since is a 404 too. A single-account install's sheet keeps its bytes.
    if bot_request && ctx.params.query("account_id").is_none() {
        let account=bot_account.and_then(|a|a.id);
        if !app.db(move|c|Ok(number_one(c,account)?)).await? {return Ok(crate::web::layout::missing())}
    }
    let exchange=if bot_request{bot_account.map(|a|a.exchange_id)}else{tracker_exchange(&app,&ctx).await?};
    let Some(exchange) = exchange else {
        return Ok(crate::web::layout::redirect(
            StatusCode::FOUND,
            &ctx.path("/tracker/pick_exchange/new"),
        ));
    };
    let kind = if bot_request {0} else {match ctx
        .params
        .form("key_type")
        .or_else(|| ctx.params.query("key_type"))
    {
        Some("trading") => 0,
        _ => 2,
    }};
    let inner = app.clone();
    let id = user.id;
    // #710 7a: the form names its account (`account_id`, a Reconnect) or none (`untargeted`, a venue tile); a 2.x form
    // names neither and saves into number 1, as before.
    // The tracker's card names them in its action (§2).
    let posted = if bot_request { None } else { ctx.params.form("account_id").or_else(|| ctx.params.query("account_id")).map(|a| a.parse::<i64>().unwrap_or(0)) };
    let untargeted = !bot_request && posted.is_none() && ctx.params.form("untargeted").or_else(|| ctx.params.query("untargeted")).is_some();
    let pick = match (posted, untargeted) { (Some(a), _) => Picked::Account(a), (None, true) => Picked::Untargeted, _ => Picked::Legacy };
    let found = app
        .db(move |c| {
            let venue = c
                .query_row("SELECT type,name FROM exchanges WHERE id=?1", [exchange], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default()))
                })
                .optional()?;
            // A bot's sheet saves into the bot's account; the tracker's into the account the form names (a merged, removed
            // or foreign one is a 404), else the exchange's.
            let account = match (bot_account, posted) {
                (Some(account), _) => account,
                (None, Some(posted)) => match crate::identity::pick(c, id, exchange, Some(posted))? { crate::identity::Pick::One(a) => a, _ => return Ok(None) },
                (None, None) => Account::first(c, id, exchange)?,
            };
            let label = crate::identity::label_or(c, account.id, venue.as_ref().map_or("", |(_, name)| name.as_str()))?.plain();
            let stored = venue.as_ref().map_or(kind, |(class, _)| slot(class, kind));
            let mut key = Key::read(c, &inner, account, stored)?;
            if untargeted { key = Key::untargeted(account); }
            Ok(Some((venue, label, key)))
        })
        .await?;
    let Some((venue, label, original)) = found else { return Ok(crate::web::layout::missing()) };
    if !bot_request && venue.as_ref().is_some_and(|(class, _)| crate::web::bot::retired_type(class)) {
        ctx.session.lock().tracker_connect=Some(exchange);
        return Ok(retired_answer(&ctx));
    }
    if !ctx.params.form.iter().any(|(name, _)| name.starts_with("api_key[")) {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    let Some((class, name)) = venue.filter(|(class, _)| ported(class)) else {
        return Ok(crate::web::layout::not_ported_for(&ctx));
    };
    if !bot_request {ctx.session.lock().tracker_connect=Some(exchange);}
    let live = ctx.params.form("api_key[passphrase]").map_or(original.passphrase.as_deref() == Some("live"), |mode| mode == "live");
    let outcome = submit(&app, &ctx, &class, &name, kind, original).await?;
    if let Submitted::Refused(reason) = &outcome { if let Some(asked) = question_response(&ctx, reason) { return Ok(asked); } }
    // D-TRACKER-SHEET, D-BOT-KEY-SHEET: a refused key says why under the sheet's fields.
    let message = match &outcome {
        Submitted::Correct(_) | Submitted::Moved { .. } => None,
        Submitted::Incorrect => Some(i18n::text(ctx.locale, "errors.incorrect_api_key_permissions", &[])),
        // ApplicationController#incorrect_api_key_message: the flags the venue named.
        Submitted::Permissions(missing, forbidden) => Some(super::key_check::permission_message(ctx.locale, &crate::tracker::rows::name_id(&class), &label, kind, missing, forbidden)),
        Submitted::Failed => Some(i18n::text(ctx.locale, "errors.api_key_permission_validation_failed", &[])),
        Submitted::Refused(reason) => Some(refusal_text(&ctx, reason)),
    };
    if let Some(message) = message {
        let (inner, v) = (app.clone(), ctx.clone());
        let (track, content) = app.db(move |c| if bot_request { Ok((String::new(), bot_step(c, &inner, &v, bot_account.map_or(0, |a| a.exchange_id), bot_account.and_then(|a| a.id), &class, &name, live, Some(&message))?)) }
                                              else { tracker_step(c, &inner, &v, exchange, pick, &class, &name, kind, live, Some(&message)) }).await?;
        let body = turbo::stream("update", "modal_content", &format!("{}\n", crate::web::wizard::sheet_frame(&track, &content)?));
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], format!("{body}\n")).into_response());
    }
    if bot_request {
        flash::set(&ctx.session,flash::NOTICE,ctx.t("errors.bots.api_key_success"));
        // 2.x's page refresh; the card's form closes the sheet on success (`modal--base#submitEnd`).
        return Ok(([(header::CONTENT_TYPE,turbo::CONTENT_TYPE)],turbo::refresh()).into_response());
    }
    { let mut data=ctx.session.lock(); data.tracker_connect=None; data.tracker_pick=None; }
    // The warnings widget is rebuilt from persisted rows, as in Tracker::AddApiKeysController.
    let v=ctx.clone();let owner=ctx.user().ok_or_else(||WebError::Config("missing warning owner".into()))?.id;
    let warnings = app.db(move|c|warnings(c,&v,owner)).await?;
    let body = turbo::stream("update", "sync-warnings", &warnings)
        + &turbo::redirect(&ctx.path("/tracker"));
    Ok(([(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response())
}

/// What a credential submission came to: ApiKey#validate_credentials!, then, for a correct key, its save.
/// `Correct(account)`: saved, on `account` (where §1 routed it; None on an install without the account tables).
pub(crate) enum Submitted { Correct(Option<i64>), Incorrect, Permissions(Vec<String>, Vec<String>), Failed, Refused(String),
    /// #710 7a: saved, but to the key's own account `to`, not to the account `from` the form named (left unchanged).
    Moved { to: i64, from: i64 } }

/// The submitted `api_key[...]` fields over the stored key (`original`), validated on `venue` outside the write lock, and a
/// correct one saved (status correct, sync error cleared) inside it: rechecked against what was validated, guarded, then the
/// syncs a reading key has woken after the commit. An incorrect or unverified key is not saved. It is saved into the
/// account `original` was read from.
pub(crate) async fn submit(app: &App, ctx: &Ctx, venue: &str, venue_name: &str, kind: i64, original: Key) -> Result<Submitted, WebError> {
    // #710 7a rule M2: Cancel on the save's question saves nothing.
    let answer = ctx.params.form("key_answer").map(str::to_string);
    if answer.as_deref() == Some("x") { return Ok(Submitted::Refused(UNASKED.into())); }
    // A tile's save starts from a blank candidate: it never inherits a stored key's omitted fields.
    let mut candidate = original.clone();
    if let Some(value) = ctx.params.form("api_key[key]") {
        candidate.key = Some(value.into());
    }
    if let Some(value) = ctx.params.form("api_key[secret]") {
        candidate.secret = Some(value.into());
    }
    if let Some(value) = ctx.params.form("api_key[passphrase]") {
        candidate.passphrase = Some(value.into());
    }
    for (i,name) in ["access_token","rsa_signature_key","rsa_encryption_key","dh_param"].iter().enumerate(){
        if let Some(value)=ctx.params.form(&format!("api_key[{name}]")){candidate.extras[i]=Some(value.into());}
    }
    if let Some(value)=ctx.params.form("api_key[ibkr_realm]"){candidate.realm=Some(value.into());}
    match submit_answered(app,venue,venue_name,kind,original,candidate,answer).await? {
        // `This key belongs to Kraken². Kraken¹ was not changed.`, on the page the save lands on.
        Submitted::Moved{to,from} => {
            let (saved,unchanged)=app.db(move|c|Ok((crate::identity::label(c,to)?.html(),crate::identity::label(c,from)?.html()))).await?;
            flash::set(&ctx.session,flash::NOTICE,i18n::t(ctx.locale,"settings.accounts.key_elsewhere_html",&[("account",i18n::Arg::Html(&saved)),("target",i18n::Arg::Html(&unchanged))]));
            Ok(Submitted::Correct(Some(to)))
        }
        other => Ok(other),
    }
}

/// `submit` of a candidate made by the caller over the stored key (`original`): the IBKR connect page's activation.
pub(crate) async fn submit_candidate(app: &App, venue: &str, venue_name: &str, kind: i64, original: Key, candidate: Key) -> Result<Submitted, WebError> {
    submit_answered(app, venue, venue_name, kind, original, candidate, None).await
}
/// `submit_candidate` with the answer to the save's question (rule M2), if one was posted.
async fn submit_answered(app: &App, venue: &str, venue_name: &str, kind: i64, original: Key, candidate: Key, answer: Option<String>) -> Result<Submitted, WebError> {
    // #710 7a: every live account's slot of this type before validation (key id and ciphertext version, absent included):
    // the slot routing picks must still be as it was under the write lock.
    let (snap_user,snap_exchange)=(original.account.user_id,original.account.exchange_id);
    let snap_app=app.clone();
    // D-ALPACA-ONE-KEY: validated as the slot asked for (a tracker key passes the reading check), stored where `slot` puts it.
    let stored=slot(venue,kind);
    let snapshot=snapshot_then_settle(app,snap_user,snap_exchange,move|c|slots(c,&snap_app,snap_user,snap_exchange,stored)).await?;
    let validated=validate(app,&candidate,&original,kind,venue).await?;
    let taught=match &validated.identity {Some(crate::identity::Observed::Read(read)) if venue!=crate::venue::ibkr::IBKR=>Some((crate::identity::Observed::Read(read.clone()),validated.handle.credentials().clone())),
        // Bybit review r1 #5: a definite refusal is the installed key's answer as a read is (validation-read venues only).
        Some(failed@crate::identity::Observed::Failed(code)) if crate::identity::VALIDATION_READS.contains(&venue)&&crate::identity::definite_failure(venue,code)=>Some((failed.clone(),validated.handle.credentials().clone())),
        _=>None};
    let installed=match original.account.id {Some(a) if !original.untargeted=>snapshot.iter().find(|(s,_)|*s==a).map(|(_,slot)|(original.account,stored,slot.clone())),_=>None};
    // #710 7a rule S2: a verdict that saves nothing still records what the account read taught (`learn_refused`).
    let refused=match &validated.result.value {
        Validity::Incorrect => Some(Submitted::Incorrect),
        Validity::Permissions { missing, forbidden } => {
            app.settings_key_logger.warn(&format!("[{venue_name}] API key validation: permissions do not match the steps"));
            Some(Submitted::Permissions(missing.clone(), forbidden.clone()))
        }
        Validity::Pending(diagnostic) => {
            let mut values = vec![
                candidate.key.clone(),
                candidate.secret.clone(),
                candidate.passphrase.clone(),
                original.key.clone(),
                original.secret.clone(),
                original.passphrase.clone(),
            ];
            values.extend(candidate.extras.iter().cloned());values.extend(original.extras.iter().cloned());
            let redaction_material = values;
            app.settings_key_logger.warn(&format!(
                "[{venue_name}] API key validation failed: {}",
                scrub(&diagnostic.log_text(),&redaction_material)
            ));
            Some(Submitted::Failed)
        }
        Validity::Correct | Validity::Account(_) | Validity::PendingActivation => None,
    };
    if let Some(refused)=refused {
        if let Some((read,material))=taught { learn_refused(app,original.account.user_id,snap_exchange,venue,material,read,installed).await?; }
        return Ok(refused);
    }
    // #710 7a rule Q1: a save that would make the exchange's second account reads the exchange's unread keys first.
    if let Some(crate::identity::Observed::Read(read))=validated.identity.clone() {
        let (inner,venue_s,material,target)=(app.clone(),venue.to_string(),validated.handle.credentials().clone(),if original.untargeted {None} else {original.account.id});
        let would_create=move|c:&Connection|->Result<bool,WebError>{
            let route=crate::identity::route_key(c,&inner.cipher,snap_user,snap_exchange,&venue_s,stored,target,&crate::identity::Reading::Read(&read),&material,None,inner.now()).map_err(WebError::Config)?;
            Ok(route==crate::identity::Route::New||replaces_unread(c,&inner,route,stored,&venue_s,&material)?)
        };
        if let Some(refusal)=read_unread(app,snap_user,snap_exchange,venue,venue_name,would_create).await? { return Ok(Submitted::Refused(refusal)); }
    }
    let venue_label=venue_name.to_string();
    // ApiKey#validate_credentials!: IBKR's activation wait is saved too (status pending_activation, its clock restarted).
    let pending=matches!(validated.result.value,Validity::PendingActivation);
    let account=match &validated.result.value {Validity::Account(account)=>Some(account.clone()),_=>None};
    let jobs=app.job_wakers()?;
    let inner = app.clone();
    let user_id=original.account.user_id;
    let venue_s=venue.to_string();
    let outcome=app.db(move|c|{
  let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
  if !crate::engine::model::account_is_current(&tx,validated.handle.producer())? {return Ok(Some("credential changed during validation".into()));}
  // The account's row first: a key of the number 1 2.x saved resolves to it from here on (a tile names none).
  let named=if original.untargeted {None} else {Some(original.account.ensure(&tx,inner.now())?)};
  // #710 7a (PLAN-710-7 §1.1): where the key goes. IBKR keeps its single-account binding (7b).
  let mut moved=None;
  let target=if venue_s==crate::venue::ibkr::IBKR {named.unwrap_or(original.account.ensure(&tx,inner.now())?)} else {
   let reading=match (crate::identity::VENUES.contains(&venue_s.as_str()),&validated.identity) {
    (false,_)=>crate::identity::Reading::NotAnIdentityVenue,
    (true,Some(crate::identity::Observed::Read(id)))=>crate::identity::Reading::Read(id),
    (true,_)=>crate::identity::Reading::Unread,
   };
   let route=crate::identity::route_key(&tx,&inner.cipher,user_id,original.account.exchange_id,&venue_s,stored,named.and_then(|a|a.id),&reading,validated.handle.credentials(),None,inner.now()).map_err(WebError::Config)?;
   let dest=match route {
    crate::identity::Route::To(id)=>id,
    // Rules Q1/P1: never a new account while another key of the exchange has no account read (one added meanwhile).
    crate::identity::Route::New if !needs_reads(&tx,user_id,original.account.exchange_id,&venue_s)?.is_empty()=>return Ok(Some(other_key_checking(&venue_label))),
    // Rule M2: a new account for an exchange account another one holds history of is asked first.
    crate::identity::Route::New=>match &validated.identity {
     Some(crate::identity::Observed::Read(read))=>match key_question(&tx,&inner.cipher,user_id,original.account.exchange_id,stored,named.and_then(|a|a.id),read,validated.handle.credentials(),answer.as_deref(),inner.now())? {
      KeyAsk::New=>crate::identity::new_account(&tx,user_id,original.account.exchange_id,inner.now())?,
      KeyAsk::To(account)=>account,
      KeyAsk::Ask(question)=>return Ok(Some(question)),
     },
     _=>crate::identity::new_account(&tx,user_id,original.account.exchange_id,inner.now())?,
    },
    crate::identity::Route::Unconfirmed=>return Ok(Some(UNCONFIRMED.into())),
   };
   if let Some(from)=named.and_then(|a|a.id).filter(|from|*from!=dest){moved=Some((dest,from));}
   Account::by_id(&tx,dest)?.ok_or_else(||WebError::Config("routed account missing".into()))?.ensure(&tx,inner.now())?
  };
  let current=Key::read(&tx,&inner,target,stored)?;
  if named.is_some_and(|n|n.id==target.id) {
   if !original.same(&current){return Ok(Some("credential changed during validation".into()));}
  } else if key_slot(&current)!=snapshot.iter().find(|(a,_)|Some(*a)==target.id).map(|(_,s)|s.clone()).unwrap_or(None)
     && !current.id.is_some_and(|k|crate::identity::is_material(&tx,&inner.cipher,k,&venue_s,validated.handle.credentials())) {
   // A routed slot is as it was before validation (absent still absent), else the save is retried; a concurrent save of
   // this very material there is this one's (a no-op success).
   return Ok(Some("credential changed during validation".into()));
  }
  if !validated.result.current_for(&Some(validated.handle.producer().clone())).is_fresh(){return Ok(Some("credential changed during validation".into()));}
  // Rule Q1: on an identity venue's exchange with two or more accounts, no save while one of its keys has no account read
  // (one added meanwhile; the slots it may replace were read before the save).
  if venue_s!=crate::venue::ibkr::IBKR&&several(&tx,user_id,original.account.exchange_id)?&&needs_reads(&tx,user_id,original.account.exchange_id,&venue_s)?.into_iter().any(|k|Some(k)!=current.id) {return Ok(Some(other_key_checking(&venue_label)));}
  let candidate=&validated.key;
  let now=format_time(inner.now());
  let status=if pending {3} else {1};
  // A submission awaiting activation always moves updated_at (activation_stalled? reads it as the last submission).
  let changed=pending||!(same(&candidate.key,&current.key)&&same(&candidate.secret,&current.secret)&&same(&candidate.passphrase,&current.passphrase)&&candidate.extras.iter().zip(&current.extras).all(|(a,b)|same(a,b))&&candidate.realm==current.realm&&current.status==1&&current.last_error.is_none());
  if current.id.is_some() { if let Some(r)=settling(&tx,&inner.cipher,&target)? { return Ok(Some(r.into())); } }
  let id={
   let now=if changed {now} else {current.updated_at.clone().unwrap_or(now)};
   let key=validated.columns[0].clone();let secret=validated.columns[1].clone();let passphrase=validated.columns[2].clone();
   match current.id{
    Some(id)=>{
     crate::identity::write_credential(&tx,&inner.cipher,id,key.as_deref(),secret.as_deref(),passphrase.as_deref(),inner.now())?;
     crate::identity::write_status(&tx,id,status,&now,false,true)?;id},
    None=>{tx.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,key,secret,passphrase,status,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,?6,?8,?7,?7)",rusqlite::params![user_id,target.exchange_id,stored,key,secret,passphrase,&now,status])?;
     let id=tx.last_insert_rowid();target.add_key(&tx,id)?;id}
   }
  };
  tx.execute("UPDATE api_keys SET access_token=?1,rsa_signature_key=?2,rsa_encryption_key=?3,dh_param=?4,ibkr_realm=?5 WHERE id=?6",rusqlite::params![validated.columns[3],validated.columns[4],validated.columns[5],validated.columns[6],candidate.realm,id])?;
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(refusal.reason()));}
  // D-IBKR-ACCOUNT: the key binds to the account it names, or is refused (nothing saved) when bound to another.
  if let Some(account)=&account {
   if let Err(refusal)=crate::venue::ibkr_account::bind(&tx,&inner.cipher,id,account,inner.now()).map_err(WebError::Config)? {
    return Ok(Some(if refusal==crate::venue::ibkr_account::OTHER_ACCOUNT {"\u{0}ibkr_other_account".into()} else {refusal}));
   }
  }
  // The read was of the material just written: recorded under its fresh ciphertext version, so the key counts as read
  // (#710 7a rule Q2) from this save on.
  let fresh=crate::sync::credentials_with_version(&tx,&inner.cipher,id).ok().map(|(_,v)|v);
  if let Some(observed)=&validated.identity{crate::identity::record(&tx,&inner.cipher,id,validated.handle.credentials(),fresh.as_ref().unwrap_or(validated.handle.producer()),observed,inner.now()).map_err(WebError::Config)?;}
  if venue_s!=crate::venue::ibkr::IBKR {
   if let (Some(crate::identity::Observed::Read(read)),Some(account))=(&validated.identity,target.id) {
    // The routed account takes the fingerprint (every key type); every other copy of this key, on another account, was
    // recorded as reading this account by `identity::record` above, so it is fenced there at once (§9.1).
    crate::identity::establish_read(&tx,&inner.cipher,account,&venue_s,read,inner.now()).map_err(WebError::Config)?;
   }
   crate::identity::write_digest(&tx,&inner.cipher,id,target.exchange_id,validated.handle.credentials()).map_err(WebError::Config)?;
  }
  // The slots a restart would register (sync::jobs::register, tracker::jobs::register): a key that reads for its
  // venue gets its syncs, and its owner the tracker's jobs. Woken after the commit only.
  // Rules M4/N7: the save may move a reader of the exchange's accounts; each account's reader is woken.
  crate::sync::settle_readers(&tx,user_id,target.exchange_id).map_err(|e|WebError::Config(e.0))?;
  let readers=readers_of(&tx,user_id,target.exchange_id)?;
  let ledger=crate::sync::ledger_reading_keys(&tx).map_err(|e|WebError::Config(e.0))?.contains(&id);
  let balances=crate::sync::balance_reading_keys(&tx).map_err(|e|WebError::Config(e.0))?.contains(&id);
  tx.commit()?;
  if stored==0 {inner.wake_engine();}
  if ledger {jobs.wake(crate::sync::jobs::LEDGER_SYNC,Some(&id.to_string()),None);}
  if balances {jobs.wake(crate::sync::jobs::BALANCE_SYNC,Some(&id.to_string()),None);}
  for k in readers.iter().filter(|k|**k!=id) {for job in [crate::sync::jobs::LEDGER_SYNC,crate::sync::jobs::BALANCE_SYNC] {jobs.wake(job,Some(&k.to_string()),None);}}
  if ledger||balances {
   for job in [crate::tracker::jobs::TRACKER_LEDGER,crate::tracker::jobs::PORTFOLIO_BACKFILL] {jobs.wake(job,Some(&user_id.to_string()),None);}
  }
  Ok(Some(match moved {Some((to,from))=>format!("\u{0}moved:{to}:{from}"),None=>format!("\u{0}saved:{}",target.id.map_or(String::new(),|a|a.to_string()))}))
 }).await?;
    if let (Some(r),Some((read,material)))=(&outcome,taught) {
        if !r.starts_with("\u{0}saved:")&&!r.starts_with("\u{0}moved:") { learn_refused(app,user_id,snap_exchange,venue,material,read,installed).await?; }
    }
    Ok(match outcome {
        Some(m) if m.starts_with("\u{0}saved:") => Submitted::Correct(m.trim_start_matches("\u{0}saved:").parse().ok()),
        Some(m) if m.starts_with("\u{0}moved:") => { let mut p=m.trim_start_matches("\u{0}moved:").split(':').map(|x|x.parse::<i64>().unwrap_or(0)); Submitted::Moved{to:p.next().unwrap_or(0),from:p.next().unwrap_or(0)} }
        Some(r)=>Submitted::Refused(r),
        None=>Submitted::Correct(None),
    })
}

/// #710 7a rule R2: a key save refused after its account read (an open order holding the slot, the slot changed meanwhile,
/// the JSON save's refusals) still records what the read taught: every stored copy of the material on an account that
/// knows another fingerprint is fenced there (`identity::fence_copies`), in a transaction of its own.
/// #710 slice 9 (G4 r6): on a venue whose identity read is its validation read (`identity::VALIDATION_READS`), a form save
/// naming its slot (`installed`: the account, the slot type and the slot as it was before the request) whose installed key
/// is still that key, at that ciphertext version, and this material records the read as that key's own (`identity::record`,
/// which fences the other copies too); its status and material stay as they are. A reader it moves is woken (rules K5/M4).
async fn learn_refused(app:&App,user:i64,exchange:i64,venue:&str,material:crate::crypto::Credentials,read:crate::identity::Observed,installed:Option<(Account,i64,Slot)>)->Result<(),WebError>{
    let (inner,venue)=(app.clone(),venue.to_string());
    let moved=app.db(move|c|{
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let current=match installed.filter(|_|crate::identity::VALIDATION_READS.contains(&venue.as_str())) {
            Some((account,kind,before))=>{let key=Key::read(&tx,&inner,account,kind)?;
                key.id.filter(|id|before.is_some()&&key_slot(&key)==before&&crate::identity::is_material(&tx,&inner.cipher,*id,&venue,&material))},
            None=>None,
        };
        let moved=match current {
            Some(id)=>{
                let generations=crate::identity::generations_of_key(&tx,id).map_err(WebError::Config)?;
                let (_,version)=crate::sync::credentials_with_version(&tx,&inner.cipher,id).map_err(|e|WebError::Config(e.0))?;
                crate::identity::record(&tx,&inner.cipher,id,&material,&version,&read,inner.now()).map_err(WebError::Config)?;
                crate::identity::moved_readers(&tx,id,&generations).map_err(WebError::Config)?
            }
            None=>{if let crate::identity::Observed::Read(read)=&read {crate::identity::fence_copies(&tx,&inner.cipher,user,exchange,&venue,&material,read,None,inner.now()).map_err(WebError::Config)?;}vec![]},
        };
        tx.commit()?;Ok(moved)
    }).await?;
    if let Ok(jobs)=app.job_wakers() {
        for k in moved { for job in [crate::sync::jobs::LEDGER_SYNC,crate::sync::jobs::BALANCE_SYNC] { jobs.wake(job,Some(&k.to_string()),None); } }
    }
    Ok(())
}
/// The reader of every live account of (user, exchange) (`sync::reader_of`).
fn readers_of(c: &Connection, user: i64, exchange: i64) -> Result<Vec<i64>, WebError> {
    let mut out = vec![];
    for (id, _) in crate::identity::accounts_of(c, user, exchange)? {
        let Some(account) = Account::by_id(c, id)? else { continue };
        out.extend(crate::sync::reader_of(c, &account.keys("k")).map_err(|e| WebError::Config(e.0))?);
    }
    Ok(out)
}
/// A slot of an account: its key's id and ciphertext version (None: absent).
type Slot = Option<(i64, Option<String>)>;
fn key_slot(k: &Key) -> Slot { k.id.map(|id| (id, k.version.as_ref().map(|v| v.cache_stamp().to_string()))) }
/// Every live account of (user, exchange) with its slot of key type `kind`.
fn slots(c: &Connection, app: &App, user: i64, exchange: i64, kind: i64) -> Result<Vec<(i64, Slot)>, WebError> {
    let mut out = vec![];
    for (id, _) in crate::identity::accounts_of(c, user, exchange)? {
        let Some(account) = Account::by_id(c, id)? else { continue };
        out.push((id, key_slot(&Key::read(c, app, account, kind)?)));
    }
    Ok(out)
}

#[derive(Template)]
#[template(path="settings/sync_warnings.html")]
struct Warnings{title:String,fixes:Vec<Fix>}
struct Fix{message:String,trading:bool,trading_path:String,reading_path:String,trading_label:String,add_label:String,replace_label:String}
/// `unresolved`: the key's last balance sync held codes no asset is named by (`rust_sync.kraken_unresolved:<key_id>`, D-KR-6),
/// a failure 2.x never records; it reads as the sync's own failure.
struct WarningKey{exchange:i64,account:Option<i64>,name:String,venue:String,status:i64,kind:i64,error:Option<String>,unresolved:bool}
impl WarningKey{
    fn failed(&self)->bool{self.unresolved||self.error.as_deref().is_some_and(|e|!crate::v2::blank(e))}
    /// ApiKey#missing_permission?: the venue's own permission_denied texts (Exchange#permission_error?), one table for every venue.
    fn permission(&self)->bool{crate::tracker::import::run::missing_permission(&self.venue,self.error.as_deref())}
}
/// `ApiKey.sync_warnings` rendered as tracker/_sync_warnings: the contents of #sync-warnings.
pub(crate) fn warnings(c:&Connection,v:&Ctx,user:i64)->Result<String,WebError>{
    let keys=crate::identity::with_key_account("k",|account|c.prepare(&format!("SELECT k.exchange_id,{},e.type,k.status,k.key_type,k.last_sync_error,EXISTS(SELECT 1 FROM app_configs a WHERE a.key='rust_sync.kraken_unresolved:'||k.id),{account} FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type!=1 ORDER BY k.id",crate::identity::plain_label_sql("e.name",account)))?
    .query_map([user],|r|Ok(WarningKey{exchange:r.get(0)?,account:r.get(7)?,name:r.get(1)?,venue:r.get(2)?,status:r.get(3)?,kind:r.get(4)?,error:r.get(5)?,unresolved:r.get(6)?}))?.collect::<Result<Vec<_>,_>>())?;
    // Per account (#710).
    let mut failed=vec![];let mut seen=std::collections::HashSet::new();
    for key in &keys{
        if !seen.insert((key.exchange,key.account)){continue}
        let group:Vec<_>=keys.iter().filter(|k|(k.exchange,k.account)==(key.exchange,key.account)).collect();
        let reading=group.iter().copied().find(|k|k.status==1&&k.kind==0&&!k.permission()).or_else(||group.iter().copied().find(|k|k.status==1&&k.kind==2)).or_else(||group.iter().copied().find(|k|k.status==1));
        let chosen=if let Some(reading)=reading{reading.failed().then_some(reading)}else{group.iter().copied().find(|k|k.kind==2&&k.failed()).or_else(||group.iter().copied().find(|k|k.failed()))};
        if let Some(key)=chosen{failed.push(key)}
    }
    if failed.is_empty(){return Ok(String::new())}
    let names=failed.iter().map(|k|k.name.as_str()).collect::<Vec<_>>().join(", ");
    let mut fixes=vec![];
    for key in failed{
        let reason=if key.permission(){Some("missing_permission")}else if key.status==2{Some("dead")}else{None};
        if let Some(reason)=reason{
            let args=[("exchange",i18n::Arg::Text(&key.name))];let label=|name:&str|i18n::t(v.locale,name,&args);
            // #710 7a §2: a fix names the account it is for.
            let target=target_query(c,key.account)?;
            fixes.push(Fix{message:label(&format!("tracker.key_fix.{}.{reason}",if key.kind==0{"trading"}else{"read_only"})),trading:key.kind==0&&key.venue!=crate::sync::ALPACA,trading_path:i18n::escape(&v.path(&format!("/tracker/add_api_key/new?exchange_id={}&key_type=trading{target}",key.exchange))),reading_path:i18n::escape(&v.path(&format!("/tracker/add_api_key/new?exchange_id={}&key_type=read_only{target}",key.exchange))),trading_label:label("tracker.replace_trading_key"),add_label:label("tracker.add_tracker_key"),replace_label:label(if key.kind==0{"tracker.replace_trading_key"}else{"tracker.replace_tracker_key"})});
        }
    }
    Ok(Warnings{title:i18n::t(v.locale,"tracker.sync_failed_for",&[("exchanges",i18n::Arg::Text(&names))]),fixes}.render()?+"\n")
}

pub(crate) fn lookup(locale: &str, key: &str) -> Option<&'static str> {
    for language in [locale, i18n::DEFAULT] {
        let key = format!("{language}.{key}");
        if let Some((_, text)) = i18n::all().iter().find(|(name, _)| *name == key) {
            return Some(*text);
        }
    }
    None
}
/// settings/widgets/_api_keys for `user`.
pub fn list(c: &Connection, ctx: &Ctx, csrf: &str, user: i64) -> Result<String, WebError> {
    // Each key named by its account (#710 slice 6).
    let rows = crate::identity::with_key_account("k", |account| c.prepare(&format!("SELECT k.id,e.name,e.type,k.key_type,{} FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 ORDER BY k.id", crate::identity::label_number_sql(account)))?
        .query_map([user], |r| Ok((r.get::<_, i64>(0)?, crate::identity::Label { name: r.get(1)?, number: r.get(4)? }, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?
        .collect::<Result<Vec<_>, _>>())?;
    let keys: Vec<super::sections::Key> = rows
        .into_iter()
        .map(|(id, name, venue, kind)| {
            let name_id = crate::tracker::rows::name_id(&venue);
            let prefix = if kind == 1 { "withdrawal_api" } else { "bot.api" };
            let instructions = lookup(ctx.locale, &format!("{prefix}.{name_id}.instructions")).is_some()
                || lookup(ctx.locale, &format!("{prefix}.{name_id}.instructions.0.text_html")).is_some();
            super::sections::Key { id, name, name_id, read_only: kind == 2, withdrawal: kind == 1, instructions }
        })
        .collect();
    // #710 (PLAN-710-6 §6.1): one Merge row per duplicate, above the keys.
    let duplicates = crate::identity::duplicates(c, user)?.into_iter()
        .map(|(d, s)| Ok((crate::identity::label(c, d)?.html(), crate::identity::label(c, s)?.html(), d, s))).collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(super::sections::api_keys(ctx, csrf, &keys, &duplicates))
}
/// settings/widgets/_market_data: the container's market data source, the one section Connect holds (D-BROKERS-AS-VENUES).
/// `show_coingecko_form`: 2.x's `@show_coingecko_form`, a switch to CoinGecko with no key to use. `platform_errors`: 2.x's
/// `@platform_errors` with `@show_platform_form`, a claim that failed. `wizard`: the provider whose connect card the index
/// wizard's Index view draws in its place (D-INDEX-CONNECT-IN-VIEW), its writes answering the wizard.
pub(crate) fn market_data(c:&Connection,app:&App,v:&Ctx,csrf:&str,show_coingecko_form:bool,platform_errors:Option<&str>,wizard:Option<&str>)->Result<String,WebError>{
    let setting=|name:&str|auth::app_config(c,&app.cipher,name);
    let has=|name:&str|Ok::<_,WebError>(setting(name)?.is_some_and(|s|!crate::v2::blank(&s)));
    let provider=setting("market_data_provider")?.unwrap_or_default();let hosted=app.config.market_data_url;
    let platform_connected=has("platform_connected_at")?;
    let proxies=super::platform::proxies(c)?;
    let hosted_note=|key:&str|i18n::t(v.locale,key,&[("provider_name",i18n::Arg::Text(&app.settings_market_provider_name))]);
    let coingecko_configured=!hosted&&provider=="coingecko";
    // What the reader is in the middle of outranks what is stored; on a plain visit that is what is stored.
    let selected=if platform_errors.is_some(){"deltabadger"}else if show_coingecko_form||coingecko_configured{"coingecko"}else if provider=="deltabadger"{"deltabadger"}else{""};
    let market_note=hosted_note("settings.market_data.deltabadger_configured");
    let platform_note=v.t(if proxies{"settings.platform.connected_with_proxies"}else{"settings.platform.connected_without_proxies"});
    let coingecko_instructions=v.t("setup.coingecko_api_key_hint_html");
    // D-MARKET-DATA-PICK: Settings' card: a claim refused or a CoinGecko switch with no key needs its form again; else the tile picked.
    let card=if platform_errors.is_some(){Some("deltabadger")}else if show_coingecko_form{Some("coingecko")}else{v.params.query("provider").filter(|p|matches!(*p,"coingecko"|"deltabadger"))};
    let m=super::sections::Market{hosted:hosted.then_some(market_note.as_str()),selected,coingecko_configured,platform_connected,platform_note:&platform_note,coingecko_instructions:&coingecko_instructions,platform_errors,card};
    Ok(match wizard { Some(provider) => super::sections::market_data_card(v,csrf,provider,&m,true), None => super::sections::market_data(v,csrf,&m) })
}
/// GET /settings/api_keys: the exchange keys.
pub async fn show_keys(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)); };
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let token = csrf.clone();
    let (pane, shell) = app.db(move |c| Ok((list(c, &v, &token, user.id)?, crate::web::shell::Shell::load(c, &inner, &user)?))).await?;
    let user = ctx.user().ok_or_else(|| WebError::Config("missing key owner".into()))?;
    super::view::section_page(&ctx, &csrf, user, &shell, "api_keys", &format!("  {pane}\n"), StatusCode::OK, vec![])
}
/// GET /settings/connect: for an admin, the container's market data; for anyone else an empty pane, as 2.x draws it.
pub async fn show(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)); };
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let token = csrf.clone();
    let (pane, shell) = app.db(move |c| {
        let pane = if user.admin { format!("\n{}\n", market_data(c, &inner, &v, &token, false, None, None)?) } else { String::new() };
        Ok((pane, crate::web::shell::Shell::load(c, &inner, &user)?))
    }).await?;
    let user = ctx.user().ok_or_else(|| WebError::Config("missing key owner".into()))?;
    super::view::section_page(&ctx, &csrf, user, &shell, "connect", &pane, StatusCode::OK, vec![])
}
/// GET /settings/api_key_permissions/:id: a key's walkthrough in the pane, the sidebar on API keys.
pub async fn modal(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let Some(id) = ctx.params.route_path.rsplit('/').next().and_then(|s| s.parse::<i64>().ok()) else {
        return Ok(crate::web::layout::missing());
    };
    let inner = app.clone();
    let v = ctx.clone();
    let found=app.db(move|c|{let key=crate::identity::with_key_account("k",|account|c.query_row(&format!("SELECT e.name,e.type,k.key_type,{} FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.id=?1 AND k.user_id=?2",crate::identity::plain_label_sql("e.name",account)),(id,user.id),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?))).optional())?;
        key.map(|(name,venue,kind,label)|Ok(((label,venue.clone()),key_instructions(c,&inner,&v,&venue,&name,kind)?,crate::web::shell::Shell::load(c,&inner,&user)?))).transpose()}).await?;
    let Some(((name, venue), instructions, shell)) = found else {
        return Ok(crate::web::layout::missing());
    };
    if !ported(&venue) {
        return Ok(crate::web::layout::not_ported_for(&ctx));
    }
    let csrf = ctx.csrf_token();
    let pane = format!(
        "  <a class=\"sheet__back\" href=\"{}\">\n    {}{}\n</a>  <h3 class=\"sheet__heading\">{}</h3>\n  <div class=\"sheet__prose\">{}</div>\n",
        i18n::escape(&ctx.path("/settings/api_keys")),
        super::dialog::BACK,
        ctx.t("settings.index.api_keys_section"),
        i18n::t(ctx.locale, "bot.setup.how_to_get_keys", &[("exchange", i18n::Arg::Text(&name))]),
        instructions
    );
    let user = ctx.user().ok_or_else(|| WebError::Config("missing key owner".into()))?;
    super::view::section_page(&ctx, &csrf, user, &shell, "api_keys", &pane, StatusCode::OK, vec![])
}
pub async fn delete(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let Some(id) = ctx
        .params
        .route_path
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
    else {
        return Ok(crate::web::layout::missing());
    };
    // #710 7a rule R6: the key's version before the standalone pre-flight reads the venue; only that version is deleted.
    let found:Option<(i64,Stored)>=app.db(move|c|{
        let exchange:Option<i64>=c.query_row("SELECT exchange_id FROM api_keys WHERE id=?1 AND user_id=?2",(id,user.id),|r|r.get(0)).optional()?;
        Ok(match exchange {Some(exchange)=>Some((exchange,stored_ciphertext(c,id)?)),None=>None})
    }).await?;
    let expected=match found {Some((exchange,stored))=>{settle_standalone(&app,user.id,exchange).await?;Some(stored)},None=>None};
    let inner = app.clone();
    let v = ctx.clone();
    let csrf = ctx.csrf_token();
    let outcome=app.db(move|c|{
  remove(c,&inner,user.id,id,expected.as_ref(),|tx|list(tx,&v,&csrf,user.id))
 }).await?;
    match outcome {
        None => Ok(crate::web::layout::missing()),
        Some(Ok(body)) => Ok((
            [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
            turbo::stream("replace", "api_keys", &body),
        )
            .into_response()),
        Some(Err(reason)) => {
            let text = refusal_text(&ctx, &reason);
            Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
                turbo::prepend_flash(
                    flash::render(&flash::take(&ctx.session, &[(flash::ALERT, text)]))?.trim_end(),
                ),
            )
                .into_response())
        }
    }
}

/// A key's deletion (SettingsController#destroy_api_key, Settings::IbkrConnectionsController#destroy): its owner's working bots
/// on its account stopped first when it trades, its ledger rows kept (unlinked), its account identity and IBKR binding dropped.
/// `then` runs inside the transaction after the delete, before the commit. None when the user has no such key (or one whose
/// account resolves to none); refused with nothing written while a bot of the account settles an order, or when the
/// eligibility guard objects. `expected`: the key's version the request saw before any venue read; a key replaced since
/// (another request saved a new one in place) is kept and the delete refused to retry (#710 7a rule R6).
pub(crate) fn remove<T>(c: &Connection, inner: &App, user: i64, id: i64, expected: Option<&Stored>,
                        then: impl FnOnce(&Connection) -> Result<T, WebError>) -> Result<Option<Result<T, String>>, WebError> {
  let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
  let kind:Option<i64>=tx.query_row("SELECT key_type FROM api_keys WHERE id=?1 AND user_id=?2",(id,user),|r|r.get(0)).optional()?;let Some(kind)=kind else{return Ok(None);};
  if let Some(expected)=expected {
   if &stored_ciphertext(&tx,id)?!=expected {return Ok(Some(Err("credential changed during validation".into())));}
  }
  // Before the delete: the key's account is read through its row.
  let Some(account)=Account::of_key(&tx,id)? else{return Ok(None);};
  if let Some(r)=settling(&tx,&inner.cipher,&account)? { return Ok(Some(Err(r.into()))); }
  let now=format_time(inner.now());
  if kind==0{
   let mut s=tx.prepare(&format!("SELECT b.id FROM bots b WHERE {} AND b.status IN(1,4,5,6) ORDER BY b.id",account.bots("b")))?;let ids=s.query_map([],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;drop(s);
   for bot in ids{tx.execute("UPDATE bots SET status=2,stopped_at=?1,stop_message_key=NULL,updated_at=?1 WHERE id=?2",(&now,bot))?;tx.execute("INSERT INTO bot_activity_logs(bot_id,event,level,message,details,created_at)VALUES(?1,'stopped',0,NULL,'{}',?2)",(bot,&now))?;}
  }
  tx.execute("UPDATE account_transactions SET api_key_id=NULL WHERE api_key_id=?1",[id])?;
  tx.execute("DELETE FROM api_keys WHERE id=?1 AND user_id=?2",(id,user))?;
  crate::identity::forget(&tx,id,&account,inner.now())?;
  if crate::venue::ibkr_account::binding(&tx,id)?.is_some() { if let Some(retained)=account.ensure(&tx,inner.now())?.id {crate::venue::ibkr_account::retain(&tx,id,retained)?;} }
  crate::venue::ibkr_account::forget(&tx,id)?;
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(Err(refusal.reason())));}
  // #710: the account's reading key now reads its balances again, so its duplicate standing may re-enter on them.
  let reader=match crate::identity::AccountRow::of(&tx,&account)? {Some(a)=>crate::sync::cache::reading_key(&tx,&a)?,None=>None};
  let out=then(&tx)?;tx.commit()?;crate::venue::ibkr::forget(id);inner.wake_engine();
  if let (Some(k),Ok(jobs))=(reader,inner.job_wakers()){for job in [crate::sync::jobs::BALANCE_SYNC,crate::sync::jobs::LEDGER_SYNC] {jobs.wake(job,Some(&k.to_string()),None);}}
  Ok(Some(Ok(out)))
}

/// A key row's stored ciphertext, every encrypted column: any save of the slot changes it (#710 7a rule R6).
pub(crate) type Stored = [Option<String>; 7];
pub(crate) fn stored_ciphertext(c: &Connection, id: i64) -> Result<Stored, WebError> { Ok(crate::sync::credential_columns(c, id)?) }

/// `legacy`'s answers to a pick (§4.2): an account not the user's live one of this exchange, and which account.
const MISSING: &str = "\0account not found";
const AMBIGUOUS: &str = "\0account required: ";
/// The browser's JSON endpoint: #497 replaces in place and resets only changed-key ledger progress.
pub async fn legacy(
    axum::extract::State(app): axum::extract::State<App>,
    axum::extract::Extension(ctx): axum::extract::Extension<Ctx>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    // 2.x reads `params.require(:api_key)` only: never a top-level `?secret=`. Its params merge the query string
    // and the body with the body winning, so `?api_key[name]=` counts only when the body has no value.
    let parameter = |name: &str| {
        let nested = format!("api_key[{name}]");
        ctx.params
            .form(&nested)
            .or_else(|| {
                ctx.params
                    .json
                    .as_ref()
                    .and_then(|j| j.get("api_key"))
                    .and_then(|root| root.get(name))
                    .and_then(|v| v.as_str())
            })
            .map(str::to_string)
            .or_else(||ctx.params.json.as_ref().and_then(|j|j.get("api_key")).and_then(|root|root.get(name)).filter(|value|value.is_number()||value.is_boolean()).map(serde_json::Value::to_string))
            .or_else(|| ctx.params.query(&nested).map(str::to_string))
    };
    if !ctx
        .params
        .form
        .iter()
        .any(|(name, _)| name.starts_with("api_key["))
        && ctx
            .params
            .json
            .as_ref()
            .and_then(|j| j.get("api_key"))
            .is_none()
    {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    let exchange = parameter("exchange_id").and_then(|s| s.parse::<i64>().ok());
    let kind = match parameter("key_type").as_deref() {
        Some("withdrawal") => 1,
        Some("read_only") => 2,
        _ => 0,
    };
    let key = parameter("key").map(|s| s.replace("\\n", "\n"));
    let secret = parameter("secret").map(|s| s.replace("\\n", "\n"));
    let passphrase = parameter("passphrase");
    let answer = parameter("key_answer");
    // #710 (PLAN-710-6 §4.1): the account the save names; none with one live account is the exchange's.
    // Only an omitted or null account_id is the exchange's account; one that is no id (`[]`) names none.
    let raw_account = ctx.params.json.as_ref().and_then(|j| j.get("api_key")).and_then(|root| root.get("account_id")).is_some_and(|v| !v.is_null());
    let posted_account = match parameter("account_id") {
        None if raw_account => return Ok(layout::missing()),
        None => None,
        Some(text) => match text.trim().parse::<i64>() { Ok(id) => Some(id), Err(_) => return Ok(layout::missing()) },
    };
    let Some(exchange) = exchange else {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
            serde_json::json!({"data":false}).to_string(),
        )
            .into_response());
    };
    // #710 7a (D-JSON-KEY-IDENTITY): an identity venue's key is read for its account first (the permission check stays
    // asynchronous), so the save routes like every other.
    let venue_type:Option<String>=app.db(move|c|Ok(c.query_row("SELECT type FROM exchanges WHERE id=?1",[exchange],|r|r.get(0)).optional()?)).await?;
    // D-ALPACA-ONE-KEY: the slot the save writes (`slot`) is the one snapshotted.
    let kind=venue_type.as_deref().map_or(kind,|t|slot(t,kind));
    // As the form save (`submit_candidate`): the named account's slot and every live account's of this type, before the
    // identity read; the slot the save writes must still be as it was under the write lock.
    let snap_app=app.clone();
    let (snap_key,snap_secret,alpaca)=(key.clone(),secret.clone(),venue_type.as_deref()==Some(crate::sync::ALPACA));
    let snapshot=snapshot_then_settle(&app,user.id,exchange,move|c|{
        let (named,ambiguous)=match crate::identity::pick(c,user.id,exchange,posted_account)? {
            crate::identity::Pick::One(a)=>(Some(Key::read(c,&snap_app,a,kind)?),None),
            crate::identity::Pick::Ambiguous(accounts)=>{
                let name:String=c.query_row("SELECT coalesce(name,'') FROM exchanges WHERE id=?1",[exchange],|r|r.get(0))?;
                (None,Some(crate::identity::Pick::required(&name,&accounts)))
            }
            crate::identity::Pick::NotFound=>(None,None),
        };
        // #710 slice 9: an Alpaca repost of the named slot's key and secret keeps the slot's stored mode when it names none or
        // one selecting the same host (review r1 #5: the material read and recorded is then the stored one, byte for byte).
        let kept_mode=named.as_ref().filter(|n|alpaca&&same(&n.key,&snap_key)&&same(&n.secret,&snap_secret)).map(|n|n.passphrase.clone());
        Ok((named.as_ref().map(key_slot),slots(c,&snap_app,user.id,exchange,kind)?,kept_mode,ambiguous))
    }).await?;
    // Alpaca branch review r1 #2: a save naming no account on an exchange with two is refused before any request.
    if let Some(required)=&snapshot.3 {
        return Ok((StatusCode::UNPROCESSABLE_ENTITY,[(header::CONTENT_TYPE,"application/json; charset=utf-8")],serde_json::json!({"data":false,"message":required}).to_string()).into_response());
    }
    // #710 slice 9 (D-ALPACA-IDENTITY-READ): ONE effective Alpaca mode, resolved before any request, is what the account read
    // reads (its host), what the save compares (a switch, to the other host, is new material) and what it writes.
    let same_host=|slot:&Option<String>|crate::identity::alpaca_mode(passphrase.as_deref())==crate::identity::alpaca_mode(slot.as_deref());
    let switch=snapshot.2.as_ref().is_some_and(|slot|passphrase.is_some()&&!same_host(slot));
    let passphrase=match &snapshot.2 { Some(slot) if passphrase.is_none()||same_host(slot)=>slot.clone(), _=>passphrase };
    let material=crate::crypto::Credentials{redaction_values:vec![],key:key.clone().unwrap_or_default(),secret:secret.clone().unwrap_or_default(),passphrase:passphrase.clone(),ibkr:None};
    let observed=match venue_type.as_deref().filter(|t|crate::identity::VENUES.contains(t)) {
        Some(venue) if key.is_some() => json_identity(&app,venue,&material).await?,
        _ => None,
    };
    // A mode switch is written only when the other host's read succeeds; otherwise it is refused and the slot stays as it is,
    // byte for byte: its 401 as an invalid key, anything else (review r1 #1: a 503) as "try again".
    if switch&&!matches!(observed,Some(crate::identity::Observed::Read(_))) {
        let body=if crate::identity::not_refused(crate::sync::ALPACA,observed.as_ref()) {(StatusCode::CONFLICT,serde_json::json!({"data":false,"message":refusal_text(&ctx,UNCONFIRMED)}))}
                 else {(StatusCode::UNPROCESSABLE_ENTITY,serde_json::json!({"data":false}))};
        return Ok((body.0,[(header::CONTENT_TYPE,"application/json; charset=utf-8")],body.1.to_string()).into_response());
    }
    let taught=match (&observed,venue_type.as_deref()) {(Some(crate::identity::Observed::Read(read)),Some(venue)) if venue!=crate::venue::ibkr::IBKR=>Some((read.clone(),venue.to_string(),material.clone())),_=>None};
    // #710 7a rule Q1, as the form save: a save that would make the exchange's second account reads its unread keys first.
    if let (Some(crate::identity::Observed::Read(read)),Some(venue))=(observed.clone(),venue_type.clone()) {
        let name:String=app.db(move|c|Ok(c.query_row("SELECT coalesce(name,'') FROM exchanges WHERE id=?1",[exchange],|r|r.get(0))?)).await?;
        let (inner,v,m,uid)=(app.clone(),venue.clone(),material.clone(),user.id);
        let would_create=move|c:&Connection|->Result<bool,WebError>{
            let target=match crate::identity::pick(c,uid,exchange,posted_account)? {crate::identity::Pick::One(a)=>a.id,_=>return Ok(false)};
            let route=crate::identity::route_key(c,&inner.cipher,uid,exchange,&v,kind,target,&crate::identity::Reading::Read(&read),&m,None,inner.now()).map_err(WebError::Config)?;
            Ok(route==crate::identity::Route::New||replaces_unread(c,&inner,route,kind,&v,&m)?)
        };
        if let Some(refusal)=read_unread(&app,user.id,exchange,&venue,&name,would_create).await? {
            return Ok((StatusCode::CONFLICT,[(header::CONTENT_TYPE,"application/json; charset=utf-8")],serde_json::json!({"data":false,"message":refusal_text(&ctx,&refusal)}).to_string()).into_response());
        }
    }
    let jobs = app.job_wakers()?;
    let inner = app.clone();
    let saved=app.db(move|c|{
  let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
  let venue:Option<String>=tx.query_row("SELECT type FROM exchanges WHERE id=?1",[exchange],|r|r.get(0)).optional()?;
  let Some(venue)=venue else{return Ok(Err(None));};
  let kind=slot(&venue,kind);
  // The account's row first: a key of the number 1 2.x saved resolves to it from here on.
  let account=match crate::identity::pick(&tx,user.id,exchange,posted_account)?{
   crate::identity::Pick::One(account)=>account,
   crate::identity::Pick::NotFound=>return Ok(Err(Some(MISSING.into()))),
   crate::identity::Pick::Ambiguous(accounts)=>{
    let name:String=tx.query_row("SELECT coalesce(name,'') FROM exchanges WHERE id=?1",[exchange],|r|r.get(0))?;
    return Ok(Err(Some(format!("{AMBIGUOUS}{}",crate::identity::Pick::required(&name,&accounts)))));
   }
  }.ensure(&tx,inner.now())?;
  // #710 7a (§1.1): the named account is the target; the key goes where `route_key` routes it. The named account's own
  // key posted again is that key (a status reset), whatever the read says.
  let named=Key::read(&tx,&inner,account,kind)?;
  // An Alpaca repost is of the same mode too: a switch is new material, routed as any (#710 slice 9, r5).
  let same_mode=|k:&Key|venue!=crate::sync::ALPACA||crate::identity::alpaca_mode(k.passphrase.as_deref())==crate::identity::alpaca_mode(passphrase.as_deref());
  let reposted=same(&named.key,&key)&&same(&named.secret,&secret)&&same_mode(&named);
  let mut read=None;
  let reading=match (crate::identity::VENUES.contains(&venue.as_str()),&observed) {
   (false,_)=>crate::identity::Reading::NotAnIdentityVenue,
   (true,Some(crate::identity::Observed::Read(id)))=>{read=Some(id.clone());crate::identity::Reading::Read(id)},
   (true,_)=>crate::identity::Reading::Unread,
  };
  // The named account's own key posted again is that key (a status reset), unless a fresh read says it opens another
  // account: then that read is recorded on it (fencing it, §9.1) and nothing is saved.
  let account=if venue==crate::venue::ibkr::IBKR||(reposted&&read.is_none()) {account} else {
   let dest=match crate::identity::route_key(&tx,&inner.cipher,user.id,exchange,&venue,kind,account.id,&reading,&material,None,inner.now()).map_err(WebError::Config)? {
    crate::identity::Route::To(id) if !reposted||Some(id)==account.id=>id,
    crate::identity::Route::To(_)|crate::identity::Route::New if reposted=>{
     // Every copy of this material off the account the read names is fenced, the posted slot's and the account's others.
     if let Some(read)=&read {crate::identity::fence_copies(&tx,&inner.cipher,user.id,exchange,&venue,&material,read,None,inner.now()).map_err(WebError::Config)?;}
     tx.commit()?;return Ok(Err(Some(KEY_READS_ELSEWHERE.into())));
    }
    crate::identity::Route::To(id)=>id,
    // Rules Q1/P1: never a second account while another key of the exchange has no account read.
    crate::identity::Route::New if !needs_reads(&tx,user.id,exchange,&venue)?.is_empty()=>{
     let name:String=tx.query_row("SELECT coalesce(name,'') FROM exchanges WHERE id=?1",[exchange],|r|r.get(0))?;
     return Ok(Err(Some(other_key_checking(&name))));
    }
    // Rule M2, as the form save: asked first; `api_key[key_answer]` answers.
    crate::identity::Route::New=>match &read {
     Some(id)=>match key_question(&tx,&inner.cipher,user.id,exchange,kind,account.id,id,&material,answer.as_deref(),inner.now())? {
      KeyAsk::New=>crate::identity::new_account(&tx,user.id,exchange,inner.now())?,
      KeyAsk::To(to)=>to,
      KeyAsk::Ask(question)=>return Ok(Err(Some(question))),
     },
     None=>crate::identity::new_account(&tx,user.id,exchange,inner.now())?,
    },
    crate::identity::Route::Unconfirmed=>return Ok(Err(Some(UNCONFIRMED.into()))),
   };
   Account::by_id(&tx,dest)?.ok_or_else(||WebError::Config("routed account missing".into()))?.ensure(&tx,inner.now())?
  };
  let current=Key::read(&tx,&inner,account,kind)?;
  let before=if account.id==named.account.id {snapshot.0.clone().unwrap_or(None)} else {snapshot.1.iter().find(|(a,_)|Some(*a)==account.id).map(|(_,s)|s.clone()).unwrap_or(None)};
  if key_slot(&current)!=before&&!current.id.is_some_and(|k|crate::identity::is_material(&tx,&inner.cipher,k,&venue,&material)) {
   // Replaced (or added) by another request while this one read the key's account: retried, never overwritten.
   return Ok(Err(Some("credential changed during validation".into())));
  }
  // Rule Q1: on an exchange with two or more accounts, no save while one of its keys has no account read (added meanwhile).
  if venue!=crate::venue::ibkr::IBKR&&several(&tx,user.id,exchange)?&&needs_reads(&tx,user.id,exchange,&venue)?.into_iter().any(|k|Some(k)!=current.id) {
   let name:String=tx.query_row("SELECT coalesce(name,'') FROM exchanges WHERE id=?1",[exchange],|r|r.get(0))?;
   return Ok(Err(Some(other_key_checking(&name))));
  }
  if current.id.is_some() { if let Some(r)=settling(&tx,&inner.cipher,&account)? { return Ok(Err(Some(r.to_string()))); } }
  let same_keys=same(&current.key,&key)&&same(&current.secret,&secret)&&same_mode(&current);
  if venue=="Exchanges::Hyperliquid"{
   let address=crate::v2::validation_regex(r"\A0x[0-9a-fA-F]{40}\z").map_err(|_|WebError::Config("invalid wallet pattern".into()))?;
   let agent=crate::v2::validation_regex(r"\A(0x)?[0-9a-fA-F]{64}\z").map_err(|_|WebError::Config("invalid agent pattern".into()))?;
   if !address.is_match(key.as_deref().unwrap_or(""))||!agent.is_match(secret.as_deref().unwrap_or("")){return Ok(Err(None));}
  }
  if !ported(&venue){return Ok(Err(Some("credential venue is not ported".into())));}
  let now=format_time(inner.now());
  let saved_passphrase=if same_keys {passphrase.as_ref().or(current.passphrase.as_ref())}else{passphrase.as_ref()};
  let id=if let Some(id)=current.id{
   let stamp=if !same_keys||current.status!=0 {now.clone()} else {current.updated_at.clone().unwrap_or_else(||now.clone())};
   if same_keys {
    // Review r1 #4 (D-COINBASE-TRANSIENT-KEEPS-CORRECT): a correct key of a venue whose identity read is its validation
    // read, reposted unchanged while that read came to no verdict, stays correct (its standing with it); the validator
    // woken below decides.
    let kept=crate::identity::VALIDATION_READS.contains(&venue.as_str())&&current.status==1&&!matches!(observed,Some(crate::identity::Observed::Read(_)))
        &&crate::identity::not_refused(&venue,observed.as_ref());
    crate::identity::write_status(&tx,id,if kept {1} else {0},&stamp,false,false)?;
   } else {
    let encrypted=|v:Option<&String>|v.map(|s|inner.cipher.encrypt(s));
    crate::identity::write_credential(&tx,&inner.cipher,id,encrypted(key.as_ref()).as_deref(),encrypted(secret.as_ref()).as_deref(),encrypted(saved_passphrase).as_deref(),inner.now())?;
    crate::identity::write_status(&tx,id,0,&stamp,false,true)?;
   }
   id
  }else{
   tx.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,key,secret,passphrase,status,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,?6,0,?7,?7)",(user.id,exchange,kind,key.as_ref().map(|s|inner.cipher.encrypt(s)),secret.as_ref().map(|s|inner.cipher.encrypt(s)),passphrase.as_ref().map(|s|inner.cipher.encrypt(s)),&now))?;
   let id=tx.last_insert_rowid();account.add_key(&tx,id)?;id
  };
  if venue!=crate::venue::ibkr::IBKR {
   if let (Some(read),Some(routed))=(&read,account.id) {
    // As `submit_candidate`: the routed account takes the fingerprint; other copies of the key are fenced (§9.1).
    crate::identity::establish_read(&tx,&inner.cipher,routed,&venue,read,inner.now()).map_err(WebError::Config)?;
    crate::identity::fence_copies(&tx,&inner.cipher,user.id,exchange,&venue,&material,read,Some(id),inner.now()).map_err(WebError::Config)?;
   }
   if !same_keys {crate::identity::write_digest(&tx,&inner.cipher,id,exchange,&material).map_err(WebError::Config)?;}
   // The key's own read, of the material just written (#710 7a rule Q2: it counts as read from here on).
   if let (Some(read),Ok((_,version)))=(&read,crate::sync::credentials_with_version(&tx,&inner.cipher,id)) {
    crate::identity::record(&tx,&inner.cipher,id,&material,&version,&crate::identity::Observed::Read(read.clone()),inner.now()).map_err(WebError::Config)?;
   }
  }
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Err(Some(refusal.reason())));}
  crate::sync::settle_readers(&tx,user.id,exchange).map_err(|e|WebError::Config(e.0))?;
  let readers=readers_of(&tx,user.id,exchange)?;
  tx.commit()?;inner.wake_engine();jobs.wake(crate::sync::jobs::API_KEY_VALIDATOR,Some(&id.to_string()),None);if !same_keys{jobs.wake(crate::sync::jobs::LEDGER_SYNC,Some(&id.to_string()),None);}
  for k in readers.iter().filter(|k|**k!=id) {for job in [crate::sync::jobs::LEDGER_SYNC,crate::sync::jobs::BALANCE_SYNC] {jobs.wake(job,Some(&k.to_string()),None);}}
  Ok(Ok(()))
 }).await?;
    if let (Err(Some(_)),Some((read,venue,material)))=(&saved,taught) { learn_refused(&app,user.id,exchange,&venue,material,crate::identity::Observed::Read(read),None).await?; }
    if matches!(&saved,Err(Some(reason)) if reason=="credential venue is not ported"){return Ok(layout::not_ported_for(&ctx));}
    if matches!(&saved,Err(Some(reason)) if reason==MISSING){return Ok(layout::missing());}
    let (status, data) = match saved {
        Ok(()) => (StatusCode::CREATED, serde_json::json!({"data":true})),
        Err(None) => (StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({"data":false})),
        // An exchange with 2+ live accounts and no account named: which one, in the services' words (§4.2).
        Err(Some(reason)) if reason==UNCONFIRMED||reason==KEY_READS_ELSEWHERE||reason.starts_with(OTHER_KEY_CHECKING) => (StatusCode::CONFLICT, serde_json::json!({"data":false,"message":refusal_text(&ctx,&reason)})),
        Err(Some(reason)) if reason.starts_with(AMBIGUOUS) => (StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({"data":false,"message":&reason[AMBIGUOUS.len()..]})),
        // Rule M2: the question, and the answers to post back as `api_key[key_answer]`.
        Err(Some(reason)) if reason.starts_with(QUESTION) => {
            let (lines, choices) = question_of(&ctx, &reason).unwrap_or_default();
            (StatusCode::CONFLICT, serde_json::json!({"data":false,"message":lines.join(" "),"answers":choices.iter().map(|(value,label)|serde_json::json!({"value":value,"label":label})).collect::<Vec<_>>()}))
        }
        Err(Some(reason)) => (StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({"data":false,"message":refusal_text(&ctx,&reason)})),
    };
    Ok((status, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], data.to_string()).into_response())
}

/// An identity venue's account read of `material` for the JSON save (D-JSON-KEY-IDENTITY), through the venue's key-check
/// egress as the key check sends it. None when nothing was sent.
async fn json_identity(app:&App,venue:&str,material:&crate::crypto::Credentials)->Result<Option<crate::identity::Observed>,WebError>{
    // #710 slice 9: Alpaca's key check has no proxy; its account read goes where that check goes, the host of the mode
    // `material` names (the effective one: the caller resolves it before the request).
    if venue==crate::sync::ALPACA {return Ok(crate::identity::read_live(venue,&app.alpaca_trading_url(material),crate::venue::http::client(),material).await)}
    let Some(name)=crate::venue::registry::key_check_proxy(venue) else {return Ok(None)};
    let base=app.venue_key_url(venue);
    let cipher=app.cipher.clone();
    let name=name.to_string();
    let egress=app.db(move|c|Ok(crate::venue::live::egress(c,&cipher,&|k|std::env::var(k).ok(),&name,&crate::venue::http::client()))).await?;
    Ok(match egress {
        Ok((client,_))=>crate::identity::read_live(venue,&base,client,material).await,
        Err(_)=>Some(crate::identity::Observed::Failed("no egress".into())),
    })
}

/// Recheck a committed slot without rewriting its credentials. Q binds the result to the captured digest.
pub(crate) async fn revalidate(app:&App,id:i64)->Result<Option<String>,WebError>{
    let inner=app.clone();
    let (credentials,version,values,kind)=app.db(move|c|{
        let (credentials,version,values)=crate::sync::validation_material(c,&inner.cipher,id).map_err(|_|WebError::Config("cannot read stored credential".into()))?;
        let (kind,venue,name)=c.query_row("SELECT k.key_type,e.type,e.name FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.id=?1",[id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?;
        Ok((credentials,version,values,(kind,venue,name)))
    }).await?;
    let (kind,venue,name)=kind;
    let handle=super::validator::ValidationHandle::stored(credentials,version);
    let (result,identity)=handle.check_on(app,&venue,kind).await?;
    let account=match &result.value {Validity::Account(account)=>Some(account.clone()),_=>None};
    let status=result.map(|result|match result {Validity::Correct|Validity::Account(_)=>1,Validity::PendingActivation=>3,Validity::Incorrect=>2,Validity::Permissions{..}=>{
        app.settings_key_logger.warn(&format!("[{name}] API key validation: permissions do not match the steps"));2
    },Validity::Pending(ref e)=>{
        app.settings_key_logger.warn(&format!("[{name}] API key validation failed: {}",scrub(&e.log_text(),&values)));0
    }});
    // D-IBKR-INCONCLUSIVE (as the validator job): an IBKR check with no verdict leaves the key as it is.
    if venue==crate::venue::ibkr::IBKR && status.value==0 {return Ok(None)}
    let jobs=app.job_wakers()?;let (inner,wakers,venue_t)=(app.clone(),jobs.clone(),venue.clone());
    let result=app.db(move|c|{
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let fenced=crate::engine::model::check_credential_record(&tx,status.origin())?;
        let before:i64=fenced.query_row("SELECT status FROM api_keys WHERE id=?1",[id],|r|r.get(0))?;
        let kept=transient_kept(&fenced,id,&venue_t,status.value,identity.as_ref())?;
        let status=status.map(|_|kept);
        // #710 7a rule M4: the recheck may make the key (or another) its account's reader; that reader is woken below.
        let generations=crate::identity::generations_of_key(&fenced,id).map_err(WebError::Config)?;
        let status=bound_status(&fenced,&inner.cipher,id,status.clone(),account.as_deref(),inner.now()).map_err(|e|WebError::Config(e.0))?;
        store_status(&fenced,&status,id,inner.now()).map_err(|_|WebError::Config("cannot store credential status".into()))?;
        if let Some(observed)=&identity{crate::identity::record(&fenced,&inner.cipher,id,handle.credentials(),handle.producer(),observed,inner.now()).map_err(WebError::Config)?;}
        if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(refusal.reason()))}
        crate::sync::settle_readers_of_key(&tx,id).map_err(|e|WebError::Config(e.0))?;
        let moved=crate::identity::moved_readers(&tx,id,&generations).map_err(WebError::Config)?;
        // The engine is woken by a status that moved; the same answer again changes nothing it reads.
        tx.commit()?;if before!=status.value{inner.wake_engine();}
        for k in moved {for job in [crate::sync::jobs::LEDGER_SYNC,crate::sync::jobs::BALANCE_SYNC] {wakers.wake(job,Some(&k.to_string()),None);}}
        Ok(None)
    }).await;
    match result{
        Err(WebError::Engine(crate::engine::EngineError::CredentialsChanged))=>{jobs.wake(crate::sync::jobs::API_KEY_VALIDATOR,Some(&id.to_string()),None);Ok(None)},
        out=>out,
    }
}

#[derive(Template)]
#[template(path="settings/alpaca_form.html")]
struct AlpacaForm<'a> { v:&'a Ctx, csrf:&'a str, action:String, form_data:&'a str, connect_title:String, instructions_title:String, instructions:String, live:bool, error:Option<&'a str>,
    name_id:String, mark:&'static str, alpaca:bool, passphrase:bool, key_label:String, secret_label:String, back:Option<String> }

/// The venues whose keys this build checks (the wizard's list): every other venue's key route is a named refusal.
fn ported(class:&str)->bool{crate::web::wizard::keys::ported(class)}
/// BotHelper#render_api_key_instructions for a key of `kind` on venue `class` named `name`: the walkthrough, the IP its
/// keys are bound to (the venue's proxy, ExchangeProxy.for) in it; a reading key's own list, else the bot's.
pub(crate) fn key_instructions(c:&Connection,app:&App,ctx:&Ctx,class:&str,name:&str,kind:i64)->Result<String,WebError>{
    let name_id=crate::tracker::rows::name_id(class);
    let proxy=crate::venue::http::proxy_for(&name_id,crate::app_config::get(c,&app.cipher,&format!("proxy_{name_id}")).map_err(WebError::Config)?,&|k|std::env::var(k).ok());
    let ip=super::key_check::whitelist_ip_html(ctx.locale,proxy.as_deref());
    let html=|prefix:&str|super::key_check::instructions_html(ctx.locale,prefix,&name_id,name,&ip);
    let mut out=match kind{1=>html("withdrawal_api"),2=>html("read_only_api").or_else(||html("bot.api")),_=>html("bot.api")}.unwrap_or_default();
    // D-DESKTOP-STABLE-IP-NOTE: a desktop's own IP is what the key gets bound to when no proxy stands in front of the venue,
    // and a home connection's IP changes; a subscription's proxies don't. The link is a full-page navigation: the card is
    // drawn inside the wizard sheet's frame and inside Settings' pane.
    if ctx.desktop()&&proxy.is_none()&&auth::app_config(c,&app.cipher,"platform_connected_at")?.is_none_or(|at|crate::v2::blank(&at)){
        let url=ctx.path("/settings/connect?provider=deltabadger");
        out.push_str(&format!("<p class=\"key-note\">{}</p>",i18n::t(ctx.locale,"bot.api.desktop_stable_ip_html",&[("url",i18n::Arg::Text(&url))])));
    }
    Ok(out)
}
/// D-ALPACA-ONE-KEY: Alpaca's key has no permissions, so the tracker's key there is the trading key the bots read (one key
/// per account, whichever page saved it); every other venue keeps the slot asked for.
pub(crate) fn slot(class:&str,kind:i64)->i64{if class==crate::sync::ALPACA&&kind==2{0}else{kind}}
/// Exchange#requires_passphrase? outside Alpaca (whose passphrase is its Paper/Live switch): the API Passphrase row.
fn requires_passphrase(class:&str)->bool{class!=crate::sync::ALPACA&&crate::venue::registry::row(class).is_some_and(|r|r.passphrase)}
/// The venue's connect card as the sheet draws it (D-WIZARD-SHEET-KEY-FORM): Settings' fields and button (`.rinput`, `.rbutton`)
/// under the venue's name alone (the button says "Connect"), the walkthrough (`instructions`) below; a refused key's reason
/// (`error`) under the fields, which it marks invalid. `name`/`class` are the exchange row's name and type. With `back` (a
/// wizard's venue step) the venue's logo is Back (D-KEY-LOGO-BACK); without (the bot page's key, D-BOT-KEY-SHEET) it is a plain
/// header.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sheet_card(ctx:&Ctx,csrf:&str,action:String,form_data:&str,name:&str,class:&str,live:bool,instructions:String,error:Option<&str>,back:Option<String>)->Result<String,WebError>{
    let name_id=crate::tracker::rows::name_id(class);
    // BotHelper#api_key_field_label: the venue's own word for a field, else the generic one.
    let label=|field:&str|{let specific=format!("bot.api.{name_id}.{field}");if lookup(ctx.locale,&specific).is_some(){ctx.t(&specific)}else{ctx.t(&format!("bot.api.{field}_label"))}};
    Ok(AlpacaForm{v:ctx,csrf,action,form_data,connect_title:i18n::escape(name),
        instructions_title:i18n::t(ctx.locale,"bot.setup.how_to_get_keys",&[("exchange",i18n::Arg::Text(name))]),instructions,live,error,
        mark:crate::web::bot::exchange_svg(&name_id),alpaca:class=="Exchanges::Alpaca",passphrase:requires_passphrase(class),key_label:label("public_key"),secret_label:label("private_key"),name_id,back}.render()?)
}
pub async fn form(axum::extract::State(app):axum::extract::State<App>, axum::extract::Extension(ctx):axum::extract::Extension<Ctx>) -> Result<Response,WebError> {
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let exchange=tracker_exchange(&app,&ctx).await?;
    let Some(exchange)=exchange else {return Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker/pick_exchange/new")))};
    // D-TRACKER-IBKR-SHEET: IBKR's connection, whatever key type the link asked for, before any key is read the generic way
    // (its material may not decrypt; its own reader offers Disconnect then).
    let class:Option<String>=app.db(move|c|Ok(c.query_row("SELECT type FROM exchanges WHERE id=?1",[exchange],|r|r.get(0)).optional()?)).await?;
    if class.as_deref()==Some(crate::venue::ibkr::IBKR){
        ctx.session.lock().tracker_connect=Some(exchange);
        // What the connection's last write earned, drawn in the card: a frame does not show the layout's flash.
        let notice=std::mem::take(&mut ctx.session.lock().flash).pop().map(|(kind,text)|(kind==flash::ALERT,text));
        let (inner,v,owner)=(app.clone(),ctx.clone(),user.clone());
        let found=app.db(move|c|Ok((super::ibkr::tracker_step(c,&inner,&v,owner.id,notice)?,crate::web::shell::Shell::load(c,&inner,&owner)?))).await?;
        let (Some((track,content)),shell)=found else {return Ok(crate::web::layout::not_ported_for(&ctx))};
        let csrf=ctx.csrf_token();
        let body=crate::web::wizard::sheet_to(&ctx,"/tracker",&track,&content)?;
        return crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]});
    }
    let requested=ctx.params.query("key_type").filter(|s|matches!(*s,"trading"|"read_only"));
    // The card keeps the slot asked for (its action and walkthrough); the key it shows is the slot saves write (`slot`).
    let asked=if requested==Some("trading"){0}else{2};
    let kind=slot(class.as_deref().unwrap_or_default(),asked);
    let (inner,owner,view,kept)=(app.clone(),user.clone(),ctx.clone(),ctx.session.lock().tracker_pick.clone());
    let found=app.db(move|c| {
        let venue:Option<(String,String)>=c.query_row("SELECT type,name FROM exchanges WHERE id=?1",[exchange],|r|Ok((r.get(0)?,r.get::<_,Option<String>>(1)?.unwrap_or_default()))).optional()?;
        // #710 7a §2: a Reconnect link's `account_id`, else the venue step's pick of this exchange, else number 1 (a 2.x form).
        // The tracker menu's venue tile (`untargeted`) always asks.
        let pick=if view.params.query("account_id").is_some(){crate::web::wizard::picked(c,&view,owner.id,exchange)?}
            else if view.params.query("untargeted").is_some(){Some(Picked::Untargeted)}
            else{Some(kept_pick(kept.as_ref(),exchange))};
        let Some(pick)=pick else{return Ok(None)};
        let account=match pick{
            Picked::Account(id)=>match crate::identity::pick(c,owner.id,exchange,Some(id))?{crate::identity::Pick::One(a)=>a,_=>return Ok(None)},
            _=>Account::first(c,owner.id,exchange)?,
        };
        // healthy_trading_key: a correct bot key that lacks no permission the sync needs.
        let errors=c.prepare(&format!("SELECT last_sync_error FROM api_keys k WHERE {} AND key_type=0 AND status=1 ORDER BY id LIMIT 1",account.keys("k")))?.query_map([],|r|r.get::<_,Option<String>>(0))?.collect::<Result<Vec<_>,_>>()?;
        // A venue tile always asks for a key, from a blank candidate.
        let untargeted=pick==Picked::Untargeted;
        let healthy=!untargeted&&venue.as_ref().is_some_and(|(class,_)|errors.iter().any(|error|!crate::tracker::import::run::missing_permission(class,error.as_deref())));
        let key=if untargeted{Key::untargeted(account)}else{Key::read(c,&inner,account,kind)?};
        Ok(Some((venue,healthy,key,crate::web::shell::Shell::load(c,&inner,&owner)?,pick)))
    }).await?;
    let Some((venue,healthy,mut key,shell,pick))=found else{return Ok(crate::web::layout::missing())};
    let Some((class,name))=venue else {return Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker/pick_exchange/new")))};
    if crate::web::bot::retired_type(&class){ctx.session.lock().tracker_connect=Some(exchange);return Ok(retired_answer(&ctx))}
    if !ported(&class){return Ok(crate::web::layout::not_ported_for(&ctx))}
    ctx.session.lock().tracker_connect=Some(exchange);
    // A GET rechecks the slot it shows; Alpaca's tracker card shows the trading key, which its own card rechecks.
    if asked==kind&&!(requested.is_none()&&healthy)&&key.status!=1&&key.key.as_deref().is_some_and(|s|!crate::v2::blank(s))&&key.secret.as_deref().is_some_and(|s|!crate::v2::blank(s)){
        if let Some(id)=key.id{
            if let Some(reason)=revalidate(&app,id).await?{
                let body=turbo::prepend_flash(&flash::render(&flash::take(&ctx.session,&[(flash::ALERT,refusal_text(&ctx,&reason))]))?);
                return Ok((StatusCode::UNPROCESSABLE_ENTITY,[(header::CONTENT_TYPE,turbo::CONTENT_TYPE)],body).into_response())
            }
            let (inner,account)=(app.clone(),key.account);
            key=app.db(move|c|Key::read(c,&inner,account,kind)).await?;
        }
    }
    if requested.is_none()&&pick!=Picked::Untargeted&&(healthy||(key.status==1&&!crate::tracker::import::run::missing_permission(&class,key.last_error.as_deref()))){return Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker")))}
    let csrf=ctx.csrf_token();
    let (inner,v,live)=(app.clone(),ctx.clone(),key.passphrase.as_deref()==Some("live"));
    let (track,content)=app.db(move|c|tracker_step(c,&inner,&v,exchange,pick,&class,&name,asked,live,None)).await?;
    let body=crate::web::wizard::sheet_to(&ctx,"/tracker",&track,&content)?;
    crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]})
}

/// D-TRACKER-SHEET: the tracker's key step in the sheet, however it was asked for (the connect button's venue step, or a venue,
/// sync-warning or gap link straight to it): the track (the venue done, its link back to the venue step; Connect current) and
/// the venue's card, `error` under its fields.
#[allow(clippy::too_many_arguments)]
/// The card's action names the pick (#710 7a §2): `account_id` (a Connected tile or a Reconnect), `untargeted` (a venue tile),
/// neither for a 2.x form.
fn tracker_step(c:&Connection,app:&App,ctx:&Ctx,exchange:i64,pick:Picked,class:&str,name:&str,kind:i64,live:bool,error:Option<&str>)->Result<(String,String),WebError>{
    use crate::web::wizard::{self, Stop};
    let instructions=key_instructions(c,app,ctx,class,name,kind)?;
    let target=match pick{Picked::Account(id)=>format!("&account_id={id}"),Picked::Untargeted=>"&untargeted=1".into(),Picked::Legacy=>String::new()};
    let action=ctx.path(&format!("/tracker/add_api_key?exchange_id={exchange}&key_type={}{target}",if kind==0{"trading"}else{"read_only"}));
    let card=sheet_card(ctx,&ctx.csrf_token(),action,SHEET_FORM,name,class,live,instructions,error,Some(ctx.path("/tracker/pick_exchange/new")))?;
    let track=wizard::track(ctx,1,vec![Stop::new("exchange").name(Some(name.to_string())).back(ctx.path("/tracker/pick_exchange/new")),Stop::new("connect")])?;
    Ok((track,wizard::keys::key_pane(&card)?))
}

/// Tracker::PickExchangesController#new: every available venue, stable ones first (Bots::Searchable), none disabled, in the sheet
/// (D-TRACKER-SHEET): the pane's search box over the venues, brokers and exchanges apart; the track's Exchange current.
pub async fn pick_form(axum::extract::State(app):axum::extract::State<App>,axum::extract::Extension(ctx):axum::extract::Extension<Ctx>)->Result<Response,WebError>{
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let inner=app.clone();let owner=user.clone();
    let (venues,connected,shell)=app.db(move|c|{
        let mut s=c.prepare("SELECT id,name,type FROM exchanges WHERE available=1")?;
        let venues=s.query_map([],|r|Ok(crate::web::bot::Exchange{id:r.get(0)?,name:r.get::<_,Option<String>>(1)?.unwrap_or_default(),class:r.get(2)?,maker_fee:None,number:None}))?.collect::<Result<Vec<_>,_>>()?;
        Ok((venues,crate::web::wizard::exchanges::connected(c,owner.id,&[0,2])?,crate::web::shell::Shell::load(c,&inner,&owner)?))
    }).await?;
    use crate::web::wizard::{self, exchanges, Stop};
    let csrf=ctx.csrf_token();
    let table=exchanges::Table{action:ctx.path("/tracker/pick_exchange"),csrf:csrf.clone(),param_name:"exchange_id".into(),venues:vec![],unavailable_note:String::new()};
    let content=exchanges::venue_pane(&ctx,table,venues,&connected)?;
    let body=wizard::sheet_to(&ctx,"/tracker",&wizard::track(&ctx,0,vec![Stop::new("exchange"),Stop::new("connect")])?,&content)?;
    crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]})
}
/// Tracker::PickExchangesController#create: `Exchange.find` (any row), the session keeps only its id.
pub async fn pick_save(axum::extract::State(app):axum::extract::State<App>,axum::extract::Extension(ctx):axum::extract::Extension<Ctx>)->Result<Response,WebError>{
    if ctx.user().is_none() {return Ok(auth::unauthenticated(&ctx))}
    // A missing or non-numeric id finds no row: RecordNotFound, 2.x's 404.
    let Some(Ok(id))=ctx.params.form("exchange_id").or_else(||ctx.params.query("exchange_id")).map(str::parse::<i64>) else {return Ok(crate::web::layout::missing())};
    let (view,user)=(ctx.clone(),ctx.user().map_or(0,|u|u.id));
    let found=app.db(move|c|Ok(if c.query_row("SELECT EXISTS(SELECT 1 FROM exchanges WHERE id=?1)",[id],|r|r.get::<_,bool>(0))?{crate::web::wizard::picked(c,&view,user,id)?}else{None})).await?;
    // #710 7a §2: a forged, merged or removed account is a 404 too.
    let Some(pick)=found else {return Ok(crate::web::layout::missing())};
    let mut kept=serde_json::json!({"exchange_id":id});
    crate::web::wizard::keep_pick(&mut kept,pick);
    let mut data=ctx.session.lock();
    data.tracker_connect=Some(id);
    data.tracker_pick=(pick!=Picked::Legacy).then_some(kept);
    drop(data);
    Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker/add_api_key/new")))
}

/// `&account_id=N` for a link or action that targets `account` while its exchange has 2+ live accounts (#710 7a §2); with
/// one, the link names its exchange alone, as before (it means that account).
pub(crate) fn target_query(c:&Connection,account:Option<i64>)->rusqlite::Result<String>{
    Ok(match account{Some(id) if crate::identity::label(c,id)?.number.is_some()=>format!("&account_id={id}"),_=>String::new()})
}
/// The tracker venue step's kept pick, for `exchange` only (another exchange's is a 2.x pick).
fn kept_pick(kept:Option<&serde_json::Value>,exchange:i64)->Picked{
    let Some(kept)=kept.filter(|k|k["exchange_id"].as_i64()==Some(exchange)) else{return Picked::Legacy};
    match kept["account_id"].as_i64(){Some(id)=>Picked::Account(id),None if crate::web::wizard::untargeted(Some(kept))=>Picked::Untargeted,None=>Picked::Legacy}
}
/// The account of the user's bot this request names (#710: its sheet reads and saves that account's key).
async fn owned_bot_account(app:&App,ctx:&Ctx)->Result<Option<Account>,WebError>{
    let Some(user)=ctx.user() else{return Ok(None)};let owner=user.id;
    let id=ctx.params.route_path.split('/').nth(2).and_then(|s|s.parse::<i64>().ok());
    let Some(id)=id else{return Ok(None)};
    app.db(move|c|{
        let exchange:Option<i64>=c.query_row("SELECT exchange_id FROM bots WHERE id=?1 AND user_id=?2",[id,owner],|r|r.get(0)).optional()?;
        Ok(match exchange {Some(_)=>Account::of_bot(c,id)?,None=>None})
    }).await
}
/// Whether `account` is its exchange's number 1 (None: the number 1 with no row yet).
fn number_one(c:&Connection,account:Option<i64>)->rusqlite::Result<bool>{
    let Some(id)=account else{return Ok(true)};
    Ok(c.query_row("SELECT number FROM exchange_accounts WHERE id=?1",[id],|r|r.get::<_,i64>(0)).optional()?==Some(1))
}
/// The form attributes of every card the sheet draws: answered in its frame (`modal_content`).
pub(crate) const SHEET_FORM:&str="data-controller=\"form--label-animations form--html5-validations\" data-turbo-frame=\"modal_content\"";
/// `/bots/:id`, from this request's `/bots/:id/add_api_key[/new]`.
fn bot_base(ctx:&Ctx)->String{ctx.params.route_path.trim_end_matches("/new").trim_end_matches("/add_api_key").to_string()}
/// D-BOT-KEY-SHEET: the bot page's key step ("Add new keys", the status bar's Connect) as the sheet's card with no track: the
/// venue's name a plain header (no venue step to go back to), `error` under its fields.
/// #710 7a §2: the card's action names the bot's account (`account_id`) while its exchange has 2+ live accounts.
#[allow(clippy::too_many_arguments)]
/// #710 7a rule S6: what a bot's key sheet is drawn for: its bot (the scope) and its exchange and account number (a number
/// is never reused; no row is number 1).
fn sheet_scope(ctx:&Ctx)->String{format!("bot-key-sheet:{}",ctx.params.route_path.split('/').nth(2).unwrap_or_default())}
fn sheet_binding(c:&Connection,exchange:i64,account:Option<i64>)->rusqlite::Result<String>{
    let number=match account{Some(id)=>c.query_row("SELECT number FROM exchange_accounts WHERE id=?1",[id],|r|r.get::<_,i64>(0)).optional()?.unwrap_or(0),None=>1};
    Ok(format!("{exchange}:{number}"))
}
#[allow(clippy::too_many_arguments)]
fn bot_step(c:&Connection,app:&App,ctx:&Ctx,exchange:i64,account:Option<i64>,class:&str,name:&str,live:bool,error:Option<&str>)->Result<String,WebError>{
    let instructions=key_instructions(c,app,ctx,class,name,0)?;
    let form_data=format!("{SHEET_FORM} data-action=\"turbo:submit-end-&gt;modal--base#submitEnd\"");
    // R11: a sheet names its account unless it is the exchange's sole number 1 (a nameless sheet means number 1).
    let target=match account{Some(id) if !number_one(c,Some(id))?=>format!("?account_id={id}"),_=>target_query(c,account)?.replacen('&',"?",1)};
    let csrf=ctx.csrf_token_for(&sheet_scope(ctx),&sheet_binding(c,exchange,account)?);
    let card=sheet_card(ctx,&csrf,ctx.path(&format!("{}/add_api_key{target}",bot_base(ctx))),&form_data,name,class,live,instructions,error,None)?;
    crate::web::wizard::keys::key_pane(&card)
}
pub async fn bot_form(axum::extract::State(app):axum::extract::State<App>,axum::extract::Extension(ctx):axum::extract::Extension<Ctx>)->Result<Response,WebError>{
    let Some(user)=ctx.user().cloned() else{return Ok(auth::unauthenticated(&ctx))};
    let Some(account)=owned_bot_account(&app,&ctx).await? else{return Ok(crate::web::layout::missing())};
    let inner=app.clone();let owner=user.clone();
    let v=ctx.clone();
    let (content,shell)=app.db(move|c|{
        let (class,name)=c.query_row("SELECT type,name FROM exchanges WHERE id=?1",[account.exchange_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?.unwrap_or_default())))?;
        if !ported(&class){return Ok((None,crate::web::shell::Shell::load(c,&inner,&owner)?))}
        let live=Key::read(c,&inner,account,0)?.passphrase.as_deref()==Some("live");
        Ok((Some(bot_step(c,&inner,&v,account.exchange_id,account.id,&class,&name,live,None)?),crate::web::shell::Shell::load(c,&inner,&owner)?))
    }).await?;
    let Some(content)=content else{return Ok(crate::web::layout::not_ported_for(&ctx))};
    let csrf=ctx.csrf_token();
    let body=crate::web::wizard::sheet_to(&ctx,&bot_base(&ctx),"",&content)?;
    crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]})
}

/// A correct IBKR check's status as stored: the key is bound to the account the check named, or it is incorrect when it is
/// bound to another (D-IBKR-ACCOUNT; nothing else is written). Any other venue's status as it is.
pub fn bound_status(c:&Connection,cipher:&crate::crypto::Cipher,id:i64,status:crate::engine::model::Produced<i64>,account:Option<&str>,at:chrono::DateTime<chrono::Utc>)->Result<crate::engine::model::Produced<i64>,crate::sync::SyncError>{
    let Some(account)=account else {return Ok(status)};
    match crate::venue::ibkr_account::bind(c,cipher,id,account,at).map_err(crate::sync::SyncError)? {
        Ok(())=>Ok(status),
        Err(refusal)=>{crate::engine::log(&format!("[Interactive Brokers] API key validation: {refusal}"));Ok(status.map(|_|2))}
    }
}
/// #710 slice 9 G4 (D-COINBASE-TRANSIENT-KEEPS-CORRECT): the status a check of key `id` writes. On a venue whose identity
/// read is its validation read (`identity::VALIDATION_READS`), a check with no verdict (`status` 0, pending) of a key that is
/// correct now leaves it correct, so the status change does not end its account's standings; a key not yet correct is
/// written pending, as 2.x writes it. Every other venue writes `status`.
/// Only a non-verdict the venue's identity classifier calls transient (`identity::not_refused` of the check's identity read,
/// `observed`): a definite refusal (Coinbase's 401) is written as before.
pub fn transient_kept(c:&Connection,id:i64,venue:&str,status:i64,observed:Option<&crate::identity::Observed>)->rusqlite::Result<i64>{
    if status!=0||!crate::identity::VALIDATION_READS.contains(&venue)||!crate::identity::not_refused(venue,observed){return Ok(status)}
    let now:Option<i64>=c.query_row("SELECT status FROM api_keys WHERE id=?1",[id],|r|r.get(0)).optional()?;
    Ok(if now==Some(1){1}else{status})
}
pub fn store_status(c:&crate::engine::model::FencedTransaction<'_>,status:&crate::engine::model::Produced<i64>,id:i64,at:chrono::DateTime<chrono::Utc>)->Result<(),crate::sync::SyncError>{
    if !status.current_for(&c.producer_version()).is_fresh() || status.origin().as_ref().is_none_or(|producer|producer.id!=id){return Err(crate::sync::SyncError(crate::engine::model::CREDENTIALS_CHANGED.into()));}
    crate::identity::write_status(c,id,status.value,&format_time(at),true,false)?;Ok(())
}

/// The pre-flight of a Merge (PLAN-710-6 §6.1), outside the lock: every placed standalone order of either account not yet
/// known terminal is read from its venue under its account's current trading key, by its stored route. Each one found
/// ended is recorded (`$.terminal`) in a credential-fenced write; false at the first one open or unreadable.
/// What the pre-flight found: every standalone order settled at its venue (`Clear`), one still open or unread
/// (`InFlight`), or one placed under another credential than its account's current trading key (`Unchecked`: the current
/// key may be another wallet, whose answer is never taken for it), named by its account and its order.
enum Preflight { Clear, InFlight, Unchecked(String, String) }
/// D-KEY-REPLACE-WAITS waits only on what is still open (#710 7a): before a key of (`user`, `exchange`) is replaced or
/// removed, every placed standalone order of its live accounts not yet known terminal is read as Merge's pre-flight reads
/// it, and one found ended is recorded. One still open or unread keeps the key waiting (`settling`).
/// A key save's destination snapshot (`snap`), taken BEFORE any venue read the save makes (the pre-flight's
/// `settle_standalone` here; the identity read and validation after it), so a slot another request changes meanwhile is
/// found changed under the write lock and never overwritten (#710 7a rule F4). Every key save starts here.
async fn snapshot_then_settle<T: Send + 'static>(app: &App, user: i64, exchange: i64, snap: impl FnOnce(&Connection) -> Result<T, WebError> + Send + 'static) -> Result<T, WebError> {
    let snapshot = app.db(snap).await?;
    settle_standalone(app, user, exchange).await?;
    Ok(snapshot)
}
async fn settle_standalone(app: &App, user: i64, exchange: i64) -> Result<(), WebError> {
    let accounts = app.db(move |c| {
        let mut out = vec![];
        for (id, _) in crate::identity::accounts_of(c, user, exchange)? {
            if c.query_row(&format!("SELECT EXISTS({})", crate::merge::unsettled_standalone(id)), [], |r| r.get::<_, bool>(0))? { out.push(id); }
        }
        Ok(out)
    }).await?;
    for a in accounts { merge_preflight(app, a, a).await?; }
    Ok(())
}
async fn merge_preflight(app: &App, d: i64, s: i64) -> Result<Preflight, WebError> {
    crate::web::bot::cancel::on_own_connection(app, move |c, app, runtime| {
        use crate::venue::{OrderStatus, Venue};
        let factory = crate::web::bot::cancel::live_factory(c, app);
        let mut q = c.prepare(&format!("SELECT o.id, x.account_id, x.external_id, x.identity, x.order_snapshot, o.credential_version, o.created_at, o.side, o.amount \
                                         FROM rust_api_orders o JOIN rust_api_order_standalone x ON x.order_id = o.id \
                                         WHERE o.id IN ({} UNION {}) ORDER BY o.id", crate::merge::unsettled_standalone(d), crate::merge::unsettled_standalone(s)))?;
        type Row = (i64, i64, Option<String>, Option<String>, Option<String>, Option<String>, String, String, String);
        let orders: Vec<Row> = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)))?.collect::<Result<_, _>>()?;
        drop(q);
        for (id, account_id, external, identity, snapshot, placed_under, placed_at, side, amount) in orders {
            let date = placed_at.chars().take(10).collect::<String>();
            let (Some(external), Some(account)) = (external, Account::by_id(c, account_id)?) else { return Ok(Preflight::InFlight) };
            // The cancel's route (bot_api::orders): the identity and snapshot stored at the claim.
            let parse = |t: Option<String>| t.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
            let mut intent = parse(identity).unwrap_or_else(|| serde_json::json!({}));
            let snapshot = parse(snapshot).unwrap_or_default();
            for field in ["symbol", "side", "limit"] { intent[field] = snapshot[field].clone(); }
            let order = crate::venue::OrderRef { external_id: external, record: crate::venue::kucoin::record_of(&intent) };
            let venue = crate::engine::run::venue_for_key(c, &factory, &app.cipher, account)?;
            // The handle's producer: the trading key's version the read is made under, which the record is fenced by.
            let Some(version) = venue.producer() else { return Ok(Preflight::InFlight) };
            // Only the credential it was placed under can speak for it.
            // Its stamp names this key, and the key's material has not changed since it was placed (a re-encryption of the
            // same material changes the stamp only: `identity::write_credential` records a real change).
            let placed = parse(placed_under).and_then(|v| crate::engine::model::CredentialVersion::from_stamp(&v));
            let changed: Option<String> = crate::app_config::get_plain(c, &crate::identity::credential_changed_key(version.id)).map_err(WebError::Config)?;
            let same = placed.as_ref().is_some_and(|p| p.cache_stamp() == version.cache_stamp()
                || (p.id == version.id && changed.as_deref().is_none_or(|t| t < placed_at.as_str())));
            if !same {
                return Ok(Preflight::Unchecked(crate::identity::label(c, account_id)?.plain(), format!("{date} {side} {amount}")));
            }
            let version = Some(version);
            let read = runtime.block_on(tokio::time::timeout(crate::venue::http::TOTAL_TIMEOUT, venue.order_with(&order, |_, _, _| Ok(true))));
            // The poll's reading (`OrderState::settled`): a KuCoin zero-fill answer old enough is final.
            let status = match read { Ok(Ok((states, _))) => states.first().map(|o| o.clone().settled(app.now()).status), _ => None };
            let terminal = match status { Some(OrderStatus::Closed) => "filled", Some(OrderStatus::Cancelled) => "cancelled", Some(OrderStatus::Failed) => "rejected", _ => return Ok(Preflight::InFlight) };
            match crate::engine::model::credential_write(c, &version, |tx| Ok(crate::merge::record_terminal(tx, id, terminal)?)) {
                Ok(_) => {}
                Err(crate::engine::EngineError::CredentialsChanged) => return Ok(Preflight::InFlight),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Preflight::Clear)
    }).await
}

/// Settings' Merge (PLAN-710-6 §6.1): duplicate `<id>` joins its survivor (`merge::run`, one transaction on the engine's
/// write path), answered as a delete: the key list replaced and a flash, or a 422 flash naming why nothing changed.
pub async fn merge(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = ctx.params.route_path.rsplit('/').next().and_then(|s| s.parse::<i64>().ok()) else { return Ok(crate::web::layout::missing()) };
    let user_id = user.id;
    // The survivor the confirmation named: another one derived by now is no Merge the user confirmed.
    let Some(posted) = ctx.params.form("survivor_id").and_then(|s| s.parse::<i64>().ok()) else { return Ok(crate::web::layout::missing()) };
    let names = app.db(move |c| {
        let Some((d, s)) = crate::identity::duplicates(c, user_id)?.into_iter().find(|(d, _)| *d == id) else { return Ok(None) };
        Ok(Some((d, s, crate::identity::label(c, d)?.plain(), crate::identity::label(c, s)?.plain())))
    }).await?;
    let Some((d, s, duplicate, survivor)) = names else { return Ok(crate::web::layout::missing()) };
    if s != posted { return Ok(crate::web::layout::missing()); }
    fn args<'a>(duplicate: &'a str, survivor: &'a str, rows: Option<&'a str>) -> Vec<(&'static str, i18n::Arg<'a>)> {
        let mut out = vec![("duplicate", i18n::Arg::Text(duplicate)), ("survivor", i18n::Arg::Text(survivor))];
        if let Some(rows) = rows { out.push(("rows", i18n::Arg::Text(rows))); }
        out
    }
    let refused = |ctx: &Ctx, text: String| -> Result<Response, WebError> {
        Ok((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
            turbo::prepend_flash(flash::render(&flash::take(&ctx.session, &[(flash::ALERT, text)]))?.trim_end())).into_response())
    };
    match merge_preflight(&app, d, s).await? {
        Preflight::Clear => {}
        Preflight::InFlight => return refused(&ctx, i18n::t(ctx.locale, "settings.accounts.merge_in_flight_html", &args(&duplicate, &survivor, None))),
        Preflight::Unchecked(account, order) => return refused(&ctx, i18n::t(ctx.locale, "settings.accounts.merge_order_unchecked_html",
                                                                             &[("account", i18n::Arg::Text(&account)), ("rows", i18n::Arg::Text(&order))])),
    }
    let inner = app.clone();
    let v = ctx.clone();
    let csrf = ctx.csrf_token();
    enum Done { Missing, Merged(crate::merge::Merged, String, Vec<(i64, bool, bool)>, (String, String)), InFlight, Stale(Vec<(&'static str, i64)>), WithdrawalKeys(Vec<String>), Unreadable(String),
                Conflict(Vec<String>), Refused(String) }
    let done = app.db(move |c| {
        let tx = crate::engine::model::immediate(c)?;
        // The names as they read in the transaction, before the duplicate is removed.
        let labels = (crate::identity::label(&tx, id)?.plain(), crate::identity::label(&tx, posted)?.plain());
        let merged = match crate::merge::run(&tx, &inner.cipher, user_id, id, posted, inner.now()) {
            Ok(None) => return Ok(Done::Missing),
            Ok(Some(merged)) => merged,
            Err(crate::merge::Refusal::InFlight) => return Ok(Done::InFlight),
            Err(crate::merge::Refusal::Stale(wake)) => return Ok(Done::Stale(wake)),
            Err(crate::merge::Refusal::WithdrawalKeys(rules)) => return Ok(Done::WithdrawalKeys(rules)),
            Err(crate::merge::Refusal::Unreadable(account)) => return Ok(Done::Unreadable(crate::identity::label(&tx, account)?.plain())),
            Err(crate::merge::Refusal::Conflict(rows)) => return Ok(Done::Conflict(rows)),
            Err(crate::merge::Refusal::Db(e)) => return Err(e.into()),
        };
        // Only the merging user's bots and rules decide: another user's neither block a Merge nor reach its answer.
        if let Err(refusal) = eligibility::guard_user(&tx, &inner.cipher, user_id) { return Ok(Done::Refused(refusal.reason())); }
        let body = list(&tx, &v, &csrf, user_id)?;
        // Only the keys the scheduler reads with (a trading key before a read-only one): a sync of another key would stamp
        // the account's balances as a producer the freshness check does not expect.
        let ledger = crate::sync::ledger_reading_keys(&tx).map_err(|e| WebError::Config(e.0))?;
        let balances = crate::sync::balance_reading_keys(&tx).map_err(|e| WebError::Config(e.0))?;
        let wakes: Vec<(i64, bool, bool)> = merged.kept_keys.iter().map(|k| (*k, ledger.contains(k), balances.contains(k))).collect();
        tx.commit()?;
        Ok(Done::Merged(merged, body, wakes, labels))
    }).await?;
    match done {
        Done::Missing => Ok(crate::web::layout::missing()),
        Done::InFlight => refused(&ctx, i18n::t(ctx.locale, "settings.accounts.merge_in_flight_html", &args(&duplicate, &survivor, None))),
        Done::Stale(wake) => {
            // The live check: the syncs that read both accounts again, then the user tries again.
            let jobs = app.job_wakers()?;
            for (job, key) in wake { jobs.wake(job, Some(&key.to_string()), None); }
            refused(&ctx, i18n::t(ctx.locale, "settings.accounts.merge_checking_html", &args(&duplicate, &survivor, None)))
        }
        Done::WithdrawalKeys(rules) => {
            let mut a = args(&duplicate, &survivor, None);
            let rules = rules.join("; ");
            a.push(("rules", i18n::Arg::Text(&rules)));
            refused(&ctx, i18n::t(ctx.locale, "settings.accounts.merge_withdrawal_key_html", &a))
        }
        Done::Unreadable(account) => refused(&ctx, i18n::t(ctx.locale, "settings.accounts.merge_unreadable_html", &[("account", i18n::Arg::Text(&account))])),
        Done::Conflict(rows) => {
            // Another user's order, in the user's words (`merge::FOREIGN_ORDER`).
            let rows: Vec<String> = rows.into_iter().map(|r| if r == crate::merge::FOREIGN_ORDER { i18n::t(ctx.locale, "settings.accounts.merge_foreign_order", &[]) }
                else if let Some(account) = r.strip_prefix(crate::merge::KUCOIN_IMPORT) { i18n::t(ctx.locale, "settings.accounts.merge_kucoin_import", &[("account", i18n::Arg::Text(account))]) } else { r }).collect();
            refused(&ctx, i18n::t(ctx.locale, "settings.accounts.merge_conflict_html", &args(&duplicate, &survivor, Some(&rows.join("; ")))))
        }
        Done::Refused(reason) => refused(&ctx, refusal_text(&ctx, &reason)),
        Done::Merged(merged, body, wakes, (duplicate, survivor)) => {
            for key in &merged.deleted_keys { crate::venue::ibkr::forget(*key); }
            app.wake_engine();
            let jobs = app.job_wakers()?;
            for (key, ledger, balances) in wakes {
                if ledger { jobs.wake(crate::sync::jobs::LEDGER_SYNC, Some(&key.to_string()), None); }
                if balances { jobs.wake(crate::sync::jobs::BALANCE_SYNC, Some(&key.to_string()), None); }
            }
            for job in [crate::tracker::jobs::TRACKER_LEDGER, crate::tracker::jobs::PORTFOLIO_BACKFILL] { jobs.wake(job, Some(&user_id.to_string()), None); }
            let text = i18n::t(ctx.locale, "settings.accounts.merged_html", &args(&duplicate, &survivor, None));
            Ok(([(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
                format!("{}{}", turbo::stream("replace", "api_keys", &body), turbo::prepend_flash(flash::render(&[flash::Message { style: "primary", text }])?.trim_end()))).into_response())
        }
    }
}

#[cfg(test)]
mod validation_producer_tests {
    use super::*;
    #[test]
    fn r4_validation_status_consumer_requires_bound_matching_reply(){
        use crate::engine::model::{self,Produced};
        let mut c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE api_keys(id INTEGER PRIMARY KEY,key TEXT,secret TEXT,passphrase TEXT,access_token TEXT,rsa_signature_key TEXT,rsa_encryption_key TEXT,dh_param TEXT,status INTEGER,updated_at TEXT); INSERT INTO api_keys VALUES(1,'key-a','secret-a','paper',NULL,NULL,NULL,NULL,0,'2026-10-01');").unwrap();
        let current=model::credential_version_by_id(&c,1).unwrap().unwrap();
        let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        {
            let fenced=model::check_credential_result(&tx,&Some(current.clone())).unwrap();
            let unbound=Produced::new(1,None);
            assert!(store_status(&fenced,&unbound,1,chrono::Utc::now()).is_err(),"R4 status consumer rejects missing producer");
            let different=model::prepared_credential_version(1,&[Some("key-b".into()),Some("secret-b".into()),Some("paper".into()),None,None,None,None]);
            let foreign=Produced::new(1,Some(different));
            assert!(store_status(&fenced,&foreign,1,chrono::Utc::now()).is_err(),"R4 status consumer rejects another producer under the same key ID");
            assert_eq!(tx.query_row("SELECT status FROM api_keys WHERE id=1",[],|r|r.get::<_,i64>(0)).unwrap(),0,"refused results change no rows");
            let healthy=Produced::new(1,Some(current));
            assert!(store_status(&fenced,&healthy,1,chrono::Utc::now()).is_ok(),"R4 healthy status result publishes normally");
        }
        tx.commit().unwrap();
        assert_eq!(c.query_row("SELECT status FROM api_keys WHERE id=1",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
}
