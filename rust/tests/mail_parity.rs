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
