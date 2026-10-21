//! Signing at send time (plan VB §1.2): which headers and body a request carries, per venue wire, built by
//! `http::ReqwestTransport` immediately before the send so a timestamp or nonce is read then and the signature covers
//! the exact bytes sent. The schemes themselves stay with their venue (`kraken::private_headers`, `binance::sign`); a new
//! scheme lands here with its venue and a known-answer test. `Prehash` is the family KuCoin and Bitvavo share (§1.2,
//! generalised from `kucoin::headers` in slice A3, KuCoin pinned by tests/kucoin.rs before and after).
use super::http::HttpRequest;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// How a `Prehash` signature is written.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Encode { Base64, Hex }

/// What a `Prehash` venue sends in its passphrase header: HMAC'd with the secret (KuCoin key version 2), as it is, or none.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Passphrase { Hmac, None }

/// HMAC-SHA256(secret, timestamp + METHOD + path + tail), the tail `?query` (a GET with params; a DELETE where
/// `delete_query`), the JSON body (a POST) or nothing; the key, signature and timestamp (and the passphrase) in the
/// venue's headers, then its constant ones, then Accept and Content-Type as JSON.
pub(crate) struct Prehash {
    pub encode: Encode,
    pub passphrase: Passphrase,
    /// Whether a DELETE signs its `?query` (Honeymaker::Clients::Bitvavo#delete_signed) or the path alone (::Kucoin's).
    pub delete_query: bool,
    /// The key, signature, timestamp and passphrase headers.
    pub names: [&'static str; 4],
    pub extra: &'static [(&'static str, &'static str)],
}

/// Honeymaker::Clients::Kucoin#signed_headers: base64, the passphrase HMAC'd, KC-API-KEY-VERSION 2; a DELETE signs the path.
pub(crate) const KUCOIN: Prehash = Prehash { encode: Encode::Base64, passphrase: Passphrase::Hmac, delete_query: false,
    names: ["KC-API-KEY", "KC-API-SIGN", "KC-API-TIMESTAMP", "KC-API-PASSPHRASE"], extra: &[("KC-API-KEY-VERSION", "2")] };
/// Honeymaker::Clients::Bitvavo#signed_headers: hex, no passphrase, BITVAVO-ACCESS-WINDOW 10000; a DELETE signs `?query`.
pub(crate) const BITVAVO: Prehash = Prehash { encode: Encode::Hex, passphrase: Passphrase::None, delete_query: true,
    names: ["BITVAVO-ACCESS-KEY", "BITVAVO-ACCESS-SIGNATURE", "BITVAVO-ACCESS-TIMESTAMP", ""], extra: &[("BITVAVO-ACCESS-WINDOW", "10000")] };

fn hmac(secret: &str, message: &str, encode: Encode) -> String {
    use base64::Engine;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(message.as_bytes());
    let bytes = mac.finalize().into_bytes();
    match encode { Encode::Base64 => base64::engine::general_purpose::STANDARD.encode(bytes), Encode::Hex => hex::encode(bytes) }
}

/// The JSON headers every honeymaker client sends (#unauthenticated_headers): a public read's whole set.
pub(crate) fn json_headers() -> Vec<(String, String)> {
    vec![("Accept".to_string(), "application/json".to_string()), ("Content-Type".to_string(), "application/json".to_string())]
}

/// `r`'s signed headers under `scheme` at `timestamp_ms`; `query` is the query exactly as sent (Faraday::Utils.build_query).
pub(crate) fn prehash(scheme: &Prehash, r: &HttpRequest, query: &str, key: &str, secret: &str, passphrase: &str, timestamp_ms: i64) -> Vec<(String, String)> {
    let ts = timestamp_ms.to_string();
    let tail = match r.method {
        "GET" if !query.is_empty() => format!("?{query}"),
        "DELETE" if scheme.delete_query && !query.is_empty() => format!("?{query}"),
        "POST" => r.body.as_ref().map(serde_json::Value::to_string).unwrap_or_default(),
        _ => String::new(),
    };
    let [key_name, sign_name, ts_name, passphrase_name] = scheme.names;
    let mut out = vec![(key_name.to_string(), key.to_string()), (sign_name.to_string(), hmac(secret, &format!("{ts}{}{}{tail}", r.method, r.path), scheme.encode)),
                       (ts_name.to_string(), ts)];
    if scheme.passphrase == Passphrase::Hmac { out.push((passphrase_name.to_string(), hmac(secret, passphrase, scheme.encode))); }
    out.extend(scheme.extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    out.extend(json_headers());
    out
}

/// Whose headers and body a request carries.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Wire {
    /// Clients::Alpaca: key and secret headers, a JSON body.
    Alpaca,
    /// Honeymaker::Clients::Binance#headers: the key in X-MBX-APIKEY; the secret only signs (`venue::binance`) and is never sent.
    Binance,
    /// Honeymaker::Clients::Kraken (venue::kraken): a private call is a nonce-first form body signed at send time.
    Kraken,
    /// Honeymaker::Clients::Kucoin (venue::kucoin): KC-API-* headers signed at send time over the exact query and body sent.
    Kucoin,
    /// Honeymaker::Clients::Bitvavo (venue::bitvavo): BITVAVO-ACCESS-* headers, `Prehash` as KuCoin's, hex.
    Bitvavo,
}

impl Wire {
    /// The venue this wire belongs to, as its stderr lines name it (`registry::Row::name`). Binance.US speaks Binance's wire
    /// and is labelled as it always was.
    pub(crate) fn label(self) -> &'static str {
        let exchange_type = match self {
            Self::Alpaca => crate::sync::ALPACA, Self::Binance => super::binance::BINANCE, Self::Kraken => "Exchanges::Kraken", Self::Kucoin => super::kucoin::KUCOIN,
            Self::Bitvavo => super::bitvavo::BITVAVO,
        };
        super::registry::row(exchange_type).expect("every wire's venue has a row").name
    }
}

