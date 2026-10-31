//! The IBKR connection (Settings::IbkrConnectionsController with settings/ibkr_connections/): its states as one component
//! (`wizard`), served by `/settings/ibkr_connect` and its downloads, activation and disconnect.
//!
//! Divergences:
//! - D-IBKR-DHGROUP: a new connection's keys are made inline when it is created (two RSA-2048 keys and the RFC 7919
//!   ffdhe2048 group), so the page goes straight to the downloads; Rails generates a DH prime in a job and shows
//!   "generating" until it has run.
//! - D-IBKR-RECONNECT: a key whose stored keys make no session (none yet: Rails' job never ran; or material this build
//!   cannot read) offers Disconnect only, then a fresh connect. Nothing completes or converts it.
//! - D-IBKR-NOECHO: the credentials form is always empty. Rails' `form_with scope: :api_key` fills it from `@api_key`,
//!   writing the stored consumer key, access token and its secret into the page.
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

/// The state the wizard renders, from the user's IBKR key (`_wizard.html.erb`'s branches, in its order).
pub enum State { None, Connected, Unusable, Stalled(String), Pending, Downloads }

/// The user's IBKR key as the wizard reads it.
pub struct Stored { pub id: i64, pub exchange: i64, status: i64, updated_at: String, material: [Option<String>; 3] }

impl Stored {
    fn usable(&self) -> bool {
        match &self.material { [Some(signing), Some(encryption), Some(dh)] => signer::usable(signing, encryption, dh), _ => false }
    }
}

/// `Exchanges::Ibkr.first`.
fn exchange(c: &Connection) -> Result<Option<i64>, WebError> {
    Ok(c.query_row("SELECT id FROM exchanges WHERE type = ?1 ORDER BY id LIMIT 1", [crate::venue::ibkr::IBKR], |r| r.get(0)).optional()?)
}

/// `current_ibkr_key`: the user's trading key on the first IBKR exchange.
pub fn stored(c: &Connection, app: &App, user: i64) -> Result<Option<Stored>, WebError> {
    let Some(exchange) = exchange(c)? else { return Ok(None) };
    let row = c.query_row("SELECT id, status, updated_at, rsa_signature_key, rsa_encryption_key, dh_param FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 AND key_type = 0 ORDER BY id LIMIT 1",
                          (user, exchange), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, [r.get::<_, Option<String>>(3)?, r.get(4)?, r.get(5)?]))).optional()?;
    let Some((id, status, updated_at, encrypted)) = row else { return Ok(None) };
    let mut material = [None, None, None];
    for (slot, value) in material.iter_mut().zip(encrypted) {
        // A blank column is no material (`.blank?`); an unreadable one is D-IBKR-RECONNECT's rejected material.
        *slot = value.and_then(|v| app.cipher.decrypt(&v).ok()).filter(|v| !crate::ruby::blank(v));
    }
    Ok(Some(Stored { id, exchange, status, updated_at, material }))
}

/// The wizard's state for `key` at `ctx.now`.
pub fn state(ctx: &Ctx, key: Option<&Stored>) -> State {
    let Some(key) = key else { return State::None };
    if !key.usable() { return State::Unusable; }
    match key.status {
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
struct Wizard<'a> { v: &'a Ctx, csrf: &'a str, state: &'a State, path: String, check_address: String, recommended_url: String, alternative_url: String }

/// The connection component: `#ibkr-connect-wizard` in `state`, its forms posting to the connect page's routes.
pub fn wizard(ctx: &Ctx, csrf: &str, state: &State) -> Result<String, WebError> {
    let check_address = i18n::t(ctx.locale, "settings.ibkr.portal_step_check_address", &[("recommended", i18n::Arg::Text(PORTALS[0])), ("alternative", i18n::Arg::Text(PORTALS[1]))]);
    let url = |host: &str| format!("https://{host}{PORTAL_PATH}");
    Ok(Wizard { v: ctx, csrf, state, path: ctx.path(PATH), check_address, recommended_url: url(PORTALS[0]), alternative_url: url(PORTALS[1]) }.render()?)
}

/// `require_ibkr_available!`: the data API's container (StockTradingSettings.ibkr_available?), or a key there already is.
fn gated(app: &App, key: Option<&Stored>) -> bool { !app.config.market_data_url && key.is_none() }

fn back(ctx: &Ctx) -> Response { layout::redirect(StatusCode::FOUND, &ctx.path(PATH)) }
fn gate(ctx: &Ctx) -> Response { layout::redirect(StatusCode::FOUND, &ctx.path("/settings/connect")) }

/// GET /settings/ibkr_connect.
pub async fn show(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let (inner, owner) = (app.clone(), user.id);
    let (key, shell) = app.db(move |c| Ok((stored(c, &inner, user.id)?, shell::Shell::load(c, &inner, &user)?))).await?;
    if gated(&app, key.as_ref()) { return Ok(gate(&ctx)); }
    let csrf = ctx.csrf_token();
    let body = format!("<div class=\"page-head\">\n  <div class=\"stitle\">{}</div>\n</div>\n\n{}\n\n<div class=\"columns\">\n  <div class=\"column gap-1\">\n    {}  </div>\n</div>\n",
                       ctx.t("settings.ibkr.title"), ctx.stream_from(&format!("user_{owner}:ibkr_connection")), wizard(&ctx, &csrf, &state(&ctx, key.as_ref()))?);
    let user = ctx.user().ok_or_else(|| WebError::Config("missing page owner".into()))?;
    shell::application(&ctx, &csrf, user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] })
}

