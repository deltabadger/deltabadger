mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{model, notice, FixedClock};
use deltabadger::enums::BotStatus;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::{AddOutcome, FakeVenue};
use serde_json::json;

fn clock(s: &str) -> FixedClock { FixedClock(s.parse::<DateTime<Utc>>().unwrap()) }
fn setup(spec: BotSpec) -> (tempfile::TempDir, store::Opened, i64) {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &spec);
    (dir, o, id)
}
fn priced() -> FakeVenue { FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0") }
fn bot(o: &store::Opened, id: i64) -> model::Bot { model::load_bot(&o.primary, id).unwrap() }
fn one<T: rusqlite::types::FromSql>(o: &store::Opened, sql: &str) -> T { o.primary.query_row(sql, [], |r| r.get(0)).unwrap() }
async fn run(o: &store::Opened, v: &FakeVenue, id: i64, at: &str) -> TickOutcome {
    tick::tick(&o.primary, v, id, &clock(at), &mut Attempts::default()).await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn a_market_tick_places_one_order_and_goes_back_to_scheduled() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let v = priced().next_add(AddOutcome::Accept("OTX-1".into()));
    assert!(matches!(run(&o, &v, id, "2026-09-01T10:00:00.5Z").await, TickOutcome::Done { placed: true }));
    assert_eq!((v.sent()[0].volume.as_str(), v.sent()[0].quote_volume), ("0.0012", false));
    let b = bot(&o, id);
    assert_eq!(b.status, BotStatus::Scheduled);
    assert_eq!(b.transient["last_action_job_at"], "2026-09-01T10:00:00.500Z");
    assert!(!b.transient.as_object().unwrap().contains_key("waiting_for_market_open"), "Rails writes nil over nothing as no change");
    assert_eq!(one::<String>(&o, "SELECT external_id FROM transactions"), "OTX-1");
    assert_eq!(v.calls("/0/private/BalanceEx"), 1, "Fundable reads the balance after the orders");
}

#[tokio::test(flavor = "current_thread")]
async fn a_limit_tick_prices_below_last_and_sends_base_volume() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("limit_ordered", json!(true)).with("limit_order_pcnt_distance", json!(0.0025)));
    let v = priced();
    run(&o, &v, id, "2026-09-01T10:00:01Z").await;
    assert_eq!(v.sent()[0].kind, deltabadger::venue::OrderKind::Limit { price: "49870.0".into() }); // 49995.0 × 0.9975, floored to 1 place
    assert!(!v.sent()[0].quote_volume);
}

#[tokio::test(flavor = "current_thread")]
async fn below_the_minimum_writes_a_skipped_row_and_places_nothing() {
    let (_d, o, id) = setup(BotSpec::weekly(0.4, "2026-09-01 10:00:00"));
    let v = priced();
    assert!(matches!(run(&o, &v, id, "2026-09-01T10:00:01Z").await, TickOutcome::Done { placed: false }));
    assert!(v.sent().is_empty());
    assert_eq!(one::<i64>(&o, "SELECT status FROM transactions"), 2);
    assert_eq!(one::<String>(&o, "SELECT event FROM bot_activity_logs"), "order_skipped");
}

#[tokio::test(flavor = "current_thread")]
async fn an_untradable_ticker_fails_the_composition_refresh_with_no_order_row() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE tickers SET trading_enabled = 0", []).unwrap();
    let v = priced();
    assert!(matches!(run(&o, &v, id, "2026-09-01T10:00:01Z").await, TickOutcome::Rescheduled));
    assert!(v.sent().is_empty());
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 0);
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_failed'");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&details).unwrap(), json!({ "error": "None of the portfolio's weighted assets trade on Kraken", "kind": null }));
}

