//! CSRF protection as 2.x's `protect_from_forgery` gives it to the pages and the compiled JS:
//! one random token per session, handed out masked (a fresh one-time pad each time, so a compressed
//! response never repeats it), in `<meta name="csrf-token">` and the hidden `authenticity_token`
//! field, and accepted from that field or the `X-CSRF-Token` header. A request with an `Origin`
//! header must also come from this deployment's own origin (2.x's forgery_protection_origin_check).
use axum::http::{header, HeaderMap};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
use subtle::ConstantTimeEq;

const LENGTH: usize = 32;

/// A new session token: 32 random bytes, base64url.
pub fn new_token() -> String {
    B64URL.encode(rand::random::<[u8; LENGTH]>())
}

/// The token to put in a page: base64url(pad || pad XOR token). Empty if the session token is unreadable.
pub fn masked(token: &str) -> String { masked_with(token, rand::random()) }
fn masked_with(token: &str, pad: [u8; LENGTH]) -> String {
    let Ok(raw) = B64URL.decode(token) else { return String::new() };
    let mixed: Vec<u8> = raw.iter().zip(pad.iter()).map(|(t, p)| t ^ p).collect();
    B64URL.encode([pad.as_slice(), mixed.as_slice()].concat())
}
/// #710 7a rule S6: `masked`, its pad also stating what the page was drawn for: 16 random bytes, then a tag of `scope` and
/// a tag of `binding`, 8 bytes each, keyed by `key`. Any masked token still unmasks (`valid`); the page carries a value as
/// random-looking as before, so its bytes do not change.
pub fn masked_for(token: &str, key: &[u8], scope: &str, binding: &str) -> String {
    let mut pad = [0u8; LENGTH];
    pad[..16].copy_from_slice(&rand::random::<[u8; 16]>());
    let (scoped, bound) = (tag(key, &pad[..16], scope), tag(key, &pad[..16], &format!("{scope}\0{binding}")));
    pad[16..24].copy_from_slice(&scoped);
    pad[24..].copy_from_slice(&bound);
    masked_with(token, pad)
}
/// What a submitted token says of `scope`: None when no page drawn for `scope` carried it (any other token), else whether
/// it was drawn for `binding`.
pub fn drawn_for(submitted: &str, key: &[u8], scope: &str, binding: &str) -> Option<bool> {
    let given = B64URL.decode(submitted.trim_end_matches('=')).ok().filter(|g| g.len() == 2 * LENGTH)?;
    if !bool::from(given[16..24].ct_eq(&tag(key, &given[..16], scope))) { return None; }
    Some(given[24..LENGTH].ct_eq(&tag(key, &given[..16], &format!("{scope}\0{binding}"))).into())
}
fn tag(key: &[u8], nonce: &[u8], label: &str) -> [u8; 8] {
    use hmac::Mac;
    let mut mac = <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(key).expect("HMAC takes any key");
    mac.update(b"csrf page binding\0");
    mac.update(nonce);
    mac.update(label.as_bytes());
    let mut out = [0u8; 8];
    out.copy_from_slice(&mac.finalize().into_bytes()[..8]);
    out
}

/// Whether `submitted` (as a page carried it) unmasks to the session's token. Constant-time in the token.
pub fn valid(token: &str, submitted: &str) -> bool {
    let (Ok(raw), Ok(given)) = (B64URL.decode(token), B64URL.decode(submitted.trim_end_matches('='))) else { return false };
    if raw.len() != LENGTH || given.len() != 2 * LENGTH {
        return false;
    }
    let (pad, mixed) = given.split_at(LENGTH);
    let unmasked: Vec<u8> = mixed.iter().zip(pad.iter()).map(|(m, p)| m ^ p).collect();
    unmasked.ct_eq(&raw).into()
}

/// 2.x's forgery_protection_origin_check: a request without an `Origin` header passes (some user
/// agents send none); one with it must name one of `expected` exactly, scheme, host and port (this
/// deployment's origins, `Config::form_origins`). A header that is not text, or that is `null`, names
/// no origin of ours and fails. (2.x raises on `null`, a 422; here it is the same refusal as any
/// other mismatch.)
pub fn same_origin(headers: &HeaderMap, expected: &[Option<&str>]) -> bool {
    match headers.get(header::ORIGIN) {
        None => true,
        Some(origin) => origin.to_str().is_ok_and(|origin| expected.iter().flatten().any(|expected| origin == *expected)),
    }
}

/// The whole check for a non-GET request: our origin, and a valid token in the form field or in the
/// `X-CSRF-Token` header. Either one is enough, as in 2.x (`any_authenticity_token_valid?`): an
/// invalid field beside a valid header passes.
pub fn verified(token: Option<&str>, headers: &HeaderMap, form_token: Option<&str>, expected_origin: &[Option<&str>]) -> bool {
    let Some(token) = token else { return false };
    let header_token = headers.get("x-csrf-token").and_then(|v| v.to_str().ok());
    same_origin(headers, expected_origin) && [form_token, header_token].into_iter().flatten().any(|submitted| valid(token, submitted))
}
