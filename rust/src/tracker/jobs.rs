//! The tracker's jobs under the scheduler (`crate::jobs`), per user with a reading key of a venue this build syncs:
//! - `tracker_ledger` (Tracker::LedgerJob): the walk, today's snapshot rows (PortfolioSnapshot.record!) and the
//!   wash-sale locks. On demand only: the ledger and balance syncs wake it after every run, as Rails' sync jobs end
//!   in a LedgerJob or a record!, and so does the backfill.
//! - `portfolio_backfill` (PortfolioSnapshot::BackfillJob): every earlier day. Rails asks for it from the tracker
//!   page; here it runs at 03:00 UTC, after the nightly syncs, when the page's own test says a sweep is wanted.
//!
//! A pass is Rails' `walk(user)`: its inputs are read once (`Inputs`, one read transaction), then the walk runs on a
//! blocking thread holding no connection, and a price Rails would fetch is fetched at that lookup, inline: the walk hands
//! the request to the job's runtime thread, which fetches, stores and answers (`pass`). Nothing a pass reads can move
//! under it. `tracker_ledger` repeats the pass while the ledger's version moved, as `Tracker::Ledger.compute!` does (up
//! to three passes), and writes nothing when the rows or balances moved after the last one.
use super::backfill::{self, Plan};
use super::fx::{self, FxTable};
use super::prices::{self, Fetch, Fetcher, PriceService, Reference, Venue};
use super::rows::Stored;
use super::walk::{self, Walked};
use crate::crypto::Cipher;
use crate::engine::Clock;
use crate::figures::{budget, dec::Dec, FiguresError};
use crate::jobs::data_api::DataApi;
use crate::jobs::schedule::{Jitter, Schedule};
use crate::jobs::{Cx, Db, Job, JobFuture, Outcome, Retry, Spec, Wake, DEADLINE};
use crate::sync::jobs::Connect;
use crate::venue::http::Transport;
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub const TRACKER_LEDGER: &str = "tracker_ledger";
pub const PORTFOLIO_BACKFILL: &str = "portfolio_backfill";

/// What one job run may cost, all its passes together: the figures' own limits (`budget::FIGURE`). Steps add up over
/// the run; held limbs are a peak, freed between passes.
pub const WALK: budget::Limits = budget::FIGURE;
/// Tracker::Ledger::COMPUTE_PASSES.
pub const COMPUTE_PASSES: usize = 3;

/// What is left of one job run's budget, and how many prices it stored itself (a run that could not publish wakes
/// the next only if it stored one).
#[derive(Clone, Copy, Debug)]
pub struct Allowance { pub steps: u64, pub stored: usize }

impl Allowance {
    pub fn run() -> Allowance { Allowance { steps: WALK.steps, stored: 0 } }
}

/// Why nothing was written: the ledger moved after the last pass read it.
pub const MOVED: &str = "the ledger moved while the tracker walked; the next run states it";
/// Why the backfill wrote nothing: the ledger moved after its walk read it.
pub const SWEEP_MOVED: &str = "the ledger moved while the tracker swept; the next run sweeps it";

pub fn message(e: FiguresError) -> String {
    match e {
        FiguresError::Sqlite(e) => format!("database: {e}"),
        FiguresError::Data(m) | FiguresError::NotComputed(m) | FiguresError::Raised(m) => m,
    }
}

/// `work` on the blocking pool, in a budget scope of what the run has left; what it took is spent.
pub async fn metered<R: Send + 'static>(db: &Db, allowance: &mut Allowance,
    work: impl FnOnce(&Connection, &Cipher) -> Result<R, FiguresError> + Send + 'static) -> Result<R, String> {
    let limits = budget::Limits { steps: allowance.steps, held: WALK.held };
    let (out, used) = db.run(move |c, cipher| Ok(budget::scope(limits, || work(c, cipher)))).await?;
    allowance.steps = allowance.steps.saturating_sub(used.steps);
    out.map_err(message)
}

/// The wall clock a write is dated by, read once the write lock is held (`written`). The jobs read the system's; a
/// test or the parity harness reads its own.
pub type Wall = std::sync::Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

pub fn system_wall() -> Wall { std::sync::Arc::new(Utc::now) }

/// One write unit (`BEGIN IMMEDIATE` … `COMMIT`, rolled back on an error), dated by the wall clock read once the
/// write lock is held, after every load and walk before it, as Rails' `Date.current` is read when the row is built.
pub fn written<R>(c: &Connection, wall: &dyn Fn() -> DateTime<Utc>, write: impl FnOnce(&Connection, DateTime<Utc>) -> Result<R, FiguresError>) -> Result<R, FiguresError> {
    let tx=rusqlite::Transaction::new_unchecked(c,rusqlite::TransactionBehavior::Immediate)?;
    match write(&tx, wall()) {
        Ok(out) => { tx.commit()?; Ok(out) }
        Err(e) => { tx.rollback().map_err(|_|FiguresError::Data("tracker transaction rollback failed".into()))?; Err(e) }
    }
}

