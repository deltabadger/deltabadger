//! A16 (PLAN-A16-signup-password.md): the security properties of signup and password reset that the page grid
//! (pages_accounts.rb) cannot show, each on the production handlers.
mod common;
use common::web::{self as web_support, Answer, Browser, Csrf, TestClock};
use deltabadger::mail::Message;
use deltabadger::web::settings::mail::{Delivery, Mailer};
use deltabadger::web::App;
use rusqlite::Connection;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const SECRET: &str = "a16-accounts-test-secret-0123456789abcdef0123456789abcdef";
const PASSWORD: &str = "Correct-horse-9";
const NEW_PASSWORD: &str = "Another-horse-7";
const AT: &str = "2026-09-10T12:00:30Z";

/// How the account mailer behaves: captures every message, then answers as told.
#[derive(Clone, Copy, PartialEq)]
enum Behaviour { Deliver, Fail, Stall }

struct Mail { sent: Mutex<Vec<Message>>, behaviour: Behaviour }
impl Mailer for Mail {
    fn deliver(&self, message: Message, _settings: Option<deltabadger::mail::smtp::Settings>, _now: chrono::DateTime<chrono::Utc>) -> Delivery {
        self.sent.lock().unwrap().push(message);
        match self.behaviour {
            Behaviour::Deliver => Box::pin(async { Ok(()) }),
            Behaviour::Fail => Box::pin(async { Err(deltabadger::web::WebError::Config("delivery failed (test)".into())) }),
            Behaviour::Stall => Box::pin(std::future::pending()),
        }
    }
}

struct Harness { dir: tempfile::TempDir, app: App, mail: Arc<Mail>, clock: Arc<TestClock> }

fn harness_with(behaviour: Behaviour, env: &[(&str, &str)], hook: Option<deltabadger::web::PasswordHook>) -> Harness {
    let dir = common::rails_install();
    let clock = TestClock::at(AT);
    let extra: serde_json::Map<String, Value> = env.iter().map(|(k, v)| (k.to_string(), Value::String(v.to_string()))).collect();
    let mail = Arc::new(Mail { sent: Mutex::new(vec![]), behaviour });
    let mut app = web_support::app_in(dir.path(), SECRET, None, &extra, clock.clone()).with_settings_mailer(mail.clone()).unwrap();
    if let Some(hook) = hook { app = app.with_password_hook(hook).unwrap(); }
    let c = db(&dir);
    let hash = deltabadger::crypto::hash_password(PASSWORD).unwrap();
    c.execute("INSERT INTO users (name, email, encrypted_password, admin, setup_completed, confirmed_at, created_at, updated_at) \
               VALUES ('Owner', 'owner@example.com', ?1, 1, 1, '2026-01-01 00:00:00', '2026-01-01 00:00:00', '2026-01-01 00:00:00')", [&hash]).unwrap();
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('registration_open', ?1, '2026-01-01', '2026-01-01')",
              [app.cipher.encrypt("true")]).unwrap();
    Harness { dir, app, mail, clock }
}

fn harness() -> Harness { harness_with(Behaviour::Deliver, &[], None) }

fn db(dir: &tempfile::TempDir) -> Connection { Connection::open(dir.path().join("production.sqlite3")).unwrap() }

