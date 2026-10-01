mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{model, FixedClock};
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
async fn a_transport_failure_of_the_funds_check_after_an_order_does_not_retry_the_order() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0"); // BalanceEx unscripted → Transient
    let mut attempts = Attempts::default();
    let out = tick::tick(&o.primary, &v, id, &clock("2026-09-01T10:00:01Z"), &mut attempts).await.unwrap();
    assert!(matches!(out, TickOutcome::Rescheduled), "retry_on must not replay a tick that already placed: {out:?}");
    assert_eq!(v.sent().len(), 1);
    assert_eq!(bot(&o, id).status, BotStatus::Retrying);
}

#[tokio::test(flavor = "current_thread")]
async fn low_funds_record_the_notification_time_like_fundable() {
    let (_d, o, id) = setup(BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "10", "0"); // 60/week × 3 days ≈ 25.7 needed
    run(&o, &v, id, "2026-09-01T10:00:01Z").await;
    assert_eq!(one::<Option<String>>(&o, &format!("SELECT last_end_of_funds_notification FROM bots WHERE id = {id}")).as_deref(), Some("2026-09-01 10:00:01"));
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
