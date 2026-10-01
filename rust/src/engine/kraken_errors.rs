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
