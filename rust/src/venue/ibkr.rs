//! IBKR's Web API session (slice I1): Clients::Ibkr::Session, ported as Rails runs it. A login (one api key) mints the
//! live session token (LST) by Diffie-Hellman and keeps it 23 h, opens the brokerage session (`ssodh/init`) when its
//! 4-minute flag has lapsed, and signs every call with the LST (`Wire::Ibkr`, at send time). A 401 or a failed mint
//! (Session::AuthError) invalidates both and retries once; the second failure stands.
//!
//! Rails caches the LST encrypted in Solid Cache, shared by its web and job processes, and serialises a login across them
//! with IbkrLock (a SQLite row, 15 s wait). Rust is one process: both live in `LOGINS`, in memory, keyed by api key and
//! replaced when the key's CredentialVersion changes (a restart costs one re-mint). The `ibkr_locks` table is not used.
use super::http::{decode_json, HttpRequest, HttpResponse, Transport, TransportError};
use super::VenueError;
use super::signers::ibkr::{self as oauth, Material, Signer};
use crate::engine::model::CredentialVersion;
use crate::engine::Clock;
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

/// exchanges.type.
pub const IBKR: &str = "Exchanges::Ibkr";
/// Session::BASE_URL.
pub const URL: &str = "https://api.ibkr.com";
pub const LST_PATH: &str = "/v1/api/oauth/live_session_token";
pub const SSODH_INIT_PATH: &str = "/v1/api/iserver/auth/ssodh/init";
/// The account list IBKR wants read before an order is placed.
pub const ACCOUNTS_PATH: &str = "/v1/api/iserver/accounts";
/// Session::LST_TTL.
pub const LST_TTL: TimeDelta = TimeDelta::hours(23);
/// Session::BROKERAGE_SESSION_TTL.
pub const BROKERAGE_SESSION_TTL: TimeDelta = TimeDelta::minutes(4);
/// IbkrLock's wait for a busy login.
pub const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Default)]
struct State {
    /// The LST and the moment it expires (Rails' cache entry, `expires_in: LST_TTL`).
    lst: Option<(String, DateTime<Utc>)>,
    /// Until when the brokerage session counts as open (Rails' `ibkr:bsession` flag).
    brokerage_until: Option<DateTime<Utc>>,
    /// The mint in flight: its DH challenge and prepend, which the LST request's header signs at send time.
    minting: Option<(String, String)>,
}

/// One IBKR login (an api key row): its signer, its session state, and the lock every operation on its consumer key holds.
pub struct Login {
    version: CredentialVersion,
    material: Material,
    signer: Signer,
    op: Arc<tokio::sync::Mutex<()>>,
    state: Mutex<State>,
}

type Op = Arc<tokio::sync::Mutex<()>>;
/// IbkrLock's rows: one lock per consumer-key fingerprint (Clients::Ibkr#lock_key, sha256 of the consumer key), shared by
/// every row and every credential version of that key. ponytail: never pruned (one entry per consumer key ever seen).
static OPS: LazyLock<Mutex<HashMap<[u8; 32], Op>>> = LazyLock::new(Mutex::default);

fn op_for(consumer_key: &str) -> Op {
    use sha2::{Digest, Sha256};
    let fingerprint: [u8; 32] = Sha256::digest(consumer_key.as_bytes()).into();
    OPS.lock().unwrap_or_else(|p| p.into_inner()).entry(fingerprint).or_default().clone()
}

impl Login {
    /// Session#initialize: the key material parsed once; the lock its consumer key's.
    pub fn new(material: Material, version: CredentialVersion) -> Result<Self, oauth::Error> {
        let op = op_for(&material.consumer_key);
        Self::with_op(material, version, op)
    }

    fn with_op(material: Material, version: CredentialVersion, op: Op) -> Result<Self, oauth::Error> {
        Ok(Self { signer: Signer::new(&material)?, material, version, op, state: Mutex::default() })
    }

    fn state(&self) -> MutexGuard<'_, State> { self.state.lock().unwrap_or_else(|p| p.into_inner()) }

    /// Session#invalidate_session!: the LST and the brokerage flag dropped.
    pub fn invalidate(&self) {
        let mut s = self.state();
        s.lst = None;
        s.brokerage_until = None;
    }

    /// The credential values a diagnostic must never show.
    pub fn sensitive(&self) -> Vec<String> {
        let m = &self.material;
        [&m.consumer_key, &m.access_token, &m.access_token_secret, &m.signature_key, &m.encryption_key].into_iter().cloned().collect()
    }

    pub fn consumer_key(&self) -> &str { &self.material.consumer_key }
    pub fn access_token_secret(&self) -> &str { &self.material.access_token_secret }

    /// Whether `r` can be signed now: the LST request during a mint, any other with an LST in hand.
    pub(crate) fn can_sign(&self, r: &HttpRequest) -> bool {
        let s = self.state();
        if r.path == LST_PATH { s.minting.is_some() } else { s.lst.is_some() }
    }

    /// `r`'s Authorization header, built at send time with a fresh nonce and timestamp (Oauth.nonce, Oauth.timestamp):
    /// the RSA-signed LST request (#live_session_token_header) or an LST-keyed HMAC call (#signed_header over the query).
    pub(crate) fn authorization(&self, r: &HttpRequest) -> Option<String> {
        let (nonce, timestamp) = (oauth::nonce(), oauth::timestamp());
        let url = format!("{}{}", r.base, r.path);
        let s = self.state();
        if r.path == LST_PATH {
            let (challenge, prepend) = s.minting.as_ref()?;
            return Some(self.signer.live_session_token_header(&url, challenge, prepend, &nonce, &timestamp));
        }
        let query: Vec<(String, String)> = r.query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        self.signer.signed_header(r.method, &url, &s.lst.as_ref()?.0, &query, &nonce, &timestamp)
    }
}

