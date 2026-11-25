//! The IBKR connection (Settings::IbkrConnectionsController with settings/ibkr_connections/): its states as one component
//! (`wizard`), served by `/settings/ibkr_connect` and its downloads, activation and disconnect.
//!
//! Divergences:
//! - D-IBKR-DHGROUP: a new connection's keys are made inline when it is created (two RSA-2048 keys and the RFC 7919
//!   ffdhe2048 group), so the page goes straight to the downloads; 2.x generates a DH prime in a job and shows
//!   "generating" until it has run.
//! - D-IBKR-RECONNECT: a key whose stored keys make no session (none yet: 2.x's job never ran; or material this build
//!   cannot read) offers Disconnect only, then a fresh connect. Nothing completes or converts it.
//! - D-IBKR-NOECHO: the credentials form is always empty. 2.x's `form_with scope: :api_key` fills it from `@api_key`,
//!   writing the stored consumer key, access token and its secret into the page.
use crate::identity::Account;
use super::keys::{self, Key, Submitted};
use crate::venue::signers::ibkr as signer;
use crate::web::{auth, flash, i18n, layout::{self, Ctx, Page}, shell, App, WebError};
use askama::Template;
use axum::{http::{header, StatusCode}, response::{IntoResponse, Response}};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

pub const PATH: &str = "/settings/ibkr_connect";
/// Exchanges::Ibkr::OAUTH_PORTALS (recommended, alternative) and OAUTH_PORTAL_PATH.
const PORTALS: [&str; 2] = ["ndcdyn.interactivebrokers.com", "www.interactivebrokers.co.uk"];
const PORTAL_PATH: &str = "/oauth/?loginType=1&action=OAUTH&clt=0&RL=1#/configuration";
/// ApiKey::ACTIVATION_DEADLINE.
const ACTIVATION_DEADLINE_DAYS: i64 = 14;
/// D-IBKR-RECONNECT's line (English only, as the rest of the IBKR copy).
const UNUSABLE: &str = "These keys were set up by an earlier version and can't be used now. Disconnect, then connect again to make new ones.";

/// The state the wizard renders, from the user's IBKR key (`_wizard.html.erb`'s branches, in its order). #710 7b:
/// `Fenced`, a correct key whose login opens another account now (D-IBKR-FENCE): its credentials again, on its own keys.
pub enum State { None, Connected, Unusable, Stalled(String), Pending, Downloads, Fenced }

/// The user's IBKR key as the wizard reads it. #710 7b: or a pending connection (`registration`: id and revision), whose
/// material lives in `ibkr_registrations` until IBKR names its account (`id` 0 then).
pub struct Stored { pub id: i64, pub exchange: i64, status: i64, updated_at: String, material: [Option<String>; 3],
                    account: Option<i64>, registration: Option<(i64, i64)>, fenced: bool }

/// #710 7b rule 11: what a request names. `Legacy`: neither (2.x's unqualified forms, the exchange's number 1); `Account`:
/// an account's own connection (`account_id`); `Registration`: a pending connection (`registration`); `Fresh`: a venue tile
/// (the tracker's untargeted pick), the user's open untargeted registration or a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target { Legacy, Account(i64), Registration(i64), Fresh }

/// The target a request names (query or form). None: a malformed id (a 404).
fn target(ctx: &Ctx) -> Option<Target> {
    let field = |name: &str| ctx.params.form(name).or_else(|| ctx.params.query(name)).filter(|v| !v.is_empty());
    match (field("account_id"), field("registration")) {
        (Some(a), _) => a.parse().ok().map(Target::Account),
        (None, Some(r)) if r == "new" => Some(Target::Fresh),
        (None, Some(r)) => r.parse().ok().map(Target::Registration),
        (None, None) => Some(Target::Legacy),
    }
}

impl Stored {
    fn usable(&self) -> bool {
        match &self.material { [Some(signing), Some(encryption), Some(dh)] => signer::usable(signing, encryption, dh), _ => false }
    }
}

