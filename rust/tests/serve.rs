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
    for path in ["/tracker/import/new", "/de/settings/account?tab=x", "/setup", "/zz/login", "/assets/application.css"] {
        let answer = browser.get(&app, path).await;
        assert_eq!(answer.status, 501, "{path}");
        assert!(answer.body.contains(&format!("GET {path}")), "{path}: {}", answer.body);
        assert_eq!(answer.header("location"), None, "never a redirect");
    }
    let framed = browser.send(&app, "GET", "/bots/new", None, web::Csrf::None, &[("turbo-frame", "modal")]).await;
    assert!(framed.status == 501 && framed.body.contains("<turbo-frame id=\"modal\">"), "Turbo shows the message in the frame it asked for: {}", framed.body);
    assert_eq!(browser.send(&app, "PUT", "/up", None, web::Csrf::None, &[]).await.status, 501, "a method a route does not take");
    for method in ["POST", "PUT", "DELETE"] {
        let cable = browser.send(&app, method, "/cable", None, web::Csrf::None, &[]).await;
        assert!(cable.status == 501 && cable.body.contains(&format!("{method} /cable")), "{method} /cable: {} {}", cable.status, cable.body);
    }
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
    // Only a POST: Rack::MethodOverride reads `_method` from no other request, whatever its body says.
    for (method, field) in [("PUT", "delete"), ("PATCH", "delete"), ("DELETE", "put")] {
        let answer = browser.send(&app, method, "/up", Some(&[("_method", field)]), web::Csrf::None, &[]).await;
        assert!(answer.status == 501 && answer.body.contains(&format!("{method} /up")), "{method} with _method={field}: {}", answer.body);
    }
    assert_eq!(browser.send(&app, "GET", "/up", Some(&[("_method", "delete")]), web::Csrf::None, &[]).await.status, 200, "a GET with a form body is a GET");
    let plain = browser.send(&app, "POST", "/up", None, web::Csrf::None, &[("content-type", "application/json")]).await;
    assert!(plain.body.contains("POST /up"), "only a form body is read: {}", plain.body);
    let get = browser.send(&app, "GET", "/up?_method=delete", None, web::Csrf::None, &[]).await;
    assert_eq!(get.status, 200, "the query string cannot change the method");
    let huge = "x".repeat(1024 * 1024);
    assert_eq!(browser.send(&app, "POST", "/up", Some(&[("field", &huge)]), web::Csrf::None, &[]).await.status, 413);
    let report = browser.send(&app, "POST", "/csp-report", Some(&[("field", &huge)]), web::Csrf::None, &[("content-type", "application/csp-report")]).await;
    assert_eq!(report.status, 204, "not a form: its body is not read at all, whatever its size");
}

/// A form is read before any route, session or limit sees the request, so its size is bounded
/// first: 64 KiB and 1,000 fields. The largest form of the pages served so far is the login form,
/// under 1 KiB.
#[tokio::test(flavor = "current_thread")]
async fn a_form_is_bounded_in_bytes_and_in_fields_before_it_is_routed() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();
    let mut status_of_bytes = async |bytes: usize| {
        let value = "x".repeat(bytes - "field=".len());
        browser.send(&app, "POST", "/up", Some(&[("field", &value)]), web::Csrf::None, &[]).await.status
    };
    assert_eq!(status_of_bytes(64 * 1024).await, 501, "64 KiB is read and routed (POST /up is not served)");
    assert_eq!(status_of_bytes(64 * 1024 + 1).await, 413, "one byte more is not");
    let mut status_of_fields = async |fields: usize| {
        let form: Vec<(&str, &str)> = vec![("a", "1"); fields];
        browser.send(&app, "POST", "/up", Some(&form), web::Csrf::None, &[]).await.status
    };
    assert_eq!(status_of_fields(1000).await, 501);
    assert_eq!(status_of_fields(1001).await, 400);
}

/// The query string is bounded like a form, and as early: 8 KiB and 1,000 fields, whatever the path.
#[tokio::test(flavor = "current_thread")]
async fn a_query_string_is_bounded_in_bytes_and_in_fields_before_it_is_routed() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();
    let fields = |count: usize| (0..count).map(|n| format!("k{n}=1")).collect::<Vec<_>>().join("&");
    let started = Instant::now();
    let page = browser.get(&app, &format!("/login?{}", fields(1000))).await;
    assert!(page.status == 200 && page.body.contains("href=\"/de/login?k0=1&#38;k100=1&#38;k101=1&#38;"), "1,000 fields are read, and the language links carry them: {} {:.300}", page.status, page.body.split("dropdown__item").nth(1).unwrap_or(""));
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    assert_eq!(browser.get(&app, &format!("/login?{}", fields(1001))).await.status, 400);
    let bytes = |count: usize| format!("x={}", "a".repeat(count - 2));
    assert_eq!(browser.get(&app, &format!("/login?{}", bytes(8 * 1024))).await.status, 200, "8 KiB");
    assert_eq!(browser.get(&app, &format!("/login?{}", bytes(8 * 1024 + 1))).await.status, 414, "one byte more");
    for path in ["/robots.txt", "/up", "/nothing-here", "/cable"] {
        assert_eq!(browser.get(&app, &format!("{path}?{}", bytes(8 * 1024 + 1))).await.status, 414, "{path}: before any route, static files included");
        assert_eq!(browser.get(&app, &format!("{path}?{}", fields(1001))).await.status, 400, "{path}");
    }
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
async fn the_bot_pages_refuse_what_this_build_does_not_render_and_change_nothing() {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00', wash_sale_enabled = 0 WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    assert_eq!(browser.get(&app, "/bots").await.status, 200, "no bots, no balances: the empty page");

    // The tracker's ring is drawn now, so holdings no longer refuse the page.
    opened.primary.execute("INSERT INTO account_balances (user_id, exchange_id, asset_id, free, locked, usd_value, synced_at, created_at, updated_at) \
                            VALUES (?1, ?2, ?3, 1, 0, 50000.0, ?4, ?4, ?4)", (seeded.user_id, seeded.exchange_id, seeded.btc, "2026-01-01 00:00:00")).unwrap();
    let with_holdings = browser.get(&app, "/bots").await;
    assert!(with_holdings.status == 200 && with_holdings.body.contains("stroke-dasharray=\"52.55 4.0\""), "one holding: one arc around the whole ring");

    // One bot: the list is its page. The seeded bot is on Kraken, which this build's pages do not serve.
    let first = common::seed::insert_bot(&opened.primary, &seeded, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    opened.primary.execute("UPDATE bots SET label = 'Bitcoin'", []).unwrap();
    let list = browser.get(&app, "/bots").await;
    assert_eq!((list.status, list.header("location")), (302, Some(format!("/bots/{first}").as_str())));
    let page = browser.get(&app, &format!("/bots/{first}")).await;
    assert!(page.status == 501 && page.body.contains(&format!("GET /bots/{first}")), "{}", page.body);
    assert_eq!(browser.get(&app, &format!("/bots/{first}/chart")).await.status, 501);
    // Two bots: the list, refused for the same reason.
    let second = common::seed::insert_bot(&opened.primary, &seeded, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    opened.primary.execute("UPDATE bots SET label = 'Bitcoin'", []).unwrap();
    assert_eq!(browser.get(&app, "/bots").await.status, 501);
    // A bot of a class these pages do not know is refused before its row is read as one they do.
    opened.primary.execute("UPDATE bots SET type = 'Bots::Signal', settings = '{}' WHERE id = ?1", [second]).unwrap();
    assert_eq!(browser.get(&app, &format!("/bots/{second}")).await.status, 501);
    // A deleted bot does not count, and its page is the list's "not found".
    opened.primary.execute("UPDATE bots SET status = 3", []).unwrap();
    assert_eq!(browser.get(&app, "/bots").await.status, 200);
    let gone = browser.get(&app, &format!("/bots/{first}")).await;
    assert_eq!((gone.status, gone.header("location")), (302, Some("/bots")));
    assert!(browser.get(&app, "/bots").await.body.contains("Such a bot doesn&#39;t exist."), "the alert of Bots::Botable#set_bot");

    // Deferred writes still answer 501 with a valid token, and no row moves.
    opened.primary.execute("UPDATE bots SET status = 1", []).unwrap();
    let before: Vec<(i64, String)> = opened.primary.prepare("SELECT status, settings FROM bots ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    for (method, path) in [("delete", format!("/bots/{first}")), ("post", "/bots/dca_multi_assets".to_string()),
                           ("post", "/api_keys".to_string()), ("post", format!("/bots/{first}/liquidate")),
                           ("post", format!("/bots/{first}/rebalance")), ("post", format!("/bots/{first}/merge")), ("post", format!("/bots/{first}/split")), ("post", format!("/bots/{first}/reverse")), ("patch", "/bots/reorder".to_string()),
                           ("post", format!("/bots/{first}/export")), ("delete", format!("/bots/{first}/transactions/1")), ("post", "/en/broadcasts/fetch_open_orders".to_string())] {
        let answer = browser.send(&app, "POST", &path, Some(&[("_method", method)]), web::Csrf::Header, &[]).await;
        assert!(answer.status == 501 && answer.body.contains(&format!("{} {path}", method.to_uppercase())), "{method} {path}: {} {}", answer.status, answer.body);
        let refused = browser.send(&app, "POST", &path, Some(&[("_method", method)]), web::Csrf::None, &[]).await;
        assert_eq!(refused.status, 302, "{method} {path} without a token is turned back before any route");
    }
    let after: Vec<(i64, String)> = opened.primary.prepare("SELECT status, settings FROM bots ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(before, after);
}

/// One request on a connection of its own, all of it within `deadline`: the connect, every write
/// and the whole answer, each given only the time that is left. `None` when that did not happen in
/// time, or nothing listens.
fn exchange(port: u16, request: &str, deadline: Instant) -> Option<String> {
    let left = || deadline.checked_duration_since(Instant::now()).filter(|left| !left.is_zero());
    let mut stream = std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), left()?).ok()?;
    let mut written = 0;
    while written < request.len() {
        stream.set_write_timeout(Some(left()?)).ok()?;
        written += stream.write(&request.as_bytes()[written..]).ok().filter(|n| *n > 0)?;
    }
    let (mut answer, mut buffer) = (Vec::new(), [0u8; 8192]);
    loop {
        stream.set_read_timeout(Some(left()?)).ok()?;
        match stream.read(&mut buffer).ok()? {
            0 => return Some(String::from_utf8_lossy(&answer).into_owned()),
            n => answer.extend_from_slice(&buffer[..n]),
        }
    }
}

/// One request to the running executable, answered within 20 s: the status, the session cookie it
/// set if it set one, and the body.
fn http(port: u16, method: &str, path: &str, cookie: Option<&str>, form: Option<&str>) -> (u16, Option<String>, String) {
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    if let Some(cookie) = cookie { request.push_str(&format!("Cookie: {cookie}\r\n")); }
    if let Some(form) = form { request.push_str(&format!("Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n", form.len())); }
    request.push_str(&format!("\r\n{}", form.unwrap_or("")));
    let answer = exchange(port, &request, Instant::now() + Duration::from_secs(20)).unwrap_or_else(|| panic!("{method} {path}: no whole answer within 20 s"));
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    let status = head.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or(0);
    let set = head.lines().find_map(|line| line.to_ascii_lowercase().starts_with("set-cookie:").then(|| line[11..].trim().split(';').next().unwrap_or("").to_string()));
    (status, set, body.to_string())
}

/// Whether the executable answers `/up` within `patience`, and never waits longer.
fn up_within(port: u16, patience: Duration) -> bool {
    exchange(port, "GET /up HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n", Instant::now() + patience).is_some_and(|answer| answer.starts_with("HTTP/1.1 200"))
}

/// A child process that is killed and reaped however the test ends: a failed assertion must not leave a server behind.
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Child {
    /// How it ended, if it ends within `patience`. Never waits longer.
    fn ended_within(&mut self, patience: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + patience;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() { return Some(status); }
            if Instant::now() >= deadline { return None; }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// `deltabadger serve` on `dir`, with everything it prints in `log`, and the port it answers on. A
/// port found free can be taken before the child binds it: a child that ends over that is started
/// again on another, a few times. It is given the secret the fixture's venue key was written with,
/// because the engine reads that key before it claims the install.
fn serving(dir: &std::path::Path, log: &std::path::Path) -> (Child, u16) {
    for _ in 0..5 {
        let port = free_port();
        let out = std::fs::File::create(log).unwrap();
        let mut child = Child(serve_command(dir, port).env("SECRET_KEY_BASE", "engine-test-secret").stdout(out.try_clone().unwrap()).stderr(out).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                let said = std::fs::read_to_string(log).unwrap_or_default();
                assert!(said.contains("in use"), "serve ended ({status}) before it answered: {said}");
                break;
            }
            // The probe gets what is left of the 30 s and at most two: the loop, not the probe, keeps the deadline.
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "serve did not answer within 30 s: {}", std::fs::read_to_string(log).unwrap_or_default());
            if up_within(port, left.min(Duration::from_secs(2))) { return (child, port); }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    panic!("five ports in a row were taken between finding them free and binding them");
}

/// What the pages are worth through the executable, which is what a person runs. `deltabadger
/// serve` takes the install over and runs the engine beside the web server, so it serves the pages
/// of an install the engine accepts and of no other. Here: a working one-asset crypto bot on
/// Alpaca with a paper key (not due, so the engine calls no venue) and a stopped basket of two,
/// signed in to over HTTP: their list, both pages and a chart, while a second engine is turned away.
/// Then the engine is seen to make a pass beside the pages: the basket of two is set to work behind
/// its back, which is the owner's instance in small, and within its idle minute the engine reads
/// the install again, finds a bot it cannot trade, and ends the process, the pages with it. And
/// started on that install, the executable does not come up at all. Every wait has a deadline,
/// and both children are killed and reaped however the test ends.
#[test]
fn the_executable_serves_the_bot_pages_of_an_install_the_engine_accepts_and_refuses_the_owners() {
    use serde_json::json;
    let (dir, opened, seeded) = common::install_alpaca();
    let c = &opened.primary;
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    c.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00', wash_sale_enabled = 0 WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('ethereum', 'ETH', 'Ethereum', 'Cryptocurrency', ?1, ?1)", ["2026-01-01 00:00:00"]).unwrap();
    let eth = c.last_insert_rowid();
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, minimum_base_size, \
               minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, 'ETH/USD', 'ETH', 'USD', ?2, ?3, 9, 2, 2, '0.001', '1', 1, 1, ?4, ?4)",
              (seeded.exchange_id, eth, seeded.quote, "2026-01-01 00:00:00")).unwrap();
    let eth_ticker = c.last_insert_rowid();
    // Scheduled, and first due in 2099: the engine has it and has nothing to do for it.
    let working = common::seed::insert_bot(c, &seeded, &common::seed::BotSpec::weekly(60.0, "2099-01-01 00:00:00"));
    let mut two = common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("allocations", json!({ seeded.btc.to_string(): 0.5, eth.to_string(): 0.5 }));
    two.status = 2;
    let basket = common::seed::insert_bot(c, &seeded, &two);
    for (asset, ticker) in [(seeded.btc, seeded.ticker_id), (eth, eth_ticker)] {
        c.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, created_at, updated_at) VALUES (?1, ?2, ?3, 0.5, 1, ?4, ?4)",
                  (basket, asset, ticker, "2026-01-01 00:00:00")).unwrap();
    }
    c.execute("UPDATE bots SET label = 'Bot ' || id", []).unwrap();
    drop(opened);
    let logs = tempfile::tempdir().unwrap();
    let said = |name: &str| std::fs::read_to_string(logs.path().join(name)).unwrap_or_default();

    let (mut server, port) = serving(dir.path(), &logs.path().join("serve.log"));
    let (status, cookie, login) = http(port, "GET", "/login", None, None);
    assert_eq!(status, 200, "{login}");
    let token = web::form_token(&login, "/login").expect("the sign-in form carries a token");
    let form = form_urlencoded::Serializer::new(String::new()).append_pair("authenticity_token", &token).append_pair("user[email]", "o@example.com")
        .append_pair("user[password]", "Correct-horse-9").finish();
    let (status, signed_in, _) = http(port, "POST", "/login", cookie.as_deref(), Some(&form));
    assert_eq!(status, 303, "the sign-in is accepted");
    let cookie = signed_in.or(cookie);
    let get = |path: &str| http(port, "GET", path, cookie.as_deref(), None);
    let list = get("/bots");
    assert_eq!(list.0, 200, "{}", list.2);
    for bot in [working, basket] { assert!(list.2.contains(&format!("id=\"tile_bots_dca_multi_asset_{bot}\"")), "the list has a tile for bot {bot}"); }
    let (working_page, basket_page, chart) = (get(&format!("/bots/{working}")), get(&format!("/bots/{basket}")), get(&format!("/bots/{basket}/chart")));
    assert_eq!((working_page.0, basket_page.0, chart.0), (200, 200, 200), "{}", working_page.2);
    assert!(working_page.2.contains(&format!("Bot {working}")) && working_page.2.contains("bot-locked"), "a working bot's page: its label, its rules locked");
    assert!(basket_page.2.contains(&format!("Bot {basket}")) && basket_page.2.matches("class=\"allocation__input\"").count() == 2 && basket_page.2.contains("value=\"50.0\""), "a basket's page: its two weights");
    // The install is held: a second engine is turned away while the pages are served.
    let second = std::fs::File::create(logs.path().join("check.log")).unwrap();
    let mut check = Child(Command::new(env!("CARGO_BIN_EXE_deltabadger")).arg("check").env("STORAGE_DIR", dir.path()).env("SECRET_KEY_BASE", "engine-test-secret")
        .stdin(Stdio::null()).stdout(second.try_clone().unwrap()).stderr(second).spawn().unwrap());
    let refused = check.ended_within(Duration::from_secs(20)).expect("check ends within 20 s");
    assert!(!refused.success() && said("check.log").contains("another Deltabadger engine"), "{}", said("check.log"));

    // The engine makes its passes beside the pages. Set to work behind its back with market-cap weights, the basket of two
    // is a bot it cannot trade: its next pass (it idles for a minute at most) reads the install again, names the bot, and
    // ends the process.
    let behind = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    behind.execute("UPDATE bots SET status = 1, settings = json_set(settings, '$.weighting', 'market_cap') WHERE id = ?1", [basket]).unwrap();
    drop(behind);
    let ended = server.ended_within(Duration::from_secs(100)).unwrap_or_else(|| panic!("no engine pass within 100 s: {}", said("serve.log")));
    assert!(!ended.success(), "the engine's pass ends the process: {ended}");
    assert!(said("serve.log").contains(&format!("bot {basket} (scheduled)")) && said("serve.log").contains("weighting market_cap"), "{}", said("serve.log"));
    assert!(!up_within(port, Duration::from_secs(2)), "and the pages went with it");

    // Started on that install, the executable does not come up: refused before anything is served.
    let out = std::fs::File::create(logs.path().join("again.log")).unwrap();
    let mut again = Child(serve_command(dir.path(), free_port()).env("SECRET_KEY_BASE", "engine-test-secret").stdout(out.try_clone().unwrap()).stderr(out).spawn().unwrap());
    let refused = again.ended_within(Duration::from_secs(30)).unwrap_or_else(|| panic!("serve neither refused nor ended within 30 s: {}", said("again.log")));
    assert_eq!(refused.code(), Some(1), "{}", said("again.log"));
    assert!(said("again.log").contains(&format!("bot {basket} (scheduled)")) && said("again.log").contains("weighting market_cap"), "{}", said("again.log"));
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