#[tokio::test(flavor = "current_thread")]
async fn a_retry_chain_ends_on_any_other_failure_so_the_next_interval_starts_fresh() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let mut attempts = Attempts::default();
    let flaky = FakeVenue::new(); // no Ticker: transient
    assert!(matches!(tick::tick(&o.primary, &flaky, id, &clock("2026-09-01T10:00:01Z"), &mut attempts).await.unwrap(), TickOutcome::RetryAfter(_)));
    let rejecting = priced().next_add(AddOutcome::Reject(vec!["EOrder:Insufficient funds".into()]));
    assert!(matches!(tick::tick(&o.primary, &rejecting, id, &clock("2026-09-01T10:00:05Z"), &mut attempts).await.unwrap(), TickOutcome::Rescheduled));
    match tick::tick(&o.primary, &flaky, id, &clock("2026-09-08T10:00:01Z"), &mut attempts).await.unwrap() {
        TickOutcome::RetryAfter(d) => assert_eq!(d.as_secs(), 3, "first attempt of a new job, not the second of the old one"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unreadable_data_fails_the_tick_instead_of_stranding_the_bot() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("missed_quote_amount", json!("not a number")));
    assert!(matches!(run(&o, &priced(), id, "2026-09-01T10:00:01Z").await, TickOutcome::Rescheduled));
    assert_eq!(bot(&o, id).status, BotStatus::Retrying, "never left executing");
}

#[tokio::test(flavor = "current_thread")]
async fn a_rejection_is_a_failed_row_and_a_second_invalid_key_stops_the_bot() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let v = priced().next_add(AddOutcome::Reject(vec!["EAPI:Invalid key".into()])).next_add(AddOutcome::Reject(vec!["EAPI:Invalid key".into()]));
    assert!(matches!(run(&o, &v, id, "2026-09-01T10:00:01Z").await, TickOutcome::Rescheduled));
    assert_eq!(bot(&o, id).last_failure_kind().as_deref(), Some("invalid_key"));
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'execution_failed'"), 0, "a failed row this tick: Rails logs nothing");
    assert!(matches!(run(&o, &v, id, "2026-09-08T10:00:01Z").await, TickOutcome::Stopped));
    assert_eq!(bot(&o, id).status, BotStatus::Stopped);
    assert_eq!(one::<String>(&o, &format!("SELECT stop_message_key FROM bots WHERE id = {id}")), "bot.status.stopped_by_error.invalid_key");
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_reply_pauses_the_bot_until_reconciled_and_never_sends_twice() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let v = priced().next_add(AddOutcome::AmbiguousPlaced("OTX-5".into())).lookup_fails(1);
    assert!(matches!(run(&o, &v, id, "2026-09-01T10:00:01Z").await, TickOutcome::AwaitingReconciliation));
    assert_eq!(bot(&o, id).status, BotStatus::Retrying);
    assert!(matches!(run(&o, &v, id, "2026-09-01T10:00:31Z").await, TickOutcome::AwaitingReconciliation), "first lookup incomplete");
    assert!(matches!(run(&o, &v, id, "2026-09-01T10:01:01Z").await, TickOutcome::Done { placed: false }), "found: nothing left to buy");
    assert_eq!((v.sent().len(), one::<i64>(&o, "SELECT count(*) FROM transactions")), (1, 1));
}

