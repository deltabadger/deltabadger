//! `deltabadger serve`, the routes around the pages, and the answer for what is not served yet.
mod common;
use common::web::{self, Browser, TestClock};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const NOW: &str = "2026-09-10T12:00:30Z";

fn serve_command(dir: &std::path::Path, port: u16) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deltabadger"));
    command.arg("serve").env("STORAGE_DIR", dir).env("SECRET_KEY_BASE", web::SECRET).env("PORT", port.to_string()).stdin(Stdio::null());
    for var in ["DATABASE_URL", "PRIMARY_DATABASE_URL", "QUEUE_DATABASE_URL", "CACHE_DATABASE_URL", "CABLE_DATABASE_URL", "DATABASE_PATH", "QUEUE_DATABASE_PATH"] {
        command.env_remove(var);
    }
    command
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// One plain HTTP/1.1 GET; `None` while nothing listens yet.
fn http_get(port: u16, path: &str) -> Option<String> {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").ok()?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;
    Some(answer)
}

#[test]
fn serve_refuses_an_install_with_no_admin_user() {
    let dir = common::rails_install();
    let out = serve_command(dir.path(), free_port()).output().unwrap();
    assert!(!out.status.success());
    let message = String::from_utf8_lossy(&out.stderr);
    assert!(message.contains("no admin user") && message.contains("Rails app"), "{message}");
}

#[test]
fn serve_needs_the_secret_key_base() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let out = serve_command(dir.path(), free_port()).env_remove("SECRET_KEY_BASE").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("SECRET_KEY_BASE is not set"), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn serve_answers_on_its_port_and_holds_the_engine_lock() {
    let (dir, opened, _) = common::install(); // seeds an admin user
    drop(opened);
    let port = free_port();
    let mut server = serve_command(dir.path(), port).stderr(Stdio::piped()).spawn().unwrap();
    let started = Instant::now();
    let answer = loop {
        if let Some(answer) = http_get(port, "/up") { break answer; }
        assert!(started.elapsed() < Duration::from_secs(10) && server.try_wait().unwrap().is_none(), "serve did not come up");
        std::thread::sleep(Duration::from_millis(50));
    };
    let second = Command::new(env!("CARGO_BIN_EXE_deltabadger")).arg("check").env("STORAGE_DIR", dir.path()).output().unwrap();
    server.kill().unwrap();
    server.wait().unwrap();
    assert!(answer.starts_with("HTTP/1.1 200") && answer.contains("background-color: green"), "{answer}");
    assert!(!second.status.success() && String::from_utf8_lossy(&second.stderr).contains("another Deltabadger engine"), "the lock is held while serving");
}

#[tokio::test(flavor = "current_thread")]
async fn up_is_rails_health_page_without_a_session() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let answer = Browser::default().get(&app, "/up").await;
    assert_eq!((answer.status, answer.body.as_str()), (200, "<!DOCTYPE html><html><body style=\"background-color: green\"></body></html>"));
    assert_eq!(answer.header("set-cookie"), None);
}

#[tokio::test(flavor = "current_thread")]
async fn a_page_this_build_does_not_serve_is_a_501_that_names_it() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();
    for path in ["/tracker", "/de/settings/account?tab=x", "/setup", "/zz/login", "/assets/application.css"] {
        let answer = browser.get(&app, path).await;
        assert_eq!(answer.status, 501, "{path}");
        assert!(answer.body.contains(&format!("GET {path}")), "{path}: {}", answer.body);
        assert_eq!(answer.header("location"), None, "never a redirect");
    }
    let framed = browser.send(&app, "GET", "/bots/new", None, web::Csrf::None, &[("turbo-frame", "modal")]).await;
    assert!(framed.status == 501 && framed.body.contains("<turbo-frame id=\"modal\">"), "Turbo shows the message in the frame it asked for: {}", framed.body);
    assert_eq!(browser.send(&app, "PUT", "/up", None, web::Csrf::None, &[]).await.status, 501, "a method a route does not take");
    let escaped = browser.get(&app, "/search?a=1&b=%3Cscript%3E").await;
    assert!(escaped.body.contains("GET /search?a=1&#38;b=%3Cscript%3E"), "the path is text, never markup: {}", escaped.body);
}

