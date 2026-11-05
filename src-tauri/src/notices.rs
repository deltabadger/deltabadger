//! D-DESKTOP-NOTIFY (PLAN-B6-3 §3, r4 §3.1): the engine's bot alerts as system notifications.
//!
//! A thread for the app's life: right after an engine start first answers (its first good `GET /desktop/notices`), then
//! every 30 s, it lists the notices waiting in the engine, shows each new one with notify-rust and acks it at once when
//! the call returned Ok. An ack means the OS call returned without an error, not that anyone saw it: on macOS and
//! Windows that call returns Ok whether or not the notification shows (step 1 of the B6-3b PR); on Linux it fails when
//! no notification server answers, and the notice is shown at the next look. An id already shown this run (its ack
//! got lost) is acked again without showing; the last 200 are kept. A full page (20) is followed by another look at once.
use crate::supervisor::{Engine, HOST};
use serde::Deserialize;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_secs(1);
const SHOWN_KEPT: usize = 200;
/// The engine's page size (rust/src/mail/outbox.rs LIST_LIMIT).
const PAGE: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Notice {
    pub id: String,
    pub title: String,
    pub body: String,
}

/// The ids shown this run, newest last.
#[derive(Default)]
pub struct Shown(VecDeque<String>);

impl Shown {
    fn has(&self, id: &str) -> bool { self.0.iter().any(|s| s == id) }
    fn add(&mut self, id: &str) {
        if self.0.len() >= SHOWN_KEPT {
            self.0.pop_front();
        }
        self.0.push_back(id.to_string());
    }
}

/// One look: `list` the waiting notices, `show` each new one, `ack` each one shown now or before. None when the list
/// could not be had; else whether the page was full and every notice on it acked (look again at once).
pub fn look(
    list: &dyn Fn() -> Option<Vec<Notice>>,
    show: &dyn Fn(&Notice) -> Result<(), String>,
    ack: &dyn Fn(&str) -> bool,
    shown: &mut Shown,
    log: &dyn Fn(&str),
) -> Option<bool> {
    let notices = list()?;
    let mut all_acked = true;
    for notice in &notices {
        if !shown.has(&notice.id) {
            if let Err(error) = show(notice) {
                log(&format!("desktop: a bot alert could not be shown ({error}); trying again at the next look"));
                all_acked = false;
                continue;
            }
            shown.add(&notice.id);
        }
        // Not acked: the engine offers it again and it is acked then, without showing it twice.
        all_acked &= ack(&notice.id);
    }
    Some(notices.len() >= PAGE && all_acked)
}

fn client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder().timeout(Duration::from_secs(5)).no_proxy().redirect(reqwest::redirect::Policy::none()).build().ok()
}

/// `GET /desktop/notices` of the engine on `port`, with the launcher secret (PLAN-B6-3 §1).
pub fn list(port: u16, launcher: &str) -> Option<Vec<Notice>> {
    #[derive(Deserialize)]
    struct Page {
        notices: Vec<Notice>,
    }
    let response = client()?.get(format!("http://{HOST}:{port}/desktop/notices")).header("X-Deltabadger-Launcher", launcher).send().ok()?;
    if response.status() != reqwest::StatusCode::OK {
        return None;
    }
    Some(serde_json::from_str::<Page>(&response.text().ok()?).ok()?.notices)
}

/// `POST /desktop/notices/ack` for one id.
pub fn ack(port: u16, launcher: &str, id: &str) -> bool {
    let Some(client) = client() else { return false };
    let body = serde_json::json!({ "ids": [id] }).to_string();
    client
        .post(format!("http://{HOST}:{port}/desktop/notices/ack"))
        .header("X-Deltabadger-Launcher", launcher)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .is_ok_and(|r| r.status().is_success())
}

