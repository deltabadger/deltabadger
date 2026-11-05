//! The desktop app's one account (PLAN-B6-3 §1, §2; `DELTABADGER_PLATFORM=desktop` only). Rust-only, every behaviour a
//! named divergence:
//! - D-DESKTOP-ACCOUNT: at `serve` start, after the takeover, the lowest-id admin is the account; with none, the
//!   lowest-id user is made admin (D-DESKTOP-ACCOUNT-PROMOTE); with no users one is inserted. It is unlocked and
//!   confirmed every start (D-DESKTOP-ACCOUNT-ACTIVE). Any other user's session reads as signed out
//!   (D-DESKTOP-OTHER-USERS-UNREACHABLE).
//! - D-DESKTOP-LAUNCHER-SIGN-IN: the launcher holds a secret minted per start (`DELTABADGER_DESKTOP_LAUNCHER`, never
//!   shown to a page) and trades it for a single-use 60 s ticket (`POST /desktop/ticket`), which the window redeems at
//!   `GET /desktop/sign_in`. No password, no form, no second factor (D-DESKTOP-NO-2FA). Without the secret nothing signs in.
//! - D-DESKTOP-NO-ACCOUNT-PAGES: signup, password, setup, confirmation, second factor, logout and the account's email,
//!   password, 2FA, registration and mail settings answer 404, before CSRF, the setup gate and any sign-in check.
//! - D-DESKTOP-BROWSER-CODE: a browser (an MCP client's OAuth page) signs in with an 8-character code from Settings.
//! - D-DESKTOP-NOTIFY: `serve` runs the mail sender with the desktop delivery (main.rs): bot alerts wait in `outbox` for
//!   the launcher (`GET /desktop/notices`, `POST /desktop/notices/ack`), which shows them as system notifications.
use super::{auth, layout, setup, App, Params, WebError};
use crate::codec::format_time;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock, PoisonError};
use subtle::ConstantTimeEq;

/// The launcher's header on `/desktop/*` (§1).
pub const LAUNCHER_HEADER: &str = "x-deltabadger-launcher";
/// The inserted account's address: RFC 2606's reserved TLD, so it can never be anyone's.
pub const EMAIL: &str = "desktop@deltabadger.invalid";
const TICKET_TTL: i64 = 60;
const TICKETS_LIVE: usize = 4;
const CODE_TTL: i64 = 10 * 60;
const CODE_LEN: usize = 8;
const CODE_WRONG_LIMIT: u32 = 5;
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// `/desktop/sign_in` per address and minute.
pub const SIGN_IN_LIMIT: u32 = 20;

/// What the desktop holds in memory for the run.
pub struct Desktop {
    launcher: Option<String>,
    locale: Option<String>,
    time_zone: Option<String>,
    account: OnceLock<i64>,
    tickets: Mutex<VecDeque<(String, i64)>>,
    code: Mutex<Option<Code>>,
    /// D-DESKTOP-NOTIFY: the bot alerts the launcher shows (PLAN-B6-3 §3).
    outbox: std::sync::Arc<crate::mail::outbox::Outbox>,
}

struct Code {
    value: String,
    expires: i64,
    wrong: u32,
}

impl Desktop {
    /// `Some` on the desktop only.
    pub fn from_env(platform: crate::jobs::app_update::Platform, env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        if platform != crate::jobs::app_update::Platform::Desktop {
            return None;
        }
        let launcher = env("DELTABADGER_DESKTOP_LAUNCHER").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if launcher.is_none() {
            crate::engine::log("desktop: started without the launcher's secret: nothing signs in (/desktop/* is refused)");
        }
        Some(Self {
            launcher,
            locale: env("DELTABADGER_DESKTOP_LOCALE"),
            time_zone: env("DELTABADGER_DESKTOP_TIME_ZONE"),
            account: OnceLock::new(),
            tickets: Mutex::default(),
            code: Mutex::default(),
            outbox: Default::default(),
        })
    }

    fn launcher_matches(&self, headers: &HeaderMap) -> bool {
        let given = headers.get(LAUNCHER_HEADER).and_then(|v| v.to_str().ok()).unwrap_or("");
        self.launcher.as_deref().is_some_and(|expected| setup::token_matches(given, expected))
    }

    fn issue_ticket(&self, now: i64) -> Result<String, WebError> {
        let ticket = hex::encode(random::<16>()?);
        let mut tickets = self.tickets.lock().unwrap_or_else(PoisonError::into_inner);
        tickets.retain(|(_, expires)| *expires > now);
        while tickets.len() >= TICKETS_LIVE {
            tickets.pop_front();
        }
        tickets.push_back((ticket.clone(), now + TICKET_TTL));
        Ok(ticket)
    }