/// Everything one pass reads, read in one transaction.
pub struct Inputs {
    /// Every row (`load_all`), and the walk's (`walk_rows`).
    pub all: Vec<Stored>,
    pub rows: Vec<Stored>,
    /// Every row's base amount, for a linked withdrawal's fee.
    pub deposits: HashMap<i64, Dec>,
    pub reference: Reference,
    pub fx: FxTable,
    /// `Exchange.all.to_h { [name_id, id] }`.
    pub ids: HashMap<String, i64>,
    /// `PortfolioSnapshot.history_version` (the rows' count and newest update) and the price generation, as read.
    pub history: String,
    pub generation: i64,
    /// The ledger cache's version as the pass began.
    pub cache: super::cache::Version,
    pub origin: crate::sync::cache::ReadOrigin,
}

/// `Inputs` for the user, in one read transaction.
pub fn load_inputs(c: &Connection, user_id: i64) -> Result<Inputs, FiguresError> {
    let tx = c.unchecked_transaction()?;
    let origin = crate::sync::cache::capture_read(&tx, user_id, None).map_err(|_| FiguresError::Data("snapshot provenance unavailable".into()))?;
    let cache = super::cache::version(&tx, user_id)?;
    let history = backfill::history_version(&tx, user_id)?;
    let generation = backfill::generation(&tx)?;
    let all = super::rows::load_all(&tx, user_id)?;
    let rows = super::rows::walk_rows(&all)?;
    let deposits = all.iter().map(|r| (r.id, r.amount.clone())).collect();
    let reference = Reference::load(&tx, &Venue::alpaca(&tx)?, &prices::symbols(&all))?;
    let fx = FxTable::load(&tx)?;
    let mut ids = HashMap::new();
    {
        let mut s = tx.prepare("SELECT id, type FROM exchanges ORDER BY id")?;
        let mut q = s.query([])?;
        while let Some(r) = q.next()? {
            let (id, kind): (i64, Option<String>) = (r.get(0)?, r.get(1)?);
            if let Some(kind) = kind { ids.insert(super::rows::name_id(&kind), id); }
        }
    }
    tx.commit()?;
    Ok(Inputs { all, rows, deposits, reference, fx, ids, history, generation, cache, origin })
}

type Answer = Result<Vec<(NaiveDate, Dec)>, String>;
type Request = (Fetch, HashSet<NaiveDate>, std::sync::mpsc::SyncSender<Answer>);

/// The walk thread's end of the fetch channel: it hands a request over and blocks for the answer.
struct Channel(tokio::sync::mpsc::Sender<Request>);
impl Fetcher for Channel {
    fn range(&mut self, f: &Fetch, have: &HashSet<NaiveDate>) -> Answer {
        let (reply, answer) = std::sync::mpsc::sync_channel(1);
        self.0.blocking_send((f.clone(), have.clone(), reply)).map_err(|_| "the tracker walk lost its fetcher".to_string())?;
        answer.recv().map_err(|_| "the tracker walk lost its fetcher".to_string())?
    }
}

/// One fetch of a pass: the request, and what came back stored in its own write unit. With no data-api configured
/// the install's provider is CoinGecko, which this build does not fetch from.
async fn fetch_and_store<T: Transport>(db: &Db, api: Option<&DataApi<T>>, f: &Fetch, have: &HashSet<NaiveDate>) -> Result<(Vec<(NaiveDate, Dec)>, usize), String> {
    let Some(api) = api else { return Err(message(super::refused("historical prices from CoinGecko"))) };
    let rows = prices::fetch_range(api, f, have).await?;
    if rows.is_empty() { return Ok((vec![], 0)); }
    let mut parsed = vec![];
    for (_, day, price) in &rows { parsed.push((*day, Dec::strict(price).map_err(|e| format!("{e:?}"))?)); }
    let currency = f.currency.clone();
    let stored = db.run(move |c, _| {
        c.execute_batch("BEGIN IMMEDIATE").map_err(|e| e.to_string())?;
        let stored = prices::store_in(c, &currency, &rows).map_err(message);
        let end = if stored.is_ok() { "COMMIT" } else { "ROLLBACK" };
        c.execute_batch(end).map_err(|e| e.to_string())?;
        stored
    }).await?;
    Ok((parsed, stored))
}