#[tokio::test(flavor = "current_thread")]
async fn transient_and_rate_limit_retries_count_separately_as_activejob_does() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let v = FakeVenue::new(); // no Ticker scripted: every price read fails transiently
    let mut attempts = Attempts::default();
    let mut waits = vec![];
    for _ in 0..3 {
        match tick::tick(&o.primary, &v, id, &clock("2026-09-01T10:00:01Z"), &mut attempts).await.unwrap() {
            TickOutcome::RetryAfter(d) => waits.push(d.as_secs()),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(waits, vec![3, 18, 83]);
    assert_eq!(attempts.rate, 0, "a rate-limit handler's count is untouched by transient failures");
    assert!(matches!(tick::tick(&o.primary, &v, id, &clock("2026-09-01T10:00:01Z"), &mut attempts).await.unwrap(), TickOutcome::Rescheduled));
    assert_eq!(one::<String>(&o, "SELECT event FROM bot_activity_logs ORDER BY id DESC LIMIT 1"), "execution_retrying");
    assert_eq!(bot(&o, id).last_failure_kind().as_deref(), Some("transient"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_transport_failure_of_the_funds_check_is_not_low_and_the_tick_succeeds() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0"); // BalanceEx unscripted → Transient → Failure result in Rails
    let out = run(&o, &v, id, "2026-09-01T10:00:01Z").await;
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert_eq!((v.sent().len(), one::<i64>(&o, "SELECT count(*) FROM transactions")), (1, 1));
    assert_eq!(bot(&o, id).status, BotStatus::Scheduled);
}

#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_balance_body_fails_the_tick_without_a_retry() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let v = FakeVenue::from_script(&json!({ "http": {
        "/0/public/Ticker": [{ "error": [], "result": { "XXBTZEUR": { "a": ["50000.0", "1", "1.000"], "b": ["49990.1", "1", "1.000"], "c": ["49995.0", "0.001"], "v": ["1", "1"], "p": ["1", "1"], "t": [1, 1], "l": ["1", "1"], "h": ["1", "1"], "o": "1" } } }],
        "/0/private/BalanceEx": ["not an object"] } }));
    let out = run(&o, &v, id, "2026-09-01T10:00:01Z").await;
    assert!(matches!(out, TickOutcome::Rescheduled), "{out:?}");
    assert_eq!(bot(&o, id).status, BotStatus::Retrying);
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'execution_failed'"), 1);
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'execution_retrying'"), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_price_read_failure_carries_rails_no_price_message() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let mut attempts = Attempts::default();
    for _ in 0..4 { tick::tick(&o.primary, &FakeVenue::new(), id, &clock("2026-09-01T10:00:01Z"), &mut attempts).await.unwrap(); }
    let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_retrying'");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&details).unwrap()["error"], "No price for BTC: no scripted Ticker");
}

#[tokio::test(flavor = "current_thread")]
async fn tick_stop_leaves_a_bot_the_user_stopped_alone() {
    let (_d, o, id) = setup(BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    o.primary.execute("UPDATE bots SET stopped_at = '2026-08-01 00:00:00', stop_message_key = 'bot.status.stopped_by_user' WHERE id = ?1", [id]).unwrap();
    tick::stop(&o.primary, id, "bot.status.stopped_by_error.invalid_key", clock("2026-09-01T10:00:01Z").0).unwrap();
    assert_eq!(one::<String>(&o, &format!("SELECT stop_message_key FROM bots WHERE id = {id}")), "bot.status.stopped_by_user");
    assert_eq!(one::<String>(&o, &format!("SELECT stopped_at FROM bots WHERE id = {id}")), "2026-08-01 00:00:00");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs WHERE event = 'stopped'"), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn low_funds_record_the_notification_time_like_fundable() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "10", "0"); // 60/week × 3 days ≈ 25.7 needed
    run(&o, &v, id, "2026-09-01T10:00:01Z").await;
    assert_eq!(one::<Option<String>>(&o, &format!("SELECT last_end_of_funds_notification FROM bots WHERE id = {id}")).as_deref(), Some("2026-09-01 10:00:01"));
}

// ---- The mail budget, as Rails stamps it, and the mail it owes (engine::notice) ----

fn transient(o: &store::Opened, id: i64) -> serde_json::Value { bot(o, id).transient }
fn rejecting(error: &str) -> FakeVenue { priced().next_add(AddOutcome::Reject(vec![error.into()])) }

#[tokio::test(flavor = "current_thread")]
async fn low_funds_after_a_buy_stamp_the_budget_and_owe_the_mail_in_the_same_write() {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let (first, second) = (seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")), seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")));
    let poor = || FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "10", "0");
    assert!(matches!(run(&o, &poor(), first, "2026-09-01T10:00:01Z").await, TickOutcome::Done { placed: true }));
    assert_eq!(transient(&o, first)[notice::FUNDS], json!({ "quote_asset": s.quote, "stamped_at": "2026-09-01T10:00:01.000Z" }));
    assert_eq!(notice::all_pending(&o.primary).unwrap().iter().map(|p| (p.bot_id, p.notice.mail())).collect::<Vec<_>>(), [(first, "end_of_funds")]);
    // Bot::Fundable#notified_in_last_day?: one mail a day per user and quote asset, whichever bot asks.
    run(&o, &poor(), second, "2026-09-01T11:00:00Z").await;
    assert!(transient(&o, second).get(notice::FUNDS).is_none());
    assert_eq!(one::<Option<String>>(&o, &format!("SELECT last_end_of_funds_notification FROM bots WHERE id = {second}")), None);
}