/// The same proxy in front of an install that has no APP_ROOT_URL. The origin is then the request's
/// own, read as Rails reads it: the scheme and the host the proxy says it was asked for.
#[tokio::test(flavor = "current_thread")]
async fn a_form_is_accepted_behind_a_tls_terminating_proxy_without_app_root_url() {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let form: [(&str, &str); 2] = [("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")];
    let submit = |headers: &'static [(&'static str, &'static str)]| {
        let app = app.clone();
        async move {
            let mut browser = Browser::default();
            browser.get(&app, "/login").await;
            browser.send(&app, "POST", "/login", Some(&form), web::Csrf::Form, headers).await.status
        }
    };
    // The request's Host is localhost:3000 (web::HOST), and it arrives over plain http.
    assert_eq!(submit(&[("origin", "https://localhost:3000"), ("x-forwarded-proto", "https")]).await, 303, "the proxy terminated TLS and says so");
    assert_eq!(submit(&[("origin", "https://bot.example"), ("x-forwarded-proto", "https"), ("x-forwarded-host", "bot.example")]).await, 303, "and names the host it was asked for");
    assert_eq!(submit(&[("origin", "http://localhost:3000")]).await, 303, "no proxy: plain http");
    assert_eq!(submit(&[("origin", "https://localhost:3000")]).await, 302, "nothing says the page was served over https");
    assert_eq!(submit(&[("origin", "http://localhost:3000"), ("x-forwarded-proto", "https")]).await, 302, "the scheme is part of an origin");
    assert_eq!(submit(&[("origin", "https://evil.example"), ("x-forwarded-proto", "https")]).await, 302);
    assert_eq!(submit(&[("origin", "https://localhost:3000"), ("x-forwarded-proto", "https"), ("x-forwarded-host", "bot.example")]).await, 302, "not the host the proxy named");
}

/// One secret, two keys: the session cookie's and the stream names'. Each is HMAC-SHA256 of
/// secret_key_base over its own label, so a value made with one is nothing to the other, and the
/// labels are fixed: changing one signs every browser out or breaks every open page's streams.
#[tokio::test(flavor = "current_thread")]
async fn the_session_key_and_the_stream_key_are_derived_under_their_own_labels() {
    use hmac::Mac;
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let derived = |label: &str| -> [u8; 32] {
        let mut mac = <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(web::SECRET.as_bytes()).unwrap();
        mac.update(label.as_bytes());
        mac.finalize().into_bytes().into()
    };
    assert_ne!(app.keys.session, app.keys.streams, "the two keys differ");
    assert_eq!(app.keys.session, derived("deltabadger rust session v1"));
    assert_eq!(app.keys.streams, derived("deltabadger rust turbo streams v1"));
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

/// A hook for `App::with_password_hook`: counts the bcrypt computations and, while `hold` is set,
/// keeps each one where it is until the test lets it go.
struct Gate {
    calls: std::sync::atomic::AtomicUsize,
    hold: std::sync::atomic::AtomicBool,
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

/// The test's ends of a `Gate`. Dropping `release` lets every held computation go, so a failed
/// assertion does not leave a thread waiting.
struct Held {
    gate: std::sync::Arc<Gate>,
    entered: tokio::sync::mpsc::UnboundedReceiver<()>,
    release: std::sync::mpsc::Sender<()>,
}

impl Held {
    fn calls(&self) -> usize { self.gate.calls.load(std::sync::atomic::Ordering::SeqCst) }
    fn hold(&self, on: bool) { self.gate.hold.store(on, std::sync::atomic::Ordering::SeqCst) }
    /// Waits until one more bcrypt computation has started and is being held.
    async fn in_flight(&mut self) {
        tokio::time::timeout(Duration::from_secs(10), self.entered.recv()).await.expect("a bcrypt computation starts").unwrap();
    }
}

/// An install whose owner signs in with "Correct-horse-9", behind a declared proxy (so each test
/// browser can have its own address for the rate limit), with a `Gate` on its bcrypt computations.
fn gated() -> (tempfile::TempDir, rusqlite::Connection, i64, deltabadger::web::App, Held) {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let env = |name: &str| match name {
        "SECRET_KEY_BASE" => Some(web::SECRET.to_string()),
        "BEHIND_PROXY" => Some("1".to_string()),
        _ => None,
    };
    let own = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap().primary;
    let (entered_tx, entered) = tokio::sync::mpsc::unbounded_channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let gate = std::sync::Arc::new(Gate { calls: 0.into(), hold: false.into(), entered: entered_tx, release: std::sync::Mutex::new(release_rx) });
    let hook = { let gate = gate.clone(); move || {
        gate.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if gate.hold.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = gate.entered.send(());
            let _ = gate.release.lock().unwrap().recv();
        }
    } };
    let app = deltabadger::web::App::new(deltabadger::web::Config::from_env(&env).unwrap(), &env, own, TestClock::at(NOW)).unwrap()
        .with_password_hook(std::sync::Arc::new(hook)).unwrap();
    (dir, opened.primary, seeded.user_id, app, Held { gate, entered, release })
}

/// One sign-in attempt by a browser of its own at 198.51.100.`n`: the login page, then the form. Its status.
fn attempt(app: &deltabadger::web::App, n: u8, email: &'static str, password: &'static str) -> tokio::task::JoinHandle<u16> {
    let app = app.clone();
    tokio::spawn(async move {
        let (mut browser, from) = (Browser::default(), format!("198.51.100.{n}"));
        browser.send(&app, "GET", "/login", None, web::Csrf::None, &[("x-forwarded-for", &from)]).await;
        browser.send(&app, "POST", "/login", Some(&[("user[email]", email), ("user[password]", password)]), web::Csrf::Form, &[("x-forwarded-for", &from)]).await.status
    })
}

fn failed_attempts(db: &rusqlite::Connection, user: i64) -> (i64, Option<String>, String) {
    db.query_row("SELECT failed_attempts, locked_at, updated_at FROM users WHERE id = ?1", [user], |r| Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), r.get(1)?, r.get(2)?))).unwrap()
}

/// bcrypt takes a good part of a second, and the web side has one database connection behind a lock.
/// The computation runs outside that lock: a sign-in in progress holds up no other request.
#[tokio::test(flavor = "current_thread")]
async fn a_sign_in_being_checked_does_not_hold_up_other_requests() {
    let (_dir, _db, _user, app, mut held) = gated();
    let mut signed_in = Browser::default();
    signed_in.get(&app, "/login").await;
    assert_eq!(signed_in.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    held.hold(true);
    let checking = attempt(&app, 1, "o@example.com", "wrong");
    held.in_flight().await;
    let page = tokio::time::timeout(Duration::from_secs(2), Browser::default().get(&app, "/login")).await.expect("the login page waited for another request's bcrypt");
    assert_eq!(page.status, 200);
    let bots = tokio::time::timeout(Duration::from_secs(2), signed_in.get(&app, "/bots")).await.expect("a signed-in page waited for another request's bcrypt");
    assert_eq!(bots.status, 200);
    held.release.send(()).unwrap();
    assert_eq!(checking.await.unwrap(), 422);
}

/// Two computations at a time, eight attempts waiting for their turn, and the one after that is
/// refused at once: a 503 that spent no attempt and wrote nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_sign_in_beyond_the_waiting_room_is_refused_and_counts_nothing() {
    let (_dir, db, user, app, mut held) = gated();
    held.hold(true);
    let mut attempts: Vec<_> = (1..=2).map(|n| attempt(&app, n, "nobody@example.com", "wrong")).collect();
    held.in_flight().await;
    held.in_flight().await;
    attempts.extend((3..=10).map(|n| attempt(&app, n, "nobody@example.com", "wrong")));
    let started = Instant::now();
    while app.password_checks_waiting() < 8 {
        assert!(started.elapsed() < Duration::from_secs(10), "{} attempts are waiting", app.password_checks_waiting());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let before = failed_attempts(&db, user);
    let (mut ninth, from) = (Browser::default(), [("x-forwarded-for", "198.51.100.11")]);
    ninth.send(&app, "GET", "/login", None, web::Csrf::None, &from).await;
    let refused = tokio::time::timeout(Duration::from_secs(2), ninth.send(&app, "POST", "/login", Some(&[("user[email]", "o@example.com"), ("user[password]", "wrong")]), web::Csrf::Form, &from))
        .await.expect("the ninth to wait is answered at once");
    assert_eq!((refused.status, refused.header("retry-after"), refused.header("x-frame-options"), refused.header("cache-control")), (503, Some("1"), None, Some("no-cache")),
               "below the controllers, as the 429");
    assert_eq!((held.calls(), app.password_checks_waiting(), failed_attempts(&db, user)), (2, 8, before.clone()), "no bcrypt, no place taken, no attempt spent");
    for _ in 0..10 { held.release.send(()).unwrap(); }
    for attempt in attempts { assert_eq!(attempt.await.unwrap(), 422); }
    assert_eq!((held.calls(), app.password_checks_waiting(), failed_attempts(&db, user)), (10, 0, before));
}

/// The computation runs outside the lock, so two attempts on one account overlap. What each then
/// does is decided on the row as it is after its own computation: both count, and the fifth locks.
#[tokio::test(flavor = "current_thread")]
async fn overlapping_wrong_passwords_for_one_account_all_count_and_the_fifth_locks() {
    let (_dir, db, user, app, mut held) = gated();
    held.hold(true);
    for (already, locked) in [(0, false), (3, true)] {
        db.execute("UPDATE users SET failed_attempts = ?1 WHERE id = ?2", (already, user)).unwrap();
        let pair = [attempt(&app, 1 + already as u8, "o@example.com", "wrong"), attempt(&app, 2 + already as u8, "o@example.com", "wrong")];
        held.in_flight().await;
        held.in_flight().await; // both have read the row by now
        held.release.send(()).unwrap();
        held.release.send(()).unwrap();
        for attempt in pair { assert_eq!(attempt.await.unwrap(), 422); }
        let (count, locked_at, _) = failed_attempts(&db, user);
        assert_eq!((count, locked_at.is_some()), (already + 2, locked), "after {already} earlier failures");
    }
    // The password changed while an attempt with the old one was being checked: refused, and not counted.
    db.execute("UPDATE users SET failed_attempts = 0, locked_at = NULL WHERE id = ?1", [user]).unwrap();
    let stale = attempt(&app, 9, "o@example.com", "Correct-horse-9");
    held.in_flight().await;
    db.execute("UPDATE users SET encrypted_password = ?1 WHERE id = ?2", (deltabadger::crypto::hash_password("Another-horse-7").unwrap(), user)).unwrap();
    held.release.send(()).unwrap();
    assert_eq!((stale.await.unwrap(), failed_attempts(&db, user).0), (422, 0));
}

/// Exactly one bcrypt computation for every sign-in attempt that reaches the controller, whatever
/// the account is, so that how long the answer takes says nothing about it.
#[tokio::test(flavor = "current_thread")]
async fn every_sign_in_attempt_costs_exactly_one_bcrypt() {
    let (_dir, db, user, app, held) = gated();
    let mut n = 0;
    let mut costs = async |email: &'static str, password: &'static str, status: u16, what: &str| {
        n += 1;
        let before = held.calls();
        assert_eq!(attempt(&app, n, email, password).await.unwrap(), status, "{what}");
        assert_eq!(held.calls() - before, 1, "{what}");
    };
    costs("o@example.com", "wrong", 422, "a known email, a wrong password").await;
    costs("o@example.com", "Correct-horse-9", 303, "a known email, the right password").await;
    costs("nobody@example.com", "wrong", 422, "an unknown email").await;
    costs("o@example.com", "", 422, "a known email, no password").await;
    costs("", "", 422, "nothing at all").await;
    db.execute("UPDATE users SET failed_attempts = 5, locked_at = '2026-09-10 12:00:00' WHERE id = ?1", [user]).unwrap();
    costs("o@example.com", "Correct-horse-9", 422, "a locked account, the right password").await;
    costs("o@example.com", "wrong", 422, "a locked account, a wrong password").await;
    db.execute("UPDATE users SET failed_attempts = 0, locked_at = NULL, otp_module = 1, otp_secret_key = 'JBSWY3DPEHPK3PXP' WHERE id = ?1", [user]).unwrap();
    costs("o@example.com", "Correct-horse-9", 302, "two-factor, the right password").await;
    costs("o@example.com", "wrong", 422, "two-factor, a wrong password").await;
    // And none for a request that is refused before the controller.
    let before = held.calls();
    let no_token = Browser::default().send(&app, "POST", "/login", Some(&[("user[email]", "o@example.com"), ("user[password]", "wrong")]), web::Csrf::None, &[("x-forwarded-for", "198.51.100.99")]).await;
    assert_eq!((no_token.status, held.calls() - before), (302, 0), "no CSRF token: no bcrypt");
}

/// The app on a local port under `limits`, served by the same loop `deltabadger serve` runs.
async fn served_under(limits: deltabadger::web::server::Limits) -> (tempfile::TempDir, deltabadger::web::App, std::net::SocketAddr) {
    served_with(limits, &[]).await
}

/// As `served_under`, with more of the environment than the secret (`BEHIND_PROXY`, …).
async fn served_with(limits: deltabadger::web::server::Limits, env: &'static [(&'static str, &'static str)]) -> (tempfile::TempDir, deltabadger::web::App, std::net::SocketAddr) {
    let (dir, opened, _) = common::install();
    drop(opened);
    let env = move |name: &str| env.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string()).or((name == "SECRET_KEY_BASE").then(|| web::SECRET.to_string()));
    let own = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap().primary;
    let app = deltabadger::web::App::new(deltabadger::web::Config::from_env(&env).unwrap(), &env, own, TestClock::at(NOW)).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(deltabadger::web::server::serve_on(listener, app.clone(), limits));
    (dir, app, address)
}

