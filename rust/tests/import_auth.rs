//! #710 (PLAN-710-importauth r6): the import routes (`/tracker/import`, `/bots/:id/import`) read their body only for a
//! signed-in, active user. Everyone else is answered as main answers the same request with an empty form, and the body is
//! never polled (D-IMPORT-AUTH-FIRST). A sign-in that ends while the request is under way writes nothing and is never
//! sealed again.
mod common;
use axum::body::{Body, Bytes};
use axum::extract::ConnectInfo;
use axum::http::{header, Request};
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec};
use common::web::{self, TestClock};
use deltabadger::web::{csrf, i18n, session, App};
use futures_util::StreamExt;
use rusqlite::hooks::{AuthAction, TransactionOperation};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const T0: &str = "2026-09-01T12:30:00Z";
const HEADER: &str = "Timestamp,Order ID,Type,Side,Amount,Value,Price,Base Asset,Quote Asset,Status";
const ROW: &str = "2026-08-01 10:00:00 UTC,x,Market,Buy,0.001,60,60000,BTC,USD,Closed";
const MIB: usize = 1024 * 1024;

struct Install { d: tempfile::TempDir, o: deltabadger::store::Opened, user: i64, salt: String, bot: i64, clock: Arc<TestClock>, app: App }

/// A configured install: its admin (confirmed) and one stopped Alpaca basket of theirs.
fn install() -> Install {
    let (d, o, s) = common::install_alpaca();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    o.primary.execute("UPDATE users SET confirmed_at='2026-01-01', wash_sale_enabled=0", []).unwrap();
    let hash: String = o.primary.query_row("SELECT encrypted_password FROM users WHERE id=?1", [s.user_id], |r| r.get(0)).unwrap();
    let clock = TestClock::at(T0);
    let app = web::app(d.path(), web::SECRET, clock.clone());
    Install { salt: hash.chars().take(29).collect(), user: s.user_id, bot, d, o, clock, app }
}

impl Install {
    fn now(&self) -> DateTime<Utc> { self.app.now() }
    /// A cookie sealed at `at` (it expires LIFETIME_SECONDS later) for `data`.
    fn cookie_at(&self, data: &session::SessionData, at: DateTime<Utc>) -> String { session::seal(&self.app.keys.session, data, at) }
    fn signed_in(&self, csrf: &str) -> session::SessionData {
        session::SessionData { user: Some((self.user, self.salt.clone())), csrf: Some(csrf.to_string()), ..Default::default() }
    }
    fn rows(&self) -> i64 { self.o.primary.query_row("SELECT count(*) FROM transactions", [], |r| r.get(0)).unwrap() }
    /// Every table's row count but the users' (a test may change a password): an import adds rows.
    fn snapshot(&self) -> Vec<(String, i64)> {
        let names: Vec<String> = self.o.primary.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name <> 'users' ORDER BY name").unwrap()
            .query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        names.into_iter().map(|name| { let n = self.o.primary.query_row(&format!("SELECT count(*) FROM \"{name}\""), [], |r| r.get(0)).unwrap(); (name, n) }).collect()
    }
    fn bot_path(&self) -> String { format!("/bots/{}/import", self.bot) }

    /// Runs `action` at the `nth` (1-based) read of `users.admin` once `armed` (None: from now on). Both the setup lookup
    /// and the account lookup read that column, once each: the first read after the body is the one that comes first.
    async fn at_admin_read(&self, armed: Option<Arc<AtomicBool>>, nth: usize, action: impl FnMut() + Send + 'static) {
        self.at(armed, nth, |action| matches!(action, AuthAction::Read { table_name: "users", column_name: "admin" }), action).await
    }

    /// Runs `action` at the `nth` statement the app's connection prepares that `is` picks, once `armed`.
    async fn at(&self, armed: Option<Arc<AtomicBool>>, nth: usize, is: fn(&AuthAction<'_>) -> bool, action: impl FnMut() + Send + 'static) {
        let action = Mutex::new(Some(action));
        let seen = AtomicUsize::new(0);
        self.app.db(move |c| {
            c.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
                if is(&context.action) && armed.as_ref().is_none_or(|armed| armed.load(Ordering::SeqCst)) && seen.fetch_add(1, Ordering::SeqCst) + 1 == nth {
                    if let Some(mut action) = action.lock().unwrap().take() { action(); }
                }
                rusqlite::hooks::Authorization::Allow
            }));
            Ok(())
        }).await.unwrap();
    }
}

/// A body of `size` bytes (in 64 KiB chunks, the last one shorter) that counts how often it is polled, and runs `first`
/// when it is polled the first time.
fn counted(content: Vec<u8>, polls: Arc<AtomicUsize>, first: impl FnOnce() + Send + 'static) -> Body {
    let chunks: Vec<Bytes> = content.chunks(64 * 1024).map(Bytes::copy_from_slice).collect();
    let mut first = Some(first);
    Body::from_stream(futures_util::stream::iter(chunks).map(move |chunk| {
        if polls.fetch_add(1, Ordering::SeqCst) == 0 { if let Some(first) = first.take() { first(); } }
        Ok::<_, std::convert::Infallible>(chunk)
    }))
}

/// `size` bytes of filler that is never read: one chunk, shared (an 8 MiB body costs nothing to build).
fn filler(size: usize, polls: Arc<AtomicUsize>) -> Body {
    let chunk = Bytes::from(vec![b'a'; 64 * 1024]);
    Body::from_stream(futures_util::stream::iter(0..size / chunk.len()).map(move |_| {
        polls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, std::convert::Infallible>(chunk.clone())
    }))
}

#[derive(Debug, PartialEq)]
struct Sent { status: u16, location: Option<String>, session: Option<session::SessionData>, cookie: Option<String>, body: String, retry_after: Option<String>, content_type: Option<String> }
impl Sent {
    fn flash(&self) -> Vec<(String, String)> { self.session.as_ref().map(|s| s.flash.clone()).unwrap_or_default() }
    /// What a test compares: everything but the new cookie's bytes (a fresh nonce each time).
    fn seen(&self) -> (u16, Option<&str>, Option<&session::SessionData>, &str) { (self.status, self.location.as_deref(), self.session.as_ref(), &self.body) }
}

