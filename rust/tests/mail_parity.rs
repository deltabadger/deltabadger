//! Mail parity: Rails sends every mail of the grid through its own mailers into ActionMailer::Base.deliveries
//! (script/rust/mails.rb), this crate builds the same mails from the same database, and each pair must be the same
//! message, byte for byte, once Date and Message-ID (the two fields that are each process's own) are set aside.
//! Then the sender, end to end on a Rails-prepared install against the fake SMTP server.
mod common;
use deltabadger::engine::notice::Notice;
use deltabadger::mail::render::{self, Recipient, Urls};
use deltabadger::mail::smtp::{notifications_sender, Env};
use deltabadger::mail::{mailbox, Message};
use rusqlite::Connection;
use serde_json::{json, Value};

/// A message with its own two header fields masked, after checking that each is there exactly once and has the form
/// it must have. Only the header section is touched: a body line that looks like one of them is body.
fn comparable(message: &str) -> String {
    let (headers, body) = message.split_once("\r\n\r\n").expect("a blank line between the headers and the body");
    let mut seen = (0, 0);
    let lines: Vec<String> = headers.split("\r\n").map(|line| {
        if let Some(date) = line.strip_prefix("Date: ") {
            assert!(chrono::DateTime::parse_from_rfc2822(date).is_ok(), "Date {date:?}");
            seen.0 += 1;
            "Date: [date]".to_string()
        } else if let Some(id) = line.strip_prefix("Message-ID: ") {
            assert!(id.starts_with('<') && id.ends_with('>') && id.matches('@').count() == 1 && id.len() > 5 && !id.contains(' '), "Message-ID {id:?}");
            seen.1 += 1;
            "Message-ID: [id]".to_string()
        } else {
            line.to_string()
        }
    }).collect();
    assert_eq!(seen, (1, 1), "a message has one Date and one Message-ID");
    format!("{}\r\n\r\n{body}", lines.join("\r\n"))
}

