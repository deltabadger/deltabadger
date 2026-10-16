//! GET /tracker/export against Rails (script/rust/record_tracker_export.rb records fixtures/tracker_export.json): the rows Rails
//! read are installed, each request is replayed, and the answer must be Rails' bytes and download headers, leaving the
//! tracker_settings Rails left.
mod common;
use common::web::{self, Browser, Csrf, TestClock};
use deltabadger::web::{csrf, session};
use rusqlite::{types::Value as Sql, Connection};
use serde_json::Value;

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/tracker_export.json")).unwrap() }

fn install(c: &Connection, tables: &Value, rails_owner: i64, owner: i64) {
    c.execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM account_transactions; DELETE FROM exchanges;").unwrap();
    for (table, rows) in tables.as_object().unwrap() {
        for row in rows.as_array().unwrap() {
            let row = row.as_object().unwrap();
            let columns: Vec<String> = row.keys().map(|k| format!("\"{k}\"")).collect();
            let values: Vec<Sql> = row.iter().map(|(k, v)| match v {
                // The recorded owner is this install's user; Rails' other owner is nobody here.
                Value::Number(n) if k == "user_id" => Sql::Integer(if n.as_i64() == Some(rails_owner) { owner } else { -1 }),
                Value::Object(real) => Sql::Real(real["real"].as_str().unwrap().parse().unwrap()),
                Value::Null => Sql::Null,
                Value::Number(n) if n.is_i64() => Sql::Integer(n.as_i64().unwrap()),
                Value::String(s) => Sql::Text(s.clone()),
                other => Sql::Text(other.to_string()),
            }).collect();
            c.execute(&format!("INSERT INTO {table} ({}) VALUES ({})", columns.join(","), vec!["?"; row.len()].join(",")),
                rusqlite::params_from_iter(values)).unwrap();
        }
    }
}

const OLD: &str = "2026-09-30 21:30:00";

#[tokio::test(flavor = "current_thread")]
async fn export_is_rails_own_csv_and_saves_the_export_type() {
    let v = vectors();
    let (dir, opened, seeded) = common::install_alpaca();
    opened.primary.execute("UPDATE users SET confirmed_at=created_at, setup_completed=1", []).unwrap();
    install(&opened.primary, &v["tables"], v["user_id"].as_i64().unwrap(), seeded.user_id);
    let hash: String = opened.primary.query_row("SELECT encrypted_password FROM users WHERE id=?1", [seeded.user_id], |r| r.get(0)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at(v["now"].as_str().unwrap()));
    let data = session::SessionData { user: Some((seeded.user_id, hash.chars().take(29).collect())), csrf: Some(csrf::new_token()), ..Default::default() };
    let mut browser = Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None };
    let cases = v["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 15);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        opened.primary.execute("UPDATE users SET tracker_settings=?1, updated_at=?2 WHERE id=?3",
            (case["settings_before"].as_str(), OLD, seeded.user_id)).unwrap();
        let answer = browser.send(&app, "GET", case["path"].as_str().unwrap(), None, Csrf::None, &[]).await;
        assert_eq!(i64::from(answer.status), case["status"].as_i64().unwrap(), "{name}: {}", answer.body);
        if answer.status == 200 {
            assert_eq!(answer.body, case["body"].as_str().unwrap(), "{name}");
            for (header, want) in case["headers"].as_object().unwrap() {
                assert_eq!(answer.header(&header.to_ascii_lowercase()), want.as_str(), "{name}: {header}");
            }
        }
        let (settings, updated): (String, String) = opened.primary.query_row("SELECT tracker_settings, updated_at FROM users WHERE id=?1",
            [seeded.user_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        let parse = |s: &str| serde_json::from_str::<Value>(s).unwrap();
        assert_eq!(parse(&settings), parse(case["settings_after"].as_str().unwrap()), "{name}: settings");
        assert_eq!(updated != OLD, case["touched"].as_bool().unwrap(), "{name}: updated_at");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn export_requires_a_signed_in_user_and_writes_nothing_without_one() {
    let (dir, opened, seeded) = common::install_alpaca();
    let before: Option<String> = opened.primary.query_row("SELECT tracker_settings FROM users WHERE id=?1", [seeded.user_id], |r| r.get(0)).unwrap();
    let app = web::app(dir.path(), web::SECRET, TestClock::at("2026-10-01T21:30:00Z"));
    let mut browser = Browser::default();
    for (path, login) in [("/tracker/export", "/login"), ("/de/tracker/export?from=2026-01-01", "/de/login")] {
        let answer = browser.get(&app, path).await;
        assert_eq!((answer.status, answer.header("location")), (302, Some(login)), "{path}");
    }
    let after: Option<String> = opened.primary.query_row("SELECT tracker_settings FROM users WHERE id=?1", [seeded.user_id], |r| r.get(0)).unwrap();
    assert_eq!(after, before);
}

/// The Ruby this export mirrors, by content: a change fails here until the vectors are recorded again (script/rust/repin).
#[test]
fn the_ruby_being_mirrored_has_not_changed() {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, sum) in vectors()["ported_sources"].as_object().unwrap() {
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(hex::encode(Sha256::digest(&bytes)), sum.as_str().unwrap(), "{path} changed since the export vectors were recorded");
    }
}