async fn send(app: &App, method: &str, path: &str, cookies: &[&str], headers: &[(&str, &str)], body: Body) -> Sent {
    let mut request = Request::builder().method(method).uri(path).header(header::HOST, web::HOST);
    if !cookies.is_empty() {
        request = request.header(header::COOKIE, cookies.iter().map(|c| format!("{}={c}", session::COOKIE)).collect::<Vec<_>>().join("; "));
    }
    for (name, value) in headers { request = request.header(*name, *value); }
    let mut request = request.body(body).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    let response = deltabadger::web::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let location = response.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().trim_start_matches("http://localhost:3000").to_string());
    let cookie = response.headers().get_all(header::SET_COOKIE).iter().filter_map(|v| v.to_str().unwrap().strip_prefix(&format!("{}=", session::COOKIE)))
        .map(|rest| rest.split(';').next().unwrap().to_string()).next_back();
    let session = cookie.as_ref().map(|c| session::open(&app.keys.session, c, app.now()).expect("the cookie written opens"));
    let (retry_after, content_type) = [header::RETRY_AFTER, header::CONTENT_TYPE].map(|name| response.headers().get(name).map(|v| v.to_str().unwrap().to_string())).into();
    let body = String::from_utf8_lossy(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).into_owned();
    Sent { status, location, session, cookie, body, retry_after, content_type }
}

fn csv() -> Vec<u8> { format!("{HEADER}\n{ROW}\n").into_bytes() }
fn t(locale: &str, key: &str) -> String { i18n::text(locale, key, &[]) }
fn alert(locale: &str, key: &str) -> Vec<(String, String)> { vec![("alert".to_string(), t(locale, key))] }

#[derive(Clone, Copy, Debug)]
enum Reader { Multipart, Form }
impl Reader {
    fn content_type(self) -> String {
        match self { Reader::Multipart => web::multipart(&[], &[]).0, Reader::Form => "application/x-www-form-urlencoded".to_string() }
    }
    fn with(self, fields: &[(&str, &str)]) -> Vec<u8> {
        match self {
            Reader::Multipart => web::multipart(fields, &[]).1,
            Reader::Form => { let mut e = form_urlencoded::Serializer::new(String::new()); for (k, v) in fields { e.append_pair(k, v); } e.finish().into_bytes() }
        }
    }
}

/// A2/C1/D1/F3: every signed-out request to an import path, by every method, through both readers and under a locale prefix
/// or a repeated slash: an 8 MiB body is never polled, and the answer is main's to the same request with an empty form.
#[tokio::test(flavor = "current_thread")]
async fn a_signed_out_request_to_an_import_path_reads_no_body_and_is_answered_as_an_empty_form() {
    let i = install();
    let secret = csrf::new_token();
    let header_token = csrf::masked(&secret);
    let anonymous = session::SessionData { csrf: Some(secret.clone()), ..Default::default() };
    let expired = i.cookie_at(&i.signed_in(&secret), i.now() - Duration::seconds(session::LIFETIME_SECONDS + 1));
    let stale = i.cookie_at(&session::SessionData { user: Some((i.user, "$2a$11$stalestalestalestalesta".into())), ..i.signed_in(&secret) }, i.now());
    let active = i.cookie_at(&i.signed_in(&secret), i.now());
    let anonymous = i.cookie_at(&anonymous, i.now());
    let bot = i.bot_path();
    let before = i.snapshot();
    // (case, cookie, header token, the state of the user's row, the POST's answer: location without a prefix, flash key)
    let cases = [
        ("no cookie", None, true, "", "/", "errors.unverified_request"),
        ("anonymous, header token", Some(anonymous.as_str()), true, "", "/login", "devise.failure.unauthenticated"),
        ("anonymous, no token", Some(anonymous.as_str()), false, "", "/", "errors.unverified_request"),
        ("expired", Some(expired.as_str()), true, "", "/", "errors.unverified_request"),
        ("salt-stale", Some(stale.as_str()), true, "", "/login", "devise.failure.unauthenticated"),
        ("locked", Some(active.as_str()), true, "locked_at='2026-09-01 12:25:00'", "/login", "devise.failure.locked"),
        ("unconfirmed", Some(active.as_str()), true, "confirmed_at=NULL", "/login", "devise.failure.unconfirmed"),
        ("locked, no token", Some(active.as_str()), false, "locked_at='2026-09-01 12:25:00'", "/", "errors.unverified_request"),
    ];
    for (case, cookie, with_token, row, location, flash) in cases {
        i.o.primary.execute("UPDATE users SET locked_at=NULL, confirmed_at='2026-01-01'", []).unwrap();
        if !row.is_empty() { i.o.primary.execute(&format!("UPDATE users SET {row}"), []).unwrap(); }
        for path in ["/tracker/import", bot.as_str(), "/de/tracker/import", "//tracker//import/", &format!("/de{bot}")] {
            for method in ["POST", "PATCH", "PUT", "DELETE"] {
                for reader in [Reader::Multipart, Reader::Form] {
                    let content_type = reader.content_type();
                    let length = (8 * MIB).to_string();
                    let mut headers = vec![("content-type", content_type.as_str()), ("content-length", length.as_str())];
                    if with_token { headers.push(("x-csrf-token", header_token.as_str())); }
                    let cookies: Vec<&str> = cookie.into_iter().collect();
                    let polls = Arc::new(AtomicUsize::new(0));
                    let answer = send(&i.app, method, path, &cookies, &headers, filler(8 * MIB, polls.clone())).await;
                    let what = format!("{case}: {method} {path} {reader:?}");
                    assert_eq!(polls.load(Ordering::SeqCst), 0, "{what}: the body is never polled");
                    // Main's answer for that request: another method than POST that passes the token check and is not an inactive
                    // user's is the 501 (`only(post(...))`); the rest are redirects.
                    if method != "POST" && flash == "devise.failure.unauthenticated" {
                        assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (501, None, vec![]), "{what}");
                    } else {
                        let prefixed = path.starts_with("/de/") && location == "/login";
                        let (want, language) = if prefixed { ("/de/login".to_string(), "de") } else { (location.to_string(), "en") };
                        assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some(want.as_str()), alert(language, flash)), "{what}");
                    }
                    assert_eq!(answer.session.as_ref().and_then(|s| s.user.clone()), None, "{what}: nobody is signed in by the answer");
                }
            }
        }
    }
    assert_eq!(i.snapshot(), before);
}

