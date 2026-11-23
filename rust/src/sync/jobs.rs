//! The syncs as jobs of the engine's scheduler (`crate::jobs`, Plan 2f): one ledger job and one balance
//! job per API key, `ledger_sync` and `balance_sync` with the key's id as 2f's scope, so each has its own record
//! (`app_configs` row `rust_job.ledger_sync:<api_key_id>`). The runner, not the job, writes that record and enforces
//! the deadline: a job only returns its `Outcome`. A run with nothing to import is `Done`, so a first success differs
//! from "never ran".
use super::balances;
use crate::engine::events::EngineEvent;
use crate::jobs::data_api::PriceSource;
use crate::jobs::schedule::{Jitter, Schedule};
use crate::jobs::{Cx, Db, Job, JobFuture, AttributedJobFuture, Outcome, Retry, Spec, Wake, DEADLINE};
use super::{balance_reading_keys, credentials_with_version, ledger, ledger_reading_keys, Failure, SyncError, ALPACA};
use crate::crypto::Credentials;
use crate::venue::alpaca::{AlpacaVenue, LiveFactory};
use crate::venue::http::{ReqwestTransport, Transport};
use rusqlite::Connection;
use std::rc::Rc;
use std::time::Duration;

pub const API_KEY_VALIDATOR: &str = "api_key_validator";
pub const LEDGER_SYNC: &str = "ledger_sync";
pub const BALANCE_SYNC: &str = "balance_sync";
/// A ledger sync's own deadline (2f R1 lets it declare one): a first sync reads an account's whole history, a page
/// of 100 per request. Past it the runner drops the run at an await; nothing is half written, and the next run
/// starts from the stored watermark.
pub const LEDGER_DEADLINE: Duration = Duration::from_secs(3600);

/// How a job reaches Alpaca for one key. `LiveFactory` is the real one (the key's own host, paper or live, as 2.x).
pub trait Connect {
    type T: Transport;
    fn connect(&self, credentials: &Credentials) -> AlpacaVenue<Self::T>;
    /// How a balance sync reaches Binance or Binance.US for one key: through `proxy` when one is configured
    /// (ExchangeProxy.for), never around it. By default nothing is sent.
    fn binance_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("Binance is not connected here")
    }
    /// Where the tracker loads the ECB's rates from (Tax::EcbFxRates.ensure_loaded!, whatever the market-data provider):
    /// the network for the live factory; by default nowhere, so a test reaches no public host.
    fn ecb(&self) -> Option<crate::tracker::fx::Source> { None }
    /// The venues' own prices for a balance sync's holdings the provider left unpriced (D-VENUE-PRICE-FALLBACK): the
    /// live factory's read every venue's public market data; by default none, so a test reaches no public host.
    fn venue_quotes(&self) -> Option<Box<dyn crate::sync::venue_quotes::VenueQuotes>> { None }
    /// The host a Binance or Binance.US key's reads go to (`venue::binance::host`); a test points it at a local server.
    fn binance_base(&self, exchange_type: &str) -> Option<String> {
        crate::venue::binance::host(exchange_type).map(|(base, _)| base.to_string())
    }
    /// How far apart a Binance ledger read's calls go; a test against a local server waits for nothing.
    fn binance_pacing(&self) -> crate::sync::binance_ledger::Pacing { crate::sync::binance_ledger::Pacing::LIVE }
    /// How a KuCoin key's syncs reach KuCoin (Honeymaker.client('kucoin', passphrase:, proxy: ExchangeProxy.for('kucoin'))).
    /// By default nothing is sent.
    fn kucoin_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("KuCoin is not connected here")
    }
    /// How a Bitvavo key's syncs reach Bitvavo (Honeymaker.client('bitvavo', proxy: ExchangeProxy.for('bitvavo'))). By default
    /// nothing is sent.
    fn bitvavo_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("Bitvavo is not connected here")
    }
    /// How a MEXC key's syncs reach MEXC (Honeymaker.client('mexc', proxy: ExchangeProxy.for('mexc'))). By default nothing is
    /// sent.
    fn mexc_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("MEXC is not connected here")
    }
    fn coinbase_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("Coinbase is not connected here")
    }
    /// How far apart a Bitvavo ledger read's calls go (Exchanges::Bitvavo::LEDGER_READ_PAUSE); a test waits for nothing.
    fn bitvavo_pause(&self) -> Duration { crate::sync::bitvavo_ledger::READ_PAUSE }
    /// How a Gemini key's syncs reach Gemini (Honeymaker.client('gemini', proxy: ExchangeProxy.for('gemini'))). By default
    /// nothing is sent.
    fn gemini_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("Gemini is not connected here")
    }
    /// How a Bitget key's syncs reach Bitget (Honeymaker.client('bitget', passphrase:, proxy: ExchangeProxy.for('bitget'))).
    /// By default nothing is sent.
    fn bitget_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("Bitget is not connected here")
    }
    /// How a Hyperliquid key's syncs reach Hyperliquid (Honeymaker.client('hyperliquid', proxy: ExchangeProxy.for('hyperliquid'))).
    /// By default nothing is sent.
    fn hyperliquid_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("Hyperliquid is not connected here")
    }
    fn bybit_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("Bybit is not connected here")
    }
    fn bitrue_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("Bitrue is not connected here")
    }
    fn bingx_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("BingX is not connected here")
    }
    /// How a Kraken key's syncs reach Kraken (Honeymaker.client('kraken', proxy: ExchangeProxy.for('kraken'))). By default
    /// nothing is sent.
    fn kraken_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("Kraken is not connected here")
    }
    /// How an IBKR key's syncs reach IBKR (through `login`'s session, signed at send time). By default nothing is sent.
    fn ibkr_transport(&self, _login: std::sync::Arc<crate::venue::ibkr::Login>, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("IBKR is not connected here")
    }
    /// Where and how the validator sends a Kraken, Binance or KuCoin key's check (`key_check`): the venue's host and a client
    /// through its proxy, given the stored AppConfig `proxy_<name>` (decrypted). By default nowhere.
    fn key_check_egress(&self, exchange_type: &str, _stored_proxy: Option<String>) -> Result<(String, reqwest::Client), String> {
        Err(format!("{exchange_type} is not connected here"))
    }
}
impl Connect for LiveFactory {
    /// ExchangeProxy.for(name): the stored proxy, else ENV `PROXY_<NAME>`; never around a configured one.
    fn key_check_egress(&self, exchange_type: &str, stored_proxy: Option<String>) -> Result<(String, reqwest::Client), String> {
        let Some(name) = crate::venue::registry::key_check_proxy(exchange_type) else { return Err(format!("{exchange_type} is not connected here")) };
        let base = crate::venue::registry::row(exchange_type).expect("a checked venue has a row").base;
        let client = match crate::venue::http::proxy_for(name, stored_proxy, &|k| std::env::var(k).ok()) {
            None => crate::venue::http::client(),
            Some(proxy) => crate::venue::http::client_via(&proxy).map_err(|e| format!("{e} ({name})"))?,
        };
        Ok((base.into(), client))
    }
    fn ecb(&self) -> Option<crate::tracker::fx::Source> { Some(crate::tracker::fx::Source::Live(crate::tracker::fx::Plain::default())) }
    fn venue_quotes(&self) -> Option<Box<dyn crate::sync::venue_quotes::VenueQuotes>> { Some(Box::new(crate::sync::venue_quotes::Quotes::live())) }
    type T = ReqwestTransport;
    fn connect(&self, credentials: &Credentials) -> AlpacaVenue<ReqwestTransport> { self.alpaca(Some(credentials.clone()), Vec::new()) }
    fn binance_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::binance(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(credentials.redaction_values.clone())
    }
    fn kucoin_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::kucoin(client, credentials.key.clone(), credentials.secret.clone(), credentials.passphrase.clone().unwrap_or_default())
            .with_sensitive_values(credentials.redaction_values.clone())
    }
    fn bitvavo_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::bitvavo(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(credentials.redaction_values.clone())
    }
    fn gemini_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::gemini(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(credentials.redaction_values.clone())
    }
    fn bitget_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::bitget(client, credentials.key.clone(), credentials.secret.clone(), credentials.passphrase.clone().unwrap_or_default())
            .with_sensitive_values(credentials.redaction_values.clone())
    }
    fn mexc_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::mexc(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(credentials.redaction_values.clone())
    }
    fn coinbase_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::coinbase(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(credentials.redaction_values.clone())
    }
    fn hyperliquid_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::hyperliquid(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(credentials.redaction_values.clone())
    }
    fn bybit_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::bybit(client, credentials.key.clone(), credentials.secret.clone())
            .with_sensitive_values(credentials.redaction_values.clone())
    }
    fn bitrue_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::bitrue(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(credentials.redaction_values.clone())
    }
    fn bingx_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::bingx(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(credentials.redaction_values.clone())
    }
    fn kraken_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::new(client, credentials.key.clone(), credentials.secret.clone()).kraken().with_sensitive_values(credentials.redaction_values.clone())
    }
    fn ibkr_transport(&self, login: std::sync::Arc<crate::venue::ibkr::Login>, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::ibkr(client, login)
    }
}