    /// The atomic take: under the one lock, a live match is removed, so two redemptions cannot both win.
    fn redeem_ticket(&self, given: &str, now: i64) -> bool {
        let mut tickets = self.tickets.lock().unwrap_or_else(PoisonError::into_inner);
        tickets.retain(|(_, expires)| *expires > now);
        let found = tickets.iter().position(|(t, _)| bool::from(t.as_bytes().ct_eq(given.as_bytes())));
        found.map(|i| tickets.remove(i)).is_some()
    }

    fn issue_code(&self, now: i64) -> Result<String, WebError> {
        let value: String = random::<CODE_LEN>()?.iter().map(|b| CROCKFORD[(*b & 31) as usize] as char).collect();
        *self.code.lock().unwrap_or_else(PoisonError::into_inner) = Some(Code { value: value.clone(), expires: now + CODE_TTL, wrong: 0 });
        Ok(value)
    }

    /// A typed code against the live one, taken when it matches; the fifth wrong one burns it.
    fn redeem_code(&self, typed: &str, now: i64) -> bool {
        let typed = normalize_code(typed);
        let mut code = self.code.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(live) = code.as_mut().filter(|c| c.expires > now) else {
            *code = None;
            return false;
        };
        if typed.len() == CODE_LEN && bool::from(live.value.as_bytes().ct_eq(typed.as_bytes())) {
            *code = None;
            return true;
        }
        live.wrong += 1;
        if live.wrong >= CODE_WRONG_LIMIT {
            *code = None;
        }
        false
    }
}

fn random<const N: usize>() -> Result<[u8; N], WebError> {
    let mut bytes = [0u8; N];
    SystemRandom::new().fill(&mut bytes).map_err(|_| WebError::Config("no system randomness".into()))?;
    Ok(bytes)
}

/// Crockford's reading: case and dashes ignored, O as 0, I and L as 1.
fn normalize_code(typed: &str) -> String {
    typed
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect()
}

/// The OS locale (`pl-PL`, `pt_BR`) as one of the app's, else the default.
pub fn map_locale(os: &str) -> &'static str {
    let language = os.split(['-', '_', '.']).next().unwrap_or("").to_ascii_lowercase();
    super::locale::known(&language).unwrap_or(super::locale::DEFAULT)
}

/// An IANA zone as the Rails name `users.time_zone` holds, the first in name order when several share it, else `UTC`.
pub fn map_zone(iana: &str) -> String {
    let Ok(zone) = iana.parse::<chrono_tz::Tz>() else { return "UTC".into() };
    let mut names: Vec<&String> = super::timezone::names().filter(|n| super::timezone::zone(n) == Some(zone)).collect();
    names.sort();
    names.first().map_or_else(|| "UTC".into(), |n| n.to_string())
}

/// D-DESKTOP-ACCOUNT, in one IMMEDIATE transaction. Returns the account's id.
pub fn ensure_account(c: &Connection, app: &App, desktop: &Desktop, now: DateTime<Utc>) -> Result<i64, WebError> {
    let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
    let lowest = |condition: &str| -> Result<Option<i64>, WebError> {
        Ok(tx.query_row(&format!("SELECT id FROM users {condition} ORDER BY id LIMIT 1"), [], |r| r.get(0)).optional()?)
    };
    let at = format_time(now);
    let id = match lowest("WHERE admin = 1")? {
        Some(id) => id,
        None => match lowest("")? {
            Some(id) => {
                tx.execute("UPDATE users SET admin = 1, updated_at = ?1 WHERE id = ?2", (&at, id))?;
                crate::engine::log(&format!("desktop: no admin: user {id} is made the desktop account's admin (D-DESKTOP-ACCOUNT-PROMOTE)"));
                id
            }
            None => {
                // A password nobody can type: bcrypt of random bytes that are dropped here.
                let unknowable = hex::encode(random::<32>()?);
                let hash = crate::crypto::hash_password(&unknowable).map_err(|e| WebError::Config(format!("bcrypt: {e}")))?;
                let locale = map_locale(desktop.locale.as_deref().unwrap_or(""));
                let zone = map_zone(desktop.time_zone.as_deref().unwrap_or(""));
                tx.execute(
                    "INSERT INTO users (name, email, encrypted_password, admin, confirmed_at, setup_completed, locale, time_zone, display_currency, \
                     otp_secret_key, created_at, updated_at) VALUES ('Deltabadger', ?1, ?2, 1, ?3, 1, ?4, ?5, 'USD', ?6, ?3, ?3)",
                    (EMAIL, &hash, &at, locale, &zone, app.cipher.encrypt_binary(&super::settings::two_fa::new_seed())),
                )?;
                tx.last_insert_rowid()
            }
        },
    };
    tx.execute("UPDATE users SET locked_at = NULL, failed_attempts = 0, confirmed_at = COALESCE(confirmed_at, ?1) WHERE id = ?2", (&at, id))?;
    tx.commit()?;
    Ok(id)
}

