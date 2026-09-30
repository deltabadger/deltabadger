mod common;
use deltabadger::crypto::*;
use std::collections::HashMap;

fn fixture_keys() -> EncryptionKeys {
    let v = common::vectors();
    EncryptionKeys::resolve(&|_| None, v["secret_key_base"].as_str().unwrap()).unwrap()
}

#[test]
fn keys_are_derived_from_secret_key_base_like_the_initializer() {
    let v = common::vectors();
    let keys = fixture_keys();
    assert_eq!(keys.primary_key, v["primary_key"].as_str().unwrap());
    assert_eq!(keys.key_derivation_salt, v["key_derivation_salt"].as_str().unwrap());
}

#[test]
fn keys_from_env_are_used_verbatim_and_half_a_pair_is_refused() {
    let both: HashMap<&str, &str> = [("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", "p"), ("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", "s")].into();
    let keys = EncryptionKeys::resolve(&|k| both.get(k).map(|v| v.to_string()), "ignored").unwrap();
    assert_eq!((keys.primary_key.as_str(), keys.key_derivation_salt.as_str()), ("p", "s"));

    let half: HashMap<&str, &str> = [("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", "p")].into();
    assert!(matches!(EncryptionKeys::resolve(&|k| half.get(k).map(|v| v.to_string()), "x"),
        Err(KeyConfigError::Partial { missing: "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT" })));

    let marker: HashMap<&str, &str> = [("ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL", "1")].into();
    assert!(matches!(EncryptionKeys::resolve(&|k| marker.get(k).map(|v| v.to_string()), "x"),
        Err(KeyConfigError::ExternalMarkerWithoutKeys)));

    let blank: HashMap<&str, &str> = [("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", "  "), ("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", "")].into();
    let derived = EncryptionKeys::resolve(&|k| blank.get(k).map(|v| v.to_string()), "sk").unwrap();
    assert_eq!(derived, EncryptionKeys::resolve(&|_| None, "sk").unwrap(), "blank counts as absent, like .presence");
}

#[test]
fn decrypts_every_rails_ciphertext() {
    let cipher = Cipher::new(&fixture_keys());
    for (name, c) in common::vectors()["ciphertexts"].as_object().unwrap() {
        assert_eq!(cipher.decrypt(c["cipher"].as_str().unwrap()).unwrap(), c["plain"].as_str().unwrap(), "{name}");
    }
}

#[test]
fn round_trips_its_own_output_and_compresses_long_values() {
    let cipher = Cipher::new(&fixture_keys());
    let long = "k".repeat(500);
    let stored = cipher.encrypt(&long);
    assert!(stored.contains("\"c\":true"));
    assert_eq!(cipher.decrypt(&stored).unwrap(), long);
    assert!(!cipher.encrypt("short").contains("\"c\""));
}

#[test]
fn malformed_envelopes_are_errors_not_panics() {
    let cipher = Cipher::new(&fixture_keys());
    for bad in [r#"{"p":"AAAA","h":{"iv":"AAAA","at":"AAAAAAAAAAAAAAAAAAAAAA=="}}"#, // 3-byte IV
                r#"{"p":"AAAA","h":{"iv":"AAAAAAAAAAAAAAAA","at":"AAAA"}}"#,          // 3-byte tag
                r#"{"p":"!!","h":{"iv":"AAAAAAAAAAAAAAAA","at":"AAAAAAAAAAAAAAAAAAAAAA=="}}"#,
                r#"{"p":"AAAA","h":{}}"#] {
        assert!(matches!(cipher.decrypt(bad), Err(DecryptError::Malformed(_))), "{bad}");
    }
}

#[test]
fn a_value_that_is_not_an_envelope_passes_through() {
    assert_eq!(Cipher::new(&fixture_keys()).decrypt("legacy-plaintext").unwrap(), "legacy-plaintext");
}

#[test]
fn an_envelope_under_another_key_is_an_error_never_the_ciphertext() {
    let other = Cipher::new(&EncryptionKeys::resolve(&|_| None, "a-different-secret").unwrap());
    let stored = common::vectors()["ciphertexts"]["short"]["cipher"].as_str().unwrap().to_string();
    assert!(matches!(other.decrypt(&stored), Err(DecryptError::Unreadable)));
}

#[test]
fn verifies_devise_hashes_including_past_72_bytes() {
    for b in common::vectors()["bcrypt"].as_array().unwrap() {
        let (pw, hash) = (b["password"].as_str().unwrap(), b["hash"].as_str().unwrap());
        assert!(verify_password(pw, hash), "{pw:.20}");
        assert!(!verify_password("wrong", hash));
    }
}

#[test]
fn hashes_like_devise() {
    let h = hash_password("correct horse ☃");
    assert!(h.starts_with("$2a$11$"), "{h}");
    assert!(verify_password("correct horse ☃", &h));
    assert!(!verify_password("", "not-a-bcrypt-hash"), "a malformed hash never verifies");
}

#[test]
fn totp_matches_rotp() {
    let t = &common::vectors()["totp"];
    let seed = t["seed"].as_str().unwrap();
    for pair in t["codes"].as_array().unwrap() {
        assert_eq!(totp_at(seed, pair[0].as_u64().unwrap()).as_deref(), pair[1].as_str(), "{pair}");
    }
    assert_eq!(totp_at("not base32!", 0), None);
}
