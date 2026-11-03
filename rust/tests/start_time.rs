//! Bot::Startable against Rails' own answers (script/rust/start_time.rb -> fixtures/start_time_vectors.json). Rails-free.
use chrono::{DateTime, Utc};
use deltabadger::codec::parse_time;
use deltabadger::engine::schedule::{checkpoints, effective, Interval};
use deltabadger::web::bot::start::initial_start_at;
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

fn rust(case: &[Value]) -> Result<DateTime<Utc>, &'static str> {
    let (zone, now, mode, value) = (case[0].as_str().unwrap(), case[1].as_str().unwrap(), case[2].as_str(), case[3].as_str());
    let now: DateTime<Utc> = now.parse().unwrap();
    if mode == Some("date") { initial_start_at(mode, None, value, now, zone) } else { initial_start_at(mode, value, None, now, zone) }
}

/// The oracle, written from chrono's own answers and nothing in src/: the chosen wall time on the first day (of the chosen
/// weekday) whose instant is after now, a repeated wall time at its earlier occurrence, a missing one an hour later until
/// it exists.
fn oracle(zone: &str, now: DateTime<Utc>, mode: &str, time: &str) -> DateTime<Utc> {
    use chrono::{Datelike, Duration, LocalResult, NaiveTime, TimeZone};
    let tz = deltabadger::web::timezone::strict(zone).unwrap();
    let (h, m) = time.split_once(':').map(|(h, m)| (h.parse().unwrap(), m.parse().unwrap())).unwrap();
    let chosen = NaiveTime::from_hms_opt(h, m, 0).unwrap();
    let today = now.with_timezone(&tz).date_naive();
    let resolve = |day: chrono::NaiveDate| {
        let mut wall = day.and_time(chosen);
        loop {
            match tz.from_local_datetime(&wall) {
                LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => return at.with_timezone(&Utc),
                LocalResult::None => wall += Duration::hours(1),
            }
        }
    };
    let weekday = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"].iter().position(|m| *m == mode);
    (0..15).map(|d| today + Duration::days(d))
        .filter(|day| weekday.is_none_or(|w| day.weekday().num_days_from_monday() as usize == w))
        .map(resolve).find(|at| *at > now).unwrap()
}

/// Every mode, wall times in and around the DST changes of Warsaw, New York, Lord Howe (30 minutes), Dublin and Casablanca
/// (negative DST), clocks a week before to a day after, exactly on a candidate, and malformed input. Rails' nil is an `Err`
/// here (the :start validation refuses those first; nil must never read as "start now"). Rust is the oracle's answer in
/// every case; where Rails differs (D-START-WALL-STEP), Rails' instant is not the chosen wall time, and each cause occurs:
/// the fixed-UTC step across a change, and a gap moved today leaking into a weekday's target day.
#[test]
fn initial_start_at_matches_rails_or_the_chosen_wall_time() {
    let v = vectors();
    let cases = v["initial_start_at"].as_array().unwrap();
    assert_eq!(cases.len(), 4351);
    let (mut same, mut step, mut leak, mut repeated) = (0, 0, 0, 0);
    for case in cases {
        let case = case.as_array().unwrap();
        let got = rust(case);
        let Some(rails) = case[4].as_str() else { assert!(got.is_err(), "{case:?}: Rails nil, Rust {got:?}"); continue };
        let rails: DateTime<Utc> = rails.parse().unwrap();
        let got = got.unwrap_or_else(|e| panic!("{case:?}: {e}"));
        let (zone, now, mode) = (case[0].as_str().unwrap(), case[1].as_str().unwrap().parse().unwrap(), case[2].as_str().unwrap());
        if mode == "date" { assert_eq!(got, rails, "{case:?}"); same += 1; continue; }
        let raw = case[3].as_str().unwrap();
        assert_eq!(got, oracle(zone, now, mode, raw), "{case:?}");
        let time = got.with_timezone(&deltabadger::web::timezone::strict(zone).unwrap()).format("%H:%M").to_string();
        let tz = deltabadger::web::timezone::strict(zone).unwrap();
        use chrono::TimeZone;
        if tz.from_local_datetime(&got.with_timezone(&tz).naive_local()).single().is_none() { repeated += 1; }
        if got == rails { same += 1; continue; }
        // Rails missed the chosen time: either its wall time is not the chosen one (a fixed-UTC step across a change), or it
        // is the chosen one moved by a gap that was today's, not the target day's.
        let local = rails.with_timezone(&tz);
        let today_missing = tz.from_local_datetime(&now.with_timezone(&tz).date_naive().and_time(got.with_timezone(&tz).time())).earliest().is_none();
        if local.format("%H:%M").to_string() != time && (rails - got).num_minutes().abs() <= 60 {
            if mode != "hour" && today_missing { leak += 1 } else { step += 1 }
            continue;
        }
        panic!("{case:?}: Rust {got}, Rails {rails}: neither the same nor Rails off the chosen wall time");
    }
    assert!(same > 3_500 && step > 0 && leak > 0 && repeated > 0, "{same} same, {step} fixed-day steps, {leak} gap leaks, {repeated} repeated");
}