/// D4: the flash language and the redirect follow main's rules: a `?locale=` gives the sign-in flash its language and the
/// redirect no prefix; a path prefix gives both; Accept-Language and a body `locale` (unread: D-IMPORT-AUTH-FIRST) give
/// neither; the unverified-request flash is English and goes to the unprefixed root, under a prefix too.
#[tokio::test(flavor = "current_thread")]
async fn a_signed_out_import_answers_in_every_locale_by_main_s_rules() {
    let i = install();
    let secret = csrf::new_token();
    let header_token = csrf::masked(&secret);
    let anonymous = i.cookie_at(&session::SessionData { csrf: Some(secret), ..Default::default() }, i.now());
    for locale in deltabadger::web::locale::LOCALES {
        for path in ["/tracker/import".to_string(), i.bot_path()] {
            for reader in [Reader::Multipart, Reader::Form] {
                let content_type = reader.content_type();
                let token = [("content-type", content_type.as_str()), ("x-csrf-token", header_token.as_str())];
                let what = format!("{locale} {path} {reader:?}");
                let polls = Arc::new(AtomicUsize::new(0));
                let query = send(&i.app, "POST", &format!("{path}?locale={locale}"), &[&anonymous], &token, filler(8 * MIB, polls.clone())).await;
                assert_eq!((query.status, query.location.as_deref(), query.flash()), (302, Some("/login"), alert(locale, "devise.failure.unauthenticated")), "{what}: ?locale");
                let prefixed = send(&i.app, "POST", &format!("/{locale}{path}"), &[&anonymous], &token, filler(8 * MIB, polls.clone())).await;
                assert_eq!((prefixed.status, prefixed.location.clone().unwrap(), prefixed.flash()), (302, format!("/{locale}/login"), alert(locale, "devise.failure.unauthenticated")), "{what}: prefix");
                let accept = [token[0], token[1], ("accept-language", locale)];
                let accepted = send(&i.app, "POST", &path, &[&anonymous], &accept, filler(8 * MIB, polls.clone())).await;
                assert_eq!((accepted.location.as_deref(), accepted.flash()), (Some("/login"), alert("en", "devise.failure.unauthenticated")), "{what}: Accept-Language");
                let body = send(&i.app, "POST", &path, &[&anonymous], &token, Body::from(reader.with(&[("locale", locale)]))).await;
                assert_eq!((body.location.as_deref(), body.flash()), (Some("/login"), alert("en", "devise.failure.unauthenticated")), "{what}: a body locale is unread");
                let unverified = send(&i.app, "POST", &format!("/{locale}{path}"), &[&anonymous], &token[..1], filler(8 * MIB, polls.clone())).await;
                assert_eq!((unverified.location.as_deref(), unverified.flash()), (Some("/"), alert("en", "errors.unverified_request")), "{what}: unverified");
                assert_eq!(polls.load(Ordering::SeqCst), 0, "{what}");
            }
        }
    }
}

/// E3, D-IMPORT-AUTH-FIRST: a signed-out browser with a CSRF secret and a valid token in the body only used to get the
/// login redirect. The body is unread now, so it is the referrer-or-`/` redirect with the unverified-request flash; a body
/// `_method` is not read either.
#[tokio::test(flavor = "current_thread")]
async fn d_import_auth_first_a_signed_out_body_is_not_read() {
    let i = install();
    let secret = csrf::new_token();
    let masked = csrf::masked(&secret);
    let anonymous = i.cookie_at(&session::SessionData { csrf: Some(secret), ..Default::default() }, i.now());
    for path in ["/tracker/import".to_string(), i.bot_path()] {
        for reader in [Reader::Multipart, Reader::Form] {
            let content_type = reader.content_type();
            let body = || Body::from(reader.with(&[("authenticity_token", &masked)]));
            let answer = send(&i.app, "POST", &path, &[&anonymous], &[("content-type", &content_type)], body()).await;
            assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/"), alert("en", "errors.unverified_request")), "{path} {reader:?}");
            let referred = [("content-type", content_type.as_str()), ("referer", "http://localhost:3000/tracker?tab=1")];
            let answer = send(&i.app, "POST", &path, &[&anonymous], &referred, body()).await;
            assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/tracker?tab=1"), alert("en", "errors.unverified_request")), "{path} {reader:?}");
            // A body `_method` would make it a PATCH (the 501); unread, it stays the POST that signs in first.
            let headers = [("content-type", content_type.as_str()), ("x-csrf-token", masked.as_str())];
            let answer = send(&i.app, "POST", &path, &[&anonymous], &headers, Body::from(reader.with(&[("_method", "patch")]))).await;
            assert_eq!((answer.status, answer.location.as_deref()), (302, Some("/login")), "{path} {reader:?}: _method");
        }
    }
}

