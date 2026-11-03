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

/// `t + n.days` on a TimeWithZone (ActiveSupport 8.1, `advance(days:)`): the wall time moves by whole days, then a wall time
/// that occurs twice takes the occurrence with the source's offset (else its DST flag), and one that does not occur moves forward an hour at a
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
            // The source's own offset when one occurrence has it (Moscow 2014, a change no DST flag marks), else its DST flag.
            LocalResult::Ambiguous(first, second) => {
                use chrono::Offset;
                let same = |at: &DateTime<Tz>| at.offset().fix() == source.offset().fix();
                let second_wins = !same(&first) && (same(&second) || (dst(second.offset()) == summer && dst(first.offset()) != summer));
                return Some(if second_wins { second } else { first }.with_timezone(&Utc));
            }
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

/// A time that names its own offset in a form `parse_form_time` does not read (`+0200`, or no seconds), as Time.zone.parse
/// reads it, in UTC ISO 8601; `None` for any other text (BotApi's create, A10).
pub fn parse_explicit_offset(text: &str) -> Option<String> {
    let t = text.strip_suffix('Z').map_or_else(|| text.to_string(), |rest| format!("{rest}+00:00"));
    ["%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%dT%H:%M:%S%z", "%Y-%m-%dT%H:%M%z", "%Y-%m-%d %H:%M:%S%z", "%Y-%m-%d %H:%M%z"].iter()
        .find_map(|f| chrono::DateTime::parse_from_str(&t, f).ok())
        .map(|at| at.with_timezone(&chrono::Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// A day's bound in `zone` as Date#beginning_of_day / #end_of_day resolve it (TimeZone#period_for_local(t, dst = true) {
/// periods.last }): a repeated wall time is the occurrence in DST when exactly one is, else the later one (Magadan
/// 2014-10-26 00:00 is +10:00); a missing one moves forward an hour at a time (Santiago 2026-09-06 00:00 is 01:00).
pub fn wall(zone: Tz, naive: chrono::NaiveDateTime) -> Option<DateTime<Utc>> {
    use chrono::{LocalResult, TimeZone};
    use chrono_tz::OffsetComponents;
    let dst = |t: &DateTime<Tz>| !t.offset().dst_offset().is_zero();
    let mut at = naive;
    for _ in 0..48 {
        match zone.from_local_datetime(&at) {
            LocalResult::Single(t) => return Some(t.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => return Some(if dst(&first) && !dst(&second) { first } else { second }.with_timezone(&Utc)),
            LocalResult::None => at = at.checked_add_signed(chrono::Duration::hours(1))?,
        }
    }
    None
}

#[test]
fn explicit_offsets_in_every_form() {
    assert_eq!(parse_explicit_offset("2026-11-05T09:30:00+0200").as_deref(), Some("2026-11-05T07:30:00Z"));
    assert_eq!(parse_explicit_offset("2026-11-05T09:30+02:00").as_deref(), Some("2026-11-05T07:30:00Z"));
    assert_eq!(parse_explicit_offset("2026-11-05T09:30Z").as_deref(), Some("2026-11-05T09:30:00Z"));
    assert_eq!(parse_explicit_offset("2026-11-05T09:30"), None, "no offset: the account's zone reads it");
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
            ("Samoa", "2011-12-16T12:30:00-10:00", 14, "2011-12-31T00:30:00+14:00"),
            ("Moscow", "2014-11-09T01:30:00+03:00", -14, "2014-10-26T01:30:00+03:00"), ("Moscow", "2014-10-12T01:30:00+04:00", 14, "2014-10-26T01:30:00+04:00"), ("UTC", "2026-01-01T00:00:00+00:00", 14, "2026-01-15T00:00:00+00:00"),
        ] {
            let at = DateTime::parse_from_rfc3339(from).unwrap().with_timezone(&Utc);
            let want = DateTime::parse_from_rfc3339(to).unwrap().with_timezone(&Utc);
            assert_eq!(advance_days(at, strict(zone).unwrap(), days), Some(want), "{zone} {from} {days}");
        }
    }
}

#[cfg(test)]
mod wall_tests {
    /// Recorded with activesupport 8.1.4 (`Time.zone = zone; Date.parse(day).beginning_of_day` and `.end_of_day`).
    #[test]
    fn day_bounds_resolve_as_activesupport() {
        for (zone, day, from, to) in [("Magadan", "2014-10-26", "2014-10-25T14:00:00Z", "2014-10-26T13:59:59.999999Z"),
                                      ("Santiago", "2026-09-06", "2026-09-06T04:00:00Z", "2026-09-07T02:59:59.999999Z"),
                                      ("Santiago", "2026-04-05", "2026-04-05T04:00:00Z", "2026-04-06T03:59:59.999999Z"),
                                      ("Warsaw", "2026-10-25", "2026-10-24T22:00:00Z", "2026-10-25T22:59:59.999999Z")] {
            let zone = super::strict(zone).unwrap();
            let day = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
            let at = |t: &str| chrono::DateTime::parse_from_rfc3339(t).unwrap().with_timezone(&chrono::Utc);
            assert_eq!(super::wall(zone, day.and_hms_micro_opt(0, 0, 0, 0).unwrap()), Some(at(from)), "{zone} {day}");
            assert_eq!(super::wall(zone, day.and_hms_micro_opt(23, 59, 59, 999_999).unwrap()), Some(at(to)), "{zone} {day}");
        }
    }
}