/// The ExchangeProxy name of a venue whose syncs go through its proxy: every synced venue but Alpaca (2.x has no
/// PROXY_ALPACA).
fn proxied(venue: &str) -> Option<&'static str> {
    use crate::venue::registry::{self, BalanceSync, LedgerSync};
    registry::row(venue).filter(|r| r.balances.is_some_and(|b| b != BalanceSync::Alpaca) || r.ledger.is_some_and(|l| l != LedgerSync::Alpaca)).map(|r| r.name_id)
}

/// The venue type of a key and, for Binance, Binance.US, KuCoin, Bitvavo, Gemini, Bitget, MEXC, Hyperliquid, Bybit, Bitrue, Coinbase and BingX, the proxy its reads go through: AppConfig
/// `proxy_<name>` (decrypted) or ENV `PROXY_<NAME>`, as ExchangeProxy.for resolves it.
pub(crate) async fn venue_of(db: &Db, key: i64) -> Result<(String, Option<String>), SyncError> {
    let (venue, stored) = db.run(move |c, cipher| {
        let venue: String = c.query_row("SELECT e.type FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id WHERE k.id = ?1", [key], |r| r.get(0)).map_err(|e| e.to_string())?;
        let Some(name) = proxied(&venue) else { return Ok((venue, None)) };
        let stored = crate::app_config::get(c, cipher, &format!("proxy_{name}"))?;
        Ok((venue, stored))
    }).await.map_err(SyncError)?;
    let proxy = proxied(&venue).and_then(|name| crate::venue::http::proxy_for(name, stored, &|k| std::env::var(k).ok()));
    Ok((venue, proxy))
}

async fn key_credentials(db: &Db, key: i64) -> Result<(Credentials, crate::engine::model::CredentialVersion), SyncError> {
    db.run(move |c, cipher| credentials_with_version(c, cipher, key).map_err(|e| e.0)).await.map_err(SyncError)
}

/// `Done` is a sync that reached the end of what the venue has, with or without anything to import: its
/// `last_success_at` is what tells a key's first successful sync from "never ran" (Plan 2d). `NothingNew` is an
/// import that stopped at its page cap with more to read: the run went well and the ledger is not yet whole, so the
/// runner records the run and no success, and the next run continues. Never `Transient`: 2.x's jobs declare no retry.
fn outcome<T>(done: Result<Result<T, Failure>, SyncError>, complete: impl Fn(&T) -> bool) -> Outcome {
    match done {
        Ok(Ok(done)) if !complete(&done) => Outcome::NothingNew,
        Ok(Ok(_)) => Outcome::Done,
        Ok(Err(failure)) => Outcome::Failed(failure.error),
        Err(SyncError(e)) if e == crate::engine::model::CREDENTIALS_CHANGED => Outcome::NothingNew,
        Err(SyncError(e)) => Outcome::Failed(e),
    }
}