/// Signed in, the body is read as before: an 8 MiB import works on both routes, a bad token is the same refusal as before,
/// and a multipart PATCH (C3) gains no read.
#[tokio::test(flavor = "current_thread")]
async fn a_signed_in_import_reads_its_body_as_before() {
    let i = install();
    let secret = csrf::new_token();
    let masked = csrf::masked(&secret);
    let cookie = i.cookie_at(&i.signed_in(&secret), i.now());
    let (content_type, _) = web::multipart(&[], &[]);
    let pad = "p".repeat(8 * MIB);
    let file = csv();
    let polls = Arc::new(AtomicUsize::new(0));
    let body = web::multipart(&[("authenticity_token", &masked), ("pad", &pad)], &[("file", "orders.csv", &file)]).1;
    let answer = send(&i.app, "POST", &i.bot_path(), &[&cookie], &[("content-type", &content_type)], counted(body, polls.clone(), || ())).await;
    assert_eq!((answer.status, answer.location.unwrap()), (302, format!("/bots/{}", i.bot)), "{}", answer.body);
    assert!(polls.load(Ordering::SeqCst) > 100, "the 8 MiB body is read");
    assert_eq!(i.rows(), 1, "the order is imported");
    // The tracker's import reads its 8 MiB file and answers it (a file with no rows it knows).
    let polls = Arc::new(AtomicUsize::new(0));
    let body = web::multipart(&[("authenticity_token", &masked)], &[("file", "big.csv", pad.as_bytes())]).1;
    let answer = send(&i.app, "POST", "/tracker/import", &[&cookie], &[("content-type", &content_type)], counted(body, polls.clone(), || ())).await;
    assert_eq!(answer.status, 422, "{}", answer.body);
    assert!(polls.load(Ordering::SeqCst) > 100);
    // A bad token, signed in: read, then refused as before.
    for path in ["/tracker/import".to_string(), i.bot_path()] {
        let polls = Arc::new(AtomicUsize::new(0));
        let body = web::multipart(&[("authenticity_token", "bad")], &[("file", "orders.csv", &file)]).1;
        let answer = send(&i.app, "POST", &path, &[&cookie], &[("content-type", &content_type)], counted(body, polls.clone(), || ())).await;
        assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/"), alert("en", "errors.unverified_request")), "{path}");
        assert!(polls.load(Ordering::SeqCst) > 0, "{path}");
        assert_eq!(answer.session.unwrap().user, Some((i.user, i.salt.clone())), "{path}: still signed in");
    }
    // C3: the 32 MiB reader stays POST-only; a signed-in multipart PATCH is the 501 and its body is not read.
    for path in ["/tracker/import".to_string(), i.bot_path()] {
        let polls = Arc::new(AtomicUsize::new(0));
        let answer = send(&i.app, "PATCH", &path, &[&cookie], &[("content-type", &content_type), ("x-csrf-token", &masked)], filler(8 * MIB, polls.clone())).await;
        assert_eq!(answer.status, 501, "{path}");
        assert_eq!(polls.load(Ordering::SeqCst), 0, "{path}");
    }
    assert_eq!(i.rows(), 1);
}

/// D2: a cookie that expires while admission looks up its account is not admitted: no poll, the signed-out answer.
#[tokio::test(flavor = "current_thread")]
async fn a_cookie_that_expires_during_the_admission_lookup_reads_no_body() {
    let i = install();
    let secret = csrf::new_token();
    let masked = csrf::masked(&secret);
    let sealed = i.now() - Duration::seconds(session::LIFETIME_SECONDS - 5);
    let cookie = i.cookie_at(&i.signed_in(&secret), sealed);
    let clock = i.clock.clone();
    let later = i.now() + Duration::seconds(10);
    i.at_admin_read(None, 1, move || clock.set(later)).await;
    let polls = Arc::new(AtomicUsize::new(0));
    let (content_type, body) = web::multipart(&[], &[("file", "orders.csv", &csv())]);
    let answer = send(&i.app, "POST", &i.bot_path(), &[&cookie], &[("content-type", &content_type), ("x-csrf-token", &masked)], counted(body, polls.clone(), || ())).await;
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    // An expired cookie opens no session, as on main: its old header token verifies against nothing.
    assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/"), alert("en", "errors.unverified_request")));
    assert_eq!(answer.session.unwrap(), session::SessionData { flash: alert("en", "errors.unverified_request"), ..Default::default() });
    assert_eq!(i.rows(), 0);
}

/// E2: a denial sticks. Cookie A expires during the admission lookup while cookie B (whose token the request carries) is
/// still valid: the request is not B's (whose answer would be the import's 422), it is the signed-out answer.
#[tokio::test(flavor = "current_thread")]
async fn a_denied_request_does_not_turn_to_another_cookie() {
    let i = install();
    let (a, b) = (csrf::new_token(), csrf::new_token());
    let first = i.cookie_at(&i.signed_in(&a), i.now() - Duration::seconds(session::LIFETIME_SECONDS - 5));
    let second = i.cookie_at(&i.signed_in(&b), i.now());
    let masked = csrf::masked(&b);
    let clock = i.clock.clone();
    let later = i.now() + Duration::seconds(10);
    i.at_admin_read(None, 1, move || clock.set(later)).await;
    let polls = Arc::new(AtomicUsize::new(0));
    let (content_type, body) = web::multipart(&[], &[]);
    let headers = [("content-type", content_type.as_str()), ("x-csrf-token", masked.as_str())];
    let answer = send(&i.app, "POST", "/tracker/import", &[&first, &second], &headers, counted(body.clone(), polls.clone(), || ())).await;
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    let signed_out = send(&i.app, "POST", "/tracker/import", &[], &headers, Body::from(body)).await;
    assert_eq!(answer.seen(), signed_out.seen());
    assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/"), alert("en", "errors.unverified_request")));
}

/// C2: admission binds the session it chose. Cookie A admits the upload and expires during the read; cookie B (the same
/// user, its token in the header) would now open: the request is not B's, nothing is imported.
#[tokio::test(flavor = "current_thread")]
async fn a_cookie_that_expires_during_the_read_does_not_hand_the_import_to_another() {
    let i = install();
    let (a, b) = (csrf::new_token(), csrf::new_token());
    let first = i.cookie_at(&i.signed_in(&a), i.now() - Duration::seconds(session::LIFETIME_SECONDS - 5));
    let second = i.cookie_at(&i.signed_in(&b), i.now());
    let clock = i.clock.clone();
    let later = i.now() + Duration::seconds(10);
    let polls = Arc::new(AtomicUsize::new(0));
    let (content_type, body) = web::multipart(&[("authenticity_token", &csrf::masked(&a))], &[("file", "orders.csv", &csv())]);
    let headers = [("content-type", content_type.as_str()), ("x-csrf-token", &csrf::masked(&b))];
    let answer = send(&i.app, "POST", &i.bot_path(), &[&first, &second], &headers, counted(body, polls.clone(), move || clock.set(later))).await;
    assert!(polls.load(Ordering::SeqCst) > 0, "admitted");
    // A's cookie expired: no session, so neither token verifies.
    assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/"), alert("en", "errors.unverified_request")));
    assert_eq!(answer.session.unwrap().user, None);
    assert_eq!(i.rows(), 0);
}

