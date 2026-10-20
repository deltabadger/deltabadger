//! Signing at send time (plan VB §1.2): which headers and body a request carries, per venue wire, built by
//! `http::ReqwestTransport` immediately before the send so a timestamp or nonce is read then and the signature covers
//! the exact bytes sent. The schemes themselves stay with their venue (`kraken::private_headers`, `kucoin::headers`,
//! `binance::sign`); a new scheme lands here with its venue and a known-answer test.
use super::http::HttpRequest;

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
}

impl Wire {
    /// The venue this wire belongs to, as its stderr lines name it (`registry::Row::name`). Binance.US speaks Binance's wire
    /// and is labelled as it always was.
    pub(crate) fn label(self) -> &'static str {
        let exchange_type = match self {
            Self::Alpaca => crate::sync::ALPACA, Self::Binance => super::binance::BINANCE, Self::Kraken => "Exchanges::Kraken", Self::Kucoin => super::kucoin::KUCOIN,
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
    if wire == Wire::Kucoin {
        // The query exactly as Faraday::Utils.build_query writes it, which is what the signature covers.
        let q = super::kucoin::query_text(r);
        url.set_query((!q.is_empty()).then_some(q.as_str()));
    }
    let mut b = client.request(method, url);
    match wire {
        Wire::Kucoin => {
            // The timestamp is read here, at send time, and the signature covers the exact bytes sent.
            let ts = super::binance::timestamp_ms(clock.now());
            for (k, v) in super::kucoin::headers(r, keys.key, keys.secret, keys.passphrase, ts) { b = b.header(k, v); }
            if let Some(body) = super::kucoin::body_text(r) { b = b.body(body); }
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
