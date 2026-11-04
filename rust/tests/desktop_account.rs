//! PLAN-B6-3 §2 (B6-3a): the desktop's one account, signed in by the launcher, the pages it does not serve, and the
//! code that signs a browser in. Everything here is `DELTABADGER_PLATFORM=desktop`; the last tests pin that nothing
//! changes with the platform unset.
mod common;
use common::web::{app_in, Answer, Browser, Csrf, TestClock, SECRET};
use deltabadger::web::{self, desktop, App};
use serde_json::{json, Map, Value};
use std::sync::Arc;

const LAUNCHER: &str = "0123456789abcdef0123456789abcdef";
const NOW: &str = "2026-11-04T12:00:00Z";

/// An install as `db:prepare` leaves it (the exported schema), with no users.
fn install() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (file, schema) in [("production.sqlite3", include_str!("fixtures/settings_primary_schema.sql")), ("production_queue.sqlite3", include_str!("fixtures/settings_queue_schema.sql"))] {
        let c = rusqlite::Connection::open(dir.path().join(file)).unwrap();
        c.execute_batch(&format!("BEGIN IMMEDIATE;\n{schema}\nCOMMIT;")).unwrap();
    }
    dir
}

fn db(dir: &tempfile::TempDir) -> rusqlite::Connection { rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap() }

fn desktop_env(launcher: Option<&str>) -> Map<String, Value> { desktop_env_in(launcher, "en-GB") }

fn desktop_env_in(launcher: Option<&str>, locale: &str) -> Map<String, Value> {
    let mut env = Map::new();
    env.insert("DELTABADGER_PLATFORM".into(), json!("desktop"));
    env.insert("DELTABADGER_DESKTOP_LOCALE".into(), json!(locale));
    env.insert("DELTABADGER_DESKTOP_TIME_ZONE".into(), json!("Europe/Warsaw"));
    if let Some(l) = launcher { env.insert("DELTABADGER_DESKTOP_LAUNCHER".into(), json!(l)); }
    env
}

/// The desktop app on `dir`, its account made as `serve` makes it after the takeover.
async fn desktop_app(dir: &tempfile::TempDir, clock: Arc<TestClock>) -> App {
    let app = app_in(dir.path(), SECRET, None, &desktop_env(Some(LAUNCHER)), clock);
    desktop::prepare(&app).await.unwrap();
    app
}

async fn ticket(app: &App, header: Option<&str>) -> Answer {
    let mut b = Browser::default();
    let headers: Vec<(&str, &str)> = header.map(|h| vec![(desktop::LAUNCHER_HEADER, h)]).unwrap_or_default();
    b.send_body(app, "POST", "/desktop/ticket", None, Csrf::None, &headers).await
}

fn ticket_value(answer: &Answer) -> String {
    let body: Value = serde_json::from_str(&answer.body).unwrap_or_else(|_| panic!("not JSON: {}", answer.body));
    body["ticket"].as_str().unwrap().to_string()
}

/// A browser signed in the way the launcher signs the window in.
async fn signed_in_window(app: &App) -> Browser {
    let t = ticket_value(&ticket(app, Some(LAUNCHER)).await);
    let mut b = Browser::default();
    let answer = b.get(app, &format!("/desktop/sign_in?ticket={t}")).await;
    assert_eq!(answer.status, 302, "{}", answer.body);
    b
}

async fn dashboard_signed_in(app: &App, b: &mut Browser) -> bool {
    let answer = b.get(app, "/bots").await;
    answer.status == 200
}

/// id, email, admin, locale, time zone, locked_at, failed attempts, confirmed.
type UserRow = (i64, String, bool, Option<String>, String, Option<String>, i64, bool);

fn users(dir: &tempfile::TempDir) -> Vec<UserRow> {
    let c = db(dir);
    let mut s = c.prepare("SELECT id, email, admin, locale, time_zone, locked_at, failed_attempts, confirmed_at IS NOT NULL FROM users ORDER BY id").unwrap();
    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))).unwrap().map(Result::unwrap).collect()
}

fn insert_user(dir: &tempfile::TempDir, email: &str, admin: bool) -> i64 {
    let c = db(dir);
    c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES (?1, ?2, 'Someone', ?3, ?4, ?4)",
              (email, deltabadger::crypto::hash_password("Correct-horse-9").unwrap(), admin, "2026-01-01 00:00:00")).unwrap();
    c.last_insert_rowid()
}