/// One run of a job outside the scheduler (the hand-run command), held to the deadline the scheduler's runner would
/// hold it to: past `Spec::deadline` the run is dropped at its next await and reported in the runner's words.
pub async fn run_within_deadline(job:&dyn Job,cx:Cx<'_>,wakes:Vec<Wake>)->Outcome {
    run_within_deadline_attributed(job,cx,wakes).await.value
}
pub async fn run_within_deadline_attributed(job:&dyn Job,cx:Cx<'_>,wakes:Vec<Wake>)->crate::engine::model::Produced<Outcome> {
    let spec=job.spec();let deadline=spec.deadline;
    let db=cx.db.clone();
    let owner=db.completion_owner(spec.name,spec.scope.as_deref()).await;
    match tokio::time::timeout(deadline,job.run_attributed(cx,wakes)).await {
        Ok(result)=>result,
        Err(_)=>{
            db.deadline_completion(owner,spec.name,spec.scope.as_deref()).await;
            crate::engine::model::Produced::new(Outcome::Failed(format!("dropped past its {}s deadline",deadline.as_secs())),None)
        },
    }
}

/// `ledger_sync`, scoped by the key: 2.x's `AccountTransaction::SyncJob` for one key. It runs nightly at 02:00 UTC
/// (`AccountTransaction::SyncAllJob`'s fan-out, recurring key `sync_all_account_transactions_job`) and after every
/// order row the engine records (`Transaction`'s after_create_commit). No retry: 2.x's job declares none, and the
/// next order or the next night is the retry. An import longer than one run wakes itself and goes on at once.
pub struct LedgerSync<C: Connect> { venues: C, key_id: i64, limits: ledger::Limits }

impl<C: Connect> LedgerSync<C> {
    pub fn new(venues: C, key_id: i64) -> Self { Self { venues, key_id, limits: ledger::Limits::RUN } }
    /// The same job with a run of fewer pages (tests: an import that takes several runs).
    pub fn within(self, limits: ledger::Limits) -> Self { Self { limits, ..self } }
}

impl<C: Connect> Job for LedgerSync<C> {
    fn spec(&self) -> Spec {
        Spec { name: LEDGER_SYNC, scope: Some(self.key_id.to_string()), schedule: Some(Schedule::Daily { hour: 2, minute: 0 }), jitter: Jitter::NONE, retry: Retry::None,
               deadline: LEDGER_DEADLINE }
    }
    /// Every order, whatever its status and whichever bot placed it, and every terminal sell fill: an event names a bot, and which key that is
    /// takes the database. On an install with several keys each key's job runs and the others find nothing new.
    fn wants(&self, event: &EngineEvent) -> bool { matches!(event, EngineEvent::OrderRecorded { .. } | EngineEvent::SaleSettled { .. }) }
    /// Whatever woke it (the schedule, a manual wake, any number of orders), a run is one sync of this key.
    fn run<'a>(&'a self,cx:Cx<'a>,wakes:Vec<Wake>)->JobFuture<'a>{ Box::pin(async move{self.run_attributed(cx,wakes).await.value}) }
    fn run_attributed<'a>(&'a self,cx:Cx<'a>,_wakes:Vec<Wake>)->AttributedJobFuture<'a>{
        Box::pin(async move {
            let key_id = self.key_id;
            let owner = match cx.db.run(move |c, _| c.query_row("SELECT user_id FROM api_keys WHERE id=?1", [key_id], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())).await {
                Ok(owner) => owner,
                Err(e) => return crate::engine::model::Produced::new(Outcome::Failed(e),None),
            };
            let completion=async {
            // #710 7a rule P7: only the account's reader syncs its ledger; it captures the reader generation here, and every
            // fenced commit of the run checks it (N7). Another key's job does nothing but wake the reader.
            let entry = match reader_entry(&cx, key_id, LEDGER_SYNC).await { Ok(Some(entry)) => entry, Ok(None) => return crate::engine::model::Produced::new(Outcome::NothingNew,None),
                Err(e) => return crate::engine::model::Produced::new(Outcome::Failed(e),None) };
            let (credentials, version) = match key_credentials(&cx.db, self.key_id).await { Ok(c) => c, Err(SyncError(e)) => return crate::engine::model::Produced::new(Outcome::Failed(e),None) };
            // A capture inside the job (each adapter re-reads its key) must be on the account captured here (#710 §6.4).
            let result=crate::sync::as_reader(entry, crate::sync::bound(version.account(), async {
            let key = self.key_id.to_string();
            let at = cx.clock.now();
            // Marked only under the captured version (#710 §6.4): a run whose key moved account is discarded before it marks.
            let captured = version.clone();
            match cx.db.run(move |c, _| {
                let tx = crate::engine::model::immediate(c).map_err(|e| format!("{e:?}"))?;
                if !crate::engine::model::key_is_current(&tx, &captured).map_err(|e| format!("{e:?}"))? { return Ok(false); }
                if let Some(entry) = &entry { if !crate::sync::reader_holds(&tx, entry).map_err(|e| e.0)? { return Ok(false); } }
                crate::jobs::state::mark_incomplete(&tx, LEDGER_SYNC, Some(&key), at)?;
                tx.commit().map_err(|e| e.to_string())?;
                Ok(true)
            }).await { Ok(true) => {}, Ok(false) => return Outcome::NothingNew, Err(e) => return Outcome::Failed(e) }
            use crate::venue::registry::{self, LedgerSync};
            let done = match venue_of(&cx.db, self.key_id).await {
                Err(e) => Err(e),
                Ok((venue, proxy)) => match registry::row(&venue).and_then(|r| r.ledger) {
                    Some(LedgerSync::Binance) =>
                        crate::sync::binance_ledger::sync_captured(&cx.db, self.venues.binance_transport(&credentials, proxy.as_deref()),
                                                                   &self.venues.binance_base(&venue).unwrap_or_default(), self.key_id, &credentials, cx.clock,
                                                                   self.venues.binance_pacing(), &version).await,
                    Some(LedgerSync::Kucoin) =>
                        crate::sync::kucoin_ledger::sync(&cx.db, self.venues.kucoin_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock,
                                                         crate::sync::kucoin_ledger::THROTTLE_WAIT).await,
                    Some(LedgerSync::Bitvavo) =>
                        crate::sync::bitvavo_ledger::sync(&cx.db, self.venues.bitvavo_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock,
                                                          self.venues.bitvavo_pause()).await,
                    Some(LedgerSync::Gemini) =>
                        crate::sync::gemini_ledger::sync(&cx.db, self.venues.gemini_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock,
                                                         crate::sync::gemini_ledger::TRANSFERS_PAUSE).await,
                    Some(LedgerSync::Bitget) =>
                        crate::sync::bitget_ledger::sync(&cx.db, self.venues.bitget_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock).await,
                    Some(LedgerSync::Mexc) =>
                        crate::sync::mexc_ledger::sync(&cx.db, self.venues.mexc_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock).await,
                    Some(LedgerSync::Coinbase) =>
                        crate::sync::coinbase_ledger::sync(&cx.db, self.venues.coinbase_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock).await,
                    Some(LedgerSync::Hyperliquid) =>
                        crate::sync::hyperliquid_ledger::sync(&cx.db, self.venues.hyperliquid_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock).await,
                    Some(LedgerSync::Bybit) =>
                        crate::sync::bybit_ledger::sync(&cx.db, self.venues.bybit_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock).await,
                    Some(LedgerSync::Bitrue) =>
                        crate::sync::bitrue_ledger::sync(&cx.db, self.venues.bitrue_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock, crate::sync::bitrue_ledger::READ_PAUSE).await,
                    Some(LedgerSync::Kraken) =>
                        crate::sync::kraken_ledger::sync(&cx.db, self.venues.kraken_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock,
                                                         crate::sync::kraken_ledger::THROTTLE_WAIT).await,
                    Some(LedgerSync::Bingx) =>
                        crate::sync::bingx_ledger::sync(&cx.db, self.venues.bingx_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock).await,
                    // Alpaca's, and a key on any other venue: ledger::load_key refuses it as it always did.
                    Some(LedgerSync::Alpaca) | None => ledger::sync_captured(&cx.db, &self.venues.connect(&credentials), self.key_id, &credentials, cx.clock, self.limits, &version).await,
                },
            };
            let outcome = outcome(done, |out| out.complete);
            // #710: a complete import may enter or end its exchange's duplicate standing (identity::reconsider).
            if outcome == Outcome::Done {
                let (key, at) = (self.key_id, cx.clock.now());
                if let Err(e) = cx.db.run(move |c, _| crate::identity::reconsider_for_key(c, key, at).map_err(|e| e.to_string())).await {
                    crate::engine::log(&format!("[sync] standing of key {key} not reconsidered: {e}"));
                }
            }
            // S-7.7: an import that stopped at its page cap goes on as soon as the runner is free, as 2.x's one job reads the
            // whole history at once; other due jobs run in between.
            // D-SYNC-DONE-TOAST: this key's part of a page's import is in, before the walk it wakes looks (progress::take).
            if outcome == Outcome::Done {
                let (key, at) = (self.key_id, cx.clock.now());
                // Unrecorded, no walk would ever end the toast: it goes now, without its success message.
                if let Err(e) = cx.db.run(move |c, _| progress::landed(c, owner, key, at)).await {
                    crate::engine::log(&format!("[sync] import progress of key {key} not recorded: {e}"));
                    let _ = cx.db.run(move |c, _| progress::clear(c, owner)).await;
                    cx.db.notifications.sync_done(owner);
                }
            }
            if outcome == Outcome::NothingNew { cx.wakers.wake(LEDGER_SYNC, Some(&self.key_id.to_string()), None); }
            // 2.x's SyncJob ends in a Tracker::LedgerJob: the user's tracker walks again once the ledger is whole.
            else if let Err(e)=crate::tracker::jobs::wake_for_key(&cx,self.key_id).await {return Outcome::Failed(e);}
            outcome

            })).await;
            crate::engine::model::Produced::new(result,Some(version))
            }.await;
            // D-SYNC-DONE-TOAST: a failed import's toast goes now; a complete one's waits for the tracker walk it woke (LedgerWalk),
            // so it says "imported" once the page shows the new numbers.
            if !matches!(completion.value, Outcome::NothingNew | Outcome::Done) {
                if let Err(e) = cx.db.run(move |c, _| progress::clear(c, owner)).await { crate::engine::log(&format!("[sync] import progress of user {owner} not cleared: {e}")); }
                cx.db.notifications.sync_done(owner);
            }
            completion
        })
    }
}

