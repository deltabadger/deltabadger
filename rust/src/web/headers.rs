//! Response headers Rails adds around every page: the default security headers (load_defaults 8.1),
//! the report-only Content-Security-Policy with its per-request nonce
//! (config/initializers/content_security_policy.rb, pinned by
//! test/integration/content_security_policy_test.rb), HSTS when SSL is forced, and Cache-Control.
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use base64::{engine::general_purpose::STANDARD as B64, Engine};

/// `SecureRandom.base64(16)`: the value of `<meta name="csp-nonce">` and of the header's nonce source.
pub fn new_nonce() -> String {
    B64.encode(rand::random::<[u8; 16]>())
}

pub fn content_security_policy(nonce: &str) -> String {
    format!(
        "default-src 'self'; font-src 'self' data:; img-src 'self' data: https:; object-src 'none'; base-uri 'self'; \
         frame-ancestors 'none'; form-action 'self'; script-src 'self' 'nonce-{nonce}'; style-src 'self' 'unsafe-inline'; \
         connect-src 'self' ipc: http://ipc.localhost; report-uri /csp-report"
    )
}

/// Marks a response that Rails produces below its controllers (rack-attack's 429, Devise's failure
/// app): it gets the policy header and Rack's Cache-Control, but not the five default headers.
#[derive(Clone)]
pub struct BelowControllers;

const DEFAULTS: [(&str, &str); 5] = [
    ("x-frame-options", "SAMEORIGIN"),
    ("x-xss-protection", "0"),
    ("x-content-type-options", "nosniff"),
    ("x-permitted-cross-domain-policies", "none"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
];

/// What Rails' middleware adds to any response: the policy, and HSTS when SSL is forced
/// (ActionDispatch::SSL's default: two years, with subdomains).
pub fn policy(headers: &mut HeaderMap, nonce: &str, force_ssl: bool) {
    if let Ok(value) = HeaderValue::from_str(&content_security_policy(nonce)) {
        headers.insert(HeaderName::from_static("content-security-policy-report-only"), value);
    }
    if force_ssl {
        headers.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static("max-age=63072000; includeSubDomains"));
    }
}

/// What a Rails controller response carries on top: the five default headers, and a Cache-Control
/// when the handler set none: `no-store` for a signed-in request (ApplicationController#set_no_cache),
/// else Rack's defaults, which are `max-age=0, private, must-revalidate` for a 200 and `no-cache` otherwise.
pub fn controller_defaults(headers: &mut HeaderMap, status: StatusCode, signed_in: bool) {
    for (name, value) in DEFAULTS {
        headers.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
    }
    cache_control(headers, status, signed_in);
}

pub fn cache_control(headers: &mut HeaderMap, status: StatusCode, signed_in: bool) {
    if headers.contains_key(header::CACHE_CONTROL) {
        return;
    }
    let value = if signed_in {
        "no-store"
    } else if status == StatusCode::OK {
        "max-age=0, private, must-revalidate"
    } else {
        "no-cache"
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
}