static LOGINS: LazyLock<Mutex<HashMap<i64, Arc<Login>>>> = LazyLock::new(Mutex::default);

fn logins() -> MutexGuard<'static, HashMap<i64, Arc<Login>>> { LOGINS.lock().unwrap_or_else(|p| p.into_inner()) }

/// The process-wide login of api key `version.id`: the one in hand while its credentials and realm are unchanged, else a
/// fresh one (no LST, no brokerage session) replacing it. Rails builds a Session, so reads ibkr_realm, on every call.
pub fn login(version: &CredentialVersion, material: &Material) -> Result<Arc<Login>, oauth::Error> {
    let mut logins = logins();
    if let Some(l) = logins.get(&version.id).filter(|l| l.version == *version && l.material.realm == material.realm) { return Ok(l.clone()) }
    let l = Arc::new(Login::new(material.clone(), version.clone())?);
    logins.insert(version.id, l.clone());
    Ok(l)
}

/// The deleted api key's login dropped, its LST with it.
pub fn forget(api_key_id: i64) { logins().remove(&api_key_id); }

/// The figures' market-data reads (Ibkr plan F1, D-IBKR-FIGURES: Rails' get_tickers_prices and get_candles answer `{}` and
/// `[]`). Their answers' shapes are provisional until a recorded session (G1).
pub const SNAPSHOT_PATH: &str = "/v1/api/iserver/marketdata/snapshot";
pub const HISTORY_PATH: &str = "/v1/api/iserver/marketdata/history";
/// Exchanges::Ibkr::FIELD_LAST, and the market-data availability field (`N…`: the login has no subscription for it).
pub const FIELD_LAST: &str = "31";
pub const FIELD_AVAILABILITY: &str = "6509";

/// One market-data GET for the figures' cache, under the login's lock (IbkrLock) for the whole read, as a raw answer: a JSON
/// answer as its text, an HTTP error status as that answer. A busy login is NotSent; a failure of the session itself (its
/// mint, an unreadable answer) Permanent.
pub async fn read<T: Transport, C: Clock + ?Sized>(session: &Session<'_, T, C>, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
    let held = session.lock().await.map_err(|e| TransportError::NotSent(e.message()))?;
    match session.signed_request(&held, "GET", &r.path, r.query.clone(), None).await {
        Ok(v) => Ok(HttpResponse { status: 200, body: v.to_string() }),
        Err(Error::Http { status, body }) => Ok(HttpResponse { status, body }),
        Err(Error::Transport(e)) => Err(e),
        Err(e) => Err(TransportError::Permanent(session.transport.redact_diagnostic(&e.message()))),
    }
}

/// What a snapshot answer says of `conid`'s last price.
#[derive(Clone, Debug, PartialEq)]
pub enum Last { Price(String), Unsubscribed, Absent }

/// Exchanges::Ibkr#snapshot_field for the figures: `conid`'s own row only (Rails falls back to the first row, which could
/// be another contract's), its field 31 without the `C` (previous close) or `H` (halted) prefix. No row, no field 31 (the
/// pre-flight answer IBKR gives a first read) or an empty one is Absent; availability `N…` is Unsubscribed.
pub fn snapshot_last(body: &Value, conid: &str) -> Last {
    let Value::Array(rows) = body else { return Last::Absent };
    let id = |r: &Value| match &r["conid"] { Value::Number(n) => n.to_string(), Value::String(s) => s.clone(), _ => String::new() };
    let Some(row) = rows.iter().find(|r| id(r) == conid) else { return Last::Absent };
    if row[FIELD_AVAILABILITY].as_str().is_some_and(|a| a.starts_with('N')) { return Last::Unsubscribed }
    let text = match &row[FIELD_LAST] { Value::String(s) => s.strip_prefix(['C', 'H']).unwrap_or(s).to_string(), Value::Number(n) => n.to_string(), _ => return Last::Absent };
    if text.is_empty() { Last::Absent } else { Last::Price(text) }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// IbkrLock: the login stayed busy past LOCK_WAIT.
    LockTimeout,
    Transport(TransportError),
    /// Faraday's raise_error: a 4xx or 5xx answer.
    Http { status: u16, body: String },
    /// Session::AuthError: the LST answer had no DH response, or the LST failed validation.
    Auth(&'static str),
    /// The access token secret does not decrypt with the encryption key (Rails raises OpenSSL's error, never retried).
    Key(&'static str),
    /// Faraday::ParsingError: a JSON answer that does not parse (never retried).
    Unreadable,
    /// An order or reply POST handed to `signed_request`, which may resend it; it goes through `Session::submit`.
    Mutation,
}

impl Error {
    /// Session#signed_request's rescue: Faraday::UnauthorizedError or AuthError.
    fn heals(&self) -> bool { matches!(self, Self::Http { status: 401, .. } | Self::Auth(_)) }

    pub(crate) fn message(self) -> String {
        match self {
            Self::LockTimeout => "the IBKR login stayed busy".into(),
            Self::Transport(TransportError::NotSent(m) | TransportError::MaybeSent(m) | TransportError::Permanent(m)) => m,
            Self::Http { status, body } => if body.is_empty() { format!("the server responded with status {status}") } else { body },
            Self::Auth(m) | Self::Key(m) => m.into(),
            Self::Unreadable => "the answer is not readable JSON".into(),
            Self::Mutation => "an order or reply sent as a read".into(),
        }
    }
}

/// An order placement or confirmation reply (POST `…/orders`, `…/reply/<id>`): IBKR takes no idempotency key, so one
/// is sent at most once (D-IBKR-SUBMIT). The order preview (`…/orders/whatif`) and the live-orders read (GET) are not.
fn is_mutation(method: &str, path: &str) -> bool { method == "POST" && (path.ends_with("/orders") || path.contains("/reply/")) }

/// Proof that the caller holds its login's lock for the whole operation (Clients::Ibkr wraps each op in IbkrLock).
pub struct Held<'a>(#[allow(dead_code)] tokio::sync::MutexGuard<'a, ()>);

/// Clients::Ibkr::Session for one login over `transport`, its clock `clock`.
pub struct Session<'a, T: Transport, C: Clock + ?Sized = dyn Clock> {
    transport: &'a T,
    base: String,
    login: Arc<Login>,
    clock: &'a C,
    dh_random: Box<dyn Fn() -> String + Send + Sync + 'a>,
}