/// One pass over `inputs`: `work` runs on a blocking thread with a price service over them, in a budget scope of what
/// the run has left; each fetch it asks for is made here, between the walk's steps, and stored before the walk goes on.
pub async fn pass<T: Transport, R: Send + 'static>(db: &Db, api: Option<&DataApi<T>>, inputs: Inputs, today: NaiveDate, allowance: &mut Allowance,
    work: impl FnOnce(&Inputs, &mut PriceService) -> Result<R, FiguresError> + Send + 'static) -> Result<(R, Inputs), String> {
    let limits = budget::Limits { steps: allowance.steps, held: WALK.held };
    let (requests, mut asked) = tokio::sync::mpsc::channel::<Request>(1);
    let walking = tokio::task::spawn_blocking(move || {
        let mut fetcher = Channel(requests);
        let (out, used) = budget::scope(limits, || {
            let mut ps = PriceService::new(&inputs.reference, &inputs.fx, today, &mut fetcher);
            work(&inputs, &mut ps)
        });
        drop(fetcher);
        (out, used, inputs)
    });
    while let Some((f, have, reply)) = asked.recv().await {
        let answer = match fetch_and_store(db, api, &f, &have).await {
            Ok((rows, stored)) => { allowance.stored += stored; Ok(rows) }
            Err(e) => Err(e),
        };
        let _ = reply.send(answer);
    }
    let (out, used, inputs) = walking.await.map_err(|e| format!("the tracker walk stopped: {e}"))?;
    allowance.steps = allowance.steps.saturating_sub(used.steps);
    Ok((out.map_err(message)?, inputs))
}

/// `Tracker::Ledger.compute!`: passes of `Ledger.scopes`, each loading the ECB rates (a PriceService is built per walk)
/// and its inputs afresh, until the ledger's version (rows and price generation) held still over one, at most three.
pub async fn compute<T: Transport>(db: &Db, api: Option<&DataApi<T>>, ecb: Option<&fx::Source>, user_id: i64, clock: &dyn Clock, allowance: &mut Allowance)
    -> Result<(Walked, Inputs, DateTime<Utc>), String> {
    let mut last = None;
    for _ in 0..COMPUTE_PASSES {
        let began = clock.now();
        let today = began.date_naive();
        fx::ensure_loaded(db, ecb, today).await;
        let inputs = metered(db, allowance, move |c, _| load_inputs(c, user_id)).await?;
        let now = crate::figures::at::At::from_utc(began).ok_or("an instant out of range")?;
        let (walked, inputs) = pass(db, api, inputs, today, allowance, move |i, ps| walk::scopes(&i.rows, &i.deposits, ps, &i.ids, now)).await?;
        let (history, generation) = db.run(move |c, _| Ok((backfill::history_version(c, user_id).map_err(message)?, backfill::generation(c).map_err(message)?))).await?;
        let still = history == inputs.history && generation == inputs.generation;
        last = Some((walked, inputs, began));
        if still { break; }
    }
    last.ok_or_else(|| "no pass ran".to_string())
}

/// Tracker::LedgerJob#perform: the passes, then today's snapshot rows and the wash-sale locks in one write unit dated
/// when it is written, and the ledger cache published beside them when its version still holds. Nothing is written
/// when the rows or the balances moved after the last pass read them. Returns the walk and whether the cache published.
pub async fn ledger_run<T: Transport>(db: &Db, api: Option<&DataApi<T>>, ecb: Option<&fx::Source>, user_id: i64, clock: &dyn Clock, wall: Wall, allowance: &mut Allowance) -> Result<Walked, String> {
    ledger_run_published(db, api, ecb, user_id, clock, wall, allowance).await.map(|(walked, _)| walked)
}

/// What became of the page cache in a ledger run's write unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Published {
    Yes,
    /// The ledger or the prices moved after the pass began: the cache stays as it was.
    Moved,
    /// No healthy reading slot, or a venue's ledger import is not current: no current page cache can be stated.
    NotCurrent,
}

async fn ledger_run_published<T: Transport>(db: &Db, api: Option<&DataApi<T>>, ecb: Option<&fx::Source>, user_id: i64, clock: &dyn Clock, wall: Wall, allowance: &mut Allowance) -> Result<(Walked, Published), String> {
    let (walked, inputs, began) = compute(db, api, ecb, user_id, clock, allowance).await?;
    let today = began.date_naive();
    let walked = std::sync::Arc::new(walked);
    let w = walked.clone();
    let published = metered(db, allowance, move |c, _| {
        // Everything read and computed first, so the write lock is held for the writes alone.
        let read = c.unchecked_transaction()?;
        let origin = inputs.origin.with_balance_producers(&read).map_err(|_| FiguresError::Data("snapshot balance provenance unavailable".into()))?;
        let rows = super::snapshot::today_rows(&read, user_id, &w, &inputs.fx, today)?;
        let locks = super::wash::targets(&read, user_id, &w.whole.loss_sales)?;
        read.commit()?;
        written_for(c, &origin, &*wall, |c, now| {
            if backfill::history_version(c, user_id)? != inputs.history { return Err(FiguresError::NotComputed(MOVED.into())); }
            super::snapshot::write(c, user_id, &rows, now.date_naive())?;
            // The lock rows' timestamps are the pass's clock reading, as Rails' frozen `Time.current` is in one job.
            super::wash::confirm_all(c, user_id, &locks, began)?;
            // A historical-only rebuild (no healthy reading slot) cannot publish a current page cache.
            let historical: bool = c.query_row("SELECT NOT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND status=1 AND key_type!=1)", [user_id], |r| r.get(0))?;
            if historical { return Ok(Published::NotCurrent); }
            if super::cache::version(c, user_id)? != inputs.cache { return Ok(Published::Moved); }
            Ok(if super::cache::publish(c, user_id, &inputs.cache, Some(&w), now)? { Published::Yes } else { Published::NotCurrent })
        })
    }).await?;
    let walked = std::sync::Arc::try_unwrap(walked).unwrap_or_else(|a| (*a).clone());
    Ok((walked, published))
}