fn recipient(c: &Connection, user_id: i64, locale: Option<&str>, to: Option<&str>) -> Recipient {
    let (email, name, stored): (String, Option<String>, Option<String>) =
        c.query_row("SELECT email, name, locale FROM users WHERE id = ?1", [user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    // An account mail is in the request's language; the test mail in the user's.
    let locale = locale.map_or_else(|| render::user_locale(stored.as_deref()), |l| deltabadger::web::locale::known(l).unwrap());
    Recipient { email: to.map_or(email, str::to_string), name: name.unwrap_or_default(), locale }
}

fn build(c: &Connection, case: &Value) -> Message {
    let sender_env = Env::read(&|name| if name == "NOTIFICATIONS_SENDER" { case["sender"].as_str().map(str::to_string) } else { None });
    let from = notifications_sender(&sender_env, &|_| None);
    let urls = Urls::from_env(&|name| if name == "APP_ROOT_URL" { case["root"].as_str().map(str::to_string) } else { None });
    if let Some(n) = case.get("notice") {
        let bot_id = n["bot_id"].as_i64().unwrap();
        let error = || n["error"].as_str().unwrap().to_string();
        let notice = match n["mail"].as_str().unwrap() {
            "end_of_funds" => {
                let quote: i64 = c.query_row("SELECT json_extract(settings, '$.quote_asset_id') FROM bots WHERE id = ?1", [bot_id], |r| r.get(0)).unwrap();
                Notice::EndOfFunds { quote_asset_id: Some(quote) }
            }
            "notify_about_error" => Notice::Error { kind: "unknown".into(), error: error() },
            "stopped_by_error" => Notice::StoppedByError { error: error() },
            "stopped_by_amount_limit" => Notice::StoppedByAmountLimit,
            other => panic!("unknown mail {other}"),
        };
        return render::for_notice(c, bot_id, &notice, &from, &urls).unwrap().expect("a mail");
    }
    let to = recipient(c, case["user_id"].as_i64().unwrap(), case["locale"].as_str(), case["to"].as_str());
    match case["account"].as_str().unwrap() {
        "reset_password_instructions" => render::reset_password_instructions(&from, &urls, &to, case["token"].as_str().unwrap()),
        "confirmation_instructions" => render::confirmation_instructions(&from, &urls, &to, case["token"].as_str().unwrap()),
        "email_already_taken" => render::email_already_taken(&from, &urls, &to),
        "test_email" => render::test_email(&from, &urls, &to),
        other => panic!("unknown account mail {other}"),
    }
}

#[test]
fn rails_and_rust_build_the_same_mails_across_the_grid() {
    let (scratch, dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::rails(scratch.path(), "test", &["db:schema:load"]);
    common::rails(scratch.path(), "test", &["runner", "script/rust/mails.rb", dir.path().to_str().unwrap()]);
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("cases.json")).unwrap()).unwrap();
    assert_eq!(cases.len(), 234, "the grid has {} mails", cases.len());
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let now = chrono::Utc::now();
    let mut failures = vec![];
    let mut encodings = std::collections::BTreeMap::new();
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let ours = build(&c, case);
        let (rails, rust) = (comparable(case["rails"].as_str().unwrap()), comparable(&ours.encode(now, "<1@rust.test>").unwrap_or_else(|why| panic!("{name}: {why}"))));
        if rails != rust { failures.push(format!("{name}\n  rails: {rails:?}\n  rust:  {rust:?}")); }
        let envelope = json!({ "from": mailbox(&ours.from).map(|m| m.address), "to": [ours.to] });
        if envelope != case["envelope"] { failures.push(format!("{name}: envelope {envelope}, Rails {}", case["envelope"])); }
        let encoding = rails.split("Content-Transfer-Encoding: ").nth(1).and_then(|rest| rest.split("\r\n").next()).unwrap_or_default().to_string();
        *encodings.entry(encoding).or_insert(0) += 1;
    }
    assert!(failures.is_empty(), "{} of {} mails differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
    // The grid must keep exercising all three transfer encodings, or it stops proving the choice between them.
    assert_eq!(encodings.keys().map(String::as_str).collect::<Vec<_>>(), ["7bit", "base64", "quoted-printable"], "{encodings:?}");
}

#[test]
fn one_changed_character_in_a_mail_is_a_difference() {
    // The comparison is whole-message equality: this pins that the mask hides the two header fields and nothing else.
    let message = "Date: Thu, 10 Sep 2026 12:00:30 +0000\r\nFrom: a@b.c\r\nTo: d@e.f\r\nMessage-ID: <1@x.test>\r\nSubject: s\r\n\r\nbody\r\n";
    assert_eq!(comparable(message), comparable(&message.replace("12:00:30", "13:00:31").replace("<1@x.test>", "<2@y.test>")));
    for (from, to) in [("Subject: s", "Subject: S"), ("From: a@b.c", "From: a@b.d"), ("body", "Body"), ("\r\n\r\nbody", "\r\nX-Extra: 1\r\n\r\nbody")] {
        assert_ne!(comparable(message), comparable(&message.replace(from, to)), "{from:?} -> {to:?}");
    }
    // A body that holds lines shaped like the two masked headers (an error text can): they are body, and compared.
    let shaped = |date: &str, id: &str| format!("{message}Date: {date}\r\nMessage-ID: {id}\r\n");
    let one = shaped("Thu, 10 Sep 2026 12:00:30 +0000", "<1@x.test>");
    assert_eq!(comparable(&one), comparable(&one));
    assert_ne!(comparable(&one), comparable(&shaped("Fri, 11 Sep 2026 12:00:30 +0000", "<1@x.test>")), "a date in the body");
    assert_ne!(comparable(&one), comparable(&shaped("Thu, 10 Sep 2026 12:00:30 +0000", "<2@x.test>")), "a message id in the body");
}

#[test]
#[should_panic(expected = "one Date and one Message-ID")]
fn a_message_with_two_dates_is_not_comparable() {
    comparable("Date: Thu, 10 Sep 2026 12:00:30 +0000\r\nDate: Thu, 10 Sep 2026 12:00:30 +0000\r\nMessage-ID: <1@x.test>\r\n\r\nbody\r\n");
}

// ---- The sender, on a Rails-prepared install ----
use common::seed::{self, BotSpec};
use common::smtp::{self, Behaviour};
use deltabadger::engine::notice::{self, Pending};
use deltabadger::engine::SystemClock;
use deltabadger::mail::sender::Sender;
use deltabadger::store;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

struct Install { _dir: tempfile::TempDir, o: store::Opened, sender_db: Connection, bot_id: i64, quote: i64 }

fn install() -> Install {
    let (dir, o, s) = common::install_alpaca();
    let bot_id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE bots SET label = 'Weekly BTC' WHERE id = ?1", [bot_id]).unwrap();
    let sender_db = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    store::configure(&sender_db).unwrap();
    Install { _dir: dir, o, sender_db, bot_id, quote: s.quote }
}

fn mark(i: &Install, key: &str, marker: Value) {
    i.o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3", (format!("$.{key}"), marker.to_string(), i.bot_id)).unwrap();
}

fn sender(db: Connection, port: u16) -> Sender<SystemClock> {
    let env = move |name: &str| match name {
        "SMTP_ADDRESS" => Some("127.0.0.1".to_string()),
        "SMTP_PORT" => Some(port.to_string()),
        "SMTP_DOMAIN" => Some("bots.example.com".to_string()),
        "SMTP_USER_NAME" => Some("alice".to_string()),
        "SMTP_PASSWORD" => Some("s3cret-pw".to_string()),
        "NOTIFICATIONS_SENDER" => Some("My Bots <bots@example.com>".to_string()),
        "APP_ROOT_URL" => Some("https://bot.example.com".to_string()),
        _ => None,
    };
    let mut s = Sender::new(db, seed::cipher(), &env, SystemClock);
    s.poll = Duration::from_millis(30);
    s.waits = vec![Duration::from_millis(40)];
    s.trust_root = Some(smtp::TLS_PEM.as_bytes().to_vec());
    s
}

async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..400 { if done() { return; } tokio::time::sleep(Duration::from_millis(10)).await; }
    panic!("timed out waiting until {what}");
}

