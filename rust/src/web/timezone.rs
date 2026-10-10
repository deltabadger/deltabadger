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

/// `time.in_time_zone(user.time_zone)`, resolved as everything else reads the owner's zone (`strict`: a Rails name or an
/// IANA id). An unknown name reads as UTC, the column's default, for display only: parsing and starts refuse it.
pub fn local(time: DateTime<Utc>, name: &str) -> DateTime<Tz> {
    time.with_timezone(&strict(name).unwrap_or(Tz::UTC))
}

pub fn names() -> impl Iterator<Item = &'static String> {
    table().keys()
}