/// A client that opens a connection and never finishes its request's head holds a connection for
/// as long as it likes, unless the server stops waiting: ten seconds outside tests.
#[tokio::test(flavor = "current_thread")]
async fn a_connection_that_does_not_finish_its_request_head_is_closed() {
    use deltabadger::web::server::Limits;
    assert_eq!((Limits::default().header_read_timeout, Limits::default().body_read_timeout, Limits::default().max_connections), (Duration::from_secs(10), Duration::from_secs(10), 1024));
    let (_dir, _app, address) = served_under(Limits { header_read_timeout: Duration::from_millis(300), ..Limits::default() }).await;
    let started = Instant::now();
    let half = web::until_closed(address, b"GET /up HT", Duration::from_secs(5)).await;
    assert!(half.is_ok(), "half a request line, and the connection is still open after 5 s: {half:?}");
    assert!(started.elapsed() >= Duration::from_millis(250), "closed by the timeout, not at once: {:?}", started.elapsed());
    let headers = web::until_closed(address, b"GET /up HTTP/1.1\r\nHost: localhost\r\nX-Slow: ", Duration::from_secs(5)).await;
    assert!(headers.is_ok(), "headers that never end: {headers:?}");
    let whole = web::until_closed(address, b"GET /up HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n", Duration::from_secs(5)).await.unwrap();
    assert!(whole.starts_with("HTTP/1.1 200") && whole.contains("background-color: green"), "{whole}");
}

/// A client that sends a request's head, declares a body and then stops must not keep its
/// connection: 1,024 of those would be every place the server has, before any rate limit counts.
/// A form's body has ten seconds in all (a 408, and the connection is closed); a body no handler
/// reads is not waited for at all: the answer goes out and the connection is closed.
#[tokio::test(flavor = "current_thread")]
async fn a_request_body_that_never_arrives_does_not_keep_its_connection() {
    use deltabadger::web::server::Limits;
    // One place: each request below is served only once the one before it has given its place back.
    let limits = Limits { body_read_timeout: Duration::from_millis(300), max_connections: 1, ..Limits::default() };
    let (_dir, _app, address) = served_under(limits).await;
    let patience = Duration::from_secs(5); // well under the header timeout, which is not what closes these
    for stalled in [
        &b"POST /login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 100\r\n\r\na=1"[..],
        b"POST /login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nTransfer-Encoding: chunked\r\n\r\n3\r\na=1\r\n",
        b"POST /csp-report HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 100\r\n\r\n",
        b"PUT /login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 500\r\n\r\n",
    ] {
        let started = Instant::now();
        let answer = web::until_closed(address, stalled, patience).await;
        let answer = answer.unwrap_or_else(|e| panic!("a form whose body stalls, and the connection is still open after 5 s: {e}"));
        assert!(answer.starts_with("HTTP/1.1 408") && answer.to_lowercase().contains("connection: close"), "{answer}");
        assert!(started.elapsed() >= Duration::from_millis(250), "at the deadline, not before: {:?}", started.elapsed());
    }
    for (unread, status) in [
        (&b"POST /csp-report HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/csp-report\r\nContent-Length: 100000\r\n\r\n{"[..], "204"),
        (b"POST /nothing-here HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\nContent-Length: 500\r\n\r\n", "302"), // no CSRF token: refused before any route
        (b"GET /up HTTP/1.1\r\nHost: localhost\r\nContent-Length: 500\r\n\r\n", "200"),
        (b"POST /cable HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n", "501"),
    ] {
        let answer = web::until_closed(address, unread, patience).await;
        let answer = answer.unwrap_or_else(|e| panic!("a body nobody reads kept its connection open: {e}\n{}", String::from_utf8_lossy(unread)));
        assert!(answer.starts_with(&format!("HTTP/1.1 {status}")), "{answer}");
    }
    let whole = web::until_closed(address, b"POST /login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 3\r\nConnection: close\r\n\r\na=1", patience).await.unwrap();
    assert!(whole.starts_with("HTTP/1.1 302"), "a form that arrives is read as before: {whole}");
}

/// At most `max_connections` connections are open at once (1,024 outside tests). One more waits to
/// be accepted until a place is free; it is not served on the side.
#[tokio::test(flavor = "current_thread")]
async fn connections_beyond_the_cap_wait_for_a_place() {
    use deltabadger::web::server::Limits;
    let (_dir, _app, address) = served_under(Limits { header_read_timeout: Duration::from_secs(60), max_connections: 2, ..Limits::default() }).await;
    let idle: Vec<std::net::TcpStream> = (0..2).map(|_| std::net::TcpStream::connect(address).unwrap()).collect();
    tokio::time::sleep(Duration::from_millis(200)).await; // both are accepted and hold their places
    let request = b"GET /up HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let waiting = web::until_closed(address, request, Duration::from_millis(500)).await;
    assert!(waiting.is_err(), "a third connection was served while two were open: {waiting:?}");
    drop(idle);
    let served = web::until_closed(address, request, Duration::from_secs(5)).await.unwrap();
    assert!(served.starts_with("HTTP/1.1 200"), "{served}");
}

/// The script every page loads: over a megabyte, the largest answer this server gives.
fn large_asset() -> &'static deltabadger::web::assets::Embedded {
    let script = deltabadger::web::assets::find(deltabadger::web::assets::path("application.js")).expect("the script is embedded");
    assert!(script.body.len() > 1_000_000, "the script is {} bytes: no longer a large answer", script.body.len());
    script
}

/// A client that asks for a large answer and then reads nothing must not keep its connection:
/// 1,024 of those would be every place the server has. A write of which the client takes nothing
/// for `write_stall_timeout` (thirty seconds outside tests) ends the connection, and its place is free.
#[tokio::test(flavor = "current_thread")]
async fn a_client_that_stops_reading_its_answer_loses_its_connection() {
    use deltabadger::web::server::Limits;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    assert_eq!(Limits::default().write_stall_timeout, Duration::from_secs(30));
    let script = large_asset();
    let stall = Duration::from_millis(500);
    // One place: /up below is served only once the stalled connection has given its place back.
    let (_dir, _app, address) = served_under(Limits { write_stall_timeout: stall, max_connections: 1, ..Limits::default() }).await;
    let started = Instant::now();
    // Sixty-four answers of a megabyte each are far more than the buffers between the two ends hold.
    let asked = 64;
    let mut stuck = tokio::net::TcpStream::connect(address).await.unwrap();
    stuck.write_all(format!("GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n", script.url).repeat(asked).as_bytes()).await.unwrap();
    let up = b"GET /up HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let waiting = web::until_closed(address, up, stall / 2).await;
    assert!(waiting.is_err(), "a second connection was served while the stalled one held the only place: {waiting:?}");
    let served = web::until_closed(address, up, Duration::from_secs(10)).await;
    let served = served.unwrap_or_else(|e| panic!("a client that reads nothing still holds the only place after 10 s: {e}"));
    assert!(served.starts_with("HTTP/1.1 200"), "{served}");
    assert!(started.elapsed() >= stall, "freed by the timeout, not before: {:?}", started.elapsed());
    // What the kernel had already taken still arrives; then the stream ends, far short of what was asked for.
    let mut received = 0;
    let mut buffer = vec![0; 1 << 16];
    let drained = async {
        loop {
            match stuck.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => received += n,
            }
        }
    };
    assert!(tokio::time::timeout(Duration::from_secs(10), drained).await.is_ok(), "the stalled connection was not closed");
    assert!(received < asked * script.body.len(), "all {asked} answers arrived: the connection was never cut");
}

/// The timeout is for a write that makes no progress, not for a slow one. A client that takes
/// four timeouts to read sixteen megabytes, a little at a time, gets every byte: in each timeout
/// it drains many times what the buffers between the two ends hold.
#[tokio::test(flavor = "current_thread")]
async fn a_slow_reader_that_keeps_reading_is_not_cut() {
    use deltabadger::web::server::Limits;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let script = large_asset();
    let stall = Duration::from_secs(1);
    let (_dir, _app, address) = served_under(Limits { write_stall_timeout: stall, ..Limits::default() }).await;
    // More than the buffers between the two ends hold, so the server's writes do wait for this reader.
    let asked = 16;
    let bodies = asked * script.body.len();
    let mut slow = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = |last: bool| format!("GET {} HTTP/1.1\r\nHost: localhost\r\n{}\r\n", script.url, if last { "Connection: close\r\n" } else { "" });
    slow.write_all((request(false).repeat(asked - 1) + &request(true)).as_bytes()).await.unwrap();
    let (started, pace) = (Instant::now(), stall * 4);
    let mut received = 0;
    let mut buffer = vec![0; 1 << 16];
    loop {
        // No further ahead than a reader that takes `pace` for everything.
        let allowed = (bodies as f64 * started.elapsed().as_secs_f64() / pace.as_secs_f64()) as usize;
        if received < bodies && received > allowed {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        }
        match slow.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => received += n,
            Err(e) => panic!("cut after {received} of {bodies} bytes and {:?}: {e}", started.elapsed()),
        }
    }
    assert!(received > bodies, "{received} of {bodies} bytes arrived before the server closed the connection");
    assert!(started.elapsed() >= stall * 3, "the reader was not slow: {:?}", started.elapsed());
}

/// What the clock measures is the server's own write, not the reader: the kernel accepts more of a
/// waiting write only when the peer has acknowledged enough to free a share of the send buffer. A
/// client with a small receive window that never stops reading, a byte every tenth of a second,
/// frees nothing of that size within the timeout, and is closed like one that stopped.
#[tokio::test(flavor = "current_thread")]
async fn a_reader_that_takes_a_byte_now_and_then_is_cut_like_one_that_stopped() {
    use deltabadger::web::server::Limits;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let script = large_asset();
    let stall = Duration::from_millis(1500);
    let (_dir, _app, address) = served_under(Limits { write_stall_timeout: stall, ..Limits::default() }).await;
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let mut trickle = socket.connect(address).await.unwrap();
    let asked = 64;
    trickle.write_all(format!("GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n", script.url).repeat(asked).as_bytes()).await.unwrap();
    let started = Instant::now();
    // One byte at a time, ten a second: this client is reading the whole time.
    let (mut received, mut buffer) = (0usize, vec![0u8; 1 << 16]);
    let mut slowly = true;
    let ended = loop {
        let take = if slowly { 1 } else { buffer.len() };
        match tokio::time::timeout(Duration::from_secs(20), trickle.read(&mut buffer[..take])).await {
            Err(_) => break None,
            Ok(Ok(0)) | Ok(Err(_)) => break Some(started.elapsed()),
            Ok(Ok(n)) => received += n,
        }
        // Slowly until the server has had its timeout twice over; then whatever is left in the buffers, at once.
        slowly = slowly && started.elapsed() < stall * 2;
        if slowly { tokio::time::sleep(Duration::from_millis(100)).await; }
    };
    let ended = ended.unwrap_or_else(|| panic!("still open after {received} bytes: a byte now and then kept the connection"));
    assert!(received < asked * script.body.len(), "all {asked} answers arrived: the connection was never cut");
    assert!(ended >= stall, "closed after {ended:?}, before the timeout");
}

/// A slow reader on a real line: twenty kilobytes a second, with the server's own thirty seconds.
/// That is six hundred kilobytes in every timeout, several times what the kernel wants freed
/// before it accepts more, so the write keeps making progress and the connection stays. Run for
/// longer than the timeout, with far more asked for than the buffers hold.
#[tokio::test(flavor = "current_thread")]
async fn a_reader_at_twenty_kilobytes_a_second_is_not_cut() {
    use deltabadger::web::server::Limits;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let script = large_asset();
    let limits = Limits::default();
    let (_dir, _app, address) = served_under(limits).await;
    let mut slow = tokio::net::TcpStream::connect(address).await.unwrap();
    slow.write_all(format!("GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n", script.url).repeat(64).as_bytes()).await.unwrap();
    let (started, rate) = (Instant::now(), 20_000.0);
    let run = limits.write_stall_timeout + Duration::from_secs(5);
    let mut received = 0usize;
    let mut buffer = vec![0; 2_000];
    while started.elapsed() < run {
        if received as f64 > rate * started.elapsed().as_secs_f64() {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }
        match slow.read(&mut buffer).await {
            Ok(0) => panic!("closed after {received} bytes and {:?}", started.elapsed()),
            Ok(n) => received += n,
            Err(e) => panic!("cut after {received} bytes and {:?}: {e}", started.elapsed()),
        }
    }
    assert!(received as f64 >= rate * limits.write_stall_timeout.as_secs_f64(), "{received} bytes in {:?}: the reader was not held to its rate", started.elapsed());
    assert!(received < 2 * script.body.len(), "{received} bytes: the reader was not slow");
    // Past the timeout and still served: the next read brings more.
    assert!(matches!(tokio::time::timeout(Duration::from_secs(5), slow.read(&mut buffer)).await, Ok(Ok(n)) if n > 0), "nothing more arrived after {:?}", started.elapsed());
}

/// An install with two confirmed accounts that share the password "Correct-horse-9": the owner
/// (o@example.com), with two-factor on when `owner_two_factor`, and second@example.com without.
/// Returns the ids of both.
fn two_accounts(owner_two_factor: bool) -> (tempfile::TempDir, rusqlite::Connection, deltabadger::web::App, i64, i64) {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE id = ?2", (&hash, seeded.user_id)).unwrap();
    if owner_two_factor {
        opened.primary.execute("UPDATE users SET otp_module = 1, otp_secret_key = ?1 WHERE id = ?2", (OTP_SEED, seeded.user_id)).unwrap();
    }
    opened.primary.execute("INSERT INTO users (email, encrypted_password, name, admin, confirmed_at, created_at, updated_at) \
                            VALUES ('second@example.com', ?1, 'Second', 0, ?2, ?2, ?2)", (&hash, "2026-01-01 00:00:00")).unwrap();
    let second = opened.primary.last_insert_rowid();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    (dir, opened.primary, app, seeded.user_id, second)
}

const OTP_SEED: &str = "JBSWY3DPEHPK3PXP";

/// The session a browser holds, opened with the app's own key.
fn session_of(app: &deltabadger::web::App, browser: &Browser) -> deltabadger::web::session::SessionData {
    deltabadger::web::session::open(&app.keys.session, browser.cookie.as_deref().unwrap(), web::at(NOW)).unwrap()
}

/// A second-factor step that was left open belongs to the sign-in that started it. A full password
/// sign-in in the same browser, as anyone, ends it: the open step cannot afterwards turn the
/// session into the first account's. (Rails keeps the pending step there.)
#[tokio::test(flavor = "current_thread")]
async fn a_password_sign_in_ends_a_second_factor_step_left_open() {
    let (_dir, _db, app, owner, second) = two_accounts(true);
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    let first = browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await;
    assert_eq!((first.status, first.header("location")), (302, Some("/verify_two_factor")));
    assert_eq!(browser.get(&app, "/verify_two_factor").await.status, 200, "the code is owed");
    assert_eq!(session_of(&app, &browser).pending.map(|pending| pending.user_id), Some(owner));

    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "second@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    assert_eq!(session_of(&app, &browser).pending, None, "the step left open is gone");
    assert_eq!(browser.get(&app, "/verify_two_factor").await.status, 302, "there is nothing to verify any more");
    assert_eq!(browser.get(&app, "/bots").await.status, 200);
    let code = deltabadger::crypto::totp_at(OTP_SEED, web::at(NOW).timestamp() as u64).unwrap();
    let late = browser.send(&app, "POST", "/verify_two_factor", Some(&[("user[otp_code_token]", &code)]), web::Csrf::Header, &[]).await;
    assert_eq!(late.status, 302, "the owner's code, valid as it is, opens nothing");
    assert_eq!(session_of(&app, &browser).user.map(|(id, _)| id), Some(second), "still the account that signed in");
}

/// A session that holds only a second-factor step still owed is nobody's: the password was right,
/// and that is all. No page of the app opens for it.
#[tokio::test(flavor = "current_thread")]
async fn a_session_with_only_a_second_factor_step_owed_is_not_signed_in() {
    use deltabadger::web::session::{self, Pending, SessionData};
    let (_dir, _db, app, owner, _) = two_accounts(true);
    let pending = SessionData { pending: Some(Pending { user_id: owner, started_at: web::at(NOW).timestamp() }), ..SessionData::default() };
    let mut browser = Browser { cookie: Some(session::seal(&app.keys.session, &pending, web::at(NOW))), ..Browser::default() };
    let bots = browser.get(&app, "/bots").await;
    assert_eq!((bots.status, bots.header("location")), (302, Some("/login")));
    assert_eq!(browser.get(&app, "/").await.header("location"), Some("/login"), "the root sends it to the login page, not to the bots");
    assert_eq!(browser.get(&app, "/verify_two_factor").await.status, 200, "the cookie is a live pending step, and still opens nothing else");
    assert_eq!(session_of(&app, &browser).user, None);
}

