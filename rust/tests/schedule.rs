mod common;
use chrono::{DateTime, Utc};
use deltabadger::engine::schedule::*;

fn micros(t: &str) -> i64 { t.parse::<DateTime<Utc>>().unwrap().timestamp_micros() }

#[test]
fn every_recorded_rails_schedule_is_reproduced() {
    let cases = common::vectors()["schedule"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 351);
    for c in cases {
        let s = &c["settings"];
        let smart = (s["smart_intervaled"] == true).then(|| s["smart_interval_quote_amount"].as_f64()).flatten();
        let eff = effective(Interval::parse(s["interval"].as_str().unwrap()).unwrap(), s["quote_amount"].as_f64().unwrap(), smart);
        let cp = checkpoints(micros(c["anchor"].as_str().unwrap()), micros(c["now"].as_str().unwrap()), eff.clone()).unwrap();
        assert_eq!(cp.next_us, micros(c["next"].as_str().unwrap()), "next {c}");
        assert_eq!(cp.last_us, micros(c["last"].as_str().unwrap()), "last {c}");
        assert_eq!(interval_count(cp.last_us, micros(c["anchor"].as_str().unwrap()), eff).unwrap(), c["count"].as_i64().unwrap(), "count {c}");
    }
}

#[test]
fn a_tick_exactly_on_the_grid_does_not_count_that_interval_yet() {
    // Rails: at now == checkpoint, ceil gives the same point, so `last` is the PREVIOUS one. The engine's
    // loop must therefore run strictly after a checkpoint (Task 11).
    let eff = effective(Interval::Day, 10.0, None);
    let anchor = micros("2026-09-01T00:00:00Z");
    let on = checkpoints(anchor, anchor + 86_400_000_000, eff.clone()).unwrap();
    let after = checkpoints(anchor, anchor + 86_400_000_001, eff).unwrap();
    assert_eq!(on.last_us, anchor);
    assert_eq!(after.last_us, anchor + 86_400_000_000);
}

#[test]
fn calendar_edges_and_extreme_splits_are_fallible() {
    for (anchor, now, eff) in [
        (DateTime::<Utc>::MAX_UTC.timestamp_micros(), DateTime::<Utc>::MAX_UTC.timestamp_micros(), Effective::Month),
        (DateTime::<Utc>::MIN_UTC.timestamp_micros(), DateTime::<Utc>::MIN_UTC.timestamp_micros() - 1, Effective::Month),
        (micros("2026-09-01T00:00:00Z"), micros("2026-09-01T00:00:30Z"), effective(Interval::Hour, 1e-18, Some(10.0))),
        (0, 30_000_000, Effective::Seconds(f64::MIN_POSITIVE)),
        (0, 30_000_000, Effective::Seconds(f64::NAN)),
        (0, 30_000_000, Effective::Seconds(0.0)),
    ] {
        assert!(checkpoints(anchor, now, eff).is_err());
    }
}

fn sell_vectors() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/sell_vectors.json")).unwrap()
}

#[test]
fn every_recorded_rails_sell_schedule_is_reproduced() {
    let cases = sell_vectors()["schedule"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 4192);
    let (mut checked, mut short) = (0, 0);
    for c in cases {
        let s = c["settings"].as_object().unwrap();
        let eff = sell_effective(s).unwrap();
        assert_eq!(eff.seconds(), c["seconds"].as_str().unwrap().parse::<f64>().unwrap(), "seconds {c}");
        assert_eq!(matches!(eff, Effective::Month | Effective::MonthSeconds(_)), c["month"] == true, "month {c}");
        // Under a second between two sells no working bot is allowed (web MIN_SPAN_SECONDS; eligibility refuses it).
        if eff.seconds() < 1.0 { short += 1; continue; }
        let cp = checkpoints(micros(c["anchor"].as_str().unwrap()), micros(c["now"].as_str().unwrap()), eff).unwrap();
        assert_eq!(cp.next_us, micros(c["next"].as_str().unwrap()), "next {c}");
        assert_eq!(cp.last_us, micros(c["last"].as_str().unwrap()), "last {c}");
        checked += 1;
    }
    println!("{checked} sell checkpoints reproduced, {short} sub-second cadences skipped");
    assert_eq!((checked, short), (3632, 560));
}

#[test]
fn a_buying_bot_has_no_sell_cadence() {
    let s = serde_json::json!({"interval": "day", "quote_amount": 10.0, "sell_interval": "hour"});
    assert_eq!(sell_effective(s.as_object().unwrap()), None);
}

#[test]
fn a_stopped_seller_resumes_only_after_a_whole_sell_interval() {
    let cases = sell_vectors()["restart"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 200);
    for c in cases {
        let eff = sell_effective(c["settings"].as_object().unwrap()).unwrap();
        let elapsed = micros(c["now"].as_str().unwrap()) - micros(c["last_action_job_at"].as_str().unwrap());
        assert_eq!(within(elapsed, &eff), c["within"] == true, "{c}");
    }
}
