//! Logical-clock venue boundary for the legacy upgrade regression.
//! Uses the existing instantaneous ScriptedTransport, so an injected engine date
//! is never compared with the HTTP adapter's real absolute send deadline.
use chrono::{DateTime,Utc};
use deltabadger::venue::{alpaca::{AlpacaVenue,Urls},http::ScriptedTransport,VenueFactory};
pub fn at()->DateTime<Utc>{"2026-09-10T12:00:30.123456Z".parse().unwrap()}
pub struct Factory(pub ScriptedTransport);
impl VenueFactory for Factory{
    type V=AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self,kind:&str,credentials:Option<deltabadger::crypto::Credentials>)->Self::V{
        assert_eq!(kind,"Exchanges::Alpaca");assert!(credentials.is_some());
        AlpacaVenue::new(self.0.clone(),Urls::for_passphrase(Some("paper")))
    }
}
pub fn failed_order()->ScriptedTransport{
    ScriptedTransport::from_script(&serde_json::json!({
        "GET /v1beta3/crypto/us/latest/quotes":[{"status":200,"body":{"quotes":{"BTC/USD":{"ap":"50000"}}}}],
        "GET /v2/account":[{"status":200,"body":{"cash":"10000"}}],
        "GET /v2/positions":[{"status":200,"body":[]}],
        "POST /v2/orders":[{"status":422,"body":{"message":"unclassified test failure"}}]
    }))
}