#[tokio::test(flavor = "current_thread")]
async fn a_buy_refused_for_funds_owes_the_end_of_funds_mail_and_leaves_updated_at_alone() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    assert!(matches!(run(&o, &rejecting("EOrder:Insufficient funds"), id, "2026-09-01T10:00:01Z").await, TickOutcome::Rescheduled));
    let t = transient(&o, id);
    assert_eq!(t[notice::FUNDS]["stamped_at"], "2026-09-01T10:00:01.000Z");
    assert_eq!((t.get(notice::ERROR), t.get("failure_notifications")), (None, None), "this kind spends Bot::Fundable's budget, not its own");
    assert_eq!(one::<String>(&o, &format!("SELECT last_end_of_funds_notification FROM bots WHERE id = {id}")), "2026-09-01 10:00:01");
    // The same refusal an hour later: the budget is spent, nothing more is owed.
    o.primary.execute(&format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.{}') WHERE id = ?1", notice::FUNDS), [id]).unwrap();
    run(&o, &rejecting("EOrder:Insufficient funds"), id, "2026-09-01T11:00:00Z").await;
    assert!(transient(&o, id).get(notice::FUNDS).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_failure_owes_one_error_mail_a_day_for_its_kind() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let error = "EGeneral:Invalid arguments:volume minimum not met";
    run(&o, &rejecting(error), id, "2026-09-01T10:00:01Z").await;
    let t = transient(&o, id);
    assert_eq!(t["failure_notifications"], json!({ "unknown": "2026-09-01T10:00:01Z" }), "Time.current.iso8601, under the key Rails uses for no kind");
    assert_eq!(t[notice::ERROR], json!({ "unknown": { "error": error, "stamped_at": "2026-09-01T10:00:01.000Z" } }));
    assert_eq!(one::<Option<String>>(&o, &format!("SELECT last_end_of_funds_notification FROM bots WHERE id = {id}")), None);
    // Inside the day: no second mail, whatever happened to the first. A success in between changes nothing (the budget is never cleared).
    run(&o, &rejecting(error), id, "2026-09-02T10:00:00Z").await;
    assert_eq!(transient(&o, id)[notice::ERROR]["unknown"]["stamped_at"], "2026-09-01T10:00:01.000Z");
    // A day and a second later the budget is open again.
    run(&o, &rejecting(error), id, "2026-09-02T10:00:02Z").await;
    let t = transient(&o, id);
    assert_eq!((t["failure_notifications"]["unknown"].clone(), t[notice::ERROR]["unknown"]["stamped_at"].clone()), (json!("2026-09-02T10:00:02Z"), json!("2026-09-02T10:00:02.000Z")));
}

#[tokio::test(flavor = "current_thread")]
async fn the_second_blocking_failure_stops_the_bot_and_owes_the_stopped_mail() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    assert!(matches!(run(&o, &rejecting("EAPI:Invalid key"), id, "2026-09-01T10:00:01Z").await, TickOutcome::Rescheduled));
    let t = transient(&o, id);
    assert_eq!(t[notice::ERROR]["invalid_key"]["error"], "EAPI:Invalid key", "the first one is an ordinary failure: notify_about_error");
    assert!(t.get(notice::STOPPED).is_none());
    assert!(matches!(run(&o, &rejecting("EAPI:Invalid key"), id, "2026-09-01T10:05:00Z").await, TickOutcome::Stopped));
    let t = transient(&o, id);
    assert_eq!(bot(&o, id).status, BotStatus::Stopped);
    assert_eq!(t[notice::STOPPED], json!({ "error": "EAPI:Invalid key", "stamped_at": "2026-09-01T10:05:00.000Z" }));
    assert_eq!(t["failure_notifications"]["invalid_key"], "2026-09-01T10:05:00Z", "a blocking stop notifies whatever the budget says, and stamps it");
    assert_eq!(t[notice::ERROR]["invalid_key"]["stamped_at"], "2026-09-01T10:00:01.000Z", "no second error mail: the stop has its own");
}