impl<'a, T: Transport, C: Clock + ?Sized> Session<'a, T, C> {
    pub fn new(transport: &'a T, login: Arc<Login>, clock: &'a C) -> Self {
        let fixed = FIXED_DH_RANDOM.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let dh_random: Box<dyn Fn() -> String + Send + Sync + 'a> = match fixed { Some(r) => Box::new(move || r.clone()), None => Box::new(oauth::dh_random) };
        Self { transport, base: URL.into(), login, clock, dh_random }
    }

    /// Sent to `base` instead of Session::BASE_URL (a test's server).
    pub fn with_base(mut self, base: &str) -> Self { if !base.is_empty() { self.base = base.into(); } self }

    /// Oauth.dh_random fixed (the recorders stub it the same way).
    pub fn with_dh_random(mut self, f: impl Fn() -> String + Send + Sync + 'a) -> Self { self.dh_random = Box::new(f); self }

    /// The session's clock reading.
    pub fn now(&self) -> DateTime<Utc> { self.clock.now() }

    /// `text` without the credential values the transport knows.
    pub fn redact(&self, text: &str) -> String { self.transport.redact_diagnostic(text) }

    /// IbkrLock.with_lock: waits up to LOCK_WAIT for the login, then fails.
    pub async fn lock(&self) -> Result<Held<'_>, Error> {
        tokio::time::timeout(LOCK_WAIT, self.login.op.lock()).await.map(Held).map_err(|_| Error::LockTimeout)
    }

    /// Session#signed_request: the brokerage session established, the call signed, one self-heal on a 401 or AuthError.
    /// Returns the answer's JSON; an answer whose Content-Type is not JSON is its body as a string (Faraday's json middleware,
    /// `content_type: /\bjson$/`).
    /// Reads only: an order or reply POST is refused (`submit` sends those).
    pub async fn signed_request(&self, _held: &Held<'_>, method: &'static str, path: &str, query: Vec<(&'static str, String)>, body: Option<Value>) -> Result<Value, Error> {
        debug_assert!(!is_mutation(method, path), "{method} {path} goes through Session::submit");
        if is_mutation(method, path) { return Err(Error::Mutation) }
        match self.attempt(method, path, &query, &body).await {
            Err(e) if e.heals() => {
                self.login.invalidate();
                self.attempt(method, path, &query, &body).await
            }
            r => r,
        }
    }

    /// An order or reply POST, sent exactly once (D-IBKR-SUBMIT). The steps before it (the LST, the brokerage session and
    /// the account list IBKR wants read first) heal as `signed_request` does; a failure there sent nothing (Transient).
    /// The POST itself is never healed or resent: a failure proven to precede sending is Transient (NotSent) or Rejected
    /// (Permanent: a TLS or local refusal while connecting); any other failure — an HTTP error status (401 included), a
    /// timeout, a lost connection, an unreadable answer — leaves the outcome open: Ambiguous, for recovery to settle.
    pub async fn submit(&self, held: &Held<'_>, path: &str, body: Value) -> Result<Value, VenueError> {
        let redact = |e: Error| self.transport.redact_diagnostic(&e.message());
        self.signed_request(held, "GET", ACCOUNTS_PATH, vec![], None).await.map_err(|e| VenueError::Transient(redact(e)))?;
        self.live_session_token().await.map_err(|e| VenueError::Transient(redact(e)))?;
        match self.request(self.http("POST", path, vec![], Some(body))).await {
            Ok(v) => Ok(v),
            Err(Error::Transport(TransportError::NotSent(m))) => Err(VenueError::Transient(self.transport.redact_diagnostic(&m))),
            Err(Error::Transport(TransportError::Permanent(m))) => Err(VenueError::Rejected(vec![self.transport.redact_diagnostic(&m)])),
            Err(e) => Err(VenueError::Ambiguous(redact(e))),
        }
    }

    async fn attempt(&self, method: &'static str, path: &str, query: &[(&'static str, String)], body: &Option<Value>) -> Result<Value, Error> {
        self.ensure_brokerage_session().await?;
        self.signed_call(method, path, query.to_vec(), body.clone()).await
    }

    /// Session#ensure_brokerage_session: `compete: true` takes over any other brokerage session of this login.
    async fn ensure_brokerage_session(&self) -> Result<(), Error> {
        if self.login.state().brokerage_until.is_some_and(|until| self.clock.now() < until) { return Ok(()) }
        self.signed_call("POST", SSODH_INIT_PATH, vec![], Some(json!({ "publish": true, "compete": true }))).await?;
        self.login.state().brokerage_until = Some(self.clock.now() + BROKERAGE_SESSION_TTL);
        Ok(())
    }

    /// Session#live_session_token: the cached LST, or a fresh mint.
    pub async fn live_session_token(&self) -> Result<String, Error> {
        let now = self.clock.now();
        if let Some((lst, _)) = self.login.state().lst.as_ref().filter(|(_, until)| now < *until) { return Ok(lst.clone()) }
        self.mint().await
    }

    /// Session#mint_live_session_token.
    async fn mint(&self) -> Result<String, Error> {
        let signer = &self.login.signer;
        let prepend = signer.prepend().ok_or(Error::Key("the access token secret does not decrypt"))?;
        let dh_random = (self.dh_random)();
        let challenge = signer.dh_challenge(&dh_random).ok_or(Error::Key("the DH random is not hex"))?;
        self.login.state().minting = Some((challenge, prepend.clone()));
        let answer = self.request(self.http("POST", LST_PATH, vec![], None)).await;
        self.login.state().minting = None;
        let answer = answer?;
        let dh_response = answer.get("diffie_hellman_response").and_then(Value::as_str).filter(|s| !crate::ruby::blank(s))
            .ok_or(Error::Auth("live_session_token: missing diffie_hellman_response"))?;
        let invalid = Error::Auth("live_session_token failed validation");
        let lst = signer.live_session_token(&dh_random, dh_response, &prepend).ok_or(invalid.clone())?;
        // `body['live_session_token_signature'].to_s`
        let signature = match answer.get("live_session_token_signature") { Some(Value::String(s)) => s.clone(), None | Some(Value::Null) => String::new(), Some(v) => v.to_string() };
        if !oauth::validate_live_session_token(&lst, &signature, signer.consumer_key()) { return Err(invalid) }
        self.login.state().lst = Some((lst.clone(), self.clock.now() + LST_TTL));
        Ok(lst)
    }

    /// Session#signed_call: the LST in hand, then the call (signed by the transport at send time).
    async fn signed_call(&self, method: &'static str, path: &str, query: Vec<(&'static str, String)>, body: Option<Value>) -> Result<Value, Error> {
        self.live_session_token().await?;
        self.request(self.http(method, path, query, body)).await
    }

    fn http(&self, method: &'static str, path: &str, query: Vec<(&'static str, String)>, body: Option<Value>) -> HttpRequest {
        HttpRequest { method, base: self.base.clone(), path: path.into(), query, body, not_after: None }
    }

    async fn request(&self, r: HttpRequest) -> Result<Value, Error> {
        let (resp, content_type) = self.transport.send_typed(&r).await.map_err(Error::Transport)?;
        if resp.status >= 400 { return Err(Error::Http { status: resp.status, body: resp.body }) }
        if !content_type.as_deref().is_none_or(is_json) { return Ok(Value::String(resp.body)) }
        // Faraday::Response::Json#parse: a blank body is nil.
        if resp.body.trim().is_empty() { return Ok(Value::Null) }
        decode_json(&resp.body).map_err(|_| Error::Unreadable)
    }
}

