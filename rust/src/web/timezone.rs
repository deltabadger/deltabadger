//! `users.time_zone` holds an ActiveSupport zone name ("Warsaw", "Eastern Time (US & Canada)"), not
//! an IANA id. time_zones.json is ActiveSupport::TimeZone::MAPPING, written by
//! script/rust/record_vectors.rb and pinned by test/contracts/rust_time_zones_test.rb.
use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;
use std::collections::HashMap;
use std::sync::OnceLock;

fn table() -> &'static HashMap<String, Tz> {
    static TABLE: OnceLock<HashMap<String, Tz>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let names: HashMap<String, String> = serde_json::from_str(include_str!("time_zones.json")).unwrap_or_default();
        names.into_iter().filter_map(|(name, iana)| iana.parse::<Tz>().ok().map(|zone| (name, zone))).collect()
    })
}

/// The zone for a stored name. Rails validates the column against this same table, so `None` means
/// a row Rails would refuse to save.
pub fn zone(name: &str) -> Option<Tz> {
    table().get(name).copied()
}

/// ActiveSupport::TimeZone[]: a Rails zone name or an IANA identifier. `None` for any other name, never UTC.
pub fn strict(name: &str) -> Option<Tz> {
    zone(name).or_else(|| name.parse::<Tz>().ok())
}

pub const REPEATED_TIME: &str = "the starting time occurs twice that day in the owner's time zone (a daylight-saving change), and which occurrence Rails picks is not proven";
pub const MISSING_TIME: &str = "the starting time does not exist that day in the owner's time zone, across a change this build does not match";

/// TimeZone#local for a wall time: one instant as it is; a wall time in a one-hour spring gap moves forward an hour, Rails'
/// answer as the start-time vectors pin it. A repeated wall time, or one in any other gap, is refused, never guessed.
pub fn resolve_local(zone: Tz, naive: NaiveDateTime) -> Result<(NaiveDateTime, DateTime<Tz>), &'static str> {
    use chrono::{Duration, LocalResult, TimeZone};
    match zone.from_local_datetime(&naive) {
        LocalResult::Single(at) => Ok((naive, at)),
        LocalResult::Ambiguous(..) => Err(REPEATED_TIME),
        LocalResult::None => {
            // A one-hour gap: the wall times an hour either side are one real hour apart.
            let hour = Duration::hours(1);
            let single = |wall: Option<NaiveDateTime>| wall.and_then(|wall| zone.from_local_datetime(&wall).single().map(|at| (wall, at)));
            match (single(naive.checked_sub_signed(hour)), single(naive.checked_add_signed(hour))) {
                (Some((_, before)), Some((wall, at))) if at.signed_duration_since(before) == hour => Ok((wall, at)),
                _ => Err(MISSING_TIME),
            }
        }
    }
}

/// `t + n.days` on a TimeWithZone (ActiveSupport 8.1, `advance(days:)`): the wall time moves by whole days, then a wall time
/// that occurs twice takes the occurrence with the source's DST flag, and one that does not occur moves forward an hour at a
/// time until it does. `None` past chrono's range (or a zone that skipped two days, which none does).
pub fn advance_days(t: DateTime<Utc>, zone: Tz, days: i64) -> Option<DateTime<Utc>> {
    use chrono::{Duration, LocalResult, TimeZone};
    use chrono_tz::OffsetComponents;
    let dst = |offset: &chrono_tz::TzOffset| !offset.dst_offset().is_zero();
    let source = t.with_timezone(&zone);
    let summer = dst(source.offset());
    let mut wall = source.naive_local().checked_add_signed(Duration::try_days(days)?)?;
    for _ in 0..48 {
        match zone.from_local_datetime(&wall) {
            LocalResult::Single(at) => return Some(at.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => return Some(if dst(second.offset()) == summer && dst(first.offset()) != summer { second } else { first }.with_timezone(&Utc)),
            LocalResult::None => wall = wall.checked_add_signed(Duration::hours(1))?,
        }
    }
    None
}

/// `time.in_time_zone(user.time_zone)`, resolved as everything else reads the owner's zone (`strict`: a Rails name or an
/// IANA id). An unknown name reads as UTC, the column's default, for display only: parsing and starts refuse it.
pub fn local(time: DateTime<Utc>, name: &str) -> DateTime<Tz> {
    time.with_timezone(&strict(name).unwrap_or(Tz::UTC))
}

pub fn names() -> impl Iterator<Item = &'static String> {
    table().keys()
}

#[cfg(test)]
mod advance_tests {
    use super::*;
    /// Recorded with activesupport 8.1.4 (`ActiveSupport::TimeZone[zone].parse(t) + days.days`), PLAN-A11 r3/r4.
    #[test]
    fn advance_days_resolves_as_activesupport() {
        for (zone, from, days, to) in [
            ("Warsaw", "2026-10-11T02:30:00+02:00", 14, "2026-10-25T02:30:00+02:00"), ("Warsaw", "2026-11-08T02:30:00+01:00", -14, "2026-10-25T02:30:00+01:00"),
            ("Warsaw", "2026-03-15T02:30:00+01:00", 14, "2026-03-29T03:30:00+02:00"), ("Warsaw", "2026-04-12T02:30:00+02:00", -14, "2026-03-29T03:30:00+02:00"),
            ("Warsaw", "2026-10-25T02:30:00+02:00", 14, "2026-11-08T02:30:00+01:00"), ("Warsaw", "2026-10-25T02:30:00+01:00", -14, "2026-10-11T02:30:00+02:00"),
            ("Warsaw", "2026-10-20T12:00:00+02:00", 14, "2026-11-03T12:00:00+01:00"),
            ("Australia/Lord_Howe", "2026-09-20T02:15:00+10:30", 14, "2026-10-04T03:15:00+11:00"), ("Australia/Lord_Howe", "2026-04-19T01:45:00+10:30", -14, "2026-04-05T01:45:00+10:30"),
            ("Samoa", "2011-12-16T12:30:00-10:00", 14, "2011-12-31T00:30:00+14:00"), ("UTC", "2026-01-01T00:00:00+00:00", 14, "2026-01-15T00:00:00+00:00"),
        ] {
            let at = DateTime::parse_from_rfc3339(from).unwrap().with_timezone(&Utc);
            let want = DateTime::parse_from_rfc3339(to).unwrap().with_timezone(&Utc);
            assert_eq!(advance_days(at, strict(zone).unwrap(), days), Some(want), "{zone} {from} {days}");
        }
    }
}
