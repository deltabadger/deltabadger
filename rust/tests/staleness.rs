//! Each reference source's bound is about two missed refreshes of the job that refreshes it; after the takeover a
//! source is as fresh as its job's last complete run and never its column's maximum; a refusal names that job's state.
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::staleness::{self, Verdict};
use deltabadger::engine::{handover, model, FixedClock};
use deltabadger::jobs::data_api::{Config, DataApi};
use deltabadger::jobs::{reference, state, Cx, Db, Outcome};
use deltabadger::lease;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::FakeFactory;
use deltabadger::venue::http::ScriptedTransport;
use serde_json::json;

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }

#[test]
fn every_bound_is_two_missed_refreshes_of_its_jobs_schedule_plus_an_hour() {
    let specs = reference::specs();
    for s in staleness::SOURCES {
        let spec = specs.iter().find(|j| j.name == s.job).unwrap_or_else(|| panic!("{} names no job", s.name));
        let schedule = spec.schedule.expect("a scheduled job");
        let period_hours = schedule.period().num_hours();
        assert_eq!(s.max_age_secs, staleness::bound_secs(period_hours), "{}", s.name);
        assert_eq!(s.max_age_secs, (2 * period_hours + 1) * 3600, "{}", s.name);
        assert_eq!(s.max_age_secs, schedule.stale_after().num_seconds(), "{}: the runner's fairness bound is the same", s.name);
    }
    assert_eq!(staleness::ALPACA_CRYPTO_TICKERS.max_age_secs, 49 * 3600, "the Alpaca crypto bound is unchanged");
    assert_eq!(staleness::VENUE_TICKERS.max_age_secs, 9 * 3600);
}

#[test]
fn a_stale_refusal_names_the_refreshing_jobs_last_error() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    // Taken over with Rails' last complete sync at 08-30 09:00; this engine's run then failed.
    state::seed(&o.primary, reference::ALPACA_CRYPTO, Some(at("2026-08-30T09:00:00Z")), at("2026-08-31T00:00:00Z")).unwrap();
    state::record_error(&o.primary, reference::ALPACA_CRYPTO, None, at("2026-09-01T10:15:04Z"), "data-api answered 502").unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    let stale = staleness::stale(&o.primary, &bot, at("2026-09-01T11:00:00Z")).unwrap().expect("past 49 h");
    for part in ["Alpaca crypto tickers", "exchange_assets.updated_at", "50h 0m old", "49h bound", reference::ALPACA_CRYPTO, "failing since", "data-api answered 502"] {
        assert!(stale.message.contains(part), "{part:?} missing from: {}", stale.message);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_run_that_fails_after_its_first_unit_leaves_the_source_ageing_through_a_restart() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let (lock, o) = (lease::lock(&p, at("2026-09-30T08:00:00Z")).unwrap(), store::open(&p).unwrap());
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    // Rails' last complete sync of the crypto catalogue, as the door measures it: the exchange assets' stamp, behind its gate row.
    o.primary.execute("INSERT INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?3) \
                       ON CONFLICT (asset_id, exchange_id) DO UPDATE SET updated_at = excluded.updated_at",
                      rusqlite::params![s.btc, s.exchange_id, "2026-09-29 10:15:00"]).unwrap();
    o.primary.execute("INSERT OR IGNORE INTO app_configs (key, value, created_at, updated_at) VALUES ('alpaca_crypto_listings_last_good_count', '31', \
                       '2026-09-29 10:15:00', '2026-09-29 10:15:00')", []).unwrap();
    let n = deltabadger::jobs::CHUNK + 50;
    let mut listings = vec![json!({ "base_asset_id": "crypto:bitcoin", "symbol": "BTC/USD", "base_decimals": 9, "quote_decimals": 2, "price_decimals": 2 })];
    for i in 0..n {
        o.primary.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', '2026-01-01 00:00:00', '2026-01-01 00:00:00')",
                          [format!("coin-{i}"), format!("C{i}")]).unwrap();
        listings.push(json!({ "base_asset_id": format!("crypto:coin-{i}"), "symbol": format!("C{i}/USD"), "base_decimals": 9, "quote_decimals": 2, "price_decimals": 2 }));
    }
    handover::take_over(&lock, &o, &seed::cipher(), "0.2.0", at("2026-09-30T08:00:00Z")).unwrap();
    assert_eq!(state::read(&o.primary, reference::ALPACA_CRYPTO, None).unwrap().rails_at, Some(at("2026-09-29T10:15:00Z")), "Rails' baseline, read once at the takeover");

    // The 10-01 run commits its first unit (500 tickers) and fails in the second.
    o.primary.execute_batch(&format!("CREATE TRIGGER late_failure BEFORE INSERT ON tickers WHEN NEW.ticker = 'C{}/USD' \
                                      BEGIN SELECT RAISE(ABORT, 'late failure'); END;", n - 1)).unwrap();
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/listings?venue=alpaca_crypto", 200, json!({ "data": listings }));
    let api = DataApi::new(Config { url: "http://data-api:3000".into(), token: "tok".into() }, t.clone(), t.clone());
    let run = Cx { db: Db::new(store::open(&p).unwrap().primary, seed::cipher()), clock: &FixedClock(at("2026-10-01T10:15:00Z")), wakers: Default::default() };
    let out = reference::run_once(reference::ALPACA_CRYPTO, Some(api), run).await;
    assert!(matches!(&out, Outcome::Failed(m) if m.contains("late failure")), "{out:?}");
    state::record_error(&o.primary, reference::ALPACA_CRYPTO, None, at("2026-10-01T10:15:00Z"), "late failure").unwrap(); // as the runner records it
    let newest: String = o.primary.query_row("SELECT max(updated_at) FROM exchange_assets", [], |r| r.get(0)).unwrap();
    assert_eq!(newest, "2026-10-01 10:15:00", "the run's first phases carry its stamp: the column alone would read fresh");

    assert_eq!(state::read(&o.primary, reference::ALPACA_CRYPTO, None).unwrap().incomplete_since, Some(at("2026-10-01T10:15:00Z")),
               "the job marked its import before the first unit, and the failure left the mark");

    let later = at("2026-10-01T11:16:00Z"); // 49 h 1 min after Rails' last complete sync
    let bot = model::load_bot(&o.primary, id).unwrap();
    let stale_everywhere = |c: &rusqlite::Connection, when: &str| {
        assert!(staleness::stale(c, &bot, later).unwrap().is_some(), "the tick, {when}");
        assert!(matches!(staleness::verdict(c, &bot, later).unwrap(), Verdict::Stale(_)), "the door (check and the takeover), {when}");
        let report = staleness::report(c, later).unwrap();
        assert!(report.iter().any(|l| l.starts_with("Alpaca crypto tickers:") && l.contains("STALE")), "check's report, {when}: {report:?}");
    };
    stale_everywhere(&o.primary, "after the partial run: its stamps are not trusted while the mark is set");
    // A restart: new connections; the record and its mark survive, nothing reseeds.
    drop((lock, o));
    stale_everywhere(&store::open(&p).unwrap().primary, "after a restart");
}

