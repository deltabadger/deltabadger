//! What rust/src/mail and engine::notice port from Ruby, against values Ruby produced (script/rust/mail_vectors.rb):
//! the mail gem's message encoding, Float#to_s, Exchange#humanize_error, SmtpSettings.current and the sender address,
//! production.rb's root URL. Rails-free.
use chrono::{TimeZone, Utc};
use deltabadger::mail::{mailbox, Message};
use deltabadger::ruby::{float_to_s, to_i};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/mail_vectors.json")).expect("mail_vectors.json parses") }

#[test]
fn every_recorded_message_is_encoded_as_the_mail_gem_encodes_it() {
    let date = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 30).unwrap();
    let cases = vectors()["messages"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 36, "{} messages", cases.len());
    let mut failures = vec![];
    for c in &cases {
        let text = |k: &str| c[k].as_str().unwrap().to_string();
        let message = Message { from: text("from"), reply_to: c["reply_to"].as_str().map(str::to_string), to: text("to"), subject: text("subject"), html: text("html") };
        let ours = message.encode(date, "<vector@deltabadger.test>").unwrap_or_else(|why| panic!("{:?}: {why}", c["subject"]));
        if ours != text("encoded") { failures.push(format!("subject {:?}, from {:?}, body {:.40?}\n  ruby: {:?}\n  rust: {ours:?}", c["subject"], c["from"], c["html"], c["encoded"])); }
        assert!(ours.is_ascii(), "a message on the wire is ASCII");
        assert_eq!(mailbox(&message.from).map(|m| m.address), c["envelope_from"].as_str().map(str::to_string), "envelope sender of {:?}", c["from"]);
        assert_eq!(json!([message.to.trim()]), c["envelope_to"], "envelope recipient");
    }
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

#[test]
fn a_display_name_outside_ascii_is_dropped() {
    // The listed divergence: the gem would write `=?UTF-8?B?…?= <a@b.c>`.
    let date = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 30).unwrap();
    let message = Message { from: "Zażółć <a@b.c>".into(), reply_to: None, to: "o@example.com".into(), subject: "s".into(), html: "x".into() };
    assert!(message.encode(date, "<i@d>").unwrap().contains("\r\nFrom: a@b.c\r\n"));
    assert_eq!(mailbox("Zażółć <a@b.c>").unwrap().address, "a@b.c");
}

/// Nothing that becomes a header may bring a line of its own: not a display name, an address, a subject (a bot's label
/// is in it) or the message id. Such a mail is refused whole; the mail gem would send it (it writes `=0A` into a
/// subject, and a line break in a display name as it is).
#[test]
fn a_value_with_a_control_character_never_reaches_a_header() {
    let date = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 30).unwrap();
    let good = Message { from: "My Bots <bots@example.com>".into(), reply_to: Some("bots@example.com".into()), to: "owner@example.com".into(),
                         subject: "Weekly BTC has been stopped".into(), html: "<p>x</p>\n".into() };
    assert!(good.encode(date, "<1@bots.example.com>").is_ok());
    let refused = |change: &dyn Fn(&mut Message), id: &str| { let mut m = good.clone(); change(&mut m); m.encode(date, id) };
    for from in ["Ops\r\nBcc: evil@example.com <bots@example.com>", "Ops\nX-Injected: yes <bots@example.com>", "\"Ops\r\n\" <bots@example.com>",
                 "bots@example.com\r\nBcc: evil@example.com", "Ops\u{0} <bots@example.com>", "  ", "not an address", "Ops <bots@example.com\r\n>"] {
        assert!(refused(&|m| m.from = from.into(), "<1@x.test>").is_err(), "sender {from:?}");
        assert!(refused(&|m| m.reply_to = Some(from.into()), "<1@x.test>").is_err(), "reply-to {from:?}");
    }
    for to in ["owner@example.com\r\nBcc: evil@example.com", "owner@example.com\nSubject: forged", "Owner <owner@example.com>", "owner@exa\tmple.com", "żółw@example.com", ""] {
        assert!(refused(&|m| m.to = to.into(), "<1@x.test>").is_err(), "recipient {to:?}");
    }
    for subject in ["Weekly\r\nBcc: evil@example.com has been stopped", "Weekly\nBTC", "Weekly\rBTC", "Weekly\tBTC", "Weekly\u{0}BTC", "Weekly\u{7f}BTC", "Zażółć\u{85}BTC"] {
        assert_eq!(refused(&|m| m.subject = subject.into(), "<1@x.test>"), Err("the subject holds a control character"), "{subject:?}");
    }
    for id in ["<1@x.test>\r\nBcc: evil@example.com", "<1@x .test>", "<1@x.test>\n"] { assert!(refused(&|_| {}, id).is_err(), "message id {id:?}"); }
    // The body is another matter: it is encoded, and what it says stays below the blank line.
    let body = "<p>error: unauthorized.\r\nBcc: evil@example.com\r\n\r\nDate: now</p>\n";
    let wire = refused(&|m| m.html = body.into(), "<1@x.test>").unwrap();
    let (headers, _) = wire.split_once("\r\n\r\n").unwrap();
    assert!(!headers.contains("Bcc") && headers.lines().count() == 10, "{headers}");
}