/// `Exchanges::Ibkr.first`.
fn exchange(c: &Connection) -> Result<Option<i64>, WebError> {
    Ok(c.query_row("SELECT id FROM exchanges WHERE type = ?1 ORDER BY id LIMIT 1", [crate::venue::ibkr::IBKR], |r| r.get(0)).optional()?)
}

/// `current_ibkr_key`: the user's trading key on the first IBKR exchange (its account, #710: the exchange's while it has one).
pub fn stored(c: &Connection, app: &App, user: i64) -> Result<Option<Stored>, WebError> {
    let Some(exchange) = exchange(c)? else { return Ok(None) };
    stored_on(c, app, user, exchange, Account::first(c, user, exchange)?, false)
}
/// `account`'s IBKR trading key; `qualified`: the request named the account (its forms carry it on).
fn stored_on(c: &Connection, app: &App, _user: i64, exchange: i64, account: Account, qualified: bool) -> Result<Option<Stored>, WebError> {
    let row = c.query_row(&format!("SELECT id, status, updated_at, rsa_signature_key, rsa_encryption_key, dh_param FROM api_keys k WHERE {} AND key_type = 0 ORDER BY id LIMIT 1", account.keys("k")),
                          [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, [r.get::<_, Option<String>>(3)?, r.get(4)?, r.get(5)?]))).optional()?;
    let Some((id, status, updated_at, encrypted)) = row else { return Ok(None) };
    let fenced: bool = c.query_row(&format!("SELECT {} FROM api_keys k WHERE k.id = ?1", crate::identity::fenced_sql(c, "k")?), [id], |r| r.get(0))?;
    Ok(Some(Stored { id, exchange, status, updated_at, material: opened(app, encrypted), account: account.id.filter(|_| qualified), registration: None, fenced }))
}

/// A stored key's three artifacts, decrypted. A blank column is no material (`.blank?`); an unreadable one is
/// D-IBKR-RECONNECT's rejected material.
fn opened(app: &App, encrypted: [Option<String>; 3]) -> [Option<String>; 3] {
    encrypted.map(|value| value.and_then(|v| app.cipher.decrypt(&v).ok()).filter(|v| !crate::v2::blank(v)))
}

/// #710 7b: what `target` names for `user`. Ok(None): nothing to show (no key, no registration yet); Err: a 404 (an
/// account or a registration that is not the user's).
pub fn stored_for(c: &Connection, app: &App, user: i64, target: Target) -> Result<Result<Option<Stored>, ()>, WebError> {
    let Some(exchange) = exchange(c)? else { return Ok(Ok(None)) };
    match target {
        Target::Legacy => Ok(Ok(stored(c, app, user)?)),
        Target::Account(id) => {
            if !crate::identity::accounts_of(c, user, exchange)?.iter().any(|(a, _)| *a == id) { return Ok(Err(())); }
            let account = Account::by_id(c, id)?.ok_or_else(|| WebError::Config("account missing".into()))?;
            Ok(Ok(stored_on(c, app, user, exchange, account, true)?))
        }
        Target::Registration(id) => match registration(c, app, user, Some(id))? { Some(r) => Ok(Ok(Some(r))), None => Ok(Err(())) },
        Target::Fresh => Ok(Ok(registration(c, app, user, None)?)),
    }
}

/// Registration `id` of `user` (None: the user's open untargeted one), as the wizard reads a key.
fn registration(c: &Connection, app: &App, user: i64, id: Option<i64>) -> Result<Option<Stored>, WebError> {
    let row = c.query_row("SELECT id, exchange_id, revision, status, updated_at, rsa_signature_key, rsa_encryption_key, dh_param, target_account_id FROM ibkr_registrations
                           WHERE user_id = ?1 AND (id = ?2 OR (?2 IS NULL AND target_account_id IS NULL)) ORDER BY id DESC LIMIT 1",
                          rusqlite::params![user, id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, String>(4)?,
                                                               [r.get::<_, Option<String>>(5)?, r.get(6)?, r.get(7)?], r.get::<_, Option<i64>>(8)?))).optional()?;
    // A registration started from an account (a reconnect) is targeted at it, if it is still live.
    let live = |a: Option<i64>, exchange: i64| -> Result<Option<i64>, WebError> { Ok(a.filter(|a| crate::identity::accounts_of(c, user, exchange).map(|l| l.iter().any(|(x, _)| x == a)).unwrap_or(false))) };
    let row = match row { Some((id, exchange, revision, status, updated_at, encrypted, target)) => Some((id, exchange, revision, status, updated_at, encrypted, live(target, exchange)?)), None => None };
    Ok(row.map(|(id, exchange, revision, status, updated_at, encrypted, target)| Stored { id: 0, exchange, status, updated_at, material: opened(app, encrypted), account: target,
                                                                               registration: Some((id, revision)), fenced: false }))
}

