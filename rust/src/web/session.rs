//! The session: one encrypted cookie, `_deltabadger_rust_session`. It is deliberately not Rails'
//! `_deltabadger_session`: switching engines costs one sign-in. AES-256-GCM under a key
//! derived from `secret_key_base` for this purpose only; the cookie name is the associated data.
//! Attributes as config/application.rb sets them for Rails: 30 days, HttpOnly, SameSite=Lax, and
//! Secure exactly when Rails' cookie would be.
//!
//! Unlike Rails, the cookie is not written again on every response. It is written when the session's
//! content changed in the request, and at no other time. A response to an older request, one that
//! still carried the previous cookie, then cannot put that previous session back over a newer one (a
//! sign-in lost to a background request, a sign-out undone by a request that was still in flight).
//! The price: the 30 days do not slide. A session ends 30 days after its content last changed,
//! however much it was used. A stateless cookie cannot be renewed in the background without letting
//! a delayed response overwrite a newer cookie, and a session kept on the server needs a table,
//! which Rust may not create before 3.0.
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use axum::http::{header, HeaderMap, HeaderValue};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub const COOKIE: &str = "_deltabadger_rust_session";
/// `expire_after: 30.days` (pinned by tests/session.rs against the recorded Rails setting).
pub const LIFETIME_SECONDS: i64 = 30 * 24 * 60 * 60;

/// The second-factor step of a sign-in that has passed the password stage (Users::SessionsController).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub user_id: i64,
    pub started_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionData {
    /// Who is signed in, with Devise's salt check: the first 29 characters of `encrypted_password`,
    /// so a password change ends every other session.
    pub user: Option<(i64, String)>,
    pub csrf: Option<String>,
    /// (type, message) in the order they were set: `alert`, `notice`, `success`.
    pub flash: Vec<(String, String)>,
    pub pending: Option<Pending>,
    /// Devise's `user_return_to`: where an unauthenticated GET was heading.
    pub return_to: Option<String>,
    /// Set at sign-in; the first bots page with no bots consumes it to open the wizard.
    pub auto_open_bot_wizard: bool,
}

impl SessionData {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn to_json(&self) -> Value {
        json!({
            "user": self.user.as_ref().map(|(id, salt)| json!([id, salt])),
            "csrf": self.csrf,
            "flash": self.flash.iter().map(|(kind, message)| json!([kind, message])).collect::<Vec<_>>(),
            "pending": self.pending.as_ref().map(|p| json!([p.user_id, p.started_at])),
            "return_to": self.return_to,
            "auto_open_bot_wizard": self.auto_open_bot_wizard,
        })
    }

    fn from_json(v: &Value) -> Self {
        let text = |value: &Value| value.as_str().map(str::to_string);
        Self {
            user: v["user"].as_array().and_then(|a| Some((a.first()?.as_i64()?, text(a.get(1)?)?))),
            csrf: text(&v["csrf"]),
            flash: v["flash"].as_array().map(|entries| entries.iter().filter_map(|e| Some((text(&e[0])?, text(&e[1])?))).collect()).unwrap_or_default(),
            pending: v["pending"].as_array().and_then(|a| Some(Pending { user_id: a.first()?.as_i64()?, started_at: a.get(1)?.as_i64()? })),
            return_to: text(&v["return_to"]),
            auto_open_bot_wizard: v["auto_open_bot_wizard"] == true,
        }
    }
}

/// The cookie value for this session: base64url(nonce || ciphertext || tag) of `{"exp", "data"}`.
pub fn seal(key: &[u8; 32], data: &SessionData, now: DateTime<Utc>) -> String {
    let plain = json!({ "exp": now.timestamp() + LIFETIME_SECONDS, "data": data.to_json() }).to_string();
    let nonce: [u8; 12] = rand::random();
    // Encryption fails only on a plaintext of 64 GiB or more; an empty value then reads as no session.
    let sealed = Aes256Gcm::new(key.into())
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain.as_bytes(), aad: COOKIE.as_bytes() })
        .unwrap_or_default();
    B64URL.encode([nonce.as_slice(), sealed.as_slice()].concat())
}

/// A cookie this key sealed: its session, and when it stops being one (epoch seconds). The expiry
/// is inside the encrypted value, so it is this server that enforces it; /cable keeps it with each
/// connection the cookie opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub data: SessionData,
    pub expires_at: i64,
}

/// The session in a cookie (see `read`).
pub fn open(key: &[u8; 32], cookie: &str, now: DateTime<Utc>) -> Option<SessionData> {
    read(key, cookie, now).map(|opened| opened.data)
}

/// `None` for anything that is not a cookie this key sealed and that has not expired: a forged,
/// truncated, foreign or old value is simply no session.
pub fn read(key: &[u8; 32], cookie: &str, now: DateTime<Utc>) -> Option<Opened> {
    let bytes = B64URL.decode(cookie).ok()?;
    if bytes.len() < 12 + 16 {
        return None;
    }
    let (nonce, sealed) = bytes.split_at(12);
    let plain = Aes256Gcm::new(key.into()).decrypt(Nonce::from_slice(nonce), Payload { msg: sealed, aad: COOKIE.as_bytes() }).ok()?;
    let value: Value = serde_json::from_slice(&plain).ok()?;
    let expires_at = value["exp"].as_i64()?;
    if expires_at <= now.timestamp() {
        return None;
    }
    Some(Opened { data: SessionData::from_json(&value["data"]), expires_at })
}

/// Every value a request's `Cookie` headers give for our exact name, in the order they were sent.
pub fn cookie_values(headers: &HeaderMap) -> impl Iterator<Item = &str> {
    headers.get_all(header::COOKIE).iter()
        .filter_map(|line| line.to_str().ok())
        .flat_map(|line| line.split(';'))
        .filter_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

/// The session a request carries: the first cookie of our name that opens. There can be several:
/// any page on a sibling subdomain may set one for the whole site, and with a longer Path the browser
/// sends it first. A value that does not open is passed over, so it cannot hide the real one.
pub fn from_request(key: &[u8; 32], headers: &HeaderMap, now: DateTime<Utc>) -> Option<Opened> {
    cookie_values(headers).find_map(|value| read(key, value, now))
}

/// `Set-Cookie` as Rails writes its session cookie, under our name.
pub fn set_cookie(value: &str, now: DateTime<Utc>, secure: bool) -> HeaderValue {
    let expires = (now + Duration::seconds(LIFETIME_SECONDS)).format("%a, %d %b %Y %H:%M:%S GMT");
    let secure = if secure { "; secure" } else { "" };
    // The value is base64url and the rest is fixed ASCII, so this cannot fail; an empty header is the fallback.
    HeaderValue::from_str(&format!("{COOKIE}={value}; path=/; expires={expires}{secure}; httponly; samesite=lax"))
        .unwrap_or(HeaderValue::from_static(""))
}

/// The session of one request, shared between the pipeline and the handler.
#[derive(Clone, Default)]
pub struct Session(Arc<Mutex<SessionData>>);

impl Session {
    pub fn new(data: SessionData) -> Self {
        Self(Arc::new(Mutex::new(data)))
    }

    /// A poisoned lock still holds the data (release builds abort on panic anyway).
    pub fn lock(&self) -> MutexGuard<'_, SessionData> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn snapshot(&self) -> SessionData {
        self.lock().clone()
    }
}