#[test]
fn floats_print_as_ruby_prints_them() {
    for pair in vectors()["floats"].as_array().unwrap() {
        let Some(bits) = pair[0]["f"].as_str() else { continue }; // an Integer: Rust's own to_string
        let f = f64::from_bits(u64::from_str_radix(bits, 16).unwrap());
        assert_eq!(float_to_s(f), pair[1].as_str().unwrap(), "{f:e}");
    }
}

#[test]
fn text_is_read_as_a_number_as_ruby_reads_it() {
    let cases = vectors()["ints"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 19);
    for pair in &cases { assert_eq!(to_i(pair[0].as_str().unwrap()), pair[1].as_i64().unwrap(), "{:?}.to_i", pair[0]); }
}

#[test]
fn the_ruby_gems_the_vectors_came_from_are_the_ones_in_the_lockfile() {
    let lock = include_str!("../../Gemfile.lock");
    for (gem, version) in vectors()["gems"].as_object().unwrap() {
        let line = format!("    {gem} ({})", version.as_str().unwrap());
        assert!(lock.contains(&line), "{gem} moved: re-run script/rust/mail_vectors.rb and the mail parity grid (expected `{line}`)");
    }
}

#[test]
fn the_ported_ruby_is_the_ruby_that_was_recorded() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let sources = vectors()["ported_sources"].as_object().unwrap().clone();
    assert_eq!(sources.len(), 19);
    for (file, recorded) in &sources {
        let now = hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap()));
        assert_eq!(&now, recorded.as_str().unwrap(), "{file} changed: re-check rust/src/mail against it, re-run script/rust/mail_vectors.rb and the mail parity grid");
    }
}

// ---- The engine's mail markers (engine::notice) ----
use deltabadger::engine::notice::Notice;

#[test]
fn markers_are_read_back_as_the_notices_they_were_written_for() {
    use deltabadger::engine::notice::{self, Pending};
    let now = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 30).unwrap();
    let at = "2026-09-10T12:00:30.000Z".to_string();
    let errors = notice::error_marker(None, "unknown", "a \"b\" <c>", now);
    let errors = notice::error_marker(Some(&errors), "throttle", "EAPI:Rate limit exceeded", now);
    let transient = json!({ "last_failure_kind": "throttle", notice::FUNDS: notice::funds_marker(Some(2), now), notice::ERROR: errors,
                            notice::STOPPED: notice::stopped_marker("unauthorized.", now), notice::LIMIT: notice::limit_marker(now) });
    let found = notice::pending_in(7, transient.as_object().unwrap());
    let pending = |notice| Pending { bot_id: 7, stamped_at: at.clone(), notice };
    // An error is bounded when its marker is written: 500 characters and an ellipsis, whatever the venue sent.
    let long = notice::stopped_marker(&"é".repeat(600), now);
    assert_eq!(long["error"].as_str().unwrap().chars().count(), 501);
    assert!(long["error"].as_str().unwrap().ends_with('…'));
    assert_eq!(notice::bounded("short", 500), "short");
    assert_eq!(found, [pending(Notice::EndOfFunds { quote_asset_id: Some(2) }),
                       pending(Notice::Error { kind: "unknown".into(), error: "a \"b\" <c>".into() }),
                       pending(Notice::Error { kind: "throttle".into(), error: "EAPI:Rate limit exceeded".into() }),
                       pending(Notice::StoppedByError { error: "unauthorized.".into() }), pending(Notice::StoppedByAmountLimit)]);
    assert_eq!(found.iter().map(|p| p.notice.mail()).collect::<Vec<_>>(), ["end_of_funds", "notify_about_error", "notify_about_error", "stopped_by_error", "stopped_by_amount_limit"]);
    assert_eq!(found[0].raised_at(), Some(now));
    // A marker of another shape is not a notice, and a kind that is not a plain word is never turned into a JSON path.
    let odd = json!({ notice::FUNDS: "yes", notice::STOPPED: { "stamped_at": "2026-09-10T12:00:30.000Z" }, notice::ERROR: { "a.b": { "error": "x", "stamped_at": "y" } }, notice::LIMIT: {} });
    assert_eq!(notice::pending_in(7, odd.as_object().unwrap()), []);
}