/// #710 7b §2 (the legacy safe spot): whether a new connection is today's key row on number 1: the exchange has at most one
/// live account, no IBKR key, no proven IBKR identity and no IBKR history evidence. Otherwise it is a registration.
fn legacy_spot(c: &Connection, user: i64, exchange: i64) -> Result<bool, WebError> {
    let accounts = crate::identity::accounts_of(c, user, exchange)?;
    if accounts.len() > 1 { return Ok(false); }
    let keys: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2)", [user, exchange], |r| r.get(0))?;
    if keys { return Ok(false); }
    for (a, _) in accounts {
        let fingerprinted: bool = c.query_row("SELECT fingerprint IS NOT NULL FROM exchange_accounts WHERE id = ?1", [a], |r| r.get(0))?;
        if (fingerprinted && crate::identity::proven(c, a)?) || !crate::venue::ibkr_account::evidence(c, a)?.is_empty() || crate::venue::ibkr_account::takeover_held(c, a)? { return Ok(false); }
    }
    Ok(true)
}

/// The wizard's state for `key` at `ctx.now`.
pub fn state(ctx: &Ctx, key: Option<&Stored>) -> State {
    let Some(key) = key else { return State::None };
    if !key.usable() { return State::Unusable; }
    match key.status {
        1 if key.fenced => State::Fenced,
        1 => State::Connected,
        3 => match crate::codec::parse_time(&key.updated_at) {
            Ok(at) if at <= ctx.now - chrono::Duration::days(ACTIVATION_DEADLINE_DAYS) => {
                let duration = crate::web::bot::signal::ago(at, ctx.now, ctx.locale);
                State::Stalled(i18n::t(ctx.locale, "settings.ibkr.activation_failed", &[("duration", i18n::Arg::Text(&duration))]))
            }
            _ => State::Pending,
        },
        _ => State::Downloads,
    }
}

#[derive(Template)]
#[template(path = "settings/ibkr_wizard.html")]
struct Wizard<'a> { v: &'a Ctx, csrf: &'a str, state: &'a State, path: String, check_address: String, recommended_url: String, alternative_url: String,
                    hidden: String, also: String, slot: bool }

/// #710 7b rule 11: what a qualified connection's forms and links carry on (`hidden` inputs, the downloads' `also` query),
/// and whether its credentials form has the question slot. All empty for the unqualified page (its bytes are 2.x's).
fn carried(key: Option<&Stored>, state: &State) -> (String, String, bool) {
    let named = match key { Some(Stored { registration: Some((id, revision)), .. }) => Some(vec![("registration", id.to_string()), ("revision", revision.to_string())]),
                            Some(Stored { account: Some(a), .. }) => Some(vec![("account_id", a.to_string())]), _ => None };
    let Some(named) = named else { return (String::new(), String::new(), matches!(state, State::Fenced)) };
    (named.iter().map(|(k, v)| format!("<input type=\"hidden\" name=\"{k}\" value=\"{v}\" />")).collect(),
     named.iter().filter(|(k, _)| *k != "revision").map(|(k, v)| format!("&amp;{k}={v}")).collect(), true)
}