/// C2: a password changed while the body is on its way ends the sign-in: nothing is imported.
#[tokio::test(flavor = "current_thread")]
async fn a_password_changed_during_the_read_imports_nothing() {
    let i = install();
    let secret = csrf::new_token();
    let cookie = i.cookie_at(&i.signed_in(&secret), i.now());
    let db = i.d.path().join("production.sqlite3");
    let polls = Arc::new(AtomicUsize::new(0));
    let (content_type, body) = web::multipart(&[("authenticity_token", &csrf::masked(&secret))], &[("file", "orders.csv", &csv())]);
    let change = move || { rusqlite::Connection::open(&db).unwrap().execute("UPDATE users SET encrypted_password='$2a$11$changedchangedchangedch.x'", []).unwrap(); };
    let answer = send(&i.app, "POST", &i.bot_path(), &[&cookie], &[("content-type", &content_type)], counted(body, polls.clone(), change)).await;
    assert!(polls.load(Ordering::SeqCst) > 0);
    assert_eq!((answer.status, answer.location.as_deref()), (302, Some("/login")));
    assert_eq!(i.rows(), 0);
}

/// D3: an admitted upload whose cookie expires during the read keeps the reader's answer to a bad body (400 truncated,
/// 413 oversized) and gets the signed-out answer to a good one; neither writes.
#[tokio::test(flavor = "current_thread")]
async fn a_cookie_that_expires_during_the_read_gets_the_reader_s_error_or_the_signed_out_answer() {
    let i = install();
    let secret = csrf::new_token();
    let (content_type, good) = web::multipart(&[("authenticity_token", &csrf::masked(&secret))], &[("file", "orders.csv", &csv())]);
    let mut huge = vec![b'a'; deltabadger::web::IMPORT_LIMIT];
    huge.extend_from_slice(b"overflow");
    let oversized = web::multipart(&[("authenticity_token", &csrf::masked(&secret))], &[("file", "big.csv", &huge)]).1;
    for (case, body, status) in [("truncated", good[..good.len() - 8].to_vec(), 400), ("oversized", oversized, 413), ("good", good.clone(), 302)] {
        i.clock.set(web::at(T0));
        let cookie = i.cookie_at(&i.signed_in(&secret), i.now() - Duration::seconds(session::LIFETIME_SECONDS - 5));
        let (clock, later) = (i.clock.clone(), i.now() + Duration::seconds(10));
        let polls = Arc::new(AtomicUsize::new(0));
        let answer = send(&i.app, "POST", &i.bot_path(), &[&cookie], &[("content-type", &content_type)], counted(body, polls.clone(), move || clock.set(later))).await;
        assert!(polls.load(Ordering::SeqCst) > 0, "{case}");
        assert_eq!(answer.status, status, "{case}: {}", answer.body);
        if status == 302 { assert_eq!((answer.location.as_deref(), answer.flash()), (Some("/"), alert("en", "errors.unverified_request")), "{case}"); }
        assert_eq!(i.rows(), 0, "{case}");
    }
}

/// D3: a stalled admitted upload is the reader's 408, however its cookie fares meanwhile.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_stalled_upload_whose_cookie_expires_is_the_reader_s_timeout() {
    let i = install();
    let secret = csrf::new_token();
    let cookie = i.cookie_at(&i.signed_in(&secret), i.now() - Duration::seconds(session::LIFETIME_SECONDS - 5));
    let (clock, later) = (i.clock.clone(), i.now() + Duration::seconds(10));
    let (content_type, good) = web::multipart(&[("authenticity_token", &csrf::masked(&secret))], &[("file", "orders.csv", &csv())]);
    let mut first = Some(move || clock.set(later));
    let start = Bytes::copy_from_slice(&good[..good.len() / 2]);
    let stalled = futures_util::stream::once(async move { if let Some(first) = first.take() { first(); } Ok::<_, std::convert::Infallible>(start) })
        .chain(futures_util::stream::pending());
    let answer = send(&i.app, "POST", &i.bot_path(), &[&cookie], &[("content-type", &content_type)], Body::from_stream(stalled)).await;
    assert_eq!(answer.status, 408, "{}", answer.body);
    assert_eq!(i.rows(), 0);
}

/// E1: the cookie expires during a post-read lookup (the setup lookup, then the account lookup: the first and the second
/// read of `users.admin` after the body), the token is valid and the import would write: an expired cookie opens no
/// session, as on main, so the answer is the unverified request's; nothing written. A check placed between the two
/// lookups misses the second case (the handler's own check would then answer it as signed out instead).
#[tokio::test(flavor = "current_thread")]
async fn a_cookie_that_expires_during_either_post_read_lookup_imports_nothing() {
    for (lookup, nth) in [("setup", 1), ("account", 2)] {
        let i = install();
        let secret = csrf::new_token();
        let cookie = i.cookie_at(&i.signed_in(&secret), i.now() - Duration::seconds(session::LIFETIME_SECONDS - 5));
        let armed = Arc::new(AtomicBool::new(false));
        let (clock, later) = (i.clock.clone(), i.now() + Duration::seconds(10));
        i.at_admin_read(Some(armed.clone()), nth, move || clock.set(later)).await;
        let (content_type, body) = web::multipart(&[("authenticity_token", &csrf::masked(&secret))], &[("file", "orders.csv", &csv())]);
        let polls = Arc::new(AtomicUsize::new(0));
        let answer = send(&i.app, "POST", &i.bot_path(), &[&cookie], &[("content-type", &content_type)], counted(body, polls, move || armed.store(true, Ordering::SeqCst))).await;
        assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/"), alert("en", "errors.unverified_request")), "{lookup}");
        assert_eq!(answer.session.unwrap(), session::SessionData { flash: alert("en", "errors.unverified_request"), ..Default::default() }, "{lookup}");
        assert_eq!(i.rows(), 0, "{lookup}");
    }
}

