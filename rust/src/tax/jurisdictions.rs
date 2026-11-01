//! Tax::Jurisdictions, as far as this build computes it.

/// D-TAX-NOT-PORTED-GATE: the jurisdictions whose crypto report this build computes. Until a jurisdiction's slice lands,
/// a create for it (web, REST, MCP) is refused up front as not ported, and nothing is ever queued for a report this
/// build cannot compute; the broker scope likewise until A12-7. Each later slice adds its countries; the gate goes with A12-7.
pub const PORTED: [&str; 1] = ["US"];

pub fn ported(country: &str) -> bool { PORTED.contains(&country) }
