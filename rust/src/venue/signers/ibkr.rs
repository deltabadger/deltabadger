//! Clients::Ibkr::Oauth (app/models/clients/ibkr/oauth.rb): IBKR's first-party OAuth 1.0a. An RSA-SHA256-signed request
//! mints a Diffie-Hellman "live session token" (LST), and every later request carries an HMAC-SHA256 signature keyed by
//! it. Pure functions of their inputs (keys, nonce, timestamp, DH random), pinned byte for byte against Rails by
//! `script/rust/record_venue_signing.rb ibkr` and against the Python-computed vectors of oauth_test.rb. The session
//! (LST cache, brokerage init, self-heal) is slice I1's.
//!
//! Also here, for the connect wizard (I3): RSA-2048 key generation, the PEM writer, and the RFC 7919 ffdhe2048 group new
//! registrations upload instead of a generated one (D-IBKR-DHGROUP; Rails generates a 2048-bit safe prime in a job).
use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use aws_lc_rs::rsa::{KeyPair, KeySize, Pkcs1PrivateDecryptingKey, PrivateDecryptingKey};
use aws_lc_rs::signature::{KeyPair as _, RSA_PKCS1_SHA256};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use num_bigint::BigUint;
use std::collections::BTreeMap;
use subtle::ConstantTimeEq;

pub const DEFAULT_REALM: &str = "limited_poa";

/// RFC 7919 §A.1 ffdhe2048's prime (generator 2), as `openssl genpkey -genparam -pkeyopt group:ffdhe2048` writes it.
pub const FFDHE2048_P: &str = "ffffffffffffffffadf85458a2bb4a9aafdc5620273d3cf1d8b9c583ce2d3695a9e13641146433fbcc939dce249b3ef97d2fe363630c75d8f681b202aec4617ad3df1ed5d5fd65612433f51f5f066ed0856365553ded1af3b557135e7f57c935984f0c70e0e68b77e2a689daf3efe8721df158a136ade73530acca4f483a797abc0ab182b324fb61d108a94bb2c8e3fbb96adab760d7f4681d4f42a3de394df4ae56ede76372bb190b07a7c8ee0a6d709e02fce1cdf7e2ecc03404cd28342f619172fe9ce98583ff8e4f1232eef28183c3fe3b1b4c6fad733bb5fcbc2ec22005c58ef1837d1683b2c6f34a26c1b2effa886b423861285c97ffffffffffffffff";

/// ffdhe2048 as the `DH PARAMETERS` PEM a new registration downloads and uploads to IBKR.
pub fn ffdhe2048_pem() -> String {
    dh_params_pem(&BigUint::parse_bytes(FFDHE2048_P.as_bytes(), 16).expect("constant"), &BigUint::from(2u8))
}

// ---- the pure helpers (Oauth's class methods)

/// Oauth.escape, Python's `quote_plus`: every byte outside `A-Za-z0-9-._~` and space as upper-case %XX, space as `+`.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Oauth.to_byte_array: big-endian bytes, with a leading 0x00 when the bit length is a multiple of 8 (0 is `[0]`).
pub fn to_byte_array(n: &BigUint) -> Vec<u8> {
    let mut bytes = n.to_bytes_be();
    if n.bits().is_multiple_of(8) && n.bits() > 0 { bytes.insert(0, 0) }
    bytes
}

/// Oauth.base_string: `prepend` + METHOD & escape(url) & escape(params sorted by key as "k=v" joined by "&"). Values
/// go in raw, as Rails puts them.
pub fn base_string(method: &str, url: &str, params: &BTreeMap<String, String>, prepend: Option<&str>) -> String {
    let sorted = params.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    format!("{}{method}&{}&{}", prepend.unwrap_or(""), escape(url), escape(&sorted))
}

/// Oauth.hmac_sha256_signature: escape(base64(HMAC-SHA256(key = base64-decoded LST, base string))). None for an LST
/// that is not base64 (one this module minted always is).
pub fn hmac_sha256_signature(base: &str, live_session_token: &str) -> Option<String> {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&B64.decode(live_session_token).ok()?).ok()?;
    mac.update(base.as_bytes());
    Some(escape(&B64.encode(mac.finalize().into_bytes())))
}

fn hex_int(s: &str) -> Option<BigUint> { BigUint::parse_bytes(s.as_bytes(), 16) }

/// Oauth.calculate_live_session_token: shared = dh_response^dh_random mod dh_prime, then
/// base64(HMAC-SHA1(key = to_byte_array(shared), msg = the prepend's bytes)). All inputs hex. None on unreadable hex.
pub fn live_session_token(dh_prime: &str, dh_random: &str, dh_response: &str, prepend: &str) -> Option<String> {
    let p = hex_int(dh_prime).filter(|p| p.bits() > 0)?;
    let shared = hex_int(dh_response)?.modpow(&hex_int(dh_random)?, &p);
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&to_byte_array(&shared)).ok()?;
    mac.update(&hex::decode(prepend).ok()?);
    Some(B64.encode(mac.finalize().into_bytes()))
}