// ---- §2.1 the account ----

#[tokio::test]
async fn a_fresh_install_gets_one_admin_with_the_os_language_and_zone_and_the_window_is_signed_in() {
    let dir = install();
    let app = app_in(dir.path(), SECRET, None, &desktop_env_in(Some(LAUNCHER), "pl-PL"), TestClock::at(NOW));
    desktop::prepare(&app).await.unwrap();
    let rows = users(&dir);
    assert_eq!(rows.len(), 1);
    let (_, email, admin, locale, zone, locked, failed, confirmed) = rows[0].clone();
    assert_eq!((email.as_str(), admin, locale.as_deref(), zone.as_str(), locked, failed, confirmed),
               ("desktop@deltabadger.invalid", true, Some("pl"), "Warsaw", None, 0, true));
    // A second start keeps the one row.
    desktop::prepare(&app).await.unwrap();
    assert_eq!(users(&dir).len(), 1);
    let mut b = signed_in_window(&app).await;
    assert!(dashboard_signed_in(&app, &mut b).await);
    // No form anywhere: the setup gate never fires, the root is the dashboard.
    assert_ne!(b.get(&app, "/").await.header("location"), Some("/setup"));
}

#[test]
fn the_placeholder_email_passes_setups_own_checks() {
    let dir = install();
    let c = db(&dir);
    assert!(web::setup::validate(&c, "en", Some("Deltabadger"), desktop::EMAIL, Some("Correct-horse-9")).unwrap().is_empty());
}

#[test]
fn the_os_language_and_zone_map_to_the_apps_or_the_defaults() {
    for (os, want) in [("pl-PL", "pl"), ("pl", "pl"), ("pt_BR", "pt"), ("de-AT", "de"), ("zh-Hans-CN", "en"), ("", "en"), ("C", "en")] {
        assert_eq!(desktop::map_locale(os), want, "{os}");
    }
    for (iana, want) in [("Europe/Warsaw", "Warsaw"), ("America/New_York", "Eastern Time (US & Canada)"), ("Etc/Nowhere", "UTC"), ("", "UTC")] {
        assert_eq!(desktop::map_zone(iana), want, "{iana}");
    }
    // Several Rails names share one zone: always the same one.
    assert_eq!(desktop::map_zone("Asia/Kolkata"), desktop::map_zone("Asia/Kolkata"));
}

#[tokio::test]
async fn a_2x_admin_with_two_factor_locked_and_unconfirmed_is_the_account_and_stays_signed_in() {
    let dir = install();
    let other = insert_user(&dir, "user@example.com", false);
    let admin = insert_user(&dir, "owner@example.com", true);
    db(&dir).execute("UPDATE users SET otp_module = 1, locked_at = ?1, failed_attempts = 5, confirmed_at = NULL WHERE id = ?2", ("2026-11-04 11:59:00", admin)).unwrap();
    let clock = TestClock::at(NOW);
    let app = desktop_app(&dir, clock.clone()).await;
    assert_eq!(app.desktop_account(), Some(admin));
    let row = users(&dir).into_iter().find(|u| u.0 == admin).unwrap();
    assert_eq!((row.5, row.6, row.7), (None, 0, true), "unlocked and confirmed");
    // The email and the other user stay as they were.
    assert_eq!(row.1, "owner@example.com");
    assert!(users(&dir).iter().any(|u| u.0 == other && !u.2));
    let mut b = signed_in_window(&app).await;
    assert!(dashboard_signed_in(&app, &mut b).await);
    // A day later, still signed in: nothing locks or unconfirms it.
    clock.set(common::web::at("2026-11-05T12:00:00Z"));
    assert!(dashboard_signed_in(&app, &mut b).await);
}

#[tokio::test]
async fn without_an_admin_the_lowest_user_is_promoted() {
    let dir = install();
    let first = insert_user(&dir, "a@example.com", false);
    insert_user(&dir, "b@example.com", false);
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    assert_eq!(app.desktop_account(), Some(first));
    assert!(users(&dir)[0].2, "promoted to admin");
    assert_eq!(users(&dir).len(), 2);
}

