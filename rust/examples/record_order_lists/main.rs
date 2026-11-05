//! Records each exchange's live order-list answers, scrubbed, as test fixtures for A28-3b/3c (PLAN-A28-fixtures.md).
//! Read-only: only the reads on `recorder::ENDPOINTS` and `recorder::PERMISSION_READS` are ever sent. Run by hand, on
//! your own read-only key, never in a test or CI. The run book is README.md beside this file.
//!
//!   cargo run --example record_order_lists -- <venue> --since RFC3339 --until RFC3339 [--symbol S] [--paper]
//!
//! Keys come from RECORDER_KEY, RECORDER_SECRET and RECORDER_PASSPHRASE only (Hyperliquid: RECORDER_KEY is the
//! wallet address and no secret is set). Output: rust/tests/fixtures/order_lists/<venue>/. It never runs git.
mod recorder;

use deltabadger::venue::http::{client, ReqwestTransport};
use std::process::ExitCode;

const USAGE: &str = "usage: record_order_lists <venue> --since RFC3339 --until RFC3339 [--symbol S] [--paper]";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(report) => { println!("{report}"); ExitCode::SUCCESS }
        Err(e) => { eprintln!("record_order_lists: {e}"); ExitCode::FAILURE }
    }
}

fn run(args: Vec<String>) -> Result<String, String> {
    recorder::refuse_key_args(&args)?;
    let mut args = args.into_iter();
    let venue = args.next().ok_or(USAGE)?;
    let (mut since, mut until, mut symbol, mut paper) = (None, None, None, false);
    while let Some(flag) = args.next() {
        let mut value = || args.next().filter(|v| !v.starts_with("--")).ok_or(USAGE);
        match flag.as_str() {
            "--since" => since = Some(value()?),
            "--until" => until = Some(value()?),
            "--symbol" => symbol = Some(value()?),
            "--paper" => paper = true,
            _ => return Err(USAGE.into()),
        }
    }
    let time = |v: Option<String>| -> Result<chrono::DateTime<chrono::Utc>, String> {
        let v = v.ok_or(USAGE)?;
        Ok(chrono::DateTime::parse_from_rfc3339(&v).map_err(|_| format!("{v} is not RFC 3339"))?.to_utc())
    };
    let window = recorder::Window { since: time(since)?, until: time(until)?, symbol };
    if window.since >= window.until { return Err("--since must be before --until".into()); }
    let base = recorder::base(&venue, paper).ok_or(USAGE)?;
    let env = |name| std::env::var(name).unwrap_or_default();
    let keys = recorder::Keys { key: env("RECORDER_KEY"), secret: env("RECORDER_SECRET"), passphrase: env("RECORDER_PASSPHRASE") };
    if keys.key.is_empty() { return Err("set RECORDER_KEY (Hyperliquid: the wallet address)".into()); }
    match (venue.as_str(), keys.secret.is_empty()) {
        ("hyperliquid", false) => return Err("Hyperliquid's reads are public: unset RECORDER_SECRET".into()),
        ("hyperliquid", true) => {}
        (_, true) => return Err("set RECORDER_SECRET".into()),
        _ => {}
    }
    if ["kucoin", "bitget"].contains(&venue.as_str()) && keys.passphrase.is_empty() { return Err("set RECORDER_PASSPHRASE".into()); }
    let (c, k, s, p) = (client(), keys.key.clone(), keys.secret.clone(), keys.passphrase.clone());
    let t = match venue.as_str() {
        "kraken" => ReqwestTransport::new(c, k, s).kraken(),
        "kucoin" => ReqwestTransport::kucoin(c, k, s, p),
        "coinbase" => ReqwestTransport::coinbase(c, k, s),
        "alpaca" => ReqwestTransport::new(c, k, s),
        "bitget" => ReqwestTransport::bitget(c, k, s, p),
        "mexc" => ReqwestTransport::mexc(c, k, s),
        "gemini" => ReqwestTransport::gemini(c, k, s),
        "bitvavo" => ReqwestTransport::bitvavo(c, k, s),
        "bingx" => ReqwestTransport::bingx(c, k, s),
        "bitrue" => ReqwestTransport::bitrue(c, k, s),
        "hyperliquid" => ReqwestTransport::hyperliquid(c, k, s),
        _ => return Err(USAGE.into()),
    };
    let out = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/order_lists"));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(recorder::record(&t, &venue, base, &window, &keys, &mut recorder::Scrubber::random(), out))
}