/// Rule P7: the job's entry, in one read: whether `key` is its account's reader and in which generation (`Some(Some)`:
/// the run is bound to it). A key whose account another key reads wakes that reader's `job` (a permission fallback or a
/// fence moved it), and its own run does nothing (`None`). An account no key can read runs unbound (`Some(None)`), as
/// before: the venue sync refuses a key that is not a reading key itself, and a named hand run syncs it whatever it is.
async fn reader_entry(cx: &Cx<'_>, key: i64, job: &'static str) -> Result<Option<Option<crate::sync::Reader>>, String> {
    let (entry, reader) = cx.db.run(move |c, _| {
        let entry = crate::sync::reader_entry(c, key).map_err(|e| e.0)?;
        let reader = match (&entry, crate::identity::Account::of_key(c, key).map_err(|e| e.to_string())?) {
            (None, Some(account)) => crate::sync::reader_of(c, &account.keys("k")).map_err(|e| e.0)?,
            _ => None,
        };
        Ok((entry, reader))
    }).await?;
    match (entry, reader) {
        (Some(entry), _) => Ok(Some(Some(entry))),
        (None, Some(r)) => { cx.wakers.wake(job, Some(&r.to_string()), None); Ok(None) }
        (None, None) => Ok(Some(None)),
    }
}

/// A balance read whose key stopped being its account's reading key while it ran (`superseded`): nothing it read is
/// committed, and the run ends as NothingNew under its version.
pub const SUPERSEDED: &str = "superseded balance reader";
/// Whether key `key_id` no longer reads its account's balances: another key of the same account is its reading key
/// (sync::balance_reading_keys). An account none of whose keys reads keeps this key reading.
pub(crate) fn superseded(c: &rusqlite::Connection, key_id: i64) -> Result<Option<i64>, SyncError> {
    let keys = crate::sync::balance_reading_keys(c)?;
    if keys.contains(&key_id) { return Ok(None); }
    let Some(account) = crate::identity::Account::of_key(c, key_id)? else { return Ok(None) };
    for k in keys { if crate::identity::Account::of_key(c, k)? == Some(account) { return Ok(Some(k)); } }
    Ok(None)
}