#[tokio::test]
async fn another_users_session_reads_as_signed_out() {
    let dir = install();
    let admin = insert_user(&dir, "owner@example.com", true);
    let other = insert_user(&dir, "user@example.com", false);
    // Confirmed and unlocked: only the desktop's own check can sign it out.
    db(&dir).execute("UPDATE users SET confirmed_at = '2026-01-01 00:00:00' WHERE id = ?1", [other]).unwrap();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    assert_eq!(app.desktop_account(), Some(admin));
    // A cookie for the other account, as a 2.x browser could still hold.
    let hash: String = db(&dir).query_row("SELECT encrypted_password FROM users WHERE id = ?1", [other], |r| r.get(0)).unwrap();
    let data = web::session::SessionData { user: Some((other, hash[..29].to_string())), ..Default::default() };
    let mut b = Browser { cookie: Some(web::session::seal(&app.keys.session, &data, common::web::at(NOW))), page: None };
    let answer = b.get(&app, "/bots").await;
    assert_eq!((answer.status, answer.header("location")), (302, Some("/login")));
}

// ---- §1 and §2.2 the launcher secret and the tickets ----

#[tokio::test]
async fn a_ticket_needs_the_launcher_header_and_sets_no_cookie() {
    let dir = install();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    for header in [None, Some(""), Some("0123456789abcdef0123456789abcdee"), Some("0123456789abcdef")] {
        let answer = ticket(&app, header).await;
        assert_eq!(answer.status, 401, "{header:?}");
        assert!(answer.header("set-cookie").is_none(), "{header:?}");
        assert_eq!(answer.header("cache-control"), Some("no-store"));
    }
    let answer = ticket(&app, Some(LAUNCHER)).await;
    assert_eq!(answer.status, 200);
    assert!(answer.header("set-cookie").is_none());
    assert_eq!(answer.header("cache-control"), Some("no-store"));
    assert_eq!(ticket_value(&answer).len(), 32);
    // Only POST.
    let mut b = Browser::default();
    assert_eq!(b.send_body(&app, "GET", "/desktop/ticket", None, Csrf::None, &[(desktop::LAUNCHER_HEADER, LAUNCHER)]).await.status, 404);
}

#[tokio::test]
async fn without_the_launcher_secret_nothing_signs_in() {
    for secret in [None, Some(""), Some("   ")] {
        let dir = install();
        let app = app_in(dir.path(), SECRET, None, &desktop_env(secret), TestClock::at(NOW));
        desktop::prepare(&app).await.unwrap();
        assert_eq!(ticket(&app, Some(LAUNCHER)).await.status, 401, "{secret:?}");
        assert_eq!(ticket(&app, secret.or(Some(""))).await.status, 401, "{secret:?}");
    }
}

#[tokio::test]
async fn a_ticket_signs_in_once_and_only_for_a_minute() {
    let dir = install();
    let clock = TestClock::at(NOW);
    let app = desktop_app(&dir, clock.clone()).await;
    let t = ticket_value(&ticket(&app, Some(LAUNCHER)).await);
    let mut first = Browser::default();
    let answer = first.get(&app, &format!("/desktop/sign_in?ticket={t}")).await;
    assert_eq!(answer.status, 302);
    assert_eq!(answer.header("cache-control"), Some("no-store"));
    assert_eq!(answer.header("referrer-policy"), Some("no-referrer"));
    assert!(dashboard_signed_in(&app, &mut first).await);
    // Spent.
    let mut second = Browser::default();
    assert_eq!(second.get(&app, &format!("/desktop/sign_in?ticket={t}")).await.status, 302);
    assert!(!dashboard_signed_in(&app, &mut second).await);
    // Expired after 60 s.
    let late = ticket_value(&ticket(&app, Some(LAUNCHER)).await);
    clock.set(common::web::at("2026-11-04T12:01:01Z"));
    let mut third = Browser::default();
    third.get(&app, &format!("/desktop/sign_in?ticket={late}")).await;
    assert!(!dashboard_signed_in(&app, &mut third).await);
    // Junk.
    let mut fourth = Browser::default();
    fourth.get(&app, "/desktop/sign_in?ticket=nope").await;
    fourth.get(&app, "/desktop/sign_in").await;
    assert!(!dashboard_signed_in(&app, &mut fourth).await);
}