/// A password sign-in gives the session a new CSRF token (Devise's
/// clean_up_csrf_token_on_authentication): a token read from the login page is no use afterwards.
/// With two-factor that change happens at the password stage, when the session is replaced; the
/// code step then keeps the token its own form was rendered with.
#[tokio::test(flavor = "current_thread")]
async fn the_csrf_token_changes_at_the_password_and_is_kept_at_the_second_factor() {
    let (_dir, _db, app, _, _) = two_accounts(true);
    let token = |browser: &Browser| session_of(&app, browser).csrf;

    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    let before = token(&browser).expect("the login page gave the session a token");
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "second@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    assert_eq!(token(&browser), None, "the old token went with the sign-in");
    browser.get(&app, "/bots").await;
    let after = token(&browser).expect("the first page gave the session a new one");
    assert_ne!(before, after);

    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    let before = token(&browser).unwrap();
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 302);
    assert_eq!(token(&browser), None, "the password stage replaced the session");
    browser.get(&app, "/verify_two_factor").await;
    let at_code_form = token(&browser).unwrap();
    assert_ne!(before, at_code_form);
    let code = deltabadger::crypto::totp_at(OTP_SEED, web::at(NOW).timestamp() as u64).unwrap();
    assert_eq!(browser.post(&app, "/verify_two_factor", &[("user[otp_code_token]", &code)]).await.status, 303);
    assert_eq!(token(&browser).as_ref(), Some(&at_code_form), "the second factor keeps it");
    browser.get(&app, "/bots").await;
    assert_eq!(token(&browser), Some(at_code_form));
}

/// The rate limit is per address, and the address is the peer's: the server has to hand it to
/// every request. Over a real connection from 127.0.0.1, the eleventh login POST of the minute is
/// refused, and it is 127.0.0.1 that was counted, not the name used when no address is known.
#[tokio::test(flavor = "current_thread")]
async fn the_peer_address_reaches_the_rate_limit_through_the_server() {
    let (_dir, app, address) = served_under(deltabadger::web::server::Limits::default()).await;
    let post = b"POST /login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 3\r\nConnection: close\r\n\r\na=1";
    for n in 1..=10 {
        let answer = web::until_closed(address, post, Duration::from_secs(5)).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 302"), "request {n} has no CSRF token, and counts all the same: {answer}");
    }
    let eleventh = web::until_closed(address, post, Duration::from_secs(5)).await.unwrap();
    assert!(eleventh.starts_with("HTTP/1.1 429") && eleventh.to_lowercase().contains("retry-after: 30"), "{eleventh}");
    let post_method = axum::http::Method::POST;
    assert_eq!(app.limiter.hit(&post_method, "/login", "unattributed", web::at(NOW)), None, "nothing was counted under the name for an unknown peer");
    assert_eq!(app.limiter.hit(&post_method, "/login", "127.0.0.1", web::at(NOW)), Some(30), "the peer's own address is over its limit");
}

/// Behind a declared proxy the address is the one the proxy wrote. A proxy that appends its own
/// X-Forwarded-For line leaves the caller's line first, so the lines are read as one list: a caller
/// that sends a different first line with every request is still one address to the limit.
#[tokio::test(flavor = "current_thread")]
async fn a_forwarding_header_sent_as_several_lines_is_one_list() {
    let (_dir, app, address) = served_with(deltabadger::web::server::Limits::default(), &[("BEHIND_PROXY", "1")]).await;
    let post = |n: usize| {
        let request = format!("POST /login HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 203.0.113.{n}\r\nX-Forwarded-For: 198.51.100.7\r\n\
                               Content-Type: application/x-www-form-urlencoded\r\nContent-Length: 3\r\nConnection: close\r\n\r\na=1");
        web::until_closed(address, Box::leak(request.into_bytes().into_boxed_slice()), Duration::from_secs(5))
    };
    for n in 1..=10 {
        let answer = post(n).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 302"), "request {n}: {answer}");
    }
    let eleventh = post(11).await.unwrap();
    assert!(eleventh.starts_with("HTTP/1.1 429"), "the caller's own first line named another address each time, and it was believed: {eleventh}");
    let method = axum::http::Method::POST;
    assert_eq!(app.limiter.hit(&method, "/login", "198.51.100.7", web::at(NOW)), Some(30), "the address the proxy appended is the one counted");
    assert_eq!(app.limiter.hit(&method, "/login", "203.0.113.1", web::at(NOW)), None);
}

/// What no in-process test can see: the first page after sign-in in a real browser, with the compiled
/// JS and CSS (script/rust/browser_check.mjs drives headless Chrome). It needs Chrome and bun, so it
/// is not part of `cargo test`: run it with `cargo test --test serve -- --ignored`.
// Preserve the read-only milestone's empty-install browser proof: a populated
// fixture alone cannot detect accidentally reopening the creation wizard at login.
#[test]
#[ignore = "needs Chrome and bun: cargo test --test serve -- --ignored"]
fn a_real_browser_signs_in_to_an_empty_install() -> Result<(), Box<dyn std::error::Error>> {
    let (dir, opened, seeded) = common::install();
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").map_err(|e| format!("{e:?}"))?;
    opened.primary.execute("UPDATE users SET encrypted_password=?1,confirmed_at='2026-01-01 00:00:00' WHERE id=?2",(hash,seeded.user_id))?;
    drop(opened);
    let port = free_port();
    let mut server = Child(serve_command(dir.path(),port).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !up_within(port,Duration::from_secs(1)) {
        if Instant::now() >= deadline || server.0.try_wait()?.is_some() { return Err("empty-install server did not start".into()); }
        std::thread::sleep(Duration::from_millis(50));
    }
    let scratch = tempfile::tempdir()?;
    let log_path = scratch.path().join("browser.log");
    let log = std::fs::File::create(&log_path)?;
    let mut check = Child(Command::new("bun").arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../script/rust/browser_check.mjs"))
        .env("BASE_URL",format!("http://127.0.0.1:{port}")).env("EMAIL","o@example.com").env("PASSWORD","Correct-horse-9")
        .env_remove("BOT_ID").stdout(log.try_clone()?).stderr(log).spawn()?);
    let status = check.ended_within(Duration::from_secs(150)).ok_or("empty-install browser deadline")?;
    assert!(status.success(),"{}",std::fs::read_to_string(log_path)?);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs Chrome and bun: cargo test --test serve -- --ignored"]
async fn a_real_browser_signs_in_and_sees_the_app_with_live_streams() -> Result<(), Box<dyn std::error::Error>> {
    use common::seed;
    use serde_json::json;
    let (dir, opened, seeded) = common::install_alpaca();
    let c = &opened.primary;
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").map_err(|e| format!("{e:?}"))?;
    c.execute("UPDATE users SET encrypted_password=?1, confirmed_at='2026-01-01 00:00:00', wash_sale_enabled=0", [hash])?;
    let mut spec = seed::BotSpec::weekly(5.0, "2026-09-01 10:00:00")
        .transient("last_action_job_at", json!("2026-09-01T10:00:01Z"));
    spec.status = 2;
    let bot = seed::insert_bot(c, &seeded, &spec);
    for _ in 0..2 { seed::insert_bot(c, &seeded, &spec); }
    c.execute("UPDATE bots SET label='Browser '||id", [])?;
    for (key,value) in [("market_data_provider","deltabadger"),("market_data_url","http://example.test"),("market_data_token","test")] {
        c.execute("INSERT INTO app_configs(key,value,created_at,updated_at) VALUES(?1,?2,'2026-01-01','2026-01-01')",(key,seed::cipher().encrypt(value)))?;
    }
    // Hold the engine: this browser proof exercises the real HTTP/CSRF/guard/cable path
    // on a scripted paper install; the executable smoke test separately proves placement.
    let app = web::app(dir.path(), "engine-test-secret", TestClock::at(NOW));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server { fn drop(&mut self) { self.0.abort(); } }
    let _server = Server(tokio::spawn(async move { let _ = deltabadger::web::server::serve_on(listener,app,Default::default()).await; }));
    let log = tempfile::tempfile()?;
    let mut check = Child(Command::new("bun").arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../script/rust/browser_check.mjs"))
        .env("BASE_URL",format!("http://{address}")).env("EMAIL","o@example.com").env("PASSWORD","Correct-horse-9")
        .env("BOT_ID",bot.to_string()).stdout(log.try_clone()?).stderr(log.try_clone()?).spawn()?);
    let deadline = Instant::now() + Duration::from_secs(150);
    let status = loop {
        if let Some(status) = check.0.try_wait()? { break status; }
        if Instant::now() >= deadline { return Err("browser check exceeded absolute 150-second deadline".into()); }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    use std::io::{Seek, SeekFrom};
    let mut log = log;
    log.seek(SeekFrom::Start(0))?;
    let mut output = String::new(); log.read_to_string(&mut output)?;
    assert!(status.success(), "{output}");
    assert_eq!(c.query_row("SELECT status FROM bots WHERE id=?1",[bot],|row|row.get::<_,i64>(0))?,3);
    assert_eq!(c.query_row("SELECT count(*) FROM transactions",[],|row|row.get::<_,i64>(0))?,0);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn wake_engine_leaves_one_permit_for_the_attached_engine_and_does_nothing_without_one() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    app.wake_engine(); // no engine attached (every router test): nothing happens, nothing fails
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    app.attach_engine(wake.clone());
    app.wake_engine();
    app.wake_engine(); // two writes before the engine looks: one more pass, not two
    assert!(tokio::time::timeout(Duration::from_millis(50), wake.notified()).await.is_ok(), "the permit is stored for an engine mid-pass");
    assert!(tokio::time::timeout(Duration::from_millis(50), wake.notified()).await.is_err(), "wakes coalesce");
}

#[tokio::test(flavor = "current_thread")]
async fn bind_refuses_an_install_with_no_admin_user_before_it_listens() {
    let dir = common::rails_install();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let refused = deltabadger::web::server::bind(&app, 0).await;
    assert!(matches!(&refused, Err(deltabadger::web::WebError::Config(m)) if m.contains("no admin user")), "{refused:?}");
    let (dir, opened, _) = common::install(); // seeds an admin user
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let listener = deltabadger::web::server::bind(&app, 0).await.unwrap();
    assert_ne!(listener.local_addr().unwrap().port(), 0, "bound, not yet serving");
}

/// Once `serve_on` is dropped (in `serve`: the engine returned), no request reaches the app. A
/// second request already on an open keep-alive connection when the server is dropped is answered 503, or the
/// connection is closed; never by the app. Deterministic: the request is written and the server dropped on this thread
/// with no await in between, so the connection task runs only after both.
#[tokio::test(flavor = "current_thread")]
async fn a_request_ready_on_a_keep_alive_connection_when_the_server_is_dropped_never_reaches_the_app() {
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut server = Box::pin(deltabadger::web::server::serve_on(listener, app, deltabadger::web::server::Limits::default()));
    let first = async {
        tokio::task::spawn_blocking(move || {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let first = web::keep_alive_get(&mut stream, "/up");
            (stream, first)
        }).await.unwrap()
    };
    let (mut stream, first) = tokio::select! {
        r = &mut server => panic!("the server stopped: {r:?}"),
        done = first => done,
    };
    assert!(first.as_deref().is_some_and(|a| a.starts_with("HTTP/1.1 200")), "{first:?}");
    assert!(web::send_get(&mut stream, "/up"), "the second request is on the wire");
    drop(server); // the engine returned
    let second = tokio::task::spawn_blocking(move || { let mut stream = stream; web::read_answer(&mut stream) }).await.unwrap();
    assert!(second.as_deref().is_none_or(|a| a.starts_with("HTTP/1.1 503")), "the app answered after the server was dropped: {second:?}");
}


/// ALLOWED_HOSTS, as Rails' HostAuthorization in front of the whole app: a request for a host that
/// is not allowed gets the same empty 403 whatever it asks for, before its query, its body, its
/// cookie or the rate limit are looked at. Without the variable every host is served.
#[tokio::test(flavor = "current_thread")]
async fn a_request_for_a_host_that_is_not_allowed_is_refused_before_anything_reads_it() {
    use tower::ServiceExt;
    let (dir, opened, _) = common::install();
    drop(opened);
    let app = web::app_allowing(dir.path(), web::SECRET, Some("app.example, .apps.example"), TestClock::at(NOW));
    async fn ask(app: &deltabadger::web::App, method: &str, path: &str, host: &str, forwarded: Option<&str>) -> (u16, Vec<String>, String) {
        let mut request = axum::http::Request::builder().method(method).uri(path).header("host", host).header("cookie", "_deltabadger_rust_session=anything");
        if let Some(forwarded) = forwarded { request = request.header("x-forwarded-host", forwarded); }
        if method == "POST" { request = request.header("content-type", "application/x-www-form-urlencoded"); }
        let mut request = request.body(axum::body::Body::from(if method == "POST" { "user%5Bemail%5D=a&".repeat(5000) } else { String::new() })).unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 40000))));
        let response = deltabadger::web::router(app.clone()).oneshot(request).await.unwrap();
        let names = response.headers().keys().map(|name| name.as_str().to_string()).collect();
        (response.status().as_u16(), names, String::from_utf8_lossy(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).into_owned())
    }
    let css = deltabadger::web::assets::path("application.css");
    let long_query = format!("/up?{}", "q=1&".repeat(4000));
    let refused = (403, vec!["content-type".to_string(), "content-length".to_string()], String::new());
    for (method, path) in [("GET", "/up"), ("GET", "/login"), ("GET", css), ("GET", "/cable"), ("POST", "/login"), ("DELETE", "/logout"), ("GET", "/nothing"), ("GET", long_query.as_str())] {
        assert_eq!(ask(&app, method, path, "evil.example", None).await, refused, "{method} {path}");
        assert_eq!(ask(&app, method, path, "app.example", Some("evil.example")).await, refused, "{method} {path} with a forwarded host");
    }
    // Twenty refused sign-ins counted nothing: the first one from an allowed host is not the limit's 429.
    for _ in 0..20 { assert_eq!(ask(&app, "POST", "/login", "evil.example", None).await.0, 403); }
    assert_ne!(ask(&app, "POST", "/login", "app.example", None).await.0, 429);
    for (host, forwarded) in [("app.example", None), ("APP.example:8443", None), ("bot.apps.example", None), ("localhost:3000", None), ("127.0.0.1", None), ("app.example", Some("evil.example, bot.apps.example"))] {
        assert_eq!(ask(&app, "GET", "/up", host, forwarded).await.0, 200, "{host} {forwarded:?}");
    }
    for host in ["a.b.apps.example", "apps.example.evil.example", "app.example.", "evil.example:80"] {
        assert_eq!(ask(&app, "GET", "/up", host, None).await.0, 403, "{host}");
    }
    let open = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    assert_eq!(ask(&open, "GET", "/up", "evil.example", Some("other.example")).await.0, 200, "no ALLOWED_HOSTS: no list");
}