#[tokio::test(flavor = "current_thread")]
async fn static_files_and_locale_prefixes_are_handled_before_routing() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();

    let css_path = deltabadger::web::assets::path("application.css");
    let css = browser.get(&app, css_path).await;
    assert_eq!((css.status, css.header("content-type"), css.header("cache-control")), (200, Some("text/css; charset=utf-8"), Some("public, max-age=31536000")));
    assert!(css.body.len() > 100_000 && css.header("set-cookie").is_none());
    let head = browser.send(&app, "HEAD", css_path, None, web::Csrf::None, &[]).await;
    assert_eq!((head.status, head.body.as_str(), head.header("content-length")), (200, "", css.header("content-length")));
    assert_eq!(browser.get(&app, "/fonts/Dosis-digits.woff2").await.header("content-type"), Some("font/woff2"));

    assert_eq!(browser.get(&app, "//up/").await.status, 200, "repeated and trailing slashes are not part of a path");
    for (path, named) in [("/de/up", "GET /de/up"), ("/en/nothing?x=1", "GET /en/nothing?x=1"), ("/de/cable", "GET /de/cable")] {
        let answer = browser.get(&app, path).await;
        assert!(answer.status == 501 && answer.body.contains(named), "{path}: {} {}", answer.status, answer.body);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_form_post_can_carry_another_method() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();
    for (given, routed) in [("patch", "PATCH"), ("PUT", "PUT"), ("Delete", "DELETE"), ("get", "POST"), ("trace", "POST")] {
        let answer = browser.send(&app, "POST", "/up", Some(&[("a", "1"), ("_method", given)]), web::Csrf::None, &[]).await;
        assert!(answer.status == 501 && answer.body.contains(&format!("{routed} /up")), "_method={given}: {}", answer.body);
    }
    let plain = browser.send(&app, "POST", "/up", None, web::Csrf::None, &[("content-type", "application/json")]).await;
    assert!(plain.body.contains("POST /up"), "only a form body is read: {}", plain.body);
    let get = browser.send(&app, "GET", "/up?_method=delete", None, web::Csrf::None, &[]).await;
    assert_eq!(get.status, 200, "the query string cannot change the method");
    let huge = "x".repeat(1024 * 1024);
    assert_eq!(browser.send(&app, "POST", "/up", Some(&[("field", &huge)]), web::Csrf::None, &[]).await.status, 413);
}

#[tokio::test(flavor = "current_thread")]
async fn up_carries_the_headers_rails_middleware_adds() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let answer = Browser::default().get(&app, "/up").await;
    assert!(answer.header("content-security-policy-report-only").unwrap().contains("script-src 'self' 'nonce-"));
    assert_eq!(answer.header("x-frame-options"), Some("SAMEORIGIN"));
    assert_eq!(answer.header("cache-control"), Some("max-age=0, private, must-revalidate"));
    assert_eq!(answer.header("content-security-policy"), None, "nothing is enforced yet: report-only, as in Rails");
}

#[tokio::test(flavor = "current_thread")]
async fn a_csp_report_is_accepted_without_a_session_or_a_token() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let answer = Browser::default().send(&app, "POST", "/csp-report", None, web::Csrf::None, &[("content-type", "application/csp-report")]).await;
    assert_eq!((answer.status, answer.header("set-cookie"), answer.header("location")), (204, None, None));
}