#[tokio::test]
async fn at_most_four_tickets_live_and_a_fifth_drops_the_oldest() {
    let dir = install();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    let mut tickets = Vec::new();
    for _ in 0..5 { tickets.push(ticket_value(&ticket(&app, Some(LAUNCHER)).await)); }
    let mut oldest = Browser::default();
    oldest.get(&app, &format!("/desktop/sign_in?ticket={}", tickets[0])).await;
    assert!(!dashboard_signed_in(&app, &mut oldest).await);
    for t in &tickets[1..] {
        let mut b = Browser::default();
        b.get(&app, &format!("/desktop/sign_in?ticket={t}")).await;
        assert!(dashboard_signed_in(&app, &mut b).await);
    }
}

#[tokio::test]
async fn two_redemptions_at_once_sign_in_one() {
    let dir = install();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    // Under the sign-in limit (20 a minute).
    for _ in 0..8 {
        let t = ticket_value(&ticket(&app, Some(LAUNCHER)).await);
        let path = format!("/desktop/sign_in?ticket={t}");
        let (mut a, mut b) = (Browser::default(), Browser::default());
        let (_, _) = tokio::join!(a.get(&app, &path), b.get(&app, &path));
        let won = [a.cookie.is_some() && dashboard_signed_in(&app, &mut a).await, b.cookie.is_some() && dashboard_signed_in(&app, &mut b).await];
        assert_eq!(won.iter().filter(|w| **w).count(), 1, "{won:?}");
    }
}

#[tokio::test]
async fn sign_in_is_rate_limited() {
    let dir = install();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    let mut b = Browser::default();
    for _ in 0..20 { assert_eq!(b.get(&app, "/desktop/sign_in?ticket=x").await.status, 302); }
    assert_eq!(b.get(&app, "/desktop/sign_in?ticket=x").await.status, 429);
}

#[tokio::test]
async fn signed_out_the_login_page_asks_the_app_to_sign_the_window_in_and_shows_a_browser_the_code_field() {
    let dir = install();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    let mut b = Browser::default();
    let answer = b.get(&app, "/bots").await;
    assert_eq!(answer.header("location"), Some("/login"));
    let page = b.get(&app, "/login").await;
    assert_eq!(page.status, 200);
    // The window's script finds this and calls sign_in_window; a browser sees the one field.
    assert!(page.body.contains("data-desktop-sign-in"), "{}", page.body);
    assert!(page.body.contains("name=\"code\""));
    assert!(page.body.contains("<meta name=\"desktop-launch\"") || web::refused::ready_token().is_none());
    for gone in ["user[email]", "user[password]", "/signup", "/password/new"] {
        assert!(!page.body.contains(gone), "{gone}");
    }
}

// ---- §2.3 what the desktop does not serve ----

/// Every path the router of web/mod.rs names, as written there, with what the desktop does with it. A route added to
/// the router without a line here fails `every_route_of_the_router_is_classified`.
const ROUTES: &[(&str, bool)] = &[
    ("/", true), ("/login", true), ("/logout", false), ("/verify_two_factor", false), ("/signup", false),
    ("/password/new", false), ("/password/edit", false), ("/password", false), ("/setup", false), ("/setup/platform_connection", false),
    ("/{page}", true), ("/bots", true), ("/rules", true), ("/confirmation", false), ("/confirmation/new", false),
    ("/settings", true), ("/settings/{*action}", true),
    ("/.well-known/oauth-authorization-server", true), ("/.well-known/oauth-protected-resource", true), ("/.well-known/{*document}", true),
    ("/oauth/register", true), ("/oauth/token", true), ("/oauth/revoke", true), ("/oauth/introspect", true), ("/oauth/token/info", true),
    ("/oauth/authorize/native", true), ("/oauth/authorize", true),
    ("/up", true), ("/health-check", true), ("/ui-ready", true), ("/csp-report", true), ("/cable", true),
];

/// The settings actions the desktop does not serve; every other one is served.
const HIDDEN_SETTINGS: &[&str] = &[
    "/settings/update_registration", "/settings/update_email_notifications", "/settings/disconnect_email", "/settings/send_test_email",
    "/settings/update_email", "/settings/update_password", "/settings/update_two_fa", "/settings/edit_two_fa",
];

