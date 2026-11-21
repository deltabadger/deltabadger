//! D-SCHEME-SWITCH: the Settings menu's colour scheme. The `scheme` cookie (set in the browser) is put on `<html>` and
//! picks the option both menu copies show as chosen; 2.x comparisons leave the row out (tests/common/html.rs).
mod common;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::web::App;

const T0: &str = "2026-09-01T12:30:00Z";

/// Which option each `[data-scheme-switch]` row shows as chosen, as `value:aria-checked:tabindex` of its `is-on` options.
fn chosen(page: &str) -> Vec<String> {
    page.split("data-scheme-switch").skip(1).map(|row| {
        let row = row.split("</div>").next().unwrap();
        row.split("<button ").skip(1).filter(|b| b.contains(" is-on\"")).map(|b| {
            let attr = |name: &str| b.split(&format!("{name}=\"")).nth(1).unwrap().split('"').next().unwrap().to_string();
            format!("{}:{}:{}", attr("data-value"), attr("aria-checked"), attr("tabindex"))
        }).collect::<Vec<_>>().join(",")
    }).collect()
}

fn html_tag(page: &str) -> &str {
    let at = page.find("<html").unwrap();
    &page[at..at + page[at..].find('>').unwrap() + 1]
}

async fn signed_in() -> (tempfile::TempDir, App, Browser) {
    let (d, o, _s) = common::install_alpaca();
    o.primary.execute("UPDATE users SET encrypted_password=?1, confirmed_at='2026-01-01'",
                      [deltabadger::crypto::hash_password("Correct-horse-9").unwrap()]).unwrap();
    let app = web::app(d.path(), "engine-test-secret", TestClock::at(T0));
    app.attach_engine(Default::default());
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    (d, app, browser)
}

#[tokio::test]
async fn the_scheme_cookie_is_on_the_page_and_chosen_in_both_menus() {
    let (_d, app, mut browser) = signed_in().await;
    // The browser sends its session cookie in its own Cookie header; the scheme rides in a second one.
    for (cookie, attribute, option) in [
        (None, "", "default"),
        (Some("scheme=space"), " data-scheme=\"space\"", "space"),
        (Some("theme=x; scheme=warm"), " data-scheme=\"warm\"", "warm"),
        (Some("scheme=forest"), " data-scheme=\"forest\"", "forest"),
        (Some("scheme=bogus"), "", "default"),
    ] {
        let headers: Vec<(&str, &str)> = cookie.map(|c| ("cookie", c)).into_iter().collect();
        let page = browser.send(&app, "GET", "/settings/account", None, Csrf::None, &headers).await;
        assert_eq!(page.status, 200, "{cookie:?}");
        assert!(html_tag(&page.body).ends_with(&format!("lang=\"en\"{attribute}>")), "{cookie:?}: {}", html_tag(&page.body));
        let on = format!("{option}:true:0");
        assert_eq!(chosen(&page.body), [on.clone(), on], "{cookie:?}: the mobile menu and the dropdown");
    }
}

#[tokio::test]
async fn the_sign_in_page_paints_in_the_scheme_too() {
    let (_d, app, _) = signed_in().await;
    let mut browser = Browser::default();
    let page = browser.send(&app, "GET", "/login", None, Csrf::None, &[("cookie", "a=b; scheme=warm")]).await;
    assert!(html_tag(&page.body).contains(" data-scheme=\"warm\""), "{}", html_tag(&page.body));
    let page = Browser::default().get(&app, "/login").await;
    assert!(!html_tag(&page.body).contains("data-scheme"));
}

/// The 2.x comparison leaves out exactly the row: what is around it is compared as before.
#[test]
fn the_normaliser_leaves_out_only_the_scheme_row() {
    let ours = "<div class=\"dropdown\"><form></form><div class=\"dropdown__item\" data-scheme-switch><button>Space</button></div>\n<a>x</a></div>";
    let v2 = "<div class=\"dropdown\"><form></form>\n<a>x</a></div>";
    assert_eq!(common::html::normalize(ours), common::html::normalize(v2));
    // D-PIXEL-SCENES: an empty state's scene goes the same way.
    let ours = "<div class=\"welcome-screen\"><canvas class=\"pixel-scene\" data-pixel-scene></canvas>\n<p>Start investing.</p></div>";
    let v2 = "<div class=\"welcome-screen\">\n<p>Start investing.</p></div>";
    assert_eq!(common::html::normalize(ours), common::html::normalize(v2));
}

/// D-MODE-SWITCH: the `mode` cookie is on `<html data-mode>` and picks System/Light/Dark in both menu copies; anything
/// else is System, which is no attribute.
#[tokio::test]
async fn the_mode_cookie_is_on_the_page_and_chosen_in_both_menus() {
    let (_d, app, mut browser) = signed_in().await;
    for (cookie, attribute, option) in [
        (None, "", "system"),
        (Some("mode=light"), " data-mode=\"light\"", "light"),
        (Some("scheme=ember; mode=dark"), " data-scheme=\"ember\" data-mode=\"dark\"", "dark"),
        (Some("mode=bogus"), "", "system"),
    ] {
        let headers: Vec<(&str, &str)> = cookie.map(|c| ("cookie", c)).into_iter().collect();
        let page = browser.send(&app, "GET", "/settings/account", None, Csrf::None, &headers).await;
        assert_eq!(page.status, 200, "{cookie:?}");
        assert!(html_tag(&page.body).ends_with(&format!("lang=\"en\"{attribute}>")), "{cookie:?}: {}", html_tag(&page.body));
        let rows: Vec<String> = page.body.split("data-mode-switch").skip(1).map(|row| {
            let row = row.split("<!--/mode-->").next().unwrap();
            row.split("<button ").skip(1).filter(|b| b.contains(" is-on\"")).map(|b| b.split("data-value=\"").nth(1).unwrap().split('"').next().unwrap().to_string()).collect::<Vec<_>>().join(",")
        }).collect();
        assert_eq!(rows, [option, option], "{cookie:?}: the mobile menu and the dropdown");
    }
}

/// The normalisers drop 2.x's sidebar logo (D-NO-MENU-LOGO) and unwrap the version's link (D-VERSION-LINK), so neither
/// comparison would notice either coming back or going; this does.
#[tokio::test]
async fn the_sidebar_has_no_logo_and_the_version_links_to_the_source() {
    let (_d, app, mut browser) = signed_in().await;
    let page = browser.send(&app, "GET", "/settings/account", None, Csrf::None, &[]).await;
    assert_eq!(page.status, 200);
    assert!(!page.body.contains("menu__section--logo") && !page.body.contains("menu__logo"));
    let version = format!("<div class=\"sheet__version\"><a href=\"https://github.com/deltabadger/deltabadger\" target=\"_blank\" rel=\"noopener\" data-version-link>{}</a></div>", deltabadger::VERSION);
    assert!(page.body.contains(&version), "{}", page.body.split("sheet__version").nth(1).unwrap_or("no version"));
}