fn markers(i: &Install) -> Vec<Pending> { notice::all_pending(&i.o.primary).unwrap() }

#[tokio::test]
async fn a_marker_becomes_one_mail_and_is_cleared_once_the_server_has_accepted_it() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        let mut i = install();
        let server = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
        mark(&i, notice::FUNDS, notice::funds_marker(Some(i.quote), chrono::Utc::now()));
        mark(&i, notice::STOPPED, notice::stopped_marker("unauthorized.", chrono::Utc::now()));
        let before: String = i.o.primary.query_row("SELECT transient_data FROM bots", [], |r| r.get(0)).unwrap();
        let (stop, stopped) = watch::channel(false);
        let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
        let run = tokio::task::spawn_local(sender(db, server.port).run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("both mails arrived and both markers are gone", || server.messages().len() == 2 && markers(&i).is_empty()).await;
        let sent = server.messages();
        assert!(sent[0].contains("\r\nSubject: Your Alpaca account is running out of USD\r\n") && sent[0].contains("\r\nFrom: My Bots <bots@example.com>\r\nTo: o@example.com\r\n"), "{}", sent[0]);
        assert!(sent[1].contains("\r\nSubject: Weekly BTC has been stopped\r\n") && sent[1].replace("=\r\n", "").contains("Alpaca rejected the API key."), "{}", sent[1]);
        assert_eq!(server.sessions()[0].lines[4..8], ["AUTH PLAIN AGFsaWNlAHMzY3JldC1wdw==", "MAIL FROM:<bots@example.com>", "RCPT TO:<o@example.com>", "DATA"]);
        // Clearing a marker touches nothing else on the row.
        let after: Value = serde_json::from_str(&i.o.primary.query_row::<String, _, _>("SELECT transient_data FROM bots", [], |r| r.get(0)).unwrap()).unwrap();
        let mut expected: Value = serde_json::from_str(&before).unwrap();
        for key in notice::KEYS { expected.as_object_mut().unwrap().remove(key); }
        assert_eq!(after, expected);
        // Nothing more is sent, and the service ends only when it is told to.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(server.messages().len(), 2);
        assert!(!run.is_finished(), "a service with nothing to do waits");
        stop.send(true).unwrap();
        assert_eq!(run.await.unwrap(), Ok(()));
    }).await;
}

