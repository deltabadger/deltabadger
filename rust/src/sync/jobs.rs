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

/// How a job reaches Alpaca for one key. `LiveFactory` is the real one (the key's own host, paper or live, as Rails).
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
    /// How far apart a Bitvavo ledger read's calls go (Exchanges::Bitvavo::LEDGER_READ_PAUSE); a test waits for nothing.
    fn bitvavo_pause(&self) -> Duration { crate::sync::bitvavo_ledger::READ_PAUSE }
    /// How a Gemini key's syncs reach Gemini (Honeymaker.client('gemini', proxy: ExchangeProxy.for('gemini'))). By default
    /// nothing is sent.
    fn gemini_transport(&self, _credentials: &Credentials, _proxy: Option<&str>) -> ReqwestTransport {
        ReqwestTransport::refused("Gemini is not connected here")
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
    fn mexc_transport(&self, credentials: &Credentials, proxy: Option<&str>) -> ReqwestTransport {
        let client = match proxy {
            None => crate::venue::http::client(),
            Some(proxy) => match crate::venue::http::client_via(proxy) { Ok(client) => client, Err(why) => return ReqwestTransport::refused(&why) },
        };
        ReqwestTransport::mexc(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(credentials.redaction_values.clone())
    }
}

/// The ExchangeProxy name of a venue whose syncs go through its proxy: every synced venue but Alpaca (Rails has no
/// PROXY_ALPACA).
fn proxied(venue: &str) -> Option<&'static str> {
    use crate::venue::registry::{self, BalanceSync, LedgerSync};
    registry::row(venue).filter(|r| r.balances.is_some_and(|b| b != BalanceSync::Alpaca) || r.ledger.is_some_and(|l| l != LedgerSync::Alpaca)).map(|r| r.name_id)
}

/// The venue type of a key and, for Binance, Binance.US, KuCoin, Bitvavo, Gemini and MEXC, the proxy its reads go through: AppConfig
/// `proxy_<name>` (decrypted) or ENV `PROXY_<NAME>`, as ExchangeProxy.for resolves it.
async fn venue_of(db: &Db, key: i64) -> Result<(String, Option<String>), SyncError> {
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
/// runner records the run and no success, and the next run continues. Never `Transient`: Rails' jobs declare no retry.
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

/// `ledger_sync`, scoped by the key: Rails' `AccountTransaction::SyncJob` for one key. It runs nightly at 02:00 UTC
/// (`AccountTransaction::SyncAllJob`'s fan-out, recurring key `sync_all_account_transactions_job`) and after every
/// order row the engine records (`Transaction`'s after_create_commit). No retry: Rails' job declares none, and the
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
    /// Every order, whatever its status and whichever bot placed it: an event names a bot, and which key that is
    /// takes the database. On an install with several keys each key's job runs and the others find nothing new.
    fn wants(&self, event: &EngineEvent) -> bool { matches!(event, EngineEvent::OrderRecorded { .. }) }
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
            let (credentials, version) = match key_credentials(&cx.db, self.key_id).await { Ok(c) => c, Err(SyncError(e)) => return crate::engine::model::Produced::new(Outcome::Failed(e),None) };
            let result=async {
            let key = self.key_id.to_string();
            let at = cx.clock.now();
            if let Err(e) = cx.db.run(move |c, _| crate::jobs::state::mark_incomplete(c, LEDGER_SYNC, Some(&key), at)).await { return Outcome::Failed(e); }
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
                    Some(LedgerSync::Mexc) =>
                        crate::sync::mexc_ledger::sync(&cx.db, self.venues.mexc_transport(&credentials, proxy.as_deref()), self.key_id, &credentials, cx.clock).await,
                    // Alpaca's, and a key on any other venue: ledger::load_key refuses it as it always did.
                    Some(LedgerSync::Alpaca) | None => ledger::sync_captured(&cx.db, &self.venues.connect(&credentials), self.key_id, &credentials, cx.clock, self.limits, &version).await,
                },
            };
            let outcome = outcome(done, |out| out.complete);
            // S-7.7: an import that stopped at its page cap goes on as soon as the runner is free, as Rails' one job reads the
            // whole history at once; other due jobs run in between.
            if outcome == Outcome::NothingNew { cx.wakers.wake(LEDGER_SYNC, Some(&self.key_id.to_string()), None); }
            // Rails' SyncJob ends in a Tracker::LedgerJob: the user's tracker walks again once the ledger is whole.
            else if let Err(e)=crate::tracker::jobs::wake_for_key(&cx,self.key_id).await {return Outcome::Failed(e);}
            outcome

            }.await;
            crate::engine::model::Produced::new(result,Some(version))
            }.await;
            if completion.value != Outcome::NothingNew { cx.db.notifications.sync_done(owner); }
            completion
        })
    }
}