/// The bot as the pages read it (web::bot), on a row the engine's fixtures write: what Rails'
/// model derives from the row, when it may not be started, and what this build refuses to render.
#[test]
fn a_bot_row_is_read_as_the_pages_need_it_and_refused_when_this_build_cannot_render_it() {
    use deltabadger::web::bot::{self, start, Bot, Kind};
    use serde_json::json;
    let (_dir, opened, seeded) = common::install_alpaca();
    let c = &opened.primary;
    let now: chrono::DateTime<chrono::Utc> = NOW.parse().unwrap();
    let id = common::seed::insert_bot(c, &seeded, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    c.execute("UPDATE bots SET label = 'Bitcoin' WHERE id = ?1", [id]).unwrap();
    let refusal = || bot::refusal(c, id, Some(false), true, bot::For::Page).unwrap();
    assert_eq!(refusal(), None, "the engine's fixture stores only what the engine reads: a row from before the rules existed");

    // What each concern's after_initialize supplies on load, and the wizard then stores, for a one-asset basket.
    let stored: String = c.query_row("SELECT settings FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    let mut settings: serde_json::Value = serde_json::from_str(&stored).unwrap();
    let defaults = json!({
        "smart_intervaled": false, "smart_interval_quote_amount": 6.0, "limit_ordered": false, "limit_order_pcnt_distance": 0.001,
        "quote_amount_limited": false, "quote_amount_limit": 1000, "price_limited": false, "price_limit": 1000000, "price_limit_range_lower_bound": 0,
        "price_limit_range_upper_bound": 1000000, "price_limit_timing_condition": "while", "price_limit_value_condition": "below",
        "price_drop_limited": false, "price_drop_limit": 0.2, "price_drop_limit_time_window_condition": "ath", "moving_average_limited": false,
        "moving_average_limit_timing_condition": "while", "moving_average_limit_value_condition": "below", "moving_average_limit_in_ma_type": "sma",
        "moving_average_limit_in_timeframe": "one_day", "moving_average_limit_in_period": 9, "indicator_limited": false, "indicator_limit": 30,
        "indicator_limit_timing_condition": "while", "indicator_limit_value_condition": "below", "indicator_limit_in_indicator": "rsi",
        "indicator_limit_in_timeframe": "one_day",
    });
    // The row lacks every one of them: the bot is read with Rails' defaults, a condition watches the
    // venue's first ticker, and nothing is written.
    let bare = Bot::find(c, seeded.user_id, id, bot::For::Page).unwrap().unwrap();
    for (key, value) in defaults.as_object().unwrap() { assert_eq!(bare.settings.get(key), Some(value), "{key}"); }
    for rule in ["price_limit", "price_drop_limit", "moving_average_limit", "indicator_limit"] {
        assert_eq!(bare.settings.get(&format!("{rule}_in_ticker_id")), Some(&json!(seeded.ticker_id)), "{rule}");
    }
    assert_eq!(c.query_row("SELECT settings FROM bots WHERE id = ?1", [id], |r| r.get::<_, String>(0)).unwrap(), stored, "a read stores nothing");
    let store = |settings: &serde_json::Value| c.execute("UPDATE bots SET settings = ?1 WHERE id = ?2", (settings.to_string(), id)).unwrap();
    // `||=`: null and false are missing too. A whole amount divides as an Integer in Ruby: a tenth of 25 is 2.
    let mut whole = settings.clone();
    (whole["quote_amount"], whole["smart_interval_quote_amount"], whole["price_limit"], whole["interval"]) = (json!(25), json!(null), json!(false), json!("day"));
    store(&whole);
    let filled = Bot::find(c, seeded.user_id, id, bot::For::Page).unwrap().unwrap();
    assert_eq!((filled.settings.get("smart_interval_quote_amount"), filled.settings.get("price_limit")), (Some(&json!(2.0)), Some(&json!(1000000))));
    for (key, value) in defaults.as_object().unwrap() { settings[key] = value.clone(); }
    store(&settings);
    assert_eq!(refusal(), None);

    let found = Bot::find(c, seeded.user_id, id, bot::For::Page).unwrap().unwrap();
    assert_eq!((found.kind, found.one_asset(), found.dom_id("tile")), (Kind::Basket, true, format!("tile_bots_dca_multi_asset_{id}")));
    assert_eq!((found.allocations_total(), found.allocations_balanced(), found.quote_decimals(), found.api_key_correct()), (1.0, true, Some(2), true));
    assert_eq!((found.tickers.len(), found.quote_symbol(), found.exchange.name_id().as_str()), (1, Some("USD"), "alpaca"));
    // Weekly since 2026-09-01 10:00: the engine's schedule gives the next and the last checkpoint.
    let checkpoints = found.checkpoints(now).unwrap();
    assert_eq!((checkpoints.last_us, checkpoints.next_us), (web::at("2026-09-08T10:00:00Z").timestamp_micros(), web::at("2026-09-15T10:00:00Z").timestamp_micros()));
    assert!(Bot::find(c, seeded.user_id + 1, id, bot::For::Page).unwrap().is_none(), "another user's bot is not found");

    // `bot.invalid?(:start)`: valid as it stands; not while its only listing is withdrawn; not with a starting time that has passed.
    let invalid = |settings: &serde_json::Value| {
        store(settings);
        start::check(c, &Bot::find(c, seeded.user_id, id, bot::For::Page).unwrap().unwrap(), now, true, "en").unwrap()
    };
    assert!(!invalid(&settings).invalid);
    c.execute("UPDATE tickers SET available = 0", []).unwrap();
    assert!(invalid(&settings).invalid, "validate_tickers_available");
    c.execute("UPDATE tickers SET available = 1", []).unwrap();
    let mut late = settings.clone();
    (late["start_time_enabled"], late["start_time_mode"], late["start_at"]) = (json!(true), json!("date"), json!("2026-09-10T12:00:30Z"));
    let check = invalid(&late);
    assert_eq!((check.invalid, check.start_at), (true, Some("must_be_future")), "a start at this very second is not in the future");
    // Bot::Startable#validate_starting_time_settings: the rule on with no mode, or an emptied one, is the mode's error,
    // whatever the clock time says; a switch that is null, "0" or absent is off, and nothing of the rule is validated.
    let starting = |enabled: serde_json::Value, mode: serde_json::Value, time: &str| {
        let mut changed = settings.clone();
        (changed["start_time_enabled"], changed["start_time_mode"], changed["start_time_of_day"]) = (enabled, mode, json!(time));
        let check = invalid(&changed);
        (check.invalid, check.start_time_mode, check.start_time_of_day)
    };
    assert_eq!(starting(json!(true), json!(null), "09:30"), (true, true, false));
    assert_eq!(starting(json!(true), json!(""), "25:00"), (true, true, false));
    assert_eq!(starting(json!("1"), json!("friday"), "25:00"), (true, false, true));
    assert_eq!(starting(json!(true), json!("hour"), "9:5"), (false, false, false));
    for off in [json!(null), json!(false), json!("0"), json!("off"), json!(""), json!(0)] {
        assert_eq!(starting(off.clone(), json!(null), "25:00"), (false, false, false), "{off}");
    }
    let mut small = settings.clone();
    (small["smart_intervaled"], small["smart_interval_quote_amount"]) = (json!(true), json!(0.001));
    assert!(invalid(&small).smart_interval_quote_amount.is_some_and(|message| message.contains("0.03")), "60 a week in slices no more often than every five minutes: 0.03 at least");
    let mut capped = settings.clone();
    (capped["quote_amount_limited"], capped["quote_amount_limit"]) = (json!(true), json!(0));
    assert!(invalid(&capped).invalid, "a cap with nothing left");
    // Smart Intervals stretch the interval by slice over amount, and Rails sets the slice no upper bound: 60 a week in slices
    // of 3,130,000 is an order every 999.8 years, which is still printed; a little more is past what the calendar here is
    // asked to hold, and a slice the size of the largest Float is no span at all. Without the rule the span is the interval.
    let apart = |slice: serde_json::Value| {
        let mut slow = settings.clone();
        (slow["smart_intervaled"], slow["smart_interval_quote_amount"]) = (json!(true), slice);
        store(&slow);
        assert_eq!(bot::refusal(c, id, Some(false), true, bot::For::Page).unwrap(), None, "the row alone does not say it");
        Bot::find(c, seeded.user_id, id, bot::For::Page).unwrap().unwrap().unrendered()
    };
    let too_far = Some("Smart Intervals that leave more than a thousand years between two orders");
    assert_eq!((apart(json!(3_130_000)), apart(json!(3_131_000)), apart(json!(1.7e308))), (None, too_far, too_far));
    // And at the other end: this bot is scheduled, and a slice of nothing, of less than nothing, or small enough to
    // underflow leaves no span to compute a checkpoint from. 60 a week is an order a second at a slice of 60/604800.
    let no_time = Some("a working bot whose Smart Intervals leave no time between two orders");
    assert_eq!((apart(json!(0)), apart(json!(-5)), apart(json!(1e-320)), apart(json!(0.00009))), (no_time, no_time, no_time, no_time));
    assert_eq!(apart(json!(0.0001)), None, "a little over a second apart");
    // A bot that is not working has no checkpoint to compute: Rails prints its own error under the field, and so does this build.
    c.execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap();
    assert_eq!((apart(json!(0)), apart(json!(-5))), (None, None));
    let mut nothing = settings.clone();
    (nothing["smart_intervaled"], nothing["smart_interval_quote_amount"]) = (json!(true), json!(0));
    assert!(invalid(&nothing).smart_interval_quote_amount.is_some(), "the floor's message, as for any amount under it");
    c.execute("UPDATE bots SET status = 1 WHERE id = ?1", [id]).unwrap();
    store(&settings);
    assert_eq!(Bot::find(c, seeded.user_id, id, bot::For::Page).unwrap().unwrap().unrendered(), None);

    // One thing at a time that this build does not render.
    let refused = |change: &dyn Fn(&mut serde_json::Value), wash_sale: Option<bool>, deltabadger: bool| {
        let mut changed = settings.clone();
        change(&mut changed);
        store(&changed);
        bot::refusal(c, id, wash_sale, deltabadger, bot::For::Page).unwrap()
    };
    assert_eq!(refused(&|s| s["direction"] = json!("selling"), Some(false), true), Some("a selling bot"));
    assert_eq!(refused(&|s| s["rebalance_enabled"] = json!(true), Some(false), true), Some("rebalancing"));
    assert_eq!(refused(&|s| s["weighting"] = json!("market_cap"), Some(false), true), Some("market-cap weights"));
    assert_eq!(refused(&|s| s["quote_amount"] = json!(0), Some(false), true), Some("settings no form would have saved"));
    assert_eq!(refused(&|s| s["interval"] = json!("year"), Some(false), true), Some("settings no form would have saved"));
    assert_eq!(refused(&|s| s["start_time_mode"] = json!("noon"), Some(false), true), Some("settings no form would have saved"));
    assert_eq!(refused(&|s| { s.as_object_mut().unwrap().remove("price_limit"); }, Some(false), true), None, "Rails has a default for it");
    assert_eq!(refused(&|s| { s.as_object_mut().unwrap().remove("allocations"); }, Some(false), true), Some("settings the wizard always stores are missing"));
    assert_eq!(refused(&|s| s["limit_order_pcnt_distance"] = json!(2), Some(false), true), Some("settings no form would have saved"));
    assert_eq!(refused(&|s| s["smart_interval_quote_amount"] = json!("5"), Some(false), true), Some("settings no form would have saved"));
    // A setting in a shape no form stores. Where Rails' own reader coerces a text (a weight with `to_f`, the rebalance
    // threshold with `to_d`, an id it looks up) and the text is plainly a number, it is read as Rails reads it;
    // everything else is refused, never read as a default or as nothing.
    let shape = Some("a setting stored in a shape this build does not read");
    let btc = seeded.btc.to_string();
    let word = "<img src=x onerror=alert(1)>"; // no word of any list
    for (key, value) in [
        ("quote_asset_id", json!(seeded.quote.to_string())), ("allocations", json!([1.0])), ("allocations", json!({ btc.clone(): "0.6abc" })),
        ("allocations", json!({ btc.clone(): true })), ("allocations", json!({ format!("0{btc}"): 1.0 })), ("allocations", json!({ format!("{btc}abc"): 1.0 })),
        ("rebalance_threshold", json!("a fifth")), ("rebalance_threshold", json!(true)), ("quote_amount_limit", json!("1000")),
        ("price_limit", json!("100")), ("price_limit_range_upper_bound", json!([1])), ("price_drop_limit", json!("0.2")), ("indicator_limit", json!("30")),
        ("moving_average_limit_in_period", json!(9.5)), ("moving_average_limit_in_period", json!("9")), ("price_limit_in_ticker_id", json!(1.5)),
        ("indicator_limit_in_ticker_id", json!("12abc")), ("start_at", json!(5)), ("start_time_of_day", json!(930)),
        // The settings that are one of a list of words are held to the list: a text that is none of them reaches no page.
        ("price_limit_timing_condition", json!(word)), ("price_limit_value_condition", json!(word)), ("price_limit_action", json!(word)),
        ("price_drop_limit_time_window_condition", json!(word)), ("price_drop_limit_action", json!(word)),
        ("moving_average_limit_timing_condition", json!(word)), ("moving_average_limit_value_condition", json!("between")),
        ("moving_average_limit_in_ma_type", json!(word)), ("moving_average_limit_in_timeframe", json!(word)), ("moving_average_limit_action", json!(word)),
        ("indicator_limit_timing_condition", json!(word)), ("indicator_limit_value_condition", json!(word)), ("indicator_limit_in_timeframe", json!(word)),
        ("indicator_limit_action", json!(word)),
        // What Rails validates when it is asked whether the bot may start, and no page prints: `indicator_limit_in_indicator` has one
        // word (app/models/bot/indicator_limitable.rb:62), a switch is true or false, a weighting and a direction are of their lists.
        ("indicator_limit_in_indicator", json!("macd")), ("indicator_limit_in_indicator", json!(["rsi"])),
        ("smart_intervaled", json!("true")), ("limit_ordered", json!(1)), ("quote_amount_limited", json!("1")), ("price_limited", json!("on")),
        ("price_drop_limited", json!(0)), ("moving_average_limited", json!([])), ("indicator_limited", json!("false")),
        ("weighting", json!("equal")), ("direction", json!("sideways")), ("index_type", json!("bottom")), ("allocation_flattening", json!(2)),
        ("rebalance_threshold", json!(0)), ("rebalance_threshold", json!(1.5)), ("rebalance_threshold", json!("0")), ("rebalance_threshold", json!("2")),
    ] {
        assert_eq!(refused(&|s| s[key] = value.clone(), Some(false), true), shape, "{key}: {value}");
    }
    for (key, value) in [("rebalance_threshold", json!("0.2")), ("rebalance_threshold", json!("")), ("rebalance_threshold", json!(false)), ("rebalance_threshold", json!(1)),
                         ("allocations", json!({ btc.clone(): "1" })), ("price_limit_in_ticker_id", json!(seeded.ticker_id.to_string())), ("price_limit", json!(null)),
                         ("price_limit_action", json!("start_selling")), ("indicator_limit_in_indicator", json!("rsi")), ("weighting", json!("manual")),
                         ("direction", json!("buying")), ("smart_intervaled", json!(null)), ("allocation_flattening", json!(0.5))] {
        assert_eq!(refused(&|s| s[key] = value.clone(), Some(false), true), None, "{key}: {value}");
    }
    let read = |change: &dyn Fn(&mut serde_json::Value)| {
        assert_eq!(refused(change, Some(false), true), None);
        Bot::find(c, seeded.user_id, id, bot::For::Page).unwrap().unwrap()
    };
    let texts = read(&|s| (s["allocations"], s["rebalance_threshold"]) = (json!({ btc.clone(): "0.6" }), json!("0.2")));
    assert_eq!((texts.allocations(), texts.rebalance_threshold().map(|share| share.to_s_f())), (vec![(seeded.btc, 0.6)], Some("0.2".to_string())), "as `to_f` and `to_d` read them");
    assert_eq!(read(&|s| s["rebalance_threshold"] = json!(0.1)).rebalance_threshold().map(|share| share.to_s_f()).as_deref(), Some("0.1"));
    assert_eq!(read(&|s| s["rebalance_threshold"] = json!("")).rebalance_threshold().map(|share| share.to_s_f()).as_deref(), Some("0.05"), "`presence`: a blank text is the default");
    // What Rails validates only while its rule is on: off, the row may hold anything and is rendered; on, it is an error under a field, and refused here.
    for (switch, key, value) in [("price_limited", "price_limit", json!(-1)), ("price_limited", "price_limit_range_lower_bound", json!(-0.5)),
                                 ("price_limited", "price_limit_range_upper_bound", json!(-1)), ("price_drop_limited", "price_drop_limit", json!(1.5)),
                                 ("price_drop_limited", "price_drop_limit", json!(-0.1)), ("moving_average_limited", "moving_average_limit_in_period", json!(0))] {
        assert_eq!(refused(&|s| (s[switch], s[key]) = (json!(false), value.clone()), Some(false), true), None, "{key}: {value}, off");
        assert_eq!(refused(&|s| (s[switch], s[key]) = (json!(true), value.clone()), Some(false), true), shape, "{key}: {value}, on");
    }
    // A cap that is on and below the smallest amount its quote states (0.01 here) is the same kind of error; the tickers say what that is, so it is asked of the loaded bot.
    let capped_at = |cap: serde_json::Value| read(&|s| (s["quote_amount_limited"], s["quote_amount_limit"]) = (json!(true), cap.clone())).unrendered();
    assert_eq!((capped_at(json!(0.001)), capped_at(json!(0)), capped_at(json!(0.01))), (Some("a spending cap below the smallest amount its quote states"), Some("a spending cap below the smallest amount its quote states"), None));
    assert_eq!(read(&|s| (s["quote_amount_limited"], s["quote_amount_limit"]) = (json!(false), json!(0))).unrendered(), None, "off, it is not validated");
    // Rails' floor is the bot's tickers' (app/models/bot/quote_amount_limitable.rb:87): none listed, none, whatever the memberships hold.
    c.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, created_at, updated_at) VALUES (?1, ?2, ?3, 1, 1, '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
              (id, seeded.btc, seeded.ticker_id)).unwrap();
    c.execute("UPDATE tickers SET available = 0", []).unwrap();
    assert_eq!(capped_at(json!(0.001)), None, "a sub-cent cap on a bot whose members are no longer listed, as Rails saves it");
    c.execute("UPDATE tickers SET available = 1", []).unwrap();
    c.execute("DELETE FROM bot_index_assets WHERE bot_id = ?1", [id]).unwrap();
    // The two times the pages read out of `transient_data`, and the carry Rails validates as not negative.
    for transient in [json!({ "last_action_job_at": 5 }), json!({ "last_action_job_at": "yesterday" }), json!({ "quote_amount_limit_enabled_at": true }),
                      json!({ "missed_quote_amount": -1 }), json!({ "missed_quote_amount": "-0.5" }), json!({ "missed_quote_amount": "a lot" }), json!({ "missed_quote_amount": [1] }),
                      json!({ "missed_quote_amount": "1e1000000000" })] {
        c.execute("UPDATE bots SET transient_data = ?1 WHERE id = ?2", (transient.to_string(), id)).unwrap();
        assert_eq!(refused(&|_| {}, Some(false), true), shape, "{transient}");
    }
    c.execute("UPDATE bots SET transient_data = ?1 WHERE id = ?2", (json!({ "last_action_job_at": "", "quote_amount_limit_enabled_at": "2026-09-01T00:00:00.000Z", "missed_quote_amount": "12.5" }).to_string(), id)).unwrap();
    assert_eq!(refused(&|_| {}, Some(false), true), None, "an empty text is no time in Rails either");
    // The carry as Rails writes it after a budget of 1e31 and a fill of 100: a BigDecimal's text of 31 digits, read within BigDec's bounds.
    c.execute("UPDATE bots SET transient_data = ?1 WHERE id = ?2", (json!({ "missed_quote_amount": "9999999999999999999999999999900.0" }).to_string(), id)).unwrap();
    assert_eq!(refused(&|_| {}, Some(false), true), None, "a carry Rails persists");
    c.execute("UPDATE bots SET transient_data = '{}' WHERE id = ?1", [id]).unwrap();
    assert_eq!(refused(&|_| {}, Some(true), true), Some("the wash-sale rule"));
    assert_eq!(refused(&|_| {}, None, true), None, "not answered, and nothing traded: Rails asks nothing yet");
    store(&settings);
    for (sql, reason) in [
        ("UPDATE bots SET label = '  '", "a bot without a label, which Rails writes on load"),
        ("UPDATE bots SET type = 'Bots::Signal'", "a bot of a type this build does not render"),
        ("UPDATE bots SET transient_data = '{\"rebalance_pending\":{\"phase\":\"selling\"}}'", "a rebalance, liquidation or redeploy in progress"),
        ("UPDATE exchanges SET type = 'Exchanges::Kraken'", "a bot on an exchange other than Alpaca"),
        // A ticker's decimals size the rounding: one past the bound, a negative one and the largest number the column holds.
        ("UPDATE tickers SET base_decimals = 15", "a ticker with more decimals than this build rounds to"),
        ("UPDATE tickers SET quote_decimals = -1", "a ticker with more decimals than this build rounds to"),
        ("UPDATE tickers SET quote_decimals = 9223372036854775807", "a ticker with more decimals than this build rounds to"),
    ] {
        c.execute_batch(&format!("SAVEPOINT change; {sql};")).unwrap();
        assert_eq!(refusal(), Some(reason), "{sql}");
        c.execute_batch("ROLLBACK TO change; RELEASE change;").unwrap();
    }
    // At the bound the bot is served, and what the pages compute from the decimals is computed.
    c.execute_batch(&format!("SAVEPOINT bound; UPDATE tickers SET base_decimals = {0}, quote_decimals = {0};", bot::MAX_DECIMALS)).unwrap();
    assert_eq!((bot::MAX_DECIMALS, refusal()), (14, None));
    let precise = Bot::find(c, seeded.user_id, id, bot::For::Page).unwrap().unwrap();
    assert_eq!(precise.quote_decimals(), Some(14));
    assert!(start::smart_interval_minimum(&precise).value.to_f() > 0.0 && !start::check(c, &precise, now, true, "en").unwrap().invalid);
    assert_eq!(deltabadger::ruby::BigDec::parse("1.23456789").unwrap().round(precise.quote_decimals().unwrap()).to_s_f(), "1.23456789");
    c.execute_batch("ROLLBACK TO bound; RELEASE bound;").unwrap();
    // A decimal the columns hold and `ruby::BigDec` does not read (its bounds: 256 characters, an exponent within ±400, 512
    // digits written out). The row alone is not refused; reading it fails with the mark the handlers answer the 501 page for,
    // and nothing is sized by it. SQLite keeps a text it cannot read as a number as the text it is.
    for (table, column) in [("tickers", "minimum_quote_size"), ("bot_index_assets", "target_allocation")] {
        c.execute_batch("SAVEPOINT stored;").unwrap();
        c.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, created_at, updated_at) VALUES (?1, ?2, ?3, 1, 1, '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
                  (id, seeded.btc, seeded.ticker_id)).unwrap();
        assert!(Bot::find(c, seeded.user_id, id, bot::For::Page).is_ok());
        for stored in ["1_0e1000000000", "1e1000000000", &"9".repeat(600)] {
            c.execute(&format!("UPDATE {table} SET {column} = ?1"), [stored]).unwrap();
            assert_eq!(refusal(), None, "{table}.{column} = {}", &stored[..14]);
            let failed = Bot::find(c, seeded.user_id, id, bot::For::Page).expect_err("a number this build does not read");
            assert!(bot::unreadable(&failed), "{table}.{column}: {failed:?}");
        }
        c.execute_batch("ROLLBACK TO stored; RELEASE stored;").unwrap();
    }
    assert!(!bot::unreadable(&deltabadger::web::WebError::Config("anything else".into())));
    c.execute("UPDATE bots SET type = 'Bots::DcaIndex', settings = ?1 WHERE id = ?2", (json!({ "quote_asset_id": seeded.quote, "quote_amount": 100, "interval": "week", "num_coins": 10,
        "allocation_flattening": 0.0, "index_type": "top", "smart_intervaled": false, "limit_ordered": false, "limit_order_pcnt_distance": 0.001 }).to_string(), id)).unwrap();
    assert_eq!(bot::refusal(c, id, Some(false), false, bot::For::Page).unwrap(), Some("an index bot whose market data comes from CoinGecko"));
    assert_eq!(bot::refusal(c, id, Some(false), true, bot::For::Page).unwrap(), None);
    store(&settings);
    c.execute("UPDATE bots SET type = 'Bots::DcaMultiAsset' WHERE id = ?1", [id]).unwrap();

    // An order that is not a scheduled buy needs the metrics walk; an account that has not answered the wash-sale question needs it once it has traded.
    let order = |side: i64, kind: &str| c.execute("INSERT INTO transactions (bot_id, exchange_id, status, external_status, side, transaction_type, bot_interval, bot_quote_amount, error_messages, created_at, updated_at) \
                                                   VALUES (?1, ?2, 0, 2, ?3, ?4, 'week', 60, '[]', '2026-09-08 10:00:00', '2026-09-08 10:00:00')", (id, seeded.exchange_id, side, kind)).unwrap();
    order(0, "REGULAR");
    assert_eq!(refusal(), None);
    assert_eq!(bot::refusal(c, id, None, true, bot::For::Page).unwrap(), Some("the wash-sale question not answered yet"));
    order(1, "REGULAR");
    assert_eq!(refusal(), Some("orders other than scheduled buys"));
    // The feed is asked page after page and does not walk the history for a sell: it refuses the page of rows that holds one.
    assert_eq!(bot::refusal(c, id, Some(false), true, bot::For::Feed).unwrap(), None);
    c.execute("DELETE FROM transactions WHERE side = 1", []).unwrap();
    // An order of another type is found by the page through an index, on either side of 'REGULAR' in its order. The feed does
    // not ask: Rails picks its ten rows first, and the page of rows that holds such an order is the one refused (Task 8).
    for kind in ["REBALANCE", "LIQUIDATION", "REDEPLOY"] {
        order(0, kind);
        assert_eq!(bot::refusal(c, id, Some(false), true, bot::For::Page).unwrap(), Some("orders other than scheduled buys"), "{kind}");
        assert_eq!(bot::refusal(c, id, Some(false), true, bot::For::Feed).unwrap(), None, "{kind}");
        c.execute("DELETE FROM transactions WHERE transaction_type = ?1", [kind]).unwrap();
    }
    let plan: String = c.prepare("EXPLAIN QUERY PLAN SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND transaction_type < 'REGULAR') OR EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND transaction_type > 'REGULAR')")
        .unwrap().query_map([id], |r| r.get::<_, String>(3)).unwrap().map(Result::unwrap).collect::<Vec<_>>().join("; ");
    assert_eq!(plan.matches("index_bot_type_created_at (bot_id=? AND transaction_type").count(), 2, "{plan}");
    // The feed is not given the four facts about the bot's orders either: each may walk the history.
    assert!(Bot::find(c, seeded.user_id, id, bot::For::Page).unwrap().unwrap().has_orders);
    assert!(!Bot::find(c, seeded.user_id, id, bot::For::Feed).unwrap().unwrap().has_orders);
}