/// The connection component: `#ibkr-connect-wizard` in `state`, its forms posting to the connect page's routes.
pub fn wizard(ctx: &Ctx, csrf: &str, state: &State) -> Result<String, WebError> { wizard_of(ctx, csrf, None, state) }
fn wizard_of(ctx: &Ctx, csrf: &str, key: Option<&Stored>, state: &State) -> Result<String, WebError> {
    let check_address = i18n::t(ctx.locale, "settings.ibkr.portal_step_check_address", &[("recommended", i18n::Arg::Text(PORTALS[0])), ("alternative", i18n::Arg::Text(PORTALS[1]))]);
    let url = |host: &str| format!("https://{host}{PORTAL_PATH}");
    let (hidden, also, slot) = carried(key, state);
    Ok(Wizard { v: ctx, csrf, state, path: ctx.path(PATH), check_address, recommended_url: url(PORTALS[0]), alternative_url: url(PORTALS[1]), hidden, also, slot }.render()?)
}

#[derive(Template)]
#[template(path = "settings/ibkr_sheet.html")]
struct Sheet<'a> { v: &'a Ctx, csrf: &'a str, state: &'a State, path: String, check_address: String, recommended_url: String, alternative_url: String,
                   back: String, mark: &'static str, notice: Option<(bool, String)>, hidden: String, also: String, slot: bool }

/// D-TRACKER-IBKR-SHEET: the tracker's IBKR key step — the track (IBKR done, Back to the venue step; Connect current) and the
/// connection as the sheet's card, `notice` (alert?, text) inside it. None when IBKR is not offered here (`gated`).
pub fn tracker_step(c: &Connection, app: &App, ctx: &Ctx, user: i64, pick: crate::web::wizard::Picked, notice: Option<(bool, String)>) -> Result<Option<(String, String)>, WebError> {
    use crate::web::wizard::Picked;
    // #710 7b: a venue tile while an IBKR account is connected opens a fresh connection; an account's link, its own.
    let target = match (pick, ctx.params.query("registration").and_then(|r| r.parse().ok())) {
        (_, Some(r)) => Target::Registration(r),
        (Picked::Account(a), None) => Target::Account(a),
        (Picked::Untargeted, None) if exchange(c)?.is_some_and(|e| !legacy_spot(c, user, e).unwrap_or(true)) => Target::Fresh,
        _ => Target::Legacy,
    };
    let key = match stored_for(c, app, user, target)? { Ok(key) => key, Err(()) => return Ok(None) };
    if gated(app, key.as_ref()) && target == Target::Legacy { return Ok(None) }
    let state = state(ctx, key.as_ref());
    let (hidden, also, slot) = carried(key.as_ref(), &state);
    let hidden = if target == Target::Fresh && key.is_none() { "<input type=\"hidden\" name=\"registration\" value=\"new\" />".to_string() } else { hidden };
    let csrf = ctx.csrf_token();
    let check_address = i18n::t(ctx.locale, "settings.ibkr.portal_step_check_address", &[("recommended", i18n::Arg::Text(PORTALS[0])), ("alternative", i18n::Arg::Text(PORTALS[1]))]);
    let url = |host: &str| format!("https://{host}{PORTAL_PATH}");
    let back = ctx.path("/tracker/pick_exchange/new");
    let card = Sheet { v: ctx, csrf: &csrf, state: &state, path: ctx.path(PATH), check_address, recommended_url: url(PORTALS[0]), alternative_url: url(PORTALS[1]),
                       back: back.clone(), mark: crate::web::bot::exchange_svg("ibkr"), notice, hidden, also, slot }.render()?;
    let track = crate::web::wizard::track(ctx, 1, vec![crate::web::wizard::Stop::new("exchange").name(Some("Interactive Brokers".into())).back(back), crate::web::wizard::Stop::new("connect")])?;
    Ok(Some((track, crate::web::wizard::keys::key_pane(&card)?)))
}

/// A write posted from the tracker's sheet (`sheet=tracker`).
fn from_sheet(ctx: &Ctx) -> bool { ctx.params.form("sheet") == Some("tracker") }