/// Oauth.validate_live_session_token: the server's signature is hex(HMAC-SHA1(key = base64-decoded LST, consumer key)),
/// compared in constant time.
pub fn validate_live_session_token(live_session_token: &str, signature: &str, consumer_key: &str) -> bool {
    let Ok(key) = B64.decode(live_session_token) else { return false };
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&key).expect("HMAC takes any key");
    mac.update(consumer_key.as_bytes());
    hex::encode(mac.finalize().into_bytes()).as_bytes().ct_eq(signature.as_bytes()).into()
}

/// Oauth.authorization_header: `OAuth realm="…", k="v", …`, params sorted by key, values raw.
pub fn authorization_header(realm: &str, params: &BTreeMap<String, String>) -> String {
    let pairs = params.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect::<Vec<_>>().join(", ");
    format!("OAuth realm=\"{realm}\", {pairs}")
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    SystemRandom::new().fill(&mut b).expect("system randomness");
    b
}

/// Oauth.nonce: 16 random alphanumerics.
pub fn nonce() -> String {
    const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    // 248 = 4 × 62: rejecting bytes at or above it keeps every character equally likely.
    std::iter::repeat_with(random_bytes::<32>).flatten().filter(|b| *b < 248).take(16).map(|b| ALNUM[(b % 62) as usize] as char).collect()
}

/// Oauth.timestamp: unix seconds.
pub fn timestamp() -> String { chrono::Utc::now().timestamp().to_string() }

/// Oauth.dh_random: 32 random bytes as 64 hex characters.
pub fn dh_random() -> String { hex::encode(random_bytes::<32>()) }

// ---- the keyed half (Oauth's instance methods)

/// What an IBKR key holds, as the api_keys row stores it: the consumer key (`key`), access token and its encrypted secret
/// (`secret`), the two RSA private keys and the DH parameters as PEM, and the realm.
#[derive(Clone)]
pub struct Material {
    pub consumer_key: String,
    pub access_token: String,
    pub access_token_secret: String,
    pub signature_key: String,
    pub encryption_key: String,
    pub dh_param: String,
    pub realm: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error { SignatureKey, EncryptionKey, DhParam }

pub struct Signer {
    consumer_key: String,
    access_token: String,
    access_token_secret: String,
    realm: String,
    signature_key: KeyPair,
    encryption_key: Pkcs1PrivateDecryptingKey,
    pub dh_prime: BigUint,
    pub dh_generator: BigUint,
}

/// An RSA private key from its PEM: PKCS#1 (`RSA PRIVATE KEY`, what Rails' `to_pem` stores) or PKCS#8 (`PRIVATE KEY`).
pub fn rsa_private_key(pem: &str) -> Option<KeyPair> {
    match read_pem(pem)? {
        (label, der) if label == "RSA PRIVATE KEY" => KeyPair::from_der(&der).ok(),
        (label, der) if label == "PRIVATE KEY" => KeyPair::from_pkcs8(&der).ok(),
        _ => None,
    }
}

impl Signer {
    /// Session#initialize's reads: the keys parsed, the DH params read, a blank realm as `limited_poa`.
    pub fn new(m: &Material) -> Result<Self, Error> {
        let signature_key = rsa_private_key(&m.signature_key).ok_or(Error::SignatureKey)?;
        // aws-lc decrypts with a PKCS#8 key only: re-encode the parsed key as PKCS#8.
        let encryption_key = rsa_private_key(&m.encryption_key)
            .and_then(|k| PrivateDecryptingKey::from_pkcs8(k.as_der().ok()?.as_ref()).ok())
            .and_then(|k| Pkcs1PrivateDecryptingKey::new(k).ok())
            .ok_or(Error::EncryptionKey)?;
        let (dh_prime, dh_generator) = read_dh_params(&m.dh_param).ok_or(Error::DhParam)?;
        let realm = if m.realm.trim().is_empty() { DEFAULT_REALM.into() } else { m.realm.clone() };
        Ok(Self { consumer_key: m.consumer_key.clone(), access_token: m.access_token.clone(), access_token_secret: m.access_token_secret.clone(),
                  realm, signature_key, encryption_key, dh_prime, dh_generator })
    }

    pub fn consumer_key(&self) -> &str { &self.consumer_key }