/// The credentials a wire signs with.
pub(crate) struct Keys<'a> { pub key: &'a str, pub secret: &'a str, pub passphrase: &'a str }

/// `r` as `wire` sends it through `client`: its URL, headers and body, signed now (`clock`).
pub(crate) fn request(wire: Wire, client: &reqwest::Client, r: &HttpRequest, keys: Keys, clock: &dyn crate::engine::Clock) -> reqwest::RequestBuilder {
    let method = reqwest::Method::from_bytes(r.method.as_bytes()).expect("a static method name");
    let mut url = r.url();
    // The query exactly as Faraday::Utils.build_query writes it, which is what a Prehash signature covers.
    let q = super::binance::build_query(&r.query);
    if matches!(wire, Wire::Kucoin | Wire::Bitvavo) { url.set_query((!q.is_empty()).then_some(q.as_str())); }
    let mut b = client.request(method, url);
    match wire {
        Wire::Kucoin | Wire::Bitvavo => {
            // The timestamp is read here, at send time, and the signature covers the exact bytes sent.
            let ts = super::binance::timestamp_ms(clock.now());
            let headers = if wire == Wire::Kucoin { super::kucoin::headers(r, keys.key, keys.secret, keys.passphrase, ts) } else { super::bitvavo::headers(r, keys.key, keys.secret, ts) };
            for (k, v) in headers { b = b.header(k, v); }
            if let Some(body) = &r.body { b = b.body(body.to_string()); }
        }
        Wire::Alpaca | Wire::Binance => {
            b = if wire == Wire::Binance { b.header("X-MBX-APIKEY", keys.key) } else { b.header("APCA-API-KEY-ID", keys.key).header("APCA-API-SECRET-KEY", keys.secret) };
            b = b.header("Accept", "application/json").header("Content-Type", "application/json");
            if let Some(body) = &r.body { b = b.body(body.to_string()); }
        }
        Wire::Kraken if r.method == "POST" => {
            // The nonce is minted here, at send time, and the signature covers the exact bytes sent.
            let form = super::kraken::private_form(r, keys.key);
            for (k, v) in super::kraken::private_headers(&r.path, &form, keys.key, keys.secret) { b = b.header(k, v); }
            b = b.body(form);
        }
        Wire::Kraken => { for (k, v) in super::kraken::public_headers() { b = b.header(k, v); } }
    }
    b
}