// ---- Where mail goes (mail::smtp) ----
use deltabadger::mail::smtp;

fn env_of(case: &Value) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| case["env"].get(name).and_then(Value::as_str).map(str::to_string)
}

#[test]
fn smtp_settings_and_the_sender_resolve_as_rails_resolves_them() {
    let v = vectors();
    let defaults = &v["smtp_defaults"];
    assert_eq!((defaults["open_timeout"].as_u64(), defaults["read_timeout"].as_u64()), (Some(5), Some(5)));
    let cases = v["smtp"].as_array().unwrap();
    assert_eq!(cases.len(), 20);
    let (mut configured, mut refused) = (0, 0);
    for c in cases {
        let env = smtp::Env::read(&env_of(c));
        let config = |key: &str| c["app_config"].get(key).and_then(Value::as_str).map(str::to_string);
        assert_eq!(smtp::notifications_sender(&env, &config), c["sender"].as_str().unwrap(), "sender of {c}");
        // What Mail::SMTP ends up with: SmtpSettings.current over the mail gem's defaults.
        let rails = |key: &str| c["settings"].get(key).filter(|v| !v.is_null()).unwrap_or(&defaults[key]).clone();
        let text = |v: Value| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
        let ours = match smtp::Settings::current(&env, &config) {
            Ok(ours) => ours,
            // Rails' nil is "deliver to localhost:25"; here it is "mail is not configured" (a listed divergence).
            Err(why) if c["settings"].is_null() => { assert_eq!(why, "no SMTP_ADDRESS, and no SMTP settings saved", "{c}"); continue }
            // The one recorded configuration Rails accepts and this crate does not: an empty SMTP_DOMAIN, which Rails
            // would send as `EHLO ` (a listed divergence).
            Err(why) => { assert_eq!((why.as_str(), text(rails("domain")).as_str()), ("SMTP_DOMAIN is not a host name or an address literal", ""), "{c}"); refused += 1; continue }
        };
        assert!(!c["settings"].is_null(), "not configured in Rails, configured here: {c}");
        assert_eq!(ours.address, text(rails("address")), "address of {c}");
        assert_eq!(ours.port, text(rails("port")), "port of {c}");
        assert_eq!(ours.domain, text(rails("domain")), "domain of {c}");
        assert_eq!(ours.credentials, (text(rails("user_name")), text(rails("password"))), "credentials of {c}");
        assert_eq!((ours.open_timeout.as_secs(), ours.read_timeout.as_secs()), (5, 5));
        // STARTTLS is required exactly when there is a user name or a password to protect.
        assert_eq!(ours.has_secret(), !ours.credentials.0.is_empty() || !ours.credentials.1.is_empty());
        configured += 1;
    }
    assert_eq!((configured, refused), (13, 1), "of the 20 recorded configurations; the other six are \"not configured\" in Rails too");
}

#[test]
fn settings_never_print_their_password() {
    let env = smtp::Env::read(&|name| match name { "SMTP_ADDRESS" => Some("mail.example.com".into()), "SMTP_USER_NAME" => Some("alice".into()), "SMTP_PASSWORD" => Some("s3cret-pw".into()), _ => None });
    let printed = format!("{:?}", smtp::Settings::current(&env, &|_| None).unwrap());
    assert!(!printed.contains("s3cret-pw") && !printed.contains("alice"), "{printed}");
    // smtp::Env holds the password too and cannot be printed at all: it has no Debug.
}

