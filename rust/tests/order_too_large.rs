//! D-ORDER-TOO-LARGE-COPY: a venue's refusal of an order above its per-order maximum reads as a plain sentence on the bot
//! page, in the orders list, in the mail and in the desktop notice (Rails shows the venue's text). Display only: the engine
//! handles the refusal exactly as any refusal of no known kind.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::events::EngineEvent;
use deltabadger::engine::tick::{self, Attempts};
use deltabadger::engine::{model, notice, FixedClock};
use deltabadger::mail::render::{self, humanize_error, Urls};
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls as AlpacaUrls};
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::web::i18n;
use serde_json::{json, Value};

const ALPACA_TOO_LARGE: &str = "order notional 1000000 exceeds max notional per order 200000";
const UNCLASSIFIED: &str = "order rejected for a reason no table names";
const T0: &str = "2026-09-01T10:00:00.5Z";
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }

fn en(venue: &str, name: &str, message: &str) -> String { humanize_error(venue, name, "en", message) }
fn de(venue: &str, name: &str, message: &str) -> String { humanize_error(venue, name, "de", message) }

#[test]
fn each_sourced_text_reads_as_the_sentence_in_english_and_german() {
    assert_eq!(en("Exchanges::Alpaca", "Alpaca", ALPACA_TOO_LARGE),
               "Alpaca takes at most 200000 per order. Lower the amount, or split it across more bots.");
    assert_eq!(de("Exchanges::Alpaca", "Alpaca", ALPACA_TOO_LARGE),
               "Alpaca nimmt höchstens 200000 pro Order an. Senke den Betrag oder verteile ihn auf mehrere Bots.");
    let generic_en = |name: &str| format!("The order is larger than {name} allows. Lower the amount, or split it across more bots.");
    let generic_de = |name: &str| format!("Die Order ist größer, als {name} erlaubt. Senke den Betrag oder verteile ihn auf mehrere Bots.");
    for (venue, name, message) in [
        ("Exchanges::Bybit", "Bybit", "Order amount too large."),
        ("Exchanges::Bybit", "Bybit", "Order quantity too large."),
        ("Exchanges::Bybit", "Bybit", "Request order quantity exceeds maximum limit"),
        ("Exchanges::Bybit", "Bybit", "The purchase amount of each order exceeds the estimated maximum purchase amount."),
        ("Exchanges::Bybit", "Bybit", "The sell quantity per order exceeds the estimated maximum sell quantity."),
        ("Exchanges::Bitvavo", "Bitvavo", r#"{"errorCode":210,"error":"`amount` exceeds the maximum for this market."}"#),
        ("Exchanges::Bitvavo", "Bitvavo", r#"{ "error": "x", "errorCode": 210 }"#),
        ("Exchanges::Kucoin", "KuCoin", "KuCoin API error 102434: Exceeds the maximum order size for the currency"),
    ] {
        assert_eq!(en(venue, name, message), generic_en(name), "{venue} {message}");
        assert_eq!(de(venue, name, message), generic_de(name), "{venue} {message}");
    }
}

#[test]
fn only_a_whole_plain_number_is_quoted_as_alpacas_limit() {
    let generic = "The order is larger than Alpaca allows. Lower the amount, or split it across more bots.";
    for message in ["order notional 1000000 exceeds max notional per order 200,000", "order notional 1000000 exceeds max notional per order 2e5",
                    "order notional 1000000 exceeds max notional per order 200000 USD", "order notional 1000000 exceeds max notional per order 200000.",
                    "order notional 1000000 exceeds max notional per order .5", "order notional 1000000 exceeds max notional per order "] {
        assert_eq!(en("Exchanges::Alpaca", "Alpaca", message), generic, "{message:?}");
    }
    assert_eq!(en("Exchanges::Alpaca", "Alpaca", "order notional 1000000 exceeds max notional per order 199999.99"),
               "Alpaca takes at most 199999.99 per order. Lower the amount, or split it across more bots.");
}

#[test]
fn other_texts_keep_what_they_read_today() {
    // No source says Binance's NOTIONAL filter means too large: it is also its too-small answer (D-RAW-REFUSALS-COPY gives
    // it the neutral size-range sentence).
    assert!(!en("Exchanges::Binance", "Binance", "Filter failure: NOTIONAL").contains("larger than"));
    // A table text under another venue stays raw.
    assert_eq!(en("Exchanges::Binance", "Binance", ALPACA_TOO_LARGE), ALPACA_TOO_LARGE);
    assert_eq!(en("Exchanges::Kraken", "Kraken", "Order amount too large."), "Order amount too large.");
    // Bitvavo by its code, never by a substring of it (D-RAW-REFUSALS-COPY shows another code's `error` field).
    assert_eq!(en("Exchanges::Bitvavo", "Bitvavo", r#"{"errorCode":2100,"error":"x"}"#), "x");
    assert_eq!(en("Exchanges::Bitvavo", "Bitvavo", r#"{"errorCode":211,"error":"`price` exceeds the maximum allowed."}"#), "`price` exceeds the maximum allowed.");
    assert_eq!(en("Exchanges::Bitvavo", "Bitvavo", r#"errorCode":210"#), r#"errorCode":210"#);
    // Bybit's price-deviation protection is not this sentence.
    assert_eq!(en("Exchanges::Bybit", "Bybit", "Your order quantity to buy is too large."), "Your order quantity to buy is too large.");
    // A kind the venue already names keeps its sentence.
    assert_eq!(en("Exchanges::Alpaca", "Alpaca", "insufficient buying power"), "Not enough funds on your Alpaca account.");
    assert_eq!(en("Exchanges::Alpaca", "Alpaca", UNCLASSIFIED), UNCLASSIFIED);
}

#[test]
fn every_locale_has_both_sentences_of_its_own() {
    for locale in deltabadger::web::locale::LOCALES {
        for key in ["order_too_large", "order_too_large_limit"] {
            let full = format!("{locale}.errors.exchange.{key}");
            let text = i18n::all().iter().find(|(k, _)| *k == full).unwrap_or_else(|| panic!("{full} missing")).1;
            assert!(text.contains("%{exchange}"), "{full}: {text}");
            assert_eq!(text.contains("%{limit}"), key.ends_with("_limit"), "{full}: {text}");
        }
        let out = humanize_error("Exchanges::Alpaca", "Alpaca", locale, ALPACA_TOO_LARGE);
        assert!(out.contains("Alpaca") && out.contains("200000") && !out.contains("%{"), "{locale}: {out}");
    }
}

// --- end to end: an Alpaca bot refused for its size -------------------------------------------------------------------

struct Install { dir: tempfile::TempDir, o: store::Opened, s: seed::Seeded, bot: i64 }
fn install() -> Install {
    let (dir, o, s) = common::install_alpaca();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("interval", json!("hour")));
    Install { dir, o, s, bot }
}
fn refusing(message: &str) -> ScriptedTransport {
    let ok = |body: Value| json!([{ "status": 200, "body": body }]);
    ScriptedTransport::from_script(&json!({
        "GET /v1beta3/crypto/us/latest/quotes": ok(json!({ "quotes": { "BTC/USD": { "ap": 64321.5, "bp": 64300.25 } } })),
        "GET /v1beta3/crypto/us/latest/trades": ok(json!({ "trades": { "BTC/USD": { "p": 64310.75 } } })),
        "GET /v2/account": ok(json!({ "cash": "100000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([])),
        "POST /v2/orders": [{ "status": 403, "body": { "code": 40310000, "message": message } }],
    }))
}
/// What the engine kept of the refusals.
fn stored(i: &Install) -> String {
    let q = |sql: &str| -> Vec<String> {
        let mut st = i.o.primary.prepare(sql).unwrap();
        st.query_map([i.bot], |r| r.get::<_, Option<String>>(0)).unwrap().map(|v| v.unwrap().unwrap_or_default()).collect()
    };
    let bot = model::load_bot(&i.o.primary, i.bot).unwrap();
    let transient = serde_json::to_string(&bot.transient).unwrap();
    format!("status {:?}\ntransient {transient}\nrows {:?}\nlogs {:?}", bot.status,
            q("SELECT status || ' ' || coalesce(error_messages, '') FROM transactions WHERE bot_id = ?1 ORDER BY id"),
            q("SELECT event || ' ' || level || ' ' || details FROM bot_activity_logs WHERE bot_id = ?1 ORDER BY id"))
}
/// The same, the raw text and the install's own key digest (a random nonce) replaced so that two twins compare.
fn state(i: &Install, raw: &str) -> String {
    let bot = model::load_bot(&i.o.primary, i.bot).unwrap();
    let digest = bot.transient["rust_failure_origin"]["ciphertext_digest"].as_str().unwrap().to_string();
    stored(i).replace(raw, "RAW").replace(&digest, "DIGEST")
}

#[tokio::test(flavor = "current_thread")]
async fn the_engine_treats_a_too_large_refusal_as_any_unclassified_one() {
    let (big, other) = (install(), install());
    for hours in [0, 1, 26] {
        let now = at(T0) + Duration::hours(hours);
        for (i, raw) in [(&big, ALPACA_TOO_LARGE), (&other, UNCLASSIFIED)] {
            let t = refusing(raw);
            tick::tick(&i.o.primary, &AlpacaVenue::new(t.clone(), AlpacaUrls::for_passphrase(Some("paper"))), i.bot, &FixedClock(now), &mut Attempts::default()).await.unwrap();
            assert_eq!(t.posted_orders().len(), 1, "+{hours} h: one refusal");
            let stored = stored(i);
            assert!(stored.contains(raw), "+{hours} h: the raw text is what is stored: {stored}");
        }
        assert_eq!(state(&big, ALPACA_TOO_LARGE), state(&other, UNCLASSIFIED), "+{hours} h");
    }
    let bot = model::load_bot(&big.o.primary, big.bot).unwrap();
    assert_eq!(bot.last_failure_kind(), None);
    assert!(bot.transient["failure_notifications"].get("unknown").is_some(), "{:?}", bot.transient);
}

#[tokio::test(flavor = "current_thread")]
async fn the_bot_page_the_orders_list_the_mail_and_the_desktop_notice_read_the_sentence() {
    let sentence = "Alpaca takes at most 200000 per order. Lower the amount, or split it across more bots.";
    let i = install();
    let t = refusing(ALPACA_TOO_LARGE);
    tick::tick(&i.o.primary, &AlpacaVenue::new(t, AlpacaUrls::for_passphrase(Some("paper"))), i.bot, &FixedClock(at(T0)), &mut Attempts::default()).await.unwrap();
    let app = common::web::app(i.dir.path(), common::web::SECRET, common::web::TestClock::at(T0));

    // The status bar, as the stream redraws it after the run.
    let bar = deltabadger::web::bot::live::render(&i.o.primary, &app, &EngineEvent::TickEnded { bot_id: i.bot }).unwrap()
        .into_iter().map(|(_, html)| html).find(|html| html.contains("target=\"status_bar_")).expect("a status bar");
    assert!(bar.contains(sentence) && !bar.contains("max notional"), "{bar}");

    // The bot page and its orders list.
    i.o.primary.execute("UPDATE users SET confirmed_at = '2026-01-01 00:00:00'", []).unwrap();
    let hash: String = i.o.primary.query_row("SELECT encrypted_password FROM users WHERE id = ?1", [i.s.user_id], |r| r.get(0)).unwrap();
    use deltabadger::web::session;
    let data = session::SessionData { user: Some((i.s.user_id, hash.chars().take(29).collect())), csrf: Some(deltabadger::web::csrf::new_token()), ..Default::default() };
    let browser = || common::web::Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None };
    let page = browser().send(&app, "GET", &format!("/bots/{}", i.bot), None, common::web::Csrf::None, &[]).await;
    assert_eq!(page.status, 200, "{:?}", page.header("location"));
    assert!(page.body.contains(sentence) && !page.body.contains("max notional"), "{}", page.body);
    let feed = browser().send(&app, "GET", &format!("/bots/{}.turbo_stream", i.bot), None, common::web::Csrf::None, &[("Turbo-Frame", "orders_pagination")]).await;
    assert!(feed.body.contains(sentence) && !feed.body.contains("max notional"), "{}", feed.body);

    // The error notice the run owes: the mail and the desktop notice.
    let bot = model::load_bot(&i.o.primary, i.bot).unwrap();
    let pending = notice::pending_in(i.bot, bot.transient.as_object().unwrap());
    let owed = pending.iter().find(|p| matches!(p.notice, notice::Notice::Error { .. })).unwrap_or_else(|| panic!("an error notice: {:?}", bot.transient));
    let mail = render::for_notice(&i.o.primary, i.bot, &owed.notice, "bot@example.com", &Urls::from_env(&|_| None)).unwrap().expect("a mail");
    assert!(mail.html.contains(sentence) && !mail.html.contains("max notional"), "{}", mail.html);
    let facts = render::facts(&i.o.primary, i.bot, &owed.notice).unwrap().expect("facts");
    let (title, body) = render::desktop_text(&facts, &owed.notice);
    assert!(body.contains(sentence) && !body.contains("max notional"), "{title} / {body}");
}