/// A text that is markup if it is printed as it is, and that closes an attribute if it is printed inside one.
const HOSTILE: &str = "<img src=x onerror=alert(1)>\" onmouseover=\"alert(2)";

/// An Alpaca install in which every text the bot pages print out of the database is `HOSTILE`: the
/// assets' symbols and names, the tickers' spellings, the venue's name, the bots' labels and stop
/// reasons, an order's error, and the settings a page prints or looks a translation up by. Two bots,
/// so that the list is a list: a retrying one-asset basket with every condition on (each sentence
/// names its lone subject), and a stopped basket of two (each sentence offers a choice).
fn hostile_install() -> (tempfile::TempDir, deltabadger::store::Opened, deltabadger::web::App, Browser, [i64; 2]) {
    use serde_json::json;
    let (dir, opened, seeded) = common::install_alpaca();
    let c = &opened.primary;
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    c.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00', wash_sale_enabled = 0 WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('ethereum', 'ETH', 'Ethereum', 'Cryptocurrency', ?1, ?1)", ["2026-01-01 00:00:00"]).unwrap();
    let eth = c.last_insert_rowid();
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, minimum_base_size, \
               minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, 'ETH/USD', 'ETH', 'USD', ?2, ?3, 9, 2, 2, '0.001', '1', 1, 1, ?4, ?4)",
              (seeded.exchange_id, eth, seeded.quote, "2026-01-01 00:00:00")).unwrap();
    let eth_ticker = c.last_insert_rowid();
    let rules = json!({ "price_limited": true, "price_drop_limited": true, "moving_average_limited": true, "indicator_limited": true, "smart_intervaled": true,
                        "limit_ordered": true, "quote_amount_limited": true, "start_time_enabled": true, "start_time_mode": "friday", "start_time_of_day": HOSTILE });
    // The settings that are free text. Those that are one of a list of words are held to the list (`bot::refusal`), so no text of theirs reaches a page.
    let texts = json!({ "start_at": HOSTILE });
    let spec = |status: i64, allocations: serde_json::Value| {
        let mut spec = common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("allocations", allocations);
        spec.status = status;
        for (key, value) in rules.as_object().unwrap().iter().chain(texts.as_object().unwrap()) { spec = spec.with(key, value.clone()); }
        spec
    };
    let lone = common::seed::insert_bot(c, &seeded, &spec(5, json!({ seeded.btc.to_string(): 1.0 })));
    let pair = common::seed::insert_bot(c, &seeded, &spec(2, json!({ seeded.btc.to_string(): 0.5, eth.to_string(): 0.5 })));
    for (bot, asset, ticker) in [(lone, seeded.btc, seeded.ticker_id), (pair, seeded.btc, seeded.ticker_id), (pair, eth, eth_ticker)] {
        c.execute("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, created_at, updated_at) VALUES (?1, ?2, ?3, 0.5, 1, ?4, ?4)",
                  (bot, asset, ticker, "2026-01-01 00:00:00")).unwrap();
    }
    // The retrying bot's last order failed, and the venue's words for it are printed in the status bar and in the feed.
    c.execute("INSERT INTO transactions (bot_id, exchange_id, status, side, transaction_type, base, quote, base_asset_id, quote_asset_id, quote_amount, bot_interval, bot_quote_amount, \
               error_messages, created_at, updated_at) VALUES (?1, ?2, 1, 0, 'REGULAR', ?3, ?3, ?4, ?5, '60', 'week', 60, ?6, '2026-09-08 10:00:00', '2026-09-08 10:00:00')",
              (lone, seeded.exchange_id, HOSTILE, seeded.btc, seeded.quote, json!([HOSTILE]).to_string())).unwrap();
    c.execute("UPDATE assets SET symbol = ?1, name = ?1", [HOSTILE]).unwrap();
    c.execute("UPDATE tickers SET base = ?1 || id, quote = ?1, ticker = ?1 || id", [HOSTILE]).unwrap(); // a venue lists a spelling once
    c.execute("UPDATE exchanges SET name = ?1", [HOSTILE]).unwrap();
    c.execute("UPDATE bots SET label = ?1, stop_message_key = ?1", [HOSTILE]).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    (dir, opened, app, Browser::default(), [lone, pair])
}

/// How often the text is on a page escaped, in either spelling of the entities (the templates write `&#60;`, the code `&lt;`).
fn escaped(body: &str) -> usize {
    body.matches("&lt;img src=x onerror=alert(1)&gt;").count() + body.matches("&#60;img src=x onerror=alert(1)&#62;").count()
}

/// What a page must not contain, and must: the text as markup or closing an attribute, and the text escaped.
fn assert_escaped(what: &str, body: &str) {
    assert!(!body.contains("<img"), "{what}: a text from the database is printed as markup:\n{}", body.split("<img").next().unwrap_or_default().chars().rev().take(300).collect::<String>().chars().rev().collect::<String>());
    assert!(!body.contains("\" onmouseover=\"alert"), "{what}: a text from the database closes an attribute");
    assert!(escaped(body) > 0, "{what}: the text is not on the page at all, so nothing was proven");
}

/// No text out of the database is markup on the bot list or the bot page. Rails prints one as
/// markup (the lone subject of a condition's sentence: `base_html.html_safe` in
/// bots/settings/_price_limit.html.erb and its three siblings), so the parity grid cannot hold this:
/// it is held here, for every text these pages print, in every place they print it.
#[tokio::test(flavor = "current_thread")]
async fn no_text_from_the_database_is_markup_on_the_bot_pages() {
    let (_dir, _opened, app, mut browser, [lone, pair]) = hostile_install();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    for path in ["/bots".to_string(), format!("/bots/{lone}"), format!("/bots/{pair}"), format!("/de/bots/{lone}"), format!("/bots/{pair}/chart")] {
        let page = browser.get(&app, &path).await;
        assert_eq!(page.status, 200, "{path}: {}", page.body);
        if path.ends_with("/chart") { assert!(!page.body.contains("<img"), "{path}"); } else { assert_escaped(&path, &page.body); }
    }
    let page = browser.get(&app, &format!("/bots/{lone}")).await.body;
    assert!(escaped(&page) >= 10, "the symbol, the label, the venue and the settings are all on this page: {}", escaped(&page));
    // The one text Rails hands to a sentence as markup: the lone subject of a condition. No locale's sentence
    // for one member prints it today, so no page can show it; the function that makes it is held directly.
    use deltabadger::web::bot::settings::lone_subject;
    let escaped_text = "&lt;img src=x onerror=alert(1)&gt;&quot; onmouseover=&quot;alert(2)";
    assert_eq!(lone_subject(&[(HOSTILE.to_string(), "7".to_string())]).as_deref(), Some(escaped_text));
    assert_eq!(lone_subject(&[]).as_deref(), Some(""));
    assert_eq!(lone_subject(&[("A".to_string(), "1".to_string()), ("B".to_string(), "2".to_string())]), None, "a choice is a select, built with its labels escaped");
}

/// An Alpaca install with two one-asset bots that have a label and an account that answered the
/// wash-sale question, signed in: the first scheduled, the second stopped with a spending cap that
/// counts from the first of September.
async fn two_plain_bots() -> (tempfile::TempDir, deltabadger::store::Opened, common::seed::Seeded, deltabadger::web::App, Browser, [i64; 2]) {
    use serde_json::json;
    let (dir, opened, seeded) = common::install_alpaca();
    let c = &opened.primary;
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    c.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00', wash_sale_enabled = 0 WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    let working = common::seed::insert_bot(c, &seeded, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("last_action_job_at", json!("2026-09-08T10:00:00.250Z")));
    let mut capped = common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limited", json!(true)).with("quote_amount_limit", json!(1000))
        .transient("quote_amount_limit_enabled_at", json!("2026-09-01T00:00:00.000Z"));
    capped.status = 2;
    let stopped = common::seed::insert_bot(c, &seeded, &capped);
    c.execute("UPDATE bots SET label = 'Bitcoin ' || id", []).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    (dir, opened, seeded, app, browser, [working, stopped])
}

/// The span between two orders is checked before any checkpoint is computed from it. A working bot
/// whose Smart Intervals amount is nothing, less than nothing or small enough to underflow is
/// refused, on its page and on the list; the same row on a bot that is not working is rendered, with
/// Rails' own error under the field (the scenario `bot_page_stored_past_forms` holds that page to Rails').
#[tokio::test(flavor = "current_thread")]
async fn a_span_of_nothing_between_two_orders_refuses_a_working_bot_before_any_checkpoint() {
    use serde_json::json;
    let (_dir, opened, _seeded, app, mut browser, [working, stopped]) = two_plain_bots().await;
    let c = &opened.primary;
    let set = |bot: i64, amount: serde_json::Value| {
        let stored: String = c.query_row("SELECT settings FROM bots WHERE id = ?1", [bot], |r| r.get(0)).unwrap();
        let mut settings: serde_json::Value = serde_json::from_str(&stored).unwrap();
        (settings["smart_intervaled"], settings["smart_interval_quote_amount"]) = (json!(true), amount);
        c.execute("UPDATE bots SET settings = ?1 WHERE id = ?2", (settings.to_string(), bot)).unwrap();
    };
    for amount in [json!(0), json!(0.0), json!(-5), json!(1e-320), json!(0.00009)] {
        set(working, amount.clone());
        assert_eq!((browser.get(&app, &format!("/bots/{working}")).await.status, browser.get(&app, "/bots").await.status), (501, 501), "{amount}");
        assert_eq!(browser.get(&app, &format!("/bots/{stopped}")).await.status, 200, "{amount}: the other bot's page is not taken along");
    }
    set(working, json!(6));
    assert_eq!(browser.get(&app, "/bots").await.status, 200);
    for amount in [json!(0), json!(-5)] {
        set(stopped, amount.clone());
        let page = browser.get(&app, &format!("/bots/{stopped}")).await;
        assert_eq!(page.status, 200, "{amount}");
        assert!(page.body.contains("form__info--invalid") && page.body.contains("data-html5-range-underflow-message"), "{amount}: the floor's message under the field");
        assert_eq!(browser.get(&app, "/bots").await.status, 200, "{amount}");
    }
}