#[tokio::test(flavor = "current_thread")]
async fn after_a_handback_only_a_rails_refresh_makes_the_column_count_again() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let (lock, o) = (lease::lock(&p, at("2026-09-30T08:00:00Z")).unwrap(), store::open(&p).unwrap());
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let stamp = |c: &rusqlite::Connection, at: &str| {
        c.execute("INSERT INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?3) \
                   ON CONFLICT (asset_id, exchange_id) DO UPDATE SET updated_at = excluded.updated_at", rusqlite::params![s.btc, s.exchange_id, at]).unwrap();
    };
    stamp(&o.primary, "2026-09-29 10:15:00");
    o.primary.execute("INSERT OR IGNORE INTO app_configs (key, value, created_at, updated_at) VALUES ('alpaca_crypto_listings_last_good_count', '31', \
                       '2026-09-29 10:15:00', '2026-09-29 10:15:00')", []).unwrap();
    handover::take_over(&lock, &o, &seed::cipher(), "0.2.0", at("2026-09-30T08:00:00Z")).unwrap();
    // A partial import of this engine marked its record and stamped some rows; then the install went back to Rails.
    state::mark_incomplete(&o.primary, reference::ALPACA_CRYPTO, None, at("2026-10-01T10:15:00Z")).unwrap();
    stamp(&o.primary, "2026-10-01 10:15:00");
    handover::hand_back(&lock, &o, &FakeFactory::default(), &seed::cipher(), &FixedClock(at("2026-10-01T12:00:00Z"))).await.unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    let now = at("2026-10-02T00:00:00Z"); // past 49 h since Rails' 09-29 sync
    assert!(matches!(staleness::verdict(&o.primary, &bot, now).unwrap(), Verdict::Stale(_)), "a stamp from before the handback may be ours: not trusted");
    // Rails runs after the handback and refreshes the catalogue: its stamp is newer than the handback.
    stamp(&o.primary, "2026-10-01 13:00:00");
    assert!(matches!(staleness::verdict(&o.primary, &bot, now).unwrap(), Verdict::Fresh), "only Rails can have written after the handback");
    assert!(!staleness::report(&o.primary, now).unwrap().iter().any(|l| l.starts_with("Alpaca crypto tickers:") && l.contains("STALE")));
}

