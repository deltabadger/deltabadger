//! What the engine announces after a tick: each order row it recorded (for the ledger sync) and each spent out-of-funds
//! budget (the mail service's wake). The funds marker itself is pinned in tests/tick.rs.
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::events::EngineEvent;
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::FixedClock;
use deltabadger::lease;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::{AddOutcome, FakeFactory, FakeVenue};
use rusqlite::Connection;
use tokio::sync::mpsc::UnboundedReceiver;

fn at(s: &str) -> FixedClock { FixedClock(s.parse::<DateTime<Utc>>().unwrap()) }
fn drained(rx: &mut UnboundedReceiver<EngineEvent>) -> Vec<EngineEvent> { std::iter::from_fn(|| rx.try_recv().ok()).collect() }
fn rows(c: &Connection) -> Vec<(i64, i64, i64)> {
    c.prepare("SELECT bot_id, id, status FROM transactions ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap()
}
/// 1 EUR free after each buy: under three days of spend, so Bot::Fundable spends the (user, EUR) budget on the first bot.
fn low_funds() -> FakeVenue { FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "1", "0") }

/// A Kraken install with `n` weekly bots of one user, all due at 2026-09-01 10:00:00.
fn engine(n: usize, v: FakeVenue) -> (tempfile::TempDir, Engine<FakeFactory>, Vec<i64>, seed::Seeded) {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, "2026-09-01T00:00:00Z".parse().unwrap()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let ids = (0..n).map(|_| seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).collect();
    (dir, Engine::new(o.primary, FakeFactory(v), seed::cipher(), lock), ids, s)
}

#[tokio::test(flavor = "current_thread")]
async fn each_recorded_row_and_each_spent_funds_budget_is_announced_once_after_its_tick() {
    let (_d, mut e, bots, s) = engine(2, low_funds());
    let mut rx = e.subscribe();
    run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    let r = rows(&e.primary);
    assert_eq!(r.len(), 2);
    assert_eq!(drained(&mut rx), vec![
        EngineEvent::OrderRecorded { bot_id: bots[0], transaction_id: r[0].1 },
        EngineEvent::FundsLow { bot_id: bots[0], user_id: s.user_id, quote_asset_id: Some(s.quote) },
        EngineEvent::OrderRecorded { bot_id: bots[1], transaction_id: r[1].1 },
    ], "the second bot finds the budget of the same user and quote spent, as Rails does");
    run::step(&mut e, &at("2026-09-01T10:00:05.5Z")).await.unwrap(); // follow-up polls only: no row, no stamp
    assert_eq!(drained(&mut rx), vec![]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_tick_failing_for_funds_announces_its_failed_row_and_the_spent_budget() {
    let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0")
        .next_add(AddOutcome::Reject(vec!["EOrder:Insufficient funds".into()]));
    let (_d, mut e, bots, s) = engine(1, v);
    let mut rx = e.subscribe();
    run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    let r = rows(&e.primary);
    assert_eq!(r[0].2, 1, "a failed row, as Rails writes for a definitive rejection");
    assert_eq!(drained(&mut rx), vec![
        EngineEvent::OrderRecorded { bot_id: bots[0], transaction_id: r[0].1 },
        EngineEvent::FundsLow { bot_id: bots[0], user_id: s.user_id, quote_asset_id: Some(s.quote) },
    ]);
}

#[tokio::test(flavor = "current_thread")]
async fn every_subscriber_hears_every_event() {
    let (_d, mut e, bots, _s) = engine(1, low_funds());
    let (mut a, mut b) = (e.subscribe(), e.subscribe());
    run::step(&mut e, &at("2026-09-01T10:00:00.5Z")).await.unwrap();
    let heard = drained(&mut a);
    assert!(matches!(heard.first(), Some(EngineEvent::OrderRecorded { bot_id, .. }) if *bot_id == bots[0]));
    assert_eq!(drained(&mut b), heard);
}