/// Where a write goes on: the connect page, or (from the sheet) the tracker's IBKR step, in the sheet's frame — never a page
/// without that frame. With no exchange known (the key gone meanwhile) the step the session's tracker venue names.
fn back_to(ctx: &Ctx, exchange: Option<i64>) -> Response { back_to_kept(ctx, exchange, &kept(ctx)) }
/// `back_to`, keeping `keep` (the account or registration the request named: rule 11).
fn back_to_kept(ctx: &Ctx, exchange: Option<i64>, keep: &str) -> Response {
    if !from_sheet(ctx) { return layout::redirect(StatusCode::FOUND, &ctx.path(&format!("{PATH}{}", keep.replacen('&', "?", 1)))) }
    let query = exchange.map(|id| format!("?exchange_id={id}{keep}")).unwrap_or_default();
    layout::redirect(StatusCode::FOUND, &ctx.path(&format!("/tracker/add_api_key/new{query}")))
}
/// The query that keeps what the request named (`&account_id=…` or `&registration=…`; empty when it named neither).
fn kept(ctx: &Ctx) -> String { keep_of(target(ctx).unwrap_or(Target::Legacy)) }
fn keep_of(target: Target) -> String {
    match target { Target::Account(a) => format!("&account_id={a}"), Target::Registration(r) => format!("&registration={r}"), Target::Fresh | Target::Legacy => String::new() }
}

/// From the sheet, a write that finished the connection (connected, or disconnected): the tracker's completion, as a tracker
/// key's save — its warnings rebuilt, the tracker visited (closing the sheet) with the flash.
async fn tracker_done(app: &App, ctx: &Ctx, user: i64) -> Result<Response, WebError> {
    let v = ctx.clone();
    let warnings = app.db(move |c| keys::warnings(c, &v, user)).await?;
    let body = crate::web::turbo::stream("update", "sync-warnings", &warnings) + &crate::web::turbo::redirect(&ctx.path("/tracker"));
    Ok(([(header::CONTENT_TYPE, crate::web::turbo::CONTENT_TYPE)], body).into_response())
}

/// `require_ibkr_available!`: the data API's container (StockTradingSettings.ibkr_available?), or a key there already is.
fn gated(app: &App, key: Option<&Stored>) -> bool { !app.config.market_data_url && key.is_none() }

fn back(ctx: &Ctx) -> Response { layout::redirect(StatusCode::FOUND, &ctx.path(PATH)) }
fn gate(ctx: &Ctx) -> Response { if from_sheet(ctx) { back_to(ctx, None) } else { layout::redirect(StatusCode::FOUND, &ctx.path("/settings/connect")) } }

/// GET /settings/ibkr_connect (#710 7b: `account_id` an account's connection, `registration` a pending one).
pub async fn show(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(target) = target(&ctx) else { return Ok(layout::missing()) };
    let (inner, owner) = (app.clone(), user.id);
    let (key, shell) = app.db(move |c| Ok((stored_for(c, &inner, user.id, target)?, shell::Shell::load(c, &inner, &user)?))).await?;
    let Ok(key) = key else { return Ok(layout::missing()) };
    if gated(&app, key.as_ref()) { return Ok(gate(&ctx)); }
    let csrf = ctx.csrf_token();
    let state = state(&ctx, key.as_ref());
    let body = format!("<div class=\"page-head\">\n  <div class=\"stitle\">{}</div>\n</div>\n\n{}\n\n<div class=\"columns\">\n  <div class=\"column gap-1\">\n    {}  </div>\n</div>\n",
                       ctx.t("settings.ibkr.title"), ctx.stream_from(&format!("user_{owner}:ibkr_connection")), wizard_of(&ctx, &csrf, key.as_ref(), &state)?);
    let user = ctx.user().ok_or_else(|| WebError::Config("missing page owner".into()))?;
    shell::application(&ctx, &csrf, user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] })
}

