//! PLAN-710-importmem C3 (D-IMPORT-BUSY): a fresh tracker upload answered busy, then the same file chosen again, in a real
//! browser (script/rust/import_busy_check.mjs): the import modal as the app renders it, the busy answer the app writes,
//! and the compiled bundle's file-picker. The retry posts the file again. Needs bun; with no Chrome found it is skipped,
//! saying so.
mod common;
use common::web::{self, Browser, TestClock};

#[tokio::test(flavor = "current_thread")]
async fn a_busy_upload_is_retried_with_the_same_file() {
    let (d, o, s) = common::install();
    o.primary.execute("UPDATE users SET confirmed_at='2026-01-01'", []).unwrap();
    let app = web::app(d.path(), web::SECRET, TestClock::at("2026-09-01T12:30:00Z"));
    let hash: String = o.primary.query_row("SELECT encrypted_password FROM users WHERE id=?1", [s.user_id], |r| r.get(0)).unwrap();
    let data = deltabadger::web::session::SessionData { user: Some((s.user_id, hash.chars().take(29).collect())), csrf: Some(deltabadger::web::csrf::new_token()), ..Default::default() };
    let mut browser = Browser { cookie: Some(deltabadger::web::session::seal(&app.keys.session, &data, app.now())), page: None };
    let modal = browser.get(&app, "/tracker/import/new").await;
    assert_eq!(modal.status, 200, "{:?} {}", modal.header("location"), modal.body);
    let files = tempfile::tempdir().unwrap();
    let path = |name: &str| files.path().join(name);
    std::fs::write(path("modal.html"), &modal.body).unwrap();
    std::fs::write(path("busy.html"), deltabadger::web::tracker::import::busy_stream("en")).unwrap();
    std::fs::write(path("orders.csv"), "date,type\n").unwrap();
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let out = std::process::Command::new("bun").arg(format!("{root}/script/rust/import_busy_check.mjs"))
        .env("MODAL", path("modal.html")).env("BUSY", path("busy.html")).env("FILE", path("orders.csv"))
        .output().expect("bun runs");
    // No Chrome on this machine: nothing to drive (script/rust/import_busy_check.mjs exits 77).
    if out.status.code() == Some(77) { eprintln!("skipped: no Chrome found (set CHROME to its binary): {}", String::from_utf8_lossy(&out.stderr)); return; }
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}
