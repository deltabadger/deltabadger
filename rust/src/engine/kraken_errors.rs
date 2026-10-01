//! Exchanges::Kraken::ERRORS and Exchange#failure_kind: substring match, first kind wins, in
//! Exchange::FAILURE_KINDS order.
pub const KRAKEN_ERRORS: [(&str, &[&str]); 6] = [
    ("insufficient_funds", &["EAPI:Insufficient funds", "EOrder:Insufficient funds"]),
    ("invalid_key", &["EAPI:Invalid key", "EAPI:Invalid signature"]),
    ("permission_denied", &["EGeneral:Permission denied"]),
    ("restricted", &["EAccount:Invalid permissions"]),
    ("throttle", &["EAPI:Rate limit exceeded"]),
    ("transient", &["EGeneral:Internal error", "EAPI:Invalid nonce", "EService:Unavailable", "EService:Busy", "EService:Deadline elapsed"]),
];

pub fn failure_kind(messages: &[String]) -> Option<&'static str> {
    KRAKEN_ERRORS.iter().find(|(_, pats)| pats.iter().any(|p| messages.iter().any(|m| m.contains(p)))).map(|(k, _)| *k)
}
fn matches(kind: &str, messages: &[String]) -> bool {
    KRAKEN_ERRORS.iter().filter(|(k, _)| *k == kind).any(|(_, pats)| pats.iter().any(|p| messages.iter().any(|m| m.contains(p))))
}
pub fn is_throttle(messages: &[String]) -> bool { matches("throttle", messages) }
pub fn is_transient(messages: &[String]) -> bool { matches("transient", messages) }
/// An AddOrder answered with one of these may or may not have been placed (Kraken failed while handling it), so the
/// answer settles nothing: the intent stays for cl_ord_id recovery. The transient kind minus "EAPI:Invalid nonce",
/// which Kraken returns before it looks at the order. A sanctioned divergence: Rails writes a failed row.
const ADD_OUTCOME_UNKNOWN: [&str; 4] = ["EGeneral:Internal error", "EService:Unavailable", "EService:Busy", "EService:Deadline elapsed"];
pub fn add_outcome_unknown(messages: &[String]) -> bool { ADD_OUTCOME_UNKNOWN.iter().any(|p| messages.iter().any(|m| m.contains(p))) }