#[tokio::test(flavor = "current_thread")]
async fn a_rate_limit_that_outlasts_its_retries_owes_the_error_mail() {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    seed::insert_tx(&o.primary, &s, id, &seed::TxSpec { status: 0, external_status: Some(0), external_id: Some("OMKT".into()), order_type: 0,
        amount: None, quote_amount: Some("60"), price: Some("50000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-01 10:00:01".into() });
    let v = FakeVenue::from_script(&json!({ "http": { "/0/private/QueryOrders": [{ "error": ["EAPI:Rate limit exceeded"] }] } }));
    let mut attempts = Attempts::default();
    for n in 1..=4 {
        let out = tick::tick(&o.primary, &v, id, &clock("2026-09-08T10:00:01Z"), &mut attempts).await.unwrap();
        assert_eq!(matches!(out, TickOutcome::Rescheduled), n == 4, "attempt {n}: {out:?}");
        assert_eq!(transient(&o, id).get(notice::ERROR).is_some(), n == 4, "nothing is owed while it is still retrying");
    }
    let t = transient(&o, id);
    assert_eq!(t[notice::ERROR]["throttle"], json!({ "error": "EAPI:Rate limit exceeded", "stamped_at": "2026-09-08T10:00:01.000Z" }));
    assert_eq!(t["failure_notifications"]["throttle"], "2026-09-08T10:00:01Z");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_that_stopped_nothing_owes_no_mail_and_a_plain_stop_leaves_the_row_s_json_alone() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE bots SET transient_data = '{\"missed_quote_amount\": \"12.50\",  \"x\": 1.0}' WHERE id = ?1", [id]).unwrap();
    let now = clock("2026-09-01T10:00:01Z").0;
    tick::stop(&o.primary, id, "bot.settings.extra_amount_limit.amount_spent", now).unwrap();
    assert_eq!(one::<String>(&o, "SELECT transient_data FROM bots"), "{\"missed_quote_amount\": \"12.50\",  \"x\": 1.0}", "byte for byte");
    assert_eq!(bot(&o, id).status, BotStatus::Stopped);
    // Already stopped: the second stop is not the one that stopped it.
    assert!(!tick::stop_owing_mail(&o.primary, id, "bot.status.stopped_by_error.invalid_key", Some((notice::STOPPED, notice::stopped_marker("x", now))), now).unwrap());
    assert!(notice::all_pending(&o.primary).unwrap().is_empty());
    // Working: stopped, and the mail owed in the same statement.
    o.primary.execute("UPDATE bots SET status = 1 WHERE id = ?1", [id]).unwrap();
    assert!(tick::stop_owing_mail(&o.primary, id, "bot.settings.extra_amount_limit.amount_spent", Some((notice::LIMIT, notice::limit_marker(now))), now).unwrap());
    assert_eq!(notice::all_pending(&o.primary).unwrap().iter().map(|p| p.notice.mail()).collect::<Vec<_>>(), ["stopped_by_amount_limit"]);
    assert_eq!(transient(&o, id)["missed_quote_amount"], "12.50");
}

/// `tick::stop` and `tick::stop_owing_mail` called on their own (a stop outside a failing exit; Plan 2c's amount limit):
/// the stop, its marker and its log line are one transaction there too. A log line that cannot be written leaves the bot
/// working and nothing owed, so the retry writes all three; inside a caller's transaction the stop goes with it.
#[test]
fn a_stop_called_on_its_own_stops_marks_and_logs_together_or_not_at_all() {
    let now = clock("2026-09-01T10:00:01Z").0;
    for owing in [false, true] {
        let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
        let before = bot(&o, id).status;
        let stop = |c: &rusqlite::Connection| match owing {
            true => tick::stop_owing_mail(c, id, "bot.settings.extra_amount_limit.amount_spent", Some((notice::LIMIT, notice::limit_marker(now))), now).map(|_| ()),
            false => tick::stop(c, id, "bot.settings.extra_amount_limit.amount_spent", now),
        };
        let written = || (bot(&o, id).status, notice::all_pending(&o.primary).unwrap().len(), one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs"));
        o.primary.execute_batch("CREATE TEMP TRIGGER refuse BEFORE INSERT ON bot_activity_logs BEGIN SELECT RAISE(ABORT, 'refused'); END").unwrap();
        assert!(stop(&o.primary).is_err());
        assert_eq!(written(), (before, 0, 0), "owing {owing}: not stopped, no marker, no log line");
        o.primary.execute_batch("DROP TRIGGER refuse").unwrap();
        // The caller's transaction, rolled back: the stop joined it and is gone with it.
        let tx = model::immediate(&o.primary).unwrap();
        stop(&tx).unwrap();
        drop(tx);
        assert_eq!(written(), (before, 0, 0), "owing {owing}: the caller's transaction is the stop's");
        // The retry writes all three.
        stop(&o.primary).unwrap();
        assert_eq!(written(), (BotStatus::Stopped, owing as usize, 1), "owing {owing}");
    }
}

/// The budget, the stop, the marker and the log line of one failing exit are one transaction: when any of them cannot
/// be written, none is, and the bot is as the tick found it. (A budget spent without its marker would be a mail the
/// owner never gets and Rails never sends.) Each case makes a later statement of the exit fail, with a trigger.
#[tokio::test(flavor = "current_thread")]
async fn a_failing_exit_writes_its_budget_its_marker_and_its_log_line_together_or_not_at_all() {
    let refuse_log = |o: &store::Opened, event: &str| o.primary.execute_batch(&format!(
        "CREATE TEMP TRIGGER refuse BEFORE INSERT ON bot_activity_logs WHEN NEW.event = '{event}' BEGIN SELECT RAISE(ABORT, 'refused'); END")).unwrap();
    // (the out-of-funds stamp, failure_notifications, markers owed, status)
    let written = |o: &store::Opened, id: i64| {
        (one::<Option<String>>(o, &format!("SELECT last_end_of_funds_notification FROM bots WHERE id = {id}")), transient(o, id).get("failure_notifications").cloned(),
         notice::all_pending(&o.primary).unwrap().len(), bot(o, id).status)
    };
    let nothing = (None, None, 0, BotStatus::Retrying); // `retrying` is the tick's own, written before the exit

    // An ordinary failure (no order row, so the exit logs): the per-kind budget and the error marker, then the log line.
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute("UPDATE tickers SET trading_enabled = 0", []).unwrap();
    refuse_log(&o, "execution_failed");
    assert!(tick::tick(&o.primary, &priced(), id, &clock("2026-09-01T10:00:01Z"), &mut Attempts::default()).await.is_err());
    assert_eq!(written(&o, id), nothing, "an ordinary failure");

    // A rate limit that outlasted its retries: the same, through the other exit.
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let seeded = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &seeded, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    seed::insert_tx(&o.primary, &seeded, id, &seed::TxSpec { status: 0, external_status: Some(0), external_id: Some("OMKT".into()), order_type: 0,
        amount: None, quote_amount: Some("60"), price: Some("50000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-01 10:00:01".into() });
    let throttled = FakeVenue::from_script(&json!({ "http": { "/0/private/QueryOrders": [{ "error": ["EAPI:Rate limit exceeded"] }] } }));
    refuse_log(&o, "execution_failed");
    let mut attempts = Attempts::default();
    for _ in 0..3 { tick::tick(&o.primary, &throttled, id, &clock("2026-09-08T10:00:01Z"), &mut attempts).await.unwrap(); }
    assert!(tick::tick(&o.primary, &throttled, id, &clock("2026-09-08T10:00:01Z"), &mut attempts).await.is_err());
    assert_eq!(written(&o, id), nothing, "an exhausted rate limit");

    // The second blocking failure: the budget, then the stop with its marker, then the stop's log line.
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("last_failure_kind", json!("invalid_key")));
    refuse_log(&o, "stopped");
    assert!(tick::tick(&o.primary, &rejecting("EAPI:Invalid key"), id, &clock("2026-09-01T10:00:01Z"), &mut Attempts::default()).await.is_err());
    assert_eq!(written(&o, id), nothing, "a stop that could not be logged: not stopped, nothing spent, nothing owed");
    assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bot_activity_logs"), 0);

    // A buy refused for funds: the stamp with its marker, then the kind. Here the kind's write is the one that fails.
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    o.primary.execute_batch("CREATE TEMP TRIGGER refuse BEFORE UPDATE OF transient_data ON bots \
        WHEN json_extract(NEW.transient_data, '$.last_failure_kind') = 'insufficient_funds' BEGIN SELECT RAISE(ABORT, 'refused'); END").unwrap();
    assert!(tick::tick(&o.primary, &rejecting("EOrder:Insufficient funds"), id, &clock("2026-09-01T10:00:01Z"), &mut Attempts::default()).await.is_err());
    assert_eq!(written(&o, id), nothing, "out of funds: no stamp without its marker and its kind");
}

#[tokio::test(flavor = "current_thread")]
async fn a_bot_stopped_mid_tick_stays_stopped() {
    let (_d, o, id) = setup(BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") });
    assert!(matches!(run(&o, &priced(), id, "2026-09-01T10:00:01Z").await, TickOutcome::Skipped));
    assert_eq!(bot(&o, id).status, BotStatus::Stopped);
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_while_addorder_awaits_its_reply_wins() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let db = o.primary.path().unwrap().to_string(); // the UI, stopping the bot from another connection
    let v = priced().next_add(AddOutcome::Accept("OTX-S".into())).on_add(move || {
        rusqlite::Connection::open(&db).unwrap().execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap();
    });
    run(&o, &v, id, "2026-09-01T10:00:01Z").await;
    assert_eq!(bot(&o, id).status, BotStatus::Stopped, "not resurrected to waiting/scheduled");
    assert_eq!(one::<String>(&o, "SELECT external_id FROM transactions"), "OTX-S", "the order already sent is recorded");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_during_the_pre_tick_sweep_places_nothing() {
    let dir = common::rails_install();
    let o = store::open(&Paths::from_env(&|_| None, dir.path())).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    seed::insert_tx(&o.primary, &s, id, &seed::TxSpec { status: 0, external_status: Some(1), external_id: Some("OOPEN".into()), order_type: 1,
        amount: Some("0.001"), quote_amount: None, price: Some("40000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-08-31 10:00:00".into() });
    let db = o.primary.path().unwrap().to_string();
    let v = priced()
        .order("OOPEN", json!({ "status": "open", "price": "40000", "vol": "0.001", "vol_exec": "0", "cost": "0", "oflags": "",
                                "descr": { "type": "buy", "ordertype": "limit", "price": "40000" } }))
        .on_query(move || { rusqlite::Connection::open(&db).unwrap().execute("UPDATE bots SET status = 2 WHERE id = ?1", [id]).unwrap(); });
    run(&o, &v, id, "2026-09-01T10:00:01Z").await;
    assert!(v.sent().is_empty(), "nothing placed for a bot stopped mid-tick");
    assert_eq!(bot(&o, id).status, BotStatus::Stopped);
}

#[tokio::test(flavor = "current_thread")]
async fn a_transient_kraken_answer_to_addorder_is_ambiguous_not_a_rejection() {
    for err in ["EService:Unavailable", "EService:Busy", "EService:Deadline elapsed", "EGeneral:Internal error"] {
        let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
        let v = priced().next_add(AddOutcome::Reject(vec![err.into()])).next_add(AddOutcome::Accept("OTX-2".into()));
        let out = run(&o, &v, id, "2026-09-01T10:00:01Z").await;
        assert!(matches!(out, TickOutcome::AwaitingReconciliation), "{err}: {out:?}");
        assert!(bot(&o, id).rust_placement().is_some(), "{err}: the intent stays until cl_ord_id recovery settles it");
        assert_eq!(one::<i64>(&o, "SELECT count(*) FROM transactions"), 0, "{err}: no failed row");
        assert_eq!(one::<String>(&o, "SELECT event FROM bot_activity_logs ORDER BY id DESC LIMIT 1"), "placement_ambiguous");
        assert!(matches!(run(&o, &v, id, "2026-09-01T10:00:41Z").await, TickOutcome::AwaitingReconciliation), "{err}: absent before deadline + 60 s proves nothing");
        assert!(matches!(run(&o, &v, id, "2026-09-01T10:01:12Z").await, TickOutcome::Done { placed: true }), "{err}: absent after it: not placed, buy now");
        assert_eq!(one::<String>(&o, "SELECT external_id FROM transactions"), "OTX-2");
    }
}

/// The tick reads the row at its start and writes transient_data after AddOrder (here: clearing last_failure_kind).
/// A web json_set landing in between must survive, and the tick's own write must land too.
#[tokio::test(flavor = "current_thread")]
async fn a_web_json_set_landing_while_addorder_awaits_survives_the_ticks_own_transient_writes() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("last_failure_kind", json!("transient")));
    let db = o.primary.path().unwrap().to_string(); // the web UI, writing key by key from its own connection
    let v = priced().next_add(AddOutcome::Accept("OTX-W".into())).on_add(move || {
        rusqlite::Connection::open(&db).unwrap()
            .execute("UPDATE bots SET transient_data = json_set(transient_data, '$.web_key', 'from the web') WHERE id = ?1", [id]).unwrap();
    });
    run(&o, &v, id, "2026-09-01T10:00:01Z").await;
    let t = bot(&o, id).transient;
    assert_eq!(t["web_key"], "from the web", "the web's key survives the engine's writes after it");
    assert!(t.get("last_failure_kind").is_none(), "the engine's own write landed too (cleared, then compacted away)");
    assert!(t.get("last_action_job_at").is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_balance_number_after_a_placement_fails_the_tick_without_a_retry_or_a_funds_notice() {
    for bad in ["NaN", "Infinity", "garbage", "1e41"] {
        let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
        let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", bad, "0").next_add(AddOutcome::Accept("OTX-1".into()));
        let out = run(&o, &v, id, "2026-09-01T10:00:01Z").await;
        assert!(matches!(out, TickOutcome::Rescheduled), "{bad}: {out:?}");
        assert_eq!(v.sent().len(), 1, "{bad}: placed once, never replayed");
        let details: String = one(&o, "SELECT details FROM bot_activity_logs WHERE event = 'execution_failed'");
        assert!(details.contains("unreadable"), "{bad}: {details}");
        assert_eq!(one::<i64>(&o, "SELECT count(*) FROM bots WHERE last_end_of_funds_notification IS NOT NULL"), 0, "{bad}: not a zero balance");
    }
}