/// `balance_sync`, scoped by the key: 2.x's `AccountBalance::SyncJob` for one key. It runs nightly at 02:30 UTC
/// (`AccountBalance::SyncAllJob`'s fan-out, recurring key `sync_all_account_balances_job`), after the ledger, or on a
/// manual wake. No retry. The price source is the scheduler's one data-api client, shared (2f S-10).
pub struct BalanceSync<C: Connect> { venues: C, prices: Rc<dyn PriceSource>, key_id: i64 }

impl<C: Connect> BalanceSync<C> {
    pub fn new(venues: C, prices: Rc<dyn PriceSource>, key_id: i64) -> Self { Self { venues, prices, key_id } }
}

impl<C: Connect> Job for BalanceSync<C> {
    fn spec(&self) -> Spec {
        Spec { name: BALANCE_SYNC, scope: Some(self.key_id.to_string()), schedule: Some(Schedule::Daily { hour: 2, minute: 30 }), jitter: Jitter::NONE, retry: Retry::None,
               deadline: DEADLINE }
    }
    fn run<'a>(&'a self,cx:Cx<'a>,wakes:Vec<Wake>)->JobFuture<'a>{ Box::pin(async move{self.run_attributed(cx,wakes).await.value}) }
    fn run_attributed<'a>(&'a self,cx:Cx<'a>,_wakes:Vec<Wake>)->AttributedJobFuture<'a>{
        Box::pin(async move {
            // #710 Merge: a key moved beside its new account's reading key keeps its registered daily run; only the reading key
            // writes the account's balances and their provenance (the freshness check names it).
            let key_id = self.key_id;
            // A key whose account reads with another key gives way to it; one whose account has no reading key runs as before.
            let gives_way = cx.db.run(move |c, _| superseded(c, key_id).map_err(|e| e.0)).await;
            let (credentials, version) = match key_credentials(&cx.db, self.key_id).await { Ok(c) => c, Err(SyncError(e)) => return crate::engine::model::Produced::new(Outcome::Failed(e),None) };
            // #710 7a rule P7: an account none of whose keys can read (fenced, or unread on a two-account exchange) is
            // read by none of them; the reader captures its generation (N7).
            let entry = match reader_entry(&cx, key_id, BALANCE_SYNC).await { Ok(Some(entry)) => entry, Ok(None) if matches!(gives_way, Ok(None)) => return crate::engine::model::Produced::new(Outcome::NothingNew,None),
                Ok(None) => None, Err(e) => return crate::engine::model::Produced::new(Outcome::Failed(e),None) };
            match gives_way {
                Ok(None) => {}
                // A run that read nothing, under the key's current version: the scheduler records it, nothing wakes it again.
                // The reader's own job reads now (registered by the wake when the reader changed since the start).
                Ok(Some(reader)) => {
                    cx.wakers.wake(BALANCE_SYNC, Some(&reader.to_string()), None);
                    return crate::engine::model::Produced::new(Outcome::NothingNew, Some(version));
                }
                Err(e) => return crate::engine::model::Produced::new(Outcome::Failed(e), None),
            }
            let result=crate::sync::as_reader(entry, async {
            // As the ledger: from before the first write unit until a complete success, the key's balances may be half
            // written (a run dropped, or a hand run stopped, between two units); a start finding the mark runs the job at once.
            let (key, at) = (self.key_id.to_string(), cx.clock.now());
            // Marked only under the captured version (#710 §6.4): a run whose key moved account is discarded before it marks.
            let (captured, reader) = (version.clone(), crate::sync::current_reader());
            match cx.db.run(move |c, _| {
                let tx = crate::engine::model::immediate(c).map_err(|e| format!("{e:?}"))?;
                if !crate::engine::model::key_is_current(&tx, &captured).map_err(|e| format!("{e:?}"))? { return Ok(false); }
                if let Some(r) = &reader { if !crate::sync::reader_holds(&tx, r).map_err(|e| e.0)? { return Ok(false); } }
                crate::jobs::state::mark_incomplete(&tx, BALANCE_SYNC, Some(&key), at)?;
                tx.commit().map_err(|e| e.to_string())?;
                Ok(true)
            }).await { Ok(true) => {}, Ok(false) => return Outcome::NothingNew, Err(e) => return Outcome::Failed(e) }
            // D-LOW-FUNDS-STATUS: the balances stop being evidence from here; the status bars that read them redraw.
            cx.db.notifications.balances_changed(self.key_id);
            use crate::venue::registry::{self, BalanceSync};
            let done = match venue_of(&cx.db, self.key_id).await {
                Err(e) => Err(e),
                Ok((venue, proxy)) => match registry::row(&venue).and_then(|r| r.balances) {
                    Some(BalanceSync::Binance) =>
                        balances::sync_binance_captured(&cx.db, self.venues.binance_transport(&credentials, proxy.as_deref()), &|t| self.venues.binance_base(t), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Kucoin) =>
                        balances::sync_kucoin_captured(&cx.db, self.venues.kucoin_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Bitvavo) =>
                        balances::sync_bitvavo_captured(&cx.db, self.venues.bitvavo_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Gemini) =>
                        balances::sync_gemini_captured(&cx.db, self.venues.gemini_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Bitget) =>
                        balances::sync_bitget_captured(&cx.db, self.venues.bitget_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Mexc) =>
                        balances::sync_mexc_captured(&cx.db, self.venues.mexc_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Coinbase) =>
                        balances::sync_coinbase_captured(&cx.db, self.venues.coinbase_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Hyperliquid) =>
                        balances::sync_hyperliquid_captured(&cx.db, self.venues.hyperliquid_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Bybit) =>
                        balances::sync_bybit_captured(&cx.db, self.venues.bybit_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Bitrue) =>
                        balances::sync_bitrue_captured(&cx.db, self.venues.bitrue_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Kraken) =>
                        balances::sync_kraken_captured(&cx.db, self.venues.kraken_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    Some(BalanceSync::Bingx) =>
                        balances::sync_bingx_captured(&cx.db, self.venues.bingx_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    // The key's process-wide login (its LST and brokerage session kept across runs).
                    Some(BalanceSync::Ibkr) => match credentials.ibkr.as_deref().map(|m| crate::venue::ibkr::login(&version, m)) {
                        Some(Ok(login)) => balances::sync_ibkr_captured(&cx.db, self.venues.ibkr_transport(login.clone(), proxy.as_deref()), login, self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                        _ => Err(SyncError(format!("api key {} has no usable IBKR session material", self.key_id))),
                    },
                    // Alpaca's, and a key on any other venue: the Alpaca sync's load_key refuses it as it always did.
                    Some(BalanceSync::Alpaca) | None => balances::sync_captured(&cx.db, &self.venues.connect(&credentials), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                },
            };
            // Its key stopped reading while it ran: nothing was committed, and the reader's own job reads.
            let outcome = match &done { Err(SyncError(e)) if e == SUPERSEDED => Outcome::NothingNew, _ => outcome(done, |_| true) };
            if outcome == Outcome::NothingNew {
                let key_id = self.key_id;
                if let Ok(Some(reader)) = cx.db.run(move |c, _| superseded(c, key_id).map_err(|e| e.0)).await {
                    cx.wakers.wake(BALANCE_SYNC, Some(&reader.to_string()), None);
                    return outcome;
                }
            }
            // #710: a pair that may be one venue account keeps the verdict of its comparable reads; a partner whose read is too
            // far off is read now, so the two stay comparable.
            if outcome == Outcome::Done {
                let (key, at) = (self.key_id, cx.clock.now());
                match cx.db.run(move |c, _| crate::identity::after_balance_read(c, key, at).map_err(|e| e.to_string())).await {
                    Ok(partners) => for partner in partners { cx.wakers.wake(BALANCE_SYNC, Some(&partner.to_string()), None); },
                    Err(e) => crate::engine::log(&format!("[sync] standing of key {key} not reconsidered: {e}")),
                }
            }
            // 2.x's AccountBalance::SyncJob ends in PortfolioSnapshot.record!, whatever the sync did: the user's tracker
            // rewrites today's rows.
            if outcome == Outcome::NothingNew { cx.wakers.wake(BALANCE_SYNC, Some(&self.key_id.to_string()), None); }
            else if let Err(e)=crate::tracker::jobs::wake_for_key(&cx,self.key_id).await {return Outcome::Failed(e);}
            outcome

            }).await;
            crate::engine::model::Produced::new(result,Some(version))
        })
    }
}

/// The jobs of an install, for the scheduler's start: per reading Alpaca, Binance, Binance.US, KuCoin, Bitvavo or MEXC key (`ApiKey.reading`,
/// what 2.x's nightly jobs iterate) one ledger job and one balance job, every ledger job before every balance job, so jobs due together run in 2.x's order. New scopes are registered by the same constructors on their first wake.
pub fn register<C: Connect + Clone + 'static>(c: &Connection, venues: &C, prices: Rc<dyn PriceSource>) -> Result<Vec<Box<dyn Job>>, SyncError> {
    let mut jobs: Vec<Box<dyn Job>> = vec![];
    for key in ledger_reading_keys(c)? { jobs.push(Box::new(LedgerSync::new(venues.clone(), key))); }
    for key in balance_reading_keys(c)? { jobs.push(Box::new(BalanceSync::new(venues.clone(), prices.clone(), key))); }
    jobs.push(Box::new(ScopeFactory { venues: venues.clone(), prices }));
    Ok(jobs)
}

struct ScopeFactory<C: Connect> { venues:C, prices:Rc<dyn PriceSource> }
impl<C: Connect + Clone + 'static> Job for ScopeFactory<C> {
    fn manual(&self)->bool { false }
    fn spec(&self)->Spec { Spec{name:"credential_scope_factory",scope:None,schedule:None,jitter:Jitter::NONE,retry:Retry::None,deadline:DEADLINE} }
    fn spawn(&self,name:&str,scope:Option<&str>)->Option<Box<dyn Job>> {
        let id=scope?.parse::<i64>().ok().filter(|id|*id>0)?;
        match name {
            API_KEY_VALIDATOR => Some(Box::new(Validator{venues:self.venues.clone(),key_id:id})),
            LEDGER_SYNC => Some(Box::new(LedgerSync::new(self.venues.clone(),id))),
            BALANCE_SYNC => Some(Box::new(BalanceSync::new(self.venues.clone(),self.prices.clone(),id))),
            _ => None,
        }
    }
    fn run<'a>(&'a self,_cx:Cx<'a>,_wakes:Vec<Wake>)->JobFuture<'a> {Box::pin(async {Outcome::Failed("scope factory cannot run as a job".into())})}
}

/// 2.x ApiKeyValidatorJob: validate a committed credential, then enqueue its trading balance sync.
pub struct Validator<C:Connect>{pub venues:C,pub key_id:i64}
impl<C:Connect> Job for Validator<C>{
    fn spec(&self)->Spec{Spec{name:API_KEY_VALIDATOR,scope:Some(self.key_id.to_string()),schedule:None,jitter:Jitter::NONE,retry:Retry::None,deadline:DEADLINE}}
    fn run<'a>(&'a self,cx:Cx<'a>,wakes:Vec<Wake>)->JobFuture<'a>{ Box::pin(async move{self.run_attributed(cx,wakes).await.value}) }
    fn run_attributed<'a>(&'a self,cx:Cx<'a>,_wakes:Vec<Wake>)->AttributedJobFuture<'a>{
        Box::pin(async move {
        let id=self.key_id;
        let (credentials,version,values)=match cx.db.run(move|c,cipher|super::validation_material(c,cipher,id).map_err(|e|e.0)).await{Ok(v)=>v,Err(e)=>return crate::engine::model::Produced::new(Outcome::Failed(e),None)};
        let handle=crate::web::settings::validator::ValidationHandle::stored(credentials,version);
        let before_result=|value|crate::engine::model::Produced::new(value,Some(handle.producer().clone()));
        let (exchange,name)=match cx.db.run(move|c,_|c.query_row("SELECT e.type,e.name FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?.unwrap_or_default()))).map_err(|_|"cannot read credential exchange".into())).await{Ok(e)=>e,Err(e)=>return before_result(Outcome::Failed(e))};
        let checked=crate::venue::registry::key_check_proxy(&exchange);
        if exchange!=ALPACA&&checked.is_none(){return before_result(Outcome::Failed("credential validation unavailable for this exchange".into()))}
        let kind=match cx.db.run(move|c,_|c.query_row("SELECT key_type FROM api_keys WHERE id=?1",[id],|r|r.get::<_,i64>(0)).map_err(|_|"cannot read credential capability".into())).await{Ok(k)=>k,Err(e)=>return before_result(Outcome::Failed(e))};
        // ApiKey#get_validity: Alpaca's account read, or the venue's own key check through its proxy (ExchangeProxy.for).
        let (result,identity)=if let Some(proxy)=checked {
            let stored=match cx.db.run(move|c,cipher|crate::app_config::get(c,cipher,&format!("proxy_{proxy}")).map_err(|_|format!("the {proxy} proxy setting is unreadable"))).await{Ok(s)=>s,Err(e)=>return before_result(Outcome::Failed(e))};
            let egress=self.venues.key_check_egress(&exchange,stored);
            match egress {
                Ok((base,client))=>handle.check_via(&exchange,&base,Ok(client),kind).await,
                Err(why)=>handle.check_via(&exchange,"",Err(why),kind).await,
            }
        } else {
            let venue=self.venues.connect(handle.credentials());
            match handle.check(&venue.urls().trading,kind).await{Ok(v)=>(v,None),Err(_)=>return before_result(Outcome::Failed("credential validation unavailable".into()))}
        };
        use crate::web::settings::validator::Validity;
        // ApiKey#update_status!: a permission problem is incorrect; a failure leaves the key pending, its reason logged.
        let account=match &result.value{Validity::Account(account)=>Some(account.clone()),_=>None};
        let status=result.map(|result|match result{Validity::Correct|Validity::Account(_)=>(1,None),Validity::PendingActivation=>(3,None),
            Validity::Incorrect=>{crate::engine::log(&format!("[{name}] API key validation: incorrect key"));(2,Some("incorrect key".to_string()))},
            Validity::Permissions{..}=>(2,Some("incorrect key".to_string())),
            Validity::Pending(e)=>{let safe=crate::web::settings::keys::scrub(&e.log_text(),&values);crate::engine::log(&format!("[{name}] API key validation failed: {safe}"));(0,Some(safe))}});
        // D-IBKR-INCONCLUSIVE: an IBKR check with no verdict (IBKR down, a timeout, a busy login) writes nothing. 2.x writes
        // pending_validation, and nothing would validate the key again: it would leave every sync and the startup validation.
        if exchange==crate::venue::ibkr::IBKR && status.value.0==0 { return crate::engine::model::Produced::new(Outcome::Failed(status.value.1.clone().unwrap_or_default()),status.origin().clone()); }
        let origin=status.origin().clone();
        let Some(version)=origin.clone() else{return crate::engine::model::Produced::new(Outcome::NothingNew,None)};
        let error=status.value.1.clone();
        // ApiKeyValidator: only a correct key's balances are synced (a key awaiting IBKR activation waits, unlogged).
        let correct=status.value.0==1;
        let status=status.map(|(status,_)|status);
        let outcome=async {
        let at=cx.clock.now();
        let (credentials,version_read)=(handle.credentials().clone(),handle.producer().clone());
        if let Err(SyncError(e))=super::commit_record_for(&cx.db,&version,move|c,cipher|{
            let status=crate::web::settings::keys::bound_status(c,cipher,id,status.clone(),account.as_deref(),at)?;
            crate::web::settings::keys::store_status(c,&status,id,at)?;
            if let Some(observed)=&identity{crate::identity::record(c,cipher,id,&credentials,&version_read,observed,at).map_err(SyncError)?;}
            Ok(((),true))
        }).await{
            if e==crate::engine::model::CREDENTIALS_CHANGED{cx.wakers.wake(API_KEY_VALIDATOR,Some(&id.to_string()),None);return Outcome::NothingNew;}
            return Outcome::Failed(e)
        }
        cx.wakers.wake_engine();
        if let Some(error)=error {return Outcome::Failed(error)}
        if kind==0 && correct {cx.wakers.wake(BALANCE_SYNC,Some(&id.to_string()),None);}
        Outcome::Done
        }.await;
        crate::engine::model::Produced::new(outcome,origin)
        })}

}

/// The correct IBKR keys with no binding (keys 2.x left): each is validated at start (API_KEY_VALIDATOR, the key check
/// that binds), so its account is known before anything reads it.
pub fn unbound_ibkr_keys(c: &Connection) -> rusqlite::Result<Vec<i64>> {
    let mut s = c.prepare("SELECT k.id FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id WHERE k.status = 1 AND k.key_type = 0 AND e.type = ?1
                           AND NOT EXISTS (SELECT 1 FROM ibkr_bindings b WHERE b.api_key_id = k.id) ORDER BY k.id")?;
    let ids = s.query_map([crate::venue::ibkr::IBKR], |r| r.get(0))?.collect();
    ids
}

/// Ibkr::CheckActivationJob's name (config/recurring.yml `ibkr_check_activation_job`).
pub const IBKR_ACTIVATION: &str = "ibkr_check_activation";

/// Ibkr::CheckActivationJob, every 6 hours at :17: each IBKR key awaiting activation is checked again, and one IBKR now
/// answers for is marked correct (and bound to its account); its parked bots resume on their next tick. Nothing else is
/// written: a key still waiting stays as it is, unlogged (the waiting is the key's state, not news). A check that fails,
/// and a key waiting past ApiKey::ACTIVATION_DEADLINE (14 days), are logged. D-IBKR-ACCOUNT: a login that now holds several
/// accounts, or names another account than the bound one, is marked incorrect and logged.
pub struct IbkrActivation<C: Connect> { pub venues: C }
impl<C: Connect> Job for IbkrActivation<C> {
    fn spec(&self) -> Spec {
        Spec { name: IBKR_ACTIVATION, scope: None, schedule: Some(Schedule::EveryHours { every: 6, minute: 17 }), jitter: Jitter::NONE, retry: Retry::None, deadline: DEADLINE }
    }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            use crate::web::settings::validator::{ValidationHandle, Validity};
            let ibkr = crate::venue::ibkr::IBKR;
            let pending = cx.db.run(move |c, _| {
                let mut s = c.prepare("SELECT k.id, k.updated_at FROM api_keys k JOIN exchanges e ON e.id = k.exchange_id WHERE k.status = 3 AND e.type = ?1 ORDER BY k.id").map_err(|e| e.to_string())?;
                let rows = s.query_map([ibkr], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
                rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
            }).await;
            let pending = match pending { Ok(p) => p, Err(e) => return Outcome::Failed(e) };
            if pending.is_empty() { return Outcome::NothingNew; }
            let mut activated = false;
            for (id, updated_at) in pending {
                let log = |line: &str| crate::engine::log(&format!("[IBKR] api_key={id} {line}"));
                if crate::codec::parse_time(&updated_at).is_ok_and(|at| at <= cx.clock.now() - chrono::Duration::days(14)) {
                    log("has waited for IBKR's activation past 14 days; a registration this old will not activate: connect again");
                }
                let material = cx.db.run(move |c, cipher| super::validation_material(c, cipher, id).map_err(|e| e.0)).await;
                let (credentials, version, values) = match material { Ok(m) => m, Err(e) => { log(&format!("activation check failed: {e}")); continue } };
                let handle = ValidationHandle::stored(credentials, version.clone());
                let stored = cx.db.run(|c, cipher| crate::app_config::get(c, cipher, "proxy_ibkr").map_err(|_| "the ibkr proxy setting is unreadable".to_string())).await;
                let egress = stored.and_then(|stored| self.venues.key_check_egress(ibkr, stored));
                let (result, _) = match egress {
                    Ok((base, client)) => handle.check_via(ibkr, &base, Ok(client), 0).await,
                    Err(why) => handle.check_via(ibkr, "", Err(why), 0).await,
                };
                let status = match &result.value {
                    Validity::Account(_) | Validity::Correct => 1,
                    Validity::Incorrect | Validity::Permissions { .. } => 2,
                    Validity::PendingActivation => continue,
                    Validity::Pending(e) => { log(&format!("activation check failed: {}", crate::web::settings::keys::scrub(&e.log_text(), &values))); continue }
                };
                let account = match &result.value { Validity::Account(a) => Some(a.clone()), _ => None };
                let status = result.map(|_| status);
                let at = cx.clock.now();
                let stored = super::commit_bots_for_ciphered(&cx.db, &version, move |c, cipher| {
                    let status = crate::web::settings::keys::bound_status(c, cipher, id, status, account.as_deref(), at)?;
                    crate::web::settings::keys::store_status(c, &status, id, at)?;
                    Ok((status.value, true))
                }).await;
                match stored {
                    Ok(1) => {
                        log("activated; parked bots will resume");
                        activated = true;
                        // Its balance sync is registered now (the scope factory), not at the next start.
                        cx.wakers.wake(BALANCE_SYNC, Some(&id.to_string()), None);
                    }
                    Ok(_) => { log("is incorrect: IBKR names no single account this connection can use"); activated = true; }
                    Err(SyncError(e)) if e == crate::engine::model::CREDENTIALS_CHANGED => {}
                    Err(SyncError(e)) => return Outcome::Failed(e),
                }
            }
            if activated { cx.wakers.wake_engine(); }
            Outcome::Done
        })
    }
}

/// D-SYNC-DONE-TOAST: the keys a page's import toast still waits on, so only the tracker walk that starts after the last of
/// them landed ends it, and no other walk (a page's cold cache, a bot's order) does. Plain and Rust-only.
pub mod progress {
    use chrono::{DateTime, Utc};
    use rusqlite::Connection;
    fn key(owner: i64) -> String { format!("rust_sync_toast.{owner}") }
    fn read(c: &Connection, owner: i64) -> Result<Option<Vec<i64>>, String> {
        Ok(crate::app_config::get_plain(c, &key(owner))?.map(|s| serde_json::from_str(&s).unwrap_or_default()))
    }
    fn write(c: &Connection, owner: i64, keys: &[i64], now: DateTime<Utc>) -> Result<(), String> {
        crate::app_config::set_plain(c, &key(owner), &serde_json::to_string(keys).map_err(|e| e.to_string())?, now)
    }
    /// One read-and-write, on its own immediate transaction: the web's `started` cannot land in between.
    fn change<R>(c: &Connection, f: impl FnOnce(&Connection) -> Result<R, String>) -> Result<R, String> {
        let tx = crate::engine::model::immediate(c).map_err(|e| format!("{e:?}"))?;
        let out = f(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(out)
    }
    /// The import button (inside its own write): these keys import now. A newer import replaces an older one's wait.
    pub fn started(c: &Connection, owner: i64, keys: &[i64], now: DateTime<Utc>) -> Result<(), String> { write(c, owner, keys, now) }
    /// A key's import completed.
    pub fn landed(c: &Connection, owner: i64, key_id: i64, now: DateTime<Utc>) -> Result<(), String> {
        change(c, |c| {
            let Some(mut keys) = read(c, owner)? else { return Ok(()) };
            keys.retain(|k| *k != key_id);
            write(c, owner, &keys, now)
        })
    }
    /// The toast ended some other way (a failed import, a deadline).
    pub fn clear(c: &Connection, owner: i64) -> Result<(), String> { crate::app_config::delete(c, &key(owner)) }
    /// Whether an import is still waited on (one started after a walk took the last: that walk leaves its toast alone).
    pub fn pending(c: &Connection, owner: i64) -> Result<bool, String> { Ok(read(c, owner)?.is_some()) }
    /// A tracker walk about to read the ledger: true (and the toast is this walk's to end) once every key landed. A key
    /// deleted since the import began never lands: it is no longer waited on.
    pub fn take(c: &Connection, owner: i64, now: DateTime<Utc>) -> Result<bool, String> {
        change(c, |c| {
            let Some(keys) = read(c, owner)? else { return Ok(false) };
            let mut left = vec![];
            for id in &keys {
                let exists: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE id=?1 AND user_id=?2)", (id, owner), |r| r.get(0)).map_err(|e| e.to_string())?;
                if exists { left.push(*id); }
            }
            if left.is_empty() { clear(c, owner)?; return Ok(true) }
            if left != keys { write(c, owner, &left, now)?; }
            Ok(false)
        })
    }
}