/// PortfolioSnapshot::BackfillJob#perform. Returns whether a history was written (none before the first transaction's
/// day has ended). Every venue's history is swept; nothing is written when the rows moved after its walk read them.
pub async fn backfill_run<C: Connect, T: Transport>(db: &Db, venues: &C, api: Option<&DataApi<T>>, ecb: Option<&fx::Source>, user_id: i64, clock: &dyn Clock) -> Result<bool, String> {
    let now = clock.now();
    let today = now.date_naive();
    let last = today.pred_opt().unwrap_or(today);
    let mut allowance = Allowance::run();
    fx::ensure_loaded(db, ecb, today).await;
    // Read before the rows are, so a row landing mid-sweep leaves the history stale.
    let begun = metered(db, &mut allowance, move |c, cipher| {
        let version = backfill::history_version(c, user_id)?;
        let all = super::rows::load_all(c, user_id)?;
        let first = all.iter().map(|r| r.date()).min();
        match first {
            Some(first) if first <= last => Ok(Some((version, first, backfill::plan(c, user_id, &all, first, last)?))),
            _ => { crate::app_config::set(c, cipher, &backfill::history_key(user_id), &version, now).map_err(FiguresError::Data)?; Ok(None) }
        }
    }).await?;
    let Some((version, first, plan)) = begun else { return Ok(false) };
    // The price service `fetch_missing` builds.
    if !plan.instruments.is_empty() { fx::ensure_loaded(db, ecb, today).await; }
    fetch_closes(db, venues, api, &plan).await?;
    // `Tracker::Ledger.money_in`: one walk, its terms alone.
    fx::ensure_loaded(db, ecb, today).await;
    let inputs = metered(db, &mut allowance, move |c, _| load_inputs(c, user_id)).await?;
    let (terms, inputs) = pass(db, api, inputs, today, &mut allowance, |i, ps| walk::terms(&i.rows, &i.deposits, ps)).await?;
    let (plan, version) = (std::sync::Arc::new(plan), std::sync::Arc::new(version));
    metered(db, &mut allowance, move |c, cipher| {
        written_for(c, &inputs.origin, &|| now, |fenced, _| {
            // The rows swept are the rows the write sees.
            if backfill::history_version(fenced, user_id)? != inputs.history { return Err(FiguresError::NotComputed(SWEEP_MOVED.into())); }
            let swept = backfill::sweep(fenced, &plan, &inputs.all, &terms, first, last, &inputs.fx, &inputs.ids)?;
            backfill::store(fenced, cipher, user_id, &swept, last, &version, now)
        })?;
        Ok(true)
    }).await
}

/// Fetches one range unless the table covers it, and stores what came back (`fetch_price_range` for the backfill's
/// instruments).
async fn fetch<T: Transport>(db: &Db, api: Option<&DataApi<T>>, f: Fetch) -> Result<usize, String> {
    let (asset, currency, from, to) = (f.symbol.clone(), f.currency.clone(), f.from, f.to);
    let have = db.run(move |c, _| prices::stored_days(c, &asset, &currency, from, to).map_err(message)).await?;
    if prices::covered(&f, &have) { return Ok(0); }
    fetch_and_store(db, api, &f, &have).await.map(|(_, stored)| stored)
}

