use deltabadger::crypto::{verify_password, Cipher};
use rusqlite::{types::ValueRef, Connection};
use serde_json::{json, Value};
use std::path::Path;
pub fn rows(dir: &Path, cipher: &Cipher) -> Value {
    let c = Connection::open(dir.join("production.sqlite3")).unwrap();
    let mut out = serde_json::Map::new();
    for table in [
        "users",
        "api_keys",
        "bots",
        "account_transactions",
        "connected_clients",
        "oauth_applications",
        "oauth_access_tokens",
        "oauth_access_grants",
        "wash_sale_locks",
        "app_configs",
    ] {
        let mut s = c
            .prepare(&format!("SELECT * FROM {table} ORDER BY id"))
            .unwrap();
        let names = s
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut q = s.query([]).unwrap();
        let mut rows = vec![];
        while let Some(r) = q.next().unwrap() {
            let mut row = serde_json::Map::new();
            for (i, name) in names.iter().enumerate() {
                let mut value = match r.get_ref(i).unwrap() {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => json!(n),
                    ValueRef::Real(n) => json!(n),
                    ValueRef::Text(v) => json!(std::str::from_utf8(v).unwrap()),
                    ValueRef::Blob(_) => panic!("unexpected blob"),
                };
                let secret = table == "users" && name == "otp_secret_key"
                    || table == "api_keys"
                        && [
                            "key",
                            "secret",
                            "passphrase",
                            "access_token",
                            "rsa_signature_key",
                            "rsa_encryption_key",
                            "dh_param",
                        ]
                        .contains(&name.as_str())
                    || table == "app_configs" && name == "value";
                if secret && !value.is_null() {
                    value = json!(cipher.decrypt(value.as_str().unwrap()).unwrap());
                }
                if [
                    "mcp_settings",
                    "rest_settings",
                    "tracker_settings",
                    "settings",
                    "transient_data",
                    "mcp_tools",
                    "rest_tools",
                ]
                .contains(&name.as_str())
                    && value.is_string()
                {
                    value = serde_json::from_str(value.as_str().unwrap()).unwrap();
                }
                if table == "users" && name == "encrypted_password" {
                    let password = ["Correct-horse-9", "Another-horse-7", "Correcthorse9Ż", "Correcthorse١!", "Correct\nhorse-9", "   "]
                        .into_iter()
                        .find(|p| verify_password(p, value.as_str().unwrap()))
                        .expect("recognized password");
                    value = json!({"verifies":password});
                }
                if table == "users" && name == "confirmation_token" && !value.is_null() {
                    let text = value.as_str().unwrap();
                    assert_eq!(text.len(), 20);
                    assert!(text
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
                    value = json!("<valid confirmation token>");
                }
                row.insert(name.clone(), value);
            }
            rows.push(Value::Object(row));
        }
        out.insert(table.into(), Value::Array(rows));
    }
    Value::Object(out)
}
