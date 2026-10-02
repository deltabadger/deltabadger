//! CSRF protection as Rails' `protect_from_forgery` gives it to the pages and the compiled JS:
//! one random token per session, handed out masked (a fresh one-time pad each time, so a compressed
//! response never repeats it), in `<meta name="csrf-token">` and the hidden `authenticity_token`
//! field, and accepted from that field or the `X-CSRF-Token` header. A request with an `Origin`
//! header must also come from this deployment's own origin (Rails' forgery_protection_origin_check).
use axum::http::{header, HeaderMap};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
use subtle::ConstantTimeEq;

const LENGTH: usize = 32;

/// A new session token: 32 random bytes, base64url.
pub fn new_token() -> String {
    B64URL.encode(rand::random::<[u8; LENGTH]>())
}

/// The token to put in a page: base64url(pad || pad XOR token). Empty if the session token is unreadable.
pub fn masked(token: &str) -> String {
    let Ok(raw) = B64URL.decode(token) else { return String::new() };
    let pad: [u8; LENGTH] = rand::random();
    let mixed: Vec<u8> = raw.iter().zip(pad.iter()).map(|(t, p)| t ^ p).collect();
    B64URL.encode([pad.as_slice(), mixed.as_slice()].concat())
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

/// Rails' forgery_protection_origin_check: a request without an `Origin` header passes (some user
/// agents send none); one with it must name this deployment's own origin exactly, scheme, host and
/// port (`expected`, from `Config::origin`). A header that is not text, or that is `null`, names no
/// origin of ours and fails. (Rails raises on `null`, a 422; here it is the same refusal as any other
/// mismatch.)
pub fn same_origin(headers: &HeaderMap, expected: Option<&str>) -> bool {
    match headers.get(header::ORIGIN) {
        None => true,
        Some(origin) => origin.to_str().ok().zip(expected).is_some_and(|(origin, expected)| origin == expected),
    }
}

/// The whole check for a non-GET request: our origin, and a valid token in the form field or in the
/// `X-CSRF-Token` header. Either one is enough, as in Rails (`any_authenticity_token_valid?`): an
/// invalid field beside a valid header passes.
pub fn verified(token: Option<&str>, headers: &HeaderMap, form_token: Option<&str>, expected_origin: Option<&str>) -> bool {
    let Some(token) = token else { return false };
    let header_token = headers.get("x-csrf-token").and_then(|v| v.to_str().ok());
    same_origin(headers, expected_origin) && [form_token, header_token].into_iter().flatten().any(|submitted| valid(token, submitted))
}