#[tokio::test(flavor = "current_thread")]
async fn the_bots_page_refuses_an_account_it_cannot_render_yet() {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    assert_eq!(browser.get(&app, "/bots").await.status, 200, "no bots, no balances: the empty page");

    let hold = |asset_id: i64| {
        opened.primary.execute("INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, usd_value, synced_at, created_at, updated_at) \
                                VALUES (?1, ?2, ?3, 1, 0, 50000.0, ?4, ?4, ?4)", (seeded.user_id, seeded.exchange_id, asset_id, "2026-01-01 00:00:00")).unwrap();
    };
    hold(seeded.quote); // the Kraken fixtures' quote asset is EUR: cash
    assert_eq!(browser.get(&app, "/bots").await.status, 200, "cash only and cash not shown: Rails draws the plain circle, and so does this page");
    opened.primary.execute("UPDATE users SET tracker_settings = '{\"show_cash\":true}' WHERE id = ?1", [seeded.user_id]).unwrap();
    assert_eq!(browser.get(&app, "/bots").await.status, 501, "the tracker shows cash: the ring would be drawn");
    opened.primary.execute("UPDATE users SET tracker_settings = '{}' WHERE id = ?1", [seeded.user_id]).unwrap();
    hold(seeded.btc);
    let with_holdings = browser.get(&app, "/bots").await;
    assert!(with_holdings.status == 501 && with_holdings.body.contains("GET /bots"), "the tracker ring is not ported: {}", with_holdings.body);
    opened.primary.execute("DELETE FROM account_balances", []).unwrap();

    let bot = common::seed::insert_bot(&opened.primary, &seeded, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    assert_eq!(browser.get(&app, "/bots").await.status, 501, "the bot list is the next plan");
    opened.primary.execute("UPDATE bots SET status = 3 WHERE id = ?1", [bot]).unwrap();
    assert_eq!(browser.get(&app, "/bots").await.status, 200, "a deleted bot does not count");
}

/// The cookie is written when the session changed, not on every response. A request that left the
/// browser before a sign-in and is answered after it (Chrome asks for the manifest's start_url in
/// the background) therefore does not put the signed-out session back. With a cookie on every
/// response, as Rails sends it, this sign-in was lost (Finding 18).
#[tokio::test(flavor = "current_thread")]
async fn a_late_answer_to_an_older_request_does_not_undo_a_sign_in() {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let clock = TestClock::at(NOW);
    let app = web::app(dir.path(), web::SECRET, clock.clone());
    let mut browser = Browser::default();
    assert!(browser.get(&app, "/login").await.header("set-cookie").is_some(), "the first page gives the session its CSRF token");
    let before_sign_in = browser.cookie.clone();
    let signed_in = browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await;
    assert!(signed_in.status == 303 && signed_in.header("set-cookie").is_some());

    // The background request: sent with the cookie from before the sign-in, answered after it.
    let mut late = Browser { cookie: before_sign_in, ..Browser::default() };
    let root = late.get(&app, "/").await;
    assert_eq!((root.status, root.header("location"), root.header("set-cookie")), (302, Some("/login"), None), "nothing changed, so no cookie");
    let login = late.get(&app, "/login").await;
    assert_eq!((login.status, login.header("set-cookie")), (200, None));
    // No answer since the sign-in carried a cookie: the browser's jar still holds the signed-in one.
    assert_eq!(browser.get(&app, "/").await.header("location"), Some("/bots"));

    // An unchanged session is never written again, however old its cookie is: the 30 days run from
    // the last change of its content, and then the session is over.
    browser.get(&app, "/bots").await; // consumes the first-page flag: the session changes once more
    for days in [0, 1, 29] {
        clock.set(web::at(NOW) + chrono::Duration::days(days));
        let page = browser.get(&app, "/bots").await;
        assert_eq!((page.status, page.header("set-cookie")), (200, None), "day {days}");
    }
    clock.set(web::at(NOW) + chrono::Duration::days(30));
    assert_eq!(browser.get(&app, "/bots").await.status, 302, "30 days after its last change the session has ended");
}

/// The cookie from before a sign-in may be days old. Its age gives no response a reason to send it
/// again: only a change of content does.
#[tokio::test(flavor = "current_thread")]
async fn an_old_anonymous_cookie_does_not_undo_a_sign_in() {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let clock = TestClock::at(NOW);
    let app = web::app(dir.path(), web::SECRET, clock.clone());
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    clock.set(web::at(NOW) + chrono::Duration::hours(25)); // the login page was left open overnight
    assert_eq!(browser.get(&app, "/login").await.header("set-cookie"), None, "a day-old cookie with nothing new in it is left alone");
    let anonymous = browser.cookie.clone();
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);

    // A request that left with the old anonymous cookie is answered after the sign-in.
    let mut late = Browser { cookie: anonymous, ..Browser::default() };
    assert_eq!(late.get(&app, "/").await.header("set-cookie"), None);
    assert_eq!(late.get(&app, "/login").await.header("set-cookie"), None);
    assert_eq!(browser.get(&app, "/").await.header("location"), Some("/bots"), "still signed in");
}

/// Signing out while another request of the same browser is still on its way. That request was
/// signed in when it left, so it is served; its answer must not hand the signed-in cookie back.
#[tokio::test(flavor = "current_thread")]
async fn a_request_authenticated_before_logout_that_completes_after_it_does_not_restore_the_session() {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let clock = TestClock::at(NOW);
    let app = web::app(dir.path(), web::SECRET, clock.clone());
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    browser.get(&app, "/bots").await; // consumes the first-page flag
    let in_flight = browser.cookie.clone(); // what a request sent now carries

    // However old the session's cookie is when the sign-out happens. With a cookie that was sent
    // again once it was a day old, the late answer below put the signed-in session back.
    for days in [0, 1, 29] {
        clock.set(web::at(NOW) + chrono::Duration::days(days));
        let mut tab = Browser { cookie: in_flight.clone(), page: browser.page.clone() };
        let signed_out = tab.post(&app, "/logout", &[("_method", "delete")]).await;
        assert!(signed_out.status == 303 && signed_out.header("set-cookie").is_some(), "day {days}: signing out changes the session, so it is written");

        let mut late = Browser { cookie: in_flight.clone(), ..Browser::default() };
        let answer = late.get(&app, "/bots").await;
        assert_eq!((answer.status, answer.header("set-cookie")), (200, None), "day {days}: served, and no cookie comes back with it");
        assert_eq!(tab.get(&app, "/bots").await.status, 302, "day {days}: the browser holds what the sign-out gave it");
    }
}

