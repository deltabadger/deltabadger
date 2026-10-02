//! config/initializers/rack_attack.rb for the pages this crate serves: per-address limits in fixed
//! 60-second windows aligned to the clock, kept in memory, and rack-attack's plain-text 429.
use super::Config;
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Response, StatusCode};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

const PERIOD: i64 = 60;

/// (rule name, route path, requests allowed per window). POST only, as in rack_attack.rb.
pub const RULES: [(&str, &str, u32); 2] = [("users/login", "/login", 10), ("users/verify_two_factor", "/verify_two_factor", 5)];

/// The addresses counted at once. An address costs about a hundred bytes, so this is some ten
/// megabytes at most. When it is reached, an address not yet counted is refused until the minute
/// ends, instead of being let through uncounted: a flood of addresses buys no free attempts.
pub const MAX_TRACKED: usize = 100_000;

/// The clock minute being counted, and (rule, address) -> requests seen in it.
#[derive(Default)]
struct Windows {
    window: i64,
    seen: HashMap<(&'static str, String), u32>,
}

#[derive(Default)]
pub struct Limiter {
    windows: Mutex<Windows>,
}

impl Limiter {
    /// Counts this request. `Some(seconds)` when it is over its rule's limit: the `retry-after` value.
    /// `method` is the one in effect after `_method` (Rack::MethodOverride runs before rack-attack).
    pub fn hit(&self, method: &Method, route_path: &str, address: &str, now: DateTime<Utc>) -> Option<i64> {
        let (rule, _, limit) = *RULES.iter().find(|(_, path, _)| method == Method::POST && *path == route_path)?;
        let epoch = now.timestamp();
        let window = epoch.div_euclid(PERIOD);
        let retry_after = PERIOD - epoch.rem_euclid(PERIOD);
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        if windows.window != window {
            // Once a minute, not on every request: an earlier minute's counts can never count again.
            windows.seen.clear();
            windows.window = window;
        }
        let key = (rule, address.to_string());
        if windows.seen.len() >= MAX_TRACKED && !windows.seen.contains_key(&key) {
            return Some(retry_after);
        }
        let seen = windows.seen.entry(key).or_insert(0);
        *seen = seen.saturating_add(1);
        (*seen > limit).then_some(retry_after)
    }

    /// How many (rule, address) pairs are being counted.
    pub fn tracked(&self) -> usize {
        self.windows.lock().unwrap_or_else(PoisonError::into_inner).seen.len()
    }
}

/// Rack::Attack.throttled_responder: the same sentence whatever tripped, in the default locale.
pub fn throttled(retry_after: i64) -> Response<Body> {
    let mut response = Response::new(Body::from(format!("{}\n", super::i18n::text(super::i18n::DEFAULT, "errors.throttled", &[]))));
    *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
    response.extensions_mut().insert(super::headers::BelowControllers);
    response
}

/// ActionDispatch::RemoteIp::TRUSTED_PROXIES: loopback, private and link-local ranges.
fn trusted(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 127 || o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168) || (o[0] == 169 && o[1] == 254)
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// The entries of a forwarding header, as Rack splits it: on commas, spaces and tabs.
fn entries(header: Option<&str>) -> impl DoubleEndedIterator<Item = &str> {
    header.unwrap_or("").split([',', ' ', '\t']).filter(|entry| !entry.is_empty())
}

/// The address in one entry. In X-Forwarded-For (`authority`) Rack accepts what a proxy may write
/// for its peer: `ip`, `ip:port`, `[v6]`, `[v6]:port`, and a bare IPv6 address
/// (Rack::Request::Helpers#forwarded_for: wrap_ipv6, then split_authority). In Client-Ip Rails
/// accepts an address only. An IPv4-mapped IPv6 address is the IPv4 address it carries.
fn address(entry: &str, authority: bool) -> Option<IpAddr> {
    let port = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    let host = match entry.strip_prefix('[') {
        Some(bracketed) if authority => {
            let (host, after) = bracketed.split_once(']')?;
            (after.is_empty() || after.strip_prefix(':').is_some_and(port)).then_some(host)?
        }
        None if authority && entry.matches(':').count() == 1 => entry.split_once(':').filter(|(_, after)| port(after))?.0,
        _ => entry,
    };
    host.parse::<IpAddr>().ok().map(|ip| ip.to_canonical())
}

/// The client's address behind a proxy. The walk starts at the peer and goes through
/// X-Forwarded-For right to left, then Client-Ip right to left, for as long as each hop is a trusted
/// proxy; the first address that is not one is the client. When every hop is a trusted proxy the
/// furthest one is taken. This is ActionDispatch::RemoteIp#calculate_ip (spoofing check off, as
/// config/application.rb sets it) with two changes, both where Rails can be told an address:
/// - Rails puts the peer last, so a peer that is no trusted proxy can name any address in a header
///   and be keyed on it. Here such a peer is the client;
/// - Rails drops an entry it cannot read and goes on to the one before it, which only the caller
///   wrote. Here the walk ends at an entry it cannot read, and the answer is the last trusted hop.
///
/// Rack's `Forwarded` header is not read.
pub fn remote_ip(peer: Option<IpAddr>, forwarded_for: Option<&str>, client_ip: Option<&str>) -> Option<IpAddr> {
    if let Some(peer) = peer.filter(|ip| !trusted(ip)) {
        return Some(peer);
    }
    let mut last_trusted = peer;
    let hops = entries(forwarded_for).rev().map(|entry| address(entry, true)).chain(entries(client_ip).rev().map(|entry| address(entry, false)));
    for hop in hops {
        match hop {
            Some(ip) if trusted(&ip) => last_trusted = Some(ip),
            Some(ip) => return Some(ip),
            None => break,
        }
    }
    last_trusted
}

/// Rack::Attack.client_ip: the peer address, unless the deployment says a proxy in front writes the
/// forwarded-for header (BEHIND_PROXY, else the same signal as FORCE_SSL), and then `remote_ip`. Never empty: a throttle
/// with no key would not count at all.
pub fn client_key(config: &Config, headers: &HeaderMap, peer: Option<IpAddr>) -> String {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let peer = peer.map(|ip| ip.to_canonical()); // an IPv4 peer on a dual-stack socket arrives as ::ffff:a.b.c.d
    let ip = if config.behind_proxy { remote_ip(peer, header("x-forwarded-for"), header("client-ip")) } else { peer };
    ip.map_or_else(|| "unattributed".to_string(), |ip| ip.to_string())
}