/// D-START-WALL-STEP: a passed time steps to the chosen wall time on the next day (or week), across a change too.
#[test]
fn a_passed_start_steps_to_the_chosen_wall_time() {
    let at = |now: &str, mode: &str, time: &str, zone: &str| initial_start_at(Some(mode), Some(time), None, now.parse().unwrap(), zone).unwrap().to_rfc3339();
    // Saturday 2026-10-24 10:00 in Warsaw (CEST): 09:30 has passed; Sunday 09:30 CET (Rails: 08:30 CET, an hour early).
    assert_eq!(at("2026-10-24T08:00:00Z", "hour", "09:30", "Warsaw"), "2026-10-25T08:30:00+00:00");
    // Saturday 2026-03-28 10:00 in Warsaw (CET): Sunday 09:30 CEST (Rails: 10:30 CEST, an hour late).
    assert_eq!(at("2026-03-28T09:00:00Z", "hour", "09:30", "Warsaw"), "2026-03-29T07:30:00+00:00");
    // Weekly: Sunday 2026-10-25 12:00 in New York (EDT), 09:30 passed: next Sunday 09:30 EST (Rails 08:30 EST).
    assert_eq!(at("2026-10-25T16:00:00Z", "sunday", "09:30", "Eastern Time (US & Canada)"), "2026-11-01T14:30:00+00:00");
    // Lord Howe's gap day (02:00 -> 02:30 on 2026-10-04): a Saturday 02:15 seen that day is next Saturday's 02:15 +11:00
    // (Rails carries the gap's 03:15 into it).
    assert_eq!(at("2026-10-03T16:37:00Z", "saturday", "02:15", "Australia/Lord_Howe"), "2026-10-09T15:15:00+00:00");
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
            let cps = checkpoints(anchor.timestamp_micros(), now.timestamp_micros(), eff.clone()).unwrap();
            assert_eq!((iso(cps.next_us), iso(cps.last_us)), (read["next"].as_str().unwrap().into(), read["last"].as_str().unwrap().into()), "{read}");
            n += 1;
        }
    }
    assert_eq!(n, 48 * 4 + 36);
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
    assert_eq!(named, iana);
    assert_eq!(iana, parse_time("2026-03-02 14:30:00").unwrap());
    assert_eq!(initial_start_at(Some("hour"), Some("09:30"), None, now, "Mars/Olympus"), Err("unknown time zone"));
}

/// D-START-REPEAT-EARLIER: a chosen time that occurs twice that day is its earlier occurrence, in every zone (Rails' pick
/// in every recorded one, Dublin's and Casablanca's negative DST included); one that does not occur moves forward an hour,
/// a 30-minute gap included.
#[test]
fn a_repeated_time_is_its_earlier_occurrence_and_a_missing_one_moves_an_hour() {
    let at = |now: &str, time: &str, zone: &str| initial_start_at(Some("hour"), Some(time), None, now.parse().unwrap(), zone).unwrap().to_rfc3339();
    // Dublin 2026-10-25 01:30 occurs at 00:30Z (IST) and 01:30Z (GMT): the earlier, as recorded Rails picks.
    assert_eq!(at("2026-10-24T22:00:00Z", "01:30", "Dublin"), "2026-10-25T00:30:00+00:00");
    // Between the two occurrences the earlier one has passed: the next day's, never today's later one.
    assert_eq!(at("2026-10-25T00:45:00Z", "01:30", "Dublin"), "2026-10-26T01:30:00+00:00");
    assert_eq!(at("2026-10-24T22:00:00Z", "02:30", "Warsaw"), "2026-10-25T00:30:00+00:00");
    // Casablanca 2026-02-15 02:30 (+01:00 then +00:00).
    assert_eq!(at("2026-02-14T23:00:00Z", "02:30", "Casablanca"), "2026-02-15T01:30:00+00:00");
    // Lord Howe 02:15 on 2026-10-04 does not exist (02:00 -> 02:30): 03:15 +11:00, as recorded.
    assert_eq!(at("2026-10-03T14:00:00Z", "02:15", "Australia/Lord_Howe"), "2026-10-03T16:15:00+00:00");
    // The one-hour gap: Warsaw 02:30 on 2026-03-29 is 03:30 CEST.
    assert_eq!(at("2026-03-28T23:30:00Z", "02:30", "Warsaw"), "2026-03-29T01:30:00+00:00");
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
    // A date that repeats in the owner's zone is its earlier occurrence, as in hour mode; one in a gap an hour later.
    assert_eq!(parse_form_time(&json!("2026-10-25T01:30"), "Europe/Dublin", now).map_err(|e| format!("{e:?}")), Ok(json!("2026-10-25T00:30:00Z")));
    assert_eq!(parse_form_time(&json!("2026-10-04T02:15"), "Australia/Lord_Howe", now).map_err(|e| format!("{e:?}")), Ok(json!("2026-10-03T16:15:00Z")));
    let start = |zone: &str| initial_start_at(Some("date"), None, Some("2026-09-11T13:30:00Z"), now, zone).map(|s| s.to_rfc3339());
    assert_eq!(start("America/New_York"), Ok("2026-09-11T13:30:00+00:00".into()));
    assert_eq!(start("Mars/Olympus"), Err("unknown time zone"));
}