    /// Oauth#rsa_sha256_signature: escape(base64(RSA PKCS#1 v1.5 SHA-256 signature)). Deterministic.
    pub fn rsa_sha256_signature(&self, base: &str) -> String {
        let mut sig = vec![0u8; self.signature_key.public_modulus_len()];
        self.signature_key.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), base.as_bytes(), &mut sig).expect("RSA signing");
        escape(&B64.encode(sig))
    }

    /// Oauth#prepend: the access-token secret (strict base64) decrypted with the encryption key (PKCS#1 v1.5), as hex.
    /// None when it does not decrypt.
    pub fn prepend(&self) -> Option<String> {
        let ciphertext = B64.decode(&self.access_token_secret).ok()?;
        let mut out = vec![0u8; self.encryption_key.min_output_size()];
        Some(hex::encode(self.encryption_key.decrypt(&ciphertext, &mut out).ok()?))
    }

    /// Oauth#dh_challenge: g^dh_random mod p, lower-case hex without leading zeros.
    pub fn dh_challenge(&self, dh_random: &str) -> Option<String> {
        Some(self.dh_generator.modpow(&hex_int(dh_random)?, &self.dh_prime).to_str_radix(16))
    }

    /// The live session token from the server's DH response (hex), with the client's `dh_random` and `prepend`.
    pub fn live_session_token(&self, dh_random: &str, dh_response: &str, prepend: &str) -> Option<String> {
        live_session_token(&self.dh_prime.to_str_radix(16), dh_random, dh_response, prepend)
    }

    fn oauth_params(&self, nonce: &str, timestamp: &str, method: &str) -> BTreeMap<String, String> {
        [("oauth_consumer_key", self.consumer_key.as_str()), ("oauth_token", &self.access_token), ("oauth_nonce", nonce),
         ("oauth_timestamp", timestamp), ("oauth_signature_method", method)]
            .into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// Oauth#live_session_token_header: RSA-SHA256 over prepend + the POST base string, the DH challenge among the params.
    pub fn live_session_token_header(&self, url: &str, dh_challenge: &str, prepend: &str, nonce: &str, timestamp: &str) -> String {
        let mut params = self.oauth_params(nonce, timestamp, "RSA-SHA256");
        params.insert("diffie_hellman_challenge".into(), dh_challenge.into());
        let sig = self.rsa_sha256_signature(&base_string("POST", url, &params, Some(prepend)));
        params.insert("oauth_signature".into(), sig);
        authorization_header(&self.realm, &params)
    }

    /// Oauth#signed_header: HMAC-SHA256 keyed by the LST. The query params join the base string (a query key wins over an
    /// oauth one, as Hash#merge) but not the header. `method` upper-case, `url` without its query.
    pub fn signed_header(&self, method: &str, url: &str, live_session_token: &str, query: &[(String, String)], nonce: &str, timestamp: &str) -> Option<String> {
        let mut params = self.oauth_params(nonce, timestamp, "HMAC-SHA256");
        let mut signed = params.clone();
        signed.extend(query.iter().cloned());
        params.insert("oauth_signature".into(), hmac_sha256_signature(&base_string(method, url, &signed, None), live_session_token)?);
        Some(authorization_header(&self.realm, &params))
    }
}

// ---- PEM and the little DER the DH parameters need

/// The first PEM block in `pem`: its label and the base64-decoded body.
pub fn read_pem(pem: &str) -> Option<(String, Vec<u8>)> {
    let rest = &pem[pem.find("-----BEGIN ")? + 11..];
    let (label, rest) = rest.split_once("-----")?;
    let body = &rest[..rest.find(&format!("-----END {label}-----"))?];
    let b64: String = body.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    Some((label.to_string(), B64.decode(b64).ok()?))
}

/// A PEM block: base64 in 64-column lines, as OpenSSL writes it.
pub fn write_pem(label: &str, der: &[u8]) -> String {
    let b64 = B64.encode(der);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).expect("base64 is ASCII")).collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

/// One DER TLV: (tag, value, rest).
fn der_tlv(d: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, d) = d.split_first()?;
    let (&first, mut d) = d.split_first()?;
    let len = if first < 0x80 { first as usize } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 || d.len() < n { return None }
        let len = d[..n].iter().fold(0usize, |a, b| (a << 8) | *b as usize);
        d = &d[n..];
        len
    };
    (d.len() >= len).then(|| (tag, &d[..len], &d[len..]))
}

fn der_len(len: usize) -> Vec<u8> {
    if len < 0x80 { return vec![len as u8] }
    let bytes: Vec<u8> = len.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
    [vec![0x80 | bytes.len() as u8], bytes].concat()
}

fn der_uint(n: &BigUint) -> Vec<u8> {
    let mut v = n.to_bytes_be();
    if v[0] & 0x80 != 0 { v.insert(0, 0) }
    [vec![0x02], der_len(v.len()), v].concat()
}