#[test]
fn a_run_that_refreshed_nothing_does_not_make_a_source_fresh() {
    let (_d, o, s) = common::install_alpaca();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    state::seed(&o.primary, reference::ALPACA_CRYPTO, Some(at("2026-08-30T09:00:00Z")), at("2026-08-31T00:00:00Z")).unwrap();
    state::record_run(&o.primary, reference::ALPACA_CRYPTO, None, at("2026-09-01T10:15:00Z")).unwrap(); // an empty payload, say
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert!(staleness::stale(&o.primary, &bot, at("2026-09-01T11:00:00Z")).unwrap().is_some(), "freshness reads last_success_at only");
}

#[tokio::test(flavor = "current_thread")]
async fn only_a_refresh_inside_rails_last_ownership_window_counts_through_a_later_takeover() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let (lock, o) = (lease::lock(&p, at("2026-09-30T08:00:00Z")).unwrap(), store::open(&p).unwrap());
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let stamp = |c: &rusqlite::Connection, at: &str| {
        c.execute("INSERT INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?3) \
                   ON CONFLICT (asset_id, exchange_id) DO UPDATE SET updated_at = excluded.updated_at", rusqlite::params![s.btc, s.exchange_id, at]).unwrap();
    };
    stamp(&o.primary, "2026-09-29 10:15:00");
    o.primary.execute("INSERT OR IGNORE INTO app_configs (key, value, created_at, updated_at) VALUES ('alpaca_crypto_listings_last_good_count', '31', \
                       '2026-09-29 10:15:00', '2026-09-29 10:15:00')", []).unwrap();
    handover::take_over(&lock, &o, &seed::cipher(), "0.2.0", at("2026-09-30T08:00:00Z")).unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    // A partial import of this engine, then a handback.
    state::mark_incomplete(&o.primary, reference::ALPACA_CRYPTO, None, at("2026-10-01T10:15:00Z")).unwrap();
    stamp(&o.primary, "2026-10-01 10:15:00");
    handover::hand_back(&lock, &o, &FakeFactory::default(), &seed::cipher(), &FixedClock(at("2026-10-01T12:00:00Z"))).await.unwrap();
    // Rails owns the install and refreshes the catalogue: inside its open window, so the door admits it.
    stamp(&o.primary, "2026-10-01 13:00:00");
    assert!(matches!(staleness::verdict(&o.primary, &bot, at("2026-10-02T00:00:00Z")).unwrap(), Verdict::Fresh), "Rails' own refresh counts");
    // This engine claims it again (the window closes at 10-02 01:00) and crashes before any later transaction.
    drop((lock, o));
    let (lock, o) = (lease::lock(&p, at("2026-10-02T01:00:00Z")).unwrap(), store::open(&p).unwrap());
    lease::claim(&lock, &o.primary, &seed::cipher(), "0.2.0", at("2026-10-02T01:00:00Z")).unwrap();
    drop((lock, o));
    // The restart takes over after that crash, and a new partial import stamps rows.
    let (lock, o) = (lease::lock(&p, at("2026-10-02T02:00:00Z")).unwrap(), store::open(&p).unwrap());
    let t = handover::take_over(&lock, &o, &seed::cipher(), "0.2.0", at("2026-10-02T02:00:00Z")).unwrap();
    assert!(matches!(t.claim, lease::Claim::AfterCrash));
    assert_eq!(deltabadger::app_config::get_plain(&o.primary, staleness::TAKEN_OVER_AT).unwrap().as_deref(), Some("2026-10-02T01:00:00.000Z"),
               "the window closed with the claim, and the restart did not move it");
    stamp(&o.primary, "2026-10-02 10:15:00");
    let now = at("2026-10-02T12:00:00Z"); // past 49 h since the last refresh this engine completed or read (Rails' 09-29 baseline)
    let stale_everywhere = |c: &rusqlite::Connection, when: &str| {
        assert!(staleness::stale(c, &bot, now).unwrap().is_some(), "the tick, {when}");
        assert!(matches!(staleness::verdict(c, &bot, now).unwrap(), Verdict::Stale(_)), "the door, {when}: 10-02 10:15 is this engine's partial stamp");
        assert!(staleness::report(c, now).unwrap().iter().any(|l| l.starts_with("Alpaca crypto tickers:") && l.contains("STALE")), "check, {when}");
    };
    stale_everywhere(&o.primary, "after the partial import");
    // A restart starts the scheduler despite staleness. The ownership window stays closed at 10-02 01:00,
    // and the partial stamp still counts for nothing at the tick or in the report.
    drop((lock, o));
    let (lock, o) = (lease::lock(&p, now).unwrap(), store::open(&p).unwrap());
    let restarted = handover::take_over(&lock, &o, &seed::cipher(), "0.2.0", now).map_err(|e| format!("{e:?}"));
    assert!(restarted.is_ok(), "the engine must start to refresh its own catalog");
    assert_eq!(deltabadger::app_config::get_plain(&o.primary, staleness::TAKEN_OVER_AT).unwrap().as_deref(), Some("2026-10-02T01:00:00.000Z"));
    stale_everywhere(&o.primary, "after the restart");
}