impl Harness {
    fn sent(&self) -> Vec<(String, String)> { self.mail.sent.lock().unwrap().iter().map(|m| (m.to.clone(), m.subject.clone())).collect() }
    fn last_token(&self) -> String {
        let sent = self.mail.sent.lock().unwrap();
        let html = &sent.last().expect("a mail was sent").html;
        let start = html.find("reset_password_token=").expect("a reset link") + "reset_password_token=".len();
        html[start..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect()
    }
    fn user(&self, email: &str) -> Value {
        db(&self.dir).query_row("SELECT encrypted_password, reset_password_token, reset_password_sent_at, failed_attempts, locked_at, last_otp_at FROM users WHERE email = ?1",
            [email], |r| Ok(serde_json::json!([r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?,
                                               r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?]))).unwrap()
    }
    async fn signup(&self, browser: &mut Browser, email: &str, password: &str) -> Answer {
        browser.get(&self.app, "/signup").await;
        browser.post(&self.app, "/signup", &[("user[name]", "Ann Example"), ("user[email]", email), ("user[password]", password)]).await
    }
    async fn request_reset(&self, browser: &mut Browser, email: &str) -> Answer {
        browser.get(&self.app, "/password/new").await;
        browser.post(&self.app, "/password", &[("user[email]", email)]).await
    }
    async fn reset(&self, browser: &mut Browser, token: &str, password: &str, code: Option<&str>) -> Answer {
        browser.get(&self.app, "/login").await; // a page with the CSRF meta tag, whatever the token is worth
        let mut form = vec![("_method", "patch"), ("user[reset_password_token]", token), ("user[password]", password), ("user[password_confirmation]", password)];
        if let Some(code) = code { form.push(("user[otp_code_token]", code)); }
        browser.send(&self.app, "POST", "/password", Some(&form), Csrf::Header, &[]).await
    }
}

/// What a client can see of an answer, the per-request CSRF token aside.
fn visible(answer: &Answer) -> (u16, Option<String>, String) {
    let body = regex::Regex::new(r#"(name="authenticity_token" value|name="csrf-token" content|name="csp-nonce" content|nonce)="[^"]*""#).unwrap().replace_all(&answer.body, "$1=\"\"").to_string();
    (answer.status, answer.header("location").map(str::to_string), body)
}

#[test]
fn reset_token_digest_is_devises() {
    // tests/fixtures/devise_token_vectors.json, recorded by script/rust/devise_tokens.rb from Devise's own generator.
    let vectors: Value = serde_json::from_str(include_str!("fixtures/devise_token_vectors.json")).unwrap();
    assert_eq!(vectors["key_digest"], "OpenSSL::Digest::SHA1");
    assert_eq!(vectors["iterations"], 65536);
    let tokens = deltabadger::crypto::ResetTokens::new(vectors["secret_key_base"].as_str().unwrap());
    for case in vectors["cases"].as_array().unwrap() {
        assert_eq!(tokens.digest(case["raw"].as_str().unwrap()).as_deref(), case["digest"].as_str(), "{case}");
    }
    assert_eq!(vectors["blank_digest"], false);
    assert_eq!(tokens.digest(""), None);
    assert_eq!(tokens.digest("  "), None);
    let (raw, digest) = tokens.generate();
    assert_eq!((raw.len(), tokens.digest(&raw)), (20, Some(digest)));
}

#[tokio::test(flavor = "current_thread")]
async fn signup_taken_answers_as_a_new_account() {
    let h = harness();
    let (mut first, mut second) = (Browser::default(), Browser::default());
    let new = h.signup(&mut first, "new@example.com", PASSWORD).await;
    let taken = h.signup(&mut second, "Owner@Example.com", PASSWORD).await;
    assert_eq!((new.status, new.header("location")), (303, Some("/login")));
    assert_eq!(visible(&new), visible(&taken));
    // The landing (D-SIGNUP-NO-THANK-YOU) is the same page, with the same notice.
    let (landed_new, landed_taken) = (first.get(&h.app, "/login").await, second.get(&h.app, "/login").await);
    assert!(landed_new.body.contains("A message with a confirmation link has been sent to your email address."), "the signed_up_but_unconfirmed notice");
    assert_eq!(visible(&landed_new), visible(&landed_taken));
    assert_eq!(h.sent(), vec![("new@example.com".into(), "Confirm your email".into()), ("owner@example.com".into(), "Email already taken".into())]);
}

#[tokio::test(flavor = "current_thread")]
async fn signup_costs_one_bcrypt_on_every_path() {
    let count = Arc::new(AtomicUsize::new(0));
    let counter = count.clone();
    let h = harness_with(Behaviour::Deliver, &[], Some(Arc::new(move || { counter.fetch_add(1, Ordering::SeqCst); })));
    for (email, password, status) in [("new@example.com", PASSWORD, 303), ("owner@example.com", PASSWORD, 303), ("other@example.com", "simple", 422)] {
        let before = count.load(Ordering::SeqCst);
        let answer = h.signup(&mut Browser::default(), email, password).await;
        assert_eq!((answer.status, count.load(Ordering::SeqCst) - before), (status, 1), "{email}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_signups_with_one_address_make_one_account() {
    // D-SIGNUP-RACE: whichever comes second takes the taken path (owner mailed), never a 500.
    let h = harness();
    let (mut a, mut b) = (Browser::default(), Browser::default());
    a.get(&h.app, "/signup").await;
    b.get(&h.app, "/signup").await;
    let form = [("user[name]", "Ann Example"), ("user[email]", "race@example.com"), ("user[password]", PASSWORD)];
    let (x, y) = tokio::join!(a.post(&h.app, "/signup", &form), b.post(&h.app, "/signup", &form));
    assert_eq!(visible(&x), visible(&y));
    assert_eq!(x.status, 303);
    let rows: i64 = db(&h.dir).query_row("SELECT COUNT(*) FROM users WHERE email = 'race@example.com'", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1);
    let mut subjects: Vec<String> = h.sent().into_iter().map(|(_, s)| s).collect();
    subjects.sort();
    assert_eq!(subjects, ["Confirm your email", "Email already taken"]);
}

#[tokio::test(flavor = "current_thread")]
async fn signup_name_with_a_line_break_is_refused() {
    // D-SIGNUP-NAME-ONE-LINE: Rails' ^…$ takes "Ann\nanything"; a tab between words is still a name, as in Rails.
    let h = harness();
    for (name, status) in [("Ann\nEvil", 422), ("Ann\r\nEvil", 422), ("Ann\n", 422), ("Ann\tExample", 303)] {
        let mut browser = Browser::default();
        browser.get(&h.app, "/signup").await;
        let email = format!("n{}@example.com", name.len());
        let answer = browser.post(&h.app, "/signup", &[("user[name]", name), ("user[email]", &email), ("user[password]", PASSWORD)]).await;
        assert_eq!(answer.status, status, "{name:?}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn signup_is_throttled() {
    // D-SIGNUP-THROTTLE: five a minute per address, as /confirmation.
    let h = harness();
    let mut browser = Browser::default();
    let statuses: Vec<u16> = {
        let mut out = vec![];
        for i in 0..6 { out.push(h.signup(&mut browser, &format!("t{i}@example.com"), "simple").await.status); }
        out
    };
    assert_eq!(statuses, [422, 422, 422, 422, 422, 429]);
}

/// The answers a mail-dependent request gives with the mailer `behaviour`: a reset for a known and an unknown address,
/// a signup, and a confirmation resend for an unconfirmed account.
async fn answers(behaviour: Behaviour, env: &[(&str, &str)]) -> Vec<(u16, Option<String>, String)> {
    let h = harness_with(behaviour, env, None);
    db(&h.dir).execute("INSERT INTO users (name, email, encrypted_password, confirmation_token, created_at, updated_at) \
                        VALUES ('Ann', 'pending@example.com', 'x', 'pendingConfirmToken1', '2026-01-01', '2026-01-01')", []).unwrap();
    let mut out = vec![];
    for email in ["owner@example.com", "nobody@example.com"] {
        let mut browser = Browser::default();
        out.push(visible(&h.request_reset(&mut browser, email).await));
        out.push(visible(&browser.get(&h.app, "/login").await));
    }
    out.push(visible(&h.signup(&mut Browser::default(), "new@example.com", PASSWORD).await));
    for email in ["pending@example.com", "nobody@example.com"] {
        let mut browser = Browser::default();
        browser.get(&h.app, "/confirmation/new").await;
        out.push(visible(&browser.post(&h.app, "/confirmation", &[("user[email]", email)]).await));
    }
    out
}

#[tokio::test(flavor = "current_thread")]
async fn the_answer_never_depends_on_mail() {
    // D-ACCOUNT-MAIL-OFF-REQUEST: delivered, failing, stalled, no SMTP at all (the live mailer), malformed SMTP settings.
    let delivered = answers(Behaviour::Deliver, &[]).await;
    assert_eq!(delivered[0], delivered[2], "a reset for a known address answers as one for an unknown address");
    assert!(delivered[1].2.contains("you will receive a password recovery link"), "D-RESET-NOTICE-COPY: the reset copy, not the confirmation copy");
    assert_eq!(delivered[1], delivered[3]);
    assert_eq!(delivered[5], delivered[6], "a resend for a known address answers as one for an unknown address");
    for (behaviour, env) in [(Behaviour::Fail, &[][..]), (Behaviour::Stall, &[][..]), (Behaviour::Deliver, &[("SMTP_ADDRESS", "smtp.example.com"), ("SMTP_PORT", "not-a-port")][..])] {
        let answers = tokio::time::timeout(std::time::Duration::from_secs(30), answers(behaviour, env)).await.expect("no request waits for mail");
        assert_eq!(answers, delivered);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_live_mailer_without_smtp_answers_alike() {
    let dir = common::rails_install();
    let clock = TestClock::at(AT);
    let app = web_support::app(dir.path(), SECRET, clock); // settings::mail::live(), no SMTP configured
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    c.execute("INSERT INTO users (name, email, encrypted_password, admin, setup_completed, confirmed_at, created_at, updated_at) \
               VALUES ('Owner', 'owner@example.com', 'x', 1, 1, '2026-01-01', '2026-01-01', '2026-01-01')", []).unwrap();
    let mut seen = vec![];
    for email in ["owner@example.com", "nobody@example.com"] {
        let mut browser = Browser::default();
        browser.get(&app, "/password/new").await;
        seen.push(visible(&browser.post(&app, "/password", &[("user[email]", email)]).await));
    }
    assert_eq!(seen[0], seen[1]);
    assert_eq!(seen[0].0, 303);
}

#[tokio::test(flavor = "current_thread")]
async fn a_full_mail_pool_drops_mail_not_answers() {
    // Every permit held by a stalled delivery: the fifth mail is dropped, and its request answers as the others (five: the
    // throttle allows five reset requests a minute).
    let h = harness_with(Behaviour::Stall, &[], None);
    let mut seen = vec![];
    for _ in 0..5 { seen.push(visible(&h.request_reset(&mut Browser::default(), "owner@example.com").await)); }
    assert!(seen.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(h.app.account_mail_slots.available_permits(), 0);
    assert_eq!(h.sent().len(), 4, "the dropped mail never reaches the mailer");
}

#[tokio::test(flavor = "current_thread")]
async fn the_mailed_link_resets_once_and_a_newer_link_supersedes_it() {
    let h = harness();
    let mut browser = Browser::default();
    h.request_reset(&mut browser, "owner@example.com").await;
    let first = h.last_token();
    h.request_reset(&mut browser, "owner@example.com").await;
    let second = h.last_token();
    assert_ne!(first, second);
    // The first link died with the second request.
    let stale = browser.get(&h.app, &format!("/password/edit?reset_password_token={first}")).await;
    assert_eq!((stale.status, stale.header("location")), (302, Some("/password/new")));
    let page = browser.get(&h.app, &format!("/password/edit?reset_password_token={second}")).await;
    assert_eq!(page.status, 200);
    // The token is in the form once, and in no language link.
    assert_eq!(page.body.matches(second.as_str()).count(), 1);
    let done = h.reset(&mut browser, &second, NEW_PASSWORD, None).await;
    assert_eq!((done.status, done.header("location")), (303, Some("/login")));
    let row = h.user("owner@example.com");
    assert!(deltabadger::crypto::verify_password(NEW_PASSWORD, row[0].as_str().unwrap()));
    assert_eq!((&row[1], &row[2]), (&Value::Null, &Value::Null));
    // Replayed: back to the request page (D-RESET-UPDATE-STALE-TOKEN), nothing written.
    let again = h.reset(&mut browser, &second, "Third-horse-11", None).await;
    assert_eq!((again.status, again.header("location")), (303, Some("/password/new")));
    assert!(browser.get(&h.app, "/password/new").await.body.contains("Your password reset link has expired. Please request a new link."));
    assert_eq!(h.user("owner@example.com"), row);
}

#[tokio::test(flavor = "current_thread")]
async fn one_token_resets_once_when_submitted_twice_at_once() {
    let h = harness();
    let mut browser = Browser::default();
    h.request_reset(&mut browser, "owner@example.com").await;
    let token = h.last_token();
    let (mut a, mut b) = (Browser::default(), Browser::default());
    let (x, y) = tokio::join!(h.reset(&mut a, &token, NEW_PASSWORD, None), h.reset(&mut b, &token, "Other-horse-12", None));
    let mut locations = vec![x.header("location").unwrap().to_string(), y.header("location").unwrap().to_string()];
    locations.sort();
    assert_eq!(locations, ["/login", "/password/new"]);
    let hash = h.user("owner@example.com")[0].as_str().unwrap().to_string();
    let winner = if x.header("location") == Some("/login") { NEW_PASSWORD } else { "Other-horse-12" };
    assert!(deltabadger::crypto::verify_password(winner, &hash));
}

#[tokio::test(flavor = "current_thread")]
async fn two_factor_switched_on_while_bcrypt_runs_is_enforced() {
    // The write is decided on the row as it is after the computation, not before.
    let flip: Arc<Mutex<Option<std::path::PathBuf>>> = Arc::new(Mutex::new(None));
    let target = flip.clone();
    let h = harness_with(Behaviour::Deliver, &[], Some(Arc::new(move || {
        if let Some(path) = target.lock().unwrap().take() {
            Connection::open(path).unwrap().execute("UPDATE users SET otp_module = 1, otp_secret_key = 'x' WHERE email = 'owner@example.com'", []).unwrap();
        }
    })));
    let mut browser = Browser::default();
    h.request_reset(&mut browser, "owner@example.com").await;
    let token = h.last_token();
    let before = h.user("owner@example.com");
    *flip.lock().unwrap() = Some(h.dir.path().join("production.sqlite3"));
    let answer = h.reset(&mut browser, &token, NEW_PASSWORD, None).await;
    assert_eq!(answer.status, 422);
    assert!(answer.body.contains("Missed 2FA code. Try again."));
    assert_eq!(h.user("owner@example.com"), before, "the password, the token and its time are unchanged");
}

#[tokio::test(flavor = "current_thread")]
async fn a_lock_expired_on_the_clock_is_lifted_and_the_counter_is_shared_with_login() {
    let h = harness();
    let c = db(&h.dir);
    c.execute("UPDATE users SET otp_module = 1, otp_secret_key = ?1, failed_attempts = 4 WHERE email = 'owner@example.com'",
              [h.app.cipher.encrypt_binary("JBSWY3DPEHPK3PXP")]).unwrap();
    let mut browser = Browser::default();
    h.request_reset(&mut browser, "owner@example.com").await;
    let token = h.last_token();
    // The fifth failure, on the reset form, locks the account for /login too.
    assert_eq!(h.reset(&mut browser, &token, NEW_PASSWORD, Some("000000")).await.status, 422);
    let locked = h.user("owner@example.com");
    assert_eq!(locked[3], 5);
    assert!(locked[4].is_string());
    let mut login = Browser::default();
    login.get(&h.app, "/login").await;
    let refused = login.post(&h.app, "/login", &[("user[email]", "owner@example.com"), ("user[password]", PASSWORD)]).await;
    assert_ne!(refused.status, 303);
    h.clock.set(web_support::at("2026-09-10T12:16:00Z"));
    // Fifteen minutes on, the reset page lifts the lock even when it refuses the request for its blank code (Rails' order).
    let blank = h.reset(&mut browser, &token, NEW_PASSWORD, Some("")).await;
    assert_eq!(blank.status, 422);
    let lifted = h.user("owner@example.com");
    assert_eq!((&lifted[3], &lifted[4]), (&serde_json::json!(0), &Value::Null));
    assert_eq!(lifted[0], locked[0]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_malformed_gmail_alias_answers_alike_with_or_without_its_account() {
    // D-SIGNUP-TAKEN-NEEDS-VALID-ADDRESS: Rails drops the format error once the alias is taken, so "ann+!@gmail.com"
    // passed (303, the owner mailed) when ann@gmail.com existed and was refused (422) otherwise.
    let mut seen = vec![];
    for existing in [true, false] {
        let h = harness();
        if existing {
            db(&h.dir).execute("INSERT INTO users (name, email, encrypted_password, created_at, updated_at) VALUES ('Ann', 'ann@gmail.com', 'x', '2026-01-01', '2026-01-01')", []).unwrap();
        }
        let answer = h.signup(&mut Browser::default(), "ann+!@gmail.com", PASSWORD).await;
        assert_eq!(answer.status, 422);
        assert!(h.sent().is_empty(), "nobody is mailed for a malformed address");
        seen.push(visible(&answer));
    }
    assert_eq!(seen[0], seen[1]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_reset_request_waits_for_a_busy_database_alike_for_any_address() {
    // Known or not, the request takes the write lock: held elsewhere, both wait for it, and answer alike once it is free.
    let h = harness();
    let hold = std::time::Duration::from_millis(600);
    let mut seen = vec![];
    for email in ["owner@example.com", "nobody@example.com"] {
        let mut browser = Browser::default();
        browser.get(&h.app, "/password/new").await;
        let path = h.dir.path().join("production.sqlite3");
        let locked = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let c = Connection::open(path).unwrap();
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            locked.0.send(()).unwrap();
            std::thread::sleep(hold);
            c.execute_batch("COMMIT").unwrap();
        });
        locked.1.recv().unwrap();
        let started = std::time::Instant::now();
        let answer = browser.post(&h.app, "/password", &[("user[email]", email)]).await;
        assert!(started.elapsed() >= hold / 2, "{email} did not wait for the write lock");
        holder.join().unwrap();
        seen.push(visible(&answer));
    }
    assert_eq!(seen[0], seen[1]);
}
