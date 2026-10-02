//! `users.time_zone` holds an ActiveSupport zone name ("Warsaw", "Eastern Time (US & Canada)"), not
//! an IANA id. time_zones.json is ActiveSupport::TimeZone::MAPPING, written by
//! script/rust/record_vectors.rb and pinned by test/contracts/rust_time_zones_test.rb.
use chrono::{DateTime, Utc};
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

/// `time.in_time_zone(user.time_zone)`; an unknown name reads as UTC, the column's default.
pub fn local(time: DateTime<Utc>, name: &str) -> DateTime<Tz> {
    time.with_timezone(&zone(name).unwrap_or(Tz::UTC))
}

pub fn names() -> impl Iterator<Item = &'static String> {
    table().keys()
}
