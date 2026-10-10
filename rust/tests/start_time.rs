//! Bot::Startable against Rails' own answers (script/rust/start_time.rb -> fixtures/start_time_vectors.json). Rails-free.
use chrono::{DateTime, Utc};
use deltabadger::codec::parse_time;
use deltabadger::engine::schedule::{checkpoints, effective, Interval};
use deltabadger::web::bot::start::{initial_start_at, StartAt};
use serde_json::Value;

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/start_time_vectors.json")).unwrap() }
fn iso(us: i64) -> String { DateTime::from_timestamp_micros(us).unwrap().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string() }

#[test]
fn the_ruby_being_mirrored_has_not_changed() {
    use sha2::{Digest, Sha256};
    let v = vectors();
    let pinned = v["ported_sources"].as_object().unwrap();
    assert_eq!(pinned.len(), 5);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, sum) in pinned {
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(hex::encode(Sha256::digest(&bytes)), sum.as_str().unwrap(), "{path} changed since the vectors were recorded");
    }
}

fn rust(case: &[Value]) -> Result<StartAt, &'static str> {
    let (zone, now, mode, value) = (case[0].as_str().unwrap(), case[1].as_str().unwrap(), case[2].as_str(), case[3].as_str());
    let now: DateTime<Utc> = now.parse().unwrap();
    if mode == Some("date") { initial_start_at(mode, None, value, now, zone) } else { initial_start_at(mode, value, None, now, zone) }
}

/// Every mode, wall times in and around both DST transitions of Warsaw and New York (the spring gap, the autumn repeat),
/// clocks a week before to a day after, exactly on a candidate, and malformed input. Rails' nil is an `Err` here (the
/// :start validation refuses those first; nil must never read as "start now").
#[test]
fn initial_start_at_matches_rails() {
    let v = vectors();
    let cases = v["initial_start_at"].as_array().unwrap();
    assert!(cases.len() > 3_800, "{} cases", cases.len());
    let (mut early, mut gaps, mut repeated) = (0, 0, 0);
    for case in cases {
        let case = case.as_array().unwrap();
        match (rust(case), case[4].as_str()) {
            (Ok(s), Some(expected)) => {
                assert_eq!(s.at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true), expected, "{case:?}");
                early += usize::from(s.early);
            }
            (Err(_), None) => {}
            // A wall time that occurs twice that day is refused: Rails' pick is not proven for every zone.
            (Err(e), Some(_)) if e.contains("occurs twice") => repeated += 1,
            (got, expected) => panic!("{case:?}: Rust {got:?}, Rails {expected:?}"),
        }
        if case[3] == "02:30" && case[0] == "Warsaw" && case[4].as_str().is_some_and(|t| t.contains("T01:30:00Z")) { gaps += 1; }
    }
    assert!(early > 0 && gaps > 0 && repeated > 0, "the grid must reach the DST cases: {early} early, {gaps} gap, {repeated} repeated");
}

/// A passed candidate steps forward in fixed UTC days, so across the autumn change it lands an hour before the chosen
/// wall time (refused), across the spring change an hour after (Rails' answer, kept).
#[test]
fn only_a_start_before_the_chosen_time_is_early() {
    let at = |now: &str, mode: &str, time: &str, zone: &str| initial_start_at(Some(mode), Some(time), None, now.parse().unwrap(), zone).unwrap();
    // Saturday 2026-10-24 10:00 in Warsaw (CEST): 09:30 has passed, +1 day is Sunday 08:30 CET.
    let s = at("2026-10-24T08:00:00Z", "hour", "09:30", "Warsaw");
    assert_eq!((s.at.to_rfc3339(), s.early), ("2026-10-25T07:30:00+00:00".into(), true));
    // The same in UTC has no change to cross.
    assert!(!at("2026-10-24T10:00:00Z", "hour", "09:30", "UTC").early);
    // Saturday 2026-03-28 10:00 in Warsaw (CET): +1 day is Sunday 10:30 CEST, later than chosen.
    let s = at("2026-03-28T09:00:00Z", "hour", "09:30", "Warsaw");
    assert_eq!((s.at.to_rfc3339(), s.early), ("2026-03-29T08:30:00+00:00".into(), false));
    // Weekly: Sunday 2026-10-25 12:00 in New York (EDT), 09:30 passed, +7 days is Sunday 08:30 EST.
    let s = at("2026-10-25T16:00:00Z", "sunday", "09:30", "Eastern Time (US & Canada)");
    assert_eq!((s.at.to_rfc3339(), s.early), ("2026-11-01T13:30:00+00:00".into(), true));
    // A late-evening time stepping across midnight is judged against its own day, not the next.
    let s = at("2026-03-28T23:00:00Z", "hour", "23:30", "Warsaw");
    assert!(!s.early, "{s:?}");
}