/// `balance_sync`, scoped by the key: Rails' `AccountBalance::SyncJob` for one key. It runs nightly at 02:30 UTC
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
            let (credentials, version) = match key_credentials(&cx.db, self.key_id).await { Ok(c) => c, Err(SyncError(e)) => return crate::engine::model::Produced::new(Outcome::Failed(e),None) };
            let result=async {
            // As the ledger: from before the first write unit until a complete success, the key's balances may be half
            // written (a run dropped, or a hand run stopped, between two units); a start finding the mark runs the job at once.
            let (key, at) = (self.key_id.to_string(), cx.clock.now());
            if let Err(e) = cx.db.run(move |c, _| crate::jobs::state::mark_incomplete(c, BALANCE_SYNC, Some(&key), at)).await { return Outcome::Failed(e); }
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
                    Some(BalanceSync::Mexc) =>
                        balances::sync_mexc_captured(&cx.db, self.venues.mexc_transport(&credentials, proxy.as_deref()), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                    // Alpaca's, and a key on any other venue: the Alpaca sync's load_key refuses it as it always did.
                    Some(BalanceSync::Alpaca) | None => balances::sync_captured(&cx.db, &self.venues.connect(&credentials), self.prices.as_ref(), self.key_id, &credentials, cx.clock, &version).await,
                },
            };
            let outcome = outcome(done, |_| true);
            // Rails' AccountBalance::SyncJob ends in PortfolioSnapshot.record!, whatever the sync did: the user's tracker
            // rewrites today's rows.
            if outcome == Outcome::NothingNew { cx.wakers.wake(BALANCE_SYNC, Some(&self.key_id.to_string()), None); }
            else if let Err(e)=crate::tracker::jobs::wake_for_key(&cx,self.key_id).await {return Outcome::Failed(e);}
            outcome

            }.await;
            crate::engine::model::Produced::new(result,Some(version))
        })
    }
}

/// The jobs of an install, for the scheduler's start: per reading Alpaca, Binance, Binance.US, KuCoin, Bitvavo or MEXC key (`ApiKey.reading`,
/// what Rails' nightly jobs iterate) one ledger job and one balance job, every ledger job before every balance job, so jobs due together run in Rails' order. New scopes are registered by the same constructors on their first wake.
pub fn register<C: Connect + Clone + 'static>(c: &Connection, venues: &C, prices: Rc<dyn PriceSource>) -> Result<Vec<Box<dyn Job>>, SyncError> {
    let mut jobs: Vec<Box<dyn Job>> = vec![];
    for key in ledger_reading_keys(c)? { jobs.push(Box::new(LedgerSync::new(venues.clone(), key))); }
    for key in balance_reading_keys(c)? { jobs.push(Box::new(BalanceSync::new(venues.clone(), prices.clone(), key))); }
    jobs.push(Box::new(ScopeFactory { venues: venues.clone(), prices }));
    Ok(jobs)
}

struct ScopeFactory<C: Connect> { venues:C, prices:Rc<dyn PriceSource> }
impl<C: Connect + Clone + 'static> Job for ScopeFactory<C> {
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

/// Rails ApiKeyValidatorJob: validate a committed credential, then enqueue its trading balance sync.
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
        let result=if let Some(proxy)=checked {
            let stored=match cx.db.run(move|c,cipher|crate::app_config::get(c,cipher,&format!("proxy_{proxy}")).map_err(|_|format!("the {proxy} proxy setting is unreadable"))).await{Ok(s)=>s,Err(e)=>return before_result(Outcome::Failed(e))};
            let egress=self.venues.key_check_egress(&exchange,stored);
            match egress {
                Ok((base,client))=>handle.check_via(&exchange,&base,Ok(client),kind).await,
                Err(why)=>handle.check_via(&exchange,"",Err(why),kind).await,
            }
        } else {
            let venue=self.venues.connect(handle.credentials());
            match handle.check(&venue.urls().trading,kind).await{Ok(v)=>v,Err(_)=>return before_result(Outcome::Failed("credential validation unavailable".into()))}
        };
        use crate::web::settings::validator::Validity;
        // ApiKey#update_status!: a permission problem is incorrect; a failure leaves the key pending, its reason logged.
        let status=result.map(|result|match result{Validity::Correct=>(1,None),
            Validity::Incorrect=>{crate::engine::log(&format!("[{name}] API key validation: incorrect key"));(2,Some("incorrect key".to_string()))},
            Validity::Permissions{..}=>(2,Some("incorrect key".to_string())),
            Validity::Pending(e)=>{let safe=crate::web::settings::keys::scrub(&e.log_text(),&values);crate::engine::log(&format!("[{name}] API key validation failed: {safe}"));(0,Some(safe))}});
        let origin=status.origin().clone();
        let Some(version)=origin.clone() else{return crate::engine::model::Produced::new(Outcome::NothingNew,None)};
        let error=status.value.1.clone();
        let status=status.map(|(status,_)|status);
        let outcome=async {
        let at=cx.clock.now();
        if let Err(SyncError(e))=super::commit_bots_for(&cx.db,&version,move|c|{
            crate::web::settings::keys::store_status(c,&status,id,at)?;
            Ok(((),true))
        }).await{
            if e==crate::engine::model::CREDENTIALS_CHANGED{cx.wakers.wake(API_KEY_VALIDATOR,Some(&id.to_string()),None);return Outcome::NothingNew;}
            return Outcome::Failed(e)
        }
        cx.wakers.wake_engine();
        if let Some(error)=error {return Outcome::Failed(error)}
        if kind==0 {cx.wakers.wake(BALANCE_SYNC,Some(&id.to_string()),None);}
        Outcome::Done
        }.await;
        crate::engine::model::Produced::new(outcome,origin)
        })}

}