/// F1: the final check comes before any answer is chosen. The cookie expires during either post-read lookup and the
/// token is invalid: the unverified-request answer, nothing written, and the cookie it returns does not sign anyone in.
#[tokio::test(flavor = "current_thread")]
async fn an_expired_sign_in_is_never_sealed_again() {
    for (lookup, nth) in [("setup", 1), ("account", 2)] {
        let i = install();
        let secret = csrf::new_token();
        let cookie = i.cookie_at(&i.signed_in(&secret), i.now() - Duration::seconds(session::LIFETIME_SECONDS - 5));
        let armed = Arc::new(AtomicBool::new(false));
        let (clock, later) = (i.clock.clone(), i.now() + Duration::seconds(10));
        i.at_admin_read(Some(armed.clone()), nth, move || clock.set(later)).await;
        let (content_type, body) = web::multipart(&[("authenticity_token", "invalid")], &[("file", "orders.csv", &csv())]);
        let polls = Arc::new(AtomicUsize::new(0));
        let answer = send(&i.app, "POST", &i.bot_path(), &[&cookie], &[("content-type", &content_type)], counted(body, polls, move || armed.store(true, Ordering::SeqCst))).await;
        assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/"), alert("en", "errors.unverified_request")), "{lookup}");
        assert_eq!(answer.session.as_ref().unwrap().user, None, "{lookup}: the resealed session has nobody in it");
        assert_eq!(i.rows(), 0, "{lookup}");
        let next = send(&i.app, "GET", "/settings/account", &[answer.cookie.as_deref().unwrap()], &[], Body::empty()).await;
        assert_eq!((next.status, next.location.as_deref()), (302, Some("/login")), "{lookup}: the returned cookie does not authenticate");
    }
}

/// The handler's write checks the sign-in again inside its own transaction: a password reset committed after the
/// pipeline's checks, while the file is read (here: as the write's transaction begins), writes nothing on either route,
/// and the request is answered as signed out.
#[tokio::test(flavor = "current_thread")]
async fn a_password_reset_while_the_file_is_read_writes_nothing() {
    let tracker = String::from("date,type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,exchange,tx_id,group_id,description\n\
                           2026-08-03T10:00:00Z,deposit,BTC,5,,,,,kraken,t1,,\n");
    for (route, file) in [("bot", csv()), ("tracker", tracker.into_bytes())] {
        let i = install();
        let secret = csrf::new_token();
        let cookie = i.cookie_at(&i.signed_in(&secret), i.now());
        let before = i.snapshot();
        let armed = Arc::new(AtomicBool::new(false));
        let db = i.d.path().join("production.sqlite3");
        let reset = move || { rusqlite::Connection::open(&db).unwrap().execute("UPDATE users SET encrypted_password='$2a$11$resetresetresetresetre.x'", []).unwrap(); };
        i.at(Some(armed.clone()), 1, |action| matches!(action, AuthAction::Transaction { operation: TransactionOperation::Begin }), reset).await;
        let path = if route == "bot" { i.bot_path() } else { "/tracker/import".to_string() };
        let (content_type, body) = web::multipart(&[("authenticity_token", &csrf::masked(&secret)), ("format", "deltabadger")], &[("file", "orders.csv", &file)]);
        let answer = send(&i.app, "POST", &path, &[&cookie], &[("content-type", &content_type)], counted(body, Arc::default(), move || armed.store(true, Ordering::SeqCst))).await;
        assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/login"), alert("en", "devise.failure.unauthenticated")), "{route}: {}", answer.body);
        assert_eq!(answer.session.unwrap().user, None, "{route}");
        assert_eq!(i.snapshot(), before, "{route}: nothing written");
    }
}

/// `finish` checks the admitted cookie again before it seals the session: one that expires while the bot's CSV is written
/// (here: as its transaction commits) is not sealed again; the returned cookie does not authenticate.
#[tokio::test(flavor = "current_thread")]
async fn a_cookie_that_expires_while_the_handler_runs_is_not_sealed_again() {
    let i = install();
    let secret = csrf::new_token();
    let cookie = i.cookie_at(&i.signed_in(&secret), i.now() - Duration::seconds(session::LIFETIME_SECONDS - 5));
    let armed = Arc::new(AtomicBool::new(false));
    let (clock, later) = (i.clock.clone(), i.now() + Duration::seconds(10));
    // rusqlite names BEGIN, RELEASE and ROLLBACK; a COMMIT is its `Unknown`.
    i.at(Some(armed.clone()), 1, |action| matches!(action, AuthAction::Transaction { operation: TransactionOperation::Unknown }), move || clock.set(later)).await;
    let (content_type, body) = web::multipart(&[("authenticity_token", &csrf::masked(&secret))], &[("file", "orders.csv", &csv())]);
    let answer = send(&i.app, "POST", &i.bot_path(), &[&cookie], &[("content-type", &content_type)], counted(body, Arc::default(), move || armed.store(true, Ordering::SeqCst))).await;
    assert_eq!((answer.status, answer.location.unwrap()), (302, format!("/bots/{}", i.bot)), "the write committed while signed in");
    assert_eq!(i.rows(), 1);
    assert_eq!(answer.session.as_ref().unwrap().user, None, "the expired sign-in is not sealed again");
    let next = send(&i.app, "GET", "/settings/account", &[answer.cookie.as_deref().unwrap()], &[], Body::empty()).await;
    assert_eq!((next.status, next.location.as_deref()), (302, Some("/login")), "the returned cookie does not authenticate");
}

/// F2: the account check is the last read before dispatch. A password change committed between the setup lookup and the
/// account lookup imports nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_password_changed_between_the_lookups_imports_nothing() {
    let i = install();
    let secret = csrf::new_token();
    let cookie = i.cookie_at(&i.signed_in(&secret), i.now());
    let armed = Arc::new(AtomicBool::new(false));
    let db = i.d.path().join("production.sqlite3");
    let change = move || { rusqlite::Connection::open(&db).unwrap().execute("UPDATE users SET encrypted_password='$2a$11$changedchangedchangedch.x'", []).unwrap(); };
    i.at_admin_read(Some(armed.clone()), 2, change).await;
    let (content_type, body) = web::multipart(&[("authenticity_token", &csrf::masked(&secret))], &[("file", "orders.csv", &csv())]);
    let polls = Arc::new(AtomicUsize::new(0));
    let answer = send(&i.app, "POST", &i.bot_path(), &[&cookie], &[("content-type", &content_type)], counted(body, polls, move || armed.store(true, Ordering::SeqCst))).await;
    assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/login"), alert("en", "devise.failure.unauthenticated")));
    assert_eq!(i.rows(), 0);
}