/// The settings actions the desktop serves.
const SERVED_SETTINGS: &[&str] = &[
    "/settings/account", "/settings/api", "/settings/api_key_permissions", "/settings/api_keys", "/settings/browser_sign_in_code",
    "/settings/connect", "/settings/destroy_api_key", "/settings/disconnect_market_data", "/settings/disconnect_stocks",
    "/settings/download_api_docs", "/settings/ibkr_connect", "/settings/ibkr_connect/activate", "/settings/ibkr_connect/download",
    "/settings/mcp", "/settings/regenerate_api_token", "/settings/resync_assets", "/settings/revoke_mcp_client",
    "/settings/update_client_tool_permissions", "/settings/update_locale", "/settings/update_market_data", "/settings/update_mcp_dry_run",
    "/settings/update_mcp_tool_group_permissions", "/settings/update_mcp_tool_permissions", "/settings/update_name",
    "/settings/update_rest_tool_group_permissions", "/settings/update_rest_tool_permissions", "/settings/update_stocks",
    "/settings/update_time_zone", "/settings/update_wash_sale", "/settings/update_display_currency", "/settings/update_hide_balances",
    "/settings/update_show_cash", "/settings/update_advanced_bots", "/settings/platform_connection",
];

#[test]
fn every_route_of_the_router_is_classified() {
    let source = include_str!("../src/web/mod.rs");
    let named: Vec<&str> = source.split(".route(\"").skip(1).filter_map(|rest| rest.split('"').next()).collect();
    for path in &named {
        // The bot, rules, tracker and wizard routes are served wholesale.
        let wholesale = ["/bots/", "/rules/", "/tracker", "/api/", "/broadcasts/", "/asset_tooltip"].iter().any(|p| path.starts_with(p));
        let listed = ROUTES.iter().find(|(p, _)| p == path);
        assert!(wholesale || listed.is_some(), "{path} is routed in web/mod.rs but not classified here");
        let served = listed.is_none_or(|(_, served)| *served);
        let probe = path.replace("{id}", "1").replace("{page}", "x").replace("{*action}", "account").replace("{*document}", "x");
        assert_eq!(!desktop::hidden(&probe), served, "{path}");
    }
    for path in HIDDEN_SETTINGS { assert!(desktop::hidden(path), "{path}"); }
    // Every action the settings dispatcher names (its literals, its action lists, its module paths) is classified.
    let dispatcher = include_str!("../src/web/settings/mod.rs");
    let mut actions: Vec<String> = dispatcher.split('"').skip(1).step_by(2)
        .filter_map(|literal| if literal.starts_with("/settings/") && !literal.contains('{') { Some(literal.to_string()) }
                    else if !literal.is_empty() && literal.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') && literal.contains('_') { Some(format!("/settings/{literal}")) }
                    else { None })
        .collect();
    actions.extend(web::settings::preferences::ACTIONS.iter().map(|a| format!("/settings/{a}")));
    actions.extend([web::settings::ibkr::PATH.to_string(), web::settings::platform::PATH.to_string()]);
    for action in &actions {
        let path = action.trim_end_matches('/');
        let hidden = HIDDEN_SETTINGS.contains(&path);
        assert!(hidden || SERVED_SETTINGS.contains(&path), "{path} is dispatched in web/settings/mod.rs but not classified here");
        assert_eq!(desktop::hidden(path), hidden, "{path}");
    }
    for path in ["/settings/account", "/settings/update_name", "/settings/update_locale", "/settings/update_time_zone", "/settings/connect",
                 "/settings/api_keys", "/settings/mcp", "/settings/api", "/settings/update_market_data", "/settings/platform_connection",
                 "/settings/browser_sign_in_code", "/settings/update_display_currency", "/settings/update_advanced_bots", "/setupx", "/signups"] {
        assert!(!desktop::hidden(path), "{path}");
    }
}