#[tokio::test]
async fn a_mail_the_server_did_not_accept_keeps_its_marker_and_is_sent_again() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        let mut i = install();
        mark(&i, notice::ERROR, notice::error_marker(None, "unknown", "order rejected: <qty> too small", chrono::Utc::now()));
        // Refused at the end of DATA: the server had the whole message and said no. Then, as after a restart, a server that accepts.
        let refusing = smtp::start(Behaviour { starttls: true, auth: true, reply: Some((".", "451 4.3.0 try again later")), ..Default::default() }).await;
        let (stop, stopped) = watch::channel(false);
        let db = Connection::open(i.o.primary.path().unwrap()).unwrap();
        store::configure(&db).unwrap();
        let run = tokio::task::spawn_local(sender(db, refusing.port).run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("the delivery was tried at least three times", || refusing.messages().len() >= 3).await;
        assert_eq!(markers(&i).len(), 1, "the marker stays while no server has accepted the mail");
        stop.send(true).unwrap();
        assert_eq!(run.await.unwrap(), Ok(()));

        let accepting = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
        let (stop, stopped) = watch::channel(false);
        let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
        let run = tokio::task::spawn_local(sender(db, accepting.port).run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("the next start sent it", || accepting.messages().len() == 1 && markers(&i).is_empty()).await;
        assert!(accepting.messages()[0].contains("order rejected: &lt;qty&gt; too small"));
        stop.send(true).unwrap();
        run.await.unwrap().unwrap();
    }).await;
}

/// The server took the mail and then never answered QUIT: that is a delivery. One mail, the marker cleared, and no
/// second attempt (a QUIT that failed the delivery would send the mail again at every retry).
#[tokio::test]
async fn a_mail_the_server_accepted_and_a_quit_it_never_answered_is_one_delivery() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        let mut i = install();
        mark(&i, notice::STOPPED, notice::stopped_marker("unauthorized.", chrono::Utc::now()));
        let server = smtp::start(Behaviour { starttls: true, auth: true, silent_quit: true, ..Default::default() }).await;
        let (stop, stopped) = watch::channel(false);
        let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
        let run = tokio::task::spawn_local(sender(db, server.port).run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("the marker was cleared", || markers(&i).is_empty()).await;
        tokio::time::sleep(Duration::from_millis(300)).await; // several looks, and the first retry's wait, later
        assert_eq!((server.messages().len(), server.sessions().len()), (1, 1));
        stop.send(true).unwrap();
        assert_eq!(run.await.unwrap(), Ok(()));
    }).await;
}

/// A server that answers DATA with its 354 and a stale 250 at once, and then refuses the message: nothing was accepted,
/// so the marker stays and the mail is tried again like any failed delivery.
#[tokio::test]
async fn a_reply_nobody_asked_for_is_no_acceptance_and_the_marker_stays() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        let mut i = install();
        mark(&i, notice::STOPPED, notice::stopped_marker("unauthorized.", chrono::Utc::now()));
        let server = smtp::start(Behaviour { starttls: true, auth: true, data_go: Some("354 continue\r\n250 stale\r\n"), reply: Some((".", "554 rejected")), ..Default::default() }).await;
        let (stop, stopped) = watch::channel(false);
        let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
        let run = tokio::task::spawn_local(sender(db, server.port).run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("the delivery was tried twice", || server.sessions().len() >= 2).await;
        assert_eq!((markers(&i).len(), server.messages().len()), (1, 0), "nothing accepted, nothing cleared");
        stop.send(true).unwrap();
        assert_eq!(run.await.unwrap(), Ok(()));
    }).await;
}

#[tokio::test]
async fn a_stop_drops_the_delivery_in_hand_and_leaves_the_marker() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        let mut i = install();
        mark(&i, notice::LIMIT, notice::limit_marker(chrono::Utc::now()));
        let server = smtp::start(Behaviour { silent: true, ..Default::default() }).await;
        let (stop, stopped) = watch::channel(false);
        let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
        let run = tokio::task::spawn_local(sender(db, server.port).run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("the sender is connected and waiting for a greeting", || server.sessions().len() == 1).await;
        let asked = std::time::Instant::now();
        stop.send(true).unwrap();
        assert_eq!(run.await.unwrap(), Ok(()));
        assert!(asked.elapsed() < Duration::from_secs(1), "a stop does not wait for the delivery");
        assert_eq!(markers(&i).len(), 1);
    }).await;
}