/// Behind a proxy that terminates TLS the request arrives over plain http, with the Host the proxy
/// uses upstream. The browser's `Origin` is the public one, in its canonical spelling, and
/// APP_ROOT_URL may spell the same origin with the default port and capitals.
#[tokio::test(flavor = "current_thread")]
async fn a_form_is_accepted_behind_a_tls_terminating_proxy_when_app_root_url_spells_the_default_port() {
    let (_dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let env = |name: &str| match name {
        "SECRET_KEY_BASE" => Some(web::SECRET.to_string()),
        "APP_ROOT_URL" => Some("https://Bot.Example:443/".to_string()),
        _ => None,
    };
    let app = deltabadger::web::App::new(deltabadger::web::Config::from_env(&env).unwrap(), &env, opened.primary, TestClock::at(NOW)).unwrap();
    let form: [(&str, &str); 2] = [("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")];
    let submit = |origin: &'static str| {
        let app = app.clone();
        async move {
            let mut browser = Browser::default();
            browser.get(&app, "/login").await;
            browser.send(&app, "POST", "/login", Some(&form), web::Csrf::Form, &[("origin", origin), ("referer", "https://bot.example/login")]).await
        }
    };
    let accepted = submit("https://bot.example").await;
    assert_eq!((accepted.status, accepted.header("location")), (303, Some("/")), "the request's own Host is localhost:3000 over http");
    assert!(accepted.header("set-cookie").is_some_and(|cookie| cookie.contains("; secure;")), "{:?}", accepted.header("set-cookie"));
    for foreign in ["http://bot.example", "https://bot.example:8443", "https://bot.example.evil.test"] {
        let refused = submit(foreign).await;
        assert_eq!((refused.status, refused.header("location")), (302, Some("/login")), "{foreign}: back to the referer, which is ours");
    }
}

/// The session is the cookie: nothing is kept on the server, so there is nothing to revoke
/// (Rails' CookieStore is the same). Signing out replaces the browser's cookie and no more.
#[tokio::test(flavor = "current_thread")]
async fn signing_out_empties_this_browsers_session_and_cannot_revoke_a_copy_of_its_cookie() {
    let (dir, opened, seeded) = common::install();
    let password = |plain: &str| {
        let hash = deltabadger::crypto::hash_password(plain).unwrap();
        opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    };
    password("Correct-horse-9");
    let clock = TestClock::at(NOW);
    let app = web::app(dir.path(), web::SECRET, clock.clone());
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    assert_eq!(browser.get(&app, "/bots").await.status, 200);
    let copied = browser.cookie.clone(); // what someone holds who read the cookie before the sign-out
    let copy = || Browser { cookie: copied.clone(), ..Browser::default() };

    assert_eq!(browser.post(&app, "/logout", &[("_method", "delete")]).await.status, 303, "the navbar's own form, with its own token");
    assert_eq!(browser.get(&app, "/bots").await.status, 302, "this browser is signed out");

    assert_eq!(copy().get(&app, "/bots").await.status, 200, "the copy still opens the app: sign-out revokes nothing");
    clock.set(web::at(NOW) + chrono::Duration::days(30) + chrono::Duration::seconds(1));
    assert_eq!(copy().get(&app, "/bots").await.status, 302, "until it expires, 30 days after it was issued");
    clock.set(web::at(NOW));
    password("Another-horse-7");
    assert_eq!(copy().get(&app, "/bots").await.status, 302, "or until the password changes, which ends every session");
}

