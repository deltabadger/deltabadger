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
}
