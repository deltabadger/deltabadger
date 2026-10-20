//! Honeymaker::Clients::Coinbase#auth_headers: a fresh JWT per request, `Authorization: Bearer <jwt>`.
//! - header `{"kid": key name, "nonce": 16 random bytes in hex, "alg": "ES256" | "EdDSA"}` (ruby-jwt merges `alg` last);
//! - claims `{"sub": key name, "iss": "coinbase-cloud", "nbf": now, "exp": now + 120, "uri": "METHOD api.coinbase.com<path>"}`,
//!   the path without its query (honeymaker signs before it sets the params);
//! - ES256 when the secret starts with `-----BEGIN EC PRIVATE KEY-----` (a SEC1 P-256 PEM; r‖s, 64 bytes), else EdDSA over
//!   the first 32 bytes of the base64-decoded secret (Base64.decode64: characters outside the alphabet are skipped).
//!
//! A secret neither reads as (an unreadable PEM, fewer than 32 decoded bytes) signs nothing: honeymaker then sends the
//! request with no Authorization header at all, which `jwt` answers as None.
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use aws_lc_rs::signature::{EcdsaKeyPair, Ed25519KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use base64::Engine;
use serde_json::json;

pub const HOST: &str = "api.coinbase.com";
const EC_PEM: &str = "-----BEGIN EC PRIVATE KEY-----";
const EC_PEM_END: &str = "-----END EC PRIVATE KEY-----";

enum Key { Es256(EcdsaKeyPair), EdDsa(Ed25519KeyPair) }

fn key(secret: &str) -> Option<Key> {
    let b64 = base64::engine::general_purpose::STANDARD;
    if let Some(rest) = secret.strip_prefix(EC_PEM) {
        let body: String = rest.split(EC_PEM_END).next()?.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let der = b64.decode(body).ok()?;
        return EcdsaKeyPair::from_private_key_der(&ECDSA_P256_SHA256_FIXED_SIGNING, &der).ok().map(Key::Es256);
    }
    let lenient = base64::engine::GeneralPurpose::new(&base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new().with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent).with_decode_allow_trailing_bits(true));
    // ponytail: decode64's skip-the-rest is approximated by dropping every non-alphabet character (padding included);
    // a secret with data after an inner `=` would differ, and no Coinbase secret has one.
    let kept: String = secret.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/').collect();
    let bytes = lenient.decode(kept).ok()?;
    Ed25519KeyPair::from_seed_unchecked(bytes.get(..32)?).ok().map(Key::EdDsa)
}

/// The JWT for one request at `now` (unix seconds) with `nonce` (the header's hex nonce), or None where honeymaker signs
/// nothing. ES256 signatures are randomized (as OpenSSL's are); EdDSA ones are deterministic.
pub fn jwt_at(key_name: &str, secret: &str, method: &str, path: &str, now: i64, nonce: &str) -> Option<String> {
    let key = key(secret)?;
    let alg = match key { Key::Es256(_) => "ES256", Key::EdDsa(_) => "EdDSA" };
    let url = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = json!({ "kid": key_name, "nonce": nonce, "alg": alg });
    let claims = json!({ "sub": key_name, "iss": "coinbase-cloud", "nbf": now, "exp": now + 120,
                         "uri": format!("{} {HOST}{path}", method.to_ascii_uppercase()) });
    let input = format!("{}.{}", url.encode(header.to_string()), url.encode(claims.to_string()));
    let signature = match key {
        Key::Es256(k) => k.sign(&SystemRandom::new(), input.as_bytes()).ok()?.as_ref().to_vec(),
        Key::EdDsa(k) => k.sign(input.as_bytes()).as_ref().to_vec(),
    };
    Some(format!("{input}.{}", url.encode(signature)))
}

/// `jwt_at` now, with a fresh random nonce (SecureRandom.hex: 16 bytes).
pub fn jwt(key_name: &str, secret: &str, method: &str, path: &str) -> Option<String> {
    let mut nonce = [0u8; 16];
    SystemRandom::new().fill(&mut nonce).ok()?;
    jwt_at(key_name, secret, method, path, chrono::Utc::now().timestamp(), &hex::encode(nonce))
}