/// The navbar's ring adds the account's holdings in SQL, and a value no Float holds makes the sum
/// infinity: a user reaches it by importing a closed buy of 1e307 and syncing balances at 100 (Rails
/// stores the product; app/services/account_balance/sync.rb). Rails raises drawing the ring
/// (app/helpers/tracker_helper.rb:505). Every handler here that loads the navbar answers the 501
/// page for it, never a 500; and the same pages are served again once the row is gone.
#[tokio::test(flavor = "current_thread")]
async fn holdings_that_add_up_to_no_number_refuse_the_pages_that_draw_the_ring() {
    let (_dir, opened, seeded, app, mut browser, [working, stopped]) = two_plain_bots().await;
    let c = &opened.primary;
    let paths = ["/bots".to_string(), format!("/bots/{working}"), format!("/bots/{stopped}"), format!("/bots/{stopped}/chart")];
    for value in [f64::INFINITY, 12.5] {
        c.execute("INSERT INTO account_balances (asset_id, exchange_id, user_id, free, locked, usd_price, usd_value, synced_at, created_at, updated_at) \
                   VALUES (?1, ?2, ?3, '1', '0', '100', ?4, '2026-09-10 12:00:00', '2026-09-10 12:00:00', '2026-09-10 12:00:00')",
                  rusqlite::params![seeded.btc, seeded.exchange_id, seeded.user_id, value]).unwrap();
        let mut statuses = vec![];
        for path in &paths { statuses.push(browser.get(&app, path).await.status); }
        assert_eq!(statuses, if value.is_finite() { [200; 4] } else { [501; 4] }, "{value}");
        c.execute("DELETE FROM account_balances", []).unwrap();
    }
}

/// The exchange menu of a basket asks which venues list every member. The database counts it
/// (Bots::DcaMultiAsset#eligible_pairs), inside the one database call the whole server shares, so a
/// large catalogue must not make it slow: fifteen venues of 10,000 listings each and a basket of 100.
#[tokio::test(flavor = "current_thread")]
async fn the_exchange_menu_of_a_large_catalogue_is_counted_by_the_database() {
    use serde_json::json;
    let (dir, opened, seeded) = common::install_alpaca();
    let c = &opened.primary;
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    c.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00', wash_sale_enabled = 0 WHERE id = ?2", (hash, seeded.user_id)).unwrap();
    c.execute_batch("BEGIN").unwrap();
    let mut assets = vec![seeded.btc];
    for n in 1..10_000 {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Stock', '2026-01-01 00:00:00', '2026-01-01 00:00:00')", (format!("a{n}.us"), format!("A{n}"))).unwrap();
        assets.push(c.last_insert_rowid());
    }
    // Every other venue a user can pick: one row per class.
    let mut venues = vec![seeded.exchange_id];
    for class in ["Binance", "BinanceUs", "Bingx", "Bitget", "Bitrue", "Bitvavo", "Bybit", "Coinbase", "Gemini", "Hyperliquid", "Ibkr", "Kraken", "Kucoin", "Mexc"] {
        c.execute("INSERT INTO exchanges (type, name, available, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, ?2, 1, '0.05', '0.05', '2026-01-01 00:00:00', '2026-01-01 00:00:00')", (format!("Exchanges::{class}"), class)).unwrap();
        venues.push(c.last_insert_rowid());
    }
    for (position, venue) in venues.iter().enumerate() {
        for (number, asset) in assets.iter().enumerate() {
            // The bot's own venue has the first listing already. Every second other venue lacks the basket's hundredth member.
            if (position == 0 && number == 0) || (position % 2 == 1 && number == 99) { continue; }
            c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, minimum_base_size, \
                       minimum_quote_size, trading_enabled, available, created_at, updated_at) VALUES (?1, ?2, ?2, 'USD', ?3, ?4, 9, 2, 2, '0.001', '1', 1, 1, '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
                      (venue, format!("A{number}"), asset, seeded.quote)).unwrap();
        }
    }
    c.execute_batch("COMMIT").unwrap();
    let weights: serde_json::Map<String, serde_json::Value> = assets.iter().take(100).map(|asset| (asset.to_string(), json!(0.01))).collect();
    let mut stopped = common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("allocations", json!(weights));
    stopped.status = 2;
    let basket = common::seed::insert_bot(c, &seeded, &stopped);
    let other = common::seed::insert_bot(c, &seeded, &stopped);
    c.execute("UPDATE bots SET label = 'Basket'", []).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(NOW));
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    let started = Instant::now();
    let page = browser.get(&app, &format!("/bots/{basket}")).await;
    let took = started.elapsed();
    assert_eq!(page.status, 200, "{}", page.body);
    // The venues that list all hundred: seven of the other fourteen, each with its button.
    assert_eq!(page.body.matches("name=\"bots_dca_multi_asset[exchange_id]\"").count(), 7, "bots {basket} and {other}");
    assert!(took < Duration::from_secs(5), "a page over 150,000 listings took {took:?}");
}

/// The orders feed prints what the venue and the engine wrote: an order's symbols and its error, an
/// event's message and its details. None of it is markup.
#[tokio::test(flavor = "current_thread")]
async fn no_text_from_the_database_is_markup_in_the_orders_feed() {
    use serde_json::json;
    let (_dir, opened, app, mut browser, [lone, _pair]) = hostile_install();
    let c = &opened.primary;
    let (exchange, base, quote): (i64, i64, i64) = c.query_row("SELECT exchange_id, base_asset_id, quote_asset_id FROM transactions LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    // A filled order and a resting one beside the failed one of the fixture, and an event of every kind that prints a detail.
    for (external_status, at) in [(2, "2026-09-08 11:00:00"), (1, "2026-09-08 12:00:00")] {
        c.execute("INSERT INTO transactions (bot_id, exchange_id, status, external_status, side, transaction_type, base, quote, base_asset_id, quote_asset_id, price, amount, amount_exec, \
                   quote_amount, quote_amount_exec, bot_interval, bot_quote_amount, error_messages, created_at, updated_at) \
                   VALUES (?1, ?2, 0, ?3, 0, 'REGULAR', ?4, ?4, ?5, ?6, '100', '0.6', '0.6', '60', '60', 'week', 60, '[]', ?7, ?7)", (lone, exchange, external_status, HOSTILE, base, quote, at)).unwrap();
    }
    let details = json!({ "error": HOSTILE, "reason": HOSTILE, "bases": HOSTILE, "base": HOSTILE, "until": HOSTILE, "order_id": HOSTILE, "ratio": HOSTILE, "limit_type": HOSTILE,
                          "source_label": HOSTILE, "source_labels": [HOSTILE, HOSTILE], "stop_message_key": HOSTILE, "next_market_open_at": HOSTILE, "count": 2 }).to_string();
    for (second, event) in ["market_closed", "merged", "split", "limit_paused", "execution_failed", "stopped", "liquidation_failed", HOSTILE].into_iter().enumerate() {
        c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, ?2, 0, ?3, ?4)", (lone, event, &details, format!("2026-09-09 10:00:0{second}"))).unwrap();
    }
    c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, message, details, created_at) VALUES (?1, 'started', 0, ?2, '{}', '2026-09-09 11:00:00')", (lone, HOSTILE)).unwrap();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    let frame = [("turbo-frame", "orders_pagination")];
    let first = browser.send(&app, "GET", &format!("/bots/{lone}.turbo_stream"), None, web::Csrf::None, &frame).await;
    assert_eq!(first.status, 200, "{}", first.body);
    assert_escaped("the first ten rows", &first.body);
    // The frame's next address carries the cursor, and with it the rest of the twelve rows.
    let next = first.body.split("src=\"").nth(1).and_then(|rest| rest.split('"').next()).expect("more rows than one page").replace("&amp;", "&");
    let second = browser.send(&app, "GET", &next, None, web::Csrf::None, &frame).await;
    assert_eq!(second.status, 200, "{next}: {}", second.body);
    assert_escaped("the rest", &second.body);
    assert!(escaped(&first.body) + escaped(&second.body) >= 14, "every row prints the text: {} and {}", escaped(&first.body), escaped(&second.body));
}

/// A number in a row that this build does not read refuses the pages that would print or add it
/// (501), and nothing else: not the other bot's page, not the server. Rails reads such a row and
/// writes the number out; here its length is what an allocation would be sized by, inside the one
/// database call every request shares. Each is written past the models, by SQL.
#[tokio::test(flavor = "current_thread")]
async fn a_stored_number_this_build_does_not_read_refuses_the_bots_pages_and_only_those() {
    let (_dir, opened, seeded, app, mut browser, [working, stopped]) = two_plain_bots().await;
    let c = &opened.primary;
    let frame = [("turbo-frame", "orders_pagination")];
    let paths = [format!("/bots/{working}"), format!("/bots/{stopped}"), "/bots".to_string(), format!("/bots/{working}/chart"), format!("/bots/{stopped}/chart")];
    let statuses = async |browser: &mut Browser| {
        let mut statuses = vec![];
        for path in &paths { statuses.push(browser.get(&app, path).await.status); }
        for bot in [working, stopped] { statuses.push(browser.send(&app, "GET", &format!("/bots/{bot}.turbo_stream"), None, web::Csrf::None, &frame).await.status); }
        statuses
    };
    assert_eq!(statuses(&mut browser).await, [200; 7], "as the install stands, everything is served");
    let order = |bot: i64, external_status: i64, column: &str, value: &dyn rusqlite::ToSql| {
        c.execute(&format!("INSERT INTO transactions (bot_id, exchange_id, status, external_status, side, transaction_type, base, quote, base_asset_id, quote_asset_id, {column}, bot_interval, \
                            bot_quote_amount, error_messages, created_at, updated_at) VALUES (?1, ?2, 0, ?3, 0, 'REGULAR', 'BTC', 'USD', ?4, ?5, ?6, 'week', 60, '[]', '2026-09-08 10:00:00', '2026-09-08 10:00:00')"),
                  rusqlite::params![bot, seeded.exchange_id, external_status, seeded.btc, seeded.quote, value]).unwrap();
    };
    // What the stopped bot spent under its cap: its page and the list add it up, its feed prints it, its chart reads no amount.
    // A closed order (2) at what it cost, and a cancelled (3) and an abandoned one (4) at what they filled before: Rails adds the
    // last two as SQLite's own numbers, and each is read as a stored number first, whatever SQLite holds it as: a text it could
    // not read as a number, or the infinity it makes of a literal too large for a Float.
    let infinity = f64::INFINITY;
    let unread: [(&str, &dyn rusqlite::ToSql); 3] = [("a text with an underscore", &"1_0e1000000000"), ("a text beyond a Float", &"1e1000000000"), ("the Float infinity", &infinity)];
    for external_status in [2, 3, 4] {
        for (what, value) in unread {
            order(stopped, external_status, "quote_amount_exec", value);
            assert_eq!(statuses(&mut browser).await, [200, 501, 501, 200, 200, 200, 501], "{what}, external status {external_status}");
            // Beside an order that filled what a decimal says: the sum is not attempted either.
            order(stopped, 2, "quote_amount_exec", &"10.5");
            assert_eq!(statuses(&mut browser).await, [200, 501, 501, 200, 200, 200, 501], "{what}, external status {external_status}, beside a fill");
            c.execute("DELETE FROM transactions", []).unwrap();
        }
    }
    // A cancelled order that filled a number SQLite holds as an Integer or a Float is added as that, and the page is served.
    for filled in [&7_i64 as &dyn rusqlite::ToSql, &7.25_f64] {
        order(stopped, 3, "quote_amount_exec", filled);
        assert_eq!(statuses(&mut browser).await, [200; 7]);
        c.execute("DELETE FROM transactions", []).unwrap();
    }
    // A price of the working bot's order, as the Float SQLite makes of a literal too large for one: only its feed prints it.
    order(working, 2, "price", &f64::INFINITY);
    assert_eq!(statuses(&mut browser).await, [200, 200, 200, 200, 200, 501, 200]);
    c.execute("DELETE FROM transactions", []).unwrap();
    // The venue's ticker, which both bots read: every page of both, and nothing is rounded or sized by it.
    for (column, value, was) in [("minimum_quote_size", "1_0e1000000000", "1"), ("base_decimals", "41", "9"), ("quote_decimals", "255", "2")] {
        c.execute(&format!("UPDATE tickers SET {column} = ?1"), [value]).unwrap();
        assert_eq!(statuses(&mut browser).await, [501; 7], "{column} = {value}");
        c.execute(&format!("UPDATE tickers SET {column} = ?1"), [was]).unwrap();
    }
    assert_eq!(statuses(&mut browser).await, [200; 7], "and with the rows as they were, everything is served again");
    assert_eq!(browser.get(&app, "/up").await.status, 200);
}

/// A page of the feed costs what its ten rows cost, however long the bot's history is. The refusal
/// that runs before every page asks nothing of the bot's orders, and the bot is loaded without the
/// facts about its orders that walk the history. Measured as a ratio, on one machine at one time: thirty pages of a history of
/// 300,000 orders against thirty pages of a history of 400. A walk of the history for every page
/// makes the long one many times slower; the bound is four times and a fifth of a second.
#[tokio::test(flavor = "current_thread")]
async fn a_page_of_the_feed_costs_the_same_however_long_the_history_is() {
    let (_dir, opened, seeded, app, mut browser, [long, _]) = two_plain_bots().await;
    let c = &opened.primary;
    let short = common::seed::insert_bot(c, &seeded, &common::seed::BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    c.execute("UPDATE bots SET label = 'Bitcoin ' || id", []).unwrap();
    // Buys only, a minute apart, all closed: the history in which no order answers the refusal's questions early.
    for (bot, orders) in [(long, 300_000), (short, 400)] {
        c.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?3) \
                   INSERT INTO transactions (bot_id, exchange_id, status, external_status, side, transaction_type, base, quote, base_asset_id, quote_asset_id, price, amount, amount_exec, \
                   quote_amount, quote_amount_exec, bot_interval, bot_quote_amount, error_messages, created_at, updated_at) \
                   SELECT ?1, ?2, 0, 2, 0, 'REGULAR', 'BTC', 'USD', ?4, ?5, '100', '0.6', '0.6', '60', '60', 'week', 60, '[]', datetime('2025-01-01', '+' || i || ' minutes'), '2026-01-01 00:00:00' FROM n",
                  (bot, seeded.exchange_id, orders, seeded.btc, seeded.quote)).unwrap();
    }
    let frame = [("turbo-frame", "orders_pagination")];
    // Thirty pages from the newest order on, each asked for at the address the page before it gave.
    let mut walk = async |bot: i64| {
        let (mut next, mut rows, started) = (format!("/bots/{bot}.turbo_stream"), 0, Instant::now());
        for _ in 0..30 {
            let page = browser.send(&app, "GET", &next, None, web::Csrf::None, &frame).await;
            assert_eq!(page.status, 200, "{next}: {}", page.body);
            rows += page.body.matches("<tr id=\"transaction_").count();
            next = page.body.split("src=\"").nth(1).and_then(|rest| rest.split('"').next()).expect("a next page").replace("&amp;", "&");
        }
        assert_eq!(rows, 300, "ten orders a page");
        started.elapsed()
    };
    // The better of two walks each, so that one stall of a busy machine is not the measurement.
    let (mut quick, mut slow) = (Duration::MAX, Duration::MAX);
    for _ in 0..2 {
        quick = quick.min(walk(short).await);
        slow = slow.min(walk(long).await);
    }
    assert!(slow < quick * 4 + Duration::from_millis(200), "thirty pages of 300,000 orders took {slow:?}, of 400 orders {quick:?}");
    // And a sell at the far end of the history is found by the page of rows that holds it, not before.
    c.execute("UPDATE transactions SET side = 1 WHERE bot_id = ?1 AND created_at = datetime('2025-01-01', '+5 minutes')", [short]).unwrap();
    assert_eq!(browser.send(&app, "GET", &format!("/bots/{short}.turbo_stream"), None, web::Csrf::None, &frame).await.status, 200);
    let cursor = "2025-01-01T00:10:30.000000Z%7Ctransaction%7C0";
    assert_eq!(browser.send(&app, "GET", &format!("/bots/{short}.turbo_stream?before={cursor}"), None, web::Csrf::None, &frame).await.status, 200, "a sale is a row like any other");
    assert_eq!(browser.get(&app, &format!("/bots/{short}")).await.status, 501, "and the bot's page, which asks once");
}