// ---- The views (mail::render) ----
use deltabadger::mail::render;
use deltabadger::web::{i18n, locale};

#[test]
fn errors_are_humanised_as_exchange_humanize_error_does() {
    let cases = vectors()["humanize"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 120);
    for c in &cases {
        let s = |k: &str| c[k].as_str().unwrap();
        assert_eq!(render::humanize_error(s("exchange_type"), s("name"), s("locale"), s("message")), s("out"), "{} {} {:?}", s("exchange_type"), s("locale"), s("message"));
    }
}

#[test]
fn the_root_url_is_what_production_rb_derives() {
    let cases = vectors()["urls"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 10);
    for c in &cases { assert_eq!(render::Urls::from_env(&env_of(c)).root, c["root"].as_str().unwrap(), "{}", c["env"]); }
}

#[test]
fn a_stored_locale_rails_does_not_know_is_english_and_a_missing_one_is_the_default() {
    // A listed divergence: Rails raises I18n::InvalidLocale for the first and sends nothing.
    assert_eq!((render::user_locale(Some("xx")), render::user_locale(None), render::user_locale(Some("pl"))), ("en", "en", "pl"));
}

#[test]
fn every_text_a_mail_uses_exists_in_every_locale() {
    let keys = ["mailer.greeting_name", "test_mailer.test_email.subject", "test_mailer.test_email.body",
                "bot_alerts_mailer.end_of_funds.subject", "bot_alerts_mailer.end_of_funds.template_html",
                "bot_alerts_mailer.notify_about_error.subject", "bot_alerts_mailer.notify_about_error.template_html",
                "bot_alerts_mailer.stopped_by_error.subject", "bot_alerts_mailer.stopped_by_error.template_html",
                "bot_alerts_mailer.stopped_by_amount_limit.subject", "bot_alerts_mailer.stopped_by_amount_limit.template_html",
                "devise.mailer.confirmation_instructions.subject", "devise.mailer.confirmation_instructions.template_html",
                "devise.mailer.confirmation_instructions.confirm_button", "devise.mailer.reset_password_instructions.subject",
                "devise.mailer.reset_password_instructions.template_html", "devise.mailer.reset_password_instructions.change_password_button",
                "devise.mailer.reset_password_instructions.ignore_mail_html", "devise.mailer.reset_password_instructions.wont_change_html",
                "devise.mailer.email_already_taken.subject", "devise.mailer.email_already_taken.message",
                "devise.mailer.email_already_taken.reset_password_message", "devise.mailer.email_already_taken.reset_password_button",
                "errors.exchange.regional_restriction", "errors.exchange.transient_nonce", "errors.exchange.transient_unavailable",
                "errors.exchange.insufficient_funds", "errors.exchange.invalid_key", "errors.exchange.permission_denied",
                "errors.exchange.restricted", "errors.exchange.rate_limited"];
    let table: std::collections::HashSet<&str> = i18n::all().iter().map(|(k, _)| *k).collect();
    let missing: Vec<String> = locale::LOCALES.iter().flat_map(|l| keys.iter().map(move |k| format!("{l}.{k}"))).filter(|k| !table.contains(k.as_str())).collect();
    assert!(missing.is_empty(), "a mail in these locales would fall back to English, which the parity grid does not cover: {missing:?}");
}

// ---- The sender's waits (mail::sender) ----
use deltabadger::mail::sender;

#[test]
fn the_first_retries_wait_as_long_as_the_delivery_job_does() {
    let ruby: Vec<u64> = vectors()["retry_waits"].as_array().unwrap().iter().map(|w| w.as_u64().unwrap()).collect();
    let dir = tempfile::tempdir().unwrap();
    let db = rusqlite::Connection::open(dir.path().join("x.sqlite3")).unwrap();
    let cipher = deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "mail-test-secret").unwrap());
    let s = sender::Sender::new(db, cipher, &|_| None, deltabadger::engine::SystemClock);
    let ours: Vec<u64> = s.waits.iter().map(|w| w.as_secs()).collect();
    assert_eq!(ours[..4], ruby[..], "ApplicationMailDeliveryJob: polynomially_longer, attempts 2 to 5");
    assert_eq!(ours[4..], [3600], "then hourly, until the marker is seven days old");
}