/// `fetch_missing` for every instrument: each coin's ranges from data-api, each stock's closes from the broker.
async fn fetch_closes<C: Connect, T: Transport>(db: &Db, venues: &C, api: Option<&DataApi<T>>, plan: &Plan) -> Result<(), String> {
    // Every bar request's key and stored closes, read once before the requests.
    let bars: Vec<backfill::Bars> = plan.fetches.iter().filter_map(|f| match f { backfill::Close::Bars(b) => Some(b.clone()), _ => None }).collect();
    let wanted = bars.clone();
    let (keys, stored) = db.run(move |c, cipher| {
        let mut keys = HashMap::new();
        for b in &wanted {
            if let std::collections::hash_map::Entry::Vacant(e) = keys.entry(b.api_key_id) { e.insert(crate::sync::credentials_with_version(c, cipher, b.api_key_id).map_err(|e| e.0)?); }
        }
        let venue = Venue::alpaca(c).map_err(message)?;
        let reference = prices::Reference::load(c, &venue, &wanted.iter().map(|b| b.symbol.clone()).collect()).map_err(message)?;
        let stored: HashMap<String, std::collections::HashSet<NaiveDate>> = wanted.iter()
            .map(|b| (b.symbol.clone(), reference.prices_over(&format!("stock:{}", b.symbol), b.from, b.to).map(|(d, _)| *d).collect())).collect();
        Ok((keys, stored))
    }).await?;
    for close in &plan.fetches {
        let bars = match close { backfill::Close::Range(f) => { fetch(db, api, f.clone()).await?; continue } backfill::Close::Bars(b) => b };
        let (Some((credentials, version)), Some(have)) = (keys.get(&bars.api_key_id), stored.get(&bars.symbol)) else { continue };
        let start = format!("{}T00:00:00Z", bars.from);
        // `stock_price_range`: any failure of the request leaves the symbol without closes.
        let raw=venues.connect(credentials);
        let handle=crate::venue::alpaca::Captured::new(&raw,version.clone());
        let Ok(body) = handle.read(true, &format!("/v2/stocks/{}/bars", bars.symbol),
                                                        vec![("limit", "10000".into()), ("start", start), ("timeframe", "1Day".into())], 8 * 1024 * 1024).await.value else { continue };
        let Ok(text) = body.text() else { continue };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else { continue };
        let Some(rows) = backfill::bar_rows(&json, bars, have) else { continue };
        if rows.is_empty() { continue; }
        let version = version.clone();
        db.run(move |c, _| {
            let tx = crate::engine::model::immediate(c).map_err(|_| "cannot begin bar cache write")?;
            let fenced=crate::engine::model::check_credential_result(&tx, &Some(version)).map_err(|e| match e { crate::engine::EngineError::CredentialsChanged => crate::engine::model::CREDENTIALS_CHANGED.to_string(), _ => "cannot check bar cache credentials".into() })?;
            prices::store_bars(&fenced, &rows).map_err(message)?;
            tx.commit().map_err(|e| e.to_string())
        }).await?;
    }
    Ok(())
}

/// `tracker_ledger`, scoped by the user.
pub struct LedgerWalk<T: Transport> { api: Rc<Option<DataApi<T>>>, ecb: Option<fx::Source>, user_id: i64, wall: Wall }

impl<T: Transport> Job for LedgerWalk<T> {
    fn spec(&self) -> Spec {
        Spec { name: TRACKER_LEDGER, scope: Some(self.user_id.to_string()), schedule: None, jitter: Jitter::NONE, retry: Retry::None, deadline: DEADLINE }
    }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            match cached_ledger_run(&cx, (*self.api).as_ref(), self.ecb.as_ref(), self.user_id, self.wall.clone()).await {
                Ok(_) => { cx.db.notifications.ledger_done(self.user_id); Outcome::Done }
                Err(e) => Outcome::Failed(e),
            }
        })
    }
}

/// Why a run's page cache did not publish: the ledger or the prices moved under it.
pub const CACHE_MOVING: &str = "Tracker ledger unavailable: inputs changed during calculation";

/// The ledger job's run: a walk that fails publishes its failure to the page cache (Rails' cache holds no entry; the
/// page states the calculation failed). A run that fails, or whose cache moved, wakes the next only if it stored a price
/// itself (passes moved by its own price writes finish then); one that stored nothing made no progress, and a writer
/// that moves the inputs wakes this job itself.
async fn cached_ledger_run<T:Transport>(cx:&Cx<'_>,api:Option<&DataApi<T>>,ecb:Option<&fx::Source>,owner:i64,wall:Wall)->Result<Walked,String> {
    let mut allowance=Allowance::run();
    let out=match ledger_run_published(&cx.db,api,ecb,owner,cx.clock,wall.clone(),&mut allowance).await {
        Ok((_,Published::Moved))=>Err(CACHE_MOVING.to_string()),
        Ok((walked,_))=>Ok(walked),
        Err(e)=>Err(e),
    };
    match &out {
        Ok(_)=>{}
        Err(e) if e==MOVED || e==CACHE_MOVING => {}
        Err(_)=>{
            let clock=wall.clone();
            cx.db.run(move|c,_|{
                let tx=c.unchecked_transaction().map_err(|e|e.to_string())?;
                let before=super::cache::version(&tx,owner).map_err(message)?;
                let origin=crate::sync::cache::capture_read(&tx,owner,None).map_err(|_|"ledger cache provenance unavailable")?;
                tx.commit().map_err(|e|e.to_string())?;
                written_for(c,&origin,&*clock,|c,now|{
                    let historical:bool=c.query_row("SELECT NOT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND status=1 AND key_type!=1)",[owner],|r|r.get(0))?;
                    if historical {return Ok(false)}
                    super::cache::publish(c,owner,&before,None,now)
                }).map_err(message)
            }).await?;
        }
    }
    if out.is_err() && allowance.stored>0 {cx.wakers.wake(TRACKER_LEDGER,Some(&owner.to_string()),None);}
    out
}

