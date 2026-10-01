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
        let cp = checkpoints(micros(c["anchor"].as_str().unwrap()), micros(c["now"].as_str().unwrap()), eff);
        assert_eq!(cp.next_us, micros(c["next"].as_str().unwrap()), "next {c}");
        assert_eq!(cp.last_us, micros(c["last"].as_str().unwrap()), "last {c}");
        assert_eq!(interval_count(cp.last_us, micros(c["anchor"].as_str().unwrap()), eff), c["count"].as_i64().unwrap(), "count {c}");
    }
}

#[test]
fn a_tick_exactly_on_the_grid_does_not_count_that_interval_yet() {
    // Rails: at now == checkpoint, ceil gives the same point, so `last` is the PREVIOUS one. The engine's
    // loop must therefore run strictly after a checkpoint (Task 11).
    let eff = effective(Interval::Day, 10.0, None);
    let anchor = micros("2026-09-01T00:00:00Z");
    let on = checkpoints(anchor, anchor + 86_400_000_000, eff);
    let after = checkpoints(anchor, anchor + 86_400_000_001, eff);
    assert_eq!(on.last_us, anchor);
    assert_eq!(after.last_us, anchor + 86_400_000_000);
}