/// `serve`'s step after the takeover (and the tests'): the account made, its id held for the run. Nothing off the desktop.
pub async fn prepare(app: &App) -> Result<Option<i64>, WebError> {
    if app.desktop.is_none() {
        return Ok(None);
    }
    let (inner, now) = (app.clone(), app.now());
    let id = app.db(move |c| ensure_account(c, &inner, inner.desktop.as_ref().expect("checked"), now)).await?;
    let held = *app.desktop.as_ref().expect("checked").account.get_or_init(|| id);
    Ok(Some(held))
}

impl App {
    /// D-DESKTOP-OTHER-USERS-UNREACHABLE for bearer tokens (REST, MCP): on the desktop only its account's tokens act;
    /// a token a 2.x install issued to another user reads as one whose user is gone. Every token elsewhere.
    pub fn desktop_owner_allowed(&self, user_id: i64) -> bool {
        self.desktop.is_none() || self.desktop_account() == Some(user_id)
    }

    /// D-DESKTOP-NOTIFY: the outbox the mail sender offers to and the launcher lists; `None` off the desktop.
    pub fn desktop_outbox(&self) -> Option<std::sync::Arc<crate::mail::outbox::Outbox>> {
        self.desktop.as_ref().map(|d| d.outbox.clone())
    }

    /// The desktop account's id, once `prepare` ran; `None` off the desktop.
    pub fn desktop_account(&self) -> Option<i64> {
        self.desktop.as_ref().and_then(|d| d.account.get().copied())
    }
}

