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
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const T0: &str = "2026-09-01T12:30:00Z";
const HEADER: &str = "Timestamp,Order ID,Type,Side,Amount,Value,Price,Base Asset,Quote Asset,Status";
const ROW: &str = "2026-08-01 10:00:00 UTC,x,Market,Buy,0.001,60,60000,BTC,USD,Closed";
const MIB: usize = 1024 * 1024;

struct Install { _d: tempfile::TempDir, o: deltabadger::store::Opened, user: i64, salt: String, bot: i64, clock: Arc<TestClock>, app: App }

/// A configured install: its admin (confirmed) and one stopped Alpaca basket of theirs.
fn install() -> Install {
    let (d, o, s) = common::install_alpaca();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    o.primary.execute("UPDATE users SET confirmed_at='2026-01-01', wash_sale_enabled=0", []).unwrap();
    let hash: String = o.primary.query_row("SELECT encrypted_password FROM users WHERE id=?1", [s.user_id], |r| r.get(0)).unwrap();
    let clock = TestClock::at(T0);
    let app = web::app(d.path(), web::SECRET, clock.clone());
    Install { salt: hash.chars().take(29).collect(), user: s.user_id, bot, _d: d, o, clock, app }
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
        let action = Mutex::new(Some(action));
        let seen = AtomicUsize::new(0);
        self.app.db(move |c| {
            c.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
                if let rusqlite::hooks::AuthAction::Read { table_name: "users", column_name: "admin" } = context.action {
                    if armed.as_ref().is_none_or(|armed| armed.load(Ordering::SeqCst)) && seen.fetch_add(1, Ordering::SeqCst) + 1 == nth {
                        if let Some(mut action) = action.lock().unwrap().take() { action(); }
                    }
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
struct Sent { status: u16, location: Option<String>, session: Option<session::SessionData>, cookie: Option<String>, body: String }
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
    let body = String::from_utf8_lossy(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).into_owned();
    Sent { status, location, session, cookie, body }
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
    /// The form with no fields: a valid zero-part multipart form, or an empty urlencoded one.
    fn empty(self) -> Vec<u8> { match self { Reader::Multipart => web::multipart(&[], &[]).1, Reader::Form => Vec::new() } }
    fn with(self, fields: &[(&str, &str)]) -> Vec<u8> {
        match self {
            Reader::Multipart => web::multipart(fields, &[]).1,
            Reader::Form => { let mut e = form_urlencoded::Serializer::new(String::new()); for (k, v) in fields { e.append_pair(k, v); } e.finish().into_bytes() }
        }
    }
}

/// A2/C1/D1/F3: every signed-out request to an import path, by every method, through both readers and under a locale prefix
/// or a repeated slash: an 8 MiB body is never polled, and the answer is the one the same request with an empty form gets.
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
                    let mut headers = vec![("content-type", content_type.as_str())];
                    if with_token { headers.push(("x-csrf-token", header_token.as_str())); }
                    let cookies: Vec<&str> = cookie.into_iter().collect();
                    let polls = Arc::new(AtomicUsize::new(0));
                    let mut big = headers.clone();
                    let length = (8 * MIB).to_string();
                    big.push(("content-length", &length));
                    let answer = send(&i.app, method, path, &cookies, &big, filler(8 * MIB, polls.clone())).await;
                    let what = format!("{case}: {method} {path} {reader:?}");
                    assert_eq!(polls.load(Ordering::SeqCst), 0, "{what}: the body is never polled");
                    let empty = send(&i.app, method, path, &cookies, &headers, Body::from(reader.empty())).await;
                    assert_eq!(answer.seen(), empty.seen(), "{what}: answered as the empty form");
                    if method == "POST" {
                        let prefixed = path.starts_with("/de/") && location == "/login";
                        let (want, language) = if prefixed { ("/de/login".to_string(), "de") } else { (location.to_string(), "en") };
                        assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some(want.as_str()), alert(language, flash)), "{what}");
                        assert_eq!(answer.session.as_ref().and_then(|s| s.user.clone()), None, "{what}: nobody is signed in by the answer");
                    }
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
    assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (302, Some("/login"), alert("en", "devise.failure.unauthenticated")));
    assert_eq!(answer.session.unwrap().user, None);
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
    assert_eq!((answer.status, answer.location.as_deref(), answer.flash()), (signed_out.status, signed_out.location.as_deref(), signed_out.flash()));
    assert_eq!((answer.status, answer.location.as_deref()), (302, Some("/")));
    assert_eq!(answer.session.unwrap().user, None);
}