/// GET /settings/ibkr_connect/download?artifact=…: a public half of the key's material, never a private key.
pub async fn download(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let inner = app.clone();
    let key = app.db(move |c| stored(c, &inner, user.id)).await?;
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
        None => Ok(back(&ctx)),
        // Rails raises reading a stored key OpenSSL cannot parse.
        Some(None) => Err(WebError::Config("unreadable IBKR key".into())),
        Some(Some(pem)) => Ok(([(header::CONTENT_TYPE, "application/x-pem-file".to_string()),
                                (header::CONTENT_DISPOSITION, format!("attachment; filename=\"ibkr_{artifact}.pem\"; filename*=UTF-8''ibkr_{artifact}.pem"))], pem).into_response()),
    }
}

/// POST /settings/ibkr_connect: the user's IBKR key, made with its keys when there is none (D-IBKR-DHGROUP). A key there
/// already is stays as it is, whatever its state (D-IBKR-RECONNECT).
pub async fn create(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let inner = app.clone();
    let (key, exchange) = app.db(move |c| Ok((stored(c, &inner, user.id)?, exchange(c)?))).await?;
    if gated(&app, key.as_ref()) { return Ok(gate(&ctx)); }
    let (None, Some(exchange)) = (key, exchange) else { return Ok(back(&ctx)) };
    // Ibkr::GenerateConnectionKeysJob's artifacts: RSA generation takes a moment, so off the async workers.
    let material = tokio::task::spawn_blocking(|| [signer::generate_rsa().0, signer::generate_rsa().0, signer::ffdhe2048_pem()])
        .await.map_err(|_| WebError::Config("IBKR key generation failed".into()))?;
    let inner = app.clone();
    app.db(move |c| {
        let tx = rusqlite::Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        // find_or_create_by!: a key made meanwhile is the one kept.
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 AND key_type = 0)", (user.id, exchange), |r| r.get(0))?;
        if !exists {
            let now = crate::codec::format_time(inner.now());
            let [signing, encryption, dh] = material.map(|pem| inner.cipher.encrypt(&pem));
            tx.execute("INSERT INTO api_keys(user_id, exchange_id, key_type, status, rsa_signature_key, rsa_encryption_key, dh_param, created_at, updated_at) VALUES(?1, ?2, 0, 0, ?3, ?4, ?5, ?6, ?6)",
                       rusqlite::params![user.id, exchange, signing, encryption, dh, now])?;
        }
        tx.commit()?;
        Ok(())
    }).await?;
    Ok(back(&ctx))
}

/// POST /settings/ibkr_connect/activate: the pasted consumer key, access token and secret, validated with the stored keys
/// (ApiKey#validate_credentials!, `keys::submit_candidate`), and the flash its answer earns.
pub async fn activate(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let inner = app.clone();
    let found = app.db(move |c| {
        let Some(key) = stored(c, &inner, user.id)? else { return Ok(None) };
        let original = Key::read(c, &inner, user.id, key.exchange, 0)?;
        Ok(Some((key, original)))
    }).await?;
    if gated(&app, found.as_ref().map(|(key, _)| key)) { return Ok(gate(&ctx)); }
    let Some((key, original)) = found else { return Ok(back(&ctx)) };
    // `params.require(:api_key)`: a submission without any of its fields is a bad request.
    let field = |name: &str| ctx.params.form(&format!("api_key[{name}]"));
    if ["key", "access_token", "secret"].iter().all(|name| field(name).is_none()) { return Ok(layout::bad_request()); }
    let (consumer, token, secret) = (field("key").unwrap_or_default(), field("access_token").unwrap_or_default(), field("secret").unwrap_or_default());
    let message = if [consumer, token, secret].iter().any(|value| crate::ruby::blank(value)) {
        (flash::ALERT, i18n::text(ctx.locale, "settings.ibkr.missing_fields", &[]))
    } else {
        let candidate = original.with_ibkr_credentials(consumer, token, secret);
        match keys::submit_candidate(&app, user.id, key.exchange, crate::venue::ibkr::IBKR, "Interactive Brokers", 0, original, candidate).await? {
            Submitted::Correct => {
                let id = key.id;
                let status: i64 = app.db(move |c| Ok(c.query_row("SELECT status FROM api_keys WHERE id = ?1", [id], |r| r.get(0))?)).await?;
                (flash::NOTICE, i18n::text(ctx.locale, if status == 3 { "settings.ibkr.activation_pending" } else { "settings.ibkr.connected" }, &[]))
            }
            Submitted::Incorrect | Submitted::Permissions(..) => (flash::ALERT, i18n::text(ctx.locale, "settings.ibkr.rejected", &[])),
            Submitted::Failed => (flash::ALERT, i18n::text(ctx.locale, "settings.ibkr.validation_failed", &[])),
            Submitted::Refused(reason) => (flash::ALERT, keys::refusal_text(&ctx, &reason)),
        }
    };
    flash::set(&ctx.session, message.0, message.1);
    Ok(back(&ctx))
}

/// DELETE /settings/ibkr_connect: the key gone, its bots stopped first, its ledger rows kept (`keys::remove`).
pub async fn destroy(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let inner = app.clone();
    let key = app.db(move |c| stored(c, &inner, user.id)).await?;
    if gated(&app, key.as_ref()) { return Ok(gate(&ctx)); }
    if let Some(key) = key {
        let inner = app.clone();
        let outcome = app.db(move |c| keys::remove(c, &inner, user.id, key.id, |_| Ok(()))).await?;
        if let Some(Err(reason)) = outcome { flash::set(&ctx.session, flash::ALERT, keys::refusal_text(&ctx, &reason)); }
    }
    Ok(back(&ctx))
}