#[tokio::test]
async fn a_wake_makes_the_sender_look_at_once_and_a_day_old_marker_is_given_up() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        let mut i = install();
        let server = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
        let (stop, stopped) = watch::channel(false);
        let (wake, woken) = mpsc::unbounded_channel::<&str>();
        let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
        let mut s = sender(db, server.port);
        s.poll = Duration::from_secs(3600); // only a wake can make it look
        let run = tokio::task::spawn_local(s.run(stopped, Some(woken)));
        tokio::time::sleep(Duration::from_millis(100)).await; // the first look found nothing
        mark(&i, notice::STOPPED, notice::stopped_marker("unauthorized.", chrono::Utc::now() - chrono::Duration::hours(25)));
        mark(&i, notice::FUNDS, notice::funds_marker(Some(i.quote), chrono::Utc::now()));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(server.sessions().is_empty(), "nothing woke it");
        wake.send("any engine event").unwrap();
        until("the fresh marker was sent and the stale one cleared", || server.messages().len() == 1 && markers(&i).is_empty()).await;
        assert!(server.messages()[0].contains("running out of USD"), "the day-old stop mail is not sent");
        stop.send(true).unwrap();
        run.await.unwrap().unwrap();
    }).await;
}

#[tokio::test]
async fn with_no_smtp_settings_nothing_is_tried_and_a_marker_waits_for_settings_or_for_its_day_to_end() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        let mut i = install();
        mark(&i, notice::STOPPED, notice::stopped_marker("unauthorized.", chrono::Utc::now() - chrono::Duration::hours(25)));
        mark(&i, notice::FUNDS, notice::funds_marker(Some(i.quote), chrono::Utc::now()));
        // Nothing in the environment, nothing saved in Settings: Rails would deliver to localhost:25 (a listed divergence).
        let db = Connection::open(i.o.primary.path().unwrap()).unwrap();
        store::configure(&db).unwrap();
        let mut unconfigured = Sender::new(db, seed::cipher(), &|_| None, SystemClock);
        unconfigured.poll = Duration::from_millis(30);
        let (stop, stopped) = watch::channel(false);
        let run = tokio::task::spawn_local(unconfigured.run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("the day-old marker was given up", || markers(&i).len() == 1).await;
        tokio::time::sleep(Duration::from_millis(200)).await; // several looks later
        assert_eq!(markers(&i).iter().map(|p| p.notice.mail()).collect::<Vec<_>>(), ["end_of_funds"], "the fresh marker waits");
        stop.send(true).unwrap();
        assert_eq!(run.await.unwrap(), Ok(()));

        // Settings appear (here the environment's, as after a restart): what is still owed goes out.
        let server = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
        let (stop, stopped) = watch::channel(false);
        let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
        let run = tokio::task::spawn_local(sender(db, server.port).run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("the waiting mail was sent", || server.messages().len() == 1 && markers(&i).is_empty()).await;
        stop.send(true).unwrap();
        run.await.unwrap().unwrap();
    }).await;
}

#[tokio::test]
async fn clearing_removes_the_marker_that_was_sent_and_no_other() {
    let i = install();
    let (c, cipher) = (&i.o.primary, seed::cipher());
    let (early, late) = (chrono::Utc::now() - chrono::Duration::minutes(5), chrono::Utc::now());
    mark(&i, notice::FUNDS, notice::funds_marker(Some(i.quote), early));
    let sent = markers(&i).remove(0);
    // Raised again while its mail was on the wire: the new marker is another mail and stays.
    mark(&i, notice::FUNDS, notice::funds_marker(Some(i.quote), late));
    assert!(!notice::clear(c, &cipher, &sent).unwrap());
    assert_eq!(markers(&i).len(), 1);
    assert!(notice::clear(c, &cipher, &markers(&i)[0]).unwrap());
    // Two kinds of error owed: each is cleared on its own, and the last one takes the key with it.
    let errors = notice::error_marker(None, "unknown", "a", early);
    mark(&i, notice::ERROR, notice::error_marker(Some(&errors), "throttle", "b", late));
    let both = markers(&i);
    assert_eq!(both.iter().map(|p| p.notice.clone()).collect::<Vec<_>>(),
               [Notice::Error { kind: "unknown".into(), error: "a".into() }, Notice::Error { kind: "throttle".into(), error: "b".into() }], "oldest first");
    assert!(notice::clear(c, &cipher, &both[0]).unwrap());
    assert_eq!(markers(&i), [both[1].clone()]);
    assert!(notice::clear(c, &cipher, &both[1]).unwrap());
    let left: String = c.query_row("SELECT transient_data FROM bots", [], |r| r.get(0)).unwrap();
    assert_eq!(left, "{}", "nothing of the markers is left on the row");
}

