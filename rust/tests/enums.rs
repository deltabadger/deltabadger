mod common;
use deltabadger::enums::*;

fn assert_matches(fixture: &str, all: &[(&str, i64)]) {
    let rails = common::vectors()["enums"][fixture].as_object().unwrap().clone();
    let ours: serde_json::Map<String, serde_json::Value> = all.iter().map(|(k, v)| (k.to_string(), (*v).into())).collect();
    assert_eq!(ours, rails, "{fixture}: Rust constants differ from the Rails enum");
}

macro_rules! pairs { ($t:ty) => { <$t>::ALL.iter().map(|(k, v)| (*k, *v as i64)).collect::<Vec<_>>() } }

#[test]
fn every_enum_matches_rails() {
    assert_matches("bot_status", &pairs!(BotStatus));
    assert_matches("rule_status", &pairs!(RuleStatus));
    assert_matches("transaction_status", &pairs!(TxStatus));
    assert_matches("transaction_side", &pairs!(TxSide));
    assert_matches("transaction_order_type", &pairs!(TxOrderType));
    assert_matches("transaction_external_status", &pairs!(TxExternalStatus));
    assert_matches("api_key_status", &pairs!(ApiKeyStatus));
    assert_matches("api_key_key_type", &pairs!(ApiKeyType));
    assert_matches("user_otp_module", &pairs!(OtpModule));
}

#[test]
fn from_i64_round_trips_and_rejects_unknowns() {
    assert_eq!(BotStatus::from_i64(6), Some(BotStatus::Waiting));
    assert_eq!(BotStatus::from_i64(99), None);
}