/// A page on a sibling subdomain can set a cookie of our name for the whole site with a longer Path.
/// The browser then sends it before the real one. It opens nothing, and must not stand in the real
/// one's way: the session is the first cookie of our exact name that opens.
#[tokio::test(flavor = "current_thread")]
async fn a_planted_cookie_of_our_name_does_not_hide_the_real_session() {
    const NAME: &str = "_deltabadger_rust_session";
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    let ours = browser.cookie.clone().unwrap();
    let expired = deltabadger::web::session::seal(&app.keys.session, &Default::default(), web::at(NOW) - chrono::Duration::days(31));
    let bots = |cookies: String| {
        let app = app.clone();
        async move { Browser::default().send(&app, "GET", "/bots", None, web::Csrf::None, &[("cookie", &cookies)]).await.status }
    };
    assert_eq!(bots(format!("{NAME}={ours}")).await, 200);
    assert_eq!(bots(format!("{NAME}=junk; {NAME}={ours}")).await, 200, "junk first, then the real one");
    assert_eq!(bots(format!("{NAME}=junk; other=1; {NAME}={expired}; {NAME}=; {NAME}={ours}")).await, 200, "nor does one of ours that has expired");
    assert_eq!(bots(format!("{NAME}=junk; {NAME}=more-junk")).await, 302, "nothing opens: no session");
    assert_eq!(bots(format!("{NAME}_x={ours}")).await, 302, "a longer name is another cookie");
    assert_eq!(bots(format!("{NAME}_x={ours}; {NAME}=junk")).await, 302);
}

/// Where an unauthenticated GET was heading is kept in the session, and the session is the cookie.
/// A browser drops a cookie of more than 4096 bytes, and the CSRF token with it, so the sign-in that
/// follows would be refused. A path over 2,048 bytes is therefore not kept: the request is sent to
/// the login page all the same, and the sign-in lands on the root.
#[tokio::test(flavor = "current_thread")]
async fn a_return_path_too_long_for_the_cookie_is_not_kept() {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    for (length, kept) in [(2048, true), (2049, false), (3000, false)] {
        let path = format!("/bots?x={}", "a".repeat(length - "/bots?x=".len()));
        assert_eq!(path.len(), length);
        let mut browser = Browser::default();
        let bounced = browser.get(&app, &path).await;
        assert_eq!((bounced.status, bounced.header("location")), (302, Some("/login")), "{length}: sent to the login page either way");
        let cookie = bounced.header("set-cookie").expect("the flash is new");
        assert!(cookie.len() <= 4096, "{length}: a Set-Cookie of {} bytes is one a browser drops", cookie.len());
        browser.get(&app, "/login").await;
        let signed_in = browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await;
        assert_eq!((signed_in.status, signed_in.header("location")), (303, Some(if kept { path.as_str() } else { "/" })), "{length}");
    }
    // A long path does not leave an earlier one in place either: the sign-in goes to the root.
    let mut browser = Browser::default();
    browser.get(&app, "/bots?filter=active").await;
    browser.get(&app, &format!("/bots?x={}", "a".repeat(3000))).await;
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.header("location"), Some("/"));
}

/// Exactly one bcrypt computation per sign-in attempt, whether the email is known or not: how long a
/// refusal takes must not say which. bcrypt at cost 11 is nearly all of either request, so an unknown
/// email answered without it would be many times faster; a quarter is far outside any noise.
#[tokio::test(flavor = "current_thread")]
async fn a_sign_in_for_an_unknown_email_costs_what_a_wrong_password_costs() {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    let mut timed = async |email: &str| {
        let started = Instant::now();
        assert_eq!(browser.post(&app, "/login", &[("user[email]", email), ("user[password]", "wrong")]).await.status, 422);
        started.elapsed()
    };
    let (known, unknown) = (timed("o@example.com").await, timed("nobody@example.com").await);
    assert!(unknown * 4 > known, "an unknown email was refused in {unknown:?}, a wrong password in {known:?}");
}

/// What no in-process test can see: the first page after sign-in in a real browser, with the compiled
/// JS and CSS (script/rust/browser_check.mjs drives headless Chrome). It needs Chrome and bun, so it
/// is not part of `cargo test`: run it with `cargo test --test serve -- --ignored`.
#[test]
#[ignore = "needs Chrome and bun: cargo test --test serve -- --ignored"]
fn a_real_browser_signs_in_and_sees_the_app_with_live_streams() {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    drop(opened);
    let port = free_port();
    let mut server = serve_command(dir.path(), port).spawn().unwrap();
    let started = Instant::now();
    while http_get(port, "/up").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10) && server.try_wait().unwrap().is_none(), "serve did not come up");
        std::thread::sleep(Duration::from_millis(50));
    }
    let check = Command::new("bun").arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../script/rust/browser_check.mjs"))
        .env("BASE_URL", format!("http://127.0.0.1:{port}")).env("EMAIL", "o@example.com").env("PASSWORD", "Correct-horse-9").output();
    server.kill().unwrap();
    server.wait().unwrap();
    let check = check.expect("bun runs the browser check");
    assert!(check.status.success(), "{}\n{}", String::from_utf8_lossy(&check.stdout), String::from_utf8_lossy(&check.stderr));
}