/// `portfolio_backfill`, scoped by the user.
pub struct Backfill<C: Connect, T: Transport> { venues: C, api: Rc<Option<DataApi<T>>>, user_id: i64 }

impl<C: Connect, T: Transport> Job for Backfill<C, T> {
    fn spec(&self) -> Spec {
        Spec { name: PORTFOLIO_BACKFILL, scope: Some(self.user_id.to_string()), schedule: Some(Schedule::Daily { hour: 3, minute: 0 }),
               jitter: Jitter::NONE, retry: Retry::None, deadline: DEADLINE }
    }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            let (user_id, today) = (self.user_id, cx.clock.now().date_naive());
            match cx.db.run(move |c, cipher| backfill::wanted(c, cipher, user_id, today).map_err(message)).await {
                Ok(false) => return Outcome::NothingNew,
                Ok(true) => {}
                Err(e) => return Outcome::Failed(e),
            }
            let ecb = self.venues.ecb();
            match backfill_run(&cx.db, &self.venues, (*self.api).as_ref(), ecb.as_ref(), self.user_id, cx.clock).await {
                // The prices that just arrived are the ones the ledger was missing: it walks again.
                Ok(true) => { cx.wakers.wake(TRACKER_LEDGER, Some(&self.user_id.to_string()), None); Outcome::Done }
                Ok(false) => Outcome::Done,
                Err(e) => Outcome::Failed(e),
            }
        })
    }
}

/// Construct a consumer for a user discovered after startup (including historical-only users).
pub fn resolve<C: Connect + Clone + 'static,T: Transport + 'static>(name:&str,user_id:i64,venues:&C,api:Rc<Option<DataApi<T>>>,wall:Wall)->Option<Box<dyn Job>>{
    match name{
        TRACKER_LEDGER=>Some(Box::new(LedgerWalk{api,ecb:venues.ecb(),user_id,wall})),
        PORTFOLIO_BACKFILL=>Some(Box::new(Backfill{venues:venues.clone(),api,user_id})),
        _=>None,
    }
}

/// The tracker's jobs, per user with a reading key of a venue this build syncs (Alpaca, Binance, Binance.US), every walk
/// before every backfill; today's rows dated by `wall` (`system_wall()` in the binary).
pub fn register<C: Connect + Clone + 'static, T: Transport + 'static>(c: &Connection, venues: &C, api: Rc<Option<DataApi<T>>>, wall: Wall) -> Result<Vec<Box<dyn Job>>, String> {
    let (mut users, mut seen): (Vec<i64>, std::collections::HashSet<i64>) = (vec![], std::collections::HashSet::new());
    let mut s = c.prepare("SELECT id, user_id FROM api_keys").map_err(|e| e.to_string())?;
    let owners: HashMap<i64, i64> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    for key in crate::sync::balance_reading_keys(c).map_err(|e| e.0)? {
        let Some(&user) = owners.get(&key) else { continue };
        if seen.insert(user) { users.push(user); }
    }
    let mut jobs: Vec<Box<dyn Job>> = vec![];
    for user_id in &users { jobs.push(Box::new(LedgerWalk { api: api.clone(), ecb: venues.ecb(), user_id: *user_id, wall: wall.clone() })); }
    for user_id in &users { jobs.push(Box::new(Backfill { venues: venues.clone(), api: api.clone(), user_id: *user_id })); }
    Ok(jobs)
}

/// What a sync job calls when its run ends: the tracker of the key's user walks again.
pub async fn wake_for_key(cx: &Cx<'_>, key_id: i64) -> Result<(), String> {
    let user = cx.db.run(move |c, _| c.query_row("SELECT user_id FROM api_keys WHERE id = ?1", [key_id], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())).await?;
    cx.wakers.wake(TRACKER_LEDGER, Some(&user.to_string()), None);
    Ok(())
}