/// A body that sends its first half and then nothing more: an upload holding its slot until the read deadline. Counts
/// its polls.
fn stalled(content: Vec<u8>, polls: Arc<AtomicUsize>) -> Body {
    let start = Bytes::from(content[..content.len() / 2].to_vec());
    Body::from_stream(futures_util::stream::once(async move { polls.fetch_add(1, Ordering::SeqCst); Ok::<_, std::convert::Infallible>(start) })
        .chain(futures_util::stream::pending()))
}

/// What the tracker's Turbo form sends.
const TURBO: &str = "text/vnd.turbo-stream.html, text/html, application/xhtml+xml";

/// Waits until `polls` saw a poll (an admitted upload holding its slot), for at most two seconds.
async fn polled(polls: &AtomicUsize) {
    for _ in 0..200 { if polls.load(Ordering::SeqCst) > 0 { return } tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
    panic!("the upload was never read");
}

/// PLAN-710-importmem B1′/E1/E5 (D-IMPORT-BUSY): two uploads read their bodies (stalled, holding both import slots); a
/// third, signed in, waits the slot's five seconds with its body never polled, and is answered busy: a 503 with
/// `Retry-After`, no cookie written. The tracker's Turbo form gets its busy line and its drop zone back; the bot's form
/// (no Turbo) a page in the user's language that leads back to the bot.
#[tokio::test(flavor = "current_thread")]
async fn a_third_concurrent_import_waits_unread_and_is_answered_busy() {
    let i = install();
    let secret = csrf::new_token();
    let masked = csrf::masked(&secret);
    let cookie = i.cookie_at(&i.signed_in(&secret), i.now());
    let (content_type, body) = web::multipart(&[("authenticity_token", &masked)], &[("file", "orders.csv", &csv())]);
    let holders: Vec<Arc<AtomicUsize>> = (0..2).map(|_| Arc::new(AtomicUsize::new(0))).collect();
    for (n, polls) in holders.iter().enumerate() {
        let (app, cookie, content_type, body, polls) = (i.app.clone(), cookie.clone(), content_type.clone(), body.clone(), polls.clone());
        let path = if n == 0 { "/tracker/import".to_string() } else { i.bot_path() };
        tokio::spawn(async move { send(&app, "POST", &path, &[&cookie], &[("content-type", &content_type)], stalled(body, polls)).await });
    }
    for polls in &holders { polled(polls).await; }
    let before = i.snapshot();
    for (path, accept, language) in [("/tracker/import".to_string(), TURBO, "en"), (i.bot_path(), "text/html", "en"), (format!("/de{}", i.bot_path()), "text/html", "de")] {
        let polls = Arc::new(AtomicUsize::new(0));
        let started = std::time::Instant::now();
        let answer = send(&i.app, "POST", &path, &[&cookie], &[("content-type", &content_type), ("accept", accept)], counted(body.clone(), polls.clone(), || ())).await;
        assert!(started.elapsed() >= deltabadger::web::IMPORT_SLOT_WAIT, "{path}: answered after the slot's wait, {:?}", started.elapsed());
        assert_eq!(polls.load(Ordering::SeqCst), 0, "{path}: the body is never polled");
        assert_eq!((answer.status, answer.retry_after.as_deref(), answer.cookie.as_deref(), answer.location.as_deref()), (503, Some("5"), None, None), "{path}: {}", answer.body);
        if accept == TURBO {
            assert_eq!(answer.content_type.as_deref(), Some("text/vnd.turbo-stream.html; charset=utf-8"));
            assert!(answer.body.contains(&i18n::escape(&t(language, "tracker.import.import_busy"))), "{}", answer.body);
            assert!(answer.body.contains(r#"class-name="is-busy" action="remove_class" targets=".dropzone""#), "the drop zone is not busy any more: {}", answer.body);
            assert!(answer.body.contains(r#"<turbo-stream action="replace" target="file"><template><input"#), "a fresh file input: {}", answer.body);
        } else {
            assert_eq!(answer.content_type.as_deref(), Some("text/html; charset=utf-8"));
            assert!(answer.body.contains(&i18n::escape(&t(language, "bot.details.stats.import_busy"))), "{}", answer.body);
            let back = if language == "de" { format!("/de/bots/{}", i.bot) } else { format!("/bots/{}", i.bot) };
            assert!(answer.body.contains(&format!("href=\"{back}\"")), "{}", answer.body);
        }
    }
    assert_eq!(i.snapshot(), before, "nothing written");
}

/// E1: a busy answer seals nothing. A cookie a second from its end, a busy answer writes no cookie (so it can neither
/// extend the sign-in nor restore one a logout in another tab ended); the next request of the logged-out tab is signed
/// out.
#[tokio::test(flavor = "current_thread")]
async fn a_busy_answer_writes_no_cookie() {
    let i = install();
    let secret = csrf::new_token();
    let masked = csrf::masked(&secret);
    let held = (deltabadger::tracker::import::service::permit(&i.app).await, deltabadger::tracker::import::service::permit(&i.app).await);
    let ending = i.cookie_at(&i.signed_in(&secret), i.now() - Duration::seconds(session::LIFETIME_SECONDS - 1));
    let (content_type, body) = web::multipart(&[("authenticity_token", &masked)], &[("file", "orders.csv", &csv())]);
    for path in ["/tracker/import".to_string(), i.bot_path()] {
        let answer = send(&i.app, "POST", &path, &[&ending], &[("content-type", &content_type), ("accept", TURBO)], Body::from(body.clone())).await;
        assert_eq!((answer.status, answer.cookie), (503, None), "{path}");
    }
    let next = send(&i.app, "GET", "/settings/account", &[], &[], Body::empty()).await;
    assert_eq!((next.status, next.location.as_deref()), (302, Some("/login")), "the logged-out tab stays signed out");
    drop(held);
}

/// C2: the sign-in ends while the request waits for a slot (its cookie expires; the password changes): no body is read,
/// the answer is the signed-out one, and its cookie signs nobody in. A slot freed after the sign-in ended does not start
/// the read either.
#[tokio::test(flavor = "current_thread")]
async fn a_sign_in_that_ends_while_waiting_for_a_slot_reads_nothing() {
    for (case, freed) in [("expired", false), ("password", false), ("expired", true), ("password", true)] {
        let i = install();
        let secret = csrf::new_token();
        let masked = csrf::masked(&secret);
        let mut held = vec![deltabadger::tracker::import::service::permit(&i.app).await, deltabadger::tracker::import::service::permit(&i.app).await];
        let cookie = i.cookie_at(&i.signed_in(&secret), i.now() - Duration::seconds(session::LIFETIME_SECONDS - 3));
        let (clock, later, db) = (i.clock.clone(), i.now() + Duration::seconds(10), i.d.path().join("production.sqlite3"));
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let end = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if case == "expired" { clock.set(later) } else {
                rusqlite::Connection::open(&db).unwrap().execute("UPDATE users SET encrypted_password='$2a$11$changedchangedchangedch.x'", []).unwrap();
            }
            let _ = tx.send(());
        });
        let (content_type, body) = web::multipart(&[("authenticity_token", &masked)], &[("file", "orders.csv", &csv())]);
        let polls = Arc::new(AtomicUsize::new(0));
        let (path, cookies, headers) = (i.bot_path(), [cookie.as_str()], [("content-type", content_type.as_str()), ("x-csrf-token", masked.as_str())]);
        let request = send(&i.app, "POST", &path, &cookies, &headers, counted(body, polls.clone(), || ()));
        let release = async { if freed { let _ = rx.await; held.clear(); } };
        let (answer, ()) = tokio::join!(request, release);
        end.await.unwrap();
        let what = format!("{case}, freed: {freed}");
        assert_eq!(polls.load(Ordering::SeqCst), 0, "{what}");
        let want = if case == "expired" { (302, Some("/"), alert("en", "errors.unverified_request")) } else { (302, Some("/login"), alert("en", "devise.failure.unauthenticated")) };
        assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), want, "{what}: {}", answer.body);
        assert_eq!(answer.session.as_ref().and_then(|s| s.user.clone()), None, "{what}");
        assert_eq!(i.rows(), 0, "{what}");
    }
}

/// B1′: a Cancel and a confirmation wait for a slot as an upload does; the busy answer reads none of their fields.
#[tokio::test(flavor = "current_thread")]
async fn a_cancel_and_a_confirmation_wait_for_a_slot_too() {
    let i = install();
    let secret = csrf::new_token();
    let masked = csrf::masked(&secret);
    let held = (deltabadger::tracker::import::service::permit(&i.app).await, deltabadger::tracker::import::service::permit(&i.app).await);
    let cookie = i.cookie_at(&i.signed_in(&secret), i.now());
    let token = "0123456789abcdef";
    let handle = format!("{token}.1.0.{}", "a".repeat(32));
    for fields in [vec![("token", token), ("cancel", "Cancel")], vec![("token", token), ("api_key_id", handle.as_str())]] {
        let mut fields = fields;
        fields.push(("authenticity_token", &masked));
        let (content_type, body) = web::multipart(&fields, &[]);
        let polls = Arc::new(AtomicUsize::new(0));
        let answer = send(&i.app, "POST", "/tracker/import", &[&cookie], &[("content-type", &content_type), ("accept", TURBO)], counted(body, polls.clone(), || ())).await;
        assert_eq!((answer.status, polls.load(Ordering::SeqCst)), (503, 0), "{fields:?}");
    }
    drop(held);
}

/// PLAN-710-importmem r1 #5: the busy page's language follows the request as every page's does: of two `?locale`, the
/// last.
#[tokio::test(flavor = "current_thread")]
async fn a_busy_page_takes_the_last_locale_of_its_query() {
    let i = install();
    let secret = csrf::new_token();
    let masked = csrf::masked(&secret);
    let held = (deltabadger::tracker::import::service::permit(&i.app).await, deltabadger::tracker::import::service::permit(&i.app).await);
    let cookie = i.cookie_at(&i.signed_in(&secret), i.now());
    let (content_type, body) = web::multipart(&[("authenticity_token", &masked)], &[("file", "orders.csv", &csv())]);
    let answer = send(&i.app, "POST", &format!("{}?locale=de&locale=fr", i.bot_path()), &[&cookie], &[("content-type", &content_type)], Body::from(body)).await;
    assert_eq!(answer.status, 503);
    assert!(answer.body.contains(&i18n::escape(&t("fr", "bot.details.stats.import_busy"))), "{}", answer.body);
    assert!(answer.body.contains(&format!("href=\"/fr/bots/{}\"", i.bot)), "{}", answer.body);
    drop(held);
}

/// PLAN-710-importmem r1 #6 (E5): the wait for a slot is five seconds exactly, on a paused clock: what admission and the
/// fresh check read takes no time, so the busy answer comes at the deadline.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_wait_for_a_slot_is_five_seconds() {
    let i = install();
    let secret = csrf::new_token();
    let masked = csrf::masked(&secret);
    let held = (deltabadger::tracker::import::service::permit(&i.app).await, deltabadger::tracker::import::service::permit(&i.app).await);
    let cookie = i.cookie_at(&i.signed_in(&secret), i.now());
    let (content_type, body) = web::multipart(&[("authenticity_token", &masked)], &[("file", "orders.csv", &csv())]);
    let polls = Arc::new(AtomicUsize::new(0));
    let started = tokio::time::Instant::now();
    let answer = send(&i.app, "POST", &i.bot_path(), &[&cookie], &[("content-type", &content_type)], counted(body, polls.clone(), || ())).await;
    assert_eq!((answer.status, polls.load(Ordering::SeqCst)), (503, 0));
    assert_eq!(started.elapsed(), std::time::Duration::from_secs(5));
    drop(held);
}