/// GET /settings/ibkr_connect/download?artifact=…: a public half of the key's material, never a private key.
pub async fn download(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(target) = target(&ctx) else { return Ok(layout::missing()) };
    let inner = app.clone();
    let Ok(key) = app.db(move |c| stored_for(c, &inner, user.id, target)).await? else { return Ok(layout::missing()) };
    if gated(&app, key.as_ref()) { return Ok(gate(&ctx)); }
    let artifact = ctx.params.query("artifact").unwrap_or_default().to_string();
    let pem = key.and_then(|key| {
        let [signing, encryption, dh] = key.material;
        match artifact.as_str() {
            "signing" => signing.map(|pem| signer::public_pem(&pem)),
            "encryption" => encryption.map(|pem| signer::public_pem(&pem)),
            "dhparam" => dh.map(Some),
            _ => None,
        }
    });
    match pem {
        None => Ok(back_to(&ctx, None)),
        // 2.x raises reading a stored key OpenSSL cannot parse.
        Some(None) => Err(WebError::Config("unreadable IBKR key".into())),
        Some(Some(pem)) => Ok(([(header::CONTENT_TYPE, "application/x-pem-file".to_string()),
                                (header::CONTENT_DISPOSITION, format!("attachment; filename=\"ibkr_{artifact}.pem\"; filename*=UTF-8''ibkr_{artifact}.pem"))], pem).into_response()),
    }
}