#[tokio::test]
async fn a_marker_the_guard_will_not_let_go_is_mailed_once_and_cleared_when_it_may_be() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        let mut i = install();
        mark(&i, notice::STOPPED, notice::stopped_marker("unauthorized.", chrono::Utc::now()));
        // The clear is a write of `bots` from outside the engine: eligibility::guard has the last word. Here it refuses
        // every such write, for a reason the clear did not cause: a setting this engine does not run.
        i.o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'))", []).unwrap();
        assert!(notice::clear(&i.o.primary, &seed::cipher(), &markers(&i)[0]).is_err());
        let server = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
        let (stop, stopped) = watch::channel(false);
        let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
        let run = tokio::task::spawn_local(sender(db, server.port).run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("the mail arrived", || server.messages().len() == 1).await;
        tokio::time::sleep(Duration::from_millis(300)).await; // ten looks later
        assert_eq!((server.messages().len(), markers(&i).len()), (1, 1), "sent once; the marker stays while the guard refuses");
        // The reason goes away: the marker goes, and no second mail.
        i.o.primary.execute("UPDATE bots SET settings = json_remove(settings, '$.price_limited')", []).unwrap();
        until("the marker was cleared", || markers(&i).is_empty()).await;
        assert_eq!(server.messages().len(), 1);
        stop.send(true).unwrap();
        run.await.unwrap().unwrap();
    }).await;
}

/// A trigger that refuses the clear of the stopped-mail marker, as a full disk or a refusing guard would.
const REFUSE_THE_CLEAR: &str = "CREATE TRIGGER refuse_the_clear BEFORE UPDATE OF transient_data ON bots \
    WHEN json_extract(OLD.transient_data, '$.rust_stopped_mail_pending') IS NOT NULL AND json_extract(NEW.transient_data, '$.rust_stopped_mail_pending') IS NULL \
    BEGIN SELECT RAISE(ABORT, 'the clear is refused'); END";

#[tokio::test]
async fn a_clear_that_fails_is_a_failed_attempt_it_backs_off_and_the_mail_is_not_sent_again() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        // Twice: a mail the server accepted, and a marker that is only to be given up (a day old). Neither may spin.
        for (age_hours, mails) in [(0, 1), (25, 0)] {
            let mut i = install();
            mark(&i, notice::STOPPED, notice::stopped_marker("unauthorized.", chrono::Utc::now() - chrono::Duration::hours(age_hours)));
            i.o.primary.execute_batch(REFUSE_THE_CLEAR).unwrap();
            let server = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
            let (stop, stopped) = watch::channel(false);
            let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
            let mut s = sender(db, server.port);
            s.waits = vec![Duration::from_millis(1500)]; // fifty looks long
            let run = tokio::task::spawn_local(s.run(stopped, None::<mpsc::UnboundedReceiver<()>>));
            until("the mail was sent, or would have been", || server.messages().len() == mails).await;
            tokio::time::sleep(Duration::from_millis(300)).await; // the clear has been tried and refused by now
            // From here the clear would succeed. Inside the backoff nobody tries it: not at the next look, not in a loop.
            i.o.primary.execute_batch("DROP TRIGGER refuse_the_clear").unwrap();
            tokio::time::sleep(Duration::from_millis(400)).await;
            assert_eq!((markers(&i).len(), server.messages().len(), server.sessions().len()), (1, mails, mails), "age {age_hours} h: nothing is retried before the wait is over");
            // After it: cleared, and the mail not sent a second time.
            until("the marker was cleared after the wait", || markers(&i).is_empty()).await;
            assert_eq!(server.messages().len(), mails, "age {age_hours} h");
            // A stop is honoured at once, whatever the sender is waiting for.
            let asked = std::time::Instant::now();
            stop.send(true).unwrap();
            assert_eq!(run.await.unwrap(), Ok(()));
            assert!(asked.elapsed() < Duration::from_secs(1));
        }
    }).await;
}