#[tokio::test]
async fn each_hidden_path_is_404_in_every_spelling_signed_in_or_not() {
    let dir = install();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    let mut window = signed_in_window(&app).await;
    let paths: Vec<&str> = ROUTES.iter().filter(|(_, s)| !s).map(|(p, _)| *p).chain(HIDDEN_SETTINGS.iter().copied()).collect();
    for path in paths {
        for spelled in [path.to_string(), format!("/de{path}"), format!("{path}.json"), format!("/pl{path}.html")] {
            for method in ["GET", "POST", "PATCH", "DELETE"] {
                for b in [&mut Browser::default(), &mut window] {
                    let answer = b.send_body(&app, method, &spelled, None, Csrf::None, &[]).await;
                    assert_eq!(answer.status, 404, "{method} {spelled}: {}", answer.header("location").unwrap_or(""));
                }
            }
            // A form POST overriding its method.
            let answer = Browser::default().send_body(&app, "POST", &spelled, Some("_method=delete".into()), Csrf::None, &[("content-type", "application/x-www-form-urlencoded")]).await;
            assert_eq!(answer.status, 404, "_method {spelled}");
        }
    }
    // Other /desktop paths.
    for path in ["/desktop", "/desktop/x", "/de/desktop/ticket"] {
        assert_eq!(Browser::default().send_body(&app, "POST", path, None, Csrf::None, &[(desktop::LAUNCHER_HEADER, LAUNCHER)]).await.status, 404, "{path}");
    }
}

#[tokio::test]
async fn the_account_pages_rows_log_out_and_sign_up_are_not_rendered() {
    let dir = install();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    let mut b = signed_in_window(&app).await;
    let page = b.get(&app, "/settings/account").await;
    assert_eq!(page.status, 200);
    for gone in ["/settings/update_email", "/settings/update_password", "/settings/edit_two_fa", "/settings/update_registration",
                 "/settings/update_email_notifications", "/logout", "/signup", "mail_not_set_up"] {
        assert!(!page.body.contains(gone), "{gone} is still on the page");
    }
    for kept in ["/settings/update_name", "/settings/update_locale", "/settings/update_time_zone", "/settings/browser_sign_in_code", "data-desktop-update"] {
        assert!(page.body.contains(kept), "{kept} is missing");
    }
}

// ---- §2.4 the browser code ----

async fn code(app: &App, window: &mut Browser) -> String {
    let page = window.get(app, "/settings/account").await;
    assert_eq!(page.status, 200, "{:?} {}", page.header("location"), page.body);
    let answer = window.post(app, "/settings/browser_sign_in_code", &[]).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let code = answer.body.split("data-desktop-code=\"").nth(1).and_then(|r| r.split('"').next()).expect("the code is shown").to_string();
    assert_eq!(code.len(), 8);
    assert!(code.chars().all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)), "{code}");
    code
}

async fn enter(app: &App, browser: &mut Browser, code: &str) -> Answer {
    browser.get(app, "/login").await;
    browser.post(app, "/login", &[("code", code)]).await
}

#[tokio::test]
async fn a_browser_code_signs_a_browser_in_once_and_continues_to_the_oauth_page() {
    let dir = install();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    let mut window = signed_in_window(&app).await;
    let code = code(&app, &mut window).await;
    let mut browser = Browser::default();
    let authorize = "/oauth/authorize?client_id=c&redirect_uri=http%3A%2F%2F127.0.0.1%3A9%2Fcb&response_type=code";
    let answer = browser.get(&app, authorize).await;
    assert_eq!(answer.header("location"), Some("/login"));
    // Typed in lower case with a dash: the same code.
    let typed = format!("{}-{}", &code[..4], &code[4..]).to_lowercase();
    let answer = enter(&app, &mut browser, &typed).await;
    assert_eq!((answer.status, answer.header("location")), (303, Some(authorize)));
    assert!(dashboard_signed_in(&app, &mut browser).await);
    // Spent.
    let mut again = Browser::default();
    assert_eq!(enter(&app, &mut again, &code).await.status, 422);
    assert!(!dashboard_signed_in(&app, &mut again).await);
}

#[tokio::test]
async fn a_code_lasts_ten_minutes_a_new_one_replaces_it_and_five_wrong_burn_it() {
    let dir = install();
    let clock = TestClock::at(NOW);
    let app = desktop_app(&dir, clock.clone()).await;
    let mut window = signed_in_window(&app).await;
    // Replaced.
    let first = code(&app, &mut window).await;
    let second = code(&app, &mut window).await;
    let mut b = Browser::default();
    if first != second { assert_eq!(enter(&app, &mut b, &first).await.status, 422); }
    // Five wrong: burnt, the right one no longer works.
    let mut b = Browser::default();
    let third = code(&app, &mut window).await;
    for _ in 0..5 { assert_eq!(enter(&app, &mut b, "00000000").await.status, 422); }
    assert_eq!(enter(&app, &mut b, &third).await.status, 422);
    // Expired after ten minutes.
    let fourth = code(&app, &mut window).await;
    clock.set(common::web::at("2026-11-04T12:10:01Z"));
    let mut b = Browser::default();
    assert_eq!(enter(&app, &mut b, &fourth).await.status, 422);
    assert!(!dashboard_signed_in(&app, &mut b).await);
}