/// `DH PARAMETERS` (PKCS#3 DHParameter: SEQUENCE { prime, base, privateValueLength OPTIONAL }) → (p, g).
pub fn read_dh_params(pem: &str) -> Option<(BigUint, BigUint)> {
    let (label, der) = read_pem(pem)?;
    if label != "DH PARAMETERS" { return None }
    let (0x30, mut seq, _) = der_tlv(&der)? else { return None };
    let mut ints = Vec::new();
    while !seq.is_empty() {
        let (0x02, v, rest) = der_tlv(seq)? else { return None };
        if v.first().is_none_or(|b| b & 0x80 != 0) { return None } // empty or negative
        ints.push(BigUint::from_bytes_be(v));
        seq = rest;
    }
    if !(2..=3).contains(&ints.len()) || ints[0].bits() < 2 { return None }
    let mut ints = ints.into_iter();
    Some((ints.next()?, ints.next()?))
}

/// The `DH PARAMETERS` PEM of (p, g), as `OpenSSL::PKey::DH#to_pem` writes it.
pub fn dh_params_pem(p: &BigUint, g: &BigUint) -> String {
    let body = [der_uint(p), der_uint(g)].concat();
    write_pem("DH PARAMETERS", &[vec![0x30], der_len(body.len()), body].concat())
}

/// A fresh RSA-2048 key pair as (private PEM, PKCS#8 `PRIVATE KEY`; public PEM, SPKI `PUBLIC KEY`, what the user uploads).
pub fn generate_rsa() -> (String, String) {
    let key = KeyPair::generate(KeySize::Rsa2048).expect("RSA-2048 generation");
    let private = write_pem("PRIVATE KEY", key.as_der().expect("PKCS#8").as_ref());
    let public = write_pem("PUBLIC KEY", key.public_key().as_der().expect("SPKI").as_ref());
    (private, public)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_byte_array_pads_a_whole_byte_bit_length() {
        for (n, want) in [(0u32, vec![0u8]), (1, vec![1]), (127, vec![127]), (128, vec![0, 128]), (255, vec![0, 255]), (256, vec![1, 0]),
                          (65_535, vec![0, 255, 255]), (65_536, vec![1, 0, 0])] {
            assert_eq!(to_byte_array(&BigUint::from(n)), want, "{n}");
        }
    }

    #[test]
    fn the_der_reader_refuses_what_is_not_dh_parameters() {
        let pem = |der: &[u8]| write_pem("DH PARAMETERS", der);
        assert!(read_dh_params(&pem(&[0x30, 0x06, 0x02, 0x01, 0x17, 0x02, 0x01, 0x02])).is_some());
        assert!(read_dh_params(&pem(&[0x30, 0x03, 0x02, 0x01, 0x17])).is_none(), "no generator");
        assert!(read_dh_params(&pem(&[0x30, 0x06, 0x02, 0x01, 0x97, 0x02, 0x01, 0x02])).is_none(), "negative prime");
        assert!(read_dh_params(&pem(&[0x30, 0x07, 0x02, 0x01, 0x17])).is_none(), "truncated");
        assert!(read_dh_params(&write_pem("PUBLIC KEY", &[0x30, 0x06, 0x02, 0x01, 0x17, 0x02, 0x01, 0x02])).is_none(), "wrong label");
        let big = BigUint::from(1u8) << 1100u32;
        assert_eq!(read_dh_params(&dh_params_pem(&big, &BigUint::from(5u8))), Some((big, BigUint::from(5u8))), "long-form lengths round-trip");
    }

    #[test]
    fn nonces_and_randoms_have_rails_shapes() {
        let (a, b) = (nonce(), nonce());
        assert!(a.len() == 16 && a.bytes().all(|c| c.is_ascii_alphanumeric()) && a != b);
        let r = dh_random();
        assert!(r.len() == 64 && r.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(timestamp().len(), 10);
    }

    #[test]
    fn ffdhe2048_is_the_openssl_group() {
        assert_eq!(ffdhe2048_pem(), "-----BEGIN DH PARAMETERS-----\nMIIBCAKCAQEA//////////+t+FRYortKmq/cViAnPTzx2LnFg84tNpWp4TZBFGQz\n+8yTnc4kmz75fS/jY2MMddj2gbICrsRhetPfHtXV/WVhJDP1H18GbtCFY2VVPe0a\n87VXE15/V8k1mE8McODmi3fipona8+/och3xWKE2rec1MKzKT0g6eXq8CrGCsyT7\nYdEIqUuyyOP7uWrat2DX9GgdT0Kj3jlN9K5W7edjcrsZCwenyO4KbXCeAvzhzffi\n7MA0BM0oNC9hkXL+nOmFg/+OTxIy7vKBg8P+OxtMb61zO7X8vC7CIAXFjvGDfRaD\nssbzSibBsu/6iGtCOGEoXJf//////////wIBAg==\n-----END DH PARAMETERS-----\n");
    }
}
