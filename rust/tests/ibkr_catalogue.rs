//! The IBKR catalogue sync (Ibkr plan R1): data-api's IBKR feed through the shared importer behind a revision fence, every
//! run through the real Scheduler. The feed bodies are data-api's C2 end-to-end fixtures, pinned byte for byte.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::{model, staleness, FixedClock};
use deltabadger::jobs::data_api::{Config, DataApi};
use deltabadger::jobs::state::{self, JobState};
use deltabadger::jobs::{reference, Scheduler};
use deltabadger::venue::http::ScriptedTransport;
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

const T1: &str = "2026-10-24T14:00:00Z";
const T2: &str = "2026-10-25T14:00:00Z";
const T3: &str = "2026-10-25T15:00:00Z";
/// The run times: after every fixture's evidence.
const RUN: &str = "2026-10-25T16:00:00Z";

/// data-api's test/fixtures/files/catalogue_e2e/SHA256SUMS.
const FIXTURES: [(&str, &str); 6] = [
    ("feed", "d118ad3d6268709c3d5e31b0b573fc421d3606067327876c60cc208e3cd4f5fc"),
    ("feed_revoked", "955cc4d26a3d177a5109cbf4e9825637b349f59c6ec5d4283fe4c505a6d31062"),
    ("feed_revoked_no_evidence", "4075b6d61cda3ae25459bb6789ba6b04455e1bbaf106b86f5b500f31598c849b"),
    ("feed_newer_evidence_same_rows", "bdea23b567826bc4187669c3cb21c39ec9559f6bbee64103a472551b6627b9e4"),
    ("feed_complete_flip_same_rows", "f4ed8e470d1808fc16fb6b9115c7e4448c6ab0275b26cd305c4af6f7b653269e"),
    ("feed_empty_complete", "d01f4653ce9a7d5824c6db17b8bf5866efbc5fa0c9a3e12d28bf8d558a401ddc"),
];

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn later(s: &str, minutes: i64) -> String { (at(s) + Duration::minutes(minutes)).to_rfc3339() }

fn fixture(name: &str) -> Value {
    let bytes = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/catalogue_e2e/{name}.json"))).unwrap();
    let sum = FIXTURES.iter().find(|(n, _)| *n == name).unwrap().1;
    assert_eq!(hex(&Sha256::digest(&bytes)), sum, "{name}.json differs from data-api's");
    serde_json::from_slice(&bytes).unwrap()
}
fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

/// A listing as the feed carries it, for payloads no fixture has.
fn listing(conid: i64, base: &str, ext: &str) -> Value {
    json!({ "listing_id": format!("ibkr:{conid}"), "venue": "ibkr", "venue_scheme": "ibkr_conid", "base": base, "quote": "EUR",
            "ticker": conid.to_string(), "base_asset_id": format!("etf:{ext}"), "base_external_id": ext, "quote_external_id": "EUR.FOREX",
            "base_decimals": 0, "quote_decimals": 2, "price_decimals": 2, "minimum_base_size": "1", "fractionable": false, "trading_enabled": true })
}
fn payload(revision: i64, generated_at: &str, complete: bool, rows: Vec<Value>) -> Value {
    json!({ "metadata": { "revision": revision, "generated_at": generated_at, "complete": complete, "count": rows.len() }, "data": rows })
}

fn primary(d: &tempfile::TempDir) -> Connection { Connection::open(d.path().join("production.sqlite3")).unwrap() }

fn add_assets(c: &Connection) {
    for (ext, symbol, category) in [("EUNL.XETRA", "EUNL", "Stock"), ("VOO.US", "VOO", "Stock"), ("IWDA.AS", "IWDA", "Stock"), ("EUR.FOREX", "EUR", "Currency")] {
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, ?3, '2026-01-01', '2026-01-01')",
                  [ext, symbol, category]).unwrap();
    }
}

/// An install with the IBKR venue and the fixtures' assets (`usd` comes from the sync itself).
fn install() -> tempfile::TempDir {
    let d = common::baseline_install();
    let c = primary(&d);
    c.execute("INSERT INTO exchanges (type, name, available, maker_fee, taker_fee, created_at, updated_at) \
               VALUES ('Exchanges::Ibkr', 'Interactive Brokers', 1, '0.0', '0.0', '2026-01-01', '2026-01-01')", []).unwrap();
    add_assets(&c);
    d
}