#[tokio::test]
async fn two_browsers_entering_one_code_at_once_sign_in_one() {
    let dir = install();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    let mut window = signed_in_window(&app).await;
    // Under the sign-in limit (10 POSTs a minute).
    for _ in 0..4 {
        let code = code(&app, &mut window).await;
        let (mut a, mut b) = (Browser::default(), Browser::default());
        a.get(&app, "/login").await;
        b.get(&app, "/login").await;
        let form = [("code", code.as_str())];
        let (ra, rb) = tokio::join!(a.post(&app, "/login", &form), b.post(&app, "/login", &form));
        assert_eq!([ra.status, rb.status].iter().filter(|s| **s == 303).count(), 1, "{} {}", ra.status, rb.status);
    }
}

// ---- off the desktop: nothing changes ----

#[tokio::test]
async fn off_the_desktop_the_desktop_paths_and_the_account_pages_are_todays() {
    let dir = install();
    let app = app_in(dir.path(), SECRET, None, &Map::new(), TestClock::at(NOW));
    assert_eq!(app.desktop_account(), None);
    // No user is made, and setup is where every page goes.
    assert_eq!(Browser::default().get(&app, "/bots").await.header("location"), Some("/setup"));
    assert!(users(&dir).is_empty());
    insert_user(&dir, "owner@example.com", true);
    // /desktop is not a route here: the same answer as any other unknown path, header or not.
    let unknown = Browser::default().send_body(&app, "POST", "/nothing/here", None, Csrf::None, &[]).await.status;
    assert_eq!(ticket(&app, Some(LAUNCHER)).await.status, unknown);
    for path in ["/login", "/signup", "/password/new"] {
        let page = Browser::default().get(&app, path).await;
        assert!(!page.body.contains("data-desktop-sign-in"), "{path}");
    }
    let login = Browser::default().get(&app, "/login").await;
    assert!(login.body.contains("user[email]"));
}

// ---- Codex r1 ----

/// D-DESKTOP-OTHER-USERS-UNREACHABLE reaches bearer tokens too: a 2.x install's token for another user no longer acts
/// through REST or MCP; the desktop account's own tokens still do.
#[tokio::test]
async fn another_users_api_and_mcp_tokens_no_longer_act() {
    let dir = install();
    let admin = insert_user(&dir, "owner@example.com", true);
    let other = insert_user(&dir, "user@example.com", false);
    {
        let c = db(&dir);
        const T: &str = "2026-11-04 11:00:00";
        c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('c','c','http://localhost/cb',0,'mcp api',?1,?1)", [T]).unwrap();
        let application = c.last_insert_rowid();
        for (owner, prefix) in [(admin, "mine"), (other, "theirs")] {
            for scope in ["api", "mcp"] {
                c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,?5,86400)", (application, owner, format!("{prefix}-{scope}"), scope, T)).unwrap();
            }
            c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,NULL,NULL,?3,?3)", (owner, application, T)).unwrap();
        }
    }
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    let mut b = Browser::default();
    let rest = |token: &'static str| format!("Bearer {token}");
    let (mine, theirs) = (rest("mine-api"), rest("theirs-api"));
    assert_ne!(b.send_body(&app, "GET", "/api/v1/bots", None, Csrf::None, &[("authorization", &mine)]).await.status, 401);
    assert_eq!(b.send_body(&app, "GET", "/api/v1/bots", None, Csrf::None, &[("authorization", &theirs)]).await.status, 401);
    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}).to_string();
    for (token, refused) in [("mine-mcp", false), ("theirs-mcp", true)] {
        let auth = format!("Bearer {token}");
        let answer = b.send_body(&app, "POST", "/mcp", Some(initialize.clone()), Csrf::None,
                                 &[("authorization", &auth), ("content-type", "application/json"), ("accept", "application/json, text/event-stream")]).await;
        assert_eq!(answer.status == 401, refused, "{token}: {} {}", answer.status, answer.body);
    }
}