/// The system notification. `focus` is called where notify-rust delivers the click.
pub fn show(notice: &Notice, app_id: &str, focus: &'static (dyn Fn() + Send + Sync)) -> Result<(), String> {
    let mut n = notify_rust::Notification::new();
    n.summary(&notice.title).body(&notice.body).appname("Deltabadger");
    // The installed app's AppUserModelID is its identifier (as the tauri bundler registers it): the toast reads
    // "Deltabadger" with its icon.
    #[cfg(windows)]
    n.app_id(app_id);
    #[cfg(not(windows))]
    let _ = app_id;
    #[cfg(all(unix, not(target_os = "macos")))]
    n.action("default", "Open");
    let handle = n.show().map_err(|e| e.to_string())?;
    // ponytail: the click focuses the window on Linux only. macOS activates the app itself on a click, and notify-rust
    // reports it there only by blocking the call; on Windows notify-rust has no click callback. Navigation to the bot:
    // none anywhere (add it with a notify-rust that reports clicks, or tauri-plugin-notification on tauri >= 2.12).
    #[cfg(all(unix, not(target_os = "macos")))]
    std::thread::spawn(move || handle.wait_for_action(|action| if action == "default" { focus() }));
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = (handle, focus);
    }
    Ok(())
}

/// macOS: notifications are posted as this bundle (its name and icon). An unbundled run keeps the default and says so.
pub fn set_app(identifier: &str, log: &dyn Fn(&str)) {
    #[cfg(target_os = "macos")]
    if let Err(error) = notify_rust::set_application(identifier) {
        log(&format!("desktop: bot alerts are posted without the app's name ({error})"));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (identifier, log);
}

/// The poller, for the app's life: looks at whichever engine start is current.
pub fn run(engine: &Engine, app_id: &str, focus: &'static (dyn Fn() + Send + Sync), log: &dyn Fn(&str)) {
    let mut shown = Shown::default();
    // The engine start last looked at, and when.
    let mut last: Option<((u16, String), Instant)> = None;
    loop {
        std::thread::sleep(TICK);
        let Some((port, launcher)) = engine.launcher() else { continue };
        let fresh = last.as_ref().is_none_or(|(at, when)| at.0 != port || at.1 != launcher || when.elapsed() >= EVERY);
        if !fresh || engine.quitting() {
            continue;
        }
        let shown_now = |n: &Notice| {
            show(n, app_id, focus)?;
            log(&format!("desktop: bot alert shown ({})", n.id));
            // Tests (debug builds): the app can be killed between the show and the ack (the B6-3b proofs).
            #[cfg(debug_assertions)]
            if let Some(secs) = std::env::var("DELTABADGER_TEST_PAUSE_BEFORE_ACK").ok().and_then(|s| s.parse().ok()) {
                log("desktop: test pause before the ack");
                std::thread::sleep(Duration::from_secs(secs));
            }
            Ok(())
        };
        let looked = look(&|| list(port, &launcher), &shown_now, &|id| ack(port, &launcher, id), &mut shown, log);
        match looked {
            // A start that does not answer yet (starting, refused): asked again next tick.
            None => {}
            Some(again) => last = Some(((port, launcher), if again { Instant::now() - EVERY } else { Instant::now() })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn n(id: &str) -> Notice { Notice { id: id.into(), title: format!("t{id}"), body: "b".into() } }

    /// A fake engine: what it offers, and the acks it got.
    struct Fake {
        offered: RefCell<Vec<Notice>>,
        acked: RefCell<Vec<String>>,
        ack_works: RefCell<bool>,
        shown: RefCell<Vec<String>>,
        show_works: RefCell<bool>,
    }

    impl Fake {
        fn new(ids: &[&str]) -> Self {
            Fake { offered: RefCell::new(ids.iter().map(|i| n(i)).collect()), acked: RefCell::default(), ack_works: RefCell::new(true), shown: RefCell::default(), show_works: RefCell::new(true) }
        }
        fn look(&self, shown: &mut Shown) -> Option<bool> {
            look(
                &|| Some(self.offered.borrow().clone()),
                &|n| if *self.show_works.borrow() { self.shown.borrow_mut().push(n.id.clone()); Ok(()) } else { Err("no notification server".into()) },
                &|id| {
                    let ok = *self.ack_works.borrow();
                    if ok {
                        self.acked.borrow_mut().push(id.to_string());
                        self.offered.borrow_mut().retain(|n| n.id != id);
                    }
                    ok
                },
                shown,
                &|_| {},
            )
        }
    }

    #[test]
    fn each_new_notice_is_shown_then_acked_and_not_offered_again() {
        let fake = Fake::new(&["a", "b"]);
        let mut shown = Shown::default();
        assert_eq!(fake.look(&mut shown), Some(false));
        assert_eq!(*fake.shown.borrow(), ["a", "b"]);
        assert_eq!(*fake.acked.borrow(), ["a", "b"]);
        assert_eq!(fake.look(&mut shown), Some(false));
        assert_eq!(fake.shown.borrow().len(), 2);
    }

    #[test]
    fn a_failed_show_is_not_acked_and_is_shown_at_the_next_look() {
        let fake = Fake::new(&["a"]);
        let mut shown = Shown::default();
        *fake.show_works.borrow_mut() = false;
        fake.look(&mut shown);
        assert!(fake.acked.borrow().is_empty(), "no ack without an Ok");
        *fake.show_works.borrow_mut() = true;
        fake.look(&mut shown);
        assert_eq!((fake.shown.borrow().clone(), fake.acked.borrow().clone()), (vec!["a".to_string()], vec!["a".to_string()]));
    }

    #[test]
    fn a_lost_ack_is_sent_again_at_the_next_look_without_showing_twice() {
        let fake = Fake::new(&["a"]);
        let mut shown = Shown::default();
        *fake.ack_works.borrow_mut() = false;
        fake.look(&mut shown);
        *fake.ack_works.borrow_mut() = true;
        fake.look(&mut shown);
        assert_eq!(*fake.shown.borrow(), ["a"]);
        assert_eq!(*fake.acked.borrow(), ["a"]);
    }

    #[test]
    fn a_full_page_asks_again_at_once_and_only_the_last_200_ids_are_kept() {
        let ids: Vec<String> = (0..PAGE).map(|i| i.to_string()).collect();
        let fake = Fake::new(&ids.iter().map(String::as_str).collect::<Vec<_>>());
        let mut shown = Shown::default();
        assert_eq!(fake.look(&mut shown), Some(true));
        assert_eq!(look(&|| None, &|_| Ok(()), &|_| true, &mut shown, &|_| {}), None, "no list: nothing done");
        for i in 0..SHOWN_KEPT { shown.add(&format!("x{i}")); }
        assert!(!shown.has("0") && shown.has("x199"));
    }

    /// The two requests against a fake engine: the launcher's header on both, the ack's JSON body.
    #[test]
    fn list_and_ack_carry_the_launcher_secret() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind((HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut seen = vec![];
            for answer in [r#"{"notices":[{"id":"7:k::t","bot_id":7,"title":"T","body":"B"}]}"#, ""] {
                let (mut s, _) = listener.accept().unwrap();
                // The headers, then as many bytes as Content-Length says.
                let mut got = String::new();
                let mut buf = [0_u8; 4096];
                while !got.split_once("\r\n\r\n").is_some_and(|(head, body)| {
                    let length = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse::<usize>().unwrap_or(0)));
                    body.len() >= length.unwrap_or(0)
                }) {
                    let read = s.read(&mut buf).unwrap();
                    got.push_str(&String::from_utf8_lossy(&buf[..read]));
                }
                let ok = got.lines().any(|l| l.eq_ignore_ascii_case("x-deltabadger-launcher: secret"));
                let status = if !ok { "401 Unauthorized" } else if answer.is_empty() { "204 No Content" } else { "200 OK" };
                write!(s, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len()).unwrap();
                seen.push(got);
            }
            seen
        });
        assert_eq!(list(port, "secret"), Some(vec![Notice { id: "7:k::t".into(), title: "T".into(), body: "B".into() }]));
        assert!(ack(port, "secret", "7:k::t"));
        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("GET /desktop/notices "));
        assert!(seen[1].starts_with("POST /desktop/notices/ack ") && seen[1].ends_with(r#"{"ids":["7:k::t"]}"#), "{}", seen[1]);
    }
}
