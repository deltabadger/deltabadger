//! PLAN-B2 R3, R4, r1.4, r1.8, r1.12, r2.6 (B2-5, D-UPGRADE-SET-ASIDE): a takeover stops the working bots and rules this
//! build refuses, with their reasons in the owner's words, and starts; only work in flight or data it can't read stops it.
mod common;
use common::seed::{self, BotSpec};
use deltabadger::engine::{handover, model, notice, set_aside, EngineError};
use deltabadger::enums::BotStatus;
use deltabadger::{lease, store};
use serde_json::json;

fn now() -> chrono::DateTime<chrono::Utc> { "2026-09-10T12:00:00Z".parse().unwrap() }
fn plain() -> BotSpec { BotSpec::weekly(60.0, "2099-01-01 00:00:00") }
fn take_over(dir: &std::path::Path, o: &store::Opened) -> Result<handover::Takeover, EngineError> {
    let lock = lease::lock(&store::Paths::from_env(&|_| None, dir), now()).unwrap();
    handover::take_over(&lock, o, &seed::cipher(), "test", now())
}
fn open_order(c: &rusqlite::Connection, s: &seed::Seeded, bot: i64) {
    seed::insert_row(c, s, bot, s.btc, &json!({"external_id":"WAIT", "external_status":0, "side":0,
        "amount":"0.001", "quote_amount":"60", "amount_exec":"0", "quote_amount_exec":"0", "created_at":"2026-09-01 10:00:01"}));
}
fn reasons(c: &rusqlite::Connection, bot: i64) -> Vec<String> {
    set_aside::reasons(model::load_bot(c, bot).unwrap().transient.get(set_aside::KEY)).unwrap_or_default()
}
fn status(c: &rusqlite::Connection, bot: i64) -> BotStatus { model::load_bot(c, bot).unwrap().status }
/// Every bot row and the counts of what a stop writes: equal before and after a refused takeover.
fn state(c: &rusqlite::Connection) -> (String, i64, i64) {
    (c.query_row("SELECT group_concat(id || ':' || status || ':' || transient_data || ':' || coalesce(stop_message_key, ''), ';') FROM bots", [], |r| r.get(0)).unwrap(),
     c.query_row("SELECT count(*) FROM bot_activity_logs", [], |r| r.get(0)).unwrap(),
     c.query_row("SELECT count(*) FROM rule_logs", [], |r| r.get(0)).unwrap())
}
const FEATURE: &str = "It uses a setting that isn't supported on this bot yet. Check its settings, then start the bot again.";
const NO_KEY: &str = "There's no Kraken API key. Add your key in Settings, then start the bot again.";

