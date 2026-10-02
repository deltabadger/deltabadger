//! ActiveRecord::Encryption as this app configures it (config/initializers/active_record_encryption.rb,
//! load_defaults 8.1): keys from the env pair or derived from secret_key_base, PBKDF2-SHA256 key, AES-256-GCM.
use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

pub const PRIMARY_KEY_VAR: &str = "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY";
pub const SALT_VAR: &str = "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT";
pub const EXTERNAL_MARKER_VAR: &str = "ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL";
const COMPRESSION_THRESHOLD: usize = 140;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionKeys {
    pub primary_key: String,
    pub key_derivation_salt: String,
}

#[derive(Debug)]
pub enum KeyConfigError {
    Partial { missing: &'static str },
    ExternalMarkerWithoutKeys,
}

#[derive(Debug)]
pub enum DecryptError {
    /// A well-formed envelope this key cannot open. Rails would hand back the ciphertext here; we refuse.
    Unreadable,
    Malformed(String),
}

fn present(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(name).filter(|v| !v.trim().is_empty())
}

impl EncryptionKeys {
    /// Mirrors EncryptionKeys.validate! / primary_key / key_derivation_salt in the initializer.
    pub fn resolve(env: &dyn Fn(&str) -> Option<String>, secret_key_base: &str) -> Result<Self, KeyConfigError> {
        let (primary, salt) = (present(env, PRIMARY_KEY_VAR), present(env, SALT_VAR));
        if present(env, EXTERNAL_MARKER_VAR).is_some() && (primary.is_none() || salt.is_none()) {
            return Err(KeyConfigError::ExternalMarkerWithoutKeys);
        }
        match (primary, salt) {
            (Some(p), Some(s)) => Ok(Self { primary_key: p, key_derivation_salt: s }),
            (Some(_), None) => Err(KeyConfigError::Partial { missing: SALT_VAR }),
            (None, Some(_)) => Err(KeyConfigError::Partial { missing: PRIMARY_KEY_VAR }),
            (None, None) => Ok(Self {
                primary_key: hex::encode(Sha256::digest(format!("{secret_key_base}-ar-encryption-primary"))),
                key_derivation_salt: hex::encode(Sha256::digest(format!("{secret_key_base}-ar-encryption-salt"))),
            }),
        }
    }
}

pub struct Cipher {
    aead: Aes256Gcm,
}

impl Cipher {
    /// ActiveSupport::KeyGenerator.new(primary_key, hash_digest_class: SHA256).generate_key(salt, 32).
    pub fn new(keys: &EncryptionKeys) -> Self {
        let mut key = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha256>(keys.primary_key.as_bytes(), keys.key_derivation_salt.as_bytes(), 65_536, &mut key);
        Self { aead: Aes256Gcm::new(&key.into()) }
    }

    pub fn encrypt(&self, plain: &str) -> String {
        let (bytes, compressed) = if plain.len() > COMPRESSION_THRESHOLD {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(plain.as_bytes()).expect("in-memory write");
            (z.finish().expect("in-memory finish"), true)
        } else {
            (plain.as_bytes().to_vec(), false)
        };
        let iv: [u8; 12] = rand::random();
        let mut sealed = self.aead.encrypt(Nonce::from_slice(&iv), bytes.as_slice()).expect("AES-GCM encrypt");
        let tag = sealed.split_off(sealed.len() - 16);
        let mut headers = json!({ "iv": B64.encode(iv), "at": B64.encode(tag) });
        if compressed {
            headers["c"] = json!(true);
        }
        json!({ "p": B64.encode(sealed), "h": headers }).to_string()
    }

    pub fn decrypt(&self, stored: &str) -> Result<String, DecryptError> {
        let Some((payload, headers)) = envelope(stored) else { return Ok(stored.to_string()) };
        let payload = payload.as_str();
        let field = |k: &str| headers.get(k).and_then(Value::as_str).ok_or_else(|| DecryptError::Malformed(format!("missing {k}")));
        let decode = |s: &str| B64.decode(s).map_err(|e| DecryptError::Malformed(e.to_string()));
        let (iv, tag) = (decode(field("iv")?)?, decode(field("at")?)?);
        if iv.len() != 12 || tag.len() != 16 {
            return Err(DecryptError::Malformed(format!("iv {} bytes, tag {} bytes", iv.len(), tag.len())));
        }
        let mut sealed = decode(payload)?;
        sealed.extend_from_slice(&tag);
        let opened = self.aead.decrypt(Nonce::from_slice(&iv), sealed.as_slice()).map_err(|_| DecryptError::Unreadable)?;
        let bytes = if headers.get("c") == Some(&Value::Bool(true)) {
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(opened.as_slice()).read_to_end(&mut out).map_err(|e| DecryptError::Malformed(e.to_string()))?;
            out
        } else {
            opened
        };
        // "e" names the Ruby encoding when it was not UTF-8 (2FA seeds are binary strings of base32
        // text). The bytes are the same either way; anything that is not valid UTF-8 is refused here.
        String::from_utf8(bytes).map_err(|e| DecryptError::Malformed(e.to_string()))
    }
}

/// `Some((p, h))` only for a JSON object shaped like Rails' message; anything else is plaintext.
fn envelope(stored: &str) -> Option<(String, serde_json::Map<String, Value>)> {
    let v: Value = serde_json::from_str(stored).ok()?;
    Some((v.get("p")?.as_str()?.to_string(), v.get("h")?.as_object()?.clone()))
}

/// Devise's database_authenticatable with this app's settings (stretches 11, no pepper).
/// Ruby's bcrypt truncates the password at 72 bytes; the bcrypt crate's `hash`/`verify` do too.
/// An error only when the system's random source has no salt to give: no password causes one.
pub fn hash_password(plain: &str) -> Result<String, bcrypt::BcryptError> {
    Ok(bcrypt::hash_with_result(plain, 11)?.format_for_version(bcrypt::Version::TwoA))
}

pub fn verify_password(plain: &str, stored_hash: &str) -> bool {
    bcrypt::verify(plain, stored_hash).unwrap_or(false)
}

/// ROTP::TOTP#at: RFC 6238, SHA1, 6 digits, 30-second step.
pub fn totp_at(seed_base32: &str, unix_seconds: u64) -> Option<String> {
    use hmac::Mac;
    let key = base32_decode(seed_base32)?;
    let mut mac = <hmac::Hmac<sha1::Sha1> as Mac>::new_from_slice(&key).ok()?;
    mac.update(&(unix_seconds / 30).to_be_bytes());
    let d = mac.finalize().into_bytes();
    let o = (d[19] & 0x0f) as usize;
    let n = u32::from_be_bytes([d[o] & 0x7f, d[o + 1], d[o + 2], d[o + 3]]) % 1_000_000;
    Some(format!("{n:06}"))
}

fn base32_decode(s: &str) -> Option<Vec<u8>> {
    const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut bits, mut acc, mut out) = (0u32, 0u64, Vec::new());
    for c in s.trim_end_matches('=').chars() {
        acc = (acc << 5) | ALPHABET.find(c.to_ascii_uppercase())? as u64;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}
/// A venue API key, decrypted. `passphrase` is Alpaca's mode ("paper"/"live"; nil reads as paper); Kraken has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials { pub key: String, pub secret: String, pub passphrase: Option<String> }