/// What Rails reads around the delayed first run: next_interval_checkpoint_at and last_interval_checkpoint_at (both
/// `.round(6)`) from the anchor Bot::Lifecycle#start wrote, just before, exactly at and just after it, and at the next run.
#[test]
fn the_checkpoints_around_the_first_run_match_rails() {
    let v = vectors();
    let mut n = 0;
    for s in v["schedule"].as_array().unwrap() {
        let started = &s["started"];
        let anchor = parse_time(started["started_at"].as_str().unwrap()).unwrap();
        // Startable#repeat_anchor_at reads start_at while the rule is on; a fresh start writes both to one time.
        assert_eq!(started["settings"]["start_at"].as_str().unwrap().parse::<DateTime<Utc>>().unwrap(), anchor);
        assert_eq!(s["wait_until"].as_str().unwrap().parse::<DateTime<Utc>>().unwrap(), anchor, "the job waits for the anchor");
        let smart = s["settings"]["smart_interval_quote_amount"].as_f64();
        let eff = effective(Interval::parse(s["interval"].as_str().unwrap()).unwrap(), 60.0, smart);
        for read in s["reads"].as_array().unwrap().iter().chain(s.get("second")) {
            let now: DateTime<Utc> = read["now"].as_str().unwrap().parse().unwrap();
            let cps = checkpoints(anchor.timestamp_micros(), now.timestamp_micros(), eff).unwrap();
            assert_eq!((iso(cps.next_us), iso(cps.last_us)), (read["next"].as_str().unwrap().into(), read["last"].as_str().unwrap().into()), "{read}");
            n += 1;
        }
    }
    assert_eq!(n, 36 * 4 + 27);
}

#[test]
fn a_stored_float_zero_is_true_as_in_active_model() {
    // ActiveModel's FALSE_VALUES is a Set matched by eql?: it holds 0, not 0.0.
    use deltabadger::web::bot::cast_boolean;
    use serde_json::json;
    assert!(!cast_boolean(Some(&json!(0))));
    assert!(cast_boolean(Some(&json!(0.0))));
    assert!(!cast_boolean(Some(&json!("0"))));
}

#[test]
fn an_iana_zone_is_read_and_an_unknown_one_is_refused() {
    let now = parse_time("2026-03-02 12:00:00").unwrap();
    // 12:00 UTC is 07:00 in New York (EST), so 09:30 there is still ahead today: 14:30 UTC.
    let named = initial_start_at(Some("hour"), Some("09:30"), None, now, "Eastern Time (US & Canada)").unwrap();
    let iana = initial_start_at(Some("hour"), Some("09:30"), None, now, "America/New_York").unwrap();
    assert_eq!(named.at, iana.at);
    assert_eq!(iana.at, parse_time("2026-03-02 14:30:00").unwrap());
    assert_eq!(initial_start_at(Some("hour"), Some("09:30"), None, now, "Mars/Olympus").map(|s| s.at), Err("unknown time zone"));
}

/// A chosen time that occurs twice that day (the autumn repeat) is refused rather than picking an occurrence: Rails'
/// pick is proven only for Warsaw and New York, and Dublin's winter period is negative DST. A missing time moves forward
/// an hour only across a one-hour gap, the shape the vectors prove; any other gap is refused.
#[test]
fn a_repeated_or_unproven_missing_local_time_is_refused() {
    let at = |now: &str, time: &str, zone: &str| initial_start_at(Some("hour"), Some(time), None, now.parse().unwrap(), zone);
    for (now, time, zone) in [("2026-10-25T00:00:00Z", "01:30", "Europe/Dublin"), ("2026-10-24T22:00:00Z", "02:30", "Warsaw"),
                              ("2026-10-24T08:00:00Z", "02:30", "Warsaw")] {
        let got = at(now, time, zone);
        assert!(got.is_err_and(|e| e.contains("occurs twice")), "{zone} {time} at {now}: {got:?}");
    }
    // Lord Howe springs forward half an hour (02:00 -> 02:30 on 2026-10-04): 02:15 does not exist there.
    let got = at("2026-10-03T14:00:00Z", "02:15", "Australia/Lord_Howe");
    assert!(got.is_err_and(|e| e.contains("does not exist")), "{got:?}");
    // The one-hour gap stays Rails' answer: Warsaw 02:30 on 2026-03-29 is 03:30 CEST.
    assert_eq!(at("2026-03-28T23:30:00Z", "02:30", "Warsaw").map(|s| s.at.to_rfc3339()), Ok("2026-03-29T01:30:00+00:00".into()));
}

/// A date start reads its wall time in the owner's zone as ActiveSupport::TimeZone[] does: a Rails name or an IANA id.
/// An unknown name is refused, in the form and at the start, never read as UTC.
#[test]
fn a_date_start_reads_the_owners_zone_strictly() {
    use deltabadger::codec::parse_form_time;
    use serde_json::json;
    let now: DateTime<Utc> = "2026-09-10T12:00:00Z".parse().unwrap();
    for zone in ["America/New_York", "Eastern Time (US & Canada)"] {
        let got = parse_form_time(&json!("2026-09-11T09:30"), zone, now).map_err(|e| format!("{e:?}"));
        assert_eq!(got, Ok(json!("2026-09-11T13:30:00Z")), "{zone}");
    }
    assert!(parse_form_time(&json!("2026-09-11T09:30"), "Mars/Olympus", now).is_err());
    // A date that repeats in the owner's zone is refused as in hour mode.
    assert!(parse_form_time(&json!("2026-10-25T01:30"), "Europe/Dublin", now).is_err());
    let start = |zone: &str| initial_start_at(Some("date"), None, Some("2026-09-11T13:30:00Z"), now, zone).map(|s| s.at.to_rfc3339());
    assert_eq!(start("America/New_York"), Ok("2026-09-11T13:30:00+00:00".into()));
    assert_eq!(start("Mars/Olympus"), Err("unknown time zone"));
}
