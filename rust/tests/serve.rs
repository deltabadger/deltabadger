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