/// One run of the IBKR job by the real Scheduler on `body` at `now`; the job's state once the runner recorded it.
async fn sync(d: &tempfile::TempDir, body: &Value, now: &str) -> JobState {
    let t = ScriptedTransport::default();
    t.reply("GET /api/v2/listings?venue_scheme=ibkr_conid", 200, body.clone());
    let api = DataApi::new(Config { url: "http://data-api:3000".into(), token: "tok".into() }, t.clone(), t);
    let jobs = reference::jobs(Some(api)).into_iter().filter(|j| j.spec().name == reference::IBKR).collect();
    let scheduler = Scheduler::new(primary(d), seed::cipher(), jobs, None);
    scheduler.wakers().wake(reference::IBKR, None, None);
    let (stop, rx) = tokio::sync::watch::channel(false);
    let (clock, now) = (FixedClock(at(now)), at(now));
    let recorded = async {
        loop {
            let st = state::read(&primary(d), reference::IBKR, None).unwrap();
            if st.last_run_at == Some(now) || st.last_error_at == Some(now) { stop.send(true).unwrap(); return st; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    let (ran, st) = tokio::time::timeout(std::time::Duration::from_secs(20), async { tokio::join!(scheduler.run(rx, &clock), recorded) })
        .await.expect("the run is recorded");
    ran.unwrap();
    st
}

/// Every row of `table`, every column, as SQLite stores it.
fn dump(c: &Connection, table: &str) -> Vec<Vec<String>> {
    let mut s = c.prepare(&format!("SELECT * FROM {table} ORDER BY 1")).unwrap();
    let n = s.column_count();
    s.query_map([], |r| (0..n).map(|i| r.get_ref(i).map(|v| format!("{v:?}"))).collect()).unwrap().collect::<Result<_, _>>().unwrap()
}
/// ticker → (available, trading_enabled).
fn flags(c: &Connection) -> Vec<(String, bool, bool)> {
    let mut s = c.prepare("SELECT ticker, available, trading_enabled FROM tickers ORDER BY ticker").unwrap();
    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap()
}
fn conids(c: &Connection) -> Vec<(i64, String)> {
    let mut s = c.prepare("SELECT k.conid, a.external_id FROM ibkr_conids k JOIN assets a ON a.id = k.asset_id ORDER BY k.conid").unwrap();
    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
}
fn fence(st: &JobState) -> (Option<i64>, Option<i64>, Option<DateTime<Utc>>) { (st.revision, st.importing_revision, st.source_at) }

#[tokio::test(flavor = "current_thread")]
async fn the_six_end_to_end_fixtures_import_as_data_api_published_them() {
    let both = |voo: bool| vec![("100001".to_string(), true, true), ("100002".to_string(), true, voo)];
    // Each scenario from `feed`'s state, as data-api's fixture test builds them.
    for (name, revision, source_at, expected) in [
        ("feed", 1, T1, both(true)),
        ("feed_revoked", 2, T2, both(false)),
        ("feed_revoked_no_evidence", 2, T1, both(false)),
        ("feed_newer_evidence_same_rows", 1, T2, both(true)),
        ("feed_complete_flip_same_rows", 3, T3, both(true)),
    ] {
        let d = install();
        let st = sync(&d, &fixture("feed"), RUN).await;
        assert_eq!(fence(&st), (Some(1), None, Some(at(T1))));
        assert_eq!(conids(&primary(&d)), vec![(100_001, "EUNL.XETRA".into()), (100_002, "VOO.US".into())]);
        let st = sync(&d, &fixture(name), &later(RUN, 1)).await;
        assert_eq!(fence(&st), (Some(revision), None, Some(at(source_at))), "{name}");
        assert_eq!(st.last_error, None, "{name}");
        assert_eq!(flags(&primary(&d)), expected, "{name}");
        let quotes: Vec<(String, String)> = primary(&d).prepare("SELECT t.ticker, q.external_id FROM tickers t JOIN assets q ON q.id = t.quote_asset_id ORDER BY t.ticker")
            .unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(quotes, vec![("100001".into(), "EUR.FOREX".into()), ("100002".into(), "usd".into())], "{name}");
    }
    // A fresh instance whose healthy run found nothing executable.
    let d = install();
    let st = sync(&d, &fixture("feed_empty_complete"), RUN).await;
    assert_eq!(fence(&st), (Some(1), None, Some(at(T1))));
    assert!(flags(&primary(&d)).is_empty() && conids(&primary(&d)).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn an_older_revision_leaves_ticker_rows_and_ibkr_conids_byte_identical() {
    let d = install();
    sync(&d, &fixture("feed_revoked"), RUN).await;
    let (tickers, ids) = (dump(&primary(&d), "tickers"), dump(&primary(&d), "ibkr_conids"));
    let before = state::read(&primary(&d), reference::IBKR, None).unwrap();
    let st = sync(&d, &fixture("feed_newer_evidence_same_rows"), &later(RUN, 1)).await; // revision 1, newer evidence
    assert_eq!((dump(&primary(&d), "tickers"), dump(&primary(&d), "ibkr_conids")), (tickers, ids));
    assert_eq!(fence(&st), fence(&before), "an older revision moves neither the revision nor the evidence");
    assert_eq!(st.last_error, None);
}

#[tokio::test(flavor = "current_thread")]
async fn a_newer_takeover_aborts_the_older_import_and_plans_against_its_committed_units() {
    let d = install();
    // Revision 1 (incomplete) inserts X (conid 100003). Right after that unit commits, revision 2 takes the import over: the
    // trigger writes revision 2's takeover in the same commit, the serial order of a takeover landing between two units.
    primary(&d).execute_batch("CREATE TRIGGER takeover AFTER INSERT ON tickers WHEN NEW.ticker = '100003' BEGIN \
        UPDATE app_configs SET value = json_set(value, '$.importing_revision', 2) WHERE key = 'rust_job.sync_ibkr_listings_job'; END;").unwrap();
    let n = payload(1, T1, false, vec![listing(100_001, "EUNL", "EUNL.XETRA"), listing(100_003, "IWDA", "IWDA.AS")]);
    let st = sync(&d, &n, RUN).await;
    assert_eq!(fence(&st), (None, Some(2), None), "revision 1's last unit aborted: it never became the revision");
    assert_eq!(st.last_error, None, "a superseded import is not an error");
    assert_eq!(flags(&primary(&d)), vec![("100001".into(), true, true), ("100003".into(), true, true)], "the unit before the takeover stays committed");
    primary(&d).execute_batch("DROP TRIGGER takeover").unwrap();
    // Revision 2, complete and without X, plans after its takeover: X is in its plan, and swept.
    let st = sync(&d, &fixture("feed_revoked"), &later(RUN, 1)).await;
    assert_eq!(fence(&st), (Some(2), None, Some(at(T2))));
    assert_eq!(flags(&primary(&d)), vec![("100001".into(), true, true), ("100002".into(), true, false), ("100003".into(), false, true)]);
    assert_eq!(conids(&primary(&d)).len(), 3, "ibkr_conids is append-only: X's identity stays");
}

#[tokio::test(flavor = "current_thread")]
async fn a_crash_between_units_leaves_the_takeover_and_the_rerun_completes_it() {
    let d = install();
    primary(&d).execute_batch("CREATE TRIGGER crash BEFORE INSERT ON tickers BEGIN SELECT RAISE(ABORT, 'crash'); END;").unwrap();
    let st = sync(&d, &fixture("feed"), RUN).await;
    assert!(st.last_error.as_deref().is_some_and(|m| m.contains("crash")), "{st:?}");
    assert_eq!(fence(&st), (None, Some(1), None));
    assert!(st.incomplete_since.is_some(), "the scheduler runs it again at the next start");
    let c = primary(&d);
    assert!(flags(&c).is_empty(), "the failed unit rolled back");
    assert_eq!(conids(&c).len(), 2, "the units before it stay committed");
    c.execute_batch("DROP TRIGGER crash").unwrap();
    let st = sync(&d, &fixture("feed"), &later(RUN, 1)).await;
    assert_eq!(fence(&st), (Some(1), None, Some(at(T1))), "the same revision takes the import over again and completes it");
    assert_eq!((st.incomplete_since, st.last_success_at), (None, Some(at(&later(RUN, 1)))));
    assert_eq!(flags(&primary(&d)), vec![("100001".into(), true, true), ("100002".into(), true, true)]);
}

#[tokio::test(flavor = "current_thread")]
async fn newer_evidence_with_the_same_revision_advances_source_at_only_and_keeps_bots_fresh() {
    let (d, o, s) = common::install_alpaca();
    o.primary.execute("UPDATE exchanges SET type = 'Exchanges::Ibkr' WHERE id = ?1", [s.exchange_id]).unwrap();
    add_assets(&o.primary);
    sync(&d, &fixture("feed"), RUN).await; // sweeps the seed's crypto ticker, so the bot comes after
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-10-01 10:00:00"))).unwrap();
    let (tickers, ids) = (dump(&primary(&d), "tickers"), dump(&primary(&d), "ibkr_conids"));
    let past = |t: &str| at(t) + Duration::hours(50);
    let stale = staleness::stale(&primary(&d), &bot, past(T1)).unwrap().expect("50 h after the evidence");
    assert!(stale.message.contains("IBKR tickers"), "{}", stale.message);
    let st = sync(&d, &fixture("feed_newer_evidence_same_rows"), &later(RUN, 1)).await;
    assert_eq!(fence(&st), (Some(1), None, Some(at(T2))));
    assert_eq!((dump(&primary(&d), "tickers"), dump(&primary(&d), "ibkr_conids")), (tickers, ids), "rows untouched");
    assert!(staleness::stale(&primary(&d), &bot, past(T1)).unwrap().is_none(), "fresh beyond 49 h of unchanged content");
    assert!(staleness::stale(&primary(&d), &bot, past(T2)).unwrap().is_some(), "the bound runs from the newest evidence");
}

#[tokio::test(flavor = "current_thread")]
async fn the_complete_flip_with_identical_rows_sweeps_and_an_empty_complete_payload_sweeps_all() {
    let d = install();
    sync(&d, &fixture("feed"), RUN).await;
    // A failed upstream run publishes incomplete rows: what it adds is imported, nothing is swept.
    let mut rows = fixture("feed")["data"].as_array().unwrap().clone();
    rows.push(listing(100_003, "IWDA", "IWDA.AS"));
    sync(&d, &payload(2, T2, false, rows), &later(RUN, 1)).await;
    let all = |x: bool| vec![("100001".to_string(), true, true), ("100002".to_string(), true, true), ("100003".to_string(), x, true)];
    assert_eq!(flags(&primary(&d)), all(true));
    let st = sync(&d, &fixture("feed_complete_flip_same_rows"), &later(RUN, 2)).await;
    assert_eq!(fence(&st), (Some(3), None, Some(at(T3))));
    assert_eq!(flags(&primary(&d)), all(false), "complete again with feed's rows: the pair it lacks is swept");
    sync(&d, &payload(4, T3, true, vec![]), &later(RUN, 3)).await;
    assert!(flags(&primary(&d)).iter().all(|(_, available, _)| !available), "{:?}", flags(&primary(&d)));
}

#[tokio::test(flavor = "current_thread")]
async fn a_revocation_with_old_evidence_disables_the_listing_without_moving_source_at() {
    let d = install();
    sync(&d, &fixture("feed"), RUN).await;
    let st = sync(&d, &fixture("feed_revoked_no_evidence"), &later(RUN, 1)).await;
    assert_eq!(fence(&st), (Some(2), None, Some(at(T1))));
    assert_eq!(flags(&primary(&d)), vec![("100001".into(), true, true), ("100002".into(), true, false)]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_future_generated_at_is_refused_and_nothing_is_written() {
    let d = install();
    let mut body = fixture("feed");
    body["metadata"]["generated_at"] = json!(later(RUN, 6));
    let st = sync(&d, &body, RUN).await;
    assert!(st.last_error.as_deref().is_some_and(|m| m.contains("in the future")), "{st:?}");
    assert_eq!(fence(&st), (None, None, None));
    assert!(flags(&primary(&d)).is_empty() && conids(&primary(&d)).is_empty());
    body["metadata"]["generated_at"] = json!(later(RUN, 4));
    let st = sync(&d, &body, &later(RUN, 1)).await;
    assert_eq!(fence(&st), (Some(1), None, Some(at(&later(RUN, 4)))), "within 5 minutes is clock skew, accepted");
}

#[tokio::test(flavor = "current_thread")]
async fn a_conid_moved_to_another_asset_is_skipped() {
    let d = install();
    sync(&d, &fixture("feed"), RUN).await;
    let mut moved = fixture("feed_revoked");
    moved["data"][1] = listing(100_002, "IWDA", "IWDA.AS"); // VOO's conid now names another asset
    let st = sync(&d, &moved, &later(RUN, 1)).await;
    assert_eq!(fence(&st), (Some(2), None, Some(at(T2))));
    assert_eq!(conids(&primary(&d)), vec![(100_001, "EUNL.XETRA".into()), (100_002, "VOO.US".into())], "the identity never moves");
    assert_eq!(flags(&primary(&d)), vec![("100001".into(), true, true), ("100002".into(), false, true)],
               "the row is skipped, and the complete payload sweeps VOO's pair");
    let c = primary(&d);
    let iwda: i64 = c.query_row("SELECT count(*) FROM tickers t JOIN assets a ON a.id = t.base_asset_id WHERE a.external_id = 'IWDA.AS'", [], |r| r.get(0)).unwrap();
    assert_eq!(iwda, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_malformed_feed_is_refused_before_anything_is_written() {
    for broken in [json!({ "metadata": { "revision": 1, "generated_at": T1, "complete": true, "count": 3 }, "data": fixture("feed")["data"] }),
                   payload(1, T1, true, vec![json!({ "listing_id": "ibkr:5", "venue_scheme": "ibkr_conid", "ticker": "6", "trading_enabled": true })]),
                   payload(1, T1, true, vec![json!({ "listing_id": "ibkr:5", "venue_scheme": "ibkr_conid", "ticker": "5" })])] {
        let d = install();
        let st = sync(&d, &broken, RUN).await;
        assert!(st.last_error.as_deref().is_some_and(|m| m.contains("nothing imported")), "{st:?}");
        assert_eq!(fence(&st), (None, None, None));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn listings_whose_assets_are_missing_leave_a_partial_revision_that_the_same_revision_completes() {
    let d = install();
    sync(&d, &payload(1, T1, true, vec![listing(100_001, "EUNL", "EUNL.XETRA"), listing(100_003, "IWDA", "IWDA.AS")]), RUN).await;
    primary(&d).execute("DELETE FROM assets WHERE external_id = 'VOO.US'", []).unwrap(); // not synced yet
    // Revision 2 is complete but VOO cannot be imported: EUNL is, nothing is swept, and the revision is marked partial.
    let st = sync(&d, &fixture("feed_revoked"), &later(RUN, 1)).await;
    assert_eq!((fence(&st), st.partial), ((Some(2), None, Some(at(T2))), true));
    assert_eq!(flags(&primary(&d)), vec![("100001".into(), true, true), ("100003".into(), true, true)], "no sweep from a partial import");
    // The asset arrives; the same revision imports again instead of only refreshing its evidence.
    primary(&d).execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('VOO.US', 'VOO', 'VOO', 'Stock', '2026-01-01', '2026-01-01')", []).unwrap();
    let st = sync(&d, &fixture("feed_revoked"), &later(RUN, 2)).await;
    assert_eq!((fence(&st), st.partial), ((Some(2), None, Some(at(T2))), false));
    assert_eq!(flags(&primary(&d)), vec![("100001".into(), true, true), ("100002".into(), true, false), ("100003".into(), false, true)]);
    assert_eq!(conids(&primary(&d)).len(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn a_complete_feed_with_one_malformed_row_is_refused_and_sweeps_nothing() {
    let d = install();
    sync(&d, &fixture("feed"), RUN).await;
    let tickers = dump(&primary(&d), "tickers");
    for (i, (field, value)) in [("base_external_id", Value::Null), ("quote_external_id", json!("")), ("base", Value::Null), ("price_decimals", json!("2")),
                           ("base_decimals", json!(-1)), ("minimum_base_size", Value::Null), ("minimum_quote_size", json!("abc"))].into_iter().enumerate() {
        let mut body = fixture("feed_revoked");
        body["data"][1][field] = value;
        let st = sync(&d, &body, &later(RUN, 1 + i as i64)).await;
        assert!(st.last_error.as_deref().is_some_and(|m| m.contains("nothing imported")), "{field}: {st:?}");
        assert_eq!(fence(&st), (Some(1), None, Some(at(T1))), "{field}");
        assert_eq!(dump(&primary(&d), "tickers"), tickers, "{field}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_feed_without_evidence_yet_imports_nothing_and_is_not_an_error_but_rows_without_evidence_are_refused() {
    // data-api's IBKR feed before any conid resolved, exactly as production serves it.
    let empty = json!({ "metadata": { "revision": 1, "generated_at": null, "complete": false, "count": 0 }, "data": [] });
    let d = install();
    let st = sync(&d, &empty, RUN).await;
    assert_eq!((st.last_error.clone(), st.last_success_at, st.last_run_at), (None, None, Some(at(RUN))), "nothing new: a run, no error, no retry");
    assert_eq!(fence(&st), (None, None, None), "no source_at");
    assert!(flags(&primary(&d)).is_empty() && conids(&primary(&d)).is_empty());
    let mut rows = fixture("feed");
    rows["metadata"]["generated_at"] = Value::Null;
    let st = sync(&d, &rows, &later(RUN, 1)).await;
    assert!(st.last_error.as_deref().is_some_and(|m| m.contains("rows without evidence")), "{st:?}");
    assert_eq!(fence(&st), (None, None, None));
    assert!(flags(&primary(&d)).is_empty());
    let mut bad = fixture("feed");
    bad["metadata"]["generated_at"] = json!("yesterday");
    let st = sync(&d, &bad, &later(RUN, 2)).await;
    assert!(st.last_error.as_deref().is_some_and(|m| m.contains("not a time")), "{st:?}");
}
