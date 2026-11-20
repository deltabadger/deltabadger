//! D-LANGUAGE-STEP (PLAN-ONBOARDING-LANGUAGE §3, §8): the language step in front of the admin form, and the rule that
//! decides whether it is shown. The rule's cases live in tests/fixtures/first_choice.json, which the desktop launcher's
//! tests (src-tauri) read too.
mod common;
use common::web::{Answer, Browser, Csrf, TestClock};
use deltabadger::store::{self, Paths};
use deltabadger::web::locale::{self, Choice};
use deltabadger::web::{self, App, Config};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

fn fixture() -> Value { serde_json::from_str(include_str!("fixtures/first_choice.json")).unwrap() }

fn choice(expected: &Value) -> Choice {
    let code = locale::known(expected[1].as_str().unwrap()).unwrap();
    match expected[0].as_str().unwrap() { "use" => Choice::Use(code), "ask" => Choice::Ask(code), other => panic!("{other}") }
}

#[test]
fn the_first_preference_decides_whether_to_ask() {
    for case in fixture()["preferences"].as_array().unwrap() {
        let languages: Vec<&str> = case["languages"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect();
        assert_eq!(locale::first_choice(&languages), choice(&case["choice"]), "{case}");
    }
}

#[test]
fn accept_language_is_read_in_q_order_without_refused_or_malformed_items() {
    for case in fixture()["parse"].as_array().unwrap() {
        let expected: Vec<&str> = case["languages"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect();
        assert_eq!(locale::accept_language(case["header"].as_str().unwrap()), expected, "{case}");
    }
    for case in fixture()["headers"].as_array().unwrap() {
        assert_eq!(locale::first_choice(&locale::accept_language(case["header"].as_str().unwrap())), choice(&case["choice"]), "{case}");
    }
}

/// The picker's order: larger language communities first (not `LOCALES`' order, which is 2.x's).
#[test]
fn the_picker_lists_every_locale_once_in_its_own_order() {
    assert_eq!(locale::PICKER, ["en", "es", "de", "fr", "pt", "it", "nl", "pl", "ru", "cs", "sk", "sv", "da", "el", "bg"]);
    let mut sorted = locale::PICKER.to_vec();
    sorted.sort();
    let mut all = locale::LOCALES.to_vec();
    all.sort();
    assert_eq!(sorted, all);
}

fn install() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (file, schema) in [("production.sqlite3", include_str!("fixtures/settings_primary_schema.sql")), ("production_queue.sqlite3", include_str!("fixtures/settings_queue_schema.sql"))] {
        let c = rusqlite::Connection::open(dir.path().join(file)).unwrap();
        c.execute_batch(&format!("BEGIN IMMEDIATE;\n{schema}\nCOMMIT;")).unwrap();
    }
    dir
}

fn app(dir: &Path, env: &[(&str, &str)]) -> App {
    let opened = store::open(&Paths::from_env(&|_| None, dir)).expect("the install passes store::check");
    let env: HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let lookup = move |name: &str| if name == "SECRET_KEY_BASE" { Some("language-test-secret".to_string()) } else { env.get(name).cloned() };
    App::new(Config::from_env(&lookup).unwrap(), &lookup, opened.primary, TestClock::at("2026-11-20T12:00:00Z")).unwrap()
        .with_figure_source(web::figure::loading::Source::Disabled).unwrap()
}

fn add_admin(dir: &Path) {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    c.execute("INSERT INTO users (email, encrypted_password, admin, confirmed_at, created_at, updated_at) VALUES ('o@example.com', 'x', 1, '2026-01-01', '2026-01-01', '2026-01-01')", []).unwrap();
}

async fn get(app: &App, path: &str, accept_language: Option<&str>) -> Answer {
    let headers: Vec<(&str, &str)> = accept_language.map(|value| ("accept-language", value)).into_iter().collect();
    Browser::default().send(app, "GET", path, None, Csrf::None, &headers).await
}

#[tokio::test(flavor = "current_thread")]
async fn the_gate_asks_only_an_english_or_unsupported_browser() {
    let dir = install();
    let app = app(dir.path(), &[]);
    for path in ["/bots", "/"] {
        for (header, location) in [(Some("en-US,en;q=0.9"), "/language"), (Some("ja,pl;q=0.8"), "/language"), (None, "/language"),
                                   (Some("de-CH,de;q=0.9,en;q=0.8"), "/setup?locale=de"), (Some("pl"), "/setup?locale=pl")] {
            let answer = get(&app, path, header).await;
            assert_eq!((answer.status, answer.header("location")), (302, Some(location)), "{path} {header:?}");
        }
    }
    // The step itself and the form are not gated.
    assert_eq!(get(&app, "/language", None).await.status, 200);
    assert_eq!(get(&app, "/setup", Some("pl")).await.status, 200, "a typed /setup answers the form");
    add_admin(dir.path());
    let answer = get(&app, "/language", Some("pl")).await;
    assert_eq!((answer.status, answer.header("location")), (302, Some("/")));
}

/// The rows of the picker: (code, checked, greeting, button label).
fn rows(body: &str) -> Vec<(String, bool, String, String)> {
    let row = regex::Regex::new(r#"<label class="language__row"[^>]*data-greeting="([^"]*)" data-continue="([^"]*)">\s*<input type="radio" name="locale" value="([a-z]{2})" lang="[a-z]{2}"( checked)?"#).unwrap();
    row.captures_iter(body).map(|c| (c[3].to_string(), c.get(4).is_some(), c[1].to_string(), c[2].to_string())).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn the_picker_puts_the_preselected_language_first_and_checked() {
    let dir = install();
    let app = app(dir.path(), &[]);
    for (path, header, preselected) in [("/language?locale=pl", None, "pl"), ("/language", Some("ja,pl;q=0.8"), "pl"), ("/language", Some("de"), "de"),
                                         ("/language", None, "en"), ("/language?locale=zz", Some("fr"), "fr"), ("/language?locale=zz", None, "en")] {
        let answer = get(&app, path, header).await;
        assert_eq!(answer.status, 200, "{path} {header:?}");
        let body = &answer.body;
        let rows = rows(body);
        let mut order: Vec<&str> = vec![preselected];
        order.extend(locale::PICKER.iter().copied().filter(|code| *code != preselected));
        assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), order, "{path} {header:?}");
        assert_eq!(rows.iter().filter(|r| r.1).map(|r| r.0.as_str()).collect::<Vec<_>>(), vec![preselected], "only the first is checked");
        for (code, _, greeting, label) in &rows {
            assert_eq!(*greeting, web::i18n::text(code, "setup.language.greeting", &[]), "{code}");
            assert_eq!(*label, web::i18n::text(code, "setup.continue", &[]), "{code}");
        }
        let hello = web::i18n::text(preselected, "setup.language.greeting", &[]);
        let next = web::i18n::text(preselected, "setup.continue", &[]);
        assert!(body.contains(&format!("<h1 class=\"language__hello\" data-language-picker-target=\"hello\">{hello}</h1>")), "{path}: {body}");
        assert!(body.contains(&format!(">{next}</button>")), "{path}");
        assert!(body.contains(&format!("<html lang=\"{preselected}\"")), "{path}");
        assert!(body.contains("<form action=\"/setup\" method=\"get\""), "{path}");
        assert!(!body.contains("devise__icon") && !body.contains("class=\"signet\""), "no logo");
        assert!(body.contains("devise--onboarding"), "{path}");
    }
}

/// Each row's native name is the dropdown's label without its flag.
#[tokio::test(flavor = "current_thread")]
async fn each_row_names_its_language_in_that_language() {
    let dir = install();
    let app = app(dir.path(), &[]);
    let body = get(&app, "/language", None).await.body;
    for (code, name) in [("en", "English"), ("de", "Deutsch"), ("pl", "Polski"), ("el", "Ελληνικά"), ("bg", "Български"), ("ru", "Русский"), ("cs", "Čeština")] {
        assert!(body.contains(&format!("<span lang=\"{code}\">{name}</span>")), "{code}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_token_redirect_goes_the_way_the_gate_goes() {
    let dir = install();
    let app = app(dir.path(), &[("SETUP_TOKEN", "good-token")]);
    for (path, header, location) in [("/setup?token=good-token", Some("en-US"), "/language"), ("/setup?token=good-token", None, "/language"),
                                      ("/setup?token=good-token", Some("de-DE,de"), "/setup?locale=de"), ("/setup?token=good-token&locale=pl", Some("de"), "/setup?locale=pl")] {
        let answer = get(&app, path, header).await;
        assert_eq!((answer.status, answer.header("location")), (302, Some(location)), "{path} {header:?}");
    }
    // The step shows nothing of the install: no token needed. The form still asks for it.
    assert_eq!(get(&app, "/language", None).await.status, 200);
    assert_eq!(get(&app, "/setup?locale=pl", None).await.status, 403);
}

#[test]
fn every_locale_has_the_new_keys() {
    for code in locale::LOCALES {
        for key in ["setup.language.greeting", "setup.create", "bot.api.desktop_stable_ip_html"] {
            assert!(web::i18n::text_chain(code, &[key.to_string()], &[]).is_some_and(|t| !t.is_empty()), "{code}: {key}");
        }
    }
}

/// The launcher's status page draws the same picker (src-tauri/ui/index.html): its table must be the engine's codes,
/// order, native names, greetings and button labels.
#[test]
fn the_launchers_picker_table_matches_the_locale_files() {
    let page = include_str!("../../src-tauri/ui/index.html");
    let start = page.find("var LANGUAGES = [").expect("the launcher's language table");
    let table = &page[start..start + page[start..].find("];").unwrap()];
    let entry = regex::Regex::new(r#"\["([a-z]{2})", "([^"]+)", "([^"]+)", "([^"]+)"\]"#).unwrap();
    let rows: Vec<(String, String, String, String)> = entry.captures_iter(table).map(|c| (c[1].into(), c[2].into(), c[3].into(), c[4].into())).collect();
    let expected: Vec<(String, String, String, String)> = locale::PICKER.iter().map(|code| (
        code.to_string(), locale::native_name(code).to_string(),
        web::i18n::text(code, "setup.language.greeting", &[]), web::i18n::text(code, "setup.continue", &[]),
    )).collect();
    assert_eq!(rows, expected);
}