/// §1 fail closed: a start without the launcher's secret makes no browser code and takes none, even for a window still
/// signed in from before.
#[tokio::test]
async fn without_the_launcher_secret_no_browser_code_is_made_or_taken() {
    let dir = install();
    let clock = TestClock::at(NOW);
    let with = desktop_app(&dir, clock.clone()).await;
    let window = signed_in_window(&with).await;
    let cookie = window.cookie.clone();
    drop(with);
    let without = app_in(dir.path(), SECRET, None, &desktop_env(None), clock);
    let mut window = Browser { cookie, page: None };
    let page = window.get(&without, "/settings/account").await;
    assert_eq!(page.status, 200, "the old cookie still opens the window's own pages");
    let answer = window.post(&without, "/settings/browser_sign_in_code", &[]).await;
    assert_eq!(answer.status, 404);
    assert!(!answer.body.contains("data-desktop-code"));
    let mut browser = Browser::default();
    assert_eq!(enter(&without, &mut browser, "00000000").await.status, 422);
}

/// Codex r2 (D-DESKTOP-NO-MAIL): the desktop's expiry clears every marker seven days old, one batch after another; a
/// marker whose clear is refused stays, and the bots after it are still swept.
#[tokio::test]
async fn the_expiry_sweeps_past_a_marker_it_cannot_clear() {
    struct Fixed;
    impl deltabadger::engine::Clock for Fixed { fn now(&self) -> chrono::DateTime<chrono::Utc> { common::web::at(NOW) } }
    let dir = install();
    let user = insert_user(&dir, "owner@example.com", true);
    let marker = |stamped: &str| json!({ "rust_set_aside_mail_pending": { "error": "a reason", "stamped_at": stamped } }).to_string();
    let c = db(&dir);
    for (label, stamped) in [("refused", "2026-10-27T11:00:00.000Z"), ("old", "2026-10-27T11:00:00.000Z"), ("fresh", "2026-11-03T12:00:00.000Z"), ("old too", "2026-10-20T12:00:00.000Z")] {
        c.execute("INSERT INTO bots (user_id, type, label, transient_data, created_at, updated_at) VALUES (?1, 'Bots::DcaSingleAsset', ?2, ?3, ?4, ?4)", (user, label, marker(stamped), "2026-01-01 00:00:00")).unwrap();
    }
    c.execute_batch("CREATE TRIGGER refuse BEFORE UPDATE ON bots WHEN OLD.label = 'refused' BEGIN SELECT RAISE(ABORT, 'refused'); END;").unwrap();
    let opened = deltabadger::store::open(&deltabadger::store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let cipher = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, SECRET).unwrap());
    let mut s = deltabadger::mail::sender::Sender::new(opened.primary, cipher, &|_| None, Fixed);
    (s.batch, s.poll) = (1, std::time::Duration::from_millis(20));
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let run = tokio::spawn(s.expire_only(stopped));
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    stop.send(true).unwrap();
    run.await.unwrap().unwrap();
    let left: Vec<(String, bool)> = c.prepare("SELECT label, instr(transient_data, '_mail_pending') > 0 FROM bots ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(left, [("refused".into(), true), ("old".into(), false), ("fresh".into(), true), ("old too".into(), false)]);
}

/// A25 (D-JOBS-SETTINGS-LINK): the window has no address bar, so the desktop account (always the install's admin) reaches
/// /jobs from Settings' Instance section, and the page opens in the window.
#[tokio::test(flavor = "current_thread")]
async fn the_desktop_window_reaches_the_jobs_page_from_settings() {
    let dir = install();
    let app = desktop_app(&dir, TestClock::at(NOW)).await;
    let mut b = signed_in_window(&app).await;
    let settings = b.get(&app, "/settings/account").await;
    assert_eq!(settings.status, 200, "{}", settings.body);
    let row = settings.body.split("<div id=\"jobs\" class=\"sitem\">").nth(1).unwrap_or_else(|| panic!("no Jobs row:\n{}", settings.body));
    assert!(row.contains("href=\"/jobs\""), "{row}");
    let jobs = b.get(&app, "/jobs").await;
    assert_eq!(jobs.status, 200, "{}", jobs.body);
    assert!(jobs.body.contains("<h1>Jobs</h1>"));
}