/// The feed refuses the page of rows that holds an order it cannot read, and no page before it; a
/// sale or an order of another type is a row like any other. Each table is asked for eleven rows and
/// ten are shown (Rails picks the merged page first: BotActivityFeed#page), so the row that only
/// says "there is a next page" must not decide anything about this one.
#[tokio::test(flavor = "current_thread")]
async fn the_feed_refuses_the_page_that_holds_the_row_and_no_page_before_it() {
    let (_dir, opened, seeded, app, mut browser, [bot, _]) = two_plain_bots().await;
    let c = &opened.primary;
    let frame = [("turbo-frame", "orders_pagination")];
    let typed = |minute: u32, side: i64, kind: &str, amount: &str| {
        c.execute("INSERT INTO transactions (bot_id, exchange_id, status, external_status, side, transaction_type, base, quote, base_asset_id, quote_asset_id, price, amount, amount_exec, \
                   quote_amount, quote_amount_exec, bot_interval, bot_quote_amount, error_messages, created_at, updated_at) \
                   VALUES (?1, ?2, 0, 2, ?3, ?4, 'BTC', 'USD', ?5, ?6, '100', ?7, ?7, '60', '60', 'week', 60, '[]', ?8, ?8)",
                  rusqlite::params![bot, seeded.exchange_id, side, kind, seeded.btc, seeded.quote, amount, format!("2026-09-08 10:{minute:02}:00")]).unwrap();
    };
    let order = |minute: u32, side: i64, amount: &str| typed(minute, side, "REGULAR", amount);
    let event = |minute: u32| {
        c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, 'started', 0, '{}', ?2)", (bot, format!("2026-09-08 10:{minute:02}:00"))).unwrap();
    };
    let mut two_pages = async |what: &str, rows: &str, held: u16| {
        let first = browser.send(&app, "GET", &format!("/bots/{bot}.turbo_stream"), None, web::Csrf::None, &frame).await;
        assert_eq!((first.status, first.body.matches(rows).count()), (200, 10), "{what}: the first page shows its ten rows");
        let next = first.body.split("src=\"").nth(1).and_then(|rest| rest.split('"').next()).expect("a next page").replace("&amp;", "&");
        assert_eq!(browser.send(&app, "GET", &next, None, web::Csrf::None, &frame).await.status, held, "{what}: the page that holds the row");
        c.execute_batch("DELETE FROM transactions; DELETE FROM bot_activity_logs;").unwrap();
    };
    // Ten buys, and a sell before them: the eleventh order.
    order(0, 1, "0.6");
    for minute in 1..=10 { order(minute, 0, "0.6"); }
    two_pages("a sell after ten buys", "<tr id=\"transaction_", 200).await;
    // Ten events, and a sell before them: the first order, and on the next page.
    order(0, 1, "0.6");
    for minute in 1..=10 { event(minute); }
    two_pages("a sell after ten events", "<tr id=\"bot_activity_log_", 200).await;
    // Ten buys, and before them a liquidation that completed: an order of another type, which only the bot's page asks about beforehand.
    typed(0, 0, "LIQUIDATION", "0.6");
    for minute in 1..=10 { order(minute, 0, "0.6"); }
    two_pages("an older liquidation after ten buys", "<tr id=\"transaction_", 200).await;
    // Ten buys, and before them a buy whose amount this build does not read.
    order(0, 0, "1_0e1000000000");
    for minute in 1..=10 { order(minute, 0, "0.6"); }
    two_pages("an amount that is not read, after ten buys", "<tr id=\"transaction_", 501).await;
}

/// The scripted CLI example, built here so a run of only this test target still has it (a no-op when it is current).
fn bot_action_executable() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let built = std::process::Command::new(env!("CARGO"))
        .args(["build", "--quiet", "--example", "scripted_cli", "--manifest-path", concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")])
        .status()?;
    if !built.success() { return Err("cargo build --example scripted_cli failed".into()); }
    let executable = std::env::current_exe()?;
    let profile = executable
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("test executable profile directory")?;
    Ok(profile.join("examples").join(format!(
        "scripted_cli{}",
        std::env::consts::EXE_SUFFIX
    )))
}

#[test]
fn bot_action_executable_smoke() -> Result<(), Box<dyn std::error::Error>> {
    use common::seed;
    use serde_json::{json, Value};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    };
    let (dir, opened, seeded) = common::install_alpaca();
    let c = &opened.primary;
    c.busy_timeout(Duration::from_secs(5))?;
    let hash =
        deltabadger::crypto::hash_password("Correct-horse-9").map_err(|e| format!("{e:?}"))?;
    c.execute(
        "UPDATE users SET encrypted_password=?1,confirmed_at=created_at,wash_sale_enabled=0",
        [hash],
    )?;
    let mut spec = seed::BotSpec::weekly(5.0, "2026-09-01 10:00:00");
    spec.status = 2;
    let crypto = seed::insert_bot(c, &seeded, &spec);
    let (stock_asset, _) = seed::add_eth_sol(c, &seeded);
    c.execute(
        "UPDATE assets SET category='Stock' WHERE id=?1",
        [stock_asset],
    )?;
    let stock = seed::insert_bot(
        c,
        &seeded,
        &seed::BotSpec {
            settings: json!({"interval":"week","quote_amount":5,"allocations":{stock_asset.to_string():1.0}}),
            ..seed::BotSpec::weekly(5.0, "2026-09-01 10:00:00")
        },
    );
    let index = seed::insert_bot(
        c,
        &seeded,
        &spec
            .with("index_type", json!("top"))
            .with("num_coins", json!(2)),
    );
    c.execute("UPDATE bots SET status=2,label='Smoke '||id", [])?;
    c.execute("UPDATE bots SET type='Bots::DcaIndex' WHERE id=?1", [index])?;
    for (key, value) in [
        ("market_data_provider", "deltabadger"),
        ("market_data_url", "http://example.test"),
        ("market_data_token", "test"),
    ] {
        c.execute("INSERT INTO app_configs(key,value,created_at,updated_at) VALUES(?1,?2,'2026-01-01','2026-01-01')",(key,seed::cipher().encrypt(value)))?;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    listener.set_nonblocking(true)?;
    let scratch = tempfile::tempdir()?;
    let binary = bot_action_executable()?;
    let stopping = Arc::new(AtomicBool::new(false));
    let done = stopping.clone();
    let submissions = Arc::new(Mutex::new(Vec::<Value>::new()));
    let calls = submissions.clone();
    let (entered, entry) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker = std::thread::spawn(move || -> Result<(), String> {
        while !done.load(Ordering::SeqCst) {
            let (mut socket, _) = match listener.accept() {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => return Err(e.to_string()),
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(20)))
                .map_err(|e| e.to_string())?;
            socket
                .set_write_timeout(Some(Duration::from_secs(20)))
                .map_err(|e| e.to_string())?;
            let mut line = String::new();
            std::io::BufRead::read_line(&mut std::io::BufReader::new(&mut socket), &mut line)
                .map_err(|e| e.to_string())?;
            let r: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            let body = match (r["method"].as_str(), r["path"].as_str()) {
                (Some("GET"), Some("/v1beta3/crypto/us/latest/quotes")) => {
                    json!({"quotes":{"BTC/USD":{"ap":64000}}})
                }
                (Some("GET"), Some("/v2/account")) => {
                    json!({"cash":"100000","non_marginable_buying_power":"100000"})
                }
                (Some("GET"), Some("/v2/positions")) => json!([]),
                (Some("POST"), Some("/v2/orders")) => {
                    calls
                        .lock()
                        .map_err(|e| e.to_string())?
                        .push(r["body"].clone());
                    entered.send(()).map_err(|e| e.to_string())?;
                    released
                        .recv_timeout(Duration::from_secs(20))
                        .map_err(|e| format!("order transmitted, waiting for HTTP Stop: {e}"))?;
                    json!({"id":"SMOKE-1","status":"pending_new"})
                }
                (Some("GET"), Some("/v2/orders/SMOKE-1")) => {
                    json!({"id":"SMOKE-1","status":"filled","symbol":"BTC/USD","type":"market","side":"buy","notional":"7","qty":null,"filled_qty":"0.000109375","filled_avg_price":"64000","limit_price":null})
                }
                _ => return Err(format!("unscripted venue request: {r}")),
            };
            socket
                .write_all(body.to_string().as_bytes())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    });
    struct ScriptGuard {
        stop: Arc<AtomicBool>,
        release: mpsc::Sender<()>,
        thread: Option<std::thread::JoinHandle<Result<(), String>>>,
    }
    impl Drop for ScriptGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = self.release.send(());
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
    let mut script = ScriptGuard {
        stop: stopping,
        release: release.clone(),
        thread: Some(worker),
    };
    let port = free_port();
    let log = std::fs::File::create(scratch.path().join("serve.log"))?;
    let command = serve_command(dir.path(), port);
    // Preserve the production command's environment sanitization and all CLI code.
    let mut scripted = Command::new(&binary);
    scripted.args(command.get_args());
    scripted.env("VENUE_ADDRESS", address.to_string());
    for (key, value) in command.get_envs() {
        match value {
            Some(v) => {
                scripted.env(key, v);
            }
            None => {
                scripted.env_remove(key);
            }
        }
    }
    let mut child = Child(
        scripted
            .env("SECRET_KEY_BASE", "engine-test-secret")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !up_within(port, Duration::from_secs(1)) {
        if child.0.try_wait()?.is_some() || Instant::now() >= deadline {
            return Err(std::fs::read_to_string(scratch.path().join("serve.log"))?.into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (_, cookie, login) = http(port, "GET", "/login", None, None);
    let token = web::form_token(&login, "/login").ok_or("login token")?;
    let form = form_urlencoded::Serializer::new(String::new())
        .append_pair("authenticity_token", &token)
        .append_pair("user[email]", "o@example.com")
        .append_pair("user[password]", "Correct-horse-9")
        .finish();
    let (status, signed_in, body) = http(port, "POST", "/login", cookie.as_deref(), Some(&form));
    assert_eq!(status, 303, "{body}");
    let cookie = signed_in.or(cookie).ok_or("signed-in cookie")?;
    let (_, page_cookie, page) = http(port, "GET", &format!("/bots/{crypto}"), Some(&cookie), None);
    let cookie = page_cookie.unwrap_or(cookie);
    let token = web::meta_token(&page).ok_or("rotated CSRF token")?;
    let action = |method: &str,
                  id: i64,
                  suffix: &str,
                  fields: &[(&str, &str)]|
     -> Result<(u16, String), Box<dyn std::error::Error>> {
        let form = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields.iter().copied())
            .finish();
        let request=format!("{method} /bots/{id}{suffix} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nCookie: {cookie}\r\nX-CSRF-Token: {token}\r\nAccept: text/vnd.turbo-stream.html\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{form}",form.len());
        let response = exchange(port, &request, Instant::now() + Duration::from_secs(20))
            .ok_or("action HTTP deadline")?;
        let status = response
            .split_whitespace()
            .nth(1)
            .ok_or("HTTP status")?
            .parse()?;
        Ok((status, response))
    };
    let edited = action(
        "PATCH",
        crypto,
        "",
        &[("bots_dca_multi_asset[quote_amount]", "7")],
    )?;
    assert_eq!(edited.0, 200, "{}", edited.1);
    assert_eq!(
        c.query_row(
            "SELECT json_extract(settings,'$.quote_amount') FROM bots WHERE id=?1",
            [crypto],
            |r| r.get::<_, f64>(0)
        )?,
        7.0
    );
    let started = action("PATCH", crypto, "/start", &[])?;
    assert_eq!(started.0, 200, "{}", started.1);
    entry.recv_timeout(Duration::from_secs(20)).map_err(|e| {
        format!(
            "HTTP Start committed; engine never submitted: {e}; {}",
            std::fs::read_to_string(scratch.path().join("serve.log")).unwrap_or_default()
        )
    })?;
    {
        let orders = submissions.lock().map_err(|e| e.to_string())?;
        assert_eq!(orders.len(), 1);
        let order = orders.first().ok_or("submission")?;
        assert_eq!(order["symbol"], "BTC/USD");
        assert_eq!(order["notional"], "7.00");
        assert_eq!(order["side"], "buy");
        assert!(order["client_order_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
    }
    assert_eq!(action("PATCH", crypto, "/stop", &[])?.0, 200);
    release.send(())?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let rows: i64 = c.query_row(
            "SELECT count(*) FROM transactions WHERE bot_id=?1 AND external_status=2",
            [crypto],
            |r| r.get(0),
        )?;
        if rows == 1 {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Stop committed and venue released; fill did not reconcile: {}",
                std::fs::read_to_string(scratch.path().join("serve.log"))?
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        c.query_row("SELECT status FROM bots WHERE id=?1", [crypto], |r| r
            .get::<_, i64>(0))?,
        2
    );
    let snapshot = || -> Result<Vec<String>, rusqlite::Error> {
        let mut out = vec![];
        for table in [
            "bots",
            "bot_index_assets",
            "bot_activity_logs",
            "transactions",
            "api_keys",
            "users",
        ] {
            let mut query = c.prepare(&format!("SELECT * FROM {table} ORDER BY id"))?;
            let columns = query.column_count();
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                let values = (0..columns)
                    .map(|i| row.get_ref(i).map(|v| format!("{v:?}")))
                    .collect::<Result<Vec<_>, _>>()?;
                out.push(format!("{table}:{values:?}"));
            }
        }
        Ok(out)
    };
    for bot in [stock, index] {
        let before = snapshot()?;
        let refused = action("PATCH", bot, "/start", &[])?;
        assert_eq!(refused.0, 422, "{}", refused.1);
        assert!(refused.1.contains("run that yet"), "{}", refused.1);
        assert_eq!(snapshot()?, before);
        assert!(up_within(port, Duration::from_secs(2)) && child.0.try_wait()?.is_none());
    }
    for (method, suffix, status) in [
        ("POST", "/archive", 7),
        ("DELETE", "/archive", 2),
        ("DELETE", "/delete", 3),
    ] {
        let answer = action(method, crypto, suffix, &[])?;
        assert_eq!(answer.0, 200, "{}", answer.1);
        if suffix == "/delete" {
            assert!(answer.1.contains("redirect") && answer.1.contains("/bots"));
        }
        assert_eq!(
            c.query_row("SELECT status FROM bots WHERE id=?1", [crypto], |r| r
                .get::<_, i64>(0))?,
            status
        );
    }
    assert_eq!(submissions.lock().map_err(|e| e.to_string())?.len(), 1);
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM transactions WHERE bot_id=?1 AND external_status=2",
            [crypto],
            |r| r.get::<_, i64>(0)
        )?,
        1
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM bot_activity_logs WHERE bot_id=?1 AND event='started'",
            [crypto],
            |r| r.get::<_, i64>(0)
        )?,
        1
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM bot_activity_logs WHERE bot_id=?1 AND event='stopped'",
            [crypto],
            |r| r.get::<_, i64>(0)
        )?,
        2
    );
    let check_log = std::fs::File::create(scratch.path().join("check.log"))?;
    let mut check = Child(
        Command::new(env!("CARGO_BIN_EXE_deltabadger"))
            .arg("check")
            .env("STORAGE_DIR", dir.path())
            .stdout(check_log.try_clone()?)
            .stderr(check_log)
            .spawn()?,
    );
    assert!(!check
        .ended_within(Duration::from_secs(20))
        .ok_or("lease check deadline")?
        .success());
    assert!(std::fs::read_to_string(scratch.path().join("check.log"))?
        .contains("another Deltabadger engine"));
    // Merged basket support means a basket alone is no longer an engine refusal.
    c.execute("UPDATE bots SET status=1,settings=json_set(settings,'$.weighting','market_cap') WHERE id=?1",[stock])?;
    // A refused write cannot wake the loop; its next bounded idle pass must end both tasks.
    assert_eq!(
        action(
            "PATCH",
            index,
            "",
            &[("bots_dca_index[label]", "supervision")]
        )?
        .0,
        422
    );
    let ended = child
        .ended_within(Duration::from_secs(100))
        .ok_or("engine supervision deadline")?;
    assert!(!ended.success());
    assert!(!up_within(port, Duration::from_secs(2)));
    let supervision_log = std::fs::read_to_string(scratch.path().join("serve.log"))?;
    assert!(
        supervision_log.contains("a write to bots skipped eligibility::guard")
            && supervision_log.contains("weighting market_cap"),
        "{ended}: {supervision_log}"
    );
    drop(release);
    script.stop.store(true, Ordering::SeqCst);
    script
        .thread
        .take()
        .ok_or("script worker")?
        .join()
        .map_err(|_| "script panic")?
        .map_err(|e| format!("venue: {e}"))?;
    Ok(())
}