/// Q/R1: check the captured producer set only after obtaining the writing lock.
pub fn written_for<R>(c:&Connection,origin:&crate::sync::cache::ReadOrigin,wall:&dyn Fn()->DateTime<Utc>,write:impl FnOnce(&crate::engine::model::FencedTransaction<'_>,DateTime<Utc>)->Result<R,FiguresError>)->Result<R,FiguresError>{
    let tx=rusqlite::Transaction::new_unchecked(c,rusqlite::TransactionBehavior::Immediate)?;
    let fenced=crate::sync::cache::fence_read(&tx,origin).map_err(|_|FiguresError::Data(crate::engine::model::CREDENTIALS_CHANGED.into()))?;
    let out=write(&fenced,wall())?;tx.commit()?;Ok(out)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::EncryptionKeys;
    use crate::engine::FixedClock;
    use crate::jobs::data_api::Config;
    use crate::venue::http::ScriptedTransport;
    use serde_json::{json, Value};

    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }
    fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
    /// data-api's answer pricing `days`.
    fn prices(days: &[(NaiveDate, f64)]) -> Value {
        json!({ "status": 200, "body": { "prices": days.iter().map(|(d, p)| json!([d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis(), p])).collect::<Vec<_>>() } })
    }
    fn data_api(answers: Vec<Value>) -> (DataApi<ScriptedTransport>, ScriptedTransport) {
        let t = ScriptedTransport::from_script(&json!({ "GET /api/v1/historical_prices": answers }));
        (DataApi::new(Config { url: "http://data-api:3000".into(), token: "t".into() }, t.clone(), t.clone()), t)
    }

    /// An Alpaca install listing AAPL and BTC, with `rows`: (entry type, symbol, amount, its price in USD, time).
    fn install(rows: &[(i64, &str, &str, Option<&str>, &str)]) -> Db {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT); INSERT INTO exchanges VALUES (1, 'Exchanges::Alpaca');
            CREATE TABLE assets (id INTEGER PRIMARY KEY, external_id TEXT, category TEXT, symbol TEXT, market_cap_rank INTEGER);
            INSERT INTO assets VALUES (1, 'AAPL.US', 'Stock', 'AAPL', NULL), (2, 'bitcoin', 'Cryptocurrency', 'BTC', 1);
            CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, base TEXT, base_asset_id INTEGER); INSERT INTO tickers VALUES (1, 1, 'AAPL', 1), (2, 1, 'BTC', 2);
            CREATE TABLE api_keys (id INTEGER PRIMARY KEY,user_id INTEGER,exchange_id INTEGER,key_type INTEGER,key TEXT,secret TEXT,passphrase TEXT,access_token TEXT,rsa_signature_key TEXT,rsa_encryption_key TEXT,dh_param TEXT);
            CREATE TABLE account_balances (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER);
            CREATE TABLE fx_rates (currency TEXT, date TEXT, rate NUMERIC, UNIQUE (currency, date));
            CREATE TABLE historical_prices (id INTEGER PRIMARY KEY, asset TEXT, currency TEXT, date TEXT, price NUMERIC, UNIQUE (asset, currency, date));
            CREATE TABLE account_transactions (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, entry_type INTEGER, base_currency TEXT, base_asset_id INTEGER, base_amount NUMERIC,
                quote_currency TEXT, quote_amount NUMERIC, fee_currency TEXT, fee_amount NUMERIC, tx_id TEXT, group_id TEXT, transacted_at TEXT, raw_data TEXT,
                manual_values TEXT, linked_transaction_id INTEGER, updated_at TEXT);").unwrap();
        for (kind, base, amount, usd, when) in rows {
            c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, transacted_at, raw_data, manual_values, updated_at) \
                       VALUES (1, 1, ?1, ?2, ?3, CASE WHEN ?4 IS NULL THEN NULL ELSE 'USD' END, ?4, ?5, '{}', '{}', ?5)", rusqlite::params![kind, base, amount, usd, when]).unwrap();
        }
        Db::new(c, Cipher::new(&EncryptionKeys::resolve(&|_| None, "tracker-jobs").unwrap()))
    }

    /// A sale before any purchase: the walk opens AAPL a second before it, at 9 September's price.
    const SOLD_FIRST: [(i64, &str, &str, Option<&str>, &str); 2] = [(1, "AAPL", "1", Some("200"), "2026-09-10 00:00:00"), (0, "AAPL", "3", Some("585"), "2026-09-11 14:30:00")];

    /// One pass: Rails' lookups, in order. The account-wide walk's opening asks first and finds nothing (a zero and a
    /// warning: no loss judged); the located walk's opening asks again and gets the price, which serves the rest of the
    /// pass. Neither result replaces the other: within the pass the global sale has no verdict.
    #[tokio::test(flavor = "current_thread")]
    async fn a_global_miss_stays_a_miss_in_its_pass_after_a_located_fetch_succeeds() {
        let (api, t) = data_api(vec![prices(&[]), prices(&[(day("2026-09-09"), 250.0)])]);
        let db = install(&SOLD_FIRST);
        let mut allowance = Allowance::run();
        let inputs = metered(&db, &mut allowance, |c, _| load_inputs(c, 1)).await.unwrap();
        let now = crate::figures::at::At::from_utc(at("2026-10-01T03:00:00Z")).unwrap();
        let (walked, _) = pass(&db, Some(&api), inputs, day("2026-10-01"), &mut allowance, move |i, ps| walk::scopes(&i.rows, &i.deposits, ps, &i.ids, now)).await.unwrap();
        assert!(walked.whole.loss_sales.is_empty(), "the global opening had no price");
        // An opening's lookup takes its warning back, and the located walk is fully priced: the figures are complete.
        assert!(!walked.whole.incomplete);
        assert_eq!(walked.venue(1).unwrap().positions[0].cost.to_s_f(), "585.0", "the located opening was priced, and sold first");
        assert_eq!(t.requests().len(), 2);
        let stored: i64 = db.run(|c, _| c.query_row("SELECT count(*) FROM historical_prices", [], |r| r.get(0)).map_err(|e| e.to_string())).await.unwrap();
        assert_eq!(stored, 1, "the fetch's price stored before the walk went on");
    }

    /// compute!: the price that pass stored moved the generation, so it walks again, and the second pass reads it from
    /// the table for both walks: the sale at $200 of a lot opened at $250 is a loss.
    #[tokio::test(flavor = "current_thread")]
    async fn a_pass_that_stored_a_price_is_walked_again_as_compute_does() {
        let (api, t) = data_api(vec![prices(&[]), prices(&[(day("2026-09-09"), 250.0)])]);
        let db = install(&SOLD_FIRST);
        let (walked, _, _) = compute(&db, Some(&api), None, 1, &FixedClock(at("2026-10-01T03:00:00Z")), &mut Allowance::run()).await.unwrap();
        assert_eq!(walked.whole.loss_sales, [("AAPL".to_string(), day("2026-09-10"))]);
        assert_eq!(t.requests().len(), 2, "the second pass asked nothing");
    }

    /// A price nobody has is a zero and a warning, never a refusal: the walk states its figures, incomplete.
    #[tokio::test(flavor = "current_thread")]
    async fn a_price_nobody_has_is_a_zero_and_the_figures_are_incomplete() {
        let (api, t) = data_api(vec![prices(&[])]);
        let db = install(&SOLD_FIRST);
        let (walked, _, _) = compute(&db, Some(&api), None, 1, &FixedClock(at("2026-10-01T03:00:00Z")), &mut Allowance::run()).await.unwrap();
        assert!(walked.whole.incomplete && walked.whole.loss_sales.is_empty());
        assert_eq!(t.requests().len(), 2, "one pass: nothing stored, nothing moved");
    }

    /// With no data-api configured the provider is CoinGecko, which this build does not fetch from: a walk that needs a
    /// price it lacks is refused with the reason, and one that needs none walks.
    #[tokio::test(flavor = "current_thread")]
    async fn an_unported_price_provider_refuses_the_walk_that_needs_it() {
        let clock = FixedClock(at("2026-10-01T03:00:00Z"));
        let out = compute(&install(&SOLD_FIRST), None::<&DataApi<ScriptedTransport>>, None, 1, &clock, &mut Allowance::run()).await.map(|_| ());
        assert_eq!(out, Err("the tracker walk is not ported for historical prices from CoinGecko".to_string()));
        let bought = [(4, "USD", "1000", None, "2026-09-01 14:00:00"), (0, "AAPL", "2", Some("400"), "2026-09-02 14:30:00")];
        assert!(compute(&install(&bought), None::<&DataApi<ScriptedTransport>>, None, 1, &clock, &mut Allowance::run()).await.is_ok());
    }

    /// The day a row is recorded on is the wall clock's once the write lock is held: a pass that begins at 23:59:59.99
    /// and whose loading and walking cross midnight records the new day.
    #[tokio::test(flavor = "current_thread")]
    async fn a_walk_that_crosses_midnight_records_the_day_of_the_write() {
        let late = at("2026-09-30T23:59:59.990Z");
        let from = std::time::Instant::now();
        let wall = move || late + chrono::Duration::from_std(from.elapsed()).unwrap();
        let db = install(&[(4, "USD", "1000", None, "2026-09-01 14:00:00"), (0, "AAPL", "2", Some("400"), "2026-09-02 14:30:00")]);
        db.run(|c, _| c.execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000)
            INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, transacted_at, raw_data, manual_values, updated_at)
            SELECT 1, 1, 0, 'AAPL', 0.001, 'USD', 0.2, '2026-09-03 14:30:00', '{}', '{}', '2026-09-03 14:30:00' FROM n").map_err(|e| e.to_string())).await.unwrap();
        let (_, _, began) = compute(&db, None::<&DataApi<ScriptedTransport>>, None, 1, &FixedClock(late), &mut Allowance::run()).await.unwrap();
        let dated = db.run(move |c, _| written(c, &wall, |_, now| Ok((began.date_naive(), now.date_naive()))).map_err(message)).await;
        assert_eq!(dated, Ok((day("2026-09-30"), day("2026-10-01"))), "the pass began before midnight; its work crossed it");
    }
}