#[tokio::test]
async fn the_engines_thread_is_never_held_while_the_sender_waits_for_a_locked_database() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        let mut i = install();
        mark(&i, notice::STOPPED, notice::stopped_marker("unauthorized.", chrono::Utc::now()));
        // Another writer holds SQLite's write lock (a long import, a migration). The sender's clear has to wait for it.
        let other = Connection::open(i.o.primary.path().unwrap()).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        // A task on this runtime thread that wants to run every millisecond, as a due tick's timer does: its longest
        // wait is how long anything held the thread.
        let longest = std::rc::Rc::new(std::cell::Cell::new(Duration::ZERO));
        let seen = longest.clone();
        let meter = tokio::task::spawn_local(async move {
            let mut last = std::time::Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(1)).await;
                seen.set(seen.get().max(last.elapsed()));
                last = std::time::Instant::now();
            }
        });
        let server = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
        let (stop, stopped) = watch::channel(false);
        let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
        let run = tokio::task::spawn_local(sender(db, server.port).run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("the mail was sent", || server.messages().len() == 1).await;
        tokio::time::sleep(Duration::from_millis(700)).await; // the clear is waiting for the lock, on the blocking pool
        assert_eq!(markers(&i).len(), 1, "the clear cannot have happened yet");
        assert!(longest.get() < Duration::from_millis(250), "the runtime thread was held for {:?}", longest.get());
        other.execute_batch("ROLLBACK").unwrap();
        until("the clear went through once the lock was gone", || markers(&i).is_empty()).await;
        assert_eq!(server.messages().len(), 1);
        meter.abort();
        stop.send(true).unwrap();
        run.await.unwrap().unwrap();
    }).await;
}

#[tokio::test]
async fn a_label_with_a_line_break_is_not_mailed_and_its_marker_is_cleared() {
    // The sender's future is not Send (it is polled on the engine's thread), so it runs on a LocalSet here.
    tokio::task::LocalSet::new().run_until(async {
        let mut i = install();
        // The label is in the subject. Nothing is cut out of it and nothing is escaped: the mail is not sent at all.
        i.o.primary.execute("UPDATE bots SET label = 'Weekly' || char(13) || char(10) || 'Bcc: evil@example.com'", []).unwrap();
        mark(&i, notice::STOPPED, notice::stopped_marker("unauthorized.", chrono::Utc::now()));
        let server = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
        let (stop, stopped) = watch::channel(false);
        let db = std::mem::replace(&mut i.sender_db, Connection::open_in_memory().unwrap());
        let run = tokio::task::spawn_local(sender(db, server.port).run(stopped, None::<mpsc::UnboundedReceiver<()>>));
        until("the marker was cleared", || markers(&i).is_empty()).await;
        assert!(server.sessions().is_empty(), "nothing was connected for it");
        stop.send(true).unwrap();
        run.await.unwrap().unwrap();
    }).await;
}

#[test]
fn a_label_a_name_and_an_error_are_bounded_in_a_mail_and_ordinary_ones_are_untouched() {
    let i = install();
    let (from, urls) = ("bots@example.com", Urls::from_env(&|_| None));
    let stopped = |error: &str| render::for_notice(&i.o.primary, i.bot_id, &Notice::StoppedByError { error: error.into() }, from, &urls).unwrap().unwrap();
    assert_eq!(stopped("unauthorized.").subject, "Weekly BTC has been stopped");
    // A listed divergence, for sizes no form produces: Rails has no bound.
    i.o.primary.execute("UPDATE bots SET label = ?1", ["ł".repeat(201)]).unwrap();
    i.o.primary.execute("UPDATE users SET name = ?1", ["n".repeat(5000)]).unwrap();
    let mail = stopped(&"e".repeat(5000));
    assert_eq!(mail.subject, format!("{}… has been stopped", "ł".repeat(200)));
    assert!(mail.html.contains(&format!("<p>Hi {}…,</p>", "n".repeat(200))) && !mail.html.contains(&"n".repeat(201)));
    assert!(mail.html.contains(&format!("<b>{}…</b>", "e".repeat(500))) && !mail.html.contains(&"e".repeat(501)));
    assert!(mail.html.len() < 4000, "{} bytes", mail.html.len());
}
