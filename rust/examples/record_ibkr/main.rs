//! Records IBKR's live answers, scrubbed, as test fixtures for A27-2..6 (PLAN-A27-1-ibkr-recorder.md). Read mode only
//! reads, on any account; paper mode also places, confirms and cancels orders, and refuses unless the login lists one
//! paper account (DU…/DF…). Run by hand, on your own login, never in a test or CI. The run book is README.md beside it.
//!
//!   cargo run --example record_ibkr -- read --us SYMBOL,CONID,ISIN --eu SYMBOL,CONID,ISIN
//!   cargo run --example record_ibkr -- paper --stock SYMBOL,CONID,ISIN --limit PRICE [--untradable CONID]
//!
//! The key material comes from the files in IBKR_RECORDER_DIR (outside the repository) only. Output:
//! rust/tests/fixtures/ibkr/<read|paper>/. It never runs git.
mod recorder;

use deltabadger::engine::model::prepared_credential_version;
use deltabadger::engine::SystemClock;
use deltabadger::venue::http::{client, ReqwestTransport};
use deltabadger::venue::ibkr::{Login, Session, ACCOUNTS_PATH, URL};
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(out) => {
            for (ids, text) in &out.prompts { eprintln!("prompt {}: {text}", ids.join(", ")); }
            println!("{}", out.report);
            ExitCode::SUCCESS
        }
        Err(e) => { eprintln!("record_ibkr: {e}"); ExitCode::FAILURE }
    }
}

/// The 401's shape: one GET of the account list with no credential at all (no Authorization, no redirects followed).
async fn unauthorized() -> Option<(u16, String)> {
    let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(30)).build().ok()?;
    let r = c.get(format!("{URL}{ACCOUNTS_PATH}")).send().await.ok()?;
    let status = r.status().as_u16();
    Some((status, r.text().await.ok()?))
}

fn run(args: Vec<String>) -> Result<recorder::Outcome, String> {
    recorder::refuse_key_args(&args)?;
    let cmd = recorder::parse_args(args)?;
    let dir = std::env::var("IBKR_RECORDER_DIR").map_err(|_| "set IBKR_RECORDER_DIR to the directory holding the key material (README.md)")?;
    let repo = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let material = recorder::material(std::path::Path::new(&dir), repo)?;
    let secrets = recorder::secrets(&material);
    let version = prepared_credential_version(0, &[Some("record_ibkr".into()), None, None, None, None, None, None]);
    let login = Arc::new(Login::new(material, version).map_err(|e| format!("the key material does not parse ({e:?})"))?);
    let mut sensitive = login.sensitive();
    sensitive.extend(secrets.iter().cloned());
    let guard = recorder::Guard::new(ReqwestTransport::ibkr(client(), login.clone()), cmd.mode, sensitive);
    let session = Session::new(&guard, login, &SystemClock);
    let out = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ibkr"));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let unauthorized = unauthorized().await;
        recorder::record(&session, &guard, &cmd, unauthorized, &secrets, &mut recorder::Scrubber::random_weeks(), out).await
    })
}