/// Each candidate kind (rows 24, 25, 28, r1.8) is set aside with its reason in the same takeover, and the healthy bot runs.
#[test]
fn each_kind_of_candidate_is_stopped_with_its_reason_and_the_other_bots_run() {
    let (dir, o, s) = common::install();
    let c = &o.primary;
    let healthy = seed::insert_bot(c, &s, &plain());
    let feature = seed::insert_bot(c, &s, &plain().with("future_price_drop_limited", json!(true)));
    // A flip whose target (selling) is refused: its sell cadence is unreadable.
    let flip = seed::insert_bot(c, &s, &plain().with("price_limited", json!(true)).with("price_limit_action", json!("start_selling")).with("sell_interval", json!("nonsense")));
    c.execute("INSERT INTO exchanges (type, name, created_at, updated_at) VALUES ('Exchanges::Bitrue', 'Bitrue', '2026-01-01', '2026-01-01')", []).unwrap();
    let bitrue = c.last_insert_rowid();
    c.execute("INSERT INTO rules (type, status, user_id, exchange_id, settings, created_at, updated_at) VALUES ('Rules::Withdrawal', 1, ?1, ?2, '{}', '2026-01-01', '2026-01-01')", [s.user_id, bitrue]).unwrap();
    let rule = c.last_insert_rowid();
    let t = take_over(dir.path(), &o).unwrap();
    assert_eq!(t.eligible, vec![healthy], "the healthy bot runs");
    assert_eq!(t.set_aside.iter().map(|a| a.id).collect::<Vec<_>>(), vec![feature, flip]);
    assert_eq!(t.rules_stopped, vec![rule]);
    assert_eq!(status(c, healthy), BotStatus::Scheduled);
    assert_eq!(reasons(c, feature), vec![FEATURE.to_string()]);
    assert_eq!(reasons(c, flip), vec!["When the price condition is met it would start selling, and it couldn't run that way. Check its selling settings, or change what the condition does, then start the bot again.".to_string()]);
    for bot in [feature, flip] {
        assert_eq!(status(c, bot), BotStatus::Stopped);
        let key: String = c.query_row("SELECT stop_message_key FROM bots WHERE id = ?1", [bot], |r| r.get(0)).unwrap();
        assert_eq!(key, set_aside::STOP_KEY);
    }
    let (rule_status, log): (i64, String) = c.query_row("SELECT r.status, l.message FROM rules r JOIN rule_logs l ON l.rule_id = r.id WHERE r.id = ?1", [rule], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(rule_status, 2, "Rails' stopped");
    assert_eq!(log, "Withdrawals from Bitrue aren't supported yet, so this rule was stopped while updating your data. Nothing was withdrawn.");
    // A restart finds nothing more to set aside: the stopped bots are no longer refused.
    let again = take_over(dir.path(), &o).unwrap();
    assert!(again.set_aside.is_empty() && again.rules_stopped.is_empty());
    assert_eq!(again.eligible, vec![healthy]);
}

/// r1.12: the stop carries its reasons on the row, on its activity line and in its mail; the row's survive the mail.
#[test]
fn the_reason_is_stored_on_the_row_the_activity_line_and_the_mail() {
    let (dir, o, s) = common::install();
    let c = &o.primary;
    let bot = seed::insert_bot(c, &s, &plain().with("future_price_drop_limited", json!(true)));
    take_over(dir.path(), &o).unwrap();
    let (details, n): (String, i64) = c.query_row("SELECT details, count(*) FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'stopped'", [bot], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(n, 1);
    let details: serde_json::Value = serde_json::from_str(&details).unwrap();
    assert_eq!((details["stop_message_key"].as_str(), details["reason"].as_str()), (Some(set_aside::STOP_KEY), Some(FEATURE)));
    let transient = model::load_bot(c, bot).unwrap().transient;
    let pending = notice::pending_in(bot, transient.as_object().unwrap());
    let Some(notice::Pending { notice: notice::Notice::SetAside { reasons: said }, .. }) = pending.first() else { panic!("{pending:?}") };
    assert_eq!(said, FEATURE);
    assert_eq!(pending.len(), 1, "its own mail, not the stopped-by-error one too");
    let urls = deltabadger::mail::render::Urls { root: "http://localhost:3000/".into() };
    let mail = deltabadger::mail::render::for_notice(c, bot, &pending[0].notice, "bot@example.com", &urls).unwrap().unwrap();
    let label: String = c.query_row("SELECT coalesce(label, '') FROM bots WHERE id = ?1", [bot], |r| r.get(0)).unwrap();
    assert_eq!(mail.subject, format!("{label} was stopped"));
    assert!(mail.html.contains("Deltabadger stopped your bot") && mail.html.contains("Nothing was bought or sold.")
        && mail.html.contains(&deltabadger::web::i18n::escape(FEATURE)), "{}", mail.html);
    assert!(!mail.html.contains("rejecting") && !mail.html.contains("3.0"), "{}", mail.html);
    assert!(notice::clear(c, &seed::cipher(), &pending[0]).unwrap());
    assert_eq!(reasons(c, bot), vec![FEATURE.to_string()], "the mail sent, the row keeps its reasons");
}

/// r1.4: a working bot refused and without a key is stopped once, with both reasons.
#[test]
fn an_ineligible_bot_without_a_key_is_stopped_once_with_both_reasons() {
    let (dir, o, s) = common::install();
    let bot = seed::insert_bot(&o.primary, &s, &plain().with("future_price_drop_limited", json!(true)));
    o.primary.execute("DELETE FROM api_keys", []).unwrap();
    let t = take_over(dir.path(), &o).unwrap();
    assert_eq!(t.set_aside.len(), 1);
    assert_eq!(reasons(&o.primary, bot), vec![FEATURE.to_string(), NO_KEY.to_string()]);
    let stops: i64 = o.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'stopped'", [bot], |r| r.get(0)).unwrap();
    assert_eq!(stops, 1);
}

/// r1.4 and rows 26, 27: what a stop cannot settle is a hard stop, C6 naming each bot, and nothing is written.
#[test]
fn work_in_flight_and_unreadable_rows_stay_hard_stops_and_write_nothing() {
    type Make = Box<dyn Fn(&rusqlite::Connection, &seed::Seeded) -> (i64, &'static str)>;
    let cases: Vec<(&str, Make)> = vec![
        ("ineligible and rebalancing", Box::new(|c, s| {
            let bot = seed::insert_bot(c, s, &plain().with("future_price_drop_limited", json!(true)).with("rebalance_enabled", json!(true)).with("rebalance_threshold", json!(0.05)));
            (bot, "It still rebalances, in a way that isn't supported yet.")
        })),
        ("no key and an open order", Box::new(|c, s| {
            let bot = seed::insert_bot(c, s, &plain());
            open_order(c, s, bot);
            c.execute("DELETE FROM api_keys", []).unwrap();
            (bot, "It has open orders, and there's no Kraken API key to look after them.")
        })),
        ("ineligible with an open order", Box::new(|c, s| {
            let bot = seed::insert_bot(c, s, &plain().with("future_price_drop_limited", json!(true)));
            open_order(c, s, bot);
            (bot, "It has open orders.")
        })),
        ("an unreadable row", Box::new(|c, s| {
            let bot = seed::insert_bot(c, s, &BotSpec { status: 2, ..plain() });
            c.execute("UPDATE bots SET started_at = 'garbage' WHERE id = ?1", [bot]).unwrap();
            (bot, "This bot's data can't be read.")
        })),
    ];
    for (case, make) in cases {
        let (dir, o, s) = common::install();
        let other = seed::insert_bot(&o.primary, &s, &plain().with("future_price_drop_limited", json!(true)));
        let (bot, line) = make(&o.primary, &s);
        let before = state(&o.primary);
        match take_over(dir.path(), &o) {
            Err(EngineError::HardStop(message)) => {
                assert!(message.contains(&format!("Bot {bot}: {line}")), "{case}: {message}");
                assert!(!message.contains(&format!("Bot {other}")), "{case}: a bot that could be set aside is not named: {message}");
            }
            other => panic!("{case}: {:?}", other.map(|t| t.eligible)),
        }
        assert_eq!(state(&o.primary), before, "{case}: rolled back");
        assert!(lease::read(&o.primary, &seed::cipher()).unwrap().is_none(), "{case}: the lease is unchanged");
    }
}

/// Codex review: a set-aside bot with an order whose outcome is unknown would still be recovered (and its order polled) once
/// stopped: it is work in flight, a hard stop.
#[test]
fn a_bot_with_an_unresolved_order_is_not_set_aside() {
    let (dir, o, s) = common::install();
    let bot = seed::insert_bot(&o.primary, &s, &plain().with("future_price_drop_limited", json!(true)));
    let intent = json!({ "side": "buy", "transaction_type": "REGULAR", "amount": "0.001", "volume": "0.001" }).to_string();
    o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_placement', json(?1)) WHERE id = ?2", rusqlite::params![intent, bot]).unwrap();
    let before = state(&o.primary);
    match take_over(dir.path(), &o) {
        Err(EngineError::HardStop(message)) => assert!(message.contains(&format!("Bot {bot}: It has an order whose result isn't known yet.")), "{message}"),
        other => panic!("{:?}", other.map(|t| t.eligible)),
    }
    assert_eq!(state(&o.primary), before);
}

/// Codex review (disproved): a row whose class is NULL fails the takeover before its transaction (the Rails command-job
/// check reads every bot's class), so nothing is stopped or claimed: it never reaches the set-aside or the claim.
#[test]
fn a_bot_without_a_type_fails_the_takeover_before_anything_is_written() {
    let (dir, o, s) = common::install();
    let bot = seed::insert_bot(&o.primary, &s, &plain());
    o.primary.execute("UPDATE bots SET type = NULL WHERE id = ?1", [bot]).unwrap();
    let before = state(&o.primary);
    let r = take_over(dir.path(), &o);
    assert!(matches!(r, Err(EngineError::Sqlite(_))), "{:?}", r.as_ref().map(|t| t.eligible.clone()).map_err(|e| format!("{e:?}")));
    assert_eq!(state(&o.primary), before);
    assert!(lease::read(&o.primary, &seed::cipher()).unwrap().is_none());
}

/// Codex review: a set-aside bot's Rails jobs go at every start, not only the one that set it aside (a crash between the
/// claim's commit and the queue delete leaves them).
#[test]
fn a_set_aside_bots_rails_jobs_go_at_a_later_start_too() {
    let (dir, o, s) = common::install();
    let bot = seed::insert_bot(&o.primary, &s, &plain().with("future_price_drop_limited", json!(true)));
    take_over(dir.path(), &o).unwrap();
    o.queue.execute("INSERT INTO solid_queue_jobs (queue_name, class_name, arguments, priority, active_job_id, created_at, updated_at) VALUES ('default', 'Bot::ActionJob', ?1, 0, 'left-behind', '2026-09-01', '2026-09-01')",
        [json!({ "arguments": [{ "_aj_globalid": format!("gid://deltabadger/Bots::DcaMultiAsset/{bot}") }] }).to_string()]).unwrap();
    take_over(dir.path(), &o).unwrap();
    let left: i64 = o.queue.query_row("SELECT count(*) FROM solid_queue_jobs WHERE active_job_id = 'left-behind'", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 0);
}

/// r1.4: two bots on one venue, one set aside: the other runs.
#[test]
fn two_bots_on_one_venue_one_set_aside_the_other_runs() {
    let (dir, o, s) = common::install();
    let runs = seed::insert_bot(&o.primary, &s, &plain());
    let aside = seed::insert_bot(&o.primary, &s, &plain().with("future_price_drop_limited", json!(true)));
    let t = take_over(dir.path(), &o).unwrap();
    assert_eq!((t.eligible, t.set_aside.iter().map(|a| a.id).collect::<Vec<_>>()), (vec![runs], vec![aside]));
}

mod web {
    use super::*;
    use common::web::{Browser, Csrf};
    use deltabadger::web::session::{self, SessionData};

    const HASH: &str = "$2a$04$abcdefghijklmnopqrstuuKq8n2RkM1bXh0Zc3TtYw5LpJv7dEoGi";

    fn browser(app: &deltabadger::web::App, user: i64) -> Browser {
        Browser { cookie: Some(session::seal(&app.keys.session, &SessionData { user: Some((user, HASH[..29].into())), ..Default::default() }, app.now())), page: None }
    }

    /// r2.6, r1.8, r1.12: after a takeover that set bots aside, each one's page says why (its status line and activity, or for
    /// a bot these pages do not render, a page of its own with Delete); delete works; a healthy bot's edit is accepted; the
    /// dashboard lists every bot; a start of a still-refused bot is refused by the guard, and a start once fixed clears the
    /// stored reasons.
    #[tokio::test(flavor = "current_thread")]
    async fn each_set_aside_bot_shows_why_and_the_install_stays_writable() -> Result<(), Box<dyn std::error::Error>> {
        let (dir, o, s) = common::install();
        let c = &o.primary;
        c.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00', wash_sale_enabled = 0", [HASH])?;
        let healthy = seed::insert_bot(c, &s, &plain());
        let feature = seed::insert_bot(c, &s, &plain().with("future_price_drop_limited", json!(true)));
        let flip = seed::insert_bot(c, &s, &plain().with("price_limited", json!(true)).with("price_limit_action", json!("start_selling")).with("sell_interval", json!("nonsense")));
        // BitMart: retired, no Rust code, so its bots' pages are not rendered.
        c.execute("INSERT INTO exchanges (type, name, created_at, updated_at) VALUES ('Exchanges::Bitmart', 'BitMart', '2026-01-01', '2026-01-01')", [])?;
        let bitmart = c.last_insert_rowid();
        let elsewhere = seed::insert_bot(c, &s, &plain());
        c.execute("UPDATE bots SET exchange_id = ?1 WHERE id = ?2", [bitmart, elsewhere])?;
        let t = take_over(dir.path(), &o).unwrap();
        assert_eq!(t.set_aside.iter().map(|a| a.id).collect::<Vec<_>>(), vec![feature, flip, elsewhere]);
        let app = common::web::app(dir.path(), "engine-test-secret", common::web::TestClock::starting(now()));
        let mut b = browser(&app, s.user_id);

        let shown = "Deltabadger stopped this bot while updating your data:";
        for bot in [feature, flip] {
            let page = b.get(&app, &format!("/bots/{bot}")).await;
            assert_eq!(page.status, 200, "{:.400}", page.body);
            assert!(page.body.contains(shown), "bot {bot}: the status line says why: {:.3000}", page.body);
        }
        let page = b.get(&app, &format!("/bots/{feature}")).await;
        assert!(page.body.contains(&deltabadger::web::i18n::escape(FEATURE)), "the reason itself: {:.3000}", page.body);
        // The mail sent, the page still says why.
        let pending = notice::pending_in(feature, model::load_bot(c, feature).unwrap().transient.as_object().ok_or("object")?);
        notice::clear(c, &seed::cipher(), &pending[0]).unwrap();
        assert!(b.get(&app, &format!("/bots/{feature}")).await.body.contains(shown));

        // A bot on an exchange this build has no code for: its own page, the reason and Delete.
        let page = b.get(&app, &format!("/bots/{elsewhere}")).await;
        assert_eq!(page.status, 200, "{:.400}", page.body);
        assert!(page.body.contains(shown) && page.body.contains("Trading on BitMart isn&#39;t supported yet") && page.body.contains(&format!("action=\"/bots/{elsewhere}/delete\"")), "{:.3000}", page.body);
        let block = page.body.split(&format!("id=\"bot_set_aside_{elsewhere}\"")).nth(1).and_then(|b| b.split("</div>").next()).unwrap_or_default();
        assert!(block.contains("<form") && !block.contains("data-turbo"), "Turbo submits the delete, as every bot form: its answer is a Turbo Stream: {block}");
        // A bot of a kind no lifecycle writer takes: no Delete it could not carry out.
        let odd = seed::insert_bot(c, &s, &BotSpec { status: 2, ..plain() });
        c.execute("UPDATE bots SET type = 'Bots::Legacy', transient_data = json_set(transient_data, '$.rust_set_aside', json('{\"reasons\":[\"x\"],\"at\":\"t\"}')) WHERE id = ?1", [odd])?;
        let page = b.get(&app, &format!("/bots/{odd}")).await;
        assert!(page.body.contains(shown) && !page.body.contains(&format!("/bots/{odd}/delete")), "{} {:.2000}", page.status, page.body);

        let list = b.get(&app, "/bots").await;
        assert_eq!(list.status, 200);
        for bot in [healthy, feature, flip, elsewhere] { assert!(list.body.contains(&format!("_{bot}\"")), "the dashboard lists bot {bot}"); }

        // r1.8: the healthy bot's edit is accepted (the guard's flip check meets no admitted invalid flip).
        b.get(&app, &format!("/bots/{healthy}")).await;
        let edit = b.send(&app, "PATCH", &format!("/bots/{healthy}"), Some(&[("bots_dca_multi_asset[quote_amount]", "61")]), Csrf::Header, &[("accept", "text/vnd.turbo-stream.html")]).await;
        assert_eq!(edit.status, 200, "{:.800}", edit.body);
        assert_eq!(c.query_row("SELECT json_extract(settings, '$.quote_amount') FROM bots WHERE id = ?1", [healthy], |r| r.get::<_, f64>(0))?, 61.0);

        // Still refused: the guard's 422, and the reasons stay.
        b.get(&app, &format!("/bots/{feature}")).await;
        let start = b.send(&app, "PATCH", &format!("/bots/{feature}/start"), Some(&[]), Csrf::Header, &[("accept", "text/vnd.turbo-stream.html")]).await;
        assert!(start.body.contains("This app can"), "{:.800}", start.body);
        assert_eq!(status(c, feature), BotStatus::Stopped);
        assert_eq!(reasons(c, feature), vec![FEATURE.to_string()]);
        // Fixed, then started: the stored reasons go.
        c.execute("UPDATE bots SET settings = json_remove(settings, '$.future_price_drop_limited') WHERE id = ?1", [feature])?;
        b.get(&app, &format!("/bots/{feature}")).await;
        let start = b.send(&app, "PATCH", &format!("/bots/{feature}/start"), Some(&[]), Csrf::Header, &[("accept", "text/vnd.turbo-stream.html")]).await;
        assert_eq!(status(c, feature), BotStatus::Scheduled, "{:.800}", start.body);
        assert!(model::load_bot(c, feature).unwrap().transient.get(set_aside::KEY).is_none(), "a start clears the set-aside");

        // Delete from the set-aside page.
        b.get(&app, &format!("/bots/{elsewhere}")).await;
        let deleted = b.post(&app, &format!("/bots/{elsewhere}/delete"), &[("_method", "delete")]).await;
        assert!(deleted.status < 400, "{} {:.800}", deleted.status, deleted.body);
        assert_eq!(status(c, elsewhere), BotStatus::Deleted);
        Ok(())
    }
}
