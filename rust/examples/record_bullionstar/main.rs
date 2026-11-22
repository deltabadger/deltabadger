//! Records BullionStar's live answers, scrubbed, as test fixtures for the venue (PLAN-bullionstar.md slice 0: the open facts;
//! README.md lists which ones a run settles).
//! Every order it sends carries `testOrder: true`: the Guard inside the transport refuses any other. Run by hand from an
//! IP on the key's whitelist, never in a test or CI. The run book is README.md beside this file.
//!
//!   cargo run --example record_bullionstar -- probe [--label NAME]
//!   cargo run --example record_bullionstar -- read [--details N]
//!   cargo run --example record_bullionstar -- orders --location ID --currency CUR --gram PRODUCT --bar PRODUCT \
//!       --payment ID --shipping ID --sell-payment ID [--unavailable PRODUCT] [--wrong-pin] [--expiry]
//!
//! The key comes from BULLIONSTAR_KEY and the PIN from BULLIONSTAR_PIN only. Output: rust/tests/fixtures/bullionstar/.
//! It never runs git.
mod recorder;

use chrono::{DateTime, Utc};
use deltabadger::venue::http::{client, HttpRequest, HttpResponse, Transport, TransportError};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::process::ExitCode;

/// The dealer's wire: the key in X-API-Key, a JSON body, the answer as it came. `client()` follows no redirect and
/// never retries. The rate-limit headers seen (F11) are kept for the report.
struct Live { client: reqwest::Client, key: String, limits: RefCell<BTreeMap<String, String>> }

impl Live {
    fn new(key: String) -> Self { Self { client: client(), key, limits: RefCell::default() } }
}

impl Transport for Live {
    fn now(&self) -> DateTime<Utc> { Utc::now() }
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let method = reqwest::Method::from_bytes(r.method.as_bytes()).map_err(|_| TransportError::Permanent("a bad method".into()))?;
        let mut b = self.client.request(method, r.url()).header("Accept", "application/json");
        if !self.key.is_empty() { b = b.header("X-API-Key", &self.key); }
        if let Some(body) = &r.body { b = b.header("Content-Type", "application/json").body(body.to_string()); }
        let resp = b.send().await.map_err(|e| if e.is_connect() { TransportError::NotSent("connect".into()) } else { TransportError::MaybeSent("no answer".into()) })?;
        for (name, value) in resp.headers() {
            let n = name.as_str().to_ascii_lowercase();
            if n.contains("ratelimit") || n.contains("rate-limit") || n == "retry-after" {
                self.limits.borrow_mut().insert(n, value.to_str().unwrap_or("?").to_string());
            }
        }
        let status = resp.status().as_u16();
        let body = resp.text().await.map_err(|_| TransportError::MaybeSent("the body was lost".into()))?;
        Ok(HttpResponse { status, body })
    }
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(report) => { println!("{report}"); ExitCode::SUCCESS }
        Err(e) => { eprintln!("record_bullionstar: {e}"); ExitCode::FAILURE }
    }
}

fn run(args: Vec<String>) -> Result<String, String> {
    let key = std::env::var("BULLIONSTAR_KEY").unwrap_or_default();
    let pin = std::env::var("BULLIONSTAR_PIN").unwrap_or_default();
    recorder::refuse_key_args(&args, &pin)?;
    let cmd = recorder::parse_args(args)?;
    if key.is_empty() { return Err("set BULLIONSTAR_KEY".into()); }
    let mode = cmd.mode();
    // The PIN is held in every mode, so the guard and the leak check look for it; only orders mode sends it.
    let live = recorder::Guard::new(Live::new(key.clone()), mode, pin.clone());
    let no_key = recorder::Guard::new(Live::new(String::new()), recorder::Mode::Probe, pin.clone());
    let wrong_key = recorder::Guard::new(Live::new(uuid::Uuid::new_v4().simple().to_string()), recorder::Mode::Probe, pin.clone());
    let out = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bullionstar"));
    let sc = recorder::Scrubber::random().with_names(recorder::KEEP_NAMES, recorder::TIME_NAMES);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let o = rt.block_on(recorder::record(&live, Some((&no_key, &wrong_key)), &cmd, &key, &pin, sc, &recorder::Opts::live(), out))?;
    let limits: Vec<String> = live.inner.limits.borrow().iter().map(|(k, v)| format!("{k}: {v}")).collect();
    Ok(format!("wrote {}\n\nobservations (not written; paste the F lines into the plan's findings):\n  {}\nF11 rate-limit headers seen: {}",
        o.dir, o.observations.join("\n  "), if limits.is_empty() { "none".into() } else { limits.join(", ") }))
}
