//! The Binance read client's transport (D2b ruling Q16): a configured proxy is the only way out, never bypassed.
use deltabadger::crypto::Credentials;
use deltabadger::sync::jobs::Connect;
use deltabadger::venue::alpaca::LiveFactory;
use deltabadger::venue::binance::{BinanceVenue, URL};
use std::io::{BufRead, BufReader};

fn credentials() -> Credentials { Credentials { key: "binance-key".into(), secret: "binance-secret".into(), passphrase: None, redaction_values: vec![] } }

#[tokio::test(flavor = "current_thread")]
async fn every_read_goes_to_the_configured_proxy() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let seen = std::thread::spawn(move || {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < until {
            if let Ok((stream, _)) = listener.accept() {
                stream.set_nonblocking(false).unwrap();
                let mut line = String::new();
                BufReader::new(stream).read_line(&mut line).unwrap();
                return line; // the stream drops here: the client's request fails
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        String::from("nothing reached the proxy")
    });
    let venue = BinanceVenue::new(LiveFactory::new().binance_transport(&credentials(), Some(&proxy)), URL, "binance-secret");
    let at = "2026-09-20T02:30:00.750000Z".parse().unwrap();
    assert!(venue.signed_get("/api/v3/account", vec![], at, 1024).await.is_err());
    assert_eq!(seen.join().unwrap().trim_end(), "CONNECT api.binance.com:443 HTTP/1.1", "the request went to the proxy, for Binance's host");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unusable_proxy_sends_nothing() {
    let venue = BinanceVenue::new(LiveFactory::new().binance_transport(&credentials(), Some("::not a url")), URL, "binance-secret");
    let at = "2026-09-20T02:30:00.750000Z".parse().unwrap();
    assert_eq!(venue.signed_get("/api/v3/account", vec![], at, 1024).await.unwrap_err(), "the configured exchange proxy is not a usable URL");
}

#[tokio::test(flavor = "current_thread")]
async fn without_a_proxy_connect_refuses_by_default() {
    struct Bare;
    impl Connect for Bare {
        type T = deltabadger::venue::http::ScriptedTransport;
        fn connect(&self, _: &Credentials) -> deltabadger::venue::alpaca::AlpacaVenue<Self::T> { unreachable!() }
    }
    let venue = BinanceVenue::new(Bare.binance_transport(&credentials(), None), URL, "binance-secret");
    let at = "2026-09-20T02:30:00.750000Z".parse().unwrap();
    assert_eq!(venue.signed_get("/api/v3/account", vec![], at, 1024).await.unwrap_err(), "Binance is not connected here");
}