/// Whether the desktop's guard answers 404 for this route path (locale prefix already stripped): D-DESKTOP-NO-ACCOUNT-PAGES.
/// Any method and any `.format`.
pub fn hidden(route_path: &str) -> bool {
    const TREES: [&str; 6] = ["/signup", "/password", "/setup", "/confirmation", "/verify_two_factor", "/logout"];
    const SETTINGS: [&str; 8] = [
        "/settings/update_registration", "/settings/update_email_notifications", "/settings/disconnect_email", "/settings/send_test_email",
        "/settings/update_email", "/settings/update_password", "/settings/update_two_fa", "/settings/edit_two_fa",
    ];
    let unformatted = match route_path.rsplit_once('/') {
        Some((head, last)) => format!("{head}/{}", last.split('.').next().unwrap_or(last)),
        None => route_path.to_string(),
    };
    TREES.iter().any(|t| unformatted == *t || unformatted.starts_with(&format!("{t}/")))
        || SETTINGS.contains(&unformatted.as_str())
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Before the pipeline (session, CSRF, the setup gate, sign-in): `Some` when the desktop answers this request itself.
/// `/desktop/sign_in` goes on to the pipeline, which needs the session (`sign_in`).
pub fn guard(app: &App, method: &Method, params: &Params, headers: &HeaderMap) -> Option<Response> {
    let desktop = app.desktop.as_ref()?;
    let path = params.route_path.as_str();
    if path == "/desktop" || path.starts_with("/desktop/") {
        return Some(match (method, path, params.path_locale) {
            (&Method::GET | &Method::HEAD, "/desktop/sign_in", None) => return None,
            (&Method::POST, "/desktop/ticket", None) => {
                if !desktop.launcher_matches(headers) {
                    return Some(no_store((StatusCode::UNAUTHORIZED, "").into_response()));
                }
                match desktop.issue_ticket(app.now().timestamp()) {
                    Ok(ticket) => no_store(([(header::CONTENT_TYPE, "application/json")], serde_json::json!({ "ticket": ticket }).to_string()).into_response()),
                    Err(e) => e.into_response(),
                }
            }
            // D-DESKTOP-NOTIFY (PLAN-B6-3 §3): the launcher lists the bot alerts to show, and acks the ones it showed.
            (&Method::GET, "/desktop/notices", None) | (&Method::POST, "/desktop/notices/ack", None) if !desktop.launcher_matches(headers) => {
                no_store((StatusCode::UNAUTHORIZED, "").into_response())
            }
            (&Method::GET, "/desktop/notices", None) => {
                let notices = desktop.outbox.list(crate::mail::outbox::LIST_LIMIT);
                no_store(([(header::CONTENT_TYPE, "application/json")], serde_json::json!({ "notices": notices }).to_string()).into_response())
            }
            (&Method::POST, "/desktop/notices/ack", None) => {
                let ids: Option<Vec<String>> = params.json.as_ref().and_then(|j| serde_json::from_value(j.get("ids")?.clone()).ok());
                match ids {
                    Some(ids) => { desktop.outbox.ack(&ids); no_store(StatusCode::NO_CONTENT.into_response()) }
                    None => no_store((StatusCode::BAD_REQUEST, "").into_response()),
                }
            }
            _ => layout::missing(),
        });
    }
    hidden(path).then(layout::missing)
}

/// `GET /desktop/sign_in` (in the pipeline, after the rate limit, before the setup gate and sign-in checks): a live
/// ticket signs the session in as the desktop account. Always a redirect, never a page.
pub async fn sign_in(app: &App, session: &super::session::Session, params: &Params) -> Response {
    let desktop = app.desktop.as_ref().expect("the pipeline checked");
    let given = params.query("ticket").unwrap_or("");
    if let Some(id) = app.desktop_account().filter(|_| desktop.launcher.is_some() && desktop.redeem_ticket(given, app.now().timestamp())) {
        match app.db(move |c| auth::User::find(c, id)).await {
            Ok(Some(user)) => auth::sign_in(&mut session.lock(), id, &user.encrypted_password, true),
            Ok(None) => {}
            Err(e) => return e.into_response(),
        }
    }
    let location = session.lock().return_to.take().unwrap_or_else(landing);
    let mut response = layout::early_redirect(StatusCode::FOUND, &location);
    response.headers_mut().insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response.extensions_mut().insert(super::headers::BelowControllers);
    response
}

/// The root at this start's launch URL (never a page the service worker cached), as the launcher opens it.
fn landing() -> String {
    super::refused::ready_token().map_or_else(|| "/".to_string(), |t| format!("/?launch={t}"))
}

/// A signed-in desktop window asks for a code to sign a browser in (Settings → Sign in a browser).
/// None without the launcher's secret: a start that cannot sign its window in signs no browser in either (§1).
pub fn new_code(app: &App) -> Result<Option<String>, WebError> {
    let desktop = app.desktop.as_ref().ok_or_else(|| WebError::Config("not the desktop".into()))?;
    if desktop.launcher.is_none() { return Ok(None); }
    desktop.issue_code(app.now().timestamp()).map(Some)
}

/// A code typed on `/login`: true when it signed this browser's session in.
pub async fn redeem_code(app: &App, session: &super::session::Session, typed: &str) -> Result<bool, WebError> {
    let Some(desktop) = app.desktop.as_ref() else { return Ok(false) };
    let Some(id) = app.desktop_account().filter(|_| desktop.launcher.is_some()) else { return Ok(false) };
    if !desktop.redeem_code(typed, app.now().timestamp()) {
        return Ok(false);
    }
    let Some(user) = app.db(move |c| auth::User::find(c, id)).await? else { return Ok(false) };
    auth::sign_in(&mut session.lock(), id, &user.encrypted_password, true);
    Ok(true)
}

/// How long a code stays usable, for the Settings row's line.
pub fn code_minutes() -> i64 { Duration::seconds(CODE_TTL).num_minutes() }

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop() -> Desktop {
        Desktop { launcher: Some("s".into()), locale: None, time_zone: None, account: OnceLock::new(), tickets: Mutex::default(), code: Mutex::default(), outbox: Default::default() }
    }

    #[test]
    fn a_code_is_taken_once_and_burns_after_five_wrong() {
        let d = desktop();
        let code = d.issue_code(0).unwrap();
        assert!(!d.redeem_code("XXXXXXXX", 1));
        assert!(d.redeem_code(&code.to_lowercase(), 2));
        assert!(!d.redeem_code(&code, 3));
        let code = d.issue_code(0).unwrap();
        for _ in 0..5 { assert!(!d.redeem_code("XXXXXXXX", 1)); }
        assert!(!d.redeem_code(&code, 1));
    }

    #[test]
    fn a_ticket_is_taken_once() {
        let d = desktop();
        let t = d.issue_ticket(0).unwrap();
        assert!(d.redeem_ticket(&t, 1));
        assert!(!d.redeem_ticket(&t, 1));
    }
}
