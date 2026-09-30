//! Writes rust/tests/fixtures/rust_vectors.json: values this crate produced, for the Rails test
//! test/contracts/rust_vectors_test.rb to read back. Run from rust/: cargo run --bin record_rust_vectors
use deltabadger::crypto::{hash_password, Cipher, EncryptionKeys};
use serde_json::{json, Map, Value};

fn main() {
    let ruby: Value = serde_json::from_str(include_str!("../../tests/fixtures/ruby_vectors.json")).unwrap();
    let secret = ruby["secret_key_base"].as_str().unwrap();
    let cipher = Cipher::new(&EncryptionKeys::resolve(&|_| None, secret).unwrap());
    let mut ciphertexts = Map::new();
    for (name, c) in ruby["ciphertexts"].as_object().unwrap() {
        let plain = c["plain"].as_str().unwrap();
        ciphertexts.insert(name.clone(), json!({ "plain": plain, "cipher": cipher.encrypt(plain) }));
    }
    let bcrypt: Vec<Value> = ruby["bcrypt"].as_array().unwrap().iter()
        .map(|b| json!({ "password": b["password"], "hash": hash_password(b["password"].as_str().unwrap()) }))
        .collect();
    let out = json!({ "secret_key_base": secret, "ciphertexts": ciphertexts, "bcrypt": bcrypt });
    std::fs::write("tests/fixtures/rust_vectors.json", serde_json::to_string_pretty(&out).unwrap() + "\n").unwrap();
    println!("wrote tests/fixtures/rust_vectors.json");
}