/// POST /settings/ibkr_connect: the user's IBKR key, made with its keys when there is none (D-IBKR-DHGROUP). A key there
/// already is stays as it is, whatever its state (D-IBKR-RECONNECT). #710 7b (D-IBKR-REGISTRATION): a new connection
/// beside an IBKR account (or with `registration=new`) is a registration, no key row, until IBKR names its account; one
/// open untargeted registration per user (and one per target account) is kept as it is.
pub async fn create(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(target) = target(&ctx) else { return Ok(layout::missing()) };
    let inner = app.clone();
    let (key, exchange) = app.db(move |c| Ok((stored_for(c, &inner, user.id, target)?, exchange(c)?))).await?;
    let Ok(key) = key else { return Ok(layout::missing()) };
    if gated(&app, key.as_ref()) && target == Target::Legacy { return Ok(gate(&ctx)); }
    let (None, Some(exchange)) = (key, exchange) else { return Ok(back_to(&ctx, exchange)) };
    // Ibkr::GenerateConnectionKeysJob's artifacts: RSA generation takes a moment, so off the async workers.
    let material = tokio::task::spawn_blocking(|| [signer::generate_rsa().0, signer::generate_rsa().0, signer::ffdhe2048_pem()])
        .await.map_err(|_| WebError::Config("IBKR key generation failed".into()))?;
    let inner = app.clone();
    let made = app.db(move |c| {
        let tx = rusqlite::Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        let now = crate::codec::format_time(inner.now());
        let [signing, encryption, dh] = material.map(|pem| inner.cipher.encrypt(&pem));
        let targeted = match target { Target::Account(a) => Some(a), _ => None };
        // Rechecked in the transaction: the safe spot is today's key row on number 1.
        if target == Target::Legacy && legacy_spot(&tx, user.id, exchange)? {
            // The account's row first, then find_or_create_by! on it: a key made meanwhile is the one kept.
            let account = Account::first(&tx, user.id, exchange)?.ensure(&tx, inner.now())?;
            if account.key(&tx, 0)?.is_none() {
                tx.execute("INSERT INTO api_keys(user_id, exchange_id, key_type, status, rsa_signature_key, rsa_encryption_key, dh_param, created_at, updated_at) VALUES(?1, ?2, 0, 0, ?3, ?4, ?5, ?6, ?6)",
                           rusqlite::params![user.id, exchange, signing, encryption, dh, now])?;
                account.add_key(&tx, tx.last_insert_rowid())?;
            }
            tx.commit()?;
            return Ok(Target::Legacy);
        }
        let open: Option<i64> = tx.query_row("SELECT id FROM ibkr_registrations WHERE user_id = ?1 AND target_account_id IS ?2 ORDER BY id DESC LIMIT 1",
                                             rusqlite::params![user.id, targeted], |r| r.get(0)).optional()?;
        let id = match open {
            Some(id) => id,
            None => {
                tx.execute("INSERT INTO ibkr_registrations(user_id, exchange_id, target_account_id, revision, rsa_signature_key, rsa_encryption_key, dh_param, status, created_at, updated_at)
                            VALUES(?1, ?2, ?3, 1, ?4, ?5, ?6, 0, ?7, ?7)", rusqlite::params![user.id, exchange, targeted, signing, encryption, dh, now])?;
                tx.last_insert_rowid()
            }
        };
        tx.commit()?;
        Ok(Target::Registration(id))
    }).await?;
    Ok(back_to_kept(&ctx, Some(exchange), &keep_of(made)))
}

/// POST /settings/ibkr_connect/activate: the pasted consumer key, access token and secret, validated with the stored keys
/// (ApiKey#validate_credentials!, `keys::submit_answered`), and the flash its answer earns. #710 7b: routed as any
/// identity venue's key (rule 1); a question (rule 2) is answered in the form's slot; a registration becomes the key of
/// the account IBKR names, or waits for IBKR's activation (rule 3).
pub async fn activate(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(target) = target(&ctx) else { return Ok(layout::missing()) };
    let inner = app.clone();
    let found = app.db(move |c| {
        let key = match stored_for(c, &inner, user.id, target)? { Ok(Some(key)) => key, Ok(None) => return Ok(Ok(None)), Err(()) => return Ok(Err(())) };
        let original = match (key.registration, key.account) {
            (Some((id, revision)), target) => {
                let base = match target { Some(a) => Account::by_id(c, a)?.ok_or_else(|| WebError::Config("account missing".into()))?, None => Account::first(c, user.id, key.exchange)? };
                keys::Key::of_registration(c, &inner, base, id, revision, target.is_none())?
            }
            (None, Some(a)) => Key::read(c, &inner, Account::by_id(c, a)?.ok_or_else(|| WebError::Config("account missing".into()))?, 0)?,
            (None, None) => Key::read(c, &inner, Account::first(c, user.id, key.exchange)?, 0)?,
        };
        Ok(Ok(Some((key, original))))
    }).await?;
    let Ok(found) = found else { return Ok(layout::missing()) };
    if gated(&app, found.as_ref().map(|(key, _)| key)) && target == Target::Legacy { return Ok(gate(&ctx)); }
    let Some((key, original)) = found else { return Ok(back_to(&ctx, None)) };
    // #710 7b rule 3: a stale registration form (another write moved it) is asked again.
    if let (Some((_, revision)), Some(posted)) = (key.registration, ctx.params.form("revision")) {
        if posted != revision.to_string() { return Ok(back_to(&ctx, Some(key.exchange))); }
    }
    // `params.require(:api_key)`: a submission without any of its fields is a bad request.
    let field = |name: &str| ctx.params.form(&format!("api_key[{name}]"));
    if ["key", "access_token", "secret"].iter().all(|name| field(name).is_none()) { return Ok(layout::bad_request()); }
    let (consumer, token, secret) = (field("key").unwrap_or_default(), field("access_token").unwrap_or_default(), field("secret").unwrap_or_default());
    let mut connected = false;
    let mut keep = kept(&ctx);
    let message = if [consumer, token, secret].iter().any(|value| crate::v2::blank(value)) {
        (flash::ALERT, i18n::text(ctx.locale, "settings.ibkr.missing_fields", &[]))
    } else {
        let candidate = original.with_ibkr_credentials(consumer, token, secret);
        let answer = ctx.params.form("key_answer").map(str::to_string);
        if answer.as_deref() == Some("x") { return Ok(back_to(&ctx, Some(key.exchange))); }
        match keys::submit_answered(&app, crate::venue::ibkr::IBKR, "Interactive Brokers", 0, original, candidate, answer).await? {
            Submitted::Correct(account) => {
                let id = if key.registration.is_some() { None } else { Some(key.id) };
                let status: i64 = match id { Some(id) => app.db(move |c| Ok(c.query_row("SELECT status FROM api_keys WHERE id = ?1", [id], |r| r.get(0))?)).await?, None => 1 };
                connected = status == 1;
                if key.registration.is_some() || key.account.is_some() { keep = account.map(|a| format!("&account_id={a}")).unwrap_or_default(); }
                (flash::NOTICE, i18n::text(ctx.locale, if status == 3 { "settings.ibkr.activation_pending" } else { "settings.ibkr.connected" }, &[]))
            }
            Submitted::Moved { to, from } => {
                connected = true;
                keep = format!("&account_id={to}");
                let (saved, unchanged) = app.db(move |c| Ok((crate::identity::label(c, to)?.html(), crate::identity::label(c, from)?.html()))).await?;
                (flash::NOTICE, i18n::t(ctx.locale, "settings.accounts.key_elsewhere_html", &[("account", i18n::Arg::Html(&saved)), ("target", i18n::Arg::Html(&unchanged))]))
            }
            Submitted::Incorrect | Submitted::Permissions(..) => (flash::ALERT, i18n::text(ctx.locale, "settings.ibkr.rejected", &[])),
            Submitted::Failed => (flash::ALERT, i18n::text(ctx.locale, "settings.ibkr.validation_failed", &[])),
            Submitted::Refused(reason) if reason == keys::REGISTRATION_WAITS => (flash::NOTICE, i18n::text(ctx.locale, "settings.ibkr.activation_pending", &[])),
            Submitted::Refused(reason) => {
                if let Some(asked) = keys::question_response(&ctx, &reason) { return Ok(asked); }
                (flash::ALERT, keys::refusal_text(&ctx, &reason))
            }
        }
    };
    flash::set(&ctx.session, message.0, message.1);
    if connected && from_sheet(&ctx) { return tracker_done(&app, &ctx, user.id).await }
    Ok(back_to_kept(&ctx, Some(key.exchange), &keep))
}

/// DELETE /settings/ibkr_connect: the key gone, its bots stopped first, its ledger rows kept (`keys::remove`). #710 7b:
/// a registration's Start over deletes only it; an unqualified Disconnect (2.x's form) removes the exchange's sole IBKR
/// key while it has one live account, and writes nothing with two or more (their pages carry the account).
pub async fn destroy(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(target) = target(&ctx) else { return Ok(layout::missing()) };
    let inner = app.clone();
    // #710 7a rule S4: the row as read here is the one removed; an activation committed meanwhile keeps its key (`remove`
    // refuses a changed row, retryable).
    let read = app.db(move |c| {
        let key = match stored_for(c, &inner, user.id, target)? { Ok(key) => key, Err(()) => return Ok(Err(())) };
        let several = match &key { Some(k) => crate::identity::accounts_of(c, user.id, k.exchange)?.len() > 1, None => false };
        Ok(Ok(match key { Some(key) if key.registration.is_none() => { let row = keys::stored_ciphertext(c, key.id)?; Some((key, Some(row), several)) } Some(key) => Some((key, None, several)), None => None }))
    }).await?;
    let Ok(read) = read else { return Ok(layout::missing()) };
    let key = read.as_ref().map(|(key, _, _)| key);
    if gated(&app, key) && target == Target::Legacy { return Ok(gate(&ctx)); }
    let exchange = key.map(|key| key.exchange);
    match read {
        Some((key, None, _)) => {
            let (id, revision) = key.registration.ok_or_else(|| WebError::Config("registration missing".into()))?;
            app.db(move |c| Ok(c.execute("DELETE FROM ibkr_registrations WHERE id = ?1 AND user_id = ?2 AND revision = ?3", rusqlite::params![id, user.id, revision])?)).await?;
            return Ok(back_to_kept(&ctx, exchange, ""));
        }
        Some((_, Some(_), true)) if target == Target::Legacy => return Ok(back_to(&ctx, exchange)),
        Some((key, Some(row), _)) => {
            let inner = app.clone();
            let outcome = app.db(move |c| keys::remove(c, &inner, user.id, key.id, Some(&row), |_| Ok(()))).await?;
            if let Some(Err(reason)) = outcome {
                flash::set(&ctx.session, flash::ALERT, keys::refusal_text(&ctx, &reason));
                // Refused, the key kept: the sheet again, the reason in it.
                return Ok(back_to(&ctx, exchange));
            }
        }
        None => {}
    }
    if from_sheet(&ctx) { return tracker_done(&app, &ctx, user.id).await }
    Ok(back(&ctx))
}