/// For tests that reach a scripted IBKR over HTTP: every new Session mints with this DH random (Oauth.dh_random stubbed,
/// as the recorders do), so a recorded server answer validates. None restores the random one.
static FIXED_DH_RANDOM: Mutex<Option<String>> = Mutex::new(None);
#[doc(hidden)]
pub fn fix_dh_random(dh_random: Option<String>) { *FIXED_DH_RANDOM.lock().unwrap_or_else(|p| p.into_inner()) = dh_random; }

/// Faraday's `content_type: /\bjson$/` over the MIME type without its parameters.
fn is_json(content_type: &str) -> bool {
    let mime = content_type.split(';').next().unwrap_or_default().trim();
    mime.strip_suffix("json").is_some_and(|head| !head.ends_with(|c: char| c.is_alphanumeric() || c == '_'))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::FixedClock;
    use crate::venue::http::{HttpResponse, ScriptedTransport};

    /// I0's recorded ffdhe2048 session: the fixed keys, Rails' frozen DH random and the scripted server's answer.
    fn fixture() -> (Material, Value) {
        let f: Value = serde_json::from_str(include_str!("../../tests/fixtures/ibkr_signing.json")).unwrap();
        let k = &f["keys"];
        let s = |n: &str| k[n].as_str().unwrap().to_string();
        let session = f["signing"][1].clone();
        let m = Material { consumer_key: s("consumer_key"), access_token: s("access_token"), access_token_secret: s("access_token_secret"),
                           signature_key: s("signature_key"), encryption_key: s("encryption_key"), dh_param: session["dh_param"].as_str().unwrap().into(), realm: s("realm") };
        (m, session)
    }

    fn version(id: i64, tag: &str) -> CredentialVersion {
        crate::engine::model::prepared_credential_version(id, &[Some(tag.into()), None, None, None, None, None, None])
    }

    pub(crate) fn new_login() -> (Arc<Login>, Value) {
        let (m, session) = fixture();
        (Arc::new(Login::with_op(m, version(1, "a"), Arc::default()).unwrap()), session)
    }

    const ACCOUNTS: &str = "GET /v1/api/iserver/accounts";
    const MINT: &str = "POST /v1/api/oauth/live_session_token";
    const INIT: &str = "POST /v1/api/iserver/auth/ssodh/init";

    pub(crate) fn scripted(session: &Value) -> ScriptedTransport {
        let t = ScriptedTransport::default();
        t.reply(MINT, 200, json!({ "diffie_hellman_response": session["server"]["diffie_hellman_response"],
                                   "live_session_token_signature": session["server"]["live_session_token_signature"],
                                   "live_session_token_expiration": 1 }));
        t.reply(INIT, 200, json!({ "authenticated": true, "connected": true, "competing": false }));
        t
    }

    fn paths(t: &ScriptedTransport) -> Vec<String> { t.requests().iter().map(|r| format!("{} {}", r.method, r.path)).collect() }

    async fn call<T: Transport, C: Clock + ?Sized>(s: &Session<'_, T, C>) -> Result<Value, Error> {
        let held = s.lock().await?;
        s.signed_request(&held, "GET", "/v1/api/iserver/accounts", vec![], None).await
    }

    pub(crate) fn at(t: &str) -> FixedClock { FixedClock(t.parse().unwrap()) }

    #[tokio::test]
    async fn mints_then_opens_the_brokerage_session_then_calls_and_reuses_both() {
        let (login, session) = new_login();
        let t = scripted(&session);
        t.reply(ACCOUNTS, 200, json!({ "accounts": ["U1"] }));
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login.clone(), &clock).with_dh_random(|| dh_random.clone());
        assert_eq!(call(&s).await.unwrap(), json!({ "accounts": ["U1"] }));
        assert_eq!(paths(&t), [MINT, INIT, ACCOUNTS]);
        assert_eq!(t.requests()[1].body, Some(json!({ "publish": true, "compete": true })));
        assert_eq!(login.state().lst.as_ref().unwrap().0, session["server"]["lst"].as_str().unwrap(), "the LST Rails computes");

        // Within 4 minutes: the cached LST and brokerage session, one call.
        let later = at("2026-10-23T10:03:59Z");
        call(&Session::new(&t, login.clone(), &later)).await.unwrap();
        assert_eq!(paths(&t).len(), 4);
        // The flag has lapsed: ssodh/init again, the LST still cached.
        let later = at("2026-10-23T10:04:00Z");
        call(&Session::new(&t, login.clone(), &later)).await.unwrap();
        assert_eq!(paths(&t)[4..], [INIT, ACCOUNTS]);
    }

    #[tokio::test]
    async fn an_expired_lst_is_minted_again() {
        let (login, session) = new_login();
        let t = scripted(&session);
        t.reply(ACCOUNTS, 200, json!([]));
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let first = at("2026-10-23T10:00:00Z");
        call(&Session::new(&t, login.clone(), &first).with_dh_random(|| dh_random.clone())).await.unwrap();
        let almost = at("2026-10-24T08:59:59Z");
        call(&Session::new(&t, login.clone(), &almost)).await.unwrap();
        assert_eq!(paths(&t).iter().filter(|p| *p == MINT).count(), 1, "23 h less a second: cached");
        let expired = at("2026-10-24T09:00:00Z");
        call(&Session::new(&t, login.clone(), &expired).with_dh_random(|| dh_random.clone())).await.unwrap();
        // The brokerage flag set at 08:59:59 is still live: only the LST is minted again.
        assert_eq!(paths(&t)[paths(&t).len() - 2..], [MINT, ACCOUNTS]);
    }

    #[tokio::test]
    async fn a_401_heals_once_by_minting_and_opening_again() {
        let (login, session) = new_login();
        let t = scripted(&session);
        t.reply(ACCOUNTS, 401, json!({ "error": "not authenticated" })).reply(ACCOUNTS, 200, json!({ "accounts": ["U1"] }));
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login, &clock).with_dh_random(|| dh_random.clone());
        assert_eq!(call(&s).await.unwrap(), json!({ "accounts": ["U1"] }));
        assert_eq!(paths(&t), [MINT, INIT, ACCOUNTS, MINT, INIT, ACCOUNTS]);
    }

    #[tokio::test]
    async fn a_second_401_fails() {
        let (login, session) = new_login();
        let t = scripted(&session);
        t.reply(ACCOUNTS, 401, json!({}));
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login.clone(), &clock).with_dh_random(|| dh_random.clone());
        assert_eq!(call(&s).await, Err(Error::Http { status: 401, body: "{}".into() }));
        assert_eq!(paths(&t), [MINT, INIT, ACCOUNTS, MINT, INIT, ACCOUNTS]);
    }

    #[tokio::test]
    async fn an_lst_that_fails_validation_heals_once_then_fails() {
        let (login, session) = new_login();
        let t = ScriptedTransport::default();
        t.reply(MINT, 200, json!({ "diffie_hellman_response": session["server"]["diffie_hellman_response"], "live_session_token_signature": "00" }));
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login.clone(), &clock).with_dh_random(|| dh_random.clone());
        assert_eq!(call(&s).await, Err(Error::Auth("live_session_token failed validation")));
        assert_eq!(paths(&t), [MINT, MINT]);
        assert!(login.state().lst.is_none() && login.state().minting.is_none());
    }

    #[tokio::test]
    async fn a_timeout_is_not_healed() {
        let (login, session) = new_login();
        let t = scripted(&session);
        t.network(ACCOUNTS, "post_send", "operation timed out");
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login, &clock).with_dh_random(|| dh_random.clone());
        assert_eq!(call(&s).await, Err(Error::Transport(TransportError::MaybeSent("operation timed out".into()))));
        assert_eq!(paths(&t), [MINT, INIT, ACCOUNTS]);
    }

    const ORDERS: &str = "POST /v1/api/iserver/account/U1/orders";
    const ORDER_PATH: &str = "/v1/api/iserver/account/U1/orders";

    async fn submit<T: Transport, C: Clock + ?Sized>(s: &Session<'_, T, C>) -> Result<Value, VenueError> {
        let held = s.lock().await.unwrap();
        s.submit(&held, ORDER_PATH, json!({ "orders": [{ "conid": 265598, "side": "BUY", "quantity": 1 }] })).await
    }

    fn submitting(f: impl FnOnce(&ScriptedTransport)) -> (Result<Value, VenueError>, Vec<String>) {
        let (login, session) = new_login();
        let t = scripted(&session);
        f(&t);
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login, &clock).with_dh_random(|| dh_random.clone());
        let r = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(submit(&s));
        (r, paths(&t))
    }

    #[test]
    fn an_order_is_posted_once_after_the_account_list() {
        let (r, sent) = submitting(|t| { t.reply(ACCOUNTS, 200, json!({ "accounts": ["U1"] })).reply(ORDERS, 200, json!([{ "order_id": "1" }])); });
        assert_eq!(r, Ok(json!([{ "order_id": "1" }])));
        assert_eq!(sent, [MINT, INIT, ACCOUNTS, ORDERS]);
    }

    #[test]
    fn a_401_on_the_order_post_is_ambiguous_and_never_resent() {
        let (r, sent) = submitting(|t| { t.reply(ACCOUNTS, 200, json!({})).reply(ORDERS, 401, json!({ "error": "not authenticated" })).reply(ORDERS, 200, json!([])); });
        assert!(matches!(r, Err(VenueError::Ambiguous(_))));
        assert_eq!(sent, [MINT, INIT, ACCOUNTS, ORDERS], "one POST, no heal");
    }

    #[test]
    fn a_401_before_the_post_heals_then_posts_once() {
        let (r, sent) = submitting(|t| { t.reply(ACCOUNTS, 401, json!({})).reply(ACCOUNTS, 200, json!({})).reply(ORDERS, 200, json!([])); });
        assert_eq!(r, Ok(json!([])));
        assert_eq!(sent, [MINT, INIT, ACCOUNTS, MINT, INIT, ACCOUNTS, ORDERS]);
    }

    #[test]
    fn a_failed_pre_step_sends_no_order_and_is_retryable() {
        let (r, sent) = submitting(|t| { t.reply(ACCOUNTS, 401, json!({})); });
        assert!(matches!(r, Err(VenueError::Transient(_))));
        assert!(!sent.iter().any(|p| p == ORDERS));
    }

    #[test]
    fn a_timeout_a_5xx_or_an_unreadable_answer_on_the_post_is_ambiguous() {
        for script in [
            (|t: &ScriptedTransport| { t.network(ORDERS, "post_send", "operation timed out"); }) as fn(&ScriptedTransport),
            |t| { t.reply(ORDERS, 503, json!({})); },
            |t| { t.reply_typed(ORDERS, 200, "application/json", "{"); },
        ] {
            let (r, sent) = submitting(|t| { t.reply(ACCOUNTS, 200, json!({})); script(t); t.reply(ORDERS, 200, json!([])); });
            assert!(matches!(r, Err(VenueError::Ambiguous(_))));
            assert_eq!(sent, [MINT, INIT, ACCOUNTS, ORDERS]);
        }
    }

    #[test]
    fn a_post_provably_not_sent_is_retryable() {
        let (r, sent) = submitting(|t| { t.reply(ACCOUNTS, 200, json!({})).network(ORDERS, "pre_send", "connection refused"); });
        assert_eq!(r, Err(VenueError::Transient("connection refused".into())));
        assert_eq!(sent, [MINT, INIT, ACCOUNTS, ORDERS]);
        let (r, _) = submitting(|t| { t.reply(ACCOUNTS, 200, json!({})).network(ORDERS, "permanent", "certificate rejected"); });
        assert_eq!(r, Err(VenueError::Rejected(vec!["certificate rejected".into()])));
    }

    #[test]
    fn only_order_and_reply_posts_are_mutations() {
        assert!(is_mutation("POST", ORDER_PATH) && is_mutation("POST", "/v1/api/iserver/reply/abc"));
        assert!(!is_mutation("GET", "/v1/api/iserver/account/orders") && !is_mutation("POST", "/v1/api/iserver/account/U1/orders/whatif"));
        assert!(!is_mutation("POST", SSODH_INIT_PATH) && !is_mutation("DELETE", "/v1/api/iserver/account/U1/order/1"));
    }

    #[cfg(debug_assertions)]
    #[tokio::test]
    #[should_panic(expected = "goes through Session::submit")]
    async fn signed_request_refuses_an_order_post() {
        let (login, session) = new_login();
        let t = scripted(&session);
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login, &clock);
        let held = s.lock().await.unwrap();
        let _ = s.signed_request(&held, "POST", ORDER_PATH, vec![], Some(json!({}))).await;
    }

    #[cfg(not(debug_assertions))]
    #[tokio::test]
    async fn signed_request_refuses_an_order_post() {
        let (login, session) = new_login();
        let t = scripted(&session);
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login, &clock);
        let held = s.lock().await.unwrap();
        assert_eq!(s.signed_request(&held, "POST", "/v1/api/iserver/reply/x", vec![], Some(json!({}))).await, Err(Error::Mutation));
        assert!(t.requests().is_empty());
    }

    /// Yields before every answer, so two callers interleave at each request.
    struct Yielding(ScriptedTransport);
    impl Transport for Yielding {
        async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
            tokio::task::yield_now().await;
            self.0.send(r).await
        }
        async fn send_typed(&self, r: &HttpRequest) -> Result<(HttpResponse, Option<String>), TransportError> {
            tokio::task::yield_now().await;
            self.0.send_typed(r).await
        }
    }

    #[tokio::test]
    async fn two_callers_share_one_mint_and_one_brokerage_session() {
        let (login, session) = new_login();
        let t = Yielding(scripted(&session));
        t.0.reply(ACCOUNTS, 200, json!([]));
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login, &clock).with_dh_random(|| dh_random.clone());
        let (a, b) = tokio::join!(call(&s), call(&s));
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(paths(&t.0), [MINT, INIT, ACCOUNTS, ACCOUNTS]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_busy_login_times_out_after_15_seconds() {
        let (login, session) = new_login();
        let t = scripted(&session);
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login, &clock);
        let _held = s.lock().await.unwrap();
        let started = tokio::time::Instant::now();
        assert!(matches!(s.lock().await, Err(Error::LockTimeout)));
        assert_eq!(started.elapsed(), LOCK_WAIT);
        assert!(t.requests().is_empty());
    }

    #[tokio::test]
    async fn a_json_answer_that_does_not_parse_fails_and_other_types_stay_text() {
        let (login, session) = new_login();
        let t = scripted(&session);
        t.reply_typed(ACCOUNTS, 200, "application/json; charset=utf-8", "{not json").reply_typed(ACCOUNTS, 200, "text/html", "<p>ok</p>");
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login, &clock).with_dh_random(|| dh_random.clone());
        assert_eq!(call(&s).await, Err(Error::Unreadable));
        assert_eq!(paths(&t), [MINT, INIT, ACCOUNTS], "a parse failure is not healed");
        assert_eq!(call(&s).await, Ok(Value::String("<p>ok</p>".into())));
        assert!(is_json("application/vnd.api+json") && !is_json("application/geojson") && !is_json("") && !is_json("text/json-ish"));
    }

    #[tokio::test]
    async fn an_unreadable_ssodh_init_answer_leaves_the_brokerage_session_closed() {
        let (login, session) = new_login();
        let t = ScriptedTransport::default();
        t.reply(MINT, 200, json!({ "diffie_hellman_response": session["server"]["diffie_hellman_response"],
                                   "live_session_token_signature": session["server"]["live_session_token_signature"] }));
        t.reply_typed(INIT, 200, "application/json", "{").reply(INIT, 200, json!({ "authenticated": true }));
        t.reply(ACCOUNTS, 200, json!([]));
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login.clone(), &clock).with_dh_random(|| dh_random.clone());
        assert_eq!(call(&s).await, Err(Error::Unreadable));
        assert!(login.state().brokerage_until.is_none());
        call(&s).await.unwrap();
        assert_eq!(paths(&t), [MINT, INIT, INIT, ACCOUNTS], "init again, the LST kept");
    }

    #[tokio::test]
    async fn two_rows_of_one_consumer_key_serialise_their_operations() {
        let (m, session) = fixture();
        let op: Op = Arc::default();
        let a = Arc::new(Login::with_op(m.clone(), version(1, "a"), op.clone()).unwrap());
        let b = Arc::new(Login::with_op(m, version(2, "b"), op).unwrap());
        let t = Yielding(scripted(&session));
        t.0.reply(ACCOUNTS, 200, json!([]));
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let sa = Session::new(&t, a, &clock).with_dh_random(|| dh_random.clone());
        let sb = Session::new(&t, b, &clock).with_dh_random(|| dh_random.clone());
        let (x, y) = tokio::join!(call(&sa), call(&sb));
        assert!(x.is_ok() && y.is_ok());
        // Each row keeps its own LST (Rails' `ibkr:lst:<id>`), and the two operations never interleave.
        assert_eq!(paths(&t.0), [MINT, INIT, ACCOUNTS, MINT, INIT, ACCOUNTS]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_lock_is_the_consumer_keys_across_rows_and_credential_changes() {
        let (m, session) = fixture();
        let t = scripted(&session);
        let clock = at("2026-10-23T10:00:00Z");
        let old = login(&version(9101, "a"), &m).unwrap();
        let s_old = Session::new(&t, old, &clock);
        let held = s_old.lock().await.unwrap();
        // The key replaced mid-operation, and a second row of the same consumer key: both wait for the old operation.
        for id_tag in [(9101, "b"), (9102, "a")] {
            let fresh = login(&version(id_tag.0, id_tag.1), &m).unwrap();
            assert!(matches!(Session::new(&t, fresh, &clock).lock().await, Err(Error::LockTimeout)), "{id_tag:?}");
        }
        drop(held);
        assert!(Session::new(&t, login(&version(9101, "b"), &m).unwrap(), &clock).lock().await.is_ok());
        forget(9101);
        forget(9102);
    }

    #[test]
    fn a_realm_change_rebuilds_the_login() {
        let (mut m, _) = fixture();
        let a = login(&version(9201, "a"), &m).unwrap();
        m.realm = "test_realm".into();
        let b = login(&version(9201, "a"), &m).unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
        let call = HttpRequest { method: "GET", base: URL.into(), path: "/x".into(), query: vec![], body: None, not_after: None };
        b.state().lst = Some(("AAAA".into(), Utc::now()));
        assert!(b.authorization(&call).unwrap().starts_with("OAuth realm=\"test_realm\", "));
        assert!(Arc::ptr_eq(&b, &login(&version(9201, "a"), &m).unwrap()));
        forget(9201);
    }

    #[test]
    fn the_store_keeps_a_login_until_its_credentials_change_or_the_key_is_deleted() {
        let (m, _) = fixture();
        let a = login(&version(9001, "a"), &m).unwrap();
        a.state().lst = Some(("x".into(), Utc::now()));
        assert!(Arc::ptr_eq(&a, &login(&version(9001, "a"), &m).unwrap()));
        let b = login(&version(9001, "b"), &m).unwrap();
        assert!(!Arc::ptr_eq(&a, &b) && b.state().lst.is_none(), "a changed key starts with no session");
        forget(9001);
        assert!(!Arc::ptr_eq(&b, &login(&version(9001, "b"), &m).unwrap()));
        forget(9001);
    }

    #[test]
    fn the_wire_signs_the_mint_and_calls_at_send_time() {
        let (login, session) = new_login();
        let lst_req = HttpRequest { method: "POST", base: URL.into(), path: LST_PATH.into(), query: vec![], body: None, not_after: None };
        let call = HttpRequest { method: "GET", base: URL.into(), path: "/v1/api/portfolio/U1/positions/0".into(), query: vec![("model", "x y".into())], body: None, not_after: None };
        assert!(!login.can_sign(&lst_req) && !login.can_sign(&call), "nothing to sign with");
        let prepend = login.signer.prepend().unwrap();
        let challenge = login.signer.dh_challenge(session["dh_random"].as_str().unwrap()).unwrap();
        login.state().minting = Some((challenge.clone(), prepend));
        let header = login.authorization(&lst_req).unwrap();
        assert!(header.starts_with("OAuth realm=\"limited_poa\", ") && header.contains(&format!("diffie_hellman_challenge=\"{challenge}\"")) && header.contains("RSA-SHA256"));
        login.state().lst = Some((session["server"]["lst"].as_str().unwrap().into(), Utc::now()));
        let header = login.authorization(&call).unwrap();
        assert!(header.contains("oauth_signature_method=\"HMAC-SHA256\"") && !header.contains("model"), "the query is signed, not sent in the header");
        let client = reqwest::Client::new();
        let keys = crate::venue::sign::Keys { key: "", secret: "", passphrase: "", ibkr: Some(&login) };
        let sent = crate::venue::sign::request(crate::venue::sign::Wire::Ibkr, &client, &call, keys, &crate::engine::SystemClock).build().unwrap();
        assert!(sent.headers()["Authorization"].to_str().unwrap().starts_with("OAuth realm="));
        assert_eq!(sent.url().as_str(), "https://api.ibkr.com/v1/api/portfolio/U1/positions/0?model=x+y");
    }

    // ---- The figures' market-data reads (F1). D-IBKR-FIGURES: Rails reads none, so every expected value here is Rust-only,
    // and the answers are provisional shapes (`source: "provisional"`) until G1 records a session.

    fn snapshot_request() -> HttpRequest {
        HttpRequest { method: "GET", base: URL.into(), path: SNAPSHOT_PATH.into(), query: vec![("conids", "265598".into()), ("fields", "31,6509".into())], body: None, not_after: None }
    }

    #[tokio::test]
    async fn a_market_data_read_is_signed_through_the_session_and_kept_raw() {
        let (login, session) = new_login();
        let t = scripted(&session);
        let answer = json!([{ "conid": 265598, "31": "190.50", "6509": "RpB", "source": "provisional" }]);
        t.reply("GET /v1/api/iserver/marketdata/snapshot", 200, answer.clone()).reply("GET /v1/api/iserver/marketdata/snapshot", 503, json!({ "error": "busy", "source": "provisional" }));
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login, &clock).with_dh_random(|| dh_random.clone());
        // Rust-only (D-IBKR-FIGURES): the answer as text, and an HTTP error status as an answer the cache settles.
        assert_eq!(read(&s, &snapshot_request()).await, Ok(HttpResponse { status: 200, body: answer.to_string() }));
        assert_eq!(read(&s, &snapshot_request()).await.unwrap().status, 503);
        assert_eq!(paths(&t), [MINT, INIT, "GET /v1/api/iserver/marketdata/snapshot", "GET /v1/api/iserver/marketdata/snapshot"]);
        assert_eq!(t.requests()[2].query, snapshot_request().query);
    }

    #[tokio::test(start_paused = true)]
    async fn a_market_data_read_waits_for_the_login_lock() {
        let (login, session) = new_login();
        let t = scripted(&session);
        let clock = at("2026-10-23T10:00:00Z");
        let s = Session::new(&t, login, &clock);
        let _held = s.lock().await.unwrap();
        // Rust-only (D-IBKR-FIGURES): a busy login is a read not sent, after IbkrLock's wait.
        assert!(matches!(read(&s, &snapshot_request()).await, Err(TransportError::NotSent(_))));
        assert!(t.requests().is_empty());
    }

    #[test]
    fn a_snapshot_names_its_conids_last_price_or_why_it_has_none() {
        // Rust-only (D-IBKR-FIGURES): Rails' snapshot_field reads field 31 of the conid's row; the rest is this reading.
        let last = |rows: Value| snapshot_last(&rows, "265598");
        assert_eq!(last(json!([{ "conid": 265598, "31": "190.50", "6509": "RpB" }])), Last::Price("190.50".into()));
        assert_eq!(last(json!([{ "conid": "265598", "31": "C189.10" }])), Last::Price("189.10".into()), "previous close");
        assert_eq!(last(json!([{ "conid": 265598, "31": "H12" }])), Last::Price("12".into()), "halted");
        assert_eq!(last(json!([{ "conid": 265598, "31": 7.5 }])), Last::Price("7.5".into()));
        assert_eq!(last(json!([{ "conid": 265598, "6509": "N", "source": "provisional" }])), Last::Unsubscribed);
        assert_eq!(last(json!([{ "conid": 265598, "31": "1", "6509": "NpB" }])), Last::Unsubscribed);
        assert_eq!(last(json!([{ "conid": 265598, "_updated": 1 }])), Last::Absent, "the pre-flight answer");
        assert_eq!(last(json!([{ "conid": 265598, "31": "" }])), Last::Absent);
        assert_eq!(last(json!([{ "conid": 8314, "31": "50" }])), Last::Absent, "another contract's row is never this one's");
        assert_eq!(last(json!({ "error": "x" })), Last::Absent);
        assert_eq!(last(json!([])), Last::Absent);
    }
}
