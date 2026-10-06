//! Rails-free input/encoding contracts, backed by measured BotApi number vectors.
use deltabadger::web::bot::mcp_input;
use serde_json::{json,Value};
#[test]
fn strict_numbers_match_rails_vectors() {
    let vectors:Vec<Value>=serde_json::from_str(include_str!("fixtures/m4_numbers.json")).unwrap();
    assert!(vectors.len()>20);
    for row in vectors {
        let value=match row["wire"].as_str() {Some(wire)=>serde_json::from_str::<Value>(wire).unwrap(),None=>row["value"].clone()};
        let got=mcp_input::number(&value).map(|n|json!(n.to_s_f())).unwrap_or(Value::Null);
        assert_eq!(got,row["decimal"],"{}",value);
    }
}
#[test]
fn rails_json_keeps_order_escapes_and_integer_digits() {
    assert_eq!(mcp_input::encode(&json!({"x":"<>&\u{2028}\u{2029}","a":0.00005,"b":18446744073709551615u64})),
        "{\"x\":\"\\u003c\\u003e\\u0026\\u2028\\u2029\",\"a\":5e-05,\"b\":18446744073709551615}");
}
#[test]
fn create_tools_remain_absent() {
    for name in ["create_bot","create_index_bot","create_signal_bot"] {
        assert!(!deltabadger::web::mcp::tools::NAMES.contains(&name));
        assert!(!deltabadger::web::mcp::protocol::metadata()["tools"].as_array().unwrap().iter().any(|t|t["name"]==name));
    }
}

#[test]
fn ruby_strip_keeps_unicode_and_removes_only_ascii_and_nul() {
    assert_eq!(mcp_input::strip("\0\t\n\u{000b}\u{000c}\r BTC \0\t"),"BTC");
    for space in ["\u{00a0}","\u{2003}","\u{202f}"] {
        let text=format!("{space}70{space}");
        assert_eq!(mcp_input::strip(&text),text);
        assert!(mcp_input::number(&json!(text)).is_none());
    }
    assert_eq!(mcp_input::number(&json!("\0\t70\t\0")).unwrap().to_s_f(),"70.0");
}
