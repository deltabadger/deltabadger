//! Records each exchange's live order-list answers, scrubbed, as test fixtures for A28-3b/3c (PLAN-A28-fixtures.md).
//! Read-only: only the reads on `recorder::ENDPOINTS` and `recorder::PERMISSION_READS` are ever sent. Run by hand, on
//! your own read-only key, never in a test or CI. The run book is README.md beside this file.
//!
//!   cargo run --example record_order_lists -- <venue> --since RFC3339 --until RFC3339 [--symbol S] [--paper]
//!
//! Keys come from RECORDER_KEY, RECORDER_SECRET and RECORDER_PASSPHRASE only (Hyperliquid: RECORDER_KEY is the
//! wallet address and no secret is set). Output: rust/tests/fixtures/order_lists/<venue>/. It never runs git.
mod recorder;
mod uta;

use deltabadger::venue::http::{client, ReqwestTransport};
use std::process::ExitCode;

const USAGE: &str = "usage: record_order_lists <venue> --since RFC3339 --until RFC3339 [--symbol S] [--paper]
       record_order_lists kucoin-uta --phase before|switched|traded --since RFC3339 --until RFC3339 --symbol S [--order ID]...
       record_order_lists kucoin-uta-csv --file EXPORT.csv";

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
    let (mut since, mut until, mut symbol, mut paper, mut phase, mut orders, mut file) = (None, None, None, false, None, vec![], None);
    while let Some(flag) = args.next() {
        let mut value = || args.next().filter(|v| !v.starts_with("--")).ok_or(USAGE);
        match flag.as_str() {
            "--since" => since = Some(value()?),
            "--until" => until = Some(value()?),
            "--symbol" => symbol = Some(value()?),
            "--paper" => paper = true,
            "--phase" => phase = Some(uta::Phase::parse(&value()?).ok_or(USAGE)?),
            "--order" => orders.push(value()?),
            "--file" => file = Some(value()?),
            _ => return Err(USAGE.into()),
        }
    }
    let out = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/order_lists"));
    let state = || -> Result<std::path::PathBuf, String> {
        let p = std::path::PathBuf::from(std::env::var("RECORDER_STATE").map_err(|_| "set RECORDER_STATE (a file beside the keys, never in the repo)")?);
        uta::state_outside(&p, std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("rust/ has a parent"))?;
        Ok(p)
    };
    if venue == "kucoin-uta-csv" {
        let (file, state) = (file.ok_or(USAGE)?, state()?);
        let (mut sc, factor) = uta::load_state(&state, false)?;
        let text = deltabadger::tracker::import::csv::as_text(&std::fs::read(&file).map_err(|e| format!("{file}: {e}"))?);
        // Any recorder key still in the environment is looked for too.
        let secrets: Vec<String> = std::env::vars().filter(|(n, v)| n.starts_with("RECORDER_") && n != "RECORDER_STATE" && !v.is_empty()).map(|(_, v)| v).collect();
        let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
        let scrubbed = uta::scrub_csv(&text, &mut sc, factor, &secrets)?;
        let dir = out.join("kucoin_uta").join("export");
        uta::save_state(&state, &sc, factor)?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("account_history.csv"), scrubbed).map_err(|e| e.to_string())?;
        return Ok(format!("wrote {}\nchanged fields:\n{}", dir.join("account_history.csv").display(), sc.report_text()));
    }
    let time = |v: Option<String>| -> Result<chrono::DateTime<chrono::Utc>, String> {
        let v = v.ok_or(USAGE)?;
        Ok(chrono::DateTime::parse_from_rfc3339(&v).map_err(|_| format!("{v} is not RFC 3339"))?.to_utc())
    };
    let window = recorder::Window { since: time(since)?, until: time(until)?, symbol };
    if window.since >= window.until { return Err("--since must be before --until".into()); }
    let base = recorder::base(if venue == "kucoin-uta" { "kucoin" } else { &venue }, paper).ok_or(USAGE)?;
    let env = |name| std::env::var(name).unwrap_or_default();
    let keys = recorder::Keys { key: env("RECORDER_KEY"), secret: env("RECORDER_SECRET"), passphrase: env("RECORDER_PASSPHRASE") };
    if keys.key.is_empty() { return Err("set RECORDER_KEY (Hyperliquid: the wallet address)".into()); }
    match (venue.as_str(), keys.secret.is_empty()) {
        ("hyperliquid", false) => return Err("Hyperliquid's reads are public: unset RECORDER_SECRET".into()),
        ("hyperliquid", true) => {}
        (_, true) => return Err("set RECORDER_SECRET".into()),
        _ => {}
    }
    if ["kucoin", "kucoin-uta", "bitget"].contains(&venue.as_str()) && keys.passphrase.is_empty() { return Err("set RECORDER_PASSPHRASE".into()); }
    let (c, k, s, p) = (client(), keys.key.clone(), keys.secret.clone(), keys.passphrase.clone());
    let t = match venue.as_str() {
        "kraken" => ReqwestTransport::new(c, k, s).kraken(),
        "kucoin" | "kucoin-uta" => ReqwestTransport::kucoin(c, k, s, p),
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
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    if venue == "kucoin-uta" {
        let (phase, state) = (phase.ok_or(USAGE)?, state()?);
        let (mut sc, factor) = uta::load_state(&state, phase == uta::Phase::Before)?;
        return rt.block_on(uta::record_uta(&t, base, phase, &window, &orders, &keys, &mut sc, factor, out, Some(&state)));
    }
    rt.block_on(recorder::record(&t, &venue, base, &window, &keys, &mut recorder::Scrubber::random(), out))
}
