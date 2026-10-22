//! Signing at send time (plan VB §1.2): which headers and body a request carries, per venue wire, built by
//! `http::ReqwestTransport` immediately before the send so a timestamp or nonce is read then and the signature covers
//! the exact bytes sent. The schemes themselves stay with their venue (`kraken::private_headers`, `binance::sign`); a new
//! scheme lands here with its venue and a known-answer test. `Prehash` is the family KuCoin, Bitvavo, Bitget and Bybit share (§1.2,
//! generalised from `kucoin::headers` in slice A3, KuCoin pinned by tests/kucoin.rs before and after). `QueryHmacHex` is the
//! family Binance and MEXC share (generalised from binance.rs `signed` in slice A5, Binance pinned by tests/binance_trade.rs).
use super::http::HttpRequest;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// How a `Prehash` signature is written.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Encode { Base64, Hex }

/// What a `Prehash` venue sends in its passphrase header: HMAC'd with the secret (KuCoin key version 2), as it is (Bitget),
/// or none.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Passphrase { Hmac, Plain, None }

/// Honeymaker's Binance-shaped clients (#get_signed, #post_signed, #delete_signed): the call's params (nils compacted) and
/// `timestamp`, a hex HMAC-SHA256 over `build_query` of exactly those added as `signature`, every pair in the query on every
/// method (a POST has no body); the key in `key_header`; the timestamp Client#timestamp_ms, `(Time.now.utc.to_f * 1_000).to_i`,
/// as `stamp` reads the clock.
pub(crate) struct QueryHmacHex { pub key_header: &'static str, pub stamp: fn(chrono::DateTime<chrono::Utc>) -> i64 }
/// Honeymaker::Clients::Binance (and ::BinanceUs), its timestamp as binance.rs has always computed it.
pub(crate) const BINANCE_QUERY: QueryHmacHex = QueryHmacHex { key_header: "X-MBX-APIKEY", stamp: super::binance::timestamp_ms };
/// Honeymaker::Clients::Mexc, its timestamp as Ruby's Float makes it (`bitvavo::ruby_ms`: 02:00:00.250 is ...249), so a
/// request is the one Rails sends at the same moment.
pub(crate) const MEXC_QUERY: QueryHmacHex = QueryHmacHex { key_header: "X-MEXC-APIKEY", stamp: super::bitvavo::ruby_ms };

pub(crate) const BITRUE_QUERY: QueryHmacHex = QueryHmacHex { key_header: "X-MBX-APIKEY", stamp: super::bitvavo::ruby_ms };

impl QueryHmacHex {
    /// `params` signed at `now` (Client#timestamp_ms): the timestamp, then the signature, appended.
    pub(crate) fn sign(&self, secret: &str, mut params: Vec<(&'static str, String)>, now: chrono::DateTime<chrono::Utc>) -> Vec<(&'static str, String)> {
        params.push(("timestamp", (self.stamp)(now).to_string()));
        let signature = hmac(secret, &super::binance::build_query(&params), Encode::Hex);
        params.push(("signature", signature));
        params
    }
}

/// The bytes covered by a venue's HMAC: method/path or Bybit's key/receive-window prefix.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum PrehashInput { MethodPath, BybitV5 }

/// HMAC-SHA256 with per-venue encoding, headers and exact query/body bytes.
pub(crate) struct Prehash {
    pub input: PrehashInput,
    pub encode: Encode,
    pub passphrase: Passphrase,
    /// Whether a DELETE signs its `?query` (Honeymaker::Clients::Bitvavo#delete_signed) or the path alone (::Kucoin's).
    pub delete_query: bool,
    /// The key, signature, timestamp and passphrase headers.
    pub names: [&'static str; 4],
    pub extra: &'static [(&'static str, &'static str)],
}

/// Honeymaker::Clients::Kucoin#signed_headers: base64, the passphrase HMAC'd, KC-API-KEY-VERSION 2; a DELETE signs the path.
pub(crate) const KUCOIN: Prehash = Prehash { input: PrehashInput::MethodPath, encode: Encode::Base64, passphrase: Passphrase::Hmac, delete_query: false,
    names: ["KC-API-KEY", "KC-API-SIGN", "KC-API-TIMESTAMP", "KC-API-PASSPHRASE"], extra: &[("KC-API-KEY-VERSION", "2")] };
/// Honeymaker::Clients::Bitvavo#signed_headers: hex, no passphrase, BITVAVO-ACCESS-WINDOW 10000; a DELETE signs `?query`.
pub(crate) const BITVAVO: Prehash = Prehash { input: PrehashInput::MethodPath, encode: Encode::Hex, passphrase: Passphrase::None, delete_query: true,
    names: ["BITVAVO-ACCESS-KEY", "BITVAVO-ACCESS-SIGNATURE", "BITVAVO-ACCESS-TIMESTAMP", ""], extra: &[("BITVAVO-ACCESS-WINDOW", "10000")] };
/// Honeymaker::Clients::Bitget#signed_headers: base64, the passphrase as it is, `locale: en-US`; it signs no DELETE.
pub(crate) const BITGET: Prehash = Prehash { input: PrehashInput::MethodPath, encode: Encode::Base64, passphrase: Passphrase::Plain, delete_query: false,
    names: ["ACCESS-KEY", "ACCESS-SIGN", "ACCESS-TIMESTAMP", "ACCESS-PASSPHRASE"], extra: &[("locale", "en-US")] };

/// Honeymaker Bybit V5: timestamp + key + receive window + exact query or JSON body.
pub(crate) const BYBIT: Prehash = Prehash { input: PrehashInput::BybitV5, encode: Encode::Hex, passphrase: Passphrase::None, delete_query: false,
    names: ["X-BAPI-API-KEY", "X-BAPI-SIGN", "X-BAPI-TIMESTAMP", ""], extra: &[("X-BAPI-RECV-WINDOW", "5000")] };

pub(crate) fn hmac(secret: &str, message: &str, encode: Encode) -> String {
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
    let message = match scheme.input {
        PrehashInput::MethodPath => format!("{ts}{}{}{tail}", r.method, r.path),
        PrehashInput::BybitV5 => {
            let tail = if r.method == "GET" { query.to_string() } else { tail };
            format!("{ts}{key}5000{tail}")
        }
    };
    let [key_name, sign_name, ts_name, passphrase_name] = scheme.names;
    let mut out = vec![(key_name.to_string(), key.to_string()), (sign_name.to_string(), hmac(secret, &message, scheme.encode)),
                       (ts_name.to_string(), ts)];
    match scheme.passphrase {
        Passphrase::Hmac => out.push((passphrase_name.to_string(), hmac(secret, passphrase, scheme.encode))),
        Passphrase::Plain => out.push((passphrase_name.to_string(), passphrase.to_string())),
        Passphrase::None => {}
    }
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
    /// Honeymaker::Clients::Bitget (venue::bitget): ACCESS-* headers, `Prehash` base64 with the passphrase as it is.
    Bitget,
    /// Honeymaker::Clients::Gemini (venue::gemini, signers::gemini): a private call is an empty-body POST whose params (the
    /// request's JSON body) travel in X-GEMINI-PAYLOAD with a nonce minted at send time; a public read is a plain GET.
    Gemini,
    /// Honeymaker::Clients::Mexc#headers (venue::mexc): Binance's wire with the key in X-MEXC-APIKEY.
    Mexc,
    /// Honeymaker::Clients::Hyperliquid (venue::hyperliquid, signers::hyperliquid): JSON POSTs; an /exchange body's `action`
    /// is signed at send time with a nonce from the process-wide source and posted as `{action, nonce, signature}`.
    Hyperliquid,
    /// Bybit V5: hex HMAC over timestamp + key + receive window + query/body.
    Bybit,
    Bitrue,
    Coinbase,
}

impl Wire {
    /// The venue this wire belongs to, as its stderr lines name it (`registry::Row::name`). Binance.US speaks Binance's wire
    /// and is labelled as it always was.
    pub(crate) fn label(self) -> &'static str {
        let exchange_type = match self {
            Self::Alpaca => crate::sync::ALPACA, Self::Binance => super::binance::BINANCE, Self::Kraken => "Exchanges::Kraken", Self::Kucoin => super::kucoin::KUCOIN,
            Self::Bitvavo => super::bitvavo::BITVAVO,
            Self::Bitget => super::bitget::BITGET,
            Self::Gemini => super::gemini::GEMINI, Self::Mexc => super::mexc::MEXC,
            Self::Hyperliquid => super::hyperliquid::HYPERLIQUID,
            Self::Bybit => super::bybit::BYBIT,
            Self::Bitrue => super::bitrue::BITRUE,
            Self::Coinbase => super::coinbase::COINBASE,
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
    // MEXC's QueryHmacHex signs that same text, so it is sent as it was signed.
    if matches!(wire, Wire::Kucoin | Wire::Bitvavo | Wire::Bitget | Wire::Mexc | Wire::Bybit | Wire::Bitrue) { url.set_query((!q.is_empty()).then_some(q.as_str())); }
    let mut b = client.request(method, url);
    match wire {
        Wire::Kucoin | Wire::Bitvavo | Wire::Bitget | Wire::Bybit => {
            // The timestamp is read here, at send time, and the signature covers the exact bytes sent.
            let ts = super::binance::timestamp_ms(clock.now());
            let headers = match wire {
                Wire::Kucoin => super::kucoin::headers(r, keys.key, keys.secret, keys.passphrase, ts),
                Wire::Bitvavo => super::bitvavo::headers(r, keys.key, keys.secret, ts),
                Wire::Bybit => super::bybit::headers(r, keys.key, keys.secret, ts),
                _ => super::bitget::headers(r, keys.key, keys.secret, keys.passphrase, ts),
            };
            for (k, v) in headers { b = b.header(k, v); }
            if let Some(body) = &r.body { b = b.body(body.to_string()); }
        }
        // Honeymaker::Clients::Bitrue#exchange_information uses get_public, whose headers are empty.
        Wire::Bitrue if !r.query.iter().any(|(k, _)| *k == "signature") => {}
        Wire::Alpaca | Wire::Binance | Wire::Mexc | Wire::Bitrue => {
            b = match wire {
                Wire::Binance => b.header(BINANCE_QUERY.key_header, keys.key),
                Wire::Mexc => b.header(MEXC_QUERY.key_header, keys.key),
                Wire::Bitrue => b.header(BITRUE_QUERY.key_header, keys.key),
                _ => b.header("APCA-API-KEY-ID", keys.key).header("APCA-API-SECRET-KEY", keys.secret),
            };
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
        Wire::Gemini if r.method == "POST" => {
            // The nonce is minted here, at send time (SPEC-E §2.9: process-wide, max(now_ms, last + 1)), and the signature
            // covers the exact payload sent. Nothing goes in the body.
            let params: Vec<(&str, serde_json::Value)> = r.body.as_ref().and_then(serde_json::Value::as_object).into_iter().flatten().map(|(k, v)| (k.as_str(), v.clone())).collect();
            let nonce = super::signers::gemini::NONCE.next_at(super::binance::timestamp_ms(clock.now()).max(0) as u64);
            for (k, v) in super::signers::gemini::headers(keys.key, keys.secret, &r.path, &params, nonce) { b = b.header(k, v); }
        }
        Wire::Gemini => {}
        Wire::Coinbase => {
            for (k, v) in json_headers() { b = b.header(k, v); }
            if let Some(jwt) = super::signers::coinbase::jwt(keys.key, keys.secret, r.method, &r.path) { b = b.header("Authorization", format!("Bearer {jwt}")); }
            if let Some(body) = &r.body { b = b.body(body.to_string()); }
        }
        Wire::Hyperliquid => {
            b = b.header("Accept", "application/json").header("Content-Type", "application/json");
            let signer = super::signers::hyperliquid::signing_key(keys.secret);
            let body = match (&r.body, signer) {
                // The nonce is minted here, at send time (Honeymaker::Clients::Hyperliquid.next_nonce), and signed with the action.
                (Some(body), Some(key)) if r.path == super::hyperliquid::EXCHANGE => {
                    let nonce = super::signers::hyperliquid::NONCE.next_at(super::binance::timestamp_ms(clock.now()).max(0) as u64);
                    super::signers::hyperliquid::signed_body(&key, &body["action"], nonce)
                }
                (Some(body), _) => body.to_string(),
                (None, _) => String::new(),
            };
            b = b.body(body);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrue_public_requests_are_unkeyed_and_private_requests_have_the_recorded_headers() {
        let clock = crate::engine::FixedClock("2026-09-20T02:00:00Z".parse().unwrap());
        let client = reqwest::Client::new();
        let mut r = HttpRequest { method: "GET", base: super::super::bitrue::URL.into(), path: "/api/v1/exchangeInfo".into(), query: vec![], body: None, not_after: None };
        let build = |r: &HttpRequest| request(Wire::Bitrue, &client, r, Keys { key: "key", secret: "secret", passphrase: "" }, &clock).build().unwrap();
        assert!(build(&r).headers().is_empty(), "honeymaker's public get sends no headers");
        r.path = "/api/v1/account".into();
        r.query = BITRUE_QUERY.sign("secret", vec![], crate::engine::Clock::now(&clock));
        let signed = build(&r);
        assert_eq!(signed.headers()["X-MBX-APIKEY"], "key");
        assert_eq!(signed.headers()["Accept"], "application/json");
        assert_eq!(signed.headers()["Content-Type"], "application/json");
    }
}
